use async_trait::async_trait;
use std::collections::HashSet;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tauri_plugin_http::reqwest::Client;
use serde_json::json;

use crate::core::chat::traits::{ChatDownloader, ChatProgressPayload};
use crate::types::AppResult;

pub struct TwitchChatDownloader;

#[async_trait]
impl ChatDownloader for TwitchChatDownloader {
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        _channel_slug: &str,
        output_path: &Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
    ) -> AppResult<()> {
        let file = File::create(output_path).await.map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);
        let mut seen_msg_ids: HashSet<String> = HashSet::new();

        let mut cursor: Option<String> = None;
        let mut chunk_count = 0;
        let mut consecutive_empty = 0;
        const EMPTY_THRESHOLD: usize = 30;

        loop {
            let variables = if let Some(ref cur) = cursor {
                json!({ "videoID": chat_id, "cursor": cur })
            } else {
                json!({ "videoID": chat_id, "contentOffsetSeconds": 0 })
            };

            let body = json!([{
                "operationName": "VideoCommentsByOffsetOrCursor",
                "variables": variables,
                "extensions": { "persistedQuery": { "version": 1, "sha256Hash": "b70a3591ff0f4e0313d126c6a1502d79a1c02baebb288227c582044aa76adf6a" } }
            }]);

            let resp = client
                .post("https://gql.twitch.tv/gql")
                .header("Client-ID", "kd1unb4b3q4t58fwlpcbzcbnm76a8fp")
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            let body_text = resp.text().await.map_err(|e| e.to_string())?;
            let val: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| e.to_string())?;

            let video = &val[0]["data"]["video"];
            if video.is_null() {
                break;
            }

            let comments = &video["comments"];
            let edges = comments["edges"].as_array();

            if let Some(edges_arr) = edges {
                if edges_arr.is_empty() {
                    consecutive_empty += 1;
                    if consecutive_empty >= EMPTY_THRESHOLD {
                        break;
                    }
                } else {
                    consecutive_empty = 0;

                    for edge in edges_arr {
                        let node = &edge["node"];
                        let message_id = edge["cursor"].as_str().unwrap_or("").to_string();

                        if message_id.is_empty() || !seen_msg_ids.insert(message_id.clone()) {
                            continue;
                        }

                        // Parse Fragments into a single string like your original code did
                        let mut content = String::new();
                        if let Some(fragments) = node["message"]["fragments"].as_array() {
                            for frag in fragments {
                                if let Some(txt) = frag["text"].as_str() {
                                    content.push_str(txt);
                                }
                            }
                        }

                        // Write out the edge node as JSONL
                        let json_line = serde_json::to_string(edge).map_err(|e| e.to_string())?;
                        writer.write_all(json_line.as_bytes()).await.map_err(|e| e.to_string())?;
                        writer.write_all(b"\n").await.map_err(|e| e.to_string())?;
                    }
                }
            }

            chunk_count += 1;
            progress_callback(ChatProgressPayload {
                current_chunk: chunk_count,
                total_estimated_chunks: None,
                progress_percentage: 0.0,
            });

            // Pagination
            let page_info = &comments["pageInfo"];
            let has_next_page = page_info["hasNextPage"].as_bool().unwrap_or(false);

            if has_next_page {
                if let Some(end_cursor) = page_info["endCursor"].as_str() {
                    cursor = Some(end_cursor.to_string());
                } else {
                    break;
                }
            } else {
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        }

        writer.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}