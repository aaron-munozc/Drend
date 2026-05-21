use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tauri_plugin_http::reqwest::Client;
use tokio::sync::mpsc;
use crate::core::types::{ChatMetadata, Platform};

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
pub struct DownloadTask {
    pub task_id: String,
    pub title: String,
    pub progress: f32,
    pub status: TaskStatus,
}

pub struct DownloadManager {
    tasks: Arc<Mutex<HashMap<String, DownloadTask>>>,
    tx: mpsc::UnboundedSender<ChatMetadata>,
    client: Client,
}

impl DownloadManager {
    pub fn new(app_handle: AppHandle, client: Client) -> Self {
        let tasks = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatMetadata>();

        let tasks_clone = Arc::clone(&tasks);
        let client_clone = client.clone();

        // Spawn background worker processing queue loop
        tokio::spawn(async move {
            while let Some(meta) = rx.recv().await {
                let task_id = meta.chat_id.clone();
                let app = app_handle.clone();
                let inner_tasks = Arc::clone(&tasks_clone);
                let inner_client = client_clone.clone();

                // Update status to processing
                {
                    if let Some(task) = inner_tasks.lock().unwrap().get_mut(&task_id) {
                        task.status = TaskStatus::Processing;
                    }
                }

                // Choose strategy layout
                let downloader: Box<dyn ChatDownloader> = match meta.platform {
                    Platform::Twitch => Box::new(TwitchChatDownloader),
                    Platform::Kick => Box::new(TwitchChatDownloader), // Swap for Kick implementation
                };

                // Execute downloading pipeline asynchronously
                let path = std::path::PathBuf::from(format!("./chat_{}.json", task_id));
                let t_id = task_id.clone();

                let res = downloader.download_chat(
                    &inner_client,
                    &meta.chat_id,
                    &meta.channel_slug,
                    &path,
                    Box::new(move |progress: ChatProgressPayload| {
                        let mut locked = inner_tasks.lock().unwrap();
                        if let Some(task) = locked.get_mut(&t_id) {
                            task.progress = progress.progress_percentage;
                            // Emit progress directly payload back to Tauri Frontend UI context
                            let _ = app.emit("download-progress", task.clone());
                        }
                    }),
                ).await;

                // Mark termination state final outcomes
                let mut locked = tasks_clone.lock().unwrap();
                if let Some(task) = locked.get_mut(&task_id) {
                    match res {
                        Ok(_) => {
                            task.status = TaskStatus::Completed;
                            task.progress = 100.0;
                        },
                        Err(e) => task.status = TaskStatus::Failed(e.to_string()),
                    }
                    let _ = app_handle.emit("download-progress", task.clone());
                }
            }
        });

        Self { tasks, tx, client }
    }

    pub fn enqueue_chat_download(&self, meta: ChatMetadata, title: String) {
        let mut locked = self.tasks.lock().unwrap();
        locked.insert(meta.chat_id.clone(), DownloadTask {
            task_id: meta.chat_id.clone(),
            title,
            progress: 0.0,
            status: TaskStatus::Queued,
        });
        let _ = self.tx.send(meta);
    }

    pub fn get_tasks(&self) -> Vec<DownloadTask> {
        self.tasks.lock().unwrap().values().cloned().collect()
    }
}