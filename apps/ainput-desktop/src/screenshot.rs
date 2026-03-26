use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use image::{ColorType, ImageFormat};
use tracing::{error, info};
use windows::Win32::Foundation::{
    COLORREF, GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BeginPaint,
    BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreatePen,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, GetDC, GetStockObject, HBITMAP, HDC, HGDIOBJ,
    InvalidateRect, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, RGBQUAD, Rectangle, SRCCOPY, SelectObject,
    SetDIBits,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, IDC_CROSS,
    LoadCursorW, MSG, PostMessageW, RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PWSTR, w};
use winit::event_loop::EventLoopProxy;

static CAPTURE_LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static CAPTURE_SESSION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static SELECTION_OVERLAY: OnceLock<SelectionOverlayController> = OnceLock::new();
const SCREENSHOT_DIM_ALPHA: u8 = 72;
const WM_AINPUT_START_SELECTION: u32 = 0x8001;

pub(crate) enum CaptureEvent {
    Started,
    Cancelled,
    Copied { saved_path: Option<PathBuf> },
    Error(String),
}

fn append_capture_debug(message: &str) {
    let base_dir = CAPTURE_LOG_DIR
        .get()
        .cloned()
        .or_else(|| std::env::current_dir().ok().map(|dir| dir.join("logs")));
    let Some(logs_dir) = base_dir else {
        return;
    };

    if fs::create_dir_all(&logs_dir).is_err() {
        return;
    }

    let log_path = logs_dir.join("capture-debug.log");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "[{timestamp}] {message}");
    }
}

fn create_owned_hbitmap_from_pixels(
    width: i32,
    height: i32,
    pixels_bgra: &[u8],
) -> Result<HBITMAP> {
    let screen = WindowDc::acquire()?;
    let bitmap = unsafe { CreateCompatibleBitmap(screen.hdc, width, height) };
    if bitmap.is_invalid() {
        return Err(anyhow!("创建剪贴板位图失败"));
    }

    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };

    let written = unsafe {
        SetDIBits(
            Some(screen.hdc),
            bitmap,
            0,
            height as u32,
            pixels_bgra.as_ptr() as *const c_void,
            &bitmap_info,
            DIB_RGB_COLORS,
        )
    };
    if written == 0 {
        unsafe {
            let _ = DeleteObject(bitmap.into());
        }
        return Err(anyhow!("写入剪贴板位图像素失败"));
    }

    Ok(bitmap)
}

pub(crate) fn start_capture_session(
    proxy: EventLoopProxy<crate::AppEvent>,
    runtime: crate::AppRuntime,
) {
    let _ = CAPTURE_LOG_DIR.set(runtime.runtime_paths.logs_dir.clone());
    thread::spawn(move || {
        let session_id = CAPTURE_SESSION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        append_capture_debug(&format!(
            "session={session_id} start auto_save={}",
            runtime.config.capture.auto_save_to_desktop
        ));
        info!("capture session requested");
        let _ = proxy.send_event(crate::AppEvent::Capture(CaptureEvent::Started));
        match run_capture_session(session_id, &runtime) {
            Ok(Some(saved_path)) => {
                append_capture_debug(&format!(
                    "session={session_id} done copied saved={}",
                    saved_path.display()
                ));
                info!(saved_path = %saved_path.display(), "capture session finished with clipboard + desktop save");
                let _ = proxy.send_event(crate::AppEvent::Capture(CaptureEvent::Copied {
                    saved_path: Some(saved_path),
                }));
            }
            Ok(None) => {
                append_capture_debug(&format!("session={session_id} done copied"));
                info!("capture session finished with clipboard only");
                let _ = proxy.send_event(crate::AppEvent::Capture(CaptureEvent::Copied {
                    saved_path: None,
                }));
            }
            Err(error) if error.to_string() == "__AINPUT_CAPTURE_CANCELLED__" => {
                append_capture_debug(&format!("session={session_id} cancelled"));
                info!("capture session cancelled");
                let _ = proxy.send_event(crate::AppEvent::Capture(CaptureEvent::Cancelled));
            }
            Err(error) => {
                append_capture_debug(&format!("session={session_id} error {error}"));
                error!(error = %error, "capture session failed");
                let _ = proxy.send_event(crate::AppEvent::Capture(CaptureEvent::Error(format!(
                    "截图失败：{error}"
                ))));
            }
        }
    });
}

