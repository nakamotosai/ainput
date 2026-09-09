use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HICON, IDC_ARROW, IDI_APPLICATION, IDI_INFORMATION,
    IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LoadCursorW, LoadIconW, LoadImageW, MF_CHECKED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, PostQuitMessage, PostThreadMessageW,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, KillTimer, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    PostMessageW, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CREATE, WM_DESTROY, WM_TIMER,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::api_settings_panel::ApiSettingsPanelController;
use crate::history_panel::HistoryPanelController;
use crate::hud::HudController;
use crate::rewrite_language::RewriteLanguageController;
use crate::rewrite_prompt::{
    PRESET_COMPACT, PRESET_CUSTOM, PRESET_LIGHT, PRESET_STANDARD, RewritePromptController,
};
use crate::rewrite_prompt_panel::RewritePromptPanelController;
use crate::hotkey_panel::HotkeyPanelController;
use crate::hotkey_user::HotkeyUserController;
use crate::voice_command::VoiceCommandController;
use crate::voice_command_panel::VoiceCommandPanelController;

const TRAY_THREAD_QUIT: u32 = WM_APP + 41;
const TRAY_CALLBACK: u32 = WM_APP + 42;
const TRAY_API_NOTIFICATION: u32 = WM_APP + 44;
// WM_APP+45/46 已被切换动画占用；外部（第二实例/部署脚本）用这个窗口消息
// 请求托盘走正常退出路径，等同于用户点「退出」。
pub(crate) const TRAY_REMOTE_QUIT_MSG: u32 = WM_APP + 47;
const SWITCH_ANIM_TIMER: usize = 0xA302;
const SWITCH_ANIM_INTERVAL_MS: u32 = 180;
const MSG_SWITCH_ANIM_START: u32 = WM_APP + 45;
const MSG_SWITCH_ANIM_STOP: u32 = WM_APP + 46;
const TRAY_UID: u32 = 1;
static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
static SWITCH_ICON_ACTIVE: AtomicBool = AtomicBool::new(false);
static SWITCH_BLINK_STATE: AtomicBool = AtomicBool::new(false);
static CACHED_APP_ICON: Mutex<Option<usize>> = Mutex::new(None);
static API_NOTIFICATION_QUEUE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static TRAY_HWND: OnceLock<usize> = OnceLock::new();

const MENU_API_SETTINGS: usize = 2010;
const MENU_HISTORY: usize = 2012;
const MENU_AUTO_START: usize = 2011;
const MENU_EXIT: usize = 2005;
const MENU_RESTART: usize = 2006;
const MENU_REWRITE_ENABLED: usize = 2700;
const MENU_PROMPT_STANDARD: usize = 2801;
const MENU_PROMPT_COMPACT: usize = 2802;
const MENU_PROMPT_LIGHT: usize = 2803;
const MENU_PROMPT_CUSTOM: usize = 2804;
const MENU_PROMPT_EDIT: usize = 2805;
const MENU_VOICE_COMMAND_ENABLED: usize = 2901;
const MENU_VOICE_COMMAND_EDIT: usize = 2902;
const MENU_HOTKEY_EDIT: usize = 2910;
const MENU_ENGINE_SENSE_VOICE: usize = 3001;
const MENU_ENGINE_QWEN3: usize = 3002;
const MENU_ENGINE_FUNASR_NANO: usize = 3003;
const MENU_ENGINE_FUNASR_GGUF: usize = 3004;
const MENU_ENGINE_PARAFORMER_STREAMING: usize = 3005;
// 2026-09-10 云端档收起：编号保留，Docker 回归时恢复菜单。
#[allow(dead_code)]
const MENU_ENGINE_NIM_WHISPER: usize = 3006;

pub struct Tray {
    thread_id: u32,
    join: Option<thread::JoinHandle<()>>,
}

impl Tray {
    pub fn start(
        hud: HudController,
        api_settings: ApiSettingsPanelController,
        history_panel: HistoryPanelController,
        rewrite_language: RewriteLanguageController,
        rewrite_prompt: RewritePromptController,
        rewrite_prompt_panel: RewritePromptPanelController,
        voice_command: VoiceCommandController,
        voice_command_panel: VoiceCommandPanelController,
        hotkey_user: HotkeyUserController,
        hotkey_panel: HotkeyPanelController,
        api_config_path: PathBuf,
        config_path: PathBuf,
        current_engine: String,
        shared_engine: Arc<Mutex<String>>,
        shared_recognizer: Arc<Mutex<Option<crate::local_asr::LocalSenseVoiceRecognizer>>>,
        shared_paraformer: Arc<
            Mutex<Option<crate::paraformer_streaming::ParaformerStreamingRecognizer>>,
        >,
        install_root: PathBuf,
        local_config: crate::config::LocalNonstreamingConfig,
        paraformer_config: crate::config::ParaformerStreamingConfig,
        gguf_config: crate::config::FunasrGgufConfig,
        nim_config: crate::config::NimWhisperConfig,
        api_notifications: mpsc::Receiver<String>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let join = thread::spawn(move || {
            TRAY_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            TRAY_STATE.with(|state| {
                *state.borrow_mut() = Some(TrayState {
                    hud,
                    api_settings,
                    history_panel,
                    rewrite_language,
                    rewrite_prompt,
                    rewrite_prompt_panel,
                    voice_command,
                    voice_command_panel,
                    hotkey_user,
                    hotkey_panel,
                    api_config_path,
                    config_path,
                    current_engine,
                    shared_engine,
                    shared_recognizer,
                    shared_paraformer,
                    install_root,
                    local_config,
                    paraformer_config,
                    gguf_config,
                    nim_config,
                    switching: Arc::new(AtomicBool::new(false)),
                    shutdown,
                });
            });
            let result = unsafe { run_tray_thread(api_notifications) };
            if let Err(error) = result {
                warn!(error = %error, "tray thread failed");
            }
        });
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("tray thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self {
            thread_id,
            join: Some(join),
        })
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, TRAY_THREAD_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            if let Err(error) = join.join() {
                warn!(?error, "tray thread join failed");
            }
        }
    }
}

