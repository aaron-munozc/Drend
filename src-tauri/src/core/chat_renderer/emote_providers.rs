use crate::core::chat_renderer::args::{ChannelIdentifiers, EmoteProviderFlags, ProviderCredentials};
use crate::core::chat_renderer::regex::{EMOTE_REGEX, IMAGE_URL_REGEX};
use crate::types::AppResult;
use serde::Deserialize;
use rustc_hash::FxHashMap;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// API Response Models
// ─────────────────────────────────────────────────────────────────────────────

// --- 7TV ---
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

// --- BTTV ---
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

// --- FFZ ---
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

// --- Twitch ---
//
// Both the global and channel endpoints return the same response envelope.
// We parse `format` and `theme_mode` to pick the best CDN variant:
//   - Prefer "animated" format over "static" when the emote supports it.
//   - Prefer "dark" theme (most stream overlays use dark backgrounds).
// When the preferred variant is absent we fall back to the static CDN URL
// built from the emote ID, which is always available.
#[derive(Deserialize)]
struct TwitchEmoteResponse {
    data: Vec<TwitchEmote>,
}

#[derive(Deserialize)]
struct TwitchEmote {
    id: String,
    name: String,
    /// e.g. ["static", "animated"]
    #[serde(default)]
    format: Vec<String>,
    /// e.g. ["light", "dark"]
    #[serde(default)]
    theme_mode: Vec<String>,
    /// e.g. ["1.0", "2.0", "3.0"]
    #[serde(default)]
    scale: Vec<String>,
}

impl TwitchEmote {
    /// Build the best CDN URL for this emote.
    ///
    /// Twitch's CDN template is:
    ///   https://static-cdn.jtvnw.net/emoticons/v2/<id>/<format>/<theme>/<scale>
    ///
    /// We always target 2× resolution (scale "2.0") for crisp rendering on
    /// HiDPI displays. Animated GIFs are preferred when available because they
    /// keep chat lively; static PNG is the safe fallback.
    fn cdn_url(&self) -> String {
        let format = if self.format.iter().any(|f| f == "animated") {
            "animated"
        } else {
            "static"
        };

        let theme = if self.theme_mode.iter().any(|t| t == "dark") {
            "dark"
        } else {
            "light"
        };

        // Pick 2.0 if advertised, else whatever Twitch says is available, else
        // hard-code "2.0" anyway (it exists for every emote in practice).
        let scale = if self.scale.iter().any(|s| s == "2.0") {
            "2.0"
        } else {
            self.scale.first().map(|s| s.as_str()).unwrap_or("2.0")
        };

        format!(
            "https://static-cdn.jtvnw.net/emoticons/v2/{}/{}/{}/{}",
            self.id, format, theme, scale
        )
    }

