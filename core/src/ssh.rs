//! SSH keypair generation for operational backchannels (e.g. SSH-ing into a
//! remote-provisioned VM to tail logs). The key lives in the lair host's data
//! directory and is created once on first boot — both `octo init` and the lair
//! binary call `ensure_ssh_keypair` so existing installs pick one up without a
//! re-init.
//!
//! Lair also maintains a **second**, signing-only Ed25519 keypair: the SSH
//! certificate authority. `ensure_ssh_ca_keypair` mints it. Children request
//! short-lived certs from lair, which signs them via `sign_user_cert` by
//! shelling out to `ssh-keygen -s`. Remote hosts that trust the CA pubkey
//! (one `TrustedUserCAKeys` line in sshd_config) accept any cert lair signs.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result};
use rand::rngs::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tracing::{debug, info, warn};

/// File name of the Ed25519 private key inside the data directory.
pub const SSH_PRIVATE_KEY_FILE: &str = "ssh_id_ed25519";
/// File name of the matching OpenSSH-format public key.
pub const SSH_PUBLIC_KEY_FILE:  &str = "ssh_id_ed25519.pub";

/// CA private key: signs short-lived user certificates for child agents.
/// Separate from `SSH_PRIVATE_KEY_FILE` so the existing operator-facing
/// identity stays unchanged when CA support is added.
pub const SSH_CA_PRIVATE_KEY_FILE: &str = "ssh_ca_ed25519";
/// CA public key: what the operator authorizes on remote hosts via
/// `TrustedUserCAKeys`.
pub const SSH_CA_PUBLIC_KEY_FILE:  &str = "ssh_ca_ed25519.pub";

/// JSON file storing the revocation list: `[{ "name": "...", "revoked_at": N }, ...]`.
pub const SSH_REVOKED_FILE: &str = "ssh_revoked.json";

/// Generate an Ed25519 SSH keypair inside `dir` if one doesn't already exist.
/// Returns `(private_path, public_path)`. Idempotent: existing keys are left
/// untouched. The private key is written `0o600` on Unix.
pub fn ensure_ssh_keypair(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    ensure_keypair_at(
        &dir.join(SSH_PRIVATE_KEY_FILE),
        &dir.join(SSH_PUBLIC_KEY_FILE),
    )
}

/// Lower-level: ensure an Ed25519 keypair exists at the given paths. Used
/// by `ensure_ssh_keypair` (lair's data dir) and by the agent role to mint
/// the child's `~/.ssh/id_ed25519{,.pub}` for SSH cert bootstrap. Idempotent.
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

/// Generate the CA Ed25519 keypair inside `dir` if it isn't already there.
/// Idempotent. Returns `(private_path, public_path)`. The private key signs
/// child certs and never leaves lair; the public key is what the operator
/// authorizes on remote hosts via `TrustedUserCAKeys`.
pub fn ensure_ssh_ca_keypair(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let priv_path = dir.join(SSH_CA_PRIVATE_KEY_FILE);
    let pub_path  = dir.join(SSH_CA_PUBLIC_KEY_FILE);

    if priv_path.exists() && pub_path.exists() {
        debug!("[ssh-ca] reusing existing CA keypair at {}", priv_path.display());
        return Ok((priv_path, pub_path));
    }

    info!("[ssh-ca] generating new Ed25519 CA keypair in {}", dir.display());
    fs::create_dir_all(dir)
        .with_context(|| format!("create ssh ca dir {}", dir.display()))?;

    let mut rng = OsRng;
    let private_key = PrivateKey::random(&mut rng, Algorithm::Ed25519)
        .context("generate Ed25519 CA private key")?;
    let private_pem = private_key.to_openssh(LineEnding::LF)
        .context("encode CA private key as OpenSSH")?;
    let public_str  = private_key.public_key().to_openssh()
        .context("encode CA public key as OpenSSH")?;

    fs::write(&priv_path, private_pem.as_bytes())
        .with_context(|| format!("write {}", priv_path.display()))?;
    fs::write(&pub_path, format!("{public_str}\n").as_bytes())
        .with_context(|| format!("write {}", pub_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&priv_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&priv_path, perms)
            .with_context(|| format!("chmod 0600 {}", priv_path.display()))?;
    }

    info!("[ssh-ca] wrote CA keypair: {} (0600) + {}", priv_path.display(), pub_path.display());
    Ok((priv_path, pub_path))
}

