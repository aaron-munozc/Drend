use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_log::fern::colors::{Color, ColoredLevelConfig};
use crate::core::{analyze_stream_url, get_download_queue, queue_chat_download};
use crate::core::commands::download::queue_vod_download;
use crate::types::Metadata;

mod core;
mod error;
mod types;

pub struct AppCache {
    pub streams: Mutex<LruCache<String, Metadata>>,
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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_http::init())
        .plugin(log_plugin.build())
        .plugin(tauri_plugin_opener::init())
        .manage(app_cache)
        .manage(client)
        .setup(move |app| {
            let manager = core::TaskManager::new(app.handle().clone());
            app.manage(manager);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            analyze_stream_url,
            queue_chat_download,
            queue_vod_download,
            get_download_queue
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}