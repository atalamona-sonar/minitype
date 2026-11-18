//! User settings, stored as JSON in the data dir. Unknown or missing fields fall
//! back to defaults, so older files keep working when settings are added.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Fn,
    RightOption,
    RightCommand,
    RightControl,
    LeftOption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Paste into the focused app, then restore the clipboard.
    Paste,
    /// Only put the text on the clipboard.
    Clipboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlay {
    Bottom,
    Top,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model: String,
    /// "auto" or an ISO code such as "it". Parakeet always auto-detects.
    pub language: String,
    pub key: Key,
    pub mode: Mode,
    pub esc_cancels: bool,
    /// Input device name; empty means the system default.
    pub microphone: String,
    pub sounds: bool,
    pub mute_while_recording: bool,
    pub output: Output,
    pub overlay: Overlay,
    pub live_preview: bool,
    /// Free the model's memory after this many idle minutes; 0 keeps it loaded.
    pub unload_after_mins: u32,
    pub start_hidden: bool,
    pub launch_at_login: bool,
    pub show_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: crate::models::DEFAULT.into(),
            language: "auto".into(),
            key: Key::Fn,
            mode: Mode::Hold,
            esc_cancels: true,
            microphone: String::new(),
            sounds: true,
            mute_while_recording: false,
            output: Output::Paste,
            overlay: Overlay::Bottom,
            live_preview: true,
            unload_after_mins: 0,
            start_hidden: true,
            launch_at_login: false,
            show_tray: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/minitype")
}

fn path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(data_dir());
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path(), json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_uses_defaults() {
        let s: Settings = serde_json::from_str(r#"{"mode":"toggle","key":"right_option"}"#).unwrap();
        assert_eq!(s.mode, Mode::Toggle);
        assert_eq!(s.key, Key::RightOption);
        assert!(s.sounds);
        assert_eq!(s.language, "auto");
    }
}
