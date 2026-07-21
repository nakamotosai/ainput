//! Native dark panel to browse local dictation history (raw vs rewrite).
//!
//! Body text uses a light opaque EDIT surface. Dark custom-colored multiline
//! EDIT on Win32 commonly stacks glyphs when WM_CTLCOLOREDIT cannot return a
//! solid brush (e.g. RefCell held across CreateWindow). Light body avoids that
//! class of bug while chrome stays dark.

use std::cell::{Cell, RefCell};
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
    ANTIALIASED_QUALITY, BACKGROUND_MODE, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FillRect, HBRUSH, HFONT, HGDIOBJ,
    OUT_OUTLINE_PRECIS, OPAQUE, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BN_CLICKED, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GetClientRect,
    GetMessageW, HMENU, IDC_ARROW, LoadCursorW, MSG, PostThreadMessageW, RegisterClassW, SW_HIDE,
    SW_RESTORE, SW_SHOW, SWP_NOMOVE, SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetWindowPos,
    SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND,
    WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::history;

const PANEL_THREAD_QUIT: u32 = WM_APP + 141;
const PANEL_OPEN: u32 = WM_APP + 142;

// Logical design sizes at 96 DPI; scaled at runtime with Windows display scale.
const CLIENT_W_96: i32 = 920;
const CLIENT_H_96: i32 = 840;
const MARGIN_96: i32 = 28;
const FONT_PX_96: i32 = 22;
const TITLE_FONT_PX_96: i32 = 30;
const PANEL_FONT_FAMILY: &str = "Microsoft YaHei UI";

const ID_TITLE: i32 = 5001;
const ID_SUMMARY: i32 = 5002;
const ID_BODY: i32 = 5003;
const ID_REFRESH: i32 = 5004;
const ID_OPEN_FOLDER: i32 = 5005;
const ID_CLOSE: i32 = 5006;

// Dark chrome
const BG: COLORREF = COLORREF(0x00_16_14_14);
const TEXT: COLORREF = COLORREF(0x00_F2_F0_F0);
const BUTTON_BG: COLORREF = COLORREF(0x00_32_2A_2A);
// Light body surface — readable multiline EDIT (dark themed EDIT stacks glyphs on Win32).
const BODY_BG: COLORREF = COLORREF(0x00_F4_F0_EC);
const BODY_TEXT: COLORREF = COLORREF(0x00_1E_1A_18);

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
    brush_body: HBRUSH,
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
            brush_body: HBRUSH::default(),
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
    /// Always-available brushes for WM_CTLCOLOR* — never depend on RefCell borrow.
    static CTL_BRUSH_BG: Cell<isize> = const { Cell::new(0) };
    static CTL_BRUSH_BODY: Cell<isize> = const { Cell::new(0) };
    static CTL_BRUSH_BUTTON: Cell<isize> = const { Cell::new(0) };
}

fn set_ctl_brushes(bg: HBRUSH, body: HBRUSH, button: HBRUSH) {
    CTL_BRUSH_BG.with(|c| c.set(bg.0 as isize));
    CTL_BRUSH_BODY.with(|c| c.set(body.0 as isize));
    CTL_BRUSH_BUTTON.with(|c| c.set(button.0 as isize));
}

fn ctl_brush_bg() -> HBRUSH {
    HBRUSH(CTL_BRUSH_BG.with(|c| c.get()) as *mut _)
}

fn ctl_brush_body() -> HBRUSH {
    HBRUSH(CTL_BRUSH_BODY.with(|c| c.get()) as *mut _)
}

fn ctl_brush_button() -> HBRUSH {
    HBRUSH(CTL_BRUSH_BUTTON.with(|c| c.get()) as *mut _)
}

fn ui_scale_from_hwnd(hwnd: Option<HWND>) -> f32 {
    let dpi = unsafe {
        if let Some(hwnd) = hwnd {
            let d = GetDpiForWindow(hwnd);
            if d > 0 { d } else { 96 }
        } else {
            96
        }
    };
    (dpi as f32 / 96.0).max(1.0)
}

fn scale_px(value_96: i32, scale: f32) -> i32 {
    ((value_96 as f32) * scale).round().max(1.0) as i32
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

fn outer_size_for_client(style: WINDOW_STYLE, client_w: i32, client_h: i32) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    let ok = unsafe { AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0)) };
    if ok.is_err() {
        return (client_w + 16, client_h + 40);
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
        // v3: light body surface + CTL brushes outside RefCell.
        lpszClassName: w!("ainput_history_panel_v3"),
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
    let (outer_w, outer_h) = outer_size_for_client(style, CLIENT_W_96, CLIENT_H_96);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_history_panel_v3"),
            PCWSTR(title.as_ptr()),
            style,
            80,
            40,
            outer_w,
            outer_h,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create history panel window failed: {error}"))?;
    let scale = ui_scale_from_hwnd(Some(hwnd));
    if (scale - 1.0).abs() > 0.01 {
        let client_w = scale_px(CLIENT_W_96, scale);
        let client_h = scale_px(CLIENT_H_96, scale);
        let (ow, oh) = outer_size_for_client(style, client_w, client_h);
        unsafe {
            let _ = SetWindowPos(hwnd, None, 0, 0, ow, oh, SWP_NOMOVE | SWP_NOZORDER);
        }
    }
    Ok(hwnd)
}

