use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ainput_automation::{AutomationActivity, AutomationClickSnapshot, AutomationOverlayHint};
use ainput_shell::{HudAnchor, HudOverlayConfig, HudTextAlign};
use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLIP_DEFAULT_PRECIS, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DEFAULT_QUALITY, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK,
    DeleteObject, DrawTextW, FF_DONTCARE, GetDC, HBRUSH, HDC, HFONT, OUT_OUTLINE_PRECIS, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics,
    HWND_TOPMOST, LAYERED_WINDOW_ATTRIBUTES_FLAGS, RegisterClassW, SET_WINDOW_POS_FLAGS,
    SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOZORDER, SendMessageW, SetLayeredWindowAttributes, SetWindowPos, SetWindowTextW,
    ShowWindow, SystemParametersInfoW, WINDOW_STYLE, WM_CTLCOLORSTATIC, WM_NCHITTEST,
    WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

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

const HUD_SCREEN_MARGIN_PX: i32 = 8;
const STATIC_TEXT_ALIGN_LEFT: u32 = 0x0000_0000;
const STATIC_TEXT_ALIGN_CENTER: u32 = 0x0000_0001;
static HUD_TEXT_COLOR: AtomicU32 = AtomicU32::new(0x00111111);
static HUD_BACKGROUND_BRUSH: AtomicIsize = AtomicIsize::new(0);

const CLICK_OUTER_SIZE_PX: i32 = 58;
const CLICK_INNER_SIZE_PX: i32 = 18;
const CLICK_ALPHA_MAX: u8 = 208;
const CLICK_LIFETIME: Duration = Duration::from_millis(420);
const CLICK_OUTER_COLOR: COLORREF = COLORREF(0x001A8CFF);
const CLICK_INNER_COLOR: COLORREF = COLORREF(0x00FFFFFF);

pub struct RecordingOverlay {
    base_x: i32,
    base_y: i32,
    track_window: ShapeWindow,
    fill_window: ShapeWindow,
    voice_shown: bool,
    voice_visible_target: bool,
    voice_pulse_enabled: bool,
    voice_progress_mode: bool,
    voice_visibility: f32,
    voice_level_target: f32,
    voice_level_current: f32,

    hud_window: HudWindow,
    hud_message: String,
    hud_persistent: bool,
    hud_hold_until: Option<Instant>,
    hud_visibility: f32,
    hud_shown: bool,

    click_outer_window: ShapeWindow,
    click_inner_window: ShapeWindow,
    active_click: Option<ActiveClick>,
    last_click_serial: u64,

    started_at: Instant,
}

struct ShapeWindow {
    hwnd: HWND,
    brush: HBRUSH,
}

struct HudWindow {
    hwnd: HWND,
    text_hwnd: HWND,
    brush: HBRUSH,
    font: HFONT,
    style: HudStyle,
}

#[derive(Debug, Clone)]
struct HudStyle {
    anchor: HudAnchor,
    offset_x_px: i32,
    offset_y_px: i32,
    width_px: i32,
    min_width_px: i32,
    min_height_px: i32,
    min_text_width_px: i32,
    padding_x_px: i32,
    padding_y_px: i32,
    font_height_px: i32,
    font_weight: i32,
    font_family: String,
    text_align: HudTextAlign,
    text_color: COLORREF,
    background_color: COLORREF,
    background_alpha: u8,
    corner_radius_px: i32,
    display_min: Duration,
}

struct ActiveClick {
    x: i32,
    y: i32,
    started_at: Instant,
}

impl HudStyle {
    fn from_config(config: &HudOverlayConfig) -> Self {
        Self {
            anchor: config.anchor,
            offset_x_px: config.offset_x_px,
            offset_y_px: config.offset_y_px,
            width_px: config.width_px.max(220),
            min_width_px: config.min_width_px.max(120),
            min_height_px: config.min_height_px.max(56),
            min_text_width_px: config.min_text_width_px.max(64),
            padding_x_px: config.padding_x_px.max(8),
            padding_y_px: config.padding_y_px.max(6),
            font_height_px: config.font_height_px.clamp(16, 96),
            font_weight: config.font_weight.clamp(100, 900),
            font_family: if config.font_family.trim().is_empty() {
                "Microsoft YaHei".to_string()
            } else {
                config.font_family.trim().to_string()
            },
            text_align: config.text_align,
            text_color: parse_color_ref(&config.text_color, "#111111"),
            background_color: parse_color_ref(&config.background_color, "#F3F3F3"),
            background_alpha: config.background_alpha,
            corner_radius_px: config.corner_radius_px.clamp(0, 120),
            display_min: Duration::from_millis(config.display_hold_ms.clamp(100, 10_000)),
        }
    }
}

