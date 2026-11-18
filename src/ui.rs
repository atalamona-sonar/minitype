//! The pill: a small dark floating bar above the Dock that shows what minitype
//! hears while you talk. It never takes focus or mouse clicks, so typing and
//! pasting keep going to the app you're working in. Main thread only.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSLineBreakMode, NSPanel, NSScreen, NSStatusWindowLevel,
    NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 40.0;
/// Gap between the pill and the bottom of the usable screen (top of the Dock).
const BOTTOM_GAP: f64 = 28.0;
/// Characters of live text shown; older words scroll off to the left.
const MAX_CHARS: usize = 70;

pub enum Look {
    Listening,
    Working,
}

pub struct Pill {
    panel: Retained<NSPanel>,
    dot: Retained<NSTextField>,
    text: Retained<NSTextField>,
}

impl Pill {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: we keep our own Retained reference, so AppKit must not free it on close.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
        panel.setIgnoresMouseEvents(true);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);

        // Dark frosted background with fully rounded ends.
        let blur = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        blur.setMaterial(NSVisualEffectMaterial::HUDWindow);
        blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        blur.setState(NSVisualEffectState::Active);
        blur.setWantsLayer(true);
        if let Some(layer) = blur.layer() {
            // SAFETY: CALayer responds to these setters; arguments are CGFloat and BOOL.
            unsafe {
                let layer: &AnyObject = &layer;
                let _: () = msg_send![layer, setCornerRadius: HEIGHT / 2.0];
                let _: () = msg_send![layer, setMasksToBounds: true];
            }
        }

        let dot = label(mtm, "●", 12.0);
        dot.setFrameOrigin(NSPoint::new(16.0, (HEIGHT - 16.0) / 2.0));
        dot.setFrameSize(NSSize::new(14.0, 16.0));

        let text = label(mtm, "", 13.0);
        text.setTextColor(Some(&NSColor::whiteColor()));
        text.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
        text.setFrameOrigin(NSPoint::new(36.0, (HEIGHT - 17.0) / 2.0));
        text.setFrameSize(NSSize::new(WIDTH - 36.0 - 18.0, 17.0));

        blur.addSubview(&dot);
        blur.addSubview(&text);
        let content: &NSView = &blur;
        panel.setContentView(Some(content));

        Self { panel, dot, text }
    }

    pub fn show(&self, mtm: MainThreadMarker, look: Look, text: &str) {
        let (color, fallback) = match look {
            Look::Listening => (NSColor::systemRedColor(), "Listening..."),
            Look::Working => (NSColor::systemOrangeColor(), "Transcribing..."),
        };
        self.dot.setTextColor(Some(&color));
        let shown = if text.is_empty() { fallback.to_string() } else { tail(text, MAX_CHARS) };
        self.text.setStringValue(&NSString::from_str(&shown));

        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let area = screen.visibleFrame();
            let x = area.origin.x + (area.size.width - WIDTH) / 2.0;
            let y = area.origin.y + BOTTOM_GAP;
            self.panel.setFrameOrigin(NSPoint::new(x, y));
        }
        self.panel.orderFrontRegardless();
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }
}

fn label(mtm: MainThreadMarker, s: &str, size: f64) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(s), mtm);
    field.setFont(Some(&NSFont::systemFontOfSize(size)));
    field
}

/// Last `max` characters, cut at a word boundary, with a leading ellipsis.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let cut: String = text.chars().skip(count - max).collect();
    let cut = cut.split_once(' ').map_or(cut.as_str(), |(_, rest)| rest);
    format!("...{cut}")
}

#[cfg(test)]
mod tests {
    use super::tail;

    #[test]
    fn keeps_the_end() {
        assert_eq!(tail("short", 10), "short");
        assert_eq!(tail("one two three four", 10), "...four");
    }
}
