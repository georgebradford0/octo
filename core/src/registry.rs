//! Persisted registry of agent (child) containers managed by lair.
//!
//! Replaces the old Kubernetes-backed list-of-Deployments view. The registry
//! is the source of truth for which agents exist, what port each owns, and
//! how to reach them. The Docker daemon is the source of truth for *runtime*
//! state (running/stopped); the poller reconciles the two.
//!
//! The file lives at `<data_dir>/agents.json`. Single-process writer (the
//! lair binary), so no fs locking is needed — an `RwLock<Registry>` in
//! `AppState` serialises in-process access, and every mutation goes through
//! `save()` which atomically renames a temp file onto the target.

#![allow(dead_code)] // wired up in Phase 1

use std::{
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Lifecycle state of an agent container. Mirrors the strings emitted to the
/// mobile wire protocol so the existing `containers` event schema is preserved.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Docker container is running.
    Running,
    /// Container exists in Docker but isn't running (`docker stop`).
    Stopped,
    /// Briefly between `docker create` and `docker start`.
    Pending,
}

impl AgentStatus {
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Stopped => "stopped",
            AgentStatus::Pending => "pending",
        }
    }
}

/// One agent the lair owns. The fields are a superset of what the mobile
/// `containers` event needs — extras support reconciliation and remote-VM
/// agents provisioned via a cloud-provisioning MCP.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentRecord {
    /// Stable, human-readable identifier (also the Docker container name for
    /// local agents). Used as `id` in the wire schema so mobile-side frames
    /// stay valid.
    pub name:          String,
    /// Docker container id (12+ char hex) for local agents. None for
    /// remote-VM agents.
    pub container_id:  Option<String>,
    /// External host advertised in the QR / `containers` event. None for
    /// local agents (lair fills in `state.public_host` at fan-out time);
    /// `Some(<public_ip>)` for remote agents.
    pub host:          Option<String>,
    /// Externally-reachable Noise port (30100–30199 for local; whatever the
    /// remote VM publishes for remote — typically 9000).
    pub port:          u16,
    /// Base32-encoded Noise static pubkey. Local agents share lair's keypair
    /// today; remote agents generate their own at boot and lair learns it via
    /// SSH-pull.
    pub pubkey:        String,
    /// Git URL the agent was launched against, if any.
    pub git_url:       Option<String>,
    /// Last observed status. Local agents reconcile against Docker; remote
    /// agents stay at whatever the registration set them to.
    pub status:        AgentStatus,
    /// Image tag the container was created from; recorded at create time so
    /// `octo reload` can report transitions without a child round-trip.
    pub image_version: String,
    /// Unix seconds when the record was created.
    pub created_at:    u64,
    /// Unix seconds the registry last observed this agent live.
    pub last_seen:     u64,
    /// Cloud instance id for remote-provisioned agents (e.g. `i-0abc...`).
    pub instance_id:   Option<String>,
    /// Provider name for remote agents (free-form: `"aws"`, `"hetzner"`, …).
    /// Lair doesn't interpret it; it's surfaced to the LLM so subsequent
    /// tool calls (terminate, etc.) know which MCP to invoke.
    #[serde(default)]
    pub provider:      Option<String>,
    /// Provider-specific metadata (region, instance_type, image_id, …). Opaque
    /// to lair; passed straight through from the LLM at registration time.
    #[serde(default)]
    pub metadata:      serde_json::Value,
}

impl AgentRecord {
    /// True when this agent lives on a remote VM (registered via
    /// `register_remote_agent`), false when it's a local Docker container.
    /// Lair gates Docker reconciliation on this so remote rows aren't
    /// dropped every poll cycle.
    pub fn is_remote(&self) -> bool {
        self.instance_id.is_some() || self.container_id.is_none()
    }
}

/// On-disk registry. Hold under an `RwLock` in `AppState`.
#[derive(Default)]
pub struct Registry {
    agents: Vec<AgentRecord>,
    path:   PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    agents: Vec<AgentRecord>,
}

