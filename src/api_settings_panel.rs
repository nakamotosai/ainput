//! Dark native settings panel for OpenAI-compatible rewrite credentials.
//! Client-area sized via AdjustWindowRectEx + ClearType YaHei UI fonts.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
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
    AdjustWindowRectEx, BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CBS_DROPDOWN, CBS_HASSTRINGS,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_LEFT,
    ES_NUMBER, ES_PASSWORD, GetClientRect, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU,
    IDC_ARROW, LoadCursorW, MSG, PostMessageW, PostThreadMessageW, RegisterClassW, SW_HIDE,
    SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::ai_rewrite::SharedRewriter;
use crate::api_config::{self, ApiConnections, ApiConnectionsConfig};
use crate::rewrite_language::RewriteLanguageController;

const PANEL_THREAD_QUIT: u32 = WM_APP + 121;
const PANEL_OPEN: u32 = WM_APP + 122;
const PANEL_MODELS_DONE: u32 = WM_APP + 123;
const PANEL_PROBE_DONE: u32 = WM_APP + 124;

/// Desired **client** area (content). Outer window size is derived via AdjustWindowRectEx.
const CLIENT_W: i32 = 720;
const CLIENT_H: i32 = 860;
const MARGIN: i32 = 40;
const FIELD_W: i32 = 640;
const FIELD_H: i32 = 44;
const LABEL_H: i32 = 30;
const GAP_AFTER_LABEL: i32 = 10;
const GAP_AFTER_FIELD: i32 = 30;
const HINT_H: i32 = 58;
const BUTTON_H: i32 = 48;
const BUTTON_W: i32 = 160;
const FONT_PX: i32 = 20;
const TITLE_FONT_PX: i32 = 26;

const ID_BASE_URL: i32 = 4001;
const ID_API_KEY: i32 = 4002;
const ID_MODEL: i32 = 4003;
const ID_TIMEOUT: i32 = 4004;
const ID_REWRITE: i32 = 4005;
const ID_FETCH: i32 = 4006;
const ID_SAVE: i32 = 4007;
const ID_CANCEL: i32 = 4008;
const ID_STATUS: i32 = 4009;
const ID_TITLE: i32 = 4010;
const ID_HINT: i32 = 4011;
const ID_LBL_URL: i32 = 4012;
const ID_LBL_KEY: i32 = 4013;
const ID_LBL_MODEL: i32 = 4014;
const ID_LBL_TIMEOUT: i32 = 4015;

const BM_SETCHECK: u32 = 0x00F1;
const BM_GETCHECK: u32 = 0x00F0;
const BST_UNCHECKED: isize = 0;
const BST_CHECKED: isize = 1;
const CB_RESETCONTENT: u32 = 0x014B;
const CB_ADDSTRING: u32 = 0x0143;
const CB_SELECTSTRING: u32 = 0x014D;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_ERR: isize = -1;

const BG: COLORREF = COLORREF(0x00_16_14_14);
const INPUT_BG: COLORREF = COLORREF(0x00_2A_24_24);
const TEXT: COLORREF = COLORREF(0x00_F2_F0_F0);
const BUTTON_BG: COLORREF = COLORREF(0x00_32_2A_2A);

const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const PANEL_FONT_FAMILY: &str = "Microsoft YaHei UI";

static FETCH_RESULT: OnceLock<Mutex<Option<Result<Vec<String>, String>>>> = OnceLock::new();
static PROBE_RESULT: OnceLock<Mutex<Option<api_config::ConnectivityProbe>>> = OnceLock::new();

fn fetch_result_slot() -> &'static Mutex<Option<Result<Vec<String>, String>>> {
    FETCH_RESULT.get_or_init(|| Mutex::new(None))
}

fn probe_result_slot() -> &'static Mutex<Option<api_config::ConnectivityProbe>> {
    PROBE_RESULT.get_or_init(|| Mutex::new(None))
}

