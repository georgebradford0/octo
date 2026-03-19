use tauri::Manager;
use tauri_plugin_shell::ShellExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Spawn claudulhud sidecar — manages the daemon for the lifetime of the app
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match handle
                    .shell()
                    .sidecar("claudulhud")
                    .expect("claudulhud sidecar not configured")
                    .args(["--host", "127.0.0.1", "--port", "8000"])
                    .spawn()
                {
                    Ok((mut rx, child)) => {
                        log::info!("[sidecar] claudulhud started (pid={})", child.pid());

                        // Store child so it's kept alive and killed on app exit
                        handle.manage(child);

                        // Forward stdout/stderr to Tauri log
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
                                CommandEvent::Error(e) => {
                                    log::error!("[claudulhud] error: {e}");
                                }
                                CommandEvent::Terminated(status) => {
                                    log::info!("[claudulhud] exited: {:?}", status);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("[sidecar] failed to start claudulhud: {e}");
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