#[derive(Clone)]
struct TrayState {
    hud: HudController,
    api_settings: ApiSettingsPanelController,
    history_panel: HistoryPanelController,
    rewrite_language: RewriteLanguageController,
    rewrite_prompt: RewritePromptController,
    rewrite_prompt_panel: RewritePromptPanelController,
    voice_command: VoiceCommandController,
    voice_command_panel: VoiceCommandPanelController,
    hotkey_user: HotkeyUserController,
    hotkey_panel: HotkeyPanelController,
    api_config_path: PathBuf,
    config_path: PathBuf,
    current_engine: String,
    shared_engine: Arc<Mutex<String>>,
    shared_recognizer: Arc<Mutex<Option<crate::local_asr::LocalSenseVoiceRecognizer>>>,
    shared_paraformer:
        Arc<Mutex<Option<crate::paraformer_streaming::ParaformerStreamingRecognizer>>>,
    install_root: PathBuf,
    local_config: crate::config::LocalNonstreamingConfig,
    paraformer_config: crate::config::ParaformerStreamingConfig,
    gguf_config: crate::config::FunasrGgufConfig,
    nim_config: crate::config::NimWhisperConfig,
    switching: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

thread_local! {
    static TRAY_READY: std::cell::RefCell<Option<mpsc::Sender<Result<u32, String>>>> =
        const { std::cell::RefCell::new(None) };
    static TRAY_STATE: std::cell::RefCell<Option<TrayState>> =
        const { std::cell::RefCell::new(None) };
}

unsafe fn run_tray_thread(api_notifications: mpsc::Receiver<String>) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow!("get module handle failed: {error}"))?;
    unsafe { register_tray_class(HINSTANCE(instance.0))? };
    let hwnd = unsafe { create_tray_window(HINSTANCE(instance.0))? };
    let _ = TRAY_HWND.set(hwnd.0 as usize);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED_MESSAGE.store(taskbar_created, Ordering::Relaxed);
    info!(
        message_id = taskbar_created,
        "registered TaskbarCreated tray recovery message"
    );
    unsafe { add_tray_icon(hwnd) };

    let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let api_thread_id = thread_id;
    thread::spawn(move || {
        while let Ok(notification) = api_notifications.recv() {
            API_NOTIFICATION_QUEUE
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .map(|mut queue| queue.push(notification))
                .ok();
            let _ = unsafe {
                PostThreadMessageW(api_thread_id, TRAY_API_NOTIFICATION, WPARAM(0), LPARAM(0))
            };
        }
    });
    TRAY_READY.with(|ready| {
        if let Some(sender) = ready.borrow_mut().take() {
            let _ = sender.send(Ok(thread_id));
        }
    });

    loop {
        let mut msg = MSG::default();
        let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if has_message.0 == -1 {
            return Err(anyhow!("tray GetMessage failed"));
        }
        if has_message.0 == 0 || msg.message == TRAY_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == TRAY_API_NOTIFICATION {
            if let Some(message) = take_api_notification() {
                unsafe { show_api_setup_balloon(hwnd, &message) };
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn take_api_notification() -> Option<String> {
    API_NOTIFICATION_QUEUE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()
        .and_then(|mut queue| {
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        })
}

unsafe fn register_tray_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        lpfnWndProc: Some(tray_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput_tray_window"),
        hCursor: cursor,
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_tray_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!("ainput {}", env!("CARGO_PKG_VERSION")));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_tray_window"),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPED.0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create tray window failed: {error}"))
}

unsafe fn add_tray_icon(hwnd: HWND) {
    if unsafe { add_tray_icon_once(hwnd) } {
        info!("ainput tray icon added");
        return;
    }
    warn!("failed to add ainput tray icon; deleting stale icon record and retrying");
    unsafe { delete_tray_icon(hwnd) };
    if unsafe { add_tray_icon_once(hwnd) } {
        info!("ainput tray icon added after stale icon cleanup");
    } else {
        warn!("failed to add ainput tray icon after stale icon cleanup");
    }
}

unsafe fn add_tray_icon_once(hwnd: HWND) -> bool {
    let data = tray_data(hwnd, true);
    unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool()
}

