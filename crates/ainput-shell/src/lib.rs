use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug)]
pub struct Bootstrap {
    pub config: AppConfig,
    pub runtime_paths: RuntimePaths,
    _log_guard: WorkerGuard,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub hotkeys: HotkeyConfig,
    pub voice: VoiceConfig,
    pub capture: CaptureConfig,
    pub automation: AutomationConfig,
    pub recording: ainput_recording::RecordingConfig,
    pub startup: StartupConfig,
    pub asr: AsrConfig,
    pub learning: LearningConfig,
    pub logging: LoggingConfig,
    #[serde(skip)]
    pub hud_overlay: HudOverlayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub voice_input: String,
    pub screen_capture: String,
    pub mouse_middle_hold_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub mode: VoiceMode,
    pub streaming: StreamingVoiceConfig,
    pub prefer_direct_paste: bool,
    pub fallback_to_clipboard: bool,
    pub history_file_name: String,
    pub history_limit: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    #[default]
    Fast,
    Streaming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingVoiceConfig {
    pub enabled: bool,
    pub model_dir: String,
    pub panel_enabled: bool,
    pub rewrite_enabled: bool,
    pub punctuation_model_dir: String,
    pub punctuation_num_threads: i32,
    pub chunk_ms: u32,
    pub ai_rewrite: StreamingAiRewriteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingAiRewriteConfig {
    pub enabled: bool,
    pub endpoint_url: String,
    pub model: String,
    pub api_key_env: String,
    pub timeout_ms: u64,
    pub debounce_ms: u64,
    pub min_visible_chars: usize,
    pub max_context_chars: usize,
    pub max_output_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HudOverlayConfig {
    pub anchor: HudAnchor,
    pub offset_x_px: i32,
    pub offset_y_px: i32,
    pub width_px: i32,
    pub min_width_px: i32,
    pub min_height_px: i32,
    pub min_text_width_px: i32,
    pub padding_x_px: i32,
    pub padding_y_px: i32,
    pub font_height_px: i32,
    pub font_weight: i32,
    pub font_family: String,
    pub text_align: HudTextAlign,
    pub text_color: String,
    pub background_color: String,
    pub background_alpha: u8,
    pub corner_radius_px: i32,
    pub display_hold_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HudAnchor {
    BottomLeft,
    #[default]
    BottomCenter,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HudTextAlign {
    #[default]
    Left,
    Center,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub enabled: bool,
    pub auto_save_to_desktop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationConfig {
    pub repeat_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupConfig {
    pub launch_at_login: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    pub model_dir: String,
    pub provider: String,
    pub sample_rate_hz: u32,
    pub language: String,
    pub use_itn: bool,
    pub num_threads: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningConfig {
    pub auto_activate_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file_name: String,
}

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root_dir: PathBuf,
    pub config_file: PathBuf,
    pub hud_overlay_file: PathBuf,
    pub legacy_config_file: PathBuf,
    pub logs_dir: PathBuf,
    pub models_dir: PathBuf,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            voice_input: "Alt+Z".to_string(),
            screen_capture: "Alt+X".to_string(),
            mouse_middle_hold_enabled: false,
        }
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: VoiceMode::Fast,
            streaming: StreamingVoiceConfig::default(),
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
            history_file_name: "voice-history.log".to_string(),
            history_limit: 500,
        }
    }
}

impl Default for StreamingVoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model_dir: "models/streaming-zipformer-small-bilingual-zh-en".to_string(),
            panel_enabled: true,
            rewrite_enabled: true,
            punctuation_model_dir:
                "models/punctuation/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8"
                    .to_string(),
            punctuation_num_threads: 1,
            chunk_ms: 60,
            ai_rewrite: StreamingAiRewriteConfig::default(),
        }
    }
}

impl Default for StreamingAiRewriteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint_url: "http://127.0.0.1:8080/v1/chat/completions".to_string(),
            model: "Qwen3-0.6B".to_string(),
            api_key_env: String::new(),
            timeout_ms: 260,
            debounce_ms: 260,
            min_visible_chars: 8,
            max_context_chars: 80,
            max_output_chars: 96,
        }
    }
}