impl RecordingOverlay {
    pub fn create(config: &HudOverlayConfig) -> Result<Self> {
        let hud_style = HudStyle::from_config(config);
        unsafe {
            let instance = GetModuleHandleW(None)
                .map_err(|error| anyhow!("resolve module handle: {error}"))?;
            let instance = HINSTANCE(instance.0);

            let track_brush = CreateSolidBrush(TRACK_COLOR);
            let fill_brush = CreateSolidBrush(FILL_COLOR);
            let hud_brush = CreateSolidBrush(hud_style.background_color);
            let click_outer_brush = CreateSolidBrush(CLICK_OUTER_COLOR);
            let click_inner_brush = CreateSolidBrush(CLICK_INNER_COLOR);

            if track_brush.is_invalid()
                || fill_brush.is_invalid()
                || hud_brush.is_invalid()
                || click_outer_brush.is_invalid()
                || click_inner_brush.is_invalid()
            {
                for brush in [
                    track_brush,
                    fill_brush,
                    hud_brush,
                    click_outer_brush,
                    click_inner_brush,
                ] {
                    if !brush.is_invalid() {
                        let _ = DeleteObject(brush.into());
                    }
                }
                return Err(anyhow!("create overlay brush failed"));
            }

            register_overlay_class(
                instance,
                w!("ainput_recording_overlay_track_surface"),
                track_brush,
            )?;
            register_overlay_class(
                instance,
                w!("ainput_recording_overlay_fill_surface"),
                fill_brush,
            )?;
            register_overlay_class(instance, w!("ainput_automation_hud_surface"), hud_brush)?;
            register_overlay_class(
                instance,
                w!("ainput_automation_click_outer_surface"),
                click_outer_brush,
            )?;
            register_overlay_class(
                instance,
                w!("ainput_automation_click_inner_surface"),
                click_inner_brush,
            )?;

            let (base_x, base_y) = work_area_bottom_center_origin();
            let track_window = create_shape_window(
                instance,
                w!("ainput_recording_overlay_track_surface"),
                track_brush,
                base_x,
                base_y + SLIDE_DISTANCE_PX,
                TRACK_WIDTH_PX,
                TRACK_HEIGHT_PX,
                TRACK_RADIUS_PX,
            )?;
            let fill_window = create_shape_window(
                instance,
                w!("ainput_recording_overlay_fill_surface"),
                fill_brush,
                base_x + TRACK_PADDING_PX,
                base_y + TRACK_PADDING_PX + SLIDE_DISTANCE_PX,
                fill_min_width(false),
                TRACK_HEIGHT_PX - TRACK_PADDING_PX * 2,
                0,
            )?;

            let hud_window = create_hud_window(instance, hud_brush, &hud_style)?;
            let click_outer_window = create_shape_window(
                instance,
                w!("ainput_automation_click_outer_surface"),
                click_outer_brush,
                0,
                0,
                CLICK_OUTER_SIZE_PX,
                CLICK_OUTER_SIZE_PX,
                CLICK_OUTER_SIZE_PX,
            )?;
            let click_inner_window = create_shape_window(
                instance,
                w!("ainput_automation_click_inner_surface"),
                click_inner_brush,
                0,
                0,
                CLICK_INNER_SIZE_PX,
                CLICK_INNER_SIZE_PX,
                CLICK_INNER_SIZE_PX,
            )?;

            Ok(Self {
                base_x,
                base_y,
                track_window,
                fill_window,
                voice_shown: false,
                voice_visible_target: false,
                voice_pulse_enabled: true,
                voice_progress_mode: false,
                voice_visibility: 0.0,
                voice_level_target: 0.0,
                voice_level_current: 0.0,
                hud_window,
                hud_message: String::new(),
                hud_persistent: false,
                hud_hold_until: None,
                hud_visibility: 0.0,
                hud_shown: false,
                click_outer_window,
                click_inner_window,
                active_click: None,
                last_click_serial: 0,
                started_at: Instant::now(),
            })
        }
    }

    pub fn show(&mut self) {
        self.voice_visible_target = true;
        self.voice_level_target = 0.0;
    }

