use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
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
    BeginPaint, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DEFAULT_QUALITY, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, EndPaint, EnumFontFamiliesExW, FF_DONTCARE, FillRect,
    GGI_MARK_NONEXISTING_GLYPHS, GetDC, GetGlyphIndicesW, HDC, HFONT, HGDIOBJ, LOGFONTW,
    OUT_OUTLINE_PRECIS, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor,
    TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CBN_CLOSEUP, CBN_DROPDOWN, CBN_EDITCHANGE, CBN_SELCHANGE, CBS_DROPDOWN,
    CBS_DROPDOWNLIST, CBS_NOINTEGRALHEIGHT, CS_HREDRAW, CS_VREDRAW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, EN_CHANGE, GetClientRect, GetMessageW,
    IDC_ARROW, KillTimer, LoadCursorW, MoveWindow, PostMessageW, PostThreadMessageW,
    RegisterClassW, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WM_SETFONT,
    WM_SIZE, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::config::HUD_BACKGROUND_ALPHA_MIN_PERCENT;
use crate::hud::{HudController, HudFontAppearance, validate_font_appearance_for_hud};

const HUD_FONT_PANEL_THREAD_QUIT: u32 = WM_APP + 101;
const HUD_FONT_PANEL_OPEN: u32 = WM_APP + 102;
const HUD_FONT_PANEL_TIMER_ID: usize = 1;
const HUD_FONT_PANEL_DEBOUNCE_MS: u32 = 250;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_SELECTSTRING: u32 = 0x014D;
const CB_SETDROPPEDWIDTH: u32 = 0x0160;
const CB_SETMINVISIBLE: u32 = 0x1701;
const CB_ERR: isize = -1;

const PANEL_WIDTH: i32 = 1080;
const PANEL_HEIGHT: i32 = 700;
const PREVIEW_HEIGHT: i32 = 230;
const CONTROL_ROW_HEIGHT: i32 = 44;
const MARGIN: i32 = 12;
const LABEL_WIDTH: i32 = 165;
const CONTROL_X: i32 = 190;
const INPUT_WIDTH: i32 = 300;
const HINT_X: i32 = 520;
const HINT_WIDTH: i32 = 500;
const COMBO_DROPDOWN_HEIGHT: i32 = 420;
const COMBO_MIN_VISIBLE_ROWS: usize = 18;

const PREVIEW_BG_DARK: COLORREF = COLORREF(0x0020_2020);
const PREVIEW_BG_LIGHT: COLORREF = COLORREF(0x00E7_E7E7);
const PREVIEW_PILL_BG: COLORREF = COLORREF(0x0014_1007);
const PREVIEW_TEXT_COLOR: COLORREF = COLORREF(0x00FF_FFFF);
const PREVIEW_SAMPLE_TEXT: &str = "听写中 日本語";
const PREVIEW_HUD_MAX_WIDTH: i32 = 780;
const PREVIEW_HUD_MIN_WIDTH: i32 = 48;
const PREVIEW_HUD_MIN_HEIGHT: i32 = 48;
const PREVIEW_HUD_PADDING_X: i32 = 14;
const PREVIEW_HUD_PADDING_Y: i32 = 8;
const PREVIEW_HUD_CORNER_RADIUS: i32 = 14;

#[derive(Clone)]
pub struct HudFontPanelController {
    thread_id: u32,
    hwnd: isize,
}

impl HudFontPanelController {
    pub fn start(hud: HudController, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, isize), String>>();
        thread::spawn(move || {
            HUD_FONT_PANEL_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            HUD_FONT_PANEL_STATE.with(|state| {
                *state.borrow_mut() = Some(HudFontPanelState::new(hud));
            });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                run_hud_font_panel_thread(shutdown)
            }));
            match result {
                Ok(Ok(())) => info!("HUD font panel thread stopped"),
                Ok(Err(error)) => warn!(error = %error, "HUD font panel thread failed"),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    warn!(panic = message, "HUD font panel thread panicked");
                }
            }
        });
        let (thread_id, hwnd) = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("HUD font panel thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { thread_id, hwnd })
    }

    pub fn open(&self) {
        unsafe {
            if let Err(error) = PostMessageW(
                Some(HWND(self.hwnd as _)),
                HUD_FONT_PANEL_OPEN,
                WPARAM(0),
                LPARAM(0),
            ) {
                warn!(
                    thread_id = self.thread_id,
                    hwnd = self.hwnd,
                    error = %error,
                    "HUD font panel open message failed"
                );
            }
        }
    }
}

