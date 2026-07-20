use std::cell::{Cell, RefCell};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use serde_json::Value;
use tracing::{info, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, DeleteObject, FF_DONTCARE, GetSysColorBrush, HFONT, OUT_OUTLINE_PRECIS,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, ES_WANTRETURN,
    GetClientRect, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW, KillTimer, LoadCursorW,
    MINMAXINFO, MSG, MoveWindow, PM_REMOVE, PeekMessageW, RegisterClassW, SB_VERT, SCROLLINFO,
    SIF_PAGE, SIF_POS, SIF_RANGE, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOZORDER,
    SendMessageW, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_GETMINMAXINFO,
    WM_MOUSEWHEEL, WM_SETFONT, WM_SIZE, WM_TIMER, WM_VSCROLL, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::asr_pool::AsrSessionPool;
use crate::modes::{InputMode, ModeStore};

const PANEL_WIDTH: i32 = 1180;
const PANEL_HEIGHT: i32 = 980;
const MIN_PANEL_WIDTH: i32 = 760;
const MIN_PANEL_HEIGHT: i32 = 520;
const BASE_DPI: u32 = 96;
const TOP_MARGIN: i32 = 12;
const DISPLAY_HEIGHT: i32 = 145;
const STATUS_HEIGHT: i32 = 42;
const BUTTON_HEIGHT: i32 = 30;
const PARAM_CONTENT_WIDTH: i32 = 1120;
const PARAM_ROW_STEP: i32 = 54;
const PARAM_CHECKBOX_STEP: i32 = 48;
const AUTO_APPLY_TIMER_ID: usize = 1;
const AUTO_APPLY_DEBOUNCE_MS: u32 = 650;
const WHEEL_DELTA_UNITS: i32 = 120;
const SCROLL_LINE_PX: i32 = 48;

const BS_AUTOCHECKBOX_STYLE: u32 = 0x0000_0003;
const CBS_DROPDOWN_STYLE: u32 = 0x0000_0002;
const CB_ADDSTRING_MSG: u32 = 0x0143;
const BM_GETCHECK_MSG: u32 = 0x00F0;
const BM_SETCHECK_MSG: u32 = 0x00F1;
const BST_CHECKED_VALUE: usize = 1;
const BN_CLICKED_CODE: u16 = 0;
const CBN_SELCHANGE_CODE: u16 = 1;
const CBN_EDITCHANGE_CODE: u16 = 5;
const EN_CHANGE_CODE: u16 = 0x0300;
const SB_LINEUP_CODE: u16 = 0;
const SB_LINEDOWN_CODE: u16 = 1;
const SB_PAGEUP_CODE: u16 = 2;
const SB_PAGEDOWN_CODE: u16 = 3;
const SB_THUMBPOSITION_CODE: u16 = 4;
const SB_THUMBTRACK_CODE: u16 = 5;

#[derive(Clone)]
pub struct DebugPanelController {
    tx: mpsc::Sender<DebugPanelCommand>,
    enabled: Arc<AtomicBool>,
}

enum DebugPanelCommand {
    Show,
    Display { text: String, status: String },
    Shutdown,
}

struct DebugPanelState {
    endpoint_url: String,
    modes: ModeStore,
    asr_sessions: AsrSessionPool,
    enabled: Arc<AtomicBool>,
    client: Client,
    hwnd: HWND,
    display_hwnd: HWND,
    status_hwnd: HWND,
    parakeet_button: HWND,
    whisper_button: HWND,
    refresh_button: HWND,
    reset_button: HWND,
    params_viewport_hwnd: HWND,
    params_content_hwnd: HWND,
    params_content_height: i32,
    params_scroll_y: Cell<i32>,
    updating_controls: Cell<bool>,
    auto_apply_pending: Cell<bool>,
    parakeet_language_hwnd: HWND,
    parakeet_punctuation_hwnd: HWND,
    parakeet_verbatim_hwnd: HWND,
    parakeet_boost_enabled_hwnd: HWND,
    parakeet_boost_hwnd: HWND,
    parakeet_legacy_hwnd: HWND,
    parakeet_partial_wait_hwnd: HWND,
    endpoint_start_history_hwnd: HWND,
    endpoint_start_threshold_hwnd: HWND,
    endpoint_stop_history_hwnd: HWND,
    endpoint_stop_threshold_hwnd: HWND,
    endpoint_stop_history_eou_hwnd: HWND,
    endpoint_stop_threshold_eou_hwnd: HWND,
    whisper_language_hwnd: HWND,
    whisper_punctuation_hwnd: HWND,
    whisper_verbatim_hwnd: HWND,
    whisper_min_audio_hwnd: HWND,
    whisper_min_rms_hwnd: HWND,
    hotwords_hwnd: HWND,
    ui_font: HFONT,
    display_font: HFONT,
}

thread_local! {
    static PANEL_STATE: RefCell<Option<DebugPanelState>> = const { RefCell::new(None) };
}

impl DebugPanelController {
    pub fn start(
        endpoint_url: String,
        modes: ModeStore,
        asr_sessions: AsrSessionPool,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<DebugPanelCommand>();
        let enabled = Arc::new(AtomicBool::new(false));
        let thread_enabled = Arc::clone(&enabled);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        thread::spawn(move || {
            let result = run_debug_panel_thread(
                endpoint_url,
                modes,
                asr_sessions,
                thread_enabled,
                shutdown,
                rx,
                ready_tx,
            );
            if let Err(error) = result {
                warn!(error = %error, "debug panel thread failed");
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("debug panel thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { tx, enabled })
    }

    pub fn open(&self) {
        self.enabled.store(true, Ordering::Relaxed);
        let _ = self.tx.send(DebugPanelCommand::Show);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn display_result(&self, text: impl Into<String>, status: impl Into<String>) {
        let _ = self.tx.send(DebugPanelCommand::Display {
            text: text.into(),
            status: status.into(),
        });
    }
}

impl Drop for DebugPanelController {
    fn drop(&mut self) {
        let _ = self.tx.send(DebugPanelCommand::Shutdown);
    }
}

fn run_debug_panel_thread(
    endpoint_url: String,
    modes: ModeStore,
    asr_sessions: AsrSessionPool,
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: mpsc::Receiver<DebugPanelCommand>,
    ready_tx: mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(6))
        .no_proxy()
        .build()?;
    unsafe {
        let instance =
            GetModuleHandleW(None).map_err(|error| anyhow!("get module handle failed: {error}"))?;
        let instance = HINSTANCE(instance.0);
        register_debug_panel_class(instance)?;
        register_debug_scroll_class(instance)?;
        register_debug_content_class(instance)?;
        let hwnd = create_debug_panel_window(instance)?;
        let state =
            create_debug_panel_controls(hwnd, endpoint_url, modes, asr_sessions, enabled, client)?;
        PANEL_STATE.with(|stored| {
            *stored.borrow_mut() = Some(state);
        });
        PANEL_STATE.with(|stored| {
            if let Some(state) = stored.borrow().as_ref() {
                layout_debug_panel(state);
            }
        });
    }
    let _ = ready_tx.send(Ok(()));
    refresh_settings_controls();
    info!("debug panel thread started");

    while !shutdown.load(Ordering::Relaxed) {
        while pump_messages()? {}
        while let Ok(command) = rx.try_recv() {
            match command {
                DebugPanelCommand::Show => unsafe {
                    PANEL_STATE.with(|stored| {
                        if let Some(state) = stored.borrow().as_ref() {
                            let _ = ShowWindow(state.hwnd, SW_SHOW);
                        }
                    });
                    set_debug_mode_enabled(true);
                    refresh_settings_controls();
                },
                DebugPanelCommand::Display { text, status } => {
                    set_display_result(&text, &status);
                }
                DebugPanelCommand::Shutdown => {
                    destroy_panel_window();
                    return Ok(());
                }
            }
        }
        thread::sleep(Duration::from_millis(16));
    }
    destroy_panel_window();
    info!("debug panel thread stopped");
    Ok(())
}

unsafe fn register_debug_panel_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(debug_panel_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput2_debug_panel"),
        hCursor: cursor,
        hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn register_debug_scroll_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(debug_scroll_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput2_debug_scroll"),
        hCursor: cursor,
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn register_debug_content_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(debug_content_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput2_debug_content"),
        hCursor: cursor,
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_debug_panel_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!("ainput2 调试面板 {}", env!("CARGO_PKG_VERSION")));
    let dpi = unsafe { GetDpiForSystem() }.max(BASE_DPI);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput2_debug_panel"),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            scale_px(100, dpi),
            scale_px(80, dpi),
            scale_px(PANEL_WIDTH, dpi),
            scale_px(PANEL_HEIGHT, dpi),
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create debug panel window failed: {error}"))
}

unsafe fn create_debug_panel_controls(
    hwnd: HWND,
    endpoint_url: String,
    modes: ModeStore,
    asr_sessions: AsrSessionPool,
    enabled: Arc<AtomicBool>,
    client: Client,
) -> Result<DebugPanelState> {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(BASE_DPI);
    let mut font_targets = Vec::new();

    let display_hwnd = unsafe {
        create_control(
            hwnd,
            "EDIT",
            "",
            TOP_MARGIN,
            TOP_MARGIN,
            1140,
            DISPLAY_HEIGHT,
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
    let status_hwnd = unsafe {
        create_control(
            hwnd,
            "STATIC",
            "调试模式关闭。识别文本只显示在上方；状态/耗时显示在这里。参数改动会自动应用到下一次录音。",
            TOP_MARGIN,
            165,
            1140,
            STATUS_HEIGHT,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(status_hwnd);

    let parakeet_button =
        unsafe { create_button(hwnd, "Parakeet 流式", 12, 212, 145, BUTTON_HEIGHT)? };
    let whisper_button =
        unsafe { create_button(hwnd, "Whisper 非流式", 168, 212, 150, BUTTON_HEIGHT)? };
    let refresh_button = unsafe { create_button(hwnd, "刷新", 340, 212, 80, BUTTON_HEIGHT)? };
    let reset_button =
        unsafe { create_button(hwnd, "重置默认", 432, 212, 100, BUTTON_HEIGHT)? };
    for control in [
        parakeet_button,
        whisper_button,
        refresh_button,
        reset_button,
    ] {
        font_targets.push(control);
    }

    let params_viewport_hwnd = unsafe {
        create_control(
            hwnd,
            "ainput2_debug_scroll",
            "",
            TOP_MARGIN,
            255,
            1140,
            690,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_VSCROLL.0
                    | WS_CLIPCHILDREN.0
                    | WS_CLIPSIBLINGS.0,
            ),
        )?
    };
    let params_content_hwnd = unsafe {
        create_control(
            params_viewport_hwnd,
            "ainput2_debug_content",
            "",
            0,
            0,
            PARAM_CONTENT_WIDTH,
            900,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPCHILDREN.0 | WS_CLIPSIBLINGS.0),
        )?
    };

    let mut y = 12;
    let parakeet_title = unsafe {
        create_control(
            params_content_hwnd,
            "STATIC",
            "Parakeet 流式参数",
            12,
            y,
            240,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(parakeet_title);
    y += 34;

    let parakeet_language_hwnd = unsafe {
        create_combo_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "语言",
            &["zh-CN"],
            "zh-CN = 当前中文主路由。一般不改；错设可能导致中文/英文都掉识别。",
        )?
    };
    y += PARAM_ROW_STEP;
    let parakeet_punctuation_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "自动标点",
            "开：模型自己加逗号句号。关：更原始，标点更少，适合排查标点乱跳。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let parakeet_verbatim_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "verbatim 原文",
            "开：尽量保留原话，减少数字/格式改写；关：模型更可能把“一”改成“1”。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let parakeet_boost_enabled_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "启用热词",
            "开：热词参与识别。关：用于排查热词是否让流式变慢、变空或误吸附。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let parakeet_boost_hwnd = unsafe {
        create_combo_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "热词 boost",
            &["0", "20", "40", "60", "80", "100"],
            "0-100。40 是当前基线；越高越偏向热词，过高可能把相近中文误拉成英文。",
        )?
    };
    y += PARAM_ROW_STEP;
    let parakeet_legacy_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "旧变体",
            "把旧拼写/发音变体也加入热词。覆盖更广，但也更容易引入误识别。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let parakeet_partial_wait_hwnd = unsafe {
        create_combo_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "partial 等待秒",
            &["0.03", "0.06", "0.10", "0.20"],
            "流式轮询节奏。越小越快但更碎/请求更多；越大更稳但显示会慢。",
        )?
    };
    y += PARAM_ROW_STEP;

    let endpoint_title = unsafe {
        create_control(
            params_content_hwnd,
            "STATIC",
            "Endpointing / 断句参数（空白 = 使用上游默认）",
            12,
            y,
            390,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    let endpoint_hint = unsafe {
        create_control(
            params_content_hwnd,
            "STATIC",
            "主要调 stop_history：大一点更不容易过早截句，小一点结束更快但可能断句。threshold 类参数只在专门排查时动。",
            420,
            y,
            720,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(endpoint_title);
    font_targets.push(endpoint_hint);
    y += 40;
    let endpoint_start_history_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            12,
            "start_history",
        )?
    };
    let endpoint_start_threshold_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            310,
            "start_threshold",
        )?
    };
    let endpoint_stop_history_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            610,
            "stop_history",
        )?
    };
    y += 42;
    let endpoint_stop_threshold_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            12,
            "stop_threshold",
        )?
    };
    let endpoint_stop_history_eou_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            310,
            "stop_history_eou",
        )?
    };
    let endpoint_stop_threshold_eou_hwnd = unsafe {
        create_endpoint_edit(
            params_content_hwnd,
            &mut font_targets,
            y,
            610,
            "stop_threshold_eou",
        )?
    };
    y += 54;

    let whisper_title = unsafe {
        create_control(
            params_content_hwnd,
            "STATIC",
            "Whisper zh 非流式参数",
            12,
            y,
            240,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(whisper_title);
    y += 34;
    let whisper_language_hwnd = unsafe {
        create_combo_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "语言",
            &["zh", "en"],
            "zh = 中文优先。改成 en 会更偏英文，但中文混说可能明显变差。",
        )?
    };
    y += PARAM_ROW_STEP;
    let whisper_punctuation_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "自动标点",
            "开：非流式结果带标点。关：更像原始字幕，适合排查模型是否乱加句号。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let whisper_verbatim_hwnd = unsafe {
        create_checkbox_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "verbatim 原文",
            "开：尽量不做格式化。关：可能更顺，但数字/英文大小写会更不可控。",
        )?
    };
    y += PARAM_CHECKBOX_STEP;
    let whisper_min_audio_hwnd = unsafe {
        create_edit_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "最短音频秒",
            "低于这个时长直接跳过。调小会响应更短语音，也更容易把误触发发出去。",
        )?
    };
    y += PARAM_ROW_STEP;
    let whisper_min_rms_hwnd = unsafe {
        create_edit_row(
            params_content_hwnd,
            &mut font_targets,
            y,
            "最低音量 dBFS",
            "越接近 0 越严格，越低越容易收进轻声/噪音；当前 -64 偏宽松。",
        )?
    };
    y += PARAM_ROW_STEP;

    let hotword_title = unsafe {
        create_control(
            params_content_hwnd,
            "STATIC",
            "正式热词列表（每行一个，也可用逗号分隔；修改后自动写入正常模式会读取的 boost_phrases）",
            12,
            y,
            980,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    y += 30;
    let hotwords_hwnd = unsafe {
        create_control(
            params_content_hwnd,
            "EDIT",
            "",
            12,
            y,
            1068,
            100,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_VSCROLL.0
                    | WS_TABSTOP.0
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32,
            ),
        )?
    };
    y += 120;
    font_targets.push(hotword_title);
    font_targets.push(hotwords_hwnd);
    let params_content_height = y;

    let ui_font = unsafe { create_panel_font("Microsoft YaHei UI", 12, 500, dpi) };
    let display_font = unsafe { create_panel_font("Microsoft YaHei UI", 16, 500, dpi) };
    if ui_font.is_invalid() || display_font.is_invalid() {
        return Err(anyhow!("create debug panel font failed"));
    }
    for control in font_targets {
        unsafe { apply_panel_font(control, ui_font) };
    }
    unsafe {
        apply_panel_font(display_hwnd, display_font);
    }

    Ok(DebugPanelState {
        endpoint_url,
        modes,
        asr_sessions,
        enabled,
        client,
        hwnd,
        display_hwnd,
        status_hwnd,
        parakeet_button,
        whisper_button,
        refresh_button,
        reset_button,
        params_viewport_hwnd,
        params_content_hwnd,
        params_content_height,
        params_scroll_y: Cell::new(0),
        updating_controls: Cell::new(false),
        auto_apply_pending: Cell::new(false),
        parakeet_language_hwnd,
        parakeet_punctuation_hwnd,
        parakeet_verbatim_hwnd,
        parakeet_boost_enabled_hwnd,
        parakeet_boost_hwnd,
        parakeet_legacy_hwnd,
        parakeet_partial_wait_hwnd,
        endpoint_start_history_hwnd,
        endpoint_start_threshold_hwnd,
        endpoint_stop_history_hwnd,
        endpoint_stop_threshold_hwnd,
        endpoint_stop_history_eou_hwnd,
        endpoint_stop_threshold_eou_hwnd,
        whisper_language_hwnd,
        whisper_punctuation_hwnd,
        whisper_verbatim_hwnd,
        whisper_min_audio_hwnd,
        whisper_min_rms_hwnd,
        hotwords_hwnd,
        ui_font,
        display_font,
    })
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

unsafe fn create_combo_row(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    label: &str,
    options: &[&str],
    explanation: &str,
) -> Result<HWND> {
    unsafe { create_row_label(parent, font_targets, y, label)? };
    let combo = unsafe { create_combo(parent, 190, y - 2, 150, 160, options)? };
    let hint = unsafe {
        create_control(
            parent,
            "STATIC",
            explanation,
            360,
            y,
            740,
            42,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(combo);
    font_targets.push(hint);
    Ok(combo)
}

unsafe fn create_checkbox_row(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    label: &str,
    explanation: &str,
) -> Result<HWND> {
    let checkbox = unsafe {
        create_control(
            parent,
            "BUTTON",
            label,
            12,
            y - 2,
            170,
            28,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX_STYLE),
        )?
    };
    let hint = unsafe {
        create_control(
            parent,
            "STATIC",
            explanation,
            190,
            y,
            910,
            42,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(checkbox);
    font_targets.push(hint);
    Ok(checkbox)
}

unsafe fn create_edit_row(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    label: &str,
    explanation: &str,
) -> Result<HWND> {
    unsafe { create_row_label(parent, font_targets, y, label)? };
    let edit = unsafe { create_single_line_edit(parent, 190, y - 2, 150, 28)? };
    let hint = unsafe {
        create_control(
            parent,
            "STATIC",
            explanation,
            360,
            y,
            740,
            42,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(edit);
    font_targets.push(hint);
    Ok(edit)
}

unsafe fn create_endpoint_edit(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    x: i32,
    label: &str,
) -> Result<HWND> {
    let label_hwnd = unsafe {
        create_control(
            parent,
            "STATIC",
            label,
            x,
            y + 4,
            145,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    let edit = unsafe { create_single_line_edit(parent, x + 150, y, 105, 28)? };
    font_targets.push(label_hwnd);
    font_targets.push(edit);
    Ok(edit)
}

unsafe fn create_row_label(
    parent: HWND,
    font_targets: &mut Vec<HWND>,
    y: i32,
    text: &str,
) -> Result<()> {
    let label = unsafe {
        create_control(
            parent,
            "STATIC",
            text,
            12,
            y + 3,
            160,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    font_targets.push(label);
    Ok(())
}

unsafe fn create_single_line_edit(
    parent: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<HWND> {
    unsafe {
        create_control(
            parent,
            "EDIT",
            "",
            x,
            y,
            width,
            height,
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            ),
        )
    }
}

unsafe fn create_combo(
    parent: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    options: &[&str],
) -> Result<HWND> {
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
                WS_CHILD.0 | WS_VISIBLE.0 | WS_BORDER.0 | WS_TABSTOP.0 | CBS_DROPDOWN_STYLE,
            ),
        )?
    };
    for option in options {
        unsafe { combo_add_string(hwnd, option) };
    }
    Ok(hwnd)
}

unsafe fn combo_add_string(hwnd: HWND, value: &str) {
    let value = HSTRING::from(value);
    unsafe {
        let _ = SendMessageW(
            hwnd,
            CB_ADDSTRING_MSG,
            Some(WPARAM(0)),
            Some(LPARAM(value.as_ptr() as isize)),
        );
    }
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
    let dpi = unsafe { GetDpiForWindow(parent) }.max(BASE_DPI);
    let instance = unsafe { GetModuleHandleW(None) }
        .map(|module| HINSTANCE(module.0))
        .map_err(|error| anyhow!("get module handle for debug panel control failed: {error}"))?;
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
    .map_err(|error| anyhow!("create debug panel control failed: {error}"))
}

fn scale_px(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + BASE_DPI as i64 / 2) / BASE_DPI as i64) as i32
}

fn child_safe_style(style: WINDOW_STYLE) -> WINDOW_STYLE {
    if style.0 & WS_CHILD.0 != 0 {
        WINDOW_STYLE(style.0 | WS_CLIPSIBLINGS.0)
    } else {
        style
    }
}

unsafe fn create_panel_font(family: &str, point_size: i32, weight: i32, dpi: u32) -> HFONT {
    let font_family = HSTRING::from(family);
    let height = -((point_size.max(1) * dpi.max(BASE_DPI) as i32 + 36) / 72);
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

fn layout_debug_panel(state: &DebugPanelState) {
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(state.hwnd, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(state.hwnd).max(BASE_DPI);
        let client_width = (rect.right - rect.left).max(scale_px(420, dpi));
        let client_height = (rect.bottom - rect.top).max(scale_px(360, dpi));
        let margin = scale_px(TOP_MARGIN, dpi);
        let display_height = scale_px(DISPLAY_HEIGHT, dpi);
        let status_height = scale_px(STATUS_HEIGHT, dpi);
        let button_height = scale_px(BUTTON_HEIGHT, dpi);
        let gap = scale_px(8, dpi);
        let top_width = (client_width - margin * 2).max(scale_px(320, dpi));

        let display_y = margin;
        let status_y = display_y + display_height + gap;
        let button_y = status_y + status_height + gap;
        let params_y = button_y + button_height + scale_px(12, dpi);
        let params_height = (client_height - params_y - margin).max(scale_px(120, dpi));

        let _ = MoveWindow(
            state.display_hwnd,
            margin,
            display_y,
            top_width,
            display_height,
            true,
        );
        let _ = MoveWindow(
            state.status_hwnd,
            margin,
            status_y,
            top_width,
            status_height,
            true,
        );
        let _ = MoveWindow(
            state.parakeet_button,
            margin,
            button_y,
            scale_px(145, dpi),
            button_height,
            true,
        );
        let _ = MoveWindow(
            state.whisper_button,
            margin + scale_px(156, dpi),
            button_y,
            scale_px(150, dpi),
            button_height,
            true,
        );
        let _ = MoveWindow(
            state.refresh_button,
            margin + scale_px(328, dpi),
            button_y,
            scale_px(80, dpi),
            button_height,
            true,
        );
        let _ = MoveWindow(
            state.reset_button,
            margin + scale_px(420, dpi),
            button_y,
            scale_px(100, dpi),
            button_height,
            true,
        );
        let _ = MoveWindow(
            state.params_viewport_hwnd,
            margin,
            params_y,
            top_width,
            params_height,
            true,
        );
        update_param_scrollbar(state);
    }
}

fn update_param_scrollbar(state: &DebugPanelState) {
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(state.params_viewport_hwnd, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(state.params_viewport_hwnd).max(BASE_DPI);
        let viewport_width = (rect.right - rect.left).max(scale_px(320, dpi));
        let viewport_height = (rect.bottom - rect.top).max(1);
        let content_height = scale_px(state.params_content_height, dpi).max(viewport_height);
        let content_width = scale_px(PARAM_CONTENT_WIDTH, dpi).max(viewport_width);
        let max_scroll = (content_height - viewport_height).max(0);
        let scroll_y = state.params_scroll_y.get().clamp(0, max_scroll);
        state.params_scroll_y.set(scroll_y);

        let _ = SetWindowPos(
            state.params_content_hwnd,
            None,
            0,
            -scroll_y,
            content_width,
            content_height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
        );

        let info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content_height.saturating_sub(1),
            nPage: viewport_height as u32,
            nPos: scroll_y,
            ..Default::default()
        };
        let _ = SetScrollInfo(state.params_viewport_hwnd, SB_VERT, &info, true);
        redraw_params_viewport(state);
    }
}

fn redraw_params_viewport(state: &DebugPanelState) {
    unsafe {
        let flags = RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW;
        let _ = RedrawWindow(Some(state.params_content_hwnd), None, None, flags);
        let _ = RedrawWindow(Some(state.params_viewport_hwnd), None, None, flags);
    }
}

fn scroll_params_by(delta_px: i32) {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            let next = state.params_scroll_y.get().saturating_add(delta_px);
            state.params_scroll_y.set(next);
            update_param_scrollbar(state);
        }
    });
}

fn scroll_params_command(code: u16, track_pos: i32) {
    let delta = PANEL_STATE.with(|stored| {
        let state = stored.borrow();
        let Some(state) = state.as_ref() else {
            return None;
        };
        unsafe {
            let mut rect = RECT::default();
            if GetClientRect(state.params_viewport_hwnd, &mut rect).is_err() {
                return None;
            }
            let viewport_height = (rect.bottom - rect.top).max(1);
            let current = state.params_scroll_y.get();
            let next = match code {
                SB_LINEUP_CODE => current - SCROLL_LINE_PX,
                SB_LINEDOWN_CODE => current + SCROLL_LINE_PX,
                SB_PAGEUP_CODE => current - viewport_height,
                SB_PAGEDOWN_CODE => current + viewport_height,
                SB_THUMBPOSITION_CODE | SB_THUMBTRACK_CODE => track_pos,
                _ => current,
            };
            Some(next - current)
        }
    });
    if let Some(delta) = delta {
        scroll_params_by(delta);
    }
}

fn wheel_delta_from_wparam(wparam: WPARAM) -> i32 {
    (((wparam.0 >> 16) & 0xffff) as u16 as i16) as i32
}

fn loword(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn hiword(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}

fn pump_messages() -> Result<bool> {
    unsafe {
        let mut msg = MSG::default();
        if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            return Ok(true);
        }
    }
    Ok(false)
}

fn destroy_panel_window() {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().take() {
            unsafe {
                let _ = DestroyWindow(state.hwnd);
                let _ = DeleteObject(state.ui_font.into());
                let _ = DeleteObject(state.display_font.into());
            }
        }
    });
}

