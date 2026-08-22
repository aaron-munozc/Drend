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
    pub fn black() -> Self {
        Self { alpha: 255, red: 20, green: 20, blue: 20 }
    }
    pub fn white() -> Self {
        Self { alpha: 255, red: 240, green: 240, blue: 240 }
    }
    pub fn solid_black() -> Self {
        Self { alpha: 255, red: 0, green: 0, blue: 0 }
    }
    pub fn highlight_gold() -> Self {
        Self { alpha: 255, red: 255, green: 215, blue: 0 }
    }
}

#[inline(always)]
fn to_unit(v: i32) -> f32 {
    v.clamp(0, 255) as f32 / 255.0
}

impl From<&ObjectColor> for Color4f {
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
/// Each platform uses a different identifier format:
/// - Twitch: numeric user/broadcaster ID (e.g. `"12345678"`)
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

    // Future platforms — uncomment when support is added:
    // /// YouTube channel ID (e.g. `"UCxxxxxx"`). Used by YouTube emoji endpoints.
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub youtube_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-platform auth credentials
// ─────────────────────────────────────────────────────────────────────────────

/// Auth credentials for platforms that require API keys or OAuth tokens.
///
/// All fields are `Option` — leave `None` for any platform that is disabled
/// or that requires no credentials. `build_emote_map` short-circuits cleanly
/// when credentials are absent and logs a warning rather than returning an error.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentials {
    /// Twitch OAuth Bearer token. Required when `EmoteProviderFlags::twitch_global`
    /// is `true`. Obtain via the Client Credentials flow (app access token) — no
    /// user login is needed for emote endpoints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_token: Option<String>,

    /// Twitch application Client-ID. Sent alongside `twitch_token` on every
    /// Twitch Helix API request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_client_id: Option<String>,

    // Future platforms — uncomment when support is added:
    // /// YouTube Data API v3 key. Required when a YouTube emote provider is enabled.
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub youtube_api_key: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Emote provider feature flags
// ─────────────────────────────────────────────────────────────────────────────

/// Controls which external emote providers are active for a render job.
///
/// Each flag independently enables recognition and rendering of that provider's
/// emotes. Disabling a provider means its emote names will be treated as plain
/// text, saving cache space, network bandwidth, and decode CPU for streams that
/// only use a subset of providers.
///
/// All providers are disabled by default — callers must opt in explicitly so
/// unused CDN fetches never happen.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct EmoteProviderFlags {
    /// Enable Kick native emote tags (`[emote:id:name]` syntax in the chat log).
    pub kick: bool,
    /// Enable 7TV emotes resolved from the emote name map.
    pub seven_tv: bool,
    /// Enable BetterTTV (BTTV) emotes resolved from the emote name map.
    pub bttv: bool,
    /// Enable FrankerFaceZ (FFZ) emotes resolved from the emote name map.
    pub ffz: bool,
    /// Enable Twitch global and channel emotes resolved from the emote name map.
    ///
    /// Requires `ProviderCredentials::twitch_token` and `twitch_client_id` to
    /// be set on the render args, and `ChannelIdentifiers::twitch_id` for
    /// channel-scoped emotes. Without credentials only the word-map lookup is
    /// skipped — no error is produced.
    pub twitch_global: bool,
    /// Render bare image URLs (`http(s)://...png|gif|webp|jpg`) found in
    /// message text as inline images.
    pub image_urls: bool,
}

impl Default for EmoteProviderFlags {
    fn default() -> Self {
        Self {
            // Kick native emote tags ([emote:ID:NAME]) are the platform's own
            // format — every Kick VOD log contains them. Defaulting to `false`
            // silently discards all emote-only messages, leaving phantom
            // zero-content bubbles that push other messages without rendering
            // anything. Must be `true` by default for a Kick chat renderer.
            kick: true,
            seven_tv: false,
            bttv: false,
            ffz: false,
            twitch_global: false,
            image_urls: false,
        }
    }
}

