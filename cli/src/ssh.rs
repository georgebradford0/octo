//! `octo ssh …` subcommands.
//!
//! Manages the SSH certificate authority lair uses to sign child certs.
//! `ca-pubkey` prints the public CA key so the operator can authorize it on
//! remote hosts (one `TrustedUserCAKeys` line in sshd_config). `revoke` /
//! `unrevoke` / `list-revoked` manage the per-child revocation list.

use anyhow::{Context, Result};
use tracing::{debug, error, info};

use crate::service;

const TOKEN_HEADER: &str = "X-Octo-Token";

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap()
}

fn mgmt_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match service::read_mgmt_token() {
        Some(t) => builder.header(TOKEN_HEADER, t),
        None    => builder,
    }
}

/// Print the CA public key (one-line OpenSSH format). Reads from lair's data
/// dir on the host directly — works even when lair isn't running. Falls
/// back to lair's HTTP API if the file isn't present locally (e.g. running
/// the CLI on a different machine than lair).
pub async fn ca_pubkey() -> Result<()> {
    let local = service::lair_data_dir().join(octo_core::SSH_CA_PUBLIC_KEY_FILE);
    if local.exists() {
        let text = std::fs::read_to_string(&local)
            .with_context(|| format!("read {}", local.display()))?;
        print!("{}", text);
        if !text.ends_with('\n') { println!(); }
        return Ok(());
    }
    // Fall back to HTTP for non-local lair deployments.
    let url = format!("{}/ssh/ca-pubkey", service::lair_http_url());
    debug!("[ssh] GET {url}");
    let resp = http_client().get(&url).send().await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("lair returned {status}: {body}");
    }
    let body = resp.text().await.context("read response body")?;
    print!("{}", body);
    if !body.ends_with('\n') { println!(); }
    Ok(())
}

pub async fn revoke(name: &str) -> Result<()> {
    let url = format!("{}/ssh/revoke/{name}", service::lair_http_url());
    debug!("[ssh] POST {url}");
    let resp = mgmt_request(http_client().post(&url)).send().await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        error!("[ssh] revoke '{name}' failed: {status}: {body}");
        anyhow::bail!("lair returned {status}: {body}");
    }
    info!("[ssh] revoked '{name}'");
    println!("Revoked '{name}'. New cert requests will be refused; existing certs expire by TTL.");
    Ok(())
}

pub async fn unrevoke(name: &str) -> Result<()> {
    let url = format!("{}/ssh/revoke/{name}", service::lair_http_url());
    debug!("[ssh] DELETE {url}");
    let resp = mgmt_request(http_client().delete(&url)).send().await
        .with_context(|| format!("DELETE {url}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND {
        println!("'{name}' was not on the revocation list.");
        return Ok(());
    }
    if !status.is_success() {
        anyhow::bail!("lair returned {status}: {body}");
    }
    info!("[ssh] unrevoked '{name}'");
    println!("Unrevoked '{name}'. New cert requests will be honored on the next tick.");
    Ok(())
}

pub async fn list_revoked() -> Result<()> {
    let url = format!("{}/ssh/revoked", service::lair_http_url());
    debug!("[ssh] GET {url}");
    let resp = http_client().get(&url).send().await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("lair returned {status}: {body}");
    }
    let list: Vec<octo_core::RevokedEntry> = resp.json().await
        .context("parse revocation list")?;
    if list.is_empty() {
        println!("No revoked agents.");
        return Ok(());
    }
    println!("{:<28} {}", "NAME", "REVOKED_AT (unix)");
    println!("{}", "-".repeat(50));
    for e in list {
        println!("{:<28} {}", e.name, e.revoked_at);
    }
    Ok(())
}