fn set_display_result(text: &str, status: &str) {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            set_window_text(state.display_hwnd, text);
            if !status.is_empty() {
                set_window_text(state.status_hwnd, status);
            }
        }
    });
}

fn set_status_text(text: &str) {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            set_window_text(state.status_hwnd, text);
        }
    });
}

fn set_debug_mode_enabled(enabled: bool) {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            state.enabled.store(enabled, Ordering::Relaxed);
            if enabled {
                state
                    .asr_sessions
                    .set_preheat_enabled(false, "debug mode enabled");
                set_window_text(
                    state.status_hwnd,
                    "调试模式已开启：不会上屏，只显示在本窗口；流式预热已暂停。",
                );
            } else {
                state
                    .asr_sessions
                    .set_preheat_enabled(true, "debug mode disabled");
                set_window_text(
                    state.status_hwnd,
                    "调试面板已关闭：恢复正常上屏；流式预热已恢复。",
                );
            }
        }
    });
}

fn set_window_text(hwnd: HWND, text: &str) {
    let text = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

fn get_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let read = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..read as usize])
    }
}

fn set_checkbox(hwnd: HWND, checked: bool) {
    unsafe {
        let _ = SendMessageW(
            hwnd,
            BM_SETCHECK_MSG,
            Some(WPARAM(if checked { BST_CHECKED_VALUE } else { 0 })),
            Some(LPARAM(0)),
        );
    }
}

