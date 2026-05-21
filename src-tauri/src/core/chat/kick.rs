use async_trait::async_trait;
use std::collections::HashSet;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tauri_plugin_http::reqwest::Client;
use chrono::{DateTime, Utc, Duration as ChronoDuration};
use url::Url;
use futures::future::join_all;

use crate::core::chat::traits::{ChatDownloader, ChatProgressPayload};
use crate::types::AppResult;

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
    ) -> AppResult<()> {
        let file = File::create(output_path).await.map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);
        let mut seen_msg_ids: HashSet<String> = HashSet::new();

        let mut next_start = start_time;

        // 🟢 Calculate true end time if it's a VOD (duration > 0)
        let end_time = if duration_ms > 0 {
            Some(start_time + ChronoDuration::milliseconds(duration_ms as i64))
        } else {
            None
        };

        const STEP_SECS: i64 = 5;
        const CONCURRENCY: usize = 10;
        const EMPTY_CYCLE_THRESHOLD: usize = 6;

        let total_estimated_chunks = if duration_ms > 0 {
            Some((duration_ms / (STEP_SECS as u64 * 1000)) as usize)
        } else {
            None
        };

        let mut consecutive_empty_cycles = 0;
        let mut chunk_count = 0;

        loop {
            let mut starts: Vec<DateTime<Utc>> = Vec::with_capacity(CONCURRENCY);
            let mut candidate = next_start;

            for _ in 0..CONCURRENCY {
                // 🟢 Stop queueing tasks if we have passed the VOD's end time
                if let Some(end) = end_time {
                    if candidate > end {
                        break;
                    }
                }
                starts.push(candidate);
                candidate += ChronoDuration::seconds(STEP_SECS);
            }

            // 🟢 If no tasks were queued, the VOD is fully downloaded
            if starts.is_empty() {
                break;
            }

            let mut fetch_futs = Vec::with_capacity(starts.len());
            for st in starts.iter() {
                let st_clone = *st;
                let client_clone = client.clone();
                let chat_id_clone = chat_id.to_string();

                fetch_futs.push(async move {
                    let mut url = Url::parse(&format!("https://web.kick.com/api/v1/chat/{}/history", chat_id_clone)).unwrap();
                    let start_str = format!("{}.{:03}Z", st_clone.format("%Y-%m-%dT%H:%M:%S"), st_clone.timestamp_subsec_millis());
                    url.query_pairs_mut().append_pair("start_time", &start_str);

                    client_clone.get(url.as_str()).header("Accept", "application/json").send().await
                });
            }

            // Fetch chunks simultaneously
            let results = join_all(fetch_futs).await;
            let mut any_messages_this_cycle = false;

            for resp_result in results {
                if let Ok(resp) = resp_result {
                    if let Ok(body_text) = resp.text().await {
                        if let Ok(chat_response) = serde_json::from_str::<serde_json::Value>(&body_text) {
                            if let Some(msgs) = chat_response["data"]["messages"].as_array() {
                                if !msgs.is_empty() {
                                    any_messages_this_cycle = true;
                                    for msg in msgs {
                                        let msg_id = msg["id"].as_str().unwrap_or("").to_string();
                                        if !msg_id.is_empty() && seen_msg_ids.insert(msg_id) {
                                            // Ensure we only write valid, unique messages
                                            let json_line = serde_json::to_string(msg).unwrap();
                                            writer.write_all(json_line.as_bytes()).await.unwrap();
                                            writer.write_all(b"\n").await.unwrap();
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
                // 🟢 Only rely on the empty cycle threshold for live streams (end_time == None)
                if end_time.is_none() && consecutive_empty_cycles >= EMPTY_CYCLE_THRESHOLD {
                    break; // Assume stream ended
                }
            }

            next_start = candidate;
            chunk_count += starts.len();

            let progress_percentage = if duration_ms > 0 {
                let elapsed_ms = (next_start - start_time).num_milliseconds().max(0) as u64;
                ((elapsed_ms as f32 / duration_ms as f32) * 100.0).min(100.0)
            } else {
                0.0 // Live streams don't have a definitive percentage
            };

            progress_callback(ChatProgressPayload {
                current_chunk: chunk_count,
                total_estimated_chunks,
                progress_percentage,
            });

            if !any_messages_this_cycle {
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
            }
        }

        writer.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}