impl Default for HudOverlayConfig {
    fn default() -> Self {
        Self {
            anchor: HudAnchor::BottomCenter,
            offset_x_px: 0,
            offset_y_px: -6,
            width_px: 560,
            min_width_px: 220,
            min_height_px: 84,
            min_text_width_px: 140,
            padding_x_px: 20,
            padding_y_px: 14,
            font_height_px: 34,
            font_weight: 700,
            font_family: "Microsoft YaHei".to_string(),
            text_align: HudTextAlign::Left,
            text_color: "#111111".to_string(),
            background_color: "#F3F3F3".to_string(),
            background_alpha: 212,
            corner_radius_px: 26,
            display_hold_ms: 650,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct HudOverlayConfigFile {
    anchor: Option<HudAnchor>,
    offset_x_px: Option<i32>,
    offset_y_px: Option<i32>,
    width_px: Option<i32>,
    min_width_px: Option<i32>,
    min_height_px: Option<i32>,
    min_text_width_px: Option<i32>,
    padding_x_px: Option<i32>,
    padding_y_px: Option<i32>,
    font_height_px: Option<i32>,
    font_weight: Option<i32>,
    font_family: Option<String>,
    text_align: Option<HudTextAlign>,
    text_color: Option<String>,
    background_color: Option<String>,
    background_alpha: Option<u8>,
    corner_radius_px: Option<i32>,
    display_hold_ms: Option<u64>,
    layout: HudOverlayLayoutSection,
    font: HudOverlayFontSection,
    background: HudOverlayBackgroundSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct HudOverlayLayoutSection {
    anchor: Option<HudAnchor>,
    offset_x_px: Option<i32>,
    offset_y_px: Option<i32>,
    width_px: Option<i32>,
    min_width_px: Option<i32>,
    min_height_px: Option<i32>,
    min_text_width_px: Option<i32>,
    padding_x_px: Option<i32>,
    padding_y_px: Option<i32>,
    corner_radius_px: Option<i32>,
    display_hold_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct HudOverlayFontSection {
    font_height_px: Option<i32>,
    font_weight: Option<i32>,
    font_family: Option<String>,
    text_align: Option<HudTextAlign>,
    text_color: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct HudOverlayBackgroundSection {
    background_color: Option<String>,
    background_alpha: Option<u8>,
}

impl HudOverlayConfigFile {
    fn into_config(self) -> HudOverlayConfig {
        let mut config = HudOverlayConfig::default();

        if let Some(value) = self.anchor.or(self.layout.anchor) {
            config.anchor = value;
        }
        if let Some(value) = self.offset_x_px.or(self.layout.offset_x_px) {
            config.offset_x_px = value;
        }
        if let Some(value) = self.offset_y_px.or(self.layout.offset_y_px) {
            config.offset_y_px = value;
        }
        if let Some(value) = self.width_px.or(self.layout.width_px) {
            config.width_px = value;
        }
        if let Some(value) = self.min_width_px.or(self.layout.min_width_px) {
            config.min_width_px = value;
        }
        if let Some(value) = self.min_height_px.or(self.layout.min_height_px) {
            config.min_height_px = value;
        }
        if let Some(value) = self.min_text_width_px.or(self.layout.min_text_width_px) {
            config.min_text_width_px = value;
        }
        if let Some(value) = self.padding_x_px.or(self.layout.padding_x_px) {
            config.padding_x_px = value;
        }
        if let Some(value) = self.padding_y_px.or(self.layout.padding_y_px) {
            config.padding_y_px = value;
        }
        if let Some(value) = self.corner_radius_px.or(self.layout.corner_radius_px) {
            config.corner_radius_px = value;
        }
        if let Some(value) = self.display_hold_ms.or(self.layout.display_hold_ms) {
            config.display_hold_ms = value;
        }
        if let Some(value) = self.font_height_px.or(self.font.font_height_px) {
            config.font_height_px = value;
        }
        if let Some(value) = self.font_weight.or(self.font.font_weight) {
            config.font_weight = value;
        }
        if let Some(value) = self.font_family.or(self.font.font_family) {
            config.font_family = value;
        }
        if let Some(value) = self.text_align.or(self.font.text_align) {
            config.text_align = value;
        }
        if let Some(value) = self.text_color.or(self.font.text_color) {
            config.text_color = value;
        }
        if let Some(value) = self.background_color.or(self.background.background_color) {
            config.background_color = value;
        }
        if let Some(value) = self.background_alpha.or(self.background.background_alpha) {
            config.background_alpha = value;
        }

        config
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_save_to_desktop: false,
        }
    }
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self { repeat_count: 1 }
    }
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            launch_at_login: true,
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model_dir: "models/sense-voice".to_string(),
            provider: "cpu".to_string(),
            sample_rate_hz: 16_000,
            language: "auto".to_string(),
            use_itn: true,
            num_threads: 4,
        }
    }
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            auto_activate_threshold: 2,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file_name: "ainput.log".to_string(),
        }
    }
}

pub fn bootstrap() -> Result<Bootstrap> {
    let runtime_paths = RuntimePaths::discover()?;
    let mut config = load_or_create_config(&runtime_paths)?;
    config.hud_overlay = load_or_create_hud_overlay_config(&runtime_paths)?;
    let log_guard = init_logging(&runtime_paths, &config.logging)?;

    tracing::info!(
        config_file = %runtime_paths.config_file.display(),
        logs_dir = %runtime_paths.logs_dir.display(),
        models_dir = %runtime_paths.models_dir.display(),
        voice_hotkey = %config.hotkeys.voice_input,
        voice_mode = ?config.voice.mode,
        capture_hotkey = %config.hotkeys.screen_capture,
        "ainput shell bootstrap complete"
    );

    Ok(Bootstrap {
        config,
        runtime_paths,
        _log_guard: log_guard,
    })
}

