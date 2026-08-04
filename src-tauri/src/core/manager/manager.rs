use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::core::chat_renderer::{process_chat_render, EmoteNameMap, RenderVideoArgs};
use crate::error::AppError;
use crate::tools;
use crate::types::AppResult;
use stream_extractor::{
    ChatDownloadOptions, KickOptions, ProgressPayload, Stream, StreamClient, StreamMetadata,
};

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    #[default]
    Any,
    Mp4,
    Mkv,
    Webm,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Best,
    Mp3,
    M4a,
    Flac,
    Wav,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrontendVodOptions {
    pub save_folder: Option<String>,
    pub file_name: Option<String>,

    // --- Explicit Format Selection ---
    pub video_format_id: Option<String>,
    pub audio_format_id: Option<String>,
    // ---------------------------------
    pub resolution: Option<u32>,
    pub video_format: Option<VideoFormat>,
    pub audio_only: Option<bool>,
    pub audio_format: Option<AudioFormat>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub force_keyframes: Option<bool>,
    pub threads: Option<usize>,
    pub limit_rate: Option<String>,
    pub cookies_browser: Option<String>,
    pub live_from_start: Option<bool>,
    pub embed_metadata: Option<bool>,
    pub embed_thumbnail: Option<bool>,
    pub embed_chapters: Option<bool>,
    pub embed_subs: Option<bool>,
    pub write_auto_subs: Option<bool>,
    pub sub_langs: Option<Vec<String>>,
    pub sponsorblock: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Queued,
    Processing,
    Merging,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskType {
    ChatDownload,
    VodDownload,
    ChatRender,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppTask {
    pub task_id: String,
    pub task_type: TaskType,
    pub title: String,
    pub progress: f32,
    pub status: TaskStatus,
    pub status_text: Option<String>,
}

pub enum JobPayload {
    ChatDownload {
        meta: StreamMetadata,
        options: FrontendChatOptions,
    },
    VodDownload {
        url: String,
        options: FrontendVodOptions,
    },
    ChatRender {
        input_path: PathBuf,
        args: RenderVideoArgs,
        cache_dir_base: PathBuf,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrontendChatOptions {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub max_retries: Option<usize>,
    pub kick_concurrency: Option<usize>,
    pub empty_cycle_threshold: Option<usize>,
    pub save_folder: Option<String>,
    pub file_name: Option<String>,
}

struct QueueItem {
    task_id: String,
    payload: JobPayload,
}

#[derive(Clone)]
pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    cancellations: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    tx: mpsc::UnboundedSender<QueueItem>,
    event_tx: mpsc::Sender<AppTask>,
}

impl TaskManager {
    pub fn new(app_handle: AppHandle) -> Self {
        let tasks: Arc<Mutex<HashMap<String, AppTask>>> = Arc::new(Mutex::new(HashMap::new()));
        let cancellations: Arc<Mutex<HashMap<String, watch::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<QueueItem>();
        let (event_tx, _) = mpsc::channel::<AppTask>(100);

        let tasks_clone = Arc::clone(&tasks);
        let cancellations_clone = Arc::clone(&cancellations);
        let event_tx_clone = event_tx.clone();

        tauri::async_runtime::spawn(async move {
            // --- NEW: Initialize a shared HTTP client for network requests ---
            let http_client = reqwest::Client::new();

            while let Some(item) = rx.recv().await {
                let task_id = item.task_id.clone();
                let app = app_handle.clone();
                let state_map = Arc::clone(&tasks_clone);
                let cancels = Arc::clone(&cancellations_clone);
                let progress_broadcast = event_tx_clone.clone();
                let http_client_clone = http_client.clone(); // Clone for the inner thread

                let mut cancel_rx = {
                    let locked_cancels = cancels.lock().unwrap();
                    if let Some(tx) = locked_cancels.get(&task_id) {
                        if *tx.borrow() {
                            let mut locked_tasks = state_map.lock().unwrap();
                            if let Some(task) = locked_tasks.get_mut(&task_id) {
                                task.status = TaskStatus::Cancelled;
                                task.status_text = Some("Cancelled before starting".into());
                                let _ = app.emit("task-progress", task.clone());
                                let _ = progress_broadcast.try_send(task.clone());
                            }
                            continue;
                        }
                        Some(tx.subscribe())
                    } else {
                        None
                    }
                };

                if cancel_rx.is_none() {
                    continue;
                }
                let mut cancel_rx = cancel_rx.unwrap();

                if let Some(task) = state_map.lock().unwrap().get_mut(&task_id) {
                    task.status = TaskStatus::Processing;
                    task.status_text = Some("Initializing...".into());
                    let _ = app.emit("task-progress", task.clone());
                    let _ = progress_broadcast.try_send(task.clone());
                }

                tauri::async_runtime::spawn(async move {
                    let result = match item.payload {
                        JobPayload::ChatDownload { meta, options } => {
                            Self::process_chat_inner(
                                &app,
                                state_map.clone(),
                                progress_broadcast.clone(),
                                &task_id,
                                meta,
                                options,
                                cancel_rx,
                            )
                            .await
                        }
                        JobPayload::VodDownload { url, options } => {
                            Self::process_vod_inner(
                                &app,
                                state_map.clone(),
                                progress_broadcast.clone(),
                                &task_id,
                                url,
                                options,
                                cancel_rx,
                            )
                            .await
                        }
                        JobPayload::ChatRender {
                            input_path,
                            args,
                            cache_dir_base,
                        } => {
                            let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            let flag_setter = Arc::clone(&cancel_flag);

                            tauri::async_runtime::spawn(async move {
                                if cancel_rx.changed().await.is_ok() {
                                    if *cancel_rx.borrow() {
                                        flag_setter
                                            .store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
                            });

                            {
                                let mut locked = state_map.lock().unwrap();
                                if let Some(task) = locked.get_mut(&task_id) {
                                    task.status_text = Some("Fetching emote metadata...".into());
                                    let _ = app.emit("task-progress", task.clone());
                                    let _ = progress_broadcast.try_send(task.clone());
                                }
                            }

                            let channel_id = "12345678"; // Replace with real extraction logic

                            // --- FIXED: Uses EmoteNameMap::build_emote_map and passes the client/flags ---
                            let emote_map = EmoteNameMap::build_emote_map(
                                &http_client_clone,
                                &args.emote_providers,
                                channel_id,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                // Fallback to empty map or return the error to fail the task
                                eprintln!("Failed to fetch emotes: {}", e);
                                EmoteNameMap::new()
                            });
                            process_chat_render(
                                &app,
                                state_map.clone(),
                                &task_id,
                                input_path,
                                args,
                                cache_dir_base,
                                emote_map,
                                cancel_flag,
                            )
                            .await
                        }
                    };

                    let mut locked = state_map.lock().unwrap();
                    if let Some(task) = locked.get_mut(&task_id) {
                        match result {
                            Ok(_) => {
                                task.status = TaskStatus::Completed;
                                task.progress = 100.0;
                                task.status_text = Some("Done!".into());
                            }
                            Err(e) => {
                                let err_msg = e.to_string();
                                if err_msg.to_lowercase().contains("cancelled") {
                                    task.status = TaskStatus::Cancelled;
                                    task.status_text = Some("Cancelled by user".into());
                                } else {
                                    task.status = TaskStatus::Failed(err_msg.clone());
                                    task.status_text = Some(err_msg);
                                }
                            }
                        }
                        let _ = app.emit("task-progress", task.clone());
                        let _ = progress_broadcast.try_send(task.clone());
                    }
                    cancels.lock().unwrap().remove(&task_id);
                });
            }
        });

        Self {
            tasks,
            cancellations,
            tx,
            event_tx,
        }
    }

    pub fn cancel_task(&self, task_id: &str) -> Result<(), String> {
        let cancels = self.cancellations.lock().unwrap();
        if let Some(tx) = cancels.get(task_id) {
            let _ = tx.send(true);

            let mut tasks = self.tasks.lock().unwrap();
            if let Some(task) = tasks.get_mut(task_id) {
                if matches!(task.status, TaskStatus::Queued) {
                    task.status = TaskStatus::Cancelled;
                    task.status_text = Some("Cancelled inside processing queue".into());
                    let _ = self.event_tx.try_send(task.clone());
                }
            }
            Ok(())
        } else {
            Err("Task is not running or already terminated.".into())
        }
    }

    async fn process_vod_inner(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: mpsc::Sender<AppTask>,
        task_id: &str,
        url: String,
        opts: FrontendVodOptions,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> AppResult<()> {
        let ytdlp_path = tools::get_ytdlp_path(app);
        let mut args = vec!["--newline".to_string(), "--ignore-errors".to_string()];

        let out_dir = opts
            .save_folder
            .clone()
            .map(PathBuf::from)
            .or_else(|| app.path().download_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let file_name = opts
            .file_name
            .clone()
            .unwrap_or_else(|| "%(title)s.%(ext)s".to_string());
        let out_path = out_dir.join(file_name);
        args.push("-o".to_string());
        args.push(out_path.to_string_lossy().to_string());

        let is_audio_only = opts.audio_only.unwrap_or(false);
        if is_audio_only {
            args.push("-f".into());
            // Use explicit Audio ID if provided
            if let Some(audio_id) = opts.audio_format_id.as_deref() {
                args.push(audio_id.to_string());
            } else {
                args.push("ba/best".into());
            }

            args.push("--extract-audio".into());
            args.push("--audio-quality".into());
            args.push("0".into());

            let target_audio = opts.audio_format.clone().unwrap_or(AudioFormat::Best);
            if target_audio != AudioFormat::Best {
                let fmt_str = match target_audio {
                    AudioFormat::Mp3 => "mp3",
                    AudioFormat::M4a => "m4a",
                    AudioFormat::Flac => "flac",
                    AudioFormat::Wav => "wav",
                    _ => "",
                };
                if !fmt_str.is_empty() {
                    args.push("--audio-format".into());
                    args.push(fmt_str.into());
                }
            }
        } else {
            args.push("-f".into());

            // Format ID Builder Logic
            match (
                opts.video_format_id.as_deref(),
                opts.audio_format_id.as_deref(),
            ) {
                (Some(vid), Some(aid)) => {
                    // Specific Video + Specific Audio (e.g., YouTube/Facebook)
                    args.push(format!("{}+{}", vid, aid));
                }
                (Some(vid), None) => {
                    // Pre-merged format with Video+Audio built-in (e.g., Twitch)
                    args.push(vid.to_string());
                }
                (None, Some(aid)) => {
                    // Failsafe: Best Video + Specific Audio
                    args.push(format!("bv*+{}", aid));
                }
                (None, None) => {
                    // Legacy Fallback using resolution
                    if let Some(res) = opts.resolution {
                        args.push(format!("bv*[height<={}]+ba/b[height<={}]/b", res, res));
                    } else {
                        args.push("bv*+ba/b".into());
                    }
                }
            }

            let target_video = opts.video_format.clone().unwrap_or(VideoFormat::Any);
            if target_video != VideoFormat::Any {
                let fmt_str = match target_video {
                    VideoFormat::Mp4 => "mp4",
                    VideoFormat::Mkv => "mkv",
                    VideoFormat::Webm => "webm",
                    _ => "",
                };
                if !fmt_str.is_empty() {
                    args.push("--merge-output-format".into());
                    args.push(fmt_str.into());
                }
            }
        }

        // Apply remaining options seamlessly
        if opts.start_ms.is_some() || opts.end_ms.is_some() {
            let start = opts.start_ms.map(|ms| ms / 1000).unwrap_or(0);
            let end_str = opts
                .end_ms
                .map(|ms| (ms / 1000).to_string())
                .unwrap_or_else(|| "inf".into());
            args.push("--download-sections".into());
            args.push(format!("*{}-{}", start, end_str));

            if opts.force_keyframes.unwrap_or(false) {
                args.push("--force-keyframes-at-cuts".into());
            }
        }

        if opts.embed_subs.unwrap_or(false) {
            args.push("--write-subs".into());
            args.push("--embed-subs".into());
            if opts.write_auto_subs.unwrap_or(false) {
                args.push("--write-auto-subs".into());
            }
            if let Some(langs) = &opts.sub_langs {
                if !langs.is_empty() {
                    args.push("--sub-langs".into());
                    args.push(langs.join(","));
                }
            }
        }

        if let Some(threads) = opts.threads {
            args.push("-N".into());
            args.push(threads.to_string());
        }
        if let Some(rate) = &opts.limit_rate {
            args.push("--limit-rate".into());
            args.push(rate.clone());
        }
        if let Some(browser) = &opts.cookies_browser {
            args.push("--cookies-from-browser".into());
            args.push(browser.clone());
        }
        if opts.live_from_start.unwrap_or(false) {
            args.push("--live-from-start".into());
        }

        args.push(url);

        let mut child = Command::new(&ytdlp_path)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Generic(format!("Failed to start yt-dlp: {}", e)))?;

        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout).lines();
        let t_id = task_id.to_string();
        let app_clone = app.clone();

        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        let _ = child.kill().await;
                        return Err(AppError::Generic("Cancelled by user".into()));
                    }
                }
                line_result = reader.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            if line.starts_with("[download]") && line.contains('%') {
                                if let Some(start) = line.find(']') {
                                    let rest = &line[start + 1..];
                                    if let Some(end) = rest.find('%') {
                                        let pct_str = rest[..end].trim();
                                        let clean_str: String = pct_str.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();

                                        if let Ok(percent) = clean_str.parse::<f32>() {
                                            let mut locked = tasks.lock().unwrap();
                                            if let Some(task) = locked.get_mut(&t_id) {
                                                task.progress = percent;
                                                task.status_text = Some("Downloading...".into());
                                                let _ = app_clone.emit("task-progress", task.clone());
                                                let _ = event_tx.try_send(task.clone());
                                            }
                                        }
                                    }
                                }
                            } else if line.contains("Merging formats") || line.contains("Extracting audio") {
                                let mut locked = tasks.lock().unwrap();
                                if let Some(task) = locked.get_mut(&t_id) {
                                    task.status = TaskStatus::Merging;
                                    task.status_text = Some("Post-processing stream...".into());
                                    let _ = app_clone.emit("task-progress", task.clone());
                                    let _ = event_tx.try_send(task.clone());
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| AppError::Generic(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(AppError::Generic(
                "Process terminated with error codes".into(),
            ))
        }
    }

    async fn process_chat_inner(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: mpsc::Sender<AppTask>,
        task_id: &str,
        meta: StreamMetadata,
        opts: FrontendChatOptions,
        cancel_rx: watch::Receiver<bool>,
    ) -> AppResult<()> {
        let t_id = task_id.to_string();
        let app_clone = app.clone();
        let progress_broadcast = event_tx.clone();

        let progress_hook = Arc::new(move |payload: ProgressPayload| {
            let mut locked = tasks.lock().unwrap();
            if let Some(task) = locked.get_mut(&t_id) {
                match payload {
                    ProgressPayload::Downloading { percent, message } => {
                        task.progress = percent as f32;
                        task.status_text = Some(message);
                    }
                    ProgressPayload::Done => task.progress = 100.0,
                    ProgressPayload::Error { message } => task.status_text = Some(message),
                    _ => {}
                }
                let _ = app_clone.emit("task-progress", task.clone());
                let _ = progress_broadcast.try_send(task.clone());
            }
        });

        let out_dir = opts
            .save_folder
            .map(PathBuf::from)
            .or_else(|| app.path().download_dir().ok());
        let kick_opts = KickOptions::default()
            .with_concurrency(opts.kick_concurrency.unwrap_or(10))
            .with_empty_cycle_threshold(opts.empty_cycle_threshold.unwrap_or(6));

        let mut engine_opts = ChatDownloadOptions::default()
            .with_max_retries(opts.max_retries.unwrap_or(8))
            .with_platform_options(kick_opts)
            .with_progress_hook(progress_hook)
            .with_cancel_rx(cancel_rx);

        if let Some(dir) = out_dir {
            engine_opts = engine_opts.with_output_dir(dir);
        }
        if let Some(name) = opts.file_name {
            engine_opts = engine_opts.with_output_name(name);
        }
        if let Some(start) = opts.start_ms {
            engine_opts = engine_opts.with_start_ms(start);
        }
        if let Some(end) = opts.end_ms {
            engine_opts = engine_opts.with_end_ms(end);
        }

        let client = StreamClient::new().map_err(|e| AppError::Generic(e.to_string()))?;
        let stream = Stream::new(meta, &client);

        stream
            .download_chat(engine_opts)
            .await
            .map_err(|e| AppError::Generic(e.to_string()))?;
        Ok(())
    }

    pub fn enqueue_chat_render(
        &self,
        task_id: Option<String>,
        input_path: PathBuf,
        args: RenderVideoArgs,
        cache_dir_base: PathBuf,
    ) -> String {
        let title = input_path
            .file_name()
            .map(|os_str| os_str.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Chat Render Job".to_string());

        // Use frontend ID if provided and non-empty, otherwise generate standard ID
        let task_id = task_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "render_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                )
            });

        let (cancel_tx, _) = watch::channel(false);
        self.cancellations
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_tx);
        self.create_task(
            &task_id,
            TaskType::ChatRender,
            format!("Rendering: {}", title),
        );

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::ChatRender {
                input_path,
                args,
                cache_dir_base,
            },
        });
        task_id
    }

    pub fn enqueue_chat_download(
        &self,
        task_id: Option<String>,
        meta: StreamMetadata,
        options: FrontendChatOptions,
    ) -> String {
        let title = meta
            .title
            .clone()
            .unwrap_or_else(|| "Unknown Stream".to_string());

        let task_id = task_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| {
                let id_str = meta.chat_id.map(|id| id.to_string()).unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                        .to_string()
                });
                format!("chat_{}", id_str)
            });

        let (cancel_tx, _) = watch::channel(false);
        self.cancellations
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_tx);
        self.create_task(&task_id, TaskType::ChatDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::ChatDownload { meta, options },
        });
        task_id
    }

    pub fn enqueue_vod_download(
        &self,
        task_id: Option<String>,
        url: String,
        title: String,
        options: FrontendVodOptions,
    ) -> String {
        let task_id = task_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "vod_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                )
            });

        let (cancel_tx, _) = watch::channel(false);
        self.cancellations
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_tx);
        self.create_task(&task_id, TaskType::VodDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::VodDownload { url, options },
        });
        task_id
    }

    fn create_task(&self, task_id: &str, task_type: TaskType, title: String) {
        let task = AppTask {
            task_id: task_id.to_string(),
            task_type,
            title,
            progress: 0.0,
            status: TaskStatus::Queued,
            status_text: Some("Queuing...".into()),
        };
        self.tasks
            .lock()
            .unwrap()
            .insert(task_id.to_string(), task.clone());
        let _ = self.event_tx.try_send(task);
    }

    pub fn get_tasks(&self) -> Vec<AppTask> {
        self.tasks.lock().unwrap().values().cloned().collect()
    }

    pub fn get_task(&self, task_id: &str) -> Option<AppTask> {
        self.tasks.lock().unwrap().get(task_id).cloned()
    }

    pub fn subscribe_events(&self) -> mpsc::Receiver<AppTask> {
        let (tx, rx) = mpsc::channel(100);
        let current_tasks = self.get_tasks();

        tauri::async_runtime::spawn(async move {
            for task in current_tasks {
                if tx.send(task).await.is_err() {
                    return;
                }
            }
        });
        rx
    }
}
