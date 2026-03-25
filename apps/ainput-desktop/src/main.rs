#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkey;
mod overlay;

use anyhow::Result;
use std::fs;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

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

#[derive(Clone)]
struct AppRuntime {
    config: ainput_shell::AppConfig,
    runtime_paths: ainput_shell::RuntimePaths,
}

enum AppEvent {
    Worker(WorkerEvent),
    Hotkey(hotkey::HotkeyState),
    OverlayTick,
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

enum WorkerEvent {
    Started,
    RecordingStarted,
    Meter(f32),
    RecordingStopped,
    Transcribing,
    Delivered(String),
    ClipboardFallback(String),
    Error(String),
}

enum WorkerCommand {
    HotkeyPressed,
    HotkeyReleased,
}

struct DesktopApp {
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_started: bool,
    overlay_tick_started: bool,
    worker_tx: Option<mpsc::Sender<WorkerCommand>>,
    hotkey_monitor: Option<hotkey::GlobalHotkeyMonitor>,
    tray_icon: Option<TrayIcon>,
    overlay: Option<overlay::RecordingOverlay>,
    overlay_available: bool,
    exit_item: Option<MenuItem>,
    status_item: Option<MenuItem>,
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
            tray_icon: None,
            overlay: None,
            overlay_available: true,
            exit_item: None,
            status_item: None,
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

        thread::spawn(move || push_to_talk_worker(runtime, proxy, shutdown, worker_rx));
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
            let help_item = MenuItem::with_id("help", "使用说明", true, None);
            let exit_item = MenuItem::with_id("exit", "退出", true, None);
            let separator = PredefinedMenuItem::separator();

            let _ = tray_menu.append(&status_item);
            let _ = tray_menu.append(&separator);
            let _ = tray_menu.append(&help_item);
            let _ = tray_menu.append(&exit_item);

            let tray_icon = TrayIconBuilder::new()
                .with_tooltip("ainput\n待机中")
                .with_icon(app_icon())
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
            self.tray_icon = Some(tray_icon);
            self.overlay = overlay;
        }

        self.start_worker_once();
        self.start_overlay_tick_once();
        if self.hotkey_monitor.is_none() {
            self.hotkey_monitor = Some(
                hotkey::GlobalHotkeyMonitor::start(self.proxy.clone(), self.shutdown.clone())
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
                    self.set_tray_status("状态：按住 Ctrl+Win 录音");
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

fn push_to_talk_worker(
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_rx: mpsc::Receiver<WorkerCommand>,
) {
    let recognizer = match build_recognizer(&runtime) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(format!(
                "初始化识别器失败：{error}"
            ))));
            return;
        }
    };

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Started));
    let mut active_recording: Option<ainput_audio::ActiveRecording> = None;

    tracing::info!(
        shortcut = %runtime.config.shortcuts.push_to_talk,
        root_dir = %runtime.runtime_paths.root_dir.display(),
        "ainput worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(16)) {
            match command {
                WorkerCommand::HotkeyPressed => {
                    if active_recording.is_none() {
                        match ainput_audio::ActiveRecording::start_default_input() {
                            Ok(recording) => {
                                active_recording = Some(recording);
                                let _ =
                                    proxy.send_event(AppEvent::Worker(WorkerEvent::RecordingStarted));
                                tracing::info!("push-to-talk recording started");
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "failed to start microphone recording");
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("启动录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
                WorkerCommand::HotkeyReleased => {
                    if let Some(recording) = active_recording.take() {
                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::RecordingStopped));

                        match recording.stop() {
                            Ok(audio) => {
                                tracing::info!(
                                    sample_rate_hz = audio.sample_rate_hz,
                                    frames = audio.samples.len(),
                                    "push-to-talk recording captured"
                                );

                                if !audio.samples.is_empty() {
                                    let _ =
                                        proxy.send_event(AppEvent::Worker(WorkerEvent::Transcribing));
                                    match recognizer.transcribe_samples(
                                        audio.sample_rate_hz,
                                        &audio.samples,
                                        "microphone",
                                    ) {
                                        Ok(transcription) => {
                                            let text = transcription.text.trim().to_string();

                                            if !text.is_empty() {
                                                thread::sleep(Duration::from_millis(120));
                                                match ainput_output::deliver_text(
                                                    &text,
                                                    runtime.config.output.prefer_direct_paste,
                                                ) {
                                                    Ok(delivery) => {
                                                        let _ = cache_recent_text(
                                                            &runtime.runtime_paths.logs_dir,
                                                            &text,
                                                        );
                                                        tracing::info!(
                                                            ?delivery,
                                                            text = %text,
                                                            "transcription delivered"
                                                        );

                                                        let event = match delivery {
                                                            ainput_output::OutputDelivery::DirectPaste => {
                                                                WorkerEvent::Delivered(text)
                                                            }
                                                            ainput_output::OutputDelivery::ClipboardOnly => {
                                                                WorkerEvent::ClipboardFallback(text)
                                                            }
                                                        };
                                                        let _ = proxy.send_event(AppEvent::Worker(event));
                                                    }
                                                    Err(error) => {
                                                        tracing::error!(
                                                            error = %error,
                                                            "failed to deliver transcription output"
                                                        );
                                                        let _ = proxy.send_event(AppEvent::Worker(
                                                            WorkerEvent::Error(format!(
                                                                "输出结果失败：{error}"
                                                            )),
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            tracing::error!(
                                                error = %error,
                                                "failed to transcribe microphone audio"
                                            );
                                            let _ = proxy.send_event(AppEvent::Worker(
                                                WorkerEvent::Error(format!("语音识别失败：{error}")),
                                            ));
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "failed to stop microphone recording");
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("停止录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
            }
        }

        if let Some(recording) = &active_recording {
            let level = normalize_audio_level(recording.current_level());
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Meter(level)));
        }
    }
}

fn app_icon() -> Icon {
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

fn normalize_audio_level(raw_level: f32) -> f32 {
    (raw_level * 6.5).sqrt().clamp(0.0, 1.0)
}
