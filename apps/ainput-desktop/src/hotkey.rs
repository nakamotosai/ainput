use std::sync::{
    Arc, OnceLock, RwLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEINPUT,
    SendInput, VIRTUAL_KEY, VK_ADD, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DECIMAL, VK_DELETE,
    VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8,
    VK_F9, VK_F10, VK_F11, VK_F12, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMLOCK, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2,
    VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_PAUSE,
    VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_SNAPSHOT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use winit::event_loop::EventLoopProxy;

const HOTKEY_CONTROL_MESSAGE: u32 = WM_APP + 7;
const MOUSE_MIDDLE_HOLD_DELAY_MS: u64 = 200;
const VOICE_HOTKEY_RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(8);
const MODIFIER_ONLY_VOICE_TRIGGER_DELAY_MS: u64 = 260;
const VOICE_HOTKEY_OUTPUT_COOLDOWN_MS: u64 = 350;

#[derive(Clone, Copy)]
pub enum HotkeyState {
    VoicePressed,
    VoiceReleased,
    ScreenshotTriggered,
    RecordingStartTriggered,
    RecordingStopTriggered,
    RecordingCancelTriggered,
    AutomationPauseTriggered,
    AutomationRecordTriggered,
    AutomationStopTriggered,
    AutomationPlayTriggered,
    AutomationCancelTriggered,
}

#[derive(Debug, Clone)]
pub struct HotkeyBindings {
    pub voice_input: String,
    pub screen_capture: String,
}

pub struct GlobalHotkeyMonitor {
    thread_id: u32,
    join_handle: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
struct HotkeyRuntimeConfig {
    voice: ParsedHotkey,
    screenshot: ParsedHotkey,
}

#[derive(Clone, Debug)]
struct ParsedHotkey {
    modifiers: HotkeyModifiers,
    key: Option<VIRTUAL_KEY>,
}

#[derive(Clone, Copy, Debug, Default)]
struct HotkeyModifiers {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
}

static HOTKEY_PROXY: OnceLock<EventLoopProxy<crate::AppEvent>> = OnceLock::new();
static HOTKEY_CONFIG: OnceLock<RwLock<HotkeyRuntimeConfig>> = OnceLock::new();
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static SCREENSHOT_ACTIVE: AtomicBool = AtomicBool::new(false);
static VOICE_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING_START_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING_STOP_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING_CANCEL_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING_CANCEL_ENABLED: AtomicBool = AtomicBool::new(false);
static AUTOMATION_PAUSE_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUTOMATION_RECORD_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUTOMATION_STOP_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUTOMATION_PLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUTOMATION_CANCEL_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUTOMATION_CANCEL_ENABLED: AtomicBool = AtomicBool::new(false);
static MOUSE_MIDDLE_ENABLED: AtomicBool = AtomicBool::new(true);
static MOUSE_MIDDLE_DOWN: AtomicBool = AtomicBool::new(false);
static MOUSE_MIDDLE_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE_MIDDLE_TOKEN: AtomicU64 = AtomicU64::new(0);
static MODIFIER_ONLY_VOICE_PENDING: AtomicBool = AtomicBool::new(false);
static MODIFIER_ONLY_VOICE_PASSTHROUGH: AtomicBool = AtomicBool::new(false);
static MODIFIER_ONLY_VOICE_TOKEN: AtomicU64 = AtomicU64::new(0);
static VOICE_HOTKEY_SUPPRESSION_COUNT: AtomicUsize = AtomicUsize::new(0);
static VOICE_HOTKEY_SUPPRESS_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

pub struct VoiceHotkeySuppressionGuard {
    active: bool,
}

impl Drop for VoiceHotkeySuppressionGuard {
    fn drop(&mut self) {
        if self.active {
            VOICE_HOTKEY_SUPPRESSION_COUNT.fetch_sub(1, Ordering::Relaxed);
            VOICE_HOTKEY_SUPPRESS_UNTIL_MS.store(
                now_epoch_ms().saturating_add(VOICE_HOTKEY_OUTPUT_COOLDOWN_MS),
                Ordering::Relaxed,
            );
            self.active = false;
        }
    }
}

impl GlobalHotkeyMonitor {
    pub fn start(
        proxy: EventLoopProxy<crate::AppEvent>,
        shutdown: Arc<AtomicBool>,
        bindings: HotkeyBindings,
        mouse_middle_enabled: bool,
    ) -> Result<Self> {
        let parsed = HotkeyRuntimeConfig {
            voice: parse_hotkey(&bindings.voice_input).map_err(|error| {
                anyhow!("invalid voice hotkey {}: {error}", bindings.voice_input)
            })?,
            screenshot: parse_hotkey(&bindings.screen_capture).map_err(|error| {
                anyhow!(
                    "invalid screenshot hotkey {}: {error}",
                    bindings.screen_capture
                )
            })?,
        };

        let _ = HOTKEY_PROXY.set(proxy);
        let _ = HOTKEY_CONFIG.set(RwLock::new(parsed.clone()));
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
                    .map_err(|error| anyhow!("install keyboard hook failed: {error}"));
            let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), instance, 0)
                .map_err(|error| anyhow!("install mouse hook failed: {error}"));

            let (keyboard_hook, mouse_hook) = match (keyboard_hook, mouse_hook) {
                (Ok(keyboard_hook), Ok(mouse_hook)) => (keyboard_hook, mouse_hook),
                (Err(error), _) | (_, Err(error)) => {
                    send_error(format!("注册全局热键失败：{error}"));
                    return;
                }
            };

            let mut msg = MSG::default();
            while !shutdown.load(Ordering::Relaxed) && GetMessageW(&mut msg, None, 0, 0).into() {
                match msg.message {
                    HOTKEY_CONTROL_MESSAGE => {}
                    _ => {
                        let _ = TranslateMessage(&msg);
                        let _ = DispatchMessageW(&msg);
                    }
                }
            }

            let _ = UnhookWindowsHookEx(keyboard_hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
            reset_hotkey_state();
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

pub fn set_voice_input_binding(voice_input: &str) -> Result<()> {
    let parsed = parse_hotkey(voice_input)
        .map_err(|error| anyhow!("invalid voice hotkey {voice_input}: {error}"))?;
    if let Some(lock) = HOTKEY_CONFIG.get() {
        let mut guard = lock
            .write()
            .map_err(|_| anyhow!("hotkey config write lock poisoned"))?;
        guard.voice = parsed;
    }
    Ok(())
}

pub fn set_mouse_middle_enabled(enabled: bool) {
    MOUSE_MIDDLE_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        if MOUSE_MIDDLE_ACTIVE.swap(false, Ordering::Relaxed) {
            send_hotkey_state(HotkeyState::VoiceReleased);
        }
        MOUSE_MIDDLE_DOWN.store(false, Ordering::Relaxed);
    }
}

pub fn set_automation_cancel_enabled(enabled: bool) {
    AUTOMATION_CANCEL_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        AUTOMATION_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
    }
}

pub fn set_recording_cancel_enabled(enabled: bool) {
    RECORDING_CANCEL_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        RECORDING_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
    }
}

