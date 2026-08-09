use crate::core::chat_renderer::args::EmoteProviderFlags;
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

// --- Twitch Global ---
#[derive(Deserialize)]
struct TwitchGlobalResponse {
    data: Vec<TwitchEmote>,
}
#[derive(Deserialize)]
struct TwitchEmote {
    id: String,
    name: String,
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
/// Stored alongside each `ResolvedEmote` so provider flags can filter at
/// lookup time without a second map lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteProvider {
    SevenTv,
    Bttv,
    Ffz,
    TwitchGlobal,
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
    /// FxHashMap: non-cryptographic hasher ~2× faster than std HashMap for
    /// short string keys, which is the dominant case for emote names.
    map: FxHashMap<String, EmoteEntry>,
}

impl EmoteNameMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetches all requested emotes concurrently.
    /// Requires a shared `reqwest::Client`.
    pub async fn build_emote_map(
        client: &reqwest::Client,
        flags: &EmoteProviderFlags,
        channel_id: &str,
        // twitch_auth: &crate::auth::TwitchAuthManager, // Uncomment when ready
    ) -> AppResult<Self> {
        let mut map = EmoteNameMap::new();

        // Spawn async futures for each enabled provider.
        let seven_tv_fut = async {
            if !flags.seven_tv {
                return vec![];
            }
            Self::fetch_7tv(client, channel_id)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("7TV Fetch Error: {}", e);
                    vec![]
                })
        };

        let bttv_fut = async {
            if !flags.bttv {
                return vec![];
            }
            Self::fetch_bttv(client, channel_id)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("BTTV Fetch Error: {}", e);
                    vec![]
                })
        };

        let ffz_fut = async {
            if !flags.ffz {
                return vec![];
            }
            Self::fetch_ffz(client, channel_id)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("FFZ Fetch Error: {}", e);
                    vec![]
                })
        };

        let twitch_fut = async {
            if !flags.twitch_global {
                return vec![];
            }
            // Uncomment the lines below when TwitchAuthManager is hooked up:
            // Self::fetch_twitch_global(client, twitch_auth).await.unwrap_or_else(|e| {
            //    eprintln!("Twitch Fetch Error: {}", e);
            //    vec![]
            // })
            vec![] // Placeholder
        };

        // Run all network requests simultaneously
        let (seven_tv_emotes, bttv_emotes, ffz_emotes, twitch_emotes) =
            tokio::join!(seven_tv_fut, bttv_fut, ffz_fut, twitch_fut);

        // Populate the map
        if !seven_tv_emotes.is_empty() {
            map.add_7tv(&seven_tv_emotes);
        }
        if !bttv_emotes.is_empty() {
            map.add_bttv(&bttv_emotes);
        }
        if !ffz_emotes.is_empty() {
            map.add_ffz(&ffz_emotes);
        }
        if !twitch_emotes.is_empty() {
            map.add_twitch_global(&twitch_emotes);
        }

        Ok(map)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Internal API Fetchers
    // ─────────────────────────────────────────────────────────────────────────────

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
                // 7TV zero-width flags typically involve bitwise checks on `e.data.flags`
                // Usually, bit 8 (256) indicates zero-width.
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

        let mut emotes = Vec::new();
        // BTTV doesn't expose zero-width nicely via this endpoint, defaulting to false
        for e in res
            .channel_emotes
            .into_iter()
            .chain(res.shared_emotes.into_iter())
        {
            emotes.push((e.code, e.id, false));
        }

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

        let mut emotes = Vec::new();
        for (_, set) in res.sets {
            for e in set.emoticons {
                emotes.push((e.name, e.id.to_string()));
            }
        }

        Ok(emotes)
    }

    /*
    async fn fetch_twitch_global(client: &reqwest::Client, auth: &crate::auth::TwitchAuthManager) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let token = auth.get_token().await?;
        let url = "https://api.twitch.tv/helix/chat/emotes/global";

        let res: TwitchGlobalResponse = client.get(url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Client-Id", &auth.client_id)
            .send().await?.error_for_status()?.json().await?;

        let emotes = res.data.into_iter().map(|e| (e.name, e.id)).collect();
        Ok(emotes)
    }
    */

    // ─────────────────────────────────────────────────────────────────────────────
    // Map Ingestion Handlers
    // ─────────────────────────────────────────────────────────────────────────────

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

    pub fn add_twitch_global(&mut self, entries: &[(String, String)]) {
        self.map.reserve(entries.len());
        for (name, id) in entries {
            self.map.insert(
                name.clone(),
                EmoteEntry {
                    emote: ResolvedEmote {
                        url: Arc::from(
                            format!(
                                "https://static-cdn.jtvnaw.net/emoticons/v2/{}/default/dark/2.0",
                                id
                            )
                                .as_str(),
                        ),
                        zero_width: false,
                    },
                    provider: EmoteProvider::TwitchGlobal,
                },
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Querying
    // ─────────────────────────────────────────────────────────────────────────────

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
            EmoteProvider::TwitchGlobal => flags.twitch_global,
        };
        if allowed {
            Some(entry.emote.clone())
        } else {
            None
        }
    }

    /// Iterate over all resolved emote URLs that pass the given provider flags.
    /// Used during the metadata scan pass to pre-warm the image cache for every
    /// provider emote that could appear in the log.
    pub fn all_urls_filtered<'a>(
        &'a self,
        flags: &'a EmoteProviderFlags,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.map.values().filter_map(move |e| {
            let allowed = match e.provider {
                EmoteProvider::SevenTv => flags.seven_tv,
                EmoteProvider::Bttv => flags.bttv,
                EmoteProvider::Ffz => flags.ffz,
                EmoteProvider::TwitchGlobal => flags.twitch_global,
            };
            if allowed {
                Some(e.emote.url.as_ref())
            } else {
                None
            }
        })
    }

    /// Unconditional iterator — kept for call sites that pre-filter on their
    /// own or that need all URLs regardless of flags (e.g. cache invalidation).
    #[inline]
    pub fn all_urls(&self) -> impl Iterator<Item = &str> {
        self.map.values().map(|e| e.emote.url.as_ref())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageToken — borrowing variant (scan pass)
// ─────────────────────────────────────────────────────────────────────────────

/// Zero-copy token produced during the metadata scan pass.
/// All variants borrow from the original message string so no heap allocation
/// is needed in the hot scan loop.
#[derive(Debug, Clone)]
pub enum MessageToken<'a> {
    Text(&'a str),
    /// Kick platform emote — `id` is the numeric string from the tag.
    KickEmote {
        id: &'a str,
    },
    ProviderEmote(ResolvedEmote),
    ImageUrl(&'a str),
}

// ─────────────────────────────────────────────────────────────────────────────
// OwnedMessageToken — owning variant (layout pass)
// ─────────────────────────────────────────────────────────────────────────────

/// Owned counterpart of [`MessageToken`] used for layout where the original
/// string slice is no longer available. Text variants stores a `Box<str>`
/// instead of `String` to avoid the 3-word overhead on the heap.
#[derive(Debug, Clone)]
pub enum OwnedMessageToken {
    Text(Box<str>),
    KickEmote { id: Box<str> },
    ProviderEmote(ResolvedEmote),
    ImageUrl(Box<str>),
}

impl OwnedMessageToken {
    #[inline(always)]
    pub fn from_borrowed(tok: &MessageToken<'_>) -> Self {
        match tok {
            MessageToken::Text(s) => Self::Text((*s).into()),
            MessageToken::KickEmote { id } => Self::KickEmote { id: (*id).into() },
            MessageToken::ProviderEmote(e) => Self::ProviderEmote(e.clone()),
            MessageToken::ImageUrl(url) => Self::ImageUrl((*url).into()),
        }
    }

    /// Convert a borrow-token vector to an owned-token vector in one pass.
    #[inline]
    pub fn vec_from_borrowed(tokens: &[MessageToken<'_>]) -> Vec<Self> {
        tokens.iter().map(Self::from_borrowed).collect()
    }
}

// No-op stub kept for call-site compatibility.
pub fn clear_token_cache() {}

// ─────────────────────────────────────────────────────────────────────────────
// tokenise  (borrowing — used in the scan pass and layout pass)
// ─────────────────────────────────────────────────────────────────────────────

/// Tokenise `text` into a flat list of [`MessageToken`]s.
///
/// `emote_map` / `flags` should be `Some(…)` when the map is non-empty and
/// name-based providers are enabled. Passing `None` silently skips text-based
/// emote resolution and is correct during the first metadata scan where you
/// only want raw token kinds, not resolved URLs.
///
/// Two cheap `contains` scans gate the regex paths so plain-text messages pay
/// only for whitespace splitting.
///
/// Provider flags are checked per-word so that disabling a provider costs only
/// a single branch in the hot path instead of a pre-filter pass over the map.
pub fn tokenise<'a>(
    text: &'a str,
    emote_map: Option<(&EmoteNameMap, &EmoteProviderFlags)>,
) -> Vec<MessageToken<'a>> {
    let flags_kick = emote_map.map(|(_, f)| f.kick).unwrap_or(true);
    let has_kick = flags_kick && text.contains("[emote:");
    let flags_url = emote_map.map(|(_, f)| f.image_urls).unwrap_or(true);
    let has_url = flags_url && text.contains("http");

    // Fast path: no structured tokens; only word-split for emote lookup.
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
/// Emote-map lookup is attempted for every non-whitespace word.
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