unsafe fn delete_tray_icon(hwnd: HWND) {
    let data = tray_data(hwnd, false);
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

unsafe fn show_api_setup_balloon(hwnd: HWND, message: &str) {
    let mut data = tray_data(hwnd, false);
    data.uFlags = NIF_INFO;
    write_wide_fixed(&mut data.szInfoTitle, "ainput API 配置提示");
    write_wide_fixed(&mut data.szInfo, message);
    data.dwInfoFlags = NIIF_INFO;
    data.Anonymous.uTimeout = 7000;
    let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    if ok.as_bool() {
        info!(message, "API setup tray balloon shown");
    } else {
        warn!(message, "API setup tray balloon failed");
    }
}

// 与 local_asr.rs 的 LocalEngine::parse 别名集保持一致
fn normalize_engine_key(engine: &str) -> String {
    let lowered = engine.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "qwen3-asr" | "qwen3_asr" | "qwen3asr" | "qwen3" => "qwen3-asr".to_string(),
        "funasr-nano" | "funasr_nano" | "fun-asr-nano" | "funasrnano" => "funasr-nano".to_string(),
        "funasr-gguf" | "funasr_gguf" | "fun-asr-gguf" | "funasrgguf" => "funasr-gguf".to_string(),
        "paraformer-streaming" | "paraformer_streaming" | "paraformer" | "paraformer-large" => {
            "paraformer-streaming".to_string()
        }
        "nim-whisper" | "nim_whisper" | "whisper-nim" | "whisper-large-v3" | "whisper_cloud" => {
            "nim-whisper".to_string()
        }
        "sense-voice" | "sense_voice" | "sensevoice" | "" => "sense-voice".to_string(),
        _ => lowered,
    }
}

fn engine_display_name(engine: &str) -> &'static str {
    match normalize_engine_key(engine).as_str() {
        "qwen3-asr" => "Qwen3-ASR",
        "funasr-nano" => "FunASR-Nano",
        "funasr-gguf" => "FunASR-GGUF",
        "paraformer-streaming" => "Paraformer流式",
        "nim-whisper" => "Whisper云端",
        _ => "SenseVoice",
    }
}

fn cached_app_icon() -> HICON {
    let mut guard = CACHED_APP_ICON.lock().unwrap();
    if let Some(icon) = *guard {
        return HICON(icon as *mut std::ffi::c_void);
    }
    let icon = load_tray_icon();
    *guard = Some(icon.0 as usize);
    icon
}

fn tray_data(hwnd: HWND, include_icon: bool) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    data.uCallbackMessage = TRAY_CALLBACK;
    if include_icon {
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.hIcon = cached_app_icon();
        let engine_label = TRAY_STATE.with(|state| {
            state
                .borrow()
                .as_ref()
                .map(|state| engine_display_name(&state.current_engine))
                .unwrap_or("SenseVoice")
        });
        write_wide_fixed(
            &mut data.szTip,
            &format!(
                "ainput v{} · 引擎：{engine_label}",
                env!("CARGO_PKG_VERSION")
            ),
        );
    }
    data
}

fn load_tray_icon() -> HICON {
    if let Some(icon) = load_runtime_icon() {
        return icon;
    }
    if let Some(icon) = load_embedded_icon() {
        return icon;
    }
    unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
}

fn load_embedded_icon() -> Option<HICON> {
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    let instance = HINSTANCE(module.0);
    let handle = unsafe { LoadIconW(Some(instance), PCWSTR(1 as *const u16)) }
        .ok()
        .filter(|icon| !icon.0.is_null())?;
    info!("loaded tray icon from embedded exe resource");
    Some(handle)
}

fn load_runtime_icon() -> Option<HICON> {
    let icon_path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("assets")
        .join("app.ico");
    if !icon_path.exists() {
        return None;
    }
    let icon_path_text = HSTRING::from(icon_path.as_os_str().to_string_lossy().as_ref());
    match unsafe {
        LoadImageW(
            None,
            PCWSTR(icon_path_text.as_ptr()),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE | LR_DEFAULTSIZE,
        )
    } {
        Ok(handle) => {
            info!(path = %icon_path.display(), "loaded custom tray icon");
            Some(HICON(handle.0))
        }
        Err(error) => {
            warn!(path = %icon_path.display(), error = %error, "load custom tray icon failed");
            None
        }
    }
}

fn write_wide_fixed(target: &mut [u16], text: &str) {
    if target.is_empty() {
        return;
    }
    let mut index = 0usize;
    for code in text.encode_utf16().take(target.len().saturating_sub(1)) {
        target[index] = code;
        index += 1;
    }
    target[index] = 0;
}

