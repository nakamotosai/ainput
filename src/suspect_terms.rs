use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::config::SuspectTermsConfig;
use crate::history::{self, HistoryRecord};
use crate::personal_corrections;

#[derive(Clone)]
pub struct SuspectTermsController {
    tx: mpsc::SyncSender<SuspectCommand>,
    suspect_path: PathBuf,
    corrections_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SuspectTermsNotification {
    pub new_items: usize,
}

#[derive(Debug)]
enum SuspectCommand {
    AnalyzeNow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SuspectTermBook {
    pub version: u32,
    pub updated_ms: u128,
    pub last_analyzed_history_ms: u128,
    pub items: Vec<SuspectTermItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SuspectTermItem {
    pub id: String,
    pub wrong: String,
    pub suggested: String,
    pub reason: String,
    pub examples: Vec<String>,
    pub confidence: f32,
    pub status: String,
    pub created_ms: u128,
    pub updated_ms: u128,
    pub source: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<String>,
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiSuggestion {
    wrong: String,
    suggested: String,
    reason: Option<String>,
    examples: Option<Vec<String>>,
    confidence: Option<f32>,
}

impl Default for SuspectTermItem {
    fn default() -> Self {
        let now = now_ms();
        Self {
            id: String::new(),
            wrong: String::new(),
            suggested: String::new(),
            reason: String::new(),
            examples: Vec::new(),
            confidence: 0.0,
            status: "pending".to_string(),
            created_ms: now,
            updated_ms: now,
            source: "qwen".to_string(),
        }
    }
}

impl Default for SuspectTermBook {
    fn default() -> Self {
        Self {
            version: 1,
            updated_ms: now_ms(),
            last_analyzed_history_ms: 0,
            items: Vec::new(),
        }
    }
}

impl SuspectTermsController {
    pub fn start(
        config: SuspectTermsConfig,
        history_path: PathBuf,
        suspect_path: PathBuf,
        corrections_path: PathBuf,
        notification_tx: mpsc::Sender<SuspectTermsNotification>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        if let Some(parent) = suspect_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create suspect terms dir {}", parent.display()))?;
        }
        if !suspect_path.exists() {
            save_book(&suspect_path, &SuspectTermBook::default())?;
        }
        let (tx, rx) = mpsc::sync_channel::<SuspectCommand>(16);
        let worker_suspect_path = suspect_path.clone();
        let worker_corrections_path = corrections_path.clone();
        thread::spawn(move || {
            let worker = SuspectTermsWorker::new(
                config,
                history_path,
                worker_suspect_path,
                worker_corrections_path,
                notification_tx,
            );
            worker.run(rx, shutdown);
        });
        Ok(Self {
            tx,
            suspect_path,
            corrections_path,
        })
    }

    pub fn analyze_now(&self) {
        let _ = self.tx.try_send(SuspectCommand::AnalyzeNow);
    }

    pub fn suspect_path(&self) -> &Path {
        &self.suspect_path
    }

    pub fn corrections_path(&self) -> &Path {
        &self.corrections_path
    }
}

struct SuspectTermsWorker {
    config: SuspectTermsConfig,
    history_path: PathBuf,
    suspect_path: PathBuf,
    corrections_path: PathBuf,
    notification_tx: mpsc::Sender<SuspectTermsNotification>,
    http: Client,
    api_key: Option<String>,
}

impl SuspectTermsWorker {
    fn new(
        config: SuspectTermsConfig,
        history_path: PathBuf,
        suspect_path: PathBuf,
        corrections_path: PathBuf,
        notification_tx: mpsc::Sender<SuspectTermsNotification>,
    ) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1000)))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| Client::new());
        let api_key = read_api_key(config.api_key_env.trim()).or_else(|| {
            let inline = config.api_key.trim().to_string();
            (!inline.is_empty()).then_some(inline)
        });
        Self {
            config,
            history_path,
            suspect_path,
            corrections_path,
            notification_tx,
            http,
            api_key,
        }
    }

    fn run(self, rx: mpsc::Receiver<SuspectCommand>, shutdown: Arc<AtomicBool>) {
        info!(
            enabled = self.config.enabled,
            suspect_path = %self.suspect_path.display(),
            corrections_path = %self.corrections_path.display(),
            api_key_present = self.api_key.is_some(),
            "suspect terms analyzer started"
        );
        let mut next_auto =
            Instant::now() + Duration::from_millis(self.config.startup_delay_ms.max(300_000));
        while !shutdown.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(SuspectCommand::AnalyzeNow) => self.run_once("manual"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if self.config.enabled && Instant::now() >= next_auto {
                self.run_once("auto");
                next_auto =
                    Instant::now() + Duration::from_millis(self.config.interval_ms.max(300_000));
            }
        }
        info!("suspect terms analyzer stopped");
    }

    fn run_once(&self, source: &str) {
        if !self.config.enabled {
            info!(source, "suspect terms analysis skipped: disabled");
            return;
        }
        match self.analyze_recent(source) {
            Ok(count) => {
                if count > 0 {
                    let _ = self
                        .notification_tx
                        .send(SuspectTermsNotification { new_items: count });
                }
                info!(
                    source,
                    new_suggestions = count,
                    "suspect terms analysis finished"
                );
            }
            Err(error) => warn!(source, error = %error, "suspect terms analysis failed"),
        }
    }

    fn analyze_recent(&self, source: &str) -> Result<usize> {
        let mut book = load_book(&self.suspect_path)?;
        let records = history::load_recent(&self.history_path, self.config.history_limit)
            .with_context(|| format!("load history {}", self.history_path.display()))?;
        let last_analyzed = book.last_analyzed_history_ms;
        let new_records = incremental_history_records(&records, last_analyzed);
        let newest_seen_ms = new_records
            .iter()
            .map(|record| record.timestamp_ms)
            .max()
            .unwrap_or(last_analyzed);
        let usable = new_records
            .into_iter()
            .filter(|record| !record.preview_text().trim().is_empty())
            .collect::<Vec<_>>();
        if usable.is_empty() {
            if newest_seen_ms > last_analyzed {
                book.last_analyzed_history_ms = newest_seen_ms;
                book.updated_ms = now_ms();
                save_book(&self.suspect_path, &book)?;
            }
            info!(
                source,
                last_analyzed_history_ms = book.last_analyzed_history_ms,
                "suspect terms analysis skipped: no new usable history"
            );
            return Ok(0);
        }
        if usable.len() < self.config.min_records.max(1) {
            info!(
                source,
                usable_records = usable.len(),
                min_records = self.config.min_records.max(1),
                "suspect terms analysis skipped: waiting for more new records"
            );
            return Ok(0);
        }
        let processed_history_ms = usable
            .iter()
            .map(|record| record.timestamp_ms)
            .max()
            .unwrap_or(newest_seen_ms);
        let suggestions = self.call_qwen(&usable)?;
        let added = merge_suggestions_into_book(&mut book, suggestions, source);
        book.last_analyzed_history_ms = processed_history_ms.max(book.last_analyzed_history_ms);
        book.updated_ms = now_ms();
        save_book(&self.suspect_path, &book)?;
        Ok(added)
    }

    fn call_qwen(&self, records: &[HistoryRecord]) -> Result<Vec<SuspectTermItem>> {
        let endpoint = self.config.endpoint_url.trim();
        if endpoint.is_empty() {
            bail!("suspect_terms.endpoint_url is empty");
        }
        let input = render_records_for_prompt(records);
        let messages = vec![
            json!({"role": "system", "content": suspect_system_prompt()}),
            json!({"role": "user", "content": input}),
        ];
        let models = models_to_try(&self.config.model, &self.config.fallback_models);
        let mut last_error = None;
        for model in models {
            let mut request = self.http.post(endpoint).json(&json!({
                "model": model,
                "messages": messages,
                "temperature": self.config.temperature,
                "max_tokens": self.config.max_output_chars.max(256),
            }));
            if let Some(api_key) = &self.api_key {
                request = request.bearer_auth(api_key);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            let response = match response.error_for_status() {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            let decoded = response
                .json::<ChatCompletionResponse>()
                .context("decode suspect terms response")?;
            let content = decoded
                .choices
                .first()
                .and_then(|choice| {
                    choice
                        .message
                        .content
                        .as_deref()
                        .filter(|text| !text.trim().is_empty())
                        .or_else(|| {
                            choice
                                .message
                                .reasoning_content
                                .as_deref()
                                .filter(|text| !text.trim().is_empty())
                        })
                })
                .ok_or_else(|| anyhow!("suspect terms response has no content"))?;
            return parse_suggestions(content, self.config.max_suggestions);
        }
        Err(anyhow!(
            "suspect terms model calls failed: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        ))
    }
}

fn incremental_history_records(
    records: &[HistoryRecord],
    last_analyzed_history_ms: u128,
) -> Vec<HistoryRecord> {
    let mut records = records
        .iter()
        .filter(|record| record.timestamp_ms > last_analyzed_history_ms)
        .cloned()
        .collect::<Vec<_>>();
    records.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms));
    records
}

pub fn load_book(path: &Path) -> Result<SuspectTermBook> {
    if !path.exists() {
        return Ok(SuspectTermBook::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read suspect book {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(SuspectTermBook::default());
    }
    let mut book = serde_json::from_str(&raw)
        .with_context(|| format!("parse suspect book {}", path.display()))?;
    normalize_loaded_book(&mut book);
    Ok(book)
}

pub fn save_book(path: &Path, book: &SuspectTermBook) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create suspect book dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(book).context("serialize suspect book")?;
    std::fs::write(path, raw).with_context(|| format!("write suspect book {}", path.display()))
}

pub fn merge_book_file_if_target_empty(target_path: &Path, source_path: &Path) -> Result<usize> {
    let mut target = load_book(target_path)?;
    if !target.items.is_empty() {
        return Ok(0);
    }
    let source = load_book(source_path)?;
    if source.items.is_empty() {
        return Ok(0);
    }
    let added = source.items.len();
    target.items = source.items;
    target.updated_ms = target.updated_ms.max(source.updated_ms).max(now_ms());
    target.last_analyzed_history_ms = target
        .last_analyzed_history_ms
        .max(source.last_analyzed_history_ms);
    sort_and_trim_items(&mut target.items);
    save_book(target_path, &target)?;
    Ok(added)
}

#[cfg(test)]
pub fn render_book(path: &Path) -> Result<String> {
    let book = load_book(path)?;
    Ok(render_book_items(&book.items))
}

#[cfg(test)]
pub fn apply_first_pending(suspect_path: &Path, corrections_path: &Path) -> Result<Option<String>> {
    let mut book = load_book(suspect_path)?;
    let Some(item) = book.items.iter_mut().find(|item| item.status == "pending") else {
        return Ok(None);
    };
    personal_corrections::append_or_update_rule(
        corrections_path,
        &item.wrong,
        &item.suggested,
        "suspect_terms_panel",
    )?;
    item.status = "applied".to_string();
    item.updated_ms = now_ms();
    let summary = format!("{} -> {}", item.wrong, item.suggested);
    sort_and_trim_items(&mut book.items);
    book.updated_ms = now_ms();
    save_book(suspect_path, &book)?;
    Ok(Some(summary))
}

#[cfg(test)]
pub fn apply_pending_ids(
    suspect_path: &Path,
    corrections_path: &Path,
    ids: &[String],
) -> Result<Vec<String>> {
    let mut book = load_book(suspect_path)?;
    let now = now_ms();
    let mut summaries = Vec::new();
    for id in ids {
        let Some(item) = book
            .items
            .iter_mut()
            .find(|item| item.id == *id && item.status == "pending")
        else {
            continue;
        };
        personal_corrections::append_or_update_rule(
            corrections_path,
            &item.wrong,
            &item.suggested,
            "suspect_terms_panel",
        )?;
        item.status = "applied".to_string();
        item.updated_ms = now;
        summaries.push(format!("{} -> {}", item.wrong, item.suggested));
    }
    if !summaries.is_empty() {
        sort_and_trim_items(&mut book.items);
        book.updated_ms = now_ms();
        save_book(suspect_path, &book)?;
    }
    Ok(summaries)
}

#[cfg(test)]
pub fn apply_first_pending_as(
    suspect_path: &Path,
    corrections_path: &Path,
    correct: &str,
) -> Result<Option<String>> {
    let correct = correct.trim();
    if correct.is_empty() {
        bail!("正确写法不能为空");
    }
    let mut book = load_book(suspect_path)?;
    let Some(item) = book.items.iter_mut().find(|item| item.status == "pending") else {
        return Ok(None);
    };
    let wrong = item.wrong.clone();
    if wrong.trim().is_empty() || wrong == correct {
        bail!("正确写法不能和错词相同");
    }
    let original_suggested = item.suggested.clone();
    personal_corrections::append_or_update_rule(
        corrections_path,
        &wrong,
        correct,
        "suspect_terms_panel_manual",
    )?;
    item.suggested = correct.to_string();
    if !original_suggested.trim().is_empty() && original_suggested != correct {
        let manual_note = format!("用户改正，原建议: {original_suggested}");
        item.reason = if item.reason.trim().is_empty() {
            manual_note
        } else {
            format!("{manual_note}；{}", item.reason)
        };
    }
    item.status = "applied".to_string();
    item.updated_ms = now_ms();
    let summary = format!("{wrong} -> {correct}");
    sort_and_trim_items(&mut book.items);
    book.updated_ms = now_ms();
    save_book(suspect_path, &book)?;
    Ok(Some(summary))
}

#[cfg(test)]
pub fn apply_item_id_as(
    suspect_path: &Path,
    corrections_path: &Path,
    id: &str,
    correct: &str,
) -> Result<Option<String>> {
    let correct = correct.trim();
    if correct.is_empty() {
        bail!("正确写法不能为空");
    }
    let mut book = load_book(suspect_path)?;
    let Some(item) = book.items.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    let wrong = item.wrong.clone();
    if wrong.trim().is_empty() || wrong == correct {
        bail!("正确写法不能和错词相同");
    }
    let original_suggested = item.suggested.clone();
    if item.status == "applied" && original_suggested != correct {
        let _ = personal_corrections::disable_matching_rules(
            corrections_path,
            &wrong,
            Some(&original_suggested),
        )?;
    }
    personal_corrections::append_or_update_rule(
        corrections_path,
        &wrong,
        correct,
        "suspect_terms_panel_manual",
    )?;
    item.suggested = correct.to_string();
    if !original_suggested.trim().is_empty() && original_suggested != correct {
        let manual_note = format!("用户改正，原建议: {original_suggested}");
        item.reason = if item.reason.trim().is_empty() {
            manual_note
        } else {
            format!("{manual_note}；{}", item.reason)
        };
    }
    item.status = "applied".to_string();
    item.updated_ms = now_ms();
    let summary = format!("{wrong} -> {correct}");
    sort_and_trim_items(&mut book.items);
    book.updated_ms = now_ms();
    save_book(suspect_path, &book)?;
    Ok(Some(summary))
}

#[derive(Debug, Clone)]
pub struct SuspectTermReviewUpdate {
    pub id: String,
    pub suggested: String,
    pub dismiss: bool,
}

#[derive(Debug, Default, Clone)]
pub struct SuspectTermReviewBatchResult {
    pub applied: Vec<String>,
    pub dismissed: Vec<String>,
    pub disabled_rules: usize,
}

pub fn apply_review_updates(
    suspect_path: &Path,
    corrections_path: &Path,
    updates: &[SuspectTermReviewUpdate],
) -> Result<SuspectTermReviewBatchResult> {
    let mut book = load_book(suspect_path)?;
    let now = now_ms();
    let mut result = SuspectTermReviewBatchResult::default();
    for update in updates {
        let Some(item) = book.items.iter_mut().find(|item| item.id == update.id) else {
            continue;
        };
        if item.status == "dismissed" {
            continue;
        }
        let wrong = item.wrong.clone();
        let original_suggested = item.suggested.clone();
        if update.dismiss {
            if item.status == "applied" {
                result.disabled_rules += personal_corrections::disable_matching_rules(
                    corrections_path,
                    &wrong,
                    Some(&original_suggested),
                )?;
            }
            item.status = "dismissed".to_string();
            item.updated_ms = now;
            result
                .dismissed
                .push(format!("{} -> {}", item.wrong, item.suggested));
            continue;
        }
        let suggested = update.suggested.trim();
        if suggested.is_empty() {
            bail!("{} 的建议不能为空", wrong);
        }
        if wrong.trim().is_empty() || wrong == suggested {
            bail!("{} 的建议不能和错词相同", wrong);
        }
        if item.status == "applied" && original_suggested != suggested {
            result.disabled_rules += personal_corrections::disable_matching_rules(
                corrections_path,
                &wrong,
                Some(&original_suggested),
            )?;
        }
        let source = if original_suggested == suggested {
            "suspect_terms_panel_batch"
        } else {
            "suspect_terms_panel_batch_manual"
        };
        personal_corrections::append_or_update_rule(corrections_path, &wrong, suggested, source)?;
        item.suggested = suggested.to_string();
        if !original_suggested.trim().is_empty() && original_suggested != suggested {
            let manual_note = format!("用户改正，原建议: {original_suggested}");
            item.reason = if item.reason.trim().is_empty() {
                manual_note
            } else {
                format!("{manual_note}；{}", item.reason)
            };
        }
        item.status = "applied".to_string();
        item.updated_ms = now;
        result.applied.push(format!("{wrong} -> {suggested}"));
    }
    if !result.applied.is_empty() || !result.dismissed.is_empty() {
        sort_and_trim_items(&mut book.items);
        book.updated_ms = now_ms();
        save_book(suspect_path, &book)?;
    }
    Ok(result)
}

#[cfg(test)]
pub fn dismiss_first_pending(suspect_path: &Path) -> Result<Option<String>> {
    let mut book = load_book(suspect_path)?;
    let Some(item) = book.items.iter_mut().find(|item| item.status == "pending") else {
        return Ok(None);
    };
    item.status = "dismissed".to_string();
    item.updated_ms = now_ms();
    let summary = format!("{} -> {}", item.wrong, item.suggested);
    sort_and_trim_items(&mut book.items);
    book.updated_ms = now_ms();
    save_book(suspect_path, &book)?;
    Ok(Some(summary))
}

#[cfg(test)]
pub fn dismiss_item_ids(
    suspect_path: &Path,
    corrections_path: &Path,
    ids: &[String],
) -> Result<Vec<String>> {
    let mut book = load_book(suspect_path)?;
    let now = now_ms();
    let mut summaries = Vec::new();
    for id in ids {
        let Some(item) = book
            .items
            .iter_mut()
            .find(|item| item.id == *id && item.status != "dismissed")
        else {
            continue;
        };
        if item.status == "applied" {
            let _ = personal_corrections::disable_matching_rules(
                corrections_path,
                &item.wrong,
                Some(&item.suggested),
            )?;
        }
        item.status = "dismissed".to_string();
        item.updated_ms = now;
        summaries.push(format!("{} -> {}", item.wrong, item.suggested));
    }
    if !summaries.is_empty() {
        sort_and_trim_items(&mut book.items);
        book.updated_ms = now_ms();
        save_book(suspect_path, &book)?;
    }
    Ok(summaries)
}

#[cfg(test)]
fn merge_suggestions(
    path: &Path,
    suggestions: Vec<SuspectTermItem>,
    source: &str,
) -> Result<usize> {
    let mut book = load_book(path)?;
    let added = merge_suggestions_into_book(&mut book, suggestions, source);
    save_book(path, &book)?;
    Ok(added)
}

fn merge_suggestions_into_book(
    book: &mut SuspectTermBook,
    suggestions: Vec<SuspectTermItem>,
    source: &str,
) -> usize {
    let mut changed = 0usize;
    let now = now_ms();
    for mut item in suggestions {
        normalize_item(&mut item, source, now);
        if item.wrong.is_empty() || item.suggested.is_empty() || item.wrong == item.suggested {
            continue;
        }
        if let Some(existing) = book
            .items
            .iter_mut()
            .find(|existing| existing.wrong == item.wrong && existing.suggested == item.suggested)
        {
            if existing.status == "dismissed" {
                continue;
            }
            existing.reason = item.reason;
            existing.examples = item.examples;
            existing.confidence = item.confidence;
            existing.updated_ms = now;
        } else if let Some(existing) = book
            .items
            .iter_mut()
            .find(|existing| existing.wrong == item.wrong)
        {
            if existing.status == "dismissed" || existing.status == "applied" {
                continue;
            }
            if item.confidence >= existing.confidence {
                existing.suggested = item.suggested;
                existing.reason = item.reason;
                existing.examples = item.examples;
                existing.confidence = item.confidence;
                existing.id = stable_id(&existing.wrong, &existing.suggested);
                existing.updated_ms = now;
            }
        } else {
            book.items.push(item);
            changed += 1;
        }
    }
    sort_and_trim_items(&mut book.items);
    book.updated_ms = now;
    changed
}

fn normalize_loaded_book(book: &mut SuspectTermBook) {
    let now = now_ms();
    for item in &mut book.items {
        item.wrong = item.wrong.trim().to_string();
        item.suggested = item.suggested.trim().to_string();
        item.status = item.status.trim().to_string();
        if item.status.is_empty() {
            item.status = "pending".to_string();
        }
        if item.id.trim().is_empty() {
            item.id = stable_id(&item.wrong, &item.suggested);
        }
        if item.updated_ms == 0 {
            item.updated_ms = now;
        }
    }
    book.items
        .retain(|item| !item.wrong.is_empty() && !item.suggested.is_empty());
    book.items.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    let mut deduped = Vec::<SuspectTermItem>::new();
    for item in book.items.drain(..) {
        if !deduped
            .iter()
            .any(|existing| existing.wrong == item.wrong && existing.suggested == item.suggested)
        {
            deduped.push(item);
        }
    }
    book.items = deduped;
    sort_and_trim_items(&mut book.items);
}

fn sort_and_trim_items(items: &mut Vec<SuspectTermItem>) {
    items.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| b.confidence.total_cmp(&a.confidence))
            .then_with(|| b.updated_ms.cmp(&a.updated_ms))
    });
    if items.len() > 200 {
        items.truncate(200);
    }
}

