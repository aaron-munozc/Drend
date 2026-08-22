use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, watch, OwnedSemaphorePermit, Semaphore};

use crate::core::chat_renderer::{process_chat_render, EmoteNameMap, RenderVideoArgs};
use crate::error::AppError;
use crate::tools;
use crate::types::AppResult;

use stream_extractor::{
    download_clip_chat, download_vod_chat, ChatDownloadOptions, KickOptions, ProgressPayload,
    Stream, StreamClient,
};

// ─────────────────────────────────────────────────────────────────────────────
// Enums / Types
// ─────────────────────────────────────────────────────────────────────────────

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
    pub video_format_id: Option<String>,
    pub audio_format_id: Option<String>,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchRenderItem {
    pub id: String,
    pub json_file_path: String,
    pub options: RenderVideoArgs,
}

// ─────────────────────────────────────────────────────────────────────────────
// QueueSettings — Persisted on disk
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSettings {
    /// Max simultaneous VOD + chat downloads (I/O-bound, default 2).
    pub max_concurrent_downloads: usize,
    /// Max simultaneous chat renders (CPU/GPU-bound, default 1).
    pub max_concurrent_renders: usize,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 2,
            max_concurrent_renders: 1,
        }
    }
}

impl QueueSettings {
    fn path(app: &AppHandle) -> Result<PathBuf, AppError> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| AppError::Generic(format!("Config dir resolution failed: {}", e)))?;
        fs::create_dir_all(&dir)
            .map_err(|e| AppError::Generic(format!("Failed to create config dir: {}", e)))?;
        Ok(dir.join("queue_settings.json"))
    }

    pub fn load(app: &AppHandle) -> Self {
        Self::path(app)
            .ok()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), AppError> {
        let path = Self::path(app)?;
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Generic(format!("Serialization error: {}", e)))?;
        fs::write(path, data)
            .map_err(|e| AppError::Generic(format!("Failed to write settings to disk: {}", e)))?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Concurrency Limits
// ─────────────────────────────────────────────────────────────────────────────

struct LimitsInner {
    download_semaphore: Arc<Semaphore>,
    render_semaphore: Arc<Semaphore>,
}

#[derive(Clone)]
struct Limits(Arc<Mutex<LimitsInner>>);

impl Limits {
    fn from_settings(s: &QueueSettings) -> Self {
        Self(Arc::new(Mutex::new(LimitsInner {
            download_semaphore: Arc::new(Semaphore::new(s.max_concurrent_downloads.max(1))),
            render_semaphore: Arc::new(Semaphore::new(s.max_concurrent_renders.max(1))),
        })))
    }

    async fn acquire_download(&self) -> OwnedSemaphorePermit {
        let sem = self.0.lock().unwrap().download_semaphore.clone();
        sem.acquire_owned().await.expect("semaphore closed")
    }

    async fn acquire_render(&self) -> OwnedSemaphorePermit {
        let sem = self.0.lock().unwrap().render_semaphore.clone();
        sem.acquire_owned().await.expect("semaphore closed")
    }

