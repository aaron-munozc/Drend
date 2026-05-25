use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::future::join_all;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri_plugin_http::reqwest::Client;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use url::Url;

use crate::core::chat::traits::{ChatDownloader, ChatProgressPayload};
use crate::core::chat::types::UnifiedChatMessage;
use crate::error::AppError;
use crate::types::AppResult;

// --- Cleanup Guard ---
struct ChatCleanupGuard {
    path: PathBuf,
    persist: bool,
}

impl Drop for ChatCleanupGuard {
    fn drop(&mut self) {
        if !self.persist && self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
            log::info!("Cleaned up incomplete chat file: {:?}", self.path);
        }
    }
}

pub struct KickChatDownloader;

#[async_trait]
impl ChatDownloader for KickChatDownloader {
    async fn download_chat(
        &self,
        client: &Client,
        chat_id: &str,
        _channel_slug: &str,
        start_time: DateTime<Utc>,
        duration_ms: u64,
        output_path: &Path,
        progress_callback: Box<dyn Fn(ChatProgressPayload) + Send + Sync>,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
        cancel_flag: Arc<AtomicBool>,
    ) -> AppResult<()> {
        // Register the cleanup guard early
        let mut guard = ChatCleanupGuard {
            path: output_path.to_path_buf(),
            persist: false,
        };

        let file = File::create(output_path).await?;
        let mut writer = BufWriter::new(file);
        let mut seen_msg_ids: HashSet<String> = HashSet::new();

        const STEP_MS: i64 = 5_000;
        const CONCURRENCY: usize = 15;
        const EMPTY_CYCLE_THRESHOLD: usize = 6;
        const HTTP_MAX_RETRIES: u32 = 4;

        let duration_ms_i = duration_ms as i64;
        let start_ms_i = start_ms.unwrap_or(0) as i64;
        let aligned_start_ms = (start_ms_i / STEP_MS) * STEP_MS;

        let actual_end_ms: Option<i64> = match end_ms {
            Some(e) => {
                let e_i = e as i64;
                if duration_ms_i > 0 {
                    Some(e_i.min(duration_ms_i))
                } else {
                    Some(e_i)
                }
            }
            None => {
                if duration_ms_i > 0 {
                    Some(duration_ms_i)
                } else {
                    None
                }
            }
        };

        let end_time = actual_end_ms.map(|ms| start_time + ChronoDuration::milliseconds(ms));
        let mut next_start = start_time + ChronoDuration::milliseconds(aligned_start_ms);

        let total_estimated_chunks = actual_end_ms.map(|e| {
            let duration_diff = (e - aligned_start_ms).max(0);
            (duration_diff / STEP_MS) as usize
        });

        let mut consecutive_empty_cycles = 0;
        let mut chunk_count = 0;

        loop {
            // Check for cancellation before spawning the next batch of requests
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(AppError::Generic("Cancelled by user".into()));
            }

            let mut starts: Vec<DateTime<Utc>> = Vec::with_capacity(CONCURRENCY);
            let mut candidate = next_start;

            for _ in 0..CONCURRENCY {
                if let Some(end) = end_time {
                    if candidate > end {
                        break;
                    }
                }
                starts.push(candidate);
                candidate += ChronoDuration::milliseconds(STEP_MS);
            }

            if starts.is_empty() {
                break;
            }

            let mut fetch_futs = Vec::with_capacity(starts.len());
            for st in starts.iter() {
                let st_clone = *st;
                let client_clone = client.clone();
                let chat_id_clone = chat_id.to_string();

                fetch_futs.push(async move {
                    let mut attempt = 0;
                    loop {
                        let mut url = Url::parse(&format!(
                            "https://web.kick.com/api/v1/chat/{}/history",
                            chat_id_clone
                        ))
                        .expect("Failed to parse static Kick URL");

                        let start_str = format!(
                            "{}.{:03}Z",
                            st_clone.format("%Y-%m-%dT%H:%M:%S"),
                            st_clone.timestamp_subsec_millis()
                        );
                        url.query_pairs_mut().append_pair("start_time", &start_str);

                        let resp = match client_clone
                            .get(url.as_str())
                            .header("Accept", "application/json")
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                attempt += 1;
                                if attempt >= HTTP_MAX_RETRIES {
                                    return None;
                                }
                                tokio::time::sleep(tokio::time::Duration::from_millis(
                                    200 * attempt as u64,
                                ))
                                .await;
                                continue;
                            }
                        };

                        let status = resp.status();
                        if status.is_success() {
                            if let Ok(body_text) = resp.text().await {
                                return serde_json::from_str::<serde_json::Value>(&body_text).ok();
                            }
                        } else if status.as_u16() == 429 {
                            attempt += 1;
                            if attempt >= HTTP_MAX_RETRIES {
                                return None;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(
                                1000 * attempt as u64,
                            ))
                            .await;
                            continue;
                        } else {
                            attempt += 1;
                            if attempt >= HTTP_MAX_RETRIES {
                                return None;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                            continue;
                        }
                        return None;
                    }
                });
            }

