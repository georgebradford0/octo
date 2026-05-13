//! `octo init` — bootstrap a lair process on the local Linux host.

use std::{
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use data_encoding::BASE32_NOPAD;
use octo_core::{ensure_ssh_keypair, Config};

use crate::service;

pub struct InitOptions<'a> {
    pub noise_port: u16,
    pub http_port:  u16,
    pub mcp_config: Option<&'a Path>,
    pub extra_env:  &'a [(String, String)],
}

/// Expand `"${VAR}"` against the operator's process env.
pub fn expand_host_env(v: &str) -> std::result::Result<String, String> {
    if !(v.starts_with("${") && v.ends_with('}')) {
        return Ok(v.to_string());
    }
    let var = &v[2..v.len() - 1];
    std::env::var(var).map_err(|_| var.to_string())
}

pub async fn run(opts: InitOptions<'_>) -> Result<()> {
    let lair_dir   = service::lair_data_dir();
    let agents_dir = service::agents_dir();
    fs::create_dir_all(&lair_dir)
        .with_context(|| format!("create {}", lair_dir.display()))?;
    fs::create_dir_all(&agents_dir)
        .with_context(|| format!("create {}", agents_dir.display()))?;

    // SSH keypair for ops backchannels.
    match ensure_ssh_keypair(&lair_dir) {
        Ok((priv_path, pub_path)) => {
            println!("SSH keypair ready:");
            println!("  private: {}", priv_path.display());
            println!("  public:  {}", pub_path.display());
        }
        Err(e) => eprintln!("warning: could not ensure SSH keypair: {e:#}"),
    }

    println!("Operator config: {}.", octo_core::config_path().display());

    if let Some(path) = opts.mcp_config {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read mcp config {}", path.display()))?;
        let mut servers: Vec<serde_json::Value> = serde_json::from_str(&text)
            .with_context(|| format!("parse mcp config {}: must be a JSON array", path.display()))?;

        let mut missing: Vec<String> = Vec::new();
        for server in &mut servers {
            for key in ["env", "headers"] {
                let Some(obj) = server.get_mut(key).and_then(|e| e.as_object_mut()) else { continue };
                for (_, val) in obj.iter_mut() {
                    let Some(s) = val.as_str() else { continue };
                    match expand_host_env(s) {
                        Ok(resolved) => *val = serde_json::Value::String(resolved),
                        Err(var)     => missing.push(var),
                    }
                }
            }
        }
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            anyhow::bail!(
                "mcp config {} references env var(s) not set in this shell: {}. \
                 Export them and re-run, or inline the values in the file.",
                path.display(),
                missing.join(", "),
            );
        }

        let dest = lair_dir.join("mcp.json");
        write_secret_file(&dest, &serde_json::to_string_pretty(&servers)?)?;
        println!("Seeded MCP config: {}", dest.display());
    }

    // Ensure `<lair_dir>/noise_key.bin` (priv || pub, 64 bytes) exists.
    let key_file = lair_dir.join("noise_key.bin");
    let pubkey_b32 = ensure_noise_keypair(&key_file)?;

    // Compose the env file passed to the lair process.
    let env_path = service::env_file_path();
    fs::create_dir_all(env_path.parent().unwrap()).ok();
    let env_text = build_env_file(opts.extra_env);
    write_secret_file(&env_path, &env_text)?;
    println!("Wrote env file: {}", env_path.display());

    // Locate the lair binary, downloading the latest `lair-v*` release
    // artefact into ~/.octo/bin/ on first run.
    let binary = service::ensure_lair_binary().await?;
    println!("Using lair binary: {}", binary.display());

    let log_file = lair_dir.join("lair.log");
    let launch = service::LairLaunch {
        noise_port: opts.noise_port,
        http_port:  opts.http_port,
        data_dir:   &lair_dir,
        agents_dir: &agents_dir,
        env_file:   &env_path,
        binary:     &binary,
        log_file:   &log_file,
    };
    println!("Starting lair...");
    let pid = service::start_lair(&launch)?;
    println!("lair pid: {pid}");

    service::write_launch(&service::LaunchRecord {
        noise_port: opts.noise_port,
        http_port:  opts.http_port,
    })?;

    println!("Waiting for lair to be ready...");
    service::wait_for_health(opts.http_port, std::time::Duration::from_secs(60)).await?;

    let ip = match service::detect_public_ip().await {
        Ok(ip) => ip,
        Err(e) => {
            eprintln!("warning: could not detect public IP ({e:#}). Falling back to 127.0.0.1.");
            "127.0.0.1".to_string()
        }
    };
    let qr_data = format!("2:{ip}:{}:{pubkey_b32}", opts.noise_port);
    println!("\nlair is live at {ip}:{}\n", opts.noise_port);
    println!("QR data: {qr_data}\n");

    let code = qrcode::QrCode::new(&qr_data).context("generate QR code")?;
    let image = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Dark)
        .light_color(qrcode::render::unicode::Dense1x2::Light)
        .build();
    println!("{image}");

    Ok(())
}

