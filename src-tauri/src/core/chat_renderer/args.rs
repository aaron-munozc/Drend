use serde::{Deserialize, Serialize};
use skia_safe::{Color, Color4f};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ObjectColor {
    pub alpha: i32,
    pub red: i32,
    pub green: i32,
    pub blue: i32,
}

impl ObjectColor {
    pub fn black() -> Self {
        Self {
            alpha: 255,
            red: 20,
            green: 20,
            blue: 20,
        }
    }
    pub fn white() -> Self {
        Self {
            alpha: 255,
            red: 240,
            green: 240,
            blue: 240,
        }
    }
    pub fn solid_black() -> Self {
        Self {
            alpha: 255,
            red: 0,
            green: 0,
            blue: 0,
        }
    }
}

fn clamp_f(v: i32) -> f32 {
    v.clamp(0, 255) as f32
}

impl From<&ObjectColor> for Color4f {
    fn from(obj: &ObjectColor) -> Self {
        Color4f::new(
            clamp_f(obj.alpha),
            clamp_f(obj.red),
            clamp_f(obj.green),
            clamp_f(obj.blue),
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

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundMode {
    #[default]
    Transparent,
    LumaMatte,
    ChromaKeyGreen,
    CustomColor,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub enum EvictionStrategy {
    #[default]
    Timed,
    PushOnly,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderVideoArgs {
    pub output_path: String,
    pub width: i32,
    pub height: i32,
    pub fps: u32,
    pub background_mode: BackgroundMode,
    pub background_color: ObjectColor,
    pub font_name: String,
    pub font_size: f32,
    pub line_spacing: i32,
    pub message_spacing: i32,
    pub message_color: ObjectColor,
    pub padding: i32,
    pub outline_usernames: bool,
    pub username_outline_width: Option<f32>,
    pub username_shadow: bool,
    pub bubble_mode_full_width: bool,
    pub bubble_color: ObjectColor,
    pub bubble_radius: f32,
    pub bubble_padding: i32,
    pub anim_slide: bool,
    pub anim_fade_in: bool,
    pub eviction_strategy: EvictionStrategy,
    pub message_hold_seconds: u32,
    pub message_fade_out_seconds: u32,
    pub pinned_users: Vec<String>,
    pub pin_duration_secs: u32,
    pub skip_users: Vec<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub time_zero_ms: Option<u64>,
    pub group_messages: bool,
    pub group_messages_window_secs: u32,
    pub center_emotes_vertically: bool,
    pub crate_premultiplied_alpha_emotes: bool,
}

impl Default for RenderVideoArgs {
    fn default() -> Self {
        Self {
            output_path: String::new(),
            width: 400,
            height: 800,
            fps: 30,
            background_mode: BackgroundMode::Transparent,
            background_color: ObjectColor::black(),
            font_name: "Inter".into(),
            font_size: 20.0,
            line_spacing: 6,
            message_spacing: 12,
            message_color: ObjectColor::white(),
            padding: 20,
            outline_usernames: false,
            username_outline_width: None,
            username_shadow: false,
            bubble_mode_full_width: false,
            bubble_color: ObjectColor::solid_black(),
            bubble_radius: 8.0,
            bubble_padding: 8,
            anim_slide: true,
            anim_fade_in: false,
            eviction_strategy: EvictionStrategy::Timed,
            message_hold_seconds: 5,
            message_fade_out_seconds: 2,
            pinned_users: vec![],
            pin_duration_secs: 10,
            skip_users: vec!["BotRix".into(), "KickBot".into()],
            start_ms: None,
            end_ms: None,
            time_zero_ms: None,
            group_messages: false,
            group_messages_window_secs: 0,
            center_emotes_vertically: true,
            crate_premultiplied_alpha_emotes: true,
        }
    }
}