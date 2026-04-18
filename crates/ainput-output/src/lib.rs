use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::thread;
use std::time::{Duration, Instant};

use ainput_data::{LearningStatus, TermCatalog};
use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextPatternRangeEndpoint_End, UIA_TextPattern2Id, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PWSTR;

const SENTENCE_FINAL_EMOJI_RULES: &[(&str, &str)] = &[
    ("笑死", "[破涕为笑]"),
    ("偷笑", "[偷笑]"),
    ("哭死", "[流泪]"),
    ("震惊", "[震惊]"),
    ("点赞", "[强]"),
    ("抱拳", "[抱拳]"),
    ("狗头", "[狗头]"),
    ("捂脸", "[捂脸]"),
];
const PASTE_STABILIZE_DELAY: Duration = Duration::from_millis(35);
const CHROME_ALT_MENU_DISMISS_DELAY: Duration = Duration::from_millis(30);

#[derive(Debug, Clone, Copy)]
pub enum OutputDelivery {
    DirectPaste,
    ClipboardOnly,
}

#[derive(Debug, Clone)]
pub struct LearnOutcome {
    pub spoken: String,
    pub canonical: String,
    pub count: u32,
    pub status: LearningStatus,
    pub auto_activated: bool,
}

#[derive(Debug, Clone)]
pub struct OutputConfig {
    pub prefer_direct_paste: bool,
    pub fallback_to_clipboard: bool,
    pub voice_hotkey_uses_alt: bool,
}

#[derive(Debug, Clone)]
pub struct OutputContextSnapshot {
    pub process_name: Option<String>,
    pub kind: OutputContextKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputContextKind {
    EditableWithContentOnRight,
    EditableAtEnd,
    Unknown,
}

pub struct OutputController {
    root_dir: PathBuf,
    term_catalog: RwLock<TermCatalog>,
}

impl OutputController {
    pub fn new(root_dir: &Path) -> Result<Self> {
        let term_catalog = TermCatalog::load(root_dir)?;
        Ok(Self {
            root_dir: root_dir.to_path_buf(),
            term_catalog: RwLock::new(term_catalog),
        })
    }

    pub fn builtin_terms_path(&self) -> Result<PathBuf> {
        let catalog = self
            .term_catalog
            .read()
            .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
        Ok(catalog.builtin_terms_path().to_path_buf())
    }

    pub fn user_terms_path(&self) -> Result<PathBuf> {
        let catalog = self
            .term_catalog
            .read()
            .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
        Ok(catalog.user_terms_path().to_path_buf())
    }

    pub fn learning_state_path(&self) -> Result<PathBuf> {
        let catalog = self
            .term_catalog
            .read()
            .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
        Ok(catalog.learning_state_path().to_path_buf())
    }

    pub fn latest_learning_entries(&self, limit: usize) -> Result<Vec<ainput_data::LearningEntry>> {
        let catalog = self
            .term_catalog
            .read()
            .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
        Ok(catalog.latest_learning_entries(limit))
    }

    pub fn deliver_text(&self, text: &str, config: &OutputConfig) -> Result<OutputDelivery> {
        let started_at = Instant::now();
        let correction_started_at = Instant::now();
        let corrected_text = {
            let catalog = self
                .term_catalog
                .read()
                .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
            catalog.apply_to_text(text)
        };
        let correction_elapsed_ms = correction_started_at.elapsed().as_millis();

        let prepare_started_at = Instant::now();
        let (prepared_text, context) = prepare_text_for_delivery(&corrected_text);
        let prepare_elapsed_ms = prepare_started_at.elapsed().as_millis();

        if prepared_text != text {
            tracing::info!(
                original = %text,
                adjusted = %prepared_text,
                context = ?context.kind,
                process_name = context.process_name.as_deref().unwrap_or("unknown"),
                "adjusted output text before delivery"
            );
        }

        if config.prefer_direct_paste {
            let direct_paste_started_at = Instant::now();
            match paste_via_clipboard(&prepared_text, &context, config) {
                Ok(()) => {
                    tracing::info!(
                        correction_elapsed_ms,
                        prepare_elapsed_ms,
                        direct_paste_elapsed_ms = direct_paste_started_at.elapsed().as_millis(),
                        deliver_text_elapsed_ms = started_at.elapsed().as_millis(),
                        context = ?context.kind,
                        process_name = context.process_name.as_deref().unwrap_or("unknown"),
                        "output delivery timing"
                    );
                    return Ok(OutputDelivery::DirectPaste);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "direct paste failed");
                    if !config.fallback_to_clipboard {
                        return Err(error);
                    }
                }
            }
        }