fn status_rank(status: &str) -> u8 {
    match status {
        "pending" => 0,
        "applied" => 1,
        "dismissed" => 2,
        _ => 3,
    }
}

fn normalize_item(item: &mut SuspectTermItem, source: &str, now: u128) {
    item.wrong = item.wrong.trim().to_string();
    item.suggested = item.suggested.trim().to_string();
    item.reason = item.reason.trim().to_string();
    item.examples = item
        .examples
        .iter()
        .map(|example| example.trim().to_string())
        .filter(|example| !example.is_empty())
        .take(4)
        .collect();
    item.confidence = item.confidence.clamp(0.0, 1.0);
    item.status = if item.status.trim().is_empty() {
        "pending".to_string()
    } else {
        item.status.trim().to_string()
    };
    item.source = source.to_string();
    item.id = stable_id(&item.wrong, &item.suggested);
    if item.created_ms == 0 {
        item.created_ms = now;
    }
    item.updated_ms = now;
}

fn parse_suggestions(content: &str, limit: usize) -> Result<Vec<SuspectTermItem>> {
    let json_text =
        extract_json_array(content).ok_or_else(|| anyhow!("no JSON array in response"))?;
    let raw =
        serde_json::from_str::<Vec<AiSuggestion>>(json_text).context("parse suggestions JSON")?;
    let now = now_ms();
    Ok(raw
        .into_iter()
        .take(limit.max(1))
        .map(|suggestion| SuspectTermItem {
            wrong: suggestion.wrong,
            suggested: suggestion.suggested,
            reason: suggestion.reason.unwrap_or_default(),
            examples: suggestion.examples.unwrap_or_default(),
            confidence: suggestion.confidence.unwrap_or(0.5),
            created_ms: now,
            updated_ms: now,
            ..Default::default()
        })
        .collect())
}

