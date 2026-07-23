//! Voice command wake phrase: "老蔡老蔡" → generate instead of dictation rewrite.
//! Toggle + editable system prompt via tray / loopback panel.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Primary wake phrase (after ASR normalization).
pub const WAKE_PHRASE: &str = "老蔡老蔡";

/// Optional ASR variants that should also trigger command mode.
const WAKE_VARIANTS: &[&str] = &["老蔡老蔡", "老菜老菜", "老财老财"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceCommand {
    /// Instruction with wake phrase stripped.
    pub instruction: String,
    /// Original ASR text.
    pub raw: String,
}

/// If `text` starts with a wake phrase, return the command body.
/// Returns `None` when this is ordinary dictation.
pub fn parse_voice_command(text: &str) -> Option<VoiceCommand> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    for wake in WAKE_VARIANTS {
        if let Some(rest) = strip_wake_from_original(trimmed, wake) {
            let instruction = rest
                .trim_start_matches(|c: char| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            ',' | '，' | '。' | '.' | '!' | '！' | '?' | '？' | ':' | '：' | '、'
                        )
                })
                .trim()
                .to_string();
            if instruction.is_empty() {
                return Some(VoiceCommand {
                    instruction: "请简短介绍你能做什么。".to_string(),
                    raw: trimmed.to_string(),
                });
            }
            return Some(VoiceCommand {
                instruction,
                raw: trimmed.to_string(),
            });
        }
    }
    None
}

pub fn is_voice_command(text: &str) -> bool {
    parse_voice_command(text).is_some()
}

pub fn command_system_prompt() -> &'static str {
    "你是嵌入语音输入法的本地助手。用户用语音下达指令。请直接完成指令，只输出最终正文结果。\n\n规则：\n- 不解释、不加前后缀、不加引号、不用 Markdown 代码块\n- 写文章/段落时直接给出正文\n- 翻译时只输出译文\n- 改写/润色时只输出改写后正文\n- 回答问题用简洁中文\n- 不要复述用户指令本身"
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VoiceCommandUserConfig {
    /// When false, wake phrase is treated as ordinary dictation.
    pub enabled: Option<bool>,
    /// Custom system prompt; empty → default `command_system_prompt()`.
    pub custom_prompt: Option<String>,
}

#[derive(Clone)]
pub struct VoiceCommandController {
    inner: Arc<VoiceCommandControllerInner>,
}

struct VoiceCommandControllerInner {
    enabled: AtomicBool,
    custom_prompt: Mutex<String>,
    path: PathBuf,
}

impl VoiceCommandController {
    pub fn load_or_default(path: PathBuf) -> Self {
        let (enabled, custom) = load_user_config(&path);
        Self {
            inner: Arc::new(VoiceCommandControllerInner {
                enabled: AtomicBool::new(enabled),
                custom_prompt: Mutex::new(custom),
                path,
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
        self.save();
        info!(enabled, "voice command enabled toggled");
    }

    pub fn custom_prompt(&self) -> String {
        self.inner
            .custom_prompt
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn set_custom_prompt(&self, prompt: &str) {
        let text = prompt.trim().to_string();
        if let Ok(mut guard) = self.inner.custom_prompt.lock() {
            *guard = text;
        }
        self.save();
        info!("voice command custom prompt saved");
    }

    /// System prompt used for voice-command generation.
    pub fn active_prompt(&self) -> String {
        let custom = self.custom_prompt();
        if custom.trim().is_empty() {
            command_system_prompt().to_string()
        } else {
            custom
        }
    }

    fn save(&self) {
        let custom = self.custom_prompt();
        let cfg = VoiceCommandUserConfig {
            enabled: Some(self.enabled()),
            custom_prompt: if custom.is_empty() {
                None
            } else {
                Some(custom)
            },
        };
        if let Err(error) = save_user_config(&self.inner.path, &cfg) {
            warn!(
                error = %error,
                path = %self.inner.path.display(),
                "save voice command config failed"
            );
        }
    }
}

fn strip_wake_from_original<'a>(text: &'a str, wake: &str) -> Option<&'a str> {
    let wake_chars: Vec<char> = wake.chars().collect();
    let mut wi = 0usize;
    let mut end_byte = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            // Allow spaces inside the wake phrase (ASR often inserts them).
            if wi > 0 && wi < wake_chars.len() {
                end_byte = idx + ch.len_utf8();
                continue;
            }
            if wi == 0 {
                // Leading whitespace already trimmed; treat as miss.
                return None;
            }
        }
        if wi < wake_chars.len() && ch == wake_chars[wi] {
            wi += 1;
            end_byte = idx + ch.len_utf8();
            if wi == wake_chars.len() {
                return Some(&text[end_byte..]);
            }
        } else {
            return None;
        }
    }
    None
}

fn load_user_config(path: &Path) -> (bool, String) {
    // Default: feature on.
    if !path.exists() {
        return (true, String::new());
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => match toml::from_str::<VoiceCommandUserConfig>(&raw) {
            Ok(cfg) => {
                let enabled = cfg.enabled.unwrap_or(true);
                let custom = cfg.custom_prompt.unwrap_or_default();
                (enabled, custom)
            }
            Err(error) => {
                warn!(error = %error, path = %path.display(), "parse voice command config failed");
                (true, String::new())
            }
        },
        Err(error) => {
            warn!(error = %error, path = %path.display(), "read voice command config failed");
            (true, String::new())
        }
    }
}

fn save_user_config(path: &Path, cfg: &VoiceCommandUserConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(cfg)?;
    std::fs::write(path, raw)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wake_and_strips() {
        let cmd = parse_voice_command("老蔡老蔡，帮我写篇100字的文章").expect("cmd");
        assert_eq!(cmd.instruction, "帮我写篇100字的文章");
    }

    #[test]
    fn detects_without_comma() {
        let cmd = parse_voice_command("老蔡老蔡帮我翻译成英文：你好").expect("cmd");
        assert!(cmd.instruction.contains("翻译"));
    }

    #[test]
    fn ignores_ordinary_dictation() {
        assert!(parse_voice_command("我现在去买菜").is_none());
        assert!(parse_voice_command("老蔡今天在吗").is_none());
    }

    #[test]
    fn allows_spaces_between_chars() {
        let cmd = parse_voice_command("老 蔡 老 蔡，写一首诗").expect("cmd");
        assert_eq!(cmd.instruction, "写一首诗");
    }

    #[test]
    fn bare_wake_gets_default_instruction() {
        let cmd = parse_voice_command("老蔡老蔡").expect("cmd");
        assert!(cmd.instruction.contains("介绍"));
    }

    #[test]
    fn controller_enabled_and_prompt_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ainput-vc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("voice-command.toml");
        let ctrl = VoiceCommandController::load_or_default(path.clone());
        assert!(ctrl.enabled());
        assert!(ctrl.active_prompt().contains("本地助手"));

        ctrl.set_enabled(false);
        assert!(!ctrl.enabled());
        ctrl.set_custom_prompt("只输出一行短诗。");
        assert_eq!(ctrl.active_prompt(), "只输出一行短诗。");

        let reloaded = VoiceCommandController::load_or_default(path);
        assert!(!reloaded.enabled());
        assert_eq!(reloaded.active_prompt(), "只输出一行短诗。");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_custom_falls_back_to_default() {
        let dir = std::env::temp_dir().join(format!("ainput-vc-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("voice-command.toml");
        let ctrl = VoiceCommandController::load_or_default(path);
        ctrl.set_custom_prompt("   ");
        assert_eq!(ctrl.active_prompt(), command_system_prompt());
        let _ = std::fs::remove_dir_all(dir);
    }
}
