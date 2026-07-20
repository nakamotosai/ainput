use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use arboard::Clipboard;
use tracing::{info, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, GetStockObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_READONLY, GetClientRect, GetMessageW, IDC_ARROW, LoadCursorW, MoveWindow,
    PostThreadMessageW, RegisterClassW, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT, WM_SIZE,
    WNDCLASSW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::history;

const HISTORY_THREAD_QUIT: u32 = WM_APP + 71;
const HISTORY_OPEN: u32 = WM_APP + 72;
const PANEL_WIDTH: i32 = 900;
const PANEL_HEIGHT: i32 = 620;
const BUTTON_HEIGHT: i32 = 30;
const MARGIN: i32 = 12;

#[derive(Clone)]
pub struct HistoryPanelController {
    thread_id: u32,
}

impl HistoryPanelController {
    pub fn start(path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        thread::spawn(move || {
            HISTORY_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            HISTORY_STATE.with(|state| {
                *state.borrow_mut() = Some(HistoryPanelState::new(path));
            });
            let result = unsafe { run_history_panel_thread(shutdown) };
            if let Err(error) = result {
                warn!(error = %error, "history panel thread failed");
            }
        });
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("history panel thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { thread_id })
    }

    pub fn open(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, HISTORY_OPEN, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for HistoryPanelController {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, HISTORY_THREAD_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

struct HistoryPanelState {
    path: PathBuf,
    hwnd: HWND,
    display_hwnd: HWND,
    status_hwnd: HWND,
    refresh_button: HWND,
    copy_button: HWND,
    clear_button: HWND,
    open_logs_button: HWND,
    last_render: String,
}

impl HistoryPanelState {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            hwnd: HWND::default(),
            display_hwnd: HWND::default(),
            status_hwnd: HWND::default(),
            refresh_button: HWND::default(),
            copy_button: HWND::default(),
            clear_button: HWND::default(),
            open_logs_button: HWND::default(),
            last_render: String::new(),
        }
    }
}

thread_local! {
    static HISTORY_READY: RefCell<Option<mpsc::Sender<Result<u32, String>>>> =
        const { RefCell::new(None) };
    static HISTORY_STATE: RefCell<Option<HistoryPanelState>> = const { RefCell::new(None) };
}

unsafe fn run_history_panel_thread(shutdown: Arc<AtomicBool>) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow!("get module handle failed: {error}"))?;
    unsafe { register_history_class(HINSTANCE(instance.0))? };
    let hwnd = unsafe { create_history_window(HINSTANCE(instance.0))? };
    HISTORY_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.hwnd = hwnd;
        }
    });
    let thread_id = unsafe { GetCurrentThreadId() };
    HISTORY_READY.with(|ready| {
        if let Some(sender) = ready.borrow_mut().take() {
            let _ = sender.send(Ok(thread_id));
        }
    });
    info!(thread_id, "history panel thread started");

    while !shutdown.load(Ordering::Relaxed) {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if has_message.0 == -1 {
            return Err(anyhow!("history panel GetMessage failed"));
        }
        if has_message.0 == 0 || msg.message == HISTORY_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == HISTORY_OPEN {
            show_and_refresh();
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

unsafe fn register_history_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        lpfnWndProc: Some(history_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput2_history_panel"),
        hCursor: cursor,
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_history_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!("ainput2 历史 / 对比 {}", env!("CARGO_PKG_VERSION")));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput2_history_panel"),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            140,
            120,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create history panel window failed: {error}"))
}

unsafe extern "system" fn history_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            if let Err(error) = unsafe { create_controls(hwnd) } {
                warn!(error = %error, "create history panel controls failed");
                return LRESULT(-1);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            layout_controls(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(HWND(lparam.0 as *mut core::ffi::c_void));
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn create_controls(hwnd: HWND) -> Result<()> {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    let refresh_button = unsafe { create_button(hwnd, "刷新", 12, 12, 76, BUTTON_HEIGHT)? };
    let copy_button = unsafe { create_button(hwnd, "复制全部", 98, 12, 96, BUTTON_HEIGHT)? };
    let clear_button = unsafe { create_button(hwnd, "清空", 204, 12, 76, BUTTON_HEIGHT)? };
    let open_logs_button =
        unsafe { create_button(hwnd, "打开日志", 290, 12, 96, BUTTON_HEIGHT)? };
    let status_hwnd = unsafe {
        create_control(
            hwnd,
            "STATIC",
            "历史记录异步写入，不阻塞上屏。",
            400,
            17,
            460,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    let display_hwnd = unsafe {
        create_control(
            hwnd,
            "EDIT",
            "",
            MARGIN,
            54,
            PANEL_WIDTH - MARGIN * 3,
            PANEL_HEIGHT - 90,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_VSCROLL.0
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_READONLY as u32,
            ),
        )?
    };
    for control in [
        refresh_button,
        copy_button,
        clear_button,
        open_logs_button,
        status_hwnd,
        display_hwnd,
    ] {
        unsafe {
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            )
        };
    }
    HISTORY_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.hwnd = hwnd;
            state.display_hwnd = display_hwnd;
            state.status_hwnd = status_hwnd;
            state.refresh_button = refresh_button;
            state.copy_button = copy_button;
            state.clear_button = clear_button;
            state.open_logs_button = open_logs_button;
        }
    });
    let _ = dpi;
    layout_controls(hwnd);
    refresh_display();
    Ok(())
}

