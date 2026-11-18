//! Speech-to-text engines behind one interface: Parakeet (default) or Whisper.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::audio::WHISPER_RATE;
use crate::models::{Kind, Model};
use crate::parakeet::Parakeet;

pub enum Engine {
    Parakeet(Box<Parakeet>),
    Whisper(Box<Whisper>),
}

impl Engine {
    /// Loads `model` from disk; it must already be downloaded.
    pub fn load(model: &Model, root: &Path) -> Result<Self, String> {
        let dir = model.dir(root);
        Ok(match model.kind {
            Kind::Parakeet => Self::Parakeet(Box::new(Parakeet::load(&dir)?)),
            Kind::Whisper => Self::Whisper(Box::new(Whisper::load(&dir.join(model.files[0]))?)),
        })
    }

    /// `language` is "auto" or an ISO code; Parakeet always auto-detects.
    pub fn transcribe(&mut self, samples: &[f32], language: &str) -> Result<String, String> {
        // Both models dislike clips under a second; pad with silence.
        let mut audio = samples.to_vec();
        let min = WHISPER_RATE as usize * 11 / 10;
        if audio.len() < min {
            audio.resize(min, 0.0);
        }
        let text = match self {
            Self::Parakeet(p) => p.transcribe(&audio)?,
            Self::Whisper(w) => w.transcribe(&audio, language)?,
        };
        Ok(clean(&text))
    }
}

pub struct Whisper {
    _ctx: WhisperContext,
    state: WhisperState,
}

impl Whisper {
    fn load(path: &Path) -> Result<Self, String> {
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("couldn't load model: {e}"))?;
        let state = ctx.create_state().map_err(|e| e.to_string())?;
        Ok(Self { _ctx: ctx, state })
    }

    fn transcribe(&mut self, audio: &[f32], language: &str) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(if language == "auto" { None } else { Some(language) });
        params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        self.state.full(params, audio).map_err(|e| e.to_string())?;
        Ok(self
            .state
            .as_iter()
            .filter_map(|s| s.to_str_lossy().ok().map(|t| t.into_owned()))
            .collect::<Vec<_>>()
            .join(" "))
    }
}

/// Drops non-speech markers such as "[BLANK_AUDIO]" or "(music)".
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn strips_markers() {
        assert_eq!(clean(" [BLANK_AUDIO] "), "");
        assert_eq!(clean(" Hello (music) world. "), "Hello world.");
    }
}
