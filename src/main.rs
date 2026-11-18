//! minitype: a minimal menu bar dictation app.
//! Hold (or toggle) the Fn key, speak, and the text is pasted into the focused app.
//! While you talk, a small pill shows a live preview of what's been heard.
//! Speech is transcribed locally with whisper.cpp; nothing leaves the machine
//! except the one-time model download.

mod audio;
mod fnkey;
mod paste;
mod stt;
mod ui;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fnkey::FnEvent;
use objc2::MainThreadMarker;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use audio::Recording;
use ui::{Look, Pill};

/// Recordings shorter than this (in 16 kHz samples) are treated as accidental taps.
const MIN_SAMPLES: usize = audio::WHISPER_RATE as usize * 3 / 10;
/// How often the live preview is refreshed while recording.
const PREVIEW_EVERY: Duration = Duration::from_millis(500);
/// The preview only re-transcribes the most recent audio, to stay fast.
const PREVIEW_WINDOW_SECS: u32 = 20;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Hold,
    Toggle,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Loading,
    Idle,
    Recording,
    Busy,
    Error,
}

enum Job {
    Preview(Vec<f32>),
    Final(Vec<f32>),
}

enum UserEvent {
    Hotkey(FnEvent),
    Menu(MenuEvent),
    State(State, Option<String>),
    Preview(String),
}

fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/minitype")
}

/// Settings are one tiny file each in the data dir.
fn read_setting(name: &str) -> Option<String> {
    fs::read_to_string(data_dir().join(name)).ok().map(|s| s.trim().to_string())
}

fn write_setting(name: &str, value: &str) {
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(data_dir().join(name), value);
}

fn sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .spawn();
}

