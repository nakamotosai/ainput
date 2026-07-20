use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::api_config::ApiConnectionsConfig;
use crate::modes::{InputMode, VoiceProfileId};

pub const HUD_BACKGROUND_ALPHA_MIN_PERCENT: u8 = 0;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub mode: ModeConfig,
    pub hotkey: HotkeyConfig,
    pub profiles: ProfileConfigs,
    pub asr: AsrConfig,
    pub whisper: WhisperConfig,
    pub local_nonstreaming: LocalNonstreamingConfig,
    pub rewrite: RewriteConfig,
    #[serde(default = "RewriteConfig::prompt_studio_default")]
    pub prompt_studio: RewriteConfig,
    pub suspect_terms: SuspectTermsConfig,
    pub term_embeddings: TermEmbeddingConfig,
    pub output: OutputConfig,
    pub hud: HudConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModeConfig {
    pub default: InputMode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub voice_input: String,
    pub poll_ms: u64,
    pub activation_delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProfileConfigs {
    pub streaming: VoiceProfileConfig,
    pub whisper: VoiceProfileConfig,
    pub local_nonstreaming: VoiceProfileConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VoiceProfileConfig {
    pub enabled: bool,
    pub mode: InputMode,
    pub hotkey: String,
    pub activation_delay_ms: u64,
    pub suppress_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    pub endpoint_url: String,
    pub chunk_ms: u32,
    pub release_grace_ms: u64,
    pub pre_roll_ms: u64,
    pub audio_ring_ms: u64,
    pub language: String,
    pub request_timeout_ms: u64,
    pub api_key_env: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WhisperConfig {
    pub endpoint_url: String,
    pub sample_rate_hz: u32,
    pub release_grace_ms: u64,
    pub min_audio_ms: u64,
    pub min_rms_dbfs: f32,
    pub request_timeout_ms: u64,
    pub api_key_env: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LocalNonstreamingConfig {
    pub model_dir: String,
    pub provider: String,
    pub sample_rate_hz: u32,
    pub language: String,
    pub use_itn: bool,
    pub num_threads: i32,
    pub release_grace_ms: u64,
    pub min_audio_ms: u64,
    pub min_rms_dbfs: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RewriteConfig {
    pub enabled: bool,
    pub streaming_enabled: bool,
    pub dynamic_budget_enabled: bool,
    pub compact_prompt_enabled: bool,
    pub streaming_prewrite_enabled: bool,
    pub prewrite_min_chars: usize,
    pub prewrite_stable_ms: u64,
    pub prewrite_debounce_ms: u64,
    pub prewrite_max_inflight: usize,
    pub mode: RewriteMode,
    pub output_language: RewriteOutputLanguage,
    pub endpoint_url: String,
    pub model: String,
    pub fallback_models: Vec<String>,
    pub api_key_env: String,
    pub api_key: String,
    pub timeout_ms: u64,
    pub debounce_ms: u64,
    pub min_chars: usize,
    pub max_output_chars: usize,
    pub temperature: f32,
    pub fallback_cooldown_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SuspectTermsConfig {
    pub enabled: bool,
    pub endpoint_url: String,
    pub model: String,
    pub fallback_models: Vec<String>,
    pub api_key_env: String,
    pub api_key: String,
    pub interval_ms: u64,
    pub startup_delay_ms: u64,
    pub history_limit: usize,
    pub min_records: usize,
    pub max_suggestions: usize,
    pub timeout_ms: u64,
    pub temperature: f32,
    pub max_output_chars: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TermEmbeddingConfig {
    pub enabled: bool,
    pub endpoint_url: String,
    pub model: String,
    pub fallback_models: Vec<String>,
    pub api_key_env: String,
    pub api_key: String,
    pub interval_ms: u64,
    pub startup_delay_ms: u64,
    pub history_limit: usize,
    pub max_items_per_run: usize,
    pub max_context_chars: usize,
    pub timeout_ms: u64,
    pub prune_inactive_cache: bool,
    pub family_min_variants: usize,
    pub family_similarity_threshold: f32,
    pub max_hotword_terms: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RewriteMode {
    AfterRelease,
    DuringHold,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RewriteOutputLanguage {
    Chinese,
    English,
    Japanese,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardPolicy {
    RetainTranscript,
    RestoreTextAfterSuccess,
    CopyOnly,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub prefer_direct_paste: bool,
    pub paste_stabilize_ms: u64,
    pub clipboard_policy: ClipboardPolicy,
    pub clipboard_retry_count: u32,
    pub clipboard_retry_backoff_ms: u64,
    pub paste_preflight_recheck: bool,
    pub replacement_preflight_recheck: bool,
    #[allow(dead_code)]
    pub clipboard_restore_delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HudConfig {
    pub style: HudVisualStyle,
    pub animation_theme: HudAnimationTheme,
    pub anchor: HudAnchor,
    pub expand_origin: HudExpandOrigin,
    pub offset_x_px: i32,
    pub offset_y_px: i32,
    pub width_px: i32,
    pub height_px: i32,
    pub min_width_px: i32,
    pub min_height_px: i32,
    pub min_text_width_px: i32,
    pub padding_x_px: i32,
    pub padding_y_px: i32,
    pub font_height_px: i32,
    pub auto_font_fit: bool,
    pub font_weight: i32,
    pub font_family: String,
    pub text_align: HudTextAlign,
    pub text_color: String,
    pub text_alpha: u8,
    pub text_effect: HudTextEffect,
    pub shadow_enabled: bool,
    pub shadow_color: String,
    pub shadow_alpha: u8,
    pub shadow_offset_x_px: i32,
    pub shadow_offset_y_px: i32,
    pub rainbow_saturation_percent: u8,
    pub rainbow_lightness_percent: u8,
    pub rainbow_step_degree: u16,
    pub background_color: String,
    pub background_alpha: u8,
    pub corner_radius_px: i32,
    pub display_hold_ms: u64,
}


#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct HudUserConfig {
    pub animation_theme: Option<HudAnimationTheme>,
    pub anchor: Option<HudAnchor>,
    pub expand_origin: Option<HudExpandOrigin>,
    pub offset_x_px: Option<i32>,
    pub offset_y_px: Option<i32>,
    pub width_px: Option<i32>,
    pub height_px: Option<i32>,
    pub padding_x_px: Option<i32>,
    pub padding_y_px: Option<i32>,
    pub auto_font_fit: Option<bool>,
    pub text_align: Option<HudTextAlign>,
    pub font_family: Option<String>,
    pub font_height_px: Option<i32>,
    pub font_weight: Option<i32>,
    pub background_alpha_percent: Option<u8>,
    pub text_color: Option<String>,
    pub text_alpha_percent: Option<u8>,
    pub background_color: Option<String>,
    pub text_effect: Option<HudTextEffect>,
    pub shadow_enabled: Option<bool>,
    pub shadow_color: Option<String>,
    pub shadow_alpha_percent: Option<u8>,
    pub shadow_offset_x_px: Option<i32>,
    pub shadow_offset_y_px: Option<i32>,
    pub rainbow_saturation_percent: Option<u8>,
    pub rainbow_lightness_percent: Option<u8>,
    pub rainbow_step_degree: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RewriteUserConfig {
    pub enabled: Option<bool>,
    pub streaming_enabled: Option<bool>,
    pub output_language: Option<RewriteOutputLanguage>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HudAnchor {
    BottomLeft,
    BottomCenter,
    TaskbarLeft,
    TaskbarCenter,
    TaskbarRight,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HudExpandOrigin {
    Left,
    Center,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HudTextAlign {
    Left,
    Center,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HudTextEffect {
    #[default]
    Solid,
    Rainbow,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HudVisualStyle {
    Minimal,
    AiConsole,
    FloatingText,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HudAnimationTheme {
    TextOnly,
    VoiceBars,
    Waveform,
    StageDots,
    AiGlow,
    MinimalPulse,
    #[default]
    FullAnimated,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let mut config: Self = toml::from_str(&raw).context("parse ainput config")?;
        config.hud.apply_runtime_defaults();
        config.apply_legacy_hotkey_if_needed(&raw);
        if let Some(parent) = path.parent() {
            let user_path = parent.join("hud-user.toml");
            if user_path.exists() {
                let raw = std::fs::read_to_string(&user_path)
                    .with_context(|| format!("read HUD user config {}", user_path.display()))?;
                let user: HudUserConfig = toml::from_str(&raw).context("parse HUD user config")?;
                config.hud.apply_user_config(&user);
                config.hud.apply_runtime_defaults();
            }
            let rewrite_user_path = parent.join("rewrite-user.toml");
            if rewrite_user_path.exists() {
                let raw = std::fs::read_to_string(&rewrite_user_path).with_context(|| {
                    format!("read rewrite user config {}", rewrite_user_path.display())
                })?;
                let user: RewriteUserConfig =
                    toml::from_str(&raw).context("parse rewrite user config")?;
                config.rewrite.apply_user_config(&user);
            }
        }
        Ok(config)
    }

    pub fn apply_api_connections(&mut self, api: &ApiConnectionsConfig) {
        let chat_endpoint = api.chat_completions_url();
        let api_key_env = api.api_key_env();
        let api_key = api.api_key();
        let asr_sidecar = api.asr_sidecar_url();
        let asr_api_key_env = api.asr_api_key_env();
        let asr_api_key = api.asr_api_key();

        if !asr_sidecar.is_empty() {
            self.asr.endpoint_url = asr_sidecar.clone();
            self.whisper.endpoint_url = asr_sidecar;
        }
        self.asr.api_key_env = asr_api_key_env.clone();
        self.asr.api_key = asr_api_key.clone();
        self.whisper.api_key_env = asr_api_key_env;
        self.whisper.api_key = asr_api_key;

        self.rewrite.endpoint_url = chat_endpoint.clone();
        self.rewrite.model = api.rewrite.model.clone();
        self.rewrite.fallback_models = api.rewrite.fallback_models.clone();
        self.rewrite.api_key_env = api_key_env.clone();
        self.rewrite.api_key = api_key.clone();

        self.prompt_studio.endpoint_url = chat_endpoint.clone();
        self.prompt_studio.model = api.rewrite.model.clone();
        self.prompt_studio.fallback_models = api.rewrite.fallback_models.clone();
        self.prompt_studio.api_key_env = api_key_env.clone();
        self.prompt_studio.api_key = api_key.clone();

        self.suspect_terms.endpoint_url = chat_endpoint;
        self.suspect_terms.model = api.rewrite.model.clone();
        self.suspect_terms.fallback_models = api.rewrite.fallback_models.clone();
        self.suspect_terms.api_key_env = api_key_env.clone();
        self.suspect_terms.api_key = api_key.clone();

        self.term_embeddings.endpoint_url = api.embeddings_url();
        self.term_embeddings.model = api.embedding.model.clone();
        self.term_embeddings.fallback_models = api.embedding.fallback_models.clone();
        self.term_embeddings.api_key_env = api.embedding_api_key_env();
        self.term_embeddings.api_key = api.embedding_api_key();
    }

    fn apply_legacy_hotkey_if_needed(&mut self, raw: &str) {
        if raw.contains("[profiles.streaming]") || raw.contains("[profiles.whisper]") {
            return;
        }
        self.profiles.streaming.hotkey = self.hotkey.voice_input.clone();
        self.profiles.streaming.activation_delay_ms = self.hotkey.activation_delay_ms;
    }
}

impl HudConfig {
    fn apply_runtime_defaults(&mut self) {
        let default = HudConfig::default();
        if self.text_alpha == 0 {
            self.text_alpha = default.text_alpha;
        }
        if self.shadow_alpha == 0 {
            self.shadow_alpha = default.shadow_alpha;
        }
        if self.rainbow_lightness_percent == 0 {
            self.rainbow_lightness_percent = default.rainbow_lightness_percent;
        }
        if self.rainbow_step_degree == 0 {
            self.rainbow_step_degree = default.rainbow_step_degree;
        }
        if self.text_color.trim().is_empty() {
            self.text_color = default.text_color;
        }
        if self.background_color.trim().is_empty() {
            self.background_color = default.background_color;
        }
        if self.shadow_color.trim().is_empty() {
            self.shadow_color = default.shadow_color;
        }
    }

    pub fn apply_user_config(&mut self, user: &HudUserConfig) {
        if let Some(animation_theme) = user.animation_theme {
            self.animation_theme = animation_theme;
        }
        if let Some(anchor) = user.anchor {
            self.anchor = anchor;
        }
        if let Some(expand_origin) = user.expand_origin {
            self.expand_origin = expand_origin;
        }
        if let Some(offset_x_px) = user.offset_x_px {
            self.offset_x_px = offset_x_px.clamp(-10_000, 10_000);
        }
        if let Some(offset_y_px) = user.offset_y_px {
            self.offset_y_px = offset_y_px.clamp(-10_000, 10_000);
        }
        if let Some(width_px) = user.width_px {
            self.width_px = width_px.clamp(120, 10_000);
        }
        if let Some(height_px) = user.height_px {
            self.height_px = height_px.clamp(24, 1000);
            self.min_height_px = self.height_px;
        }
        if let Some(padding_x_px) = user.padding_x_px {
            self.padding_x_px = padding_x_px.clamp(0, 96);
        }
        if let Some(padding_y_px) = user.padding_y_px {
            self.padding_y_px = padding_y_px.clamp(0, 48);
        }
        if let Some(auto_font_fit) = user.auto_font_fit {
            self.auto_font_fit = auto_font_fit;
        }
        if let Some(text_align) = user.text_align {
            self.text_align = text_align;
        }
        if let Some(font_family) = user.font_family.as_ref().map(|value| value.trim()) {
            if !font_family.is_empty() {
                self.font_family = font_family.to_string();
            }
        }
        if let Some(font_height_px) = user.font_height_px {
            if font_height_px > 0 {
                self.font_height_px = font_height_px;
            }
        }
        if let Some(font_weight) = user.font_weight {
            self.font_weight = font_weight.clamp(100, 900);
        }
        if let Some(percent) = user.background_alpha_percent {
            self.background_alpha = alpha_percent_to_byte(percent);
        }
        if let Some(text_color) = user.text_color.as_ref().map(|value| value.trim()) {
            if !text_color.is_empty() {
                self.text_color = normalize_hex_color(text_color, &self.text_color);
            }
        }
        if let Some(percent) = user.text_alpha_percent {
            self.text_alpha = alpha_percent_to_byte(percent);
        }
        if let Some(background_color) = user.background_color.as_ref().map(|value| value.trim()) {
            if !background_color.is_empty() {
                self.background_color =
                    normalize_hex_color(background_color, &self.background_color);
            }
        }
        if let Some(effect) = user.text_effect {
            self.text_effect = effect;
        }
        if let Some(enabled) = user.shadow_enabled {
            self.shadow_enabled = enabled;
        }
        if let Some(shadow_color) = user.shadow_color.as_ref().map(|value| value.trim()) {
            if !shadow_color.is_empty() {
                self.shadow_color = normalize_hex_color(shadow_color, &self.shadow_color);
            }
        }
        if let Some(percent) = user.shadow_alpha_percent {
            self.shadow_alpha = alpha_percent_to_byte(percent);
        }
        if let Some(offset) = user.shadow_offset_x_px {
            self.shadow_offset_x_px = offset.clamp(-32, 32);
        }
        if let Some(offset) = user.shadow_offset_y_px {
            self.shadow_offset_y_px = offset.clamp(-32, 32);
        }
        if let Some(percent) = user.rainbow_saturation_percent {
            self.rainbow_saturation_percent = percent.clamp(0, 100);
        }
        if let Some(percent) = user.rainbow_lightness_percent {
            self.rainbow_lightness_percent = percent.clamp(0, 100);
        }
        if let Some(degree) = user.rainbow_step_degree {
            self.rainbow_step_degree = degree.clamp(1, 180);
        }
    }
}

impl HudUserConfig {
    pub fn from_config(config: &HudConfig) -> Self {
        Self {
            animation_theme: Some(config.animation_theme),
            anchor: Some(config.anchor),
            expand_origin: Some(config.expand_origin),
            offset_x_px: Some(config.offset_x_px),
            offset_y_px: Some(config.offset_y_px),
            width_px: Some(config.width_px),
            height_px: Some(config.height_px),
            padding_x_px: Some(config.padding_x_px),
            padding_y_px: Some(config.padding_y_px),
            auto_font_fit: Some(config.auto_font_fit),
            text_align: Some(config.text_align),
            font_family: Some(config.font_family.clone()),
            font_height_px: Some(config.font_height_px),
            font_weight: Some(config.font_weight),
            background_alpha_percent: Some(alpha_byte_to_percent(config.background_alpha)),
            text_color: Some(config.text_color.clone()),
            text_alpha_percent: Some(alpha_byte_to_percent(config.text_alpha)),
            background_color: Some(config.background_color.clone()),
            text_effect: Some(config.text_effect),
            shadow_enabled: Some(config.shadow_enabled),
            shadow_color: Some(config.shadow_color.clone()),
            shadow_alpha_percent: Some(alpha_byte_to_percent(config.shadow_alpha)),
            shadow_offset_x_px: Some(config.shadow_offset_x_px),
            shadow_offset_y_px: Some(config.shadow_offset_y_px),
            rainbow_saturation_percent: Some(config.rainbow_saturation_percent),
            rainbow_lightness_percent: Some(config.rainbow_lightness_percent),
            rainbow_step_degree: Some(config.rainbow_step_degree),
        }
    }

    pub fn merge(&mut self, next: HudUserConfig) {
        if next.animation_theme.is_some() {
            self.animation_theme = next.animation_theme;
        }
        if next.anchor.is_some() {
            self.anchor = next.anchor;
        }
        if next.expand_origin.is_some() {
            self.expand_origin = next.expand_origin;
        }
        if next.offset_x_px.is_some() {
            self.offset_x_px = next.offset_x_px;
        }
        if next.offset_y_px.is_some() {
            self.offset_y_px = next.offset_y_px;
        }
        if next.width_px.is_some() {
            self.width_px = next.width_px;
        }
        if next.height_px.is_some() {
            self.height_px = next.height_px;
        }
        if next.padding_x_px.is_some() {
            self.padding_x_px = next.padding_x_px;
        }
        if next.padding_y_px.is_some() {
            self.padding_y_px = next.padding_y_px;
        }
        if next.auto_font_fit.is_some() {
            self.auto_font_fit = next.auto_font_fit;
        }
        if next.text_align.is_some() {
            self.text_align = next.text_align;
        }
        if next.font_family.is_some() {
            self.font_family = next.font_family;
        }
        if next.font_height_px.is_some() {
            self.font_height_px = next.font_height_px;
        }
        if next.font_weight.is_some() {
            self.font_weight = next.font_weight;
        }
        if next.background_alpha_percent.is_some() {
            self.background_alpha_percent = next.background_alpha_percent;
        }
        if next.text_color.is_some() {
            self.text_color = next.text_color;
        }
        if next.text_alpha_percent.is_some() {
            self.text_alpha_percent = next.text_alpha_percent;
        }
        if next.background_color.is_some() {
            self.background_color = next.background_color;
        }
        if next.text_effect.is_some() {
            self.text_effect = next.text_effect;
        }
        if next.shadow_enabled.is_some() {
            self.shadow_enabled = next.shadow_enabled;
        }
        if next.shadow_color.is_some() {
            self.shadow_color = next.shadow_color;
        }
        if next.shadow_alpha_percent.is_some() {
            self.shadow_alpha_percent = next.shadow_alpha_percent;
        }
        if next.shadow_offset_x_px.is_some() {
            self.shadow_offset_x_px = next.shadow_offset_x_px;
        }
        if next.shadow_offset_y_px.is_some() {
            self.shadow_offset_y_px = next.shadow_offset_y_px;
        }
        if next.rainbow_saturation_percent.is_some() {
            self.rainbow_saturation_percent = next.rainbow_saturation_percent;
        }
        if next.rainbow_lightness_percent.is_some() {
            self.rainbow_lightness_percent = next.rainbow_lightness_percent;
        }
        if next.rainbow_step_degree.is_some() {
            self.rainbow_step_degree = next.rainbow_step_degree;
        }
    }
}

pub fn save_hud_user_config(path: &Path, user: &HudUserConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create HUD user config dir {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(user).context("serialize HUD user config")?;
    std::fs::write(path, raw).with_context(|| format!("write HUD user config {}", path.display()))
}

fn normalize_hex_color(value: &str, fallback: &str) -> String {
    let value = value.trim();
    let Some(hex) = value.strip_prefix('#') else {
        return fallback.to_string();
    };
    if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("#{hex}")
    } else {
        fallback.to_string()
    }
}

pub fn save_rewrite_user_config(path: &Path, user: &RewriteUserConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create rewrite user config dir {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(user).context("serialize rewrite user config")?;
    std::fs::write(path, raw)
        .with_context(|| format!("write rewrite user config {}", path.display()))
}

pub fn alpha_percent_to_byte(percent: u8) -> u8 {
    let percent = percent.clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100) as u16;
    ((percent * 255 + 50) / 100).clamp(0, 255) as u8
}

pub fn alpha_byte_to_percent(alpha: u8) -> u8 {
    (((alpha as u16) * 100 + 127) / 255).clamp(0, 100) as u8
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: ModeConfig::default(),
            hotkey: HotkeyConfig::default(),
            profiles: ProfileConfigs::default(),
            asr: AsrConfig::default(),
            whisper: WhisperConfig::default(),
            local_nonstreaming: LocalNonstreamingConfig::default(),
            rewrite: RewriteConfig::default(),
            prompt_studio: RewriteConfig::prompt_studio_default(),
            suspect_terms: SuspectTermsConfig::default(),
            term_embeddings: TermEmbeddingConfig::default(),
            output: OutputConfig::default(),
            hud: HudConfig::default(),
        }
    }
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            default: InputMode::LocalNonstreaming,
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            voice_input: "CapsLock".to_string(),
            poll_ms: 8,
            activation_delay_ms: 200,
        }
    }
}

impl Default for ProfileConfigs {
    fn default() -> Self {
        Self {
            streaming: VoiceProfileConfig::for_profile(VoiceProfileId::StreamingDefault),
            whisper: VoiceProfileConfig::for_profile(VoiceProfileId::WhisperCapslock),
            local_nonstreaming: VoiceProfileConfig::for_profile(VoiceProfileId::LocalNonstreaming),
        }
    }
}

impl VoiceProfileConfig {
    fn for_profile(profile: VoiceProfileId) -> Self {
        match profile {
            VoiceProfileId::StreamingDefault => Self {
                enabled: false,
                mode: InputMode::StreamingAsr,
                hotkey: "Ctrl".to_string(),
                activation_delay_ms: 200,
                suppress_key: false,
            },
            VoiceProfileId::WhisperCapslock => Self {
                enabled: false,
                mode: InputMode::WhisperZh,
                hotkey: "Alt+Z".to_string(),
                activation_delay_ms: 250,
                suppress_key: true,
            },
            VoiceProfileId::LocalNonstreaming => Self {
                enabled: true,
                mode: InputMode::LocalNonstreaming,
                hotkey: "CapsLock".to_string(),
                activation_delay_ms: 220,
                suppress_key: true,
            },
        }
    }
}

impl Default for VoiceProfileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: InputMode::StreamingAsr,
            hotkey: String::new(),
            activation_delay_ms: 200,
            suppress_key: false,
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            endpoint_url: String::new(),
            chunk_ms: 100,
            release_grace_ms: 0,
            pre_roll_ms: 160,
            audio_ring_ms: 600,
            language: "zh-CN".to_string(),
            request_timeout_ms: 8000,
            api_key_env: String::new(),
            api_key: String::new(),
        }
    }
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            endpoint_url: String::new(),
            sample_rate_hz: 16_000,
            release_grace_ms: 80,
            min_audio_ms: 1800,
            min_rms_dbfs: -56.0,
            request_timeout_ms: 8000,
            api_key_env: String::new(),
            api_key: String::new(),
        }
    }
}

impl Default for LocalNonstreamingConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/sense-voice".to_string(),
            provider: "cpu".to_string(),
            sample_rate_hz: 16_000,
            language: "auto".to_string(),
            use_itn: true,
            num_threads: 4,
            release_grace_ms: 80,
            min_audio_ms: 800,
            min_rms_dbfs: -56.0,
        }
    }
}

impl Default for RewriteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            streaming_enabled: false,
            dynamic_budget_enabled: true,
            compact_prompt_enabled: true,
            streaming_prewrite_enabled: false,
            prewrite_min_chars: 8,
            prewrite_stable_ms: 700,
            prewrite_debounce_ms: 900,
            prewrite_max_inflight: 1,
            mode: RewriteMode::DuringHold,
            output_language: RewriteOutputLanguage::Chinese,
            endpoint_url: String::new(),
            model: String::new(),
            fallback_models: Vec::new(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
            timeout_ms: 3000,
            debounce_ms: 260,
            min_chars: 4,
            max_output_chars: 256,
            temperature: 0.1,
            fallback_cooldown_ms: 60_000,
        }
    }
}

impl RewriteConfig {
    pub fn prompt_studio_default() -> Self {
        Self {
            enabled: false,
            timeout_ms: 8000,
            ..Default::default()
        }
    }

    pub fn apply_user_config(&mut self, user: &RewriteUserConfig) {
        if let Some(enabled) = user.enabled {
            self.enabled = enabled;
        }
        if let Some(streaming_enabled) = user.streaming_enabled {
            self.streaming_enabled = streaming_enabled;
        }
        if let Some(output_language) = user.output_language {
            self.output_language = output_language;
        }
    }
}

impl Default for SuspectTermsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint_url: String::new(),
            model: String::new(),
            fallback_models: Vec::new(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
            interval_ms: 300_000,
            startup_delay_ms: 300_000,
            history_limit: 80,
            min_records: 1,
            max_suggestions: 12,
            timeout_ms: 12_000,
            temperature: 0.1,
            max_output_chars: 2048,
        }
    }
}

