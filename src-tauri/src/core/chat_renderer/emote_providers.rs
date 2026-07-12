use crate::core::chat_renderer::regex::{EMOTE_REGEX, IMAGE_URL_REGEX};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmoteProvider {
    Kick,
    Twitch,
    Bttv,
    Ffz,
    SevenTv,
}

#[derive(Debug, Clone)]
pub struct ResolvedEmote {
    pub provider: EmoteProvider,
    pub id: String,
    pub name: String,
    pub url: String,
    pub zero_width: bool,
}

#[derive(Default)]
pub struct EmoteNameMap {
    map: HashMap<String, ResolvedEmote>,
}

impl EmoteNameMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_twitch_global(&mut self, entries: &[(String, String)]) {
        for (name, id) in entries {
            let url = format!(
                "https://static-cdn.jtvnw.net/emoticons/v2/{}/default/dark/2.0",
                id
            );
            self.map.insert(
                name.clone(),
                ResolvedEmote {
                    provider: EmoteProvider::Twitch,
                    id: id.clone(),
                    name: name.clone(),
                    url,
                    zero_width: false,
                },
            );
        }
    }

    pub fn add_bttv(&mut self, entries: &[(String, String, bool)]) {
        for (name, hash, zero_width) in entries {
            let url = format!("https://cdn.betterttv.net/emote/{}/2x", hash);
            self.map.insert(
                name.clone(),
                ResolvedEmote {
                    provider: EmoteProvider::Bttv,
                    id: hash.clone(),
                    name: name.clone(),
                    url,
                    zero_width: *zero_width,
                },
            );
        }
    }

    pub fn add_ffz(&mut self, entries: &[(String, String)]) {
        for (name, id) in entries {
            let url = format!("https://cdn.frankerfacez.com/emoticon/{}/2", id);
            self.map.insert(
                name.clone(),
                ResolvedEmote {
                    provider: EmoteProvider::Ffz,
                    id: id.clone(),
                    name: name.clone(),
                    url,
                    zero_width: false,
                },
            );
        }
    }

    pub fn add_7tv(&mut self, entries: &[(String, String, bool)]) {
        for (name, id, zero_width) in entries {
            let url = format!("https://cdn.7tv.app/emote/{}/2x.webp", id);
            self.map.insert(
                name.clone(),
                ResolvedEmote {
                    provider: EmoteProvider::SevenTv,
                    id: id.clone(),
                    name: name.clone(),
                    url,
                    zero_width: *zero_width,
                },
            );
        }
    }

    pub fn lookup(&self, word: &str) -> Option<&ResolvedEmote> {
        self.map.get(word)
    }
}

#[derive(Debug, Clone)]
pub enum MessageToken {
    Text(String),
    KickEmote { id: String, name: String },
    ProviderEmote(ResolvedEmote),
    ImageUrl(String),
}

pub fn tokenise(text: &str, emote_map: Option<&EmoteNameMap>) -> Vec<MessageToken> {
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let len = text.len();

    struct Span {
        start: usize,
        end: usize,
        kind: SpanKind,
    }
    enum SpanKind {
        KickEmote(String, String),
        ImageUrl(String),
    }

    let mut spans = Vec::new();

    for m in EMOTE_REGEX.captures_iter(text) {
        if let (Some(full), Some(id), Some(name)) = (m.get(0), m.name("id"), m.name("name")) {
            spans.push(Span {
                start: full.start(),
                end: full.end(),
                kind: SpanKind::KickEmote(id.as_str().to_owned(), name.as_str().to_owned()),
            });
        }
    }

    for m in IMAGE_URL_REGEX.find_iter(text) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            kind: SpanKind::ImageUrl(m.as_str().to_owned()),
        });
    }

    spans.sort_unstable_by_key(|s| s.start);

    for span in spans {
        if span.start > pos {
            push_text_segment(&text[pos..span.start], emote_map, &mut tokens);
        }
        match span.kind {
            SpanKind::KickEmote(id, name) => tokens.push(MessageToken::KickEmote { id, name }),
            SpanKind::ImageUrl(url) => tokens.push(MessageToken::ImageUrl(url)),
        }
        pos = span.end;
    }

    if pos < len {
        push_text_segment(&text[pos..], emote_map, &mut tokens);
    }

    tokens
}

fn push_text_segment(seg: &str, map: Option<&EmoteNameMap>, out: &mut Vec<MessageToken>) {
    let mut buf = String::new();
    for word in seg.split_whitespace() {
        if let Some(emote) = map.and_then(|m| m.lookup(word)) {
            if !buf.is_empty() {
                out.push(MessageToken::Text(std::mem::take(&mut buf)));
            }
            out.push(MessageToken::ProviderEmote(emote.clone()));
        } else {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(word);
        }
    }
    if !buf.is_empty() {
        out.push(MessageToken::Text(buf));
    }
}