impl EmoteProviderFlags {
    /// Returns `true` if any word-map emote provider (7TV / BTTV / FFZ / Twitch)
    /// is enabled. Used to short-circuit the hash-map lookup in `push_word`
    /// entirely when all name-based providers are disabled, so plain-text
    /// messages pay zero map cost.
    ///
    /// Kick is intentionally excluded — it uses structured `[emote:id:name]`
    /// tags, not the word map, and is gated separately in `tokenise`.
    #[inline(always)]
    pub fn any_name_provider_enabled(&self) -> bool {
        self.seven_tv || self.bttv || self.ffz || self.twitch_global
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

/// Controls what the canvas background looks like behind the chat.
///
/// # Choosing the right mode
///
/// | Mode            | Output container | Alpha support | Recommended for                    |
/// |-----------------|------------------|---------------|------------------------------------|
/// | `Transparent`   | `.mov` (ProRes 4444) | True RGBA  | Final compositing in DaVinci / AE  |
/// | `LumaMatte`     | `.mp4`           | Side-car mask | Editors without ProRes; web export |
/// | `ChromaKeyGreen`| `.mp4`           | Keyed in NLE  | Legacy green-screen workflows      |
/// | `CustomColor`   | `.mp4`           | None          | Burned-in captions or preview      |
///
/// **`Transparent`** — emits ProRes 4444 (`yuva444p10le`). True RGBA in a
/// single file. Highest quality, largest file. No GPU required; encoding is
/// done by FFmpeg's software `prores_ks` encoder. Use this when you have an
/// NLE that supports ProRes 4444 (DaVinci Resolve, Final Cut Pro, Premiere).
///
/// **`LumaMatte`** — the canvas is doubled in width: the left half carries the
/// RGB colour pass and the right half carries the luminance alpha mask (white =
/// opaque, black = transparent). A single `filter_complex` in FFmpeg then
/// crops + merges the two halves and overlays them onto the base video. This
/// avoids a separate `.matte` file while remaining fully lossless at the
/// overlay stage. Use this when you need H.264 output but still want clean
/// edges, or when the editing tool can't handle ProRes.
///
/// **`ChromaKeyGreen`** — solid `rgb(0,255,0)` background. Simple to set up
/// in any NLE with a colour-keyer, but suffers from green fringing on
/// anti-aliased text edges. Prefer `LumaMatte` or `Transparent` when possible.
///
/// **`CustomColor`** — opaque fill with `background_color`. Use for
/// burned-in previews where transparency is not needed, or for direct
/// recording-over output where no compositing step follows.
///
/// # Direct overlay (`overlay_video_path`)
///
/// When a base video is provided, `LumaMatte` is the default and recommended
/// mode. The pipeline becomes:
///
/// ```text
/// [base video] ──┐
///                ├─ overlay filter_complex ─► encoded output
/// [chat frames] ─┘   (alpha reconstructed from luma matte)
/// ```
///
/// `Transparent` also works in direct-overlay mode; FFmpeg receives raw BGRA
/// frames and handles the alpha compositing via `alpha=premultiplied`.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundMode {
    /// Fully transparent — output is RGBA with alpha channel (ProRes 4444 `.mov`).
    /// Best quality; largest file. Requires an NLE that supports ProRes 4444.
    Transparent,
    /// Side-by-side luma matte: left half = colour, right half = alpha mask.
    /// The canvas width is automatically doubled; FFmpeg reconstructs the alpha.
    /// Good balance of quality and compatibility with H.264 output.
    #[default]
    LumaMatte,
    /// Solid chroma-key green (0, 255, 0) — for legacy keying workflows.
    /// Produces green fringing on anti-aliased edges; prefer `LumaMatte`.
    ChromaKeyGreen,
    /// Solid fill using `background_color`. No transparency; smallest file.
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
/// Choosing `Draft` costs nothing at render time — it only affects decode
/// quality. For typical stream emote sizes (32–56 px tall) `Standard` is
/// perceptually indistinguishable from `High`.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum QualityPreset {
    /// Nearest-neighbor — fastest decode, no filtering. Fine for pixel-art emotes.
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
    /// System font family name (e.g. `"Inter"`, `"Arial"`).
    pub font_name: String,
    pub font_size: f32,
    /// Extra vertical space (px) added between glyph ascent and descent.
    pub line_spacing: i32,

    // ── Layout ───────────────────────────────────────────────────────────────
    /// Vertical gap (px) between consecutive message bubbles.
    pub message_spacing: i32,
    /// Inset from the canvas edge where bubbles begin.
    pub padding: i32,

    // ── Message text ─────────────────────────────────────────────────────────
    pub message_color: ObjectColor,
    /// Draw a stroke outline behind each username glyph.
    pub outline_usernames: bool,
    /// Stroke width in pixels; defaults to 1.5 when `None`.
    pub username_outline_width: Option<f32>,
    /// Draw a soft drop-shadow behind each username glyph.
    pub username_shadow: bool,

    // ── Bubbles ───────────────────────────────────────────────────────────────
    /// When true, all bubbles stretch to the full canvas width.
    pub bubble_mode_full_width: bool,
    pub bubble_color: ObjectColor,
    /// Corner radius in pixels for the bubble rectangle.
    pub bubble_radius: f32,
    /// Inner padding (px) between the bubble edge and the text.
    pub bubble_padding: i32,

    // ── Entrance animations ───────────────────────────────────────────────────
    /// Slide messages in from the right edge over 500 ms.
    pub anim_slide: bool,
    /// Fade messages in from transparent over 500 ms.
    pub anim_fade_in: bool,

    // ── Message lifecycle ─────────────────────────────────────────────────────
    pub eviction_strategy: EvictionStrategy,
    /// Seconds a message stays fully opaque before fading (Timed only).
    pub message_hold_seconds: u32,
    /// Seconds over which a message fades out (Timed only).
    pub message_fade_out_seconds: u32,

    // ── User management ───────────────────────────────────────────────────────
    /// Usernames whose messages receive the highlight border.
    pub pinned_users: Vec<String>,
    /// Border color used for pinned/highlighted messages.
    pub highlight_color: ObjectColor,
    /// How long (seconds) the highlight border is shown.
    pub pin_duration_secs: u32,
    /// Usernames whose messages are silently dropped (e.g. bots).
    pub skip_users: Vec<String>,

    // ── Message grouping ──────────────────────────────────────────────────────
    /// Merge consecutive messages from the same user into a single bubble.
    pub group_messages: bool,
    /// Maximum gap (seconds) between messages that can be grouped.
    pub group_messages_window_secs: u32,

    // ── Emote providers ───────────────────────────────────────────────────────
    /// Per-provider feature flags. Disable individual providers to skip their
    /// emote resolution, cache population, and network requests entirely.
    pub emote_providers: EmoteProviderFlags,

    /// Platform channel IDs used to fetch channel-scoped emotes (7TV, BTTV,
    /// FFZ, Twitch channel emotes). Required when any name-based provider is
    /// enabled. Ignored — no network call is made — when all flags are off.
    pub channel_ids: ChannelIdentifiers,

    /// OAuth tokens and API keys for platforms that require authentication.
    /// Twitch requires `twitch_token` + `twitch_client_id` when
    /// `emote_providers.twitch_global` is `true`.
    pub provider_credentials: ProviderCredentials,

    // ── Emotes & images ───────────────────────────────────────────────────────
    pub quality_preset: QualityPreset,
    /// Maximum number of decoded emote images held in the in-process LRU cache.
    /// Increase for streams with large emote sets; decrease to save RAM.
    pub max_cached_emotes: usize,
    /// Vertically center emote images within their text line.
    pub center_emotes_vertically: bool,
    /// Decode emotes with premultiplied alpha for faster Skia compositing.
    pub create_premultiplied_alpha_emotes: bool,

    /// When `true` (default), GIF emotes are fully decoded to Skia Images at
    /// cache warm-up time. Fastest at render time; uses ~250 KB RAM per unique
    /// animated emote.
    ///
    /// Set to `false` for streams with many unique animated emotes (>20) to
    /// defer pixel decode to first render access. Saves 200–500 MB RAM at the
    /// cost of a one-time decode stall on first frame that references each emote.
    /// The stall is amortised across the render via `OnceLock` — subsequent
    /// frames for the same emote pay only an atomic load.
    pub eager_gif_decode: bool,

    // ── Time window ───────────────────────────────────────────────────────────
    /// Only process messages after this epoch-ms offset (optional).
    pub start_ms: Option<u64>,
    /// Stop processing after this epoch-ms offset (optional).
    pub end_ms: Option<u64>,
    /// Override the zero-point timestamp for relative offsets (optional).
    pub time_zero_ms: Option<u64>,

    // ── Base video overlay ────────────────────────────────────────────────────
    /// Path to a video file that the chat render is composited onto.
    ///
    /// When set, chat frames are piped directly into FFmpeg's `overlay`
    /// filter without writing a temporary file. The base video audio track
    /// is copied as-is. Set `use_immediate_pipe_overlay = true` to also
    /// skip any intermediate chat video file and composite on-the-fly.
    pub overlay_video_path: Option<String>,
    pub overlay_x: Option<i32>,
    pub overlay_y: Option<i32>,
    /// Scale the chat layer to this width before compositing (optional).
    pub overlay_width: Option<i32>,
    /// Scale the chat layer to this height before compositing (optional).
    pub overlay_height: Option<i32>,

    // ── Pipeline extensions ───────────────────────────────────────────────────
    /// Pipe raw frames directly into FFmpeg stdin, skipping temp files.
    ///
    /// When `true` the background mode is forced to `Transparent` so the
    /// BGRA pixel stream carries full alpha. FFmpeg composites this stream
    /// directly onto `overlay_video_path` without any intermediate file on
    /// disk. This is the lowest-latency and lowest-disk-usage pipeline.
    ///
    /// Requires `overlay_video_path` to be set; ignored otherwise.
    pub use_immediate_pipe_overlay: bool,
    /// Solid-color shapes drawn above the background, below chat.
    /// Pre-fill the chat overlay with messages that would already be on-screen
    /// at frame 0.
    ///
    /// Without this, if the first message in the log arrives at t=7 s the
    /// overlay is blank for the first 7 seconds.  With this enabled, any
    /// message whose timestamp falls within `[time_zero - message_hold_seconds,
    /// time_zero]` is injected at frame 0 as if it had already been on screen
    /// for the appropriate amount of time — hold/eviction/fade-out timers are
    /// adjusted so the message expires at exactly the right moment.  Slide and
    /// fade-in animations are suppressed for pre-filled messages since they
    /// are already "settled" when the video begins.
    ///
    /// Requires `time_zero_ms` (or `start_ms`) to be set so the engine knows
    /// where "frame 0" is in stream time.  Has no effect when `time_zero_ms`
    /// is not set (the default behaviour makes the first message define t=0).
    pub prefill_from_start: bool,
    pub shape_overlays: Vec<CustomShapeOverlay>,
    /// Image assets drawn above the background, below chat.
    pub image_overlays: Vec<CustomImageOverlay>,
    /// How to fill frames when the base video outlasts the chat log.
    pub timeline_mismatch_strategy: TimelineMismatchStrategy,

    // ── CPU tuning ────────────────────────────────────────────────────────────
    /// Cap on the number of rayon worker threads used for frame rendering.
    /// `None` → auto-detect (uses all logical CPUs, capped per quality preset).
    /// Reduce to leave headroom for other processes or to limit RAM use
    /// (each worker allocates its own Skia raster surface).
    pub max_render_threads: Option<usize>,

    /// Cap on simultaneous emote/image downloads during cache warm-up.
    /// Defaults to 8; reduce on metered connections.
    pub max_download_concurrency: Option<usize>,
}

impl Default for RenderVideoArgs {
    fn default() -> Self {
        Self {
            // Output
            output_path: String::new(),

            // Canvas
            width: 400,
            height: 800,
            fps: 24,
            background_mode: BackgroundMode::LumaMatte,
            background_color: ObjectColor::black(),

            // Typography
            font_name: "Inter".into(),
            font_size: 20.0,
            line_spacing: 6,

            // Layout
            message_spacing: 12,
            padding: 20,

            // Message text
            message_color: ObjectColor::white(),
            outline_usernames: false,
            username_outline_width: None,
            username_shadow: false,

            // Bubbles
            bubble_mode_full_width: false,
            bubble_color: ObjectColor::solid_black(),
            bubble_radius: 8.0,
            bubble_padding: 8,

            // Entrance animations
            anim_slide: false,
            anim_fade_in: false,

            // Message lifecycle
            eviction_strategy: EvictionStrategy::PushOnly,
            message_hold_seconds: 5,
            message_fade_out_seconds: 2,

            // User management
            pinned_users: vec![],
            highlight_color: ObjectColor::highlight_gold(),
            pin_duration_secs: 10,
            skip_users: vec!["BotRix".into(), "KickBot".into()],

            // Message grouping
            group_messages: false,
            group_messages_window_secs: 0,

            // Emote providers — all off by default; callers opt in
            emote_providers: EmoteProviderFlags::default(),
            channel_ids: ChannelIdentifiers::default(),
            provider_credentials: ProviderCredentials::default(),

            // Emotes & images
            quality_preset: QualityPreset::Standard,
            max_cached_emotes: 180,
            center_emotes_vertically: true,
            create_premultiplied_alpha_emotes: true,
            eager_gif_decode: true,

            // Time window
            start_ms: None,
            end_ms: None,
            time_zero_ms: None,

            // Base video overlay
            overlay_video_path: None,
            overlay_x: Some(0),
            overlay_y: Some(0),
            overlay_width: None,
            overlay_height: None,

            // Pipeline extensions
            use_immediate_pipe_overlay: false,
            prefill_from_start: false,
            shape_overlays: vec![],
            image_overlays: vec![],
            timeline_mismatch_strategy: TimelineMismatchStrategy::FreezeLastFrame,

            // CPU tuning
            max_render_threads: None,
            max_download_concurrency: None,
        }
    }
}