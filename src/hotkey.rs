use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tracing::{info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, KEYEVENTF_KEYUP, VK_CAPITAL, keybd_event,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1, XBUTTON2,
};

use crate::config::{HotkeyConfig, ProfileConfigs, VoiceProfileConfig};
use crate::modes::{InputMode, VoiceProfileId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerPhase {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Voice(VoiceTriggerEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceTriggerEvent {
    pub profile_id: VoiceProfileId,
    pub mode: InputMode,
    pub phase: TriggerPhase,
}

impl VoiceTriggerEvent {
    fn pressed(profile_id: VoiceProfileId, mode: InputMode) -> Self {
        Self {
            profile_id,
            mode,
            phase: TriggerPhase::Pressed,
        }
    }

    fn released(profile_id: VoiceProfileId, mode: InputMode) -> Self {
        Self {
            profile_id,
            mode,
            phase: TriggerPhase::Released,
        }
    }
}

pub struct HotkeyMonitor {
    stop: Arc<AtomicBool>,
    joins: Vec<thread::JoinHandle<()>>,
    /// Message-loop thread that owns WH_KEYBOARD_LL / WH_MOUSE_LL hooks.
    input_hook_thread_id: Option<u32>,
}

#[derive(Debug, Clone)]
struct ParsedHotkey {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: Option<u16>,
}

const RELEASE_DEBOUNCE_MS: u64 = 45;
const INPUT_HOOK_QUIT: u32 = WM_APP + 88;
const VK_ALT: u16 = 0x12;
const VK_Z: u16 = b'Z' as u16;
/// VK_XBUTTON1 / VK_XBUTTON2 — mouse back/forward side buttons (Razer side keys usually).
const VK_XBUTTON1: u16 = 0x05;
const VK_XBUTTON2: u16 = 0x06;
static CAPSLOCK_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_Z_DOWN: AtomicBool = AtomicBool::new(false);
static MOUSE_X1_DOWN: AtomicBool = AtomicBool::new(false);
static MOUSE_X2_DOWN: AtomicBool = AtomicBool::new(false);
static SUPPRESS_CAPSLOCK_HOTKEY: AtomicBool = AtomicBool::new(false);
static SUPPRESS_ALT_Z_HOTKEY: AtomicBool = AtomicBool::new(false);
static SUPPRESS_MOUSE_X1: AtomicBool = AtomicBool::new(false);
static SUPPRESS_MOUSE_X2: AtomicBool = AtomicBool::new(false);

impl HotkeyMonitor {
    pub fn start(
        config: HotkeyConfig,
        profiles: ProfileConfigs,
        tx: mpsc::Sender<HotkeyEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let poll = Duration::from_millis(config.poll_ms.max(4));
        let mut joins = Vec::new();
        let mut input_hook_thread_id = None;

        let suppress_capslock = profile_suppresses(&profiles, is_capslock_hotkey);
        let suppress_alt_z = profile_suppresses(&profiles, is_alt_z_hotkey);
        let suppress_mouse_x1 = profile_suppresses(&profiles, |hk| is_mouse_x_hotkey(hk, VK_XBUTTON1));
        let suppress_mouse_x2 = profile_suppresses(&profiles, |hk| is_mouse_x_hotkey(hk, VK_XBUTTON2));
        let need_input_hook =
            suppress_capslock || suppress_alt_z || suppress_mouse_x1 || suppress_mouse_x2;

        if need_input_hook {
            match start_input_hook_thread(
                suppress_capslock,
                suppress_alt_z,
                suppress_mouse_x1,
                suppress_mouse_x2,
            ) {
                Ok((thread_id, join)) => {
                    input_hook_thread_id = Some(thread_id);
                    joins.push(join);
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        "input hook failed; suppressed profiles may be disabled"
                    );
                }
            }
        }

        if profiles.streaming.enabled {
            joins.push(spawn_polling_profile_monitor(
                VoiceProfileId::StreamingDefault,
                profiles.streaming.clone(),
                poll,
                tx.clone(),
                Arc::clone(&shutdown),
                Arc::clone(&stop),
            )?);
        }

        if profiles.whisper.enabled {
            match suppressed_kind_for(&profiles.whisper) {
                Some(kind) => {
                    if input_hook_thread_id.is_some() {
                        joins.push(spawn_suppressed_state_monitor(
                            VoiceProfileId::WhisperCapslock,
                            profiles.whisper.clone(),
                            poll,
                            tx.clone(),
                            Arc::clone(&shutdown),
                            Arc::clone(&stop),
                            kind,
                        ));
                    } else {
                        warn!(
                            hotkey = %profiles.whisper.hotkey,
                            "input hook unavailable; whisper profile disabled"
                        );
                    }
                }
                None if profiles.whisper.suppress_key => {
                    warn!(
                        hotkey = %profiles.whisper.hotkey,
                        "suppress_key is only supported for CapsLock, Alt+Z, or MouseX1/X2; whisper profile disabled"
                    );
                }
                None => {
                    joins.push(spawn_polling_profile_monitor(
                        VoiceProfileId::WhisperCapslock,
                        profiles.whisper,
                        poll,
                        tx.clone(),
                        Arc::clone(&shutdown),
                        Arc::clone(&stop),
                    )?);
                }
            }
        }

        if profiles.local_nonstreaming.enabled {
            match suppressed_kind_for(&profiles.local_nonstreaming) {
                Some(kind) => {
                    if input_hook_thread_id.is_some() {
                        joins.push(spawn_suppressed_state_monitor(
                            VoiceProfileId::LocalNonstreaming,
                            profiles.local_nonstreaming.clone(),
                            poll,
                            tx,
                            shutdown,
                            Arc::clone(&stop),
                            kind,
                        ));
                    } else {
                        warn!(
                            hotkey = %profiles.local_nonstreaming.hotkey,
                            "input hook unavailable; local_nonstreaming profile disabled"
                        );
                    }
                }
                None if profiles.local_nonstreaming.suppress_key => {
                    warn!(
                        hotkey = %profiles.local_nonstreaming.hotkey,
                        "suppress_key is only supported for CapsLock, Alt+Z, or MouseX1/X2; local_nonstreaming profile disabled"
                    );
                }
                None => {
                    joins.push(spawn_polling_profile_monitor(
                        VoiceProfileId::LocalNonstreaming,
                        profiles.local_nonstreaming,
                        poll,
                        tx,
                        shutdown,
                        Arc::clone(&stop),
                    )?);
                }
            }
        }

        Ok(Self {
            stop,
            joins,
            input_hook_thread_id,
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread_id) = self.input_hook_thread_id.take() {
            unsafe {
                let _ = PostThreadMessageW(thread_id, INPUT_HOOK_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        for join in self.joins.drain(..) {
            if let Err(error) = join.join() {
                warn!(?error, "hotkey monitor thread join failed");
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SuppressedHotkeyKind {
    CapsLock,
    AltZ,
    MouseX1,
    MouseX2,
}

fn profile_suppresses(profiles: &ProfileConfigs, pred: impl Fn(&str) -> bool) -> bool {
    (profiles.whisper.enabled && profiles.whisper.suppress_key && pred(&profiles.whisper.hotkey))
        || (profiles.local_nonstreaming.enabled
            && profiles.local_nonstreaming.suppress_key
            && pred(&profiles.local_nonstreaming.hotkey))
}

fn suppressed_kind_for(profile: &VoiceProfileConfig) -> Option<SuppressedHotkeyKind> {
    if !profile.suppress_key {
        return None;
    }
    if is_capslock_hotkey(&profile.hotkey) {
        Some(SuppressedHotkeyKind::CapsLock)
    } else if is_alt_z_hotkey(&profile.hotkey) {
        Some(SuppressedHotkeyKind::AltZ)
    } else if is_mouse_x_hotkey(&profile.hotkey, VK_XBUTTON1) {
        Some(SuppressedHotkeyKind::MouseX1)
    } else if is_mouse_x_hotkey(&profile.hotkey, VK_XBUTTON2) {
        Some(SuppressedHotkeyKind::MouseX2)
    } else {
        None
    }
}

fn spawn_polling_profile_monitor(
    profile_id: VoiceProfileId,
    profile: VoiceProfileConfig,
    poll: Duration,
    tx: mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    let parsed = parse_hotkey(&profile.hotkey)?;
    let label = profile.hotkey.clone();
    let mode = profile.mode;
    let activation_delay = Duration::from_millis(profile.activation_delay_ms);
    let release_debounce = Duration::from_millis(RELEASE_DEBOUNCE_MS);
    Ok(thread::spawn(move || {
        info!(
            hotkey = %label,
            profile = profile_id.as_str(),
            mode = ?mode,
            "hotkey polling monitor started"
        );
        run_boolean_hotkey_loop(
            profile_id,
            mode,
            activation_delay,
            release_debounce,
            poll,
            tx,
            shutdown,
            stop,
            |_| parsed.is_pressed(),
        );
        info!(
            profile = profile_id.as_str(),
            "hotkey polling monitor stopped"
        );
    }))
}

fn spawn_suppressed_state_monitor(
    profile_id: VoiceProfileId,
    profile: VoiceProfileConfig,
    poll: Duration,
    tx: mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    kind: SuppressedHotkeyKind,
) -> thread::JoinHandle<()> {
    // Reset only the kind we own — other suppressed keys may be shared across profiles.
    match kind {
        SuppressedHotkeyKind::CapsLock => CAPSLOCK_DOWN.store(false, Ordering::Relaxed),
        SuppressedHotkeyKind::AltZ => ALT_Z_DOWN.store(false, Ordering::Relaxed),
        SuppressedHotkeyKind::MouseX1 => MOUSE_X1_DOWN.store(false, Ordering::Relaxed),
        SuppressedHotkeyKind::MouseX2 => MOUSE_X2_DOWN.store(false, Ordering::Relaxed),
    }
    let mode = profile.mode;
    let activation_delay = Duration::from_millis(profile.activation_delay_ms);
    let release_debounce = Duration::from_millis(RELEASE_DEBOUNCE_MS);
    thread::spawn(move || {
        info!(
            hotkey = %profile.hotkey,
            profile = profile_id.as_str(),
            mode = ?mode,
            suppressed_hotkey = ?kind,
            "suppressed hotkey state monitor started"
        );
        run_boolean_hotkey_loop(
            profile_id,
            mode,
            activation_delay,
            release_debounce,
            poll,
            tx,
            shutdown,
            stop,
            move |_| match kind {
                SuppressedHotkeyKind::CapsLock => CAPSLOCK_DOWN.load(Ordering::Relaxed),
                SuppressedHotkeyKind::AltZ => ALT_Z_DOWN.load(Ordering::Relaxed),
                SuppressedHotkeyKind::MouseX1 => MOUSE_X1_DOWN.load(Ordering::Relaxed),
                SuppressedHotkeyKind::MouseX2 => MOUSE_X2_DOWN.load(Ordering::Relaxed),
            },
        );
        match kind {
            SuppressedHotkeyKind::CapsLock => CAPSLOCK_DOWN.store(false, Ordering::Relaxed),
            SuppressedHotkeyKind::AltZ => ALT_Z_DOWN.store(false, Ordering::Relaxed),
            SuppressedHotkeyKind::MouseX1 => MOUSE_X1_DOWN.store(false, Ordering::Relaxed),
            SuppressedHotkeyKind::MouseX2 => MOUSE_X2_DOWN.store(false, Ordering::Relaxed),
        }
        info!(
            profile = profile_id.as_str(),
            suppressed_hotkey = ?kind,
            "suppressed hotkey state monitor stopped"
        );
    })
}

#[allow(clippy::too_many_arguments)]
fn run_boolean_hotkey_loop<F>(
    profile_id: VoiceProfileId,
    mode: InputMode,
    activation_delay: Duration,
    release_debounce: Duration,
    poll: Duration,
    tx: mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    mut is_pressed: F,
) where
    F: FnMut(bool) -> bool,
{
    let mut active = false;
    let mut down_since: Option<Instant> = None;
    let mut up_since: Option<Instant> = None;
    while !shutdown.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
        let pressed = is_pressed(active);
        if pressed {
            up_since = None;
            let since = down_since.get_or_insert_with(Instant::now);
            if !active && since.elapsed() >= activation_delay {
                active = true;
                let _ = tx.send(HotkeyEvent::Voice(VoiceTriggerEvent::pressed(profile_id, mode)));
            }
        } else {
            down_since = None;
            if active {
                let since = up_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= release_debounce {
                    active = false;
                    up_since = None;
                    let _ = tx.send(HotkeyEvent::Voice(VoiceTriggerEvent::released(profile_id, mode)));
                }
            } else {
                up_since = None;
            }
        }
        thread::sleep(poll);
    }
    if active {
        let _ = tx.send(HotkeyEvent::Voice(VoiceTriggerEvent::released(profile_id, mode)));
    }
}

fn start_input_hook_thread(
    suppress_capslock: bool,
    suppress_alt_z: bool,
    suppress_mouse_x1: bool,
    suppress_mouse_x2: bool,
) -> Result<(u32, thread::JoinHandle<()>)> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let join = thread::spawn(move || {
        SUPPRESS_CAPSLOCK_HOTKEY.store(suppress_capslock, Ordering::Relaxed);
        SUPPRESS_ALT_Z_HOTKEY.store(suppress_alt_z, Ordering::Relaxed);
        SUPPRESS_MOUSE_X1.store(suppress_mouse_x1, Ordering::Relaxed);
        SUPPRESS_MOUSE_X2.store(suppress_mouse_x2, Ordering::Relaxed);
        CAPSLOCK_DOWN.store(false, Ordering::Relaxed);
        ALT_Z_DOWN.store(false, Ordering::Relaxed);
        MOUSE_X1_DOWN.store(false, Ordering::Relaxed);
        MOUSE_X2_DOWN.store(false, Ordering::Relaxed);
        let thread_id = unsafe { GetCurrentThreadId() };
        if suppress_capslock {
            force_capslock_off("before_hook_install");
        }

        let need_keyboard = suppress_capslock || suppress_alt_z;
        let need_mouse = suppress_mouse_x1 || suppress_mouse_x2;

        let keyboard_hook = if need_keyboard {
            match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) } {
                Ok(hook) => Some(hook),
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("keyboard hook: {error}")));
                    return;
                }
            }
        } else {
            None
        };

        let mouse_hook = if need_mouse {
            match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) } {
                Ok(hook) => Some(hook),
                Err(error) => {
                    if let Some(kh) = keyboard_hook {
                        let _ = unsafe { UnhookWindowsHookEx(kh) };
                    }
                    let _ = ready_tx.send(Err(format!("mouse hook: {error}")));
                    return;
                }
            }
        } else {
            None
        };

        let _ = ready_tx.send(Ok(thread_id));
        info!(
            thread_id,
            suppress_capslock,
            suppress_alt_z,
            suppress_mouse_x1,
            suppress_mouse_x2,
            "low-level input hooks installed"
        );
        loop {
            let mut msg = MSG::default();
            let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if has_message.0 == -1 {
                warn!("input hook GetMessage failed");
                break;
            }
            if has_message.0 == 0 || msg.message == INPUT_HOOK_QUIT {
                break;
            }
        }
        if let Some(hook) = mouse_hook {
            if let Err(error) = unsafe { UnhookWindowsHookEx(hook) } {
                warn!(error = %error, "unhook WH_MOUSE_LL failed");
            }
        }
        if let Some(hook) = keyboard_hook {
            if let Err(error) = unsafe { UnhookWindowsHookEx(hook) } {
                warn!(error = %error, "unhook WH_KEYBOARD_LL failed");
            }
        }
        CAPSLOCK_DOWN.store(false, Ordering::Relaxed);
        ALT_Z_DOWN.store(false, Ordering::Relaxed);
        MOUSE_X1_DOWN.store(false, Ordering::Relaxed);
        MOUSE_X2_DOWN.store(false, Ordering::Relaxed);
        SUPPRESS_CAPSLOCK_HOTKEY.store(false, Ordering::Relaxed);
        SUPPRESS_ALT_Z_HOTKEY.store(false, Ordering::Relaxed);
        SUPPRESS_MOUSE_X1.store(false, Ordering::Relaxed);
        SUPPRESS_MOUSE_X2.store(false, Ordering::Relaxed);
        if suppress_capslock {
            force_capslock_off("after_hook_stop");
        }
        info!(thread_id, "low-level input hooks stopped");
    });
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow!("input hook thread did not initialize"))?
        .map_err(|error| anyhow!(error))?;
    Ok((thread_id, join))
}