    fn apply(&self, s: &QueueSettings) {
        let mut inner = self.0.lock().unwrap();
        inner.download_semaphore = Arc::new(Semaphore::new(s.max_concurrent_downloads.max(1)));
        inner.render_semaphore = Arc::new(Semaphore::new(s.max_concurrent_renders.max(1)));
        log::info!(
            "[Queue] Limits updated — downloads: {}, renders: {}",
            s.max_concurrent_downloads,
            s.max_concurrent_renders
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TaskManager
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TaskManager {
    app: AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    cancellations: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    limits: Limits,
    settings: Arc<Mutex<QueueSettings>>,
    event_tx: broadcast::Sender<AppTask>,
}

impl TaskManager {
    pub fn new(app_handle: AppHandle) -> Self {
        let settings = QueueSettings::load(&app_handle);
        log::info!(
            "[Queue] Loaded settings — downloads: {}, renders: {}",
            settings.max_concurrent_downloads,
            settings.max_concurrent_renders
        );
        let (event_tx, _) = broadcast::channel(256);
        Self {
            app: app_handle,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            limits: Limits::from_settings(&settings),
            settings: Arc::new(Mutex::new(settings)),
            event_tx,
        }
    }

    pub fn apply_settings(&self, new_settings: QueueSettings) -> Result<(), AppError> {
        new_settings.save(&self.app)?;
        self.limits.apply(&new_settings);
        *self.settings.lock().unwrap() = new_settings;
        Ok(())
    }

    pub fn get_settings(&self) -> QueueSettings {
        self.settings.lock().unwrap().clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AppTask> {
        self.event_tx.subscribe()
    }

    // ─────────────────────────────────────────────────────────────────────
    // Enqueue Methods
    // ─────────────────────────────────────────────────────────────────────

    pub fn enqueue_chat_render(
        &self,
        task_id: Option<String>,
        input_path: PathBuf,
        args: RenderVideoArgs,
        cache_dir_base: PathBuf,
    ) -> String {
        let title = input_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Chat Render".to_string());

        let task_id = self.resolve_task_id(task_id, "render");
        self.setup_task(&task_id, TaskType::ChatRender, format!("Rendering: {}", title));

        let app = self.app.clone();
        let tasks = Arc::clone(&self.tasks);
        let cancellations = Arc::clone(&self.cancellations);
        let limits = self.limits.clone();
        let event_tx = self.event_tx.clone();
        let tid = task_id.clone();

        tauri::async_runtime::spawn(async move {
            let mut cancel_rx = match cancellations.lock().unwrap().get(&tid) {
                Some(tx) => tx.subscribe(),
                None => return,
            };

            // Acquire permit while respecting early cancellation
            let _permit = tokio::select! {
                permit = limits.acquire_render() => permit,
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        Self::mark_cancelled_waiting(&app, &tasks, &event_tx, &tid);
                        return;
                    }
                    limits.acquire_render().await
                }
            };

            Self::mark_processing(&app, &tasks, &event_tx, &tid, "Initializing...");

            let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag_setter = Arc::clone(&cancel_flag);
            let mut cancel_rx_clone = cancel_rx.clone();
            tauri::async_runtime::spawn(async move {
                while cancel_rx_clone.changed().await.is_ok() {
                    if *cancel_rx_clone.borrow() {
                        flag_setter.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            });

            let http_client = reqwest::Client::new();
            let emote_map = EmoteNameMap::build_emote_map(
                &http_client,
                &args.emote_providers,
                &args.channel_ids,
                &args.provider_credentials,
            )
                .await
                .unwrap_or_else(|e| {
                    log::warn!("[Render {}] Emote fetch failed: {}", tid, e);
                    EmoteNameMap::new()
                });

            let result = process_chat_render(
                &app,
                tasks.clone(),
                &tid,
                input_path,
                args,
                cache_dir_base,
                emote_map,
                cancel_flag,
            )
                .await;

            Self::finalize_task(&app, &tasks, &cancellations, &event_tx, &tid, result);
        });

        task_id
    }

    pub fn enqueue_batch_chat_render(
        &self,
        items: Vec<(String, PathBuf, RenderVideoArgs, PathBuf)>,
    ) -> Vec<String> {
        items
            .into_iter()
            .map(|(id, path, args, cache)| {
                self.enqueue_chat_render(Some(id), path, args, cache)
            })
            .collect()
    }

    pub fn enqueue_chat_download(
        &self,
        task_id: Option<String>,
        meta: Stream,
        options: FrontendChatOptions,
    ) -> String {
        // 1. Borrow `&meta` so we don't move fields out of `meta`
        let (title_opt, chat_id_opt) = match &meta {
            Stream::Vod(v) => (v.title.as_deref(), v.chat_id.as_deref()),
            Stream::Clip(c) => (c.title.as_deref(), c.chat_id.as_deref()),
            Stream::Live(l) => (l.title.as_deref(), l.chat_id.as_deref()),
            _ => (None, None),
        };

        let title = title_opt.unwrap_or("Unknown Stream");

        let task_id = task_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| match chat_id_opt {
                Some(id) => format!("chat_{id}"),
                None => format!("chat_{}", Self::timestamp_id()),
            });

        self.setup_task(&task_id, TaskType::ChatDownload, title.to_string());

        let app = self.app.clone();
        let tasks = Arc::clone(&self.tasks);
        let cancellations = Arc::clone(&self.cancellations);
        let limits = self.limits.clone();
        let event_tx = self.event_tx.clone();
        let tid = task_id.clone();

        tauri::async_runtime::spawn(async move {
            let mut cancel_rx = match cancellations.lock().unwrap().get(&tid) {
                Some(tx) => tx.subscribe(),
                None => return,
            };

            let _permit = tokio::select! {
            permit = limits.acquire_download() => permit,
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    Self::mark_cancelled_waiting(&app, &tasks, &event_tx, &tid);
                    return;
                }
                limits.acquire_download().await
            }
        };

            Self::mark_processing(&app, &tasks, &event_tx, &tid, "Connecting...");

            let result =
                Self::process_chat_inner(&app, tasks.clone(), &event_tx, &tid, meta, options, cancel_rx)
                    .await;

            Self::finalize_task(&app, &tasks, &cancellations, &event_tx, &tid, result);
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
        let task_id = self.resolve_task_id(task_id, "vod");
        self.setup_task(&task_id, TaskType::VodDownload, title);

        let app = self.app.clone();
        let tasks = Arc::clone(&self.tasks);
        let cancellations = Arc::clone(&self.cancellations);
        let limits = self.limits.clone();
        let event_tx = self.event_tx.clone();
        let tid = task_id.clone();

        tauri::async_runtime::spawn(async move {
            let mut cancel_rx = match cancellations.lock().unwrap().get(&tid) {
                Some(tx) => tx.subscribe(),
                None => return,
            };

            let _permit = tokio::select! {
                permit = limits.acquire_download() => permit,
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        Self::mark_cancelled_waiting(&app, &tasks, &event_tx, &tid);
                        return;
                    }
                    limits.acquire_download().await
                }
            };

            Self::mark_processing(&app, &tasks, &event_tx, &tid, "Starting download...");

            let result =
                Self::process_vod_inner(&app, tasks.clone(), &event_tx, &tid, url, options, cancel_rx)
                    .await;

            Self::finalize_task(&app, &tasks, &cancellations, &event_tx, &tid, result);
        });

        task_id
    }

    // ─────────────────────────────────────────────────────────────────────
    // Task Management
    // ─────────────────────────────────────────────────────────────────────

    pub fn cancel_task(&self, task_id: &str) -> Result<(), String> {
        let cancels = self.cancellations.lock().unwrap();
        if let Some(tx) = cancels.get(task_id) {
            let _ = tx.send(true);
            let mut tasks = self.tasks.lock().unwrap();
            if let Some(task) = tasks.get_mut(task_id) {
                if matches!(task.status, TaskStatus::Queued) {
                    task.status = TaskStatus::Cancelled;
                    task.status_text = Some("Cancelled while waiting".into());
                    let _ = self.app.emit("task-progress", task.clone());
                    let _ = self.event_tx.send(task.clone());
                }
            }
            Ok(())
        } else {
            Err("Task not found or already terminated.".into())
        }
    }

    pub fn get_tasks(&self) -> Vec<AppTask> {
        self.tasks.lock().unwrap().values().cloned().collect()
    }

    pub fn get_task(&self, task_id: &str) -> Option<AppTask> {
        self.tasks.lock().unwrap().get(task_id).cloned()
    }

    // ─────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────

    fn resolve_task_id(&self, provided: Option<String>, prefix: &str) -> String {
        provided
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("{}_{}", prefix, Self::timestamp_id()))
    }

