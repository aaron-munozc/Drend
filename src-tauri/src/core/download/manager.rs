use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::mpsc;
use tokio::sync::watch;

// Using the updated types from the library
use stream_extractor::{
    ChatOptions as ExtractorChatOptions, DownloadOptions as ExtractorDownloadOptions,
    ProgressPayload, QualityPreference, Stream, StreamClient, StreamMetadata, VideoFormat,
};
use crate::types::AppResult;

// 1. SPECIFIC FRONTEND CONFIGS
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrontendVodOptions {
    pub quality: Option<QualityPreference>,
    pub format: Option<VideoFormat>,
    pub threads: Option<usize>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub buffer_ms: Option<u64>,
    pub save_folder: Option<String>,
    pub file_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrontendChatOptions {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub buffer_ms: Option<u64>,
    pub max_retries: Option<usize>,
    pub kick_concurrency: Option<usize>,
    pub empty_cycle_threshold: Option<usize>,
    pub save_folder: Option<String>,
    pub file_name: Option<String>,
}

// 2. UI STATE TYPES
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Queued,
    Processing,
    Merging,
    Completed,
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
    ChatDownload { meta: StreamMetadata, options: FrontendChatOptions },
    VodDownload { meta: StreamMetadata, options: FrontendVodOptions },
}

struct QueueItem {
    task_id: String,
    payload: JobPayload,
}

// 3. MANAGER
pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    tx: mpsc::UnboundedSender<QueueItem>,
}

