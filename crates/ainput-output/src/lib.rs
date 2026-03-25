use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextPatternRangeEndpoint_End, UIA_TextPattern2Id, UIA_TextPatternId,
};

const AUTO_ACTIVATE_THRESHOLD: u32 = 2;
const AUTO_SECTION_MARKER: &str = "# --- auto learned mappings ---";

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
    pub activated: bool,
}

pub fn deliver_text(
    text: &str,
    prefer_direct_paste: bool,
    root_dir: &Path,
) -> Result<OutputDelivery> {
    let started_at = Instant::now();
    let correction_started_at = Instant::now();
    let corrected_text = apply_user_term_corrections(text, root_dir);
    let correction_elapsed_ms = correction_started_at.elapsed().as_millis();
    let prepare_started_at = Instant::now();
    let prepared_text = prepare_text_for_delivery(&corrected_text);
    let prepare_elapsed_ms = prepare_started_at.elapsed().as_millis();

    if prepared_text != text {
        tracing::info!(original = %text, adjusted = %prepared_text, "adjusted output text before delivery");
    }

    if prefer_direct_paste {
        let direct_paste_started_at = Instant::now();
        match paste_via_clipboard(&prepared_text) {
            Ok(()) => {
                tracing::info!(
                    correction_elapsed_ms,
                    prepare_elapsed_ms,
                    direct_paste_elapsed_ms = direct_paste_started_at.elapsed().as_millis(),
                    deliver_text_elapsed_ms = started_at.elapsed().as_millis(),
                    "output delivery timing"
                );
                return Ok(OutputDelivery::DirectPaste);
            }
            Err(error) => {
                tracing::warn!(error = %error, "direct paste failed, fallback to clipboard");
            }
        }
    }

    let clipboard_started_at = Instant::now();
    copy_to_clipboard(&prepared_text)?;
    tracing::info!(
        correction_elapsed_ms,
        prepare_elapsed_ms,
        clipboard_only_elapsed_ms = clipboard_started_at.elapsed().as_millis(),
        deliver_text_elapsed_ms = started_at.elapsed().as_millis(),
        "output delivery timing"
    );
    Ok(OutputDelivery::ClipboardOnly)
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("open clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("write text into clipboard")?;
    Ok(())
}

pub fn ensure_user_terms_document(root_dir: &Path) -> Result<PathBuf> {
    let path = user_terms_path(root_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create user terms directory {}", parent.display()))?;
    }

    if !path.exists() {
        let document = if legacy_user_terms_json_path(root_dir).exists() {
            migrate_legacy_json_terms(&legacy_user_terms_json_path(root_dir))?
        } else {
            UserTermsDocument::default()
        };
        save_user_terms_document(&path, &document)?;
    }

    Ok(path)
}

pub fn learn_from_recent_correction(
    root_dir: &Path,
    logs_dir: &Path,
) -> Result<Option<LearnOutcome>> {
    let user_terms_path = ensure_user_terms_document(root_dir)?;
    let original_text = fs::read_to_string(logs_dir.join("last_result.txt"))
        .context("read last_result.txt for learning")?;

    let corrected_text = {
        let mut clipboard = Clipboard::new().context("open clipboard")?;
        clipboard
            .get_text()
            .context("read corrected text from clipboard")?
    };

    let Some((spoken, canonical)) = infer_single_word_correction(&original_text, &corrected_text)
    else {
        return Ok(None);
    };

    let mut document = load_user_terms_document(&user_terms_path)?;
    let (count, activated) = {
        let mapping = document.record_mapping(&spoken, &canonical);
        (mapping.count, mapping.count >= AUTO_ACTIVATE_THRESHOLD)
    };
    save_user_terms_document(&user_terms_path, &document)?;

    Ok(Some(LearnOutcome {
        spoken,
        canonical,
        count,
        activated,
    }))
}

fn paste_via_clipboard(text: &str) -> Result<()> {
    let clipboard_started_at = Instant::now();
    copy_to_clipboard(text)?;
    let clipboard_elapsed_ms = clipboard_started_at.elapsed().as_millis();

    let controller_started_at = Instant::now();
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| anyhow!("create enigo output controller: {error}"))?;
    let controller_elapsed_ms = controller_started_at.elapsed().as_millis();
    let key_send_started_at = Instant::now();
    enigo.key(Key::Control, Press).context("press ctrl")?;
    enigo.key(Key::V, Click).context("send v key")?;
    enigo.key(Key::Control, Release).context("release ctrl")?;
    let key_send_elapsed_ms = key_send_started_at.elapsed().as_millis();
    tracing::info!(
        clipboard_elapsed_ms,
        controller_elapsed_ms,
        key_send_elapsed_ms,
        settle_elapsed_ms = 0,
        paste_via_clipboard_elapsed_ms = clipboard_started_at.elapsed().as_millis(),
        "paste timing"
    );

    Ok(())
}

fn prepare_text_for_delivery(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    match focused_has_content_on_right() {
        Ok(Some(true)) => strip_trailing_period(text),
        Ok(Some(false)) => ensure_trailing_period(text),
        Ok(None) => text.to_string(),
        Err(error) => {
            tracing::warn!(error = %error, "failed to inspect caret context, keep original punctuation");
            text.to_string()
        }
    }
}

