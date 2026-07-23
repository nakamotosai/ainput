use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use arboard::Clipboard;
#[cfg(test)]
use serde::Deserialize;
use tracing::{info, warn};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPattern2Id, UIA_TextPatternId,
};
use windows::Win32::UI::Controls::{EM_GETSEL, EM_SETSEL};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_TYPE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
    VK_XBUTTON1, VK_XBUTTON2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SendMessageTimeoutW, WM_GETTEXT, WM_GETTEXTLENGTH,
};
use windows::core::{BOOL, IUnknown, PWSTR};

use crate::config::{ClipboardPolicy, OutputConfig};

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
const VK_ALT: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
const VK_RWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5C);
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(350);
const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(8);
const TARGET_TEXT_TIMEOUT_MS: u32 = 25;
const TARGET_TEXT_MAX_U16: usize = 32_768;
const MAX_SAFE_REPLACEMENT_CHARS: usize = 1000;
/// Chromium / Electron often drop tail events when one SendInput floods Shift+Left.
/// Keep each keyboard batch small and pause between batches so the caret can catch up.
const SHIFT_LEFT_CHUNK_SIZE: usize = 4;
const SHIFT_LEFT_CHUNK_GAP: Duration = Duration::from_millis(10);
const SELECTION_SETTLE_MS: u64 = 12;