fn is_checkbox_checked(hwnd: HWND) -> bool {
    unsafe {
        SendMessageW(hwnd, BM_GETCHECK_MSG, Some(WPARAM(0)), Some(LPARAM(0))).0 as usize
            == BST_CHECKED_VALUE
    }
}

fn refresh_settings_controls() {
    let result = PANEL_STATE.with(|stored| {
        let state = stored.borrow();
        let Some(state) = state.as_ref() else {
            return Err(anyhow!("debug panel state not ready"));
        };
        let value = state
            .client
            .get(format!("{}/v1/settings/asr", state.endpoint_url))
            .send()?
            .error_for_status()?
            .json::<Value>()?;
        log_settings_snapshot("refresh", &value);
        fill_settings_controls(state, &value);
        Ok::<(), anyhow::Error>(())
    });
    match result {
        Ok(()) => set_status_text("已刷新 sidecar 当前正式 ASR 参数。"),
        Err(error) => set_status_text(&format!("刷新失败：{error}")),
    }
}

fn apply_settings_controls() {
    let result = PANEL_STATE.with(|stored| {
        let state = stored.borrow();
        let Some(state) = state.as_ref() else {
            return Err(anyhow!("debug panel state not ready"));
        };
        state.auto_apply_pending.set(false);
        unsafe {
            let _ = KillTimer(Some(state.hwnd), AUTO_APPLY_TIMER_ID);
        }
        let payload = build_settings_patch(state)?;
        log_settings_snapshot("apply_payload", &payload);
        let _response = state
            .client
            .patch(format!("{}/v1/settings/asr", state.endpoint_url))
            .json(&payload)
            .send()?
            .error_for_status()?
            .json::<Value>()?;
        let readback = state
            .client
            .get(format!("{}/v1/settings/asr", state.endpoint_url))
            .send()?
            .error_for_status()?
            .json::<Value>()?;
        log_settings_snapshot("apply_readback", &readback);
        fill_settings_controls(state, &readback);
        state
            .asr_sessions
            .invalidate_ready("debug panel ASR settings applied");
        Ok::<String, anyhow::Error>(settings_status_summary(&readback))
    });
    match result {
        Ok(summary) => set_status_text(&format!(
            "已保存：{summary}；旧预热已失效，下一次录音生效。"
        )),
        Err(error) => set_status_text(&format!("应用失败：{error}")),
    }
}

