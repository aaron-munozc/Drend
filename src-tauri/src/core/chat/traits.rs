use crate::types::AppResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri_plugin_http::reqwest::Client;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatProgressPayload {
    pub current_chunk: usize,
    pub total_estimated_chunks: Option<usize>,
    pub progress_percentage: f32,
}

#[async_trait::async_trait]
pub trait ChatDownloader: Send + Sync {
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        channel_slug: &str,
        start_time: DateTime<Utc>,
        duration_ms: u64,
        output_path: &std::path::Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
        cancel_flag: Arc<AtomicBool>,
    ) -> AppResult<()>;
}
