//! Fn (Globe) key as a push-to-talk key.
//!
//! macOS has no public API to register a lone modifier as a global shortcut, so we
//! watch keyboard events with a CoreGraphics event tap on a dedicated thread.
//! The tap needs Accessibility permission; until it is granted, creating it fails
//! and we keep retrying. If macOS disables the tap (it does that to slow taps),
//! we tear it down and create a fresh one.

use std::cell::Cell;
use std::thread;
use std::time::Duration;

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnEvent {
    Down,
    Up,
    /// Another key went down while Fn was held: a regular Fn shortcut.
    Interrupted,
}

/// Virtual keycode of the Fn key (kVK_Function).
const FN_KEYCODE: i64 = 63;

/// Spawns the listener thread. `on_event` is called on that thread.
pub fn start(on_event: impl Fn(FnEvent) + Send + 'static) {
    thread::Builder::new()
        .name("fn-key".into())
        .spawn(move || loop {
            let held = Cell::new(false);
            let created = CGEventTap::with_enabled(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
                |_proxy, kind, event| match kind {
                    CGEventType::FlagsChanged
                        if event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE)
                            == FN_KEYCODE =>
                    {
                        let down = event.get_flags().contains(CGEventFlags::CGEventFlagSecondaryFn);
                        if held.replace(down) != down {
                            on_event(if down { FnEvent::Down } else { FnEvent::Up });
                        }
                        // Keep Fn away from the focused app (terminals print junk otherwise).
                        CallbackResult::Drop
                    }
                    CGEventType::KeyDown => {
                        if held.replace(false) {
                            on_event(FnEvent::Interrupted);
                        }
                        CallbackResult::Keep
                    }
                    CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                        // Leave the run loop; the outer loop builds a new tap.
                        CFRunLoop::get_current().stop();
                        CallbackResult::Keep
                    }
                    _ => CallbackResult::Keep,
                },
                CFRunLoop::run_current,
            );
            if created.is_err() {
                // Most likely Accessibility isn't granted yet.
                thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("fn-key thread");
}
