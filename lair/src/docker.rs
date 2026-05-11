//! Thin wrapper around `bollard` for the operations lair needs against the
//! local Docker daemon. Replaces what `k8s-ops::k8s` did against the Kubernetes
//! API. Everything is single-host: lair runs on one machine, sibling agents
//! run on the same machine as named containers.
//!
//! Scope is intentionally narrow: create / start / stop / destroy / list /
//! logs. Anything fancier (networks, builds, image pulls) is done out-of-band
//! by `octo init` or the operator.

#![allow(dead_code)] // wired up in Phase 1

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use bollard::{
    container::{
        Config as ContainerConfig, CreateContainerOptions, ListContainersOptions,
        LogOutput, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        StopContainerOptions,
    },
    secret::{HostConfig, Mount, MountTypeEnum, PortBinding, RestartPolicy, RestartPolicyNameEnum},
    volume::{CreateVolumeOptions, RemoveVolumeOptions},
    Docker,
};
use futures_util::stream::StreamExt;
use serde::Serialize;

/// Label every lair-managed container/volume carries so the poller can find
/// them without consulting the registry first. Matches the historical
/// `octo.managed=1` k8s label so the convention stays familiar.
pub const MANAGED_LABEL_KEY:   &str = "octo.managed";
pub const MANAGED_LABEL_VALUE: &str = "1";

/// Image tag used for the child role. Today the same image as lair — the role
/// switch is done via the container's `command:`.
pub const DEFAULT_AGENT_IMAGE: &str = "ghcr.io/georgebradford0/lair:latest";

/// Command override that flips the merged binary into the agent role.
pub const AGENT_COMMAND: &[&str] = &["/usr/local/bin/octo-lair", "--role", "agent"];

/// Build a Docker client from local defaults. Works for:
/// - `/var/run/docker.sock` (Linux host or socket-mounted container)
/// - `DOCKER_HOST` (TCP / SSH endpoint set by the operator)
/// - Docker Desktop's user-namespaced socket
///
/// We hold this in `Arc<Docker>` on `AppState` so it can be cheaply cloned
/// into the per-tool async closures.
pub fn build_client() -> Result<Arc<Docker>> {
    let docker = Docker::connect_with_local_defaults()
        .context("connect to local Docker daemon (is it running, and is the socket reachable?)")?;
    Ok(Arc::new(docker))
}

/// Parameters for spinning up a new agent container. Mirrors the shape of
/// the old `CreateChildParams` so the lair call sites stay readable.
#[derive(Clone, Debug)]
pub struct CreateAgentParams<'a> {
    pub name:              &'a str,
    pub image:             &'a str,
    pub git_url:           Option<&'a str>,
    /// Host port that publishes the container's internal Noise port (9000).
    pub host_noise_port:   u16,
    /// External host advertised in QR codes (used as `PUBLIC_HOST` env).
    pub public_host:       &'a str,
    /// Hex-encoded 64-byte (private ++ public) Noise keypair.
    pub noise_private_key: &'a str,
    pub startup_script:    Option<&'a str>,
    pub startup_prompt:    Option<&'a str>,
    /// Inherited from lair so child agents have a working API key.
    pub anthropic_api_key: Option<&'a str>,
    pub gh_token:          Option<&'a str>,
    pub model:             Option<&'a str>,
    pub openai_api_url:    Option<&'a str>,
    pub openai_api_key:    Option<&'a str>,
    /// AGENT_PURPOSE — only meaningful when `git_url` is None.
    pub agent_purpose:     Option<&'a str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DockerContainerInfo {
    pub name:   String,
    pub id:     String,
    /// Raw Docker state string ("running", "exited", "created", ...).
    pub state:  String,
    pub image:  String,
}

fn managed_labels(name: &str) -> HashMap<String, String> {
    HashMap::from([
        (MANAGED_LABEL_KEY.to_string(), MANAGED_LABEL_VALUE.to_string()),
        ("octo.name".to_string(),       name.to_string()),
    ])
}

fn volume_name(agent_name: &str, suffix: &str) -> String {
    format!("agent-{agent_name}-{suffix}")
}

async fn ensure_volume(docker: &Docker, agent_name: &str, suffix: &str) -> Result<String> {
    let name = volume_name(agent_name, suffix);
    docker
        .create_volume(CreateVolumeOptions {
            name:   name.clone(),
            driver: "local".to_string(),
            labels: managed_labels(agent_name),
            ..Default::default()
        })
        .await
        .with_context(|| format!("ensure docker volume {name}"))?;
    Ok(name)
}

