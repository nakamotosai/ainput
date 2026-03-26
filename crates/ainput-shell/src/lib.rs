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
    pub startup: StartupConfig,
    pub asr: AsrConfig,
    pub learning: LearningConfig,
    pub logging: LoggingConfig,
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
    pub prefer_direct_paste: bool,
    pub fallback_to_clipboard: bool,
    pub history_file_name: String,
    pub history_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub enabled: bool,
    pub auto_save_to_desktop: bool,
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
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
            history_file_name: "voice-history.log".to_string(),
            history_limit: 500,
        }
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
    let config = load_or_create_config(&runtime_paths)?;
    let log_guard = init_logging(&runtime_paths, &config.logging)?;

    tracing::info!(
        config_file = %runtime_paths.config_file.display(),
        logs_dir = %runtime_paths.logs_dir.display(),
        models_dir = %runtime_paths.models_dir.display(),
        voice_hotkey = %config.hotkeys.voice_input,
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
    fs::write(&paths.config_file, payload)
        .with_context(|| format!("write config file {}", paths.config_file.display()))?;
    Ok(())
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
    candidate.join("AGENTS.md").exists() || candidate.join("Cargo.toml").exists()
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
        let raw = fs::read_to_string(&paths.config_file)
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

[capture]
# 是否启用截图主功能。
enabled = {capture_enabled}

# 截图完成后，是否额外自动保存 PNG 到桌面。
auto_save_to_desktop = {auto_save_to_desktop}

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
        prefer_direct_paste = config.voice.prefer_direct_paste,
        fallback_to_clipboard = config.voice.fallback_to_clipboard,
        history_file_name = config.voice.history_file_name,
        history_limit = config.voice.history_limit,
        capture_enabled = config.capture.enabled,
        auto_save_to_desktop = config.capture.auto_save_to_desktop,
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
