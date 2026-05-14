//! `octo mcp …` — manage the per-process `mcp.json`.
//!
//! All configs live on the host filesystem now:
//!   - lair:  `~/.octo/lair/mcp.json`
//!   - agent: `~/.octo/agents/<name>/data/mcp.json`
//!
//! Both lair and child agent processes watch their `mcp.json` and hot-reload
//! on change. Adding a new entry is a plain file edit followed by tailing the
//! agent's log for the `[mcp] '<name>' connected` marker.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::service;

const LAIR_AGENT_NAME: &str = "lair";

#[derive(Serialize, Deserialize, Clone, Debug)]
struct McpServerConfig {
    name:    String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args:    Vec<String>,
    #[serde(default)]
    env:     HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url:     Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    headers: HashMap<String, String>,
}

fn mcp_path(agent: &str) -> PathBuf {
    if agent == LAIR_AGENT_NAME {
        service::lair_data_dir().join("mcp.json")
    } else {
        service::agents_dir().join(agent).join("data").join("mcp.json")
    }
}

fn agent_log_path(agent: &str) -> PathBuf {
    if agent == LAIR_AGENT_NAME {
        service::lair_data_dir().join("lair.log")
    } else {
        service::agents_dir().join(agent).join("agent.log")
    }
}

fn read_mcp(agent: &str) -> Result<Vec<McpServerConfig>> {
    let path = mcp_path(agent);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return Ok(Vec::new()),
    };
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn write_mcp(agent: &str, configs: &[McpServerConfig]) -> Result<()> {
    let path = mcp_path(agent);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(configs)?;
    crate::init::write_secret_file(&path, &json)
}

fn read_log_tail(agent: &str, bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = agent_log_path(agent);
    let Ok(meta) = std::fs::metadata(&path) else { return String::new(); };
    let offset = meta.len().saturating_sub(bytes);
    let Ok(mut f) = std::fs::File::open(&path) else { return String::new(); };
    f.seek(SeekFrom::Start(offset)).ok();
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok();
    buf
}

pub async fn list(agent: &str) -> Result<()> {
    let configs = read_mcp(agent)?;
    if configs.is_empty() {
        println!("No MCP servers configured in '{agent}'.");
        return Ok(());
    }
    for c in &configs {
        let args = if c.args.is_empty() { String::new() } else { format!(" {}", c.args.join(" ")) };
        println!("{}: {}{}", c.name, c.command, args);
        for k in c.env.keys() {
            println!("    {k}");
        }
    }
    Ok(())
}

pub async fn add(
    agent: &str,
    name:  &str,
    command: &str,
    args: &[String],
    env_pairs: &[String],
) -> Result<()> {
    let mut configs = read_mcp(agent)?;

    if configs.iter().any(|c| c.name == name) {
        anyhow::bail!("MCP server '{name}' already exists in '{agent}'");
    }

    let mut env = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    for pair in env_pairs {
        let (k, v) = pair.split_once('=')
            .with_context(|| format!("invalid env pair '{pair}': expected KEY=VALUE"))?;
        match crate::init::expand_host_env(v) {
            Ok(resolved) => { env.insert(k.to_string(), resolved); }
            Err(var)     => missing.push(var),
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        anyhow::bail!(
            "env var(s) not visible to this process: {}. Verify with `env | grep <NAME>` — \
             variables defined in ~/.bashrc must be `export`ed to reach child processes. \
             Otherwise pass literal values.",
            missing.join(", "),
        );
    }

    configs.push(McpServerConfig {
        name:    name.to_string(),
        command: command.to_string(),
        args:    args.to_vec(),
        env,
        url:     None,
        headers: HashMap::new(),
    });

    println!("→ writing config to '{agent}'");
    write_mcp(agent, &configs)?;

    let connected_marker  = format!("[mcp] '{name}' connected");
    let spawn_fail_marker = format!("[mcp] failed to spawn '{name}'");
    let init_fail_marker  = format!("[mcp] '{name}' initialize failed");
    let no_tools_marker   = format!("[mcp] warning: server '{name}' advertised no tools");

    println!("→ waiting for MCP server to connect (up to 60s)...");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let logs = loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let logs = read_log_tail(agent, 64 * 1024);
        let done = logs.contains(&connected_marker)
            || logs.contains(&no_tools_marker)
            || logs.contains(&spawn_fail_marker)
            || logs.contains(&init_fail_marker);
        if done || tokio::time::Instant::now() >= deadline {
            break logs;
        }
    };

    for line in logs.lines() {
        if line.contains("[mcp]") && (line.contains(&format!("'{name}'")) || line.contains("hot-reload")) {
            println!("  {line}");
        }
    }

    let success = logs.contains(&connected_marker) || logs.contains(&no_tools_marker);

    if !success {
        configs.retain(|c| c.name != name);
        write_mcp(agent, &configs)?;
    }

    if logs.contains(&connected_marker) {
        println!("MCP server '{name}' connected successfully.");
    } else if logs.contains(&no_tools_marker) {
        println!("MCP server '{name}' connected but advertised no tools.");
    } else if logs.contains(&spawn_fail_marker) {
        anyhow::bail!("MCP server '{name}' failed to spawn — command not found or not executable.");
    } else if logs.contains(&init_fail_marker) {
        anyhow::bail!("MCP server '{name}' process started but MCP handshake failed.");
    } else {
        anyhow::bail!("MCP server '{name}' did not confirm connection within timeout — entry not saved. Run `octo logs {agent}` to investigate.");
    }

    Ok(())
}