impl Drop for HudFontPanelController {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as _)),
                HUD_FONT_PANEL_THREAD_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
            let _ = PostThreadMessageW(
                self.thread_id,
                HUD_FONT_PANEL_THREAD_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

struct HudFontPanelState {
    hud: HudController,
    hwnd: HWND,
    preview_hwnd: HWND,
    status_hwnd: HWND,
    family_hwnd: HWND,
    size_hwnd: HWND,
    weight_hwnd: HWND,
    alpha_hwnd: HWND,
    reset_button: HWND,
    ui_font: HFONT,
    preview_font: HFONT,
    appearance: HudFontAppearance,
    updating_controls: Cell<bool>,
    family_dropdown_open: Cell<bool>,
}

impl HudFontPanelState {
    fn new(hud: HudController) -> Self {
        Self {
            hud,
            hwnd: HWND::default(),
            preview_hwnd: HWND::default(),
            status_hwnd: HWND::default(),
            family_hwnd: HWND::default(),
            size_hwnd: HWND::default(),
            weight_hwnd: HWND::default(),
            alpha_hwnd: HWND::default(),
            reset_button: HWND::default(),
            ui_font: HFONT::default(),
            preview_font: HFONT::default(),
            appearance: HudFontAppearance::default(),
            updating_controls: Cell::new(false),
            family_dropdown_open: Cell::new(false),
        }
    }
}

thread_local! {
    static HUD_FONT_PANEL_READY: RefCell<Option<mpsc::Sender<Result<(u32, isize), String>>>> =
        const { RefCell::new(None) };
    static HUD_FONT_PANEL_STATE: RefCell<Option<HudFontPanelState>> = const { RefCell::new(None) };
}

unsafe fn run_hud_font_panel_thread(shutdown: Arc<AtomicBool>) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow!("get module handle failed: {error}"))?;
    unsafe { register_panel_class(HINSTANCE(instance.0))? };
    unsafe { register_preview_class(HINSTANCE(instance.0))? };
    let hwnd = unsafe { create_panel_window(HINSTANCE(instance.0))? };
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.hwnd = hwnd;
        }
    });
    let thread_id = unsafe { GetCurrentThreadId() };
    HUD_FONT_PANEL_READY.with(|ready| {
        if let Some(sender) = ready.borrow_mut().take() {
            let _ = sender.send(Ok((thread_id, hwnd.0 as isize)));
        }
    });
    info!(thread_id, "HUD font panel thread started");

    while !shutdown.load(Ordering::Relaxed) {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if has_message.0 == -1 {
            return Err(anyhow!("HUD font panel GetMessage failed"));
        }
        if has_message.0 == 0 || msg.message == HUD_FONT_PANEL_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == HUD_FONT_PANEL_OPEN {
            show_and_refresh();
            continue;
        }
        if msg.message == WM_TIMER && msg.wParam.0 == HUD_FONT_PANEL_TIMER_ID {
            flush_apply_timer();
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

unsafe fn register_panel_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(panel_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput_hud_font_panel"),
        hCursor: cursor,
        hbrBackground: unsafe {
            windows::Win32::Graphics::Gdi::GetSysColorBrush(
                windows::Win32::Graphics::Gdi::COLOR_WINDOW,
            )
        },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn register_preview_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(preview_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput_hud_font_preview"),
        hCursor: cursor,
        hbrBackground: unsafe {
            windows::Win32::Graphics::Gdi::GetSysColorBrush(
                windows::Win32::Graphics::Gdi::COLOR_WINDOW,
            )
        },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_panel_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!(
        "ainput HUD 字体面板 {}",
        env!("CARGO_PKG_VERSION")
    ));
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_hud_font_panel"),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            scale_px(140, dpi),
            scale_px(110, dpi),
            scale_px(PANEL_WIDTH, dpi),
            scale_px(PANEL_HEIGHT, dpi),
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create HUD font panel window failed: {error}"))
}

unsafe extern "system" fn panel_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        panel_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(payload) => {
            log_wnd_proc_panic("panel", msg, payload);
            LRESULT(0)
        }
    }
}

unsafe fn panel_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            if let Err(error) = unsafe { create_panel_controls(hwnd) } {
                warn!(error = %error, "create HUD font panel controls failed");
                return LRESULT(-1);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            layout_panel(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(HWND(lparam.0 as _), hiword(wparam.0));
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

unsafe extern "system" fn preview_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        preview_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(payload) => {
            log_wnd_proc_panic("preview", msg, payload);
            LRESULT(0)
        }
    }
}

unsafe fn preview_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as _);
            let _ = unsafe { SetBkMode(hdc, TRANSPARENT) };
            LRESULT(
                unsafe {
                    windows::Win32::Graphics::Gdi::GetSysColorBrush(
                        windows::Win32::Graphics::Gdi::COLOR_WINDOW,
                    )
                }
                .0 as isize,
            )
        }
        WM_PAINT => {
            paint_preview(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn log_wnd_proc_panic(proc_name: &'static str, msg: u32, payload: Box<dyn std::any::Any + Send>) {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    warn!(
        proc_name,
        msg,
        panic = message,
        "HUD font panel wndproc panic recovered"
    );
}

unsafe fn create_panel_controls(hwnd: HWND) -> Result<()> {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let mut font_targets = Vec::new();
    let state = HUD_FONT_PANEL_STATE
        .with(|stored| {
            stored
                .borrow()
                .as_ref()
                .map(|state| state.hud.font_appearance())
        })
        .unwrap_or_else(HudFontAppearance::default);
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(panel) = stored.borrow_mut().as_mut() {
            panel.appearance = state.clone();
        }
    });

    let preview_hwnd = unsafe {
        create_control(
            hwnd,
            "ainput_hud_font_preview",
            "",
            MARGIN,
            MARGIN,
            1000,
            PREVIEW_HEIGHT,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPCHILDREN.0 | WS_CLIPSIBLINGS.0),
        )?
    };

    let status_hwnd = unsafe {
        create_control(
            hwnd,
            "STATIC",
            "改动会自动保存到 hud-user.toml，并立即同步到 live HUD。",
            MARGIN,
            MARGIN + PREVIEW_HEIGHT + 10,
            1000,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(status_hwnd);

    let font_options = installed_font_families(&state.family);
    let size_options = string_options(&[
        "24", "28", "32", "36", "40", "48", "72", "96", "120", "180", "240",
    ]);
    let weight_options = string_options(&["400", "500", "600", "700", "800", "900"]);
    let alpha_options = string_options(&["40", "60", "75", "88", "100"]);
    info!(
        font_count = font_options.len(),
        combo_dropdown_height = COMBO_DROPDOWN_HEIGHT,
        combo_min_visible_rows = COMBO_MIN_VISIBLE_ROWS,
        preview_font_height_px = state.height_px,
        "HUD font panel controls ready"
    );

    let mut y = MARGIN + PREVIEW_HEIGHT + 44;
    let family_hwnd = unsafe {
        create_combo_row(
            hwnd,
            &mut font_targets,
            y,
            "字体",
            &font_options,
            ComboKind::DropdownList,
            "只显示中文/日文字体；当前字体会保留在列表中。",
        )?
    };
    y += CONTROL_ROW_HEIGHT;
    let size_hwnd = unsafe {
        create_combo_row(
            hwnd,
            &mut font_targets,
            y,
            "字号",
            &size_options,
            ComboKind::Editable,
            "可直接输入任意正数字号；不会再被 96px 上限截断。",
        )?
    };
    y += CONTROL_ROW_HEIGHT;
    let weight_hwnd = unsafe {
        create_combo_row(
            hwnd,
            &mut font_targets,
            y,
            "字重",
            &weight_options,
            ComboKind::Editable,
            "400 是常规，700 是粗体。越粗在复杂背景上越容易看清。",
        )?
    };
    y += CONTROL_ROW_HEIGHT;
    let alpha_hwnd = unsafe {
        create_combo_row(
            hwnd,
            &mut font_targets,
            y,
            "背景不透明度 %",
            &alpha_options,
            ComboKind::Editable,
            "低于 35 会按 35 保存，避免真实 HUD 变成看不见。",
        )?
    };
    y += CONTROL_ROW_HEIGHT + 6;
    let reset_button = unsafe { create_button(hwnd, "恢复默认", MARGIN, y, 110, 30)? };
    font_targets.push(reset_button);

    let ui_font = unsafe { create_panel_font("Microsoft YaHei UI", 12, 500, dpi) };
    if ui_font.is_invalid() {
        return Err(anyhow!("create HUD font panel ui font failed"));
    }
    for control in font_targets {
        unsafe { apply_panel_font(control, ui_font) };
    }

    let preview_font = unsafe { create_preview_font(&state) };
    if preview_font.is_invalid() {
        let _ = unsafe { DeleteObject(ui_font.into()) };
        return Err(anyhow!("create HUD font preview font failed"));
    }
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(panel) = stored.borrow_mut().as_mut() {
            panel.preview_hwnd = preview_hwnd;
            panel.status_hwnd = status_hwnd;
            panel.family_hwnd = family_hwnd;
            panel.size_hwnd = size_hwnd;
            panel.weight_hwnd = weight_hwnd;
            panel.alpha_hwnd = alpha_hwnd;
            panel.reset_button = reset_button;
            panel.ui_font = ui_font;
            panel.preview_font = preview_font;
        }
    });

    set_controls_from_appearance(&state);
    refresh_preview_from_controls();
    Ok(())
}