pub fn wait_for_voice_hotkey_release(timeout: Duration) -> bool {
    let Some(config) = current_config() else {
        return true;
    };

    if !config.voice.modifiers.any() {
        return true;
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if config.voice.modifiers.fully_released() {
            return true;
        }
        thread::sleep(VOICE_HOTKEY_RELEASE_POLL_INTERVAL);
    }

    config.voice.modifiers.fully_released()
}

pub fn voice_hotkey_uses_alt() -> bool {
    current_config().is_some_and(|config| config.voice.modifiers.alt)
}

pub fn suppress_voice_hotkey_for_output() -> VoiceHotkeySuppressionGuard {
    VOICE_HOTKEY_SUPPRESSION_COUNT.fetch_add(1, Ordering::Relaxed);
    cancel_pending_modifier_only_voice_trigger();
    VOICE_ACTIVE.store(false, Ordering::Relaxed);
    VoiceHotkeySuppressionGuard { active: true }
}

pub fn reset_hotkey_state() {
    CTRL_DOWN.store(false, Ordering::Relaxed);
    ALT_DOWN.store(false, Ordering::Relaxed);
    SHIFT_DOWN.store(false, Ordering::Relaxed);
    WIN_DOWN.store(false, Ordering::Relaxed);
    SCREENSHOT_ACTIVE.store(false, Ordering::Relaxed);
    VOICE_ACTIVE.store(false, Ordering::Relaxed);
    RECORDING_START_ACTIVE.store(false, Ordering::Relaxed);
    RECORDING_STOP_ACTIVE.store(false, Ordering::Relaxed);
    RECORDING_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
    AUTOMATION_PAUSE_ACTIVE.store(false, Ordering::Relaxed);
    AUTOMATION_RECORD_ACTIVE.store(false, Ordering::Relaxed);
    AUTOMATION_STOP_ACTIVE.store(false, Ordering::Relaxed);
    AUTOMATION_PLAY_ACTIVE.store(false, Ordering::Relaxed);
    AUTOMATION_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
    MOUSE_MIDDLE_DOWN.store(false, Ordering::Relaxed);
    MOUSE_MIDDLE_ACTIVE.store(false, Ordering::Relaxed);
    MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
    MODIFIER_ONLY_VOICE_PASSTHROUGH.store(false, Ordering::Relaxed);
    VOICE_HOTKEY_SUPPRESS_UNTIL_MS.store(0, Ordering::Relaxed);
}

