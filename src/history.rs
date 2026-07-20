use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistoryRecord {
    pub utterance_id: String,
    pub timestamp_ms: u128,
    pub profile_id: String,
    pub mode: String,
    pub raw_text: String,
    pub finalized_text: String,
    pub pasted_text: String,
    pub target_process: String,
    pub target_class: String,
    pub target_title: String,
    pub target_context_source: String,
    pub target_right_context: String,
    pub finalizer_actions: String,
    pub output_actions: String,
    pub rewrite_enabled: bool,
    pub rewrite_model: String,
    pub rewrite_attempts: String,
    pub rewrite_elapsed_ms: u128,
    pub rewrite_error: String,
    pub rewrite_text: String,
    pub partial_updates: usize,
    pub audio_ms: u64,
    pub asr_elapsed_ms: u128,
    #[serde(default)]
    pub phase_timings: String,
    pub total_elapsed_ms: u128,
    pub error: String,
    pub skipped_reason: String,
}

impl HistoryRecord {
    pub fn new(utterance_id: &str, profile_id: &str, mode: &str) -> Self {
        Self {
            utterance_id: utterance_id.to_string(),
            timestamp_ms: now_ms(),
            profile_id: profile_id.to_string(),
            mode: mode.to_string(),
            ..Default::default()
        }
    }

    pub fn preview_text(&self) -> &str {
        if !self.pasted_text.trim().is_empty() {
            &self.pasted_text
        } else if !self.finalized_text.trim().is_empty() {
            &self.finalized_text
        } else {
            &self.raw_text
        }
    }
}

#[derive(Clone)]
pub struct HistoryService {
    tx: mpsc::SyncSender<HistoryRecord>,
    path: PathBuf,
}