fn schedule_auto_apply() {
    PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            if state.updating_controls.get() {
                return;
            }
            state.auto_apply_pending.set(true);
            unsafe {
                let _ = SetTimer(
                    Some(state.hwnd),
                    AUTO_APPLY_TIMER_ID,
                    AUTO_APPLY_DEBOUNCE_MS,
                    None,
                );
            }
            set_window_text(
                state.status_hwnd,
                "参数已修改，正在自动应用；下一次录音使用新设置。",
            );
        }
    });
}

fn flush_auto_apply_timer() {
    let should_apply = PANEL_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = KillTimer(Some(state.hwnd), AUTO_APPLY_TIMER_ID);
            }
            let pending = state.auto_apply_pending.get();
            state.auto_apply_pending.set(false);
            pending && !state.updating_controls.get()
        } else {
            false
        }
    });
    if should_apply {
        apply_settings_controls();
    }
}

fn reset_settings_controls() {
    let result = PANEL_STATE.with(|stored| {
        let state = stored.borrow();
        let Some(state) = state.as_ref() else {
            return Err(anyhow!("debug panel state not ready"));
        };
        let response = state
            .client
            .post(format!("{}/v1/settings/reset-profile", state.endpoint_url))
            .send()?
            .error_for_status()?
            .json::<Value>()?;
        fill_settings_controls(state, &response);
        state
            .asr_sessions
            .invalidate_ready("debug panel ASR settings reset");
        Ok::<(), anyhow::Error>(())
    });
    match result {
        Ok(()) => set_status_text("已重置为调试默认参数；旧预热已失效。"),
        Err(error) => set_status_text(&format!("重置失败：{error}")),
    }
}

