//! Dark native settings panel for OpenAI-compatible rewrite credentials.
//! Spacious layout, NVIDIA preset base URL, model list pull, timeout.

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
    CreateSolidBrush, DEFAULT_GUI_FONT, FillRect, GetStockObject, HBRUSH, SetBkColor, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CBS_DROPDOWN, CBS_HASSTRINGS, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_LEFT, ES_NUMBER,
    ES_PASSWORD, GetClientRect, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU,
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

const PANEL_WIDTH: i32 = 560;
const PANEL_HEIGHT: i32 = 640;
const MARGIN: i32 = 28;
const FIELD_W: i32 = 500;
const FIELD_H: i32 = 36;
const LABEL_H: i32 = 22;
const GAP_AFTER_LABEL: i32 = 8;
const GAP_AFTER_FIELD: i32 = 22;
const HINT_H: i32 = 40;

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

static FETCH_RESULT: OnceLock<Mutex<Option<Result<Vec<String>, String>>>> = OnceLock::new();

fn fetch_result_slot() -> &'static Mutex<Option<Result<Vec<String>, String>>> {
    FETCH_RESULT.get_or_init(|| Mutex::new(None))
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
            fetching: false,
        }
    }
}

thread_local! {
    static PANEL_READY: RefCell<Option<mpsc::Sender<Result<u32, String>>>> =
        const { RefCell::new(None) };
    static PANEL_STATE: RefCell<Option<PanelState>> = const { RefCell::new(None) };
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
    info!(thread_id, "API settings panel thread started");

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
        lpszClassName: w!("ainput_api_settings"),
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
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_api_settings"),
            PCWSTR(title.as_ptr()),
            style,
            160,
            80,
            PANEL_WIDTH,
            PANEL_HEIGHT,
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

        let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        state.brush_bg = unsafe { CreateSolidBrush(BG) };
        state.brush_input = unsafe { CreateSolidBrush(INPUT_BG) };
        state.brush_button = unsafe { CreateSolidBrush(BUTTON_BG) };

        let instance = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let child = |class: PCWSTR,
                     text: &str,
                     style: WINDOW_STYLE,
                     x: i32,
                     y: i32,
                     w: i32,
                     h: i32,
                     id: i32|
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

        let mut y = 24i32;

        let _title = child(
            w!("STATIC"),
            "API / 改写设置",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            26,
            ID_TITLE,
        )?;
        y += 32;
        let _hint = child(
            w!("STATIC"),
            "填写 OpenAI 兼容接口。默认已预填 NVIDIA。Key 只保存在本机。",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            HINT_H,
            ID_HINT,
        )?;
        y += HINT_H + 12;

        let _lbl_url = child(
            w!("STATIC"),
            "Base URL",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_URL,
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
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        let _lbl_model = child(
            w!("STATIC"),
            "Model（可手填，或点「拉取模型」后下拉选择）",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_MODEL,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
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
            FIELD_W - 130,
            220,
            ID_MODEL,
        )?;
        let _fetch = child(
            w!("BUTTON"),
            "拉取模型",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + FIELD_W - 120,
            y,
            120,
            FIELD_H,
            ID_FETCH,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        let _lbl_timeout = child(
            w!("STATIC"),
            "超时（毫秒，连不上时的兜底）",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            LABEL_H,
            ID_LBL_TIMEOUT,
        )?;
        y += LABEL_H + GAP_AFTER_LABEL;
        state.timeout = child(
            w!("EDIT"),
            &DEFAULT_TIMEOUT_MS.to_string(),
            WINDOW_STYLE(WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            160,
            FIELD_H,
            ID_TIMEOUT,
        )?;
        y += FIELD_H + GAP_AFTER_FIELD;

        state.rewrite_check = child(
            w!("BUTTON"),
            "启用 AI 改写（本地听写后润色）",
            WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            FIELD_W,
            28,
            ID_REWRITE,
        )?;
        y += 36;

        state.status = child(
            w!("STATIC"),
            "",
            WINDOW_STYLE(0),
            MARGIN,
            y,
            FIELD_W,
            40,
            ID_STATUS,
        )?;
        y += 48;

        let _save = child(
            w!("BUTTON"),
            "保存",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN,
            y,
            130,
            40,
            ID_SAVE,
        )?;
        let _cancel = child(
            w!("BUTTON"),
            "取消",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            MARGIN + 150,
            y,
            130,
            40,
            ID_CANCEL,
        )?;
        Ok(())
    })
}

unsafe fn show_panel() {
    PANEL_STATE.with(|cell| {
        let borrow = cell.borrow();
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

fn start_fetch_models() {
    let snapshot = PANEL_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow.as_mut()?;
        if state.fetching {
            set_status(state, "正在拉取，请稍候…");
            return None;
        }
        let base = get_window_text(state.base_url);
        let key = get_window_text(state.api_key);
        let timeout_ms = parse_timeout_ms(&get_window_text(state.timeout));
        if base.trim().is_empty() {
            set_status(state, "请先填写 Base URL");
            return None;
        }
        if key.trim().is_empty() {
            set_status(state, "请先填写 API Key 再拉取模型");
            return None;
        }
        state.fetching = true;
        set_status(
            state,
            &format!("正在拉取模型列表…（超时 {timeout_ms} ms）"),
        );
        Some((state.hwnd.0 as isize, base, key, timeout_ms))
    });
    let Some((hwnd_raw, base, key, timeout_ms)) = snapshot else {
        return;
    };
    thread::spawn(move || {
        let result = api_config::list_models(&base, &key, "/v1/models", timeout_ms)
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
    PANEL_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        state.fetching = false;
        match result {
            Some(Ok(models)) => {
                let count = models.len();
                let prefer = get_combo_text(state.model);
                fill_model_combo(state.model, &models, &prefer);
                set_status(state, &format!("已拉取 {count} 个模型，请下拉选择"));
            }
            Some(Err(error)) => {
                let short = if error.chars().count() > 140 {
                    format!("{}…", error.chars().take(140).collect::<String>())
                } else {
                    error
                };
                set_status(state, &format!("拉取失败（可手填 Model）：{short}"));
            }
            None => set_status(state, "拉取结果丢失，请重试"),
        }
    });
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

fn save_fields(state: &PanelState) {
    let base = get_window_text(state.base_url);
    let key = get_window_text(state.api_key);
    let model = get_combo_text(state.model);
    let timeout_ms = parse_timeout_ms(&get_window_text(state.timeout));
    let enable_rewrite = unsafe {
        SendMessageW(
            state.rewrite_check,
            BM_GETCHECK,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 == BST_CHECKED
    };

    if enable_rewrite && base.trim().is_empty() {
        set_status(state, "启用改写前请先填写 Base URL");
        return;
    }
    if enable_rewrite && model.trim().is_empty() {
        set_status(state, "启用改写前请先填写或选择 Model");
        return;
    }
    if enable_rewrite && key.trim().is_empty() {
        set_status(state, "启用改写前请先填写 API Key");
        return;
    }

    let mut config = if state.api_path.exists() {
        match std::fs::read_to_string(&state.api_path)
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
        path: state.api_path.clone(),
        config,
    };
    if let Err(error) = connections.save() {
        set_status(state, &format!("保存失败：{error}"));
        return;
    }

    state.rewrite_language.set_rewrite_enabled(enable_rewrite);
    if let Err(error) = state.rewriter.apply_connection(
        &connections.config.cliproxyapi.base_url,
        &connections.config.cliproxyapi.api_key,
        &connections.config.rewrite.model,
        &connections.config.cliproxyapi.chat_completions_path,
        timeout_ms,
    ) {
        set_status(state, &format!("已写盘，热加载失败：{error}"));
        return;
    }

    let msg = if enable_rewrite {
        format!("已保存 · AI 改写开启 · 超时 {timeout_ms} ms")
    } else {
        "已保存 · 仅本地听写".to_string()
    };
    set_status(state, &msg);
    info!(
        path = %connections.path.display(),
        rewrite_enabled = enable_rewrite,
        timeout_ms,
        "API settings saved"
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
                if let Some(state) = state.borrow().as_ref() {
                    unsafe {
                        let _ = FillRect(hdc, &rect, state.brush_bg);
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
                    .borrow()
                    .as_ref()
                    .map(|s| s.brush_bg)
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
                    .borrow()
                    .as_ref()
                    .map(|s| s.brush_input)
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
                    .borrow()
                    .as_ref()
                    .map(|s| s.brush_button)
                    .unwrap_or_default()
            });
            LRESULT(brush.0 as isize)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if code == BN_CLICKED {
                match id {
                    ID_SAVE => {
                        PANEL_STATE.with(|state| {
                            if let Some(state) = state.borrow().as_ref() {
                                save_fields(state);
                            }
                        });
                    }
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
        WM_CLOSE => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
