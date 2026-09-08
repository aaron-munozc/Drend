use serde::{Deserialize, Serialize};
use skia_safe::{Color, Color4f};

// ─────────────────────────────────────────────────────────────────────────────
// Color
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ObjectColor {
    pub alpha: i32,
    pub red: i32,
    pub green: i32,
    pub blue: i32,
}

impl ObjectColor {
    #[inline(always)]
    pub fn black() -> Self {
        Self {
            alpha: 255,
            red: 20,
            green: 20,
            blue: 20,
        }
    }
    #[inline(always)]
    pub fn white() -> Self {
        Self {
            alpha: 255,
            red: 240,
            green: 240,
            blue: 240,
        }
    }
    #[inline(always)]
    pub fn solid_black() -> Self {
        Self {
            alpha: 255,
            red: 0,
            green: 0,
            blue: 0,
        }
    }
    #[inline(always)]
    pub fn highlight_gold() -> Self {
        Self {
            alpha: 255,
            red: 255,
            green: 215,
            blue: 0,
        }
    }
}

/// Clamp an i32 in [0, 255] to a unit float — no branches on the fast path.
#[inline(always)]
fn to_unit(v: i32) -> f32 {
    v.clamp(0, 255) as f32 * (1.0 / 255.0)
}

impl From<&ObjectColor> for Color4f {
    #[inline(always)]
    fn from(obj: &ObjectColor) -> Self {
        Color4f::new(
            to_unit(obj.red),
            to_unit(obj.green),
            to_unit(obj.blue),
            to_unit(obj.alpha),
        )
    }
}

impl From<&ObjectColor> for Color {
    #[inline(always)]
    fn from(c: &ObjectColor) -> Self {
        Color::from_argb(
            c.alpha.clamp(0, 255) as u8,
            c.red.clamp(0, 255) as u8,
            c.green.clamp(0, 255) as u8,
            c.blue.clamp(0, 255) as u8,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-platform channel identifiers
// ─────────────────────────────────────────────────────────────────────────────

/// Platform-specific channel IDs needed to fetch channel-scoped emotes.
///
/// Fields are `None` when the corresponding platform is not in use.
/// Adding a new platform only requires adding a field here and a matching
/// fetcher in `EmoteNameMap::build_emote_map` — nothing else changes.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelIdentifiers {
    /// Twitch numeric broadcaster ID. Used by 7TV, BTTV, FFZ, and the Twitch
    /// channel-emote endpoint — all four require the same ID format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-platform auth credentials
// ─────────────────────────────────────────────────────────────────────────────

/// Auth credentials for platforms that require API keys or OAuth tokens.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentials {
    /// Twitch OAuth Bearer token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_token: Option<String>,
    /// Twitch application Client-ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_client_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Emote provider feature flags
// ─────────────────────────────────────────────────────────────────────────────

/// Controls which external emote providers are active for a render job.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct EmoteProviderFlags {
    /// Enable Kick native emote tags (`[emote:id:name]` syntax).
    pub kick: bool,
    /// Enable 7TV emotes.
    pub seven_tv: bool,
    /// Enable BetterTTV (BTTV) emotes.
    pub bttv: bool,
    /// Enable FrankerFaceZ (FFZ) emotes.
    pub ffz: bool,
    /// Enable Twitch global and channel emotes.
    pub twitch_global: bool,
}

impl Default for EmoteProviderFlags {
    fn default() -> Self {
        Self {
            kick: true,
            seven_tv: false,
            bttv: false,
            ffz: false,
            twitch_global: false,
        }
    }
}

impl EmoteProviderFlags {
    /// Returns `true` if any word-map emote provider (7TV / BTTV / FFZ / Twitch)
    /// is enabled. Used to short-circuit the hash-map lookup in `push_word`
    /// entirely when all name-based providers are disabled.
    #[inline(always)]
    pub fn any_name_provider_enabled(&self) -> bool {
        self.seven_tv || self.bttv || self.ffz || self.twitch_global
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

/// Controls what the canvas background looks like behind the chat.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundMode {
    /// Fully transparent RGBA output (ProRes 4444 `.mov`).
    Transparent,
    /// Side-by-side luma matte. The canvas width is doubled; FFmpeg reconstructs alpha.
    #[default]
    LumaMatte,
    /// Solid chroma-key green (0, 255, 0).
    ChromaKeyGreen,
    /// Solid fill using `background_color`.
    CustomColor,
}

/// Controls when messages are removed from the visible stack.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum EvictionStrategy {
    /// Messages are pushed off the top edge as new ones arrive — no timer.
    #[default]
    PushOnly,
    /// Messages fade out after `message_hold_seconds` and are then removed.
    Timed,
}

/// Scaling filter used when resizing emote images.
///
/// Applied once at decode time; stored decoded images are never re-filtered.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum QualityPreset {
    /// Nearest-neighbor — fastest decode, no filtering.
    Draft,
    /// Bilinear (triangle) — good balance of speed and quality. **Default.**
    #[default]
    Standard,
    /// Lanczos3 — best quality for photo-style emotes, ~3× slower than `Standard`.
    High,
}

/// What to do when the background video clip outlasts the chat log.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum TimelineMismatchStrategy {
    /// Hold the last rendered chat frame over remaining video frames.
    #[default]
    FreezeLastFrame,
    /// Stop rendering chat; let remaining video frames pass through clean.
    RenderClearCanvas,
    /// Loop the chat timeline from the beginning.
    LoopChatLog,
}