    pub fn hide(&mut self) {
        self.voice_visible_target = false;
        self.voice_level_target = 0.0;
        self.voice_pulse_enabled = true;
        self.voice_progress_mode = false;
    }

    pub fn set_level(&mut self, level: f32) {
        self.voice_level_target = level.clamp(0.0, 1.0);
    }

    pub fn set_pulse_enabled(&mut self, enabled: bool) {
        self.voice_pulse_enabled = enabled;
    }

    pub fn show_status_hud(&mut self, message: &str, persistent: bool) {
        self.suppress_voice_bar_immediately();

        if self.hud_message != message {
            self.hud_message = message.to_string();
            self.hud_window.set_text(message);
        }

        self.hud_persistent = persistent;
        self.hud_hold_until = Some(Instant::now() + self.hud_window.style.display_min);
    }

    pub fn clear_status_hud(&mut self) {
        self.hud_persistent = false;
        self.hud_hold_until = Some(Instant::now());
    }

    pub fn update_automation_feedback(
        &mut self,
        activity: AutomationActivity,
        overlay_hint: Option<&AutomationOverlayHint>,
        click: Option<&AutomationClickSnapshot>,
        status_line: &str,
    ) {
        self.suppress_voice_bar_immediately();
        let persistent = matches!(
            activity,
            AutomationActivity::Recording
                | AutomationActivity::Playing
                | AutomationActivity::Paused
        );
        let message = overlay_hint
            .map(|hint| hint.text.as_str())
            .unwrap_or(status_line);
        let should_show = persistent || overlay_hint.is_some();

        if should_show && self.hud_message != message {
            self.hud_message = message.to_string();
            self.hud_window.set_text(message);
        }

        self.hud_persistent = persistent;
        self.hud_hold_until = Some(if should_show {
            Instant::now() + self.hud_window.style.display_min
        } else {
            Instant::now()
        });

        if let Some(click) = click
            && click.serial != self.last_click_serial
        {
            self.last_click_serial = click.serial;
            self.active_click = Some(ActiveClick {
                x: click.x,
                y: click.y,
                started_at: Instant::now(),
            });
        }
    }

    pub fn tick(&mut self) {
        self.tick_voice_bar();
        self.tick_hud();
        self.tick_click_effect();
    }

    fn suppress_voice_bar_immediately(&mut self) {
        self.voice_visible_target = false;
        self.voice_level_target = 0.0;
        self.voice_level_current = 0.0;
        self.voice_visibility = 0.0;
        self.voice_pulse_enabled = true;
        self.voice_progress_mode = false;
        if self.voice_shown {
            unsafe {
                let _ = ShowWindow(self.track_window.hwnd, SW_HIDE);
                let _ = ShowWindow(self.fill_window.hwnd, SW_HIDE);
            }
            self.voice_shown = false;
        }
    }

