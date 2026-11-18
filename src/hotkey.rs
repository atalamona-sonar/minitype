//! Push-to-talk on a single modifier key (Fn, Right Option...), plus Esc to cancel.
//!
//! macOS has no public API to register a lone modifier as a global shortcut, so we
//! watch keyboard events with a CoreGraphics event tap on a dedicated thread.
//! The tap needs Accessibility permission; until it is granted, creating it fails
//! and we keep retrying. If macOS disables the tap (it does that to slow taps),
//! we tear it down and create a fresh one.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};

use crate::settings::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down,
    Up,
    /// Another key went down while the hotkey was held: a regular shortcut.
    Interrupted,
    /// Esc while recording (and Esc-to-cancel is on).
    Cancel,
}

const ESC_KEYCODE: i64 = 53;

static KEY: AtomicU8 = AtomicU8::new(0);
static ESC_CANCELS: AtomicBool = AtomicBool::new(true);
static RECORDING: AtomicBool = AtomicBool::new(false);

fn key_from(n: u8) -> Key {
    match n {
        1 => Key::RightOption,
        2 => Key::RightCommand,
        3 => Key::RightControl,
        4 => Key::LeftOption,
        _ => Key::Fn,
    }
}

fn key_to(k: Key) -> u8 {
    match k {
        Key::Fn => 0,
        Key::RightOption => 1,
        Key::RightCommand => 2,
        Key::RightControl => 3,
        Key::LeftOption => 4,
    }
}

/// (virtual keycode, device-specific flag bit) for each key.
fn spec(k: Key) -> (i64, u64) {
    match k {
        Key::Fn => (63, 0x0080_0000),
        Key::RightOption => (61, 0x40),
        Key::RightCommand => (54, 0x10),
        Key::RightControl => (62, 0x2000),
        Key::LeftOption => (58, 0x20),
    }
}

/// Applies settings; safe to call at any time.
pub fn configure(key: Key, esc_cancels: bool) {
    KEY.store(key_to(key), Ordering::Relaxed);
    ESC_CANCELS.store(esc_cancels, Ordering::Relaxed);
}

/// Tells the listener whether a recording is running, so Esc is only taken then.
pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

/// Spawns the listener thread. `on_event` is called on that thread.
pub fn start(on_event: impl Fn(KeyEvent) + Send + 'static) {
    thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || loop {
            let held = Cell::new(false);
            let created = CGEventTap::with_enabled(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
                |_proxy, kind, event| {
                    let key = key_from(KEY.load(Ordering::Relaxed));
                    let (code, mask) = spec(key);
                    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                    match kind {
                        CGEventType::FlagsChanged if keycode == code => {
                            let down = event.get_flags().bits() & mask != 0;
                            if held.replace(down) != down {
                                on_event(if down { KeyEvent::Down } else { KeyEvent::Up });
                            }
                            // Keep Fn away from apps (terminals print junk otherwise).
                            // Real modifiers must pass, or apps lose track of them.
                            if key == Key::Fn { CallbackResult::Drop } else { CallbackResult::Keep }
                        }
                        CGEventType::KeyDown
                            if keycode == ESC_KEYCODE
                                && RECORDING.load(Ordering::Relaxed)
                                && ESC_CANCELS.load(Ordering::Relaxed) =>
                        {
                            held.set(false);
                            on_event(KeyEvent::Cancel);
                            CallbackResult::Drop
                        }
                        CGEventType::KeyDown => {
                            if held.replace(false) {
                                on_event(KeyEvent::Interrupted);
                            }
                            CallbackResult::Keep
                        }
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                            // Leave the run loop; the outer loop builds a new tap.
                            CFRunLoop::get_current().stop();
                            CallbackResult::Keep
                        }
                        _ => CallbackResult::Keep,
                    }
                },
                CFRunLoop::run_current,
            );
            if created.is_err() {
                // Most likely Accessibility isn't granted yet.
                thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("hotkey thread");
}
