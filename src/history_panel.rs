//! Native dark panel to browse local dictation history (raw vs rewrite).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tracing::{info, warn};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FillRect, HBRUSH, HFONT, HGDIOBJ, OUT_OUTLINE_PRECIS,
    SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BN_CLICKED, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GetClientRect,
    GetMessageW, HMENU, IDC_ARROW, LoadCursorW, MSG, PostThreadMessageW, RegisterClassW, SW_HIDE,
    SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT, WNDCLASSW, WS_BORDER,
    WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::history;

const PANEL_THREAD_QUIT: u32 = WM_APP + 141;
const PANEL_OPEN: u32 = WM_APP + 142;

const CLIENT_W: i32 = 780;
const CLIENT_H: i32 = 720;
const MARGIN: i32 = 28;
const FONT_PX: i32 = 18;
const TITLE_FONT_PX: i32 = 24;
const PANEL_FONT_FAMILY: &str = "Microsoft YaHei UI";

const ID_TITLE: i32 = 5001;
const ID_SUMMARY: i32 = 5002;
const ID_BODY: i32 = 5003;
const ID_REFRESH: i32 = 5004;
const ID_OPEN_FOLDER: i32 = 5005;
const ID_CLOSE: i32 = 5006;

const BG: COLORREF = COLORREF(0x00_16_14_14);
const INPUT_BG: COLORREF = COLORREF(0x00_2A_24_24);
const TEXT: COLORREF = COLORREF(0x00_F2_F0_F0);
const BUTTON_BG: COLORREF = COLORREF(0x00_32_2A_2A);

#[derive(Clone)]
pub struct HistoryPanelController {
    thread_id: u32,
}

