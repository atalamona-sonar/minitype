//! minitype: a minimal menu bar dictation app for macOS.
//! Hold (or toggle) a key, speak, and the text lands in the focused app.
//! A small overlay shows a live preview while you talk. Speech is transcribed
//! locally (Parakeet or Whisper); only the one-time model download uses the network.

mod audio;
mod hotkey;
mod models;
mod parakeet;
mod paste;
mod settings;
mod settings_window;
mod stt;
mod system;
mod ui;

use std::fs;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use audio::Recording;
use hotkey::KeyEvent;
use settings::{Mode, Output, Overlay, Settings, data_dir};
use settings_window::SettingsWindow;
use stt::Engine;
use ui::{Look, Pill};

/// Recordings shorter than this (in 16 kHz samples) are treated as accidental taps.
const MIN_SAMPLES: usize = audio::WHISPER_RATE as usize * 3 / 10;
const PREVIEW_EVERY: Duration = Duration::from_millis(500);
/// The preview only re-transcribes the most recent audio, to stay fast.
const PREVIEW_WINDOW_SECS: u32 = 20;

/// What the transcription worker needs to know from the settings.
#[derive(Clone)]
struct EngineConfig {
    model: String,
    language: String,
    unload_after_mins: u32,
}

impl From<&Settings> for EngineConfig {
    fn from(s: &Settings) -> Self {
        Self { model: s.model.clone(), language: s.language.clone(), unload_after_mins: s.unload_after_mins }
    }
}

enum Job {
    Configure(EngineConfig),
    /// Load the model now (a recording just started) if it was unloaded.
    Warm,
    Preview(Vec<f32>),
    Final(Vec<f32>, Output),
}

#[derive(Clone, PartialEq)]
enum EngineStatus {
    Downloading(u32),
    Loading,
    Ready,
    Unloaded,
    Error(String),
}

enum UserEvent {
    Key(KeyEvent),
    Menu(MenuEvent),
    Engine(EngineStatus),
    Preview(String),
    Done,
    Ipc(String),
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Recording,
    Busy,
}

fn status_text(e: &EngineStatus, model: &str) -> String {
    let name = models::find(model).name;
    match e {
        EngineStatus::Downloading(p) => format!("Downloading {name}... {p}%"),
        EngineStatus::Loading => format!("Loading {name}..."),
        EngineStatus::Ready => format!("{name} ready"),
        EngineStatus::Unloaded => format!("{name} (unloaded)"),
        EngineStatus::Error(msg) => format!("Error: {msg}"),
    }
}

