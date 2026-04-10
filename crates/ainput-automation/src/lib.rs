use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VK_ESCAPE, VK_F7, VK_F8, VK_F9, VK_F10,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SW_SHOWNOACTIVATE, SetCursorPos, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::PCWSTR;

const SLOT_NAME_WATCH_INTERVAL: Duration = Duration::from_millis(400);
const PLAYBACK_WAIT_SLICE: Duration = Duration::from_millis(10);
const PLAYBACK_PAUSE_SLICE: Duration = Duration::from_millis(30);
const SLOT_COUNT: usize = 10;
const REPEAT_COUNT_MAX: usize = 5;
const CONTROL_VKEYS: [u32; 5] = [
    VK_F7.0 as u32,
    VK_F8.0 as u32,
    VK_F9.0 as u32,
    VK_F10.0 as u32,
    VK_ESCAPE.0 as u32,
];

pub const PAUSE_HOTKEY: &str = "F7";
pub const RECORD_HOTKEY: &str = "F8";
pub const STOP_HOTKEY: &str = "F9";
pub const PLAY_HOTKEY: &str = "F10";
pub const CANCEL_HOTKEY: &str = "Esc";

static APP_STATE: OnceLock<Arc<AppState>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationActivity {
    Idle,
    Recording,
    Playing,
    Paused,
    Error,
}

#[derive(Clone, Debug)]
pub struct SlotSnapshot {
    pub slot: usize,
    pub label: String,
    pub has_recording: bool,
}