pub(crate) fn debug_test_clipboard_write() -> Result<()> {
    let width = 64;
    let height = 64;
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            pixels[i] = 0x20;
            pixels[i + 1] = (x as u8).saturating_mul(3);
            pixels[i + 2] = (y as u8).saturating_mul(3);
            pixels[i + 3] = 0xFF;
        }
    }
    let snapshot = ScreenSnapshot {
        virtual_left: 0,
        virtual_top: 0,
        width: width as i32,
        height: height as i32,
        pixels_bgra: pixels.clone(),
    };
    info!("running clipboard self-test image write");
    copy_image_to_clipboard(0, &snapshot)
}

pub(crate) fn debug_capture_fullscreen_to_clipboard() -> Result<()> {
    info!("running fullscreen capture self-test");
    let snapshot = ScreenSnapshot::capture()?;
    copy_image_to_clipboard(0, &snapshot)
}

fn run_capture_session(session_id: u64, runtime: &crate::AppRuntime) -> Result<Option<PathBuf>> {
    let snapshot = ScreenSnapshot::capture()?;
    append_capture_debug(&format!(
        "session={session_id} snapshot width={} height={}",
        snapshot.width, snapshot.height
    ));
    let selection = SelectionSession::run(snapshot.clone())?;
    append_capture_debug(&format!(
        "session={session_id} selection left={} top={} width={} height={}",
        selection.left, selection.top, selection.width, selection.height
    ));
    info!(
        left = selection.left,
        top = selection.top,
        width = selection.width,
        height = selection.height,
        "capture selection completed"
    );
    let selected = snapshot.crop(selection)?;
    append_capture_debug(&format!(
        "session={session_id} crop width={} height={} bytes={}",
        selected.width,
        selected.height,
        selected.pixels_bgra.len()
    ));
    copy_image_to_clipboard(session_id, &selected)?;

    if runtime.config.capture.auto_save_to_desktop {
        let path = save_png_to_desktop(&selected)?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

#[derive(Clone)]
struct ScreenSnapshot {
    virtual_left: i32,
    virtual_top: i32,
    width: i32,
    height: i32,
    pixels_bgra: Vec<u8>,
}

impl ScreenSnapshot {
    fn capture() -> Result<Self> {
        unsafe {
            let virtual_left = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let virtual_top = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if width <= 0 || height <= 0 {
                return Err(anyhow!("读取虚拟桌面尺寸失败"));
            }

            let screen_dc = WindowDc::acquire()?;
            let memory_dc = MemoryDc::create(screen_dc.hdc)?;
            let bitmap = DibSection::create(screen_dc.hdc, width, height)?;

            let previous = SelectObject(memory_dc.hdc, HGDIOBJ(bitmap.handle.0));
            if previous.0.is_null() {
                return Err(anyhow!("选择截图位图失败"));
            }

            let copied = BitBlt(
                memory_dc.hdc,
                0,
                0,
                width,
                height,
                Some(screen_dc.hdc),
                virtual_left,
                virtual_top,
                SRCCOPY | CAPTUREBLT,
            );
            let _ = SelectObject(memory_dc.hdc, previous);

            if copied.is_err() {
                return Err(anyhow!("拷贝屏幕位图失败"));
            }

            info!(
                width,
                height, virtual_left, virtual_top, "screen snapshot captured"
            );

            let pixels = bitmap.copy_pixels();
            Ok(Self {
                virtual_left,
                virtual_top,
                width,
                height,
                pixels_bgra: pixels.clone(),
            })
        }
    }

    fn crop(&self, rect: SelectionRect) -> Result<Self> {
        if rect.width <= 0 || rect.height <= 0 {
            return Err(anyhow!("截图区域为空"));
        }

        let stride = self.width as usize * 4;
        let target_stride = rect.width as usize * 4;
        let mut pixels = vec![0u8; target_stride * rect.height as usize];

        for row in 0..rect.height as usize {
            let src_y = rect.top as usize + row;
            let src_start = src_y * stride + rect.left as usize * 4;
            let dst_start = row * target_stride;
            pixels[dst_start..dst_start + target_stride]
                .copy_from_slice(&self.pixels_bgra[src_start..src_start + target_stride]);
        }

        Ok(Self {
            virtual_left: self.virtual_left + rect.left,
            virtual_top: self.virtual_top + rect.top,
            width: rect.width,
            height: rect.height,
            pixels_bgra: pixels,
        })
    }

    fn rgba_pixels(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.pixels_bgra.len());
        for chunk in self.pixels_bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 255]);
        }
        rgba
    }
}