fn apply_user_term_corrections(text: &str, root_dir: &Path) -> String {
    let path = match ensure_user_terms_document(root_dir) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(error = %error, "failed to prepare user terms document");
            return text.to_string();
        }
    };

    let document = match load_user_terms_document(&path) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!(error = %error, "failed to load user terms document");
            return text.to_string();
        }
    };

    correct_ascii_terms(text, &document)
}

fn correct_ascii_terms(text: &str, document: &UserTermsDocument) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if is_term_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_term_char(chars[index]) {
                index += 1;
            }

            let token: String = chars[start..index].iter().collect();
            let replacement = correct_ascii_token(&token, document).unwrap_or(token);
            result.push_str(&replacement);
        } else {
            result.push(chars[index]);
            index += 1;
        }
    }

    result
}

fn correct_ascii_token(token: &str, document: &UserTermsDocument) -> Option<String> {
    if !token.is_ascii() || token.chars().count() < 4 {
        return None;
    }

    let lowered = token.to_ascii_lowercase();

    for mapping in &document.mappings {
        if mapping.count < AUTO_ACTIVATE_THRESHOLD {
            continue;
        }

        if mapping
            .spoken
            .iter()
            .any(|spoken| spoken.eq_ignore_ascii_case(&lowered))
        {
            return Some(mapping.canonical.clone());
        }
    }

    for canonical in &document.glossary {
        if canonical.eq_ignore_ascii_case(token) {
            return Some(canonical.clone());
        }
    }

    let mut best: Option<(&str, usize)> = None;
    for canonical in &document.glossary {
        if !canonical.is_ascii() || canonical.chars().count() < 4 {
            continue;
        }

        let distance = levenshtein(&lowered, &canonical.to_ascii_lowercase());
        if distance > 2 {
            continue;
        }

        match best {
            Some((_, best_distance)) if distance >= best_distance => {}
            _ => best = Some((canonical.as_str(), distance)),
        }
    }

    best.map(|(canonical, _)| canonical.to_string())
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
    matches!(
        text.chars().last(),
        Some('。' | '！' | '？' | '!' | '?' | '.')
    )
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

fn infer_single_word_correction(
    original_text: &str,
    corrected_text: &str,
) -> Option<(String, String)> {
    let original_tokens = extract_ascii_word_tokens(original_text);
    let corrected_tokens = extract_ascii_word_tokens(corrected_text);

    if original_tokens.len() != corrected_tokens.len() {
        return None;
    }

    let mut diff: Option<(String, String)> = None;
    for (original, corrected) in original_tokens.iter().zip(corrected_tokens.iter()) {
        if original.eq_ignore_ascii_case(corrected) {
            continue;
        }

        if diff.is_some() {
            return None;
        }

        diff = Some((original.to_ascii_lowercase(), corrected.clone()));
    }

    diff.filter(|(spoken, canonical)| spoken != &canonical.to_ascii_lowercase())
}

fn extract_ascii_word_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if is_term_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_term_char(chars[index]) {
                index += 1;
            }

            let token: String = chars[start..index].iter().collect();
            if token.is_ascii() && token.chars().count() >= 2 {
                tokens.push(token);
            }
        } else {
            index += 1;
        }
    }

    tokens
}

fn is_term_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+')
}

fn user_terms_path(root_dir: &Path) -> PathBuf {
    root_dir.join("data").join("terms").join("user_terms.txt")
}

fn load_user_terms_document(path: &Path) -> Result<UserTermsDocument> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read user terms file {}", path.display()))?;

    Ok(parse_text_terms_document(&raw))
}

fn save_user_terms_document(path: &Path, document: &UserTermsDocument) -> Result<()> {
    let payload = render_text_terms_document(document);
    fs::write(path, payload).with_context(|| format!("write user terms file {}", path.display()))
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();

    let mut prev: Vec<usize> = (0..=right_chars.len()).collect();
    let mut curr = vec![0usize; right_chars.len() + 1];

    for (i, left_char) in left_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = if left_char == right_char { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + substitution_cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[right_chars.len()]
}

struct UserTermsDocument {
    glossary: Vec<String>,
    mappings: Vec<UserTermMapping>,
}

impl Default for UserTermsDocument {
    fn default() -> Self {
        Self {
            glossary: vec![
                "skill".to_string(),
                "emoji".to_string(),
                "OpenAI".to_string(),
                "Codex".to_string(),
            ],
            mappings: Vec::new(),
        }
    }
}

fn legacy_user_terms_json_path(root_dir: &Path) -> PathBuf {
    root_dir.join("data").join("terms").join("user_terms.json")
}

fn migrate_legacy_json_terms(path: &Path) -> Result<UserTermsDocument> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read legacy user terms file {}", path.display()))?;
    let mut glossary = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim().trim_end_matches(',');
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            let entry = trimmed.trim_matches('"');
            if !entry.is_empty()
                && entry != "version"
                && entry != "notes"
                && entry != "glossary"
                && entry != "mappings"
            {
                glossary.push(entry.to_string());
            }
        }
    }

    glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
    glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    if glossary.is_empty() {
        return Ok(UserTermsDocument::default());
    }

    Ok(UserTermsDocument {
        glossary,
        mappings: Vec::new(),
    })
}