/// The transcription worker: owns the model, handles one job at a time.
fn worker(rx: mpsc::Receiver<Job>, proxy: EventLoopProxy<UserEvent>, mut cfg: EngineConfig) {
    let root = data_dir();
    let send = |e| {
        let _ = proxy.send_event(e);
    };
    let mut engine: Option<Engine> = None;

    let ensure = |engine: &mut Option<Engine>, cfg: &EngineConfig| -> bool {
        if engine.is_some() {
            return true;
        }
        let model = models::find(&cfg.model);
        if !model.is_downloaded(&root) {
            let progress = |p| send(UserEvent::Engine(EngineStatus::Downloading(p)));
            if let Err(e) = model.download(&root, &progress) {
                send(UserEvent::Engine(EngineStatus::Error(e)));
                return false;
            }
        }
        send(UserEvent::Engine(EngineStatus::Loading));
        match Engine::load(model, &root) {
            Ok(e) => {
                *engine = Some(e);
                send(UserEvent::Engine(EngineStatus::Ready));
                true
            }
            Err(e) => {
                send(UserEvent::Engine(EngineStatus::Error(e)));
                false
            }
        }
    };

    ensure(&mut engine, &cfg);
    loop {
        let job = if engine.is_some() && cfg.unload_after_mins > 0 {
            match rx.recv_timeout(Duration::from_secs(u64::from(cfg.unload_after_mins) * 60)) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => {
                    engine = None;
                    send(UserEvent::Engine(EngineStatus::Unloaded));
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };

        match job {
            Job::Configure(new) => {
                let reload = new.model != cfg.model;
                cfg = new;
                if reload {
                    engine = None;
                    ensure(&mut engine, &cfg);
                }
            }
            Job::Warm => {
                ensure(&mut engine, &cfg);
            }
            Job::Preview(samples) => {
                // Previews never trigger a load; they just skip if there's no model.
                let text = engine
                    .as_mut()
                    .and_then(|e| e.transcribe(&samples, &cfg.language).ok())
                    .unwrap_or_default();
                send(UserEvent::Preview(text));
            }
            Job::Final(samples, output) => {
                if ensure(&mut engine, &cfg)
                    && let Some(e) = engine.as_mut()
                {
                    match e.transcribe(&samples, &cfg.language) {
                        Ok(text) if !text.is_empty() => {
                            let res = match output {
                                Output::Paste => paste::paste(&text),
                                Output::Clipboard => paste::copy(&text),
                            };
                            if let Err(e) = res {
                                eprintln!("[output] {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("[stt] {e}"),
                    }
                }
                send(UserEvent::Done);
            }
        }
    }
}

/// The first version kept the Whisper model directly in the data dir.
fn migrate_old_layout() {
    let root = data_dir();
    let old = root.join("ggml-small.bin");
    if old.exists() {
        let dir = models::find("whisper-small").dir(&root);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::rename(&old, dir.join("ggml-small.bin"));
    }
    let _ = fs::remove_file(root.join("mode"));
    let _ = fs::remove_file(root.join("preview"));
}

/// Debug helper: `minitype --transcribe file.wav [model-id]` prints the transcript.
fn transcribe_file(path: &str, model_id: &str) -> Result<(), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / scale).collect()
        }
    };
    let mono: Vec<f32> = samples
        .chunks(usize::from(spec.channels).max(1))
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect();
    let audio = audio::resample_to_whisper(&mono, spec.sample_rate);
    let model = models::find(model_id);
    let root = data_dir();
    if !model.is_downloaded(&root) {
        model.download(&root, &|p| eprint!("\rdownloading {p}%"))?;
        eprintln!();
    }
    let mut engine = Engine::load(model, &root)?;
    let started = Instant::now();
    let text = engine.transcribe(&audio, "auto")?;
    eprintln!("[{}] {:.2}s for {:.1}s of audio", model.id, started.elapsed().as_secs_f32(),
              audio.len() as f32 / audio::WHISPER_RATE as f32);
    println!("{text}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--transcribe") {
        let path = args.get(2).map_or("", String::as_str);
        let model = args.get(3).map_or(models::DEFAULT, String::as_str);
        if let Err(e) = transcribe_file(path, model) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    migrate_old_layout();
    let first_run = !data_dir().join("settings.json").exists();
    let mut settings = Settings::load();
    settings.save();
    hotkey::configure(settings.key, settings.esc_cancels);

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // Menu bar app: no Dock icon, no app switcher entry.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let proxy = event_loop.create_proxy();
    hotkey::start(move |e| {
        let _ = proxy.send_event(UserEvent::Key(e));
    });
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let proxy = event_loop.create_proxy();
    let cfg = EngineConfig::from(&settings);
    thread::spawn(move || worker(job_rx, proxy, cfg));

    let status_item = MenuItem::new("Starting...", false, None);
    let settings_item = MenuItem::new("Settings...", true, None);
    let quit_item = MenuItem::new("Quit minitype", true, None);
    let menu = Menu::new();
    menu.append_items(&[&status_item, &PredefinedMenuItem::separator(), &settings_item, &quit_item])
        .expect("menu");

    let proxy = event_loop.create_proxy();
    let mut tray: Option<TrayIcon> = None;
    let mut pill: Option<Pill> = None;
    let mut settings_win: Option<SettingsWindow> = None;
    let mut state = State::Idle;
    let mut engine = EngineStatus::Loading;
    let mut recording: Option<Recording> = None;
    let mut preview_in_flight = false;
    let mut preview_text = String::new();

    event_loop.run(move |event, target, control_flow| {
        let mtm = MainThreadMarker::new().expect("event loop runs on the main thread");
        let mut redraw = false;
        let mut open_settings = false;

        match event {
            Event::NewEvents(StartCause::Init) => {
                pill = Some(Pill::new(mtm));
                open_settings = first_run || !settings.start_hidden;
                redraw = true;
            }
            // Launching minitype again while it runs: show settings (the way back
            // when the menu bar icon is hidden).
            Event::Reopen { .. } => open_settings = true,
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(r) = &recording
                    && settings.live_preview
                    && settings.overlay != Overlay::Off
                    && !preview_in_flight
                {
                    let samples = r.snapshot(PREVIEW_WINDOW_SECS);
                    if samples.len() >= audio::WHISPER_RATE as usize / 2 {
                        preview_in_flight = job_tx.send(Job::Preview(samples)).is_ok();
                    }
                }
            }
            Event::UserEvent(UserEvent::Preview(text)) => {
                preview_in_flight = false;
                if state != State::Idle && !text.is_empty() {
                    preview_text = text;
                    redraw = true;
                }
            }
            Event::UserEvent(UserEvent::Engine(e)) => {
                engine = e;
                redraw = true;
            }
            Event::UserEvent(UserEvent::Done) => {
                state = State::Idle;
                redraw = true;
            }
            Event::UserEvent(UserEvent::Key(KeyEvent::Cancel | KeyEvent::Interrupted))
                if recording.is_some() =>
            {
                // Esc, or the key was part of a shortcut (Fn+F5...): discard.
                recording = None;
                state = State::Idle;
                stop_feedback(&settings, "Funk");
                redraw = true;
            }
            Event::UserEvent(UserEvent::Key(e)) => {
                let start = match (settings.mode, e) {
                    (_, KeyEvent::Down) => recording.is_none(),
                    (Mode::Hold, KeyEvent::Up) => false,
                    _ => return,
                };
                if start {
                    let usable = matches!(engine, EngineStatus::Ready | EngineStatus::Unloaded);
                    if state != State::Idle || !usable {
                        return;
                    }
                    match Recording::start(&settings.microphone) {
                        Ok(r) => {
                            recording = Some(r);
                            preview_text.clear();
                            state = State::Recording;
                            if engine == EngineStatus::Unloaded {
                                let _ = job_tx.send(Job::Warm);
                            }
                            if settings.sounds {
                                system::sound("Tink");
                            }
                            if settings.mute_while_recording {
                                system::mute_output(Duration::from_millis(250));
                            }
                        }
                        Err(e) => eprintln!("[audio] {e}"),
                    }
                } else if let Some(r) = recording.take() {
                    stop_feedback(&settings, "Pop");
                    let samples = r.finish();
                    if samples.len() >= MIN_SAMPLES {
                        let _ = job_tx.send(Job::Final(samples, settings.output));
                        state = State::Busy;
                    } else {
                        state = State::Idle;
                    }
                }
                redraw = true;
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == *quit_item.id() {
                    system::unmute_output(None);
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                if e.id == *settings_item.id() {
                    open_settings = true;
                }
            }
            Event::UserEvent(UserEvent::Ipc(json)) => {
                if let Ok(new) = serde_json::from_str::<Settings>(&json) {
                    apply(&settings, &new, &job_tx);
                    settings = new;
                    settings.save();
                    redraw = true;
                }
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. }
                if settings_win.as_ref().is_some_and(|w| w.id() == window_id) =>
            {
                settings_win = None;
            }
            _ => {}
        }

        if open_settings {
            if settings_win.is_none() {
                let status = status_text(&engine, &settings.model);
                match SettingsWindow::open(target, proxy.clone(), UserEvent::Ipc, &settings, &audio::input_devices(), &status) {
                    Ok(w) => settings_win = Some(w),
                    Err(e) => eprintln!("[settings] {e}"),
                }
            }
            if let Some(w) = &settings_win {
                w.focus();
            }
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }

        if redraw {
            hotkey::set_recording(recording.is_some());
            let status = status_text(&engine, &settings.model);
            status_item.set_text(&status);
            if let Some(w) = &settings_win {
                w.set_status(&status);
            }

            // Menu bar icon: create or drop it to follow the setting.
            if settings.show_tray && tray.is_none() {
                tray = TrayIconBuilder::new()
                    .with_menu(Box::new(menu.clone()))
                    .with_tooltip("minitype")
                    .build()
                    .ok();
            } else if !settings.show_tray {
                tray = None;
            }
            if let Some(t) = &tray {
                let _ = t.set_icon(Some(icon(state, &engine)));
            }

            if let Some(p) = &pill {
                let top = settings.overlay == Overlay::Top;
                let text = if settings.live_preview { preview_text.as_str() } else { "" };
                match state {
                    _ if settings.overlay == Overlay::Off => p.hide(),
                    State::Recording if engine == EngineStatus::Unloaded => p.show(mtm, Look::Loading, "", top),
                    State::Recording => p.show(mtm, Look::Listening, text, top),
                    State::Busy => p.show(mtm, Look::Working, text, top),
                    State::Idle => p.hide(),
                }
            }
        }

        // Wake up regularly while recording, to refresh the preview.
        *control_flow = if recording.is_some() {
            ControlFlow::WaitUntil(Instant::now() + PREVIEW_EVERY)
        } else {
            ControlFlow::Wait
        };
    });
}

/// End-of-recording feedback: restore speakers, then play `sound` if enabled.
fn stop_feedback(s: &Settings, sound: &'static str) {
    let sound = s.sounds.then_some(sound);
    if s.mute_while_recording {
        system::unmute_output(sound);
    } else if let Some(name) = sound {
        system::sound(name);
    }
}

/// Pushes changed settings to the parts of the app that need them.
fn apply(old: &Settings, new: &Settings, jobs: &mpsc::Sender<Job>) {
    hotkey::configure(new.key, new.esc_cancels);
    if (old.model.as_str(), old.language.as_str(), old.unload_after_mins)
        != (new.model.as_str(), new.language.as_str(), new.unload_after_mins)
    {
        let _ = jobs.send(Job::Configure(EngineConfig::from(new)));
    }
    if old.launch_at_login != new.launch_at_login
        && let Err(e) = system::set_launch_at_login(new.launch_at_login)
    {
        eprintln!("[login] {e}");
    }
}

/// A small dot icon whose colour shows the state.
fn icon(state: State, engine: &EngineStatus) -> Icon {
    const SIZE: u32 = 32;
    let (rgb, filled) = match (state, engine) {
        (State::Recording, _) => ([230, 50, 50], true),
        (State::Busy, _) => ([240, 160, 30], true),
        (_, EngineStatus::Error(_)) => ([230, 50, 50], false),
        (_, EngineStatus::Downloading(_) | EngineStatus::Loading) => ([150, 150, 150], false),
        _ => ([120, 120, 120], true),
    };
    let c = (SIZE as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let inside = if filled { d <= 11.0 } else { (8.5..=11.0).contains(&d) };
            rgba.extend_from_slice(&rgb);
            rgba.push(if inside { 255 } else { 0 });
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("icon")
}