/// 统一退出路径：标记 shutdown（主循环/各子线程下一轮全部收尾），
/// 销毁托盘窗口（WM_DESTROY 里删托盘图标），退出消息循环。
/// 用户点「退出」、点「重启」、以及第二实例发 TRAY_REMOTE_QUIT_MSG 都走这里。
fn request_app_exit(hwnd: HWND) {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.shutdown.store(true, Ordering::Relaxed);
        }
    });
    unsafe {
        let _ = DestroyWindow(hwnd);
        PostQuitMessage(0);
    }
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let state_snapshot = TRAY_STATE.with(|state| {
        state.borrow().as_ref().map(|state| {
            (
                state.rewrite_language.rewrite_enabled(),
                state.rewrite_prompt.preset(),
                state.rewrite_prompt.preset_label().to_string(),
                state.voice_command.enabled(),
                state.hotkey_user.local_nonstreaming(),
                state.current_engine.clone(),
                state.switching.load(Ordering::Relaxed),
            )
        })
    });
    let Some((
        rewrite_enabled,
        prompt_preset,
        prompt_label,
        voice_command_enabled,
        voice_hotkey_label,
        current_engine,
        switching_engine,
    )) = state_snapshot
    else {
        let _ = unsafe { DestroyMenu(menu) };
        return;
    };
    let engine_label = engine_display_name(&current_engine);

    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!(
                "ainput v{} · 引擎：{engine_label}",
                env!("CARGO_PKG_VERSION")
            ),
        );
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!(
                "{voice_hotkey_label}：本地语音 · {}",
                if rewrite_enabled {
                    "AI改写"
                } else {
                    "原文直出"
                }
            ),
        );
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        let rewrite_flag = if rewrite_enabled {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        append_menu_text(
            menu,
            MF_STRING | rewrite_flag,
            MENU_REWRITE_ENABLED,
            "本地语音 AI 改写",
        );
        append_menu_text(menu, MF_STRING, MENU_API_SETTINGS, "API / 改写设置…");
        append_menu_text(menu, MF_STRING, MENU_HISTORY, "听写历史…");
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!("改写提示词 · 当前：{prompt_label}"),
        );
        append_menu_text(
            menu,
            MF_STRING
                | if prompt_preset == PRESET_STANDARD {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_PROMPT_STANDARD,
            "提示词：标准",
        );
        append_menu_text(
            menu,
            MF_STRING
                | if prompt_preset == PRESET_COMPACT {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_PROMPT_COMPACT,
            "提示词：精简",
        );
        append_menu_text(
            menu,
            MF_STRING
                | if prompt_preset == PRESET_LIGHT {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_PROMPT_LIGHT,
            "提示词：轻润色",
        );
        append_menu_text(
            menu,
            MF_STRING
                | if prompt_preset == PRESET_CUSTOM {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_PROMPT_CUSTOM,
            "提示词：自定义",
        );
        append_menu_text(menu, MF_STRING, MENU_PROMPT_EDIT, "编辑改写提示词…");
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        let voice_flag = if voice_command_enabled {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        append_menu_text(
            menu,
            MF_STRING | voice_flag,
            MENU_VOICE_COMMAND_ENABLED,
            "语音指令（老蔡老蔡）",
        );
        append_menu_text(
            menu,
            MF_STRING,
            MENU_VOICE_COMMAND_EDIT,
            "编辑语音指令提示词…",
        );
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!("语音快捷键 · 当前：{voice_hotkey_label}"),
        );
        append_menu_text(menu, MF_STRING, MENU_HOTKEY_EDIT, "自定义语音快捷键…");
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        let engine_flag = if switching_engine { MF_STRING | MF_GRAYED } else { MF_STRING };
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            if switching_engine { "识别引擎 · 正在切换，请稍等…" } else { "识别引擎 · 点选即热切换" },
        );
        append_menu_text(
            menu,
            engine_flag
                | if normalize_engine_key(&current_engine) == "sense-voice" {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_ENGINE_SENSE_VOICE,
            "SenseVoice（默认·最快）",
        );
        append_menu_text(
            menu,
            engine_flag
                | if normalize_engine_key(&current_engine) == "qwen3-asr" {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_ENGINE_QWEN3,
            "Qwen3-ASR 0.6B（更准·较慢）",
        );
        append_menu_text(
            menu,
            engine_flag
                | if normalize_engine_key(&current_engine) == "funasr-gguf" {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_ENGINE_FUNASR_GGUF,
            "FunASR-GGUF（更准·本地）",
        );
        append_menu_text(
            menu,
            engine_flag
                | if normalize_engine_key(&current_engine) == "paraformer-streaming" {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            MENU_ENGINE_PARAFORMER_STREAMING,
            "Paraformer流式（边说边出）",
        );
        // 2026-09-10 云端档收起：hosted 语音已下架，入口隐藏（Docker 回归时恢复）。
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        let auto_start = is_auto_start_enabled();
        let flag = if auto_start { MF_CHECKED } else { MF_UNCHECKED };
        append_menu_text(menu, MF_STRING | flag, MENU_AUTO_START, "开机自启动");
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        append_menu_text(menu, MF_STRING, MENU_RESTART, "重启");
        append_menu_text(menu, MF_STRING, MENU_EXIT, "退出");
    }

    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        let _ = unsafe { SetForegroundWindow(hwnd) };
        let command = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                Some(0),
                hwnd,
                None,
            )
        };
        match command.0 as usize {
            MENU_API_SETTINGS => open_api_settings(),
            MENU_HISTORY => open_history_panel(),
            MENU_REWRITE_ENABLED => set_rewrite_enabled(!rewrite_enabled),
            MENU_PROMPT_STANDARD => set_rewrite_prompt_preset(PRESET_STANDARD),
            MENU_PROMPT_COMPACT => set_rewrite_prompt_preset(PRESET_COMPACT),
            MENU_PROMPT_LIGHT => set_rewrite_prompt_preset(PRESET_LIGHT),
            MENU_PROMPT_CUSTOM => set_rewrite_prompt_preset(PRESET_CUSTOM),
            MENU_PROMPT_EDIT => open_rewrite_prompt_panel(),
            MENU_VOICE_COMMAND_ENABLED => set_voice_command_enabled(!voice_command_enabled),
            MENU_VOICE_COMMAND_EDIT => open_voice_command_panel(),
            MENU_HOTKEY_EDIT => open_hotkey_panel(),
            MENU_ENGINE_SENSE_VOICE => set_local_engine("sense-voice", "models/sense-voice"),
            MENU_ENGINE_QWEN3 => set_local_engine("qwen3-asr", "models/qwen3-asr"),
            MENU_ENGINE_FUNASR_GGUF => set_funasr_gguf_backend(),
            MENU_ENGINE_PARAFORMER_STREAMING => set_paraformer_backend(),
            MENU_AUTO_START => toggle_auto_start(),
            MENU_RESTART => {
                // 重启 = 以当前 exe 再拉一个新实例：新实例发现互斥锁被占，
                // 会给本进程托盘窗口发 TRAY_REMOTE_QUIT_MSG，等老实例收完尾
                // 释放锁后自动接任。这里只需触发并退出即可。
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).spawn();
                }
                request_app_exit(hwnd);
            }
            MENU_EXIT => {
                request_app_exit(hwnd);
            }
            _ => {}
        }
    }
    let _ = unsafe { DestroyMenu(menu) };
}