impl UserTermsDocument {
    fn record_mapping(&mut self, spoken: &str, canonical: &str) -> &UserTermMapping {
        if !self
            .glossary
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(canonical))
        {
            self.glossary.push(canonical.to_string());
            self.glossary
                .sort_by_key(|entry| entry.to_ascii_lowercase());
            self.glossary
                .dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        }

        if let Some(index) = self
            .mappings
            .iter()
            .position(|entry| entry.canonical.eq_ignore_ascii_case(canonical))
        {
            let mapping = &mut self.mappings[index];
            if !mapping
                .spoken
                .iter()
                .any(|entry| entry.eq_ignore_ascii_case(spoken))
            {
                mapping.spoken.push(spoken.to_ascii_lowercase());
            }
            mapping.count += 1;
            mapping.spoken.sort();
            mapping.spoken.dedup();
            return mapping;
        }

        self.mappings.push(UserTermMapping {
            spoken: vec![spoken.to_ascii_lowercase()],
            canonical: canonical.to_string(),
            count: 1,
        });
        self.mappings
            .last()
            .expect("newly pushed mapping should exist")
    }
}

#[derive(Debug, Clone)]
struct UserTermMapping {
    spoken: Vec<String>,
    canonical: String,
    count: u32,
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

        if hr == RPC_E_CHANGED_MODE {
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

fn parse_text_terms_document(raw: &str) -> UserTermsDocument {
    let mut glossary = Vec::new();
    let mut mappings = Vec::new();
    let mut in_auto_section = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == AUTO_SECTION_MARKER {
            in_auto_section = true;
            continue;
        }

        if trimmed.starts_with('#') {
            continue;
        }

        if in_auto_section {
            if let Some(mapping) = parse_mapping_line(trimmed) {
                mappings.push(mapping);
            }
        } else {
            glossary.push(trimmed.to_string());
        }
    }

    glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
    glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    UserTermsDocument { glossary, mappings }
}

fn parse_mapping_line(line: &str) -> Option<UserTermMapping> {
    let (left, rest) = line.split_once("=>")?;
    let spoken = left.trim().to_ascii_lowercase();
    let (canonical, count) = if let Some((canonical, suffix)) = rest.split_once("|") {
        let canonical = canonical.trim().to_string();
        let count = suffix
            .trim()
            .strip_prefix("count=")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        (canonical, count)
    } else {
        (rest.trim().to_string(), 1)
    };

    if spoken.is_empty() || canonical.is_empty() {
        return None;
    }

    Some(UserTermMapping {
        spoken: vec![spoken],
        canonical,
        count,
    })
}

fn render_text_terms_document(document: &UserTermsDocument) -> String {
    let mut lines = vec![
        "# ainput 用户术语词表".to_string(),
        "# 下面直接一行写一个正确词；不要写 JSON，不要加引号".to_string(),
        "# 例如：skill  emoji  OpenAI  Codex".to_string(),
        String::new(),
    ];

    for entry in &document.glossary {
        lines.push(entry.clone());
    }

    lines.push(String::new());
    lines.push(AUTO_SECTION_MARKER.to_string());
    lines.push("# 以下由程序自动维护；你不用手改".to_string());

    if document.mappings.is_empty() {
        lines.push("# 例子：scale => skill | count=2".to_string());
    } else {
        for mapping in &document.mappings {
            for spoken in &mapping.spoken {
                lines.push(format!(
                    "{} => {} | count={}",
                    spoken, mapping.canonical, mapping.count
                ));
            }
        }
    }

    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        AUTO_ACTIVATE_THRESHOLD, UserTermMapping, UserTermsDocument, correct_ascii_terms,
        ensure_trailing_period, has_terminal_punctuation, infer_single_word_correction,
        strip_trailing_period,
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
    fn infers_single_ascii_word_correction() {
        let result = infer_single_word_correction("please add scale here", "please add skill here");
        assert_eq!(result, Some(("scale".to_string(), "skill".to_string())));
    }

    #[test]
    fn learned_mapping_activates_after_two_corrections() {
        let document = UserTermsDocument {
            glossary: vec!["skill".to_string()],
            mappings: vec![UserTermMapping {
                spoken: vec!["scale".to_string()],
                canonical: "skill".to_string(),
                count: AUTO_ACTIVATE_THRESHOLD,
            }],
        };

        assert_eq!(
            correct_ascii_terms("add scale now", &document),
            "add skill now".to_string()
        );
    }

    #[test]
    fn glossary_supports_small_fuzzy_corrections() {
        let document = UserTermsDocument {
            glossary: vec!["emoji".to_string()],
            mappings: Vec::new(),
        };

        assert_eq!(
            correct_ascii_terms("send imoji please", &document),
            "send emoji please".to_string()
        );
    }
}
