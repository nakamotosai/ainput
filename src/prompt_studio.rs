use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use arboard::Clipboard;
use tracing::{info, warn};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, GetStockObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_READONLY, ES_WANTRETURN, GetClientRect, GetMessageW, GetWindowTextLengthW, GetWindowTextW,
    IDC_ARROW, LoadCursorW, MoveWindow, PostThreadMessageW, RegisterClassW, SW_HIDE, SW_RESTORE,
    SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT,
    WM_SIZE, WNDCLASSW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::ai_rewrite::{AiRewriter, default_rewrite_prompt};
use crate::config::RewriteConfig;
use crate::history;

const PROMPT_THREAD_QUIT: u32 = WM_APP + 81;
const PROMPT_OPEN: u32 = WM_APP + 82;
const PANEL_WIDTH: i32 = 1120;
const PANEL_HEIGHT: i32 = 760;
const MARGIN: i32 = 12;
const BUTTON_HEIGHT: i32 = 30;

#[derive(Clone)]
pub struct PromptStudioController {
    thread_id: u32,
}

impl PromptStudioController {
    pub fn start(
        config: RewriteConfig,
        history_path: PathBuf,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        thread::spawn(move || {
            PROMPT_READY.with(|ready| {
                *ready.borrow_mut() = Some(ready_tx);
            });
            PROMPT_STATE.with(|state| {
                *state.borrow_mut() = Some(PromptStudioState::new(config, history_path));
            });
            let result = unsafe { run_prompt_thread(shutdown) };
            if let Err(error) = result {
                warn!(error = %error, "Prompt Studio thread failed");
            }
        });
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("Prompt Studio thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { thread_id })
    }

    pub fn open(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, PROMPT_OPEN, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for PromptStudioController {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, PROMPT_THREAD_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

struct PromptStudioState {
    config: RewriteConfig,
    history_path: PathBuf,
    hwnd: HWND,
    source_hwnd: HWND,
    prompt_hwnd: HWND,
    result_hwnd: HWND,
    status_hwnd: HWND,
    load_button: HWND,
    test_button: HWND,
    copy_button: HWND,
}

impl PromptStudioState {
    fn new(config: RewriteConfig, history_path: PathBuf) -> Self {
        Self {
            config,
            history_path,
            hwnd: HWND::default(),
            source_hwnd: HWND::default(),
            prompt_hwnd: HWND::default(),
            result_hwnd: HWND::default(),
            status_hwnd: HWND::default(),
            load_button: HWND::default(),
            test_button: HWND::default(),
            copy_button: HWND::default(),
        }
    }
}

thread_local! {
    static PROMPT_READY: RefCell<Option<mpsc::Sender<Result<u32, String>>>> =
        const { RefCell::new(None) };
    static PROMPT_STATE: RefCell<Option<PromptStudioState>> = const { RefCell::new(None) };
}

unsafe fn run_prompt_thread(shutdown: Arc<AtomicBool>) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow!("get module handle failed: {error}"))?;
    unsafe { register_prompt_class(HINSTANCE(instance.0))? };
    let hwnd = unsafe { create_prompt_window(HINSTANCE(instance.0))? };
    PROMPT_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.hwnd = hwnd;
        }
    });
    let thread_id = unsafe { GetCurrentThreadId() };
    PROMPT_READY.with(|ready| {
        if let Some(sender) = ready.borrow_mut().take() {
            let _ = sender.send(Ok(thread_id));
        }
    });
    info!(thread_id, "Prompt Studio thread started");
    while !shutdown.load(Ordering::Relaxed) {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if has_message.0 == -1 {
            return Err(anyhow!("Prompt Studio GetMessage failed"));
        }
        if has_message.0 == 0 || msg.message == PROMPT_THREAD_QUIT {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        if msg.message == PROMPT_OPEN {
            show_prompt_studio();
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

unsafe fn register_prompt_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSW {
        lpfnWndProc: Some(prompt_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput_prompt_studio"),
        hCursor: cursor,
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_prompt_window(instance: HINSTANCE) -> Result<HWND> {
    let title = HSTRING::from(format!(
        "ainput Prompt Studio {}",
        env!("CARGO_PKG_VERSION")
    ));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ainput_prompt_studio"),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            120,
            90,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create Prompt Studio window failed: {error}"))
}

unsafe extern "system" fn prompt_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            if let Err(error) = unsafe { create_controls(hwnd) } {
                warn!(error = %error, "create Prompt Studio controls failed");
                return LRESULT(-1);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            layout_controls(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(HWND(lparam.0 as *mut core::ffi::c_void));
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

unsafe fn create_controls(hwnd: HWND) -> Result<()> {
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    let load_button =
        unsafe { create_button(hwnd, "载入最近历史", 12, 12, 116, BUTTON_HEIGHT)? };
    let test_button = unsafe { create_button(hwnd, "测试 Prompt", 140, 12, 112, BUTTON_HEIGHT)? };
    let copy_button = unsafe { create_button(hwnd, "复制结果", 264, 12, 88, BUTTON_HEIGHT)? };
    let status_hwnd = unsafe {
        create_control(
            hwnd,
            "STATIC",
            "Prompt Studio 只显示结果，不自动粘贴。",
            370,
            17,
            700,
            24,
            WS_CHILD | WS_VISIBLE,
        )?
    };
    let source_hwnd = unsafe {
        create_multiline_edit(hwnd, "输入或载入历史文本", 12, 54, 520, 170, false)?
    };
    let prompt_hwnd =
        unsafe { create_multiline_edit(hwnd, default_rewrite_prompt(), 12, 240, 520, 430, false)? };
    let result_hwnd = unsafe { create_multiline_edit(hwnd, "", 548, 54, 540, 616, true)? };
    for control in [
        load_button,
        test_button,
        copy_button,
        status_hwnd,
        source_hwnd,
        prompt_hwnd,
        result_hwnd,
    ] {
        unsafe {
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            )
        };
    }
    PROMPT_STATE.with(|stored| {
        if let Some(state) = stored.borrow_mut().as_mut() {
            state.hwnd = hwnd;
            state.source_hwnd = source_hwnd;
            state.prompt_hwnd = prompt_hwnd;
            state.result_hwnd = result_hwnd;
            state.status_hwnd = status_hwnd;
            state.load_button = load_button;
            state.test_button = test_button;
            state.copy_button = copy_button;
        }
    });
    layout_controls(hwnd);
    Ok(())
}

unsafe fn create_button(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    unsafe { create_control(parent, "BUTTON", text, x, y, w, h, WS_CHILD | WS_VISIBLE) }
}

unsafe fn create_multiline_edit(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    readonly: bool,
) -> Result<HWND> {
    let readonly_style = if readonly { ES_READONLY as u32 } else { 0 };
    unsafe {
        create_control(
            parent,
            "EDIT",
            text,
            x,
            y,
            width,
            height,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_BORDER.0
                    | WS_VSCROLL.0
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | readonly_style,
            ),
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_control(
    parent: HWND,
    class_name: &str,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
) -> Result<HWND> {
    let class_name = HSTRING::from(class_name);
    let text = HSTRING::from(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(text.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            Some(parent),
            None,
            None,
            None,
        )
    }
    .map_err(|error| anyhow!("create Prompt Studio control failed: {error}"))
}

fn show_prompt_studio() {
    PROMPT_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = ShowWindow(state.hwnd, SW_RESTORE);
                let _ = ShowWindow(state.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(state.hwnd);
            }
            set_status(&format!(
                "模型: {} | endpoint: {}",
                state.config.model, state.config.endpoint_url
            ));
        }
    });
}

fn layout_controls(hwnd: HWND) {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let left_w = ((width - MARGIN * 3) / 2).max(360);
    let right_x = MARGIN * 2 + left_w;
    let right_w = (width - right_x - MARGIN).max(320);
    let prompt_y = 240;
    PROMPT_STATE.with(|stored| {
        if let Some(state) = stored.borrow().as_ref() {
            unsafe {
                let _ = MoveWindow(state.source_hwnd, MARGIN, 54, left_w, 170, true);
                let _ = MoveWindow(
                    state.prompt_hwnd,
                    MARGIN,
                    prompt_y,
                    left_w,
                    height - prompt_y - MARGIN,
                    true,
                );
                let _ = MoveWindow(state.result_hwnd, right_x, 54, right_w, height - 66, true);
                let _ = MoveWindow(state.status_hwnd, 370, 17, (width - 382).max(180), 24, true);
            }
        }
    });
}

fn handle_command(control: HWND) {
    let action = PROMPT_STATE.with(|stored| {
        stored.borrow().as_ref().and_then(|state| {
            if control == state.load_button {
                Some("load")
            } else if control == state.test_button {
                Some("test")
            } else if control == state.copy_button {
                Some("copy")
            } else {
                None
            }
        })
    });
    match action {
        Some("load") => load_latest_history(),
        Some("test") => run_prompt_test(),
        Some("copy") => copy_result(),
        _ => {}
    }
}

fn load_latest_history() {
    let (path, source_hwnd) = PROMPT_STATE
        .with(|stored| {
            stored
                .borrow()
                .as_ref()
                .map(|state| (state.history_path.clone(), state.source_hwnd))
        })
        .unwrap_or_default();
    match history::load_recent(&path, 1) {
        Ok(records) => {
            if let Some(record) = records.last() {
                set_window_text(source_hwnd, record.preview_text());
                set_status("已载入最近一条历史文本。");
            } else {
                set_status("暂无历史记录。");
            }
        }
        Err(error) => set_status(&format!("载入历史失败: {error}")),
    }
}

fn run_prompt_test() {
    let (config, source_hwnd, prompt_hwnd, result_hwnd) = PROMPT_STATE
        .with(|stored| {
            stored.borrow().as_ref().map(|state| {
                (
                    state.config.clone(),
                    state.source_hwnd,
                    state.prompt_hwnd,
                    state.result_hwnd,
                )
            })
        })
        .unwrap_or_default();
    let source = get_window_text(source_hwnd);
    let mut prompt = get_window_text(prompt_hwnd);
    if prompt.trim().is_empty() {
        prompt = default_rewrite_prompt().to_string();
    }
    if source.trim().is_empty() {
        set_status("源文本为空。");
        return;
    }
    let started = Instant::now();
    set_status("测试中...");
    match AiRewriter::new(config).and_then(|rewriter| {
        let model = rewriter.model().to_string();
        let endpoint = rewriter.endpoint_url().to_string();
        rewriter
            .rewrite_with_prompt(&source, &prompt)
            .map(|result| (model, endpoint, result))
    }) {
        Ok((model, endpoint, Some(result))) => {
            set_window_text(result_hwnd, &result);
            set_status(&format!(
                "完成 {} ms | model={} | endpoint={}",
                started.elapsed().as_millis(),
                model,
                endpoint
            ));
        }
        Ok((model, _, None)) => {
            set_window_text(result_hwnd, &source);
            set_status(&format!(
                "无改写 {} ms | model={}",
                started.elapsed().as_millis(),
                model
            ));
        }
        Err(error) => {
            set_status(&format!(
                "失败 {} ms: {error}",
                started.elapsed().as_millis()
            ));
        }
    }
}

fn copy_result() {
    let result_hwnd = PROMPT_STATE
        .with(|stored| stored.borrow().as_ref().map(|state| state.result_hwnd))
        .unwrap_or_default();
    let text = get_window_text(result_hwnd);
    if text.trim().is_empty() {
        return;
    }
    match Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
        Ok(()) => set_status("已复制结果。"),
        Err(error) => set_status(&format!("复制失败: {error}")),
    }
}

fn get_window_text(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..copied as usize])
}

fn set_status(text: &str) {
    PROMPT_STATE.with(|stored| {
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
