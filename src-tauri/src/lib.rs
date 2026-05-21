use std::sync::Arc;
use std::time::Duration;
use tauri::http::{HeaderMap, HeaderValue};
use tauri_plugin_http::reqwest::Client;
use tauri_plugin_http::reqwest::cookie::Jar;
use tauri_plugin_log::fern::colors::{Color, ColoredLevelConfig};
use crate::core::*;
use crate::types::AppState;

mod core;
mod error;
mod types;

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
    let mut headers = HeaderMap::new();

    headers.insert(
        "user-agent",
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/143.0.0.0 Safari/537.36",
        ),
    );

    headers.insert(
        "accept",
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,\
                 image/avif,image/webp,image/apng,*/*;q=0.8",
        ),
    );

    headers.insert("accept-language", HeaderValue::from_static("en;q=0.8"));

    headers.insert("upgrade-insecure-requests", HeaderValue::from_static("1"));

    headers.insert("cache-control", HeaderValue::from_static("max-age=0"));

    let jar = Arc::new(Jar::default());

    // Inside your AppClient::new implementation:
    let client = Client::builder()
        .default_headers(headers)
        .cookie_provider(jar.clone())
        .timeout(Duration::from_secs(30))         // Added from old code
        .pool_max_idle_per_host(10)               // Added from old code
        .http2_adaptive_window(true)
        .build()
        .expect("Failed to build client");

    let app_state = AppState { client };

    tauri::Builder::default()
        .plugin(tauri_plugin_http::init())
        .plugin(log_plugin.build())
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![analyze_stream_url])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
