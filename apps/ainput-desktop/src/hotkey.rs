use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LWIN, VK_RCONTROL, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use winit::event_loop::EventLoopProxy;

#[derive(Clone, Copy)]
pub enum HotkeyState {
    Pressed,
    Released,
}

pub struct GlobalHotkeyMonitor {
    thread_id: u32,
    join_handle: Option<thread::JoinHandle<()>>,
}

static HOTKEY_PROXY: OnceLock<EventLoopProxy<crate::AppEvent>> = OnceLock::new();
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static COMBO_DOWN: AtomicBool = AtomicBool::new(false);

impl GlobalHotkeyMonitor {
    pub fn start(
        proxy: EventLoopProxy<crate::AppEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let _ = HOTKEY_PROXY.set(proxy);
        let (thread_id_tx, thread_id_rx) = std::sync::mpsc::channel();

        let join_handle = thread::spawn(move || unsafe {
            let thread_id = GetCurrentThreadId();
            let _ = thread_id_tx.send(thread_id);

            let instance = GetModuleHandleW(None).ok().map(|module| HINSTANCE(module.0));
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), instance, 0)
                .map_err(|error| anyhow!("install global keyboard hook failed: {error}"));

            let hook = match hook {
                Ok(hook) => hook,
                Err(error) => {
                    if let Some(proxy) = HOTKEY_PROXY.get() {
                        let _ = proxy.send_event(crate::AppEvent::Worker(crate::WorkerEvent::Error(
                            format!("注册全局热键失败：{error}"),
                        )));
                    }
                    return;
                }
            };

            let mut msg = MSG::default();
            while !shutdown.load(Ordering::Relaxed) && GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(hook);
            CTRL_DOWN.store(false, Ordering::Relaxed);
            WIN_DOWN.store(false, Ordering::Relaxed);
            COMBO_DOWN.store(false, Ordering::Relaxed);
        });

        let thread_id = thread_id_rx
            .recv()
            .map_err(|_| anyhow!("read hotkey thread id failed"))?;

        Ok(Self {
            thread_id,
            join_handle: Some(join_handle),
        })
    }
}

impl Drop for GlobalHotkeyMonitor {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let keyboard = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;

        if is_down || is_up {
            update_modifier_state(VIRTUAL_KEY(keyboard.vkCode as u16), is_down);
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn update_modifier_state(vk: VIRTUAL_KEY, is_down: bool) {
    match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
            CTRL_DOWN.store(is_down, Ordering::Relaxed);
        }
        VK_LWIN | VK_RWIN => {
            WIN_DOWN.store(is_down, Ordering::Relaxed);
        }
        _ => return,
    }

    let now_pressed = CTRL_DOWN.load(Ordering::Relaxed) && WIN_DOWN.load(Ordering::Relaxed);
    let was_pressed = COMBO_DOWN.swap(now_pressed, Ordering::Relaxed);
    if now_pressed != was_pressed {
        if let Some(proxy) = HOTKEY_PROXY.get() {
            let event = if now_pressed {
                HotkeyState::Pressed
            } else {
                HotkeyState::Released
            };
            let _ = proxy.send_event(crate::AppEvent::Hotkey(event));
        }
    }
}