thread_local! {
    static COM_INIT_ATTEMPTED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetryStats {
    attempts: u32,
    retries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardWriteReport {
    policy: ClipboardPolicy,
    set_attempts: u32,
    set_retries: u32,
    set_elapsed_ms: u128,
    previous_text: Option<String>,
    previous_text_captured: bool,
    previous_text_error: String,
    restore_attempted: bool,
    restore_ok: bool,
    restore_error: String,
}

impl ClipboardWriteReport {
    fn clipboard_retained(&self) -> bool {
        !(self.restore_attempted && self.restore_ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteOutcome {
    pub text: String,
    pub target_context: TargetInsertionContext,
    pub target_summary: TargetSummary,
    pub target_fingerprint: TargetFingerprint,
    pub text_actions: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacementOutcome {
    pub applied: bool,
    pub reason: String,
    pub context_source: String,
    pub output_actions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteOutputRoute {
    ReplaceCapable,
    HudFirstFinalPaste,
}

impl RewriteOutputRoute {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplaceCapable => "replace_capable",
            Self::HudFirstFinalPaste => "hud_first_final_paste",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPunctuationPolicy {
    TargetAware,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMatchPolicy {
    BestEffort,
    RequireSame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTarget {
    pub summary: TargetSummary,
    pub fingerprint: TargetFingerprint,
    pub context: TargetInsertionContext,
    pub route: RewriteOutputRoute,
    process_path: String,
}

impl OutputTarget {
    fn is_wezterm(&self) -> bool {
        self.summary
            .process_name
            .eq_ignore_ascii_case("wezterm-gui.exe")
    }
}

impl ReplacementOutcome {
    pub fn skipped(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            applied: false,
            context_source: String::new(),
            output_actions: format!("replacement_skipped:{reason}"),
            reason,
        }
    }

    fn applied(context_source: impl Into<String>) -> Self {
        let context_source = context_source.into();
        Self {
            applied: true,
            reason: String::new(),
            output_actions: format!("replacement_applied:{context_source}"),
            context_source,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetSummary {
    pub process_name: String,
    pub class_name: String,
    pub title: String,
}

impl TargetSummary {
    pub fn is_terminal_target(&self) -> bool {
        is_terminal_process_name(&self.process_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetFingerprint {
    pub hwnd: usize,
    pub process_id: u32,
    pub process_name: String,
    pub class_name: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInsertionContext {
    pub right: TargetRightContext,
    pub source: &'static str,
    pub focus_class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetRightContext {
    Empty,
    NonEmpty,
    Unknown,
}

pub fn capture_output_target() -> OutputTarget {
    let target = ForegroundWindowSnapshot::capture();
    let summary = target.summary();
    let context = TargetInsertionContext::capture(&target);
    let route = classify_rewrite_output_route(&summary, &context);
    OutputTarget {
        summary,
        fingerprint: target.fingerprint(),
        context,
        route,
        process_path: target.process_path.clone().unwrap_or_default(),
    }
}

pub fn foreground_matches_target(original_target: &TargetFingerprint) -> bool {
    let current = ForegroundWindowSnapshot::capture();
    same_window_identity(original_target, &current.fingerprint())
}

fn retry_with_backoff<T, E, F>(
    retry_count: u32,
    backoff: Duration,
    mut operation: F,
) -> (std::result::Result<T, E>, RetryStats)
where
    F: FnMut() -> std::result::Result<T, E>,
{
    let max_attempts = retry_count.saturating_add(1).max(1);
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        match operation() {
            Ok(value) => {
                return (
                    Ok(value),
                    RetryStats {
                        attempts,
                        retries: attempts.saturating_sub(1),
                    },
                );
            }
            Err(error) if attempts >= max_attempts => {
                return (
                    Err(error),
                    RetryStats {
                        attempts,
                        retries: attempts.saturating_sub(1),
                    },
                );
            }
            Err(_) => {
                if !backoff.is_zero() {
                    thread::sleep(backoff);
                }
            }
        }
    }
}

fn capture_previous_clipboard_text(policy: ClipboardPolicy) -> (Option<String>, String) {
    if !policy.restores_text_after_success() {
        return (None, String::new());
    }
    match Clipboard::new()
        .map_err(|error| anyhow!("{error}"))
        .and_then(|mut clipboard| clipboard.get_text().map_err(|error| anyhow!("{error}")))
    {
        Ok(text) if !text.is_empty() => (Some(text), String::new()),
        Ok(_) => (None, "previous_text_empty".to_string()),
        Err(error) => (None, error.to_string()),
    }
}

fn set_clipboard_text_with_retry(
    text: &str,
    config: &OutputConfig,
) -> Result<ClipboardWriteReport> {
    let started_at = Instant::now();
    let policy = config.clipboard_policy;
    let (previous_text, previous_text_error) = capture_previous_clipboard_text(policy);
    let text_to_set = text.to_string();
    let (set_result, stats) = retry_with_backoff(
        config.clipboard_retry_count,
        Duration::from_millis(config.clipboard_retry_backoff_ms),
        || {
            let mut clipboard = Clipboard::new().map_err(|error| anyhow!("{error}"))?;
            clipboard
                .set_text(text_to_set.clone())
                .map_err(|error| anyhow!("{error}"))
        },
    );
    set_result?;
    Ok(ClipboardWriteReport {
        policy,
        set_attempts: stats.attempts,
        set_retries: stats.retries,
        set_elapsed_ms: started_at.elapsed().as_millis(),
        previous_text_captured: previous_text.is_some(),
        previous_text,
        previous_text_error,
        restore_attempted: false,
        restore_ok: false,
        restore_error: String::new(),
    })
}

fn restore_previous_clipboard_text_if_needed(
    report: &mut ClipboardWriteReport,
    config: &OutputConfig,
) {
    if !report.policy.restores_text_after_success() || !report.previous_text_captured {
        return;
    }
    let Some(previous_text) = report.previous_text.clone() else {
        report.restore_attempted = true;
        report.restore_error = "previous_text_missing".to_string();
        return;
    };
    if config.clipboard_restore_delay_ms > 0 {
        thread::sleep(Duration::from_millis(config.clipboard_restore_delay_ms));
    }
    report.restore_attempted = true;
    let (restore_result, _) = retry_with_backoff(
        config.clipboard_retry_count,
        Duration::from_millis(config.clipboard_retry_backoff_ms),
        || {
            let mut clipboard = Clipboard::new().map_err(|error| anyhow!("{error}"))?;
            clipboard
                .set_text(previous_text.clone())
                .map_err(|error| anyhow!("{error}"))
        },
    );
    match restore_result {
        Ok(()) => {
            report.restore_ok = true;
        }
        Err(error) => {
            report.restore_error = error.to_string();
        }
    }
}

fn append_action(actions: &mut String, action: &str) {
    if actions.is_empty() || actions == "none" {
        *actions = action.to_string();
    } else {
        actions.push(',');
        actions.push_str(action);
    }
}

fn direct_output_disabled_reason(config: &OutputConfig) -> Option<&'static str> {
    if config.clipboard_policy.is_copy_only() {
        Some("copy_only_policy")
    } else if !config.prefer_direct_paste {
        Some("direct_paste_disabled")
    } else {
        None
    }
}

fn input_preflight_block_reason(
    modifiers_down: bool,
    mouse_down: bool,
    modifier_reason: &'static str,
    mouse_reason: &'static str,
) -> Option<&'static str> {
    if modifiers_down {
        Some(modifier_reason)
    } else if mouse_down {
        Some(mouse_reason)
    } else {
        None
    }
}

fn current_input_preflight_block_reason(
    modifier_reason: &'static str,
    mouse_reason: &'static str,
) -> Option<&'static str> {
    input_preflight_block_reason(
        any_modifier_down(),
        any_mouse_button_down(),
        modifier_reason,
        mouse_reason,
    )
}

pub fn paste_text_with_trace(
    text: &str,
    config: &OutputConfig,
    utterance_id: &str,
) -> Result<PasteOutcome> {
    let target = capture_output_target();
    paste_text_to_target_with_trace(
        text,
        &target,
        config,
        utterance_id,
        TargetPunctuationPolicy::TargetAware,
        TargetMatchPolicy::BestEffort,
    )
}

pub fn paste_text_to_target_with_trace(
    text: &str,
    target: &OutputTarget,
    config: &OutputConfig,
    utterance_id: &str,
    punctuation_policy: TargetPunctuationPolicy,
    match_policy: TargetMatchPolicy,
) -> Result<PasteOutcome> {
    if text.trim().is_empty() {
        return Ok(PasteOutcome {
            text: String::new(),
            target_context: target.context.clone(),
            target_summary: target.summary.clone(),
            target_fingerprint: target.fingerprint.clone(),
            text_actions: "none".to_string(),
        });
    }

    if matches!(match_policy, TargetMatchPolicy::RequireSame)
        && !foreground_matches_target(&target.fingerprint)
    {
        warn!(
            utterance_id,
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            "paste skipped because captured target changed"
        );
        return Err(anyhow!("target changed before paste"));
    }

    let mut prepared = match punctuation_policy {
        TargetPunctuationPolicy::TargetAware => {
            apply_target_punctuation_rule(text, target.context.right)
        }
        TargetPunctuationPolicy::Preserve => PasteOutcomeText {
            text: text.to_string(),
            actions: "target_punctuation_preserved".to_string(),
        },
    };
    let input_before_clipboard = InputStateSnapshot::capture();
    if target.is_wezterm() {
        info!(
            utterance_id,
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_process_path = %target.process_path,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            target_right_context = target.context.right.as_str(),
            target_context_source = target.context.source,
            target_focus_class = %target.context.focus_class.as_deref().unwrap_or(""),
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_has_terminal_mouse_escape = contains_terminal_mouse_escape(&prepared.text),
            modifiers = %input_before_clipboard.modifiers,
            mouse_buttons = %input_before_clipboard.mouse_buttons,
            cursor = %input_before_clipboard.cursor_label(),
            prefer_direct_paste = config.prefer_direct_paste,
            paste_stabilize_ms = config.paste_stabilize_ms,
            clipboard_policy = config.clipboard_policy.as_str(),
            "terminal paste forensic snapshot before clipboard"
        );
    }
    if contains_terminal_mouse_escape(&prepared.text) {
        warn!(
            utterance_id,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_preview = %short_text(&prepared.text, 160),
            target_process = %target.summary.process_name,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            "prepared paste text resembles a terminal mouse escape sequence"
        );
    }
    if prepared.text.trim().is_empty() {
        return Ok(PasteOutcome {
            text: prepared.text,
            target_context: target.context.clone(),
            target_summary: target.summary.clone(),
            target_fingerprint: target.fingerprint.clone(),
            text_actions: prepared.actions,
        });
    }
    if target.is_wezterm() && any_mouse_button_down() {
        warn!(
            utterance_id,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            modifiers = %input_before_clipboard.modifiers,
            mouse_buttons = %input_before_clipboard.mouse_buttons,
            cursor = %input_before_clipboard.cursor_label(),
            target_process = %target.summary.process_name,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            "terminal paste blocked while mouse button is down"
        );
        return Err(anyhow!(
            "terminal paste blocked while mouse button is down; release the mouse and retry"
        ));
    }
    append_action(
        &mut prepared.actions,
        &format!("clipboard_policy_{}", config.clipboard_policy.as_str()),
    );
    let started_at = Instant::now();
    let mut clipboard_report = match set_clipboard_text_with_retry(&prepared.text, config) {
        Ok(report) => report,
        Err(error) => {
            warn!(
                utterance_id,
                error = %error,
                text_chars = prepared.text.chars().count(),
                text_hash = stable_text_hash(&prepared.text),
                text_preview = %short_text(&prepared.text, 160),
                clipboard_policy = config.clipboard_policy.as_str(),
                clipboard_retry_count = config.clipboard_retry_count,
                clipboard_retry_backoff_ms = config.clipboard_retry_backoff_ms,
                "paste failed because clipboard set failed after bounded retries"
            );
            return Err(error);
        }
    };
    let clipboard_set_ms = clipboard_report.set_elapsed_ms;

    if let Some(copy_only_reason) = direct_output_disabled_reason(config) {
        append_action(&mut prepared.actions, copy_only_reason);
        info!(
            utterance_id,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_preview = %short_text(&prepared.text, 160),
            target_text_actions = %prepared.actions,
            target_right_context = target.context.right.as_str(),
            target_context_source = target.context.source,
            target_focus_class = %target.context.focus_class.as_deref().unwrap_or(""),
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_process_path = %target.process_path,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            modifiers_before_clipboard = %input_before_clipboard.modifiers,
            mouse_buttons_before_clipboard = %input_before_clipboard.mouse_buttons,
            cursor_before_clipboard = %input_before_clipboard.cursor_label(),
            text_has_terminal_mouse_escape = contains_terminal_mouse_escape(&prepared.text),
            clipboard_set_ms,
            clipboard_policy = clipboard_report.policy.as_str(),
            clipboard_set_attempts = clipboard_report.set_attempts,
            clipboard_set_retries = clipboard_report.set_retries,
            clipboard_previous_text_captured = clipboard_report.previous_text_captured,
            clipboard_previous_text_error = %clipboard_report.previous_text_error,
            paste_total_ms = started_at.elapsed().as_millis(),
            direct_paste = false,
            clipboard_retained = clipboard_report.clipboard_retained(),
            output_action = copy_only_reason,
            "paste timing"
        );
        return Ok(PasteOutcome {
            text: prepared.text,
            target_context: target.context.clone(),
            target_summary: target.summary.clone(),
            target_fingerprint: target.fingerprint.clone(),
            text_actions: prepared.actions,
        });
    }

    let modifiers_released = wait_for_paste_modifiers_released();
    let input_before_sendinput = InputStateSnapshot::capture();
    if !modifiers_released {
        append_action(&mut prepared.actions, "copy_only_modifier_still_down");
        warn!(
            utterance_id,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_preview = %short_text(&prepared.text, 160),
            target_text_actions = %prepared.actions,
            target_right_context = target.context.right.as_str(),
            target_context_source = target.context.source,
            target_focus_class = %target.context.focus_class.as_deref().unwrap_or(""),
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_process_path = %target.process_path,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            modifiers_before_clipboard = %input_before_clipboard.modifiers,
            mouse_buttons_before_clipboard = %input_before_clipboard.mouse_buttons,
            cursor_before_clipboard = %input_before_clipboard.cursor_label(),
            modifiers_before_sendinput = %input_before_sendinput.modifiers,
            mouse_buttons_before_sendinput = %input_before_sendinput.mouse_buttons,
            cursor_before_sendinput = %input_before_sendinput.cursor_label(),
            clipboard_set_ms,
            clipboard_policy = clipboard_report.policy.as_str(),
            clipboard_set_attempts = clipboard_report.set_attempts,
            clipboard_set_retries = clipboard_report.set_retries,
            clipboard_retained = clipboard_report.clipboard_retained(),
            output_action = "copy_only_modifier_still_down",
            "paste blocked because modifier key is still down; text retained in clipboard"
        );
        return Ok(PasteOutcome {
            text: prepared.text,
            target_context: target.context.clone(),
            target_summary: target.summary.clone(),
            target_fingerprint: target.fingerprint.clone(),
            text_actions: prepared.actions,
        });
    }
    thread::sleep(Duration::from_millis(config.paste_stabilize_ms));
    let input_after_stabilize = InputStateSnapshot::capture();
    if config.paste_preflight_recheck {
        if let Some(reason) = current_input_preflight_block_reason(
            "modifier_still_down_before_paste",
            "mouse_button_down_before_paste",
        ) {
            append_action(&mut prepared.actions, &format!("copy_only_{reason}"));
            warn!(
                utterance_id,
                reason,
                text_chars = prepared.text.chars().count(),
                text_hash = stable_text_hash(&prepared.text),
                text_preview = %short_text(&prepared.text, 160),
                target_text_actions = %prepared.actions,
                target_hwnd = target.fingerprint.hwnd,
                target_pid = target.fingerprint.process_id,
                target_process = %target.summary.process_name,
                target_process_path = %target.process_path,
                target_class = %target.summary.class_name,
                target_title = %target.summary.title,
                modifiers_after_stabilize = %input_after_stabilize.modifiers,
                mouse_buttons_after_stabilize = %input_after_stabilize.mouse_buttons,
                cursor_after_stabilize = %input_after_stabilize.cursor_label(),
                clipboard_set_ms,
                clipboard_policy = clipboard_report.policy.as_str(),
                clipboard_set_attempts = clipboard_report.set_attempts,
                clipboard_set_retries = clipboard_report.set_retries,
                clipboard_retained = clipboard_report.clipboard_retained(),
                output_action = "copy_only_preflight_blocked",
                "paste skipped after clipboard write because input preflight failed; text retained in clipboard"
            );
            return Ok(PasteOutcome {
                text: prepared.text,
                target_context: target.context.clone(),
                target_summary: target.summary.clone(),
                target_fingerprint: target.fingerprint.clone(),
                text_actions: prepared.actions,
            });
        }
    }
    let paste_result = send_ctrl_v();
    let paste_done_ms = started_at.elapsed().as_millis();
    if paste_result.is_ok() {
        restore_previous_clipboard_text_if_needed(&mut clipboard_report, config);
    }
    if target.is_wezterm() {
        info!(
            utterance_id,
            modifiers_released,
            before_wait_modifiers = %input_before_clipboard.modifiers,
            before_wait_mouse_buttons = %input_before_clipboard.mouse_buttons,
            before_sendinput_modifiers = %input_before_sendinput.modifiers,
            before_sendinput_mouse_buttons = %input_before_sendinput.mouse_buttons,
            after_stabilize_modifiers = %input_after_stabilize.modifiers,
            after_stabilize_mouse_buttons = %input_after_stabilize.mouse_buttons,
            cursor_before = %input_before_clipboard.cursor_label(),
            cursor_before_sendinput = %input_before_sendinput.cursor_label(),
            cursor_after_stabilize = %input_after_stabilize.cursor_label(),
            paste_result_ok = paste_result.is_ok(),
            paste_done_ms,
            clipboard_policy = clipboard_report.policy.as_str(),
            clipboard_set_attempts = clipboard_report.set_attempts,
            clipboard_set_retries = clipboard_report.set_retries,
            clipboard_restore_attempted = clipboard_report.restore_attempted,
            clipboard_restore_ok = clipboard_report.restore_ok,
            "terminal paste forensic snapshot after SendInput"
        );
    }
    match &paste_result {
        Ok(()) => info!(
            utterance_id,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_preview = %short_text(&prepared.text, 160),
            target_text_actions = %prepared.actions,
            target_right_context = target.context.right.as_str(),
            target_context_source = target.context.source,
            target_focus_class = %target.context.focus_class.as_deref().unwrap_or(""),
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_process_path = %target.process_path,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            modifiers_before_clipboard = %input_before_clipboard.modifiers,
            mouse_buttons_before_clipboard = %input_before_clipboard.mouse_buttons,
            cursor_before_clipboard = %input_before_clipboard.cursor_label(),
            modifiers_before_sendinput = %input_before_sendinput.modifiers,
            mouse_buttons_before_sendinput = %input_before_sendinput.mouse_buttons,
            cursor_before_sendinput = %input_before_sendinput.cursor_label(),
            modifiers_after_stabilize = %input_after_stabilize.modifiers,
            mouse_buttons_after_stabilize = %input_after_stabilize.mouse_buttons,
            cursor_after_stabilize = %input_after_stabilize.cursor_label(),
            text_has_terminal_mouse_escape = contains_terminal_mouse_escape(&prepared.text),
            clipboard_set_ms,
            clipboard_policy = clipboard_report.policy.as_str(),
            clipboard_set_attempts = clipboard_report.set_attempts,
            clipboard_set_retries = clipboard_report.set_retries,
            clipboard_previous_text_captured = clipboard_report.previous_text_captured,
            clipboard_previous_text_error = %clipboard_report.previous_text_error,
            clipboard_restore_attempted = clipboard_report.restore_attempted,
            clipboard_restore_ok = clipboard_report.restore_ok,
            clipboard_restore_error = %clipboard_report.restore_error,
            paste_done_ms,
            paste_total_ms = paste_done_ms,
            paste_stabilize_ms = config.paste_stabilize_ms,
            direct_paste = true,
            clipboard_retained = clipboard_report.clipboard_retained(),
            modifiers_released,
            output_action = "pasted",
            "paste timing"
        ),
        Err(error) => warn!(
            utterance_id,
            error = %error,
            text_chars = prepared.text.chars().count(),
            text_hash = stable_text_hash(&prepared.text),
            text_preview = %short_text(&prepared.text, 160),
            target_text_actions = %prepared.actions,
            target_right_context = target.context.right.as_str(),
            target_context_source = target.context.source,
            target_focus_class = %target.context.focus_class.as_deref().unwrap_or(""),
            target_hwnd = target.fingerprint.hwnd,
            target_pid = target.fingerprint.process_id,
            target_process = %target.summary.process_name,
            target_process_path = %target.process_path,
            target_class = %target.summary.class_name,
            target_title = %target.summary.title,
            modifiers_before_clipboard = %input_before_clipboard.modifiers,
            mouse_buttons_before_clipboard = %input_before_clipboard.mouse_buttons,
            cursor_before_clipboard = %input_before_clipboard.cursor_label(),
            modifiers_before_sendinput = %input_before_sendinput.modifiers,
            mouse_buttons_before_sendinput = %input_before_sendinput.mouse_buttons,
            cursor_before_sendinput = %input_before_sendinput.cursor_label(),
            modifiers_after_stabilize = %input_after_stabilize.modifiers,
            mouse_buttons_after_stabilize = %input_after_stabilize.mouse_buttons,
            cursor_after_stabilize = %input_after_stabilize.cursor_label(),
            text_has_terminal_mouse_escape = contains_terminal_mouse_escape(&prepared.text),
            clipboard_set_ms,
            clipboard_policy = clipboard_report.policy.as_str(),
            clipboard_set_attempts = clipboard_report.set_attempts,
            clipboard_set_retries = clipboard_report.set_retries,
            clipboard_previous_text_captured = clipboard_report.previous_text_captured,
            clipboard_previous_text_error = %clipboard_report.previous_text_error,
            clipboard_restore_attempted = clipboard_report.restore_attempted,
            clipboard_restore_ok = clipboard_report.restore_ok,
            clipboard_restore_error = %clipboard_report.restore_error,
            paste_done_ms,
            paste_total_ms = paste_done_ms,
            paste_stabilize_ms = config.paste_stabilize_ms,
            direct_paste = true,
            clipboard_retained = clipboard_report.clipboard_retained(),
            modifiers_released,
            output_action = "paste_failed_clipboard_retained",
            "paste failed; text retained in clipboard"
        ),
    }
    paste_result.map(|()| PasteOutcome {
        text: prepared.text,
        target_context: target.context.clone(),
        target_summary: target.summary.clone(),
        target_fingerprint: target.fingerprint.clone(),
        text_actions: prepared.actions,
    })
}

pub fn replace_recent_paste_with_trace(
    raw_pasted_text: &str,
    replacement: &str,
    original_target: &TargetFingerprint,
    config: &OutputConfig,
    utterance_id: &str,
) -> ReplacementOutcome {
    let raw_char_count = match replacement_candidate_char_count(raw_pasted_text, replacement) {
        Ok(count) => count,
        Err(reason) => {
            log_replacement_skip(utterance_id, reason, raw_pasted_text, replacement, None);
            return ReplacementOutcome::skipped(reason);
        }
    };
    if !config.prefer_direct_paste {
        log_replacement_skip(
            utterance_id,
            "direct_paste_disabled",
            raw_pasted_text,
            replacement,
            None,
        );
        return ReplacementOutcome::skipped("direct_paste_disabled");
    }
    if config.clipboard_policy.is_copy_only() {
        log_replacement_skip(
            utterance_id,
            "copy_only_policy",
            raw_pasted_text,
            replacement,
            None,
        );
        return ReplacementOutcome::skipped("copy_only_policy");
    }

    let current_target = ForegroundWindowSnapshot::capture();
    let current_fingerprint = current_target.fingerprint();
    if !same_window_identity(original_target, &current_fingerprint) {
        log_replacement_skip(
            utterance_id,
            "target_changed",
            raw_pasted_text,
            replacement,
            Some(&current_target),
        );
        return ReplacementOutcome::skipped("target_changed");
    }
    if current_target.is_terminal_target() {
        log_replacement_skip(
            utterance_id,
            "terminal_target",
            raw_pasted_text,
            replacement,
            Some(&current_target),
        );
        return ReplacementOutcome::skipped("terminal_target");
    }
    if any_mouse_button_down() {
        log_replacement_skip(
            utterance_id,
            "mouse_button_down",
            raw_pasted_text,
            replacement,
            Some(&current_target),
        );
        return ReplacementOutcome::skipped("mouse_button_down");
    }
    if !wait_for_paste_modifiers_released() {
        log_replacement_skip(
            utterance_id,
            "modifier_still_down",
            raw_pasted_text,
            replacement,
            Some(&current_target),
        );
        return ReplacementOutcome::skipped("modifier_still_down");
    }

    let context_source = match replacement_left_context_source(&current_target, raw_pasted_text) {
        Ok(source) => source,
        Err(reason) => {
            log_replacement_skip(
                utterance_id,
                reason,
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            return ReplacementOutcome::skipped(reason);
        }
    };

    let started_at = Instant::now();
    if config.replacement_preflight_recheck {
        if let Some(reason) = current_input_preflight_block_reason(
            "modifier_still_down_before_selection",
            "mouse_button_down_before_selection",
        ) {
            log_replacement_skip(
                utterance_id,
                reason,
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            return ReplacementOutcome::skipped(reason);
        }
    }
    let mut clipboard_report = match set_clipboard_text_with_retry(replacement, config) {
        Ok(report) => report,
        Err(error) => {
            warn!(
                utterance_id,
                error = %error,
                clipboard_policy = config.clipboard_policy.as_str(),
                clipboard_retry_count = config.clipboard_retry_count,
                clipboard_retry_backoff_ms = config.clipboard_retry_backoff_ms,
                "async rewrite replacement skipped because clipboard set failed"
            );
            return ReplacementOutcome::skipped("clipboard_set_failed");
        }
    };
    thread::sleep(Duration::from_millis(config.paste_stabilize_ms));
    if config.replacement_preflight_recheck {
        if let Some(reason) = current_input_preflight_block_reason(
            "modifier_still_down_before_selection",
            "mouse_button_down_before_selection",
        ) {
            log_replacement_skip(
                utterance_id,
                reason,
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            return ReplacementOutcome::skipped(reason);
        }
    }
    // Prefer UIA/EM_SETSEL exact selection. Keyboard Shift+Left is only a last resort
    // and must be chunked — bulk SendInput is the root of partial-select duplicates
    // (e.g. leftover "你可" + full rewrite → "你可你可以…") in Claude/Chromium.
    let selection_method = match select_raw_text_for_replacement(
        &current_target,
        raw_pasted_text,
        raw_char_count,
    ) {
        Ok(method) => method,
        Err(reason) => {
            log_replacement_skip(
                utterance_id,
                reason,
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            return ReplacementOutcome::skipped(reason);
        }
    };
    thread::sleep(Duration::from_millis(
        config.paste_stabilize_ms.max(SELECTION_SETTLE_MS),
    ));
    match selection_covers_raw(raw_pasted_text, raw_char_count) {
        SelectionCoverage::Full => {}
        SelectionCoverage::Partial | SelectionCoverage::Empty => {
            // Do not paste into a partial selection — that leaves raw prefix and looks like stutter.
            log_replacement_skip(
                utterance_id,
                "selection_incomplete",
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            warn!(
                utterance_id,
                raw_chars = raw_char_count,
                selection_method,
                "async rewrite replacement skipped because selection did not cover full raw paste"
            );
            return ReplacementOutcome::skipped("selection_incomplete");
        }
        SelectionCoverage::Unknown => {
            // Electron sometimes hides selection from UIA after keyboard select.
            // Only allow unknown when we used exact UIA/edit methods; keyboard alone is too risky.
            if selection_method == "keyboard_shift_left" {
                log_replacement_skip(
                    utterance_id,
                    "selection_unverified",
                    raw_pasted_text,
                    replacement,
                    Some(&current_target),
                );
                return ReplacementOutcome::skipped("selection_unverified");
            }
        }
    }
    if config.replacement_preflight_recheck {
        if let Some(reason) = current_input_preflight_block_reason(
            "modifier_still_down_before_replacement_paste",
            "mouse_button_down_before_replacement_paste",
        ) {
            log_replacement_skip(
                utterance_id,
                reason,
                raw_pasted_text,
                replacement,
                Some(&current_target),
            );
            return ReplacementOutcome::skipped(reason);
        }
    }
    match send_ctrl_v() {
        Ok(()) => {
            restore_previous_clipboard_text_if_needed(&mut clipboard_report, config);
            info!(
                utterance_id,
                raw_chars = raw_char_count,
                raw_hash = stable_text_hash(raw_pasted_text),
                replacement_chars = replacement.chars().count(),
                replacement_hash = stable_text_hash(replacement),
                target_hwnd = current_target.hwnd_value(),
                target_pid = current_target.process_id.unwrap_or_default(),
                target_process = %current_target.process_name.as_deref().unwrap_or("unknown"),
                target_class = %current_target.class_name.as_deref().unwrap_or("unknown"),
                target_title = %current_target.title.as_deref().unwrap_or(""),
                context_source,
                selection_method,
                clipboard_policy = clipboard_report.policy.as_str(),
                clipboard_set_attempts = clipboard_report.set_attempts,
                clipboard_set_retries = clipboard_report.set_retries,
                clipboard_previous_text_captured = clipboard_report.previous_text_captured,
                clipboard_previous_text_error = %clipboard_report.previous_text_error,
                clipboard_restore_attempted = clipboard_report.restore_attempted,
                clipboard_restore_ok = clipboard_report.restore_ok,
                clipboard_restore_error = %clipboard_report.restore_error,
                clipboard_retained = clipboard_report.clipboard_retained(),
                replacement_total_ms = started_at.elapsed().as_millis(),
                "async rewrite replaced recent raw paste"
            );
            ReplacementOutcome::applied(context_source)
        }
        Err(error) => {
            warn!(
                utterance_id,
                error = %error,
                raw_chars = raw_char_count,
                replacement_chars = replacement.chars().count(),
                context_source,
                selection_method,
                "async rewrite replacement paste failed"
            );
            ReplacementOutcome::skipped("replacement_paste_failed")
        }
    }
}

pub fn apply_target_punctuation_rule(text: &str, right: TargetRightContext) -> PasteOutcomeText {
    let mut actions = Vec::new();
    let mut prepared = text.to_string();
    match right {
        TargetRightContext::Empty | TargetRightContext::Unknown => {
            if should_append_period_for_target(&prepared) {
                prepared.push('。');
                actions.push("target_append_period");
            }
        }
        TargetRightContext::NonEmpty => {
            let trimmed = trim_trailing_periods(&prepared);
            if trimmed.len() != prepared.len() {
                prepared.truncate(trimmed.len());
                actions.push("target_strip_period_before_right_text");
            }
        }
    }
    if actions.is_empty() {
        actions.push("none");
    }
    PasteOutcomeText {
        text: prepared,
        actions: actions.join(","),
    }
}

fn classify_rewrite_output_route(
    summary: &TargetSummary,
    context: &TargetInsertionContext,
) -> RewriteOutputRoute {
    if summary.is_terminal_target() {
        return RewriteOutputRoute::HudFirstFinalPaste;
    }
    if is_replace_capable_context_source(context.source)
        || is_known_raw_first_rewrite_process(&summary.process_name)
    {
        RewriteOutputRoute::ReplaceCapable
    } else {
        RewriteOutputRoute::HudFirstFinalPaste
    }
}

fn is_replace_capable_context_source(source: &str) -> bool {
    matches!(
        source,
        "standard_text_control" | "uia_text_pattern" | "uia_text_pattern2"
    )
}

fn is_known_raw_first_rewrite_process(process_name: &str) -> bool {
    matches!(
        process_name.to_ascii_lowercase().as_str(),
        "weixin.exe" | "wechat.exe" | "wechatappex.exe" | "notepad.exe"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteOutcomeText {
    pub text: String,
    pub actions: String,
}

impl TargetRightContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NonEmpty => "non_empty",
            Self::Unknown => "unknown",
        }
    }
}

impl TargetInsertionContext {
    fn unknown(source: &'static str) -> Self {
        Self {
            right: TargetRightContext::Unknown,
            source,
            focus_class: None,
        }
    }

    fn capture(target: &ForegroundWindowSnapshot) -> Self {
        let Some(foreground_hwnd) = target.hwnd else {
            return Self::unknown("no_foreground");
        };
        unsafe {
            let thread_id = GetWindowThreadProcessId(foreground_hwnd, None);
            if thread_id == 0 {
                return Self::unknown("no_foreground_thread");
            }
            let mut gui = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if GetGUIThreadInfo(thread_id, &mut gui).is_err() {
                return Self::unknown("gui_thread_info_failed");
            }
            let focus_hwnd = if gui.hwndFocus.0.is_null() {
                foreground_hwnd
            } else {
                gui.hwndFocus
            };
            let focus_class = Some(window_class_name(focus_hwnd)).filter(|value| !value.is_empty());
            if target.is_wezterm() {
                return Self {
                    right: TargetRightContext::NonEmpty,
                    source: "terminal_safe_no_period_no_cli",
                    focus_class,
                };
            }
            if focus_class
                .as_deref()
                .is_some_and(is_standard_text_control_class)
            {
                if let Some(right) = capture_standard_text_control_right_context(focus_hwnd) {
                    return Self {
                        right,
                        source: "standard_text_control",
                        focus_class,
                    };
                }
            }
            if let Some(mut context) = capture_uia_text_right_context() {
                if context.focus_class.is_none() {
                    context.focus_class = focus_class;
                }
                return context;
            }
            Self {
                right: TargetRightContext::Unknown,
                source: "target_text_unavailable",
                focus_class,
            }
        }
    }
}

fn replacement_candidate_char_count(
    raw_pasted_text: &str,
    replacement: &str,
) -> std::result::Result<usize, &'static str> {
    if raw_pasted_text.trim().is_empty() {
        return Err("raw_text_empty");
    }
    if replacement.trim().is_empty() {
        return Err("replacement_empty");
    }
    if replacement == raw_pasted_text {
        return Err("replacement_same_as_raw");
    }
    let raw_char_count = raw_pasted_text.chars().count();
    if raw_char_count == 0 {
        return Err("raw_text_empty");
    }
    if raw_char_count > MAX_SAFE_REPLACEMENT_CHARS {
        return Err("raw_text_too_long");
    }
    // Defense in depth: never wipe raw paste with a catastrophically short rewrite
    // (WeChat: 18→「我」; short: 「他把我的脏话改掉了。」→「他把」).
    let replacement_char_count = replacement.chars().count();
    if raw_char_count >= 6 {
        if replacement_char_count <= 3 && replacement_char_count < raw_char_count {
            return Err("replacement_too_short");
        }
        if replacement_char_count * 100 < raw_char_count * 50 {
            return Err("replacement_too_short");
        }
    }
    Ok(raw_char_count)
}

fn same_window_identity(original: &TargetFingerprint, current: &TargetFingerprint) -> bool {
    if original.hwnd == 0 || current.hwnd == 0 || original.hwnd != current.hwnd {
        return false;
    }
    if original.process_id != 0
        && current.process_id != 0
        && original.process_id != current.process_id
    {
        return false;
    }
    if !original.process_name.is_empty()
        && !current.process_name.is_empty()
        && !original
            .process_name
            .eq_ignore_ascii_case(&current.process_name)
    {
        return false;
    }
    if !original.class_name.is_empty()
        && !current.class_name.is_empty()
        && original.class_name != current.class_name
    {
        return false;
    }
    true
}

fn replacement_left_context_source(
    target: &ForegroundWindowSnapshot,
    raw_pasted_text: &str,
) -> std::result::Result<&'static str, &'static str> {
    let Some(foreground_hwnd) = target.hwnd else {
        return Err("target_unknown");
    };
    unsafe {
        let thread_id = GetWindowThreadProcessId(foreground_hwnd, None);
        if thread_id == 0 {
            return Err("target_thread_unknown");
        }
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread_id, &mut gui).is_err() {
            return Err("gui_thread_info_failed");
        }
        let focus_hwnd = if gui.hwndFocus.0.is_null() {
            foreground_hwnd
        } else {
            gui.hwndFocus
        };
        let focus_class = window_class_name(focus_hwnd);
        if is_standard_text_control_class(&focus_class) {
            if capture_standard_text_control_left_context(focus_hwnd, raw_pasted_text) {
                return Ok("standard_text_control");
            }
            // One short settle retry — WeChat/Electron UIA sometimes lags after paste.
            thread::sleep(Duration::from_millis(45));
            return capture_standard_text_control_left_context(focus_hwnd, raw_pasted_text)
                .then_some("standard_text_control")
                .ok_or("left_context_mismatch");
        }
    }
    match capture_uia_text_left_context(raw_pasted_text) {
        UiaLeftContext::Matches => Ok("uia_text_pattern"),
        UiaLeftContext::Mismatch => {
            thread::sleep(Duration::from_millis(45));
            match capture_uia_text_left_context(raw_pasted_text) {
                UiaLeftContext::Matches => Ok("uia_text_pattern"),
                UiaLeftContext::Mismatch => Err("left_context_mismatch"),
                UiaLeftContext::SelectionActive => Err("selection_active"),
                UiaLeftContext::Unavailable => Err("left_context_unavailable"),
            }
        }
        UiaLeftContext::SelectionActive => Err("selection_active"),
        UiaLeftContext::Unavailable => Err("left_context_unavailable"),
    }
}

fn capture_standard_text_control_left_context(hwnd: HWND, raw_pasted_text: &str) -> bool {
    let Some((selection_start, selection_end)) = edit_selection(hwnd) else {
        return false;
    };
    if selection_start != selection_end {
        return false;
    }
    let selection_end = selection_end as usize;
    if selection_end > TARGET_TEXT_MAX_U16 {
        return false;
    }
    let Some(text_len) = control_text_len(hwnd) else {
        return false;
    };
    let Some(text) = control_text_u16(hwnd, text_len.min(TARGET_TEXT_MAX_U16)) else {
        return false;
    };
    if selection_end > text.len() {
        return false;
    }
    String::from_utf16_lossy(&text[..selection_end]).ends_with(raw_pasted_text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiaLeftContext {
    Matches,
    Mismatch,
    SelectionActive,
    Unavailable,
}

fn is_standard_text_control_class(class_name: &str) -> bool {
    let lower = class_name.to_ascii_lowercase();
    lower == "edit"
        || lower.contains("richedit")
        || lower.contains("rich_edit")
        || lower == "richeditd2dpt"
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct WezTermPane {
    pane_id: u64,
    cursor_x: usize,
    cursor_y: i32,
    #[serde(default)]
    is_active: bool,
}

#[cfg(test)]
fn parse_wezterm_panes(output: &str) -> Option<Vec<WezTermPane>> {
    let json_start = output.find('[')?;
    serde_json::from_str(&output[json_start..]).ok()
}

#[cfg(test)]
fn terminal_line_right_context(line: &str, cursor_x: usize) -> TargetRightContext {
    let mut column = 0usize;
    for ch in line.chars() {
        let width = terminal_char_width(ch);
        if column >= cursor_x {
            if is_terminal_right_meaningful(ch) {
                return TargetRightContext::NonEmpty;
            }
        } else if column + width > cursor_x && is_terminal_right_meaningful(ch) {
            return TargetRightContext::NonEmpty;
        }
        column += width;
    }
    TargetRightContext::Empty
}

#[cfg(test)]
fn is_terminal_right_meaningful(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(
            ch,
            '│' | '┃'
                | '║'
                | '┆'
                | '┇'
                | '┊'
                | '┋'
                | '┤'
                | '┐'
                | '┘'
                | '┨'
                | '┫'
                | '╢'
                | '╗'
                | '╝'
        )
}

#[cfg(test)]
fn terminal_char_width(ch: char) -> usize {
    if ch.is_control() {
        0
    } else if is_wide_terminal_char(ch) {
        2
    } else {
        1
    }
}

#[cfg(test)]
fn is_wide_terminal_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
    )
}

fn capture_standard_text_control_right_context(hwnd: HWND) -> Option<TargetRightContext> {
    let (selection_start, selection_end) = edit_selection(hwnd)?;
    let selection_end = selection_start.max(selection_end) as usize;
    let text_len = control_text_len(hwnd)?;
    if selection_end >= text_len {
        return Some(TargetRightContext::Empty);
    }
    let text = control_text_u16(hwnd, text_len.min(TARGET_TEXT_MAX_U16))?;
    if selection_end >= text.len() {
        return Some(TargetRightContext::NonEmpty);
    }
    if utf16_has_non_whitespace(&text[selection_end..]) {
        Some(TargetRightContext::NonEmpty)
    } else {
        Some(TargetRightContext::Empty)
    }
}

fn edit_selection(hwnd: HWND) -> Option<(u32, u32)> {
    let mut selection_start = 0u32;
    let mut selection_end = 0u32;
    let mut message_result = 0usize;
    let status = unsafe {
        SendMessageTimeoutW(
            hwnd,
            EM_GETSEL,
            WPARAM((&mut selection_start as *mut u32) as usize),
            LPARAM((&mut selection_end as *mut u32) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            TARGET_TEXT_TIMEOUT_MS,
            Some(&mut message_result),
        )
    };
    if status.0 == 0 {
        return None;
    }
    Some((selection_start, selection_end))
}

fn control_text_len(hwnd: HWND) -> Option<usize> {
    let mut message_result = 0usize;
    let status = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXTLENGTH,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            TARGET_TEXT_TIMEOUT_MS,
            Some(&mut message_result),
        )
    };
    if status.0 == 0 {
        return None;
    }
    Some(message_result)
}

fn control_text_u16(hwnd: HWND, max_u16: usize) -> Option<Vec<u16>> {
    let mut buffer = vec![0u16; max_u16.saturating_add(1).max(1)];
    let mut message_result = 0usize;
    let status = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            TARGET_TEXT_TIMEOUT_MS,
            Some(&mut message_result),
        )
    };
    if status.0 == 0 {
        return None;
    }
    buffer.truncate(message_result.min(buffer.len()));
    Some(buffer)
}

fn capture_uia_text_right_context() -> Option<TargetInsertionContext> {
    ensure_com_initialized();
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        let element = automation.GetFocusedElement().ok()?;
        let focus_class = element
            .CurrentClassName()
            .ok()
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty());

        if let Ok(pattern2) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            let mut is_active = BOOL(0);
            if let Ok(caret) = pattern2.GetCaretRange(&mut is_active) {
                if let Ok(document) = pattern2.DocumentRange() {
                    let right = right_context_from_uia_ranges(&document, &caret)?;
                    return Some(TargetInsertionContext {
                        right,
                        source: "uia_text_pattern2",
                        focus_class,
                    });
                }
            }
        }

        let pattern = element
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok()?;
        let selection = pattern.GetSelection().ok()?;
        if selection.Length().ok()? <= 0 {
            return None;
        }
        let selected_range = selection.GetElement(0).ok()?;
        let document = pattern.DocumentRange().ok()?;
        let right = right_context_from_uia_ranges(&document, &selected_range)?;
        Some(TargetInsertionContext {
            right,
            source: "uia_text_pattern",
            focus_class,
        })
    }
}

fn ensure_com_initialized() {
    COM_INIT_ATTEMPTED.with(|attempted| {
        if !attempted.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            attempted.set(true);
        }
    });
}

fn capture_uia_text_left_context(raw_pasted_text: &str) -> UiaLeftContext {
    ensure_com_initialized();
    unsafe {
        let Ok(automation) = CoCreateInstance::<_, IUIAutomation>(
            &CUIAutomation,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        ) else {
            return UiaLeftContext::Unavailable;
        };
        let Ok(element) = automation.GetFocusedElement() else {
            return UiaLeftContext::Unavailable;
        };
        if let Ok(pattern2) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            let mut is_active = BOOL(0);
            if let Ok(caret) = pattern2.GetCaretRange(&mut is_active) {
                return uia_range_left_context(&caret, raw_pasted_text);
            }
        }
        let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        else {
            return UiaLeftContext::Unavailable;
        };
        let Ok(selection) = pattern.GetSelection() else {
            return UiaLeftContext::Unavailable;
        };
        if selection.Length().ok().unwrap_or_default() <= 0 {
            return UiaLeftContext::Unavailable;
        }
        let Ok(selected_range) = selection.GetElement(0) else {
            return UiaLeftContext::Unavailable;
        };
        uia_range_left_context(&selected_range, raw_pasted_text)
    }
}

fn uia_range_left_context(
    caret_or_selection: &windows::Win32::UI::Accessibility::IUIAutomationTextRange,
    raw_pasted_text: &str,
) -> UiaLeftContext {
    unsafe {
        let Ok(compare) = caret_or_selection.CompareEndpoints(
            TextPatternRangeEndpoint_Start,
            caret_or_selection,
            TextPatternRangeEndpoint_End,
        ) else {
            return UiaLeftContext::Unavailable;
        };
        if compare != 0 {
            return UiaLeftContext::SelectionActive;
        }
        let Ok(left) = caret_or_selection.Clone() else {
            return UiaLeftContext::Unavailable;
        };
        let raw_chars = raw_pasted_text
            .chars()
            .count()
            .min(MAX_SAFE_REPLACEMENT_CHARS);
        if raw_chars == 0 {
            return UiaLeftContext::Unavailable;
        }
        if left
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -(raw_chars as i32),
            )
            .is_err()
        {
            return UiaLeftContext::Unavailable;
        }
        match left.GetText((raw_chars + 8) as i32) {
            Ok(text) if text.to_string().ends_with(raw_pasted_text) => UiaLeftContext::Matches,
            Ok(_) => UiaLeftContext::Mismatch,
            Err(_) => UiaLeftContext::Unavailable,
        }
    }
}

fn right_context_from_uia_ranges(
    document: &windows::Win32::UI::Accessibility::IUIAutomationTextRange,
    caret_or_selection: &windows::Win32::UI::Accessibility::IUIAutomationTextRange,
) -> Option<TargetRightContext> {
    unsafe {
        let right = document.Clone().ok()?;
        right
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                caret_or_selection,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        let text = right.GetText(32).ok()?.to_string();
        if text.chars().any(|ch| !ch.is_whitespace()) {
            Some(TargetRightContext::NonEmpty)
        } else {
            Some(TargetRightContext::Empty)
        }
    }
}

fn utf16_has_non_whitespace(text: &[u16]) -> bool {
    String::from_utf16_lossy(text)
        .chars()
        .any(|ch| !ch.is_whitespace())
}

fn should_append_period_for_target(text: &str) -> bool {
    let Some(last) = text.chars().last() else {
        return false;
    };
    // Emoji endings (e.g. 🤣 from 笑死 replacement) must stay bare — no 。
    if is_emoji_char(last) {
        return false;
    }
    !matches!(
        last,
        '.' | '。'
            | '．'
            | ','
            | '，'
            | '!'
            | '！'
            | '?'
            | '？'
            | ';'
            | '；'
            | ':'
            | '：'
            | '、'
            | ')'
            | '）'
            | ']'
            | '】'
            | '}'
            | '」'
            | '』'
    )
}

fn is_emoji_char(ch: char) -> bool {
    let code = ch as u32;
    // Common emoji ranges: Misc Symbols, Dingbats, Emoticons, Supplemental Symbols.
    (0x1F300..=0x1FAFF).contains(&code)
        || (0x2600..=0x27BF).contains(&code)
        || matches!(
            ch,
            '🤣' | '😂' | '😄' | '😆' | '😅' | '😊' | '😁' | '😉' | '😎' | '👍' | '🙏' | '✨'
        )
}

fn trim_trailing_periods(text: &str) -> &str {
    text.trim_end_matches(|ch| matches!(ch, '.' | '。' | '．'))
}

#[derive(Debug, Default)]
struct ForegroundWindowSnapshot {
    hwnd: Option<HWND>,
    process_id: Option<u32>,
    process_name: Option<String>,
    process_path: Option<String>,
    class_name: Option<String>,
    title: Option<String>,
}

impl ForegroundWindowSnapshot {
    fn capture() -> Self {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return Self::default();
            }
            let mut process_id = 0u32;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            let process_path = process_image_path(process_id);
            let process_name = process_path.as_ref().and_then(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            });
            Self {
                hwnd: Some(hwnd),
                process_id: Some(process_id).filter(|pid| *pid != 0),
                process_name,
                process_path,
                class_name: Some(window_class_name(hwnd)).filter(|value| !value.is_empty()),
                title: Some(window_title(hwnd)).filter(|value| !value.is_empty()),
            }
        }
    }

    fn hwnd_value(&self) -> usize {
        self.hwnd.map(|hwnd| hwnd.0 as usize).unwrap_or_default()
    }

    fn summary(&self) -> TargetSummary {
        TargetSummary {
            process_name: self.process_name.clone().unwrap_or_default(),
            class_name: self.class_name.clone().unwrap_or_default(),
            title: self.title.clone().unwrap_or_default(),
        }
    }

    fn fingerprint(&self) -> TargetFingerprint {
        TargetFingerprint {
            hwnd: self.hwnd_value(),
            process_id: self.process_id.unwrap_or_default(),
            process_name: self.process_name.clone().unwrap_or_default(),
            class_name: self.class_name.clone().unwrap_or_default(),
            title: self.title.clone().unwrap_or_default(),
        }
    }

    fn is_wezterm(&self) -> bool {
        self.process_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("wezterm-gui.exe"))
    }

    fn is_terminal_target(&self) -> bool {
        self.process_name
            .as_deref()
            .is_some_and(is_terminal_process_name)
    }
}

pub fn is_terminal_process_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "wezterm-gui.exe"
            | "windowsterminal.exe"
            | "windowsterminalpreview.exe"
            | "conhost.exe"
            | "alacritty.exe"
            | "mintty.exe"
            | "tabby.exe"
            | "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
    )
}

fn process_image_path(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if result.is_err() || size == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

fn window_class_name(hwnd: HWND) -> String {
    unsafe {
        let mut buffer = vec![0u16; 256];
        let read = GetClassNameW(hwnd, &mut buffer);
        if read <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..read as usize])
    }
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; (len as usize + 1).min(512)];
        let read = GetWindowTextW(hwnd, &mut buffer);
        short_text(&String::from_utf16_lossy(&buffer[..read as usize]), 160)
    }
}

