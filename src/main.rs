#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai_rewrite;
mod api_config;
mod api_settings_panel;
mod asr_pool;
mod audio;
mod cloud_asr;
mod config;
mod debug_panel;
mod history;
mod history_panel;
mod hotkey;
mod hotkey_panel;
mod hotkey_user;
mod hud;
mod local_asr;
mod modes;
mod output;
mod personal_corrections;
mod pipeline;
mod resample;
mod rewrite_language;
mod rewrite_prompt;
mod rewrite_prompt_panel;
mod suspect_terms;
mod term_embeddings;
mod tray;
mod voice_command;
mod voice_command_panel;
mod web_ui;
mod worker;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;

use anyhow::{Context, Result};
use tracing::{error, info};

#[cfg(windows)]
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    PROCESS_SYSTEM_DPI_AWARE, SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, SetProcessDPIAware, MB_ICONERROR, MB_OK};
#[cfg(windows)]
use windows::core::PCWSTR;

fn main() {
    install_panic_hook();
    #[cfg(windows)]
    {
        let _single_instance_mutex = match acquire_single_instance_lock() {
            Ok(Some(handle)) => handle,
            Ok(None) => {
                show_already_running();
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("ainput: single-instance lock failed: {error:#}");
                show_startup_error(&format!("单实例锁创建失败：{error:#}"));
                std::process::exit(1);
            }
        };
    }
    if let Err(error) = run_app() {
        let message = format!("{error:#}");
        eprintln!("ainput failed: {message}");
        error!(error = %message, "ainput exited with error");
        show_startup_error(&message);
        std::process::exit(1);
    }
}

