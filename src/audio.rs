//! Microphone capture. The cpal stream lives on its own thread (it isn't `Send`
//! on macOS) and only exists while recording, so the mic indicator is off when idle.
//! The sample buffer is shared, so the UI can grab a snapshot for live preview.

use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

pub const WHISPER_RATE: u32 = 16_000;

type Buffer = Arc<Mutex<Vec<f32>>>;

pub struct Recording {
    buffer: Buffer,
    rate: u32,
    stop_tx: mpsc::Sender<()>,
    done_rx: mpsc::Receiver<()>,
}

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

/// Names of the available microphones.
pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|it| it.filter_map(|d| device_name(&d)).collect())
        .unwrap_or_default()
}

impl Recording {
    /// Records from the microphone called `mic`, or the default one if `mic` is
    /// empty or not connected.
    pub fn start(mic: &str) -> Result<Self, String> {
        let mic = mic.to_string();
        let buffer = Buffer::default();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let (done_tx, done_rx) = mpsc::channel();

        let buf = buffer.clone();
        thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let stream = match open_stream(&mic, &buf) {
                    Ok((stream, rate)) => {
                        let _ = ready_tx.send(Ok(rate));
                        stream
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Blocks until finish() is called or the Recording is dropped.
                let _ = stop_rx.recv();
                drop(stream);
                let _ = done_tx.send(());
            })
            .map_err(|e| e.to_string())?;

        match ready_rx.recv() {
            Ok(Ok(rate)) => Ok(Self { buffer, rate, stop_tx, done_rx }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("audio thread exited before the mic opened".into()),
        }
    }

    /// The last `max_secs` of audio so far, as mono 16 kHz, without stopping.
    pub fn snapshot(&self, max_secs: u32) -> Vec<f32> {
        let buf = self.buffer.lock().unwrap_or_else(|p| p.into_inner());
        let keep = (self.rate * max_secs) as usize;
        let tail = &buf[buf.len().saturating_sub(keep)..];
        resample(tail, self.rate, WHISPER_RATE)
    }

    /// Stops the mic and returns all audio as mono 16 kHz.
    pub fn finish(self) -> Vec<f32> {
        let _ = self.stop_tx.send(());
        let _ = self.done_rx.recv();
        let samples = std::mem::take(&mut *self.buffer.lock().unwrap_or_else(|p| p.into_inner()));
        resample(&samples, self.rate, WHISPER_RATE)
    }
}

fn open_stream(mic: &str, buffer: &Buffer) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let chosen = if mic.is_empty() {
        None
    } else {
        host.input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| device_name(d).as_deref() == Some(mic)))
    };
    let device = chosen
        .or_else(|| host.default_input_device())
        .ok_or("no microphone found")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels()).max(1);
    let format = config.sample_format();
    let cfg = config.config();
    let buf = buffer.clone();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &cfg, channels, buf),
        SampleFormat::I16 => build::<i16>(&device, &cfg, channels, buf),
        SampleFormat::I32 => build::<i32>(&device, &cfg, channels, buf),
        SampleFormat::U16 => build::<u16>(&device, &cfg, channels, buf),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    buffer: Buffer,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            *config,
            move |data: &[T], _: &_| {
                let mut buf = buffer.lock().unwrap_or_else(|p| p.into_inner());
                // Downmix to mono.
                buf.extend(data.chunks(channels).map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len() as f32
                }));
            },
            |err| eprintln!("[audio] {err}"),
            None,
        )
        .map_err(|e| e.to_string())
}

pub fn resample_to_whisper(input: &[f32], from: u32) -> Vec<f32> {
    resample(input, from, WHISPER_RATE)
}

/// Linear interpolation resampler. Good enough for speech.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let out_len = (input.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}