unsafe fn create_button(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<HWND> {
    unsafe {
        create_control(
            parent,
            "BUTTON",
            text,
            x,
            y,
            width,
            height,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        )
    }
}

fn string_options(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

pub(crate) fn installed_font_families(current_family: &str) -> Vec<String> {
    let current_family = current_family.trim().to_string();
    let mut enumerated = BTreeSet::new();
    for family in [
        current_family.as_str(),
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "Microsoft JhengHei UI",
        "Microsoft JhengHei",
        "DengXian",
        "SimSun",
        "SimHei",
        "FangSong",
        "KaiTi",
        "Yu Gothic UI",
        "Yu Gothic",
        "Meiryo UI",
        "Meiryo",
        "MS Gothic",
        "MS Mincho",
        "Noto Sans CJK SC",
        "Noto Sans CJK JP",
        "Source Han Sans SC",
        "Source Han Sans JP",
    ] {
        let family = family.trim();
        if !family.is_empty() {
            enumerated.insert(family.to_string());
        }
    }

    unsafe {
        let hdc = GetDC(None);
        if !hdc.0.is_null() {
            let mut logfont = LOGFONTW {
                lfCharSet: DEFAULT_CHARSET,
                ..Default::default()
            };
            let fonts_ptr = &mut enumerated as *mut BTreeSet<String>;
            let _ = EnumFontFamiliesExW(
                hdc,
                &mut logfont,
                Some(enum_font_family_proc),
                LPARAM(fonts_ptr as isize),
                0,
            );
            let _ = ReleaseDC(None, hdc);
        }
    }

    let mut values = enumerated
        .into_iter()
        .filter(|font| {
            font == &current_family || (is_cjk_font_name(font) || font_supports_cjk_glyphs(font))
        })
        .filter(|font| font == &current_family || !is_likely_english_only_font(font))
        .collect::<Vec<_>>();
    if !current_family.is_empty() && !values.iter().any(|font| font == &current_family) {
        values.push(current_family);
    }
    values.sort_by_key(|value| font_sort_key(value));
    values
}

unsafe extern "system" fn enum_font_family_proc(
    logfont: *const LOGFONTW,
    _text_metric: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    if logfont.is_null() || lparam.0 == 0 {
        return 1;
    }
    let fonts = unsafe { &mut *(lparam.0 as *mut BTreeSet<String>) };
    let name = utf16_array_to_string(unsafe { &(*logfont).lfFaceName });
    let name = name.trim();
    if !name.is_empty() && !name.starts_with('@') {
        fonts.insert(name.to_string());
    }
    1
}

fn utf16_array_to_string(value: &[u16]) -> String {
    let end = value.iter().position(|ch| *ch == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn font_sort_key(value: &str) -> (u8, String) {
    let priority = match value {
        "Microsoft YaHei UI" => 0,
        "Microsoft YaHei" => 1,
        "Microsoft JhengHei UI" => 2,
        "Microsoft JhengHei" => 3,
        "DengXian" => 4,
        "SimSun" => 5,
        "SimHei" => 6,
        "FangSong" => 7,
        "KaiTi" => 8,
        "Yu Gothic UI" => 9,
        "Yu Gothic" => 10,
        "Meiryo UI" => 11,
        "Meiryo" => 12,
        "MS Gothic" => 13,
        "MS Mincho" => 14,
        _ => 20,
    };
    (priority, value.to_lowercase())
}

fn is_cjk_font_name(value: &str) -> bool {
    let lower = value.to_lowercase();
    [
        "yahei",
        "jhenghei",
        "mingliu",
        "dengxian",
        "simsun",
        "nsimsun",
        "simhei",
        "fangsong",
        "kaiti",
        "kai",
        "heiti",
        "song",
        "yu gothic",
        "yu mincho",
        "meiryo",
        "ms gothic",
        "ms pgothic",
        "ms mincho",
        "ms pmincho",
        "noto sans cjk",
        "noto serif cjk",
        "source han",
        "sarasa",
        "hiragino",
        "kozuka",
        "ud digikyo",
        "biz ud",
        "malgun",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_likely_english_only_font(value: &str) -> bool {
    let lower = value.to_lowercase();
    [
        "arial",
        "bahnschrift",
        "calibri",
        "cambria",
        "candara",
        "cascadia",
        "comic sans",
        "consolas",
        "constantia",
        "corbel",
        "courier",
        "franklin gothic",
        "gabriola",
        "georgia",
        "impact",
        "lucida",
        "marlett",
        "palatino",
        "segoe",
        "symbol",
        "tahoma",
        "times new roman",
        "trebuchet",
        "verdana",
        "webdings",
        "wingdings",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn font_supports_cjk_glyphs(family: &str) -> bool {
    if family.trim().is_empty() || is_likely_english_only_font(family) {
        return false;
    }
    unsafe {
        let hdc = GetDC(None);
        if hdc.0.is_null() {
            return false;
        }
        let appearance = HudFontAppearance {
            family: family.trim().to_string(),
            height_px: 32,
            weight: 400,
            background_alpha_percent: 100,
        };
        let font = create_preview_font(&appearance);
        if font.is_invalid() {
            let _ = ReleaseDC(None, hdc);
            return false;
        }
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let sample = "中文日本語かなカナ";
        let sample_wide = sample.encode_utf16().collect::<Vec<_>>();
        let mut glyphs = vec![0u16; sample_wide.len()];
        let result = GetGlyphIndicesW(
            hdc,
            PCWSTR(sample_wide.as_ptr()),
            sample_wide.len() as i32,
            glyphs.as_mut_ptr(),
            GGI_MARK_NONEXISTING_GLYPHS,
        );
        let _ = SelectObject(hdc, old_font);
        let _ = DeleteObject(font.into());
        let _ = ReleaseDC(None, hdc);
        result != u32::MAX && glyphs.iter().filter(|glyph| **glyph != 0xffff).count() >= 4
    }
}

#[derive(Clone, Copy)]
enum ComboKind {
    Editable,
    DropdownList,
}

unsafe fn create_combo_row(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    label: &str,
    options: &[String],
    kind: ComboKind,
    explanation: &str,
) -> Result<HWND> {
    let _ = unsafe { create_row_label(parent, font_targets, y, label)? };
    let combo = unsafe {
        create_combo(
            parent,
            CONTROL_X,
            y - 2,
            INPUT_WIDTH,
            COMBO_DROPDOWN_HEIGHT,
            options,
            kind,
        )?
    };
    let hint = unsafe {
        create_control(
            parent,
            "STATIC",
            explanation,
            HINT_X,
            y,
            HINT_WIDTH,
            38,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(combo);
    font_targets.push(hint);
    Ok(combo)
}

unsafe fn create_row_label(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    text: &str,
) -> Result<HWND> {
    let label = unsafe {
        create_control(
            parent,
            "STATIC",
            text,
            12,
            y + 3,
            LABEL_WIDTH,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(label);
    Ok(label)
}

unsafe fn create_control(
    parent: HWND,
    class: &str,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
) -> Result<HWND> {
    let class = HSTRING::from(class);
    let text = HSTRING::from(text);
    let dpi = unsafe { GetDpiForWindow(parent) }.max(96);
    let instance = unsafe { GetModuleHandleW(None) }
        .map(|module| HINSTANCE(module.0))
        .map_err(|error| anyhow!("get module handle for HUD font panel control failed: {error}"))?;
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(text.as_ptr()),
            child_safe_style(style),
            scale_px(x, dpi),
            scale_px(y, dpi),
            scale_px(width, dpi),
            scale_px(height, dpi),
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create HUD font panel control failed: {error}"))
}

unsafe fn create_combo(
    parent: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    options: &[String],
    kind: ComboKind,
) -> Result<HWND> {
    let combo_style = match kind {
        ComboKind::Editable => CBS_DROPDOWN,
        ComboKind::DropdownList => CBS_DROPDOWNLIST,
    };
    let hwnd = unsafe {
        create_control(
            parent,
            "COMBOBOX",
            "",
            x,
            y,
            width,
            height,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_TABSTOP.0
                    | WS_VSCROLL.0
                    | combo_style as u32
                    | CBS_NOINTEGRALHEIGHT as u32,
            ),
        )?
    };
    for option in options {
        unsafe { combo_add_string(hwnd, option) };
    }
    unsafe {
        let _ = SendMessageW(
            hwnd,
            CB_SETMINVISIBLE,
            Some(WPARAM(COMBO_MIN_VISIBLE_ROWS)),
            Some(LPARAM(0)),
        );
        let _ = SendMessageW(
            hwnd,
            CB_SETDROPPEDWIDTH,
            Some(WPARAM(
                scale_px(width.max(260), GetDpiForWindow(parent).max(96)) as usize,
            )),
            Some(LPARAM(0)),
        );
    }
    Ok(hwnd)
}

unsafe fn combo_add_string(hwnd: HWND, value: &str) {
    let value = HSTRING::from(value);
    unsafe {
        let _ = SendMessageW(
            hwnd,
            CB_ADDSTRING,
            Some(WPARAM(0)),
            Some(LPARAM(value.as_ptr() as isize)),
        );
    }
}

unsafe fn create_panel_font(family: &str, point_size: i32, weight: i32, dpi: u32) -> HFONT {
    let font_family = HSTRING::from(family);
    let height = -((point_size.max(1) * dpi.max(96) as i32 + 36) / 72);
    unsafe {
        CreateFontW(
            height,
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
            DEFAULT_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(font_family.as_ptr()),
        )
    }
}

unsafe fn create_preview_font(appearance: &HudFontAppearance) -> HFONT {
    unsafe {
        CreateFontW(
            -appearance.height_px.max(1).abs(),
            0,
            0,
            0,
            appearance.weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_OUTLINE_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(HSTRING::from(appearance.family.as_str()).as_ptr()),
        )
    }
}

unsafe fn apply_panel_font(hwnd: HWND, font: HFONT) {
    unsafe {
        let _ = SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

fn layout_panel(hwnd: HWND) {
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        let margin = scale_px(MARGIN, dpi);
        let width = (rect.right - rect.left - margin * 2).max(scale_px(320, dpi));
        let preview_height = scale_px(PREVIEW_HEIGHT, dpi);
        let row_height = scale_px(CONTROL_ROW_HEIGHT, dpi);
        let preview_y = margin;
        let status_y = preview_y + preview_height + scale_px(10, dpi);
        let rows_y = status_y + scale_px(32, dpi);
        let _ = MoveWindow(
            panel_preview_hwnd(),
            margin,
            preview_y,
            width,
            preview_height,
            true,
        );
        let _ = MoveWindow(
            panel_status_hwnd(),
            margin,
            status_y,
            width,
            scale_px(24, dpi),
            true,
        );
        let _ = MoveWindow(
            panel_family_hwnd(),
            scale_px(CONTROL_X, dpi),
            rows_y,
            scale_px(INPUT_WIDTH, dpi),
            scale_px(COMBO_DROPDOWN_HEIGHT, dpi),
            true,
        );
        let _ = MoveWindow(
            panel_size_hwnd(),
            scale_px(CONTROL_X, dpi),
            rows_y + row_height,
            scale_px(INPUT_WIDTH, dpi),
            scale_px(COMBO_DROPDOWN_HEIGHT, dpi),
            true,
        );
        let _ = MoveWindow(
            panel_weight_hwnd(),
            scale_px(CONTROL_X, dpi),
            rows_y + row_height * 2,
            scale_px(INPUT_WIDTH, dpi),
            scale_px(COMBO_DROPDOWN_HEIGHT, dpi),
            true,
        );
        let _ = MoveWindow(
            panel_alpha_hwnd(),
            scale_px(CONTROL_X, dpi),
            rows_y + row_height * 3,
            scale_px(INPUT_WIDTH, dpi),
            scale_px(COMBO_DROPDOWN_HEIGHT, dpi),
            true,
        );
        let _ = MoveWindow(
            panel_reset_button(),
            margin,
            rows_y + row_height * 4 + scale_px(4, dpi),
            scale_px(110, dpi),
            scale_px(30, dpi),
            true,
        );
        let _ =
            windows::Win32::Graphics::Gdi::InvalidateRect(Some(panel_preview_hwnd()), None, true);
    }
}

fn show_and_refresh() {
    let hwnd = HUD_FONT_PANEL_STATE.with(|stored| stored.borrow().as_ref().map(|state| state.hwnd));
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        refresh_from_hud();
        info!("HUD font panel opened from tray menu");
    }
}

fn refresh_from_hud() {
    let appearance = HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.hud.font_appearance())
    });
    if let Some(appearance) = appearance {
        set_controls_from_appearance(&appearance);
        apply_preview_appearance(&appearance);
        HUD_FONT_PANEL_STATE.with(|stored| {
            if let Some(state) = stored.borrow_mut().as_mut() {
                state.appearance = appearance.clone();
            }
        });
        set_status_text("已读取当前 HUD 外观。修改会直接保存并更新 live HUD。");
    }
}

fn set_controls_from_appearance(appearance: &HudFontAppearance) {
    let handles = HUD_FONT_PANEL_STATE.with(|stored| {
        stored.borrow_mut().as_mut().map(|state| {
            state.updating_controls.set(true);
            (
                state.family_hwnd,
                state.size_hwnd,
                state.weight_hwnd,
                state.alpha_hwnd,
            )
        })
    });
    let Some((family_hwnd, size_hwnd, weight_hwnd, alpha_hwnd)) = handles else {
        return;
    };
    select_combo_text_or_set(family_hwnd, &appearance.family);
    set_window_text(size_hwnd, &appearance.height_px.to_string());
    set_window_text(weight_hwnd, &appearance.weight.to_string());
    set_window_text(alpha_hwnd, &appearance.background_alpha_percent.to_string());
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.appearance = appearance.clone();
            state.updating_controls.set(false);
        }
    });
}

fn refresh_preview_from_controls() {
    match parse_appearance_from_controls() {
        Ok(appearance) => {
            HUD_FONT_PANEL_STATE.with(|stored| {
                if let Some(state) = stored.borrow_mut().as_mut() {
                    state.appearance = appearance.clone();
                }
            });
            apply_preview_appearance(&appearance);
            set_status_text("预览已更新，修改会自动保存并同步到 live HUD。");
        }
        Err(error) => set_status_text(&format!("预览更新失败：{error}")),
    }
}

fn apply_preview_appearance(appearance: &HudFontAppearance) {
    let handles = HUD_FONT_PANEL_STATE.with(|stored| {
        let mut stored = stored.borrow_mut();
        let state = stored.as_mut()?;
        unsafe {
            let new_font = create_preview_font(appearance);
            if new_font.is_invalid() {
                warn!(?appearance, "create HUD font preview font failed");
                return None;
            }
            let old_font = std::mem::replace(&mut state.preview_font, new_font);
            Some((state.preview_hwnd, old_font))
        }
    });
    if let Some((preview_hwnd, old_font)) = handles {
        unsafe {
            let _ = DeleteObject(old_font.into());
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(preview_hwnd), None, true);
        }
    }
}

fn parse_appearance_from_controls() -> Result<HudFontAppearance> {
    HUD_FONT_PANEL_STATE.with(|stored| {
        let state = stored.borrow();
        let Some(state) = state.as_ref() else {
            return Err(anyhow!("HUD font panel state not ready"));
        };
        let family = combo_text_or_selection(state.family_hwnd)
            .trim()
            .to_string();
        if family.is_empty() {
            return Err(anyhow!("字体不能为空"));
        }
        let height_px = parse_positive_i32_value(combo_text_or_selection(state.size_hwnd), "字号")?;
        let weight =
            parse_i32_value(combo_text_or_selection(state.weight_hwnd), "字重")?.clamp(100, 900);
        let background_alpha_percent =
            parse_u8_value(combo_text_or_selection(state.alpha_hwnd), "背景不透明度")?
                .clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100);
        Ok(HudFontAppearance {
            family,
            height_px,
            weight,
            background_alpha_percent,
        })
    })
}

fn apply_appearance_from_controls() {
    let result = parse_appearance_from_controls();
    match result {
        Ok(appearance) => {
            let applied = HUD_FONT_PANEL_STATE.with(|stored| {
                if let Some(state) = stored.borrow().as_ref() {
                    state.hud.apply_font_appearance(&appearance)
                } else {
                    Err(anyhow!("HUD font panel state not ready"))
                }
            });
            match applied {
                Ok(()) => {
                    HUD_FONT_PANEL_STATE.with(|stored| {
                        if let Some(state) = stored.borrow_mut().as_mut() {
                            state.appearance = appearance.clone();
                        }
                    });
                    set_status_text("已保存并实时更新到 live HUD。");
                }
                Err(error) => set_status_text(&format!(
                    "未保存：{error}。live HUD 继续使用上一次可用字体，听写不受影响。"
                )),
            }
        }
        Err(error) => set_status_text(&format!("保存失败：{error}")),
    }
}

fn schedule_apply() {
    match parse_appearance_from_controls()
        .and_then(|appearance| validate_font_appearance_for_hud(&appearance).map(|_| ()))
    {
        Ok(()) => {}
        Err(error) => {
            set_status_text(&format!(
                "未保存：{error}。live HUD 继续使用上一次可用字体，听写不受影响。"
            ));
            return;
        }
    }
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            if state.updating_controls.get() {
                return;
            }
            unsafe {
                let _ = KillTimer(Some(state.hwnd), HUD_FONT_PANEL_TIMER_ID);
                let _ = SetTimer(
                    Some(state.hwnd),
                    HUD_FONT_PANEL_TIMER_ID,
                    HUD_FONT_PANEL_DEBOUNCE_MS,
                    None,
                );
            }
        }
    });
}