impl TaskManager {
    pub fn new(app_handle: AppHandle) -> Self {
        let tasks: Arc<Mutex<HashMap<String, AppTask>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<QueueItem>();

        let tasks_clone = Arc::clone(&tasks);

        tauri::async_runtime::spawn(async move {
            while let Some(item) = rx.recv().await {
                let task_id = item.task_id.clone();
                let app = app_handle.clone();
                let state_map = Arc::clone(&tasks_clone);

                // Mark as starting
                if let Some(task) = state_map.lock().unwrap().get_mut(&task_id) {
                    task.status = TaskStatus::Processing;
                    task.status_text = Some("Initializing...".into());
                    let _ = app.emit("task-progress", task.clone());
                }

                // Spawn worker
                tauri::async_runtime::spawn(async move {
                    let result = match item.payload {
                        JobPayload::ChatDownload { meta, options } => {
                            Self::process_chat(&app, state_map.clone(), &task_id, meta, options).await
                        }
                        JobPayload::VodDownload { meta, options } => {
                            Self::process_vod(&app, state_map.clone(), &task_id, meta, options).await
                        }
                    };

                    // Handle completion/failure
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
                                // Intercept explicit cancellations gracefully
                                if err_msg.to_lowercase().contains("cancelled") {
                                    task.status = TaskStatus::Failed("Cancelled".into());
                                    task.status_text = Some("Cancelled by user".into());
                                } else {
                                    task.status = TaskStatus::Failed(err_msg.clone());
                                    task.status_text = Some(err_msg);
                                }
                            }
                        }
                        let _ = app.emit("task-progress", task.clone());
                    }
                });
            }
        });

        Self { tasks, tx }
    }

    // --- VOD WORKER ---
    async fn process_vod(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        task_id: &str,
        meta: StreamMetadata,
        opts: FrontendVodOptions,
    ) -> AppResult<()> {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_handler = app.listen(format!("cancel-task-{}", task_id), move |_| {
            let _ = cancel_tx.send(true);
        });

        let t_id = task_id.to_string();
        let app_clone = app.clone();

        let progress_hook = Arc::new(move |payload: ProgressPayload| {
            let mut locked = tasks.lock().unwrap();
            if let Some(task) = locked.get_mut(&t_id) {
                match payload {
                    ProgressPayload::Downloading { percent, message } => {
                        task.status = TaskStatus::Processing;
                        task.progress = percent as f32;
                        task.status_text = Some(message);
                    }
                    ProgressPayload::Merging => {
                        task.status = TaskStatus::Merging;
                        task.status_text = Some("Merging streams...".into());
                    }
                    ProgressPayload::Done => task.progress = 100.0,
                    ProgressPayload::Error { message } => {
                        task.status_text = Some(format!("Engine error: {}", message))
                    }
                }
                let _ = app_clone.emit("task-progress", task.clone());
            }
        });

        let out_dir = opts
            .save_folder
            .map(PathBuf::from)
            .or_else(|| app.path().download_dir().ok());

        // Map frontend quality index to the library's Enum
        let quality = opts
            .quality
            .unwrap_or(QualityPreference::Best);

        let engine_opts = ExtractorDownloadOptions {
            output_dir: out_dir,
            output_name: opts.file_name,
            threads: opts.threads.unwrap_or(4),
            quality,
            format: VideoFormat::default(),
            start_ms: opts.start_ms,
            end_ms: opts.end_ms,
            buffer_ms: None,
            progress_hook: Some(progress_hook),
            cancel_rx: Some(cancel_rx),
        };

        let client = StreamClient::new()?;

        let stream = Stream::new(meta, &client);

        stream.download_video(engine_opts).await?;

        app.unlisten(cancel_handler);
        Ok(())
    }

    // --- CHAT WORKER ---
    async fn process_chat(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        task_id: &str,
        meta: StreamMetadata,
        opts: FrontendChatOptions,
    ) -> AppResult<()> {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_handler = app.listen(format!("cancel-task-{}", task_id), move |_| {
            let _ = cancel_tx.send(true);
        });

        let t_id = task_id.to_string();
        let app_clone = app.clone();

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
            }
        });

        let out_dir = opts
            .save_folder
            .map(PathBuf::from)
            .or_else(|| app.path().download_dir().ok());

        let engine_opts = ExtractorChatOptions {
            output_dir: out_dir,
            output_name: opts.file_name,
            start_ms: opts.start_ms,
            end_ms: opts.end_ms,
            buffer_ms: None,
            max_retries: opts.max_retries.unwrap_or(8),
            kick_concurrency: opts.kick_concurrency.unwrap_or(10),
            empty_cycle_threshold: opts.empty_cycle_threshold.unwrap_or(6),
            progress_hook: Some(progress_hook),
            cancel_rx: Some(cancel_rx),
        };

        let client = StreamClient::new()?;
        let stream = Stream::new(meta, &client);
        stream.download_chat(engine_opts).await?;

        app.unlisten(cancel_handler);
        Ok(())
    }

    // --- ENQUEUE METHODS ---
    pub fn enqueue_chat_download(&self, meta: StreamMetadata, options: FrontendChatOptions) -> String {
        let title = meta.title.clone().unwrap_or_else(|| "Unknown Stream".to_string());

        let id_str = meta.chat_id.map(|id| id.to_string()).unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_string()
        });

        let task_id = format!("chat_{}", id_str);
        self.create_task(&task_id, TaskType::ChatDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::ChatDownload { meta, options },
        });
        task_id
    }

    pub fn enqueue_vod_download(&self, meta: StreamMetadata, options: FrontendVodOptions) -> String {
        // Fallback to title from metadata natively
        let title = meta.title.clone().unwrap_or_else(|| "Unknown Stream".to_string());

        let id_str = meta.vod_uuid.clone().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_string()
        });

        let task_id = format!("vod_{}", id_str);
        self.create_task(&task_id, TaskType::VodDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::VodDownload { meta, options },
        });
        task_id
    }

    fn create_task(&self, task_id: &str, task_type: TaskType, title: String) {
        self.tasks.lock().unwrap().insert(
            task_id.to_string(),
            AppTask {
                task_id: task_id.to_string(),
                task_type,
                title,
                progress: 0.0,
                status: TaskStatus::Queued,
                status_text: Some("Queuing...".into()),
            },
        );
    }

    pub fn get_tasks(&self) -> Vec<AppTask> {
        self.tasks.lock().unwrap().values().cloned().collect()
    }
}