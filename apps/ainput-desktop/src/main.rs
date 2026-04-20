#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_rewrite;
mod hotkey;
mod instance;
mod maintenance;
mod overlay;
mod screenshot;
mod streaming_fixtures;
mod streaming_state;
mod worker;

use ainput_automation::{AutomationActivity, AutomationService, AutomationSnapshot};
use ainput_output::{OutputContextKind, OutputContextSnapshot};
use ainput_recording::{RecordingActivity, RecordingService, RecordingSnapshot};
use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use maintenance::{MaintenanceHandle, SharedRuntimeState};
use reqwest::Url;
use reqwest::blocking::Client;
use serde::Serialize;
use std::any::Any;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::PCWSTR;
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use worker::{WorkerCommand, WorkerEvent, WorkerKind};

const RUN_REGISTRY_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_REGISTRY_VALUE_NAME: &str = "ainput";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const HUD_CONFIG_POLL_INTERVAL: Duration = Duration::from_millis(250);
const HUD_TICK_INTERVAL: Duration = Duration::from_millis(16);
const TRAY_LOADING_FRAME_INTERVAL: Duration = Duration::from_millis(220);
const OLLAMA_STARTUP_WAIT_TIMEOUT: Duration = Duration::from_secs(12);
const OLLAMA_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(400);
const JSON_OUTPUT_PATH_ENV: &str = "AINPUT_JSON_OUTPUT_PATH";
const STREAMING_ENDPOINT_TRAILING_SILENCE_SECS: f32 = 10.0;
const STREAMING_ENDPOINT_MAX_UTTERANCE_SECS: f32 = 20.0;
const STREAMING_HOLD_TO_TALK_HOTKEY: &str = "Ctrl";

fn main() {
    ainput_recording::configure_dpi_awareness();
    if let Err(error) = try_main() {
        tracing::error!(error = %error, "ainput startup failed");
        show_error_dialog(
            "ainput 启动失败",
            &format!("ainput 没有成功启动。\n\n{}", error),
        );
    }
}

