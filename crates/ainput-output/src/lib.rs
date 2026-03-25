use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};

#[derive(Debug, Clone, Copy)]
pub enum OutputDelivery {
    DirectPaste,
    ClipboardOnly,
}

pub fn deliver_text(text: &str, prefer_direct_paste: bool) -> Result<OutputDelivery> {
    if prefer_direct_paste {
        match paste_via_clipboard(text) {
            Ok(()) => return Ok(OutputDelivery::DirectPaste),
            Err(error) => {
                tracing::warn!(error = %error, "direct paste failed, fallback to clipboard");
            }
        }
    }

    copy_to_clipboard(text)?;
    Ok(OutputDelivery::ClipboardOnly)
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("open clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("write text into clipboard")?;
    Ok(())
}

fn paste_via_clipboard(text: &str) -> Result<()> {
    copy_to_clipboard(text)?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| anyhow::anyhow!("create enigo output controller: {error}"))?;
    enigo.key(Key::Control, Press).context("press ctrl")?;
    enigo
        .key(Key::Unicode('v'), Click)
        .context("send v key")?;
    enigo.key(Key::Control, Release).context("release ctrl")?;
    thread::sleep(Duration::from_millis(80));

    Ok(())
}
