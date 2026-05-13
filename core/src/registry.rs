//! Persisted registry of agent (child) processes managed by lair.
//!
//! Lair spawns each child as an `octo-lair --role agent` OS process; this
//! registry records what was spawned and how to reach it. It is the source
//! of truth for which agents exist and what port each owns. Process liveness
//! (via pid) is checked by lair's poller and reconciled into `status`.
//!
//! The file lives at `<data_dir>/agents.json`. Single-process writer (the
//! lair binary), so no fs locking is needed — a `Mutex<Registry>` in
//! `AppState` serialises in-process access, and every mutation goes through
//! `save()` which atomically renames a temp file onto the target.

use std::{
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Lifecycle state of an agent process. Mirrors the strings emitted to the
/// mobile wire protocol.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Process is alive.
    Running,
    /// Process exited (clean or crashed).
    Stopped,
    /// Spawned but not yet observed alive by the poller.
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

/// One agent the lair owns.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentRecord {
    /// Stable, human-readable identifier. Doubles as the `id` mobile sees.
    pub name:           String,
    /// OS pid of the last `octo-lair --role agent` process spawned for this
    /// agent. `None` if the process has exited / was never spawned. Lair's
    /// poller flips status based on `kill(pid, 0)` liveness.
    pub pid:            Option<u32>,
    /// Local TCP port on which the child's HTTP server binds (loopback only).
    /// Lair proxies mobile traffic to this port. Allocated from 30100–30199.
    pub port:           u16,
    /// Git URL the agent was launched against, if any.
    pub git_url:        Option<String>,
    /// Last observed status. Reconciled against pid liveness on every poll.
    pub status:         AgentStatus,
    /// Lair version (`CARGO_PKG_VERSION`) at the time the row was created.
    /// Surfaced in `octo agents list` so the operator can see staleness.
    pub binary_version: String,
    /// Unix seconds when the record was created.
    pub created_at:     u64,
    /// Unix seconds the registry last observed the process alive.
    pub last_seen:      u64,
}

/// On-disk registry. Held under a `Mutex` in `AppState`.
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

    /// Insert or replace a row by name, preserving insertion order on updates.
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
        // catch up on the next structural change.
        Ok(true)
    }

    pub fn update_pid(&mut self, name: &str, pid: Option<u32>) -> Result<bool> {
        let Some(r) = self.get_mut(name) else { return Ok(false); };
        if r.pid == pid { return Ok(false); }
        r.pid = pid;
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

/// Map a Unix `kill(pid, 0)` liveness result to a status. Used by lair's
/// poller — keeps `lair.rs` a thin mapper.
pub fn status_from_alive(alive: bool) -> AgentStatus {
    if alive { AgentStatus::Running } else { AgentStatus::Stopped }
}