unsafe fn append_menu_text(
    menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    flags: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
    id: usize,
    label: &str,
) {
    let label = HSTRING::from(label);
    let _ = unsafe { AppendMenuW(menu, flags, id, PCWSTR(label.as_ptr())) };
}

fn open_api_settings() {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.api_settings.open();
            info!(
                path = %state.api_config_path.display(),
                "API settings panel opened from tray"
            );
        }
    });
}

fn open_history_panel() {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.history_panel.open();
            info!("history panel opened from tray");
        }
    });
}

fn set_rewrite_enabled(enabled: bool) {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.rewrite_language.set_rewrite_enabled(enabled);
            let label = if enabled { "AI改写" } else { "原文直出" };
            state
                .hud
                .show_text(&format!("本地语音：{label}"), false, false);
            info!(rewrite_enabled = enabled, "rewrite toggle from tray");
        }
    });
}

fn set_rewrite_prompt_preset(preset: u8) {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            if preset == PRESET_CUSTOM && state.rewrite_prompt.custom_prompt().trim().is_empty() {
                state.rewrite_prompt_panel.open();
                state
                    .hud
                    .show_text("请先填写自定义提示词", false, false);
                return;
            }
            state.rewrite_prompt.set_preset(preset);
            let label = state.rewrite_prompt.preset_label();
            state
                .hud
                .show_text(&format!("改写提示词：{label}"), false, false);
            info!(preset, label, "rewrite prompt preset from tray");
        }
    });
}

fn open_rewrite_prompt_panel() {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.rewrite_prompt_panel.open();
            info!("rewrite prompt panel opened from tray");
        }
    });
}

fn set_voice_command_enabled(enabled: bool) {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.voice_command.set_enabled(enabled);
            let label = if enabled { "已开启" } else { "已关闭" };
            state
                .hud
                .show_text(&format!("语音指令（老蔡老蔡）：{label}"), false, false);
            info!(enabled, "voice command toggle from tray");
        }
    });
}

fn open_voice_command_panel() {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.voice_command_panel.open();
            info!("voice command panel opened from tray");
        }
    });
}

fn open_hotkey_panel() {
    TRAY_STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.hotkey_panel.open();
            info!("hotkey panel opened from tray");
        }
    });
}