fn fill_settings_controls(state: &DebugPanelState, value: &Value) {
    state.updating_controls.set(true);
    let parakeet = value.get("parakeet").unwrap_or(&Value::Null);
    let whisper = value.get("whisper").unwrap_or(&Value::Null);
    let endpointing = parakeet.get("endpointing").unwrap_or(&Value::Null);

    set_window_text(
        state.parakeet_language_hwnd,
        &json_str(parakeet, "language_code", "zh-CN"),
    );
    set_checkbox(
        state.parakeet_punctuation_hwnd,
        json_bool(parakeet, "enable_automatic_punctuation", true),
    );
    set_checkbox(
        state.parakeet_verbatim_hwnd,
        json_bool(parakeet, "verbatim_transcripts", true),
    );
    set_checkbox(
        state.parakeet_boost_enabled_hwnd,
        json_bool(parakeet, "boost_enabled", true),
    );
    set_window_text(
        state.parakeet_boost_hwnd,
        &format_number(json_f64(parakeet, "boost", 40.0)),
    );
    set_checkbox(
        state.parakeet_legacy_hwnd,
        json_bool(parakeet, "include_legacy_variants", false),
    );
    set_window_text(
        state.parakeet_partial_wait_hwnd,
        &format_partial_wait(json_f64(parakeet, "partial_wait_sec", 0.06)),
    );

    set_window_text(
        state.endpoint_start_history_hwnd,
        &format_optional_i64(endpointing.get("start_history")),
    );
    set_window_text(
        state.endpoint_start_threshold_hwnd,
        &format_optional_f64(endpointing.get("start_threshold")),
    );
    set_window_text(
        state.endpoint_stop_history_hwnd,
        &format_optional_i64(endpointing.get("stop_history")),
    );
    set_window_text(
        state.endpoint_stop_threshold_hwnd,
        &format_optional_f64(endpointing.get("stop_threshold")),
    );
    set_window_text(
        state.endpoint_stop_history_eou_hwnd,
        &format_optional_i64(endpointing.get("stop_history_eou")),
    );
    set_window_text(
        state.endpoint_stop_threshold_eou_hwnd,
        &format_optional_f64(endpointing.get("stop_threshold_eou")),
    );

    set_window_text(
        state.whisper_language_hwnd,
        &json_str(whisper, "language_code", "zh"),
    );
    set_checkbox(
        state.whisper_punctuation_hwnd,
        json_bool(whisper, "enable_automatic_punctuation", true),
    );
    set_checkbox(
        state.whisper_verbatim_hwnd,
        json_bool(whisper, "verbatim_transcripts", true),
    );
    set_window_text(
        state.whisper_min_audio_hwnd,
        &format_number(json_f64(whisper, "min_audio_sec", 0.35)),
    );
    set_window_text(
        state.whisper_min_rms_hwnd,
        &format_number(json_f64(whisper, "min_rms_dbfs", -64.0)),
    );

    let hotwords = parakeet
        .get("boost_phrases")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .unwrap_or_default();
    set_window_text(state.hotwords_hwnd, &hotwords);
    state.updating_controls.set(false);
}

