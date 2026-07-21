//! Dark native settings panel for OpenAI-compatible rewrite credentials.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{
    Arc,
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
    BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, ES_LEFT, ES_PASSWORD, GetClientRect, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, LoadCursorW, MSG, PostThreadMessageW,
    RegisterClassW, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::ai_rewrite::SharedRewriter;
use crate::api_config::{ApiConnections, ApiConnectionsConfig};
use crate::rewrite_language::RewriteLanguageController;

const PANEL_THREAD_QUIT: u32 = WM_APP + 121;
const PANEL_OPEN: u32 = WM_APP + 122;
const PANEL_WIDTH: i32 = 520;
const PANEL_HEIGHT: i32 = 460;

const ID_BASE_URL: i32 = 4001;
const ID_API_KEY: i32 = 4002;
const ID_MODEL: i32 = 4003;
const ID_REWRITE: i32 = 4004;
const ID_SAVE: i32 = 4005;
const ID_CANCEL: i32 = 4006;
const ID_STATUS: i32 = 4007;
const ID_TITLE: i32 = 4010;
const ID_HINT: i32 = 4011;
const ID_LBL_URL: i32 = 4012;
const ID_LBL_KEY: i32 = 4013;
const ID_LBL_MODEL: i32 = 4014;

// BM_SETCHECK / BM_GETCHECK
const BM_SETCHECK: u32 = 0x00F1;
const BM_GETCHECK: u32 = 0x00F0;
const BST_UNCHECKED: isize = 0;
const BST_CHECKED: isize = 1;

// Dark palette (COLORREF is 0x00BBGGRR)
const BG: COLORREF = COLORREF(0x00_16_14_14);
const INPUT_BG: COLORREF = COLORREF(0x00_2A_24_24);
const TEXT: COLORREF = COLORREF(0x00_F2_F0_F0);
const BUTTON_BG: COLORREF = COLORREF(0x00_32_2A_2A);

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
    rewrite_check: HWND,
    status: HWND,
    brush_bg: HBRUSH,
    brush_input: HBRUSH,
    brush_button: HBRUSH,
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
            rewrite_check: HWND::default(),
            status: HWND::default(),
            brush_bg: HBRUSH::default(),
            brush_input: HBRUSH::default(),
            brush_button: HBRUSH::default(),
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
            120,
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
            let _ = font;
            Ok(child_hwnd)
        };

        let margin = 28i32;
        let field_w = 460i32;
        let field_h = 34i32;
        let mut y = 22i32;

        let _title = child(
            w!("STATIC"),
            "API / 改写设置",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            28,
            ID_TITLE,
        )?;
        y += 34;
        let _hint = child(
            w!("STATIC"),
            "填写 OpenAI 兼容接口。Key 只保存在本机 state/config。",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            22,
            ID_HINT,
        )?;
        y += 34;

        let _lbl_url = child(
            w!("STATIC"),
            "Base URL",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            20,
            ID_LBL_URL,
        )?;
        y += 22;
        state.base_url = child(
            w!("EDIT"),
            "",
            WINDOW_STYLE(WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_LEFT as u32 | WS_TABSTOP.0),
            margin,
            y,
            field_w,
            field_h,
            ID_BASE_URL,
        )?;
        y += field_h + 16;

        let _lbl_key = child(
            w!("STATIC"),
            "API Key",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            20,
            ID_LBL_KEY,
        )?;
        y += 22;
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
            margin,
            y,
            field_w,
            field_h,
            ID_API_KEY,
        )?;
        y += field_h + 16;

        let _lbl_model = child(
            w!("STATIC"),
            "Model",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            20,
            ID_LBL_MODEL,
        )?;
        y += 22;
        state.model = child(
            w!("EDIT"),
            "",
            WINDOW_STYLE(WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_LEFT as u32 | WS_TABSTOP.0),
            margin,
            y,
            field_w,
            field_h,
            ID_MODEL,
        )?;
        y += field_h + 18;

        state.rewrite_check = child(
            w!("BUTTON"),
            "启用 AI 改写（本地听写后润色）",
            WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0),
            margin,
            y,
            field_w,
            26,
            ID_REWRITE,
        )?;
        y += 40;

        state.status = child(
            w!("STATIC"),
            "",
            WINDOW_STYLE(0),
            margin,
            y,
            field_w,
            22,
            ID_STATUS,
        )?;
        y += 34;

        let _save = child(
            w!("BUTTON"),
            "保存",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            margin,
            y,
            120,
            36,
            ID_SAVE,
        )?;
        let _cancel = child(
            w!("BUTTON"),
            "取消",
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            margin + 136,
            y,
            120,
            36,
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
    let (base, key, model) = match load_api_file(&state.api_path) {
        Ok(v) => v,
        Err(error) => {
            set_status(state, &format!("读取配置失败：{error}"));
            (String::new(), String::new(), String::new())
        }
    };
    set_window_text(state.base_url, &base);
    set_window_text(state.api_key, &key);
    set_window_text(state.model, &model);
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
        if base.trim().is_empty() {
            "未配置 API · 仅本地听写"
        } else {
            "已加载本地配置"
        },
    );
}

fn load_api_file(path: &std::path::Path) -> Result<(String, String, String)> {
    if !path.exists() {
        return Ok((String::new(), String::new(), String::new()));
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let config: ApiConnectionsConfig =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok((
        config.cliproxyapi.base_url,
        config.cliproxyapi.api_key,
        config.rewrite.model,
    ))
}

fn save_fields(state: &PanelState) {
    let base = get_window_text(state.base_url);
    let key = get_window_text(state.api_key);
    let model = get_window_text(state.model);
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
        set_status(state, "启用改写前请先填写 Model");
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
    ) {
        set_status(state, &format!("已写盘，热加载失败：{error}"));
        return;
    }

    set_status(
        state,
        if enable_rewrite {
            "已保存 · AI 改写已开启"
        } else {
            "已保存 · 仅本地听写"
        },
    );
    info!(
        path = %connections.path.display(),
        rewrite_enabled = enable_rewrite,
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
        WM_CTLCOLOREDIT => {
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
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