// ─────────────────────────────────────────────────────────────────────────────
// Hot-emote caching policy
// ─────────────────────────────────────────────────────────────────────────────

/// Frequency-aware cache eviction policy for emotes.
///
/// Chat logs have a heavily skewed access distribution: typically 5 % of
/// unique emotes account for 90 %+ of references. A vanilla LRU will thrash
/// on these hot emotes if the cache is slightly smaller than the full emote
/// set because cold emotes will bump hot ones on every cache miss.
///
/// The `HotPin` strategy partitions the LRU into two tiers:
///   - **Hot tier** (`hot_pin_threshold`): emotes referenced ≥ N times are
///     permanently pinned and exempt from LRU eviction.
///   - **Cold tier**: standard LRU for all other emotes.
///
/// Use `Standard` (vanilla LRU) when the emote set is small enough to fit
/// entirely in `max_cached_emotes` — pinning has no benefit then.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum EmoteCachePolicy {
    /// Standard LRU with no pinning (default; good for small emote sets).
    #[default]
    Standard,
    /// Two-tier: pin emotes with reference count ≥ `hot_pin_threshold`.
    ///
    /// The hot tier consumes at most `hot_tier_max_entries` slots from the
    /// total `max_cached_emotes` budget. Pinned entries never occupy evictable
    /// slots, so the cold tier always has room for new arrivals.
    HotPin {
        /// Minimum reference count before an emote is promoted to the hot tier.
        /// A value of 10 is a good starting point for typical Twitch spam logs.
        hot_pin_threshold: u32,
        /// Maximum number of entries in the hot (pinned) tier. Caps memory
        /// even when the log has hundreds of equally-spammed emotes.
        hot_tier_max_entries: usize,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Animated-emote rendering policy
// ─────────────────────────────────────────────────────────────────────────────

/// Controls how animated emotes (GIF / animated WebP) are timed.
///
/// All instances of the same emote on screen share a single wall-clock origin
/// so their frames are always in sync — you never see two copies of KEKW on
/// different frames. The origin is pinned to the render job start time and
/// never drifts.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimatedEmotePolicy {
    /// Target animation frame rate for emotes that do not carry per-frame delay
    /// metadata (i.e. the GIF delay field is 0 or missing).
    ///
    /// Defaults to 24 FPS (≈ 41 ms/frame), which is the project-wide render FPS.
    /// Setting this higher than the render FPS wastes compute — emotes cannot
    /// animate faster than one step per rendered frame.
    pub fallback_fps: u32,

    /// Maximum number of animated emotes allowed to be *visible* simultaneously
    /// before the renderer starts culling low-priority animations.
    ///
    /// Set to `u32::MAX` to disable culling entirely (the default).
    /// Useful on low-end hardware where many simultaneous GIFs spike CPU.
    pub max_concurrent_animated: u32,
}

