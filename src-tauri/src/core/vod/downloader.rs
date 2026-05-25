use crate::core::download::manager::DownloadOptions;
use crate::core::vod::utils::run_ffmpeg;
use crate::core::AppTask;
use futures::stream::{self, StreamExt};
use m3u8_rs::Playlist;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_http::reqwest::Client;
use tokio::{fs, io::AsyncWriteExt, time::Duration};
use url::Url;

// --- Bulletproof Cleanup Guard ---
// Ensures that if the function exits early (error or cancel), the incomplete output file is wiped.
struct OutputCleanupGuard {
    path: PathBuf,
    persist: bool,
}

impl Drop for OutputCleanupGuard {
    fn drop(&mut self) {
        if !self.persist && self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
            log::info!("Cleaned up incomplete VOD file: {:?}", self.path);
        }
    }
}

pub async fn process_vod_download(
    client: &Client,
    app: &AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    task_id: &str,
    m3u8_url: String,
    options: DownloadOptions,
) -> Result<(), String> {
    // 1. Setup Cancellation Listener
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_flag_clone = Arc::clone(&cancel_flag);

    let event_name = format!("cancel-task-{}", task_id);
    let cancel_handler = app.listen(event_name, move |_| {
        cancel_flag_clone.store(true, Ordering::SeqCst);
    });

    // Run the core logic wrapped so we can guarantee the listener is removed on exit
    let res =
        process_vod_download_inner(client, app, tasks, task_id, m3u8_url, options, cancel_flag)
            .await;

    // Prevent memory leak from dangling listeners
    app.unlisten(cancel_handler);

    res
}

