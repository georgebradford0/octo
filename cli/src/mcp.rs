use std::collections::HashMap;

use anyhow::{Context, Result};
use claudulhu_k8s_ops::k8s;
use serde::{Deserialize, Serialize};

const MCP_PATH: &str = "/data/mcp.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
struct McpServerConfig {
    name:    String,
    command: String,
    #[serde(default)]
    args:    Vec<String>,
    #[serde(default)]
    env:     HashMap<String, String>,
}

async fn read_config(pod: &str) -> Result<Vec<McpServerConfig>> {
    let raw = k8s::exec_in_pod(pod, &["cat", MCP_PATH]).await;
    match raw {
        Ok(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).context("parse mcp.json")
        }
        _ => Ok(vec![]),
    }
}

async fn write_config(pod: &str, configs: &[McpServerConfig]) -> Result<()> {
    let json = serde_json::to_string_pretty(configs)?;
    k8s::write_pod_file(pod, MCP_PATH, &json).await
}

async fn get_pod(container: &str) -> Result<String> {
    let client = k8s::build_client().await?;
    k8s::get_running_pod(&client, container).await
}

pub async fn list(container: &str) -> Result<()> {
    let pod     = get_pod(container).await?;
    let configs = read_config(&pod).await?;
    if configs.is_empty() {
        println!("No MCP servers configured in '{container}'.");
        return Ok(());
    }
    for c in &configs {
        let args = if c.args.is_empty() {
            String::new()
        } else {
            format!(" {}", c.args.join(" "))
        };
        println!("{}: {}{}", c.name, c.command, args);
        for k in c.env.keys() {
            println!("    {k}");
        }
    }
    Ok(())
}

pub async fn add(
    container: &str,
    name: &str,
    command: &str,
    args: &[String],
    env_pairs: &[String],
) -> Result<()> {
    let pod = get_pod(container).await?;
    let mut configs = read_config(&pod).await?;

    if configs.iter().any(|c| c.name == name) {
        anyhow::bail!("MCP server '{name}' already exists in '{container}'");
    }

    let mut env = HashMap::new();
    for pair in env_pairs {
        let (k, v) = pair.split_once('=')
            .with_context(|| format!("invalid env pair '{pair}': expected KEY=VALUE"))?;
        env.insert(k.to_string(), v.to_string());
    }

    configs.push(McpServerConfig {
        name:    name.to_string(),
        command: command.to_string(),
        args:    args.to_vec(),
        env,
    });

    write_config(&pod, &configs).await?;

    // Wait for the hot-reload watcher (polls every 2s) to pick up the change
    // and attempt to connect, then check pod logs for the result.
    print!("Waiting for MCP server '{name}' to connect...");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let logs = k8s::exec_in_pod(&pod, &[
        "sh", "-c",
        &format!("kubectl logs -n {} $(hostname) --since=10s 2>/dev/null || cat /proc/1/fd/1 2>/dev/null || true", k8s::NAMESPACE),
    ]).await.unwrap_or_default();

    // Fall back to kubectl logs from the CLI side.
    let logs = if logs.trim().is_empty() {
        tokio::process::Command::new("kubectl")
            .args(["logs", "-n", k8s::NAMESPACE, &pod, "--since=10s"])
            .output().await
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    } else {
        logs
    };

    let connected_marker  = format!("[mcp] '{name}' connected");
    let spawn_fail_marker = format!("[mcp] failed to spawn '{name}'");
    let init_fail_marker  = format!("[mcp] '{name}' initialize failed");
    let no_tools_marker   = format!("[mcp] warning: server '{name}' advertised no tools");

    if logs.contains(&connected_marker) {
        println!(" connected.");
    } else if logs.contains(&spawn_fail_marker) {
        // Roll back the config entry so the user isn't left with a broken server.
        configs.retain(|c| c.name != name);
        write_config(&pod, &configs).await?;
        anyhow::bail!("MCP server '{name}' failed to spawn — command not found or not executable. Entry removed.");
    } else if logs.contains(&init_fail_marker) {
        configs.retain(|c| c.name != name);
        write_config(&pod, &configs).await?;
        anyhow::bail!("MCP server '{name}' process started but MCP handshake failed. Entry removed.");
    } else if logs.contains(&no_tools_marker) {
        println!(" connected (warning: no tools advertised).");
    } else {
        println!("\n  Could not confirm connection from logs — check with: claudulhu logs rulyeh");
    }

    Ok(())
}

pub async fn remove(container: &str, name: &str) -> Result<()> {
    let pod = get_pod(container).await?;
    let mut configs = read_config(&pod).await?;
    let before = configs.len();
    configs.retain(|c| c.name != name);
    if configs.len() == before {
        anyhow::bail!("MCP server '{name}' not found in '{container}'");
    }
    write_config(&pod, &configs).await?;
    println!("Removed MCP server '{name}' from '{container}'.");
    Ok(())
}