#[derive(Clone)]
pub struct ApiSettingsPanelController {
    thread_id: u32,
}

impl ApiSettingsPanelController {
    pub fn start(
        api_path: PathBuf,
        rewrite_language: RewriteLanguageController,
        rewriter: SharedRewriter,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        thread::spawn(move || {
            PANEL_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            PANEL_STATE.with(|state| {
                *state.borrow_mut() = Some(PanelState::new(api_path, rewrite_language, rewriter));
            });
            if let Err(error) = unsafe { run_panel_thread(shutdown) } {
                warn!(error = %error, "API settings panel thread failed");
            }
        });
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("API settings panel thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { thread_id })
    }

    pub fn open(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, PANEL_OPEN, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for ApiSettingsPanelController {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, PANEL_THREAD_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

struct PanelState {
    api_path: PathBuf,
    rewrite_language: RewriteLanguageController,
    rewriter: SharedRewriter,
    hwnd: HWND,
    base_url: HWND,
    api_key: HWND,
    model: HWND,
    timeout: HWND,
    rewrite_check: HWND,
    status: HWND,
    brush_bg: HBRUSH,
    brush_input: HBRUSH,
    brush_button: HBRUSH,
    font: HFONT,
    title_font: HFONT,
    fetching: bool,
}

impl PanelState {
    fn new(
        api_path: PathBuf,
        rewrite_language: RewriteLanguageController,
        rewriter: SharedRewriter,
    ) -> Self {
        Self {
            api_path,
            rewrite_language,
            rewriter,
            hwnd: HWND::default(),
            base_url: HWND::default(),
            api_key: HWND::default(),
            model: HWND::default(),
            timeout: HWND::default(),
            rewrite_check: HWND::default(),
            status: HWND::default(),
            brush_bg: HBRUSH::default(),
            brush_input: HBRUSH::default(),
            brush_button: HBRUSH::default(),
            font: HFONT::default(),
            title_font: HFONT::default(),
            fetching: false,
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
    // AdjustWindowRectEx expands rect from client → outer (includes caption/borders).
    let ok = unsafe { AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0)) };
    if ok.is_err() {
        // Fallback: typical caption+border padding so content is never clipped.
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
    info!(thread_id, client_w = CLIENT_W, client_h = CLIENT_H, "API settings panel thread started");

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
            return Err(anyhow!("API settings GetMessage failed"));
        }
        if has.0 == 0 || msg.message == PANEL_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == PANEL_OPEN {
            unsafe { show_panel() };
            continue;
        }
        if msg.message == PANEL_MODELS_DONE {
            handle_fetch_result();
            continue;
        }
        if msg.message == PANEL_PROBE_DONE {
            handle_probe_result();
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
        lpszClassName: w!("ainput_api_settings_v3"),
        hCursor: cursor,
        hbrBackground: unsafe { CreateSolidBrush(BG) },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_panel_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!("ainput · API 设置 {}", env!("CARGO_PKG_VERSION")));
    let style = WINDOW_STYLE(
        WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_CLIPCHILDREN.0,
    );
    let (outer_w, outer_h) = outer_size_for_client(style);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_api_settings_v3"),
            PCWSTR(title.as_ptr()),
            style,
            120,
            40,
            outer_w,
            outer_h,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create API settings window failed: {error}"))
}

unsafe fn create_panel_controls(hwnd: HWND) -> Result<()> {
    PANEL_STATE.with(|state| {
        let mut borrow = state.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return Err(anyhow!("panel state missing"));
        };

        state.font = unsafe { create_ui_font(FONT_PX, 400) };
        state.title_font = unsafe { create_ui_font(TITLE_FONT_PX, 600) };
        if state.font.is_invalid() || state.title_font.is_invalid() {
            return Err(anyhow!("create panel UI font failed (Microsoft YaHei UI)"));
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
                     use_title_font: bool|
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
            .map_err(|error| anyhow!("create child {id}: {error}"))?;
            let font = if use_title_font {
                title_font
            } else {
                body_font
            };
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

        let mut y = 28i32;

        let _title = child(
            w!("STATIC"),
            "API / 改写设置",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            34,
            ID_TITLE,
            true,
        )?;
        y += 42;
        let _hint = child(
            w!("STATIC"),
            "填写 OpenAI 兼容接口。默认已预填 NVIDIA。\r\nAPI Key 只保存在本机，不会上传到 ainput 服务器。",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            HINT_H,
            ID_HINT,
            false,
        )?;
        y += HINT_H + 18;

        let _lbl_url = child(
            w!("STATIC"),
            "Base URL",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_URL,
            false,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
        state.base_url = child(
            w!("EDIT"),
            NVIDIA_BASE_URL,
            WINDOW_STYLE(WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_LEFT as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            FIELD_W,
            FIELD_H,
            ID_BASE_URL,
            false,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        let _lbl_key = child(
            w!("STATIC"),
            "API Key",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_KEY,
            false,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
        state.api_key = child(
            w!("EDIT"),
            "",
            WINDOW_STYLE(
                WS_BORDER.0
                    | ES_AUTOHSCROLL as u32
                    | ES_LEFT as u32
                    | ES_PASSWORD as u32
                    | WS_TABSTOP.0,
            ),
            MARGIN,
            y,
            FIELD_W,
            FIELD_H,
            ID_API_KEY,
            false,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        let _lbl_model = child(
            w!("STATIC"),
            "模型（可手填，或填 Key 后点右侧「拉取模型」）",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_MODEL,
            false,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
        let model_w = FIELD_W - 150;
        state.model = child(
            w!("COMBOBOX"),
            "",
            WINDOW_STYLE(
                WS_BORDER.0
                    | CBS_DROPDOWN as u32
                    | CBS_HASSTRINGS as u32
                    | WS_VSCROLL.0
                    | WS_TABSTOP.0,
            ),
            MARGIN,
            y,
            model_w,
            280,
            ID_MODEL,
            false,
        )?;
        let _fetch = child(
            w!("BUTTON"),
            "拉取模型",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + model_w + 12,
            y,
            138,
            FIELD_H,
            ID_FETCH,
            false,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        let _lbl_timeout = child(
            w!("STATIC"),
            "超时毫秒（连不上时的兜底，默认 5000）",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_TIMEOUT,
            false,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
        state.timeout = child(
            w!("EDIT"),
            &DEFAULT_TIMEOUT_MS.to_string(),
            WINDOW_STYLE(WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            200,
            FIELD_H,
            ID_TIMEOUT,
            false,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        state.rewrite_check = child(
            w!("BUTTON"),
            "启用 AI 改写（本地听写后润色）",
            WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            FIELD_W,
            36,
            ID_REWRITE,
            false,
        )?;
        y += 48;

        state.status = child(
            w!("STATIC"),
            "",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            48,
            ID_STATUS,
            false,
        )?;
        y += 60;

        // Bottom action row — must stay inside CLIENT_H.
        let _save = child(
            w!("BUTTON"),
            "保存",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            BUTTON_W,
            BUTTON_H,
            ID_SAVE,
            false,
        )?;
        let _cancel = child(
            w!("BUTTON"),
            "取消",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + BUTTON_W + 24,
            y,
            BUTTON_W,
            BUTTON_H,
            ID_CANCEL,
            false,
        )?;

        let bottom = y + BUTTON_H + MARGIN;
        if bottom > CLIENT_H {
            warn!(
                bottom,
                client_h = CLIENT_H,
                "API panel layout exceeds client height — enlarge CLIENT_H"
            );
        } else {
            info!(bottom, client_h = CLIENT_H, "API panel layout fits client area");
        }
        Ok(())
    })
}

unsafe fn show_panel() {
    PANEL_STATE.with(|cell| {
        let Ok(borrow) = cell.try_borrow() else {
            return;
        };
        let Some(state) = borrow.as_ref() else {
            return;
        };
        load_fields(state);
        unsafe {
            let _ = ShowWindow(state.hwnd, SW_RESTORE);
            let _ = ShowWindow(state.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(state.hwnd);
        }
        info!("API settings panel opened");
    });
}

fn load_fields(state: &PanelState) {
    let (base, key, model, timeout_ms) = match load_api_file(&state.api_path) {
        Ok(v) => v,
        Err(error) => {
            set_status(state, &format!("读取配置失败：{error}"));
            (
                NVIDIA_BASE_URL.to_string(),
                String::new(),
                String::new(),
                DEFAULT_TIMEOUT_MS,
            )
        }
    };
    let base = if base.trim().is_empty() {
        NVIDIA_BASE_URL.to_string()
    } else {
        base
    };
    set_window_text(state.base_url, &base);
    set_window_text(state.api_key, &key);
    set_window_text(state.model, &model);
    set_window_text(state.timeout, &timeout_ms.to_string());
    let enabled = state.rewrite_language.rewrite_enabled();
    unsafe {
        SendMessageW(
            state.rewrite_check,
            BM_SETCHECK,
            Some(WPARAM(if enabled {
                BST_CHECKED as usize
            } else {
                BST_UNCHECKED as usize
            })),
            Some(LPARAM(0)),
        );
    }
    set_status(
        state,
        if key.trim().is_empty() {
            "已预填 NVIDIA · 填入 Key 后可拉取模型列表"
        } else {
            "已加载本地配置"
        },
    );
}

fn load_api_file(path: &std::path::Path) -> Result<(String, String, String, u64)> {
    if !path.exists() {
        return Ok((
            NVIDIA_BASE_URL.to_string(),
            String::new(),
            String::new(),
            DEFAULT_TIMEOUT_MS,
        ));
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let config: ApiConnectionsConfig =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let timeout = if config.rewrite.timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        config.rewrite.timeout_ms
    };
    Ok((
        config.cliproxyapi.base_url,
        config.cliproxyapi.api_key,
        config.rewrite.model,
        timeout,
    ))
}

fn parse_timeout_ms(text: &str) -> u64 {
    text.trim()
        .parse::<u64>()
        .ok()
        .map(|v| v.clamp(500, 120_000))
        .unwrap_or(DEFAULT_TIMEOUT_MS)
}

/// Snapshot of HWND handles for UI updates **without** holding PANEL_STATE borrow.
/// Holding `RefCell::borrow_mut` across `SetWindowTextW`/`SendMessageW` re-enters
/// `WM_CTLCOLOR*` which also borrows PANEL_STATE → panic → whole process exits.
#[derive(Clone, Copy)]
struct PanelUiHandles {
    hwnd: HWND,
    base_url: HWND,
    api_key: HWND,
    model: HWND,
    timeout: HWND,
    status: HWND,
}

fn panel_ui_handles() -> Option<PanelUiHandles> {
    PANEL_STATE.with(|cell| {
        let borrow = cell.try_borrow().ok()?;
        let state = borrow.as_ref()?;
        Some(PanelUiHandles {
            hwnd: state.hwnd,
            base_url: state.base_url,
            api_key: state.api_key,
            model: state.model,
            timeout: state.timeout,
            status: state.status,
        })
    })
}

fn set_status_hwnd(status: HWND, text: &str) {
    set_window_text(status, text);
}

fn set_fetching(flag: bool) {
    PANEL_STATE.with(|cell| {
        if let Ok(mut borrow) = cell.try_borrow_mut() {
            if let Some(state) = borrow.as_mut() {
                state.fetching = flag;
            }
        }
    });
}

fn is_fetching() -> bool {
    PANEL_STATE.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|b| b.as_ref().map(|s| s.fetching))
            .unwrap_or(false)
    })
}


fn load_models_path_for_fetch() -> Option<String> {
    let path = PANEL_STATE.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|b| b.as_ref().map(|s| s.api_path.clone()))
    })?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("cliproxyapi")
        .and_then(|v| v.get("models_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn start_fetch_models() {
    let Some(ui) = panel_ui_handles() else {
        return;
    };
    if is_fetching() {
        set_status_hwnd(ui.status, "正在拉取，请稍候…");
        return;
    }
    let base = get_window_text(ui.base_url);
    let key = get_window_text(ui.api_key);
    let timeout_ms = parse_timeout_ms(&get_window_text(ui.timeout));
    if base.trim().is_empty() {
        set_status_hwnd(ui.status, "请先填写 Base URL");
        return;
    }
    if key.trim().is_empty() {
        set_status_hwnd(ui.status, "请先填写 API Key 再拉取模型");
        return;
    }
    let models_path = load_models_path_for_fetch().unwrap_or_else(|| "/v1/models".to_string());
    set_fetching(true);
    set_status_hwnd(
        ui.status,
        &format!("正在拉取模型列表…（超时 {timeout_ms} ms）"),
    );
    info!(timeout_ms, base = %base, models_path = %models_path, "API panel model fetch started");
    let hwnd_raw = ui.hwnd.0 as isize;
    thread::spawn(move || {
        // Never panic across the thread boundary — always post a result.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            api_config::list_models(&base, &key, &models_path, timeout_ms)
        }))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("拉取线程内部 panic")))
        .map_err(|error| format!("{error:#}"));
        if let Ok(mut slot) = fetch_result_slot().lock() {
            *slot = Some(result);
        }
        let hwnd = HWND(hwnd_raw as *mut _);
        unsafe {
            let _ = PostMessageW(Some(hwnd), PANEL_MODELS_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

fn handle_fetch_result() {
    let result = fetch_result_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    set_fetching(false);
    let Some(ui) = panel_ui_handles() else {
        return;
    };
    match result {
        Some(Ok(mut models)) => {
            // Cap combo fill — NVIDIA lists can be huge; keep UI responsive.
            const MAX_COMBO: usize = 800;
            let total = models.len();
            if models.len() > MAX_COMBO {
                models.truncate(MAX_COMBO);
            }
            let prefer = get_combo_text(ui.model);
            fill_model_combo(ui.model, &models, &prefer);
            let msg = if total > models.len() {
                format!(
                    "已拉取 {total} 个模型，列表显示前 {} 个（可手填完整 id）",
                    models.len()
                )
            } else {
                format!("已拉取 {total} 个模型，请下拉选择")
            };
            set_status_hwnd(ui.status, &msg);
            info!(total, shown = models.len(), "API panel model fetch ok");
        }
        Some(Err(error)) => {
            let short = if error.chars().count() > 160 {
                format!("{}…", error.chars().take(160).collect::<String>())
            } else {
                error
            };
            set_status_hwnd(ui.status, &format!("拉取失败（可手填模型）：{short}"));
            warn!(error = %short, "API panel model fetch failed");
        }
        None => set_status_hwnd(ui.status, "拉取结果丢失，请重试"),
    }
}

fn fill_model_combo(combo: HWND, models: &[String], prefer: &str) {
    unsafe {
        SendMessageW(combo, CB_RESETCONTENT, Some(WPARAM(0)), Some(LPARAM(0)));
        for model in models {
            let text = HSTRING::from(model.as_str());
            SendMessageW(
                combo,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
        if !prefer.trim().is_empty() {
            let prefer_h = HSTRING::from(prefer.trim());
            let found = SendMessageW(
                combo,
                CB_SELECTSTRING,
                Some(WPARAM((-1isize) as usize)),
                Some(LPARAM(prefer_h.as_ptr() as isize)),
            );
            if found.0 == CB_ERR {
                set_window_text(combo, prefer.trim());
            }
        }
    }
}

/// Save path: write config (including API key) to disk, hot-reload rewriter,
/// then async probe connectivity (HTTP status + latency). No RefCell hold across UI.
fn save_fields() {
    let Some(ui) = panel_ui_handles() else {
        return;
    };

    // Snapshot controller handles without holding borrow across SetWindowText.
    let snapshot = PANEL_STATE.with(|cell| {
        let borrow = cell.try_borrow().ok()?;
        let state = borrow.as_ref()?;
        Some((
            state.api_path.clone(),
            state.rewrite_language.clone(),
            state.rewriter.clone(),
            state.rewrite_check,
        ))
    });
    let Some((api_path, rewrite_language, rewriter, rewrite_check)) = snapshot else {
        return;
    };

    let base = get_window_text(ui.base_url);
    let key = get_window_text(ui.api_key);
    let model = get_combo_text(ui.model);
    let timeout_ms = parse_timeout_ms(&get_window_text(ui.timeout));
    let enable_rewrite = unsafe {
        SendMessageW(
            rewrite_check,
            BM_GETCHECK,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 == BST_CHECKED
    };

    if enable_rewrite && base.trim().is_empty() {
        set_status_hwnd(ui.status, "启用改写前请先填写 Base URL");
        return;
    }
    if enable_rewrite && model.trim().is_empty() {
        set_status_hwnd(ui.status, "启用改写前请先填写或选择模型");
        return;
    }
    if enable_rewrite && key.trim().is_empty() {
        set_status_hwnd(ui.status, "启用改写前请先填写 API Key");
        return;
    }

    let mut config = if api_path.exists() {
        match std::fs::read_to_string(&api_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<ApiConnectionsConfig>(&raw).ok())
        {
            Some(config) => config,
            None => ApiConnectionsConfig::default(),
        }
    } else {
        ApiConnectionsConfig::default()
    };
    config.cliproxyapi.base_url = base.trim().trim_end_matches('/').to_string();
    if config.cliproxyapi.base_url.is_empty() {
        config.cliproxyapi.base_url = NVIDIA_BASE_URL.to_string();
    }
    // Persist API key to local state file (not uploaded to ainput).
    config.cliproxyapi.api_key = key.trim().to_string();
    if config.cliproxyapi.api_key_env.trim().is_empty() {
        config.cliproxyapi.api_key_env = "AINPUT_API_KEY".to_string();
    }
    if config.cliproxyapi.chat_completions_path.trim().is_empty() {
        config.cliproxyapi.chat_completions_path = "/v1/chat/completions".to_string();
    }
    if config.cliproxyapi.models_path.trim().is_empty() {
        config.cliproxyapi.models_path = "/v1/models".to_string();
    }
    config.rewrite.model = model.trim().to_string();
    config.rewrite.timeout_ms = timeout_ms;

    let connections = ApiConnections {
        path: api_path,
        config,
    };
    if let Err(error) = connections.save() {
        set_status_hwnd(ui.status, &format!("保存失败：{error}"));
        return;
    }

    rewrite_language.set_rewrite_enabled(enable_rewrite);
    if let Err(error) = rewriter.apply_connection(
        &connections.config.cliproxyapi.base_url,
        &connections.config.cliproxyapi.api_key,
        &connections.config.rewrite.model,
        &connections.config.cliproxyapi.chat_completions_path,
        timeout_ms,
    ) {
        set_status_hwnd(ui.status, &format!("Key 已保存，热加载失败：{error}"));
        return;
    }

    let key_note = if connections.config.cliproxyapi.api_key.is_empty() {
        "Key 空"
    } else {
        "Key 已落盘"
    };
    let rewrite_note = if enable_rewrite {
        "改写开"
    } else {
        "仅听写"
    };
    set_status_hwnd(
        ui.status,
        &format!("已保存（{key_note} · {rewrite_note}）· 测连通中…"),
    );
    info!(
        path = %connections.path.display(),
        rewrite_enabled = enable_rewrite,
        timeout_ms,
        key_saved = !connections.config.cliproxyapi.api_key.is_empty(),
        "API settings saved; probing connectivity"
    );

    // Async connectivity probe — never block UI thread / never panic process.
    let hwnd_raw = ui.hwnd.0 as isize;
    let probe_base = connections.config.cliproxyapi.base_url.clone();
    let probe_key = connections.config.cliproxyapi.api_key.clone();
    let probe_path = connections.config.cliproxyapi.models_path.clone();
    thread::spawn(move || {
        let probe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            api_config::probe_connectivity(&probe_base, &probe_key, &probe_path, timeout_ms)
        }))
        .unwrap_or_else(|_| api_config::ConnectivityProbe {
            ok: false,
            status: 0,
            latency_ms: 0,
            url: String::new(),
            error: Some("连通探测线程 panic".to_string()),
        });
        if let Ok(mut slot) = probe_result_slot().lock() {
            *slot = Some(probe);
        }
        let hwnd = HWND(hwnd_raw as *mut _);
        unsafe {
            let _ = PostMessageW(Some(hwnd), PANEL_PROBE_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

fn handle_probe_result() {
    let probe = probe_result_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let Some(ui) = panel_ui_handles() else {
        return;
    };
    let Some(probe) = probe else {
        set_status_hwnd(ui.status, "已保存 · 连通结果丢失，可再点保存重测");
        return;
    };
    let msg = if probe.ok {
        format!(
            "已保存 · Key 已落盘 · 连通 OK · HTTP {} · {} ms",
            probe.status, probe.latency_ms
        )
    } else if probe.status > 0 {
        format!(
            "已保存 · Key 已落盘 · 连通异常 · HTTP {} · {} ms",
            probe.status, probe.latency_ms
        )
    } else {
        let err = probe
            .error
            .as_deref()
            .unwrap_or("网络错误");
        let short = if err.chars().count() > 80 {
            format!("{}…", err.chars().take(80).collect::<String>())
        } else {
            err.to_string()
        };
        format!(
            "已保存 · Key 已落盘 · 连通失败 · {} ms · {short}",
            probe.latency_ms
        )
    };
    set_status_hwnd(ui.status, &msg);
    info!(
        ok = probe.ok,
        status = probe.status,
        latency_ms = probe.latency_ms,
        url = %probe.url,
        "API connectivity probe done"
    );
}

fn set_status(state: &PanelState, text: &str) {
    set_window_text(state.status, text);
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
        let mut buf = vec![0u16; (len as usize) + 1];
        let written = GetWindowTextW(hwnd, &mut buf);
        if written <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..written as usize])
    }
}

fn get_combo_text(hwnd: HWND) -> String {
    let edit = get_window_text(hwnd);
    if !edit.trim().is_empty() {
        return edit;
    }
    unsafe {
        let sel = SendMessageW(hwnd, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        if sel.0 == CB_ERR {
            return String::new();
        }
        let len = SendMessageW(
            hwnd,
            CB_GETLBTEXTLEN,
            Some(WPARAM(sel.0 as usize)),
            Some(LPARAM(0)),
        );
        if len.0 <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len.0 as usize) + 1];
        let written = SendMessageW(
            hwnd,
            CB_GETLBTEXT,
            Some(WPARAM(sel.0 as usize)),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        );
        if written.0 <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..written.0 as usize])
    }
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
            // try_borrow: never panic if a caller holds PANEL_STATE during SetWindowText.
            let brush = PANEL_STATE.with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|b| b.as_ref().map(|s| s.brush_bg))
                    .unwrap_or_default()
            });
            LRESULT(brush.0 as isize)
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
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
                    ID_SAVE => save_fields(),
                    ID_CANCEL => {
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                    ID_FETCH => start_fetch_models(),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        m if m == PANEL_MODELS_DONE => {
            handle_fetch_result();
            LRESULT(0)
        }
        m if m == PANEL_PROBE_DONE => {
            handle_probe_result();
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
