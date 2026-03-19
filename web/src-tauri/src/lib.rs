use std::{fs, path::PathBuf, sync::Mutex};
use tauri::Manager;
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandChild;

struct DaemonChild(Mutex<Option<CommandChild>>);

fn config_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".claudulhu")
        .join("desktop-config.json")
}

fn read_repo() -> Option<String> {
    let data = fs::read_to_string(config_path()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    json["repo"].as_str().map(String::from)
}

fn write_repo(repo: &str) {
    let path = config_path();
    fs::create_dir_all(path.parent().unwrap()).ok();
    fs::write(path, serde_json::json!({ "repo": repo }).to_string()).ok();
}

async fn launch_daemon(handle: tauri::AppHandle, repo: String) {
    let state = handle.state::<DaemonChild>();

    // Kill any existing daemon before starting a new one
    if let Some(child) = state.0.lock().unwrap().take() {
        let _ = child.kill();
    }

    match handle
        .shell()
        .sidecar("claudulhud")
        .expect("claudulhud sidecar not configured")
        .args(["--repo", &repo, "--host", "127.0.0.1", "--port", "8000"])
        .spawn()
    {
        Ok((mut rx, child)) => {
            log::info!("[sidecar] claudulhud started repo={repo}");
            *state.0.lock().unwrap() = Some(child);

            use tauri_plugin_shell::process::CommandEvent;
            while let Some(event) = rx.recv().await {
                match event {
                    CommandEvent::Stdout(line) => {
                        if let Ok(s) = String::from_utf8(line) {
                            log::info!("[claudulhud] {}", s.trim_end());
                        }
                    }
                    CommandEvent::Stderr(line) => {
                        if let Ok(s) = String::from_utf8(line) {
                            log::warn!("[claudulhud] {}", s.trim_end());
                        }
                    }
                    CommandEvent::Terminated(status) => {
                        log::info!("[claudulhud] exited: {:?}", status);
                        break;
                    }
                    _ => {}
                }
            }
        }
        Err(e) => log::error!("[sidecar] failed to start claudulhud: {e}"),
    }
}

// Returns the stored repo path (if any)
#[tauri::command]
fn get_repo() -> Option<String> {
    read_repo()
}

// Stores repo, (re)starts claudulhud — called from the frontend after user picks a folder
#[tauri::command]
async fn start_daemon(app: tauri::AppHandle, repo: String) -> Result<(), String> {
    write_repo(&repo);
    tauri::async_runtime::spawn(launch_daemon(app, repo));
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(DaemonChild(Mutex::new(None)))
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Auto-start daemon if a repo was previously selected
            if let Some(repo) = read_repo() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(launch_daemon(handle, repo));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_repo, start_daemon])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