impl Drop for GlobalHotkeyMonitor {
    fn drop(&mut self) {
        let _ = unsafe {
            PostThreadMessageW(self.thread_id, HOTKEY_CONTROL_MESSAGE, WPARAM(0), LPARAM(0))
        };
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
        let vk = VIRTUAL_KEY(keyboard.vkCode as u16);

        if keyboard.flags.0 & 0x10 != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }

        update_modifier_state(vk, is_down, is_up);

        if matches!(vk, VIRTUAL_KEY(0x58) | VK_MENU | VK_LMENU | VK_RMENU) && (is_down || is_up) {
            tracing::debug!(
                event_vk = vk.0,
                scan_code = keyboard.scanCode,
                flags = keyboard.flags.0,
                message,
                is_down,
                is_up,
                ctrl_down = CTRL_DOWN.load(Ordering::Relaxed),
                alt_down = ALT_DOWN.load(Ordering::Relaxed),
                shift_down = SHIFT_DOWN.load(Ordering::Relaxed),
                win_down = WIN_DOWN.load(Ordering::Relaxed),
                "observed raw hotkey-related key event"
            );
        }

        if let Some(config) = current_config() {
            if config.voice.key.is_none() {
                if let Some(result) =
                    handle_modifier_only_voice_hotkey(vk, is_down, is_up, &config.voice)
                {
                    return result;
                }
            }

            if vk == VK_F1 && is_down && !RECORDING_START_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::RecordingStartTriggered);
                return LRESULT(1);
            }
            if vk == VK_F1 && is_up {
                RECORDING_START_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if vk == VK_F2 && is_down && !RECORDING_STOP_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::RecordingStopTriggered);
                return LRESULT(1);
            }
            if vk == VK_F2 && is_up {
                RECORDING_STOP_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if RECORDING_CANCEL_ENABLED.load(Ordering::Relaxed)
                && vk == VK_ESCAPE
                && is_down
                && !RECORDING_CANCEL_ACTIVE.swap(true, Ordering::Relaxed)
            {
                send_hotkey_state(HotkeyState::RecordingCancelTriggered);
                return LRESULT(1);
            }
            if RECORDING_CANCEL_ENABLED.load(Ordering::Relaxed) && vk == VK_ESCAPE && is_up {
                RECORDING_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if vk == VK_F7 && is_down && !AUTOMATION_PAUSE_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::AutomationPauseTriggered);
                return LRESULT(1);
            }
            if vk == VK_F7 && is_up {
                AUTOMATION_PAUSE_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if vk == VK_F8 && is_down && !AUTOMATION_RECORD_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::AutomationRecordTriggered);
                return LRESULT(1);
            }
            if vk == VK_F8 && is_up {
                AUTOMATION_RECORD_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if vk == VK_F9 && is_down && !AUTOMATION_STOP_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::AutomationStopTriggered);
                return LRESULT(1);
            }
            if vk == VK_F9 && is_up {
                AUTOMATION_STOP_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if vk == VK_F10 && is_down && !AUTOMATION_PLAY_ACTIVE.swap(true, Ordering::Relaxed) {
                send_hotkey_state(HotkeyState::AutomationPlayTriggered);
                return LRESULT(1);
            }
            if vk == VK_F10 && is_up {
                AUTOMATION_PLAY_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if AUTOMATION_CANCEL_ENABLED.load(Ordering::Relaxed)
                && vk == VK_ESCAPE
                && is_down
                && !AUTOMATION_CANCEL_ACTIVE.swap(true, Ordering::Relaxed)
            {
                send_hotkey_state(HotkeyState::AutomationCancelTriggered);
                return LRESULT(1);
            }
            if AUTOMATION_CANCEL_ENABLED.load(Ordering::Relaxed) && vk == VK_ESCAPE && is_up {
                AUTOMATION_CANCEL_ACTIVE.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }

            if let Some(primary_key) = config.screenshot.key {
                if vk == primary_key
                    && is_down
                    && config.screenshot.modifiers.matches_pressed()
                    && !SCREENSHOT_ACTIVE.swap(true, Ordering::Relaxed)
                {
                    tracing::info!(
                        screenshot_vk = primary_key.0,
                        event_vk = vk.0,
                        ctrl_down = CTRL_DOWN.load(Ordering::Relaxed),
                        alt_down = ALT_DOWN.load(Ordering::Relaxed),
                        shift_down = SHIFT_DOWN.load(Ordering::Relaxed),
                        win_down = WIN_DOWN.load(Ordering::Relaxed),
                        "screenshot hotkey matched in keyboard hook"
                    );
                    send_hotkey_state(HotkeyState::ScreenshotTriggered);
                    return LRESULT(1);
                }

                if vk == primary_key && is_up {
                    tracing::info!(
                        screenshot_vk = primary_key.0,
                        event_vk = vk.0,
                        "screenshot hotkey released in keyboard hook"
                    );
                    SCREENSHOT_ACTIVE.store(false, Ordering::Relaxed);
                    return LRESULT(1);
                }
            }

            if SCREENSHOT_ACTIVE.load(Ordering::Relaxed)
                && config.screenshot.modifiers.any_released_requirement()
            {
                SCREENSHOT_ACTIVE.store(false, Ordering::Relaxed);
            }

            if voice_hotkey_suppressed_for_output() {
                cancel_pending_modifier_only_voice_trigger();
                VOICE_ACTIVE.store(false, Ordering::Relaxed);
                return unsafe { CallNextHookEx(None, code, wparam, lparam) };
            }

            if let Some(primary_key) = config.voice.key {
                if vk == primary_key && is_down && config.voice.modifiers.matches_pressed() {
                    if !VOICE_ACTIVE.swap(true, Ordering::Relaxed) {
                        send_hotkey_state(HotkeyState::VoicePressed);
                    }
                    return LRESULT(1);
                }
                if vk == primary_key && is_up && VOICE_ACTIVE.swap(false, Ordering::Relaxed) {
                    send_hotkey_state(HotkeyState::VoiceReleased);
                    return LRESULT(1);
                }
            } else if is_down
                && config.voice.modifiers.matches_pressed()
                && !VOICE_ACTIVE.swap(true, Ordering::Relaxed)
            {
                send_hotkey_state(HotkeyState::VoicePressed);
            }

            if VOICE_ACTIVE.load(Ordering::Relaxed)
                && config.voice.modifiers.any_released_requirement()
                && VOICE_ACTIVE.swap(false, Ordering::Relaxed)
            {
                send_hotkey_state(HotkeyState::VoiceReleased);
                return unsafe { CallNextHookEx(None, code, wparam, lparam) };
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn handle_modifier_only_voice_hotkey(
    vk: VIRTUAL_KEY,
    is_down: bool,
    is_up: bool,
    voice: &ParsedHotkey,
) -> Option<LRESULT> {
    if !voice.modifiers.any() {
        return None;
    }

    let relevant_modifier = voice.modifiers.contains_vk(vk);
    let exact_match = voice.modifiers.exactly_pressed();
    let passthrough = MODIFIER_ONLY_VOICE_PASSTHROUGH.load(Ordering::Relaxed);
    let pending = MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed);
    let active = VOICE_ACTIVE.load(Ordering::Relaxed);

    if passthrough {
        if is_up && relevant_modifier && voice.modifiers.fully_released() {
            MODIFIER_ONLY_VOICE_PASSTHROUGH.store(false, Ordering::Relaxed);
        }
        return None;
    }

    if is_down && !relevant_modifier {
        if pending || active {
            cancel_modifier_only_voice_capture(active);
        }
        return None;
    }

    if is_down && relevant_modifier && exact_match {
        if !pending && !active {
            arm_modifier_only_voice_pending(voice.modifiers);
        }
        return None;
    }

    if is_down && relevant_modifier {
        if pending || active {
            cancel_modifier_only_voice_capture(active);
        }
        return None;
    }

    if is_up && relevant_modifier {
        MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
        if active && VOICE_ACTIVE.swap(false, Ordering::Relaxed) {
            tracing::info!("modifier-only voice hotkey released in keyboard hook");
            send_hotkey_state(HotkeyState::VoiceReleased);
        }
    }

    None
}

fn voice_hotkey_suppressed_for_output() -> bool {
    VOICE_HOTKEY_SUPPRESSION_COUNT.load(Ordering::Relaxed) > 0
        || now_epoch_ms() < VOICE_HOTKEY_SUPPRESS_UNTIL_MS.load(Ordering::Relaxed)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn cancel_pending_modifier_only_voice_trigger() {
    MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
    MODIFIER_ONLY_VOICE_PASSTHROUGH.store(false, Ordering::Relaxed);
    MODIFIER_ONLY_VOICE_TOKEN.fetch_add(1, Ordering::Relaxed);
}

fn arm_modifier_only_voice_pending(modifiers: HotkeyModifiers) {
    MODIFIER_ONLY_VOICE_PENDING.store(true, Ordering::Relaxed);
    let token = MODIFIER_ONLY_VOICE_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(MODIFIER_ONLY_VOICE_TRIGGER_DELAY_MS));
        if MODIFIER_ONLY_VOICE_TOKEN.load(Ordering::Relaxed) != token {
            return;
        }
        if MODIFIER_ONLY_VOICE_PASSTHROUGH.load(Ordering::Relaxed) {
            return;
        }
        if !MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed) {
            return;
        }
        if !modifiers.exactly_pressed() {
            MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
            return;
        }
        MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
        if !VOICE_ACTIVE.swap(true, Ordering::Relaxed) {
            tracing::info!("modifier-only voice hotkey matched in keyboard hook");
            send_hotkey_state(HotkeyState::VoicePressed);
        }
    });
}