impl Default for TermEmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint_url: String::new(),
            model: String::new(),
            fallback_models: Vec::new(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
            interval_ms: 300_000,
            startup_delay_ms: 20_000,
            history_limit: 120,
            max_items_per_run: 24,
            max_context_chars: 480,
            timeout_ms: 12_000,
            prune_inactive_cache: true,
            family_min_variants: 2,
            family_similarity_threshold: 0.72,
            max_hotword_terms: 256,
        }
    }
}

impl Default for RewriteOutputLanguage {
    fn default() -> Self {
        Self::Chinese
    }
}

impl RewriteOutputLanguage {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Chinese => 0,
            Self::English => 1,
            Self::Japanese => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::English,
            2 => Self::Japanese,
            _ => Self::Chinese,
        }
    }

    pub fn is_translation(self) -> bool {
        !matches!(self, Self::Chinese)
    }
}

impl Default for ClipboardPolicy {
    fn default() -> Self {
        Self::RetainTranscript
    }
}

impl ClipboardPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetainTranscript => "retain_transcript",
            Self::RestoreTextAfterSuccess => "restore_text_after_success",
            Self::CopyOnly => "copy_only",
        }
    }

    pub fn restores_text_after_success(self) -> bool {
        matches!(self, Self::RestoreTextAfterSuccess)
    }

    pub fn is_copy_only(self) -> bool {
        matches!(self, Self::CopyOnly)
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            prefer_direct_paste: true,
            paste_stabilize_ms: 25,
            clipboard_policy: ClipboardPolicy::RetainTranscript,
            clipboard_retry_count: 3,
            clipboard_retry_backoff_ms: 35,
            paste_preflight_recheck: true,
            replacement_preflight_recheck: true,
            clipboard_restore_delay_ms: 100,
        }
    }
}