        if !config.fallback_to_clipboard {
            return Err(anyhow!("clipboard fallback disabled"));
        }

        let clipboard_started_at = Instant::now();
        copy_to_clipboard(&prepared_text)?;
        tracing::info!(
            correction_elapsed_ms,
            prepare_elapsed_ms,
            clipboard_only_elapsed_ms = clipboard_started_at.elapsed().as_millis(),
            deliver_text_elapsed_ms = started_at.elapsed().as_millis(),
            context = ?context.kind,
            process_name = context.process_name.as_deref().unwrap_or("unknown"),
            "output delivery timing"
        );
        Ok(OutputDelivery::ClipboardOnly)
    }

    pub fn prepare_streaming_text(&self, text: &str) -> Result<String> {
        self.apply_term_corrections(text)
    }

    pub fn paste_text_verbatim(&self, text: &str, config: &OutputConfig) -> Result<()> {
        let context = inspect_output_context().unwrap_or_else(|error| {
            tracing::warn!(error = %error, "failed to inspect caret context for streaming paste");
            OutputContextSnapshot {
                process_name: None,
                kind: OutputContextKind::Unknown,
            }
        });

        paste_via_clipboard(text, &context, config)
    }

    pub fn learn_from_recent_correction(
        &self,
        original_text: &str,
        corrected_text: &str,
        auto_activate_threshold: u32,
    ) -> Result<Option<LearnOutcome>> {
        let mut catalog = self
            .term_catalog
            .write()
            .map_err(|_| anyhow!("term catalog write lock poisoned"))?;
        let outcome = catalog.record_recent_correction(
            original_text,
            corrected_text,
            Some(auto_activate_threshold),
        )?;

        Ok(outcome.map(|outcome| LearnOutcome {
            spoken: outcome.spoken,
            canonical: outcome.canonical,
            count: outcome.count,
            status: outcome.status,
            auto_activated: outcome.auto_activated,
        }))
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn apply_term_corrections(&self, text: &str) -> Result<String> {
        let catalog = self
            .term_catalog
            .read()
            .map_err(|_| anyhow!("term catalog read lock poisoned"))?;
        Ok(catalog.apply_to_text(text))
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("open clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("write text into clipboard")?;
    Ok(())
}

fn paste_via_clipboard(
    text: &str,
    context: &OutputContextSnapshot,
    config: &OutputConfig,
) -> Result<()> {
    let clipboard_started_at = Instant::now();
    copy_to_clipboard(text)?;
    let clipboard_elapsed_ms = clipboard_started_at.elapsed().as_millis();
    // Give the foreground app one short frame to settle after the hotkey is released.
    thread::sleep(PASTE_STABILIZE_DELAY);

    let controller_started_at = Instant::now();
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| anyhow!("create enigo output controller: {error}"))?;
    let controller_elapsed_ms = controller_started_at.elapsed().as_millis();
    let clear_menu_focus_started_at = Instant::now();
    if should_clear_alt_menu_focus(context, config) {
        enigo
            .key(Key::Escape, Click)
            .context("dismiss chrome alt menu focus")?;
        thread::sleep(CHROME_ALT_MENU_DISMISS_DELAY);
    }
    let clear_menu_focus_elapsed_ms = clear_menu_focus_started_at.elapsed().as_millis();
    let key_send_started_at = Instant::now();
    enigo.key(Key::Control, Press).context("press ctrl")?;
    enigo.key(Key::V, Click).context("send v key")?;
    enigo.key(Key::Control, Release).context("release ctrl")?;
    let key_send_elapsed_ms = key_send_started_at.elapsed().as_millis();
    tracing::info!(
        clipboard_elapsed_ms,
        controller_elapsed_ms,
        clear_menu_focus_elapsed_ms,
        key_send_elapsed_ms,
        paste_via_clipboard_elapsed_ms = clipboard_started_at.elapsed().as_millis(),
        "paste timing"
    );

    Ok(())
}

fn should_clear_alt_menu_focus(context: &OutputContextSnapshot, config: &OutputConfig) -> bool {
    config.voice_hotkey_uses_alt
        && context
            .process_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("chrome.exe"))
}

fn prepare_text_for_delivery(text: &str) -> (String, OutputContextSnapshot) {
    if text.is_empty() {
        return (
            String::new(),
            OutputContextSnapshot {
                process_name: None,
                kind: OutputContextKind::Unknown,
            },
        );
    }

    match inspect_output_context() {
        Ok(snapshot) => (
            prepare_text_for_delivery_in_context(text, snapshot.kind),
            snapshot,
        ),
        Err(error) => {
            tracing::warn!(error = %error, "failed to inspect caret context");
            (
                text.to_string(),
                OutputContextSnapshot {
                    process_name: None,
                    kind: OutputContextKind::Unknown,
                },
            )
        }
    }
}

fn prepare_text_for_delivery_in_context(text: &str, context: OutputContextKind) -> String {
    let rewritten = apply_voice_actions(text, context);

    match context {
        OutputContextKind::EditableWithContentOnRight => strip_trailing_period(&rewritten),
        OutputContextKind::EditableAtEnd => ensure_trailing_period(&rewritten),
        OutputContextKind::Unknown => ensure_trailing_period(&rewritten),
    }
}

fn apply_voice_actions(text: &str, context: OutputContextKind) -> String {
    match context {
        OutputContextKind::EditableAtEnd => replace_sentence_final_emoji_trigger(text),
        OutputContextKind::EditableWithContentOnRight | OutputContextKind::Unknown => {
            text.to_string()
        }
    }
}

fn replace_sentence_final_emoji_trigger(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return text.to_string();
    }

