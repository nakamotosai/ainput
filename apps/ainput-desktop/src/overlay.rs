use std::time::Instant;

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, CreateSolidBrush, DeleteObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics,
    HWND_TOPMOST, LAYERED_WINDOW_ATTRIBUTES_FLAGS, RegisterClassW, SET_WINDOW_POS_FLAGS,
    SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, SystemParametersInfoW, WINDOW_STYLE,
    WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

const TRACK_WIDTH_PX: i32 = 136;
const TRACK_HEIGHT_PX: i32 = 24;
const TRACK_RADIUS_PX: i32 = 18;
const TRACK_PADDING_PX: i32 = 4;
const BOTTOM_MARGIN_PX: i32 = 18;
const SLIDE_DISTANCE_PX: i32 = 18;
const TRACK_ALPHA_MAX: u8 = 185;
const FILL_ALPHA_MAX: u8 = 245;
const TRACK_COLOR: COLORREF = COLORREF(0x00404040);
const FILL_COLOR: COLORREF = COLORREF(0x00F4F4F4);

pub struct RecordingOverlay {
    base_x: i32,
    base_y: i32,
    track_window: OverlayWindow,
    fill_window: OverlayWindow,
    shown: bool,
    visible_target: bool,
    pulse_enabled: bool,
    current_visibility: f32,
    level_target: f32,
    current_level: f32,
    started_at: Instant,
}

struct OverlayWindow {
    hwnd: HWND,
    brush: HBRUSH,
}

impl RecordingOverlay {
    pub fn create() -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)
                .map_err(|error| anyhow!("resolve module handle: {error}"))?;
            let track_brush = CreateSolidBrush(TRACK_COLOR);
            if track_brush.is_invalid() {
                return Err(anyhow!("create track overlay brush failed"));
            }
            let fill_brush = CreateSolidBrush(FILL_COLOR);
            if fill_brush.is_invalid() {
                let _ = DeleteObject(track_brush.into());
                return Err(anyhow!("create fill overlay brush failed"));
            }

            register_overlay_class(
                HINSTANCE(instance.0),
                w!("ainput_recording_overlay_track_surface"),
                track_brush,
            )?;
            register_overlay_class(
                HINSTANCE(instance.0),
                w!("ainput_recording_overlay_fill_surface"),
                fill_brush,
            )?;

            let (base_x, base_y) = work_area_bottom_center_origin();
            let track_window = create_overlay_window(
                HINSTANCE(instance.0),
                w!("ainput_recording_overlay_track_surface"),
                track_brush,
                base_x,
                base_y + SLIDE_DISTANCE_PX,
                TRACK_WIDTH_PX,
                TRACK_HEIGHT_PX,
                TRACK_RADIUS_PX,
            )?;

            let fill_height = TRACK_HEIGHT_PX - TRACK_PADDING_PX * 2;
            let fill_window = create_overlay_window(
                HINSTANCE(instance.0),
                w!("ainput_recording_overlay_fill_surface"),
                fill_brush,
                base_x + TRACK_PADDING_PX,
                base_y + TRACK_PADDING_PX + SLIDE_DISTANCE_PX,
                fill_min_width(),
                fill_height,
                0,
            )?;

            Ok(Self {
                base_x,
                base_y,
                track_window,
                fill_window,
                shown: false,
                visible_target: false,
                pulse_enabled: true,
                current_visibility: 0.0,
                level_target: 0.0,
                current_level: 0.0,
                started_at: Instant::now(),
            })
        }
    }

    pub fn show(&mut self) {
        self.visible_target = true;
        self.level_target = 0.0;
    }

    pub fn hide(&mut self) {
        self.visible_target = false;
        self.level_target = 0.0;
        self.pulse_enabled = true;
    }

    pub fn set_level(&mut self, level: f32) {
        self.level_target = level.clamp(0.0, 1.0);
    }

    pub fn set_pulse_enabled(&mut self, enabled: bool) {
        self.pulse_enabled = enabled;
    }

    pub fn tick(&mut self) {
        let visibility_target = if self.visible_target { 1.0 } else { 0.0 };
        self.current_visibility = smooth_step(self.current_visibility, visibility_target, 0.20);

        let pulse = if self.visible_target && self.pulse_enabled {
            0.07 + 0.03 * ((self.started_at.elapsed().as_secs_f32() * 5.0).sin() * 0.5 + 0.5)
        } else {
            0.0
        };
        let effective_level = self.level_target.max(pulse).clamp(0.0, 1.0);
        self.current_level = smooth_step(self.current_level, effective_level, 0.16);

        if self.current_visibility > 0.01 && !self.shown {
            unsafe {
                let _ = ShowWindow(self.track_window.hwnd, SW_SHOWNOACTIVATE);
                let _ = ShowWindow(self.fill_window.hwnd, SW_SHOWNOACTIVATE);
            }
            self.shown = true;
        }

        if self.shown {
            let offset =
                ((1.0 - self.current_visibility) * SLIDE_DISTANCE_PX as f32).round() as i32;
            let track_y = self.base_y + offset;
            let track_alpha = (TRACK_ALPHA_MAX as f32 * self.current_visibility).round() as u8;
            let fill_alpha = (FILL_ALPHA_MAX as f32 * self.current_visibility).round() as u8;
            let fill_width = current_fill_width(self.current_level);
            let fill_height = TRACK_HEIGHT_PX - TRACK_PADDING_PX * 2;

            unsafe {
                let _ = SetWindowPos(
                    self.track_window.hwnd,
                    Some(HWND_TOPMOST),
                    self.base_x,
                    track_y,
                    TRACK_WIDTH_PX,
                    TRACK_HEIGHT_PX,
                    SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
                );
                let _ = SetWindowPos(
                    self.fill_window.hwnd,
                    Some(HWND_TOPMOST),
                    self.base_x + TRACK_PADDING_PX,
                    track_y + TRACK_PADDING_PX,
                    fill_width,
                    fill_height,
                    SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
                );
                let _ = SetLayeredWindowAttributes(
                    self.track_window.hwnd,
                    COLORREF(0),
                    track_alpha,
                    LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
                );
                let _ = SetLayeredWindowAttributes(
                    self.fill_window.hwnd,
                    COLORREF(0),
                    fill_alpha,
                    LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
                );
            }
        }

        if self.current_visibility < 0.01 && self.shown && !self.visible_target {
            unsafe {
                let _ = ShowWindow(self.track_window.hwnd, SW_HIDE);
                let _ = ShowWindow(self.fill_window.hwnd, SW_HIDE);
            }
            self.shown = false;
            self.current_level = 0.0;
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.brush.into());
        }
    }
}