fn run_app() -> Result<()> {
    let dpi_awareness = configure_process_dpi_awareness();
    let install_root = resolve_install_root().context("resolve install root")?;
    let state_root = resolve_state_root(&install_root).context("resolve state root")?;
    migrate_state_root(&install_root, &state_root).context("migrate state root")?;
    let _log_guard = init_logging(&state_root)?;
    info!(
        install_root = %install_root.display(),
        state_root = %state_root.display(),
        version = env!("CARGO_PKG_VERSION"),
        dpi_awareness,
        "ainput starting"
    );

    let api_connections = api_config::ApiConnections::load_or_create(&state_root, &install_root)
        .context("load API connections config")?;
    let config_path = resolve_config_path(&state_root, &install_root);
    let mut config = config::AppConfig::load(&config_path)
        .with_context(|| format!("load config {}", config_path.display()))?;
    config.apply_api_connections(&api_connections.config);
    let hotkey_user = hotkey_user::HotkeyUserController::load_or_default(
        state_root.join("config").join("hotkey-user.toml"),
        &config.profiles.local_nonstreaming.hotkey,
    );
    hotkey_user.apply_to_profiles(&mut config.profiles);
    personal_corrections::init(state_root.join("config").join("personal-corrections.json"));
    let rewrite_language = rewrite_language::RewriteLanguageController::new(
        config.rewrite.enabled,
        config.rewrite.streaming_enabled,
        config.rewrite.output_language,
        state_root.join("config").join("rewrite-user.toml"),
    );
    info!(
        default_mode = ?config.mode.default,
        hotkey = %config.hotkey.voice_input,
        hotkey_activation_delay_ms = config.hotkey.activation_delay_ms,
        streaming_hotkey = %config.profiles.streaming.hotkey,
        streaming_activation_delay_ms = config.profiles.streaming.activation_delay_ms,
        whisper_hotkey = %config.profiles.whisper.hotkey,
        whisper_activation_delay_ms = config.profiles.whisper.activation_delay_ms,
        whisper_suppress_key = config.profiles.whisper.suppress_key,
        local_nonstreaming_hotkey = %config.profiles.local_nonstreaming.hotkey,
        local_nonstreaming_activation_delay_ms = config.profiles.local_nonstreaming.activation_delay_ms,
        local_nonstreaming_enabled = config.profiles.local_nonstreaming.enabled,
        asr_endpoint = %config.asr.endpoint_url,
        local_nonstreaming_model_dir = %config.local_nonstreaming.model_dir,
        api_config_path = %api_connections.path.display(),
        rewrite_endpoint = %config.rewrite.endpoint_url,
        rewrite_model = %config.rewrite.model,
        rewrite_fallback_models = ?config.rewrite.fallback_models,
        api_key_env = %config.rewrite.api_key_env,
        api_key_inline_present = !config.rewrite.api_key.trim().is_empty(),
        asr_pre_roll_ms = config.asr.pre_roll_ms,
        asr_audio_ring_ms = config.asr.audio_ring_ms,
        paste_stabilize_ms = config.output.paste_stabilize_ms,
        clipboard_retained_after_paste = true,
        hud_style = ?config.hud.style,
        hud_width_px = config.hud.width_px,
        hud_display_hold_ms = config.hud.display_hold_ms,
        rewrite_enabled = rewrite_language.rewrite_enabled(),
        streaming_rewrite_enabled = rewrite_language.streaming_rewrite_enabled(),
        rewrite_output_language = ?rewrite_language.current(),
        term_embeddings_enabled = config.term_embeddings.enabled,
        term_embeddings_endpoint = %config.term_embeddings.endpoint_url,
        term_embeddings_model = %config.term_embeddings.model,
        "config loaded"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            shutdown.store(true, Ordering::Relaxed);
        })
        .context("install Ctrl+C handler")?;
    }

    let hud_user_config_path = state_root.join("config").join("hud-user.toml");
    let hud = hud::HudController::start(
        config.hud.clone(),
        hud_user_config_path,
        Arc::clone(&shutdown),
    )
    .context("start HUD")?;
    let modes = modes::ModeStore::new(config.mode.default);
    // Cloud ASR clients remain for disabled legacy code paths; empty endpoints are allowed.
    // Public product path is local SenseVoice only — do not require cloud ASR health.
    let asr = cloud_asr::CloudAsrClient::new(&config.asr).context("create cloud ASR client")?;
    if config.profiles.streaming.enabled {
        match asr.health() {
            Ok(health) => info!(
                model = %health.model.as_deref().unwrap_or("unknown"),
                "optional cloud ASR health ok"
            ),
            Err(error) => error!(error = %error, "optional cloud ASR health probe failed"),
        }
    } else {
        info!("cloud streaming ASR profile disabled; skip health probe");
    }
    // Public product keeps streaming off: construct pool without auto-preheat noise.
    let asr_sessions = if config.profiles.streaming.enabled {
        asr_pool::AsrSessionPool::new(asr.clone())
    } else {
        asr_pool::AsrSessionPool::new_without_preheat(asr.clone())
    };
    if !config.profiles.streaming.enabled {
        asr_sessions.set_preheat_enabled(false, "streaming_profile_disabled");
    }
    let debug_panel = debug_panel::DebugPanelController::default();
    let history = history::HistoryService::start(
        state_root.join("logs").join("history.jsonl"),
        Arc::clone(&shutdown),
    )
    .context("start history service")?;
    let corrections_path = state_root.join("config").join("personal-corrections.json");
    let (suspect_notification_tx, _suspect_notification_rx) = mpsc::channel();
    let (api_notification_tx, api_notification_rx) = mpsc::channel();
    // Background analyzers stay disabled by default; no UI entry points ship in public product.
    let _suspect_terms = suspect_terms::SuspectTermsController::start(
        config.suspect_terms.clone(),
        history.path().to_path_buf(),
        state_root.join("logs").join("suspect-terms.json"),
        corrections_path.clone(),
        suspect_notification_tx,
        Arc::clone(&shutdown),
    )
    .context("start suspect terms analyzer")?;
    let _term_embeddings = term_embeddings::TermEmbeddingController::start(
        config.term_embeddings.clone(),
        corrections_path,
        state_root.join("logs").join("suspect-terms.json"),
        history.path().to_path_buf(),
        state_root.join("logs").join("term-embeddings.json"),
        state_root.join("logs").join("term-embedding-status.json"),
        state_root.join("logs").join("term-families.json"),
        state_root.join("logs").join("term-hotwords.json"),
        Arc::clone(&shutdown),
    )
    .context("start term embedding worker")?;

    let shared_rewriter = ai_rewrite::SharedRewriter::new(config.rewrite.clone());
    let rewrite_prompt = rewrite_prompt::RewritePromptController::load_or_default(
        state_root.join("config").join("rewrite-prompt.toml"),
    );
    let voice_command = voice_command::VoiceCommandController::load_or_default(
        state_root.join("config").join("voice-command.toml"),
    );
    let api_settings = api_settings_panel::ApiSettingsPanelController::start(
        api_connections.path.clone(),
        rewrite_language.clone(),
        shared_rewriter.clone(),
        Arc::clone(&shutdown),
    )
    .context("start API settings panel")?;
    let history_panel = history_panel::HistoryPanelController::start(
        history.path().to_path_buf(),
        Arc::clone(&shutdown),
    )
    .context("start history panel")?;
    let rewrite_prompt_panel = rewrite_prompt_panel::RewritePromptPanelController::start(
        rewrite_prompt.clone(),
        Arc::clone(&shutdown),
    )
    .context("start rewrite prompt panel")?;
    let voice_command_panel = voice_command_panel::VoiceCommandPanelController::start(
        voice_command.clone(),
        Arc::clone(&shutdown),
    )
    .context("start voice command panel")?;
    let hotkey_panel = hotkey_panel::HotkeyPanelController::start(
        hotkey_user.clone(),
        Arc::clone(&shutdown),
    )
    .context("start hotkey panel")?;
    let _tray = tray::Tray::start(
        hud.clone(),
        api_settings,
        history_panel,
        rewrite_language.clone(),
        rewrite_prompt.clone(),
        rewrite_prompt_panel,
        voice_command.clone(),
        voice_command_panel,
        hotkey_user.clone(),
        hotkey_panel,
        api_connections.path.clone(),
        api_notification_rx,
        Arc::clone(&shutdown),
    )
    .context("start tray icon")?;
    spawn_api_setup_probe(
        api_connections.config.clone(),
        state_root.clone(),
        api_notification_tx,
    );
    let audio = audio::AudioHub::start_default(config.asr.audio_ring_ms)
        .context("start resident microphone")?;
    hud.bind_audio_level(audio.level_share());
    let whisper =
        cloud_asr::WhisperClient::new(&config.whisper).context("create cloud Whisper client")?;
    let local_recognizer = local_asr::LocalSenseVoiceRecognizer::create(
        &config.local_nonstreaming,
        &install_root,
    )
    .context("create local SenseVoice recognizer (required)")?;
    info!(
        model_dir = %config.local_nonstreaming.model_dir,
        "local SenseVoice recognizer ready"
    );

    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let hotkey_monitor = hotkey::HotkeyMonitor::start(
        config.hotkey.clone(),
        config.profiles.clone(),
        hotkey_tx,
        Arc::clone(&shutdown),
    )
    .context("start hotkey monitor")?;

    let mut worker = worker::VoiceWorker::new(
        config,
        asr,
        whisper,
        Some(local_recognizer),
        asr_sessions,
        modes,
        audio,
        hud,
        debug_panel,
        history,
        shared_rewriter,
        rewrite_language,
        rewrite_prompt,
        voice_command,
        shutdown,
    );
    let result = worker.run(hotkey_rx);
    hotkey_monitor.stop();
    result
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let message = format!("ainput panic at {location}: {payload}");
        eprintln!("{message}");
        error!(%message, "ainput panic");
        show_startup_error(&message);
        default_hook(info);
    }));
}

