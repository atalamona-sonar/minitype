//! The settings window: a small native window hosting `settings.html` in the
//! system web view. Every change is sent back as the full settings JSON.

use serde_json::json;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebView, WebViewBuilder};

use crate::models::{self, Kind};
use crate::settings::{Settings, data_dir};

const HTML: &str = include_str!("settings.html");

pub struct SettingsWindow {
    window: Window,
    webview: WebView,
}

impl SettingsWindow {
    pub fn open<T: 'static>(
        target: &EventLoopWindowTarget<T>,
        proxy: EventLoopProxy<T>,
        to_event: fn(String) -> T,
        settings: &Settings,
        mics: &[String],
        status: &str,
    ) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("minitype")
            .with_inner_size(LogicalSize::new(440.0, 640.0))
            .with_resizable(false)
            .build(target)
            .map_err(|e| e.to_string())?;

        let root = data_dir();
        let model_list: Vec<_> = models::ALL
            .iter()
            .map(|m| {
                json!({
                    "id": m.id, "name": m.name, "blurb": m.blurb, "size_mb": m.size_mb,
                    "kind": if m.kind == Kind::Parakeet { "parakeet" } else { "whisper" },
                    "downloaded": m.is_downloaded(&root),
                })
            })
            .collect();
        let init = json!({ "settings": settings, "models": model_list, "mics": mics, "status": status });

        let webview = WebViewBuilder::new()
            .with_initialization_script(format!("window.__INIT = {init};"))
            .with_html(HTML)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(to_event(req.body().clone()));
            })
            .build(&window)
            .map_err(|e| e.to_string())?;

        Ok(Self { window, webview })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn set_status(&self, text: &str) {
        let arg = serde_json::to_string(text).unwrap_or_default();
        let _ = self.webview.evaluate_script(&format!("window.setStatus && setStatus({arg})"));
    }

    pub fn focus(&self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }
}
