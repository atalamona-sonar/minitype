//! Small macOS integrations: feedback sounds, muting speakers while recording,
//! and launch at login.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub fn sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .spawn();
}

fn osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript").args(["-e", script]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// True while minitype muted the output (so we only unmute what we muted).
static WE_MUTED: AtomicBool = AtomicBool::new(false);

/// Mutes system output, after `delay` so a start sound can still be heard.
pub fn mute_output(delay: Duration) {
    thread::spawn(move || {
        thread::sleep(delay);
        if osascript("output muted of (get volume settings)").as_deref() == Some("false") {
            osascript("set volume output muted true");
            WE_MUTED.store(true, Ordering::SeqCst);
        }
    });
}

/// Restores output if we muted it, then plays `then_sound` if given.
pub fn unmute_output(then_sound: Option<&'static str>) {
    thread::spawn(move || {
        if WE_MUTED.swap(false, Ordering::SeqCst) {
            osascript("set volume output muted false");
        }
        if let Some(s) = then_sound {
            sound(s);
        }
    });
}

const AGENT_LABEL: &str = "com.atalamona.minitype";

fn agent_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/LaunchAgents").join(format!("{AGENT_LABEL}.plist"))
}

/// Adds or removes a per-user LaunchAgent that starts this app at login.
pub fn set_launch_at_login(on: bool) -> Result<(), String> {
    let path = agent_path();
    if !on {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{AGENT_LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        exe.display()
    );
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, plist).map_err(|e| e.to_string())
}