#[derive(Clone, Copy)]
struct SelectionRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

struct SelectionSession;

impl SelectionSession {
    fn run(snapshot: ScreenSnapshot) -> Result<SelectionRect> {
        let controller = selection_overlay_controller()?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        controller
            .request_tx
            .send(OverlayRequest::BeginSelection {
                snapshot,
                response_tx,
            })
            .map_err(|_| anyhow!("截图窗口线程不可用"))?;

        unsafe {
            let _ = PostMessageW(
                Some(HWND(controller.hwnd_raw as *mut c_void)),
                WM_AINPUT_START_SELECTION,
                WPARAM(0),
                LPARAM(0),
            );
        }

        match response_rx
            .recv()
            .map_err(|_| anyhow!("截图窗口线程已断开"))?
        {
            OverlayResponse::Selected(rect) => Ok(rect),
            OverlayResponse::Cancelled => Err(anyhow!("__AINPUT_CAPTURE_CANCELLED__")),
            OverlayResponse::Error(message) => Err(anyhow!(message)),
        }
    }
}

struct SelectionOverlayController {
    hwnd_raw: isize,
    request_tx: mpsc::Sender<OverlayRequest>,
}

enum OverlayRequest {
    BeginSelection {
        snapshot: ScreenSnapshot,
        response_tx: mpsc::SyncSender<OverlayResponse>,
    },
}

enum OverlayResponse {
    Selected(SelectionRect),
    Cancelled,
    Error(String),
}

struct SelectionHostState {
    request_rx: mpsc::Receiver<OverlayRequest>,
    active: Option<SelectionWindowState>,
    response_tx: Option<mpsc::SyncSender<OverlayResponse>>,
}

struct SelectionWindowState {
    snapshot: ScreenSnapshot,
    original_surface: Rc<BitmapSurface>,
    dimmed_surface: Rc<BitmapSurface>,
    frame_surface: Rc<BitmapSurface>,
    dragging: bool,
    drag_start: POINT,
    drag_current: POINT,
    result: Option<SelectionRect>,
    cancelled: bool,
}

impl SelectionWindowState {
    fn new(snapshot: ScreenSnapshot) -> Result<Self> {
        let screen = WindowDc::acquire()?;
        let original_surface = Rc::new(BitmapSurface::from_pixels(
            screen.hdc,
            snapshot.width,
            snapshot.height,
            &snapshot.pixels_bgra,
        )?);
        Ok(Self {
            original_surface: original_surface.clone(),
            dimmed_surface: Rc::new(create_dimmed_surface(&snapshot, &original_surface)?),
            frame_surface: Rc::new(BitmapSurface::blank(
                screen.hdc,
                snapshot.width,
                snapshot.height,
            )?),
            snapshot,
            dragging: false,
            drag_start: POINT::default(),
            drag_current: POINT::default(),
            result: None,
            cancelled: false,
        })
    }

    fn current_rect(&self) -> Option<SelectionRect> {
        let left = self
            .drag_start
            .x
            .min(self.drag_current.x)
            .clamp(0, self.snapshot.width);
        let top = self
            .drag_start
            .y
            .min(self.drag_current.y)
            .clamp(0, self.snapshot.height);
        let right = self
            .drag_start
            .x
            .max(self.drag_current.x)
            .clamp(0, self.snapshot.width);
        let bottom = self
            .drag_start
            .y
            .max(self.drag_current.y)
            .clamp(0, self.snapshot.height);
        let width = right - left;
        let height = bottom - top;
        if width < 2 || height < 2 {
            None
        } else {
            Some(SelectionRect {
                left,
                top,
                width,
                height,
            })
        }
    }
}

