//! The models minitype can use, and their one-time download.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT: &str = "parakeet-v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Parakeet,
    Whisper,
}

pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    pub kind: Kind,
    /// Base URL; each file is fetched from `{base}/{file}`.
    pub base: &'static str,
    pub files: &'static [&'static str],
    pub size_mb: u32,
}

pub const ALL: &[Model] = &[
    Model {
        id: "parakeet-v3",
        name: "Parakeet V3",
        blurb: "Fast and accurate. 25 European languages, auto-detected.",
        kind: Kind::Parakeet,
        base: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main",
        files: &["vocab.txt", "nemo128.onnx", "decoder_joint-model.int8.onnx", "encoder-model.int8.onnx"],
        size_mb: 670,
    },
    Model {
        id: "whisper-small",
        name: "Whisper Small",
        blurb: "99 languages. Lighter, less accurate.",
        kind: Kind::Whisper,
        base: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main",
        files: &["ggml-small.bin"],
        size_mb: 470,
    },
    Model {
        id: "whisper-turbo",
        name: "Whisper Large v3 Turbo",
        blurb: "99 languages. Most accurate, slowest.",
        kind: Kind::Whisper,
        base: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main",
        files: &["ggml-large-v3-turbo-q5_0.bin"],
        size_mb: 550,
    },
];

/// HTTP client that trusts what macOS trusts (Keychain roots), so downloads
/// also work behind company TLS-inspecting proxies.
fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .new_agent()
}

pub fn find(id: &str) -> &'static Model {
    ALL.iter().find(|m| m.id == id).unwrap_or(&ALL[0])
}

impl Model {
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join("models").join(self.id)
    }

    pub fn is_downloaded(&self, root: &Path) -> bool {
        let dir = self.dir(root);
        self.files.iter().all(|f| dir.join(f).exists())
    }

    /// Downloads missing files. `progress` gets 0..=100 for the whole model.
    pub fn download(&self, root: &Path, progress: &dyn Fn(u32)) -> Result<PathBuf, String> {
        let dir = self.dir(root);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let total = u64::from(self.size_mb) * 1_000_000;
        let mut done: u64 = 0;
        for file in self.files {
            let dest = dir.join(file);
            if dest.exists() {
                done += fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                continue;
            }
            let url = format!("{}/{}", self.base, file);
            let resp = agent().get(&url).call().map_err(|e| format!("download failed: {e}"))?;
            let mut reader = resp.into_body().into_reader();
            let part = dir.join(format!("{file}.part"));
            let mut out = fs::File::create(&part).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 16];
            let mut last = u32::MAX;
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(format!("download failed: {e}")),
                };
                out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                done += n as u64;
                let pct = ((done * 100) / total).min(99) as u32;
                if pct != last {
                    last = pct;
                    progress(pct);
                }
            }
            fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        }
        progress(100);
        Ok(dir)
    }
}