fn flush_apply_timer() {
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = KillTimer(Some(state.hwnd), HUD_FONT_PANEL_TIMER_ID);
            }
        }
    });
    apply_appearance_from_controls();
}

fn reset_to_default() {
    let default = HudFontAppearance::default();
    set_controls_from_appearance(&default);
    apply_preview_appearance(&default);
    apply_appearance_from_controls();
}

#[derive(Clone, Copy)]
enum PanelCommandAction {
    None,
    Reset,
    DropdownOpen,
    SelectionPreviewOnly,
    SelectionPreviewSchedule,
    PreviewSchedule,
}

fn handle_command(control: HWND, notification: u16) {
    let action = HUD_FONT_PANEL_STATE.with(|stored| {
        let stored = stored.borrow();
        let Some(state) = stored.as_ref() else {
            return PanelCommandAction::None;
        };
        if state.updating_controls.get() {
            return PanelCommandAction::None;
        }
        let notification = u32::from(notification);
        if control == state.reset_button {
            return if notification == BN_CLICKED {
                PanelCommandAction::Reset
            } else {
                PanelCommandAction::None
            };
        }
        if !is_font_control(state, control) {
            return PanelCommandAction::None;
        }
        if control == state.family_hwnd && notification == CBN_DROPDOWN {
            state.family_dropdown_open.set(true);
            return PanelCommandAction::DropdownOpen;
        }
        if control == state.family_hwnd && notification == CBN_CLOSEUP {
            state.family_dropdown_open.set(false);
            return PanelCommandAction::SelectionPreviewSchedule;
        }
        if control == state.family_hwnd && notification == CBN_SELCHANGE {
            return if state.family_dropdown_open.get() {
                PanelCommandAction::SelectionPreviewOnly
            } else {
                PanelCommandAction::SelectionPreviewSchedule
            };
        }
        if notification == CBN_SELCHANGE {
            return PanelCommandAction::SelectionPreviewSchedule;
        }
        if notification == CBN_EDITCHANGE || notification == EN_CHANGE {
            return PanelCommandAction::PreviewSchedule;
        }
        PanelCommandAction::None
    });

    match action {
        PanelCommandAction::None => {}
        PanelCommandAction::Reset => reset_to_default(),
        PanelCommandAction::DropdownOpen => {}
        PanelCommandAction::SelectionPreviewOnly => {
            sync_combo_selection_to_text(control);
            refresh_preview_from_controls();
        }
        PanelCommandAction::SelectionPreviewSchedule => {
            sync_combo_selection_to_text(control);
            refresh_preview_from_controls();
            schedule_apply();
        }
        PanelCommandAction::PreviewSchedule => {
            refresh_preview_from_controls();
            schedule_apply();
        }
    }
}