fn wait_for_paste_modifiers_released() -> bool {
    let started = Instant::now();
    while started.elapsed() < MODIFIER_RELEASE_TIMEOUT {
        if !modifier_down(VK_CONTROL)
            && !modifier_down(VK_SHIFT)
            && !modifier_down(VK_ALT)
            && !modifier_down(VK_LWIN)
            && !modifier_down(VK_RWIN)
        {
            return true;
        }
        thread::sleep(MODIFIER_POLL_INTERVAL);
    }
    false
}

fn modifier_down(vk: VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(vk.0 as i32) };
    (state as u16 & 0x8000) != 0
}

fn any_modifier_down() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_ALT, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(modifier_down)
}

fn any_mouse_button_down() -> bool {
    [
        VK_LBUTTON.0,
        VK_RBUTTON.0,
        VK_MBUTTON.0,
        VK_XBUTTON1.0,
        VK_XBUTTON2.0,
    ]
    .into_iter()
    .any(|vk| {
        let state = unsafe { GetAsyncKeyState(vk as i32) };
        (state as u16 & 0x8000) != 0
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionCoverage {
    Full,
    Partial,
    Empty,
    Unknown,
}

/// Select the just-pasted raw text so Ctrl+V can overwrite it with the rewrite.
/// Order: UIA TextRange::Select → standard EM_SETSEL → chunked Shift+Left keyboard.
fn select_raw_text_for_replacement(
    target: &ForegroundWindowSnapshot,
    raw_pasted_text: &str,
    raw_char_count: usize,
) -> std::result::Result<&'static str, &'static str> {
    if raw_char_count == 0 {
        return Err("raw_text_empty");
    }
    if try_select_raw_via_uia(raw_pasted_text, raw_char_count) {
        return Ok("uia_text_select");
    }
    if try_select_raw_via_edit_control(target, raw_pasted_text, raw_char_count) {
        return Ok("standard_edit_setsel");
    }
    send_shift_left_repeated(raw_char_count).map_err(|_| "selection_send_failed")?;
    Ok("keyboard_shift_left")
}

fn try_select_raw_via_uia(raw_pasted_text: &str, raw_char_count: usize) -> bool {
    ensure_com_initialized();
    unsafe {
        let Ok(automation) = CoCreateInstance::<_, IUIAutomation>(
            &CUIAutomation,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        ) else {
            return false;
        };
        let Ok(element) = automation.GetFocusedElement() else {
            return false;
        };
        let pattern: IUIAutomationTextPattern = if let Ok(pattern2) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            // TextPattern2 also implements TextPattern methods via cast path: use caret range then Select.
            let mut is_active = BOOL(0);
            if let Ok(caret) = pattern2.GetCaretRange(&mut is_active) {
                return select_uia_left_range(&caret, raw_pasted_text, raw_char_count);
            }
            match element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                Ok(p) => p,
                Err(_) => return false,
            }
        } else {
            match element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                Ok(p) => p,
                Err(_) => return false,
            }
        };
        let Ok(selection) = pattern.GetSelection() else {
            return false;
        };
        if selection.Length().ok().unwrap_or_default() <= 0 {
            return false;
        }
        let Ok(selected_range) = selection.GetElement(0) else {
            return false;
        };
        select_uia_left_range(&selected_range, raw_pasted_text, raw_char_count)
    }
}