pub fn save_config(paths: &RuntimePaths, config: &AppConfig) -> Result<()> {
    let payload = render_config_file(config);
    write_utf8_bom_text_file(&paths.config_file, &payload)
        .with_context(|| format!("write config file {}", paths.config_file.display()))?;
    Ok(())
}

pub fn load_hud_overlay_config(paths: &RuntimePaths) -> Result<HudOverlayConfig> {
    if paths.hud_overlay_file.exists() {
        return read_hud_overlay_config_file(&paths.hud_overlay_file);
    }

    load_or_create_hud_overlay_config(paths)
}

impl RuntimePaths {
    pub fn discover() -> Result<Self> {
        let root_dir = if let Ok(value) = env::var("AINPUT_ROOT") {
            PathBuf::from(value)
        } else {
            discover_root_dir()?
        };

        Ok(Self {
            config_file: root_dir.join("config").join("ainput.toml"),
            hud_overlay_file: root_dir.join("config").join("hud-overlay.toml"),
            legacy_config_file: root_dir.join("config").join("ainput.config.json"),
            logs_dir: root_dir.join("logs"),
            models_dir: root_dir.join("models").join("sense-voice"),
            root_dir,
        })
    }
}

fn discover_root_dir() -> Result<PathBuf> {
    let current_dir = env::current_dir().context("resolve current working directory")?;
    if has_project_root_markers(&current_dir) {
        return Ok(current_dir);
    }

    let exe_dir = env::current_exe()
        .context("resolve current executable path")?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("resolve executable directory"))?;

    if has_runtime_asset_markers(&exe_dir) {
        return Ok(exe_dir);
    }

    if let Some(found) = find_runtime_root(&exe_dir) {
        return Ok(found);
    }

    if let Some(found) = find_project_root(&exe_dir) {
        return Ok(found);
    }

    Err(anyhow!(
        "could not determine ainput runtime root from current directory {} or executable directory {}",
        current_dir.display(),
        exe_dir.display()
    ))
}