/// Read the CA public key (the `ssh-ed25519 …` one-liner) from `dir`.
pub fn read_ca_public_key(dir: &Path) -> Result<String> {
    let path = dir.join(SSH_CA_PUBLIC_KEY_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read CA public key at {}", path.display()))?;
    Ok(text.trim().to_string())
}

/// Sign `child_pubkey` with the CA private key at `ca_priv_path`, producing
/// an OpenSSH user certificate valid for `valid_secs` seconds. `key_id` is
/// stamped into the cert and shows up in remote sshd logs on each connection
/// — use it to identify which child the cert was issued for. `principal` is
/// the SSH principal name (typically the child's agent name); remote hosts
/// can match against `AuthorizedPrincipalsFile` to do per-principal authz.
///
/// Returns the cert text (a single line starting with `ssh-ed25519-cert-v01@openssh.com`).
///
/// Shells out to the system `ssh-keygen -s` rather than re-implementing the
/// OpenSSH cert format. `ssh-keygen` must be on PATH (it ships in
/// `openssh-client`, which is in the lair image).
pub fn sign_user_cert(
    ca_priv_path:  &Path,
    child_pubkey:  &str,
    key_id:        &str,
    principal:     &str,
    valid_secs:    u64,
) -> Result<String> {
    use std::io::Write;

    // ssh-keygen takes the pubkey as a file path. Drop the child's pubkey to
    // a temp file in a private dir, sign it, read the resulting `<file>-cert.pub`,
    // and clean up.
    let tmp_dir = tempdir_under(&std::env::temp_dir())
        .context("create tempdir for cert signing")?;
    let pub_path = tmp_dir.join("child.pub");
    {
        let mut f = fs::OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&pub_path)
            .with_context(|| format!("open {}", pub_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&pub_path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&pub_path, perms).ok();
        }
        f.write_all(child_pubkey.trim().as_bytes())
            .with_context(|| format!("write {}", pub_path.display()))?;
        f.write_all(b"\n").ok();
    }

    let validity = format!("+{valid_secs}s");
    let output = std::process::Command::new("ssh-keygen")
        .args([
            "-q",
            "-s", ca_priv_path.to_string_lossy().as_ref(),
            "-I", key_id,
            "-n", principal,
            "-V", &validity,
            pub_path.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawn ssh-keygen -s for cert signing")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = fs::remove_dir_all(&tmp_dir);
        anyhow::bail!(
            "ssh-keygen failed (exit {:?}): {}",
            output.status.code(), stderr.trim(),
        );
    }

    let cert_path = tmp_dir.join("child-cert.pub");
    let cert_text = fs::read_to_string(&cert_path)
        .with_context(|| format!("read signed cert at {}", cert_path.display()))?;
    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(cert_text.trim().to_string())
}

/// Create a temp subdir with `0700` perms. Used as scratch space for cert
/// signing so the cleartext pubkey + cert never sit in a world-readable
/// location.
fn tempdir_under(parent: &Path) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = parent.join(format!("octo-ssh-sign-{}-{}", std::process::id(), nanos));
    fs::create_dir(&path)
        .with_context(|| format!("create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&path, perms).ok();
    }
    Ok(path)
}

// ── Revocation store ──────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct RevokedEntry {
    pub name:       String,
    pub revoked_at: u64,
}

/// Load the revocation list from `dir/ssh_revoked.json`. Missing file → empty.
pub fn load_revocations(dir: &Path) -> Vec<RevokedEntry> {
    let path = dir.join(SSH_REVOKED_FILE);
    let Ok(text) = fs::read_to_string(&path) else { return Vec::new(); };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        warn!("[ssh-ca] {} is malformed, treating as empty: {e}", path.display());
        Vec::new()
    })
}

fn save_revocations(dir: &Path, list: &[RevokedEntry]) -> Result<()> {
    let path = dir.join(SSH_REVOKED_FILE);
    let json = serde_json::to_string_pretty(list)
        .context("serialize revocation list")?;
    // Atomic rename via a sibling tmp file so a partial write can't leave
    // the file truncated.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json.as_bytes())
        .with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&tmp, perms).ok();
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Add `name` to the revocation list. Idempotent — duplicate names are
/// collapsed; `revoked_at` is left at the original time.
pub fn revoke(dir: &Path, name: &str, now_secs: u64) -> Result<()> {
    let mut list = load_revocations(dir);
    if list.iter().any(|e| e.name == name) {
        debug!("[ssh-ca] '{name}' already revoked");
        return Ok(());
    }
    list.push(RevokedEntry { name: name.to_string(), revoked_at: now_secs });
    save_revocations(dir, &list)?;
    info!("[ssh-ca] revoked '{name}' at {now_secs}");
    Ok(())
}

/// Remove `name` from the revocation list. Returns `Ok(true)` if a row was
/// removed, `Ok(false)` if the name wasn't there.
pub fn unrevoke(dir: &Path, name: &str) -> Result<bool> {
    let mut list = load_revocations(dir);
    let before = list.len();
    list.retain(|e| e.name != name);
    if list.len() == before {
        return Ok(false);
    }
    save_revocations(dir, &list)?;
    info!("[ssh-ca] unrevoked '{name}'");
    Ok(true)
}

/// Cheap predicate for the cert-issuance / refresh paths.
pub fn is_revoked(dir: &Path, name: &str) -> bool {
    load_revocations(dir).iter().any(|e| e.name == name)
}
