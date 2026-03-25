use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL,
    VK_LCONTROL, VK_LWIN, VK_RCONTROL, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
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
static WIN_PENDING: AtomicBool = AtomicBool::new(false);
static WIN_PASSTHROUGH_ACTIVE: AtomicBool = AtomicBool::new(false);
static WIN_SUPPRESS_UNTIL_UP: AtomicBool = AtomicBool::new(false);
static WIN_TRACKED_VK: AtomicU64 = AtomicU64::new(0);
static MOUSE_MIDDLE_ENABLED: AtomicBool = AtomicBool::new(true);
static MOUSE_MIDDLE_DOWN: AtomicBool = AtomicBool::new(false);
static MOUSE_MIDDLE_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_MIDDLE_TOKEN: AtomicU64 = AtomicU64::new(0);

const MOUSE_MIDDLE_HOLD_DELAY_MS: u64 = 200;

impl GlobalHotkeyMonitor {
    pub fn start(
        proxy: EventLoopProxy<crate::AppEvent>,
        shutdown: Arc<AtomicBool>,
        mouse_middle_enabled: bool,
    ) -> Result<Self> {
        let _ = HOTKEY_PROXY.set(proxy);
        MOUSE_MIDDLE_ENABLED.store(mouse_middle_enabled, Ordering::Relaxed);
        let (thread_id_tx, thread_id_rx) = std::sync::mpsc::channel();

        let join_handle = thread::spawn(move || unsafe {
            let thread_id = GetCurrentThreadId();
            let _ = thread_id_tx.send(thread_id);

            let instance = GetModuleHandleW(None)
                .ok()
                .map(|module| HINSTANCE(module.0));
            let keyboard_hook =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), instance, 0)
                    .map_err(|error| anyhow!("install global keyboard hook failed: {error}"));
            let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), instance, 0)
                .map_err(|error| anyhow!("install global mouse hook failed: {error}"));

            let (keyboard_hook, mouse_hook) = match (keyboard_hook, mouse_hook) {
                (Ok(keyboard_hook), Ok(mouse_hook)) => (keyboard_hook, mouse_hook),
                (Err(error), _) => {
                    if let Some(proxy) = HOTKEY_PROXY.get() {
                        let _ = proxy.send_event(crate::AppEvent::Worker(
                            crate::WorkerEvent::Error(format!("注册全局热键失败：{error}")),
                        ));
                    }
                    return;
                }
                (_, Err(error)) => {
                    if let Some(proxy) = HOTKEY_PROXY.get() {
                        let _ = proxy.send_event(crate::AppEvent::Worker(
                            crate::WorkerEvent::Error(format!("注册鼠标热键失败：{error}")),
                        ));
                    }
                    return;
                }
            };

            let mut msg = MSG::default();
            while !shutdown.load(Ordering::Relaxed) && GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(keyboard_hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
            CTRL_DOWN.store(false, Ordering::Relaxed);
            WIN_DOWN.store(false, Ordering::Relaxed);
            COMBO_DOWN.store(false, Ordering::Relaxed);
            WIN_PENDING.store(false, Ordering::Relaxed);
            WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
            WIN_SUPPRESS_UNTIL_UP.store(false, Ordering::Relaxed);
            WIN_TRACKED_VK.store(0, Ordering::Relaxed);
            MOUSE_MIDDLE_DOWN.store(false, Ordering::Relaxed);
            MOUSE_MIDDLE_ACTIVE.store(false, Ordering::Relaxed);
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

pub fn set_mouse_middle_enabled(enabled: bool) {
    MOUSE_MIDDLE_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        if MOUSE_MIDDLE_ACTIVE.swap(false, Ordering::Relaxed) {
            if let Some(proxy) = HOTKEY_PROXY.get() {
                let _ = proxy.send_event(crate::AppEvent::Hotkey(HotkeyState::Released));
            }
        }
        MOUSE_MIDDLE_DOWN.store(false, Ordering::Relaxed);
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

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let keyboard = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;

        if is_down || is_up {
            let vk = VIRTUAL_KEY(keyboard.vkCode as u16);
            if WIN_PENDING.load(Ordering::Relaxed)
                && is_down
                && !matches!(
                    vk,
                    VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_LWIN | VK_RWIN
                )
            {
                flush_pending_win_press();
            }
            if update_modifier_state(vk, is_down) {
                return LRESULT(1);
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    if !MOUSE_MIDDLE_ENABLED.load(Ordering::Relaxed) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let mouse = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
    if mouse.flags & 0x0000_0001 != 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    match wparam.0 as u32 {
        WM_MBUTTONDOWN => {
            handle_middle_button_down();
            return LRESULT(1);
        }
        WM_MBUTTONUP => {
            handle_middle_button_up();
            return LRESULT(1);
        }
        _ => {}
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn update_modifier_state(vk: VIRTUAL_KEY, is_down: bool) -> bool {
    match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
            let was_ctrl_down = CTRL_DOWN.swap(is_down, Ordering::Relaxed);

            if is_down
                && !was_ctrl_down
                && WIN_DOWN.load(Ordering::Relaxed)
                && WIN_PENDING.swap(false, Ordering::Relaxed)
            {
                WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                WIN_SUPPRESS_UNTIL_UP.store(true, Ordering::Relaxed);
                if !COMBO_DOWN.swap(true, Ordering::Relaxed) {
                    send_hotkey_state(HotkeyState::Pressed);
                }
                return false;
            }

            if !is_down && was_ctrl_down && COMBO_DOWN.swap(false, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::Released);
                WIN_PENDING.store(false, Ordering::Relaxed);
                if WIN_DOWN.load(Ordering::Relaxed) {
                    WIN_SUPPRESS_UNTIL_UP.store(true, Ordering::Relaxed);
                }
            }

            false
        }
        VK_LWIN | VK_RWIN => {
            if is_down {
                WIN_DOWN.store(true, Ordering::Relaxed);
                WIN_TRACKED_VK.store(vk.0 as u64, Ordering::Relaxed);

                if CTRL_DOWN.load(Ordering::Relaxed) {
                    WIN_PENDING.store(false, Ordering::Relaxed);
                    WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                    WIN_SUPPRESS_UNTIL_UP.store(true, Ordering::Relaxed);
                    if !COMBO_DOWN.swap(true, Ordering::Relaxed) {
                        send_hotkey_state(HotkeyState::Pressed);
                    }
                    return true;
                }

                WIN_PENDING.store(true, Ordering::Relaxed);
                WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                return true;
            }

            WIN_DOWN.store(false, Ordering::Relaxed);

            if COMBO_DOWN.swap(false, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::Released);
                WIN_PENDING.store(false, Ordering::Relaxed);
                WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                WIN_SUPPRESS_UNTIL_UP.store(false, Ordering::Relaxed);
                WIN_TRACKED_VK.store(0, Ordering::Relaxed);
                return true;
            }

            if WIN_SUPPRESS_UNTIL_UP.swap(false, Ordering::Relaxed) {
                WIN_PENDING.store(false, Ordering::Relaxed);
                WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                WIN_TRACKED_VK.store(0, Ordering::Relaxed);
                return true;
            }

            if WIN_PENDING.swap(false, Ordering::Relaxed) {
                WIN_PASSTHROUGH_ACTIVE.store(false, Ordering::Relaxed);
                let tracked = VIRTUAL_KEY(WIN_TRACKED_VK.swap(0, Ordering::Relaxed) as u16);
                synthesize_win_key_press_and_release(tracked);
                return true;
            }

            if WIN_PASSTHROUGH_ACTIVE.swap(false, Ordering::Relaxed) {
                WIN_TRACKED_VK.store(0, Ordering::Relaxed);
                return false;
            }

            WIN_TRACKED_VK.store(0, Ordering::Relaxed);
            false
        }
        _ => false,
    }
}

fn handle_middle_button_down() {
    MOUSE_MIDDLE_DOWN.store(true, Ordering::Relaxed);
    MOUSE_MIDDLE_ACTIVE.store(false, Ordering::Relaxed);
    let token = MOUSE_MIDDLE_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(MOUSE_MIDDLE_HOLD_DELAY_MS));

        if !MOUSE_MIDDLE_ENABLED.load(Ordering::Relaxed) {
            return;
        }

        if !MOUSE_MIDDLE_DOWN.load(Ordering::Relaxed) {
            return;
        }

        if MOUSE_MIDDLE_TOKEN.load(Ordering::Relaxed) != token {
            return;
        }

        if !MOUSE_MIDDLE_ACTIVE.swap(true, Ordering::Relaxed) {
            if let Some(proxy) = HOTKEY_PROXY.get() {
                let _ = proxy.send_event(crate::AppEvent::Hotkey(HotkeyState::Pressed));
            }
        }
    });
}

