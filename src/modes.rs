#![allow(dead_code)]

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    StreamingAsr,
    WhisperZh,
    LocalNonstreaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceProfileId {
    StreamingDefault,
    WhisperCapslock,
    LocalNonstreaming,
}

impl VoiceProfileId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamingDefault => "streaming_default",
            Self::WhisperCapslock => "whisper_capslock",
            Self::LocalNonstreaming => "local_nonstreaming",
        }
    }
}

#[derive(Clone)]
pub struct ModeStore {
    inner: Arc<ModeState>,
}

struct ModeState {
    mode: AtomicU8,
}

impl ModeStore {
    pub fn new(mode: InputMode) -> Self {
        Self {
            inner: Arc::new(ModeState {
                mode: AtomicU8::new(mode_to_u8(mode)),
            }),
        }
    }

    pub fn get(&self) -> InputMode {
        u8_to_mode(self.inner.mode.load(Ordering::Relaxed))
    }

    pub fn set(&self, mode: InputMode) {
        self.inner.mode.store(mode_to_u8(mode), Ordering::Relaxed);
    }
}

impl Default for InputMode {
    fn default() -> Self {
        Self::LocalNonstreaming
    }
}

fn mode_to_u8(mode: InputMode) -> u8 {
    match mode {
        InputMode::StreamingAsr => 0,
        InputMode::WhisperZh => 1,
        InputMode::LocalNonstreaming => 2,
    }
}

fn u8_to_mode(value: u8) -> InputMode {
    match value {
        1 => InputMode::WhisperZh,
        2 => InputMode::LocalNonstreaming,
        _ => InputMode::StreamingAsr,
    }
}

#[cfg(test)]
mod tests {
    use super::{InputMode, ModeStore};

    #[test]
    fn defaults_to_local_nonstreaming() {
        let modes = ModeStore::new(InputMode::default());
        assert_eq!(modes.get(), InputMode::LocalNonstreaming);
    }

    #[test]
    fn mode_sets_between_registered_voice_modes() {
        let modes = ModeStore::new(InputMode::StreamingAsr);
        modes.set(InputMode::WhisperZh);
        assert_eq!(modes.get(), InputMode::WhisperZh);
        modes.set(InputMode::LocalNonstreaming);
        assert_eq!(modes.get(), InputMode::LocalNonstreaming);
        modes.set(InputMode::StreamingAsr);
        assert_eq!(modes.get(), InputMode::StreamingAsr);
    }
}