    fn tick_voice_bar(&mut self) {
        let visibility_target = if self.voice_visible_target { 1.0 } else { 0.0 };
        self.voice_visibility = smooth_step(self.voice_visibility, visibility_target, 0.20);

        let pulse = if self.voice_visible_target && self.voice_pulse_enabled {
            0.07 + 0.03 * ((self.started_at.elapsed().as_secs_f32() * 5.0).sin() * 0.5 + 0.5)
        } else {
            0.0
        };
        let effective_level = self.voice_level_target.max(pulse).clamp(0.0, 1.0);
        self.voice_level_current = smooth_step(self.voice_level_current, effective_level, 0.16);

        if self.voice_visibility > 0.01 && !self.voice_shown {
            unsafe {
                let _ = ShowWindow(self.track_window.hwnd, SW_SHOWNOACTIVATE);
                let _ = ShowWindow(self.fill_window.hwnd, SW_SHOWNOACTIVATE);
            }
            self.voice_shown = true;
        }

        if self.voice_shown {
            let offset = ((1.0 - self.voice_visibility) * SLIDE_DISTANCE_PX as f32).round() as i32;
            let track_y = self.base_y + offset;
            let track_alpha = (TRACK_ALPHA_MAX as f32 * self.voice_visibility).round() as u8;
            let fill_alpha = (FILL_ALPHA_MAX as f32 * self.voice_visibility).round() as u8;
            let fill_width = current_fill_width(self.voice_level_current, self.voice_progress_mode);
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

        if self.voice_visibility < 0.01 && self.voice_shown && !self.voice_visible_target {
            unsafe {
                let _ = ShowWindow(self.track_window.hwnd, SW_HIDE);
                let _ = ShowWindow(self.fill_window.hwnd, SW_HIDE);
            }
            self.voice_shown = false;
            self.voice_level_current = 0.0;
        }
    }

    fn tick_hud(&mut self) {
        let should_show = hud_should_show(self.hud_persistent, self.hud_hold_until, Instant::now());
        self.hud_visibility = smooth_step(
            self.hud_visibility,
            if should_show { 1.0 } else { 0.0 },
            0.18,
        );

        if self.hud_visibility > 0.01 && !self.hud_shown {
            self.hud_window.show();
            self.hud_shown = true;
        }

        if self.hud_shown {
            let alpha =
                (self.hud_window.style.background_alpha as f32 * self.hud_visibility).round() as u8;
            self.hud_window.set_alpha(alpha);
        }

        if self.hud_visibility < 0.01 && self.hud_shown && !should_show {
            self.hud_window.hide();
            self.hud_shown = false;
        }
    }

    fn tick_click_effect(&mut self) {
        let Some(click) = &self.active_click else {
            self.hide_click_windows();
            return;
        };

        let progress = (click.started_at.elapsed().as_secs_f32() / CLICK_LIFETIME.as_secs_f32())
            .clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.active_click = None;
            self.hide_click_windows();
            return;
        }

        let outer_size = (CLICK_INNER_SIZE_PX as f32
            + (CLICK_OUTER_SIZE_PX - CLICK_INNER_SIZE_PX) as f32 * progress)
            .round() as i32;
        let inner_size = (CLICK_INNER_SIZE_PX as f32 + 10.0 * progress).round() as i32;
        let alpha = (CLICK_ALPHA_MAX as f32 * (1.0 - progress)).round() as u8;

        self.show_click_window(
            &self.click_outer_window,
            click.x,
            click.y,
            outer_size,
            alpha / 2,
        );
        self.show_click_window(
            &self.click_inner_window,
            click.x,
            click.y,
            inner_size,
            alpha,
        );
    }

    fn show_click_window(&self, window: &ShapeWindow, x: i32, y: i32, size: i32, alpha: u8) {
        let left = x - size / 2;
        let top = y - size / 2;
        unsafe {
            let _ = ShowWindow(window.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                window.hwnd,
                Some(HWND_TOPMOST),
                left,
                top,
                size,
                size,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            let _ = apply_rounded_region(window.hwnd, size, size, size);
            let _ = SetLayeredWindowAttributes(
                window.hwnd,
                COLORREF(0),
                alpha,
                LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
            );
        }
    }

    fn hide_click_windows(&self) {
        unsafe {
            let _ = ShowWindow(self.click_outer_window.hwnd, SW_HIDE);
            let _ = ShowWindow(self.click_inner_window.hwnd, SW_HIDE);
        }
    }
}

impl Drop for ShapeWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.brush.into());
        }
    }
}

impl Drop for HudWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.text_hwnd);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.font.into());
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

unsafe fn create_shape_window(
    instance: HINSTANCE,
    class_name: windows::core::PCWSTR,
    brush: HBRUSH,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
) -> Result<ShapeWindow> {
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

    unsafe { apply_rounded_region(hwnd, width, height, radius)? };
    unsafe {
        SetLayeredWindowAttributes(
            hwnd,
            COLORREF(0),
            0,
            LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
        )
    }
    .map_err(|_| anyhow!("configure overlay transparency failed"))?;

    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    Ok(ShapeWindow { hwnd, brush })
}