pub async fn import_from_file(agent: &str, path: &std::path::Path) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let entries: Vec<McpServerConfig> = serde_json::from_str(&text)
        .context("parse JSON — expected an array of MCP server objects")?;
    if entries.is_empty() {
        println!("No entries found in '{}'.", path.display());
        return Ok(());
    }

    let mut missing: Vec<String> = Vec::new();
    let resolved: Vec<McpServerConfig> = entries.into_iter().map(|mut e| {
        let expand_map = |m: HashMap<String, String>, missing: &mut Vec<String>| -> HashMap<String, String> {
            m.into_iter().filter_map(|(k, v)| {
                match crate::init::expand_host_env(&v) {
                    Ok(resolved) => Some((k, resolved)),
                    Err(var)     => { missing.push(var); None }
                }
            }).collect()
        };
        e.env     = expand_map(e.env,     &mut missing);
        e.headers = expand_map(e.headers, &mut missing);
        e
    }).collect();

    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        anyhow::bail!(
            "env var(s) not visible to this process: {}. Verify with `env | grep <NAME>` — \
             variables defined in ~/.bashrc must be `export`ed to reach child processes. \
             Otherwise inline the values in '{}'.",
            missing.join(", "),
            path.display(),
        );
    }

    // Preflight: verify every stdio entry's command exists on this shell's
    // PATH. Unlike `octo mcp add` (which waits for the connect marker and
    // rolls back on spawn failure), `import` writes the whole file in one
    // shot — without this check, missing tools land in `mcp.json` and lair
    // fails the spawn on every subsequent hot-reload. URL-based entries
    // (`url: "..."`) are HTTP, no process spawn, so they're skipped.
    let mut missing_commands: Vec<(String, String)> = Vec::new();
    for entry in &resolved {
        if entry.url.is_some() { continue; }
        let cmd = entry.command.trim();
        if cmd.is_empty() {
            anyhow::bail!("MCP server '{}' has neither `command` nor `url`", entry.name);
        }
        if !command_on_path(cmd) {
            missing_commands.push((entry.name.clone(), cmd.to_string()));
        }
    }
    if !missing_commands.is_empty() {
        let mut msg = String::from("the following MCP server commands are not on this shell's PATH:\n");
        for (name, cmd) in &missing_commands {
            msg.push_str(&format!("  '{name}' → '{cmd}'\n"));
        }
        msg.push_str(
            "\nInstall the missing tool(s) (e.g. `curl -LsSf https://astral.sh/uv/install.sh | sh` \
             for uv/uvx) and re-run. If the tool is installed but lair's PATH differs from yours, \
             run `octo reload` to re-spawn lair from your current shell."
        );
        anyhow::bail!(msg);
    }

    println!("Importing {} MCP server(s) into '{agent}' (replacing existing config)...", resolved.len());
    write_mcp(agent, &resolved)?;
    println!("Imported successfully.");
    Ok(())
}

/// True if `name` resolves to an executable lookup on this process's `PATH`.
/// Absolute / relative paths are checked for plain existence; bare names are
/// walked across `PATH` entries.
fn command_on_path(name: &str) -> bool {
    if name.contains('/') {
        return std::path::Path::new(name).exists();
    }
    let Some(path) = std::env::var_os("PATH") else { return false; };
    std::env::split_paths(&path).any(|dir| dir.join(name).exists())
}

pub async fn remove(agent: &str, name: &str) -> Result<()> {
    let mut configs = read_mcp(agent)?;
    let before = configs.len();
    configs.retain(|c| c.name != name);
    if configs.len() == before {
        anyhow::bail!("MCP server '{name}' not found in '{agent}'");
    }
    write_mcp(agent, &configs)?;
    println!("Removed MCP server '{name}' from '{agent}'.");
    Ok(())
}