impl Registry {
    /// Load `<dir>/agents.json` if it exists, otherwise return an empty
    /// registry bound to that path. Corrupt files are logged and treated as
    /// empty so a single bad write can't lock lair out forever.
    pub fn load(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create registry dir {}", parent.display()))?;
        }
        let agents = match fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => {
                match serde_json::from_str::<RegistryFile>(&text) {
                    Ok(f) => f.agents,
                    Err(e) => {
                        tracing::warn!("[registry] {} is corrupt ({e}); starting empty", path.display());
                        Vec::new()
                    }
                }
            }
            _ => Vec::new(),
        };
        Ok(Self { agents, path })
    }

    pub fn list(&self) -> &[AgentRecord] { &self.agents }

    pub fn get(&self, name: &str) -> Option<&AgentRecord> {
        self.agents.iter().find(|a| a.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut AgentRecord> {
        self.agents.iter_mut().find(|a| a.name == name)
    }

    /// Insert or replace a row by name, preserving insertion order on
    /// updates. Used by retryable / resumable flows (remote-agent
    /// registration) where the same name needs to transition Pending →
    /// Running across multiple tool calls.
    pub fn set(&mut self, record: AgentRecord) -> Result<()> {
        if let Some(slot) = self.agents.iter_mut().find(|a| a.name == record.name) {
            *slot = record;
        } else {
            self.agents.push(record);
        }
        self.save()
    }

    pub fn add(&mut self, record: AgentRecord) -> Result<()> {
        if self.agents.iter().any(|a| a.name == record.name) {
            anyhow::bail!("agent '{}' already exists in registry", record.name);
        }
        self.agents.push(record);
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let before = self.agents.len();
        self.agents.retain(|a| a.name != name);
        let removed = self.agents.len() != before;
        if removed { self.save()?; }
        Ok(removed)
    }

    pub fn update_status(&mut self, name: &str, status: AgentStatus) -> Result<bool> {
        let Some(r) = self.get_mut(name) else { return Ok(false); };
        if r.status == status { return Ok(false); }
        r.status = status;
        self.save()?;
        Ok(true)
    }

    pub fn update_last_seen(&mut self, name: &str, ts: u64) -> Result<bool> {
        let Some(r) = self.get_mut(name) else { return Ok(false); };
        r.last_seen = ts;
        // last_seen changes constantly; don't fsync the file every poll —
        // the in-memory copy is the live one and the persisted copy will
        // catch up on the next structural change. Callers that care about
        // durability can call `save()` directly.
        Ok(true)
    }

    pub fn update_image_version(&mut self, name: &str, version: &str) -> Result<bool> {
        let Some(r) = self.get_mut(name) else { return Ok(false); };
        if r.image_version == version { return Ok(false); }
        r.image_version = version.to_string();
        self.save()?;
        Ok(true)
    }

    pub fn update_container_id(&mut self, name: &str, container_id: Option<String>) -> Result<bool> {
        let Some(r) = self.get_mut(name) else { return Ok(false); };
        if r.container_id == container_id { return Ok(false); }
        r.container_id = container_id;
        self.save()?;
        Ok(true)
    }

    /// First port in `range` not currently used by any registered agent.
    pub fn assign_free_port(&self, range: std::ops::RangeInclusive<u16>) -> Option<u16> {
        let used: std::collections::HashSet<u16> =
            self.agents.iter().map(|a| a.port).collect();
        range.into_iter().find(|p| !used.contains(p))
    }

    /// Write the registry to disk atomically (temp file + rename on the same
    /// filesystem). Safe to call from any thread that holds a write lock.
    pub fn save(&self) -> Result<()> {
        let file = RegistryFile { agents: self.agents.clone() };
        let json = serde_json::to_string_pretty(&file)
            .context("serialise registry")?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &json)
            .with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

/// Helper to format the wire-protocol status string from the typed enum.
/// Kept here so the lair poller stays a thin mapper.
pub fn status_from_docker(state: &str) -> AgentStatus {
    match state {
        "running" => AgentStatus::Running,
        "created" | "restarting" => AgentStatus::Pending,
        _ => AgentStatus::Stopped,
    }
}