fn build_settings_patch(state: &DebugPanelState) -> Result<Value> {
    let hotwords = parse_hotwords(&get_window_text(state.hotwords_hwnd));
    if hotwords.is_empty() {
        return Err(anyhow!("热词列表为空；至少保留一个正式热词"));
    }
    Ok(serde_json::json!({
        "parakeet": {
            "language_code": required_text(state.parakeet_language_hwnd, "Parakeet language_code")?,
            "enable_automatic_punctuation": is_checkbox_checked(state.parakeet_punctuation_hwnd),
            "verbatim_transcripts": is_checkbox_checked(state.parakeet_verbatim_hwnd),
            "boost_enabled": is_checkbox_checked(state.parakeet_boost_enabled_hwnd),
            "boost": parse_f64_field(state.parakeet_boost_hwnd, "Parakeet boost")?,
            "boost_phrases": hotwords,
            "include_legacy_variants": is_checkbox_checked(state.parakeet_legacy_hwnd),
            "partial_wait_sec": parse_f64_field(state.parakeet_partial_wait_hwnd, "Parakeet partial_wait_sec")?,
            "endpointing": {
                "start_history": parse_optional_i64_field(state.endpoint_start_history_hwnd, "start_history")?,
                "start_threshold": parse_optional_f64_field(state.endpoint_start_threshold_hwnd, "start_threshold")?,
                "stop_history": parse_optional_i64_field(state.endpoint_stop_history_hwnd, "stop_history")?,
                "stop_threshold": parse_optional_f64_field(state.endpoint_stop_threshold_hwnd, "stop_threshold")?,
                "stop_history_eou": parse_optional_i64_field(state.endpoint_stop_history_eou_hwnd, "stop_history_eou")?,
                "stop_threshold_eou": parse_optional_f64_field(state.endpoint_stop_threshold_eou_hwnd, "stop_threshold_eou")?,
            }
        },
        "whisper": {
            "language_code": required_text(state.whisper_language_hwnd, "Whisper language_code")?,
            "enable_automatic_punctuation": is_checkbox_checked(state.whisper_punctuation_hwnd),
            "verbatim_transcripts": is_checkbox_checked(state.whisper_verbatim_hwnd),
            "min_audio_sec": parse_f64_field(state.whisper_min_audio_hwnd, "Whisper min_audio_sec")?,
            "min_rms_dbfs": parse_f64_field(state.whisper_min_rms_hwnd, "Whisper min_rms_dbfs")?,
        }
    }))
}