fn ensure_noise_keypair(path: &Path) -> Result<String> {
    if let Ok(bytes) = fs::read(path) {
        if bytes.len() == 64 {
            println!("Reusing existing Noise keypair at {}.", path.display());
            return Ok(BASE32_NOPAD.encode(&bytes[32..]));
        }
        eprintln!(
            "warning: {} is {} bytes (expected 64) — regenerating Noise keypair.",
            path.display(),
            bytes.len(),
        );
    }
    println!("Generating Noise_XX_25519 keypair...");
    let builder = snow::Builder::new(
        "Noise_XX_25519_ChaChaPoly_SHA256".parse().context("parse noise params")?,
    );
    let kp = builder.generate_keypair().context("generate keypair")?;
    let mut combined = kp.private.clone();
    combined.extend_from_slice(&kp.public);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, &combined)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).ok();
    }
    println!("Wrote Noise keypair to {}.", path.display());
    Ok(BASE32_NOPAD.encode(&kp.public))
}

/// Operator env file (`~/.octo/lair-env`). Managed runtime knobs come from
/// `octo init` flags + `service::start_lair`; this file only carries
/// operator-supplied extras like GH_TOKEN.
pub fn build_env_file(extra_env: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in extra_env {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// Env keys octo manages itself (set by `service::start_lair`). The
/// `octo env` subcommand refuses to add or remove these.
pub const MANAGED_ENV_KEYS: &[&str] = &[
    "NOISE_PORT", "PUBLIC_PORT", "OCTO_DATA_DIR", "OCTO_AGENTS_DIR",
    "OCTO_SKIP_SHELL_ENV", "OCTO_LAIR_BINARY",
];

pub fn parse_extra_env(raw: &[String]) -> Result<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(raw.len());
    for pair in raw {
        let (k, v) = pair.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("'{pair}' must be KEY=VALUE")
        })?;
        let k = k.trim();
        if k.is_empty() {
            anyhow::bail!("'{pair}': empty KEY");
        }
        let first = k.chars().next().unwrap();
        if !(first.is_ascii_alphabetic() || first == '_') {
            anyhow::bail!("'{pair}': KEY must start with letter or underscore");
        }
        if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            anyhow::bail!("'{pair}': KEY may only contain letters, digits, and underscores");
        }
        if MANAGED_ENV_KEYS.contains(&k) {
            anyhow::bail!("'{k}': reserved name managed by octo");
        }
        out.push((k.to_string(), v.to_string()));
    }
    Ok(out)
}

pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

pub fn serialize_env_file(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in entries {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// Stop + respawn lair with the persisted launch record. Used by `octo reload`
/// and `octo env set/unset`.
pub async fn restart_lair(reason: &str) -> Result<()> {
    let rec = service::read_launch().ok_or_else(|| anyhow::anyhow!(
        "~/.octo/lair-launch.json is missing. Re-run `octo init` once to record \
         launch parameters; subsequent `{reason}` calls won't need flags."
    ))?;
    let lair_dir = service::lair_data_dir();
    let agents_dir = service::agents_dir();
    let env_path = service::env_file_path();
    let binary   = service::ensure_lair_binary().await?;
    let log_file = lair_dir.join("lair.log");
    let launch = service::LairLaunch {
        noise_port: rec.noise_port,
        http_port:  rec.http_port,
        data_dir:   &lair_dir,
        agents_dir: &agents_dir,
        env_file:   &env_path,
        binary:     &binary,
        log_file:   &log_file,
    };
    println!("Restarting lair ({reason})...");
    service::start_lair(&launch)?;
    println!("Waiting for lair to be ready...");
    service::wait_for_health(rec.http_port, std::time::Duration::from_secs(60)).await?;
    println!("lair ready.");
    Ok(())
}

pub fn write_secret_file(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).ok();
    }
    Ok(())
}

/// Hydrate `Config` from `~/.octo/config.json` or a `--config` override path.
pub fn load_config(explicit: Option<&Path>) -> Result<Config> {
    match explicit {
        Some(p) => {
            if !p.exists() {
                anyhow::bail!("config file not found: {}", p.display());
            }
            let text = fs::read_to_string(p)
                .with_context(|| format!("read {}", p.display()))?;
            serde_json::from_str::<Config>(&text)
                .with_context(|| format!("invalid JSON in {}", p.display()))
        }
        None => Ok(octo_core::read_config()),
    }
}
