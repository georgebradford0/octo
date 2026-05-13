//! Local process management for the lair binary.
//!
//! Replaces what `dockerd.rs` did against the Docker daemon. The CLI now
//! spawns `octo-lair --role lair` as a detached background OS process and
//! tracks the pid in a pidfile. No systemd, no launchd — just `fork(2)`-style
//! double-spawn with a pidfile for `octo reload` / `octo destroy`.
//!
//! Linux only.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};

pub const LAIR_DEFAULT_HTTP_PORT:  u16 = 8000;
pub const LAIR_DEFAULT_NOISE_PORT: u16 = 8443;

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Operator's config dir. Always `$HOME/.octo`.
pub fn config_dir() -> PathBuf { home_dir().join(".octo") }

/// Lair's runtime data dir on this host. `<config_dir>/lair`.
pub fn lair_data_dir() -> PathBuf { config_dir().join("lair") }

/// Per-agent dirs root: `<config_dir>/agents`.
pub fn agents_dir() -> PathBuf { config_dir().join("agents") }

/// Pidfile lair writes when spawned by the CLI.
pub fn pidfile_path() -> PathBuf { lair_data_dir().join("lair.pid") }

/// Operator-supplied env vars passed into the lair process (one KEY=VALUE per
/// line). Persisted across reloads.
pub fn env_file_path() -> PathBuf { config_dir().join("lair-env") }

/// Bookkeeping for `octo reload` — records the ports passed to `octo init`.
pub fn launch_config_path() -> PathBuf { config_dir().join("lair-launch.json") }

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LaunchRecord {
    pub noise_port: u16,
    pub http_port:  u16,
}

pub fn write_launch(rec: &LaunchRecord) -> Result<()> {
    let path = launch_config_path();
    fs::create_dir_all(path.parent().unwrap()).ok();
    let body = serde_json::to_string_pretty(rec).context("encode lair-launch.json")?;
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn read_launch() -> Option<LaunchRecord> {
    fs::read_to_string(launch_config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// `kill(pid, 0)` style liveness probe. Linux-only.
fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 is a pure liveness probe; no signal is actually delivered.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn read_pid() -> Option<i32> {
    fs::read_to_string(pidfile_path())
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
}

fn write_pid(pid: i32) -> Result<()> {
    let p = pidfile_path();
    fs::create_dir_all(p.parent().unwrap()).ok();
    fs::write(&p, pid.to_string()).with_context(|| format!("write {}", p.display()))
}

/// True if a lair process is currently alive according to the pidfile.
pub fn is_running() -> bool {
    match read_pid() {
        Some(pid) => pid_alive(pid),
        None      => false,
    }
}

const REPO_SLUG:  &str = "georgebradford0/octo";

/// Path the CLI installs `octo-lair` to when it has to download it itself.
pub fn managed_lair_path() -> PathBuf {
    home_dir().join(".octo").join("bin").join("octo-lair")
}

/// Locate the `octo-lair` binary without trying to download. Checks
/// `$OCTO_LAIR_BINARY`, `$PATH`, the sibling `octo-lair` next to the current
/// CLI binary, then `~/.octo/bin/octo-lair`.
pub fn resolve_lair_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("OCTO_LAIR_BINARY") {
        if !p.is_empty() && Path::new(&p).exists() {
            return Ok(PathBuf::from(p));
        }
    }
    if let Ok(p) = which("octo-lair") {
        return Ok(p);
    }
    if let Ok(self_exe) = std::env::current_exe() {
        if let Some(parent) = self_exe.parent() {
            let candidate = parent.join("octo-lair");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    let home_candidate = managed_lair_path();
    if home_candidate.exists() {
        return Ok(home_candidate);
    }
    anyhow::bail!(
        "could not find 'octo-lair' binary on PATH or at {}. Set $OCTO_LAIR_BINARY to override, \
         or let `octo init` fetch it from the latest lair-v* release.",
        managed_lair_path().display(),
    );
}

/// Find the lair binary if one is already on disk; otherwise download the
/// latest `lair-v*` release artefact for this host's architecture into
/// `~/.octo/bin/octo-lair` and return that path.
pub async fn ensure_lair_binary() -> Result<PathBuf> {
    if let Ok(p) = resolve_lair_binary() {
        return Ok(p);
    }
    println!("octo-lair not found on this host — downloading latest release...");
    download_lair_binary().await
}

/// Always-download path. Pulls the latest `lair-v*` release artefact for
/// this host's architecture and writes it to `~/.octo/bin/octo-lair`,
/// overwriting whatever's there. Used by the init flow and could be wired
/// to a future `octo lair update` subcommand.
pub async fn download_lair_binary() -> Result<PathBuf> {
    let artifact = match std::env::consts::ARCH {
        "x86_64"  => "octo-lair-linux-x86_64",
        "aarch64" => "octo-lair-linux-aarch64",
        a => anyhow::bail!("unsupported architecture: {a}"),
    };

    // Find the most recent `lair-v*` release tag via the GitHub releases API.
    let client = reqwest::Client::builder()
        .user_agent(concat!("octo/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build http client")?;

    let releases: serde_json::Value = client
        .get(format!("https://api.github.com/repos/{REPO_SLUG}/releases?per_page=50"))
        .send().await
        .context("GET /releases")?
        .json().await
        .context("decode releases JSON")?;

    let tag = releases.as_array()
        .ok_or_else(|| anyhow::anyhow!("/releases didn't return an array"))?
        .iter()
        .filter_map(|r| r.get("tag_name").and_then(|t| t.as_str()))
        .find(|t| t.starts_with("lair-v"))
        .ok_or_else(|| anyhow::anyhow!(
            "no `lair-v*` release found on {REPO_SLUG}. Dispatch the `lair Release` GitHub \
             Actions workflow once to publish one, or set $OCTO_LAIR_BINARY to point at a \
             locally-built binary."
        ))?
        .to_string();

    let url = format!("https://github.com/{REPO_SLUG}/releases/download/{tag}/{artifact}");
    println!("  fetching {url}");
    let bytes = client.get(&url).send().await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("download {tag}/{artifact}"))?
        .bytes().await
        .context("read response body")?;

    let dest = managed_lair_path();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    // Write atomically (temp file + rename) so a partially-downloaded file
    // can't masquerade as the real binary on a future `octo init`.
    let tmp = dest.with_extension("download");
    fs::write(&tmp, &bytes)
        .with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms).ok();
    }
    fs::rename(&tmp, &dest)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dest.display()))?;
    println!("  installed octo-lair to {} ({tag})", dest.display());
    Ok(dest)
}

