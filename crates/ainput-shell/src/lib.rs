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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub shortcuts: ShortcutConfig,
    pub asr: AsrConfig,
    pub output: OutputConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub push_to_talk: String,
    pub mouse_middle_hold_enabled: bool,
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
pub struct OutputConfig {
    pub prefer_direct_paste: bool,
    pub fallback_to_clipboard: bool,
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
    pub logs_dir: PathBuf,
    pub models_dir: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            shortcuts: ShortcutConfig::default(),
            asr: AsrConfig::default(),
            output: OutputConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            push_to_talk: "Ctrl+Win".to_string(),
            mouse_middle_hold_enabled: true,
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

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
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
        shortcut = %config.shortcuts.push_to_talk,
        "ainput shell bootstrap complete"
    );

    Ok(Bootstrap {
        config,
        runtime_paths,
        _log_guard: log_guard,
    })
}

pub fn save_config(paths: &RuntimePaths, config: &AppConfig) -> Result<()> {
    let payload = serde_json::to_string_pretty(config).context("serialize config")?;
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
            config_file: root_dir.join("config").join("ainput.config.json"),
            logs_dir: root_dir.join("logs"),
            models_dir: root_dir.join("models").join("sense-voice"),
            root_dir,
        })
    }
}

fn discover_root_dir() -> Result<PathBuf> {
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

    let current_dir = env::current_dir().context("resolve current working directory")?;
    if let Some(found) = find_runtime_root(&current_dir) {
        return Ok(found);
    }

    Ok(current_dir)
}

fn find_runtime_root(start: &Path) -> Option<PathBuf> {
    for candidate in start.ancestors() {
        if has_runtime_asset_markers(candidate) {
            return Some(candidate.to_path_buf());
        }
    }

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
    candidate.join("config").join("ainput.config.json").exists()
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

    if !paths.config_file.exists() {
        let default_config = AppConfig::default();
        let payload =
            serde_json::to_string_pretty(&default_config).context("serialize default config")?;
        fs::write(&paths.config_file, payload)
            .with_context(|| format!("write default config {}", paths.config_file.display()))?;
        return Ok(default_config);
    }

    let raw = fs::read_to_string(&paths.config_file)
        .with_context(|| format!("read config file {}", paths.config_file.display()))?;
    let config = serde_json::from_str(&raw)
        .with_context(|| format!("parse config file {}", paths.config_file.display()))?;
    Ok(config)
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