impl Default for HudConfig {
    fn default() -> Self {
        Self {
            style: HudVisualStyle::FloatingText,
            animation_theme: HudAnimationTheme::MinimalPulse,
            anchor: HudAnchor::TaskbarCenter,
            expand_origin: HudExpandOrigin::Center,
            offset_x_px: 0,
            offset_y_px: 0,
            width_px: 620,
            height_px: 46,
            min_width_px: 48,
            min_height_px: 46,
            min_text_width_px: 1,
            padding_x_px: 18,
            padding_y_px: 7,
            font_height_px: 26,
            auto_font_fit: true,
            font_weight: 600,
            font_family: "Microsoft YaHei UI".to_string(),
            text_align: HudTextAlign::Center,
            text_color: "#FFFFFF".to_string(),
            text_alpha: 255,
            text_effect: HudTextEffect::Solid,
            shadow_enabled: false,
            shadow_color: "#000000".to_string(),
            shadow_alpha: 160,
            shadow_offset_x_px: 1,
            shadow_offset_y_px: 1,
            rainbow_saturation_percent: 45,
            rainbow_lightness_percent: 78,
            rainbow_step_degree: 28,
            background_color: "#071014".to_string(),
            background_alpha: 230,
            corner_radius_px: 16,
            display_hold_ms: 650,
        }
    }
}