unsafe fn create_hud_window(
    instance: HINSTANCE,
    brush: HBRUSH,
    style: &HudStyle,
) -> Result<HudWindow> {
    let initial_width = style.width_px.max(style.min_width_px);
    let initial_height = style.min_height_px;
    let text_style_bits = match style.text_align {
        HudTextAlign::Left => STATIC_TEXT_ALIGN_LEFT,
        HudTextAlign::Center => STATIC_TEXT_ALIGN_CENTER,
    };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ainput_automation_hud_surface"),
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            initial_width,
            initial_height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create automation hud window failed: {error}"))?;

    unsafe { apply_rounded_region(hwnd, initial_width, initial_height, style.corner_radius_px)? };
    unsafe {
        SetLayeredWindowAttributes(
            hwnd,
            COLORREF(0),
            0,
            LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
        )
    }
    .map_err(|_| anyhow!("configure automation hud transparency failed"))?;

    let text_hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TRANSPARENT,
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE((WS_CHILD | WS_VISIBLE).0 | text_style_bits),
            style.padding_x_px,
            style.padding_y_px,
            (initial_width - style.padding_x_px * 2).max(1),
            (initial_height - style.padding_y_px * 2).max(1),
            Some(hwnd),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create automation hud text failed: {error}"))?;

    let font_family = HSTRING::from(style.font_family.as_str());
    let font = unsafe {
        CreateFontW(
            style.font_height_px,
            0,
            0,
            0,
            style.font_weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_OUTLINE_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(font_family.as_ptr()),
        )
    };
    if font.is_invalid() {
        let _ = unsafe { DestroyWindow(text_hwnd) };
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(anyhow!("create automation hud font failed"));
    }

    HUD_TEXT_COLOR.store(style.text_color.0, Ordering::Relaxed);
    HUD_BACKGROUND_BRUSH.store(brush.0 as isize, Ordering::Relaxed);

    unsafe {
        let _ = SendMessageW(
            text_hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    };

    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    Ok(HudWindow {
        hwnd,
        text_hwnd,
        brush,
        font,
        style: style.clone(),
    })
}

unsafe fn apply_rounded_region(hwnd: HWND, width: i32, height: i32, radius: i32) -> Result<()> {
    if radius <= 0 {
        return Ok(());
    }

    let region = unsafe { CreateRoundRectRgn(0, 0, width, height, radius, radius) };
    if region.is_invalid() {
        return Err(anyhow!("create rounded overlay region failed"));
    }

    if unsafe { SetWindowRgn(hwnd, Some(region), true) } != 1 {
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

fn work_area_rect() -> RECT {
    unsafe {
        let mut work_area = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut work_area as *mut RECT).cast()),
            Default::default(),
        )
        .is_ok()
        {
            return work_area;
        }

        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN).max(0),
            bottom: GetSystemMetrics(SM_CYSCREEN).max(0),
        }
    }
}

fn fill_min_width(progress_mode: bool) -> i32 {
    if progress_mode { 0 } else { 30 }
}

fn current_fill_width(level: f32, progress_mode: bool) -> i32 {
    let inner_max = TRACK_WIDTH_PX - TRACK_PADDING_PX * 2;
    let inner_min = fill_min_width(progress_mode);
    let normalized = if progress_mode {
        level.clamp(0.0, 1.0)
    } else {
        level.clamp(0.0, 1.0).powf(0.75)
    };
    inner_min + ((inner_max - inner_min) as f32 * normalized).round() as i32
}

fn smooth_step(current: f32, target: f32, amount: f32) -> f32 {
    current + (target - current) * amount
}

fn hud_should_show(hud_persistent: bool, hud_hold_until: Option<Instant>, now: Instant) -> bool {
    hud_persistent || hud_hold_until.is_some_and(|hold_until| now <= hold_until)
}

impl HudWindow {
    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn set_alpha(&self, alpha: u8) {
        unsafe {
            let _ = SetLayeredWindowAttributes(
                self.hwnd,
                COLORREF(0),
                alpha,
                LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002),
            );
        }
    }

    fn set_text(&self, text: &str) {
        self.resize_to_fit(text);
        let text = HSTRING::from(text);
        unsafe {
            let _ = SetWindowTextW(self.text_hwnd, &text);
        }
    }

    fn resize_to_fit(&self, text: &str) {
        let work_area = work_area_rect();
        let available_width = (work_area.right - work_area.left - HUD_SCREEN_MARGIN_PX * 2)
            .max(self.style.min_width_px);
        let available_height = (work_area.bottom - work_area.top - HUD_SCREEN_MARGIN_PX * 2)
            .max(self.style.min_height_px);
        let preferred_width = self
            .style
            .width_px
            .clamp(self.style.min_width_px, available_width);
        let max_text_width = (preferred_width - self.style.padding_x_px * 2).max(1);
        let (text_width, text_height) =
            measure_hud_text(self.text_hwnd, self.font, text, max_text_width, &self.style);
        let hud_width = (text_width + self.style.padding_x_px * 2)
            .clamp(self.style.min_width_px, available_width);
        let hud_height = (text_height + self.style.padding_y_px * 2)
            .clamp(self.style.min_height_px, available_height);

        let base_x = match self.style.anchor {
            HudAnchor::BottomLeft => work_area.left + HUD_SCREEN_MARGIN_PX,
            HudAnchor::BottomCenter => {
                work_area.left + ((work_area.right - work_area.left - hud_width) / 2)
            }
        };
        let base_y = work_area.bottom - hud_height - HUD_SCREEN_MARGIN_PX;
        let hud_x = clamp_i32(
            base_x + self.style.offset_x_px,
            work_area.left + HUD_SCREEN_MARGIN_PX,
            (work_area.right - hud_width - HUD_SCREEN_MARGIN_PX).max(work_area.left),
        );
        let hud_y = clamp_i32(
            base_y + self.style.offset_y_px,
            work_area.top + HUD_SCREEN_MARGIN_PX,
            (work_area.bottom - hud_height - HUD_SCREEN_MARGIN_PX).max(work_area.top),
        );

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                hud_x,
                hud_y,
                hud_width,
                hud_height,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            let _ = apply_rounded_region(
                self.hwnd,
                hud_width,
                hud_height,
                self.style.corner_radius_px,
            );
            let _ = SetWindowPos(
                self.text_hwnd,
                None,
                self.style.padding_x_px,
                self.style.padding_y_px,
                (hud_width - self.style.padding_x_px * 2).max(1),
                (hud_height - self.style.padding_y_px * 2).max(1),
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0 | SWP_NOZORDER.0),
            );
        }
    }
}

