#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkey;
mod instance;
mod maintenance;
mod overlay;
mod screenshot;
mod worker;

use ainput_automation::{AutomationActivity, AutomationService};
use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use maintenance::{MaintenanceHandle, SharedRuntimeState};
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
use std::time::Duration;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use worker::{WorkerCommand, WorkerEvent};

const RUN_REGISTRY_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_REGISTRY_VALUE_NAME: &str = "ainput";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() -> Result<()> {
    let bootstrap = ainput_shell::bootstrap()?;
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("transcribe-wav") {
        let runtime = build_runtime(&bootstrap)?;
        let wav_path = args
            .get(2)
            .ok_or_else(|| anyhow!("usage: ainput-desktop transcribe-wav <path-to-wav>"))?;
        let recognizer = build_recognizer(&runtime)?;
        let transcription = recognizer.transcribe_wav_file(wav_path)?;
        cache_recent_text(&bootstrap.runtime_paths.logs_dir, &transcription.text)?;
        println!("{}", transcription.text);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("record-once") {
        let runtime = build_runtime(&bootstrap)?;
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

    if args.get(1).map(String::as_str) == Some("bootstrap") {
        println!(
            "ainput bootstrap ready: voice_hotkey={}, capture_hotkey={}, config={}",
            bootstrap.config.hotkeys.voice_input,
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

    instance::replace_existing_instance()?;
    run_desktop_app(bootstrap)
}

fn run_desktop_app(bootstrap: ainput_shell::Bootstrap) -> Result<()> {
    let runtime = build_runtime(&bootstrap)?;
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

fn build_runtime(bootstrap: &ainput_shell::Bootstrap) -> Result<AppRuntime> {
    let output_controller = Arc::new(ainput_output::OutputController::new(
        &bootstrap.runtime_paths.root_dir,
    )?);
    let shared_state = SharedRuntimeState::new();
    let maintenance = MaintenanceHandle::start(
        bootstrap.runtime_paths.logs_dir.clone(),
        bootstrap.config.voice.history_file_name.clone(),
        bootstrap.config.voice.history_limit,
    );

    Ok(AppRuntime {
        config: bootstrap.config.clone(),
        runtime_paths: bootstrap.runtime_paths.clone(),
        output_controller,
        shared_state,
        maintenance,
    })
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
    maintenance: MaintenanceHandle,
}

pub(crate) enum AppEvent {
    Worker(WorkerEvent),
    Hotkey(hotkey::HotkeyState),
    Capture(screenshot::CaptureEvent),
    AutomationUpdated,
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
}

struct DesktopApp {
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_started: bool,
    overlay_tick_started: bool,
    worker_tx: Option<mpsc::Sender<WorkerCommand>>,
    hotkey_monitor: Option<hotkey::GlobalHotkeyMonitor>,
    automation_service: Option<AutomationService>,
    tray_icon: Option<TrayIcon>,
    overlay: Option<overlay::RecordingOverlay>,
    overlay_available: bool,
    mode: AppMode,
    exit_item: Option<MenuItem>,
    status_item: Option<MenuItem>,
    learn_terms_item: Option<MenuItem>,
    mouse_middle_item: Option<CheckMenuItem>,
    launch_at_login_item: Option<CheckMenuItem>,
    capture_save_item: Option<CheckMenuItem>,
    automation_status_item: Option<MenuItem>,
    automation_slot_items: Vec<CheckMenuItem>,
    automation_repeat_items: Vec<CheckMenuItem>,
    automation_edit_names_item: Option<MenuItem>,
    automation_open_dir_item: Option<MenuItem>,
}

impl DesktopApp {
    fn new(runtime: AppRuntime, proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            runtime,
            proxy,
            shutdown: Arc::new(AtomicBool::new(false)),
            worker_started: false,
            overlay_tick_started: false,
            worker_tx: None,
            hotkey_monitor: None,
            automation_service: None,
            tray_icon: None,
            overlay: None,
            overlay_available: true,
            mode: AppMode::Idle,
            exit_item: None,
            status_item: None,
            learn_terms_item: None,
            mouse_middle_item: None,
            launch_at_login_item: None,
            capture_save_item: None,
            automation_status_item: None,
            automation_slot_items: Vec::new(),
            automation_repeat_items: Vec::new(),
            automation_edit_names_item: None,
            automation_open_dir_item: None,
        }
    }

    fn start_worker_once(&mut self) {
        if self.worker_started {
            return;
        }

        self.worker_started = true;
        let runtime = self.runtime.clone();
        let proxy = self.proxy.clone();
        let shutdown = self.shutdown.clone();
        let (worker_tx, worker_rx) = mpsc::channel();
        self.worker_tx = Some(worker_tx);

        thread::spawn(move || worker::push_to_talk_worker(runtime, proxy, shutdown, worker_rx));
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
                thread::sleep(Duration::from_millis(33));
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

    fn build_tray_once(&mut self) -> Result<()> {
        if self.tray_icon.is_some() {
            return Ok(());
        }

        if self.automation_service.is_none() {
            let proxy = self.proxy.clone();
            self.automation_service = Some(AutomationService::start(
                automation_storage_dir(&self.runtime),
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

        let tray_menu = Menu::new();
        let status_item = MenuItem::with_id("status", "状态：待机中", false, None);

        let voice_hotkey_hint = MenuItem::with_id(
            "voice_hotkey_hint",
            format!("按住说话热键：{}", self.runtime.config.hotkeys.voice_input),
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
        let automation_repeat_items: Vec<CheckMenuItem> = (1..=5)
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
                &help_item,
            ],
        )?;

        let separator = PredefinedMenuItem::separator();
        let _ = tray_menu.append(&status_item);
        let _ = tray_menu.append(&separator);
        let _ = tray_menu.append(&voice_menu);
        let _ = tray_menu.append(&capture_menu);
        let _ = tray_menu.append(&automation_menu);
        let _ = tray_menu.append(&learning_menu);
        let _ = tray_menu.append(&general_menu);
        let _ = tray_menu.append(&exit_item);

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("ainput\n待机中")
            .with_icon(app_icon(&self.runtime))
            .with_menu(Box::new(tray_menu))
            .build()
            .map_err(|error| anyhow!("create tray icon: {error}"))?;

        let overlay = match overlay::RecordingOverlay::create() {
            Ok(overlay) => Some(overlay),
            Err(error) => {
                tracing::error!(error = %error, "create recording overlay failed");
                None
            }
        };
        self.overlay_available = overlay.is_some();

        self.status_item = Some(status_item);
        self.exit_item = Some(exit_item);
        self.learn_terms_item = Some(learn_terms_item);
        self.mouse_middle_item = Some(mouse_middle_item);
        self.launch_at_login_item = Some(launch_at_login_item);
        self.capture_save_item = Some(capture_save_item);
        self.automation_status_item = Some(automation_status_item);
        self.automation_slot_items = automation_slot_items;
        self.automation_repeat_items = automation_repeat_items;
        self.automation_edit_names_item = Some(automation_edit_names_item);
        self.automation_open_dir_item = Some(automation_open_dir_item);
        self.tray_icon = Some(tray_icon);
        self.overlay = overlay;
        self.sync_automation_menu();
        Ok(())
    }

    fn sync_automation_menu(&self) {
        let Some(service) = &self.automation_service else {
            return;
        };
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
    }

    fn handle_automation_update(&mut self) {
        let Some(service) = &self.automation_service else {
            return;
        };
        let snapshot = service.snapshot();
        self.sync_automation_menu();

        match snapshot.activity {
            AutomationActivity::Recording => {
                self.mode = AppMode::Automation;
                self.set_tray_status("状态：按键精灵录制中");
            }
            AutomationActivity::Playing => {
                self.mode = AppMode::Automation;
                self.set_tray_status("状态：按键精灵回放中");
            }
            AutomationActivity::Paused => {
                self.mode = AppMode::Automation;
                self.set_tray_status("状态：按键精灵已暂停");
            }
            AutomationActivity::Error => {
                self.mode = AppMode::Idle;
                self.set_tray_status(&format!(
                    "状态：按键精灵错误 - {}",
                    shorten(&snapshot.status_line, 14)
                ));
            }
            AutomationActivity::Idle => {
                if self.mode == AppMode::Automation {
                    self.mode = AppMode::Idle;
                    self.set_tray_status("状态：待机中");
                }
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
            self.set_tray_status(&format!(
                "状态：托盘初始化失败 - {}",
                shorten(&error.to_string(), 16)
            ));
            return;
        }

        self.start_worker_once();
        self.start_overlay_tick_once();
        if self.hotkey_monitor.is_none() {
            self.hotkey_monitor = Some(
                hotkey::GlobalHotkeyMonitor::start(
                    self.proxy.clone(),
                    self.shutdown.clone(),
                    hotkey::HotkeyBindings {
                        voice_input: self.runtime.config.hotkeys.voice_input.clone(),
                        screen_capture: self.runtime.config.hotkeys.screen_capture.clone(),
                    },
                    self.runtime.config.hotkeys.mouse_middle_hold_enabled,
                )
                .expect("start global hotkey monitor"),
            );
        }

        match sync_launch_at_login(self.runtime.config.startup.launch_at_login) {
            Ok(()) => self.set_tray_status("状态：待机中"),
            Err(error) => {
                tracing::error!(error = %error, "sync launch-at-login setting failed");
                self.set_tray_status(&format!(
                    "状态：开机启动设置失败 - {}",
                    shorten(&error.to_string(), 14)
                ));
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Worker(worker_event) => match worker_event {
                WorkerEvent::Started => {
                    self.mode = AppMode::Idle;
                    self.set_tray_status("状态：待机中");
                }
                WorkerEvent::RecordingStarted => {
                    self.mode = AppMode::Voice;
                    self.set_tray_status("状态：正在录音");
                    if let Some(overlay) = &mut self.overlay {
                        overlay.show();
                    }
                }
                WorkerEvent::Meter(level) => {
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_level(level);
                    }
                }
                WorkerEvent::RecordingStopped => {
                    self.set_tray_status("状态：录音结束");
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_level(0.0);
                        overlay.hide();
                    }
                }
                WorkerEvent::Transcribing => {
                    self.mode = AppMode::Voice;
                    self.set_tray_status("状态：正在识别");
                }
                WorkerEvent::IgnoredSilence => {
                    self.mode = AppMode::Idle;
                    self.set_tray_status("状态：待机中");
                }
                WorkerEvent::Delivered => {
                    self.mode = AppMode::Idle;
                    self.set_tray_status("状态：文本已直贴");
                }
                WorkerEvent::ClipboardFallback => {
                    self.mode = AppMode::Idle;
                    self.set_tray_status("状态：已复制到剪贴板");
                }
                WorkerEvent::Error(message) => {
                    self.mode = AppMode::Idle;
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_level(0.0);
                        overlay.hide();
                    }
                    self.set_tray_status(&format!("状态：错误 - {}", shorten(&message, 18)));
                }
            },
            AppEvent::Hotkey(state) => match state {
                hotkey::HotkeyState::VoicePressed => {
                    if self.mode == AppMode::Idle && self.runtime.config.voice.enabled {
                        self.mode = AppMode::Voice;
                        if let Some(worker_tx) = &self.worker_tx {
                            let _ = worker_tx.send(WorkerCommand::HotkeyPressed);
                        }
                    }
                }
                hotkey::HotkeyState::VoiceReleased => {
                    if self.mode == AppMode::Voice
                        && let Some(worker_tx) = &self.worker_tx
                    {
                        let _ = worker_tx.send(WorkerCommand::HotkeyReleased);
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
            AppEvent::OverlayTick => {
                if let Some(overlay) = &mut self.overlay {
                    overlay.tick();
                }
            }
            AppEvent::Tray(event) => {
                if let TrayIconEvent::DoubleClick { .. } = event {
                    self.set_tray_status("状态：待机中");
                }
            }
            AppEvent::Menu(event) => self.handle_menu_event(event_loop, event),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.hotkey_monitor = None;
        self.automation_service = None;
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

        if self
            .exit_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
            self.shutdown.store(true, Ordering::Relaxed);
            event_loop.exit();
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
        let Some(service) = &self.automation_service else {
            return false;
        };

        if self
            .automation_edit_names_item
            .as_ref()
            .map(|item| event.id == *item.id())
            .unwrap_or(false)
        {
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
            self.set_tray_status_from_result(
                service.open_slots_dir(),
                "状态：已打开按键精灵录制目录",
            );
            return true;
        }

        if let Some(index) = self
            .automation_slot_items
            .iter()
            .position(|item| event.id == *item.id())
        {
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
            match service.select_repeat_count(index + 1) {
                Ok(()) => self.sync_automation_menu(),
                Err(error) => self.set_tray_status(&format!(
                    "状态：按键精灵轮数失败 - {}",
                    shorten(&error.to_string(), 14)
                )),
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
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let distance = (dx * dx + dy * dy).sqrt();

            let (r, g, b, a) = if distance < 13.5 {
                (22, 93, 255, 255)
            } else if distance < 15.5 {
                (14, 52, 146, 255)
            } else {
                (0, 0, 0, 0)
            };

            rgba.extend_from_slice(&[r, g, b, a]);
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
    runtime
        .runtime_paths
        .root_dir
        .join("data")
        .join("automation")
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
