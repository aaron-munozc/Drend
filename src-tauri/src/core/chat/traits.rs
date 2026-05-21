use chrono::{DateTime, Utc};
use tauri_plugin_http::reqwest::Client;
use crate::types::AppResult;

pub struct ChatProgressPayload {
    pub current_chunk: usize,
    pub total_estimated_chunks: Option<usize>,
    pub progress_percentage: f32,
}

#[async_trait::async_trait]
pub trait ChatDownloader: Send + Sync {
    /// Iterates, parses chunks, downloads logs, and executes progress updates
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        channel_slug: &str,
        start_time: DateTime<Utc>,
        duration_ms: u64,
        output_path: &std::path::Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
    ) -> AppResult<()>;
}