fn is_font_control(state: &HudFontPanelState, control: HWND) -> bool {
    [
        state.family_hwnd,
        state.size_hwnd,
        state.weight_hwnd,
        state.alpha_hwnd,
    ]
    .contains(&control)
}

fn set_status_text(text: &str) {
    HUD_FONT_PANEL_STATE.with(|stored| {
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

fn select_combo_text_or_set(hwnd: HWND, text: &str) {
    let wide = HSTRING::from(text);
    unsafe {
        let result = SendMessageW(
            hwnd,
            CB_SELECTSTRING,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(wide.as_ptr() as isize)),
        );
        if result.0 == CB_ERR {
            combo_add_string(hwnd, text);
            let _ = SendMessageW(
                hwnd,
                CB_SELECTSTRING,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
    }
}

fn get_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let read = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..read as usize])
    }
}

fn combo_text_or_selection(hwnd: HWND) -> String {
    let text = get_window_text(hwnd);
    if !text.trim().is_empty() {
        return text;
    }
    combo_selected_text(hwnd).unwrap_or_default()
}

fn sync_combo_selection_to_text(hwnd: HWND) {
    let Some(text) = combo_selected_text(hwnd) else {
        return;
    };
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            state.updating_controls.set(true);
        }
    });
    set_window_text(hwnd, &text);
    HUD_FONT_PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            state.updating_controls.set(false);
        }
    });
}