fn try_main() -> Result<()> {
    let bootstrap = ainput_shell::bootstrap().context("读取 ainput 启动配置失败")?;
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("transcribe-wav") {
        let runtime = build_runtime(&bootstrap, false)?;
        let wav_path = args
            .get(2)
            .ok_or_else(|| anyhow!("usage: ainput-desktop transcribe-wav <path-to-wav>"))?;
        let recognizer = build_recognizer(&runtime)?;
        let transcription = recognizer.transcribe_wav_file(wav_path)?;
        cache_recent_text(&bootstrap.runtime_paths.logs_dir, &transcription.text)?;
        println!("{}", transcription.text);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("transcribe-streaming-wav") {
        let runtime = build_runtime(&bootstrap, true)?;
        let wav_path = args.get(2).ok_or_else(|| {
            anyhow!("usage: ainput-desktop transcribe-streaming-wav <path-to-wav>")
        })?;
        let chunk_ms = args
            .get(3)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(runtime.config.voice.streaming.chunk_ms)
            .clamp(80, 500);
        let recognizer = build_streaming_recognizer(&runtime)?;
        let chunk_num_samples =
            (((runtime.config.asr.sample_rate_hz as usize) * chunk_ms as usize) / 1000)
                .max((runtime.config.asr.sample_rate_hz as usize) / 20);
        let transcription = recognizer.transcribe_wav_file(wav_path, chunk_num_samples)?;
        cache_recent_text(&bootstrap.runtime_paths.logs_dir, &transcription.text)?;
        println!("{}", transcription.text);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("record-once") {
        let runtime = build_runtime(&bootstrap, false)?;
        let seconds = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("3")
            .parse::<u64>()?;
        let recognizer = build_recognizer(&runtime)?;
        let recording = ainput_audio::ActiveRecording::start_default_input()?;
        thread::sleep(Duration::from_secs(seconds));
        let audio = recording.stop()?;
        let transcription = recognizer.transcribe_samples(
            audio.sample_rate_hz,
            &audio.samples,
            format!("microphone-{seconds}s"),
        )?;
        cache_recent_text(&bootstrap.runtime_paths.logs_dir, &transcription.text)?;
        println!("{}", transcription.text);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("test-ai-rewrite") {
        let runtime = build_runtime(&bootstrap, true)?;
        let current_tail = args.get(2).ok_or_else(|| {
            anyhow!("usage: ainput-desktop test-ai-rewrite <current-tail> [frozen-prefix]")
        })?;
        let frozen_prefix = args.get(3).cloned().unwrap_or_default();
        let ai_rewriter = runtime
            .ai_rewriter
            .as_ref()
            .ok_or_else(|| anyhow!("local AI rewrite client is not enabled"))?;
        let response = ai_rewriter.rewrite_tail(ai_rewrite::AiRewriteRequest {
            frozen_prefix,
            current_tail: current_tail.clone(),
            context: OutputContextSnapshot {
                process_name: Some("ainput-cli".to_string()),
                kind: OutputContextKind::EditableAtEnd,
            },
        })?;
        if let Some(response) = response {
            println!("{}", response.rewritten_tail);
        }
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("probe-streaming-live") {
        let runtime = build_runtime(&bootstrap, true)?;
        let seconds = args
            .get(2)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(8);
        let recognizer = build_streaming_recognizer(&runtime)?;
        let report = worker::probe_streaming_live_session(&runtime, &recognizer, seconds)?;
        emit_json_report(&report)?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("replay-streaming-wav") {
        let runtime = build_runtime(&bootstrap, true)?;
        let wav_path = args.get(2).ok_or_else(|| {
            anyhow!("usage: ainput-desktop replay-streaming-wav <path-to-wav> [expected-text]")
        })?;
        let expected_text = args.get(3).map(String::as_str);
        let recognizer = build_streaming_recognizer(&runtime)?;
        let report = worker::replay_streaming_wav(
            &runtime,
            &recognizer,
            "single",
            std::path::Path::new(wav_path),
            expected_text,
            1,
            None,
            3,
        )?;
        emit_json_report(&report)?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("replay-streaming-manifest") {
        let runtime = build_runtime(&bootstrap, true)?;
        let manifest_path = args.get(2).ok_or_else(|| {
            anyhow!("usage: ainput-desktop replay-streaming-manifest <manifest.json>")
        })?;
        let manifest_text = fs::read_to_string(manifest_path)
            .with_context(|| format!("read manifest file {manifest_path}"))?;
        let manifest_text = manifest_text.trim_start_matches('\u{feff}');
        let manifest: streaming_fixtures::StreamingFixtureManifest =
            serde_json::from_str(&manifest_text)
                .with_context(|| format!("parse manifest file {manifest_path}"))?;
        let recognizer = build_streaming_recognizer(&runtime)?;
        let report = worker::replay_streaming_manifest(
            &runtime,
            &recognizer,
            std::path::Path::new(manifest_path),
            &manifest,
        )?;
        emit_json_report(&report)?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("bootstrap") {
        println!(
            "ainput bootstrap ready: voice_hotkey={}, voice_mode={}, capture_hotkey={}, config={}",
            bootstrap.config.hotkeys.voice_input,
            match bootstrap.config.voice.mode {
                ainput_shell::VoiceMode::Fast => "fast",
                ainput_shell::VoiceMode::Streaming => "streaming",
            },
            bootstrap.config.hotkeys.screen_capture,
            bootstrap.runtime_paths.config_file.display()
        );
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("clipboard-selftest-image") {
        screenshot::debug_test_clipboard_write()?;
        println!("clipboard self-test image written");
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("capture-fullscreen-selftest") {
        screenshot::debug_capture_fullscreen_to_clipboard()?;
        println!("fullscreen capture self-test written");
        return Ok(());
    }

    instance::replace_existing_instance()
        .context("替换已在运行的 ainput 进程失败，请先关闭旧版输入法后再试")?;
    run_desktop_app(bootstrap).context("初始化 ainput 托盘和后台服务失败")
}

fn effective_voice_hotkey_binding(
    mode: ainput_shell::VoiceMode,
    configured_fast_hotkey: &str,
) -> String {
    match mode {
        ainput_shell::VoiceMode::Fast => configured_fast_hotkey.to_string(),
        ainput_shell::VoiceMode::Streaming => STREAMING_HOLD_TO_TALK_HOTKEY.to_string(),
    }
}

fn emit_json_report<T: Serialize>(report: &T) -> Result<()> {
    let report_json = serde_json::to_vec_pretty(report)?;
    if let Ok(output_path) = std::env::var(JSON_OUTPUT_PATH_ENV) {
        let output_path = PathBuf::from(output_path);
        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create json report directory {}", parent.display()))?;
        }
        fs::write(&output_path, report_json)
            .with_context(|| format!("write json report {}", output_path.display()))?;
    } else {
        println!("{}", String::from_utf8(report_json)?);
    }
    Ok(())
}

fn run_desktop_app(bootstrap: ainput_shell::Bootstrap) -> Result<()> {
    let runtime = build_runtime(
        &bootstrap,
        bootstrap.config.voice.mode == ainput_shell::VoiceMode::Streaming,
    )?;
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    }));

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(AppEvent::Menu(event));
    }));

    let mut app = DesktopApp::new(runtime, proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn build_runtime(
    bootstrap: &ainput_shell::Bootstrap,
    prepare_streaming_ai_rewrite: bool,
) -> Result<AppRuntime> {
    let output_controller = Arc::new(ainput_output::OutputController::new(
        &bootstrap.runtime_paths.root_dir,
    )?);
    let shared_state = SharedRuntimeState::new();
    let maintenance = MaintenanceHandle::start(
        bootstrap.runtime_paths.logs_dir.clone(),
        bootstrap.config.voice.history_file_name.clone(),
        bootstrap.config.voice.history_limit,
    );
    let ai_rewriter = if prepare_streaming_ai_rewrite {
        try_prepare_streaming_ai_rewriter(&bootstrap.config.voice.streaming.ai_rewrite)
    } else {
        None
    };

    Ok(AppRuntime {
        config: bootstrap.config.clone(),
        runtime_paths: bootstrap.runtime_paths.clone(),
        output_controller,
        shared_state,
        ai_rewriter,
        maintenance,
    })
}

fn try_prepare_streaming_ai_rewriter(
    ai_config: &ainput_shell::StreamingAiRewriteConfig,
) -> Option<Arc<ai_rewrite::AiRewriteClient>> {
    if !ai_config.enabled {
        return None;
    }

    if let Err(error) = ensure_local_ai_rewrite_backend_ready(ai_config) {
        tracing::warn!(
            error = %error,
            "failed to prepare streaming AI rewrite backend; keeping streaming fallback path"
        );
        return None;
    }

    match ai_rewrite::AiRewriteClient::from_config(ai_config) {
        Ok(Some(ai_rewriter)) => {
            let ai_rewriter = Arc::new(ai_rewriter);
            tracing::info!(
                endpoint_url = %ai_config.endpoint_url,
                model = %ai_config.model,
                "streaming AI rewrite client enabled"
            );
            let prewarm_client = Arc::clone(&ai_rewriter);
            thread::spawn(move || {
                if let Err(error) = prewarm_client.prewarm() {
                    tracing::warn!(error = %error, "streaming AI rewrite warmup failed");
                }
            });
            Some(ai_rewriter)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "failed to initialize streaming AI rewrite client; keeping streaming fallback path"
            );
            None
        }
    }
}

fn ensure_local_ai_rewrite_backend_ready(
    ai_config: &ainput_shell::StreamingAiRewriteConfig,
) -> Result<()> {
    if !ai_config.enabled {
        return Ok(());
    }

    let Some(tags_url) = local_ollama_tags_url(&ai_config.endpoint_url)? else {
        return Ok(());
    };

    if probe_local_ollama(&tags_url) {
        tracing::info!(tags_url = %tags_url, "local Ollama already reachable");
        return Ok(());
    }

    let launch_path = resolve_ollama_launch_path();
    match &launch_path {
        Some(path) => tracing::info!(
            tags_url = %tags_url,
            launch_path = %path.display(),
            "local Ollama is not reachable; attempting auto-start"
        ),
        None => tracing::warn!(
            tags_url = %tags_url,
            "local Ollama is not reachable and no launch path was found"
        ),
    }

    if let Some(path) = &launch_path
        && let Err(error) = start_ollama_background(path)
    {
        tracing::warn!(
            launch_path = %path.display(),
            error = %error,
            "failed to auto-start local Ollama"
        );
    }

    let deadline = Instant::now() + OLLAMA_STARTUP_WAIT_TIMEOUT;
    while Instant::now() < deadline {
        if probe_local_ollama(&tags_url) {
            tracing::info!(tags_url = %tags_url, "local Ollama became reachable after auto-start");
            return Ok(());
        }
        thread::sleep(OLLAMA_STARTUP_POLL_INTERVAL);
    }

    tracing::warn!(
        tags_url = %tags_url,
        waited_ms = OLLAMA_STARTUP_WAIT_TIMEOUT.as_millis(),
        "local Ollama is still unreachable after auto-start wait window"
    );
    Ok(())
}

fn local_ollama_tags_url(endpoint_url: &str) -> Result<Option<Url>> {
    let url = Url::parse(endpoint_url.trim()).with_context(|| {
        format!("parse voice.streaming.ai_rewrite.endpoint_url: {endpoint_url}")
    })?;
    let host = url.host_str().unwrap_or_default();
    let is_local_ollama =
        matches!(host, "127.0.0.1" | "localhost") && url.port_or_known_default() == Some(11434);
    if !is_local_ollama {
        return Ok(None);
    }

    let mut tags_url = url;
    tags_url.set_path("/api/tags");
    tags_url.set_query(None);
    Ok(Some(tags_url))
}

fn probe_local_ollama(tags_url: &Url) -> bool {
    match Client::builder()
        .timeout(Duration::from_millis(900))
        .no_proxy()
        .build()
    {
        Ok(client) => match client.get(tags_url.clone()).send() {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

fn resolve_ollama_launch_path() -> Option<PathBuf> {
    let local_cli = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("Programs").join("Ollama").join("ollama.exe"));
    if let Some(path) = local_cli
        && path.exists()
    {
        return Some(path);
    }

    let local_app = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("Programs").join("Ollama").join("ollama app.exe"));
    if let Some(path) = local_app
        && path.exists()
    {
        return Some(path);
    }

    None
}

fn start_ollama_background(path: &std::path::Path) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut command = Command::new(path);
    if file_name == "ollama.exe" {
        command.arg("serve").creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .with_context(|| format!("start Ollama via {}", path.display()))?;
    Ok(())
}

fn build_recognizer(runtime: &AppRuntime) -> Result<ainput_asr::SenseVoiceRecognizer> {
    ainput_asr::SenseVoiceRecognizer::create(&ainput_asr::SenseVoiceConfig {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.asr.model_dir),
        provider: runtime.config.asr.provider.clone(),
        sample_rate_hz: runtime.config.asr.sample_rate_hz as i32,
        language: runtime.config.asr.language.clone(),
        use_itn: runtime.config.asr.use_itn,
        num_threads: runtime.config.asr.num_threads,
    })
}

fn build_streaming_recognizer(
    runtime: &AppRuntime,
) -> Result<ainput_asr::StreamingZipformerRecognizer> {
    ainput_asr::StreamingZipformerRecognizer::create(&ainput_asr::StreamingZipformerConfig {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.voice.streaming.model_dir),
        provider: runtime.config.asr.provider.clone(),
        sample_rate_hz: runtime.config.asr.sample_rate_hz as i32,
        num_threads: runtime.config.asr.num_threads,
        decoding_method: "greedy_search".to_string(),
        enable_endpoint: true,
        rule1_min_trailing_silence: STREAMING_ENDPOINT_TRAILING_SILENCE_SECS,
        rule2_min_trailing_silence: STREAMING_ENDPOINT_TRAILING_SILENCE_SECS,
        rule3_min_utterance_length: STREAMING_ENDPOINT_MAX_UTTERANCE_SECS,
    })
}

fn cache_recent_text(logs_dir: &std::path::Path, text: &str) -> Result<()> {
    fs::write(logs_dir.join("last_result.txt"), text).map_err(Into::into)
}

fn open_in_notepad(path: PathBuf) -> Result<()> {
    Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

fn open_in_explorer(path: PathBuf) -> Result<()> {
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

fn open_readme_document(runtime: &AppRuntime) -> Result<()> {
    open_in_notepad(runtime.runtime_paths.root_dir.join("README.md"))
}

fn open_config_document(runtime: &AppRuntime) -> Result<()> {
    open_in_notepad(runtime.runtime_paths.config_file.clone())
}

fn open_hud_overlay_document(runtime: &AppRuntime) -> Result<()> {
    open_in_notepad(runtime.runtime_paths.hud_overlay_file.clone())
}

fn open_voice_history_document(runtime: &AppRuntime) -> Result<()> {
    open_in_notepad(
        runtime
            .runtime_paths
            .logs_dir
            .join(&runtime.config.voice.history_file_name),
    )
}

fn open_logs_directory(runtime: &AppRuntime) -> Result<()> {
    open_in_explorer(runtime.runtime_paths.logs_dir.clone())
}

#[derive(Clone)]
pub(crate) struct AppRuntime {
    config: ainput_shell::AppConfig,
    runtime_paths: ainput_shell::RuntimePaths,
    output_controller: Arc<ainput_output::OutputController>,
    shared_state: SharedRuntimeState,
    ai_rewriter: Option<Arc<ai_rewrite::AiRewriteClient>>,
    maintenance: MaintenanceHandle,
}

pub(crate) enum AppEvent {
    Worker(WorkerEvent),
    Hotkey(hotkey::HotkeyState),
    Capture(screenshot::CaptureEvent),
    AutomationUpdated,
    RecordingUpdated,
    OverlayTick,
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Idle,
    Voice,
    Capture,
    Automation,
    Recording,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayVisualState {
    Idle,
    Loading,
    Voice,
    ScreenRecording,
    AutomationRecording,
    AutomationPlaying,
    AutomationPaused,
    Error,
}

struct DesktopApp {
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    overlay_tick_started: bool,
    fast_worker_tx: Option<mpsc::Sender<WorkerCommand>>,
    streaming_worker_tx: Option<mpsc::Sender<WorkerCommand>>,
    fast_worker_ready: bool,
    streaming_worker_ready: bool,
    hotkey_monitor: Option<hotkey::GlobalHotkeyMonitor>,
    automation_service: Option<AutomationService>,
    recording_service: Option<RecordingService>,
    tray_icon: Option<TrayIcon>,
    overlay: Option<overlay::RecordingOverlay>,
    overlay_available: bool,
    hud_overlay_last_modified: Option<SystemTime>,
    hud_overlay_last_checked_at: Instant,
    hud_overlay_last_error: Option<String>,
    mode: AppMode,
    tray_visual_state: TrayVisualState,
    tray_visual_frame: u8,
    tray_loading_last_frame_at: Instant,
    exit_item: Option<MenuItem>,
    restart_item: Option<MenuItem>,
    status_item: Option<MenuItem>,
    voice_hotkey_hint_item: Option<MenuItem>,
    voice_mode_fast_item: Option<CheckMenuItem>,
    voice_mode_streaming_item: Option<CheckMenuItem>,
    open_hud_overlay_item: Option<MenuItem>,
    learn_terms_item: Option<MenuItem>,
    mouse_middle_item: Option<CheckMenuItem>,
    launch_at_login_item: Option<CheckMenuItem>,
    capture_save_item: Option<CheckMenuItem>,
    automation_status_item: Option<MenuItem>,
    automation_slot_items: Vec<CheckMenuItem>,
    automation_repeat_items: Vec<CheckMenuItem>,
    automation_repeat_current_item: Option<MenuItem>,
    automation_repeat_custom_item: Option<MenuItem>,
    automation_edit_names_item: Option<MenuItem>,
    automation_open_dir_item: Option<MenuItem>,
    recording_status_item: Option<MenuItem>,
    recording_audio_item: Option<CheckMenuItem>,
    recording_mouse_item: Option<CheckMenuItem>,
    recording_watermark_item: Option<CheckMenuItem>,
    recording_set_watermark_text_item: Option<MenuItem>,
    recording_position_items: Vec<CheckMenuItem>,
    recording_fps_items: Vec<CheckMenuItem>,
    recording_quality_items: Vec<CheckMenuItem>,
}

impl DesktopApp {
    fn new(runtime: AppRuntime, proxy: EventLoopProxy<AppEvent>) -> Self {
        let hud_overlay_last_modified =
            hud_overlay_modified_at(&runtime.runtime_paths.hud_overlay_file);
        Self {
            runtime,
            proxy,
            shutdown: Arc::new(AtomicBool::new(false)),
            overlay_tick_started: false,
            fast_worker_tx: None,
            streaming_worker_tx: None,
            fast_worker_ready: false,
            streaming_worker_ready: false,
            hotkey_monitor: None,
            automation_service: None,
            recording_service: None,
            tray_icon: None,
            overlay: None,
            overlay_available: true,
            hud_overlay_last_modified,
            hud_overlay_last_checked_at: Instant::now(),
            hud_overlay_last_error: None,
            mode: AppMode::Idle,
            tray_visual_state: TrayVisualState::Idle,
            tray_visual_frame: 0,
            tray_loading_last_frame_at: Instant::now(),
            exit_item: None,
            restart_item: None,
            status_item: None,
            voice_hotkey_hint_item: None,
            voice_mode_fast_item: None,
            voice_mode_streaming_item: None,
            open_hud_overlay_item: None,
            learn_terms_item: None,
            mouse_middle_item: None,
            launch_at_login_item: None,
            capture_save_item: None,
            automation_status_item: None,
            automation_slot_items: Vec::new(),
            automation_repeat_items: Vec::new(),
            automation_repeat_current_item: None,
            automation_repeat_custom_item: None,
            automation_edit_names_item: None,
            automation_open_dir_item: None,
            recording_status_item: None,
            recording_audio_item: None,
            recording_mouse_item: None,
            recording_watermark_item: None,
            recording_set_watermark_text_item: None,
            recording_position_items: Vec::new(),
            recording_fps_items: Vec::new(),
            recording_quality_items: Vec::new(),
        }
    }

    fn start_fast_worker_once(&mut self) {
        if self.fast_worker_tx.is_some() {
            return;
        }

        let runtime = self.runtime.clone();
        let proxy = self.proxy.clone();
        let shutdown = self.shutdown.clone();
        let (worker_tx, worker_rx) = mpsc::channel();
        self.fast_worker_tx = Some(worker_tx);
        self.fast_worker_ready = false;
        if self.mode == AppMode::Idle {
            self.refresh_idle_tray_surface();
        }

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker::push_to_talk_worker(runtime, proxy.clone(), shutdown.clone(), worker_rx);
            }));

            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let message = match result {
                Ok(()) => "语音线程已退出，下次按快捷键会自动重试".to_string(),
                Err(payload) => format!("语音线程异常退出：{}", panic_message(payload.as_ref())),
            };
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Unavailable(message)));
        });
    }

    fn start_streaming_worker_once(&mut self) {
        if self.streaming_worker_tx.is_some() {
            return;
        }

        self.ensure_streaming_ai_rewrite_ready();
        let runtime = self.runtime.clone();
        let proxy = self.proxy.clone();
        let shutdown = self.shutdown.clone();
        let (worker_tx, worker_rx) = mpsc::channel();
        self.streaming_worker_tx = Some(worker_tx);
        self.streaming_worker_ready = false;
        if self.mode == AppMode::Idle {
            self.refresh_idle_tray_surface();
        }

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker::streaming_push_to_talk_worker(
                    runtime,
                    proxy.clone(),
                    shutdown.clone(),
                    worker_rx,
                );
            }));

            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let message = match result {
                Ok(()) => "流式语音线程已退出，下次按快捷键会自动重试".to_string(),
                Err(payload) => {
                    format!("流式语音线程异常退出：{}", panic_message(payload.as_ref()))
                }
            };
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Unavailable(message)));
        });
    }

    fn ensure_streaming_ai_rewrite_ready(&mut self) {
        if self.runtime.ai_rewriter.is_some() {
            return;
        }

        self.runtime.ai_rewriter =
            try_prepare_streaming_ai_rewriter(&self.runtime.config.voice.streaming.ai_rewrite);
    }

    fn start_overlay_tick_once(&mut self) {
        if self.overlay_tick_started {
            return;
        }

        self.overlay_tick_started = true;
        let proxy = self.proxy.clone();
        let shutdown = self.shutdown.clone();
        thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                let _ = proxy.send_event(AppEvent::OverlayTick);
                thread::sleep(HUD_TICK_INTERVAL);
            }
        });
    }

    fn set_tray_status(&self, status: &str) {
        let rendered_status = if self.overlay_available {
            status.to_string()
        } else {
            format!("{status}（无底部提示条）")
        };

        if let Some(tray_icon) = &self.tray_icon {
            let tooltip = format!("ainput\n{rendered_status}");
            let _ = tray_icon.set_tooltip(Some(tooltip));
        }

        self.set_tray_status_menu_only(status);
    }

    fn set_tray_visual_state(&mut self, state: TrayVisualState, frame: u8) {
        if self.tray_visual_state == state && self.tray_visual_frame == frame {
            return;
        }

        self.tray_visual_state = state;
        self.tray_visual_frame = frame;
        if let Some(tray_icon) = &self.tray_icon {
            let _ = tray_icon.set_icon(Some(app_status_icon(&self.runtime, state, frame)));
        }
    }

    fn set_tray_status_menu_only(&self, status: &str) {
        let rendered_status = if self.overlay_available {
            status.to_string()
        } else {
            format!("{status}（无底部提示条）")
        };

        if let Some(status_item) = &self.status_item {
            status_item.set_text(&rendered_status);
        }
    }

    fn fast_worker_loading(&self) -> bool {
        self.fast_worker_tx.is_some() && !self.fast_worker_ready
    }

    fn streaming_worker_loading(&self) -> bool {
        self.streaming_worker_tx.is_some() && !self.streaming_worker_ready
    }

    fn has_loading_voice_workers(&self) -> bool {
        self.fast_worker_loading() || self.streaming_worker_loading()
    }

    fn loading_status_text(&self) -> Option<String> {
        let mut pending = Vec::new();
        if self.fast_worker_loading() {
            pending.push("极速语音模型");
        }
        if self.streaming_worker_loading() {
            pending.push("流式语音模型");
        }
        if pending.is_empty() {
            None
        } else {
            Some(format!("状态：正在加载{}...", pending.join("、")))
        }
    }

    fn refresh_idle_tray_surface(&mut self) {
        if let Some(status) = self.loading_status_text() {
            if self.tray_visual_state != TrayVisualState::Loading {
                self.tray_loading_last_frame_at = Instant::now();
            }
            self.set_tray_visual_state(TrayVisualState::Loading, 0);
            self.set_tray_status(&status);
        } else {
            self.set_tray_visual_state(TrayVisualState::Idle, 0);
            self.set_tray_status(&self.idle_status_text());
        }
    }

    fn animate_loading_tray_if_needed(&mut self) {
        if self.mode != AppMode::Idle || !self.has_loading_voice_workers() {
            return;
        }
        if self.tray_loading_last_frame_at.elapsed() < TRAY_LOADING_FRAME_INTERVAL {
            return;
        }
        self.tray_loading_last_frame_at = Instant::now();

        let next_frame = if self.tray_visual_state == TrayVisualState::Loading {
            self.tray_visual_frame.wrapping_add(1) % 4
        } else {
            0
        };
        self.set_tray_visual_state(TrayVisualState::Loading, next_frame);
    }

    fn handle_voice_worker_ready(&mut self, kind: WorkerKind) {
        match kind {
            WorkerKind::Fast => self.fast_worker_ready = true,
            WorkerKind::Streaming => self.streaming_worker_ready = true,
        }

        if self.mode == AppMode::Idle {
            self.refresh_idle_tray_surface();
        }
    }

    fn try_send_fast_worker_command(&self, command: WorkerCommand) -> bool {
        self.fast_worker_tx
            .as_ref()
            .is_some_and(|worker_tx| worker_tx.send(command).is_ok())
    }

    fn send_fast_worker_command(&mut self, command: WorkerCommand) -> bool {
        self.start_fast_worker_once();
        if self.try_send_fast_worker_command(command) {
            return true;
        }

        tracing::warn!(
            ?command,
            "fast voice worker channel unavailable, restarting worker"
        );
        self.fast_worker_tx = None;
        self.start_fast_worker_once();
        if self.try_send_fast_worker_command(command) {
            return true;
        }

        self.handle_fast_worker_unavailable("极速语音线程不可用，请重试或用托盘菜单重新启动");
        false
    }

    fn try_send_streaming_worker_command(&self, command: WorkerCommand) -> bool {
        self.streaming_worker_tx
            .as_ref()
            .is_some_and(|worker_tx| worker_tx.send(command).is_ok())
    }

    fn send_streaming_worker_command(&mut self, command: WorkerCommand) -> bool {
        self.start_streaming_worker_once();
        if self.try_send_streaming_worker_command(command) {
            return true;
        }

        tracing::warn!(
            ?command,
            "streaming voice worker channel unavailable, restarting worker"
        );
        self.streaming_worker_tx = None;
        self.start_streaming_worker_once();
        if self.try_send_streaming_worker_command(command) {
            return true;
        }

        self.handle_streaming_worker_unavailable("流式语音线程不可用，请重试或检查模型目录");
        false
    }

    fn handle_fast_worker_unavailable(&mut self, message: &str) {
        self.fast_worker_tx = None;
        self.fast_worker_ready = false;
        self.handle_worker_error(message);
    }

    fn handle_streaming_worker_unavailable(&mut self, message: &str) {
        self.streaming_worker_tx = None;
        self.streaming_worker_ready = false;
        self.handle_worker_error(message);
    }

    fn handle_worker_error(&mut self, message: &str) {
        self.mode = AppMode::Idle;
        self.set_tray_visual_state(TrayVisualState::Error, 0);
        self.clear_streaming_status_overlay();
        if let Some(overlay) = &mut self.overlay {
            overlay.set_level(0.0);
            overlay.hide();
        }
        self.set_tray_status(&format!("状态：错误 - {}", shorten(message, 18)));
    }

    fn voice_mode_label(mode: ainput_shell::VoiceMode) -> &'static str {
        match mode {
            ainput_shell::VoiceMode::Fast => "极速语音识别",
            ainput_shell::VoiceMode::Streaming => "流式语音识别",
        }
    }

    fn idle_status_text(&self) -> String {
        format!(
            "状态：待机中（{}）",
            Self::voice_mode_label(self.runtime.config.voice.mode)
        )
    }

    fn streaming_status_text(&self) -> &'static str {
        "状态：流式语音识别中"
    }

    fn streaming_partial_message(prepared_text: &str) -> Option<String> {
        let message = prepared_text.trim();
        if !message.is_empty() {
            return Some(message.to_string());
        }
        None
    }

    fn startup_ready_message(&self) -> String {
        match self.runtime.config.voice.mode {
            ainput_shell::VoiceMode::Fast => format!(
                "ainput 已启动，按 {} 开始语音输入",
                self.current_voice_hotkey_binding()
            ),
            ainput_shell::VoiceMode::Streaming => format!(
                "ainput 已启动，按住 {} 说话",
                self.current_voice_hotkey_binding()
            ),
        }
    }

    fn sync_voice_mode_menu(&self) {
        if let Some(item) = &self.voice_mode_fast_item {
            item.set_checked(self.runtime.config.voice.mode == ainput_shell::VoiceMode::Fast);
        }
        if let Some(item) = &self.voice_mode_streaming_item {
            item.set_checked(self.runtime.config.voice.mode == ainput_shell::VoiceMode::Streaming);
        }
    }

    fn current_voice_hotkey_binding(&self) -> String {
        effective_voice_hotkey_binding(
            self.runtime.config.voice.mode,
            &self.runtime.config.hotkeys.voice_input,
        )
    }

    fn sync_voice_hotkey_hint(&self) {
        if let Some(item) = &self.voice_hotkey_hint_item {
            item.set_text(&format!(
                "按住说话热键：{}",
                self.current_voice_hotkey_binding()
            ));
        }
    }

    fn set_voice_mode(&mut self, mode: ainput_shell::VoiceMode) -> Result<()> {
        if self.runtime.config.voice.mode == mode {
            self.sync_voice_mode_menu();
            self.sync_voice_hotkey_hint();
            return Ok(());
        }

        if self.mode != AppMode::Idle {
            return Err(anyhow!("请先结束当前操作后再切换语音模式"));
        }

        let previous_mode = self.runtime.config.voice.mode;
        self.runtime.config.voice.mode = mode;
        self.sync_voice_mode_menu();

        if let Err(error) =
            ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
        {
            self.runtime.config.voice.mode = previous_mode;
            self.sync_voice_mode_menu();
            return Err(error);
        }

        let current_voice_hotkey_binding = self.current_voice_hotkey_binding();
        if let Err(error) = hotkey::set_voice_input_binding(&current_voice_hotkey_binding) {
            self.runtime.config.voice.mode = previous_mode;
            self.sync_voice_mode_menu();
            self.sync_voice_hotkey_hint();
            let _ = ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config);
            return Err(error);
        }

        self.refresh_idle_tray_surface();
        self.sync_voice_hotkey_hint();
        self.prewarm_current_voice_worker();
        Ok(())
    }

    fn prewarm_current_voice_worker(&mut self) {
        match self.runtime.config.voice.mode {
            ainput_shell::VoiceMode::Fast => self.start_fast_worker_once(),
            ainput_shell::VoiceMode::Streaming => self.start_streaming_worker_once(),
        }
    }

    fn show_streaming_status_overlay(
        &mut self,
        message: &str,
        persistent: bool,
        char_streaming: bool,
    ) {
        if !self.runtime.config.voice.streaming.panel_enabled {
            if let Some(overlay) = &mut self.overlay {
                overlay.clear_status_hud();
            }
            return;
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.show_status_hud(message, persistent, char_streaming);
        }
    }

    fn clear_streaming_status_overlay(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.clear_status_hud();
        }
    }

    fn reload_hud_overlay_if_changed(&mut self) {
        if self.hud_overlay_last_checked_at.elapsed() < HUD_CONFIG_POLL_INTERVAL {
            return;
        }
        self.hud_overlay_last_checked_at = Instant::now();

        let modified = hud_overlay_modified_at(&self.runtime.runtime_paths.hud_overlay_file);
        if modified == self.hud_overlay_last_modified {
            return;
        }

        match ainput_shell::load_hud_overlay_config(&self.runtime.runtime_paths) {
            Ok(config) => {
                self.hud_overlay_last_modified = modified;
                self.hud_overlay_last_error = None;
                self.runtime.config.hud_overlay = config.clone();

                let overlay_reload_result = if let Some(overlay) = &mut self.overlay {
                    overlay.apply_hud_config(&config)
                } else {
                    match overlay::RecordingOverlay::create(&config) {
                        Ok(overlay) => {
                            self.overlay = Some(overlay);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                };

                match overlay_reload_result {
                    Ok(()) => {
                        self.overlay_available = self.overlay.is_some();
                        tracing::info!(
                            path = %self.runtime.runtime_paths.hud_overlay_file.display(),
                            anchor = ?config.anchor,
                            offset_x_px = config.offset_x_px,
                            offset_y_px = config.offset_y_px,
                            width_px = config.width_px,
                            min_width_px = config.min_width_px,
                            padding_x_px = config.padding_x_px,
                            padding_y_px = config.padding_y_px,
                            font_height_px = config.font_height_px,
                            font_weight = config.font_weight,
                            text_align = ?config.text_align,
                            background_alpha = config.background_alpha,
                            "HUD overlay config hot reloaded"
                        );
                        self.set_tray_status_menu_only("状态：HUD 参数已热加载");
                    }
                    Err(error) => {
                        self.overlay_available = self.overlay.is_some();
                        self.set_tray_status(&format!(
                            "状态：HUD 热加载失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                        tracing::error!(error = %error, "apply HUD overlay config failed");
                    }
                }
            }
            Err(error) => {
                self.hud_overlay_last_modified = modified;
                let error_text = error.to_string();
                if self.hud_overlay_last_error.as_ref() != Some(&error_text) {
                    self.set_tray_status(&format!(
                        "状态：HUD 参数解析失败 - {}",
                        shorten(&error_text, 16)
                    ));
                    tracing::warn!(error = %error, "parse HUD overlay config failed");
                    self.hud_overlay_last_error = Some(error_text);
                }
            }
        }
    }

    fn build_tray_once(&mut self) -> Result<()> {
        if self.tray_icon.is_some() {
            return Ok(());
        }

        if self.automation_service.is_none() {
            let proxy = self.proxy.clone();
            self.automation_service = Some(AutomationService::start(
                automation_storage_dir(&self.runtime),
                self.runtime.config.automation.repeat_count,
                move || {
                    let _ = proxy.send_event(AppEvent::AutomationUpdated);
                },
            )?);
        }
        let automation_snapshot = self
            .automation_service
            .as_ref()
            .expect("automation service initialized")
            .snapshot();
        if self.runtime.config.automation.repeat_count != automation_snapshot.repeat_count {
            self.runtime.config.automation.repeat_count = automation_snapshot.repeat_count;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                tracing::warn!(error = %error, "persist sanitized automation repeat count failed");
            }
        }
        if self.recording_service.is_none() {
            let proxy = self.proxy.clone();
            self.recording_service = Some(RecordingService::start(move || {
                let _ = proxy.send_event(AppEvent::RecordingUpdated);
            })?);
        }
        let recording_snapshot = self
            .recording_service
            .as_ref()
            .expect("recording service initialized")
            .snapshot();

        let tray_menu = Menu::new();
        let status_item = MenuItem::with_id("status", self.idle_status_text(), false, None);
        let voice_mode_fast_item = CheckMenuItem::with_id(
            "voice_mode_fast",
            Self::voice_mode_label(ainput_shell::VoiceMode::Fast),
            true,
            self.runtime.config.voice.mode == ainput_shell::VoiceMode::Fast,
            None,
        );
        let voice_mode_streaming_item = CheckMenuItem::with_id(
            "voice_mode_streaming",
            Self::voice_mode_label(ainput_shell::VoiceMode::Streaming),
            true,
            self.runtime.config.voice.mode == ainput_shell::VoiceMode::Streaming,
            None,
        );
        let open_hud_overlay_item =
            MenuItem::with_id("open_hud_overlay_config", "打开 HUD 参数文档", true, None);

        let voice_hotkey_hint = MenuItem::with_id(
            "voice_hotkey_hint",
            format!("按住说话热键：{}", self.current_voice_hotkey_binding()),
            false,
            None,
        );
        let mouse_middle_item = CheckMenuItem::with_id(
            "mouse_middle_toggle",
            "启用鼠标中键长按录音",
            true,
            self.runtime.config.hotkeys.mouse_middle_hold_enabled,
            None,
        );
        let open_history_item = MenuItem::with_id("open_voice_history", "打开语音历史", true, None);
        let voice_menu = Submenu::with_id_and_items(
            "voice_menu",
            "语音",
            true,
            &[&voice_hotkey_hint, &mouse_middle_item, &open_history_item],
        )?;

        let capture_hotkey_hint = MenuItem::with_id(
            "capture_hotkey_hint",
            format!("截图热键：{}", self.runtime.config.hotkeys.screen_capture),
            false,
            None,
        );
        let capture_save_item = CheckMenuItem::with_id(
            "capture_save_toggle",
            "截图后自动保存到桌面",
            true,
            self.runtime.config.capture.auto_save_to_desktop,
            None,
        );
        let capture_menu = Submenu::with_id_and_items(
            "capture_menu",
            "截图",
            true,
            &[&capture_hotkey_hint, &capture_save_item],
        )?;

        let recording_status_item = MenuItem::with_id(
            "recording_status",
            recording_snapshot.status_line.clone(),
            false,
            None,
        );
        let recording_hotkey_hint = MenuItem::with_id(
            "recording_hotkey_hint",
            format!(
                "热键：{} 框选并开始 / {} 停止并导出",
                ainput_recording::START_HOTKEY,
                ainput_recording::STOP_HOTKEY
            ),
            false,
            None,
        );
        let recording_audio_item = CheckMenuItem::with_id(
            "recording_audio_toggle",
            "录制系统音频",
            true,
            self.runtime.config.recording.record_audio,
            None,
        );
        let recording_mouse_item = CheckMenuItem::with_id(
            "recording_mouse_toggle",
            "录制鼠标移动",
            true,
            self.runtime.config.recording.capture_mouse,
            None,
        );
        let recording_watermark_item = CheckMenuItem::with_id(
            "recording_watermark_toggle",
            "启用水印",
            true,
            self.runtime.config.recording.watermark.enabled,
            None,
        );
        let recording_set_watermark_text_item = MenuItem::with_id(
            "recording_set_watermark_text",
            "设置水印文本...",
            true,
            None,
        );
        let recording_position_items: Vec<CheckMenuItem> =
            ainput_recording::WATERMARK_POSITION_PRESETS
                .into_iter()
                .map(|position| {
                    CheckMenuItem::with_id(
                        format!("recording_position_{position:?}"),
                        position.label(),
                        true,
                        position == self.runtime.config.recording.watermark.position,
                        None,
                    )
                })
                .collect();
        let recording_fps_items: Vec<CheckMenuItem> = ainput_recording::FPS_PRESETS
            .into_iter()
            .map(|fps| {
                CheckMenuItem::with_id(
                    format!("recording_fps_{fps}"),
                    format!("{fps} FPS"),
                    true,
                    fps == self.runtime.config.recording.fps,
                    None,
                )
            })
            .collect();
        let recording_quality_items: Vec<CheckMenuItem> = ainput_recording::QUALITY_PRESETS
            .into_iter()
            .map(|quality| {
                CheckMenuItem::with_id(
                    format!("recording_quality_{quality:?}"),
                    quality.label(),
                    true,
                    quality == self.runtime.config.recording.quality,
                    None,
                )
            })
            .collect();
        let recording_position_menu = {
            let mut items: Vec<&dyn IsMenuItem> = Vec::new();
            for item in &recording_position_items {
                items.push(item);
            }
            Submenu::with_id_and_items("recording_position_menu", "水印位置", true, &items)?
        };
        let recording_fps_menu = {
            let mut items: Vec<&dyn IsMenuItem> = Vec::new();
            for item in &recording_fps_items {
                items.push(item);
            }
            Submenu::with_id_and_items("recording_fps_menu", "帧率", true, &items)?
        };
        let recording_quality_menu = {
            let mut items: Vec<&dyn IsMenuItem> = Vec::new();
            for item in &recording_quality_items {
                items.push(item);
            }
            Submenu::with_id_and_items("recording_quality_menu", "画质", true, &items)?
        };
        let recording_menu = Submenu::with_id_and_items(
            "recording_menu",
            "录屏",
            true,
            &[
                &recording_status_item,
                &recording_hotkey_hint,
                &recording_audio_item,
                &recording_mouse_item,
                &recording_watermark_item,
                &recording_set_watermark_text_item,
                &recording_position_menu,
                &recording_fps_menu,
                &recording_quality_menu,
            ],
        )?;

        let automation_status_item = MenuItem::with_id(
            "automation_status",
            automation_snapshot.status_line.clone(),
            false,
            None,
        );
        let automation_hotkey_hint = MenuItem::with_id(
            "automation_hotkey_hint",
            format!(
                "热键：{} 暂停继续 / {} 录制 / {} 保存 / {} 回放 / {} 停止",
                ainput_automation::PAUSE_HOTKEY,
                ainput_automation::RECORD_HOTKEY,
                ainput_automation::STOP_HOTKEY,
                ainput_automation::PLAY_HOTKEY,
                ainput_automation::CANCEL_HOTKEY
            ),
            false,
            None,
        );
        let automation_slot_items: Vec<CheckMenuItem> = automation_snapshot
            .slots
            .iter()
            .map(|slot| {
                CheckMenuItem::with_id(
                    format!("automation_slot_{}", slot.slot),
                    format_automation_slot_label(slot),
                    true,
                    slot.slot == automation_snapshot.active_slot,
                    None,
                )
            })
            .collect();
        let automation_repeat_items: Vec<CheckMenuItem> = (1
            ..=ainput_automation::REPEAT_COUNT_PRESET_MAX)
            .map(|repeat_count| {
                CheckMenuItem::with_id(
                    format!("automation_repeat_{repeat_count}"),
                    format_automation_repeat_label(repeat_count),
                    true,
                    repeat_count == automation_snapshot.repeat_count,
                    None,
                )
            })
            .collect();
        let automation_repeat_current_item = MenuItem::with_id(
            "automation_repeat_current",
            format_current_automation_repeat_label(automation_snapshot.repeat_count),
            false,
            None,
        );
        let automation_repeat_custom_item = MenuItem::with_id(
            "automation_repeat_custom",
            "设置自定义回放轮数...",
            true,
            None,
        );
        let automation_edit_names_item =
            MenuItem::with_id("automation_edit_names", "编辑槽位名称", true, None);
        let automation_open_dir_item =
            MenuItem::with_id("automation_open_dir", "打开录制目录", true, None);
        let automation_sep_1 = PredefinedMenuItem::separator();
        let automation_sep_2 = PredefinedMenuItem::separator();
        let automation_sep_3 = PredefinedMenuItem::separator();
        let mut automation_items: Vec<&dyn IsMenuItem> = vec![
            &automation_status_item,
            &automation_hotkey_hint,
            &automation_sep_1,
        ];
        for item in &automation_slot_items {
            automation_items.push(item);
        }
        automation_items.push(&automation_sep_2);
        for item in &automation_repeat_items {
            automation_items.push(item);
        }
        automation_items.push(&automation_repeat_current_item);
        automation_items.push(&automation_repeat_custom_item);
        automation_items.push(&automation_sep_3);
        automation_items.push(&automation_edit_names_item);
        automation_items.push(&automation_open_dir_item);
        let automation_menu =
            Submenu::with_id_and_items("automation_menu", "按键精灵", true, &automation_items)?;

        let learn_terms_item =
            MenuItem::with_id("learn_terms", "从当前剪贴板学习最近一次修正", true, None);
        let open_user_terms_item =
            MenuItem::with_id("open_user_terms", "打开用户术语文件", true, None);
        let open_learning_state_item =
            MenuItem::with_id("open_learning_state", "打开学习状态文件", true, None);
        let open_builtin_terms_item =
            MenuItem::with_id("open_builtin_terms", "打开内置 AI 词库", true, None);
        let learning_menu = Submenu::with_id_and_items(
            "learning_menu",
            "术语与学习",
            true,
            &[
                &learn_terms_item,
                &open_user_terms_item,
                &open_learning_state_item,
                &open_builtin_terms_item,
            ],
        )?;

        let launch_at_login_item = CheckMenuItem::with_id(
            "launch_at_login_toggle",
            "开机自动启动",
            true,
            self.runtime.config.startup.launch_at_login,
            None,
        );
        let open_config_item = MenuItem::with_id("open_config", "打开配置文件", true, None);
        let open_logs_item = MenuItem::with_id("open_logs", "打开日志目录", true, None);
        let restart_item = MenuItem::with_id("restart", "重新启动", true, None);
        let help_item = MenuItem::with_id("help", "使用说明", true, None);
        let exit_item = MenuItem::with_id("exit", "退出", true, None);
        let general_menu = Submenu::with_id_and_items(
            "general_menu",
            "通用",
            true,
            &[
                &launch_at_login_item,
                &open_config_item,
                &open_logs_item,
                &restart_item,
                &help_item,
            ],
        )?;

        let separator = PredefinedMenuItem::separator();
        let mode_separator = PredefinedMenuItem::separator();
        let hud_separator = PredefinedMenuItem::separator();
        let _ = tray_menu.append(&status_item);
        let _ = tray_menu.append(&separator);
        let _ = tray_menu.append(&voice_mode_fast_item);
        let _ = tray_menu.append(&voice_mode_streaming_item);
        let _ = tray_menu.append(&mode_separator);
        let _ = tray_menu.append(&open_hud_overlay_item);
        let _ = tray_menu.append(&hud_separator);
        let _ = tray_menu.append(&voice_menu);
        let _ = tray_menu.append(&capture_menu);
        let _ = tray_menu.append(&recording_menu);
        let _ = tray_menu.append(&automation_menu);
        let _ = tray_menu.append(&learning_menu);
        let _ = tray_menu.append(&general_menu);
        let _ = tray_menu.append(&exit_item);

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip(format!("ainput\n{}", self.idle_status_text()))
            .with_icon(app_status_icon(&self.runtime, TrayVisualState::Idle, 0))
            .with_menu(Box::new(tray_menu))
            .build()
            .map_err(|error| anyhow!("create tray icon: {error}"))?;

        let overlay = match overlay::RecordingOverlay::create(&self.runtime.config.hud_overlay) {
            Ok(overlay) => Some(overlay),
            Err(error) => {
                tracing::error!(error = %error, "create recording overlay failed");
                None
            }
        };
        self.overlay_available = overlay.is_some();

        self.status_item = Some(status_item);
        self.voice_hotkey_hint_item = Some(voice_hotkey_hint);
        self.voice_mode_fast_item = Some(voice_mode_fast_item);
        self.voice_mode_streaming_item = Some(voice_mode_streaming_item);
        self.open_hud_overlay_item = Some(open_hud_overlay_item);
        self.exit_item = Some(exit_item);
        self.restart_item = Some(restart_item);
        self.learn_terms_item = Some(learn_terms_item);
        self.mouse_middle_item = Some(mouse_middle_item);
        self.launch_at_login_item = Some(launch_at_login_item);
        self.capture_save_item = Some(capture_save_item);
        self.automation_status_item = Some(automation_status_item);
        self.automation_slot_items = automation_slot_items;
        self.automation_repeat_items = automation_repeat_items;
        self.automation_repeat_current_item = Some(automation_repeat_current_item);
        self.automation_repeat_custom_item = Some(automation_repeat_custom_item);
        self.automation_edit_names_item = Some(automation_edit_names_item);
        self.automation_open_dir_item = Some(automation_open_dir_item);
        self.recording_status_item = Some(recording_status_item);
        self.recording_audio_item = Some(recording_audio_item);
        self.recording_mouse_item = Some(recording_mouse_item);
        self.recording_watermark_item = Some(recording_watermark_item);
        self.recording_set_watermark_text_item = Some(recording_set_watermark_text_item);
        self.recording_position_items = recording_position_items;
        self.recording_fps_items = recording_fps_items;
        self.recording_quality_items = recording_quality_items;
        self.tray_icon = Some(tray_icon);
        self.overlay = overlay;
        self.sync_voice_mode_menu();
        self.refresh_idle_tray_surface();
        hotkey::set_automation_cancel_enabled(false);
        self.sync_automation_menu();
        self.sync_recording_menu();
        Ok(())
    }

    fn sync_automation_menu(&self) {
        let Some(service) = &self.automation_service else {
            return;
        };
        let _ = service.refresh_slot_names();
        let snapshot = service.snapshot();

        if let Some(item) = &self.automation_status_item {
            item.set_text(&snapshot.status_line);
        }

        for (index, item) in self.automation_slot_items.iter().enumerate() {
            if let Some(slot) = snapshot.slots.get(index) {
                item.set_text(&format_automation_slot_label(slot));
                item.set_checked(slot.slot == snapshot.active_slot);
            }
        }

        for (index, item) in self.automation_repeat_items.iter().enumerate() {
            let repeat_count = index + 1;
            item.set_checked(repeat_count == snapshot.repeat_count);
        }
        if let Some(item) = &self.automation_repeat_current_item {
            item.set_text(&format_current_automation_repeat_label(
                snapshot.repeat_count,
            ));
        }
    }

    fn sync_recording_menu(&self) {
        let Some(service) = &self.recording_service else {
            return;
        };
        let snapshot = service.snapshot();

        if let Some(item) = &self.recording_status_item {
            item.set_text(&snapshot.status_line);
        }
        if let Some(item) = &self.recording_audio_item {
            item.set_checked(self.runtime.config.recording.record_audio);
        }
        if let Some(item) = &self.recording_mouse_item {
            item.set_checked(self.runtime.config.recording.capture_mouse);
        }
        if let Some(item) = &self.recording_watermark_item {
            item.set_checked(self.runtime.config.recording.watermark.enabled);
        }

        for (index, item) in self.recording_position_items.iter().enumerate() {
            if let Some(position) = ainput_recording::WATERMARK_POSITION_PRESETS.get(index) {
                item.set_checked(*position == self.runtime.config.recording.watermark.position);
            }
        }
        for (index, item) in self.recording_fps_items.iter().enumerate() {
            if let Some(fps) = ainput_recording::FPS_PRESETS.get(index) {
                item.set_checked(*fps == self.runtime.config.recording.fps);
            }
        }
        for (index, item) in self.recording_quality_items.iter().enumerate() {
            if let Some(quality) = ainput_recording::QUALITY_PRESETS.get(index) {
                item.set_checked(*quality == self.runtime.config.recording.quality);
            }
        }
    }

    fn automation_hotkeys_allowed(&self) -> bool {
        !matches!(
            self.mode,
            AppMode::Voice | AppMode::Capture | AppMode::Recording
        )
    }

    fn automation_cancel_enabled(snapshot: &AutomationSnapshot) -> bool {
        matches!(
            snapshot.activity,
            AutomationActivity::Recording
                | AutomationActivity::Playing
                | AutomationActivity::Paused
        )
    }

    fn recording_cancel_enabled(snapshot: &RecordingSnapshot) -> bool {
        snapshot.activity == RecordingActivity::Recording
    }

    fn automation_tray_frame(snapshot: &AutomationSnapshot) -> u8 {
        ((snapshot.elapsed_ms / 180) % 4) as u8
    }

    fn refresh_automation_overlay(&mut self, snapshot: &AutomationSnapshot) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        // Automation uses its own HUD/click feedback instead of the bottom bar.
        overlay.hide();
        overlay.update_automation_feedback(
            snapshot.activity,
            snapshot.overlay_hint.as_ref(),
            snapshot.last_click.as_ref(),
            &snapshot.status_line,
        );
    }

    fn handle_automation_update(&mut self) {
        let Some(service) = &self.automation_service else {
            return;
        };
        let snapshot = service.snapshot();
        hotkey::set_automation_cancel_enabled(Self::automation_cancel_enabled(&snapshot));
        self.sync_automation_menu();
        self.refresh_automation_overlay(&snapshot);

        match snapshot.activity {
            AutomationActivity::Recording => {
                self.mode = AppMode::Automation;
                self.set_tray_visual_state(
                    TrayVisualState::AutomationRecording,
                    Self::automation_tray_frame(&snapshot),
                );
                self.set_tray_status(&snapshot.status_line);
            }
            AutomationActivity::Playing => {
                self.mode = AppMode::Automation;
                self.set_tray_visual_state(
                    TrayVisualState::AutomationPlaying,
                    Self::automation_tray_frame(&snapshot),
                );
                self.set_tray_status(&snapshot.status_line);
            }
            AutomationActivity::Paused => {
                self.mode = AppMode::Automation;
                self.set_tray_visual_state(TrayVisualState::AutomationPaused, 0);
                self.set_tray_status(&snapshot.status_line);
            }
            AutomationActivity::Error => {
                self.mode = AppMode::Idle;
                self.set_tray_visual_state(TrayVisualState::Error, 0);
                self.set_tray_status(&snapshot.status_line);
            }
            AutomationActivity::Idle => {
                if self.mode == AppMode::Automation {
                    self.mode = AppMode::Idle;
                }
                self.set_tray_visual_state(TrayVisualState::Idle, 0);
                self.set_tray_status(&snapshot.status_line);
            }
        }
    }

    fn handle_recording_update(&mut self) {
        let Some(service) = &self.recording_service else {
            return;
        };
        let snapshot = service.snapshot();
        hotkey::set_recording_cancel_enabled(Self::recording_cancel_enabled(&snapshot));
        self.sync_recording_menu();

        match snapshot.activity {
            RecordingActivity::Selecting => {
                self.mode = AppMode::Recording;
                self.set_tray_visual_state(TrayVisualState::ScreenRecording, 0);
                self.set_tray_status("状态：录屏框选中");
            }
            RecordingActivity::Recording => {
                self.mode = AppMode::Recording;
                self.set_tray_visual_state(TrayVisualState::ScreenRecording, 1);
                self.set_tray_status("状态：录屏中");
            }
            RecordingActivity::Stopping => {
                self.mode = AppMode::Recording;
                self.set_tray_visual_state(TrayVisualState::ScreenRecording, 0);
                self.set_tray_status("状态：正在停止录屏");
            }
            RecordingActivity::Error => {
                self.mode = AppMode::Idle;
                self.set_tray_visual_state(TrayVisualState::Error, 0);
                self.set_tray_status(&format!(
                    "状态：录屏错误 - {}",
                    shorten(&snapshot.status_line, 16)
                ));
            }
            RecordingActivity::Idle => {
                if self.mode == AppMode::Recording {
                    self.mode = AppMode::Idle;
                }
                self.set_tray_visual_state(TrayVisualState::Idle, 0);
                self.set_tray_status_menu_only("状态：待机中");
            }
        }
    }
}

impl ApplicationHandler<AppEvent> for DesktopApp {
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);

        if let Err(error) = self.build_tray_once() {
            tracing::error!(error = %error, "tray initialization failed");
            show_error_dialog(
                "ainput 启动失败",
                &format!("托盘初始化失败，ainput 无法继续运行。\n\n{}", error),
            );
            self.shutdown.store(true, Ordering::Relaxed);
            event_loop.exit();
            return;
        }

        self.start_overlay_tick_once();
        self.prewarm_current_voice_worker();
        if self.hotkey_monitor.is_none() {
            match hotkey::GlobalHotkeyMonitor::start(
                self.proxy.clone(),
                self.shutdown.clone(),
                hotkey::HotkeyBindings {
                    voice_input: self.current_voice_hotkey_binding(),
                    screen_capture: self.runtime.config.hotkeys.screen_capture.clone(),
                },
                self.runtime.config.hotkeys.mouse_middle_hold_enabled,
            ) {
                Ok(monitor) => {
                    self.hotkey_monitor = Some(monitor);
                }
                Err(error) => {
                    tracing::error!(error = %error, "start global hotkey monitor failed");
                    self.set_tray_visual_state(TrayVisualState::Error, 0);
                    self.set_tray_status(&format!(
                        "状态：热键初始化失败 - {}",
                        shorten(&error.to_string(), 14)
                    ));
                    show_error_dialog(
                        "ainput 热键初始化失败",
                        &format!("快捷键注册失败，ainput 当前不能正常使用。\n\n{}", error),
                    );
                    return;
                }
            }
        }

        match sync_launch_at_login(self.runtime.config.startup.launch_at_login) {
            Ok(()) => self.refresh_idle_tray_surface(),
            Err(error) => {
                tracing::error!(error = %error, "sync launch-at-login setting failed");
                self.set_tray_status(&format!(
                    "状态：开机启动设置失败 - {}",
                    shorten(&error.to_string(), 14)
                ));
            }
        }

        let startup_message = self.startup_ready_message();
        self.show_streaming_status_overlay(&startup_message, false, false);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Worker(worker_event) => match worker_event {
                WorkerEvent::Ready(kind) => {
                    self.handle_voice_worker_ready(kind);
                }
                WorkerEvent::RecordingStarted => {
                    self.mode = AppMode::Voice;
                    self.set_tray_visual_state(TrayVisualState::Voice, 0);
                    self.set_tray_status("状态：正在录音");
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_pulse_enabled(true);
                        overlay.show();
                    }
                }
                WorkerEvent::Meter(level) => {
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_pulse_enabled(true);
                        overlay.set_level(level);
                    }
                }
                WorkerEvent::RecordingStopped => {
                    self.set_tray_visual_state(TrayVisualState::Voice, 1);
                    self.set_tray_status("状态：录音结束");
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_level(0.0);
                        overlay.hide();
                    }
                }
                WorkerEvent::Transcribing => {
                    self.mode = AppMode::Voice;
                    self.set_tray_visual_state(TrayVisualState::Voice, 1);
                    self.set_tray_status("状态：正在识别");
                }
                WorkerEvent::IgnoredSilence => {
                    self.mode = AppMode::Idle;
                    self.clear_streaming_status_overlay();
                    self.refresh_idle_tray_surface();
                }
                WorkerEvent::Delivered => {
                    self.mode = AppMode::Idle;
                    self.clear_streaming_status_overlay();
                    self.set_tray_visual_state(TrayVisualState::Idle, 0);
                    self.set_tray_status(&format!(
                        "状态：文本已直贴（{}）",
                        Self::voice_mode_label(self.runtime.config.voice.mode)
                    ));
                }
                WorkerEvent::ClipboardFallback => {
                    self.mode = AppMode::Idle;
                    self.clear_streaming_status_overlay();
                    self.set_tray_visual_state(TrayVisualState::Idle, 0);
                    self.set_tray_status(&format!(
                        "状态：已复制到剪贴板（{}）",
                        Self::voice_mode_label(self.runtime.config.voice.mode)
                    ));
                }
                WorkerEvent::StreamingStarted => {
                    self.mode = AppMode::Voice;
                    self.set_tray_visual_state(TrayVisualState::Voice, 0);
                    self.set_tray_status(self.streaming_status_text());
                    self.clear_streaming_status_overlay();
                }
                WorkerEvent::StreamingPartial {
                    raw_text,
                    prepared_text,
                } => {
                    self.mode = AppMode::Voice;
                    self.set_tray_visual_state(TrayVisualState::Voice, 0);
                    self.set_tray_status("状态：流式实时识别中");
                    let _ = raw_text;
                    if let Some(message) = Self::streaming_partial_message(&prepared_text) {
                        self.show_streaming_status_overlay(&message, true, true);
                    }
                }
                WorkerEvent::StreamingFlushing => {
                    self.mode = AppMode::Voice;
                    self.set_tray_visual_state(TrayVisualState::Voice, 1);
                    self.set_tray_status("状态：流式语音识别收尾中");
                }
                WorkerEvent::StreamingClipboardFallback(text) => {
                    self.mode = AppMode::Idle;
                    self.set_tray_visual_state(TrayVisualState::Idle, 0);
                    self.set_tray_status("状态：流式整理结果已复制到剪贴板");
                    let _ = text;
                    self.clear_streaming_status_overlay();
                }
                WorkerEvent::StreamingFinal(text) => {
                    self.mode = AppMode::Idle;
                    self.set_tray_visual_state(TrayVisualState::Idle, 0);
                    self.set_tray_status("状态：流式整理结果已完成");
                    let _ = text;
                    self.clear_streaming_status_overlay();
                }
                WorkerEvent::Error(message) => {
                    self.handle_worker_error(&message);
                }
                WorkerEvent::Unavailable(message) => {
                    self.handle_worker_error(&message);
                }
            },
            AppEvent::Hotkey(state) => match state {
                hotkey::HotkeyState::VoicePressed => {
                    if self.mode == AppMode::Idle && self.runtime.config.voice.enabled {
                        match self.runtime.config.voice.mode {
                            ainput_shell::VoiceMode::Fast => {
                                self.mode = AppMode::Voice;
                                let _ = self.send_fast_worker_command(WorkerCommand::HotkeyPressed);
                            }
                            ainput_shell::VoiceMode::Streaming => {
                                if !self.runtime.config.voice.streaming.enabled {
                                    self.set_tray_visual_state(TrayVisualState::Error, 0);
                                    self.set_tray_status("状态：流式语音识别已在配置中关闭");
                                    return;
                                }
                                self.mode = AppMode::Voice;
                                self.set_tray_visual_state(TrayVisualState::Voice, 0);
                                self.set_tray_status(self.streaming_status_text());
                                self.clear_streaming_status_overlay();
                                let _ = self
                                    .send_streaming_worker_command(WorkerCommand::HotkeyPressed);
                            }
                        }
                    }
                }
                hotkey::HotkeyState::VoiceReleased => {
                    if self.mode == AppMode::Voice {
                        match self.runtime.config.voice.mode {
                            ainput_shell::VoiceMode::Fast => {
                                let _ =
                                    self.send_fast_worker_command(WorkerCommand::HotkeyReleased);
                            }
                            ainput_shell::VoiceMode::Streaming => {
                                self.set_tray_visual_state(TrayVisualState::Voice, 1);
                                self.set_tray_status("状态：流式语音识别收尾中");
                                let _ = self
                                    .send_streaming_worker_command(WorkerCommand::HotkeyReleased);
                            }
                        }
                    }
                }
                hotkey::HotkeyState::ScreenshotTriggered => {
                    if self.mode == AppMode::Idle && self.runtime.config.capture.enabled {
                        tracing::info!(
                            hotkey = %self.runtime.config.hotkeys.screen_capture,
                            "enter capture mode from screenshot hotkey"
                        );
                        hotkey::reset_hotkey_state();
                        self.mode = AppMode::Capture;
                        screenshot::start_capture_session(self.proxy.clone(), self.runtime.clone());
                    }
                }
                hotkey::HotkeyState::RecordingStartTriggered => {
                    if self.mode == AppMode::Idle && self.runtime.config.recording.enabled {
                        hotkey::reset_hotkey_state();
                        self.mode = AppMode::Recording;
                        if let Some(service) = &self.recording_service
                            && let Err(error) =
                                service.begin_recording(self.runtime.config.recording.clone())
                        {
                            self.mode = AppMode::Idle;
                            self.set_tray_status(&format!(
                                "状态：录屏启动失败 - {}",
                                shorten(&error.to_string(), 16)
                            ));
                        }
                    }
                }
                hotkey::HotkeyState::RecordingStopTriggered => {
                    if let Some(service) = &self.recording_service {
                        let snapshot = service.snapshot();
                        if snapshot.activity == RecordingActivity::Recording
                            && let Err(error) = service.stop_recording()
                        {
                            self.set_tray_status(&format!(
                                "状态：录屏停止失败 - {}",
                                shorten(&error.to_string(), 16)
                            ));
                        }
                    }
                }
                hotkey::HotkeyState::RecordingCancelTriggered => {
                    if let Some(service) = &self.recording_service {
                        let snapshot = service.snapshot();
                        if snapshot.activity == RecordingActivity::Recording
                            && let Err(error) = service.cancel_recording()
                        {
                            self.set_tray_status(&format!(
                                "状态：录屏取消失败 - {}",
                                shorten(&error.to_string(), 16)
                            ));
                        }
                    }
                }
                hotkey::HotkeyState::AutomationPauseTriggered => {
                    if self.automation_hotkeys_allowed()
                        && let Some(service) = &self.automation_service
                    {
                        service.toggle_pause_playback();
                    }
                }
                hotkey::HotkeyState::AutomationRecordTriggered => {
                    if self.automation_hotkeys_allowed()
                        && let Some(service) = &self.automation_service
                        && let Err(error) = service.start_recording()
                    {
                        self.set_tray_visual_state(TrayVisualState::Error, 0);
                        self.set_tray_status(&format!(
                            "状态：按键精灵录制失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                    }
                }
                hotkey::HotkeyState::AutomationStopTriggered => {
                    if self.automation_hotkeys_allowed()
                        && let Some(service) = &self.automation_service
                        && let Err(error) = service.stop_recording()
                    {
                        self.set_tray_visual_state(TrayVisualState::Error, 0);
                        self.set_tray_status(&format!(
                            "状态：按键精灵保存失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                    }
                }
                hotkey::HotkeyState::AutomationPlayTriggered => {
                    if self.automation_hotkeys_allowed()
                        && let Some(service) = &self.automation_service
                        && let Err(error) = service.start_playback()
                    {
                        self.set_tray_visual_state(TrayVisualState::Error, 0);
                        self.set_tray_status(&format!(
                            "状态：按键精灵回放失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                    }
                }
                hotkey::HotkeyState::AutomationCancelTriggered => {
                    if let Some(service) = &self.automation_service
                        && let Err(error) = service.stop_active()
                    {
                        self.set_tray_visual_state(TrayVisualState::Error, 0);
                        self.set_tray_status(&format!(
                            "状态：按键精灵停止失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                    }
                }
            },
            AppEvent::Capture(capture_event) => match capture_event {
                screenshot::CaptureEvent::Started => {
                    self.mode = AppMode::Capture;
                }
                screenshot::CaptureEvent::Cancelled => {
                    hotkey::reset_hotkey_state();
                    self.mode = AppMode::Idle;
                }
                screenshot::CaptureEvent::Copied { saved_path } => {
                    hotkey::reset_hotkey_state();
                    self.mode = AppMode::Idle;
                    let _ = saved_path;
                }
                screenshot::CaptureEvent::Error(message) => {
                    hotkey::reset_hotkey_state();
                    self.mode = AppMode::Idle;
                    self.set_tray_status(&format!("状态：错误 - {}", shorten(&message, 18)));
                }
            },
            AppEvent::AutomationUpdated => self.handle_automation_update(),
            AppEvent::RecordingUpdated => self.handle_recording_update(),
            AppEvent::OverlayTick => {
                self.reload_hud_overlay_if_changed();
                self.animate_loading_tray_if_needed();
                if self.mode == AppMode::Automation
                    && let Some(service) = &self.automation_service
                {
                    let snapshot = service.snapshot();
                    self.refresh_automation_overlay(&snapshot);
                    match snapshot.activity {
                        AutomationActivity::Recording => self.set_tray_visual_state(
                            TrayVisualState::AutomationRecording,
                            Self::automation_tray_frame(&snapshot),
                        ),
                        AutomationActivity::Playing => self.set_tray_visual_state(
                            TrayVisualState::AutomationPlaying,
                            Self::automation_tray_frame(&snapshot),
                        ),
                        AutomationActivity::Paused => {
                            self.set_tray_visual_state(TrayVisualState::AutomationPaused, 0)
                        }
                        AutomationActivity::Idle | AutomationActivity::Error => {}
                    }
                }
                if let Some(overlay) = &mut self.overlay {
                    overlay.tick();
                }
            }
            AppEvent::Tray(event) => {
                if let TrayIconEvent::DoubleClick { .. } = event {
                    self.refresh_idle_tray_surface();
                }
            }
            AppEvent::Menu(event) => self.handle_menu_event(event_loop, event),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.hotkey_monitor = None;
        self.fast_worker_tx = None;
        self.streaming_worker_tx = None;
        hotkey::set_recording_cancel_enabled(false);
        hotkey::set_automation_cancel_enabled(false);
        self.automation_service = None;
        self.recording_service = None;
        if let Some(overlay) = &mut self.overlay {
            overlay.hide();
        }
        TrayIconEvent::set_event_handler::<fn(TrayIconEvent)>(None);
        MenuEvent::set_event_handler::<fn(MenuEvent)>(None);
    }
}

impl DesktopApp {
    fn handle_menu_event(&mut self, event_loop: &ActiveEventLoop, event: MenuEvent) {
        if self.handle_automation_menu_event(&event) {
            return;
        }
        if self.handle_recording_menu_event(&event) {
            return;
        }

        if self
            .voice_mode_fast_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match self.set_voice_mode(ainput_shell::VoiceMode::Fast) {
                Ok(()) => {
                    if self.has_loading_voice_workers() {
                        self.refresh_idle_tray_surface();
                    } else {
                        self.set_tray_status("状态：已切换到极速语音识别");
                    }
                }
                Err(error) => self.set_tray_status(&format!(
                    "状态：切换语音模式失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
            return;
        }

        if self
            .voice_mode_streaming_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match self.set_voice_mode(ainput_shell::VoiceMode::Streaming) {
                Ok(()) => {
                    if self.has_loading_voice_workers() {
                        self.refresh_idle_tray_surface();
                    } else {
                        self.set_tray_status("状态：已切换到流式语音识别");
                    }
                }
                Err(error) => self.set_tray_status(&format!(
                    "状态：切换语音模式失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
            return;
        }

        if self
            .exit_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            self.shutdown.store(true, Ordering::Relaxed);
            event_loop.exit();
        } else if self
            .restart_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match restart_application(&self.runtime) {
                Ok(()) => {
                    self.shutdown.store(true, Ordering::Relaxed);
                    event_loop.exit();
                }
                Err(error) => self.set_tray_status(&format!(
                    "状态：重启失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
        } else if event.id.0 == "help" {
            self.set_tray_status_from_result(
                open_readme_document(&self.runtime),
                "状态：已打开使用说明",
            );
        } else if event.id.0 == "open_config" {
            self.set_tray_status_from_result(
                open_config_document(&self.runtime),
                "状态：已打开配置文件",
            );
        } else if event.id.0 == "open_hud_overlay_config" {
            self.set_tray_status_from_result(
                open_hud_overlay_document(&self.runtime),
                "状态：已打开 HUD 参数文档",
            );
        } else if event.id.0 == "open_logs" {
            self.set_tray_status_from_result(
                open_logs_directory(&self.runtime),
                "状态：已打开日志目录",
            );
        } else if event.id.0 == "open_voice_history" {
            self.set_tray_status_from_result(
                open_voice_history_document(&self.runtime),
                "状态：已打开语音历史",
            );
        } else if event.id.0 == "open_user_terms" {
            match self.runtime.output_controller.user_terms_path() {
                Ok(path) => self
                    .set_tray_status_from_result(open_in_notepad(path), "状态：已打开用户术语文件"),
                Err(error) => self.set_tray_status(&format!(
                    "状态：打开术语失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
        } else if event.id.0 == "open_learning_state" {
            match self.runtime.output_controller.learning_state_path() {
                Ok(path) => self
                    .set_tray_status_from_result(open_in_notepad(path), "状态：已打开学习状态文件"),
                Err(error) => self.set_tray_status(&format!(
                    "状态：打开学习状态失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
        } else if event.id.0 == "open_builtin_terms" {
            match self.runtime.output_controller.builtin_terms_path() {
                Ok(path) => self
                    .set_tray_status_from_result(open_in_notepad(path), "状态：已打开内置 AI 词库"),
                Err(error) => self.set_tray_status(&format!(
                    "状态：打开内置词库失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
        } else if self
            .launch_at_login_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let previous_enabled = self.runtime.config.startup.launch_at_login;
            let next_enabled = !previous_enabled;
            self.runtime.config.startup.launch_at_login = next_enabled;
            if let Some(item) = &self.launch_at_login_item {
                item.set_checked(next_enabled);
            }
            let result = sync_launch_at_login(next_enabled).and_then(|_| {
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
                    .inspect_err(|_| {
                        let _ = sync_launch_at_login(previous_enabled);
                    })
            });
            if let Err(error) = result {
                self.runtime.config.startup.launch_at_login = previous_enabled;
                if let Some(item) = &self.launch_at_login_item {
                    item.set_checked(previous_enabled);
                }
                self.set_tray_status(&format!(
                    "状态：开机启动设置失败 - {}",
                    shorten(&error.to_string(), 14)
                ));
            } else {
                self.set_tray_status(if next_enabled {
                    "状态：已开启开机自动启动"
                } else {
                    "状态：已关闭开机自动启动"
                });
            }
        } else if self
            .mouse_middle_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let next_enabled = !self.runtime.config.hotkeys.mouse_middle_hold_enabled;
            self.runtime.config.hotkeys.mouse_middle_hold_enabled = next_enabled;
            hotkey::set_mouse_middle_enabled(next_enabled);
            if let Some(item) = &self.mouse_middle_item {
                item.set_checked(next_enabled);
            }
            match ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config) {
                Ok(()) => self.set_tray_status(if next_enabled {
                    "状态：已启用鼠标中键长按录音"
                } else {
                    "状态：已关闭鼠标中键长按录音"
                }),
                Err(error) => {
                    self.runtime.config.hotkeys.mouse_middle_hold_enabled = !next_enabled;
                    hotkey::set_mouse_middle_enabled(!next_enabled);
                    if let Some(item) = &self.mouse_middle_item {
                        item.set_checked(!next_enabled);
                    }
                    self.set_tray_status(&format!(
                        "状态：保存设置失败 - {}",
                        shorten(&error.to_string(), 16)
                    ));
                }
            }
        } else if self
            .capture_save_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let next_enabled = !self.runtime.config.capture.auto_save_to_desktop;
            self.runtime.config.capture.auto_save_to_desktop = next_enabled;
            if let Some(item) = &self.capture_save_item {
                item.set_checked(next_enabled);
            }
            match ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config) {
                Ok(()) => self.set_tray_status(if next_enabled {
                    "状态：已开启截图后自动保存到桌面"
                } else {
                    "状态：已关闭截图后自动保存到桌面"
                }),
                Err(error) => {
                    self.runtime.config.capture.auto_save_to_desktop = !next_enabled;
                    if let Some(item) = &self.capture_save_item {
                        item.set_checked(!next_enabled);
                    }
                    self.set_tray_status(&format!(
                        "状态：保存设置失败 - {}",
                        shorten(&error.to_string(), 16)
                    ));
                }
            }
        } else if self
            .learn_terms_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match self.learn_from_clipboard() {
                Ok(status) => self.set_tray_status(&status),
                Err(error) => self.set_tray_status(&format!(
                    "状态：学习失败 - {}",
                    shorten(&error.to_string(), 18)
                )),
            }
        }
    }

    fn handle_automation_menu_event(&mut self, event: &MenuEvent) -> bool {
        if self.automation_service.is_none() {
            return false;
        }

        if self
            .automation_edit_names_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let service = self
                .automation_service
                .as_ref()
                .expect("automation service initialized");
            let _ = service.refresh_slot_names();
            self.set_tray_status_from_result(
                service.open_slot_names_file(),
                "状态：已打开按键精灵槽位名称",
            );
            return true;
        }

        if self
            .automation_open_dir_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let service = self
                .automation_service
                .as_ref()
                .expect("automation service initialized");
            self.set_tray_status_from_result(
                service.open_slots_dir(),
                "状态：已打开按键精灵录制目录",
            );
            return true;
        }

        if self
            .automation_repeat_custom_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match prompt_for_automation_repeat_count(self.runtime.config.automation.repeat_count) {
                Ok(Some(repeat_count)) => {
                    if let Err(error) = self.set_automation_repeat_count(repeat_count) {
                        self.set_tray_status(&format!(
                            "状态：按键精灵轮数失败 - {}",
                            shorten(&error.to_string(), 14)
                        ));
                    }
                }
                Ok(None) => {}
                Err(error) => self.set_tray_status(&format!(
                    "状态：按键精灵轮数失败 - {}",
                    shorten(&error.to_string(), 14)
                )),
            }
            return true;
        }

        if let Some(index) = self
            .automation_slot_items
            .iter()
            .position(|item| event.id == *item.id())
        {
            let service = self
                .automation_service
                .as_ref()
                .expect("automation service initialized");
            match service.select_slot(index + 1) {
                Ok(()) => self.sync_automation_menu(),
                Err(error) => self.set_tray_status(&format!(
                    "状态：按键精灵切槽失败 - {}",
                    shorten(&error.to_string(), 14)
                )),
            }
            return true;
        }

        if let Some(index) = self
            .automation_repeat_items
            .iter()
            .position(|item| event.id == *item.id())
        {
            match self.set_automation_repeat_count(index + 1) {
                Ok(()) => {}
                Err(error) => self.set_tray_status(&format!(
                    "状态：按键精灵轮数失败 - {}",
                    shorten(&error.to_string(), 14)
                )),
            }
            return true;
        }

        false
    }

    fn set_automation_repeat_count(&mut self, repeat_count: usize) -> Result<()> {
        let previous = self.runtime.config.automation.repeat_count;
        {
            let service = self
                .automation_service
                .as_ref()
                .ok_or_else(|| anyhow!("automation service not initialized"))?;
            service.select_repeat_count(repeat_count)?;
        }
        self.runtime.config.automation.repeat_count = repeat_count;
        if let Err(error) =
            ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
        {
            self.runtime.config.automation.repeat_count = previous;
            if let Some(service) = &self.automation_service {
                let _ = service.select_repeat_count(previous);
            }
            self.sync_automation_menu();
            return Err(error);
        }

        self.sync_automation_menu();
        self.set_tray_status(&format!("状态：按键精灵回放轮数已切到 {repeat_count}"));
        Ok(())
    }

    fn handle_recording_menu_event(&mut self, event: &MenuEvent) -> bool {
        if self
            .recording_set_watermark_text_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            match prompt_for_recording_watermark_text(&self.runtime.config.recording.watermark.text)
            {
                Ok(Some(text)) => {
                    let previous = self.runtime.config.recording.watermark.text.clone();
                    self.runtime.config.recording.watermark.text = text;
                    if let Err(error) =
                        ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
                    {
                        self.runtime.config.recording.watermark.text = previous;
                        self.set_tray_status(&format!(
                            "状态：保存录屏设置失败 - {}",
                            shorten(&error.to_string(), 16)
                        ));
                    } else {
                        self.sync_recording_menu();
                        self.set_tray_status("状态：已更新录屏水印文本");
                    }
                }
                Ok(None) => {}
                Err(error) => self.set_tray_status(&format!(
                    "状态：水印输入失败 - {}",
                    shorten(&error.to_string(), 16)
                )),
            }
            return true;
        }

        if self
            .recording_audio_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let next_enabled = !self.runtime.config.recording.record_audio;
            let previous = self.runtime.config.recording.record_audio;
            self.runtime.config.recording.record_audio = next_enabled;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.record_audio = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(if next_enabled {
                    "状态：已开启录制系统音频"
                } else {
                    "状态：已关闭录制系统音频"
                });
            }
            return true;
        }

        if self
            .recording_mouse_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let next_enabled = !self.runtime.config.recording.capture_mouse;
            let previous = self.runtime.config.recording.capture_mouse;
            self.runtime.config.recording.capture_mouse = next_enabled;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.capture_mouse = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(if next_enabled {
                    "状态：已开启录制鼠标移动"
                } else {
                    "状态：已关闭录制鼠标移动"
                });
            }
            return true;
        }

        if self
            .recording_watermark_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            let next_enabled = !self.runtime.config.recording.watermark.enabled;
            let previous = self.runtime.config.recording.watermark.enabled;
            self.runtime.config.recording.watermark.enabled = next_enabled;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.watermark.enabled = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(if next_enabled {
                    "状态：已开启录屏水印"
                } else {
                    "状态：已关闭录屏水印"
                });
            }
            return true;
        }

        if let Some(index) = self
            .recording_position_items
            .iter()
            .position(|item| event.id == *item.id())
            && let Some(position) = ainput_recording::WATERMARK_POSITION_PRESETS.get(index)
        {
            let previous = self.runtime.config.recording.watermark.position;
            self.runtime.config.recording.watermark.position = *position;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.watermark.position = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(&format!("状态：录屏水印已切到{}", position.label()));
            }
            return true;
        }

        if let Some(index) = self
            .recording_fps_items
            .iter()
            .position(|item| event.id == *item.id())
            && let Some(fps) = ainput_recording::FPS_PRESETS.get(index)
        {
            let previous = self.runtime.config.recording.fps;
            self.runtime.config.recording.fps = *fps;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.fps = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(&format!("状态：录屏帧率已切到 {} FPS", fps));
            }
            return true;
        }

        if let Some(index) = self
            .recording_quality_items
            .iter()
            .position(|item| event.id == *item.id())
            && let Some(quality) = ainput_recording::QUALITY_PRESETS.get(index)
        {
            let previous = self.runtime.config.recording.quality;
            self.runtime.config.recording.quality = *quality;
            if let Err(error) =
                ainput_shell::save_config(&self.runtime.runtime_paths, &self.runtime.config)
            {
                self.runtime.config.recording.quality = previous;
                self.set_tray_status(&format!(
                    "状态：保存录屏设置失败 - {}",
                    shorten(&error.to_string(), 16)
                ));
            } else {
                self.sync_recording_menu();
                self.set_tray_status(&format!("状态：录屏画质已切到{}", quality.label()));
            }
            return true;
        }

        false
    }

    fn set_tray_status_from_result(&self, result: Result<()>, ok_status: &str) {
        match result {
            Ok(()) => self.set_tray_status(ok_status),
            Err(error) => self.set_tray_status(&format!(
                "状态：操作失败 - {}",
                shorten(&error.to_string(), 16)
            )),
        }
    }

    fn learn_from_clipboard(&self) -> Result<String> {
        let original = self
            .runtime
            .shared_state
            .last_voice_text()
            .ok_or_else(|| anyhow!("当前没有最近一次语音结果"))?;
        let corrected = {
            let mut clipboard = Clipboard::new().context("open clipboard")?;
            clipboard
                .get_text()
                .context("read corrected text from clipboard")?
        };

        match self
            .runtime
            .output_controller
            .learn_from_recent_correction(
                &original,
                &corrected,
                self.runtime.config.learning.auto_activate_threshold,
            )? {
            Some(outcome) => {
                let status = match outcome.status {
                    ainput_data::LearningStatus::Active => format!(
                        "状态：已学习 {} -> {}（已生效）",
                        outcome.spoken, outcome.canonical
                    ),
                    ainput_data::LearningStatus::Candidate => format!(
                        "状态：已记录候选 {} -> {}（{}/{}）",
                        outcome.spoken,
                        outcome.canonical,
                        outcome.count,
                        self.runtime.config.learning.auto_activate_threshold
                    ),
                    ainput_data::LearningStatus::Disabled => format!(
                        "状态：已记录但当前禁用 {} -> {}",
                        outcome.spoken, outcome.canonical
                    ),
                };
                Ok(status)
            }
            None => Ok("状态：未识别到单词修正，先复制修正后文本".to_string()),
        }
    }
}

fn app_status_icon(runtime: &AppRuntime, state: TrayVisualState, frame: u8) -> Icon {
    if state == TrayVisualState::Idle {
        return app_icon(runtime);
    }
    animated_status_icon(state, frame)
}

fn show_error_dialog(title: &str, message: &str) {
    let title_wide = to_wide_null(title);
    let message_wide = to_wide_null(message);
    let _ = unsafe {
        MessageBoxW(
            Some(HWND(std::ptr::null_mut())),
            PCWSTR(message_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        )
    };
}

fn to_wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn app_icon(runtime: &AppRuntime) -> Icon {
    let icon_path = runtime
        .runtime_paths
        .root_dir
        .join("assets")
        .join("app-icon.ico");
    if let Ok(icon) = Icon::from_path(&icon_path, Some((32, 32))) {
        return icon;
    }

    tracing::warn!(
        icon_path = %icon_path.display(),
        "failed to load custom icon from file, fallback to built-in placeholder icon"
    );

    fallback_app_icon()
}

fn fallback_app_icon() -> Icon {
    animated_status_icon(TrayVisualState::Idle, 0)
}

fn animated_status_icon(state: TrayVisualState, frame: u8) -> Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let pulse = match frame % 4 {
        0 => 0.64,
        1 => 0.82,
        2 => 1.0,
        _ => 0.84,
    };

    let (outer_r, outer_g, outer_b, inner_r, inner_g, inner_b) = match state {
        TrayVisualState::Idle => (14, 52, 146, 22, 93, 255),
        TrayVisualState::Loading => (28, 72, 108, 98, 178, 235),
        TrayVisualState::Voice => (18, 87, 214, 91, 184, 255),
        TrayVisualState::ScreenRecording => (120, 26, 26, 239, 68, 68),
        TrayVisualState::AutomationRecording => (128, 29, 29, 255, 92, 92),
        TrayVisualState::AutomationPlaying => (20, 92, 42, 56, 196, 96),
        TrayVisualState::AutomationPaused => (140, 90, 18, 247, 189, 49),
        TrayVisualState::Error => (113, 57, 10, 245, 125, 39),
    };

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let distance = (dx * dx + dy * dy).sqrt();
            let halo = matches!(
                state,
                TrayVisualState::AutomationRecording
                    | TrayVisualState::AutomationPlaying
                    | TrayVisualState::AutomationPaused
            ) && distance >= 12.8
                && distance <= 15.3
                && ((frame as i32 + x as i32 / 4 + y as i32 / 5) % 4 == 0);

            let (r, g, b, a) = if distance < 13.5 {
                (
                    (inner_r as f32 * pulse).round() as u8,
                    (inner_g as f32 * pulse).round() as u8,
                    (inner_b as f32 * pulse).round() as u8,
                    255,
                )
            } else if distance < 15.5 {
                if halo {
                    (255, 255, 255, 255)
                } else {
                    (outer_r, outer_g, outer_b, 255)
                }
            } else {
                (0, 0, 0, 0)
            };

            let mut pixel = [r, g, b, a];
            match state {
                TrayVisualState::AutomationPlaying => {
                    let shift = (frame % 4) as i32 - 1;
                    if x as i32 >= 12 + shift
                        && x as i32 <= 21 + shift
                        && y >= 9
                        && y <= 22
                        && x as i32 - (11 + shift) >= (y as i32 - 9) / 2
                    {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::AutomationPaused => {
                    let blink_on = frame % 4 != 1;
                    if blink_on
                        && (((x >= 10 && x <= 13) || (x >= 18 && x <= 21)) && y >= 9 && y <= 22)
                    {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::AutomationRecording | TrayVisualState::ScreenRecording => {
                    let badge_dx = x as f32 - 16.0;
                    let badge_dy = y as f32 - 16.0;
                    let badge_radius = match frame % 4 {
                        0 => 4.0,
                        1 => 5.0,
                        2 => 6.0,
                        _ => 5.0,
                    };
                    if (badge_dx * badge_dx + badge_dy * badge_dy).sqrt() < badge_radius {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::Error => {
                    if (x >= 10 && x <= 21 && y >= 10 && y <= 21)
                        && ((x as i32 - y as i32).abs() <= 1 || ((x + y) as i32 - 31).abs() <= 1)
                    {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::Loading => {
                    if x >= 14 && x <= 18 && y >= 14 && y <= 18 {
                        pixel = [255, 255, 255, 255];
                    }

                    let angle = (frame % 4) as f32 * std::f32::consts::FRAC_PI_2;
                    let dot_x = 16.0 + angle.cos() * 6.0;
                    let dot_y = 16.0 + angle.sin() * 6.0;
                    let orbit_distance =
                        ((x as f32 - dot_x).powi(2) + (y as f32 - dot_y).powi(2)).sqrt();
                    if orbit_distance <= 2.4 {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::Voice => {
                    if x >= 14 && x <= 18 && y >= 9 && y <= 18 {
                        pixel = [255, 255, 255, 255];
                    }
                    if x >= 12 && x <= 20 && y >= 18 && y <= 20 {
                        pixel = [255, 255, 255, 255];
                    }
                }
                TrayVisualState::Idle => {}
            }

            rgba.extend_from_slice(&pixel);
        }
    }

    Icon::from_rgba(rgba, size, size).expect("create tray icon pixels")
}

fn shorten(text: &str, max_chars: usize) -> String {
    let mut shortened = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        shortened.push_str("...");
    }
    shortened
}

fn sync_launch_at_login(enabled: bool) -> Result<()> {
    if enabled {
        set_launch_at_login_registry_value()
    } else {
        remove_launch_at_login_registry_value()
    }
}

fn set_launch_at_login_registry_value() -> Result<()> {
    let exe = current_exe_for_launch_at_login()?;
    let status = hidden_status(Command::new("reg").args([
        "add",
        RUN_REGISTRY_KEY,
        "/v",
        RUN_REGISTRY_VALUE_NAME,
        "/t",
        "REG_SZ",
        "/d",
        exe.as_str(),
        "/f",
    ]))?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("write launch-at-login registry value failed"))
    }
}

fn remove_launch_at_login_registry_value() -> Result<()> {
    let delete_status = hidden_status(Command::new("reg").args([
        "delete",
        RUN_REGISTRY_KEY,
        "/v",
        RUN_REGISTRY_VALUE_NAME,
        "/f",
    ]))?;

    if delete_status.success() {
        return Ok(());
    }

    let query_status = hidden_status(Command::new("reg").args([
        "query",
        RUN_REGISTRY_KEY,
        "/v",
        RUN_REGISTRY_VALUE_NAME,
    ]))?;

    if !query_status.success() {
        Ok(())
    } else {
        Err(anyhow!("remove launch-at-login registry value failed"))
    }
}

fn current_exe_for_launch_at_login() -> Result<String> {
    let path = std::env::current_exe()?;
    Ok(quote_command_path(path))
}

fn restart_application(runtime: &AppRuntime) -> Result<()> {
    let current_exe = std::env::current_exe().context("resolve current executable path")?;
    let mut command = Command::new(&current_exe);
    command
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(&runtime.runtime_paths.root_dir)
        .env("AINPUT_ROOT", &runtime.runtime_paths.root_dir);
    command
        .spawn()
        .with_context(|| format!("restart {}", current_exe.display()))?;
    Ok(())
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "未知 panic".to_string()
}

fn quote_command_path(path: PathBuf) -> String {
    format!("\"{}\"", path.as_os_str().to_string_lossy())
}

fn hidden_status(command: &mut Command) -> Result<std::process::ExitStatus> {
    command
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(Into::into)
}

fn automation_storage_dir(runtime: &AppRuntime) -> PathBuf {
    let legacy_dir = runtime
        .runtime_paths
        .root_dir
        .join("data")
        .join("automation");

    let persistent_dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("ainput").join("data").join("automation"))
        .unwrap_or_else(|| legacy_dir.clone());

    if let Err(error) = migrate_automation_storage_if_needed(
        &runtime.runtime_paths.root_dir,
        &legacy_dir,
        &persistent_dir,
    ) {
        tracing::warn!(error = %error, target = %persistent_dir.display(), "failed to prepare persistent automation storage");
    }

    persistent_dir
}

fn hud_overlay_modified_at(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn migrate_automation_storage_if_needed(
    root_dir: &std::path::Path,
    legacy_dir: &std::path::Path,
    persistent_dir: &std::path::Path,
) -> Result<()> {
    if persistent_dir == legacy_dir {
        fs::create_dir_all(persistent_dir)?;
        return Ok(());
    }

    fs::create_dir_all(persistent_dir)?;
    if automation_dir_has_any_data(persistent_dir) {
        return Ok(());
    }

    if automation_dir_has_any_data(legacy_dir) {
        copy_dir_contents(legacy_dir, persistent_dir)?;
        return Ok(());
    }

    if let Some(previous_dir) = latest_sibling_automation_dir(root_dir) {
        copy_dir_contents(&previous_dir, persistent_dir)?;
    }

    Ok(())
}

fn latest_sibling_automation_dir(root_dir: &std::path::Path) -> Option<PathBuf> {
    let parent = root_dir.parent()?;
    let current_name = root_dir.file_name()?.to_string_lossy().to_string();
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();

    for entry in fs::read_dir(parent).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name == current_name || !file_name.starts_with("ainput-") {
            continue;
        }

        let automation_dir = path.join("data").join("automation");
        if !automation_dir_has_any_data(&automation_dir) {
            continue;
        }

        let modified = fs::metadata(&automation_dir)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        candidates.push((modified, automation_dir));
    }

    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates.into_iter().map(|(_, path)| path).next()
}

fn automation_dir_has_any_data(path: &std::path::Path) -> bool {
    if !path.exists() {
        return false;
    }

    let slot_names_path = path.join("slot-names.json");
    if slot_names_path.exists() {
        return true;
    }

    let slots_dir = path.join("slots");
    match fs::read_dir(&slots_dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    }
}

fn copy_dir_contents(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    fs::create_dir_all(destination)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_path = entry.path();
        let target_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir_contents(&entry_path, &target_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &target_path)?;
        }
    }

    Ok(())
}

fn format_automation_slot_label(slot: &ainput_automation::SlotSnapshot) -> String {
    format!(
        "{}{}",
        slot.label,
        if slot.has_recording {
            " [已录制]"
        } else {
            " [空]"
        }
    )
}

fn format_automation_repeat_label(repeat_count: usize) -> String {
    format!("回放轮数 {repeat_count}")
}

fn format_current_automation_repeat_label(repeat_count: usize) -> String {
    if repeat_count > ainput_automation::REPEAT_COUNT_PRESET_MAX {
        format!("当前轮数：{repeat_count}（自定义）")
    } else {
        format!("当前轮数：{repeat_count}")
    }
}

fn prompt_for_automation_repeat_count(current: usize) -> Result<Option<usize>> {
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Add-Type -AssemblyName Microsoft.VisualBasic; $v=[Microsoft.VisualBasic.Interaction]::InputBox('请输入按键精灵回放轮数（1 到 {max}）','ainput 按键精灵回放轮数','{current}'); Write-Output $v",
        max = ainput_automation::REPEAT_COUNT_MAX,
        current = current
    );
    let output = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .output()
        .context("打开按键精灵回放轮数输入框失败")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("按键精灵回放轮数输入框失败: {}", stderr.trim()));
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }

    let repeat_count = value.parse::<usize>().map_err(|_| {
        anyhow!(
            "请输入 1 到 {} 之间的整数",
            ainput_automation::REPEAT_COUNT_MAX
        )
    })?;
    if !(1..=ainput_automation::REPEAT_COUNT_MAX).contains(&repeat_count) {
        return Err(anyhow!(
            "请输入 1 到 {} 之间的整数",
            ainput_automation::REPEAT_COUNT_MAX
        ));
    }

    Ok(Some(repeat_count))
}

fn prompt_for_recording_watermark_text(current: &str) -> Result<Option<String>> {
    let escaped_default = current.replace('\'', "''");
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Add-Type -AssemblyName Microsoft.VisualBasic; $v=[Microsoft.VisualBasic.Interaction]::InputBox('请输入录屏水印文本','ainput 录屏水印','{escaped_default}'); Write-Output $v"
    );
    let output = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .output()
        .context("打开录屏水印输入框失败")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("录屏水印输入框失败: {}", stderr.trim()));
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_cancel_enabled_only_during_active_automation_states() {
        for activity in [
            AutomationActivity::Recording,
            AutomationActivity::Playing,
            AutomationActivity::Paused,
        ] {
            assert!(DesktopApp::automation_cancel_enabled(&AutomationSnapshot {
                activity,
                status_line: String::new(),
                active_slot: 1,
                active_slot_label: String::new(),
                repeat_count: 1,
                elapsed_ms: 0,
                total_ms: None,
                progress_ratio: None,
                overlay_hint: None,
                last_click: None,
                slots: Vec::new(),
            }));
        }

        for activity in [AutomationActivity::Idle, AutomationActivity::Error] {
            assert!(!DesktopApp::automation_cancel_enabled(
                &AutomationSnapshot {
                    activity,
                    status_line: String::new(),
                    active_slot: 1,
                    active_slot_label: String::new(),
                    repeat_count: 1,
                    elapsed_ms: 0,
                    total_ms: None,
                    progress_ratio: None,
                    overlay_hint: None,
                    last_click: None,
                    slots: Vec::new(),
                }
            ));
        }
    }

    #[test]
    fn recording_cancel_enabled_only_while_recording_is_live() {
        assert!(DesktopApp::recording_cancel_enabled(&RecordingSnapshot {
            activity: RecordingActivity::Recording,
            status_line: String::new(),
            output_path: None,
        }));

        for activity in [
            RecordingActivity::Idle,
            RecordingActivity::Selecting,
            RecordingActivity::Stopping,
            RecordingActivity::Error,
        ] {
            assert!(!DesktopApp::recording_cancel_enabled(&RecordingSnapshot {
                activity,
                status_line: String::new(),
                output_path: None,
            }));
        }
    }

    #[test]
    fn automation_tray_frame_advances_every_180ms_and_wraps() {
        assert_eq!(
            DesktopApp::automation_tray_frame(&AutomationSnapshot {
                activity: AutomationActivity::Playing,
                status_line: String::new(),
                active_slot: 1,
                active_slot_label: String::new(),
                repeat_count: 1,
                elapsed_ms: 0,
                total_ms: None,
                progress_ratio: None,
                overlay_hint: None,
                last_click: None,
                slots: Vec::new(),
            }),
            0
        );
        assert_eq!(
            DesktopApp::automation_tray_frame(&AutomationSnapshot {
                activity: AutomationActivity::Playing,
                status_line: String::new(),
                active_slot: 1,
                active_slot_label: String::new(),
                repeat_count: 1,
                elapsed_ms: 180,
                total_ms: None,
                progress_ratio: None,
                overlay_hint: None,
                last_click: None,
                slots: Vec::new(),
            }),
            1
        );
        assert_eq!(
            DesktopApp::automation_tray_frame(&AutomationSnapshot {
                activity: AutomationActivity::Playing,
                status_line: String::new(),
                active_slot: 1,
                active_slot_label: String::new(),
                repeat_count: 1,
                elapsed_ms: 720,
                total_ms: None,
                progress_ratio: None,
                overlay_hint: None,
                last_click: None,
                slots: Vec::new(),
            }),
            0
        );
    }

    #[test]
    fn streaming_partial_message_prefers_text_and_skips_empty_preview() {
        assert_eq!(
            DesktopApp::streaming_partial_message("  已经识别出来了 "),
            Some("已经识别出来了".to_string())
        );
        assert_eq!(DesktopApp::streaming_partial_message("   "), None);
        assert_eq!(DesktopApp::streaming_partial_message("\t"), None);
    }
}