fn selection_overlay_controller() -> Result<&'static SelectionOverlayController> {
    if let Some(controller) = SELECTION_OVERLAY.get() {
        return Ok(controller);
    }

    let (init_tx, init_rx) = mpsc::sync_channel(1);
    thread::spawn(move || run_selection_overlay_thread(init_tx));
    let controller = init_rx
        .recv()
        .map_err(|_| anyhow!("截图窗口线程启动失败"))??;
    let _ = SELECTION_OVERLAY.set(controller);
    SELECTION_OVERLAY
        .get()
        .ok_or_else(|| anyhow!("截图窗口控制器初始化失败"))
}

fn run_selection_overlay_thread(init_tx: mpsc::SyncSender<Result<SelectionOverlayController>>) {
    let result = (|| -> Result<()> {
        unsafe {
            static CLASS_REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            CLASS_REGISTERED.get_or_init(|| {
                let instance = GetModuleHandleW(None).expect("resolve module handle");
                let cursor = LoadCursorW(None, IDC_CROSS).expect("load cross cursor");
                let class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(selection_window_proc),
                    hInstance: HINSTANCE(instance.0),
                    lpszClassName: w!("ainput_screenshot_overlay"),
                    hCursor: cursor,
                    ..Default::default()
                };
                let _ = RegisterClassW(&class);
            });

            let instance = GetModuleHandleW(None).context("resolve module handle")?;
            let (request_tx, request_rx) = mpsc::channel();
            let state = Box::new(SelectionHostState {
                request_rx,
                active: None,
                response_tx: None,
            });
            let state_ptr = Box::into_raw(state);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
                w!("ainput_screenshot_overlay"),
                w!(""),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(instance.0)),
                Some(state_ptr as *const c_void),
            )
            .map_err(|error| anyhow!("创建截图窗口失败：{error}"))?;

            let _ = init_tx.send(Ok(SelectionOverlayController {
                hwnd_raw: hwnd.0 as isize,
                request_tx,
            }));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            let _ = DestroyWindow(hwnd);
            let _ = Box::from_raw(state_ptr);
            Ok(())
        }
    })();

    if let Err(error) = result {
        let _ = init_tx.send(Err(error));
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
            let state_ptr = unsafe { (*create).lpCreateParams } as *mut SelectionHostState;
            let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };
            LRESULT(1)
        }
        WM_CREATE => LRESULT(0),
        WM_ERASEBKGND => LRESULT(1),
        WM_AINPUT_START_SELECTION => {
            if let Some(host) = host_state_mut(hwnd) {
                match host.request_rx.try_recv() {
                    Ok(OverlayRequest::BeginSelection {
                        snapshot,
                        response_tx,
                    }) => match SelectionWindowState::new(snapshot) {
                        Ok(state) => {
                            let left = state.snapshot.virtual_left;
                            let top = state.snapshot.virtual_top;
                            let width = state.snapshot.width;
                            let height = state.snapshot.height;
                            host.active = Some(state);
                            host.response_tx = Some(response_tx);
                            let _ = unsafe {
                                SetWindowPos(
                                    hwnd,
                                    Some(HWND(null_mut())),
                                    left,
                                    top,
                                    width,
                                    height,
                                    SWP_SHOWWINDOW | SWP_NOACTIVATE,
                                )
                            };
                            request_selection_repaint(hwnd);
                        }
                        Err(error) => {
                            let _ = response_tx.send(OverlayResponse::Error(error.to_string()));
                        }
                    },
                    Err(_) => {}
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(state) = active_state_mut(hwnd) {
                state.dragging = true;
                state.drag_start = point_from_lparam(lparam);
                state.drag_current = state.drag_start;
                let _ = unsafe { SetCapture(hwnd) };
                request_selection_repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(state) = active_state_mut(hwnd)
                && state.dragging
            {
                state.drag_current = point_from_lparam(lparam);
                request_selection_repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(state) = active_state_mut(hwnd)
                && state.dragging
            {
                state.drag_current = point_from_lparam(lparam);
                state.dragging = false;
                state.result = state.current_rect();
                let _ = unsafe { ReleaseCapture() };
                request_selection_repaint(hwnd);
                finish_selection(hwnd);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some(state) = active_state_mut(hwnd) {
                state.cancelled = true;
                request_selection_repaint(hwnd);
            }
            finish_selection(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u16 == VK_ESCAPE.0 {
                if let Some(state) = active_state_mut(hwnd) {
                    state.cancelled = true;
                    request_selection_repaint(hwnd);
                }
                finish_selection(hwnd);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            if let Some(state) = active_state_mut(hwnd) {
                paint_selection_window(hwnd, state);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint_selection_window(hwnd: HWND, state: &SelectionWindowState) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        let dirty = paint.rcPaint;
        let dirty_width = dirty.right - dirty.left;
        let dirty_height = dirty.bottom - dirty.top;

        let _ = BitBlt(
            state.frame_surface.dc,
            0,
            0,
            state.snapshot.width,
            state.snapshot.height,
            Some(state.dimmed_surface.dc),
            0,
            0,
            SRCCOPY,
        );

        if let Some(rect) = state.current_rect() {
            let _ = BitBlt(
                state.frame_surface.dc,
                rect.left,
                rect.top,
                rect.width,
                rect.height,
                Some(state.original_surface.dc),
                rect.left,
                rect.top,
                SRCCOPY,
            );
            draw_selection_border(state.frame_surface.dc, rect);
        }

        if dirty_width > 0 && dirty_height > 0 {
            let _ = BitBlt(
                hdc,
                dirty.left,
                dirty.top,
                dirty_width,
                dirty_height,
                Some(state.frame_surface.dc),
                dirty.left,
                dirty.top,
                SRCCOPY,
            );
        }

        let _ = EndPaint(hwnd, &paint);
    }
}

fn host_state_mut(hwnd: HWND) -> Option<&'static mut SelectionHostState> {
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SelectionHostState;
    if state_ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *state_ptr })
    }
}

fn active_state_mut(hwnd: HWND) -> Option<&'static mut SelectionWindowState> {
    host_state_mut(hwnd)?.active.as_mut()
}

fn finish_selection(hwnd: HWND) {
    if let Some(host) = host_state_mut(hwnd) {
        if let Some(response_tx) = host.response_tx.take() {
            let response = match host.active.take() {
                Some(state) if state.cancelled => OverlayResponse::Cancelled,
                Some(state) => match state.result {
                    Some(rect) => OverlayResponse::Selected(rect),
                    None => OverlayResponse::Cancelled,
                },
                None => OverlayResponse::Cancelled,
            };
            let _ = response_tx.send(response);
        }
    }

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 as i16) as i32,
        y: ((lparam.0 >> 16) as i16) as i32,
    }
}

