use crate::core::chat_renderer::args::QualityPreset;
use crate::core::chat_renderer::helpers::{decode_emote_bytes_to_emote_data, guess_ext};
use crate::error::AppError;
use crate::types::AppResult;
use futures::stream::{self, StreamExt};
use lru::LruCache;
use parking_lot::Mutex as PLMutex;
use rustc_hash::{FxHashMap, FxHasher};
use serde::{Deserialize, Serialize};
use skia_safe::{Image, TextBlob};
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

/// Pre-measured layout line for efficient draw loop mapping
#[derive(Clone)]
pub struct LayoutLine {
    pub tokens: Vec<LayoutToken>,
    pub line_height: f32,
    pub total_width: f32,
}
#[derive(Clone)]
pub enum LayoutToken {
    Glyph {
        blob: TextBlob,
        x: f32,
        y: f32,
        width: f32,
    },
    Emote {
        data: Arc<EmoteData>,
        x: f32,
        y: f32,
    },
}

#[derive(Clone)]
pub enum EmoteData {
    Static {
        img: Image,
        w: i32,
        h: i32,
    },
    /// All GIF frames pre-decoded into Skia Images upfront.
    ///
    /// Default when the emote set is small. RAM cost = frames × w × h × 4 bytes.
    /// For a 56-px, 20-frame emote that's ~250 KB per unique GIF emote.
    Animated {
        frames: Arc<[Image]>,
        durations_ms: Arc<[u32]>,
        cum_durations: Arc<[u32]>,
        total_ms: u32,
        w: i32,
        h: i32,
    },
    /// Compressed GIF bytes kept in memory; Skia frames decoded on first access.
    ///
    /// Enabled via `args.eager_gif_decode = false`. Avoids paying the upfront
    /// RAM cost of holding every decoded frame for the entire render job.
    /// Useful for streams with >20 unique animated emotes (can save 200-500 MB).
    ///
    /// `decoded_cache` uses `OnceLock` so only the first render thread to touch
    /// this emote pays the decode cost; all subsequent accesses are lock-free reads.
    LazyGif {
        raw_bytes: Arc<[u8]>,
        cum_durations: Arc<[u32]>,
        total_ms: u32,
        w: i32,
        h: i32,
        target_h: u32,
        alpha_type: skia_safe::AlphaType,
        decoded_cache: Arc<std::sync::OnceLock<Arc<[Image]>>>,
    },
}

impl EmoteData {
    pub fn width(&self) -> i32 {
        match self {
            Self::Static { w, .. } | Self::Animated { w, .. } | Self::LazyGif { w, .. } => *w,
        }
    }

    pub fn height(&self) -> i32 {
        match self {
            Self::Static { h, .. } | Self::Animated { h, .. } | Self::LazyGif { h, .. } => *h,
        }
    }

    #[inline(always)]
    pub fn frame_at(&self, t_ms: u64) -> Option<&Image> {
        match self {
            // Static: unconditional return — no branch on length/total_ms.
            Self::Static { img, .. } => Some(img),

            Self::Animated { frames, cum_durations, total_ms, .. } => {
                if *total_ms == 0 {
                    return frames.first();
                }
                let looped = (t_ms % *total_ms as u64) as u32;
                let idx = cum_durations
                    .partition_point(|&c| c <= looped)
                    .min(frames.len().saturating_sub(1));
                Some(unsafe { frames.get_unchecked(idx) })
            }

            // LazyGif: OnceLock ensures exactly one thread ever decodes the GIF.
            // All subsequent frame_at calls on this emote are a single atomic load.
            Self::LazyGif {
                raw_bytes,
                cum_durations,
                total_ms,
                target_h,
                alpha_type,
                decoded_cache,
                ..
            } => {
                let frames = decoded_cache.get_or_init(|| {
                    crate::core::chat_renderer::helpers::decode_gif_to_skia_frames(
                        raw_bytes,
                        *target_h,
                        *alpha_type,
                    )
                        .unwrap_or_else(|_| Arc::from(vec![]))
                });

                if frames.is_empty() || *total_ms == 0 {
                    return frames.first();
                }
                let looped = (t_ms % *total_ms as u64) as u32;
                let idx = cum_durations
                    .partition_point(|&c| c <= looped)
                    .min(frames.len().saturating_sub(1));
                Some(unsafe { frames.get_unchecked(idx) })
            }
        }
    }
}

/// EmoteCache caches decoded emotes in memory (LRU) and on disk.
pub struct EmoteCache {
    base: PathBuf,
    pub(crate) mem: Arc<PLMutex<LruCache<i32, Arc<EmoteData>>>>,
    inflight: Arc<PLMutex<FxHashMap<i32, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    missing_disk: Arc<PLMutex<LruCache<i32, Instant>>>,
    client: Client,
    target_emote_h: u32,
    quality: QualityPreset,
    /// See `RenderVideoArgs::eager_gif_decode`.
    pub(crate) eager_gif_decode: bool,
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
            quality: self.quality.clone(),
            eager_gif_decode: self.eager_gif_decode,
        }
    }
}

