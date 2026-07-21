use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tracing::{info, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CREATESTRUCTW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HICON, IDC_ARROW, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LoadCursorW, LoadIconW, LoadImageW, MF_CHECKED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, PostQuitMessage, PostThreadMessageW,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CREATE, WM_DESTROY,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::api_settings_panel::ApiSettingsPanelController;
use crate::history_panel::HistoryPanelController;
use crate::hud::HudController;
use crate::rewrite_language::RewriteLanguageController;

const TRAY_THREAD_QUIT: u32 = WM_APP + 41;
const TRAY_CALLBACK: u32 = WM_APP + 42;
const TRAY_API_NOTIFICATION: u32 = WM_APP + 44;
const TRAY_UID: u32 = 1;
static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
static API_NOTIFICATION_QUEUE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

const MENU_API_SETTINGS: usize = 2010;
const MENU_HISTORY: usize = 2012;
const MENU_AUTO_START: usize = 2011;
const MENU_EXIT: usize = 2005;
const MENU_REWRITE_ENABLED: usize = 2700;

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
        api_config_path: PathBuf,
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
                    api_config_path,
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
    api_config_path: PathBuf,
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

fn tray_data(hwnd: HWND, include_icon: bool) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    data.uCallbackMessage = TRAY_CALLBACK;
    if include_icon {
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.hIcon = load_tray_icon();
        write_wide_fixed(
            &mut data.szTip,
            &format!("ainput {}", env!("CARGO_PKG_VERSION")),
        );
    }
    data
}

fn load_tray_icon() -> HICON {
    if let Some(icon) = load_runtime_icon() {
        return icon;
    }
    unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
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

unsafe fn show_tray_menu(hwnd: HWND) {
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let state_snapshot = TRAY_STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.rewrite_language.rewrite_enabled())
    });
    let Some(rewrite_enabled) = state_snapshot else {
        let _ = unsafe { DestroyMenu(menu) };
        return;
    };

    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!("ainput {}", env!("CARGO_PKG_VERSION")),
        );
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
        append_menu_text(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            &format!(
                "CapsLock：本地语音 · {}",
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
        let auto_start = is_auto_start_enabled();
        let flag = if auto_start { MF_CHECKED } else { MF_UNCHECKED };
        append_menu_text(menu, MF_STRING | flag, MENU_AUTO_START, "开机自启动");
    }
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
    unsafe {
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
            MENU_AUTO_START => toggle_auto_start(),
            MENU_EXIT => {
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

fn is_auto_start_enabled() -> bool {
    std::process::Command::new("reg")
        .args([
            "query",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "/v",
            "ainput",
        ])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn toggle_auto_start() {
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
            let _ = lparam.0 as *const CREATESTRUCTW;
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { delete_tray_icon(hwnd) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