fn select_uia_left_range(
    caret_or_selection: &windows::Win32::UI::Accessibility::IUIAutomationTextRange,
    raw_pasted_text: &str,
    raw_char_count: usize,
) -> bool {
    unsafe {
        let Ok(compare) = caret_or_selection.CompareEndpoints(
            TextPatternRangeEndpoint_Start,
            caret_or_selection,
            TextPatternRangeEndpoint_End,
        ) else {
            return false;
        };
        // Collapse active selection to its end (caret) before expanding left over raw paste.
        let Ok(range) = caret_or_selection.Clone() else {
            return false;
        };
        if compare != 0 {
            let _ = range.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                caret_or_selection,
                TextPatternRangeEndpoint_End,
            );
        }
        if range
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -(raw_char_count as i32),
            )
            .is_err()
        {
            return false;
        }
        let Ok(text) = range.GetText((raw_char_count + 8) as i32) else {
            return false;
        };
        let got = text.to_string();
        // Must match the raw paste exactly (or ends_with with same char length after left expand).
        if got != raw_pasted_text
            && !(got.ends_with(raw_pasted_text) && got.chars().count() == raw_char_count)
        {
            return false;
        }
        range.Select().is_ok()
    }
}

fn try_select_raw_via_edit_control(
    target: &ForegroundWindowSnapshot,
    raw_pasted_text: &str,
    raw_char_count: usize,
) -> bool {
    let Some(foreground_hwnd) = target.hwnd else {
        return false;
    };
    unsafe {
        let thread_id = GetWindowThreadProcessId(foreground_hwnd, None);
        if thread_id == 0 {
            return false;
        }
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread_id, &mut gui).is_err() {
            return false;
        }
        let focus_hwnd = if gui.hwndFocus.0.is_null() {
            foreground_hwnd
        } else {
            gui.hwndFocus
        };
        let focus_class = window_class_name(focus_hwnd);
        if !is_standard_text_control_class(&focus_class) {
            return false;
        }
        let Some((selection_start, selection_end)) = edit_selection(focus_hwnd) else {
            return false;
        };
        if selection_start != selection_end {
            return false;
        }
        let caret = selection_end as usize;
        let Some(text_len) = control_text_len(focus_hwnd) else {
            return false;
        };
        let Some(text) = control_text_u16(focus_hwnd, text_len.min(TARGET_TEXT_MAX_U16)) else {
            return false;
        };
        if caret > text.len() {
            return false;
        }
        let left = String::from_utf16_lossy(&text[..caret]);
        if !left.ends_with(raw_pasted_text) {
            return false;
        }
        // EM_SETSEL uses UTF-16 code units, not Unicode scalar chars.
        let raw_utf16_len = raw_pasted_text.encode_utf16().count();
        if raw_utf16_len == 0 || caret < raw_utf16_len {
            return false;
        }
        let start = (caret - raw_utf16_len) as u32;
        let end = caret as u32;
        let mut message_result = 0usize;
        let status = SendMessageTimeoutW(
            focus_hwnd,
            EM_SETSEL,
            WPARAM(start as usize),
            LPARAM(end as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            TARGET_TEXT_TIMEOUT_MS,
            Some(&mut message_result),
        );
        if status.0 == 0 {
            return false;
        }
        // Confirm selection length.
        if let Some((s, e)) = edit_selection(focus_hwnd) {
            return e.saturating_sub(s) as usize == raw_utf16_len && e as usize == caret;
        }
        let _ = raw_char_count;
        true
    }
}