    let without_terminal_punctuation = trimmed.trim_end_matches(is_terminal_punctuation_char);
    for (trigger, emoji) in SENTENCE_FINAL_EMOJI_RULES {
        if without_terminal_punctuation.ends_with(trigger) {
            let prefix =
                &without_terminal_punctuation[..without_terminal_punctuation.len() - trigger.len()];
            return format!("{prefix}{emoji}");
        }
    }

    text.to_string()
}

fn strip_trailing_period(text: &str) -> String {
    text.strip_suffix('。')
        .or_else(|| text.strip_suffix('.'))
        .unwrap_or(text)
        .to_string()
}

fn ensure_trailing_period(text: &str) -> String {
    if has_terminal_punctuation(text) {
        text.to_string()
    } else {
        format!("{text}。")
    }
}

fn has_terminal_punctuation(text: &str) -> bool {
    matches!(text.chars().last(), Some(ch) if is_terminal_punctuation_char(ch))
        || is_emoji_token(text)
}

fn is_terminal_punctuation_char(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '!' | '?' | '.')
}

fn is_emoji_token(text: &str) -> bool {
    SENTENCE_FINAL_EMOJI_RULES
        .iter()
        .any(|(_, emoji)| text.ends_with(emoji))
}

fn inspect_output_context() -> Result<OutputContextSnapshot> {
    let process_name = foreground_process_name()?;

    match focused_has_content_on_right() {
        Ok(Some(true)) => Ok(OutputContextSnapshot {
            process_name,
            kind: OutputContextKind::EditableWithContentOnRight,
        }),
        Ok(Some(false)) => Ok(OutputContextSnapshot {
            process_name,
            kind: OutputContextKind::EditableAtEnd,
        }),
        Ok(None) => Ok(OutputContextSnapshot {
            process_name,
            kind: OutputContextKind::Unknown,
        }),
        Err(error) => Err(error),
    }
}