/// Create the two per-agent volumes, the container, and start it. Returns the
/// container id assigned by Docker.
pub async fn create_agent_container(docker: &Docker, p: &CreateAgentParams<'_>) -> Result<String> {
    let data_volume = ensure_volume(docker, p.name, "data").await?;
    let workspace_volume = ensure_volume(docker, p.name, "workspace").await?;

    let mut env: Vec<String> = vec![
        "NOISE_PORT=9000".to_string(),
        format!("PUBLIC_PORT={}", p.host_noise_port),
        format!("PUBLIC_HOST={}", p.public_host),
        format!("NOISE_PRIVATE_KEY={}", p.noise_private_key),
        "NOISE_KEY_FILE=/data/noise_key.bin".to_string(),
        "OCTO_DATA_DIR=/data".to_string(),
    ];
    if let Some(v) = p.git_url           { env.push(format!("GIT_URL={v}")); }
    if let Some(v) = p.startup_script    { env.push(format!("STARTUP_SCRIPT={v}")); }
    if let Some(v) = p.startup_prompt    { env.push(format!("STARTUP_PROMPT={v}")); }
    if let Some(v) = p.anthropic_api_key { env.push(format!("ANTHROPIC_API_KEY={v}")); }
    if let Some(v) = p.gh_token          { env.push(format!("GH_TOKEN={v}")); }
    if let Some(v) = p.model             { env.push(format!("MODEL={v}")); }
    if let Some(v) = p.openai_api_url    { env.push(format!("OPENAI_API_URL={v}")); }
    if let Some(v) = p.openai_api_key    { env.push(format!("OPENAI_API_KEY={v}")); }
    if let Some(v) = p.agent_purpose     { env.push(format!("AGENT_PURPOSE={v}")); }
    if std::env::var("OCTO_DEV").as_deref() == Ok("1") {
        env.push("OCTO_DEV=1".to_string());
    }

    let port_bindings = HashMap::from([(
        "9000/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip:   Some("0.0.0.0".to_string()),
            host_port: Some(p.host_noise_port.to_string()),
        }]),
    )]);

    let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
    exposed_ports.insert("9000/tcp".to_string(), HashMap::new());

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),
        mounts: Some(vec![
            Mount {
                target:     Some("/data".to_string()),
                source:     Some(data_volume),
                typ:        Some(MountTypeEnum::VOLUME),
                ..Default::default()
            },
            Mount {
                target:     Some("/workspace".to_string()),
                source:     Some(workspace_volume),
                typ:        Some(MountTypeEnum::VOLUME),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    let cmd: Vec<String> = AGENT_COMMAND.iter().map(|s| s.to_string()).collect();
    let labels = managed_labels(p.name);

    let config = ContainerConfig::<String> {
        image: Some(p.image.to_string()),
        cmd:   Some(cmd),
        env:   Some(env),
        labels: Some(labels),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        ..Default::default()
    };

    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name:     p.name.to_string(),
                platform: None,
            }),
            config,
        )
        .await
        .with_context(|| format!("docker create_container {}", p.name))?;

    docker
        .start_container(p.name, None::<StartContainerOptions<String>>)
        .await
        .with_context(|| format!("docker start_container {}", p.name))?;

    Ok(created.id)
}

/// Resume a stopped container without touching its config or volumes.
pub async fn start_container(docker: &Docker, name: &str) -> Result<()> {
    docker
        .start_container(name, None::<StartContainerOptions<String>>)
        .await
        .with_context(|| format!("docker start_container {name}"))?;
    Ok(())
}

/// Graceful stop with a small timeout — Docker SIGKILLs after `t` seconds.
pub async fn stop_container(docker: &Docker, name: &str) -> Result<()> {
    docker
        .stop_container(name, Some(StopContainerOptions { t: 10 }))
        .await
        .with_context(|| format!("docker stop_container {name}"))?;
    Ok(())
}

/// Remove a container and, if `remove_volumes`, the two named volumes it owns.
pub async fn destroy_container(docker: &Docker, name: &str, remove_volumes: bool) -> Result<()> {
    docker
        .remove_container(
            name,
            Some(RemoveContainerOptions {
                force: true,
                v:     false, // volume binding cleanup is done separately so we control it
                ..Default::default()
            }),
        )
        .await
        .with_context(|| format!("docker remove_container {name}"))?;

    if remove_volumes {
        for suffix in ["data", "workspace"] {
            let vol = volume_name(name, suffix);
            if let Err(e) = docker
                .remove_volume(&vol, None::<RemoveVolumeOptions>)
                .await
            {
                tracing::warn!("[docker] remove_volume {vol}: {e}");
            }
        }
    }
    Ok(())
}

/// List containers labelled `octo.managed=1` regardless of state.
pub async fn list_managed(docker: &Docker) -> Result<Vec<DockerContainerInfo>> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("{MANAGED_LABEL_KEY}={MANAGED_LABEL_VALUE}")],
    );

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await
        .context("docker list_containers")?;

    Ok(containers
        .into_iter()
        .filter_map(|c| {
            // Container names come back as "/foo"; strip the leading slash.
            let name = c.names
                .and_then(|ns| ns.into_iter().next())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();
            let id    = c.id.unwrap_or_default();
            let state = c.state.unwrap_or_default();
            let image = c.image.unwrap_or_default();
            if name.is_empty() || id.is_empty() { return None; }
            Some(DockerContainerInfo { name, id, state, image })
        })
        .collect())
}

/// Tail logs from a managed container. Returns a stream of plain-text lines
/// (stdout + stderr merged). `since` is a unix timestamp; 0 = beginning of
/// time. `follow = true` keeps the stream open and yields new lines live.
pub fn logs_stream<'a>(
    docker: &'a Docker,
    name:   &'a str,
    follow: bool,
    since:  i64,
) -> impl futures_util::Stream<Item = String> + 'a {
    let opts = LogsOptions::<String> {
        stdout: true,
        stderr: true,
        follow,
        since,
        tail: "all".to_string(),
        timestamps: false,
        ..Default::default()
    };
    docker.logs(name, Some(opts)).filter_map(|res| async move {
        match res {
            Ok(LogOutput::StdOut { message })
            | Ok(LogOutput::StdErr { message })
            | Ok(LogOutput::Console { message }) => {
                Some(String::from_utf8_lossy(&message).to_string())
            }
            Ok(LogOutput::StdIn { .. }) => None,
            Err(e) => {
                tracing::warn!("[docker] logs stream error: {e}");
                None
            }
        }
    })
}