impl Default for HudAnchor {
    fn default() -> Self {
        Self::TaskbarCenter
    }
}

impl Default for HudExpandOrigin {
    fn default() -> Self {
        Self::Center
    }
}

impl Default for HudTextAlign {
    fn default() -> Self {
        Self::Left
    }
}

impl Default for HudVisualStyle {
    fn default() -> Self {
        Self::FloatingText
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, ClipboardPolicy, HudAnimationTheme, HudVisualStyle, RewriteMode,
        RewriteOutputLanguage,
    };
    use crate::modes::InputMode;

    #[test]
    fn defaults_to_three_voice_profiles() {
        let config = AppConfig::default();
        assert_eq!(config.mode.default, InputMode::LocalNonstreaming);
        assert_eq!(config.hotkey.voice_input, "CapsLock");
        assert_eq!(config.hotkey.activation_delay_ms, 200);
        assert!(!config.profiles.streaming.enabled);
        assert_eq!(config.profiles.streaming.mode, InputMode::StreamingAsr);
        assert_eq!(config.profiles.streaming.hotkey, "Ctrl");
        assert!(!config.profiles.whisper.enabled);
        assert_eq!(config.profiles.whisper.mode, InputMode::WhisperZh);
        assert_eq!(config.profiles.whisper.hotkey, "Alt+Z");
        assert!(config.profiles.whisper.suppress_key);
        assert!(config.profiles.local_nonstreaming.enabled);
        assert_eq!(
            config.profiles.local_nonstreaming.mode,
            InputMode::LocalNonstreaming
        );
        assert_eq!(config.profiles.local_nonstreaming.hotkey, "CapsLock");
        assert_eq!(config.profiles.local_nonstreaming.activation_delay_ms, 220);
        assert!(config.profiles.local_nonstreaming.suppress_key);
        assert_eq!(config.local_nonstreaming.model_dir, "models/sense-voice");
        assert_eq!(config.local_nonstreaming.provider, "cpu");
        assert!(!config.rewrite.enabled);
        assert!(!config.rewrite.streaming_enabled);
        assert_eq!(config.rewrite.min_chars, 4);
        assert_eq!(
            config.rewrite.output_language,
            RewriteOutputLanguage::Chinese
        );
        assert_eq!(config.prompt_studio.model, "");
        assert!(config.prompt_studio.fallback_models.is_empty());
        assert_eq!(config.whisper.sample_rate_hz, 16000);
        assert!(config.output.prefer_direct_paste);
        assert_eq!(config.output.paste_stabilize_ms, 25);
        assert_eq!(
            config.output.clipboard_policy,
            ClipboardPolicy::RetainTranscript
        );
        assert_eq!(config.output.clipboard_retry_count, 3);
        assert_eq!(config.output.clipboard_retry_backoff_ms, 35);
        assert!(config.output.paste_preflight_recheck);
        assert!(config.output.replacement_preflight_recheck);
        assert_eq!(config.hud.width_px, 620);
        assert_eq!(config.hud.min_width_px, 48);
        assert_eq!(config.hud.min_height_px, 46);
        assert_eq!(config.hud.style, HudVisualStyle::FloatingText);
        assert_eq!(config.hud.animation_theme, HudAnimationTheme::MinimalPulse);
    }

