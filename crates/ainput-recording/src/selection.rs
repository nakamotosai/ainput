use std::ffi::c_void;
use std::sync::{OnceLock, mpsc};
use std::thread;

use anyhow::{Context, Result, anyhow};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject,
    HGDIOBJ, InvalidateRect, NULL_BRUSH, PS_SOLID, Rectangle, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, HTTRANSPARENT, HWND_TOPMOST, IDC_CROSS, LWA_ALPHA, LWA_COLORKEY,
    LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, TranslateMessage,
    WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY,
    WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

const TRANSPARENT_COLOR: COLORREF = COLORREF(0x00FF00FF);
const SELECTION_BORDER_COLOR: COLORREF = COLORREF(0x0000FF00);
const RECORDING_BORDER_COLOR: COLORREF = COLORREF(0x000000FF);
const MIN_SELECTION_SIZE: i32 = 8;
const WM_AINPUT_RECORDING_FRAME_CLOSE: u32 = WM_APP + 41;
const SELECTION_OVERLAY_ALPHA: u8 = 72;
const OVERLAY_BORDER_THICKNESS: i32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRegion {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

pub struct RecordingFrame {
    hwnd: HWND,
    join_handle: Option<thread::JoinHandle<()>>,
}

unsafe impl Send for RecordingFrame {}

impl RecordingFrame {
    pub fn show(region: CaptureRegion) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let join_handle = thread::spawn(move || {
            let _ = run_recording_frame_thread(region, tx);
        });
        let hwnd_raw = rx.recv().map_err(|_| anyhow!("录屏边框线程启动失败"))??;
        Ok(Self {
            hwnd: HWND(hwnd_raw as *mut c_void),
            join_handle: Some(join_handle),
        })
    }

    pub fn close(mut self) {
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd),
                WM_AINPUT_RECORDING_FRAME_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn configure_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn active_monitor_count() -> usize {
    unsafe { GetSystemMetrics(SM_CMONITORS).max(1) as usize }
}

pub fn choose_region_interactive() -> Result<Option<CaptureRegion>> {
    register_selection_class();
    let metrics = virtual_screen_metrics()?;

    let state = Box::new(SelectionState {
        virtual_left: metrics.left,
        virtual_top: metrics.top,
        virtual_width: metrics.width,
        virtual_height: metrics.height,
        dragging: false,
        drag_start: POINT::default(),
        drag_current: POINT::default(),
        result: None,
        cancelled: false,
    });
    let state_ptr = Box::into_raw(state);

    let instance = unsafe { GetModuleHandleW(None) }.context("读取模块句柄失败")?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0),
            w!("ainput_record_selection"),
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            metrics.left,
            metrics.top,
            metrics.width,
            metrics.height,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            Some(state_ptr as *const c_void),
        )
    }
    .map_err(|error| anyhow!("创建框选窗口失败: {error}"))?;

    unsafe {
        SetLayeredWindowAttributes(hwnd, COLORREF(0), SELECTION_OVERLAY_ALPHA, LWA_ALPHA)
            .context("设置框选窗口透明度失败")?;
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            metrics.left,
            metrics.top,
            metrics.width,
            metrics.height,
            SWP_SHOWWINDOW,
        );
        let _ = SetForegroundWindow(hwnd);
    }

    unsafe {
        let mut msg = MSG::default();
        let mut escape_latched = false;
        loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                if msg.message == WM_QUIT {
                    break;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            if msg.message == WM_QUIT {
                break;
            }

            let escape_down = GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0;
            if escape_down && !escape_latched {
                cancel_selection(hwnd);
                escape_latched = true;
            } else if !escape_down {
                escape_latched = false;
            }

            thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    let state = unsafe { Box::from_raw(state_ptr) };
    if state.cancelled {
        return Ok(None);
    }

    Ok(state.result.map(|relative| CaptureRegion {
        left: state.virtual_left + relative.left,
        top: state.virtual_top + relative.top,
        width: relative.width,
        height: relative.height,
    }))
}

struct VirtualScreenMetrics {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

struct SelectionState {
    virtual_left: i32,
    virtual_top: i32,
    virtual_width: i32,
    virtual_height: i32,
    dragging: bool,
    drag_start: POINT,
    drag_current: POINT,
    result: Option<RelativeRect>,
    cancelled: bool,
}

#[derive(Clone, Copy)]
struct RelativeRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

struct FrameState {
    border_width: i32,
}

fn run_recording_frame_thread(
    region: CaptureRegion,
    init_tx: mpsc::SyncSender<Result<isize>>,
) -> Result<()> {
    register_frame_class();

    let state = Box::new(FrameState {
        border_width: OVERLAY_BORDER_THICKNESS,
    });
    let state_ptr = Box::into_raw(state);
    let instance = unsafe { GetModuleHandleW(None) }.context("读取模块句柄失败")?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_TOPMOST.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_TRANSPARENT.0
                    | WS_EX_LAYERED.0,
            ),
            w!("ainput_record_frame"),
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            region.left,
            region.top,
            region.width,
            region.height,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            Some(state_ptr as *const c_void),
        )
    }
    .map_err(|error| anyhow!("创建录屏边框窗口失败: {error}"))?;

    unsafe {
        SetLayeredWindowAttributes(hwnd, TRANSPARENT_COLOR, 0, LWA_COLORKEY)
            .context("设置录屏边框透明色失败")?;
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            region.left,
            region.top,
            region.width,
            region.height,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(Some(hwnd), None, true);
    }

    let _ = init_tx.send(Ok(hwnd.0 as isize));

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    Ok(())
}

fn register_selection_class() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| unsafe {
        let instance = GetModuleHandleW(None).expect("resolve module handle");
        let cursor = LoadCursorW(None, IDC_CROSS).expect("load cross cursor");
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(selection_window_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: w!("ainput_record_selection"),
            hCursor: cursor,
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
    });
}

