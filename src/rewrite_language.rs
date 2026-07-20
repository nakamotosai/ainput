use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use tracing::warn;

use crate::config::{RewriteOutputLanguage, RewriteUserConfig, save_rewrite_user_config};

#[derive(Clone)]
pub struct RewriteLanguageController {
    inner: Arc<RewriteLanguageControllerInner>,
}

struct RewriteLanguageControllerInner {
    enabled: AtomicBool,
    streaming_enabled: AtomicBool,
    output_language: AtomicU8,
    user_config_path: PathBuf,
}

impl RewriteLanguageController {
    pub fn new(
        initial_enabled: bool,
        initial_streaming_enabled: bool,
        initial: RewriteOutputLanguage,
        user_config_path: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(RewriteLanguageControllerInner {
                enabled: AtomicBool::new(initial_enabled),
                streaming_enabled: AtomicBool::new(initial_streaming_enabled),
                output_language: AtomicU8::new(initial.as_u8()),
                user_config_path,
            }),
        }
    }

    pub fn rewrite_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn streaming_rewrite_enabled(&self) -> bool {
        self.inner.streaming_enabled.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> RewriteOutputLanguage {
        RewriteOutputLanguage::from_u8(self.inner.output_language.load(Ordering::Relaxed))
    }

    pub fn set_rewrite_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
        self.save_user_config();
    }

    pub fn set_streaming_rewrite_enabled(&self, enabled: bool) {
        self.inner
            .streaming_enabled
            .store(enabled, Ordering::Relaxed);
        self.save_user_config();
    }

    pub fn set_output_language(&self, language: RewriteOutputLanguage) {
        self.inner
            .output_language
            .store(language.as_u8(), Ordering::Relaxed);
        self.save_user_config();
    }

    fn save_user_config(&self) {
        let user = RewriteUserConfig {
            enabled: Some(self.rewrite_enabled()),
            streaming_enabled: Some(self.streaming_rewrite_enabled()),
            output_language: Some(self.current()),
        };
        if let Err(error) = save_rewrite_user_config(&self.inner.user_config_path, &user) {
            warn!(
                error = %error,
                path = %self.inner.user_config_path.display(),
                "save rewrite output language user config failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RewriteLanguageController;
    use crate::config::RewriteOutputLanguage;

    #[test]
    fn controller_persists_enabled_and_language_together() {
        let dir =
            std::env::temp_dir().join(format!("ainput-rewrite-controller-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("rewrite-user.toml");
        let controller = RewriteLanguageController::new(
            true,
            false,
            RewriteOutputLanguage::Chinese,
            path.clone(),
        );

        controller.set_rewrite_enabled(false);
        let raw = std::fs::read_to_string(&path).expect("read rewrite user config");
        assert!(raw.contains("enabled = false"));
        assert!(raw.contains("streaming_enabled = false"));
        assert!(raw.contains("output_language = \"chinese\""));

        controller.set_streaming_rewrite_enabled(true);
        let raw = std::fs::read_to_string(&path).expect("read rewrite user config");
        assert!(raw.contains("enabled = false"));
        assert!(raw.contains("streaming_enabled = true"));

        controller.set_output_language(RewriteOutputLanguage::English);
        let raw = std::fs::read_to_string(&path).expect("read rewrite user config");
        assert!(raw.contains("enabled = false"));
        assert!(raw.contains("streaming_enabled = true"));
        assert!(raw.contains("output_language = \"english\""));

        let _ = std::fs::remove_dir_all(dir);
    }
}