impl Default for AnimatedEmotePolicy {
    fn default() -> Self {
        Self {
            fallback_fps: 24,
            max_concurrent_animated: u32::MAX,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mid-layer overlay types
// ─────────────────────────────────────────────────────────────────────────────

/// A solid-color rounded rectangle drawn above the background, below chat.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CustomShapeOverlay {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: ObjectColor,
    pub corner_radius: f32,
}

/// An image asset composited above the background, below chat.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CustomImageOverlay {
    /// Absolute path to a PNG / JPEG / WEBP / GIF file on disk.
    pub asset_path: String,
    pub x: f32,
    pub y: f32,
    /// Drawn at the image's native width when `None`.
    pub width: Option<f32>,
    /// Drawn at the image's native height when `None`.
    pub height: Option<f32>,
    /// Opacity in the range 0.0–1.0.
    pub alpha: f32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Viewport / dirty-rect tuning
// ─────────────────────────────────────────────────────────────────────────────

/// Controls dirty-rectangle clipping for partial redraws.
///
/// When enabled, the renderer clips Skia's canvas to only the bounding boxes
/// of animated emotes (via `canvas.clip_rect`) on frames where only animation
/// is changing and the message stack is otherwise identical. This eliminates
/// redundant compositing of static regions and can halve GPU command count on
/// animation-heavy streams.
///
/// For video-export pipelines every frame must be complete, so dirty-rect
/// optimisation is only useful in interactive/preview contexts where frames
/// are displayed directly (not piped to FFmpeg). The engine auto-disables this
/// when `output_path` is set to a file.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirtyRectPolicy {
    /// Enable animated-emote dirty-rect clipping. Default: false (safe for
    /// video export; set `true` only in interactive preview mode).
    pub enabled: bool,

    /// Margin (pixels) added around each animated emote's bounding box before
    /// clipping. A small margin avoids sub-pixel edge artifacts when emotes
    /// are drawn with anti-aliasing. Default: 2.
    pub margin_px: f32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Full configuration for a single chat-render job.
///
/// Fields are grouped into logical sections and ordered so that the most
/// commonly tweaked options appear first.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderVideoArgs {
    // ── Output ───────────────────────────────────────────────────────────────
    /// Destination file path (e.g. `/tmp/chat_overlay.mov`).
    pub output_path: String,

    // ── Canvas ───────────────────────────────────────────────────────────────
    pub width: i32,
    pub height: i32,
    pub fps: u32,
    pub background_mode: BackgroundMode,
    /// Only used when `background_mode == CustomColor`.
    pub background_color: ObjectColor,

    // ── Typography ───────────────────────────────────────────────────────────
    pub font_name: String,
    pub font_size: f32,
    pub line_spacing: i32,

    // ── Layout ───────────────────────────────────────────────────────────────
    pub message_spacing: i32,
    pub padding: i32,

    // ── Message text ─────────────────────────────────────────────────────────
    pub message_color: ObjectColor,
    pub outline_usernames: bool,
    pub username_outline_width: Option<f32>,
    pub username_shadow: bool,

    // ── Bubbles ───────────────────────────────────────────────────────────────
    pub bubble_mode_full_width: bool,
    pub bubble_color: ObjectColor,
    pub bubble_radius: f32,
    pub bubble_padding: i32,

    // ── Entrance animations ───────────────────────────────────────────────────
    pub anim_slide: bool,
    pub anim_fade_in: bool,

    // ── Message lifecycle ─────────────────────────────────────────────────────
    pub eviction_strategy: EvictionStrategy,
    pub message_hold_seconds: u32,
    pub message_fade_out_seconds: u32,

    // ── User management ───────────────────────────────────────────────────────
    pub pinned_users: Vec<String>,
    pub highlight_color: ObjectColor,
    pub pin_duration_secs: u32,
    pub skip_users: Vec<String>,

    // ── Message grouping ──────────────────────────────────────────────────────
    pub group_messages: bool,
    pub group_messages_window_secs: u32,

    // ── Emote providers ───────────────────────────────────────────────────────
    pub emote_providers: EmoteProviderFlags,
    pub channel_ids: ChannelIdentifiers,
    pub provider_credentials: ProviderCredentials,

    // ── Emotes & images ───────────────────────────────────────────────────────
    pub quality_preset: QualityPreset,

    /// Maximum number of decoded emote images held in the LRU cache.
    /// Hot-tier pinned emotes do NOT count against this limit.
    pub max_cached_emotes: usize,

    pub center_emotes_vertically: bool,
    pub create_premultiplied_alpha_emotes: bool,

    /// When `true` (default), GIF frames are fully decoded to Skia Images at
    /// warm-up time. Fastest at render time; uses ~250 KB RAM per unique animated
    /// emote. Set `false` for streams with >20 unique animated emotes to defer
    /// decode to first access via `OnceLock`.
    pub eager_gif_decode: bool,

    // ── Two-tier hot-emote cache ───────────────────────────────────────────────
    /// Cache eviction policy. Use `HotPin` for spam-heavy streams where a small
    /// number of emotes dominate usage. Default: `Standard` (vanilla LRU).
    pub emote_cache_policy: EmoteCachePolicy,

    // ── Animated emote policy ─────────────────────────────────────────────────
    pub animated_emote_policy: AnimatedEmotePolicy,

    // ── Dirty-rect clipping (interactive/preview only) ────────────────────────
    pub dirty_rect_policy: DirtyRectPolicy,

    // ── Time window ───────────────────────────────────────────────────────────
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub time_zero_ms: Option<u64>,

    // ── Base video overlay ────────────────────────────────────────────────────
    pub overlay_video_path: Option<String>,
    pub overlay_x: Option<i32>,
    pub overlay_y: Option<i32>,
    pub overlay_width: Option<i32>,
    pub overlay_height: Option<i32>,

    // ── Pipeline extensions ───────────────────────────────────────────────────
    pub use_immediate_pipe_overlay: bool,
    pub prefill_from_start: bool,
    pub shape_overlays: Vec<CustomShapeOverlay>,
    pub image_overlays: Vec<CustomImageOverlay>,
    pub timeline_mismatch_strategy: TimelineMismatchStrategy,

    // ── CPU / memory tuning ────────────────────────────────────────────────────
    pub max_render_threads: Option<usize>,

    /// Soft RAM budget for decoded/rendered pixel buffers, in MiB.
    pub render_memory_budget_mb: Option<usize>,

    /// Number of rawvideo frames FFmpeg is allowed to queue from stdin.
    pub ffmpeg_input_queue_frames: Option<usize>,

    /// Cap on simultaneous emote/image downloads during cache warm-up.
    pub max_download_concurrency: Option<usize>,

    // ── Arena / bump allocation ────────────────────────────────────────────────
    /// Per-frame arena capacity in bytes. A `bumpalo::Bump` arena of this size
    /// is reset between frames to recycle transient layout allocations without
    /// hitting the system allocator. Default: 512 KiB. Increase if you have
    /// many long messages per frame; decrease on memory-constrained devices.
    pub frame_arena_bytes: usize,
}

impl Default for RenderVideoArgs {
    fn default() -> Self {
        Self {
            output_path: String::new(),
            width: 400,
            height: 800,
            fps: 24,
            background_mode: BackgroundMode::LumaMatte,
            background_color: ObjectColor::black(),
            font_name: "Inter".into(),
            font_size: 20.0,
            line_spacing: 6,
            message_spacing: 12,
            padding: 20,
            message_color: ObjectColor::white(),
            outline_usernames: false,
            username_outline_width: None,
            username_shadow: false,
            bubble_mode_full_width: false,
            bubble_color: ObjectColor::solid_black(),
            bubble_radius: 8.0,
            bubble_padding: 8,
            anim_slide: false,
            anim_fade_in: false,
            eviction_strategy: EvictionStrategy::PushOnly,
            message_hold_seconds: 5,
            message_fade_out_seconds: 2,
            pinned_users: vec![],
            highlight_color: ObjectColor::highlight_gold(),
            pin_duration_secs: 10,
            skip_users: vec!["BotRix".into(), "KickBot".into()],
            group_messages: false,
            group_messages_window_secs: 0,
            emote_providers: EmoteProviderFlags::default(),
            channel_ids: ChannelIdentifiers::default(),
            provider_credentials: ProviderCredentials::default(),
            quality_preset: QualityPreset::Standard,
            max_cached_emotes: 180,
            center_emotes_vertically: true,
            create_premultiplied_alpha_emotes: true,
            eager_gif_decode: true,
            emote_cache_policy: EmoteCachePolicy::default(),
            animated_emote_policy: AnimatedEmotePolicy::default(),
            dirty_rect_policy: DirtyRectPolicy::default(),
            start_ms: None,
            end_ms: None,
            time_zero_ms: None,
            overlay_video_path: None,
            overlay_x: Some(0),
            overlay_y: Some(0),
            overlay_width: None,
            overlay_height: None,
            use_immediate_pipe_overlay: false,
            prefill_from_start: false,
            shape_overlays: vec![],
            image_overlays: vec![],
            timeline_mismatch_strategy: TimelineMismatchStrategy::FreezeLastFrame,
            max_render_threads: None,
            render_memory_budget_mb: Some(384),
            ffmpeg_input_queue_frames: Some(16),
            max_download_concurrency: None,
            frame_arena_bytes: 512 * 1024, // 512 KiB
        }
    }
}
