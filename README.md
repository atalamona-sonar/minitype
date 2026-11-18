# minitype

Minimal macOS menu bar dictation. Hold a key (Fn by default), speak, and the text lands in the focused app. Transcription runs locally; only the one-time model download uses the network.

- **Models**: Parakeet V3 (default: fast, accurate, 25 European languages, auto-detected), Whisper Small, Whisper Large v3 Turbo (99 languages, language can be fixed).
- **Live preview**: a small pill shows what's been heard while you talk; the final text is a full pass over the whole recording.
- **Menu bar dot**: grey = ready, red = recording, orange = transcribing, hollow = downloading/loading (red hollow = error).

## Settings

Menu bar icon > Settings..., or open minitype again while it runs (the way back if you hide the menu bar icon).

| Group | Settings |
|---|---|
| Transcription | Model, language (Whisper only), output (paste or clipboard only), live preview |
| Shortcut | Key (Fn, Right Option/Command/Control, Left Option), hold to talk or press to toggle, Esc cancels |
| Audio | Microphone, sounds, mute speakers while recording |
| App | Overlay (bottom, top, off), unload model when idle, start hidden, launch at login, menu bar icon |

Stored in `~/Library/Application Support/minitype/settings.json`; models live in `models/` next to it.

## Build and install

Needs Rust and CMake (`brew install rustup cmake`) and a code-signing identity in your keychain.

```sh
./bundle.sh                      # signs with "SpeakType Local Signing"
./bundle.sh "My Signing Identity"
```

Installs to `~/Applications/minitype.app`. Always sign with the same identity, so macOS keeps the Microphone and Accessibility permissions across rebuilds.

Debug: `target/release/minitype --transcribe file.wav [parakeet-v3|whisper-small|whisper-turbo]` prints the transcript and timing.

## First run

1. The settings window opens and the default model downloads (~670 MB).
2. Allow Microphone when asked.
3. Allow Accessibility (System Settings > Privacy & Security > Accessibility), needed for the hotkey and for pasting.
4. If pressing Fn opens the emoji picker or switches input source, set System Settings > Keyboard > "Press 🌐 key to" to **Do Nothing**.

## Credits

- Parakeet TDT 0.6B v3 by NVIDIA, CC-BY-4.0; ONNX conversion by istupakov on Hugging Face. Run with our own decoder on ONNX Runtime.
- Whisper by OpenAI, MIT; run with whisper.cpp.
