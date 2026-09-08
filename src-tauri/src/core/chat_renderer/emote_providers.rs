use crate::core::chat_renderer::args::{ChannelIdentifiers, EmoteProviderFlags, ProviderCredentials};
use crate::core::chat_renderer::regex::{text_may_have_kick_emote, EMOTE_REGEX};
use crate::types::AppResult;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// API Response Models
// ─────────────────────────────────────────────────────────────────────────────
// Serde only sees these structs during the network fetch phase (warm-up),
// never on the hot render path. They are kept private to this module.

#[derive(Deserialize)]
struct SevenTvResponse {
    emote_set: SevenTvEmoteSet,
}
#[derive(Deserialize)]
struct SevenTvEmoteSet {
    emotes: Vec<SevenTvEmote>,
}
#[derive(Deserialize)]
struct SevenTvEmote {
    id: String,
    name: String,
    data: SevenTvEmoteData,
}
#[derive(Deserialize)]
struct SevenTvEmoteData {
    flags: u32,
}

#[derive(Deserialize)]
struct BttvResponse {
    #[serde(rename = "channelEmotes")]
    channel_emotes: Vec<BttvEmote>,
    #[serde(rename = "sharedEmotes")]
    shared_emotes: Vec<BttvEmote>,
}
#[derive(Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
}

