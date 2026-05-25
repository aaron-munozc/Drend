use crate::core::chat_renderer::helpers::{decode_emote_bytes_to_emote_data, guess_ext};
use crate::error::AppError;
use crate::types::AppResult;
use futures::stream::{self, StreamExt};
use lru::LruCache;
use parking_lot::Mutex as PLMutex;
use rustc_hash::{FxHashMap, FxHasher};
use serde::{Deserialize, Serialize};
use skia_safe::Image;
use std::fmt::Write as FmtWrite;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri_plugin_http::reqwest::Client;
use tokio::sync::{oneshot, Semaphore};

const MISSING_DISK_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ImageMetaSidecar {
    pub w: i32,
    pub h: i32,
}

#[derive(Debug)]
pub enum EmoteData {
    Static {
        img: Image,
        w: i32,
        h: i32,
    },
    Animated {
        frames: Vec<Image>,
        durations_ms: Vec<u32>,
        cum_durations: Vec<u32>,
        total_ms: u32,
        w: i32,
        h: i32,
    },
}

impl EmoteData {
    pub fn width(&self) -> i32 {
        match self {
            Self::Static { w, .. } | Self::Animated { w, .. } => *w,
        }
    }

    pub fn height(&self) -> i32 {
        match self {
            Self::Static { h, .. } | Self::Animated { h, .. } => *h,
        }
    }

    pub fn frame_at(&self, t_ms: u64) -> Option<&Image> {
        match self {
            Self::Static { img, .. } => Some(img),
            Self::Animated {
                frames,
                cum_durations,
                total_ms,
                ..
            } => {
                if frames.is_empty() || *total_ms == 0 {
                    return None;
                }
                let rel_ms = (t_ms % *total_ms as u64) as u32;
                if frames.len() <= 8 {
                    for (idx, &end_ms) in cum_durations.iter().enumerate() {
                        if rel_ms < end_ms {
                            return frames.get(idx);
                        }
                    }
                    return frames.last();
                }
                let idx = match cum_durations.binary_search(&rel_ms) {
                    Ok(i) => i + 1,
                    Err(i) => i,
                };
                frames.get(idx.min(frames.len() - 1))
            }
        }
    }
}

// ... (Keep the rest of your exact EmoteCache and ImageCache implementations here)
// EmoteCache and ImageCache remain exactly as you provided them, as their
// internal LRU/Disk architecture is perfectly sound for the new system.

/// EmoteCache caches decoded emotes in memory (LRU) and on disk.
/// It prevents duplicate concurrent downloads by an in-flight registry.
pub struct EmoteCache {
    base: PathBuf,
    pub(crate) mem: Arc<PLMutex<LruCache<i32, Arc<EmoteData>>>>,
    inflight: Arc<PLMutex<FxHashMap<i32, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    /// Negative cache for disk misses: prevents repeated metadata calls for absent files.
    missing_disk: Arc<PLMutex<LruCache<i32, Instant>>>,
    client: Client,
    target_emote_h: u32,
}

impl Clone for EmoteCache {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            mem: self.mem.clone(),
            inflight: self.inflight.clone(),
            missing_disk: self.missing_disk.clone(),
            client: self.client.clone(),
            target_emote_h: self.target_emote_h,
        }
    }
}

impl EmoteCache {
    pub(crate) fn new(base: PathBuf, capacity: usize, target_emote_h: u32) -> Self {
        let cap = capacity.max(1);
        let mem = LruCache::new(NonZeroUsize::new(cap).unwrap());
        let miss_cap = NonZeroUsize::new(cap.max(64)).unwrap();
        Self {
            base,
            mem: Arc::new(PLMutex::new(mem)),
            inflight: Arc::new(PLMutex::new(FxHashMap::default())),
            missing_disk: Arc::new(PLMutex::new(LruCache::new(miss_cap))),
            client: Client::new(),
            target_emote_h,
        }
    }

    pub(crate) fn target_height(&self) -> u32 {
        self.target_emote_h
    }

    fn disk_path_for(&self, id: i32, ext: &str) -> PathBuf {
        self.base.join(format!("{}.{}", id, ext))
    }

    fn remember_missing_disk(&self, id: i32) {
        let mut miss = self.missing_disk.lock();
        miss.put(id, Instant::now() + MISSING_DISK_TTL);
    }

    fn is_missing_disk_cached(&self, id: i32) -> bool {
        let now = Instant::now();
        let mut miss = self.missing_disk.lock();
        if let Some(&until) = miss.get(&id) {
            if until > now {
                return true;
            }
        }
        miss.pop(&id);
        false
    }