fn foreground_process_name() -> Result<Option<String>> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0.is_null() {
            return Ok(None);
        }

        let mut process_id = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id == 0 {
            return Ok(None);
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)?;
        let mut buffer = vec![0u16; MAX_PATH as usize];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        if result.is_err() {
            return Ok(None);
        }

        let full_path = String::from_utf16_lossy(&buffer[..len as usize]);
        Ok(Path::new(&full_path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string()))
    }
}

fn focused_has_content_on_right() -> Result<Option<bool>> {
    let _com = ComApartment::initialize()?;

    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                .context("create UI Automation client")?;
        let focused = automation
            .GetFocusedElement()
            .context("get focused UI Automation element")?;

        if let Ok(text_pattern2) =
            focused.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            let mut is_active = 0i32;
            let caret_range = text_pattern2
                .GetCaretRange((&mut is_active as *mut i32).cast())
                .context("get text caret range")?;

            if is_active == 0 {
                return Ok(None);
            }

            let document_range = text_pattern2
                .DocumentRange()
                .context("get text document range")?;
            return compare_range_end_with_document_end(&caret_range, &document_range).map(Some);
        }

        if let Ok(text_pattern) =
            focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            let selections = text_pattern
                .GetSelection()
                .context("get text selection range")?;
            if selections.Length().context("get text selection length")? <= 0 {
                return Ok(None);
            }

            let selection_range = selections
                .GetElement(0)
                .context("get first text selection range")?;
            let document_range = text_pattern
                .DocumentRange()
                .context("get text document range")?;
            return compare_range_end_with_document_end(&selection_range, &document_range)
                .map(Some);
        }
    }

    Ok(None)
}

fn compare_range_end_with_document_end(
    current_range: &IUIAutomationTextRange,
    document_range: &IUIAutomationTextRange,
) -> Result<bool> {
    let comparison = unsafe {
        current_range
            .CompareEndpoints(
                TextPatternRangeEndpoint_End,
                document_range,
                TextPatternRangeEndpoint_End,
            )
            .context("compare caret end with document end")?
    };

    Ok(comparison < 0)
}

struct ComApartment {
    should_uninitialize: bool,
}

impl ComApartment {
    fn initialize() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

        if hr.is_ok() {
            return Ok(Self {
                should_uninitialize: true,
            });
        }