fn cancel_modifier_only_voice_capture(active: bool) {
    MODIFIER_ONLY_VOICE_PENDING.store(false, Ordering::Relaxed);
    MODIFIER_ONLY_VOICE_PASSTHROUGH.store(true, Ordering::Relaxed);
    if active && VOICE_ACTIVE.swap(false, Ordering::Relaxed) {
        tracing::info!("modifier-only voice hotkey cancelled by chord in keyboard hook");
        send_hotkey_state(HotkeyState::VoiceReleased);
    }
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
            LRESULT(1)
        }
        WM_MBUTTONUP => {
            handle_middle_button_up();
            LRESULT(1)
        }
        _ => unsafe { CallNextHookEx(None, code, wparam, lparam) },
    }
}

fn current_config() -> Option<HotkeyRuntimeConfig> {
    HOTKEY_CONFIG
        .get()
        .and_then(|config| config.read().ok())
        .map(|config| HotkeyRuntimeConfig {
            voice: config.voice.clone(),
            screenshot: config.screenshot.clone(),
        })
}

fn update_modifier_state(vk: VIRTUAL_KEY, is_down: bool, is_up: bool) {
    match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
            if is_down {
                CTRL_DOWN.store(true, Ordering::Relaxed);
            }
            if is_up {
                CTRL_DOWN.store(false, Ordering::Relaxed);
            }
        }
        VK_MENU | VK_LMENU | VK_RMENU => {
            if is_down {
                ALT_DOWN.store(true, Ordering::Relaxed);
            }
            if is_up {
                ALT_DOWN.store(false, Ordering::Relaxed);
            }
        }
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => {
            if is_down {
                SHIFT_DOWN.store(true, Ordering::Relaxed);
            }
            if is_up {
                SHIFT_DOWN.store(false, Ordering::Relaxed);
            }
        }
        VK_LWIN | VK_RWIN => {
            if is_down {
                WIN_DOWN.store(true, Ordering::Relaxed);
            }
            if is_up {
                WIN_DOWN.store(false, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

impl HotkeyModifiers {
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    fn matches_pressed(self) -> bool {
        (!self.ctrl || CTRL_DOWN.load(Ordering::Relaxed))
            && (!self.alt || ALT_DOWN.load(Ordering::Relaxed))
            && (!self.shift || SHIFT_DOWN.load(Ordering::Relaxed))
            && (!self.win || WIN_DOWN.load(Ordering::Relaxed))
    }

    fn exactly_pressed(self) -> bool {
        self.ctrl == CTRL_DOWN.load(Ordering::Relaxed)
            && self.alt == ALT_DOWN.load(Ordering::Relaxed)
            && self.shift == SHIFT_DOWN.load(Ordering::Relaxed)
            && self.win == WIN_DOWN.load(Ordering::Relaxed)
    }

    fn any_released_requirement(self) -> bool {
        (self.ctrl && !CTRL_DOWN.load(Ordering::Relaxed))
            || (self.alt && !ALT_DOWN.load(Ordering::Relaxed))
            || (self.shift && !SHIFT_DOWN.load(Ordering::Relaxed))
            || (self.win && !WIN_DOWN.load(Ordering::Relaxed))
    }

    fn fully_released(self) -> bool {
        (!self.ctrl || !CTRL_DOWN.load(Ordering::Relaxed))
            && (!self.alt || !ALT_DOWN.load(Ordering::Relaxed))
            && (!self.shift || !SHIFT_DOWN.load(Ordering::Relaxed))
            && (!self.win || !WIN_DOWN.load(Ordering::Relaxed))
    }

    fn contains_vk(self, vk: VIRTUAL_KEY) -> bool {
        (self.ctrl && matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL))
            || (self.alt && matches!(vk, VK_MENU | VK_LMENU | VK_RMENU))
            || (self.shift && matches!(vk, VK_SHIFT | VK_LSHIFT | VK_RSHIFT))
            || (self.win && matches!(vk, VK_LWIN | VK_RWIN))
    }
}

fn parse_hotkey(text: &str) -> Result<ParsedHotkey> {
    let mut modifiers = HotkeyModifiers::default();
    let mut key: Option<VIRTUAL_KEY> = None;

    for token in text
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "win" | "windows" => modifiers.win = true,
            other => {
                if key.is_some() {
                    return Err(anyhow!("only one primary key is allowed, got {other}"));
                }
                key = Some(parse_primary_key(other)?);
            }
        }
    }

    if !modifiers.any() {
        return Err(anyhow!("at least one modifier is required"));
    }

    Ok(ParsedHotkey { modifiers, key })
}