    fn disk_any_path_for_blocking(&self, id: i32) -> Option<PathBuf> {
        if self.is_missing_disk_cached(id) {
            return None;
        }

        for ext in ["png", "gif", "webp", "jpg", "bin"] {
            let p = self.disk_path_for(id, ext);
            if p.exists() {
                return Some(p);
            }
        }

        self.remember_missing_disk(id);
        None
    }

    pub(crate) fn get(&self, id: i32) -> Option<Arc<EmoteData>> {
        let mut m = self.mem.lock();
        m.get(&id).cloned()
    }

    fn sidecar_path_for(&self, id: i32) -> PathBuf {
        self.base.join(format!("{}.meta.json", id))
    }

    fn write_sidecar_blocking(&self, id: i32, w: i32, h: i32) {
        let sidecar = self.sidecar_path_for(id);
        let meta = ImageMetaSidecar { w, h };
        if let Ok(bytes) = serde_json::to_vec(&meta) {
            let _ = std::fs::write(sidecar, bytes);
        }
    }

    async fn decode_bytes_rayon(bytes: Vec<u8>, target_h: u32) -> AppResult<Arc<EmoteData>> {
        let (tx, rx) = oneshot::channel();
        rayon::spawn(move || {
            let decoded = decode_emote_bytes_to_emote_data(&bytes, target_h)
                .map_err(|e| AppError::EmoteCache(e.to_string()))
                .map(Arc::new);
            let _ = tx.send(decoded);
        });
        rx.await
            .map_err(|_| AppError::InternalError("decode task dropped".into()))?
    }

    /// Ensure the given emote ids are cached (disk or memory).
    ///
    /// - Uses a semaphore only for the download stage.
    /// - Uses Rayon for decode so CPU-bound work spreads across cores.
    /// - Uses a negative disk cache to avoid repeated metadata misses.
    pub(crate) async fn ensure_cached(&self, ids: &[i32]) -> AppResult<()> {
        tokio::fs::create_dir_all(&self.base).await?;

        if ids.is_empty() {
            return Ok(());
        }

        let download_limit = std::cmp::min(8usize, ids.len().max(1));
        let download_sem = Arc::new(Semaphore::new(download_limit));
        let ec = self.clone();

        let stream = stream::iter(ids.iter().copied()).map(move |id| {
            let ec = ec.clone();
            let download_sem = download_sem.clone();
            async move {
                if ec.get(id).is_some() {
                    return Ok(());
                }

                let rx_opt = {
                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.get_mut(&id) {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        Some(rx)
                    } else {
                        infl.insert(id, Vec::new());
                        None
                    }
                };

                if let Some(rx) = rx_opt {
                    match rx.await {
                        Ok(Ok(arc)) => {
                            let mut m = ec.mem.lock();
                            m.put(id, arc);
                            return Ok(());
                        }
                        Ok(Err(err)) => return Err(AppError::EmoteCache(err)),
                        Err(_) => {
                            return Err(AppError::InternalError("inflight leader dropped".into()))
                        }
                    }
                }

                // Disk path lookup is cache-aware and only hits metadata when needed.
                if let Some(path) = ec.disk_any_path_for_blocking(id) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc = Self::decode_bytes_rayon(bytes, target_h).await?;

                    {
                        let mut m = ec.mem.lock();
                        m.put(id, arc.clone());
                    }

                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.remove(&id) {
                        for tx in waiters {
                            let _ = tx.send(Ok(arc.clone()));
                        }
                    }

                    return Ok(());
                }

                // Download stage only: semaphore does not cover decode.
                let _permit = download_sem
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::InternalError("download semaphore closed".into()))?;

                let url = format!("https://files.kick.com/emotes/{}/fullsize", id);
                let resp = ec
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| AppError::Http(format!("request failed: {}", e)))?;

                if !resp.status().is_success() {
                    let msg = format!("download failed {}: {}", id, resp.status());
                    ec.remember_missing_disk(id);
                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.remove(&id) {
                        for tx in waiters {
                            let _ = tx.send(Err(msg.clone()));
                        }
                    }
                    return Err(AppError::Http(msg));
                }

                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AppError::Http(format!("read bytes: {}", e)))?
                    .to_vec();

                let ext = guess_ext(&bytes);
                let disk_path = ec.disk_path_for(id, &ext);
                let tmp = disk_path.with_extension("part");

                tokio::fs::write(&tmp, &bytes).await?;
                tokio::fs::rename(&tmp, &disk_path).await?;

                // Decode is intentionally outside the download semaphore.
                let target_h = ec.target_height();
                let arc = Self::decode_bytes_rayon(bytes, target_h).await?;
                let (w, h) = (arc.width(), arc.height());

                // Best-effort sidecar for future fast metadata reads.
                ec.write_sidecar_blocking(id, w, h);

                {
                    let mut m = ec.mem.lock();
                    m.put(id, arc.clone());
                }