unsafe fn create_button(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    unsafe { create_control(parent, "BUTTON", text, x, y, w, h, WS_CHILD | WS_VISIBLE) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_control(
    parent: HWND,
    class_name: &str,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
) -> Result<HWND> {
    let class_name = HSTRING::from(class_name);
    let text = HSTRING::from(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(text.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            Some(parent),
            None,
            None,
            None,
        )
    }
    .map_err(|error| anyhow!("create history panel control failed: {error}"))
}

fn show_and_refresh() {
    HISTORY_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = ShowWindow(state.hwnd, SW_RESTORE);
                let _ = ShowWindow(state.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(state.hwnd);
            }
        }
    });
    refresh_display();
}

fn layout_controls(hwnd: HWND) {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    HISTORY_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = MoveWindow(
                    state.display_hwnd,
                    MARGIN,
                    54,
                    width - MARGIN * 2,
                    height - 66,
                    true,
                );
                let _ = MoveWindow(state.status_hwnd, 400, 17, (width - 412).max(120), 24, true);
            }
        }
    });
}

fn handle_command(control: HWND) {
    let action = HISTORY_STATE.with(|stored| {
        stored.borrow().as_ref().and_then(|state| {
            if control == state.refresh_button {
                Some("refresh")
            } else if control == state.copy_button {
                Some("copy")
            } else if control == state.clear_button {
                Some("clear")
            } else if control == state.open_logs_button {
                Some("open")
            } else {
                None
            }
        })
    });
    match action {
        Some("refresh") => refresh_display(),
        Some("copy") => copy_rendered(),
        Some("clear") => clear_history(),
        Some("open") => open_logs_dir(),
        _ => {}
    }
}

fn refresh_display() {
    let (display_hwnd, status_hwnd, path) = HISTORY_STATE
        .with(|stored| {
            stored
                .borrow()
                .as_ref()
                .map(|state| (state.display_hwnd, state.status_hwnd, state.path.clone()))
        })
        .unwrap_or_default();
    if display_hwnd.0.is_null() {
        return;
    }
    match history::load_recent(&path, 500) {
        Ok(records) => {
            let rendered = history::render_history(&records);
            set_window_text(display_hwnd, &rendered);
            set_window_text(
                status_hwnd,
                &format!("{} 条记录 | {}", records.len(), path.display()),
            );
            HISTORY_STATE.with(|stored| {
                if let Some(state) = stored.borrow_mut().as_mut() {
                    state.last_render = rendered;
                }
            });
        }
        Err(error) => {
            set_window_text(status_hwnd, &format!("读取失败: {error}"));
        }
    }
}

fn copy_rendered() {
    let text = HISTORY_STATE
        .with(|stored| {
            stored
                .borrow()
                .as_ref()
                .map(|state| state.last_render.clone())
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return;
    }
    match Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
        Ok(()) => set_status("已复制当前历史视图。"),
        Err(error) => set_status(&format!("复制失败: {error}")),
    }
}

fn clear_history() {
    let path = HISTORY_STATE
        .with(|stored| stored.borrow().as_ref().map(|state| state.path.clone()))
        .unwrap_or_default();
    match history::clear(&path) {
        Ok(()) => {
            set_status("历史已清空。");
            refresh_display();
        }
        Err(error) => set_status(&format!("清空失败: {error}")),
    }
}

fn open_logs_dir() {
    let path = HISTORY_STATE
        .with(|stored| stored.borrow().as_ref().map(|state| state.path.clone()))
        .unwrap_or_default();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = Command::new("explorer.exe").arg(parent).spawn() {
        set_status(&format!("打开日志目录失败: {error}"));
    }
}

fn set_status(text: &str) {
    HISTORY_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            set_window_text(state.status_hwnd, text);
        }
    });
}

fn set_window_text(hwnd: HWND, text: &str) {
    let text = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}
