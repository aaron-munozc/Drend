use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager}; // Added Listener for v2 events
use tauri_plugin_http::reqwest::Client;
use tokio::sync::mpsc;

// Adjust these imports to match your project structure
use crate::core::chat::kick::KickChatDownloader;
use crate::core::chat::traits::{ChatDownloader, ChatProgressPayload};
use crate::core::chat::twitch::TwitchChatDownloader;
use crate::core::chat_renderer::{process_chat_render, RenderVideoArgs};
use crate::core::fetcher::types::{ChatMetadata, Platform};
use crate::core::vod::process_vod_download;

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOptions {
    pub quality: Option<String>,
    pub threads: Option<u8>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub duration: Option<i64>,
    pub save_folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Queued,
    Processing,
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
    pub current_step: usize,
    pub total_steps: Option<usize>,
    pub status: TaskStatus,
    pub status_text: Option<String>,
}

pub enum JobPayload {
    ChatDownload {
        meta: ChatMetadata,
        options: DownloadOptions,
    },
    VodDownload {
        m3u8_url: String,
        options: DownloadOptions,
    },
    ChatRender {
        input_path: PathBuf,
        args: RenderVideoArgs,
    },
}

struct QueueItem {
    task_id: String,
    payload: JobPayload,
}

pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    tx: mpsc::UnboundedSender<QueueItem>,
}