fn extract_json_array(content: &str) -> Option<&str> {
    let start = content.find('[')?;
    let end = content.rfind(']')?;
    (end >= start).then_some(&content[start..=end])
}

#[cfg(test)]
fn render_book_items(items: &[SuspectTermItem]) -> String {
    let mut out = String::new();
    out.push_str("ainput 疑似错词\r\n\r\n");
    if items.is_empty() {
        out.push_str("暂无建议。后台会定期分析最近语音历史，也可以点“立即分析”。\r\n");
        return out;
    }
    for (index, item) in items.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} -> {}  置信度 {:.0}%\r\n",
            index + 1,
            item.status,
            item.wrong,
            item.suggested,
            item.confidence * 100.0
        ));
        if !item.reason.is_empty() {
            out.push_str(&format!("   原因: {}\r\n", one_line(&item.reason, 120)));
        }
        for example in item.examples.iter().take(2) {
            out.push_str(&format!("   例子: {}\r\n", one_line(example, 140)));
        }
    }
    out
}

fn render_records_for_prompt(records: &[HistoryRecord]) -> String {
    let mut out = String::new();
    out.push_str("请分析下面语音输入历史，找出疑似 ASR 错词。\n");
    for record in records.iter().rev().take(60).rev() {
        out.push_str(&format!(
            "- id={} mode={} raw={:?} final={:?} pasted={:?} actions={:?}\n",
            record.utterance_id,
            record.mode,
            one_line(&record.raw_text, 160),
            one_line(&record.finalized_text, 160),
            one_line(&record.pasted_text, 160),
            record.finalizer_actions,
        ));
    }
    out
}