fn set_local_engine(engine: &str, model_dir: &str) {
    TRAY_STATE.with(|state| {
        let (slot, install_root, switching_flag, hud, new_config) = {
            let mut state_cell = state.borrow_mut();
            let Some(state) = state_cell.as_mut() else {
                return;
            };
            if state.switching.load(Ordering::Relaxed) {
                state.hud.show_text("正在切换引擎，请稍等完成", false, false);
                return;
            }
            if state.current_engine == engine {
                state.hud.show_text(&format!("识别引擎已是：{engine}"), true, false);
                return;
            }
            match update_local_engine_config(&state.config_path, engine, model_dir) {
                Ok(()) => {
                    state.current_engine = engine.to_string();
                    if let Ok(mut live) = state.shared_engine.lock() {
                        *live = engine.to_string();
                    }
                    state.switching.store(true, Ordering::Relaxed);
                    if let Some(&addr) = TRAY_HWND.get() {
                    let hwnd_v = HWND(addr as *mut std::ffi::c_void);
                    let _ = unsafe { PostMessageW(Some(hwnd_v), MSG_SWITCH_ANIM_START, WPARAM(0), LPARAM(0)) };
                }
                    state.hud.show_text(
                        &format!("识别引擎切换中：{engine}\n约 5-10 秒后自动生效，不用重启"),
                        false,
                        false,
                    );
                    info!(engine, config_path = %state.config_path.display(), "local ASR engine switch requested from tray");
                    let mut cfg = state.local_config.clone();
                    cfg.engine = engine.to_string();
                    cfg.model_dir = model_dir.to_string();
                    (
                        Arc::clone(&state.shared_recognizer),
                        state.install_root.clone(),
                        Arc::clone(&state.switching),
                        state.hud.clone(),
                        cfg,
                    )
                }
                Err(error) => {
                    state.hud.show_text(&format!("切换失败：{error}"), false, false);
                    warn!(error = %error, engine, "failed to persist local ASR engine switch");
                    return;
                }
            }
        };
        let engine_owned = engine.to_string();
        thread::spawn(move || {
            let result =
                crate::local_asr::LocalSenseVoiceRecognizer::create(&new_config, &install_root);
            let toast = match result {
                Ok(recognizer) => match slot.lock() {
                    Ok(mut guard) => {
                        *guard = Some(recognizer);
                        format!("识别引擎已切换：{engine_owned}\n立即生效，不用重启")
                    }
                    Err(_) => "切换失败：内存槽位异常".to_string(),
                },
                Err(error) => format!("切换失败：{error}"),
            };
            switching_flag.store(false, Ordering::Relaxed);
            if let Some(&addr) = TRAY_HWND.get() {
                let hwnd_v = HWND(addr as *mut std::ffi::c_void);
                let _ = unsafe { PostMessageW(Some(hwnd_v), MSG_SWITCH_ANIM_STOP, WPARAM(0), LPARAM(0)) };
            }
            hud.show_text(&toast, true, false);
            info!(engine = %engine_owned, "local ASR engine hot-swap finished");
        });
    });
}
fn set_paraformer_backend() {
    const ENGINE: &str = "paraformer-streaming";
    const MODEL_DIR: &str = "models/paraformer-streaming";
    TRAY_STATE.with(|state| {
        let (slot, install_root, switching_flag, hud, new_config) = {
            let mut state_cell = state.borrow_mut();
            let Some(state) = state_cell.as_mut() else {
                return;
            };
            if state.switching.load(Ordering::Relaxed) {
                state.hud.show_text("正在切换引擎，请稍等完成", false, false);
                return;
            }
            if normalize_engine_key(&state.current_engine) == ENGINE {
                state.hud.show_text("识别引擎已是：paraformer-streaming", true, false);
                return;
            }
            match update_local_engine_config(&state.config_path, ENGINE, MODEL_DIR) {
                Ok(()) => {
                    state.current_engine = ENGINE.to_string();
                    if let Ok(mut live) = state.shared_engine.lock() {
                        *live = ENGINE.to_string();
                    }
                    state.switching.store(true, Ordering::Relaxed);
                    if let Some(&addr) = TRAY_HWND.get() {
                        let hwnd_v = HWND(addr as *mut std::ffi::c_void);
                        let _ = unsafe {
                            PostMessageW(Some(hwnd_v), MSG_SWITCH_ANIM_START, WPARAM(0), LPARAM(0))
                        };
                    }
                    state.hud.show_text(
                        "识别引擎切换中：paraformer-streaming\n约 5-10 秒后自动生效，不用重启",
                        false,
                        false,
                    );
                    info!(engine = ENGINE, config_path = %state.config_path.display(), "paraformer streaming switch requested from tray");
                    let mut cfg = state.paraformer_config.clone();
                    cfg.model_dir = MODEL_DIR.to_string();
                    (
                        Arc::clone(&state.shared_paraformer),
                        state.install_root.clone(),
                        Arc::clone(&state.switching),
                        state.hud.clone(),
                        cfg,
                    )
                }
                Err(error) => {
                    state.hud.show_text(&format!("切换失败：{error}"), false, false);
                    warn!(error = %error, engine = ENGINE, "failed to persist paraformer switch");
                    return;
                }
            }
        };
        thread::spawn(move || {
            let result = crate::paraformer_streaming::ParaformerStreamingRecognizer::create(
                &new_config,
                &install_root,
            );
            let toast = match result {
                Ok(recognizer) => match slot.lock() {
                    Ok(mut guard) => {
                        let provider = recognizer.provider_used().to_string();
                        *guard = Some(recognizer);
                        format!("识别引擎已切换：Paraformer流式（{provider}）\n立即生效，不用重启")
                    }
                    Err(_) => "切换失败：内存槽位异常".to_string(),
                },
                Err(error) => format!("切换失败：{error}"),
            };
            switching_flag.store(false, Ordering::Relaxed);
            if let Some(&addr) = TRAY_HWND.get() {
                let hwnd_v = HWND(addr as *mut std::ffi::c_void);
                let _ = unsafe {
                    PostMessageW(Some(hwnd_v), MSG_SWITCH_ANIM_STOP, WPARAM(0), LPARAM(0))
                };
            }
            hud.show_text(&toast, true, false);
            info!("paraformer streaming hot-swap finished");
        });
    });
}