fn selection_covers_raw(raw_pasted_text: &str, raw_char_count: usize) -> SelectionCoverage {
    if let Some(selected) = read_uia_selected_text() {
        if selected == raw_pasted_text {
            return SelectionCoverage::Full;
        }
        if !selected.is_empty() {
            let selected_chars = selected.chars().count();
            if selected_chars < raw_char_count {
                // Classic partial Shift+Left: only a prefix of the raw paste is selected.
                return SelectionCoverage::Partial;
            }
            if selected_chars == raw_char_count && selected != raw_pasted_text {
                return SelectionCoverage::Partial;
            }
            if selected_chars > raw_char_count {
                // Over-selected; paste would destroy extra user text.
                return SelectionCoverage::Partial;
            }
            return SelectionCoverage::Partial;
        }
        // Empty selection string: caret collapsed or host hides selection. Fall through.
    }

    // If caret is still immediately after the raw paste, selection never took.
    match capture_uia_text_left_context(raw_pasted_text) {
        UiaLeftContext::Matches => SelectionCoverage::Empty,
        UiaLeftContext::SelectionActive => {
            // Selection exists but GetText path failed earlier; refuse blind paste.
            SelectionCoverage::Unknown
        }
        UiaLeftContext::Mismatch => SelectionCoverage::Unknown,
        UiaLeftContext::Unavailable => SelectionCoverage::Unknown,
    }
}