impl HistoryPanelController {
    pub fn start(history_path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        thread::spawn(move || {
            PANEL_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            PANEL_STATE.with(|state| {
                *state.borrow_mut() = Some(PanelState::new(history_path));
            });
            if let Err(error) = unsafe { run_panel_thread(shutdown) } {
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
            let _ = PostThreadMessageW(self.thread_id, PANEL_OPEN, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for HistoryPanelController {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, PANEL_THREAD_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

struct PanelState {
    history_path: PathBuf,
    hwnd: HWND,
    summary: HWND,
    body: HWND,
    brush_bg: HBRUSH,
    brush_input: HBRUSH,
    brush_button: HBRUSH,
    font: HFONT,
    title_font: HFONT,
}

impl PanelState {
    fn new(history_path: PathBuf) -> Self {
        Self {
            history_path,
            hwnd: HWND::default(),
            summary: HWND::default(),
            body: HWND::default(),
            brush_bg: HBRUSH::default(),
            brush_input: HBRUSH::default(),
            brush_button: HBRUSH::default(),
            font: HFONT::default(),
            title_font: HFONT::default(),
        }
    }
}

thread_local! {
    static PANEL_READY: RefCell<Option<mpsc::Sender<Result<u32, String>>>> =
        const { RefCell::new(None) };
    static PANEL_STATE: RefCell<Option<PanelState>> = const { RefCell::new(None) };
}

unsafe fn create_ui_font(height_px: i32, weight: i32) -> HFONT {
    let family = HSTRING::from(PANEL_FONT_FAMILY);
    unsafe {
        CreateFontW(
            -height_px.abs(),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_OUTLINE_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(family.as_ptr()),
        )
    }
}

fn outer_size_for_client(style: WINDOW_STYLE) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: CLIENT_W,
        bottom: CLIENT_H,
    };
    let ok = unsafe { AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0)) };
    if ok.is_err() {
        return (CLIENT_W + 16, CLIENT_H + 40);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

unsafe fn run_panel_thread(shutdown: Arc<AtomicBool>) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow!("get module handle failed: {error}"))?;
    unsafe { register_panel_class(HINSTANCE(instance.0))? };
    let hwnd = unsafe { create_panel_window(HINSTANCE(instance.0))? };
    unsafe { create_panel_controls(hwnd)? };
    PANEL_STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.hwnd = hwnd;
        }
    });
    let thread_id = unsafe { GetCurrentThreadId() };
    PANEL_READY.with(|ready| {
        if let Some(sender) = ready.borrow_mut().take() {
            let _ = sender.send(Ok(thread_id));
        }
    });
    info!(thread_id, "history panel thread started");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        let mut msg = MSG::default();
        let has = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if has.0 == -1 {
            return Err(anyhow!("history panel GetMessage failed"));
        }
        if has.0 == 0 || msg.message == PANEL_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == PANEL_OPEN {
            open_panel_ui();
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn register_panel_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        lpfnWndProc: Some(panel_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput_history_panel_v1"),
        hCursor: cursor,
        hbrBackground: unsafe { CreateSolidBrush(BG) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_panel_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!("ainput · 听写历史 {}", env!("CARGO_PKG_VERSION")));
    let style = WINDOW_STYLE(
        WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_CLIPCHILDREN.0,
    );
    let (outer_w, outer_h) = outer_size_for_client(style);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_history_panel_v1"),
            PCWSTR(title.as_ptr()),
            style,
            100,
            60,
            outer_w,
            outer_h,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create history panel window failed: {error}"))
}

unsafe fn create_panel_controls(hwnd: HWND) -> Result<()> {
    PANEL_STATE.with(|state| {
        let mut borrow = state.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return Err(anyhow!("history panel state missing"));
        };
        state.font = unsafe { create_ui_font(FONT_PX, 400) };
        state.title_font = unsafe { create_ui_font(TITLE_FONT_PX, 600) };
        if state.font.is_invalid() || state.title_font.is_invalid() {
            return Err(anyhow!("create history panel font failed"));
        }
        state.brush_bg = unsafe { CreateSolidBrush(BG) };
        state.brush_input = unsafe { CreateSolidBrush(INPUT_BG) };
        state.brush_button = unsafe { CreateSolidBrush(BUTTON_BG) };

        let instance = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let body_font = state.font;
        let title_font = state.title_font;
        let child = |class: PCWSTR,
                     text: &str,
                     style: WINDOW_STYLE,
                     x: i32,
                     y: i32,
                     w: i32,
                     h: i32,
                     id: i32,
                     use_title: bool|
         -> Result<HWND> {
            let text = HSTRING::from(text);
            let child_hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    PCWSTR(text.as_ptr()),
                    WINDOW_STYLE(style.0 | WS_CHILD.0 | WS_VISIBLE.0),
                    x,
                    y,
                    w,
                    h,
                    Some(hwnd),
                    Some(HMENU(id as isize as *mut _)),
                    Some(HINSTANCE(instance.0)),
                    None,
                )
            }
            .map_err(|error| anyhow!("create history child {id}: {error}"))?;
            let font = if use_title { title_font } else { body_font };
            unsafe {
                SendMessageW(
                    child_hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            Ok(child_hwnd)
        };

        let field_w = CLIENT_W - MARGIN * 2;
        let mut y = 22i32;
        let _title = child(
            w!("STATIC"),
            "听写历史",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            field_w,
            32,
            ID_TITLE,
            true,
        )?;
        y += 40;
        state.summary = child(
            w!("STATIC"),
            "加载中…",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            field_w,
            48,
            ID_SUMMARY,
            false,
        )?;
        y += 56;
        let body_h = CLIENT_H - y - 80;
        state.body = child(
            w!("EDIT"),
            "",
            WINDOW_STYLE(
                WS_BORDER.0
                    | WS_VSCROLL.0
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | WS_TABSTOP.0,
            ),
            MARGIN,
            y,
            field_w,
            body_h,
            ID_BODY,
            false,
        )?;
        y += body_h + 16;
        let _refresh = child(
            w!("BUTTON"),
            "刷新",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            120,
            44,
            ID_REFRESH,
            false,
        )?;
        let _open = child(
            w!("BUTTON"),
            "打开存档目录",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + 140,
            y,
            160,
            44,
            ID_OPEN_FOLDER,
            false,
        )?;
        let _close = child(
            w!("BUTTON"),
            "关闭",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + 320,
            y,
            120,
            44,
            ID_CLOSE,
            false,
        )?;
        Ok(())
    })
}

fn open_panel_ui() {
    let snapshot = PANEL_STATE.with(|cell| {
        let borrow = cell.try_borrow().ok()?;
        let state = borrow.as_ref()?;
        Some((
            state.hwnd,
            state.summary,
            state.body,
            state.history_path.clone(),
        ))
    });
    let Some((hwnd, summary, body, path)) = snapshot else {
        return;
    };
    refresh_content(summary, body, &path);
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    info!(path = %path.display(), "history panel opened");
}

fn refresh_content(summary: HWND, body: HWND, path: &std::path::Path) {
    match history::load_recent(path, 300) {
        Ok(records) => {
            let total = records.len();
            let rewrite_n = records.iter().filter(|r| r.rewrite_enabled).count();
            let with_before_after = records
                .iter()
                .filter(|r| {
                    r.rewrite_enabled
                        && !r.raw_text.trim().is_empty()
                        && (!r.rewrite_text.trim().is_empty() || !r.pasted_text.trim().is_empty())
                })
                .count();
            set_window_text(
                summary,
                &format!(
                    "共 {total} 条 · 开启过改写 {rewrite_n} 条 · 有前后对比 {with_before_after} 条\r\n存档: {}",
                    path.display()
                ),
            );
            set_window_text(body, &history::render_history(&records));
        }
        Err(error) => {
            set_window_text(summary, &format!("读取失败：{error}"));
            set_window_text(body, "");
        }
    }
}

fn set_window_text(hwnd: HWND, text: &str) {
    let text = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

fn open_history_folder() {
    let path = PANEL_STATE.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|b| b.as_ref().map(|s| s.history_path.clone()))
    });
    let Some(path) = path else {
        return;
    };
    let folder = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| path.clone());
    let _ = std::process::Command::new("explorer.exe")
        .arg(folder.as_os_str())
        .spawn();
}

