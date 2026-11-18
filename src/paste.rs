//! Types text into the focused app: clipboard + Cmd+V, then restores the clipboard.

use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// macOS virtual key code for "V" (layout independent).
const KEY_V: u32 = 9;

/// Leaves `text` on the clipboard without pasting.
pub fn copy(text: &str) -> Result<(), String> {
    Clipboard::new()
        .and_then(|mut c| c.set_text(text))
        .map_err(|e| e.to_string())
}

pub fn paste(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    let saved = clipboard.get_text().ok();
    clipboard.set_text(text).map_err(|e| e.to_string())?;

    // Let the user release the hotkey's modifiers first.
    thread::sleep(Duration::from_millis(150));
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Meta, Direction::Press).map_err(|e| e.to_string())?;
    let click = enigo.key(Key::Other(KEY_V), Direction::Click);
    enigo.key(Key::Meta, Direction::Release).map_err(|e| e.to_string())?;
    click.map_err(|e| e.to_string())?;

    // Give the target app time to read the clipboard, then put the old text back
    // unless the user copied something else meanwhile.
    thread::sleep(Duration::from_millis(300));
    if clipboard.get_text().ok().as_deref() == Some(text) {
        match saved {
            Some(old) => clipboard.set_text(old),
            None => clipboard.clear(),
        }
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