#[cfg(windows)]
/// Named-mutex single-instance guard. Returns `Some(handle)` when this process
/// is the first instance; the handle must stay open for the process lifetime.
/// Returns `Ok(None)` when another instance already holds the lock.
fn acquire_single_instance_lock() -> Result<Option<windows::Win32::Foundation::HANDLE>> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    let handle =
        unsafe { CreateMutexW(None, false, w!("Local\\ainput_single_instance")) }
            .context("create single-instance mutex")?;
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Ok(None);
    }
    Ok(Some(handle))
}

#[cfg(not(windows))]
fn acquire_single_instance_lock() -> Result<Option<()>> {
    Ok(Some(()))
}

#[cfg(windows)]
fn show_already_running() {
    use std::os::windows::ffi::OsStrExt;
    let title: Vec<u16> = std::ffi::OsStr::new("ainput")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body: Vec<u16> = std::ffi::OsStr::new("ainput 已经在运行（不允许同时开两个）。")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        );
    }
}

#[cfg(not(windows))]
fn show_already_running() {}

fn show_startup_error(message: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let title: Vec<u16> = std::ffi::OsStr::new("ainput 启动失败")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let body_src = if message.chars().count() > 900 {
            format!(
                "{}…\n\n完整错误已写入 state\\logs\\ainput.log（若日志已初始化）。",
                message.chars().take(900).collect::<String>()
            )
        } else {
            format!("{message}\n\n若仍无法启动，请查看 state\\logs\\ainput.log。")
        };
        let body: Vec<u16> = std::ffi::OsStr::new(&body_src)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = message;
    }
}