fn handle_middle_button_up() {
    let was_down = MOUSE_MIDDLE_DOWN.swap(false, Ordering::Relaxed);
    if !was_down {
        return;
    }

    if MOUSE_MIDDLE_ACTIVE.swap(false, Ordering::Relaxed) {
        if let Some(proxy) = HOTKEY_PROXY.get() {
            let _ = proxy.send_event(crate::AppEvent::Hotkey(HotkeyState::Released));
        }
        return;
    }

    synthesize_middle_click();
}

fn synthesize_middle_click() {
    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MIDDLEDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MIDDLEUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];

    let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn flush_pending_win_press() {
    if !WIN_PENDING.swap(false, Ordering::Relaxed) {
        return;
    }

    let vk = VIRTUAL_KEY(WIN_TRACKED_VK.load(Ordering::Relaxed) as u16);
    synthesize_win_key(vk, KEYBD_EVENT_FLAGS(0));
    WIN_PASSTHROUGH_ACTIVE.store(true, Ordering::Relaxed);
}

fn synthesize_win_key_press_and_release(vk: VIRTUAL_KEY) {
    synthesize_win_key(vk, KEYBD_EVENT_FLAGS(0));
    synthesize_win_key(vk, KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0));
}

fn synthesize_win_key(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) {
    let inputs = [INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }];
    let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn send_hotkey_state(state: HotkeyState) {
    if let Some(proxy) = HOTKEY_PROXY.get() {
        let _ = proxy.send_event(crate::AppEvent::Hotkey(state));
    }
}
