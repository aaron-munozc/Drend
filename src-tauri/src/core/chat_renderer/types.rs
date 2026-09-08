use crate::core::chat_renderer::args::{EmoteCachePolicy, QualityPreset};
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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri_plugin_http::reqwest::Client;
use tokio::sync::{oneshot, Semaphore};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const MISSING_DISK_TTL: Duration = Duration::from_secs(30);

/// Minimum reference count before we even consider atomic frequency tracking.
/// Below this threshold emotes are treated as cold and live in the LRU tier.
const HOT_PROMOTION_HYSTERESIS: u32 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Layout types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ImageMetaSidecar {
    pub w: i32,
    pub h: i32,
}

/// Pre-measured layout line for efficient draw loop mapping.
/// `tokens` is heap-allocated once per layout; never mutated after creation.
#[derive(Clone)]
pub struct LayoutLine {
    pub tokens: Vec<LayoutToken>,
}

#[derive(Clone)]
pub enum LayoutToken {
    Glyph {
        blob: TextBlob,
        x: f32,
        y: f32,
    },
    Emote {
        data: Arc<EmoteData>,
        x: f32,
        y: f32,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// EmoteData — static / animated / lazy variants
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum EmoteData {
    /// A single decoded Skia image. Uploaded once; zero per-frame cost.
    Static {
        img: Image,
        w: i32,
        h: i32,
    },

    /// All GIF frames pre-decoded into Skia Images upfront.
    ///
    /// - `frames`: Arc-wrapped slice — zero-copy clone between threads.
    /// - `cum_durations`: cumulative per-frame delay in ms, used for O(log n)
    ///   binary search in `frame_at`.
    /// - `total_ms`: full animation loop duration; used as the modulus.
    ///
    /// All instances of the same emote share the *same* `Arc<[Image]>` so
    /// no pixel data is duplicated in memory regardless of how many times the
    /// emote appears in the log.
    Animated {
        frames: Arc<[Image]>,
        cum_durations: Arc<[u32]>,
        total_ms: u32,
        w: i32,
        h: i32,
    },

    /// Compressed GIF bytes retained; Skia frames decoded on first access.
    ///
    /// The `OnceLock` guarantees exactly one decode per emote regardless of
    /// racing render threads. All subsequent accesses are a single atomic load.
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
    #[inline(always)]
    pub fn width(&self) -> i32 {
        match self {
            Self::Static { w, .. } | Self::Animated { w, .. } | Self::LazyGif { w, .. } => *w,
        }
    }

    #[inline(always)]
    pub fn height(&self) -> i32 {
        match self {
            Self::Static { h, .. } | Self::Animated { h, .. } | Self::LazyGif { h, .. } => *h,
        }
    }

    /// Select the correct frame for wall-clock offset `t_ms`.
    ///
    /// Uses `partition_point` (binary search) on the cumulative duration slice,
    /// which is O(log n) in frame count. For typical GIFs with ≤ 30 frames
    /// this is 5 comparisons; for static emotes it is a single match arm.
    ///
    /// All emote instances on screen share the same `t_ms` argument (derived
    /// from the same render-job clock), so their animation frames are always
    /// in lock-step.
    #[inline(always)]
    pub fn frame_at(&self, t_ms: u64) -> Option<&Image> {
        match self {
            Self::Static { img, .. } => Some(img),

            Self::Animated { frames, cum_durations, total_ms, .. } => {
                if *total_ms == 0 {
                    return frames.first();
                }
                let looped = (t_ms % *total_ms as u64) as u32;
                // `partition_point` returns the index of the first element
                // strictly greater than `looped`. This gives us the frame
                // whose cumulative start time ≤ looped < next frame's start.
                let idx = cum_durations
                    .partition_point(|&c| c <= looped)
                    .min(frames.len().saturating_sub(1));
                // SAFETY: idx is clamped to frames.len()-1 above.
                Some(unsafe { frames.get_unchecked(idx) })
            }

            Self::LazyGif {
                raw_bytes,
                cum_durations,
                total_ms,
                target_h,
                alpha_type,
                decoded_cache,
                ..
            } => {
                // OnceLock: the first racing thread decodes; all others wait on
                // the lock-free fast path thereafter (single atomic load).
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

// ─────────────────────────────────────────────────────────────────────────────
// Frequency counter — lock-free reference counting for hot-pin promotion
// ─────────────────────────────────────────────────────────────────────────────

/// Shared atomic reference counter for a single emote ID.
///
/// Increment this on every cache hit (not just miss) so the hot tier
/// promotion decision is driven by actual render-loop usage, not just
/// how often the emote appears in the raw chat log.
#[derive(Debug)]
pub struct EmoteFreq {
    pub count: AtomicU32,
}

impl EmoteFreq {
    fn new() -> Self {
        Self { count: AtomicU32::new(1) }
    }
    /// Increment and return the new count. Uses `Relaxed` ordering — we only
    /// need approximate frequency, not strict happens-before ordering.
    #[inline(always)]
    pub fn hit(&self) -> u32 {
        self.count.fetch_add(1, Ordering::Relaxed) + 1
    }
    #[inline(always)]
    pub fn load(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Two-tier EmoteCache
//
// Architecture:
//   HOT tier  — FxHashMap pinned entries; never evicted; O(1) lookup.
//   COLD tier — LruCache; standard eviction when `mem_lru` reaches capacity.
//
// Promotion: on every mem hit for a cold entry, the frequency counter is
// checked. When count ≥ hot_pin_threshold AND hot tier is not full, the entry
// is atomically moved from cold to hot. This happens at most once per emote.
// ─────────────────────────────────────────────────────────────────────────────

pub struct EmoteCache {
    base: PathBuf,

    /// Hot (pinned) tier: entries that have exceeded the frequency threshold.
    /// FxHashMap gives O(1) average probe with no eviction overhead.
    mem_hot: Arc<PLMutex<FxHashMap<i32, Arc<EmoteData>>>>,

    /// Cold (LRU) tier: standard recency-based eviction.
    mem_lru: Arc<PLMutex<LruCache<i32, Arc<EmoteData>>>>,

    /// Per-emote access frequency counters. Shared across cache instances
    /// cloned into different async tasks.
    freq: Arc<PLMutex<FxHashMap<i32, Arc<EmoteFreq>>>>,

    inflight: Arc<PLMutex<FxHashMap<i32, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    missing_disk: Arc<PLMutex<LruCache<i32, Instant>>>,
    client: Client,
    target_emote_h: u32,
    quality: QualityPreset,
    pub(crate) eager_gif_decode: bool,

    // Hot-pin parameters derived from EmoteCachePolicy.
    hot_pin_threshold: u32,
    hot_tier_max: usize,
}

impl Clone for EmoteCache {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            mem_hot: self.mem_hot.clone(),
            mem_lru: self.mem_lru.clone(),
            freq: self.freq.clone(),
            inflight: self.inflight.clone(),
            missing_disk: self.missing_disk.clone(),
            client: self.client.clone(),
            target_emote_h: self.target_emote_h,
            quality: self.quality.clone(),
            eager_gif_decode: self.eager_gif_decode,
            hot_pin_threshold: self.hot_pin_threshold,
            hot_tier_max: self.hot_tier_max,
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
        policy: &EmoteCachePolicy,
    ) -> Self {
        let cap = capacity.max(1);
        let mem_lru = LruCache::new(NonZeroUsize::new(cap).unwrap());
        let miss_cap = NonZeroUsize::new(cap.max(64)).unwrap();

        let (hot_pin_threshold, hot_tier_max) = match policy {
            EmoteCachePolicy::Standard => (u32::MAX, 0),
            EmoteCachePolicy::HotPin { hot_pin_threshold, hot_tier_max_entries } => {
                (*hot_pin_threshold, *hot_tier_max_entries)
            }
        };

        Self {
            base,
            mem_hot: Arc::new(PLMutex::new(FxHashMap::default())),
            mem_lru: Arc::new(PLMutex::new(mem_lru)),
            freq: Arc::new(PLMutex::new(FxHashMap::default())),
            inflight: Arc::new(PLMutex::new(FxHashMap::default())),
            missing_disk: Arc::new(PLMutex::new(LruCache::new(miss_cap))),
            client: Client::new(),
            target_emote_h,
            quality,
            eager_gif_decode,
            hot_pin_threshold,
            hot_tier_max,
        }
    }

    #[inline(always)]
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

    /// O(1) hot-tier probe → O(1) LRU probe.
    ///
    /// Increments the frequency counter on every hit so hot emotes are promoted
    /// automatically without a separate pre-scan pass.
    pub(crate) fn get(&self, id: i32) -> Option<Arc<EmoteData>> {
        // 1. Hot tier — no LRU bookkeeping, no lock contention with eviction.
        {
            let hot = self.mem_hot.lock();
            if let Some(arc) = hot.get(&id) {
                // Still bump frequency so the count stays accurate for logging.
                self.bump_freq(id);
                return Some(arc.clone());
            }
        }

        // 2. Cold LRU tier.
        let arc = {
            let mut lru = self.mem_lru.lock();
            lru.get(&id).cloned()
        }?;

        // 3. Consider promoting to hot tier.
        let count = self.bump_freq(id);
        if count >= self.hot_pin_threshold && count >= HOT_PROMOTION_HYSTERESIS {
            let mut hot = self.mem_hot.lock();
            if hot.len() < self.hot_tier_max && !hot.contains_key(&id) {
                hot.insert(id, arc.clone());
                // Remove from cold tier to reclaim the slot for other emotes.
                let mut lru = self.mem_lru.lock();
                lru.pop(&id);
            }
        }

        Some(arc)
    }

    /// Increment and return the new frequency for `id`.
    #[inline(always)]
    fn bump_freq(&self, id: i32) -> u32 {
        let mut freq_map = self.freq.lock();
        if let Some(f) = freq_map.get(&id) {
            f.hit()
        } else {
            freq_map.insert(id, Arc::new(EmoteFreq::new()));
            1
        }
    }

    /// Insert into the appropriate tier based on current frequency.
    fn insert(&self, id: i32, arc: Arc<EmoteData>) {
        let count = {
            let freq_map = self.freq.lock();
            freq_map.get(&id).map(|f| f.load()).unwrap_or(0)
        };
        if count >= self.hot_pin_threshold && self.hot_tier_max > 0 {
            let mut hot = self.mem_hot.lock();
            if hot.len() < self.hot_tier_max {
                hot.insert(id, arc);
                return;
            }
        }
        let mut lru = self.mem_lru.lock();
        lru.put(id, arc);
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

        // Filter out already-cached IDs before building the async stream.
        let uncached: Vec<i32> = ids
            .iter()
            .copied()
            .filter(|&id| self.get(id).is_none())
            .collect();

        if uncached.is_empty() {
            return Ok(());
        }

        let download_limit = std::cmp::min(8usize, uncached.len().max(1));
        let download_sem = Arc::new(Semaphore::new(download_limit));
        let ec = self.clone();

        let stream = stream::iter(uncached.into_iter()).map(move |id| {
            let ec = ec.clone();
            let download_sem = download_sem.clone();
            async move {
                // Double-check after acquiring the filter above — another task
                // may have populated the cache in the meantime.
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
                    return match rx.await {
                        Ok(Ok(arc)) => { ec.insert(id, arc); Ok(()) }
                        Ok(Err(err)) => Err(AppError::EmoteCache(err)),
                        Err(_) => Err(AppError::InternalError("inflight leader dropped".into())),
                    };
                }

                if let Some(path) = ec.disk_any_path(id) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc = Self::decode_bytes_rayon(
                        bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode,
                    ).await?;
                    ec.write_sidecar_blocking(id, arc.width(), arc.height());
                    ec.notify_inflight(id, arc);
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
                    ec.fail_inflight(id, msg.clone());
                    return Err(AppError::Http(msg));
                }

                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AppError::Http(format!("read bytes: {}", e)))?
                    .to_vec();

                let ext = guess_ext(&bytes);
                let disk_path = ec.disk_path_for(id, ext);
                let tmp = disk_path.with_extension("part");
                tokio::fs::write(&tmp, &bytes).await?;
                tokio::fs::rename(&tmp, &disk_path).await?;

                let target_h = ec.target_height();
                let arc = Self::decode_bytes_rayon(
                    bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode,
                ).await?;
                ec.write_sidecar_blocking(id, arc.width(), arc.height());
                ec.notify_inflight(id, arc);
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

    /// Insert `arc` into the cache and wake all waiters for `id`.
    fn notify_inflight(&self, id: i32, arc: Arc<EmoteData>) {
        self.insert(id, arc.clone());
        let mut infl = self.inflight.lock();
        if let Some(waiters) = infl.remove(&id) {
            for tx in waiters {
                let _ = tx.send(Ok(arc.clone()));
            }
        }
    }

    /// Propagate a failure to all waiters for `id`.
    fn fail_inflight(&self, id: i32, msg: String) {
        let mut infl = self.inflight.lock();
        if let Some(waiters) = infl.remove(&id) {
            for tx in waiters {
                let _ = tx.send(Err(msg.clone()));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Two-tier ImageCache (mirrors EmoteCache for URL-keyed images)
// ─────────────────────────────────────────────────────────────────────────────

pub struct ImageCache {
    base: PathBuf,
    mem_hot: Arc<PLMutex<FxHashMap<u64, Arc<EmoteData>>>>,
    mem_lru: Arc<PLMutex<LruCache<u64, Arc<EmoteData>>>>,
    freq: Arc<PLMutex<FxHashMap<u64, Arc<EmoteFreq>>>>,
    inflight: Arc<PLMutex<FxHashMap<u64, Vec<oneshot::Sender<Result<Arc<EmoteData>, String>>>>>>,
    missing_disk: Arc<PLMutex<LruCache<u64, Instant>>>,
    meta: Arc<PLMutex<LruCache<u64, ImageMetaSidecar>>>,
    client: Client,
    target_emote_h: u32,
    quality: QualityPreset,
    pub(crate) eager_gif_decode: bool,
    hot_pin_threshold: u32,
    hot_tier_max: usize,
}

impl Clone for ImageCache {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            mem_hot: self.mem_hot.clone(),
            mem_lru: self.mem_lru.clone(),
            freq: self.freq.clone(),
            inflight: self.inflight.clone(),
            missing_disk: self.missing_disk.clone(),
            meta: self.meta.clone(),
            client: self.client.clone(),
            target_emote_h: self.target_emote_h,
            quality: self.quality.clone(),
            eager_gif_decode: self.eager_gif_decode,
            hot_pin_threshold: self.hot_pin_threshold,
            hot_tier_max: self.hot_tier_max,
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
        policy: &EmoteCachePolicy,
    ) -> Self {
        let cap = capacity.max(1);
        let mem_lru = LruCache::new(NonZeroUsize::new(cap).unwrap());
        let miss_cap = NonZeroUsize::new(cap.max(64)).unwrap();
        let meta_cap = NonZeroUsize::new(cap.max(256)).unwrap();

        let (hot_pin_threshold, hot_tier_max) = match policy {
            EmoteCachePolicy::Standard => (u32::MAX, 0),
            EmoteCachePolicy::HotPin { hot_pin_threshold, hot_tier_max_entries } => {
                (*hot_pin_threshold, *hot_tier_max_entries)
            }
        };

        Self {
            base,
            mem_hot: Arc::new(PLMutex::new(FxHashMap::default())),
            mem_lru: Arc::new(PLMutex::new(mem_lru)),
            freq: Arc::new(PLMutex::new(FxHashMap::default())),
            inflight: Arc::new(PLMutex::new(FxHashMap::default())),
            missing_disk: Arc::new(PLMutex::new(LruCache::new(miss_cap))),
            meta: Arc::new(PLMutex::new(LruCache::new(meta_cap))),
            client: Client::new(),
            target_emote_h,
            quality,
            eager_gif_decode,
            hot_pin_threshold,
            hot_tier_max,
        }
    }

    #[inline(always)]
    pub(crate) fn target_height(&self) -> u32 {
        self.target_emote_h
    }

    #[inline(always)]
    fn hash_url(&self, url: &str) -> u64 {
        let mut hasher = FxHasher::default();
        url.hash(&mut hasher);
        hasher.finish()
    }

    #[inline(always)]
    fn hash_to_stem(hash: u64) -> String {
        let mut s = String::with_capacity(16);
        let _ = write!(&mut s, "{:016x}", hash);
        s
    }

    fn disk_path_for_hash(&self, hash: u64, ext: &str) -> PathBuf {
        let stem = Self::hash_to_stem(hash);
        self.base.join(format!("{}.{}", stem, ext))
    }

    fn remember_missing_disk(&self, hash: u64) {
        let mut miss = self.missing_disk.lock();
        miss.put(hash, Instant::now() + MISSING_DISK_TTL);
    }

    fn is_missing_disk_cached(&self, hash: u64) -> bool {
        let now = Instant::now();
        let mut miss = self.missing_disk.lock();
        if let Some(&until) = miss.get(&hash) {
            if until > now { return true; }
        }
        miss.pop(&hash);
        false
    }

    fn disk_any_path_by_hash(&self, hash: u64) -> Option<PathBuf> {
        if self.is_missing_disk_cached(hash) { return None; }
        for ext in ["png", "gif", "webp", "jpg", "jpeg", "bin"] {
            let p = self.disk_path_for_hash(hash, ext);
            if p.exists() { return Some(p); }
        }
        self.remember_missing_disk(hash);
        None
    }

    /// Hot-tier probe → LRU probe, with frequency-driven promotion.
    pub(crate) fn get(&self, url: &str) -> Option<Arc<EmoteData>> {
        let key = self.hash_url(url);

        {
            let hot = self.mem_hot.lock();
            if let Some(arc) = hot.get(&key) {
                self.bump_freq(key);
                return Some(arc.clone());
            }
        }

        let arc = {
            let mut lru = self.mem_lru.lock();
            lru.get(&key).cloned()
        }?;

        let count = self.bump_freq(key);
        if count >= self.hot_pin_threshold && count >= HOT_PROMOTION_HYSTERESIS {
            let mut hot = self.mem_hot.lock();
            if hot.len() < self.hot_tier_max && !hot.contains_key(&key) {
                hot.insert(key, arc.clone());
                let mut lru = self.mem_lru.lock();
                lru.pop(&key);
            }
        }

        Some(arc)
    }

    #[inline(always)]
    fn bump_freq(&self, key: u64) -> u32 {
        let mut freq_map = self.freq.lock();
        if let Some(f) = freq_map.get(&key) {
            f.hit()
        } else {
            freq_map.insert(key, Arc::new(EmoteFreq::new()));
            1
        }
    }

    fn insert(&self, key: u64, arc: Arc<EmoteData>) {
        let count = {
            let freq_map = self.freq.lock();
            freq_map.get(&key).map(|f| f.load()).unwrap_or(0)
        };
        if count >= self.hot_pin_threshold && self.hot_tier_max > 0 {
            let mut hot = self.mem_hot.lock();
            if hot.len() < self.hot_tier_max {
                hot.insert(key, arc);
                return;
            }
        }
        let mut lru = self.mem_lru.lock();
        lru.put(key, arc);
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

        if urls.is_empty() { return Ok(()); }

        let uncached: Vec<String> = urls
            .iter()
            .filter(|u| self.get(u).is_none())
            .cloned()
            .collect();

        if uncached.is_empty() { return Ok(()); }

        let download_limit = std::cmp::min(8usize, uncached.len().max(1));
        let download_sem = Arc::new(Semaphore::new(download_limit));
        let ec = self.clone();

        let stream = stream::iter(uncached.into_iter()).map(move |url| {
            let ec = ec.clone();
            let download_sem = download_sem.clone();
            async move {
                let key = ec.hash_url(&url);

                if ec.get(&url).is_some() { return Ok(()); }

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
                    return match rx.await {
                        Ok(Ok(arc)) => { ec.insert(key, arc); Ok(()) }
                        Ok(Err(err)) => Err(AppError::EmoteCache(err)),
                        Err(_) => Err(AppError::InternalError("inflight leader dropped".into())),
                    };
                }

                if let Some(path) = ec.disk_any_path_by_hash(key) {
                    let target_h = ec.target_height();
                    let bytes = tokio::fs::read(&path).await?;
                    let arc = Self::decode_bytes_rayon(
                        bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode,
                    ).await?;
                    let (w, h) = (arc.width(), arc.height());
                    ec.store_sidecar_blocking(key, w, h);
                    ec.notify_inflight(key, arc);
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
                    ec.fail_inflight(key, msg.clone());
                    return Err(AppError::Http(msg));
                }

                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AppError::Http(format!("read bytes: {}", e)))?
                    .to_vec();

                let ext = guess_ext(&bytes);
                let disk_path = ec.disk_path_for_hash(key, ext);
                let tmp = disk_path.with_extension("part");
                tokio::fs::write(&tmp, &bytes).await?;
                tokio::fs::rename(&tmp, &disk_path).await?;

                let target_h = ec.target_height();
                let arc = Self::decode_bytes_rayon(
                    bytes, target_h, ec.quality.clone(), true, ec.eager_gif_decode,
                ).await?;
                let (w, h) = (arc.width(), arc.height());
                ec.store_sidecar_blocking(key, w, h);
                ec.notify_inflight(key, arc);
                Ok(())
            }
        });

        let results: Vec<Result<(), AppError>> =
            stream.buffer_unordered(download_limit).collect().await;
        for r in results { r?; }
        Ok(())
    }

    fn notify_inflight(&self, key: u64, arc: Arc<EmoteData>) {
        self.insert(key, arc.clone());
        let mut infl = self.inflight.lock();
        if let Some(waiters) = infl.remove(&key) {
            for tx in waiters { let _ = tx.send(Ok(arc.clone())); }
        }
    }

    fn fail_inflight(&self, key: u64, msg: String) {
        let mut infl = self.inflight.lock();
        if let Some(waiters) = infl.remove(&key) {
            for tx in waiters { let _ = tx.send(Err(msg.clone())); }
        }
    }
}
