# minitype

Minimal macOS menu bar dictation. Press the Fn (Globe) key, speak, and the text is pasted into the focused app. Transcription runs locally (whisper.cpp, multilingual `small` model, Metal GPU).

- **Hold to talk** (default): hold Fn while speaking, release to paste. Fn plus another key (Fn+F5, Fn+arrows) cancels the recording, so normal Fn shortcuts still work.
- **Press to toggle**: press once to start, again to stop. Switch modes in the menu bar menu; the choice is saved.
- **Live preview**: while you talk, a small pill above the Dock shows what's been heard so far (refreshed every 0.5 s). The pasted text is a final full-accuracy pass. Turn it off in the menu.
- The menu bar dot shows the state: grey = ready, red = recording, orange = transcribing, hollow = loading model or error.

## Build and install

Needs Rust and CMake (`brew install rustup cmake`) and a code-signing identity in your keychain.

```sh
./bundle.sh                      # signs with "SpeakType Local Signing"
./bundle.sh "My Signing Identity"
```

Installs to `~/Applications/minitype.app`. Always sign with the same identity, so macOS keeps the Microphone and Accessibility permissions across rebuilds.

## First run

1. Wait for the dot to turn solid grey. The first launch downloads the ~470 MB model to `~/Library/Application Support/minitype/`.
2. Allow Microphone when asked.
3. Allow Accessibility (System Settings > Privacy & Security > Accessibility), which is needed to listen for Fn and to paste.
4. If pressing Fn opens the emoji picker or switches input source, set System Settings > Keyboard > "Press 🌐 key to" to **Do Nothing**.