#[derive(Clone, Debug)]
pub struct AutomationSnapshot {
    pub activity: AutomationActivity,
    pub status_line: String,
    pub active_slot: usize,
    pub active_slot_label: String,
    pub repeat_count: usize,
    pub elapsed_ms: u64,
    pub total_ms: Option<u64>,
    pub progress_ratio: Option<f32>,
    pub slots: Vec<SlotSnapshot>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum RecordedEvent {
    MouseMove {
        time_offset_ms: u64,
        x: i32,
        y: i32,
    },
    MouseButton {
        time_offset_ms: u64,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    },
    MouseWheel {
        time_offset_ms: u64,
        x: i32,
        y: i32,
        delta: i32,
        horizontal: bool,
    },
    Key {
        time_offset_ms: u64,
        vk_code: u32,
        scan_code: u32,
        pressed: bool,
        extended: bool,
        system: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum MouseButton {
    Left,
    Right,
    Middle,
}

struct RecorderState {
    started_at: Option<Instant>,
    events: Vec<RecordedEvent>,
    slot: usize,
}

impl Default for RecorderState {
    fn default() -> Self {
        Self {
            started_at: None,
            events: Vec::new(),
            slot: 1,
        }
    }
}

type UpdateCallback = dyn Fn() + Send + Sync + 'static;

struct AppState {
    recorder: Mutex<RecorderState>,
    slot_names: Mutex<Vec<String>>,
    status_line: Mutex<String>,
    activity: Mutex<AutomationActivity>,
    recording: AtomicBool,
    playing: AtomicBool,
    paused_playback: AtomicBool,
    stop_playback: AtomicBool,
    shutdown_requested: AtomicBool,
    active_slot: AtomicUsize,
    repeat_count: AtomicUsize,
    playback_elapsed_ms: AtomicU64,
    playback_total_ms: AtomicU64,
    base_dir: PathBuf,
    notify: Arc<UpdateCallback>,
}

impl AppState {
    fn new(base_dir: PathBuf, notify: Arc<UpdateCallback>) -> Self {
        Self {
            recorder: Mutex::new(RecorderState::default()),
            slot_names: Mutex::new(Vec::new()),
            status_line: Mutex::new(default_status_line()),
            activity: Mutex::new(AutomationActivity::Idle),
            recording: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            paused_playback: AtomicBool::new(false),
            stop_playback: AtomicBool::new(false),
            shutdown_requested: AtomicBool::new(false),
            active_slot: AtomicUsize::new(1),
            repeat_count: AtomicUsize::new(1),
            playback_elapsed_ms: AtomicU64::new(0),
            playback_total_ms: AtomicU64::new(0),
            base_dir,
            notify,
        }
    }

    fn start_recording(&self) -> Result<()> {
        self.ensure_slot_names_loaded()?;

        if self.playing.load(Ordering::SeqCst) {
            let message = "按键精灵：回放中，不能开始录制";
            self.set_status(AutomationActivity::Error, message);
            return Err(anyhow!(message));
        }
        if self.recording.load(Ordering::SeqCst) {
            self.set_status(
                AutomationActivity::Recording,
                format!("按键精灵：正在录制 {}", self.slot_label(self.active_slot())),
            );
            return Ok(());
        }

        let slot = self.active_slot();
        let mut recorder = self.recorder.lock().expect("recorder lock poisoned");
        recorder.events.clear();
        recorder.started_at = Some(Instant::now());
        recorder.slot = slot;
        drop(recorder);

        self.playback_elapsed_ms.store(0, Ordering::SeqCst);
        self.playback_total_ms.store(0, Ordering::SeqCst);
        self.recording.store(true, Ordering::SeqCst);
        self.set_status(
            AutomationActivity::Recording,
            format!("按键精灵：开始录制 {}", self.slot_label(slot)),
        );
        Ok(())
    }

    fn stop_recording(&self) -> Result<()> {
        if !self.recording.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        let (slot, event_count, payload) = {
            let mut recorder = self.recorder.lock().expect("recorder lock poisoned");
            recorder.started_at = None;
            let slot = recorder.slot;
            let event_count = recorder.events.len();
            let payload = serde_json::to_string_pretty(&recorder.events)?;
            (slot, event_count, payload)
        };

        let path = self.slot_path(slot);
        fs::create_dir_all(self.slots_dir()).with_context(|| {
            format!("create automation slots dir {}", self.slots_dir().display())
        })?;
        fs::write(&path, payload)
            .with_context(|| format!("write automation slot {}", path.display()))?;
        self.set_status(
            AutomationActivity::Idle,
            format!(
                "按键精灵：已保存 {}（{} 条事件）",
                self.slot_label(slot),
                event_count
            ),
        );
        Ok(())
    }

    fn stop_active(&self) -> Result<()> {
        if self.recording.load(Ordering::SeqCst) {
            self.stop_recording()?;
            return Ok(());
        }

        self.stop_playback();
        Ok(())
    }

    fn stop_playback(&self) {
        if self.playing.load(Ordering::SeqCst) {
            self.paused_playback.store(false, Ordering::SeqCst);
            self.stop_playback.store(true, Ordering::SeqCst);
            self.set_status(AutomationActivity::Idle, "按键精灵：已请求停止回放");
        }
    }

    fn toggle_pause_playback(&self) {
        if !self.playing.load(Ordering::SeqCst) {
            return;
        }

        let slot_label = self.slot_label(self.active_slot());
        if self.paused_playback.swap(false, Ordering::SeqCst) {
            self.set_status(
                AutomationActivity::Playing,
                format!("按键精灵：继续回放 {}", slot_label),
            );
        } else {
            self.paused_playback.store(true, Ordering::SeqCst);
            self.set_status(
                AutomationActivity::Paused,
                format!("按键精灵：已暂停 {}", slot_label),
            );
        }
    }

    fn pause_playback_on_user_input(&self, trigger: &str) {
        if !self.playing.load(Ordering::SeqCst) {
            return;
        }

        if !self.paused_playback.swap(true, Ordering::SeqCst) {
            self.set_status(
                AutomationActivity::Paused,
                format!("按键精灵：检测到{trigger}手动输入，已自动暂停"),
            );
        }
    }

    fn push_event(&self, event: RecordedEvent) {
        let mut recorder = self.recorder.lock().expect("recorder lock poisoned");
        recorder.events.push(event);
    }

    fn current_offset_ms(&self) -> Option<u64> {
        let recorder = self.recorder.lock().expect("recorder lock poisoned");
        recorder
            .started_at
            .map(|started_at| started_at.elapsed().as_millis() as u64)
    }

    fn active_slot(&self) -> usize {
        self.active_slot.load(Ordering::SeqCst)
    }

    fn repeat_count(&self) -> usize {
        self.repeat_count.load(Ordering::SeqCst)
    }

    fn select_slot(&self, slot: usize) -> Result<()> {
        self.ensure_slot_names_loaded()?;
        self.active_slot.store(slot, Ordering::SeqCst);
        self.set_status(
            AutomationActivity::Idle,
            format!("按键精灵：已切换到 {}", self.slot_label(slot)),
        );
        Ok(())
    }

    fn select_repeat_count(&self, repeat_count: usize) {
        self.repeat_count.store(repeat_count, Ordering::SeqCst);
        self.set_status(
            AutomationActivity::Idle,
            format!("按键精灵：回放轮数已切到 {repeat_count}"),
        );
    }

    fn slots_dir(&self) -> PathBuf {
        self.base_dir.join("slots")
    }

    fn slot_path(&self, slot: usize) -> PathBuf {
        self.slots_dir().join(format!("slot-{slot}.json"))
    }

    fn slot_has_recording(&self, slot: usize) -> bool {
        self.slot_path(slot).exists()
    }

    fn slot_names_path(&self) -> PathBuf {
        self.base_dir.join("slot-names.json")
    }

    fn ensure_slot_names_loaded(&self) -> Result<()> {
        let names = self.read_slot_names_from_disk()?;
        let mut guard = self.slot_names.lock().expect("slot names lock poisoned");
        *guard = names;
        Ok(())
    }

    fn refresh_slot_names(&self) -> Result<bool> {
        let names = self.read_slot_names_from_disk()?;
        let mut guard = self.slot_names.lock().expect("slot names lock poisoned");
        if *guard == names {
            return Ok(false);
        }
        *guard = names;
        drop(guard);
        self.notify_only();
        Ok(true)
    }

    fn read_slot_names_from_disk(&self) -> Result<Vec<String>> {
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("create automation base dir {}", self.base_dir.display()))?;

        let path = self.slot_names_path();
        if !path.exists() {
            let defaults: Vec<String> = (1..=SLOT_COUNT)
                .map(|slot| format!("槽位 {slot}"))
                .collect();
            let payload = serde_json::to_string_pretty(&defaults)?;
            fs::write(&path, payload)
                .with_context(|| format!("write automation slot names {}", path.display()))?;
        }

        let payload = fs::read_to_string(&path)
            .with_context(|| format!("read automation slot names {}", path.display()))?;
        let mut names: Vec<String> = serde_json::from_str(&payload)
            .with_context(|| format!("parse automation slot names {}", path.display()))?;
        if names.len() < SLOT_COUNT {
            for slot in names.len() + 1..=SLOT_COUNT {
                names.push(format!("槽位 {slot}"));
            }
        } else if names.len() > SLOT_COUNT {
            names.truncate(SLOT_COUNT);
        }
        Ok(names)
    }

    fn slot_label(&self, slot: usize) -> String {
        let guard = self.slot_names.lock().expect("slot names lock poisoned");
        guard
            .get(slot.saturating_sub(1))
            .cloned()
            .unwrap_or_else(|| format!("槽位 {slot}"))
    }

    fn open_slot_names_file(&self) -> Result<()> {
        self.ensure_slot_names_loaded()?;
        open_path(&self.slot_names_path())
    }

    fn open_slots_dir(&self) -> Result<()> {
        fs::create_dir_all(self.slots_dir()).with_context(|| {
            format!("create automation slots dir {}", self.slots_dir().display())
        })?;
        open_path(&self.slots_dir())
    }

    fn set_playback_progress(&self, elapsed_ms: u64, total_ms: u64) {
        self.playback_elapsed_ms.store(elapsed_ms, Ordering::SeqCst);
        self.playback_total_ms.store(total_ms, Ordering::SeqCst);
    }

    fn clear_playback_progress(&self) {
        self.playback_elapsed_ms.store(0, Ordering::SeqCst);
        self.playback_total_ms.store(0, Ordering::SeqCst);
    }

    fn snapshot(&self) -> AutomationSnapshot {
        let activity = *self.activity.lock().expect("activity lock poisoned");
        let active_slot = self.active_slot();
        let repeat_count = self.repeat_count();
        let active_slot_label = self.slot_label(active_slot);
        let slots = (1..=SLOT_COUNT)
            .map(|slot| SlotSnapshot {
                slot,
                label: self.slot_label(slot),
                has_recording: self.slot_has_recording(slot),
            })
            .collect();

        let (elapsed_ms, total_ms, progress_ratio) = match activity {
            AutomationActivity::Recording => (self.current_offset_ms().unwrap_or(0), None, None),
            AutomationActivity::Playing | AutomationActivity::Paused => {
                let elapsed_ms = self.playback_elapsed_ms.load(Ordering::SeqCst);
                let total_ms = self.playback_total_ms.load(Ordering::SeqCst);
                let total_ms = (total_ms > 0).then_some(total_ms);
                let progress_ratio =
                    total_ms.map(|total| (elapsed_ms as f32 / total as f32).clamp(0.0, 1.0));
                (elapsed_ms, total_ms, progress_ratio)
            }
            AutomationActivity::Idle | AutomationActivity::Error => (0, None, None),
        };

        AutomationSnapshot {
            activity,
            status_line: self
                .status_line
                .lock()
                .expect("status lock poisoned")
                .clone(),
            active_slot,
            active_slot_label,
            repeat_count,
            elapsed_ms,
            total_ms,
            progress_ratio,
            slots,
        }
    }

    fn set_status<S: Into<String>>(&self, activity: AutomationActivity, status_line: S) {
        *self.activity.lock().expect("activity lock poisoned") = activity;
        *self.status_line.lock().expect("status lock poisoned") = status_line.into();
        self.notify_only();
    }

    fn notify_only(&self) {
        (self.notify)();
    }
}

pub struct AutomationService {
    state: Arc<AppState>,
    hook_thread_id: u32,
    hook_join_handle: Option<thread::JoinHandle<()>>,
    slot_name_watch_join_handle: Option<thread::JoinHandle<()>>,
}

impl AutomationService {
    pub fn start<F>(base_dir: PathBuf, on_update: F) -> Result<Self>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let notify = Arc::new(on_update);
        let state = Arc::new(AppState::new(base_dir, notify));
        state.ensure_slot_names_loaded()?;

        APP_STATE
            .set(state.clone())
            .map_err(|_| anyhow!("ainput automation service already started"))?;

        unsafe {
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }

        let (thread_id_tx, thread_id_rx) = std::sync::mpsc::channel();
        let hook_join_handle = thread::spawn(move || unsafe {
            let thread_id = GetCurrentThreadId();
            let _ = thread_id_tx.send(thread_id);

            let instance = GetModuleHandleW(None)
                .ok()
                .map(|module| HINSTANCE(module.0));
            let keyboard_hook =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), instance, 0)
                    .map_err(|error| anyhow!("install automation keyboard hook failed: {error}"));
            let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), instance, 0)
                .map_err(|error| anyhow!("install automation mouse hook failed: {error}"));