fn log_settings_snapshot(action: &str, value: &Value) {
    let parakeet = value.get("parakeet").unwrap_or(&Value::Null);
    let whisper = value.get("whisper").unwrap_or(&Value::Null);
    let endpointing = parakeet.get("endpointing").unwrap_or(&Value::Null);
    let hotword_count = parakeet
        .get("boost_phrases")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    info!(
        action = %action,
        parakeet_punctuation = json_bool(parakeet, "enable_automatic_punctuation", true),
        parakeet_verbatim = json_bool(parakeet, "verbatim_transcripts", true),
        parakeet_boost_enabled = json_bool(parakeet, "boost_enabled", true),
        parakeet_boost = json_f64(parakeet, "boost", 40.0),
        parakeet_partial_wait_sec = json_f64(parakeet, "partial_wait_sec", 0.06),
        parakeet_start_history = format_optional_i64(endpointing.get("start_history")),
        parakeet_start_threshold = format_optional_f64(endpointing.get("start_threshold")),
        parakeet_stop_history = format_optional_i64(endpointing.get("stop_history")),
        parakeet_stop_threshold = format_optional_f64(endpointing.get("stop_threshold")),
        parakeet_stop_history_eou = format_optional_i64(endpointing.get("stop_history_eou")),
        parakeet_stop_threshold_eou = format_optional_f64(endpointing.get("stop_threshold_eou")),
        whisper_punctuation = json_bool(whisper, "enable_automatic_punctuation", true),
        whisper_verbatim = json_bool(whisper, "verbatim_transcripts", true),
        whisper_language = %json_str(whisper, "language_code", "zh"),
        whisper_min_audio_sec = json_f64(whisper, "min_audio_sec", 0.35),
        whisper_min_rms_dbfs = json_f64(whisper, "min_rms_dbfs", -64.0),
        hotword_count,
        "debug panel ASR settings snapshot"
    );
}

fn settings_status_summary(value: &Value) -> String {
    let parakeet = value.get("parakeet").unwrap_or(&Value::Null);
    let endpointing = parakeet.get("endpointing").unwrap_or(&Value::Null);
    format!(
        "partial_wait_sec={}, stop_history={}, punctuation={}",
        format_partial_wait(json_f64(parakeet, "partial_wait_sec", 0.06)),
        format_optional_i64(endpointing.get("stop_history")).if_empty("空"),
        if json_bool(parakeet, "enable_automatic_punctuation", true) {
            "开"
        } else {
            "关"
        }
    )
}

fn required_text(hwnd: HWND, name: &str) -> Result<String> {
    let text = get_window_text(hwnd).trim().to_string();
    if text.is_empty() {
        return Err(anyhow!("{name} 不能为空"));
    }
    Ok(text)
}

fn parse_f64_field(hwnd: HWND, name: &str) -> Result<f64> {
    let raw = required_text(hwnd, name)?;
    raw.parse::<f64>()
        .map_err(|error| anyhow!("{name} 不是有效数字：{error}"))
}

fn parse_optional_i64_field(hwnd: HWND, name: &str) -> Result<Value> {
    let raw = get_window_text(hwnd).trim().to_string();
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    let parsed = raw
        .parse::<i64>()
        .map_err(|error| anyhow!("{name} 不是有效整数：{error}"))?;
    Ok(serde_json::json!(parsed))
}

fn parse_optional_f64_field(hwnd: HWND, name: &str) -> Result<Value> {
    let raw = get_window_text(hwnd).trim().to_string();
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    let parsed = raw
        .parse::<f64>()
        .map_err(|error| anyhow!("{name} 不是有效数字：{error}"))?;
    Ok(serde_json::json!(parsed))
}

fn parse_hotwords(raw: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    for phrase in raw.split(|ch| matches!(ch, '\n' | '\r' | ',' | '，')) {
        let phrase = phrase.trim();
        if phrase.is_empty() {
            continue;
        }
        if !phrases.iter().any(|existing| existing == phrase) {
            phrases.push(phrase.to_string());
        }
    }
    phrases
}

fn json_str(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn json_bool(value: &Value, key: &str, default: bool) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn json_f64(value: &Value, key: &str, default: f64) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn format_optional_i64(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_i64)
        .map_or_else(String::new, |value| value.to_string())
}

