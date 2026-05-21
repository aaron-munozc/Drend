use tauri_plugin_http::reqwest::Client;
use crate::core::chat::traits::{ChatDownloader, ChatProgressPayload};
use crate::types::AppResult;
use std::path::Path;

pub struct TwitchChatDownloader;

#[async_trait::async_trait]
impl ChatDownloader for TwitchChatDownloader {
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        _channel_slug: &str,
        output_path: &Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
    ) -> AppResult<()> {
        // Twitch chats loop over GQL pagination chunks
        let total_chunks = 10; // Mock calculation based on video duration
        for chunk in 1..=total_chunks {
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await; // Simulating fetch

            progress_callback(ChatProgressPayload {
                current_chunk: chunk,
                total_estimated_chunks: Some(total_chunks),
                progress_percentage: (chunk as f32 / total_chunks as f32) * 100.0,
            });
        }
        // Write consolidated JSON to output_path here...
        Ok(())
    }
}