fn find_runtime_root(start: &Path) -> Option<PathBuf> {
    for candidate in start.ancestors() {
        if has_runtime_asset_markers(candidate) {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    for candidate in start.ancestors() {
        if has_project_root_markers(candidate) {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn has_project_root_markers(candidate: &Path) -> bool {
    candidate.join("Cargo.toml").exists()
}

fn has_runtime_asset_markers(candidate: &Path) -> bool {
    candidate.join("config").join("ainput.toml").exists()
        || candidate.join("config").join("ainput.config.json").exists()
        || candidate.join("models").join("sense-voice").exists()
}

fn load_or_create_config(paths: &RuntimePaths) -> Result<AppConfig> {
    if let Some(parent) = paths.config_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }

    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("create logs directory {}", paths.logs_dir.display()))?;
    fs::create_dir_all(&paths.models_dir)
        .with_context(|| format!("create models directory {}", paths.models_dir.display()))?;

    if paths.config_file.exists() {
        let raw = read_text_file_strip_utf8_bom(&paths.config_file)
            .with_context(|| format!("read config file {}", paths.config_file.display()))?;
        let config = toml::from_str(&raw)
            .with_context(|| format!("parse config file {}", paths.config_file.display()))?;
        return Ok(config);
    }

    let config = if paths.legacy_config_file.exists() {
        let raw = fs::read_to_string(&paths.legacy_config_file).with_context(|| {
            format!(
                "read legacy config file {}",
                paths.legacy_config_file.display()
            )
        })?;
        let legacy = serde_json::from_str::<LegacyAppConfig>(&raw).with_context(|| {
            format!(
                "parse legacy config file {}",
                paths.legacy_config_file.display()
            )
        })?;
        legacy.into_current()
    } else {
        AppConfig::default()
    };

    save_config(paths, &config)?;
    Ok(config)
}

fn load_or_create_hud_overlay_config(paths: &RuntimePaths) -> Result<HudOverlayConfig> {
    if let Some(parent) = paths.hud_overlay_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create HUD config directory {}", parent.display()))?;
    }

    if paths.hud_overlay_file.exists() {
        let config = read_hud_overlay_config_file(&paths.hud_overlay_file)?;
        let payload = render_hud_overlay_config_file(&config);
        write_utf16le_bom_text_file(&paths.hud_overlay_file, &payload).with_context(|| {
            format!(
                "rewrite HUD config file {}",
                paths.hud_overlay_file.display()
            )
        })?;
        return Ok(config);
    }

    let config = HudOverlayConfig::default();
    let payload = render_hud_overlay_config_file(&config);
    write_utf16le_bom_text_file(&paths.hud_overlay_file, &payload)
        .with_context(|| format!("write HUD config file {}", paths.hud_overlay_file.display()))?;
    Ok(config)
}

fn read_hud_overlay_config_file(path: &Path) -> Result<HudOverlayConfig> {
    let raw = read_text_file_with_bom_support(path)
        .with_context(|| format!("read HUD config file {}", path.display()))?;
    let normalized = normalize_legacy_hud_overlay_text(&raw);
    toml::from_str::<HudOverlayConfigFile>(&normalized)
        .map(HudOverlayConfigFile::into_config)
        .with_context(|| format!("parse HUD config file {}", path.display()))
}

fn normalize_legacy_hud_overlay_text(raw: &str) -> String {
    const HUD_KEYS: &[&str] = &[
        "anchor",
        "offset_x_px",
        "offset_y_px",
        "width_px",
        "min_width_px",
        "min_height_px",
        "min_text_width_px",
        "padding_x_px",
        "padding_y_px",
        "corner_radius_px",
        "display_hold_ms",
        "font_height_px",
        "font_weight",
        "font_family",
        "text_align",
        "text_color",
        "background_color",
        "background_alpha",
    ];

    raw.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                for key in HUD_KEYS {
                    let marker = format!("{key} =");
                    if let Some(position) = line.find(&marker) {
                        let valid_boundary = position == 0
                            || !line[..position]
                                .chars()
                                .last()
                                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
                        if valid_boundary {
                            return line[position..].to_string();
                        }
                    }
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_text_file_strip_utf8_bom(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read text file {}", path.display()))?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("decode UTF-8 text file {}", path.display()))?;
    Ok(strip_utf8_bom(&text).to_string())
}

fn read_text_file_with_bom_support(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read text file {}", path.display()))?;
    decode_text_file_with_bom(path, &bytes)
}

fn write_utf8_bom_text_file(path: &Path, content: &str) -> Result<()> {
    let normalized = strip_utf8_bom(content);
    let mut bytes = Vec::with_capacity(3 + normalized.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(normalized.as_bytes());
    fs::write(path, bytes).with_context(|| format!("write UTF-8 BOM file {}", path.display()))?;
    Ok(())
}

fn write_utf16le_bom_text_file(path: &Path, content: &str) -> Result<()> {
    let normalized = strip_utf8_bom(content);
    let mut bytes = Vec::with_capacity(2 + normalized.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in normalized.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, bytes)
        .with_context(|| format!("write UTF-16LE BOM file {}", path.display()))?;
    Ok(())
}

fn decode_text_file_with_bom(path: &Path, bytes: &[u8]) -> Result<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec())
            .with_context(|| format!("decode UTF-8 BOM text file {}", path.display()));
    }

    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16_units(path, rest, true);
    }

    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16_units(path, rest, false);
    }

    String::from_utf8(bytes.to_vec())
        .map(|text| strip_utf8_bom(&text).to_string())
        .with_context(|| format!("decode UTF-8 text file {}", path.display()))
}

fn decode_utf16_units(path: &Path, bytes: &[u8], little_endian: bool) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(anyhow!(
            "decode UTF-16 text file {}: odd byte length",
            path.display()
        ));
    }

    let units = bytes
        .chunks_exact(2)
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();

    String::from_utf16(&units)
        .with_context(|| format!("decode UTF-16 text file {}", path.display()))
}

fn strip_utf8_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

