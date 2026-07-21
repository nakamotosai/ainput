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
    out.push_str("ainput 听写历史（本机存档，不上云）\r\n\r\n");
    if records.is_empty() {
        out.push_str("暂无记录。按住 CapsLock 说几句后点刷新。\r\n");
        return out;
    }

    out.push_str(&format!("合计 {} 条（新→旧）\r\n\r\n", records.len()));
    for (index, record) in records.iter().rev().enumerate() {
        let n = index + 1;
        let when = format_timestamp_ms(record.timestamp_ms);
        let target = if record.target_process.trim().is_empty() {
            "未知应用".to_string()
        } else {
            record.target_process.clone()
        };
        let mode_label = if record.rewrite_enabled {
            "AI改写"
        } else {
            "原文直出"
        };
        out.push_str(&format!(
            "—— #{n} · {when} · {mode_label} · {target} · {}ms ——\r\n",
            record.total_elapsed_ms
        ));

        let raw = first_nonempty(&[&record.raw_text, &record.finalized_text]);
        let rewritten = first_nonempty(&[&record.rewrite_text, &record.pasted_text]);
        let pasted = record.pasted_text.trim();

        if record.rewrite_enabled {
            out.push_str(&format!("改写前: {}\r\n", display_or_empty(raw)));
            out.push_str(&format!("改写后: {}\r\n", display_or_empty(rewritten)));
            if !pasted.is_empty() && pasted != rewritten {
                out.push_str(&format!("最终粘贴: {}\r\n", pasted));
            }
            if !record.rewrite_model.is_empty() || record.rewrite_elapsed_ms > 0 {
                out.push_str(&format!(
                    "模型: {} · 改写耗时 {}ms\r\n",
                    if record.rewrite_model.is_empty() {
                        "(未记)"
                    } else {
                        &record.rewrite_model
                    },
                    record.rewrite_elapsed_ms
                ));
            }
            if !record.rewrite_error.is_empty() {
                out.push_str(&format!("改写错误: {}\r\n", record.rewrite_error));
            }
        } else {
            out.push_str(&format!("原文: {}\r\n", display_or_empty(record.preview_text())));
        }

        if !record.error.is_empty() || !record.skipped_reason.is_empty() {
            out.push_str(&format!(
                "状态: {}{}\r\n",
                record.error, record.skipped_reason
            ));
        }
        out.push_str("\r\n");
    }
    out
}

fn first_nonempty<'a>(parts: &[&'a str]) -> &'a str {
    for part in parts {
        if !part.trim().is_empty() {
            return part.trim();
        }
    }
    ""
}

fn display_or_empty(text: &str) -> &str {
    if text.trim().is_empty() {
        "(空)"
    } else {
        text
    }
}

fn format_timestamp_ms(timestamp_ms: u128) -> String {
    // History uses wall-clock millis since UNIX epoch when available.
    if timestamp_ms < 1_000_000_000_000 {
        return format!("{timestamp_ms}");
    }
    let secs = (timestamp_ms / 1000) as i64;
    // Local-friendly HH:MM:SS from epoch without external crates.
    // Windows: convert via systemtime when possible.
    match std::time::SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64)) {
        Some(time) => match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => {
                let total = d.as_secs();
                // Approximate local by using UTC stamp; panel also shows path for full jsonl.
                let hours = (total / 3600) % 24;
                let mins = (total / 60) % 60;
                let s = total % 60;
                format!("UTC {hours:02}:{mins:02}:{s:02}")
            }
            Err(_) => format!("{timestamp_ms}"),
        },
        None => format!("{timestamp_ms}"),
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
    fn renders_raw_and_rewrite_before_after() {
        let mut raw_only = HistoryRecord::new("utt-1", "local_nonstreaming", "local_nonstreaming");
        raw_only.raw_text = "原句一".to_string();
        raw_only.pasted_text = "原句一".to_string();
        raw_only.rewrite_enabled = false;
        raw_only.total_elapsed_ms = 200;

        let mut rewritten = HistoryRecord::new("utt-2", "local_nonstreaming", "local_nonstreaming");
        rewritten.raw_text = "改写前句子".to_string();
        rewritten.rewrite_text = "改写后句子".to_string();
        rewritten.pasted_text = "改写后句子".to_string();
        rewritten.rewrite_enabled = true;
        rewritten.rewrite_model = "demo-model".to_string();
        rewritten.rewrite_elapsed_ms = 123;
        rewritten.total_elapsed_ms = 900;

        let rendered = render_history(&[raw_only, rewritten]);
        assert!(rendered.contains("合计 2 条"));
        assert!(rendered.contains("原文: 原句一"));
        assert!(rendered.contains("改写前: 改写前句子"));
        assert!(rendered.contains("改写后: 改写后句子"));
        assert!(rendered.contains("demo-model"));
    }

    #[test]
    fn renders_rewrite_error_when_present() {
        let mut record = HistoryRecord::new("utt-1", "local_nonstreaming", "local_nonstreaming");
        record.raw_text = "你好".to_string();
        record.rewrite_enabled = true;
        record.rewrite_error = "timeout".to_string();
        let rendered = render_history(&[record]);
        assert!(rendered.contains("改写错误: timeout"));
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