            let (keyboard_hook, mouse_hook) = match (keyboard_hook, mouse_hook) {
                (Ok(keyboard_hook), Ok(mouse_hook)) => (keyboard_hook, mouse_hook),
                (Err(error), _) | (_, Err(error)) => {
                    if let Some(state) = APP_STATE.get() {
                        state.set_status(AutomationActivity::Error, format!("按键精灵：{error}"));
                    }
                    return;
                }
            };

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
            }

            let _ = UnhookWindowsHookEx(keyboard_hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
        });

        let hook_thread_id = thread_id_rx
            .recv()
            .map_err(|_| anyhow!("read automation hook thread id failed"))?;

        let watch_state = state.clone();
        let slot_name_watch_join_handle =
            thread::spawn(move || spawn_slot_name_watch_loop(watch_state));

        state.set_status(AutomationActivity::Idle, default_status_line());

        Ok(Self {
            state,
            hook_thread_id,
            hook_join_handle: Some(hook_join_handle),
            slot_name_watch_join_handle: Some(slot_name_watch_join_handle),
        })
    }

    pub fn snapshot(&self) -> AutomationSnapshot {
        self.state.snapshot()
    }

    pub fn start_recording(&self) -> Result<()> {
        self.state.start_recording()
    }

    pub fn stop_recording(&self) -> Result<()> {
        self.state.stop_recording()
    }

    pub fn start_playback(&self) -> Result<()> {
        start_playback(self.state.clone())
    }

    pub fn toggle_pause_playback(&self) {
        self.state.toggle_pause_playback();
    }

    pub fn stop_active(&self) -> Result<()> {
        self.state.stop_active()
    }

    pub fn select_slot(&self, slot: usize) -> Result<()> {
        if !(1..=SLOT_COUNT).contains(&slot) {
            return Err(anyhow!("invalid automation slot {slot}"));
        }
        self.state.select_slot(slot)
    }

    pub fn select_repeat_count(&self, repeat_count: usize) -> Result<()> {
        if !(1..=REPEAT_COUNT_MAX).contains(&repeat_count) {
            return Err(anyhow!("invalid automation repeat count {repeat_count}"));
        }
        self.state.select_repeat_count(repeat_count);
        Ok(())
    }

    pub fn open_slot_names_file(&self) -> Result<()> {
        self.state.open_slot_names_file()
    }

    pub fn open_slots_dir(&self) -> Result<()> {
        self.state.open_slots_dir()
    }

    pub fn refresh_slot_names(&self) -> Result<()> {
        let _ = self.state.refresh_slot_names()?;
        Ok(())
    }
}