fn render_config_file(config: &AppConfig) -> String {
    format!(
        r#"# ainput 配置文件
# 说明：
# 1. 这是 ainput 的正式配置文件，按 TOML 格式读取。
# 2. `#` 开头的内容是中文注释，不会影响程序运行。
# 3. 修改后通常重启 ainput 即可生效。

[hotkeys]
# 按住说话的主热键。
# 当前默认：Alt+Z
voice_input = "{voice_input}"

# 触发截图的主热键。
# 当前默认：Alt+X
screen_capture = "{screen_capture}"

# 是否启用“鼠标中键长按录音”。
# true = 启用，false = 关闭
mouse_middle_hold_enabled = {mouse_middle_hold_enabled}

[voice]
# 是否启用语音输入主功能。
enabled = {voice_enabled}

# 当前语音模式。
# fast = 极速语音识别
# streaming = 流式语音识别
mode = "{voice_mode}"

# 是否优先尝试“直接粘贴到当前输入框”。
# true = 先写剪贴板再模拟 Ctrl+V
# false = 不做直贴，只走剪贴板
prefer_direct_paste = {prefer_direct_paste}

# 当直接粘贴失败时，是否退回到“仅写入剪贴板”。
fallback_to_clipboard = {fallback_to_clipboard}

# 语音历史文件名。
# 一行一条，滚动保留最近 history_limit 条。
history_file_name = "{history_file_name}"

# 语音历史最多保留多少条。
history_limit = {history_limit}

[voice.streaming]
# 是否启用流式语音识别模式。
enabled = {streaming_enabled}

# 流式 ASR 模型目录。
model_dir = "{streaming_model_dir}"

# 是否显示流式语音面板。
panel_enabled = {streaming_panel_enabled}

# 是否启用流式短句整理。
rewrite_enabled = {streaming_rewrite_enabled}

# 流式标点模型目录。
punctuation_model_dir = "{streaming_punctuation_model_dir}"

# 流式标点模型线程数。
punctuation_num_threads = {streaming_punctuation_num_threads}

# 流式音频块时长（毫秒）。
# 数值越小，HUD 更新会更勤；数值越大，吞吐更稳但刷新会更慢。
chunk_ms = {streaming_chunk_ms}

[voice.streaming.ai_rewrite]
# 是否启用本地 AI 实时改写。
# 只会改当前还在变化的尾巴；HUD 显示什么，最终就提交什么。
enabled = {streaming_ai_rewrite_enabled}

# 本地 OpenAI 兼容接口地址。
# 例如 llama.cpp server、vLLM 或别的本地兼容服务。
endpoint_url = "{streaming_ai_rewrite_endpoint_url}"

# 改写模型名称。
model = "{streaming_ai_rewrite_model}"

# 可选：从哪个环境变量读取 API Key。
# 纯本地服务通常留空即可。
api_key_env = "{streaming_ai_rewrite_api_key_env}"

# 单次 AI 改写超时（毫秒）。
timeout_ms = {streaming_ai_rewrite_timeout_ms}

# 两次 AI 改写之间的最小间隔（毫秒）。
debounce_ms = {streaming_ai_rewrite_debounce_ms}

# 至少累计多少个可见字，才触发 AI 改写。
min_visible_chars = {streaming_ai_rewrite_min_visible_chars}

# 发送给模型的冻结前缀最大字符数。
max_context_chars = {streaming_ai_rewrite_max_context_chars}

# 允许模型返回的尾巴最大字符数。
max_output_chars = {streaming_ai_rewrite_max_output_chars}

[capture]
# 是否启用截图主功能。
enabled = {capture_enabled}

# 截图完成后，是否额外自动保存 PNG 到桌面。
auto_save_to_desktop = {auto_save_to_desktop}

[automation]
# 按键精灵默认回放轮数。
# 托盘里切换后会立即生效，并在下次启动时继续沿用。
repeat_count = {automation_repeat_count}

[recording]
# 是否启用录屏主功能。
enabled = {recording_enabled}

# 是否录制系统播放音频。
record_audio = {recording_audio}

# 是否录制鼠标移动。
capture_mouse = {recording_capture_mouse}

# 录屏帧率预设，只支持 30 / 60 / 90 / 144。
fps = {recording_fps}

# 录屏画质预设。
# low = CRF 28，medium = CRF 20，high = CRF 15
quality = "{recording_quality}"

[recording.watermark]
# 是否启用录屏水印。
enabled = {recording_watermark_enabled}

# 水印文本。
text = "{recording_watermark_text}"

# 水印位置。
# 支持：left_top / right_top / left_bottom / right_bottom / moving_flash / random_walk
position = "{recording_watermark_position}"

[startup]
# 是否在 Windows 登录后自动启动 ainput。
launch_at_login = {launch_at_login}

[asr]
# 语音识别模型目录。
# 默认是项目根目录下的 models/sense-voice
model_dir = "{model_dir}"

# 推理后端。
# 当前一般保持 cpu 即可。
provider = "{provider}"

# 录音采样率。
# 默认 16000，不建议随便改。
sample_rate_hz = {sample_rate_hz}

# 识别语言。
# auto = 自动判断
language = "{language}"

# 是否启用 ITN（Inverse Text Normalization）。
# 一般保持 true。
use_itn = {use_itn}

# ASR 推理线程数。
# 线程越多不一定越快，默认 4。
num_threads = {num_threads}

[learning]
# 自动学习从“候选”变成“正式生效”所需的最小次数。
# 例如 2 表示同一纠错出现两次后自动激活。
auto_activate_threshold = {auto_activate_threshold}

[logging]
# 日志级别。
# 常用：info / debug / warn / error
level = "{log_level}"

# 主日志文件名。
file_name = "{log_file_name}"
"#,
        voice_input = config.hotkeys.voice_input,
        screen_capture = config.hotkeys.screen_capture,
        mouse_middle_hold_enabled = config.hotkeys.mouse_middle_hold_enabled,
        voice_enabled = config.voice.enabled,
        voice_mode = match config.voice.mode {
            VoiceMode::Fast => "fast",
            VoiceMode::Streaming => "streaming",
        },
        prefer_direct_paste = config.voice.prefer_direct_paste,
        fallback_to_clipboard = config.voice.fallback_to_clipboard,
        history_file_name = config.voice.history_file_name,
        history_limit = config.voice.history_limit,
        streaming_enabled = config.voice.streaming.enabled,
        streaming_model_dir = config.voice.streaming.model_dir,
        streaming_panel_enabled = config.voice.streaming.panel_enabled,
        streaming_rewrite_enabled = config.voice.streaming.rewrite_enabled,
        streaming_punctuation_model_dir = config.voice.streaming.punctuation_model_dir,
        streaming_punctuation_num_threads = config.voice.streaming.punctuation_num_threads,
        streaming_chunk_ms = config.voice.streaming.chunk_ms,
        streaming_ai_rewrite_enabled = config.voice.streaming.ai_rewrite.enabled,
        streaming_ai_rewrite_endpoint_url = config
            .voice
            .streaming
            .ai_rewrite
            .endpoint_url
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        streaming_ai_rewrite_model = config
            .voice
            .streaming
            .ai_rewrite
            .model
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        streaming_ai_rewrite_api_key_env = config
            .voice
            .streaming
            .ai_rewrite
            .api_key_env
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        streaming_ai_rewrite_timeout_ms = config.voice.streaming.ai_rewrite.timeout_ms,
        streaming_ai_rewrite_debounce_ms = config.voice.streaming.ai_rewrite.debounce_ms,
        streaming_ai_rewrite_min_visible_chars =
            config.voice.streaming.ai_rewrite.min_visible_chars,
        streaming_ai_rewrite_max_context_chars =
            config.voice.streaming.ai_rewrite.max_context_chars,
        streaming_ai_rewrite_max_output_chars = config.voice.streaming.ai_rewrite.max_output_chars,
        capture_enabled = config.capture.enabled,
        auto_save_to_desktop = config.capture.auto_save_to_desktop,
        automation_repeat_count = config.automation.repeat_count,
        recording_enabled = config.recording.enabled,
        recording_audio = config.recording.record_audio,
        recording_capture_mouse = config.recording.capture_mouse,
        recording_fps = config.recording.fps,
        recording_quality = match config.recording.quality {
            ainput_recording::VideoQuality::Low => "low",
            ainput_recording::VideoQuality::Medium => "medium",
            ainput_recording::VideoQuality::High => "high",
        },
        recording_watermark_enabled = config.recording.watermark.enabled,
        recording_watermark_text = config
            .recording
            .watermark
            .text
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        recording_watermark_position = match config.recording.watermark.position {
            ainput_recording::WatermarkPosition::LeftTop => "left_top",
            ainput_recording::WatermarkPosition::RightTop => "right_top",
            ainput_recording::WatermarkPosition::LeftBottom => "left_bottom",
            ainput_recording::WatermarkPosition::RightBottom => "right_bottom",
            ainput_recording::WatermarkPosition::MovingFlash => "moving_flash",
            ainput_recording::WatermarkPosition::RandomWalk => "random_walk",
            ainput_recording::WatermarkPosition::Center => "center",
        },
        launch_at_login = config.startup.launch_at_login,
        model_dir = config.asr.model_dir,
        provider = config.asr.provider,
        sample_rate_hz = config.asr.sample_rate_hz,
        language = config.asr.language,
        use_itn = config.asr.use_itn,
        num_threads = config.asr.num_threads,
        auto_activate_threshold = config.learning.auto_activate_threshold,
        log_level = config.logging.level,
        log_file_name = config.logging.file_name,
    )
}