fn parse_primary_key(token: &str) -> Result<VIRTUAL_KEY> {
    let upper = token.to_ascii_uppercase();
    let vk = match upper.as_str() {
        "SPACE" => VK_SPACE,
        "ENTER" => VK_RETURN,
        "TAB" => VK_TAB,
        "ESC" | "ESCAPE" => VK_ESCAPE,
        "UP" => VK_UP,
        "DOWN" => VK_DOWN,
        "LEFT" => VK_LEFT,
        "RIGHT" => VK_RIGHT,
        "HOME" => VK_HOME,
        "END" => VK_END,
        "PAGEUP" => VK_PRIOR,
        "PAGEDOWN" => VK_NEXT,
        "INSERT" => VK_INSERT,
        "DELETE" => VK_DELETE,
        "BACKSPACE" => VK_BACK,
        "PRINTSCREEN" => VK_SNAPSHOT,
        "PAUSE" => VK_PAUSE,
        "CAPSLOCK" => VK_CAPITAL,
        "NUMLOCK" => VK_NUMLOCK,
        "NUM0" => VK_NUMPAD0,
        "NUM1" => VK_NUMPAD1,
        "NUM2" => VK_NUMPAD2,
        "NUM3" => VK_NUMPAD3,
        "NUM4" => VK_NUMPAD4,
        "NUM5" => VK_NUMPAD5,
        "NUM6" => VK_NUMPAD6,
        "NUM7" => VK_NUMPAD7,
        "NUM8" => VK_NUMPAD8,
        "NUM9" => VK_NUMPAD9,
        "NUM+" => VK_ADD,
        "NUM-" => VK_SUBTRACT,
        "NUM*" => VK_MULTIPLY,
        "NUM/" => VK_DIVIDE,
        "NUM." => VK_DECIMAL,
        "F1" => VK_F1,
        "F2" => VK_F2,
        "F3" => VK_F3,
        "F4" => VK_F4,
        "F5" => VK_F5,
        "F6" => VK_F6,
        "F7" => VK_F7,
        "F8" => VK_F8,
        "F9" => VK_F9,
        "F10" => VK_F10,
        "F11" => VK_F11,
        "F12" => VK_F12,
        _ if upper.len() == 1 => VIRTUAL_KEY(upper.as_bytes()[0] as u16),
        _ => return Err(anyhow!("unsupported key {token}")),
    };
    Ok(vk)
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
        if voice_hotkey_suppressed_for_output() {
            return;
        }

        if !MOUSE_MIDDLE_ACTIVE.swap(true, Ordering::Relaxed) {
            tracing::info!("mouse middle hold voice hotkey matched");
            send_hotkey_state(HotkeyState::VoicePressed);
        }
    });
}