        if hr == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            return Ok(Self {
                should_uninitialize: false,
            });
        }

        Err(anyhow!("initialize COM apartment: {hr:?}"))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputConfig, OutputContextKind, OutputContextSnapshot, apply_voice_actions,
        ensure_trailing_period, has_terminal_punctuation, prepare_text_for_delivery_in_context,
        replace_sentence_final_emoji_trigger, should_clear_alt_menu_focus, strip_trailing_period,
    };

    #[test]
    fn strips_trailing_chinese_and_english_period() {
        assert_eq!(strip_trailing_period("你好。"), "你好");
        assert_eq!(strip_trailing_period("hello."), "hello");
        assert_eq!(strip_trailing_period("你好！"), "你好！");
    }

    #[test]
    fn ensures_period_only_when_missing_terminal_mark() {
        assert_eq!(ensure_trailing_period("你好"), "你好。");
        assert_eq!(ensure_trailing_period("你好。"), "你好。");
        assert_eq!(ensure_trailing_period("你好！"), "你好！");
    }

    #[test]
    fn detects_terminal_sentence_punctuation() {
        assert!(has_terminal_punctuation("你好。"));
        assert!(has_terminal_punctuation("hello?"));
        assert!(!has_terminal_punctuation("hello"));
    }

    #[test]
    fn replaces_sentence_final_emoji_trigger_with_matching_token() {
        assert_eq!(
            replace_sentence_final_emoji_trigger("这个功能太离谱了笑死"),
            "这个功能太离谱了[破涕为笑]"
        );
        assert_eq!(
            replace_sentence_final_emoji_trigger("别这样偷笑。"),
            "别这样[偷笑]"
        );
        assert_eq!(
            replace_sentence_final_emoji_trigger("我直接震惊！"),
            "我直接[震惊]"
        );
        assert_eq!(
            replace_sentence_final_emoji_trigger("先这样抱拳。"),
            "先这样[抱拳]"
        );
        assert_eq!(
            replace_sentence_final_emoji_trigger("这个功能太离谱了笑死。"),
            "这个功能太离谱了[破涕为笑]"
        );
    }

    #[test]
    fn keeps_mid_sentence_emoji_trigger_unchanged() {
        assert_eq!(
            replace_sentence_final_emoji_trigger("我都快笑死了但是还没说完"),
            "我都快笑死了但是还没说完"
        );
        assert_eq!(
            replace_sentence_final_emoji_trigger("我给你点个赞然后继续说"),
            "我给你点个赞然后继续说"
        );
    }

    #[test]
    fn only_applies_voice_actions_at_editable_end() {
        assert_eq!(
            apply_voice_actions("这个功能太离谱了笑死", OutputContextKind::EditableAtEnd),
            "这个功能太离谱了[破涕为笑]"
        );
        assert_eq!(
            apply_voice_actions(
                "这个功能太离谱了笑死",
                OutputContextKind::EditableWithContentOnRight
            ),
            "这个功能太离谱了笑死"
        );
        assert_eq!(
            apply_voice_actions("这个功能太离谱了狗头", OutputContextKind::EditableAtEnd),
            "这个功能太离谱了[狗头]"
        );
    }

    #[test]
    fn does_not_append_period_after_emoji_token() {
        assert_eq!(
            ensure_trailing_period("这个功能太离谱了[破涕为笑]"),
            "这个功能太离谱了[破涕为笑]"
        );
        assert_eq!(ensure_trailing_period("收到[抱拳]"), "收到[抱拳]");
        assert_eq!(ensure_trailing_period("懂了[狗头]"), "懂了[狗头]");
    }

    #[test]
    fn prepares_text_with_emoji_rule_before_period_logic() {
        assert_eq!(
            prepare_text_for_delivery_in_context(
                "这个功能太离谱了笑死",
                OutputContextKind::EditableAtEnd
            ),
            "这个功能太离谱了[破涕为笑]"
        );
        assert_eq!(
            prepare_text_for_delivery_in_context(
                "这个功能太离谱了笑死",
                OutputContextKind::Unknown
            ),
            "这个功能太离谱了笑死。"
        );
        assert_eq!(
            prepare_text_for_delivery_in_context("普通一句话", OutputContextKind::Unknown),
            "普通一句话。"
        );
    }

    #[test]
    fn clears_alt_menu_focus_only_for_chrome_with_alt_voice_hotkey() {
        let chrome_context = OutputContextSnapshot {
            process_name: Some("chrome.exe".to_string()),
            kind: OutputContextKind::EditableAtEnd,
        };
        let edge_context = OutputContextSnapshot {
            process_name: Some("msedge.exe".to_string()),
            kind: OutputContextKind::EditableAtEnd,
        };
        let alt_config = OutputConfig {
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
            voice_hotkey_uses_alt: true,
        };
        let ctrl_config = OutputConfig {
            prefer_direct_paste: true,
            fallback_to_clipboard: true,
            voice_hotkey_uses_alt: false,
        };

        assert!(should_clear_alt_menu_focus(&chrome_context, &alt_config));
        assert!(!should_clear_alt_menu_focus(&chrome_context, &ctrl_config));
        assert!(!should_clear_alt_menu_focus(&edge_context, &alt_config));
    }
}
