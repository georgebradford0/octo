//! `octo agents …` subcommands. The registry lives at
//! `<lair_data_dir>/agents.json` — lair owns it, the CLI reads it. Mutations
//! (start / stop / delete) go through Docker directly; lair's poller picks up
//! the result on its next 10s tick (or sooner when the action also leaves a
//! marker in Docker that the poller will reconcile against).

use std::path::PathBuf;

use anyhow::{Context, Result};
use bollard::Docker;
use octo_core::Registry;

use crate::dockerd;

fn registry_path() -> PathBuf {
    dockerd::lair_data_dir().join("agents.json")
}

pub async fn list() -> Result<()> {
    let path = registry_path();
    if !path.exists() {
        println!("No agents (lair hasn't been started yet — no registry at {}).", path.display());
        return Ok(());
    }
    let reg = Registry::load(path).context("load agent registry")?;
    let agents = reg.list();
    if agents.is_empty() {
        println!("No agents.");
        return Ok(());
    }
    println!("{:<28} {:<8} {:<6} {}", "NAME", "STATUS", "PORT", "GIT URL");
    println!("{}", "-".repeat(80));
    for a in agents {
        println!(
            "{:<28} {:<8} {:<6} {}",
            a.name,
            a.status.as_wire_str(),
            a.port,
            a.git_url.as_deref().unwrap_or(""),
        );
    }
    Ok(())
}

pub async fn start(d: &Docker, name: &str) -> Result<()> {
    dockerd::start_named(d, name).await?;
    println!("Started '{name}'. lair will pick up the new status within ~10s.");
    Ok(())
}

pub async fn stop(d: &Docker, name: &str) -> Result<()> {
    dockerd::stop_named(d, name).await?;
    println!("Stopped '{name}'. lair will pick up the new status within ~10s.");
    Ok(())
}

pub async fn delete(d: &Docker, name: &str, yes: bool) -> Result<()> {
    if !yes {
        use std::io::Write;
        print!("Delete '{name}' and both volumes? This is irreversible. [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }
    dockerd::delete_agent_full(d, name).await?;
    println!("Deleted container '{name}' and its named volumes. lair will drop the registry row on its next poll.");
    Ok(())
}
