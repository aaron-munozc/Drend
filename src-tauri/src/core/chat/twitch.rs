use crate::core::chat::types::UnifiedChatMessage;
use crate::core::chat::{ChatDownloader, ChatProgressPayload};
use crate::types::AppResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri_plugin_http::reqwest::Client;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;

// --- Cleanup Guard ---
// Ensures we don't leave corrupted/incomplete files if the user cancels
struct ChatCleanupGuard {
    path: PathBuf,
    persist: bool,
}

impl Drop for ChatCleanupGuard {
    fn drop(&mut self) {
        if !self.persist && self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
            log::info!("Cleaned up incomplete Twitch chat file: {:?}", self.path);
        }
    }
}

pub struct TwitchChatDownloader;

#[async_trait]
impl ChatDownloader for TwitchChatDownloader {
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        _channel_slug: &str,
        _start_time: DateTime<Utc>,
        duration_ms: u64,
        output_path: &Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
        cancel_flag: Arc<AtomicBool>, // <--- Added Interruption Token
    ) -> AppResult<()> {
        // 1. Register the cleanup guard early
        let mut guard = ChatCleanupGuard {
            path: output_path.to_path_buf(),
            persist: false,
        };

        let threads = 8; // Number of concurrent chunks

        // Resolve target ranges
        let actual_start_sec = start_ms.unwrap_or(0) as f64 / 1000.0;
        let actual_end_sec = end_ms.unwrap_or(duration_ms) as f64 / 1000.0;
        let target_duration = actual_end_sec - actual_start_sec;

        if target_duration <= 0.0 {
            return Ok(());
        }

        let chunk_size = target_duration / threads as f64;
        let mut handles = Vec::with_capacity(threads);

        let (prog_tx, mut prog_rx) = mpsc::channel::<f32>(32);
        let total_chunks = threads;

        let progress_task = tokio::spawn(async move {
            let mut completed_chunks = 0;
            while let Some(_chunk_progress) = prog_rx.recv().await {
                completed_chunks += 1;
                let overall_progress = (completed_chunks as f32 / total_chunks as f32) * 100.0;

                progress_callback(ChatProgressPayload {
                    current_chunk: completed_chunks,
                    total_estimated_chunks: Some(total_chunks),
                    progress_percentage: overall_progress.min(100.0),
                });
            }
        });

        // 2. Spawn parallel workers
        for i in 0..threads {
            let thread_client = client.clone();
            let thread_chat_id = chat_id.to_string();
            let thread_tx = prog_tx.clone();
            let worker_cancel = Arc::clone(&cancel_flag);

            let chunk_start = actual_start_sec + (chunk_size * i as f64);
            let chunk_end = if i == threads - 1 {
                actual_end_sec
            } else {
                chunk_start + chunk_size
            };

            handles.push(tokio::spawn(async move {
                download_chunk_worker(
                    thread_client,
                    thread_chat_id,
                    chunk_start,
                    chunk_end,
                    worker_cancel, // Pass token into the worker
                )
                .await
                .map(|edges| {
                    let _ = thread_tx.try_send(100.0);
                    edges
                })
            }));
        }

        drop(prog_tx);

        let file = File::create(output_path).await?;
        let mut writer = BufWriter::new(file);
        let mut seen_msg_ids: HashSet<String> = HashSet::new();

        // 3. Await chunks sequentially (maintaining chronological order)
        for handle in handles {
            // Check cancellation before processing the next chunk's data
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled by user",
                )
                .into());
            }

            let edges = handle
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))??;

            for edge in edges {
                // Double check cancellation during heavy IO writes
                if cancel_flag.load(Ordering::Relaxed) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Cancelled by user",
                    )
                    .into());
                }

                let node = &edge["node"];
                let message_id = node["id"].as_str().unwrap_or("").to_string();

                if message_id.is_empty() || !seen_msg_ids.insert(message_id.clone()) {
                    continue;
                }

                let username = node["commenter"]["displayName"]
                    .as_str()
                    .unwrap_or("Unknown")
                    .to_string();
                let color = node["message"]["userColor"]
                    .as_str()
                    .unwrap_or("#FFFFFF")
                    .to_string();
                let offset_sec = node["contentOffsetSeconds"].as_f64().unwrap_or(0.0);

                let timestamp_str = node["createdAt"].as_str().unwrap_or("");
                let timestamp_ms = if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp_str) {
                    dt.timestamp_millis() as u64
                } else {
                    0
                };

                let mut content = String::new();
                if let Some(fragments) = node["message"]["fragments"].as_array() {
                    for frag in fragments {
                        if let Some(text) = frag["text"].as_str() {
                            content.push_str(text);
                        }
                    }
                }

                let unified_msg = UnifiedChatMessage {
                    id: message_id,
                    username,
                    color,
                    content,
                    offset_sec,
                    timestamp_ms,
                };

                let json_line = serde_json::to_string(&unified_msg)?;
                writer.write_all(json_line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
            }
        }

        writer.flush().await?;
        let _ = progress_task.await;

        // 4. Success! Let the user keep the file.
        guard.persist = true;
        Ok(())
    }
}

