//! Local speech-to-text with whisper.cpp. Downloads the model on first run.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::audio::WHISPER_RATE;

const MODEL_FILE: &str = "ggml-small.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";

pub struct Stt {
    _ctx: WhisperContext,
    state: WhisperState,
}

impl Stt {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = ensure_model(dir)?;
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
            .map_err(|e| format!("couldn't load model: {e}"))?;
        let state = ctx.create_state().map_err(|e| e.to_string())?;
        Ok(Self { _ctx: ctx, state })
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(None); // auto-detect
        params.set_n_threads(threads());
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // whisper.cpp rejects clips shorter than 1 s.
        let mut audio = samples.to_vec();
        let min = WHISPER_RATE as usize * 11 / 10;
        if audio.len() < min {
            audio.resize(min, 0.0);
        }
        self.state.full(params, &audio).map_err(|e| e.to_string())?;

        let text = self
            .state
            .as_iter()
            .filter_map(|s| s.to_str_lossy().ok().map(|t| t.into_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(clean(&text))
    }
}

/// Drops whisper's non-speech markers such as "[BLANK_AUDIO]" or "(music)".
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

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4)
}

fn ensure_model(dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join(MODEL_FILE);
    if path.exists() {
        return Ok(path);
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{MODEL_FILE}.part"));
    eprintln!("[stt] downloading {MODEL_URL}");
    let resp = ureq::get(MODEL_URL).call().map_err(|e| format!("download failed: {e}"))?;
    let mut reader = resp.into_body().into_reader();
    let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
    io::copy(&mut reader, &mut file).map_err(|e| format!("download failed: {e}"))?;
    fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
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