fn force_capslock_off(reason: &'static str) {
    let is_on = unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 };
    if !is_on {
        return;
    }
    unsafe {
        keybd_event(VK_CAPITAL.0 as u8, 0x45, Default::default(), 0);
        thread::sleep(Duration::from_millis(25));
        keybd_event(VK_CAPITAL.0 as u8, 0x45, KEYEVENTF_KEYUP, 0);
    }
    thread::sleep(Duration::from_millis(50));
    let still_on = unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 };
    if still_on {
        warn!(reason, "failed to force CapsLock off");
    } else {
        info!(reason, "forced CapsLock off for suppressed CapsLock hotkey");
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let keyboard = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };

        if SUPPRESS_CAPSLOCK_HOTKEY.load(Ordering::Relaxed)
            && keyboard.vkCode == VK_CAPITAL.0 as u32
        {
            match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => CAPSLOCK_DOWN.store(true, Ordering::Relaxed),
                WM_KEYUP | WM_SYSKEYUP => CAPSLOCK_DOWN.store(false, Ordering::Relaxed),
                _ => {}
            }
            return LRESULT(1);
        }
        if SUPPRESS_ALT_Z_HOTKEY.load(Ordering::Relaxed) && keyboard.vkCode == VK_Z as u32 {
            let alt_is_down = key_down(VK_ALT);
            match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN if alt_is_down => {
                    ALT_Z_DOWN.store(true, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    let was_down = ALT_Z_DOWN.swap(false, Ordering::Relaxed);
                    if was_down || alt_is_down {
                        return LRESULT(1);
                    }
                }
                _ => {}
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Strip MouseX1/X2 system Back/Forward (and app delivery) while keeping hold-to-talk state.
/// mouseData high word is XBUTTON1=1 / XBUTTON2=2 (same packing as GET_XBUTTON_WPARAM).
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_XBUTTONDOWN || msg == WM_XBUTTONUP {
            let mouse = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let xbutton = ((mouse.mouseData >> 16) & 0xFFFF) as u16;
            let is_down = msg == WM_XBUTTONDOWN;
            if xbutton == XBUTTON1 && SUPPRESS_MOUSE_X1.load(Ordering::Relaxed) {
                MOUSE_X1_DOWN.store(is_down, Ordering::Relaxed);
                return LRESULT(1);
            }
            if xbutton == XBUTTON2 && SUPPRESS_MOUSE_X2.load(Ordering::Relaxed) {
                MOUSE_X2_DOWN.store(is_down, Ordering::Relaxed);
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn parse_hotkey(input: &str) -> Result<ParsedHotkey> {
    let mut parsed = ParsedHotkey {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        key: None,
    };
    for raw in input.split('+') {
        let token = raw.trim().to_ascii_lowercase();
        match token.as_str() {
            "ctrl" | "control" => parsed.ctrl = true,
            "alt" => parsed.alt = true,
            "shift" => parsed.shift = true,
            "win" | "windows" | "meta" => parsed.win = true,
            "" => {}
            key => {
                if parsed.key.is_some() {
                    return Err(anyhow!("hotkey has multiple non-modifier keys: {input}"));
                }
                parsed.key = Some(parse_key(key)?);
            }
        }
    }
    if parsed.key.is_none() && !parsed.has_modifier() {
        return Err(anyhow!("hotkey is empty: {input}"));
    }
    Ok(parsed)
}

fn parse_key(key: &str) -> Result<u16> {
    if key.len() == 1 {
        let ch = key.as_bytes()[0];
        if ch.is_ascii_alphabetic() {
            return Ok(ch.to_ascii_uppercase() as u16);
        }
        if ch.is_ascii_digit() {
            return Ok(ch as u16);
        }
    }
    match key {
        "space" => Ok(0x20),
        "tab" => Ok(0x09),
        "capslock" | "caps_lock" | "caps lock" => Ok(VK_CAPITAL.0),
        "enter" | "return" => Ok(0x0D),
        "escape" | "esc" => Ok(0x1B),
        // Mouse side buttons (system-level X1/X2; Razer maps many side keys here)
        "mousex1" | "mouse_x1" | "xbutton1" | "x1" | "mouse4" | "mouse_4" | "browser_back" => {
            Ok(VK_XBUTTON1)
        }
        "mousex2" | "mouse_x2" | "xbutton2" | "x2" | "mouse5" | "mouse_5" | "browser_forward" => {
            Ok(VK_XBUTTON2)
        }
        f if f.starts_with('f') => {
            let n: u16 = f[1..]
                .parse()
                .map_err(|_| anyhow!("unsupported hotkey key: {key}"))?;
            if (1..=24).contains(&n) {
                Ok(0x70 + n - 1)
            } else {
                Err(anyhow!("unsupported function key: {key}"))
            }
        }
        _ => Err(anyhow!("unsupported hotkey key: {key}")),
    }
}

fn is_capslock_hotkey(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "capslock" | "caps_lock" | "caps lock"
    )
}

fn is_alt_z_hotkey(input: &str) -> bool {
    let Ok(parsed) = parse_hotkey(input) else {
        return false;
    };
    parsed.alt && !parsed.ctrl && !parsed.shift && !parsed.win && parsed.key == Some(VK_Z)
}

fn is_mouse_side_hotkey(input: &str) -> bool {
    is_mouse_x_hotkey(input, VK_XBUTTON1) || is_mouse_x_hotkey(input, VK_XBUTTON2)
}

fn is_mouse_x_hotkey(input: &str, which: u16) -> bool {
    let Ok(parsed) = parse_hotkey(input) else {
        return false;
    };
    parsed.key == Some(which)
        && !parsed.ctrl
        && !parsed.alt
        && !parsed.shift
        && !parsed.win
}

/// CapsLock / Alt+Z / MouseX1 / MouseX2 can be swallowed by the low-level input hooks.
pub fn hotkey_supports_suppress(input: &str) -> bool {
    is_capslock_hotkey(input) || is_alt_z_hotkey(input) || is_mouse_side_hotkey(input)
}

pub fn validate_hotkey_label(input: &str) -> Result<String> {
    let parsed = parse_hotkey(input)?;
    // Reject pure modifiers with no key — hold-to-talk needs a discrete key/button.
    if parsed.key.is_none() {
        return Err(anyhow!(
            "快捷键需要一个主键（如 CapsLock / F13 / MouseX2），不能只有 Ctrl/Alt"
        ));
    }
    Ok(parse_hotkey_label(input)?)
}

/// Canonical label for storage / UI.
pub fn parse_hotkey_label(input: &str) -> Result<String> {
    let parsed = parse_hotkey(input)?;
    let mut parts = Vec::new();
    if parsed.ctrl {
        parts.push("Ctrl".to_string());
    }
    if parsed.alt {
        parts.push("Alt".to_string());
    }
    if parsed.shift {
        parts.push("Shift".to_string());
    }
    if parsed.win {
        parts.push("Win".to_string());
    }
    if let Some(vk) = parsed.key {
        parts.push(vk_to_label(vk));
    }
    if parts.is_empty() {
        return Err(anyhow!("hotkey is empty"));
    }
    Ok(parts.join("+"))
}

pub fn vk_display_name(label: &str) -> String {
    parse_hotkey_label(label).unwrap_or_else(|_| label.to_string())
}

fn vk_to_label(vk: u16) -> String {
    match vk {
        0x14 => "CapsLock".to_string(),
        0x20 => "Space".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Enter".to_string(),
        0x1B => "Esc".to_string(),
        VK_XBUTTON1 => "MouseX1".to_string(),
        VK_XBUTTON2 => "MouseX2".to_string(),
        v if (0x70..=0x87).contains(&v) => format!("F{}", v - 0x70 + 1),
        v if (b'A' as u16..=b'Z' as u16).contains(&v) => format!("{}", v as u8 as char),
        v if (b'0' as u16..=b'9' as u16).contains(&v) => format!("{}", v as u8 as char),
        other => format!("VK_{other:02X}"),
    }
}

/// Poll until a new non-modifier key/button is pressed (after all probe keys released).
/// Captures the main key only (MouseX / F-keys / CapsLock / letter). Combos: type manually.
pub fn capture_next_hotkey(timeout: Duration) -> Result<String> {
    let probes: Vec<u16> = {
        let mut v = vec![
            VK_XBUTTON1,
            VK_XBUTTON2,
            VK_CAPITAL.0,
            0x20, // space
            0x09, // tab
            0x0D, // enter
            0x1B, // esc
        ];
        for c in b'A'..=b'Z' {
            v.push(c as u16);
        }
        for c in b'0'..=b'9' {
            v.push(c as u16);
        }
        for i in 0..24u16 {
            v.push(0x70 + i);
        }
        v
    };
    let deadline = Instant::now() + timeout;
    let release_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < release_deadline {
        if probes.iter().all(|&vk| !key_down(vk)) {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    while Instant::now() < deadline {
        for &vk in &probes {
            if key_down(vk) {
                let label = vk_to_label(vk);
                let wait_up = Instant::now() + Duration::from_secs(2);
                while Instant::now() < wait_up && key_down(vk) {
                    thread::sleep(Duration::from_millis(15));
                }
                return Ok(label);
            }
        }
        thread::sleep(Duration::from_millis(12));
    }
    Err(anyhow!("超时：未捕获到按键或鼠标侧键"))
}

impl ParsedHotkey {
    fn is_pressed(&self) -> bool {
        (!self.ctrl || key_down(0x11))
            && (!self.alt || key_down(0x12))
            && (!self.shift || key_down(0x10))
            && (!self.win || key_down(0x5B) || key_down(0x5C))
            && self.key.is_none_or(key_down)
    }

    fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

fn key_down(vk: u16) -> bool {
    let state = unsafe { GetAsyncKeyState(vk as i32) };
    (state as u16 & 0x8000) != 0
}

#[cfg(test)]
mod tests {
    use super::{
        hotkey_supports_suppress, is_alt_z_hotkey, is_capslock_hotkey, is_mouse_side_hotkey,
        parse_hotkey, parse_hotkey_label, validate_hotkey_label,
    };

    #[test]
    fn parses_capslock_hotkey_aliases() {
        assert!(is_capslock_hotkey("CapsLock"));
        assert!(is_capslock_hotkey("caps lock"));
        let parsed = parse_hotkey("CapsLock").expect("parse capslock");
        assert_eq!(parsed.key, Some(0x14));
    }

    #[test]
    fn detects_alt_z_hotkey_for_suppression() {
        assert!(is_alt_z_hotkey("Alt+Z"));
        assert!(is_alt_z_hotkey("alt + z"));
        assert!(!is_alt_z_hotkey("Ctrl+Z"));
        assert!(!is_alt_z_hotkey("Alt+Shift+Z"));
    }

    #[test]
    fn parses_mouse_side_buttons() {
        let x2 = parse_hotkey("MouseX2").expect("x2");
        assert_eq!(x2.key, Some(0x06));
        assert!(is_mouse_side_hotkey("MouseX2"));
        // Same as CapsLock: side buttons are swallowed so browser Back/Forward never fires.
        assert!(hotkey_supports_suppress("MouseX2"));
        assert!(hotkey_supports_suppress("MouseX1"));
        assert!(!hotkey_supports_suppress("F13"));
        assert_eq!(parse_hotkey_label("mousex1").unwrap(), "MouseX1");
        assert!(validate_hotkey_label("F13").is_ok());
        assert!(validate_hotkey_label("Ctrl").is_err());
    }
}