    #[test]
    fn hud_user_config_can_override_alpha_percent() {
        let mut config = AppConfig::default();
        config.hud.apply_user_config(&super::HudUserConfig {
            background_alpha_percent: Some(0),
            ..Default::default()
        });
        assert_eq!(config.hud.background_alpha, super::alpha_percent_to_byte(0));

        config.hud.apply_user_config(&super::HudUserConfig {
            background_alpha_percent: Some(100),
            ..Default::default()
        });
        assert_eq!(config.hud.background_alpha, 255);
    }

    #[test]
    fn hud_user_config_keeps_large_font_height_without_legacy_cap() {
        let mut config = AppConfig::default();
        config.hud.apply_user_config(&super::HudUserConfig {
            font_height_px: Some(180),
            ..Default::default()
        });
        assert_eq!(config.hud.font_height_px, 180);

        config.hud.apply_user_config(&super::HudUserConfig {
            font_height_px: Some(0),
            ..Default::default()
        });
        assert_eq!(config.hud.font_height_px, 180);
    }

    #[test]
    fn hud_user_config_can_override_animation_theme() {
        let mut config = AppConfig::default();
        config.hud.apply_user_config(&super::HudUserConfig {
            animation_theme: Some(HudAnimationTheme::VoiceBars),
            ..Default::default()
        });
        assert_eq!(config.hud.animation_theme, HudAnimationTheme::VoiceBars);
    }