fn set_funasr_gguf_backend() {
    const ENGINE: &str = "funasr-gguf";
    const MODEL_DIR: &str = "models/funasr-gguf";
    TRAY_STATE.with(|state| {
        let mut state_cell = state.borrow_mut();
        let Some(state) = state_cell.as_mut() else {
            return;
        };
        if state.switching.load(Ordering::Relaxed) {
            state.hud.show_text("正在切换引擎，请稍等完成", false, false);
            return;
        }
        if normalize_engine_key(&state.current_engine) == ENGINE {
            state.hud.show_text("识别引擎已是：funasr-gguf", true, false);
            return;
        }
        match update_local_engine_config(&state.config_path, ENGINE, MODEL_DIR) {
            Ok(()) => {
                state.current_engine = ENGINE.to_string();
                if let Ok(mut live) = state.shared_engine.lock() {
                    *live = ENGINE.to_string();
                }
                state.hud.show_text(
                    "识别引擎已切换：FunASR-GGUF\n先跑 scripts/start_gguf_sidecar.ps1 起边车\n边车没起会如实报错，不用重启",
                    true,
                    false,
                );
                info!(engine = ENGINE, config_path = %state.config_path.display(), "funasr-gguf switch from tray");
            }
            Err(error) => {
                state.hud.show_text(&format!("切换失败：{error}"), false, false);
                warn!(error = %error, engine = ENGINE, "failed to persist gguf switch");
            }
        }
    });
}

#[allow(dead_code)]
fn set_nim_whisper_backend() {
    const ENGINE: &str = "nim-whisper";
    const MODEL_DIR: &str = "models/nim-whisper";
    TRAY_STATE.with(|state| {
        let mut state_cell = state.borrow_mut();
        let Some(state) = state_cell.as_mut() else {
            return;
        };
        if state.switching.load(Ordering::Relaxed) {
            state.hud.show_text("正在切换引擎，请稍等完成", false, false);
            return;
        }
        if normalize_engine_key(&state.current_engine) == ENGINE {
            state.hud.show_text("识别引擎已是：nim-whisper", true, false);
            return;
        }
        match update_local_engine_config(&state.config_path, ENGINE, MODEL_DIR) {
            Ok(()) => {
                state.current_engine = ENGINE.to_string();
                if let Ok(mut live) = state.shared_engine.lock() {
                    *live = ENGINE.to_string();
                }
                let endpoint = state.nim_config.endpoint_url.clone();
                state.hud.show_text(
                    &format!("识别引擎已切换：Whisper云端\n{endpoint}\n连不上会如实报错（需本地 NIM 容器或云授权），不用重启"),
                    true,
                    false,
                );
                info!(engine = ENGINE, config_path = %state.config_path.display(), "nim-whisper switch from tray");
            }
            Err(error) => {
                state.hud.show_text(&format!("切换失败：{error}"), false, false);
                warn!(error = %error, engine = ENGINE, "failed to persist nim switch");
            }
        }
    });
}

fn update_local_engine_config(config_path: &std::path::Path, engine: &str, model_dir: &str) -> Result<()> {
    use std::fs;

    if !config_path.exists() {
        anyhow::bail!("config file not found: {}", config_path.display());
    }
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let section = "[local_nonstreaming]";
    let updates = [("engine", engine), ("model_dir", model_dir)];
    let mut in_section = false;
    let mut section_seen = false;
    let mut replaced = [false; 2];
    let mut output: Vec<String> = Vec::new();
    let mut append_missing = |output: &mut Vec<String>, replaced: &[bool; 2]| {
        for (index, (key, value)) in updates.iter().enumerate() {
            if !replaced[index] {
                output.push(format!("{key} = \"{value}\""));
            }
        }
    };
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section {
                append_missing(&mut output, &replaced);
                replaced = [true; 2];
            }
            if trimmed.eq_ignore_ascii_case(section) {
                section_seen = true;
            }
            in_section = trimmed.eq_ignore_ascii_case(section);
            output.push(line.to_string());
            continue;
        }
        let mut emitted = false;
        if in_section {
            for (index, (key, value)) in updates.iter().enumerate() {
                let is_key = trimmed.split('=').next().map(str::trim) == Some(*key);
                if is_key && !replaced[index] {
                    output.push(format!("{key} = \"{value}\""));
                    replaced[index] = true;
                    emitted = true;
                    break;
                }
            }
        }
        if !emitted {
            output.push(line.to_string());
        }
    }
    if in_section {
        append_missing(&mut output, &replaced);
    }
    if !section_seen {
        output.push(section.to_string());
        for (key, value) in &updates {
            output.push(format!("{key} = \"{value}\""));
        }
    }
    let write_target = config_path.with_extension("toml.tmp-write");
    fs::write(&write_target, format!("{}\n", output.join("\n")))
        .with_context(|| format!("write config {}", write_target.display()))?;
    fs::rename(&write_target, config_path)
        .with_context(|| format!("replace config {}", config_path.display()))
}