fn copy_image_to_clipboard(session_id: u64, image: &ScreenSnapshot) -> Result<()> {
    let dib_bytes = build_cf_dib_bytes(image);
    let bitmap = create_owned_hbitmap_from_pixels(image.width, image.height, &image.pixels_bgra)?;
    append_capture_debug(&format!(
        "session={session_id} clipboard-begin width={} height={} dib_bytes={}",
        image.width,
        image.height,
        dib_bytes.len()
    ));
    info!(
        width = image.width,
        height = image.height,
        dib_bytes = dib_bytes.len(),
        "begin clipboard image write"
    );
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, dib_bytes.len()).context("分配剪贴板内存失败")?;
        let mut bitmap_transferred = false;
        let mut dib_transferred = false;
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            let _ = DeleteObject(bitmap.into());
            error!("GlobalLock failed for clipboard image");
            return Err(anyhow!("锁定剪贴板内存失败"));
        }
        std::ptr::copy_nonoverlapping(dib_bytes.as_ptr(), ptr as *mut u8, dib_bytes.len());
        let _ = GlobalUnlock(hglobal);

        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !opened {
            let _ = GlobalFree(Some(hglobal));
            let _ = DeleteObject(bitmap.into());
            append_capture_debug(&format!("session={session_id} clipboard-open-failed"));
            error!("OpenClipboard failed after retries");
            return Err(anyhow!("打开系统剪贴板失败"));
        }
        info!("clipboard opened");
        append_capture_debug(&format!(
            "session={session_id} clipboard-opened seq={}",
            GetClipboardSequenceNumber()
        ));

        let result = EmptyClipboard()
            .context("清空系统剪贴板失败")
            .and_then(|_| {
                info!("clipboard emptied");
                append_capture_debug(&format!(
                    "session={session_id} clipboard-emptied seq={}",
                    GetClipboardSequenceNumber()
                ));
                SetClipboardData(CF_BITMAP.0 as u32, Some(HANDLE(bitmap.0)))
                    .context("写入系统剪贴板位图失败")
                    .and_then(|_| {
                        bitmap_transferred = true;
                        append_capture_debug(&format!(
                            "session={session_id} clipboard-cf_bitmap-ok"
                        ));
                        SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hglobal.0)))
                            .context("写入系统剪贴板 DIB 失败")
                    })
                    .map(|_| {
                        dib_transferred = true;
                    })
            });
        let _ = CloseClipboard();

        if result.is_err() {
            if !dib_transferred {
                let _ = GlobalFree(Some(hglobal));
            }
            if !bitmap_transferred {
                let _ = DeleteObject(bitmap.into());
            }
            append_capture_debug(&format!(
                "session={session_id} clipboard-error {}",
                result.as_ref().err().unwrap()
            ));
            error!(error = %result.as_ref().err().unwrap(), "clipboard image write failed");
        } else {
            append_capture_debug(&format!(
                "session={session_id} clipboard-success seq={}",
                GetClipboardSequenceNumber()
            ));
            info!("clipboard image write finished");
        }

        result
    }
}