fn which(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("PATH not set"))?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("'{name}' not found on PATH")
}

/// Parse the env file into KEY=VALUE pairs that can be applied to a process.
fn read_env_file(env_path: &Path) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(env_path) else { return Vec::new(); };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

#[derive(Clone, Debug)]
pub struct LairLaunch<'a> {
    pub noise_port: u16,
    pub http_port:  u16,
    pub data_dir:   &'a Path,
    pub agents_dir: &'a Path,
    pub env_file:   &'a Path,
    pub binary:     &'a Path,
    pub log_file:   &'a Path,
}

/// Stop any running lair process, then spawn a fresh one detached from the
/// current shell. Returns the new pid. Caller is responsible for verifying
/// readiness via `wait_for_health`.
pub fn start_lair(launch: &LairLaunch<'_>) -> Result<i32> {
    stop_lair_if_running();

    fs::create_dir_all(launch.data_dir).ok();
    fs::create_dir_all(launch.agents_dir).ok();
    if let Some(parent) = launch.log_file.parent() { fs::create_dir_all(parent).ok(); }

    let log = fs::OpenOptions::new()
        .create(true).append(true).open(launch.log_file)
        .with_context(|| format!("open lair log {}", launch.log_file.display()))?;
    let log2 = log.try_clone().context("clone log fd for stderr")?;

    let mut cmd = std::process::Command::new(launch.binary);
    cmd.arg("--role").arg("lair");

    // Managed env. These take precedence and aren't overridable via the env
    // file (the CLI rejects them as reserved names earlier).
    cmd.env("OCTO_DATA_DIR",   launch.data_dir);
    cmd.env("OCTO_AGENTS_DIR", launch.agents_dir);
    cmd.env("NOISE_PORT",      launch.noise_port.to_string());
    cmd.env("PUBLIC_PORT",     launch.noise_port.to_string());
    cmd.env("OCTO_SKIP_SHELL_ENV", "1");
    cmd.env("OCTO_LAIR_BINARY", launch.binary);

    // Operator-supplied env.
    for (k, v) in read_env_file(launch.env_file) {
        cmd.env(k, v);
    }

    cmd.stdin(Stdio::null())
       .stdout(Stdio::from(log))
       .stderr(Stdio::from(log2));

    // Detach into a new process group so the parent shell exiting doesn't kill it.
    // `process_group(0)` makes the child its own session leader on Unix.
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);

    let _ = launch.http_port; // currently the lair binary hardcodes 8000

    let child = cmd.spawn()
        .with_context(|| format!("spawn lair binary {}", launch.binary.display()))?;
    let pid = child.id() as i32;
    write_pid(pid)?;
    // Don't wait on the child — we want it to keep running after the CLI exits.
    std::mem::forget(child);
    Ok(pid)
}

/// Send SIGTERM to the running lair, wait a moment, then SIGKILL if it's
/// still alive. Removes the pidfile when done. No-op if not running.
pub fn stop_lair_if_running() {
    let Some(pid) = read_pid() else { return; };
    if !pid_alive(pid) {
        let _ = fs::remove_file(pidfile_path());
        return;
    }
    // SAFETY: standard SIGTERM/SIGKILL system call.
    unsafe { libc::kill(pid, libc::SIGTERM); }
    for _ in 0..50 {
        if !pid_alive(pid) { break; }
        std::thread::sleep(Duration::from_millis(100));
    }
    if pid_alive(pid) {
        unsafe { libc::kill(pid, libc::SIGKILL); }
    }
    let _ = fs::remove_file(pidfile_path());
}

/// Wait for `http://127.0.0.1:<port>/health` to return 200, up to `timeout`.
pub async fn wait_for_health(port: u16, timeout: Duration) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/health");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("lair did not become ready within {:?}", timeout);
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

pub async fn detect_public_ip() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let resp = client.get("https://api.ipify.org").send().await
        .context("detect public IP via api.ipify.org")?;
    let body = resp.text().await.context("read ipify body")?;
    Ok(body.trim().to_string())
}

/// CLI ↔ lair management API base URL. Lair binds HTTP on
/// `0.0.0.0:LAIR_DEFAULT_HTTP_PORT`; the CLI hits 127.0.0.1.
pub fn lair_http_url() -> String {
    let port = read_launch().map(|r| r.http_port).unwrap_or(LAIR_DEFAULT_HTTP_PORT);
    format!("http://127.0.0.1:{port}")
}