unsafe fn register_overlay_class(
    instance: HINSTANCE,
    class_name: windows::core::PCWSTR,
    brush: HBRUSH,
) -> Result<()> {
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_wnd_proc),
        hInstance: instance,
        lpszClassName: class_name,
        hbrBackground: brush,
        ..Default::default()
    };

    let _ = unsafe { RegisterClassW(&class) };
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_overlay_window(
    instance: HINSTANCE,
    class_name: windows::core::PCWSTR,
    brush: HBRUSH,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
) -> Result<OverlayWindow> {
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create overlay window failed: {error}"))?;

    if unsafe {
        SetLayeredWindowAttributes(
            hwnd,
            COLORREF(0),
            0,
            LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
        )
    }
    .is_err()
    {
        unsafe {
            DestroyWindow(hwnd)?;
            let _ = DeleteObject(brush.into());
        }
        return Err(anyhow!("configure overlay transparency failed"));
    }

    unsafe { apply_rounded_region(hwnd, width, height, radius)? };
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };

    Ok(OverlayWindow { hwnd, brush })
}

unsafe fn apply_rounded_region(hwnd: HWND, width: i32, height: i32, radius: i32) -> Result<()> {
    if radius <= 0 {
        return Ok(());
    }

    let region = unsafe { CreateRoundRectRgn(0, 0, width, height, radius, radius) };
    if region.is_invalid() {
        return Err(anyhow!("create rounded overlay region failed"));
    }

    let applied = unsafe { windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(region), true) };
    if applied != 1 {
        let _ = unsafe { DeleteObject(region.into()) };
        return Err(anyhow!("apply overlay region failed"));
    }

    Ok(())
}

fn work_area_bottom_center_origin() -> (i32, i32) {
    unsafe {
        let mut work_area = RECT::default();
        let got_work_area = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut work_area as *mut RECT).cast()),
            Default::default(),
        )
        .is_ok();

        let (left, right, bottom) = if got_work_area {
            (work_area.left, work_area.right, work_area.bottom)
        } else {
            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            (0, screen_width, screen_height)
        };

        let x = left + ((right - left - TRACK_WIDTH_PX) / 2);
        let y = bottom - TRACK_HEIGHT_PX - BOTTOM_MARGIN_PX;
        (x.max(0), y.max(0))
    }
}

fn fill_min_width() -> i32 {
    30
}

fn current_fill_width(level: f32) -> i32 {
    let inner_max = TRACK_WIDTH_PX - TRACK_PADDING_PX * 2;
    let inner_min = fill_min_width();
    let normalized = level.clamp(0.0, 1.0).powf(0.75);
    inner_min + ((inner_max - inner_min) as f32 * normalized).round() as i32
}

fn smooth_step(current: f32, target: f32, amount: f32) -> f32 {
    current + (target - current) * amount
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(-1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