fn combo_selected_text(hwnd: HWND) -> Option<String> {
    unsafe {
        let selected = SendMessageW(hwnd, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0;
        if selected == CB_ERR || selected < 0 {
            return None;
        }
        let len = SendMessageW(
            hwnd,
            CB_GETLBTEXTLEN,
            Some(WPARAM(selected as usize)),
            Some(LPARAM(0)),
        )
        .0;
        if len == CB_ERR || len < 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = SendMessageW(
            hwnd,
            CB_GETLBTEXT,
            Some(WPARAM(selected as usize)),
            Some(LPARAM(buffer.as_mut_ptr() as isize)),
        )
        .0;
        if copied == CB_ERR || copied < 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..copied as usize]))
    }
}

fn parse_i32_value(raw: String, name: &str) -> Result<i32> {
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return Err(anyhow!("{name} 不能为空"));
    }
    raw.parse::<i32>()
        .map_err(|error| anyhow!("{name} 不是有效整数：{error}"))
}

fn parse_positive_i32_value(raw: String, name: &str) -> Result<i32> {
    let value = parse_i32_value(raw, name)?;
    if value <= 0 {
        return Err(anyhow!("{name} 必须是正整数"));
    }
    Ok(value)
}

fn parse_u8_value(raw: String, name: &str) -> Result<u8> {
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return Err(anyhow!("{name} 不能为空"));
    }
    raw.parse::<u8>()
        .map_err(|error| anyhow!("{name} 不是有效整数：{error}"))
}