fn format_optional_f64(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_f64)
        .map_or_else(String::new, format_number)
}

fn format_number(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn format_partial_wait(value: f64) -> String {
    format!("{value:.2}")
}

trait EmptyTextFallback {
    fn if_empty(self, fallback: &'static str) -> String;
}

impl EmptyTextFallback for String {
    fn if_empty(self, fallback: &'static str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[derive(Clone, Copy)]
enum DebugCommandAction {
    None,
    SetMode(InputMode),
    Refresh,
    Reset,
    ApplyNow,
    ScheduleAutoApply,
}

fn classify_debug_command(
    state: &DebugPanelState,
    control: HWND,
    notification: u16,
) -> DebugCommandAction {
    if control == state.parakeet_button {
        return DebugCommandAction::SetMode(InputMode::StreamingAsr);
    }
    if control == state.whisper_button {
        return DebugCommandAction::SetMode(InputMode::WhisperZh);
    }
    if control == state.refresh_button {
        return DebugCommandAction::Refresh;
    }
    if control == state.reset_button {
        return DebugCommandAction::Reset;
    }
    if state.updating_controls.get() || !is_settings_control(state, control) {
        return DebugCommandAction::None;
    }
    match notification {
        BN_CLICKED_CODE | CBN_SELCHANGE_CODE => DebugCommandAction::ApplyNow,
        EN_CHANGE_CODE | CBN_EDITCHANGE_CODE => DebugCommandAction::ScheduleAutoApply,
        _ => DebugCommandAction::None,
    }
}

fn is_settings_control(state: &DebugPanelState, control: HWND) -> bool {
    [
        state.parakeet_language_hwnd,
        state.parakeet_punctuation_hwnd,
        state.parakeet_verbatim_hwnd,
        state.parakeet_boost_enabled_hwnd,
        state.parakeet_boost_hwnd,
        state.parakeet_legacy_hwnd,
        state.parakeet_partial_wait_hwnd,
        state.endpoint_start_history_hwnd,
        state.endpoint_start_threshold_hwnd,
        state.endpoint_stop_history_hwnd,
        state.endpoint_stop_threshold_hwnd,
        state.endpoint_stop_history_eou_hwnd,
        state.endpoint_stop_threshold_eou_hwnd,
        state.whisper_language_hwnd,
        state.whisper_punctuation_hwnd,
        state.whisper_verbatim_hwnd,
        state.whisper_min_audio_hwnd,
        state.whisper_min_rms_hwnd,
        state.hotwords_hwnd,
    ]
    .contains(&control)
}

fn handle_debug_command(wparam: WPARAM, lparam: LPARAM) {
    let control = HWND(lparam.0 as *mut core::ffi::c_void);
    let notification = hiword(wparam.0);
    let action = PANEL_STATE.with(|stored| {
        stored
            .borrow()
            .as_ref()
            .map(|state| classify_debug_command(state, control, notification))
            .unwrap_or(DebugCommandAction::None)
    });
    match action {
        DebugCommandAction::None => {}
        DebugCommandAction::SetMode(mode) => {
            PANEL_STATE.with(|stored| {
                if let Some(state) = stored.borrow().as_ref() {
                    state.modes.set(mode);
                }
            });
            match mode {
                InputMode::StreamingAsr => set_status_text("调试模式：Parakeet 流式。"),
                InputMode::WhisperZh => set_status_text("调试模式：Whisper 非流式。"),
                InputMode::LocalNonstreaming => set_status_text("调试模式：本地非流式。"),
            }
        }
        DebugCommandAction::Refresh => refresh_settings_controls(),
        DebugCommandAction::Reset => reset_settings_controls(),
        DebugCommandAction::ApplyNow => apply_settings_controls(),
        DebugCommandAction::ScheduleAutoApply => schedule_auto_apply(),
    }
}

extern "system" fn debug_panel_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            handle_debug_command(wparam, lparam);
            LRESULT(0)
        }
        WM_SIZE => {
            PANEL_STATE.with(|stored| {
                if let Some(state) = stored.borrow().as_ref()
                    && state.hwnd == hwnd
                {
                    layout_debug_panel(state);
                }
            });
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let minmax = lparam.0 as *mut MINMAXINFO;
            if !minmax.is_null() {
                unsafe {
                    let dpi = GetDpiForWindow(hwnd).max(BASE_DPI);
                    (*minmax).ptMinTrackSize.x = scale_px(MIN_PANEL_WIDTH, dpi);
                    (*minmax).ptMinTrackSize.y = scale_px(MIN_PANEL_HEIGHT, dpi);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == AUTO_APPLY_TIMER_ID {
                flush_auto_apply_timer();
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEWHEEL => {
            let delta = wheel_delta_from_wparam(wparam);
            if delta != 0 {
                let steps = (-delta / WHEEL_DELTA_UNITS).clamp(-3, 3);
                scroll_params_by(steps * SCROLL_LINE_PX * 3);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            flush_auto_apply_timer();
            PANEL_STATE.with(|stored| {
                if let Some(state) = stored.borrow().as_ref() {
                    unsafe {
                        let _ = ShowWindow(state.hwnd, SW_HIDE);
                    }
                }
            });
            set_debug_mode_enabled(false);
            LRESULT(0)
        }
        WM_CREATE => {
            let _ = lparam.0 as *const CREATESTRUCTW;
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

extern "system" fn debug_content_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            handle_debug_command(wparam, lparam);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = wheel_delta_from_wparam(wparam);
            if delta != 0 {
                let steps = (-delta / WHEEL_DELTA_UNITS).clamp(-3, 3);
                scroll_params_by(steps * SCROLL_LINE_PX * 3);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

extern "system" fn debug_scroll_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_VSCROLL => {
            let code = loword(wparam.0);
            let track_pos = hiword(wparam.0) as i32;
            scroll_params_command(code, track_pos);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = wheel_delta_from_wparam(wparam);
            if delta != 0 {
                let steps = (-delta / WHEEL_DELTA_UNITS).clamp(-3, 3);
                scroll_params_by(steps * SCROLL_LINE_PX * 3);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
