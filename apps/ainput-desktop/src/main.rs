#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkey;
mod overlay;
mod worker;

use anyhow::Result;
use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use worker::{WorkerCommand, WorkerEvent};

fn main() -> Result<()> {
    let bootstrap = ainput_shell::bootstrap()?;
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("transcribe-wav") {
        let wav_path = args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("usage: ainput-desktop transcribe-wav <path-to-wav>"))?;

        let recognizer = build_recognizer(&AppRuntime {
            config: bootstrap.config.clone(),
            runtime_paths: bootstrap.runtime_paths.clone(),
        })?;

        let transcription = recognizer.transcribe_wav_file(wav_path)?;
        cache_recent_text(&bootstrap.runtime_paths.logs_dir, &transcription.text)?;
        println!("{}", transcription.text);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("record-once") {
        let seconds = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("3")
            .parse::<u64>()?;

        let recognizer = build_recognizer(&AppRuntime {
            config: bootstrap.config.clone(),
            runtime_paths: bootstrap.runtime_paths.clone(),
        })?;
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
            "ainput bootstrap ready: shortcut={}, config={}",
            bootstrap.config.shortcuts.push_to_talk,
            bootstrap.runtime_paths.config_file.display()
        );
        return Ok(());
    }

    run_desktop_app(bootstrap)
}