fn scale_px(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

fn child_safe_style(style: WINDOW_STYLE) -> WINDOW_STYLE {
    if style.0 & WS_CHILD.0 != 0 {
        WINDOW_STYLE(style.0 | WS_CLIPSIBLINGS.0)
    } else {
        style
    }
}

fn hiword(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}

fn panel_preview_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.preview_hwnd)
            .unwrap_or_default()
    })
}

fn panel_status_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.status_hwnd)
            .unwrap_or_default()
    })
}

fn panel_family_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.family_hwnd)
            .unwrap_or_default()
    })
}

fn panel_size_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.size_hwnd)
            .unwrap_or_default()
    })
}

fn panel_weight_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.weight_hwnd)
            .unwrap_or_default()
    })
}

fn panel_alpha_hwnd() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.alpha_hwnd)
            .unwrap_or_default()
    })
}

fn panel_reset_button() -> HWND {
    HUD_FONT_PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| state.reset_button)
            .unwrap_or_default()
    })
}

fn paint_preview(hwnd: HWND) {
    unsafe {
        let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.0.is_null() {
            return;
        }
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            let _ = EndPaint(hwnd, &ps);
            return;
        }
        paint_checkerboard(hdc, rect);
        let snapshot = HUD_FONT_PANEL_STATE.with(|stored| {
            stored
                .borrow()
                .as_ref()
                .map(|state| (state.appearance.clone(), state.preview_font))
        });
        if let Some((appearance, font)) = snapshot {
            let pill = preview_pill_rect(hdc, rect, font);
            let fill_color = appearance_alpha_color(&appearance);
            let fill = CreateSolidBrush(fill_color);
            let _ = FillRect(hdc, &pill, fill);
            let border = CreatePen(PS_SOLID, 1, darken_preview_fill(fill_color));
            let pill_brush = CreateSolidBrush(fill_color);
            let old_pen = SelectObject(hdc, HGDIOBJ(border.0));
            let old_brush = SelectObject(hdc, HGDIOBJ(pill_brush.0));
            let radius = PREVIEW_HUD_CORNER_RADIUS;
            let _ = RoundRect(
                hdc,
                pill.left,
                pill.top,
                pill.right,
                pill.bottom,
                radius,
                radius,
            );
            let _ = SelectObject(hdc, old_pen);
            let _ = SelectObject(hdc, old_brush);
            let _ = DeleteObject(border.into());
            let _ = DeleteObject(fill.into());
            let _ = DeleteObject(pill_brush.into());
            paint_preview_text(hdc, pill, font, &appearance);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn paint_preview_text(hdc: HDC, pill: RECT, font: HFONT, appearance: &HudFontAppearance) {
    unsafe {
        if font.is_invalid() {
            return;
        }
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, appearance_text_color(appearance));
        let mut text_rect = RECT {
            left: pill.left + PREVIEW_HUD_PADDING_X,
            top: pill.top + PREVIEW_HUD_PADDING_Y,
            right: pill.right - PREVIEW_HUD_PADDING_X,
            bottom: pill.bottom - PREVIEW_HUD_PADDING_Y,
        };
        let mut text = PREVIEW_SAMPLE_TEXT.encode_utf16().collect::<Vec<_>>();
        let _ = DrawTextW(
            hdc,
            text.as_mut_slice(),
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old_font);
    }
}

fn preview_pill_rect(hdc: HDC, rect: RECT, font: HFONT) -> RECT {
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let available_width = (width - 32).max(PREVIEW_HUD_MIN_WIDTH);
    let max_hud_width = PREVIEW_HUD_MAX_WIDTH
        .min(available_width)
        .max(PREVIEW_HUD_MIN_WIDTH);
    let max_text_width = (max_hud_width - PREVIEW_HUD_PADDING_X * 2).max(1);
    let (text_width, text_height) = measure_preview_text(hdc, font, max_text_width);
    let pill_width =
        (text_width + PREVIEW_HUD_PADDING_X * 2).clamp(PREVIEW_HUD_MIN_WIDTH, max_hud_width);
    let pill_height = (text_height + PREVIEW_HUD_PADDING_Y * 2).max(PREVIEW_HUD_MIN_HEIGHT);
    let left = (width - pill_width) / 2;
    let top = (height - pill_height) / 2;
    RECT {
        left,
        top,
        right: left + pill_width,
        bottom: top + pill_height,
    }
}

fn measure_preview_text(hdc: HDC, font: HFONT, max_text_width: i32) -> (i32, i32) {
    if font.is_invalid() {
        return (PREVIEW_HUD_MIN_WIDTH, PREVIEW_HUD_MIN_HEIGHT);
    }
    unsafe {
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let mut text_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut text = PREVIEW_SAMPLE_TEXT.encode_utf16().collect::<Vec<_>>();
        let _ = DrawTextW(
            hdc,
            text.as_mut_slice(),
            &mut text_rect,
            DT_CALCRECT | DT_CENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old_font);
        (
            (text_rect.right - text_rect.left).clamp(1, max_text_width.max(1)),
            (text_rect.bottom - text_rect.top).max(1),
        )
    }
}

fn paint_checkerboard(hdc: HDC, rect: RECT) {
    unsafe {
        let tile = 18;
        for y in (rect.top..rect.bottom).step_by(tile as usize) {
            for x in (rect.left..rect.right).step_by(tile as usize) {
                let is_dark = ((x / tile) + (y / tile)) % 2 == 0;
                let color = if is_dark {
                    PREVIEW_BG_DARK
                } else {
                    PREVIEW_BG_LIGHT
                };
                let brush = CreateSolidBrush(color);
                let block = RECT {
                    left: x,
                    top: y,
                    right: (x + tile).min(rect.right),
                    bottom: (y + tile).min(rect.bottom),
                };
                let _ = FillRect(hdc, &block, brush);
                let _ = DeleteObject(brush.into());
            }
        }
    }
}

fn appearance_alpha_color(appearance: &HudFontAppearance) -> COLORREF {
    let alpha = appearance
        .background_alpha_percent
        .clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100);
    let fg = PREVIEW_PILL_BG;
    blend_with_sample_background(fg, PREVIEW_BG_DARK, alpha)
}