/// Create children **without** holding PANEL_STATE RefCell across CreateWindow/SendMessage
/// (those re-enter WM_CTLCOLOR* and would get a null brush → stacked glyphs).
unsafe fn create_panel_controls(hwnd: HWND) -> Result<()> {
    let scale = ui_scale_from_hwnd(Some(hwnd));
    let font_px = scale_px(FONT_PX_96, scale);
    let title_px = scale_px(TITLE_FONT_PX_96, scale);
    let margin = scale_px(MARGIN_96, scale);
    let client_w = scale_px(CLIENT_W_96, scale);
    let client_h = scale_px(CLIENT_H_96, scale);
    info!(
        scale,
        font_px, title_px, client_w, client_h, "history panel layout scaled for display DPI"
    );

    let font = unsafe { create_ui_font(font_px, 400) };
    let title_font = unsafe { create_ui_font(title_px, 600) };
    if font.is_invalid() || title_font.is_invalid() {
        return Err(anyhow!("create history panel font failed"));
    }
    let brush_bg = unsafe { CreateSolidBrush(BG) };
    let brush_body = unsafe { CreateSolidBrush(BODY_BG) };
    let brush_button = unsafe { CreateSolidBrush(BUTTON_BG) };
    set_ctl_brushes(brush_bg, brush_body, brush_button);

    let instance = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
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
        let use_font = if use_title { title_font } else { font };
        unsafe {
            SendMessageW(
                child_hwnd,
                WM_SETFONT,
                Some(WPARAM(use_font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        Ok(child_hwnd)
    };

    let field_w = client_w - margin * 2;
    let title_h = scale_px(36, scale);
    let summary_h = scale_px(56, scale);
    let btn_h = scale_px(48, scale);
    let btn_w = scale_px(128, scale);
    let btn_w_wide = scale_px(180, scale);
    let gap = scale_px(16, scale);
    let mut y = scale_px(22, scale);

    let _title = child(
        w!("STATIC"),
        "听写历史",
        WINDOW_STYLE(0),
        margin,
        y,
        field_w,
        title_h,
        ID_TITLE,
        true,
    )?;
    y += title_h + scale_px(10, scale);
    let summary = child(
        w!("STATIC"),
        "加载中…",
        WINDOW_STYLE(0),
        margin,
        y,
        field_w,
        summary_h,
        ID_SUMMARY,
        false,
    )?;
    y += summary_h + scale_px(12, scale);
    let body_h = (client_h - y - btn_h - scale_px(28, scale)).max(scale_px(200, scale));
    let body = child(
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
        margin,
        y,
        field_w,
        body_h,
        ID_BODY,
        false,
    )?;
    y += body_h + gap;
    let _refresh = child(
        w!("BUTTON"),
        "刷新",
        WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
        margin,
        y,
        btn_w,
        btn_h,
        ID_REFRESH,
        false,
    )?;
    let _open = child(
        w!("BUTTON"),
        "打开存档目录",
        WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
        margin + btn_w + gap,
        y,
        btn_w_wide,
        btn_h,
        ID_OPEN_FOLDER,
        false,
    )?;
    let _close = child(
        w!("BUTTON"),
        "关闭",
        WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
        margin + btn_w + gap + btn_w_wide + gap,
        y,
        btn_w,
        btn_h,
        ID_CLOSE,
        false,
    )?;

    // Store HWNDs / resources after all CreateWindow calls finished.
    PANEL_STATE.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            state.summary = summary;
            state.body = body;
            state.brush_bg = brush_bg;
            state.brush_body = brush_body;
            state.brush_button = brush_button;
            state.font = font;
            state.title_font = title_font;
        }
    });
    Ok(())
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
                let brush = ctl_brush_bg();
                if !brush.is_invalid() {
                    let _ = FillRect(hdc, &rect, brush);
                }
            }
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, TEXT);
            }
            LRESULT(ctl_brush_bg().0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            // Light opaque body: solid fill matches brush — no stacked glyphs.
            unsafe {
                let _ = SetBkMode(hdc, OPAQUE);
                SetBkColor(hdc, BODY_BG);
                SetTextColor(hdc, BODY_TEXT);
            }
            LRESULT(ctl_brush_body().0 as isize)
        }
        WM_CTLCOLORBTN => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, TEXT);
            }
            LRESULT(ctl_brush_button().0 as isize)
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
                    ID_CLOSE => unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    },
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
                    unsafe {
                        if !state.font.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(state.font.0));
                        }
                        if !state.title_font.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(state.title_font.0));
                        }
                        if !state.brush_bg.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(state.brush_bg.0));
                        }
                        if !state.brush_body.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(state.brush_body.0));
                        }
                        if !state.brush_button.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(state.brush_button.0));
                        }
                    }
                    state.font = HFONT::default();
                    state.title_font = HFONT::default();
                    state.brush_bg = HBRUSH::default();
                    state.brush_body = HBRUSH::default();
                    state.brush_button = HBRUSH::default();
                }
            });
            set_ctl_brushes(HBRUSH::default(), HBRUSH::default(), HBRUSH::default());
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// Silence unused import if OPAQUE is used via path.
#[allow(dead_code)]
const _OPAQUE_CHECK: BACKGROUND_MODE = OPAQUE;
