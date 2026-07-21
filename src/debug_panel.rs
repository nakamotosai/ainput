//! Public product: debug panel UI removed. Keep a no-op controller so the
//! voice pipeline can compile without a second window.

#[derive(Clone, Default)]
pub struct DebugPanelController;

impl DebugPanelController {
    pub fn is_enabled(&self) -> bool {
        false
    }

    pub fn display_result(&self, _text: impl AsRef<str>, _status: impl AsRef<str>) {}
}