fn appearance_text_color(appearance: &HudFontAppearance) -> COLORREF {
    let alpha = appearance
        .background_alpha_percent
        .clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100);
    blend_with_sample_background(PREVIEW_TEXT_COLOR, PREVIEW_BG_DARK, alpha)
}

fn blend_with_sample_background(fg: COLORREF, bg: COLORREF, alpha_percent: u8) -> COLORREF {
    let alpha = alpha_percent.clamp(1, 100) as u32;
    let inv = 100u32.saturating_sub(alpha);
    let (fr, fg_c, fb) = colorref_components(fg);
    let (br, bg_c, bb) = colorref_components(bg);
    let mix = |a: u8, b: u8| -> u8 {
        ((a as u32 * alpha + b as u32 * inv + 50) / 100).clamp(0, 255) as u8
    };
    colorref_rgb(mix(fr, br), mix(fg_c, bg_c), mix(fb, bb))
}

fn darken_preview_fill(color: COLORREF) -> COLORREF {
    let (r, g, b) = colorref_components(color);
    colorref_rgb(r / 2, g / 2, b / 2)
}

fn colorref_components(color: COLORREF) -> (u8, u8, u8) {
    let value = color.0;
    (
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
    )
}

fn colorref_rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

#[cfg(test)]
mod tests {
    use super::{
        COMBO_DROPDOWN_HEIGHT, font_sort_key, installed_font_families, is_cjk_font_name,
        is_likely_english_only_font,
    };

    #[test]
    fn installed_font_families_reads_windows_font_library() {
        let fonts = installed_font_families("Microsoft YaHei");
        assert!(
            fonts.len() > 5,
            "expected filtered CJK Windows font library, got {} fonts: {:?}",
            fonts.len(),
            fonts
        );
        assert!(fonts.iter().any(|font| font == "Microsoft YaHei"));
        assert!(fonts.iter().all(|font| !font.starts_with('@')));
        assert!(!fonts.iter().any(|font| font == "Consolas"));
        assert!(!fonts.iter().any(|font| font == "Cascadia Code"));
        assert!(!fonts.iter().any(|font| font == "Segoe UI"));
    }

    #[test]
    fn font_sort_keeps_common_hud_fonts_first() {
        assert!(font_sort_key("Microsoft YaHei UI") < font_sort_key("Segoe UI"));
        assert!(font_sort_key("Meiryo") < font_sort_key("Consolas"));
    }

    #[test]
    fn cjk_font_filter_rejects_common_english_only_fonts() {
        assert!(is_cjk_font_name("Microsoft YaHei"));
        assert!(is_cjk_font_name("Yu Gothic UI"));
        assert!(is_likely_english_only_font("Consolas"));
        assert!(is_likely_english_only_font("Segoe UI"));
    }

    #[test]
    fn combo_dropdown_height_keeps_list_visible() {
        assert!(COMBO_DROPDOWN_HEIGHT >= 360);
    }
}