fn run_desktop_app(bootstrap: ainput_shell::Bootstrap) -> Result<()> {
    let runtime = AppRuntime {
        config: bootstrap.config.clone(),
        runtime_paths: bootstrap.runtime_paths.clone(),
    };

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

fn open_user_terms_document(runtime: &AppRuntime) -> Result<()> {
    let path = ainput_output::ensure_user_terms_document(&runtime.runtime_paths.root_dir)?;
    Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

fn open_readme_document(runtime: &AppRuntime) -> Result<()> {
    let path = runtime.runtime_paths.root_dir.join("README.md");
    Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

#[derive(Clone)]
pub(crate) struct AppRuntime {
    config: ainput_shell::AppConfig,
    runtime_paths: ainput_shell::RuntimePaths,
}

pub(crate) enum AppEvent {
    Worker(WorkerEvent),
    Hotkey(hotkey::HotkeyState),
    OverlayTick,
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

struct DesktopApp {
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_started: bool,
    overlay_tick_started: bool,
    resource_monitor_started: bool,
    worker_tx: Option<mpsc::Sender<WorkerCommand>>,
    hotkey_monitor: Option<hotkey::GlobalHotkeyMonitor>,
    tray_icon: Option<TrayIcon>,
    overlay: Option<overlay::RecordingOverlay>,
    overlay_available: bool,
    exit_item: Option<MenuItem>,
    status_item: Option<MenuItem>,
    open_terms_item: Option<MenuItem>,
    learn_terms_item: Option<MenuItem>,
    mouse_middle_item: Option<CheckMenuItem>,
}

impl DesktopApp {
    fn new(runtime: AppRuntime, proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            runtime,
            proxy,
            shutdown: Arc::new(AtomicBool::new(false)),
            worker_started: false,
            overlay_tick_started: false,
            resource_monitor_started: false,
            worker_tx: None,
            hotkey_monitor: None,
            tray_icon: None,
            overlay: None,
            overlay_available: true,
            exit_item: None,
            status_item: None,
            open_terms_item: None,
            learn_terms_item: None,
            mouse_middle_item: None,
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
                thread::sleep(Duration::from_millis(7));
            }
        });
    }

    fn start_resource_monitor_once(&mut self) {
        if self.resource_monitor_started {
            return;
        }

        self.resource_monitor_started = true;
        let shutdown = self.shutdown.clone();
        thread::spawn(move || {
            let pid = Pid::from_u32(std::process::id());
            let mut system = System::new_all();

            while !shutdown.load(Ordering::Relaxed) {
                log_process_heartbeat(&mut system, pid);

                for _ in 0..300 {
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
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

        if let Some(status_item) = &self.status_item {
            status_item.set_text(&rendered_status);
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

        if self.tray_icon.is_none() {
            let tray_menu = Menu::new();
            let status_item = MenuItem::with_id("status", "状态：待机中", false, None);
            let learn_terms_item = MenuItem::with_id("learn_terms", "学习最近一次修正", true, None);
            let open_terms_item = MenuItem::with_id("open_terms", "手动添加易错词", true, None);
            let mouse_middle_item = CheckMenuItem::with_id(
                "mouse_middle_toggle",
                "启用鼠标中键长按录音",
                true,
                self.runtime.config.shortcuts.mouse_middle_hold_enabled,
                None,
            );
            let help_item = MenuItem::with_id("help", "使用说明", true, None);
            let exit_item = MenuItem::with_id("exit", "退出", true, None);
            let separator = PredefinedMenuItem::separator();

            let _ = tray_menu.append(&status_item);
            let _ = tray_menu.append(&separator);
            let _ = tray_menu.append(&mouse_middle_item);
            let _ = tray_menu.append(&learn_terms_item);
            let _ = tray_menu.append(&open_terms_item);
            let _ = tray_menu.append(&help_item);
            let _ = tray_menu.append(&exit_item);

            let tray_icon = TrayIconBuilder::new()
                .with_tooltip("ainput\n待机中")
                .with_icon(app_icon(&self.runtime))
                .with_menu(Box::new(tray_menu))
                .build()
                .expect("create ainput tray icon");

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
            self.open_terms_item = Some(open_terms_item);
            self.learn_terms_item = Some(learn_terms_item);
            self.mouse_middle_item = Some(mouse_middle_item);
            self.tray_icon = Some(tray_icon);
            self.overlay = overlay;
        }

        self.start_worker_once();
        self.start_overlay_tick_once();
        self.start_resource_monitor_once();
        if self.hotkey_monitor.is_none() {
            self.hotkey_monitor = Some(
                hotkey::GlobalHotkeyMonitor::start(
                    self.proxy.clone(),
                    self.shutdown.clone(),
                    self.runtime.config.shortcuts.mouse_middle_hold_enabled,
                )
                .expect("start global hotkey monitor"),
            );
        }
        self.set_tray_status("状态：待机中");
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Worker(worker_event) => match worker_event {
                WorkerEvent::Started => {
                    self.set_tray_status("状态：待机中");
                }
                WorkerEvent::RecordingStarted => {
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
                    self.set_tray_status("状态：正在识别");
                }
                WorkerEvent::IgnoredSilence => {
                    self.set_tray_status("状态：待机中");
                }
                WorkerEvent::Delivered(_text) => {
                    self.set_tray_status("状态：待机中");
                }
                WorkerEvent::ClipboardFallback(_text) => {
                    self.set_tray_status("状态：已复制到剪贴板");
                }
                WorkerEvent::Error(message) => {
                    if let Some(overlay) = &mut self.overlay {
                        overlay.set_level(0.0);
                        overlay.hide();
                    }
                    self.set_tray_status(&format!("状态：错误 - {}", shorten(&message, 18)));
                }
            },
            AppEvent::Hotkey(state) => {
                if let Some(worker_tx) = &self.worker_tx {
                    let command = match state {
                        hotkey::HotkeyState::Pressed => WorkerCommand::HotkeyPressed,
                        hotkey::HotkeyState::Released => WorkerCommand::HotkeyReleased,
                    };
                    let _ = worker_tx.send(command);
                }
            }
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
            AppEvent::Menu(event) => {
                if self
                    .exit_item
                    .as_ref()
                    .map(|item| event.id == *item.id())
                    .unwrap_or(false)
                {
                    self.shutdown.store(true, Ordering::Relaxed);
                    event_loop.exit();
                } else if event.id.0 == "help" {
                    match open_readme_document(&self.runtime) {
                        Ok(()) => self.set_tray_status("状态：已打开使用说明"),
                        Err(error) => self.set_tray_status(&format!(
                            "状态：打开说明失败 - {}",
                            shorten(&error.to_string(), 16)
                        )),
                    }
                } else if self
                    .mouse_middle_item
                    .as_ref()
                    .map(|item| event.id == *item.id())
                    .unwrap_or(false)
                {
                    let next_enabled = !self.runtime.config.shortcuts.mouse_middle_hold_enabled;
                    self.runtime.config.shortcuts.mouse_middle_hold_enabled = next_enabled;
                    hotkey::set_mouse_middle_enabled(next_enabled);
                    if let Some(item) = &self.mouse_middle_item {
                        item.set_checked(next_enabled);
                    }
                    match ainput_shell::save_config(
                        &self.runtime.runtime_paths,
                        &self.runtime.config,
                    ) {
                        Ok(()) => {
                            let status = if next_enabled {
                                "状态：已启用鼠标中键长按录音"
                            } else {
                                "状态：已关闭鼠标中键长按录音"
                            };
                            self.set_tray_status(status);
                        }
                        Err(error) => {
                            self.runtime.config.shortcuts.mouse_middle_hold_enabled = !next_enabled;
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
                    .open_terms_item
                    .as_ref()
                    .map(|item| event.id == *item.id())
                    .unwrap_or(false)
                {
                    match open_user_terms_document(&self.runtime) {
                        Ok(()) => self.set_tray_status("状态：已打开易错词文档"),
                        Err(error) => self.set_tray_status(&format!(
                            "状态：打开文档失败 - {}",
                            shorten(&error.to_string(), 16)
                        )),
                    }
                } else if self
                    .learn_terms_item
                    .as_ref()
                    .map(|item| event.id == *item.id())
                    .unwrap_or(false)
                {
                    match ainput_output::learn_from_recent_correction(
                        &self.runtime.runtime_paths.root_dir,
                        &self.runtime.runtime_paths.logs_dir,
                    ) {
                        Ok(Some(outcome)) => {
                            let status = if outcome.activated {
                                format!(
                                    "状态：已学习 {} -> {}（已生效）",
                                    outcome.spoken, outcome.canonical
                                )
                            } else {
                                format!(
                                    "状态：已记录 {} -> {}（{}/2）",
                                    outcome.spoken, outcome.canonical, outcome.count
                                )
                            };
                            self.set_tray_status(&status);
                        }
                        Ok(None) => {
                            self.set_tray_status("状态：未识别到单词修正，先复制修正后文本");
                        }
                        Err(error) => self.set_tray_status(&format!(
                            "状态：学习失败 - {}",
                            shorten(&error.to_string(), 16)
                        )),
                    }
                }
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.hotkey_monitor = None;
        if let Some(overlay) = &mut self.overlay {
            overlay.hide();
        }
        TrayIconEvent::set_event_handler::<fn(TrayIconEvent)>(None);
        MenuEvent::set_event_handler::<fn(MenuEvent)>(None);
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

fn log_process_heartbeat(system: &mut System, pid: Pid) {
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

    let Some(process) = system.process(pid) else {
        tracing::warn!("process heartbeat skipped because current process is unavailable");
        return;
    };

    tracing::info!(
        cpu_usage_percent = format_args!("{:.2}", process.cpu_usage()),
        working_set_mb = format_args!("{:.1}", process.memory() as f64 / 1024.0 / 1024.0),
        virtual_memory_mb =
            format_args!("{:.1}", process.virtual_memory() as f64 / 1024.0 / 1024.0),
        runtime_seconds = process.run_time(),
        "process heartbeat"
    );
}