    /// Returns true when this is an animated emote. Used to mark the resolved
    /// emote so the cache can handle GIF vs PNG decode paths correctly.
    #[allow(dead_code)]
    fn is_animated(&self) -> bool {
        self.format.iter().any(|f| f == "animated")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolvedEmote
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
// EmoteProvider — tag each entry with its source
// ─────────────────────────────────────────────────────────────────────────────

/// Which CDN/network a named emote originates from.
///
/// Stored alongside each `ResolvedEmote` so provider flags can filter at
/// lookup time without a second map lookup.
///
/// # Provider priority (insertion order into `EmoteNameMap`)
///
/// When two providers define the same emote name the *last writer wins* in the
/// flat `FxHashMap`. The `build_emote_map` method inserts in this order:
///
///   7TV → BTTV → FFZ → TwitchGlobal → TwitchChannel
///
/// So channel-specific emotes (TwitchChannel, then 7TV/BTTV/FFZ which are
/// always channel-scoped) shadow global ones. Reverse the insertion order if
/// you prefer global emotes to take precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteProvider {
    SevenTv,
    Bttv,
    Ffz,
    /// Twitch built-in global emotes (Kappa, LUL, PogChamp, …).
    TwitchGlobal,
    /// Broadcaster-specific Twitch subscriber / Bits / follower emotes.
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

#[derive(Default, Clone)]
pub struct EmoteNameMap {
    /// Flat map: emote name → (resolved emote, provider tag).
    ///
    /// `FxHashMap` uses a non-cryptographic hasher that is ~2× faster than the
    /// standard library's `SipHash` for the short string keys that dominate
    /// emote name lookups (most emote names are 4–12 characters).
    map: FxHashMap<String, EmoteEntry>,
}

impl EmoteNameMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetches all requested emotes concurrently and builds the name map.
    ///
    /// All network requests are dispatched simultaneously via `tokio::join!`.
    /// Individual provider failures are logged and produce an empty result
    /// rather than propagating an error, so a single unreachable CDN doesn't
    /// abort the entire render.
    ///
    /// # Credentials
    ///
    /// Twitch requires `credentials.twitch_token` and `credentials.twitch_client_id`
    /// when `flags.twitch_global` is `true`. If either is absent or empty the
    /// Twitch future short-circuits before making any network call and logs a
    /// warning — no error is returned.
    ///
    /// 7TV, BTTV, and FFZ require `channel_ids.twitch_id` to fetch
    /// channel-scoped emotes. When it is `None` those fetches are skipped and
    /// a warning is logged.
    ///
    /// # Adding a new platform
    ///
    /// 1. Add a field to `ChannelIdentifiers` and `ProviderCredentials` in `args.rs`.
    /// 2. Add a flag to `EmoteProviderFlags`.
    /// 3. Add a provider variant to `EmoteProvider`.
    /// 4. Write a `fetch_<platform>` async fn and an `add_<platform>` ingestion fn.
    /// 5. Wire them into the `tokio::join!` block below.
    pub async fn build_emote_map(
        client: &reqwest::Client,
        flags: &EmoteProviderFlags,
        channel_ids: &ChannelIdentifiers,
        credentials: &ProviderCredentials,
    ) -> AppResult<Self> {
        let mut map = EmoteNameMap::new();

        // Resolve the Twitch channel ID once — shared by 7TV, BTTV, FFZ, and
        // the Twitch channel-emote endpoint.
        let twitch_id = channel_ids.twitch_id.as_deref().unwrap_or("");

        // ── 7TV ──────────────────────────────────────────────────────────────
        let seven_tv_fut = async {
            if !flags.seven_tv {
                return vec![];
            }
            if twitch_id.is_empty() {
                log::warn!("[emotes] 7TV is enabled but channel_ids.twitch_id is not set — skipping");
                return vec![];
            }
            Self::fetch_7tv(client, twitch_id)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("[emotes] 7TV fetch error: {}", e);
                    vec![]
                })
        };

