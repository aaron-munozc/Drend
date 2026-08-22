use std::fs;
use crate::core::{
    analyze_url, get_download_queue,
    queue_chat_download, queue_chat_render,
    queue_vod_download, cancel_task,
    update_queue_settings,
    get_queue_settings,
    queue_batch_chat_render,
};
use crate::types::Metadata;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_cli::CliExt;
use tauri_plugin_log::fern::colors::{Color, ColoredLevelConfig};

mod core;
mod error;
mod server;
mod tools;
mod types;

pub struct AppCache {
    pub streams: Mutex<LruCache<String, Metadata>>,
}



#[tauri::command]
fn read_directory_files(path: String) -> Result<Vec<String>, String> {
    let dir = Path::new(&path);

    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }

    if !dir.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory '{}': {}", path, e))?;

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry
            .map_err(|e| format!("Failed to read directory entry: {}", e))?;

        let entry_path = entry.path();

        // Only return files, not subdirectories.
        if entry_path.is_file() {
            files.push(entry_path.to_string_lossy().into_owned());
        }
    }

    Ok(files)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let log_plugin = {
        let builder = tauri_plugin_log::Builder::new();

        if cfg!(debug_assertions) {
            builder
                .level(log::LevelFilter::Debug)
                .targets([tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                )])
                .with_colors(ColoredLevelConfig {
                    error: Color::BrightRed,
                    warn: Color::BrightYellow,
                    info: Color::BrightGreen,
                    debug: Color::BrightBlue,
                    trace: Color::BrightBlack,
                })
        } else {
            builder
                .level(log::LevelFilter::Warn)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("logs".to_string()),
                    },
                ))
        }
    };

    let cache_capacity = NonZeroUsize::new(50).unwrap();
    let app_cache = AppCache {
        streams: Mutex::new(LruCache::new(cache_capacity)),
    };

    let client = stream_extractor::StreamClient::new().expect("Failed to build client");
    let server_controller = server::ServerController::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_cli::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_http::init())
        .plugin(log_plugin.build())
        .plugin(tauri_plugin_opener::init())
        .manage(app_cache)
        .manage(client)
        .setup(move |app| {
            let manager = core::TaskManager::new(app.handle().clone());
            let mut run_headless = false;
            let mut run_server = false;

            match app.cli().matches() {
                Ok(matches) => {
                    // Check if the server flag was passed
                    if let Some(arg) = matches.args.get("server") {
                        if arg.occurrences > 0 {
                            run_server = true;
                            let app_handle = app.handle().clone();
                            let manager_clone = manager.clone();
                            let client_clone = app
                                .state::<stream_extractor::StreamClient>()
                                .inner()
                                .clone();
                            let controller_clone = server_controller.clone();

                            tauri::async_runtime::spawn(async move {
                                let _ = controller_clone
                                    .start(app_handle, manager_clone, client_clone)
                                    .await;
                            });
                        }
                    }

                    // Check if the standalone headless flag was passed
                    if let Some(arg) = matches.args.get("background") {
                        if arg.occurrences > 0 {
                            run_headless = true;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("CLI parsing failed: {}", e);
                }
            }

            app.manage(manager);
            app.manage(server_controller.clone());

            // ==========================================
            // SYSTEM TRAY SETUP
            // ==========================================

            let show_i = MenuItem::with_id(app, "show", "Show Main Window", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit Server", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            // Grab the default icon compiled into the Tauri app
            let icon = app.default_window_icon().cloned();

            let tray_builder =
                TrayIconBuilder::new()
                    .menu(&menu)
                    .on_menu_event(|app_handle, event| match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app_handle.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            // Gracefully shut down the Axum server before killing the Tauri process
                            let app_clone = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let controller = app_clone.state::<server::ServerController>();
                                let _ = controller.stop().await; // Unbinds the port safely
                                app_clone.exit(0); // Safely drops the Tokio runtime and exits
                            });
                        }
                        _ => {}
                    });

            // Conditionally attach the icon if one was successfully loaded
            let tray_builder = if let Some(i) = icon {
                tray_builder.icon(i)
            } else {
                tray_builder
            };

            let _tray = tray_builder.build(app)?;

            // ==========================================
            // WINDOW LIFECYCLE MANAGEMENT
            // ==========================================

            if let Some(window) = app.get_webview_window("main") {
                if run_headless {
                    let _ = window.hide();
                }

                // THE CIRCUMSTANCE:
                // Only hijack the standard window closing behavior if the user intentionally
                // ran the app in a background state (--server or --headless).
                if run_server || run_headless {
                    let window_clone = window.clone();

                    window.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            // 1. Prevent the window from being destroyed
                            api.prevent_close();
                            // 2. Hide it instead, routing lifecycle management to the Tray
                            let _ = window_clone.hide();
                        }
                    });
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            read_directory_files,
            analyze_url,
            queue_chat_download,
            queue_vod_download,
            queue_chat_render,
            get_download_queue,
            cancel_task,
            update_queue_settings,
            get_queue_settings,
            queue_batch_chat_render,
            tools::check_ytdlp,
            tools::install_ytdlp,
            tools::check_ffmpeg,
            tools::install_ffmpeg,
            server::start_api_server,
            server::stop_api_server
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}