fn read_uia_selected_text() -> Option<String> {
    ensure_com_initialized();
    unsafe {
        let automation = CoCreateInstance::<_, IUIAutomation>(
            &CUIAutomation,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?;
        let element = automation.GetFocusedElement().ok()?;
        let pattern = element
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok()
            .or_else(|| {
                // Some hosts only expose TextPattern2; still try TextPattern id after probe.
                let _ = element
                    .GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
                    .ok()?;
                element
                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                    .ok()
            })?;
        let selection = pattern.GetSelection().ok()?;
        if selection.Length().ok().unwrap_or_default() <= 0 {
            return Some(String::new());
        }
        let range = selection.GetElement(0).ok()?;
        let Ok(compare) = range.CompareEndpoints(
            TextPatternRangeEndpoint_Start,
            &range,
            TextPatternRangeEndpoint_End,
        ) else {
            return None;
        };
        if compare == 0 {
            return Some(String::new());
        }
        let text = range.GetText(-1).ok()?.to_string();
        Some(text)
    }
}

/// Chunked Shift+Left so Chromium/Electron can apply each batch (bulk SendInput drops tail keys).
fn send_shift_left_repeated(count: usize) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    // Hold Shift for the whole selection; release only at the end.
    let shift_down = [key_input(VK_SHIFT, KEYBD_EVENT_FLAGS(0))];
    let sent = unsafe { SendInput(&shift_down, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(anyhow!("SendInput shift-down sent {sent}/1 events"));
    }

    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(SHIFT_LEFT_CHUNK_SIZE);
        let mut inputs = Vec::with_capacity(chunk.saturating_mul(2));
        for _ in 0..chunk {
            inputs.push(key_input(VK_LEFT, KEYBD_EVENT_FLAGS(0)));
            inputs.push(key_input(VK_LEFT, KEYEVENTF_KEYUP));
        }
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent != inputs.len() as u32 {
            let _ = unsafe {
                SendInput(
                    &[key_input(VK_SHIFT, KEYEVENTF_KEYUP)],
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
            return Err(anyhow!(
                "SendInput left chunk sent {sent}/{} events (remaining={remaining})",
                inputs.len()
            ));
        }
        remaining -= chunk;
        if remaining > 0 {
            thread::sleep(SHIFT_LEFT_CHUNK_GAP);
        }
    }

    let shift_up = [key_input(VK_SHIFT, KEYEVENTF_KEYUP)];
    let sent = unsafe { SendInput(&shift_up, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(anyhow!("SendInput shift-up sent {sent}/1 events"));
    }
    Ok(())
}

fn send_ctrl_v() -> Result<()> {
    let inputs = [
        key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(anyhow!("SendInput sent {sent}/{} events", inputs.len()));
    }
    Ok(())
}

fn log_replacement_skip(
    utterance_id: &str,
    reason: &str,
    raw_pasted_text: &str,
    replacement: &str,
    target: Option<&ForegroundWindowSnapshot>,
) {
    warn!(
        utterance_id,
        reason,
        raw_chars = raw_pasted_text.chars().count(),
        raw_hash = stable_text_hash(raw_pasted_text),
        replacement_chars = replacement.chars().count(),
        replacement_hash = stable_text_hash(replacement),
        target_hwnd = target.map(ForegroundWindowSnapshot::hwnd_value).unwrap_or_default(),
        target_pid = target.and_then(|snapshot| snapshot.process_id).unwrap_or_default(),
        target_process = %target
            .and_then(|snapshot| snapshot.process_name.as_deref())
            .unwrap_or("unknown"),
        target_class = %target
            .and_then(|snapshot| snapshot.class_name.as_deref())
            .unwrap_or("unknown"),
        target_title = %target
            .and_then(|snapshot| snapshot.title.as_deref())
            .unwrap_or(""),
        "async rewrite replacement skipped"
    );
}

fn key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_TYPE(INPUT_KEYBOARD.0),
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: Default::default(),
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputStateSnapshot {
    modifiers: String,
    mouse_buttons: String,
    cursor: Option<(i32, i32)>,
}

impl InputStateSnapshot {
    fn capture() -> Self {
        Self {
            modifiers: pressed_key_names(&[
                ("ctrl", VK_CONTROL.0),
                ("shift", VK_SHIFT.0),
                ("alt", VK_ALT.0),
                ("lwin", VK_LWIN.0),
                ("rwin", VK_RWIN.0),
            ]),
            mouse_buttons: pressed_key_names(&[
                ("left", VK_LBUTTON.0),
                ("right", VK_RBUTTON.0),
                ("middle", VK_MBUTTON.0),
                ("x1", VK_XBUTTON1.0),
                ("x2", VK_XBUTTON2.0),
            ]),
            cursor: cursor_position(),
        }
    }

    fn cursor_label(&self) -> String {
        self.cursor
            .map(|(x, y)| format!("{x},{y}"))
            .unwrap_or_else(|| "unknown".to_string())
    }
}

fn pressed_key_names(keys: &[(&'static str, u16)]) -> String {
    let pressed = keys
        .iter()
        .filter_map(|(name, vk)| {
            let state = unsafe { GetAsyncKeyState(*vk as i32) };
            ((state as u16 & 0x8000) != 0).then_some(*name)
        })
        .collect::<Vec<_>>();
    if pressed.is_empty() {
        "none".to_string()
    } else {
        pressed.join("+")
    }
}

fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        Some((point.x, point.y))
    } else {
        None
    }
}

fn contains_terminal_mouse_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(index, pair)| {
        pair == b"[<"
            && bytes[index + 2..]
                .iter()
                .take(24)
                .any(|byte| matches!(*byte, b'M' | b'm'))
    })
}

fn stable_text_hash(text: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn short_text(text: &str, max_chars: usize) -> String {
    let mut value = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        value.push_str("...");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{
        SHIFT_LEFT_CHUNK_SIZE, RewriteOutputRoute, TargetFingerprint, TargetInsertionContext,
        TargetRightContext, apply_target_punctuation_rule, classify_rewrite_output_route,
        contains_terminal_mouse_escape, direct_output_disabled_reason,
        input_preflight_block_reason, is_standard_text_control_class, parse_wezterm_panes,
        replacement_candidate_char_count, retry_with_backoff, same_window_identity, short_text,
        stable_text_hash, terminal_line_right_context,
    };
    use crate::config::{ClipboardPolicy, OutputConfig};
    use std::time::Duration;

    #[test]
    fn text_hash_is_stable_for_same_text() {
        assert_eq!(stable_text_hash("hello"), stable_text_hash("hello"));
        assert_ne!(stable_text_hash("hello"), stable_text_hash("hello!"));
    }

    #[test]
    fn shift_left_chunks_are_small_to_avoid_chromium_drop() {
        // Guard against regressions that re-flood SendInput with dozens of Left keys.
        assert!(SHIFT_LEFT_CHUNK_SIZE <= 8);
        assert!(SHIFT_LEFT_CHUNK_SIZE >= 1);
    }

    #[test]
    fn partial_raw_prefix_stutter_example() {
        // Documented user failure: bulk Shift+Left selected only "你可" then paste rewrite.
        let raw = "你可以去调查一下后台记录";
        let rewrite = "你可以去调查一下后台记录";
        let partial = "你可";
        let stutter = format!("{partial}{rewrite}");
        assert!(stutter.starts_with(partial));
        assert_ne!(stutter, rewrite);
        assert!(raw.starts_with(partial));
        assert!(partial.chars().count() < raw.chars().count());
    }

    #[test]
    fn retry_with_backoff_counts_success_after_failures() {
        let mut calls = 0;
        let (result, stats) = retry_with_backoff(3, Duration::from_millis(0), || {
            calls += 1;
            if calls < 3 { Err("locked") } else { Ok("ok") }
        });
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.retries, 2);
    }

    #[test]
    fn retry_with_backoff_returns_last_failure_when_exhausted() {
        let mut calls = 0;
        let (result, stats) = retry_with_backoff(2, Duration::from_millis(0), || {
            calls += 1;
            Err::<(), _>(calls)
        });
        assert_eq!(result.unwrap_err(), 3);
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.retries, 2);
    }

    #[test]
    fn preflight_reason_prefers_modifier_over_mouse() {
        assert_eq!(
            input_preflight_block_reason(true, true, "modifier", "mouse"),
            Some("modifier")
        );
        assert_eq!(
            input_preflight_block_reason(false, true, "modifier", "mouse"),
            Some("mouse")
        );
        assert_eq!(
            input_preflight_block_reason(false, false, "modifier", "mouse"),
            None
        );
    }

    #[test]
    fn direct_output_disabled_reason_routes_copy_only_before_direct_disabled() {
        let mut config = OutputConfig::default();
        assert_eq!(direct_output_disabled_reason(&config), None);

        config.prefer_direct_paste = false;
        assert_eq!(
            direct_output_disabled_reason(&config),
            Some("direct_paste_disabled")
        );

        config.clipboard_policy = ClipboardPolicy::CopyOnly;
        assert_eq!(
            direct_output_disabled_reason(&config),
            Some("copy_only_policy")
        );
    }

    #[test]
    fn clipboard_retained_reflects_policy_restore_success() {
        let mut report = super::ClipboardWriteReport {
            policy: ClipboardPolicy::RetainTranscript,
            set_attempts: 1,
            set_retries: 0,
            set_elapsed_ms: 0,
            previous_text: None,
            previous_text_captured: false,
            previous_text_error: String::new(),
            restore_attempted: false,
            restore_ok: false,
            restore_error: String::new(),
        };
        assert!(report.clipboard_retained());

        report.policy = ClipboardPolicy::RestoreTextAfterSuccess;
        report.restore_attempted = true;
        report.restore_ok = false;
        assert!(report.clipboard_retained());

        report.restore_ok = true;
        assert!(!report.clipboard_retained());
    }

    #[test]
    fn short_text_limits_chars() {
        assert_eq!(short_text("abcdef", 3), "abc...");
        assert_eq!(short_text("abc", 3), "abc");
    }

    #[test]
    fn detects_terminal_mouse_escape_like_text() {
        assert!(contains_terminal_mouse_escape("[<35;100;32M"));
        assert!(contains_terminal_mouse_escape("\u{1b}[<0;12;8m"));
        assert!(!contains_terminal_mouse_escape("普通文本 [< 不是鼠标事件"));
    }

    #[test]
    fn target_rule_adds_period_only_when_right_side_is_empty() {
        let empty = apply_target_punctuation_rule("我现在测试", TargetRightContext::Empty);
        assert_eq!(empty.text, "我现在测试。");
        assert_eq!(empty.actions, "target_append_period");

        let non_empty = apply_target_punctuation_rule("我现在测试", TargetRightContext::NonEmpty);
        assert_eq!(non_empty.text, "我现在测试");
        assert_eq!(non_empty.actions, "none");
    }

    #[test]
    fn target_rule_does_not_append_period_after_emoji() {
        let laugh = apply_target_punctuation_rule("🤣", TargetRightContext::Empty);
        assert_eq!(laugh.text, "🤣");
        assert_eq!(laugh.actions, "none");

        let with_prefix = apply_target_punctuation_rule("这个梗🤣", TargetRightContext::Empty);
        assert_eq!(with_prefix.text, "这个梗🤣");
        assert_eq!(with_prefix.actions, "none");
    }

    #[test]
    fn target_rule_removes_trailing_period_before_right_side_text_or_symbol() {
        for text in [
            "我现在测试。",
            "我现在测试.",
            "我现在测试．",
            "我现在测试。。",
        ] {
            let prepared = apply_target_punctuation_rule(text, TargetRightContext::NonEmpty);
            assert_eq!(prepared.text, "我现在测试");
            assert_eq!(prepared.actions, "target_strip_period_before_right_text");
        }
        let prepared = apply_target_punctuation_rule("为什么？", TargetRightContext::NonEmpty);
        assert_eq!(prepared.text, "为什么？");
    }

    #[test]
    fn target_rule_preserves_old_behavior_when_context_is_unknown() {
        let prepared = apply_target_punctuation_rule("我现在测试", TargetRightContext::Unknown);
        assert_eq!(prepared.text, "我现在测试。");
        assert_eq!(prepared.actions, "target_append_period");
    }

    #[test]
    fn standard_text_class_gate_rejects_qt_windows() {
        assert!(is_standard_text_control_class("Edit"));
        assert!(is_standard_text_control_class("RichEditD2DPT"));
        assert!(!is_standard_text_control_class("Qt51514QWindowIcon"));
    }

    #[test]
    fn target_summary_detects_terminal_processes() {
        let terminal = super::TargetSummary {
            process_name: "wezterm-gui.exe".to_string(),
            class_name: "org.wezfurlong.wezterm".to_string(),
            title: String::new(),
        };
        assert!(terminal.is_terminal_target());

        let app = super::TargetSummary {
            process_name: "Weixin.exe".to_string(),
            class_name: "Qt51514QWindowIcon".to_string(),
            title: "WeChat".to_string(),
        };
        assert!(!app.is_terminal_target());
    }

    #[test]
    fn tabby_is_treated_as_terminal_for_output_safety() {
        let terminal = super::TargetSummary {
            process_name: "Tabby.exe".to_string(),
            class_name: "Chrome_WidgetWin_1".to_string(),
            title: "vps-us (.ssh/config)".to_string(),
        };

        assert!(terminal.is_terminal_target());
        assert!(super::is_terminal_process_name("Tabby.exe"));
    }

    #[test]
    fn rewrite_output_route_uses_hud_first_for_terminals() {
        let summary = super::TargetSummary {
            process_name: "WindowsTerminal.exe".to_string(),
            class_name: "CASCADIA_HOSTING_WINDOW_CLASS".to_string(),
            title: String::new(),
        };
        let context = TargetInsertionContext {
            right: TargetRightContext::Unknown,
            source: "uia_text_pattern2",
            focus_class: None,
        };
        assert_eq!(
            classify_rewrite_output_route(&summary, &context),
            RewriteOutputRoute::HudFirstFinalPaste
        );
    }

    #[test]
    fn rewrite_output_route_uses_raw_first_for_proven_text_targets() {
        let summary = super::TargetSummary {
            process_name: "app.exe".to_string(),
            class_name: "Edit".to_string(),
            title: String::new(),
        };
        let context = TargetInsertionContext {
            right: TargetRightContext::Empty,
            source: "standard_text_control",
            focus_class: Some("Edit".to_string()),
        };
        assert_eq!(
            classify_rewrite_output_route(&summary, &context),
            RewriteOutputRoute::ReplaceCapable
        );
    }

    #[test]
    fn rewrite_output_route_keeps_wechat_raw_first_but_unknown_apps_fallback() {
        let context = TargetInsertionContext {
            right: TargetRightContext::Unknown,
            source: "target_text_unavailable",
            focus_class: None,
        };
        let wechat = super::TargetSummary {
            process_name: "Weixin.exe".to_string(),
            class_name: "Qt51514QWindowIcon".to_string(),
            title: "WeChat".to_string(),
        };
        assert_eq!(
            classify_rewrite_output_route(&wechat, &context),
            RewriteOutputRoute::ReplaceCapable
        );

        let unknown = super::TargetSummary {
            process_name: "unknown.exe".to_string(),
            class_name: "CustomSurface".to_string(),
            title: String::new(),
        };
        assert_eq!(
            classify_rewrite_output_route(&unknown, &context),
            RewriteOutputRoute::HudFirstFinalPaste
        );
    }

    #[test]
    fn replacement_candidate_gate_rejects_unsafe_text() {
        assert_eq!(
            replacement_candidate_char_count("", "改写").unwrap_err(),
            "raw_text_empty"
        );
        assert_eq!(
            replacement_candidate_char_count("原文", "").unwrap_err(),
            "replacement_empty"
        );
        assert_eq!(
            replacement_candidate_char_count("原文", "原文").unwrap_err(),
            "replacement_same_as_raw"
        );
        assert_eq!(replacement_candidate_char_count("原文", "改写").unwrap(), 2);
        // Catastrophic shrink must never replace a long raw paste (WeChat 我。 incident).
        assert_eq!(
            replacement_candidate_char_count(
                "我楼下拉面店也有用GPT申图的电了。",
                "我。"
            )
            .unwrap_err(),
            "replacement_too_short"
        );
        assert_eq!(
            replacement_candidate_char_count("他把我的脏话改掉了。", "他把。").unwrap_err(),
            "replacement_too_short"
        );
    }

    #[test]
    fn target_identity_ignores_title_drift_but_requires_same_window() {
        let original = TargetFingerprint {
            hwnd: 7,
            process_id: 42,
            process_name: "notepad.exe".to_string(),
            class_name: "Notepad".to_string(),
            title: "Untitled - Notepad".to_string(),
        };
        let mut current = original.clone();
        current.title = "*Untitled - Notepad".to_string();
        assert!(same_window_identity(&original, &current));
        current.hwnd = 8;
        assert!(!same_window_identity(&original, &current));
    }

    #[test]
    fn parses_active_wezterm_pane_json() {
        let panes =
            parse_wezterm_panes(r#"[{"pane_id":7,"cursor_x":12,"cursor_y":23,"is_active":true}]"#)
                .unwrap();
        assert_eq!(panes[0].pane_id, 7);
        assert_eq!(panes[0].cursor_x, 12);
        assert_eq!(panes[0].cursor_y, 23);
        assert!(panes[0].is_active);
    }

    #[test]
    fn terminal_line_context_ignores_padding_and_box_border() {
        assert_eq!(
            terminal_line_right_context("│ hello      │", 7),
            TargetRightContext::Empty
        );
        assert_eq!(
            terminal_line_right_context("│ hello world │", 3),
            TargetRightContext::NonEmpty
        );
        assert_eq!(
            terminal_line_right_context("│ 你好世界 │", 3),
            TargetRightContext::NonEmpty
        );
    }
}