fn spawn_api_setup_probe(
    api: api_config::ApiConnectionsConfig,
    state_root: PathBuf,
    notification_tx: mpsc::Sender<String>,
) {
    thread::spawn(move || {
        if !api.setup_checks_enabled() {
            info!("OpenAI-compatible API model setup check disabled");
            return;
        }
        if api.chat_completions_url().trim().is_empty()
            || api.chat_completions_url().trim() == "/v1/chat/completions"
        {
            info!("rewrite API base_url empty; skip model setup check");
            return;
        }
        let status = api.probe_setup_status();
        if let Err(error) = api_config::write_setup_status(&state_root, &status) {
            error!(error = %error, "write API setup status failed");
        }
        if status.ok {
            info!(
                models_url = %status.models_url,
                available_model_count = status.available_model_count,
                required_models = ?status.required_models,
                "OpenAI-compatible API model setup check passed"
            );
            return;
        }
        if let Some(message) = api_config::setup_warning_message(&status) {
            let _ = notification_tx.send(message.clone());
            error!(
                models_url = %status.models_url,
                missing_models = ?status.missing_models,
                probe_error = ?status.error,
                warning = %message,
                "OpenAI-compatible API model setup check failed"
            );
        }
    });
}

#[cfg(windows)]
fn configure_process_dpi_awareness() -> &'static str {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() {
            return "per_monitor_v2";
        }
        if SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE).is_ok() {
            return "per_monitor";
        }
        if SetProcessDpiAwareness(PROCESS_SYSTEM_DPI_AWARE).is_ok() {
            return "system";
        }
        if SetProcessDPIAware().as_bool() {
            return "system_legacy";
        }
    }
    "failed"
}

#[cfg(not(windows))]
fn configure_process_dpi_awareness() -> &'static str {
    "not_windows"
}


fn resolve_config_path(state_root: &std::path::Path, install_root: &std::path::Path) -> PathBuf {
    let primary = state_root.join("config").join("ainput.toml");
    if primary.exists() {
        return primary;
    }
    let legacy_state = state_root.join("config").join("ainput2.toml");
    if legacy_state.exists() {
        return legacy_state;
    }
    let install_primary = install_root.join("config").join("ainput.toml");
    if install_primary.exists() {
        return primary; // load will copy via migrate; path stays under state
    }
    let install_legacy = install_root.join("config").join("ainput2.toml");
    if install_legacy.exists() {
        return legacy_state;
    }
    primary
}

fn resolve_install_root() -> Result<PathBuf> {
    let exe_dir = std::env::current_exe()
        .context("read current exe path")?
        .parent()
        .map(std::path::Path::to_path_buf)
        .context("current exe path has no parent")?;
    if exe_dir.join("config").join("ainput.toml").exists()
        || exe_dir.join("config").join("ainput2.toml").exists()
    {
        return Ok(exe_dir);
    }
    std::env::current_dir().context("read current directory")
}

fn resolve_state_root(install_root: &std::path::Path) -> Result<PathBuf> {
    // Public green package: state lives under install_root/state (never project-level dist parent).
    for env_name in ["AINPUT_STATE_ROOT", "AINPUT2_STATE_ROOT"] {
        if let Ok(value) = std::env::var(env_name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }
    }
    Ok(install_root.join("state"))
}