fn measure_hud_text(
    text_hwnd: HWND,
    font: HFONT,
    text: &str,
    max_text_width: i32,
    style: &HudStyle,
) -> (i32, i32) {
    if text.trim().is_empty() {
        return (style.min_text_width_px, style.font_height_px);
    }

    unsafe {
        let hdc = GetDC(Some(text_hwnd));
        if hdc.0.is_null() {
            return (style.min_text_width_px, style.font_height_px);
        }

        let old_font = SelectObject(hdc, font.into());
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: max_text_width,
            bottom: 0,
        };
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        let align = match style.text_align {
            HudTextAlign::Left => DT_LEFT,
            HudTextAlign::Center => DT_CENTER,
        };
        let _ = DrawTextW(
            hdc,
            utf16.as_mut_slice(),
            &mut rect,
            DT_CALCRECT | align | DT_WORDBREAK | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old_font);
        let _ = ReleaseDC(Some(text_hwnd), hdc);

        (
            (rect.right - rect.left).clamp(
                style.min_text_width_px,
                max_text_width.max(style.min_text_width_px),
            ),
            (rect.bottom - rect.top).max(style.font_height_px),
        )
    }
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as _);
            let _ = unsafe { SetBkMode(hdc, TRANSPARENT) };
            let _ = unsafe { SetTextColor(hdc, COLORREF(HUD_TEXT_COLOR.load(Ordering::Relaxed))) };
            LRESULT(HUD_BACKGROUND_BRUSH.load(Ordering::Relaxed))
        }
        WM_NCHITTEST => LRESULT(-1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    if min > max {
        return min;
    }
    value.clamp(min, max)
}

fn parse_color_ref(value: &str, fallback: &str) -> COLORREF {
    parse_color_ref_hex(value)
        .unwrap_or_else(|| parse_color_ref_hex(fallback).unwrap_or(COLORREF(0x00111111)))
}

fn parse_color_ref_hex(value: &str) -> Option<COLORREF> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }

    let rgb = u32::from_str_radix(hex, 16).ok()?;
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    Some(COLORREF((b << 16) | (g << 8) | r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_fill_width_keeps_voice_mode_minimum_and_progress_can_empty() {
        assert_eq!(fill_min_width(false), 30);
        assert_eq!(fill_min_width(true), 0);
        assert_eq!(current_fill_width(0.0, false), 30);
        assert_eq!(current_fill_width(0.0, true), 0);
    }

    #[test]
    fn current_fill_width_reaches_full_span_at_max_level() {
        let inner_max = TRACK_WIDTH_PX - TRACK_PADDING_PX * 2;
        assert_eq!(current_fill_width(1.0, false), inner_max);
        assert_eq!(current_fill_width(1.0, true), inner_max);
    }

    #[test]
    fn hud_should_show_honors_persistence_and_minimum_display_window() {
        let now = Instant::now();
        assert!(hud_should_show(true, None, now));
        assert!(hud_should_show(
            false,
            Some(now + Duration::from_millis(1)),
            now
        ));
        assert!(hud_should_show(false, Some(now), now));
        assert!(!hud_should_show(
            false,
            Some(now - Duration::from_millis(1)),
            now
        ));
        assert!(!hud_should_show(false, None, now));
    }
}
