//! User override for the local voice hotkey (keyboard / mouse side buttons).
//! Persists to state/config/hotkey-user.toml and is applied at process start.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::ProfileConfigs;
use crate::hotkey::{hotkey_supports_suppress, parse_hotkey_label, validate_hotkey_label};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HotkeyUserFile {
    /// Voice hold key for local SenseVoice profile. Examples: CapsLock, F13, MouseX2, Ctrl+F8
    pub local_nonstreaming: Option<String>,
}

#[derive(Clone)]
pub struct HotkeyUserController {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    local_nonstreaming: Mutex<String>,
}

impl HotkeyUserController {
    pub fn load_or_default(path: PathBuf, fallback: &str) -> Self {
        let from_disk = load_file(&path);
        let label = from_disk
            .local_nonstreaming
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string());
        let label = if validate_hotkey_label(&label).is_ok() {
            normalize_label(&label)
        } else {
            warn!(hotkey = %label, "invalid saved hotkey; falling back to {fallback}");
            fallback.to_string()
        };
        Self {
            inner: Arc::new(Inner {
                path,
                local_nonstreaming: Mutex::new(label),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn local_nonstreaming(&self) -> String {
        self.inner
            .local_nonstreaming
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "CapsLock".to_string())
    }

    pub fn set_local_nonstreaming(&self, label: &str) -> Result<String, String> {
        let normalized = validate_hotkey_label(label).map_err(|e| e.to_string())?;
        let normalized = normalize_label(&normalized);
        if let Ok(mut guard) = self.inner.local_nonstreaming.lock() {
            *guard = normalized.clone();
        }
        let file = HotkeyUserFile {
            local_nonstreaming: Some(normalized.clone()),
        };
        if let Err(error) = save_file(&self.inner.path, &file) {
            return Err(format!("保存失败: {error}"));
        }
        info!(hotkey = %normalized, "voice hotkey saved (restart required)");
        Ok(normalized)
    }

    /// Apply override onto profiles before HotkeyMonitor starts.
    pub fn apply_to_profiles(&self, profiles: &mut ProfileConfigs) {
        let label = self.local_nonstreaming();
        profiles.local_nonstreaming.hotkey = label.clone();
        // CapsLock / Alt+Z / MouseX1 / MouseX2 swallow OS Back/Forward or key events.
        profiles.local_nonstreaming.suppress_key = hotkey_supports_suppress(&label);
        info!(
            hotkey = %label,
            suppress_key = profiles.local_nonstreaming.suppress_key,
            "applied user voice hotkey"
        );
    }
}

fn normalize_label(label: &str) -> String {
    // Canonical display form via re-parse pretty tokens.
    parse_hotkey_label(label).unwrap_or_else(|_| label.trim().to_string())
}

fn load_file(path: &Path) -> HotkeyUserFile {
    if !path.exists() {
        return HotkeyUserFile::default();
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).unwrap_or_else(|error| {
            warn!(error = %error, path = %path.display(), "parse hotkey-user.toml failed");
            HotkeyUserFile::default()
        }),
        Err(error) => {
            warn!(error = %error, path = %path.display(), "read hotkey-user.toml failed");
            HotkeyUserFile::default()
        }
    }
}

fn save_file(path: &Path, file: &HotkeyUserFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(file)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_mouse_side_button() {
        let dir = std::env::temp_dir().join(format!("ainput-hk-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hotkey-user.toml");
        let ctrl = HotkeyUserController::load_or_default(path.clone(), "CapsLock");
        assert_eq!(ctrl.local_nonstreaming(), "CapsLock");
        let saved = ctrl.set_local_nonstreaming("MouseX2").expect("save");
        assert_eq!(saved, "MouseX2");
        let reloaded = HotkeyUserController::load_or_default(path, "CapsLock");
        assert_eq!(reloaded.local_nonstreaming(), "MouseX2");
        let _ = std::fs::remove_dir_all(dir);
    }
}
