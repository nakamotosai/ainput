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
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
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
    keyboard_hook_thread_id: Option<u32>,
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
const KEYBOARD_HOOK_QUIT: u32 = WM_APP + 88;
const VK_ALT: u16 = 0x12;
const VK_Z: u16 = b'Z' as u16;
static CAPSLOCK_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_Z_DOWN: AtomicBool = AtomicBool::new(false);
static SUPPRESS_CAPSLOCK_HOTKEY: AtomicBool = AtomicBool::new(false);
static SUPPRESS_ALT_Z_HOTKEY: AtomicBool = AtomicBool::new(false);

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
        let mut keyboard_hook_thread_id = None;
        let suppress_capslock = (profiles.whisper.enabled
            && profiles.whisper.suppress_key
            && is_capslock_hotkey(&profiles.whisper.hotkey))
            || (profiles.local_nonstreaming.enabled
                && profiles.local_nonstreaming.suppress_key
                && is_capslock_hotkey(&profiles.local_nonstreaming.hotkey));
        let suppress_alt_z = (profiles.whisper.enabled
            && profiles.whisper.suppress_key
            && is_alt_z_hotkey(&profiles.whisper.hotkey))
            || (profiles.local_nonstreaming.enabled
                && profiles.local_nonstreaming.suppress_key
                && is_alt_z_hotkey(&profiles.local_nonstreaming.hotkey));

        // Keyboard hook for CapsLock/Alt+Z suppression when needed.
        match start_keyboard_hook_thread(suppress_capslock, suppress_alt_z) {
            Ok((thread_id, join)) => {
                keyboard_hook_thread_id = Some(thread_id);
                joins.push(join);
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "keyboard hook failed; suppressed profiles may be disabled"
                );
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
            if is_capslock_hotkey(&profiles.whisper.hotkey) && profiles.whisper.suppress_key {
                if keyboard_hook_thread_id.is_some() {
                    joins.push(spawn_suppressed_state_monitor(
                        VoiceProfileId::WhisperCapslock,
                        profiles.whisper.clone(),
                        poll,
                        tx.clone(),
                        Arc::clone(&shutdown),
                        Arc::clone(&stop),
                        SuppressedHotkeyKind::CapsLock,
                    ));
                } else {
                    warn!("CapsLock hook unavailable; whisper_capslock profile disabled");
                }
            } else if is_alt_z_hotkey(&profiles.whisper.hotkey) && profiles.whisper.suppress_key {
                if keyboard_hook_thread_id.is_some() {
                    joins.push(spawn_suppressed_state_monitor(
                        VoiceProfileId::WhisperCapslock,
                        profiles.whisper.clone(),
                        poll,
                        tx.clone(),
                        Arc::clone(&shutdown),
                        Arc::clone(&stop),
                        SuppressedHotkeyKind::AltZ,
                    ));
                } else {
                    warn!("Alt+Z hook unavailable; whisper profile disabled");
                }
            } else if profiles.whisper.suppress_key {
                warn!(
                    hotkey = %profiles.whisper.hotkey,
                    "suppress_key is only supported for CapsLock or Alt+Z; whisper profile disabled"
                );
            } else {
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

        if profiles.local_nonstreaming.enabled {
            if is_capslock_hotkey(&profiles.local_nonstreaming.hotkey)
                && profiles.local_nonstreaming.suppress_key
            {
                if keyboard_hook_thread_id.is_some() {
                    joins.push(spawn_suppressed_state_monitor(
                        VoiceProfileId::LocalNonstreaming,
                        profiles.local_nonstreaming.clone(),
                        poll,
                        tx,
                        shutdown,
                        Arc::clone(&stop),
                        SuppressedHotkeyKind::CapsLock,
                    ));
                } else {
                    warn!("CapsLock hook unavailable; local_nonstreaming profile disabled");
                }
            } else if is_alt_z_hotkey(&profiles.local_nonstreaming.hotkey)
                && profiles.local_nonstreaming.suppress_key
            {
                if keyboard_hook_thread_id.is_some() {
                    joins.push(spawn_suppressed_state_monitor(
                        VoiceProfileId::LocalNonstreaming,
                        profiles.local_nonstreaming.clone(),
                        poll,
                        tx,
                        shutdown,
                        Arc::clone(&stop),
                        SuppressedHotkeyKind::AltZ,
                    ));
                } else {
                    warn!("Alt+Z hook unavailable; local_nonstreaming profile disabled");
                }
            } else if profiles.local_nonstreaming.suppress_key {
                warn!(
                    hotkey = %profiles.local_nonstreaming.hotkey,
                    "suppress_key is only supported for CapsLock or Alt+Z; local_nonstreaming profile disabled"
                );
            } else {
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

        Ok(Self {
            stop,
            joins,
            keyboard_hook_thread_id,
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread_id) = self.keyboard_hook_thread_id.take() {
            unsafe {
                let _ = PostThreadMessageW(thread_id, KEYBOARD_HOOK_QUIT, WPARAM(0), LPARAM(0));
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
    CAPSLOCK_DOWN.store(false, Ordering::Relaxed);
    ALT_Z_DOWN.store(false, Ordering::Relaxed);
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
            },
        );
        match kind {
            SuppressedHotkeyKind::CapsLock => CAPSLOCK_DOWN.store(false, Ordering::Relaxed),
            SuppressedHotkeyKind::AltZ => ALT_Z_DOWN.store(false, Ordering::Relaxed),
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

fn start_keyboard_hook_thread(
    suppress_capslock: bool,
    suppress_alt_z: bool,
) -> Result<(u32, thread::JoinHandle<()>)> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let join = thread::spawn(move || {
        SUPPRESS_CAPSLOCK_HOTKEY.store(suppress_capslock, Ordering::Relaxed);
        SUPPRESS_ALT_Z_HOTKEY.store(suppress_alt_z, Ordering::Relaxed);
        CAPSLOCK_DOWN.store(false, Ordering::Relaxed);
        ALT_Z_DOWN.store(false, Ordering::Relaxed);
        let thread_id = unsafe { GetCurrentThreadId() };
        if suppress_capslock {
            force_capslock_off("before_hook_install");
        }
        let hook =
            match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) } {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error}")));
                    return;
                }
            };
        let _ = ready_tx.send(Ok(thread_id));
        info!(
            thread_id,
            suppress_capslock, suppress_alt_z, "low-level keyboard hook installed"
        );
        loop {
            let mut msg = MSG::default();
            let has_message = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if has_message.0 == -1 {
                warn!("CapsLock hook GetMessage failed");
                break;
            }
            if has_message.0 == 0 || msg.message == KEYBOARD_HOOK_QUIT {
                break;
            }
        }
        if let Err(error) = unsafe { UnhookWindowsHookEx(hook) } {
            warn!(error = %error, "unhook CapsLock low-level keyboard hook failed");
        }
        CAPSLOCK_DOWN.store(false, Ordering::Relaxed);
        ALT_Z_DOWN.store(false, Ordering::Relaxed);
        SUPPRESS_CAPSLOCK_HOTKEY.store(false, Ordering::Relaxed);
        SUPPRESS_ALT_Z_HOTKEY.store(false, Ordering::Relaxed);
        if suppress_capslock {
            force_capslock_off("after_hook_stop");
        }
        info!(thread_id, "low-level keyboard hook stopped");
    });
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow!("CapsLock hook thread did not initialize"))?
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
    use super::{is_alt_z_hotkey, is_capslock_hotkey, parse_hotkey};

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
}