fn save_png_to_desktop(image: &ScreenSnapshot) -> Result<PathBuf> {
    let desktop_dir = desktop_dir()?;
    let file_path = next_desktop_file_path(&desktop_dir);
    let rgba = image.rgba_pixels();
    image::save_buffer_with_format(
        &file_path,
        &rgba,
        image.width as u32,
        image.height as u32,
        ColorType::Rgba8,
        ImageFormat::Png,
    )
    .with_context(|| format!("保存截图到 {} 失败", file_path.display()))?;
    Ok(file_path)
}

fn next_desktop_file_path(desktop_dir: &std::path::Path) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    desktop_dir.join(format!("ainput-screenshot-{millis}.png"))
}

fn build_cf_dib_bytes(image: &ScreenSnapshot) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: image.width,
        biHeight: image.height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: (image.width * image.height * 4) as u32,
        ..Default::default()
    };

    let mut bytes = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + image.pixels_bgra.len());
    let header_bytes = unsafe {
        std::slice::from_raw_parts(
            &header as *const BITMAPINFOHEADER as *const u8,
            size_of::<BITMAPINFOHEADER>(),
        )
    };
    bytes.extend_from_slice(header_bytes);

    let stride = image.width as usize * 4;
    for row in (0..image.height as usize).rev() {
        let start = row * stride;
        bytes.extend_from_slice(&image.pixels_bgra[start..start + stride]);
    }

    bytes
}

fn desktop_dir() -> Result<PathBuf> {
    unsafe {
        let path: PWSTR = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None)
            .context("读取桌面目录失败")?;
        let desktop = path.to_string().context("转换桌面目录路径失败")?;
        CoTaskMemFree(Some(path.0 as _));
        Ok(PathBuf::from(desktop))
    }
}

struct WindowDc {
    hdc: HDC,
}

impl WindowDc {
    fn acquire() -> Result<Self> {
        let hdc = unsafe { GetDC(None) };
        if hdc.is_invalid() {
            Err(anyhow!("获取屏幕 DC 失败"))
        } else {
            Ok(Self { hdc })
        }
    }
}

impl Drop for WindowDc {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, self.hdc);
        }
    }
}

struct MemoryDc {
    hdc: HDC,
}

impl MemoryDc {
    fn create(parent: HDC) -> Result<Self> {
        let hdc = unsafe { CreateCompatibleDC(Some(parent)) };
        if hdc.is_invalid() {
            Err(anyhow!("创建内存 DC 失败"))
        } else {
            Ok(Self { hdc })
        }
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.hdc);
        }
    }
}

struct DibSection {
    handle: HBITMAP,
    bits: *mut u8,
    bytes_len: usize,
}