    fn timestamp_id() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string()
    }

    fn emit(app: &AppHandle, event_tx: &broadcast::Sender<AppTask>, task: &AppTask) {
        let _ = app.emit("task-progress", task.clone());
        let _ = event_tx.send(task.clone());
    }

    fn setup_task(&self, task_id: &str, task_type: TaskType, title: String) {
        let (cancel_tx, _) = watch::channel(false);
        self.cancellations
            .lock()
            .unwrap()
            .insert(task_id.to_string(), cancel_tx);

        let task = AppTask {
            task_id: task_id.to_string(),
            task_type,
            title,
            progress: 0.0,
            status: TaskStatus::Queued,
            status_text: Some("Waiting for available slot...".into()),
        };
        self.tasks
            .lock()
            .unwrap()
            .insert(task_id.to_string(), task.clone());
        Self::emit(&self.app, &self.event_tx, &task);
    }

    fn mark_processing(
        app: &AppHandle,
        tasks: &Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: &broadcast::Sender<AppTask>,
        task_id: &str,
        text: &str,
    ) {
        let mut locked = tasks.lock().unwrap();
        if let Some(task) = locked.get_mut(task_id) {
            task.status = TaskStatus::Processing;
            task.status_text = Some(text.into());
            Self::emit(app, event_tx, task);
        }
    }

    fn mark_cancelled_waiting(
        app: &AppHandle,
        tasks: &Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: &broadcast::Sender<AppTask>,
        task_id: &str,
    ) {
        let mut locked = tasks.lock().unwrap();
        if let Some(task) = locked.get_mut(task_id) {
            task.status = TaskStatus::Cancelled;
            task.status_text = Some("Cancelled before starting".into());
            Self::emit(app, event_tx, task);
        }
    }

    fn finalize_task(
        app: &AppHandle,
        tasks: &Arc<Mutex<HashMap<String, AppTask>>>,
        cancellations: &Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
        event_tx: &broadcast::Sender<AppTask>,
        task_id: &str,
        result: AppResult<()>,
    ) {
        {
            let mut locked = tasks.lock().unwrap();
            if let Some(task) = locked.get_mut(task_id) {
                match result {
                    Ok(_) => {
                        task.status = TaskStatus::Completed;
                        task.progress = 100.0;
                        task.status_text = Some("Done!".into());
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.to_lowercase().contains("cancelled") {
                            task.status = TaskStatus::Cancelled;
                            task.status_text = Some("Cancelled by user".into());
                        } else {
                            task.status = TaskStatus::Failed(msg.clone());
                            task.status_text = Some(msg);
                        }
                    }
                }
                Self::emit(app, event_tx, task);
            }
        }
        cancellations.lock().unwrap().remove(task_id);
    }

    async fn process_vod_inner(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: &broadcast::Sender<AppTask>,
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
            match (opts.video_format_id.as_deref(), opts.audio_format_id.as_deref()) {
                (Some(vid), Some(aid)) => args.push(format!("{}+{}", vid, aid)),
                (Some(vid), None) => args.push(vid.to_string()),
                (None, Some(aid)) => args.push(format!("bv*+{}", aid)),
                (None, None) => {
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
        let event_tx_clone = event_tx.clone();

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
                                        let clean: String = rest[..end]
                                            .trim()
                                            .chars()
                                            .filter(|c| c.is_ascii_digit() || *c == '.')
                                            .collect();
                                        if let Ok(pct) = clean.parse::<f32>() {
                                            let mut locked = tasks.lock().unwrap();
                                            if let Some(task) = locked.get_mut(&t_id) {
                                                task.progress = pct;
                                                task.status_text = Some("Downloading...".into());
                                                Self::emit(&app_clone, &event_tx_clone, task);
                                            }
                                        }
                                    }
                                }
                            } else if line.contains("Merging formats") || line.contains("Extracting audio") {
                                let mut locked = tasks.lock().unwrap();
                                if let Some(task) = locked.get_mut(&t_id) {
                                    task.status = TaskStatus::Merging;
                                    task.status_text = Some("Post-processing...".into());
                                    Self::emit(&app_clone, &event_tx_clone, task);
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
            Err(AppError::Generic("yt-dlp exited with errors".into()))
        }
    }

    async fn process_chat_inner(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        event_tx: &broadcast::Sender<AppTask>,
        task_id: &str,
        meta: Stream,
        opts: FrontendChatOptions,
        cancel_rx: watch::Receiver<bool>,
    ) -> AppResult<()> {
        let t_id = task_id.to_string();
        let app_clone = app.clone();
        let event_tx_clone = event_tx.clone();
        let tasks_clone = tasks.clone();

        let progress_hook = Arc::new(move |payload: ProgressPayload| {
            let mut locked = tasks_clone.lock().unwrap();
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
                Self::emit(&app_clone, &event_tx_clone, task);
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

        let client = StreamClient::new()
            .map_err(|e| AppError::Generic(format!("Failed to initialize client: {:?}", e)))?;

        match meta {
            Stream::Vod(vod) => {
                download_vod_chat(&client, &vod, engine_opts)
                    .await
                    .map_err(|e| AppError::Generic(format!("{:?}", e)))?;
            }
            Stream::Clip(clip) => {
                download_clip_chat(&client, &clip, engine_opts)
                    .await
                    .map_err(|e| AppError::Generic(format!("{:?}", e)))?;
            }
            Stream::Live(_) => {
                return Err(AppError::Generic(
                    "Live streams do not support chat downloading".into(),
                ));
            }
            _ => {
                return Err(AppError::Generic(
                    "Unsupported stream type for chat downloading".into(),
                ));
            }
        }

        Ok(())
    }
}