        // ── BTTV ─────────────────────────────────────────────────────────────
        let bttv_fut = async {
            if !flags.bttv {
                return vec![];
            }
            if twitch_id.is_empty() {
                log::warn!("[emotes] BTTV is enabled but channel_ids.twitch_id is not set — skipping");
                return vec![];
            }
            Self::fetch_bttv(client, twitch_id)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("[emotes] BTTV fetch error: {}", e);
                    vec![]
                })
        };

        // ── FFZ ──────────────────────────────────────────────────────────────
        let ffz_fut = async {
            if !flags.ffz {
                return vec![];
            }
            if twitch_id.is_empty() {
                log::warn!("[emotes] FFZ is enabled but channel_ids.twitch_id is not set — skipping");
                return vec![];
            }
            Self::fetch_ffz(client, twitch_id)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("[emotes] FFZ fetch error: {}", e);
                    vec![]
                })
        };

        // ── Twitch (global + channel) ─────────────────────────────────────────
        //
        // Both endpoints require a Bearer token and a Client-Id header.
        // The caller is responsible for obtaining and refreshing the token;
        // this function only uses whatever it is handed.
        //
        // If twitch_global is disabled the future returns two empty vecs
        // without touching the network, so absent/stale credentials are safe.
        let twitch_fut = async {
            if !flags.twitch_global {
                return (Vec::new(), Vec::new());
            }

            let token = credentials.twitch_token.as_deref().unwrap_or("");
            let client_id = credentials.twitch_client_id.as_deref().unwrap_or("");

            if token.is_empty() || client_id.is_empty() {
                log::warn!(
                    "[emotes] Twitch is enabled but provider_credentials.twitch_token / \
                     twitch_client_id are not set — skipping"
                );
                return (Vec::new(), Vec::new());
            }

            // Run global and channel fetches in parallel — they hit different
            // endpoints and neither depends on the other's result.
            let (global_result, channel_result) = tokio::join!(
                Self::fetch_twitch_global(client, token, client_id),
                Self::fetch_twitch_channel(client, token, client_id, twitch_id),
            );

            let global = global_result.unwrap_or_else(|e| {
                log::warn!("[emotes] Twitch global emote fetch error: {}", e);
                Vec::new()
            });

            let channel = channel_result.unwrap_or_else(|e| {
                log::warn!("[emotes] Twitch channel emote fetch error: {}", e);
                Vec::new()
            });

            (global, channel)
        };

        // ── Dispatch all providers concurrently ───────────────────────────────
        let (seven_tv_emotes, bttv_emotes, ffz_emotes, (twitch_global, twitch_channel)) =
            tokio::join!(seven_tv_fut, bttv_fut, ffz_fut, twitch_fut);

        // ── Populate map (insertion order determines shadowing priority) ───────
        //
        // Priority (last writer wins in the flat FxHashMap):
        //   7TV → BTTV → FFZ → TwitchGlobal → TwitchChannel
        //
        // TwitchChannel is inserted last so a broadcaster's custom emote
        // (e.g. a channel-specific "LUL" variant) shadows the global one.
        if !seven_tv_emotes.is_empty() {
            map.add_7tv(&seven_tv_emotes);
        }
        if !bttv_emotes.is_empty() {
            map.add_bttv(&bttv_emotes);
        }
        if !ffz_emotes.is_empty() {
            map.add_ffz(&ffz_emotes);
        }
        if !twitch_global.is_empty() {
            map.add_twitch(&twitch_global, EmoteProvider::TwitchGlobal);
        }
        if !twitch_channel.is_empty() {
            map.add_twitch(&twitch_channel, EmoteProvider::TwitchChannel);
        }

        Ok(map)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Internal API Fetchers
    // ─────────────────────────────────────────────────────────────────────────

    async fn fetch_7tv(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String, bool)>, reqwest::Error> {
        let url = format!("https://7tv.io/v3/users/twitch/{}", channel_id);
        let res: SevenTvResponse = client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let emotes = res
            .emote_set
            .emotes
            .into_iter()
            .map(|e| {
                // Bit 256 (0x100) in the flags field marks a zero-width emote.
                // Zero-width emotes are composited over the preceding token
                // without advancing the layout cursor.
                let is_zero_width = (e.data.flags & 256) != 0;
                (e.name, e.id, is_zero_width)
            })
            .collect();

        Ok(emotes)
    }

    async fn fetch_bttv(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String, bool)>, reqwest::Error> {
        let url = format!(
            "https://api.betterttv.net/3/cached/users/twitch/{}",
            channel_id
        );
        let res: BttvResponse = client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // BTTV doesn't expose zero-width information via this endpoint.
        // There is no official zero-width flag in the v3 API response, so all
        // BTTV emotes are treated as normal (non-stacking) emotes.
        let emotes = res
            .channel_emotes
            .into_iter()
            .chain(res.shared_emotes)
            .map(|e| (e.code, e.id, false))
            .collect();

        Ok(emotes)
    }

    async fn fetch_ffz(
        client: &reqwest::Client,
        channel_id: &str,
    ) -> Result<Vec<(String, String)>, reqwest::Error> {
        let url = format!("https://api.frankerfacez.com/v1/room/id/{}", channel_id);
        let res: FfzResponse = client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let emotes = res
            .sets
            .into_values()
            .flat_map(|set| set.emoticons)
            .map(|e| (e.name, e.id.to_string()))
            .collect();

        Ok(emotes)
    }

    /// Fetch Twitch **global** emotes (Kappa, LUL, PogChamp, etc.).
    ///
    /// Endpoint: `GET https://api.twitch.tv/helix/chat/emotes/global`
    ///
    /// Returns `TwitchEmote` records with `format`, `theme_mode`, and `scale`
    /// populated so `TwitchEmote::cdn_url()` can pick the best CDN variant.
    async fn fetch_twitch_global(
        client: &reqwest::Client,
        access_token: &str,
        client_id: &str,
    ) -> Result<Vec<TwitchEmote>, reqwest::Error> {
        let res: TwitchEmoteResponse = client
            .get("https://api.twitch.tv/helix/chat/emotes/global")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Client-Id", client_id)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(res.data)
    }

    /// Fetch **channel-specific** Twitch emotes (subscriber, Bits, follower).
    ///
    /// Endpoint: `GET https://api.twitch.tv/helix/chat/emotes?broadcaster_id=<id>`
    ///
    /// These emotes are recognised by name (e.g. `xqcW`, `forsenE`) the same
    /// way as global emotes — Twitch doesn't embed structured tags into chat
    /// messages for either category. Recognition is purely word-based.
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
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(res.data)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Map Ingestion Handlers
    // ─────────────────────────────────────────────────────────────────────────

    pub fn add_7tv(&mut self, entries: &[(String, String, bool)]) {
        self.map.reserve(entries.len());
        for (name, id, zero_width) in entries {
            self.map.insert(
                name.clone(),
                EmoteEntry {
                    emote: ResolvedEmote {
                        url: Arc::from(
                            format!("https://cdn.7tv.app/emote/{}/2x.webp", id).as_str(),
                        ),
                        zero_width: *zero_width,
                    },
                    provider: EmoteProvider::SevenTv,
                },
            );
        }
    }

    pub fn add_bttv(&mut self, entries: &[(String, String, bool)]) {
        self.map.reserve(entries.len());
        for (name, hash, zero_width) in entries {
            self.map.insert(
                name.clone(),
                EmoteEntry {
                    emote: ResolvedEmote {
                        url: Arc::from(
                            format!("https://cdn.betterttv.net/emote/{}/2x", hash).as_str(),
                        ),
                        zero_width: *zero_width,
                    },
                    provider: EmoteProvider::Bttv,
                },
            );
        }
    }

    pub fn add_ffz(&mut self, entries: &[(String, String)]) {
        self.map.reserve(entries.len());
        for (name, id) in entries {
            self.map.insert(
                name.clone(),
                EmoteEntry {
                    emote: ResolvedEmote {
                        // FFZ serves PNG only. "2" is the 2× (56px) scale tier.
                        url: Arc::from(
                            format!("https://cdn.frankerfacez.com/emoticon/{}/2", id).as_str(),
                        ),
                        zero_width: false,
                    },
                    provider: EmoteProvider::Ffz,
                },
            );
        }
    }

    /// Ingest a slice of raw `TwitchEmote` records produced by either
    /// `fetch_twitch_global` or `fetch_twitch_channel`.
    ///
    /// The `provider` argument lets the caller tag entries with the correct
    /// variant (`TwitchGlobal` vs `TwitchChannel`) so the flag filter in
    /// `lookup` can distinguish them if needed in the future. For now both
    /// variants share the `twitch_global` flag — see `EmoteProviderFlags`.
    ///
    /// # CDN URL selection
    ///
    /// `TwitchEmote::cdn_url()` inspects the `format`, `theme_mode`, and
    /// `scale` arrays from the API response to pick the best variant:
    ///
    /// - **Format** — "animated" (GIF) is preferred over "static" (PNG) so
    ///   emotes like PogChamp animate in the overlay.
    /// - **Theme** — "dark" is preferred because stream overlays almost always
    ///   sit on a dark or transparent background.
    /// - **Scale** — "2.0" (56 px) is preferred; falls back to whatever the
    ///   API reports, then hard-codes "2.0" as a last resort (it exists for
    ///   every emote in practice).
    fn add_twitch(&mut self, entries: &[TwitchEmote], provider: EmoteProvider) {
        self.map.reserve(entries.len());
        for emote in entries {
            self.map.insert(
                emote.name.clone(),
                EmoteEntry {
                    emote: ResolvedEmote {
                        url: Arc::from(emote.cdn_url().as_str()),
                        // Twitch has no zero-width emote concept.
                        zero_width: false,
                    },
                    provider,
                },
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Querying
    // ─────────────────────────────────────────────────────────────────────────

    /// Look up a word against the emote map, respecting the active provider
    /// flags. Returns `None` if the word is not an emote or its provider is
    /// disabled.
    #[inline(always)]
    pub fn lookup(&self, word: &str, flags: &EmoteProviderFlags) -> Option<ResolvedEmote> {
        let entry = self.map.get(word)?;
        let allowed = match entry.provider {
            EmoteProvider::SevenTv => flags.seven_tv,
            EmoteProvider::Bttv => flags.bttv,
            EmoteProvider::Ffz => flags.ffz,
            // Both Twitch variants are gated by the same flag for now.
            // Add a dedicated `twitch_channel` flag to `EmoteProviderFlags`
            // if you ever need to enable/disable them independently.
            EmoteProvider::TwitchGlobal | EmoteProvider::TwitchChannel => flags.twitch_global,
        };
        if allowed {
            Some(entry.emote.clone())
        } else {
            None
        }
    }

}

// ─────────────────────────────────────────────────────────────────────────────
// MessageToken — borrowing variant (scan pass)
// ─────────────────────────────────────────────────────────────────────────────

/// Zero-copy token produced during the metadata scan pass.
///
/// All variants borrow from the original message string so no heap allocation
/// is needed in the hot scan loop.
#[derive(Debug, Clone)]
pub enum MessageToken<'a> {
    Text(&'a str),
    /// Kick platform emote — `id` is the numeric string from the `[emote:id:name]` tag.
    KickEmote {
        id: &'a str,
    },
    /// A resolved third-party emote (7TV / BTTV / FFZ / Twitch).
    ///
    /// Twitch emotes land here too: the tokeniser matches them by plain word
    /// (e.g. "LUL", "KEKW", "xqcW") rather than by a structured tag, because
    /// Twitch chat embeds no per-message emote metadata in VOD logs.
    ProviderEmote(ResolvedEmote),
    ImageUrl(&'a str),
}

// No-op stub kept for call-site compatibility.
pub fn clear_token_cache() {}

// ─────────────────────────────────────────────────────────────────────────────
// tokenise  (borrowing — used in the scan pass and layout pass)
// ─────────────────────────────────────────────────────────────────────────────

/// Tokenise `text` into a flat list of [`MessageToken`]s.
///
/// # Emote detection strategies by platform
///
/// **Kick** — Emotes are embedded as structured tags: `[emote:123:KEKW]`.
/// The EMOTE_REGEX captures the numeric ID. No word-map lookup is needed;
/// the Kick CDN URL is constructed from the ID at render time.
///
/// **Twitch / 7TV / BTTV / FFZ** — These providers have no structured tag in
/// VOD chat logs. Emotes appear as plain words (e.g. `LUL`, `KEKW`, `xqcW`,
/// `PauseChamp`). Every whitespace-delimited word is checked against the
/// `EmoteNameMap`. This is case-sensitive and exact-match only — "lul" ≠ "LUL".
///
/// # Parameters
///
/// - `emote_map` — pass `Some((map, flags))` to enable word-based emote lookup.
///   Pass `None` during the first metadata scan to skip resolution and return
///   raw token kinds only.
///
/// # Performance notes
///
/// Two cheap `contains` scans gate the regex paths so plain-text messages pay
/// only for whitespace splitting. Provider flags are checked per-word so that
/// disabling a provider costs a single branch rather than a pre-filter pass
/// over the map.
pub fn tokenise<'a>(
    text: &'a str,
    emote_map: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
) -> Vec<MessageToken<'a>> {
    let flags_kick = emote_map.map(|(_, f)| f.kick).unwrap_or(true);
    let has_kick = flags_kick && text.contains("[emote:");
    let flags_url = emote_map.map(|(_, f)| f.image_urls).unwrap_or(true);
    let has_url = flags_url && text.contains("http");

    // Fast path: no structured tokens — word-split only for emote lookup.
    if !has_kick && !has_url {
        let mut tokens = Vec::with_capacity(8);
        push_text_segment(text, emote_map, &mut tokens);
        return tokens;
    }

    let mut tokens = Vec::with_capacity(16);
    let mut pos = 0usize;

    enum SpanKind<'a> {
        KickEmote(&'a str),
        ImageUrl(&'a str),
    }
    struct Span<'a> {
        start: usize,
        end: usize,
        kind: SpanKind<'a>,
    }

    // Preallocate a small fixed capacity — most messages have ≤ 4 structured tokens.
    let mut spans: Vec<Span<'_>> = Vec::with_capacity(4);

    if has_kick {
        for m in EMOTE_REGEX.captures_iter(text) {
            if let (Some(full), Some(id)) = (m.get(0), m.name("id")) {
                spans.push(Span {
                    start: full.start(),
                    end: full.end(),
                    kind: SpanKind::KickEmote(id.as_str()),
                });
            }
        }
    }

    if has_url {
        for m in IMAGE_URL_REGEX.find_iter(text) {
            spans.push(Span {
                start: m.start(),
                end: m.end(),
                kind: SpanKind::ImageUrl(m.as_str()),
            });
        }
    }

    // Sort by start position so we can linearly scan the string once.
    spans.sort_unstable_by_key(|s| s.start);

    for span in spans {
        if span.start > pos {
            push_text_segment(&text[pos..span.start], emote_map, &mut tokens);
        }
        match span.kind {
            SpanKind::KickEmote(id) => tokens.push(MessageToken::KickEmote { id }),
            SpanKind::ImageUrl(url) => tokens.push(MessageToken::ImageUrl(url)),
        }
        pos = span.end;
    }

    if pos < text.len() {
        push_text_segment(&text[pos..], emote_map, &mut tokens);
    }

    tokens
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Split `seg` on whitespace and push each part as a [`MessageToken`].
///
/// Emote-map lookup is attempted for every non-whitespace word. Single space
/// characters are preserved as `Text(" ")` tokens so the layout pass can
/// measure them without re-splitting. Other whitespace sequences are collapsed.
fn push_text_segment<'a>(
    seg: &'a str,
    map_flags: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
    out: &mut Vec<MessageToken<'a>>,
) {
    let mut last_end = 0;

    for (start, part) in seg.match_indices(|c: char| c.is_whitespace()) {
        let word = &seg[last_end..start];
        if !word.is_empty() {
            push_word(word, map_flags, out);
        }
        // Preserve single spaces as tokens so the layout pass can measure them
        // without re-splitting. Other whitespace (tabs, double-spaces, etc.) is
        // collapsed to a single space to avoid bloating token vectors.
        if part == " " {
            out.push(MessageToken::Text(part));
        } else if !part.trim().is_empty() {
            // Non-space, non-whitespace: treat as a regular word token.
            out.push(MessageToken::Text(part));
        }
        // Pure-whitespace sequences wider than one space are intentionally dropped.
        last_end = start + part.len();
    }

    let tail = &seg[last_end..];
    if !tail.is_empty() {
        push_word(tail, map_flags, out);
    }
}

/// Attempt an emote-map lookup for a single word.
///
/// If the word resolves to an emote it becomes a `ProviderEmote` token;
/// otherwise it becomes a `Text` token. This is the hot path for Twitch /
/// 7TV / BTTV / FFZ emote recognition — no regex, just a hash-map probe.
#[inline(always)]
fn push_word<'a>(
    word: &'a str,
    map_flags: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
    out: &mut Vec<MessageToken<'a>>,
) {
    match map_flags.and_then(|(m, f)| {
        if f.any_name_provider_enabled() {
            m.lookup(word, f)
        } else {
            None
        }
    }) {
        Some(emote) => out.push(MessageToken::ProviderEmote(emote)),
        None => out.push(MessageToken::Text(word)),
    }
}