fn render_hud_overlay_config_file(config: &HudOverlayConfig) -> String {
    format!(
        r##"# ainput HUD 参数文档
# 用法说明：
# 1. 这是专门给流式语音 HUD 用的参数文件。
# 2. 每个参数上方都有中文注释，按注释改完后保存文件。
# 3. 保存文件后会自动热加载，不需要重启。
# 4. 如果当前 HUD 正显示着，保存后会立刻按新参数刷新位置、大小、颜色和字体。
# 5. 颜色统一写成 "#RRGGBB" 格式，例如黑色 "#111111"，白色 "#FFFFFF"。

[layout]
# HUD 停靠位置。
# 可选值：
# - "bottom_center" = 屏幕正下方，默认推荐
# - "bottom_left" = 屏幕左下角
anchor = "{anchor}"

# 水平方向偏移（像素）。
# 正数 = 往右移，负数 = 往左移。
offset_x_px = {offset_x_px}

# 垂直方向偏移（像素）。
# 正数 = 往下移，负数 = 往上移。
# 默认给一个轻微负值，让 HUD 正好贴在任务栏上方。
offset_y_px = {offset_y_px}

# HUD 的目标宽度（像素）。
# 字太长时会在这个宽度附近自动换行。
width_px = {width_px}

# HUD 的最小宽度（像素）。
# 字很少时也不会比这个更窄。
min_width_px = {min_width_px}

# HUD 的最小高度（像素）。
# 字很少时也不会比这个更矮。
min_height_px = {min_height_px}

# 文本区域的最小宽度（像素）。
# 如果只显示“请说话”这类短字，仍然至少保留这么宽的文本区。
min_text_width_px = {min_text_width_px}

# HUD 左右内边距（像素）。
# 调大后文字离边缘更远，看起来更松。
padding_x_px = {padding_x_px}

# HUD 上下内边距（像素）。
# 调大后 HUD 会更高，文字不容易贴边。
padding_y_px = {padding_y_px}

# HUD 圆角半径（像素）。
# 0 = 直角；数值越大越圆。
corner_radius_px = {corner_radius_px}

# 松手后“已识别”提示至少保留多久（毫秒）。
# 想让提示闪得更快就调小，想看得更久就调大。
display_hold_ms = {display_hold_ms}

[font]
# 字号高度（像素）。
# 这是你最常调的参数；觉得太小就把它调大。
font_height_px = {font_height_px}

# 字重。
# 常用值：
# - 400 = 常规
# - 500 = 中等
# - 700 = 加粗
font_weight = {font_weight}

# 字体名称。
# 常见可用值：
# - "Microsoft YaHei"
# - "PingFang SC"
# - "SimHei"
font_family = "{font_family}"

# 文本对齐方式。
# 可选值：
# - "left" = 左对齐
# - "center" = 居中
text_align = "{text_align}"

# 文字颜色，格式固定为 "#RRGGBB"。
text_color = "{text_color}"

[background]
# 背景颜色，格式固定为 "#RRGGBB"。
background_color = "{background_color}"

# 背景透明度。
# 0 = 完全透明；255 = 完全不透明。
background_alpha = {background_alpha}
"##,
        anchor = match config.anchor {
            HudAnchor::BottomLeft => "bottom_left",
            HudAnchor::BottomCenter => "bottom_center",
        },
        offset_x_px = config.offset_x_px,
        offset_y_px = config.offset_y_px,
        width_px = config.width_px,
        min_width_px = config.min_width_px,
        min_height_px = config.min_height_px,
        min_text_width_px = config.min_text_width_px,
        padding_x_px = config.padding_x_px,
        padding_y_px = config.padding_y_px,
        corner_radius_px = config.corner_radius_px,
        display_hold_ms = config.display_hold_ms,
        font_height_px = config.font_height_px,
        font_weight = config.font_weight,
        font_family = config
            .font_family
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        text_align = match config.text_align {
            HudTextAlign::Left => "left",
            HudTextAlign::Center => "center",
        },
        text_color = config.text_color,
        background_color = config.background_color,
        background_alpha = config.background_alpha,
    )
}