async fn process_vod_download_inner(
    client: &Client,
    app: &AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    task_id: &str,
    m3u8_url: String,
    options: DownloadOptions,
    cancel_flag: Arc<AtomicBool>,
) -> Result<(), String> {
    let concurrency = options
        .threads
        .map(|t| (t as usize).clamp(1, 16))
        .unwrap_or(4);

    // 1. Resolve Save Path
    let mut out_path = if let Some(folder) = &options.save_folder {
        PathBuf::from(folder)
    } else {
        app.path()
            .download_dir()
            .or_else(|_| app.path().desktop_dir())
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    out_path.push(format!("{}.mp4", task_id));

    // Register the cleanup guard for the output file
    let mut out_guard = OutputCleanupGuard {
        path: out_path.clone(),
        persist: false, // Will be set to true only upon complete success
    };

    if cancel_flag.load(Ordering::Relaxed) {
        return Err("Cancelled by user".into());
    }

    // 2. Fetch and Parse Manifest
    let manifest_bytes = client
        .get(&m3u8_url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch manifest: {}", e))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let parsed_playlist = m3u8_rs::parse_playlist(&manifest_bytes)
        .map_err(|_| "Failed to parse m3u8 playlist".to_string())?
        .1;

    let media_playlist_url = match parsed_playlist {
        Playlist::MasterPlaylist(master) => {
            let best_variant = master
                .variants
                .iter()
                .max_by_key(|v| v.bandwidth)
                .ok_or("No variants found in master playlist")?;
            Url::parse(&m3u8_url)
                .unwrap()
                .join(&best_variant.uri)
                .unwrap()
        }
        Playlist::MediaPlaylist(_) => Url::parse(&m3u8_url).unwrap(),
    };

    let media_bytes = client
        .get(media_playlist_url.as_str())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let playlist = match m3u8_rs::parse_media_playlist(&media_bytes) {
        Ok((_, p)) => p,
        Err(_) => return Err("Invalid media playlist".into()),
    };

    // 3. Time Range Slicing
    let start_target = options.start_ms.unwrap_or(0) as f64;
    let end_target = options
        .end_ms
        .filter(|&e| e > 0)
        .or_else(|| options.duration.filter(|&d| d > 0).map(|d| d as u64))
        .map(|d| d as f64);

    let mut selected = Vec::with_capacity(playlist.segments.len());
    let mut current_ms = 0.0;
    let mut first_seg_start = -1.0;

    for (idx, seg) in playlist.segments.iter().enumerate() {
        let dur_ms = seg.duration as f64 * 1000.0;
        let seg_end = current_ms + dur_ms;

        if seg_end > start_target && end_target.map_or(true, |e| current_ms < e) {
            if first_seg_start < 0.0 {
                first_seg_start = current_ms;
            }
            selected.push((idx, seg.uri.clone()));
        }
        current_ms += dur_ms;
    }

    if selected.is_empty() {
        return Err("No segments overlap with requested range.".into());
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return Err("Cancelled by user".into());
    }

    // 4. Temporary Workspace
    // TempDir automatically wipes its contents when this function drops (success or failure)
    let tmp = tempfile::Builder::new()
        .prefix("vod_")
        .tempdir_in(out_path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("Temp dir error: {}", e))?;
    let tmp_path = tmp.path().to_path_buf();

    // 5. Concurrent Chunk Streaming
    let total_count = selected.len() as f64;
    let downloaded = Arc::new(AtomicU64::new(0));

    let stream_cancel = Arc::clone(&cancel_flag);

    let paths = stream::iter(selected)
        .map(|(idx, uri)| {
            let client = client.clone();
            let url = media_playlist_url.join(&uri).unwrap();
            let path = tmp_path.join(format!("{:08}.ts", idx));
            let counter = Arc::clone(&downloaded);
            let tasks_clone = Arc::clone(&tasks);
            let app_clone = app.clone();
            let t_id = task_id.to_string();
            let worker_cancel = Arc::clone(&stream_cancel);

            async move {
                if worker_cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled by user".to_string());
                }

                let mut attempts = 0;
                loop {
                    match client.get(url.clone()).send().await {
                        Ok(mut resp) => {
                            let mut file =
                                fs::File::create(&path).await.map_err(|e| e.to_string())?;
                            let mut streaming_failed = false;

                            loop {
                                // Fast interrupt between network chunks
                                if worker_cancel.load(Ordering::Relaxed) {
                                    return Err("Cancelled by user".to_string());
                                }

                                match resp.chunk().await {
                                    Ok(Some(chunk)) => {
                                        if let Err(e) = file.write_all(&chunk).await {
                                            log::error!("Write error on chunk {}: {}", idx, e);
                                            streaming_failed = true;
                                            break;
                                        }
                                    }
                                    Ok(None) => break, // Stream complete
                                    Err(e) => {
                                        log::error!(
                                            "Network chunk read failure on stream {}: {}",
                                            idx,
                                            e
                                        );
                                        streaming_failed = true;
                                        break;
                                    }
                                }
                            }

                            if !streaming_failed {
                                file.flush().await.map_err(|e| e.to_string())?;
                                break;
                            }
                        }
                        Err(_) => {}
                    }

                    attempts += 1;
                    if attempts >= 3 {
                        return Err(format!(
                            "Failed to download segment {} after 3 retries",
                            idx
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(400 * attempts as u64)).await;
                }

                let done = counter.fetch_add(1, Ordering::Relaxed) + 1;

                let mut locked = tasks_clone.lock().unwrap();
                if let Some(task) = locked.get_mut(&t_id) {
                    task.progress = (done as f32 / total_count as f32) * 100.0;
                    task.current_step = done as usize;
                    task.total_steps = Some(total_count as usize);
                    task.status_text = Some(format!("Downloading... ({:.1}%)", task.progress));
                    let _ = app_clone.emit("task-progress", task.clone());
                }

                Ok(path)
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<Result<PathBuf, String>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err("Cancelled by user".into());
    }

    // 6. Setup FFmpeg Merge
    let list_path = tmp_path.join("list.txt");
    let list_content = paths
        .iter()
        .map(|p| format!("file '{}'", p.to_str().unwrap().replace("\\", "/")))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&list_path, list_content)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut locked = tasks.lock().unwrap();
        if let Some(task) = locked.get_mut(task_id) {
            task.status_text = Some("Merging segments losslessly...".into());
            let _ = app.emit("task-progress", task.clone());
        }
    }

    // 7. Execute FFmpeg
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
    ];

    if start_target > 0.0 || end_target.is_some() {
        let accurate_seek = (start_target - first_seg_start).max(0.0) / 1000.0;
        args.push("-ss".into());
        args.push(format!("{:.3}", accurate_seek));
    }

    args.push("-i".into());
    args.push(list_path.to_string_lossy().into_owned());

    if let Some(d) = end_target {
        args.push("-t".into());
        args.push(format!("{:.3}", (d - start_target) / 1000.0));
    }

    args.extend([
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        out_path.to_string_lossy().into_owned(),
    ]);

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (status, stderr_path, _) = run_ffmpeg(&arg_refs, &tmp_path, "merge")
        .await
        .map_err(|e| e.to_string())?;

    // Final safety check: if cancelled during the FFmpeg build, fail here.
    if cancel_flag.load(Ordering::Relaxed) {
        return Err("Cancelled by user".into());
    }

    if status != 0 {
        let err_log = fs::read_to_string(stderr_path).await.unwrap_or_default();
        return Err(format!("FFmpeg failed: {}", err_log));
    }

    // Success! Tell the guard to leave the file alone so the user keeps the video.
    out_guard.persist = true;

    Ok(())
}