fn migrate_state_root(install_root: &std::path::Path, state_root: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(state_root.join("config"))
        .with_context(|| format!("create state config {}", state_root.display()))?;
    std::fs::create_dir_all(state_root.join("logs"))
        .with_context(|| format!("create state logs {}", state_root.display()))?;
    copy_if_missing(
        &install_root.join("config").join("ainput.toml"),
        &state_root.join("config").join("ainput.toml"),
    )?;
    for name in [
        "hud-user.toml",
        "rewrite-user.toml",
        "personal-corrections.json",
    ] {
        copy_state_file_from_install_or_sibling_dist(
            install_root,
            &install_root.join("config").join(name),
            &state_root.join("config").join(name),
        )?;
    }
    for name in ["history.jsonl", "suspect-terms.json"] {
        copy_state_file_from_install_or_sibling_dist(
            install_root,
            &install_root.join("logs").join(name),
            &state_root.join("logs").join(name),
        )?;
    }
    merge_empty_suspect_book_from_sibling_dist(
        install_root,
        &install_root.join("logs").join("suspect-terms.json"),
        &state_root.join("logs").join("suspect-terms.json"),
    )?;
    Ok(())
}

fn merge_empty_suspect_book_from_sibling_dist(
    install_root: &std::path::Path,
    primary: &std::path::Path,
    target: &std::path::Path,
) -> Result<()> {
    if primary.exists()
        && suspect_terms::merge_book_file_if_target_empty(target, primary).unwrap_or_default() > 0
    {
        return Ok(());
    }
    let Some(dist_root) = install_root.parent() else {
        return Ok(());
    };
    if !dist_root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("dist"))
    {
        return Ok(());
    }
    let Some(relative) = primary
        .strip_prefix(install_root)
        .ok()
        .map(std::path::Path::to_path_buf)
    else {
        return Ok(());
    };
    let mut candidates = std::fs::read_dir(dist_root)
        .with_context(|| format!("read dist root {}", dist_root.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path != install_root)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.cmp(left));
    for candidate in candidates {
        let source = candidate.join(&relative);
        if !source.exists() {
            continue;
        }
        if suspect_terms::merge_book_file_if_target_empty(target, &source).unwrap_or_default() > 0 {
            return Ok(());
        }
    }
    Ok(())
}

fn copy_state_file_from_install_or_sibling_dist(
    install_root: &std::path::Path,
    primary: &std::path::Path,
    to: &std::path::Path,
) -> Result<()> {
    if to.exists() {
        return Ok(());
    }
    if primary.exists() {
        return copy_if_missing(primary, to);
    }
    let Some(dist_root) = install_root.parent() else {
        return Ok(());
    };
    if !dist_root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("dist"))
    {
        return Ok(());
    }
    let relative = primary
        .strip_prefix(install_root)
        .ok()
        .map(std::path::Path::to_path_buf);
    let Some(relative) = relative else {
        return Ok(());
    };
    let mut candidates = std::fs::read_dir(dist_root)
        .with_context(|| format!("read dist root {}", dist_root.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path != install_root)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.cmp(left));
    for candidate in candidates {
        let source = candidate.join(&relative);
        if source.exists() {
            return copy_if_missing(&source, to);
        }
    }
    Ok(())
}

fn copy_if_missing(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    if to.exists() || !from.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create state parent {}", parent.display()))?;
    }
    std::fs::copy(from, to)
        .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn init_logging(root: &std::path::Path) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let logs_dir: PathBuf = root.join("logs");
    std::fs::create_dir_all(&logs_dir).context("create logs dir")?;
    // Rotate oversized never-rolled log so search stays usable (was ~392MB).
    let legacy_log = logs_dir.join("ainput.log");
    if let Ok(meta) = std::fs::metadata(&legacy_log) {
        const MAX_LEGACY_BYTES: u64 = 32 * 1024 * 1024;
        if meta.len() > MAX_LEGACY_BYTES {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bak = logs_dir.join(format!("ainput.log.bak-{stamp}"));
            match std::fs::rename(&legacy_log, &bak) {
                Ok(()) => eprintln!(
                    "ainput: archived oversized log {} -> {}",
                    legacy_log.display(),
                    bak.display()
                ),
                Err(error) => eprintln!(
                    "ainput: could not archive oversized log {}: {error}",
                    legacy_log.display()
                ),
            }
        }
    }
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "ainput.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ainput=info,info".into()),
        )
        .with_ansi(false)
        .init();
    Ok(guard)
}
