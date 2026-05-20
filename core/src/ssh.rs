//! Container-level SSH keypair.
//!
//! Each container that runs `lair` (or `lair --role agent`) keeps **one**
//! Ed25519 keypair at `$HOME/.ssh/id_ed25519{,.pub}` — the conventional
//! OpenSSH location, so any tool inside the container (the agentic loop's
//! `bash` shell-outs, raw `ssh user@host`, `git push`, etc.) finds it
//! without `-i` flags.
//!
//! The lair parent process generates it on startup if missing. Child agents
//! spawned in the same container do **not** generate their own keys; lair
//! seeds the agent's `$HOME/.ssh/` from the container keypair before spawn
//! (`AgentSupervisor::spawn` in `lair/src/agent_proc.rs`). This way the
//! whole container — and every process inside — speaks SSH with one
//! identity, so the operator only registers one public key per container
//! on external services (Prime Intellect, GitHub, etc.).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rand::rngs::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tracing::{debug, info};

/// Standard OpenSSH name for the container's private key.
pub const SSH_PRIVATE_KEY_FILE: &str = "id_ed25519";
/// Standard OpenSSH name for the container's matching public key.
pub const SSH_PUBLIC_KEY_FILE:  &str = "id_ed25519.pub";

/// Ensure `<home>/.ssh/id_ed25519{,.pub}` exists. Creates `.ssh/` with
/// `0o700` if missing, generates a fresh Ed25519 keypair if either file
/// is absent. Idempotent: existing keypairs are left untouched.
///
/// Returns `(private_path, public_path)`. Both paths are absolute.
pub fn ensure_container_ssh_keypair(home: &Path) -> Result<(PathBuf, PathBuf)> {
    let ssh_dir = home.join(".ssh");
    fs::create_dir_all(&ssh_dir)
        .with_context(|| format!("create ssh dir {}", ssh_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&ssh_dir)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&ssh_dir, perms)
            .with_context(|| format!("chmod 0700 {}", ssh_dir.display()))?;
    }

    ensure_keypair_at(
        &ssh_dir.join(SSH_PRIVATE_KEY_FILE),
        &ssh_dir.join(SSH_PUBLIC_KEY_FILE),
    )
}

/// Resolve `<home>/.ssh/id_ed25519` without touching the filesystem.
pub fn container_ssh_private_key(home: &Path) -> PathBuf {
    home.join(".ssh").join(SSH_PRIVATE_KEY_FILE)
}

/// Resolve `<home>/.ssh/id_ed25519.pub` without touching the filesystem.
pub fn container_ssh_public_key(home: &Path) -> PathBuf {
    home.join(".ssh").join(SSH_PUBLIC_KEY_FILE)
}

/// Ensure an Ed25519 keypair exists at the given paths. Idempotent: returns
/// early if both files already exist. The private key is written `0o600`.
pub fn ensure_keypair_at(priv_path: &Path, pub_path: &Path) -> Result<(PathBuf, PathBuf)> {
    if priv_path.exists() && pub_path.exists() {
        debug!("[ssh] reusing existing keypair at {}", priv_path.display());
        return Ok((priv_path.to_path_buf(), pub_path.to_path_buf()));
    }

    let dir = priv_path.parent().unwrap_or_else(|| Path::new("."));
    info!("[ssh] generating new Ed25519 keypair in {}", dir.display());
    fs::create_dir_all(dir)
        .with_context(|| format!("create ssh key dir {}", dir.display()))?;

    let mut rng = OsRng;
    let private_key = PrivateKey::random(&mut rng, Algorithm::Ed25519)
        .context("generate Ed25519 private key")?;
    let private_pem = private_key.to_openssh(LineEnding::LF)
        .context("encode private key as OpenSSH")?;
    let public_str  = private_key.public_key().to_openssh()
        .context("encode public key as OpenSSH")?;

    fs::write(priv_path, private_pem.as_bytes())
        .with_context(|| format!("write {}", priv_path.display()))?;
    fs::write(pub_path, format!("{public_str}\n").as_bytes())
        .with_context(|| format!("write {}", pub_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(priv_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(priv_path, perms)
            .with_context(|| format!("chmod 0600 {}", priv_path.display()))?;
    }

    info!("[ssh] wrote keypair: {} (0600) + {}", priv_path.display(), pub_path.display());
    Ok((priv_path.to_path_buf(), pub_path.to_path_buf()))
}