fn main() {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // Menu bar only: no Dock icon, no app switcher entry.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let proxy = event_loop.create_proxy();
    fnkey::start(move |e| {
        let _ = proxy.send_event(UserEvent::Hotkey(e));
    });
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    // Transcription worker: loads the model once, then handles one job at a time.
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let proxy = event_loop.create_proxy();
    thread::spawn(move || {
        let send = |e| {
            let _ = proxy.send_event(e);
        };
        let mut stt = match stt::Stt::load(&data_dir()) {
            Ok(stt) => stt,
            Err(e) => return send(UserEvent::State(State::Error, Some(e))),
        };
        send(UserEvent::State(State::Idle, None));
        for job in job_rx {
            match job {
                Job::Preview(samples) => {
                    let text = stt.transcribe(&samples).unwrap_or_default();
                    send(UserEvent::Preview(text));
                }
                Job::Final(samples) => {
                    match stt.transcribe(&samples) {
                        Ok(text) if !text.is_empty() => {
                            if let Err(e) = paste::paste(&text) {
                                eprintln!("[paste] {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("[stt] {e}"),
                    }
                    send(UserEvent::State(State::Idle, None));
                }
            }
        }
    });

    let mut mode = if read_setting("mode").as_deref() == Some("toggle") { Mode::Toggle } else { Mode::Hold };
    let mut live = read_setting("preview").as_deref() != Some("off");

    let status_item = MenuItem::new("Loading model...", false, None);
    let hold_item = CheckMenuItem::new("Hold Fn to talk", true, mode == Mode::Hold, None);
    let toggle_item = CheckMenuItem::new("Press Fn to toggle", true, mode == Mode::Toggle, None);
    let live_item = CheckMenuItem::new("Show live preview", true, live, None);
    let quit_item = MenuItem::new("Quit minitype", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &hold_item,
        &toggle_item,
        &PredefinedMenuItem::separator(),
        &live_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])
    .expect("menu");

    let mut tray: Option<TrayIcon> = None;
    let mut pill: Option<Pill> = None;
    let mut state = State::Loading;
    let mut recording: Option<Recording> = None;
    let mut preview_in_flight = false;
    let mut preview_text = String::new();

    event_loop.run(move |event, _, control_flow| {
        let mtm = MainThreadMarker::new().expect("event loop runs on the main thread");
        let mut new_state = None;

        match event {
            Event::NewEvents(StartCause::Init) => {
                // On macOS the tray icon must be created once the event loop is running.
                tray = Some(
                    TrayIconBuilder::new()
                        .with_menu(Box::new(menu.clone()))
                        .with_tooltip("minitype")
                        .with_icon(icon(State::Loading))
                        .build()
                        .expect("tray icon"),
                );
                pill = Some(Pill::new(mtm));
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                // Preview tick: re-transcribe the recent audio if the worker is free.
                if let Some(r) = &recording
                    && live
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
                if matches!(state, State::Recording | State::Busy) && !text.is_empty() {
                    preview_text = text;
                    let look = if state == State::Recording { Look::Listening } else { Look::Working };
                    if let Some(p) = &pill {
                        p.show(mtm, look, &preview_text);
                    }
                }
            }
            Event::UserEvent(UserEvent::State(s, msg)) => {
                if let Some(msg) = msg {
                    eprintln!("[minitype] {msg}");
                    status_item.set_text(format!("Error: {msg}"));
                }
                new_state = Some(s);
            }
            Event::UserEvent(UserEvent::Hotkey(FnEvent::Interrupted)) => {
                // Fn was part of a shortcut (Fn+F5...): discard, don't transcribe.
                if mode == Mode::Hold && recording.take().is_some() {
                    new_state = Some(State::Idle);
                }
            }
            Event::UserEvent(UserEvent::Hotkey(e)) => {
                let start = match (mode, e) {
                    (_, FnEvent::Down) => recording.is_none(),
                    (Mode::Hold, _) => false,
                    (Mode::Toggle, _) => return,
                };
                if start {
                    if state != State::Idle {
                        return;
                    }
                    match Recording::start() {
                        Ok(r) => {
                            recording = Some(r);
                            preview_text.clear();
                            sound("Tink");
                            new_state = Some(State::Recording);
                        }
                        Err(e) => eprintln!("[audio] {e}"),
                    }
                } else if let Some(r) = recording.take() {
                    sound("Pop");
                    let samples = r.finish();
                    if samples.len() >= MIN_SAMPLES {
                        let _ = job_tx.send(Job::Final(samples));
                        new_state = Some(State::Busy);
                    } else {
                        new_state = Some(State::Idle);
                    }
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == *quit_item.id() {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                if e.id == *hold_item.id() {
                    mode = Mode::Hold;
                } else if e.id == *toggle_item.id() {
                    mode = Mode::Toggle;
                } else if e.id == *live_item.id() {
                    live = live_item.is_checked();
                    write_setting("preview", if live { "on" } else { "off" });
                }
                hold_item.set_checked(mode == Mode::Hold);
                toggle_item.set_checked(mode == Mode::Toggle);
                write_setting("mode", if mode == Mode::Toggle { "toggle" } else { "hold" });
            }
            _ => {}
        }

        if let Some(s) = new_state {
            state = s;
            if let Some(tray) = &tray {
                let _ = tray.set_icon(Some(icon(s)));
            }
            if let Some(p) = &pill {
                match s {
                    State::Recording if live => p.show(mtm, Look::Listening, ""),
                    State::Busy if live => p.show(mtm, Look::Working, &preview_text),
                    _ => p.hide(),
                }
            }
            let label = match s {
                State::Loading => "Loading model...",
                State::Idle => "Ready",
                State::Recording => "Recording...",
                State::Busy => "Transcribing...",
                State::Error => "Error",
            };
            if s != State::Error {
                status_item.set_text(label);
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

/// A small dot icon whose colour shows the state.
fn icon(state: State) -> Icon {
    const SIZE: u32 = 32;
    let (rgb, filled) = match state {
        State::Loading => ([150, 150, 150], false),
        State::Idle => ([120, 120, 120], true),
        State::Recording => ([230, 50, 50], true),
        State::Busy => ([240, 160, 30], true),
        State::Error => ([230, 50, 50], false),
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