                let mut infl = ec.inflight.lock();
                if let Some(waiters) = infl.remove(&id) {
                    for tx in waiters {
                        let _ = tx.send(Ok(arc.clone()));
                    }
                }

                Ok(())
            }
        });

        let results: Vec<Result<(), AppError>> =
            stream.buffer_unordered(download_limit).collect().await;
        for r in results {
            r?;
        }

        Ok(())
    }
}

/// ImageCache mirrors EmoteCache, but uses a stable u64 content hash key internally.
/// This avoids hashing the full URL string repeatedly for the in-memory hot path.
pub struct ImageCache {
    base: PathBuf,
    pub(crate) mem: Arc<PLMutex<LruCache<u64, Arc<EmoteData>>>>,
    inflight: Arc<PLMutex<FxHashMap<u64, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    missing_disk: Arc<PLMutex<LruCache<u64, Instant>>>,
    /// Optional small metadata cache: lets callers query size without forcing decode.
    meta: Arc<PLMutex<LruCache<u64, ImageMetaSidecar>>>,
    client: Client,
    target_emote_h: u32,
}

impl Clone for ImageCache {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            mem: self.mem.clone(),
            inflight: self.inflight.clone(),
            missing_disk: self.missing_disk.clone(),
            meta: self.meta.clone(),
            client: self.client.clone(),
            target_emote_h: self.target_emote_h,
        }
    }
}

impl ImageCache {
    pub(crate) fn new(base: PathBuf, capacity: usize, target_emote_h: u32) -> Self {
        let cap = capacity.max(1);
        let mem = LruCache::new(NonZeroUsize::new(cap).unwrap());
        let miss_cap = NonZeroUsize::new(cap.max(64)).unwrap();
        let meta_cap = NonZeroUsize::new(cap.max(256)).unwrap();
        Self {
            base,
            mem: Arc::new(PLMutex::new(mem)),
            inflight: Arc::new(PLMutex::new(FxHashMap::default())),
            missing_disk: Arc::new(PLMutex::new(LruCache::new(miss_cap))),
            meta: Arc::new(PLMutex::new(LruCache::new(meta_cap))),
            client: Client::new(),
            target_emote_h,
        }
    }

    pub(crate) fn target_height(&self) -> u32 {
        self.target_emote_h
    }

    #[inline]
    fn hash_url(&self, url: &str) -> u64 {
        let mut hasher = FxHasher::default();
        url.hash(&mut hasher);
        hasher.finish()
    }

    #[inline]
    fn hash_to_stem(hash: u64) -> String {
        // Stable and compact; avoids the extra churn of decimal string conversion.
        let mut s = String::with_capacity(16);
        let _ = write!(&mut s, "{:016x}", hash);
        s
    }

    fn disk_path_for_hash(&self, hash: u64, ext: &str) -> PathBuf {
        let stem = Self::hash_to_stem(hash);
        self.base.join(format!("{}.{}", stem, ext))
    }

    fn disk_path_for(&self, url: &str, ext: &str) -> PathBuf {
        self.disk_path_for_hash(self.hash_url(url), ext)
    }

    #[inline]
    fn remember_missing_disk(&self, hash: u64) {
        let mut miss = self.missing_disk.lock();
        miss.put(hash, Instant::now() + MISSING_DISK_TTL);
    }

    #[inline]
    fn is_missing_disk_cached(&self, hash: u64) -> bool {
        let now = Instant::now();
        let mut miss = self.missing_disk.lock();
        if let Some(&until) = miss.get(&hash) {
            if until > now {
                return true;
            }
        }
        miss.pop(&hash);
        false
    }

    fn disk_any_path_async_by_hash(&self, hash: u64) -> Option<PathBuf> {
        if self.is_missing_disk_cached(hash) {
            return None;
        }

        for ext in ["png", "gif", "webp", "jpg", "jpeg", "bin"] {
            let p = self.disk_path_for_hash(hash, ext);
            if p.exists() {
                return Some(p);
            }
        }

        self.remember_missing_disk(hash);
        None
    }

    pub(crate) fn get(&self, url: &str) -> Option<Arc<EmoteData>> {
        let key = self.hash_url(url);
        let mut m = self.mem.lock();
        m.get(&key).cloned()
    }

    /// Fast metadata path: callers can query width/height without requiring a decoded image.
    pub(crate) fn peek_dimensions(&self, url: &str) -> Option<(i32, i32)> {
        let key = self.hash_url(url);
        {
            let mut meta = self.meta.lock();
            if let Some(v) = meta.get(&key) {
                return Some((v.w, v.h));
            }
        }

        let sidecar_stem = Self::hash_to_stem(key);
        let sidecar = self.base.join(format!("{}.meta.json", sidecar_stem));
        let bytes = std::fs::read(sidecar).ok()?;
        let parsed: ImageMetaSidecar = serde_json::from_slice(&bytes).ok()?;
        {
            let mut meta = self.meta.lock();
            meta.put(key, parsed);
        }
        Some((parsed.w, parsed.h))
    }

