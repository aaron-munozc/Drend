use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::Stream as FuturesStream;
use futures_util::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as TokioMutex;

use crate::core::{analyze_url_core, AppTask, FrontendChatOptions, FrontendVodOptions, QueueSettings, RenderVideoArgs, TaskManager};
use stream_extractor::StreamClient;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct ServerState {
    pub manager: TaskManager,
    pub app_handle: AppHandle,
    pub stream_client: StreamClient,
}

#[derive(Clone)]
pub struct ServerController {
    shutdown_tx: Arc<TokioMutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl ServerController {
    pub fn new() -> Self {
        Self {
            shutdown_tx: Arc::new(TokioMutex::new(None)),
        }
    }

    pub async fn start(
        &self,
        app_handle: AppHandle,
        manager: TaskManager,
        stream_client: StreamClient,
    ) -> Result<(), String> {
        let mut tx_lock = self.shutdown_tx.lock().await;
        if tx_lock.is_some() {
            return Err("Server is already running.".to_string());
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        *tx_lock = Some(tx);

        let state = ServerState {
            manager,
            app_handle,
            stream_client,
        };

        tauri::async_runtime::spawn(async move {
            let app = Router::new()
                .route("/api/health", get(health_check))
                .route("/api/tasks", get(get_tasks))
                .route("/api/tasks/{id}", get(get_single_task))
                .route("/api/tasks/{id}/cancel", post(cancel_task))
                .route("/api/tasks/events", get(stream_task_events))
                .route("/api/analyze", post(analyze_url_handler))
                .route("/api/vod/download", post(trigger_video_download))
                .route("/api/chat/download", post(trigger_chat_download))
                .route("/api/chat/render", post(trigger_chat_render))
                .route(
                    "/api/settings/queue",
                    get(get_queue_settings_handler).post(update_queue_settings_handler),
                )
                .with_state(state);

            match tokio::net::TcpListener::bind("127.0.0.1:61423").await {
                Ok(listener) => {
                    log::info!("Localhost API running on http://127.0.0.1:61423");
                    if let Err(e) = axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            let _ = rx.await;
                            log::info!("Localhost API shutting down gracefully.");
                        })
                        .await
                    {
                        log::error!("API Server error: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("Failed to start API server: {}", e);
                }
            }
        });

        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        let mut tx_lock = self.shutdown_tx.lock().await;
        if let Some(tx) = tx_lock.take() {
            let _ = tx.send(());
            Ok(())
        } else {
            Err("Server is not currently running.".to_string())
        }
    }
}

async fn health_check() -> impl axum::response::IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "healthy" })),
    )
}

#[tauri::command]
pub async fn start_api_server(
    app_handle: AppHandle,
    manager: tauri::State<'_, TaskManager>,
    client: tauri::State<'_, StreamClient>,
    controller: tauri::State<'_, ServerController>,
) -> Result<String, String> {
    controller
        .start(app_handle, (*manager).clone(), (*client).clone())
        .await?;
    Ok("API Server started".to_string())
}

#[tauri::command]
pub async fn stop_api_server(
    controller: tauri::State<'_, ServerController>,
) -> Result<String, String> {
    controller.stop().await?;
    Ok("API Server stopped".to_string())
}

async fn get_tasks(State(state): State<ServerState>) -> Json<Vec<AppTask>> {
    Json(state.manager.get_tasks())
}

async fn get_single_task(
    State(state): State<ServerState>,
    Path(task_id): Path<String>,
) -> Result<Json<AppTask>, StatusCode> {
    match state.manager.get_task(&task_id) {
        Some(task) => Ok(Json(task)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn cancel_task(
    State(state): State<ServerState>,
    Path(task_id): Path<String>,
) -> Result<Json<String>, (StatusCode, String)> {
    state
        .manager
        .cancel_task(&task_id)
        .map(|_| Json("Task cancellation successfully processed".to_string()))
        .map_err(|err| (StatusCode::BAD_REQUEST, err))
}

async fn stream_task_events(
    State(state): State<ServerState>,
) -> Sse<impl FuturesStream<Item = Result<Event, Infallible>>> {
    let rx = state.manager.subscribe_events();

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(task) => return Some((task, rx)),
                Err(RecvError::Lagged(_)) => continue, // Automatically skip lagged frames
                Err(RecvError::Closed) => return None,  // Channel closed, terminate stream
            }
        }
    })
        .map(|task| {
            Ok(Event::default()
                .json_data(&task)
                .unwrap_or_else(|_| Event::default().comment("serialization error")))
        });

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