impl Drop for AutomationService {
    fn drop(&mut self) {
        self.state.shutdown_requested.store(true, Ordering::SeqCst);
        self.state.stop_playback.store(true, Ordering::SeqCst);
        let _ = unsafe { PostThreadMessageW(self.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(handle) = self.slot_name_watch_join_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.hook_join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_slot_name_watch_loop(state: Arc<AppState>) {
    let mut last_modified = slot_names_modified_at(&state.slot_names_path());
    loop {
        if state.shutdown_requested.load(Ordering::SeqCst) {
            break;
        }

        let current_modified = slot_names_modified_at(&state.slot_names_path());
        if current_modified != last_modified {
            last_modified = current_modified;
            if let Err(error) = state.refresh_slot_names() {
                state.set_status(AutomationActivity::Error, format!("按键精灵：{error}"));
            }
        }

        thread::sleep(SLOT_NAME_WATCH_INTERVAL);
    }
}

fn slot_names_modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn start_playback(state: Arc<AppState>) -> Result<()> {
    if state.recording.load(Ordering::SeqCst) {
        return Err(anyhow!("录制中，不能开始回放"));
    }
    if state.playing.swap(true, Ordering::SeqCst) {
        state.set_status(
            if state.paused_playback.load(Ordering::SeqCst) {
                AutomationActivity::Paused
            } else {
                AutomationActivity::Playing
            },
            format!(
                "按键精灵：正在回放 {}",
                state.slot_label(state.active_slot())
            ),
        );
        return Ok(());
    }

    if let Err(error) = state.ensure_slot_names_loaded() {
        state.playing.store(false, Ordering::SeqCst);
        return Err(error);
    }

    let slot = state.active_slot();
    let repeat_count = state.repeat_count();
    let path = state.slot_path(slot);
    let payload = match fs::read_to_string(&path)
        .with_context(|| format!("read automation slot {}", path.display()))
    {
        Ok(payload) => payload,
        Err(error) => {
            state.playing.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let events: Vec<RecordedEvent> = match serde_json::from_str(&payload)
        .with_context(|| format!("parse automation slot {}", path.display()))
    {
        Ok(events) => events,
        Err(error) => {
            state.playing.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };

    if events.is_empty() {
        state.playing.store(false, Ordering::SeqCst);
        return Err(anyhow!("录制文件为空，无法回放"));
    }

    let single_total_ms = events
        .iter()
        .map(RecordedEvent::offset_ms)
        .max()
        .unwrap_or(0);
    let total_ms = single_total_ms.saturating_mul(repeat_count as u64).max(1);
    state.stop_playback.store(false, Ordering::SeqCst);
    state.paused_playback.store(false, Ordering::SeqCst);
    state.set_playback_progress(0, total_ms);
    state.set_status(
        AutomationActivity::Playing,
        format!(
            "按键精灵：开始回放 {}（{} 轮）",
            state.slot_label(slot),
            repeat_count
        ),
    );

    thread::spawn(move || {
        let mut finished_normally = true;

        for cycle_index in 0..repeat_count {
            if state.stop_playback.load(Ordering::SeqCst) {
                finished_normally = false;
                break;
            }

            let started = Instant::now();
            let mut paused_started_at: Option<Instant> = None;
            let mut paused_total = Duration::ZERO;
            let completed_before_cycle_ms = single_total_ms.saturating_mul(cycle_index as u64);

            for event in &events {
                if state.stop_playback.load(Ordering::SeqCst) {
                    finished_normally = false;
                    break;
                }

                let offset = event.offset_ms();
                let target = Duration::from_millis(offset);
                loop {
                    if state.stop_playback.load(Ordering::SeqCst) {
                        finished_normally = false;
                        break;
                    }

                    if state.paused_playback.load(Ordering::SeqCst) {
                        if paused_started_at.is_none() {
                            paused_started_at = Some(Instant::now());
                        }
                        thread::sleep(PLAYBACK_PAUSE_SLICE);
                        continue;
                    }

                    if let Some(paused_started) = paused_started_at.take() {
                        paused_total += paused_started.elapsed();
                    }

                    let elapsed = started.elapsed().saturating_sub(paused_total);
                    let cycle_elapsed_ms = elapsed.as_millis() as u64;
                    state.set_playback_progress(
                        completed_before_cycle_ms
                            .saturating_add(cycle_elapsed_ms.min(single_total_ms)),
                        total_ms,
                    );

                    if elapsed >= target {
                        break;
                    }

                    let wait = target - elapsed;
                    thread::sleep(wait.min(PLAYBACK_WAIT_SLICE));
                }

                if state.stop_playback.load(Ordering::SeqCst) {
                    finished_normally = false;
                    break;
                }

                if let Err(error) = unsafe { playback_event(event) } {
                    state.set_status(
                        AutomationActivity::Error,
                        format!("按键精灵：回放失败 - {error}"),
                    );
                    state.stop_playback.store(true, Ordering::SeqCst);
                    state.playing.store(false, Ordering::SeqCst);
                    state.clear_playback_progress();
                    return;
                }
            }

            state.set_playback_progress(
                completed_before_cycle_ms.saturating_add(single_total_ms),
                total_ms,
            );
        }

        state.paused_playback.store(false, Ordering::SeqCst);
        state.stop_playback.store(false, Ordering::SeqCst);
        state.playing.store(false, Ordering::SeqCst);
        state.clear_playback_progress();
        if finished_normally {
            state.set_status(AutomationActivity::Idle, "按键精灵：回放结束");
        } else {
            state.set_status(AutomationActivity::Idle, "按键精灵：已停止当前回放");
        }
    });

    Ok(())
}

impl RecordedEvent {
    fn offset_ms(&self) -> u64 {
        match self {
            RecordedEvent::MouseMove { time_offset_ms, .. }
            | RecordedEvent::MouseButton { time_offset_ms, .. }
            | RecordedEvent::MouseWheel { time_offset_ms, .. }
            | RecordedEvent::Key { time_offset_ms, .. } => *time_offset_ms,
        }
    }
}

unsafe fn playback_event(event: &RecordedEvent) -> Result<()> {
    match event {
        RecordedEvent::MouseMove { x, y, .. } => unsafe { move_cursor(*x, *y)? },
        RecordedEvent::MouseButton {
            x,
            y,
            button,
            pressed,
            ..
        } => {
            unsafe { move_cursor(*x, *y)? };
            let flag = match (button, pressed) {
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            unsafe { send_mouse_flag(flag, 0)? };
        }
        RecordedEvent::MouseWheel {
            x,
            y,
            delta,
            horizontal,
            ..
        } => {
            unsafe { move_cursor(*x, *y)? };
            let flag = if *horizontal {
                MOUSEEVENTF_HWHEEL
            } else {
                MOUSEEVENTF_WHEEL
            };
            unsafe { send_mouse_flag(flag, *delta as u32)? };
        }
        RecordedEvent::Key {
            scan_code,
            pressed,
            extended,
            ..
        } => {
            let mut flags = KEYEVENTF_SCANCODE;
            if !pressed {
                flags |= KEYEVENTF_KEYUP;
            }
            if *extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }

            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: Default::default(),
                        wScan: *scan_code as u16,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { send_inputs(&[input])? };
        }
    }

    Ok(())
}

unsafe fn move_cursor(x: i32, y: i32) -> Result<()> {
    unsafe {
        SetCursorPos(x, y)
            .ok()
            .with_context(|| format!("move cursor to ({x}, {y}) failed"))
    }
}

unsafe fn send_mouse_flag(flags: MOUSE_EVENT_FLAGS, mouse_data: u32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { send_inputs(&[input]) }
}

unsafe fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        Err(anyhow!(
            "SendInput only sent {sent}/{} input events",
            inputs.len()
        ))
    } else {
        Ok(())
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(state) = APP_STATE.get()
    {
        let info = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        let injected = (info.flags & LLKHF_INJECTED) == LLKHF_INJECTED;
        let pressed = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let released = matches!(message, WM_KEYUP | WM_SYSKEYUP);

        if state.playing.load(Ordering::SeqCst)
            && !injected
            && !CONTROL_VKEYS.contains(&info.vkCode)
            && (pressed || released)
        {
            state.pause_playback_on_user_input("键盘");
        }

        if state.recording.load(Ordering::SeqCst)
            && !state.playing.load(Ordering::SeqCst)
            && !injected
            && !CONTROL_VKEYS.contains(&info.vkCode)
            && (pressed || released)
            && let Some(offset) = state.current_offset_ms()
        {
            state.push_event(RecordedEvent::Key {
                time_offset_ms: offset,
                vk_code: info.vkCode,
                scan_code: info.scanCode,
                pressed,
                extended: (info.flags & LLKHF_EXTENDED) == LLKHF_EXTENDED,
                system: matches!(message, WM_SYSKEYDOWN | WM_SYSKEYUP),
            });
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(state) = APP_STATE.get()
    {
        let info = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        let injected = info.flags & 0x01 != 0;

        if state.playing.load(Ordering::SeqCst) && !injected {
            match message {
                WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
                | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    state.pause_playback_on_user_input("鼠标");
                }
                _ => {}
            }
        }

        if state.recording.load(Ordering::SeqCst)
            && !state.playing.load(Ordering::SeqCst)
            && !injected
            && let Some(offset) = state.current_offset_ms()
        {
            let point = info.pt;
            match message {
                WM_MOUSEMOVE => state.push_event(RecordedEvent::MouseMove {
                    time_offset_ms: offset,
                    x: point.x,
                    y: point.y,
                }),
                WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
                | WM_MBUTTONUP => {
                    let (button, pressed) = match message {
                        WM_LBUTTONDOWN => (MouseButton::Left, true),
                        WM_LBUTTONUP => (MouseButton::Left, false),
                        WM_RBUTTONDOWN => (MouseButton::Right, true),
                        WM_RBUTTONUP => (MouseButton::Right, false),
                        WM_MBUTTONDOWN => (MouseButton::Middle, true),
                        WM_MBUTTONUP => (MouseButton::Middle, false),
                        _ => unreachable!(),
                    };

                    state.push_event(RecordedEvent::MouseButton {
                        time_offset_ms: offset,
                        x: point.x,
                        y: point.y,
                        button,
                        pressed,
                    });
                }
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    let delta = ((info.mouseData >> 16) & 0xffff) as i16 as i32;
                    state.push_event(RecordedEvent::MouseWheel {
                        time_offset_ms: offset,
                        x: point.x,
                        y: point.y,
                        delta,
                        horizontal: message == WM_MOUSEHWHEEL,
                    });
                }
                _ => {}
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn open_path(path: &Path) -> Result<()> {
    let operation = to_wide("open");
    let target = to_wide(&path.to_string_lossy());
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNOACTIVATE,
        )
    };

    if result.0 as usize <= 32 {
        return Err(anyhow!("open path failed: {}", path.display()));
    }
    Ok(())
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn default_status_line() -> String {
    format!(
        "按键精灵：待机（{} 仅暂停回放 / {} 录制 / {} 保存 / {} 回放 / {} 停止）",
        PAUSE_HOTKEY, RECORD_HOTKEY, STOP_HOTKEY, PLAY_HOTKEY, CANCEL_HOTKEY
    )
}