impl EmoteCache {
    pub(crate) fn new(
        base: PathBuf,
        capacity: usize,
        target_emote_h: u32,
        quality: QualityPreset,
        eager_gif_decode: bool,
    ) -> Self {
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
            quality,
            eager_gif_decode,
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

    /// Check local disk for a cached emote file.
    ///
    /// Deliberately synchronous: each stat call takes ~1 µs on a warm dentry
    /// cache. The async overhead of `spawn_blocking` (~10–30 µs) exceeds the
    /// total cost of 5 stat calls, so keeping this sync is strictly faster.
    fn disk_any_path(&self, id: i32) -> Option<PathBuf> {
        if self.is_missing_disk_cached(id) {
            return None;
        }

        for ext in ["png", "gif", "webp", "jpg", "bin"] {
            let p = self.base.join(format!("{}.{}", id, ext));
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

    async fn decode_bytes_rayon(
        bytes: Vec<u8>,
        target_h: u32,
        quality: QualityPreset,
        premultiply: bool,
        eager_gif_decode: bool,
    ) -> AppResult<Arc<EmoteData>> {
        let (tx, rx) = oneshot::channel();
        rayon::spawn(move || {
            let decoded = decode_emote_bytes_to_emote_data(
                &bytes, target_h, premultiply, &quality, eager_gif_decode,
            )
                .map_err(|e| AppError::EmoteCache(e.to_string()))
                .map(Arc::new);
            let _ = tx.send(decoded);
        });
        rx.await
          .map_err(|_| AppError::InternalError("decode task dropped".into()))?
    }

    pub(crate) async fn ensure_cached(&self, ids: &[i32]) -> AppResult<()> {
        tokio::fs::create_dir_all(&self.base).await?;

        if ids.is_empty() {
            return Ok(());
        }

        // Cap concurrent downloads: 8 is a good default for broadband; callers
        // can lower this via args.max_download_concurrency on metered connections.
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

                if let Some(path) = ec.disk_any_path(id) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc =
                        Self::decode_bytes_rayon(bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode).await?;
                    // Persist dimensions so future runs skip re-decoding for sizing.
                    ec.write_sidecar_blocking(id, arc.width(), arc.height());

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

                let _permit = download_sem
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::InternalError("manager semaphore closed".into()))?;

                let url = format!("https://files.kick.com/emotes/{}/fullsize", id);
                let resp = ec
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| AppError::Http(format!("request failed: {}", e)))?;

                if !resp.status().is_success() {
                    let msg = format!("manager failed {}: {}", id, resp.status());
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

                let target_h = ec.target_height();
                let arc =
                    Self::decode_bytes_rayon(bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode).await?;
                let (w, h) = (arc.width(), arc.height());

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

pub struct ImageCache {
    base: PathBuf,
    pub(crate) mem: Arc<PLMutex<LruCache<u64, Arc<EmoteData>>>>,
    inflight: Arc<PLMutex<FxHashMap<u64, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    missing_disk: Arc<PLMutex<LruCache<u64, Instant>>>,
    meta: Arc<PLMutex<LruCache<u64, ImageMetaSidecar>>>,
    client: Client,
    target_emote_h: u32,
    quality: QualityPreset,
    pub(crate) eager_gif_decode: bool,
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
            quality: self.quality.clone(),
            eager_gif_decode: self.eager_gif_decode,
        }
    }
}

impl ImageCache {
    pub(crate) fn new(
        base: PathBuf,
        capacity: usize,
        target_emote_h: u32,
        quality: QualityPreset,
        eager_gif_decode: bool,
    ) -> Self {
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
            quality,
            eager_gif_decode,
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

    /// Synchronous disk probe — see `EmoteCache::disk_any_path` for rationale.
    fn disk_any_path_by_hash(&self, hash: u64) -> Option<PathBuf> {
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

    async fn decode_bytes_rayon(
        bytes: Vec<u8>,
        target_h: u32,
        quality: QualityPreset,
        premultiply: bool,
        eager_gif_decode: bool,
    ) -> AppResult<Arc<EmoteData>> {
        let (tx, rx) = oneshot::channel();
        rayon::spawn(move || {
            let decoded = decode_emote_bytes_to_emote_data(
                &bytes, target_h, premultiply, &quality, eager_gif_decode,
            )
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

                if let Some(path) = ec.disk_any_path_by_hash(key) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc =
                        Self::decode_bytes_rayon(bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode).await?;
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
                    .map_err(|_| AppError::InternalError("manager semaphore closed".into()))?;

                let resp = ec
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| AppError::Http(format!("request failed: {}", e)))?;

                if !resp.status().is_success() {
                    let msg = format!("manager failed {}: {}", url, resp.status());
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
                let arc =
                    Self::decode_bytes_rayon(bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode).await?;
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