    #[test]
    fn config_accepts_archived_rewrite_table_and_whisper_mode() {
        let config: AppConfig = toml::from_str(
            r#"
            [mode]
            default = "whisper_zh"

            [rewrite]
            enabled = true
            mode = "during_hold"
            output_language = "japanese"
            timeout_ms = 3000
            fallback_models = ["gpt-5.3-codex"]

            [whisper]
            min_audio_ms = 500
            "#,
        )
        .expect("parse config");

        assert_eq!(config.mode.default, InputMode::WhisperZh);
        assert_eq!(config.whisper.min_audio_ms, 500);
        assert!(config.rewrite.enabled);
        assert_eq!(config.rewrite.mode, RewriteMode::DuringHold);
        assert_eq!(
            config.rewrite.output_language,
            RewriteOutputLanguage::Japanese
        );
        assert_eq!(config.rewrite.fallback_models, vec!["gpt-5.3-codex"]);
    }

    #[test]
    fn config_accepts_output_clipboard_policy_values() {
        let config: AppConfig = toml::from_str(
            r#"
            [output]
            clipboard_policy = "restore_text_after_success"
            clipboard_retry_count = 5
            clipboard_retry_backoff_ms = 12
            paste_preflight_recheck = false
            replacement_preflight_recheck = false
            clipboard_restore_delay_ms = 240
            "#,
        )
        .expect("parse output config");

        assert_eq!(
            config.output.clipboard_policy,
            ClipboardPolicy::RestoreTextAfterSuccess
        );
        assert_eq!(
            config.output.clipboard_policy.as_str(),
            "restore_text_after_success"
        );
        assert!(config.output.clipboard_policy.restores_text_after_success());
        assert_eq!(config.output.clipboard_retry_count, 5);
        assert_eq!(config.output.clipboard_retry_backoff_ms, 12);
        assert!(!config.output.paste_preflight_recheck);
        assert!(!config.output.replacement_preflight_recheck);
        assert_eq!(config.output.clipboard_restore_delay_ms, 240);
    }