// reg.exe 是控制台程序，不加 CREATE_NO_WINDOW 每次开托盘菜单都会闪黑框
// （2026-09-03 用户又一次现场抓到：点托盘图标就闪终端窗）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn is_auto_start_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg")
        .args([
            "query",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "/v",
            "ainput",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn toggle_auto_start() {
    use std::os::windows::process::CommandExt;
    let enabled = is_auto_start_enabled();
    if enabled {
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "ainput",
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        info!("auto-start disabled");
    } else {
        let exe_path = match std::env::current_exe() {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(error) => {
                warn!(error = %error, "cannot get current exe path for auto-start");
                return;
            }
        };
        let _ = std::process::Command::new("reg")
            .args([
                "add",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "ainput",
                "/t",
                "REG_SZ",
                "/d",
                &exe_path,
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        info!(path = %exe_path, "auto-start enabled");
    }
}

extern "system" fn tray_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED_MESSAGE.load(Ordering::Relaxed);
    if taskbar_created != 0 && msg == taskbar_created {
        info!("TaskbarCreated received; re-adding ainput tray icon");
        unsafe { add_tray_icon(hwnd) };
        return LRESULT(0);
    }

    match msg {
        TRAY_CALLBACK => {
            let mouse_msg = lparam.0 as u32;
            if mouse_msg == WM_LBUTTONUP || mouse_msg == WM_RBUTTONUP {
                unsafe { show_tray_menu(hwnd) };
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_CREATE => {
            // 不常驻定时器：仅在引擎切换期间 PostMessage 才会拉起来
            LRESULT(0)
        }
        MSG_SWITCH_ANIM_START => {
            let _ = unsafe {
                SetTimer(Some(hwnd), SWITCH_ANIM_TIMER, SWITCH_ANIM_INTERVAL_MS, None)
            };
            LRESULT(0)
        }
        MSG_SWITCH_ANIM_STOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SWITCH_ANIM_TIMER) };
            if SWITCH_ICON_ACTIVE.swap(false, Ordering::Relaxed) {
                let data = tray_data(hwnd, true);
                let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
            }
            SWITCH_BLINK_STATE.store(false, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == SWITCH_ANIM_TIMER {
                let switching = TRAY_STATE
                    .with(|state| state.borrow().as_ref().map(|s| s.switching.load(Ordering::Relaxed)).unwrap_or(false));
                if !switching {
                    // 闲时仅在「上一拍刚结束切换」时恢复原图标一次，平时不碰托盘
                    if SWITCH_ICON_ACTIVE.swap(false, Ordering::Relaxed) {
                        let data = tray_data(hwnd, true);
                        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                    }
                    return LRESULT(0);
                }
                SWITCH_ICON_ACTIVE.store(true, Ordering::Relaxed);
                let blink = SWITCH_BLINK_STATE.load(Ordering::Relaxed);
                let mut data = tray_data(hwnd, true);
                // 切换中：应用图标与系统信息图标交替闪烁
                if blink {
                    unsafe {
                        data.hIcon = LoadIconW(None, IDI_INFORMATION).unwrap_or(data.hIcon);
                    }
                }
                data.uFlags = NIF_ICON | NIF_TIP;
                let is_switching_tip = if switching { "｜切换中" } else { "" };
                let engine_label = TRAY_STATE.with(|state| {
                    state.borrow().as_ref().map(|s| s.current_engine.clone()).unwrap_or_default()
                });
                write_wide_fixed(
                    &mut data.szTip,
                    &format!("ainput v{} · 引擎：{}{}", env!("CARGO_PKG_VERSION"), engine_display_name(&engine_label), is_switching_tip),
                );
                let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                SWITCH_BLINK_STATE.store(!blink, Ordering::Relaxed);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = unsafe { KillTimer(Some(hwnd), SWITCH_ANIM_TIMER) };
            unsafe { delete_tray_icon(hwnd) };
            LRESULT(0)
        }
        m if m == TRAY_REMOTE_QUIT_MSG => {
            // 外部进程（第二实例 / 部署脚本）请求退出：走与用户点「退出」
            // 完全相同的路径，托盘图标和状态都会正常收尾。
            request_app_exit(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::update_local_engine_config;

    #[test]
    fn update_engine_config_present_key_succeeds() {
        let dir = std::env::temp_dir().join("ainput-tray-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ainput.toml");
        std::fs::write(
            &path,
            "[mode]\ndefault = \"local_nonstreaming\"\n\n[local_nonstreaming]\nengine = \"sense-voice\"\nmodel_dir = \"models/sense-voice\"\nnum_threads = 4\n\n[rewrite]\nenabled = false\n",
        )
        .unwrap();

        update_local_engine_config(&path, "qwen3-asr", "models/qwen3-asr").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("engine = \"qwen3-asr\""));
        assert!(raw.contains("model_dir = \"models/qwen3-asr\""));
        assert!(raw.contains("num_threads = 4"));
        assert!(raw.contains("[rewrite]"));
        assert!(raw.contains("enabled = false"));

        let result = update_local_engine_config(&path, "funasr-nano", "models/funasr-nano");
        assert!(result.is_ok());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("engine = \"funasr-nano\""));
    }

    #[test]
    fn missing_keys_are_filled_in() {
        // 2026-09-02：update_local_engine_config 容错语义改为「缺键补齐」
        // （此前要求 is_err 的断言已过时）。
        let dir = std::env::temp_dir().join("ainput-tray-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("missing.toml");
        std::fs::write(&path, "[local_nonstreaming]\nnum_threads = 4\n").unwrap();
        update_local_engine_config(&path, "qwen3-asr", "models/qwen3-asr").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("engine = \"qwen3-asr\""));
        assert!(raw.contains("model_dir = \"models/qwen3-asr\""));
        assert!(raw.contains("num_threads = 4"));
    }

    #[test]
    fn missing_config_file_is_err() {
        let dir = std::env::temp_dir().join("ainput-tray-test");
        let path = dir.join("no-such-file.toml");
        assert!(update_local_engine_config(&path, "qwen3-asr", "models/qwen3-asr").is_err());
    }
}