fn handle_middle_button_up() {
    let was_down = MOUSE_MIDDLE_DOWN.swap(false, Ordering::Relaxed);
    if !was_down {
        return;
    }

    if MOUSE_MIDDLE_ACTIVE.swap(false, Ordering::Relaxed) {
        send_hotkey_state(HotkeyState::VoiceReleased);
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

fn send_hotkey_state(state: HotkeyState) {
    if let Some(proxy) = HOTKEY_PROXY.get() {
        let _ = proxy.send_event(crate::AppEvent::Hotkey(state));
    }
}

fn send_error(message: String) {
    if let Some(proxy) = HOTKEY_PROXY.get() {
        let _ = proxy.send_event(crate::AppEvent::Worker(crate::WorkerEvent::Error(message)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test mutex poisoned")
    }

    fn install_test_config(voice_input: &str) {
        let config = HotkeyRuntimeConfig {
            voice: parse_hotkey(voice_input).expect("voice hotkey should parse"),
            screenshot: parse_hotkey("Alt+X").expect("screenshot hotkey should parse"),
        };

        if let Some(lock) = HOTKEY_CONFIG.get() {
            *lock.write().expect("hotkey config write lock poisoned") = config;
        } else {
            let _ = HOTKEY_CONFIG.set(RwLock::new(config));
        }
    }

    #[test]
    fn voice_hotkey_uses_alt_tracks_runtime_config() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Alt+Z");
        assert!(voice_hotkey_uses_alt());

        install_test_config("Ctrl+Shift+Z");
        assert!(!voice_hotkey_uses_alt());
    }

    #[test]
    fn wait_for_voice_hotkey_release_succeeds_after_modifier_clears() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Alt+Z");
        ALT_DOWN.store(true, Ordering::Relaxed);

        let releaser = thread::spawn(|| {
            thread::sleep(Duration::from_millis(20));
            ALT_DOWN.store(false, Ordering::Relaxed);
        });

        let started_at = Instant::now();
        assert!(wait_for_voice_hotkey_release(Duration::from_millis(100)));
        assert!(started_at.elapsed() >= Duration::from_millis(16));

        releaser
            .join()
            .expect("modifier releaser thread should finish");
        reset_hotkey_state();
    }

    #[test]
    fn wait_for_voice_hotkey_release_times_out_when_modifier_stays_pressed() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Alt+Z");
        ALT_DOWN.store(true, Ordering::Relaxed);

        assert!(!wait_for_voice_hotkey_release(Duration::from_millis(20)));

        reset_hotkey_state();
    }

    #[test]
    fn modifier_only_ctrl_never_swallows_ctrl_down_or_up() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Ctrl");
        let config = current_config().expect("test config should be installed");

        CTRL_DOWN.store(true, Ordering::Relaxed);
        assert!(
            handle_modifier_only_voice_hotkey(VK_CONTROL, true, false, &config.voice).is_none()
        );
        assert!(MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed));

        CTRL_DOWN.store(false, Ordering::Relaxed);
        assert!(
            handle_modifier_only_voice_hotkey(VK_CONTROL, false, true, &config.voice).is_none()
        );
        assert!(!MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed));
        assert!(!VOICE_ACTIVE.load(Ordering::Relaxed));

        reset_hotkey_state();
    }

    #[test]
    fn ctrl_letter_shortcut_cancels_voice_capture_without_swallowing_shortcut() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Ctrl");
        let config = current_config().expect("test config should be installed");

        CTRL_DOWN.store(true, Ordering::Relaxed);
        assert!(
            handle_modifier_only_voice_hotkey(VK_CONTROL, true, false, &config.voice).is_none()
        );
        assert!(MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed));

        assert!(
            handle_modifier_only_voice_hotkey(VIRTUAL_KEY(0x43), true, false, &config.voice)
                .is_none()
        );
        assert!(!MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed));
        assert!(MODIFIER_ONLY_VOICE_PASSTHROUGH.load(Ordering::Relaxed));

        CTRL_DOWN.store(false, Ordering::Relaxed);
        assert!(
            handle_modifier_only_voice_hotkey(VK_CONTROL, false, true, &config.voice).is_none()
        );
        assert!(!MODIFIER_ONLY_VOICE_PASSTHROUGH.load(Ordering::Relaxed));

        reset_hotkey_state();
    }

    #[test]
    fn output_suppression_tracks_guard_lifetime_and_cancels_pending_ctrl_voice() {
        let _guard = test_guard();
        reset_hotkey_state();
        install_test_config("Ctrl");
        CTRL_DOWN.store(true, Ordering::Relaxed);
        MODIFIER_ONLY_VOICE_PENDING.store(true, Ordering::Relaxed);
        let old_token = MODIFIER_ONLY_VOICE_TOKEN.load(Ordering::Relaxed);

        {
            let _suppression = suppress_voice_hotkey_for_output();
            assert!(voice_hotkey_suppressed_for_output());
            assert!(!MODIFIER_ONLY_VOICE_PENDING.load(Ordering::Relaxed));
            assert!(MODIFIER_ONLY_VOICE_TOKEN.load(Ordering::Relaxed) > old_token);
            assert!(!VOICE_ACTIVE.load(Ordering::Relaxed));
        }

        assert!(voice_hotkey_suppressed_for_output());
        assert!(VOICE_HOTKEY_SUPPRESS_UNTIL_MS.load(Ordering::Relaxed) >= now_epoch_ms());
        reset_hotkey_state();
        assert!(!voice_hotkey_suppressed_for_output());
    }
}