/// The isolated worker that handles Twitch's GQL sequential pagination for a specific time window
async fn download_chunk_worker(
    client: Client,
    chat_id: String,
    start_sec: f64,
    end_sec: f64,
    cancel_flag: Arc<AtomicBool>, // <--- Added token parameter
) -> AppResult<Vec<serde_json::Value>> {
    let mut edges = Vec::new();
    let mut cursor: Option<String> = None;
    let mut consecutive_empty = 0;
    const EMPTY_THRESHOLD: usize = 10;
    const ERROR_RETRY_LIMIT: usize = 5;
    let mut error_count = 0;

    loop {
        // Break network loop immediately if cancelled
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(
                std::io::Error::new(std::io::ErrorKind::Interrupted, "Cancelled by user").into(),
            );
        }

        let variables = if let Some(ref cur) = cursor {
            json!({ "videoID": chat_id, "cursor": cur })
        } else {
            json!({ "videoID": chat_id, "contentOffsetSeconds": start_sec })
        };

        let body = json!([{
            "operationName": "VideoCommentsByOffsetOrCursor",
            "variables": variables,
            "extensions": { "persistedQuery": { "version": 1, "sha256Hash": "b70a3591ff0f4e0313d126c6a1502d79a1c02baebb288227c582044aa76adf6a" } }
        }]);

        let body_str = serde_json::to_string(&body)?;

        let resp = match client
            .post("https://gql.twitch.tv/gql")
            .header("Client-ID", "kd1unb4b3q4t58fwlpcbzcbnm76a8fp")
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => {
                error_count += 1;
                if error_count >= ERROR_RETRY_LIMIT {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(500 * error_count as u64))
                    .await;
                continue;
            }
        };

        // Re-check after waiting for IO
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(
                std::io::Error::new(std::io::ErrorKind::Interrupted, "Cancelled by user").into(),
            );
        }

        let body_text = resp.text().await?;
        let val: serde_json::Value = serde_json::from_str(&body_text).unwrap_or(json!([]));

        error_count = 0;

        let video = &val[0]["data"]["video"];
        if video.is_null() {
            break;
        }

        let comments = &video["comments"];
        let edges_arr_opt = comments["edges"].as_array();

        if let Some(edges_arr) = edges_arr_opt {
            if edges_arr.is_empty() {
                consecutive_empty += 1;
                if consecutive_empty >= EMPTY_THRESHOLD {
                    break;
                }
            } else {
                consecutive_empty = 0;

                let mut latest_offset = start_sec;
                for edge in edges_arr {
                    if let Some(offset) = edge["node"]["contentOffsetSeconds"].as_f64() {
                        latest_offset = offset;
                        if offset > end_sec {
                            return Ok(edges);
                        }
                    }
                    edges.push(edge.clone());
                }

                if latest_offset >= end_sec {
                    break;
                }
            }
        }

        let page_info = &comments["pageInfo"];
        if page_info["hasNextPage"].as_bool().unwrap_or(false) {
            if let Some(end_cursor) = page_info["endCursor"].as_str() {
                cursor = Some(end_cursor.to_string());
            } else {
                break;
            }
        } else {
            break;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    Ok(edges)
}