impl DibSection {
    fn create(parent: HDC, width: i32, height: i32) -> Result<Self> {
        let mut bits: *mut c_void = null_mut();
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let handle = unsafe {
            CreateDIBSection(
                Some(parent),
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }?;
        if handle.is_invalid() || bits.is_null() {
            Err(anyhow!("创建截图位图失败"))
        } else {
            Ok(Self {
                handle,
                bits: bits as *mut u8,
                bytes_len: width as usize * height as usize * 4,
            })
        }
    }

    fn copy_pixels(&self) -> Vec<u8> {
        unsafe { std::slice::from_raw_parts(self.bits, self.bytes_len).to_vec() }
    }
}

impl Drop for DibSection {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.handle.into());
        }
    }
}

fn request_selection_repaint(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn create_dimmed_surface(
    snapshot: &ScreenSnapshot,
    original_surface: &BitmapSurface,
) -> Result<BitmapSurface> {
    let screen = WindowDc::acquire()?;
    let dimmed = BitmapSurface::blank(screen.hdc, snapshot.width, snapshot.height)?;
    let black = BitmapSurface::solid_color(screen.hdc, 1, 1, [0, 0, 0, 255])?;

    unsafe {
        let _ = BitBlt(
            dimmed.dc,
            0,
            0,
            snapshot.width,
            snapshot.height,
            Some(original_surface.dc),
            0,
            0,
            SRCCOPY,
        );
        let _ = AlphaBlend(
            dimmed.dc,
            0,
            0,
            snapshot.width,
            snapshot.height,
            black.dc,
            0,
            0,
            1,
            1,
            BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: SCREENSHOT_DIM_ALPHA,
                AlphaFormat: 0,
            },
        );
    }

    Ok(dimmed)
}

fn draw_selection_border(hdc: HDC, rect: SelectionRect) {
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00FF_FFFF));
        if pen.is_invalid() {
            return;
        }

        let previous_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let previous_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(
            hdc,
            rect.left,
            rect.top,
            rect.left + rect.width,
            rect.top + rect.height,
        );
        let _ = SelectObject(hdc, previous_pen);
        let _ = SelectObject(hdc, previous_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

struct BitmapSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl BitmapSurface {
    fn blank(parent: HDC, width: i32, height: i32) -> Result<Self> {
        let dc = unsafe { CreateCompatibleDC(Some(parent)) };
        if dc.is_invalid() {
            return Err(anyhow!("创建位图表面 DC 失败"));
        }

        let bitmap = unsafe { CreateCompatibleBitmap(parent, width, height) };
        if bitmap.is_invalid() {
            unsafe {
                let _ = DeleteDC(dc);
            }
            return Err(anyhow!("创建兼容位图失败"));
        }

        let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        if previous.0.is_null() {
            unsafe {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
            }
            return Err(anyhow!("选择兼容位图失败"));
        }

        Ok(Self {
            dc,
            bitmap,
            previous,
        })
    }

    fn solid_color(parent: HDC, width: i32, height: i32, bgra: [u8; 4]) -> Result<Self> {
        let pixels = bgra.repeat((width * height) as usize);
        Self::from_pixels(parent, width, height, &pixels)
    }

    fn from_pixels(parent: HDC, width: i32, height: i32, pixels_bgra: &[u8]) -> Result<Self> {
        let dc = unsafe { CreateCompatibleDC(Some(parent)) };
        if dc.is_invalid() {
            return Err(anyhow!("创建位图表面 DC 失败"));
        }

        let mut bits: *mut c_void = null_mut();
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };

        let bitmap = unsafe {
            CreateDIBSection(
                Some(parent),
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }
        .context("创建位图表面失败")?;
        if bitmap.is_invalid() || bits.is_null() {
            unsafe {
                let _ = DeleteDC(dc);
            }
            return Err(anyhow!("位图表面无效"));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(pixels_bgra.as_ptr(), bits as *mut u8, pixels_bgra.len());
        }
        let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        if previous.0.is_null() {
            unsafe {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
            }
            return Err(anyhow!("选择位图表面失败"));
        }

        Ok(Self {
            dc,
            bitmap,
            previous,
        })
    }
}

impl Drop for BitmapSurface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