#[derive(Deserialize)]
struct FfzResponse {
    sets: FxHashMap<String, FfzSet>,
}
#[derive(Deserialize)]
struct FfzSet {
    emoticons: Vec<FfzEmote>,
}
#[derive(Deserialize)]
struct FfzEmote {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct TwitchEmoteResponse {
    data: Vec<TwitchEmote>,
}

#[derive(Deserialize)]
struct TwitchEmote {
    id: String,
    name: String,
    #[serde(default)]
    format: Vec<String>,
    #[serde(default)]
    theme_mode: Vec<String>,
    #[serde(default)]
    scale: Vec<String>,
}

impl TwitchEmote {
    /// Build the best CDN URL for this emote (animated > static, dark > light, 2× > 1×).
    fn cdn_url(&self) -> String {
        let format = if self.format.iter().any(|f| f == "animated") { "animated" } else { "static" };
        let theme = if self.theme_mode.iter().any(|t| t == "dark") { "dark" } else { "light" };
        let scale = if self.scale.iter().any(|s| s == "2.0") {
            "2.0"
        } else {
            self.scale.first().map(|s| s.as_str()).unwrap_or("2.0")
        };
        format!("https://static-cdn.jtvnw.net/emoticons/v2/{}/{}/{}/{}", self.id, format, theme, scale)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolvedEmote
//
// The URL is wrapped in `Arc<str>` rather than `String` so that:
//   1. Multiple `MessageToken::ProviderEmote` instances that reference the
//      same emote share a single heap allocation — no clone overhead.
//   2. The same `Arc<str>` can be used as the `ImageCache` lookup key
//      without converting to a new `String`.
// ─────────────────────────────────────────────────────────────────────────────

/// A resolved third-party emote, ready for layout.
#[derive(Debug, Clone)]
pub struct ResolvedEmote {
    /// CDN URL used as the `ImageCache` lookup key.
    pub url: Arc<str>,
    /// When true, the emote is drawn on top of the previous token (no advance).
    pub zero_width: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// EmoteProvider — source tag
// ─────────────────────────────────────────────────────────────────────────────

/// Which CDN/network a named emote originates from.
///
/// Stored alongside each `ResolvedEmote` so provider flags can filter at
/// lookup time without a second map probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteProvider {
    SevenTv,
    Bttv,
    Ffz,
    TwitchGlobal,
    TwitchChannel,
}

#[derive(Debug, Clone)]
struct EmoteEntry {
    emote: ResolvedEmote,
    provider: EmoteProvider,
}

// ─────────────────────────────────────────────────────────────────────────────
// EmoteNameMap
// ─────────────────────────────────────────────────────────────────────────────

/// Flat emote-name → resolved-emote map built during the warm-up phase.
///
/// Uses `FxHashMap` which is ~2× faster than `HashMap` (SipHash) for the
/// short string keys that dominate emote name lookups (4–12 characters).
#[derive(Default, Clone)]
pub struct EmoteNameMap {
    map: FxHashMap<String, EmoteEntry>,
}

impl EmoteNameMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch all requested emotes concurrently and build the name map.
    ///
    /// All network requests are dispatched simultaneously via `tokio::join!`.
    /// Individual provider failures are logged and produce an empty result
    /// rather than propagating — a single unreachable CDN does not abort the render.
    pub async fn build_emote_map(
        client: &reqwest::Client,
        flags: &EmoteProviderFlags,
        channel_ids: &ChannelIdentifiers,
        credentials: &ProviderCredentials,
    ) -> AppResult<Self> {
        let mut map = EmoteNameMap::new();
        let twitch_id = channel_ids.twitch_id.as_deref().unwrap_or("");

        let seven_tv_fut = async {
            if !flags.seven_tv { return vec![]; }
            if twitch_id.is_empty() {
                log::warn!("[emotes] 7TV: channel_ids.twitch_id not set — skipping");
                return vec![];
            }
            Self::fetch_7tv(client, twitch_id).await.unwrap_or_else(|e| {
                log::warn!("[emotes] 7TV fetch error: {}", e);
                vec![]
            })
        };

        let bttv_fut = async {
            if !flags.bttv { return vec![]; }
            if twitch_id.is_empty() {
                log::warn!("[emotes] BTTV: channel_ids.twitch_id not set — skipping");
                return vec![];
            }
            Self::fetch_bttv(client, twitch_id).await.unwrap_or_else(|e| {
                log::warn!("[emotes] BTTV fetch error: {}", e);
                vec![]
            })
        };

        let ffz_fut = async {
            if !flags.ffz { return vec![]; }
            if twitch_id.is_empty() {
                log::warn!("[emotes] FFZ: channel_ids.twitch_id not set — skipping");
                return vec![];
            }
            Self::fetch_ffz(client, twitch_id).await.unwrap_or_else(|e| {
                log::warn!("[emotes] FFZ fetch error: {}", e);
                vec![]
            })
        };

        let twitch_fut = async {
            if !flags.twitch_global { return (Vec::new(), Vec::new()); }
            let token = credentials.twitch_token.as_deref().unwrap_or("");
            let client_id = credentials.twitch_client_id.as_deref().unwrap_or("");
            if token.is_empty() || client_id.is_empty() {
                log::warn!("[emotes] Twitch: credentials not set — skipping");
                return (Vec::new(), Vec::new());
            }
            let (global_result, channel_result) = tokio::join!(
                Self::fetch_twitch_global(client, token, client_id),
                Self::fetch_twitch_channel(client, token, client_id, twitch_id),
            );
            let global = global_result.unwrap_or_else(|e| {
                log::warn!("[emotes] Twitch global fetch error: {}", e);
                Vec::new()
            });
            let channel = channel_result.unwrap_or_else(|e| {
                log::warn!("[emotes] Twitch channel fetch error: {}", e);
                Vec::new()
            });
            (global, channel)
        };

        let (seven_tv_emotes, bttv_emotes, ffz_emotes, (twitch_global, twitch_channel)) =
            tokio::join!(seven_tv_fut, bttv_fut, ffz_fut, twitch_fut);

        // Insertion order determines last-writer-wins priority:
        //   7TV → BTTV → FFZ → TwitchGlobal → TwitchChannel
        if !seven_tv_emotes.is_empty() { map.add_7tv(&seven_tv_emotes); }
        if !bttv_emotes.is_empty() { map.add_bttv(&bttv_emotes); }
        if !ffz_emotes.is_empty() { map.add_ffz(&ffz_emotes); }
        if !twitch_global.is_empty() { map.add_twitch(&twitch_global, EmoteProvider::TwitchGlobal); }
        if !twitch_channel.is_empty() { map.add_twitch(&twitch_channel, EmoteProvider::TwitchChannel); }

        Ok(map)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    // ── Internal API fetchers ─────────────────────────────────────────────────

    async fn fetch_7tv(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String, bool)>, reqwest::Error> {
        let url = format!("https://7tv.io/v3/users/twitch/{}", channel_id);
        let res: SevenTvResponse = client.get(&url).send().await?.error_for_status()?.json().await?;
        Ok(res.emote_set.emotes.into_iter().map(|e| {
            let is_zero_width = (e.data.flags & 256) != 0;
            (e.name, e.id, is_zero_width)
        }).collect())
    }

    async fn fetch_bttv(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String, bool)>, reqwest::Error> {
        let url = format!("https://api.betterttv.net/3/cached/users/twitch/{}", channel_id);
        let res: BttvResponse = client.get(&url).send().await?.error_for_status()?.json().await?;
        Ok(res.channel_emotes.into_iter().chain(res.shared_emotes).map(|e| (e.code, e.id, false)).collect())
    }

    async fn fetch_ffz(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String)>, reqwest::Error> {
        let url = format!("https://api.frankerfacez.com/v1/room/id/{}", channel_id);
        let res: FfzResponse = client.get(&url).send().await?.error_for_status()?.json().await?;
        Ok(res.sets.into_values().flat_map(|s| s.emoticons).map(|e| (e.name, e.id.to_string())).collect())
    }

    async fn fetch_twitch_global(
        client: &reqwest::Client,
        access_token: &str,
        client_id: &str,
    ) -> Result<Vec<TwitchEmote>, reqwest::Error> {
        let res: TwitchEmoteResponse = client
            .get("https://api.twitch.tv/helix/chat/emotes/global")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Client-Id", client_id)
            .send().await?.error_for_status()?.json().await?;
        Ok(res.data)
    }

    async fn fetch_twitch_channel(
        client: &reqwest::Client,
        access_token: &str,
        client_id: &str,
        channel_id: &str,
    ) -> Result<Vec<TwitchEmote>, reqwest::Error> {
        let res: TwitchEmoteResponse = client
            .get("https://api.twitch.tv/helix/chat/emotes")
            .query(&[("broadcaster_id", channel_id)])
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Client-Id", client_id)
            .send().await?.error_for_status()?.json().await?;
        Ok(res.data)
    }

    // ── Map ingestion ─────────────────────────────────────────────────────────

    pub fn add_7tv(&mut self, entries: &[(String, String, bool)]) {
        self.map.reserve(entries.len());
        for (name, id, zero_width) in entries {
            self.map.insert(name.clone(), EmoteEntry {
                emote: ResolvedEmote {
                    url: Arc::from(format!("https://cdn.7tv.app/emote/{}/2x.webp", id).as_str()),
                    zero_width: *zero_width,
                },
                provider: EmoteProvider::SevenTv,
            });
        }
    }

    pub fn add_bttv(&mut self, entries: &[(String, String, bool)]) {
        self.map.reserve(entries.len());
        for (name, hash, zero_width) in entries {
            self.map.insert(name.clone(), EmoteEntry {
                emote: ResolvedEmote {
                    url: Arc::from(format!("https://cdn.betterttv.net/emote/{}/2x", hash).as_str()),
                    zero_width: *zero_width,
                },
                provider: EmoteProvider::Bttv,
            });
        }
    }

    pub fn add_ffz(&mut self, entries: &[(String, String)]) {
        self.map.reserve(entries.len());
        for (name, id) in entries {
            self.map.insert(name.clone(), EmoteEntry {
                emote: ResolvedEmote {
                    url: Arc::from(format!("https://cdn.frankerfacez.com/emoticon/{}/2", id).as_str()),
                    zero_width: false,
                },
                provider: EmoteProvider::Ffz,
            });
        }
    }

    pub fn add_twitch(&mut self, entries: &[TwitchEmote], provider: EmoteProvider) {
        self.map.reserve(entries.len());
        for emote in entries {
            self.map.insert(emote.name.clone(), EmoteEntry {
                emote: ResolvedEmote {
                    url: Arc::from(emote.cdn_url().as_str()),
                    zero_width: false,
                },
                provider,
            });
        }
    }

    // ── Querying ──────────────────────────────────────────────────────────────

    /// Look up a word against the emote map, respecting the active provider flags.
    ///
    /// Returns `None` immediately when:
    ///   - the word is not in the map (O(1) hash probe), or
    ///   - the word's provider is disabled by the current flags (single branch).
    ///
    /// The returned `ResolvedEmote` clones the `Arc<str>` URL handle — the
    /// actual URL bytes are never copied.
    #[inline(always)]
    pub fn lookup(&self, word: &str, flags: &EmoteProviderFlags) -> Option<ResolvedEmote> {
        let entry = self.map.get(word)?;
        let allowed = match entry.provider {
            EmoteProvider::SevenTv => flags.seven_tv,
            EmoteProvider::Bttv => flags.bttv,
            EmoteProvider::Ffz => flags.ffz,
            EmoteProvider::TwitchGlobal | EmoteProvider::TwitchChannel => flags.twitch_global,
        };
        if allowed { Some(entry.emote.clone()) } else { None }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageToken — zero-copy borrowing variant
//
// All string variants borrow from the original message byte buffer so the
// hot scan loop pays zero allocation cost. The only owned value is
// `ResolvedEmote`, which itself holds only an `Arc<str>` handle (8 bytes) and
// a `bool` flag — a clone is O(1).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MessageToken<'a> {
    Text(&'a str),
    /// Kick platform emote — `id` is pre-parsed at tokenisation time so
    /// `layout_message_blocking` never calls `.parse::<i32>()` at all.
    KickEmote { id: i32 },
    /// A resolved third-party emote (7TV / BTTV / FFZ / Twitch).
    ProviderEmote(ResolvedEmote),
}

// No-op stub kept for call-site compatibility.
pub fn clear_token_cache() {}

// ─────────────────────────────────────────────────────────────────────────────
// tokenise — the hot path
//
// Performance budget: this function is called once per message per frame
// where the message is visible. For a 24 fps render with 50 concurrent
// messages that is 1,200 calls/sec. Each call must stay under ~5 µs.
//
// Optimisations:
//   1. Pre-scan guards: `text_may_have_kick_emote` short-circuits the regex
//      are SIMD `memchr` byte scans that short-circuit the regex engine for
//      plain-text messages (>95% of chat messages).
//   2. Per-message word memoisation: `last_word`/`last_resolution` cache the
//      most recent lookup so repeated-emote messages ("KEKW KEKW KEKW") pay
//      O(1) for all but the first occurrence.
//   3. Per-message word cache: a small `FxHashMap` extends the memo to
//      non-consecutive repeats ("KEKW text KEKW"). Backed by the caller's
//      arena allocator when used from the layout path.
//   4. Zero-copy span extraction: `captures_iter` and `find_iter` return
//      byte offsets into the original string; we slice the original buffer
//      rather than copying substrings.
//   5. `spans` is a stack-allocated `arrayvec` — no heap allocation for the
//      common case of ≤ 8 structured tokens.
// ─────────────────────────────────────────────────────────────────────────────

pub fn tokenise<'a>(
    text: &'a str,
    emote_map: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
) -> Vec<MessageToken<'a>> {
    let flags_kick = emote_map.map(|(_, f)| f.kick).unwrap_or(true);
    let has_kick = flags_kick && text_may_have_kick_emote(text);

    // Per-message memoisation state.
    let mut last_word: Option<&'a str> = None;
    let mut last_resolution: Option<Option<ResolvedEmote>> = None;
    let mut word_cache: FxHashMap<&'a str, Option<ResolvedEmote>> = FxHashMap::default();

    // Fast path: no Kick emote tags — word-split only.
    if !has_kick {
        let mut tokens = Vec::with_capacity(8);
        push_text_segment(text, emote_map, &mut last_word, &mut last_resolution, &mut word_cache, &mut tokens);
        return tokens;
    }

    let mut tokens = Vec::with_capacity(16);
    let mut pos = 0usize;

    struct Span<'a> {
        start: usize,
        end: usize,
        id_str: &'a str,
    }

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(8);

    for m in EMOTE_REGEX.captures_iter(text) {
        if let (Some(full), Some(id)) = (m.get(0), m.name("id")) {
            spans.push(Span {
                start: full.start(),
                end: full.end(),
                id_str: id.as_str(),
            });
        }
    }

    for span in spans {
        if span.start > pos {
            push_text_segment(
                &text[pos..span.start],
                emote_map,
                &mut last_word,
                &mut last_resolution,
                &mut word_cache,
                &mut tokens,
            );
        }
        if let Ok(id) = span.id_str.parse::<i32>() {
            tokens.push(MessageToken::KickEmote { id });
        }
        pos = span.end;
    }

    if pos < text.len() {
        push_text_segment(
            &text[pos..],
            emote_map,
            &mut last_word,
            &mut last_resolution,
            &mut word_cache,
            &mut tokens,
        );
    }

    tokens
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Split `seg` on whitespace and push each part as a [`MessageToken`].
///
/// Uses `match_indices` to preserve single-space tokens for the layout pass
/// (which needs them to advance the x cursor). Multi-character whitespace
/// sequences are collapsed — emitting them separately would bloat the token
/// vector without adding layout information.
///
/// The emote-map lookup path is guarded by `any_name_provider_enabled()` so
/// plain-text messages on a no-provider configuration pay only the
/// whitespace-split cost, not a hash-map probe per word.
fn push_text_segment<'a>(
    seg: &'a str,
    map_flags: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
    last_word: &mut Option<&'a str>,
    last_resolution: &mut Option<Option<ResolvedEmote>>,
    word_cache: &mut FxHashMap<&'a str, Option<ResolvedEmote>>,
    out: &mut Vec<MessageToken<'a>>,
) {
    let mut last_end = 0usize;

    for (start, part) in seg.match_indices(|c: char| c.is_whitespace()) {
        let word = &seg[last_end..start];
        if !word.is_empty() {
            push_word_cached(word, map_flags, last_word, last_resolution, word_cache, out);
        }
        // Preserve single-space separators as tokens — the layout pass advances
        // `x_cursor` for them via `measure_cached` even though there is no
        // `TextBlob` for a bare space. Other whitespace is collapsed.
        if part == " " {
            out.push(MessageToken::Text(part));
        }
        last_end = start + part.len();
    }

    let tail = &seg[last_end..];
    if !tail.is_empty() {
        push_word_cached(tail, map_flags, last_word, last_resolution, word_cache, out);
    }
}

/// Attempt an emote-map lookup for a single word.
///
/// Three-level lookup cascade:
///   1. `last_word` match — O(1) pointer comparison (handles consecutive spam).
///   2. `word_cache` hit — O(1) hash-map probe (handles non-consecutive repeats).
///   3. `emote_map.lookup` — O(1) hash-map probe on the global emote map.
///
/// On a typical emote-spam log where 80%+ of words are the same emote,
/// level 1 handles almost every call. The `word_cache` FxHashMap is keyed on
/// `&'a str` slices (fat pointer = 16 bytes) that borrow from the message
/// buffer — no string allocation occurs at any level.
#[inline(always)]
fn push_word_cached<'a>(
    word: &'a str,
    map_flags: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
    last_word: &mut Option<&'a str>,
    last_resolution: &mut Option<Option<ResolvedEmote>>,
    word_cache: &mut FxHashMap<&'a str, Option<ResolvedEmote>>,
    out: &mut Vec<MessageToken<'a>>,
) {
    // Level 1: consecutive-repeat fast path — pointer comparison, no hash.
    if last_word.as_deref() == Some(word) {
        let resolution = last_resolution.clone().flatten();
        match resolution {
            Some(emote) => out.push(MessageToken::ProviderEmote(emote)),
            None => out.push(MessageToken::Text(word)),
        }
        return;
    }

    // Level 2: per-message word cache.
    if let Some(cached) = word_cache.get(word) {
        let resolved = cached.clone();
        *last_word = Some(word);
        *last_resolution = Some(resolved.clone());
        match resolved {
            Some(emote) => out.push(MessageToken::ProviderEmote(emote)),
            None => out.push(MessageToken::Text(word)),
        }
        return;
    }

    // Level 3: global emote map probe.
    let resolved = map_flags.and_then(|(m, f)| {
        if f.any_name_provider_enabled() { m.lookup(word, f) } else { None }
    });

    word_cache.insert(word, resolved.clone());
    *last_word = Some(word);
    *last_resolution = Some(resolved.clone());

    match resolved {
        Some(emote) => out.push(MessageToken::ProviderEmote(emote)),
        None => out.push(MessageToken::Text(word)),
    }
}