            let results = join_all(fetch_futs).await;

            // Check cancellation again right after expensive network IO
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(AppError::Generic("Cancelled by user".into()));
            }

            let mut any_messages_this_cycle = false;
            let mut reached_hard_end = false;

            for chat_response_opt in results {
                if let Some(chat_response) = chat_response_opt {
                    if let Some(msgs) = chat_response["data"]["messages"].as_array() {
                        if !msgs.is_empty() {
                            any_messages_this_cycle = true;
                            for msg in msgs {
                                if let Some(created_at_str) = msg["created_at"].as_str() {
                                    if let Ok(msg_dt) = DateTime::parse_from_rfc3339(created_at_str)
                                    {
                                        if let Some(end) = end_time {
                                            if msg_dt.with_timezone(&Utc) > end {
                                                reached_hard_end = true;
                                                continue;
                                            }
                                        }

                                        let msg_id = msg["id"].as_str().unwrap_or("").to_string();
                                        if !msg_id.is_empty() && seen_msg_ids.insert(msg_id.clone())
                                        {
                                            let username = msg["sender"]["username"]
                                                .as_str()
                                                .unwrap_or("Unknown")
                                                .to_string();
                                            let color = msg["sender"]["identity"]["color"]
                                                .as_str()
                                                .unwrap_or("#FFFFFF")
                                                .to_string();
                                            let content =
                                                msg["content"].as_str().unwrap_or("").to_string();
                                            let timestamp_ms = msg_dt.timestamp_millis() as u64;
                                            let offset_sec = ((msg_dt.timestamp_millis()
                                                - start_time.timestamp_millis())
                                                as f64)
                                                / 1000.0;

                                            let unified_msg = UnifiedChatMessage {
                                                id: msg_id,
                                                username,
                                                color,
                                                content,
                                                offset_sec,
                                                timestamp_ms,
                                            };

                                            let json_line = serde_json::to_string(&unified_msg)
                                                .unwrap_or_default();
                                            if !json_line.is_empty() {
                                                writer.write_all(json_line.as_bytes()).await?;
                                                writer.write_all(b"\n").await?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if any_messages_this_cycle {
                consecutive_empty_cycles = 0;
            } else {
                consecutive_empty_cycles += 1;
                if end_time.is_none() && consecutive_empty_cycles >= EMPTY_CYCLE_THRESHOLD {
                    break;
                }
            }

            next_start = candidate;
            chunk_count += starts.len();

            let progress_percentage = if let Some(total) = total_estimated_chunks {
                if total == 0 {
                    100.0
                } else {
                    ((chunk_count as f32 / total as f32) * 100.0).min(100.0)
                }
            } else {
                0.0
            };

            progress_callback(ChatProgressPayload {
                current_chunk: chunk_count,
                total_estimated_chunks,
                progress_percentage,
            });

            if reached_hard_end {
                break;
            }

            if !any_messages_this_cycle {
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
            } else {
                tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
            }
        }

        writer.flush().await?;

        // Success! Disable the deletion guard so the user keeps the file
        guard.persist = true;
        Ok(())
    }
}