    #[test]
    fn legacy_output_config_keeps_retain_transcript_defaults() {
        let config: AppConfig = toml::from_str(
            r#"
            [output]
            prefer_direct_paste = true
            paste_stabilize_ms = 25
            "#,
        )
        .expect("parse legacy output config");

        assert_eq!(
            config.output.clipboard_policy,
            ClipboardPolicy::RetainTranscript
        );
        assert_eq!(config.output.clipboard_retry_count, 3);
        assert!(config.output.paste_preflight_recheck);
        assert!(config.output.replacement_preflight_recheck);
    }

    #[test]
    fn rewrite_user_config_overrides_output_language_and_enabled_state() {
        let dir = std::env::temp_dir().join(format!(
            "ainput-rewrite-user-config-{}",
            std::process::id()
        ));
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).expect("create temp config dir");
        let config_path = config_dir.join("ainput.toml");
        std::fs::write(
            &config_path,
            r#"
            [rewrite]
            enabled = true
            output_language = "chinese"
            "#,
        )
        .expect("write config");
        std::fs::write(
            config_dir.join("rewrite-user.toml"),
            r#"
            enabled = false
            streaming_enabled = true
            output_language = "english"
            "#,
        )
        .expect("write rewrite user config");

        let config = AppConfig::load(&config_path).expect("load config");
        assert_eq!(
            config.rewrite.output_language,
            RewriteOutputLanguage::English
        );
        assert!(!config.rewrite.enabled);
        assert!(config.rewrite.streaming_enabled);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_accepts_profile_hotkeys() {
        let config: AppConfig = toml::from_str(
            r#"
            [profiles.streaming]
            hotkey = "Ctrl"
            activation_delay_ms = 320

            [profiles.whisper]
            mode = "whisper_zh"
            hotkey = "CapsLock"
            activation_delay_ms = 120
            suppress_key = true
            "#,
        )
        .expect("parse profile config");

        assert_eq!(config.profiles.streaming.hotkey, "Ctrl");
        assert_eq!(config.profiles.streaming.mode, InputMode::StreamingAsr);
        assert_eq!(config.profiles.whisper.hotkey, "CapsLock");
        assert_eq!(config.profiles.whisper.mode, InputMode::WhisperZh);
        assert!(config.profiles.whisper.suppress_key);
    }

    #[test]
    fn legacy_hotkey_config_maps_to_streaming_profile_when_loaded() {
        let temp =
            std::env::temp_dir().join(format!("ainput-legacy-hotkey-{}.toml", std::process::id()));
        std::fs::write(
            &temp,
            r#"
            [hotkey]
            voice_input = "Ctrl"
            activation_delay_ms = 111
            "#,
        )
        .expect("write temp config");
        let config = AppConfig::load(&temp).expect("load config");
        let _ = std::fs::remove_file(&temp);

        assert_eq!(config.profiles.streaming.hotkey, "Ctrl");
        assert_eq!(config.profiles.streaming.activation_delay_ms, 111);
        assert_eq!(config.profiles.whisper.hotkey, "Alt+Z");
    }
}