async fn get_queue_settings_handler(
    State(state): State<ServerState>,
) -> Json<QueueSettings> {
    Json(state.manager.get_settings())
}

async fn update_queue_settings_handler(
    State(state): State<ServerState>,
    Json(payload): Json<QueueSettings>,
) -> Result<Json<String>, (StatusCode, String)> {
    state
        .manager
        .apply_settings(payload)
        .map(|_| Json("Settings updated successfully".to_string()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(Deserialize)]
pub struct AnalyzeReq {
    url: String,
}

async fn analyze_url_handler(
    State(state): State<ServerState>,
    Json(payload): Json<AnalyzeReq>,
) -> Result<Json<crate::types::Metadata>, (StatusCode, String)> {
    let cache = state.app_handle.state::<crate::AppCache>();

    match analyze_url_core(payload.url, &state.app_handle, &state.stream_client, &cache).await {
        Ok(meta) => Ok(Json(meta)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Deserialize)]
pub struct VodDownloadReq {
    url: String,
    options: Option<FrontendVodOptions>,
}

async fn trigger_video_download(
    State(state): State<ServerState>,
    Json(payload): Json<VodDownloadReq>,
) -> Result<Json<String>, (StatusCode, String)> {
    let cache = state.app_handle.state::<crate::AppCache>();

    let cached_meta = {
        let mut lock = cache.streams.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Lock poisoned".to_string(),
            )
        })?;
        lock.get(&payload.url).cloned()
    };

    let meta = match cached_meta {
        Some(m) => m,
        None => analyze_url_core(
            payload.url.clone(),
            &state.app_handle,
            &state.stream_client,
            &cache,
        )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    };

    let task_id = state.manager.enqueue_vod_download(
        None,
        meta.normalized.original_url,
        meta.normalized.title,
        payload.options.unwrap_or_default(),
    );

    Ok(Json(task_id))
}

#[derive(Deserialize)]
pub struct ChatDownloadReq {
    url: String,
    options: Option<FrontendChatOptions>,
}

async fn trigger_chat_download(
    State(state): State<ServerState>,
    Json(payload): Json<ChatDownloadReq>,
) -> Result<Json<String>, (StatusCode, String)> {
    let cache = state.app_handle.state::<crate::AppCache>();

    let cached_meta = {
        let mut lock = cache.streams.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Lock poisoned".to_string(),
            )
        })?;
        lock.get(&payload.url).cloned()
    };

    let meta = match cached_meta {
        Some(m) => m,
        None => analyze_url_core(
            payload.url.clone(),
            &state.app_handle,
            &state.stream_client,
            &cache,
        )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    };

    let stream_metadata = meta.stream_metadata.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Chat download is not supported for this platform/URL.".to_string(),
        )
    })?;

    let task_id = state.manager.enqueue_chat_download(
        None,
        stream_metadata,
        payload.options.unwrap_or_default(),
    );

    Ok(Json(task_id))
}

#[derive(Deserialize)]
pub struct RenderReq {
    json_file_path: String,
    options: Option<RenderVideoArgs>,
}

async fn trigger_chat_render(
    State(state): State<ServerState>,
    Json(payload): Json<RenderReq>,
) -> Json<String> {
    let input_path = PathBuf::from(payload.json_file_path);
    let cache_dir_base = state.app_handle.path().app_cache_dir().unwrap_or_default();

    let task_id = state.manager.enqueue_chat_render(
        None,
        input_path,
        payload.options.unwrap_or_default(),
        cache_dir_base,
    );
    Json(task_id)
}