fn register_frame_class() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| unsafe {
        let instance = GetModuleHandleW(None).expect("resolve module handle");
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(frame_window_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: w!("ainput_record_frame"),
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
    });
}

fn virtual_screen_metrics() -> Result<VirtualScreenMetrics> {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err(anyhow!("读取虚拟桌面尺寸失败"));
        }

        Ok(VirtualScreenMetrics {
            left,
            top,
            width,
            height,
        })
    }
}

unsafe extern "system" fn selection_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            let state_ptr = unsafe { (*create).lpCreateParams } as *mut SelectionState;
            let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };
            LRESULT(1)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            if let Some(state) = selection_state_mut(hwnd) {
                state.dragging = true;
                state.drag_start = point_from_lparam(lparam);
                state.drag_current = state.drag_start;
                state.result = None;
                let _ = unsafe { SetCapture(hwnd) };
                request_repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(state) = selection_state_mut(hwnd)
                && state.dragging
            {
                state.drag_current = clamp_point(
                    point_from_lparam(lparam),
                    state.virtual_width,
                    state.virtual_height,
                );
                request_repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(state) = selection_state_mut(hwnd)
                && state.dragging
            {
                state.dragging = false;
                state.drag_current = clamp_point(
                    point_from_lparam(lparam),
                    state.virtual_width,
                    state.virtual_height,
                );
                state.result = compute_relative_rect(state.drag_start, state.drag_current);
                let _ = unsafe { ReleaseCapture() };
                if state.result.is_none() {
                    state.cancelled = true;
                }
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            cancel_selection(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u16 == VK_ESCAPE.0 {
                cancel_selection(hwnd);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            if let Some(state) = selection_state_mut(hwnd) {
                paint_selection_window(hwnd, state);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn frame_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            let state_ptr = unsafe { (*create).lpCreateParams } as *mut FrameState;
            let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };
            LRESULT(1)
        }
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_AINPUT_RECORDING_FRAME_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint_frame_window(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let state_ptr = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut FrameState;
            if !state_ptr.is_null() {
                let _ = unsafe { Box::from_raw(state_ptr) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn cancel_selection(hwnd: HWND) {
    if let Some(state) = selection_state_mut(hwnd) {
        state.cancelled = true;
    }
    unsafe {
        let _ = ReleaseCapture();
        let _ = DestroyWindow(hwnd);
    }
}

fn selection_state_mut(hwnd: HWND) -> Option<&'static mut SelectionState> {
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SelectionState;
    if state_ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *state_ptr })
}

fn frame_state(hwnd: HWND) -> Option<&'static FrameState> {
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut FrameState;
    if state_ptr.is_null() {
        return None;
    }
    Some(unsafe { &*state_ptr })
}

fn paint_selection_window(hwnd: HWND, state: &SelectionState) {
    unsafe {
        let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        fill_client_with_color(hwnd, hdc, COLORREF(0x00000000));

        let current = if state.dragging {
            compute_relative_rect(state.drag_start, state.drag_current)
        } else {
            state.result
        };
        if let Some(rect) = current {
            draw_border(
                hdc,
                rect.left,
                rect.top,
                rect.width,
                rect.height,
                SELECTION_BORDER_COLOR,
                OVERLAY_BORDER_THICKNESS,
            );
        }

        let _ = EndPaint(hwnd, &paint);
    }
}

fn paint_frame_window(hwnd: HWND) {
    unsafe {
        let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        fill_client_with_key(hwnd, hdc);

        let border_width = frame_state(hwnd)
            .map(|state| state.border_width)
            .unwrap_or(OVERLAY_BORDER_THICKNESS);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width > 0 && height > 0 {
            draw_border(
                hdc,
                0,
                0,
                width,
                height,
                RECORDING_BORDER_COLOR,
                border_width,
            );
        }

        let _ = EndPaint(hwnd, &paint);
    }
}

fn fill_client_with_key(hwnd: HWND, hdc: windows::Win32::Graphics::Gdi::HDC) {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let brush = CreateSolidBrush(TRANSPARENT_COLOR);
        if !brush.is_invalid() {
            let _ = FillRect(hdc, &rect, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
}

fn fill_client_with_color(hwnd: HWND, hdc: windows::Win32::Graphics::Gdi::HDC, color: COLORREF) {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let brush = CreateSolidBrush(color);
        if !brush.is_invalid() {
            let _ = FillRect(hdc, &rect, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
}

fn draw_border(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    color: COLORREF,
    thickness: i32,
) {
    unsafe {
        let pen = CreatePen(PS_SOLID, thickness, color);
        if pen.is_invalid() {
            return;
        }

        let previous_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let previous_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(hdc, left, top, left + width, top + height);
        let _ = SelectObject(hdc, previous_pen);
        let _ = SelectObject(hdc, previous_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

fn request_repaint(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

fn clamp_point(point: POINT, max_width: i32, max_height: i32) -> POINT {
    POINT {
        x: point.x.clamp(0, max_width),
        y: point.y.clamp(0, max_height),
    }
}

fn compute_relative_rect(start: POINT, current: POINT) -> Option<RelativeRect> {
    let left = start.x.min(current.x);
    let top = start.y.min(current.y);
    let right = start.x.max(current.x);
    let bottom = start.y.max(current.y);
    let width = right - left;
    let height = bottom - top;

    if width < MIN_SELECTION_SIZE || height < MIN_SELECTION_SIZE {
        None
    } else {
        Some(RelativeRect {
            left,
            top,
            width,
            height,
        })
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 as i16) as i32,
        y: ((lparam.0 >> 16) as i16) as i32,
    }
}