impl TaskManager {
    pub fn new(app_handle: AppHandle, client: Client) -> Self {
        let tasks: Arc<Mutex<HashMap<String, AppTask>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<QueueItem>();

        let tasks_clone = Arc::clone(&tasks);
        let client_clone = client;

        tauri::async_runtime::spawn(async move {
            while let Some(item) = rx.recv().await {
                let task_id = item.task_id.clone();
                let app = app_handle.clone();
                let inner_tasks = Arc::clone(&tasks_clone);
                let inner_client = client_clone.clone();

                {
                    if let Some(task) = inner_tasks.lock().unwrap().get_mut(&task_id) {
                        task.status = TaskStatus::Processing;
                        task.status_text = Some("Starting process...".into());
                        let _ = app.emit("task-progress", task.clone());
                    }
                }

                tauri::async_runtime::spawn(async move {
                    let result = match item.payload {
                        JobPayload::ChatDownload { meta, options } => {
                            Self::process_chat_download(
                                &inner_client,
                                &app,
                                inner_tasks.clone(),
                                &task_id,
                                meta,
                                options,
                            )
                            .await
                        }
                        JobPayload::VodDownload { m3u8_url, options } => {
                            process_vod_download(
                                &inner_client,
                                &app,
                                inner_tasks.clone(),
                                &task_id,
                                m3u8_url,
                                options,
                            )
                            .await
                        }
                        JobPayload::ChatRender { input_path, args } => {
                            Self::process_render_job(
                                &app,
                                inner_tasks.clone(),
                                &task_id,
                                input_path,
                                args,
                            )
                            .await
                        }
                    };

                    let mut locked = inner_tasks.lock().unwrap();
                    if let Some(task) = locked.get_mut(&task_id) {
                        match result {
                            Ok(_) => {
                                task.status = TaskStatus::Completed;
                                task.progress = 100.0;
                                task.status_text = Some("Done!".into());
                            }
                            Err(e) => {
                                // Prevent overriding a cancel status with a generic failure if already marked
                                if !e.contains("Cancelled") {
                                    task.status = TaskStatus::Failed(e.clone());
                                    task.status_text = Some(format!("Error: {}", e));
                                } else {
                                    task.status = TaskStatus::Failed("Cancelled".into());
                                    task.status_text = Some("Cancelled by user".into());
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

    // --- RENDER WORKER ---
    async fn process_render_job(
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        task_id: &str,
        input_path: PathBuf,
        args: RenderVideoArgs,
    ) -> Result<(), String> {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag_clone = Arc::clone(&cancel_flag);

        // Listen for a dynamic frontend cancel event specific to this task ID
        let event_name = format!("cancel-task-{}", task_id);
        let cancel_handler = app.listen(event_name, move |_| {
            cancel_flag_clone.store(true, Ordering::SeqCst);
        });

        // Safely resolve the system's preferred app cache directory, fallback to local ".cache"
        let cache_dir = app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| PathBuf::from(".cache"));

        // Call the decoupled engine, passing in our UI hooks
        let result = process_chat_render(
            app,
            tasks,
            task_id,
            input_path,
            args,
            cache_dir,
            cancel_flag,
        )
        .await
        .map_err(|e| e.to_string());

        // Cleanup listener to prevent memory leaks
        app.unlisten(cancel_handler);

        result
    }

    // --- DOWNLOAD WORKER ---
    async fn process_chat_download(
        client: &Client,
        app: &AppHandle,
        tasks: Arc<Mutex<HashMap<String, AppTask>>>,
        task_id: &str,
        meta: ChatMetadata,
        options: DownloadOptions,
    ) -> Result<(), String> {
        // 1. Setup Cancellation
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag_clone = Arc::clone(&cancel_flag);

        let event_name = format!("cancel-task-{}", task_id);
        let cancel_handler = app.listen(event_name, move |_| {
            cancel_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let downloader: Box<dyn ChatDownloader> = match meta.platform {
            Platform::Twitch => Box::new(TwitchChatDownloader),
            Platform::Kick => Box::new(KickChatDownloader),
        };

        let mut save_path: PathBuf = if let Some(folder) = options.save_folder {
            PathBuf::from(folder)
        } else {
            app.path()
                .download_dir()
                .or_else(|_| app.path().desktop_dir())
                .unwrap()
        };

        save_path.push(format!("{}.jsonl", task_id));

        let t_id = task_id.to_string();
        let start_time = meta.start_time.unwrap_or_else(chrono::Utc::now);
        let app_clone = app.clone();

        // 2. Execute with token
        let result = downloader
            .download_chat(
                client,
                &meta.chat_id,
                &meta.channel_slug,
                start_time,
                meta.duration_ms,
                &save_path,
                Box::new(move |progress: ChatProgressPayload| {
                    let mut locked = tasks.lock().unwrap();
                    if let Some(task) = locked.get_mut(&t_id) {
                        task.progress = progress.progress_percentage;
                        task.current_step = progress.current_chunk;
                        task.total_steps = progress.total_estimated_chunks;

                        if let Some(total) = progress.total_estimated_chunks {
                            task.status_text = Some(format!(
                                "Downloading... Chunk {}/{} ({:.1}%)",
                                progress.current_chunk, total, progress.progress_percentage
                            ));
                        } else {
                            task.status_text = Some(format!(
                                "Downloading... Chunk {} ({:.1}%)",
                                progress.current_chunk, progress.progress_percentage
                            ));
                        }
                        let _ = app_clone.emit("task-progress", task.clone());
                    }
                }),
                options.start_ms,
                options.end_ms,
                cancel_flag, // Pass the flag down
            )
            .await
            .map_err(|e| e.to_string());

        // 3. Prevent memory leak
        app.unlisten(cancel_handler);

        result
    }
    // --- ENQUEUE METHODS ---
    pub fn enqueue_chat_render(
        &self,
        input_path: PathBuf,
        title: String,
        args: RenderVideoArgs,
    ) -> String {
        // Generate a fast random hash for the render job ID
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let task_id = format!("render_{}", timestamp);

        self.create_task(&task_id, TaskType::ChatRender, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::ChatRender { input_path, args },
        });
        task_id
    }

    pub fn enqueue_chat_download(
        &self,
        meta: ChatMetadata,
        title: String,
        options: DownloadOptions,
    ) -> String {
        let task_id = format!("chat_{}", meta.chat_id);
        self.create_task(&task_id, TaskType::ChatDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::ChatDownload { meta, options },
        });
        task_id
    }

    pub fn enqueue_vod_download(
        &self,
        m3u8_url: String,
        video_id: String,
        title: String,
        options: DownloadOptions,
    ) -> String {
        let task_id = format!("vod_{}", video_id);
        self.create_task(&task_id, TaskType::VodDownload, title);

        let _ = self.tx.send(QueueItem {
            task_id: task_id.clone(),
            payload: JobPayload::VodDownload { m3u8_url, options },
        });
        task_id
    }

    fn create_task(&self, task_id: &str, task_type: TaskType, title: String) {
        let mut t = self.tasks.lock().unwrap();
        t.insert(
            task_id.to_string(),
            AppTask {
                task_id: task_id.to_string(),
                task_type,
                title,
                progress: 0.0,
                current_step: 0,
                total_steps: None,
                status: TaskStatus::Queued,
                status_text: Some("Queuing...".into()),
            },
        );
    }

    pub fn get_tasks(&self) -> Vec<AppTask> {
        self.tasks.lock().unwrap().values().cloned().collect()
    }
}
