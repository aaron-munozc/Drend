use crate::error::AppError;
use crate::types::AppResult;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use which::which;

// --- COMMON PATH HELPERS ---

/// Helper to consistently get the app's local data directory
fn get_app_dir(app: &AppHandle) -> PathBuf {
    app.path()
       .app_local_data_dir()
       .unwrap_or_else(|_| PathBuf::from("."))
}

// ==========================================
// YT-DLP LOGIC
// ==========================================

/// Path where the app manages/downloads its own yt-dlp binary.
pub fn get_local_ytdlp_path(app: &AppHandle) -> PathBuf {
    let binary_name = if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    get_app_dir(app).join(binary_name)
}

/// Resolves the executable path to use for yt-dlp.
pub fn get_ytdlp_path(app: &AppHandle) -> PathBuf {
    let local_path = get_local_ytdlp_path(app);

    if local_path.is_file() {
        return local_path;
    }
    if let Ok(global_path) = which("yt-dlp") {
        return global_path;
    }
    local_path
}

#[tauri::command]
pub async fn check_ytdlp(app: AppHandle) -> bool {
    get_ytdlp_path(&app).exists()
}

#[tauri::command]
pub async fn install_ytdlp(app: AppHandle) -> AppResult<()> {
    let target = if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else {
        "yt-dlp" // Linux
    };

    let url = format!(
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}",
        target
    );

    // ALWAYS write to the local directory, not the resolved global one
    let path = get_local_ytdlp_path(&app);

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Generic(format!("Failed to create data directory: {}", e)))?;
    }

    // GitHub often blocks requests without a User-Agent
    let client = reqwest::Client::builder()
        .user_agent("MyTauriApp/1.0")
        .build()
        .map_err(|e| AppError::Generic(e.to_string()))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to fetch yt-dlp: {}", e)))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to read response bytes: {}", e)))?;

    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| AppError::Generic(format!("Failed to write yt-dlp binary: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path)
            .await
            .map_err(|e| AppError::Generic(e.to_string()))?
            .permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&path, perms)
            .await
            .map_err(|e| AppError::Generic(e.to_string()))?;
    }

    Ok(())
}

// ==========================================
// FFMPEG LOGIC
// ==========================================

pub fn get_local_ffmpeg_path(app: &AppHandle) -> PathBuf {
    let binary_name = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    get_app_dir(app).join(binary_name)
}

pub fn get_ffmpeg_path(app: &AppHandle) -> PathBuf {
    let local_path = get_local_ffmpeg_path(app);

    if local_path.is_file() {
        return local_path;
    }

    let global_name = if cfg!(target_os = "windows") { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(global_path) = which(global_name) {
        return global_path;
    }

    local_path
}

#[tauri::command]
pub async fn check_ffmpeg(app: AppHandle) -> bool {
    get_ffmpeg_path(&app).exists()
}

#[tauri::command]
pub async fn install_ffmpeg(app: AppHandle) -> AppResult<()> {
    let path = get_local_ffmpeg_path(&app);

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Generic(format!("Failed to create data directory: {}", e)))?;
    }

    // FFmpeg static build sources
    let url = if cfg!(target_os = "windows") {
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
    } else if cfg!(target_os = "macos") {
        "https://evermeet.cx/ffmpeg/ffmpeg-116035-gc6435e7280.zip"
    } else {
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
    };

    let client = reqwest::Client::builder()
        .user_agent("MyTauriApp/1.0")
        .build()
        .map_err(|e| AppError::Generic(e.to_string()))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to fetch FFmpeg: {}", e)))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to read response bytes: {}", e)))?;

    // Offload synchronous decompression & extraction to a blocking thread pool
    let path_clone = path.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let cursor = std::io::Cursor::new(bytes);

        // --- WINDOWS & MACOS ZIP EXTRACTION ---
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| AppError::Generic(format!("Failed to parse zip archive: {}", e)))?;

            let mut found = false;
            for i in 0..archive.len() {
                let mut file = archive.by_index(i)
                                      .map_err(|e| AppError::Generic(format!("Failed to read archive entry: {}", e)))?;

                let name = file.name();
                let is_target = if cfg!(target_os = "windows") {
                    name.ends_with("ffmpeg.exe")
                } else {
                    name == "ffmpeg" || name.ends_with("/ffmpeg")
                };

                if is_target && !file.is_dir() {
                    let mut out = std::fs::File::create(&path_clone)
                        .map_err(|e| AppError::Generic(format!("Failed to create destination file: {}", e)))?;
                    std::io::copy(&mut file, &mut out)
                        .map_err(|e| AppError::Generic(format!("Failed to extract file: {}", e)))?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(AppError::Generic("ffmpeg binary not found inside zip archive".to_string()));
            }
        }

        // --- LINUX TAR.XZ EXTRACTION ---
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let decompressor = xz2::read::XzDecoder::new(cursor);
            let mut archive = tar::Archive::new(decompressor);
            let entries = archive.entries()
                                 .map_err(|e| AppError::Generic(format!("Failed to read tar entries: {}", e)))?;

            let mut found = false;
            for entry_result in entries {
                let mut entry = entry_result
                    .map_err(|e| AppError::Generic(format!("Failed to parse tar entry: {}", e)))?;

                let entry_path = entry.path()
                                      .map_err(|e| AppError::Generic(format!("Failed to read tar entry path: {}", e)))?;

                // BtbN Linux builds place the binary at `ffmpeg-xxx/bin/ffmpeg`
                if entry_path.ends_with("ffmpeg") && entry.header().entry_type().is_file() {
                    let mut out = std::fs::File::create(&path_clone)
                        .map_err(|e| AppError::Generic(format!("Failed to create destination file: {}", e)))?;
                    std::io::copy(&mut entry, &mut out)
                        .map_err(|e| AppError::Generic(format!("Failed to extract file: {}", e)))?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(AppError::Generic("ffmpeg binary not found inside tar.xz archive".to_string()));
            }
        }

        Ok(())
    })
        .await
        .map_err(|e| AppError::Generic(format!("Extraction thread panicked: {}", e)))??;

    // Set executable permissions for Linux / macOS
    #[cfg(unix)]
    {
        if path.exists() {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&path)
                .await
                .map_err(|e| AppError::Generic(e.to_string()))?
                .permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&path, perms)
                .await
                .map_err(|e| AppError::Generic(e.to_string()))?;
        }
    }

    Ok(())
}