extern "system" fn panel_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
            }
            PANEL_STATE.with(|state| {
                if let Ok(borrow) = state.try_borrow() {
                    if let Some(state) = borrow.as_ref() {
                        unsafe {
                            let _ = FillRect(hdc, &rect, state.brush_bg);
                        }
                    }
                }
            });
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, TEXT);
            }
            let brush = PANEL_STATE.with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|b| b.as_ref().map(|s| s.brush_bg))
                    .unwrap_or_default()
            });
            LRESULT(brush.0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            unsafe {
                SetBkColor(hdc, INPUT_BG);
                SetTextColor(hdc, TEXT);
            }
            let brush = PANEL_STATE.with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|b| b.as_ref().map(|s| s.brush_input))
                    .unwrap_or_default()
            });
            LRESULT(brush.0 as isize)
        }
        WM_CTLCOLORBTN => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, TEXT);
            }
            let brush = PANEL_STATE.with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|b| b.as_ref().map(|s| s.brush_button))
                    .unwrap_or_default()
            });
            LRESULT(brush.0 as isize)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if code == BN_CLICKED {
                match id {
                    ID_REFRESH => {
                        let snapshot = PANEL_STATE.with(|cell| {
                            let borrow = cell.try_borrow().ok()?;
                            let state = borrow.as_ref()?;
                            Some((state.summary, state.body, state.history_path.clone()))
                        });
                        if let Some((summary, body, path)) = snapshot {
                            refresh_content(summary, body, &path);
                        }
                    }
                    ID_OPEN_FOLDER => open_history_folder(),
                    ID_CLOSE => {
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PANEL_STATE.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    if !state.font.is_invalid() {
                        unsafe {
                            let _ = DeleteObject(HGDIOBJ(state.font.0));
                        }
                        state.font = HFONT::default();
                    }
                    if !state.title_font.is_invalid() {
                        unsafe {
                            let _ = DeleteObject(HGDIOBJ(state.title_font.0));
                        }
                        state.title_font = HFONT::default();
                    }
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