fn suspect_system_prompt() -> &'static str {
    "你是语音输入法的疑似错词分析器。只输出 JSON 数组，不要 Markdown，不要解释。

任务：从用户最近的 ASR 历史中，找出高概率的错词映射，尤其是中英文混杂时技术词、产品名、缩写被听成中文同音词的情况。

输出格式：
[
  {\"wrong\":\"扣带\",\"suggested\":\"Codex\",\"reason\":\"多次出现在编程上下文，像 Codex 的同音误识别\",\"examples\":[\"我在用扣带编程\"],\"confidence\":0.9}
]

要求：
- wrong 必须是历史里实际出现的短词或短语。
- suggested 必须是用户可能真正想输入的标准写法。
- 不要输出普通错别字猜测；只输出高价值、可加入个人词典的稳定错词。
- Codex/项目管理语境里“收口项目”是用户常用真实说法，不要把它误判成“收录/收购”类改写。
- 最多输出 12 条。没有可靠发现就输出 []。"
}

fn models_to_try(primary: &str, fallback: &[String]) -> Vec<String> {
    let mut models = Vec::new();
    for model in std::iter::once(primary).chain(fallback.iter().map(String::as_str)) {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    models
}

fn read_api_key(primary_env: &str) -> Option<String> {
    let names = [
        primary_env,
        "AINPUT_CLIPROXYAPI_8317_KEY",
        "CODEX_CLIPROXYAPI_8317_API_KEY",
    ];
    for name in names {
        if name.trim().is_empty() {
            continue;
        }
        if let Ok(value) = std::env::var(name.trim()) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
        #[cfg(windows)]
        if let Some(value) = read_windows_user_env_var(name.trim()) {
            return Some(value);
        }
    }
    None
}

#[cfg(windows)]
fn read_windows_user_env_var(name: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};

    if name.trim().is_empty() {
        return None;
    }
    let subkey = HSTRING::from("Environment");
    let value_name = HSTRING::from(name);
    let mut bytes = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes < 2 {
        return None;
    }
    let mut buffer = vec![0u16; bytes.div_ceil(2) as usize];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer
        .iter()
        .position(|code| *code == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..len]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn stable_id(wrong: &str, suggested: &str) -> String {
    let mut hash = 1469598103934665603u64;
    for byte in format!("{wrong}\0{suggested}").as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn one_line(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = compact.chars().take(max_chars).collect::<String>();
    if compact.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        SuspectTermItem, SuspectTermReviewUpdate, apply_first_pending, apply_first_pending_as,
        apply_item_id_as, apply_pending_ids, apply_review_updates, dismiss_first_pending,
        dismiss_item_ids, incremental_history_records, load_book, merge_suggestions,
        parse_suggestions, render_book,
    };
    use crate::history::HistoryRecord;

    #[test]
    fn incremental_history_records_only_returns_newer_records_in_order() {
        let records = vec![
            HistoryRecord {
                timestamp_ms: 300,
                raw_text: "三".to_string(),
                ..Default::default()
            },
            HistoryRecord {
                timestamp_ms: 100,
                raw_text: "一".to_string(),
                ..Default::default()
            },
            HistoryRecord {
                timestamp_ms: 200,
                raw_text: "二".to_string(),
                ..Default::default()
            },
        ];
        let selected = incremental_history_records(&records, 100);
        assert_eq!(
            selected
                .iter()
                .map(|record| record.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![200, 300]
        );
    }

    #[test]
    fn parses_json_suggestions_from_model_text() {
        let parsed = parse_suggestions(
            "```json\n[{\"wrong\":\"扣带\",\"suggested\":\"Codex\",\"reason\":\"编程上下文\",\"examples\":[\"我在用扣带\"],\"confidence\":0.88}]\n```",
            12,
        )
        .expect("parse suggestions");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].wrong, "扣带");
        assert_eq!(parsed[0].suggested, "Codex");
    }

    #[test]
    fn merges_and_applies_first_pending_suggestion() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "口袋".to_string(),
                suggested: "Codex".to_string(),
                confidence: 0.9,
                reason: "test".to_string(),
                examples: vec!["这个口袋很好用".to_string()],
                ..Default::default()
            }],
            "test",
        )
        .expect("merge");
        let rendered = render_book(&suspect_path).expect("render");
        assert!(rendered.contains("口袋 -> Codex"));
        let applied =
            apply_first_pending(&suspect_path, &corrections_path).expect("apply first pending");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        assert_eq!(applied.as_deref(), Some("口袋 -> Codex"));
    }

    #[test]
    fn merge_counts_only_new_suggestions_and_keeps_existing_pending() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-incremental");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let first = merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "抽真".to_string(),
                suggested: "抽帧".to_string(),
                confidence: 0.95,
                reason: "video context".to_string(),
                examples: vec!["只有推特需要抽真".to_string()],
                ..Default::default()
            }],
            "test",
        )
        .expect("first merge");
        let second = merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "抽真".to_string(),
                suggested: "抽帧".to_string(),
                confidence: 0.99,
                reason: "updated".to_string(),
                examples: vec!["更新例句".to_string()],
                ..Default::default()
            }],
            "test",
        )
        .expect("second merge");
        let book = load_book(&suspect_path).expect("load book");
        let _ = std::fs::remove_file(&suspect_path);
        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert_eq!(book.items.len(), 1);
        assert_eq!(book.items[0].status, "pending");
        assert_eq!(book.items[0].wrong, "抽真");
        assert_eq!(book.items[0].suggested, "抽帧");
    }

    #[test]
    fn can_dismiss_first_pending_suggestion() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-dismiss");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "收购这个项目".to_string(),
                suggested: "收录这个项目".to_string(),
                confidence: 0.83,
                reason: "project context".to_string(),
                examples: vec!["请你收购这个项目".to_string()],
                ..Default::default()
            }],
            "test",
        )
        .expect("merge");
        let dismissed = dismiss_first_pending(&suspect_path).expect("dismiss first pending");
        let book = load_book(&suspect_path).expect("load book");
        let _ = std::fs::remove_file(&suspect_path);
        assert_eq!(dismissed.as_deref(), Some("收购这个项目 -> 收录这个项目"));
        assert_eq!(book.items[0].status, "dismissed");
    }

    #[test]
    fn can_apply_first_pending_with_manual_correction() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-manual");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "收购这个项目".to_string(),
                suggested: "收录这个项目".to_string(),
                confidence: 0.83,
                reason: "project context".to_string(),
                examples: vec!["请你收购这个项目".to_string()],
                ..Default::default()
            }],
            "test",
        )
        .expect("merge");
        let applied = apply_first_pending_as(&suspect_path, &corrections_path, "收口这个项目")
            .expect("manual apply");
        let book = load_book(&suspect_path).expect("load book");
        let corrections =
            crate::personal_corrections::load_store(&corrections_path).expect("load corrections");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        assert_eq!(applied.as_deref(), Some("收购这个项目 -> 收口这个项目"));
        assert_eq!(book.items[0].status, "applied");
        assert_eq!(book.items[0].suggested, "收口这个项目");
        assert_eq!(corrections.rules[0].wrong, "收购这个项目");
        assert_eq!(corrections.rules[0].correct, "收口这个项目");
    }

    #[test]
    fn can_apply_and_dismiss_selected_pending_suggestions() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-selected");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![
                SuspectTermItem {
                    wrong: "必安黄金".to_string(),
                    suggested: "币安黄金".to_string(),
                    confidence: 0.91,
                    ..Default::default()
                },
                SuspectTermItem {
                    wrong: "收购这个项目".to_string(),
                    suggested: "收录这个项目".to_string(),
                    confidence: 0.83,
                    ..Default::default()
                },
            ],
            "test",
        )
        .expect("merge");
        let book = load_book(&suspect_path).expect("load book");
        let apply_id = book
            .items
            .iter()
            .find(|item| item.wrong == "必安黄金")
            .expect("apply item")
            .id
            .clone();
        let dismiss_id = book
            .items
            .iter()
            .find(|item| item.wrong == "收购这个项目")
            .expect("dismiss item")
            .id
            .clone();

        let applied = apply_pending_ids(&suspect_path, &corrections_path, &[apply_id])
            .expect("apply selected");
        let dismissed = dismiss_item_ids(&suspect_path, &corrections_path, &[dismiss_id])
            .expect("dismiss selected");
        let book = load_book(&suspect_path).expect("reload book");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);

        assert_eq!(applied, vec!["必安黄金 -> 币安黄金"]);
        assert_eq!(dismissed, vec!["收购这个项目 -> 收录这个项目"]);
        assert_eq!(
            book.items
                .iter()
                .find(|item| item.wrong == "必安黄金")
                .expect("applied item")
                .status,
            "applied"
        );
        assert_eq!(
            book.items
                .iter()
                .find(|item| item.wrong == "收购这个项目")
                .expect("dismissed item")
                .status,
            "dismissed"
        );
    }

    #[test]
    fn can_apply_selected_item_with_manual_correction() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-selected-manual");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "收购这个项目".to_string(),
                suggested: "收录这个项目".to_string(),
                confidence: 0.83,
                ..Default::default()
            }],
            "test",
        )
        .expect("merge");
        let book = load_book(&suspect_path).expect("load book");
        let id = book.items[0].id.clone();
        let applied = apply_item_id_as(&suspect_path, &corrections_path, &id, "收口这个项目")
            .expect("manual selected");
        let book = load_book(&suspect_path).expect("reload book");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        assert_eq!(applied.as_deref(), Some("收购这个项目 -> 收口这个项目"));
        assert_eq!(book.items[0].suggested, "收口这个项目");
        assert_eq!(book.items[0].status, "applied");
    }

    #[test]
    fn dismissing_applied_item_disables_matching_correction() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-dismiss-applied");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![SuspectTermItem {
                wrong: "大酒保公园".to_string(),
                suggested: "大酒保公司".to_string(),
                confidence: 0.85,
                ..Default::default()
            }],
            "test",
        )
        .expect("merge");
        let book = load_book(&suspect_path).expect("load book");
        let id = book.items[0].id.clone();
        apply_pending_ids(&suspect_path, &corrections_path, std::slice::from_ref(&id))
            .expect("apply selected");
        dismiss_item_ids(&suspect_path, &corrections_path, &[id]).expect("dismiss applied");
        let corrections =
            crate::personal_corrections::load_store(&corrections_path).expect("load corrections");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        assert_eq!(corrections.rules.len(), 1);
        assert!(!corrections.rules[0].enabled);
    }

    #[test]
    fn can_apply_review_batch_with_edits_and_dismissals() {
        let dir = std::env::temp_dir();
        let suffix = format!("{}-{}", std::process::id(), "suspect-review-batch");
        let suspect_path = dir.join(format!("ainput-suspect-{suffix}.json"));
        let corrections_path = dir.join(format!("ainput-corrections-{suffix}.json"));
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);
        merge_suggestions(
            &suspect_path,
            vec![
                SuspectTermItem {
                    wrong: "必安黄金".to_string(),
                    suggested: "币安黄金".to_string(),
                    confidence: 0.91,
                    ..Default::default()
                },
                SuspectTermItem {
                    wrong: "收购这个项目".to_string(),
                    suggested: "收录这个项目".to_string(),
                    confidence: 0.83,
                    ..Default::default()
                },
                SuspectTermItem {
                    wrong: "大酒保公园".to_string(),
                    suggested: "大酒保公司".to_string(),
                    confidence: 0.85,
                    ..Default::default()
                },
            ],
            "test",
        )
        .expect("merge");
        let book = load_book(&suspect_path).expect("load book");
        let binance_id = book
            .items
            .iter()
            .find(|item| item.wrong == "必安黄金")
            .expect("binance")
            .id
            .clone();
        let closeout_id = book
            .items
            .iter()
            .find(|item| item.wrong == "收购这个项目")
            .expect("closeout")
            .id
            .clone();
        let dismiss_id = book
            .items
            .iter()
            .find(|item| item.wrong == "大酒保公园")
            .expect("dismiss")
            .id
            .clone();

        let result = apply_review_updates(
            &suspect_path,
            &corrections_path,
            &[
                SuspectTermReviewUpdate {
                    id: binance_id,
                    suggested: "币安黄金".to_string(),
                    dismiss: false,
                },
                SuspectTermReviewUpdate {
                    id: closeout_id,
                    suggested: "收口这个项目".to_string(),
                    dismiss: false,
                },
                SuspectTermReviewUpdate {
                    id: dismiss_id,
                    suggested: "大酒保公司".to_string(),
                    dismiss: true,
                },
            ],
        )
        .expect("apply review batch");
        let book = load_book(&suspect_path).expect("reload book");
        let corrections =
            crate::personal_corrections::load_store(&corrections_path).expect("corrections");
        let _ = std::fs::remove_file(&suspect_path);
        let _ = std::fs::remove_file(&corrections_path);

        assert_eq!(result.applied.len(), 2);
        assert_eq!(result.dismissed.len(), 1);
        assert_eq!(
            book.items
                .iter()
                .find(|item| item.wrong == "收购这个项目")
                .expect("closeout")
                .suggested,
            "收口这个项目"
        );
        assert_eq!(
            book.items
                .iter()
                .find(|item| item.wrong == "大酒保公园")
                .expect("dismissed")
                .status,
            "dismissed"
        );
        assert!(corrections.rules.iter().any(|rule| {
            rule.wrong == "收购这个项目" && rule.correct == "收口这个项目" && rule.enabled
        }));
        assert!(
            corrections
                .rules
                .iter()
                .any(|rule| rule.wrong == "必安黄金" && rule.correct == "币安黄金" && rule.enabled)
        );
    }
}