fn init_logging(paths: &RuntimePaths, logging: &LoggingConfig) -> Result<WorkerGuard> {
    let log_file = resolve_log_file(paths, logging);
    let log_dir = log_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.logs_dir.clone());
    let log_name = log_file
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("ainput.log"));
    let file_appender = tracing_appender::rolling::never(log_dir, log_name);
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(logging.level.as_str()))
        .context("build log filter")?;

    fmt()
        .with_env_filter(env_filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_line_number(true)
        .try_init()
        .map_err(|err| anyhow!("initialize tracing subscriber: {err}"))?;

    Ok(guard)
}

fn resolve_log_file(paths: &RuntimePaths, logging: &LoggingConfig) -> PathBuf {
    let candidate = Path::new(&logging.file_name);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        paths.logs_dir.join(candidate)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyAppConfig {
    shortcuts: LegacyShortcutConfig,
    startup: StartupConfig,
    asr: AsrConfig,
    capture: LegacyCaptureConfig,
    output: LegacyOutputConfig,
    logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct LegacyShortcutConfig {
    push_to_talk: String,
    screen_capture: String,
    mouse_middle_hold_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyCaptureConfig {
    auto_save_to_desktop: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct LegacyOutputConfig {
    prefer_direct_paste: bool,
    fallback_to_clipboard: bool,
}

impl Default for LegacyShortcutConfig {
    fn default() -> Self {
        Self {
            push_to_talk: "Alt+Z".to_string(),
            screen_capture: "Alt+X".to_string(),
            mouse_middle_hold_enabled: false,
        }
    }
}

impl Default for LegacyOutputConfig {
    fn default() -> Self {
        Self {
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
        }
    }
}

impl LegacyAppConfig {
    fn into_current(self) -> AppConfig {
        let mut config = AppConfig {
            startup: self.startup,
            asr: self.asr,
            logging: self.logging,
            ..AppConfig::default()
        };

        config.hotkeys.mouse_middle_hold_enabled = self.shortcuts.mouse_middle_hold_enabled;
        config.capture.auto_save_to_desktop = self.capture.auto_save_to_desktop;
        config.voice.prefer_direct_paste = self.output.prefer_direct_paste;
        config.voice.fallback_to_clipboard = self.output.fallback_to_clipboard;

        if !self.shortcuts.push_to_talk.trim().is_empty() {
            config.hotkeys.voice_input = self.shortcuts.push_to_talk;
        }
        if !self.shortcuts.screen_capture.trim().is_empty() {
            config.hotkeys.screen_capture = self.shortcuts.screen_capture;
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn load_hud_overlay_config_does_not_rewrite_existing_file() {
        let paths = temp_runtime_paths("hot-reload-readonly");
        let mut config = HudOverlayConfig::default();
        config.font_height_px = 72;
        config.width_px = 880;
        write_hud_config(&paths, &config);

        let before_bytes = fs::read(&paths.hud_overlay_file).expect("read hud config before");
        let before_modified = fs::metadata(&paths.hud_overlay_file)
            .expect("stat hud config before")
            .modified()
            .expect("hud config modified before");

        std::thread::sleep(Duration::from_millis(20));
        let loaded = load_hud_overlay_config(&paths).expect("load hud overlay config");
        let after_bytes = fs::read(&paths.hud_overlay_file).expect("read hud config after");
        let after_modified = fs::metadata(&paths.hud_overlay_file)
            .expect("stat hud config after")
            .modified()
            .expect("hud config modified after");

        assert_eq!(loaded.font_height_px, 72);
        assert_eq!(loaded.width_px, 880);
        assert_eq!(before_bytes, after_bytes);
        assert_eq!(before_modified, after_modified);
    }

    #[test]
    fn startup_hud_loader_preserves_existing_values() {
        let paths = temp_runtime_paths("startup-preserves-values");
        let mut config = HudOverlayConfig::default();
        config.font_height_px = 68;
        config.offset_y_px = -32;
        write_hud_config(&paths, &config);

        let loaded = load_or_create_hud_overlay_config(&paths).expect("startup hud load");
        let reloaded = read_hud_overlay_config_file(&paths.hud_overlay_file).expect("reload hud");

        assert_eq!(loaded.font_height_px, 68);
        assert_eq!(loaded.offset_y_px, -32);
        assert_eq!(reloaded.font_height_px, 68);
        assert_eq!(reloaded.offset_y_px, -32);
    }

    #[test]
    fn project_root_markers_require_cargo_toml() {
        let dir = unique_temp_dir("project-root-marker");
        fs::create_dir_all(&dir).expect("create temp root");
        fs::write(dir.join("AGENTS.md"), "# temp").expect("write temp agents");

        assert!(!has_project_root_markers(&dir));

        fs::write(dir.join("Cargo.toml"), "[workspace]\n").expect("write temp cargo");
        assert!(has_project_root_markers(&dir));
    }

    #[test]
    fn legacy_commented_key_lines_are_salvaged() {
        let raw = r##"[layout]
# 旧坏格式 width_px = 900
# 旧坏格式 min_width_px = 260

[font]
# 旧坏格式 font_height_px = 72
font_weight = 700
font_family = "Microsoft YaHei"
text_align = "center"
text_color = "#111111"

[background]
# 旧坏格式 background_alpha = 255
"##;

        let config =
            toml::from_str::<HudOverlayConfigFile>(&normalize_legacy_hud_overlay_text(raw))
                .expect("parse salvaged hud config")
                .into_config();

        assert_eq!(config.width_px, 900);
        assert_eq!(config.min_width_px, 260);
        assert_eq!(config.font_height_px, 72);
        assert_eq!(config.background_alpha, 255);
    }

    #[test]
    fn render_config_file_contains_streaming_ai_rewrite_section() {
        let rendered = render_config_file(&AppConfig::default());
        assert!(rendered.contains("[voice.streaming.ai_rewrite]"));
        assert!(rendered.contains("endpoint_url = \"http://127.0.0.1:8080/v1/chat/completions\""));
        assert!(rendered.contains("model = \"Qwen3-0.6B\""));
    }

    #[test]
    fn streaming_ai_rewrite_defaults_are_disabled_but_present() {
        let config = StreamingAiRewriteConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.timeout_ms, 260);
        assert_eq!(config.debounce_ms, 260);
        assert_eq!(config.max_output_chars, 96);
    }

    fn temp_runtime_paths(label: &str) -> RuntimePaths {
        let root_dir = unique_temp_dir(label);
        RuntimePaths {
            config_file: root_dir.join("config").join("ainput.toml"),
            hud_overlay_file: root_dir.join("config").join("hud-overlay.toml"),
            legacy_config_file: root_dir.join("config").join("ainput.config.json"),
            logs_dir: root_dir.join("logs"),
            models_dir: root_dir.join("models").join("sense-voice"),
            root_dir,
        }
    }

    fn write_hud_config(paths: &RuntimePaths, config: &HudOverlayConfig) {
        fs::create_dir_all(
            paths
                .hud_overlay_file
                .parent()
                .expect("hud config parent directory"),
        )
        .expect("create hud config directory");
        let payload = render_hud_overlay_config_file(config);
        write_utf16le_bom_text_file(&paths.hud_overlay_file, &payload).expect("write hud config");
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        let dir = env::temp_dir().join(format!("ainput-shell-{label}-{unique}"));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove stale temp dir");
        }
        dir
    }
}
