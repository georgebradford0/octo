//! Outbound SSH from lair to remote-VM agents. Used by `register_remote_agent`
//! to pull the agent's published identity (`/data/agent-info.json`) after the
//! VM finishes cloud-init.
//!
//! Shells out to the system `ssh` binary so host-key handling, key auth, and
//! `known_hosts` come for free. The lair container ships `openssh-client`
//! exactly for this.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, warn};

/// Where the agent container publishes its identity on the *host* of the
/// remote VM. The userdata script bind-mounts `/var/lib/octo/agent-data`
/// onto `/data` inside the agent container, so the agent's own
/// `/data/agent-info.json` shows up here on the VM's filesystem.
pub const REMOTE_AGENT_INFO_PATH: &str = "/var/lib/octo/agent-data/agent-info.json";

/// Path inside the lair container where the SSH known_hosts file lives.
/// Stored on the bind-mounted `/data` so accept-new entries persist across
/// lair restarts.
pub fn known_hosts_path() -> PathBuf {
    octo_core::data_dir().join("known_hosts")
}

/// Parsed contents of `/data/agent-info.json` as written by `lair/src/agent.rs`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentInfo {
    pub pubkey:   String,
    pub port:     u16,
    #[serde(default)]
    pub ready_at: u64,
}

/// One SSH attempt. Returns `Ok(Some(info))` on success, `Ok(None)` if the
/// connection succeeded but the file isn't there yet (cloud-init still
/// running), or `Err(_)` for a hard failure (auth, network).
async fn try_read_once(
    host:          &str,
    ssh_user:      &str,
    key_path:      &Path,
    connect_secs:  u64,
) -> Result<Option<AgentInfo>> {
    let known_hosts = known_hosts_path();
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let target = format!("{ssh_user}@{host}");
    let connect = format!("ConnectTimeout={connect_secs}");
    let known   = format!("UserKnownHostsFile={}", known_hosts.display());
    let key     = key_path.to_string_lossy().to_string();
    let remote_cat = format!("cat {REMOTE_AGENT_INFO_PATH} 2>/dev/null");

    let output = Command::new("ssh")
        .args([
            "-i",                              key.as_str(),
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "BatchMode=yes",
            "-o", &connect,
            "-o", &known,
            target.as_str(),
            remote_cat.as_str(),
        ])
        .output()
        .await
        .context("spawn ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // ssh exit 255 typically = connection error (port closed, host
        // unreachable, auth failure). Treat as a hard error so the caller
        // surfaces it instead of looping forever.
        if let Some(code) = output.status.code() {
            if code == 255 {
                anyhow::bail!("ssh to {host} failed: {}", stderr.trim());
            }
        }
        // Other non-zero exits are typically `cat`'s "no such file" — the VM
        // is reachable but the agent hasn't published yet.
        debug!("[ssh] {host} agent-info not yet present");
        return Ok(None);
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .context("ssh stdout is not utf-8")?
        .trim();
    if stdout.is_empty() {
        return Ok(None);
    }
    let info: AgentInfo = serde_json::from_str(stdout)
        .with_context(|| format!("parse agent-info.json from {host}"))?;
    Ok(Some(info))
}

/// Poll the remote VM via SSH until it publishes `agent-info.json` or the
/// total timeout elapses. Cloud-init on a fresh VM commonly takes 1–3
/// minutes (apt-get, docker install, image pull); default `total_timeout` is
/// 5 minutes.
pub async fn await_agent_info(
    host:          &str,
    ssh_user:      &str,
    key_path:      &Path,
    total_timeout: Duration,
    poll_every:    Duration,
) -> Result<AgentInfo> {
    let deadline = tokio::time::Instant::now() + total_timeout;
    let mut last_err: Option<anyhow::Error> = None;

    while tokio::time::Instant::now() < deadline {
        match try_read_once(host, ssh_user, key_path, /*connect_secs=*/10).await {
            Ok(Some(info)) => return Ok(info),
            Ok(None) => {
                last_err = None;
                tokio::time::sleep(poll_every).await;
            }
            Err(e) => {
                // Network / auth failure. Retry — cloud-init might still be
                // bringing up sshd. But keep the last error so we have
                // something useful to return on final timeout.
                warn!("[ssh] {host}: {e:#}; retrying");
                last_err = Some(e);
                tokio::time::sleep(poll_every).await;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!(
        "timed out waiting for {host} to publish agent-info.json after {:?}",
        total_timeout
    )))
}

/// Read lair's own SSH public key from disk so it can be embedded in
/// bootstrap userdata. Returns the trimmed line (no trailing newline).
pub fn read_lair_public_key() -> Result<String> {
    let path = octo_core::data_dir().join(octo_core::SSH_PUBLIC_KEY_FILE);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read lair ssh public key at {}", path.display()))?;
    Ok(text.trim().to_string())
}

/// Common `ssh` argv prefix used by every helper here. Keeps options
/// consistent (BatchMode = no password prompts, accept-new = TOFU on the
/// first connection, ConnectTimeout = bail fast on dead networks).
fn ssh_argv(key_path: &Path, target: &str) -> Vec<String> {
    let known_hosts = known_hosts_path();
    vec![
        "-i".to_string(),  key_path.to_string_lossy().into_owned(),
        "-o".to_string(),  "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),  "BatchMode=yes".to_string(),
        "-o".to_string(),  "ConnectTimeout=10".to_string(),
        "-o".to_string(),  format!("UserKnownHostsFile={}", known_hosts.display()),
        target.to_string(),
    ]
}

/// Number of attempts each one-shot SSH op is given before giving up. With
/// exponential backoff starting at 2s, total worst-case wait per op is
/// 2 + 4 + 8 + 16 = 30s of sleep across 4 attempts — long enough to absorb
/// sshd-during-cloud-init flakiness without making the LLM wait forever.
const SSH_OP_ATTEMPTS:    u32       = 4;
const SSH_OP_INITIAL_DELAY: Duration = Duration::from_secs(2);

/// Retry an SSH op with exponential backoff. Returns the first successful
/// result, or the last error after `SSH_OP_ATTEMPTS` failures.
async fn retry_ssh_op<F, Fut, T>(label: &str, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = SSH_OP_INITIAL_DELAY;
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=SSH_OP_ATTEMPTS {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt < SSH_OP_ATTEMPTS {
                    warn!("[ssh] {label} attempt {attempt}/{SSH_OP_ATTEMPTS} failed ({e:#}); retrying in {:?}", delay);
                    last_err = Some(e);
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                } else {
                    return Err(e.context(format!("{label} failed after {SSH_OP_ATTEMPTS} attempts")));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{label}: exhausted retries")))
}

/// Write `content` to `remote_path` on `host` via `ssh ... 'umask + cat > path'`.
/// The content is piped through SSH stdin so it never lands on disk on the
/// lair side and is never visible in `ps`. `mode` is the desired file mode
/// (typically `0o600` for files with secrets). Retried with exponential
/// backoff on transient SSH errors.
pub async fn write_file(
    host:        &str,
    ssh_user:    &str,
    key_path:    &Path,
    remote_path: &str,
    content:     &str,
    mode:        u32,
) -> Result<()> {
    let label = format!("write_file {remote_path}@{host}");
    retry_ssh_op(&label, || async {
        write_file_once(host, ssh_user, key_path, remote_path, content, mode).await
    }).await
}

async fn write_file_once(
    host:        &str,
    ssh_user:    &str,
    key_path:    &Path,
    remote_path: &str,
    content:     &str,
    mode:        u32,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let target = format!("{ssh_user}@{host}");
    let remote_cmd = format!(
        "set -e; umask 0077; mkdir -p \"$(dirname {p})\"; cat > {p}; chmod {m:o} {p}",
        p = shell_escape(remote_path),
        m = mode,
    );

    let mut argv = ssh_argv(key_path, &target);
    argv.push(remote_cmd);

    let mut child = tokio::process::Command::new("ssh")
        .args(&argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn ssh write_file")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes()).await
            .context("write content to ssh stdin")?;
        stdin.shutdown().await.ok();
    }
    let output = child.wait_with_output().await.context("wait ssh write_file")?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh write_file {remote_path} on {host}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(())
}

/// Pipe `script` to `bash -s` on `host` over SSH. Standard pattern for running
/// a multi-line bash blob without ever writing it to disk on either side.
/// Returns the script's stdout on success. Retried with exponential backoff
/// on transient SSH errors — make sure the script is idempotent (we re-run
/// it from the top on each attempt).
pub async fn run_script(
    host:     &str,
    ssh_user: &str,
    key_path: &Path,
    script:   &str,
) -> Result<String> {
    let label = format!("run_script@{host}");
    retry_ssh_op(&label, || async {
        run_script_once(host, ssh_user, key_path, script).await
    }).await
}

async fn run_script_once(
    host:     &str,
    ssh_user: &str,
    key_path: &Path,
    script:   &str,
) -> Result<String> {
    use tokio::io::AsyncWriteExt;

    let target = format!("{ssh_user}@{host}");
    let mut argv = ssh_argv(key_path, &target);
    argv.push("bash -s".to_string());

    let mut child = tokio::process::Command::new("ssh")
        .args(&argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn ssh run_script")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await
            .context("write script to ssh stdin")?;
        stdin.shutdown().await.ok();
    }
    let output = child.wait_with_output().await.context("wait ssh run_script")?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh run_script on {host}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Minimal single-quote escape for embedding a value inside a `'...'` shell
/// string. We use it for paths that we want to pass to bash literally.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