    fn store_sidecar_blocking(&self, hash: u64, w: i32, h: i32) {
        let sidecar_stem = Self::hash_to_stem(hash);
        let sidecar = self.base.join(format!("{}.meta.json", sidecar_stem));
        let meta = ImageMetaSidecar { w, h };
        if let Ok(bytes) = serde_json::to_vec(&meta) {
            let _ = std::fs::write(sidecar, bytes);
        }
        let mut cache = self.meta.lock();
        cache.put(hash, meta);
    }

    async fn decode_bytes_rayon(bytes: Vec<u8>, target_h: u32) -> AppResult<Arc<EmoteData>> {
        let (tx, rx) = oneshot::channel();
        rayon::spawn(move || {
            let decoded = decode_emote_bytes_to_emote_data(&bytes, target_h)
                .map_err(|e| AppError::EmoteCache(e.to_string()))
                .map(Arc::new);
            let _ = tx.send(decoded);
        });
        rx.await
            .map_err(|_| AppError::InternalError("decode task dropped".into()))?
    }

    pub(crate) async fn ensure_cached(&self, urls: &[String]) -> AppResult<()> {
        tokio::fs::create_dir_all(&self.base).await?;

        if urls.is_empty() {
            return Ok(());
        }

        let download_limit = std::cmp::min(8usize, urls.len().max(1));
        let download_sem = Arc::new(Semaphore::new(download_limit));
        let ec = self.clone();

        let stream = stream::iter(urls.iter().cloned()).map(move |url| {
            let ec = ec.clone();
            let download_sem = download_sem.clone();
            async move {
                let key = ec.hash_url(&url);

                if ec.get(&url).is_some() {
                    return Ok(());
                }

                let rx_opt = {
                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.get_mut(&key) {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        Some(rx)
                    } else {
                        infl.insert(key, Vec::new());
                        None
                    }
                };

                if let Some(rx) = rx_opt {
                    match rx.await {
                        Ok(Ok(arc)) => {
                            let mut m = ec.mem.lock();
                            m.put(key, arc);
                            return Ok(());
                        }
                        Ok(Err(err)) => return Err(AppError::EmoteCache(err)),
                        Err(_) => {
                            return Err(AppError::InternalError("inflight leader dropped".into()))
                        }
                    }
                }

                if let Some(path) = ec.disk_any_path_async_by_hash(key) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc = Self::decode_bytes_rayon(bytes, target_h).await?;
                    let (w, h) = (arc.width(), arc.height());
                    ec.store_sidecar_blocking(key, w, h);

                    {
                        let mut m = ec.mem.lock();
                        m.put(key, arc.clone());
                    }

                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.remove(&key) {
                        for tx in waiters {
                            let _ = tx.send(Ok(arc.clone()));
                        }
                    }

                    return Ok(());
                }

                let _permit = download_sem
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::InternalError("download semaphore closed".into()))?;

                let resp = ec
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| AppError::Http(format!("request failed: {}", e)))?;

                if !resp.status().is_success() {
                    let msg = format!("download failed {}: {}", url, resp.status());
                    ec.remember_missing_disk(key);
                    let mut infl = ec.inflight.lock();
                    if let Some(waiters) = infl.remove(&key) {
                        for tx in waiters {
                            let _ = tx.send(Err(msg.clone()));
                        }
                    }
                    return Err(AppError::Http(msg));
                }

                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AppError::Http(format!("read bytes: {}", e)))?
                    .to_vec();

                let ext = guess_ext(&bytes);
                let disk_path = ec.disk_path_for_hash(key, &ext);
                let tmp = disk_path.with_extension("part");

                tokio::fs::write(&tmp, &bytes).await?;
                tokio::fs::rename(&tmp, &disk_path).await?;

                let target_h = ec.target_height();
                let arc = Self::decode_bytes_rayon(bytes, target_h).await?;
                let (w, h) = (arc.width(), arc.height());
                ec.store_sidecar_blocking(key, w, h);

                {
                    let mut m = ec.mem.lock();
                    m.put(key, arc.clone());
                }

                let mut infl = ec.inflight.lock();
                if let Some(waiters) = infl.remove(&key) {
                    for tx in waiters {
                        let _ = tx.send(Ok(arc.clone()));
                    }
                }

                Ok(())
            }
        });

        let results: Vec<Result<(), AppError>> =
            stream.buffer_unordered(download_limit).collect().await;
        for r in results {
            r?;
        }

        Ok(())
    }
}
