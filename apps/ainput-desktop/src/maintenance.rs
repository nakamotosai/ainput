use std::fs;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::worker::VoiceHistoryEntry;

#[derive(Clone)]
pub(crate) struct SharedRuntimeState {
    last_voice_text: Arc<Mutex<Option<String>>>,
}

impl SharedRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            last_voice_text: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_last_voice_text(&self, text: String) {
        if let Ok(mut slot) = self.last_voice_text.lock() {
            *slot = Some(text);
        }
    }

    pub(crate) fn last_voice_text(&self) -> Option<String> {
        self.last_voice_text
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

#[derive(Clone)]
pub(crate) struct MaintenanceHandle {
    tx: mpsc::Sender<MaintenanceTask>,
}

impl MaintenanceHandle {
    pub(crate) fn start(
        logs_dir: std::path::PathBuf,
        history_file_name: String,
        history_limit: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(task) = rx.recv() {
                if let Err(error) = handle_task(&logs_dir, &history_file_name, history_limit, task)
                {
                    tracing::error!(error = %error, "maintenance task failed");
                }
            }
        });
        Self { tx }
    }

    pub(crate) fn persist_voice_result(&self, entry: VoiceHistoryEntry) {
        let _ = self.tx.send(MaintenanceTask::PersistVoiceResult(entry));
    }
}

enum MaintenanceTask {
    PersistVoiceResult(VoiceHistoryEntry),
}

fn handle_task(
    logs_dir: &std::path::Path,
    history_file_name: &str,
    history_limit: usize,
    task: MaintenanceTask,
) -> anyhow::Result<()> {
    match task {
        MaintenanceTask::PersistVoiceResult(entry) => {
            fs::write(logs_dir.join("last_result.txt"), &entry.text)?;

            let history_path = logs_dir.join(history_file_name);
            let existing = fs::read_to_string(&history_path).unwrap_or_default();
            let mut lines: Vec<String> = existing.lines().map(ToOwned::to_owned).collect();
            lines.push(format!(
                "{}\t{}\t{}",
                entry.timestamp,
                entry.delivery_label,
                entry.text.replace(['\r', '\n'], " ")
            ));
            if lines.len() > history_limit {
                let keep_from = lines.len().saturating_sub(history_limit);
                lines = lines.split_off(keep_from);
            }
            fs::write(history_path, lines.join("\n"))?;
        }
    }

    Ok(())
}