impl HistoryService {
    pub fn start(path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create history dir {}", parent.display()))?;
        }
        let (tx, rx) = mpsc::sync_channel::<HistoryRecord>(256);
        let writer_path = path.clone();
        thread::spawn(move || {
            info!(path = %writer_path.display(), "history writer started");
            while !shutdown.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(record) => {
                        if let Err(error) = append_record(&writer_path, &record) {
                            warn!(error = %error, "history append failed");
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            while let Ok(record) = rx.try_recv() {
                if let Err(error) = append_record(&writer_path, &record) {
                    warn!(error = %error, "history append during shutdown failed");
                }
            }
            info!(path = %writer_path.display(), "history writer stopped");
        });
        Ok(Self { tx, path })
    }

    pub fn record(&self, record: HistoryRecord) {
        if let Err(error) = self.tx.try_send(record) {
            warn!(error = %error, "history queue full or closed; dropping record");
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn load_recent(path: &Path, limit: usize) -> Result<Vec<HistoryRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).with_context(|| format!("open history {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.with_context(|| format!("read history {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryRecord>(&line) {
            Ok(record) => records.push(record),
            Err(error) => warn!(error = %error, "skip malformed history record"),
        }
    }
    if records.len() > limit {
        records.drain(0..records.len() - limit);
    }
    Ok(records)
}

pub fn clear(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create history dir {}", parent.display()))?;
    }
    File::create(path).with_context(|| format!("clear history {}", path.display()))?;
    Ok(())
}

pub fn render_history(records: &[HistoryRecord]) -> String {
    let mut out = String::new();
    out.push_str("ainput 历史 / 对比\r\n\r\n");
    if records.is_empty() {
        out.push_str("暂无记录。\r\n");
        return out;
    }

    let latest_streaming = records
        .iter()
        .rev()
        .find(|record| record.profile_id == "streaming_default");
    let latest_cloud = records
        .iter()
        .rev()
        .find(|record| record.profile_id == "whisper_capslock" && !record.pasted_text.is_empty());
    let latest_local = records
        .iter()
        .rev()
        .find(|record| record.profile_id == "local_nonstreaming" && !record.pasted_text.is_empty());

    out.push_str("最近对比\r\n");
    out.push_str(&format_comparison_line("Ctrl / 流式预览", latest_streaming));
    out.push_str(&format_comparison_line("CapsLock / 快速本地", latest_local));
    out.push_str(&format_comparison_line("Alt+Z / 云端备用", latest_cloud));
    out.push_str("\r\n记录\r\n");
    for record in records.iter().rev() {
        out.push_str(&format!(
            "[{}] {} {} {}ms {} {}\r\n",
            record.timestamp_ms,
            record.profile_id,
            record.mode,
            record.total_elapsed_ms,
            record.target_context_source,
            one_line(record.preview_text(), 120)
        ));
        if !record.error.is_empty() || !record.skipped_reason.is_empty() {
            out.push_str(&format!(
                "  状态: {}{}\r\n",
                record.error, record.skipped_reason
            ));
        }
        if record.rewrite_enabled {
            out.push_str(&format!(
                "  AI: model={} elapsed={}ms attempts={} error={}\r\n",
                record.rewrite_model,
                record.rewrite_elapsed_ms,
                record.rewrite_attempts,
                record.rewrite_error
            ));
        }
        if !record.phase_timings.is_empty() {
            out.push_str(&format!("  阶段: {}\r\n", record.phase_timings));
        }
    }
    out
}

fn format_comparison_line(label: &str, record: Option<&HistoryRecord>) -> String {
    match record {
        Some(record) => format!(
            "- {}: {}ms, {}\r\n",
            label,
            record.total_elapsed_ms,
            one_line(record.preview_text(), 120)
        ),
        None => format!("- {}: 暂无记录\r\n", label),
    }
}

fn append_record(path: &Path, record: &HistoryRecord) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open history {}", path.display()))?;
    let line = serde_json::to_string(record).context("serialize history record")?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("write history {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write history newline {}", path.display()))?;
    Ok(())
}

fn one_line(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        match chars.next() {
            Some(ch) => out.push(ch),
            None => return out,
        }
    }
    if chars.next().is_some() {
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
    use super::{HistoryRecord, append_record, load_recent, render_history};

    #[test]
    fn renders_streaming_and_whisper_comparison() {
        let mut streaming = HistoryRecord::new("utt-1", "streaming_default", "streaming_asr");
        streaming.pasted_text = "你好 Codex".to_string();
        streaming.total_elapsed_ms = 200;
        let mut whisper = HistoryRecord::new("utt-2", "whisper_capslock", "whisper_zh");
        whisper.pasted_text = "你好 CodeX".to_string();
        whisper.total_elapsed_ms = 900;
        let mut local = HistoryRecord::new("utt-3", "local_nonstreaming", "local_nonstreaming");
        local.pasted_text = "你好本地".to_string();
        local.total_elapsed_ms = 300;
        let rendered = render_history(&[streaming, whisper, local]);
        assert!(rendered.contains("Ctrl / 流式预览"));
        assert!(rendered.contains("CapsLock / 快速本地"));
        assert!(rendered.contains("Alt+Z / 云端备用"));
        assert!(rendered.contains("你好 Codex"));
        assert!(rendered.contains("你好 CodeX"));
    }

    #[test]
    fn renders_phase_timings_when_present() {
        let mut record = HistoryRecord::new("utt-1", "local_nonstreaming", "local_nonstreaming");
        record.pasted_text = "你好".to_string();
        record.phase_timings = "record=800ms;asr=300ms;rewrite=400ms;paste=30ms".to_string();
        let rendered = render_history(&[record]);
        assert!(rendered.contains("阶段: record=800ms;asr=300ms;rewrite=400ms;paste=30ms"));
    }

    #[test]
    fn appends_and_loads_jsonl_records() {
        let path =
            std::env::temp_dir().join(format!("ainput-history-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut record = HistoryRecord::new("utt-jsonl", "streaming_default", "streaming_asr");
        record.pasted_text = "历史测试".to_string();
        append_record(&path, &record).expect("append record");
        let loaded = load_recent(&path, 10).expect("load history");
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].utterance_id, "utt-jsonl");
        assert_eq!(loaded[0].pasted_text, "历史测试");
    }
}
