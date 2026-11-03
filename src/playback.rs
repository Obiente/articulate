//! Native playback of retained session audio, starting at a transcript time.
//!
//! Audio is decoded and played in Rust; the renderer only asks to start or
//! stop and reads the position. Decoding runs ahead of the output callback
//! through a lock-free ring buffer. The output stream is created and dropped
//! on the playback thread.
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

pub struct Player {
    pub session_id: String,
    from_ms: u64,
    rate: u32,
    stop: Arc<AtomicBool>,
    played: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

type Shared = Arc<Mutex<Option<String>>>;

/// Open the output and start a stream that plays mono samples from the ring.
fn open(
    output: Option<&str>,
    played: Arc<AtomicU64>,
    error: Shared,
) -> Result<(cpal::Stream, u32, ringbuf::HeapProd<f32>)> {
    let host = cpal::default_host();
    let device = output
        .and_then(|name| {
            host.output_devices()
                .ok()?
                .find(|device| device.name().is_ok_and(|n| n == name))
        })
        .or_else(|| host.default_output_device())
        .context("No speaker or headphone output found")?;
    let config = device.default_output_config()?;
    let rate = config.sample_rate().0;
    let channels = usize::from(config.channels()).max(1);
    // About two seconds decoded ahead.
    let (producer, mut consumer) = HeapRb::<f32>::new(rate as usize * 2).split();
    let mut fill = move |data: &mut [f32]| {
        let mut frames = 0;
        for frame in data.chunks_mut(channels) {
            match consumer.try_pop() {
                Some(sample) => {
                    frame.fill(sample);
                    frames += 1;
                }
                None => frame.fill(0.0),
            }
        }
        played.fetch_add(frames, Ordering::Relaxed);
    };
    let on_error = move |problem: cpal::StreamError| {
        *error.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(format!("Playback stopped: {problem}"));
    };
    let stream_config: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| fill(data),
            on_error,
            None,
        )?,
        cpal::SampleFormat::I16 => {
            let mut buffer = Vec::new();
            device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _| {
                    buffer.resize(data.len(), 0.0);
                    fill(&mut buffer);
                    for (target, sample) in data.iter_mut().zip(&buffer) {
                        *target = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                on_error,
                None,
            )?
        }
        _ => bail!("This output device's audio format is not supported for playback"),
    };
    stream.play().context("Playback could not start")?;
    Ok((stream, rate, producer))
}

impl Player {
    /// Play `path` from `from_ms` on `output` (the system default when `None`).
    pub fn start(
        session_id: String,
        path: PathBuf,
        from_ms: u64,
        output: Option<String>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let played = Arc::new(AtomicU64::new(0));
        let error: Shared = Arc::new(Mutex::new(None));
        let (ready_tx, ready) = mpsc::channel::<Result<u32>>();
        let thread_stop = stop.clone();
        let thread_played = played.clone();
        let thread_error = error.clone();
        let thread = std::thread::Builder::new()
            .name("retained-audio-playback".into())
            .spawn(move || {
                let (stream, rate, mut producer) =
                    match open(output.as_deref(), thread_played, thread_error.clone()) {
                        Ok(opened) => opened,
                        Err(problem) => {
                            let _ = ready_tx.send(Err(problem));
                            return;
                        }
                    };
                let _ = ready_tx.send(Ok(rate));
                if let Err(problem) = feed(&path, from_ms, rate, &mut producer, &thread_stop) {
                    *thread_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(format!("{problem:#}"));
                }
                // Let the device play what is buffered, then stop.
                while !thread_stop.load(Ordering::Relaxed) && !producer.is_empty() {
                    std::thread::sleep(Duration::from_millis(50));
                }
                std::thread::sleep(Duration::from_millis(150));
                drop(stream);
            })
            .context("Could not start playback")?;
        let rate = match ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(rate)) => rate,
            Ok(Err(problem)) => {
                let _ = thread.join();
                return Err(problem);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                bail!("The audio output did not respond");
            }
        };
        Ok(Self {
            session_id,
            from_ms,
            rate,
            stop,
            played,
            error,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Whether audio is still playing.
    pub fn playing(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// The transcript time being heard.
    pub fn position_ms(&self) -> u64 {
        self.from_ms + self.played.load(Ordering::Relaxed) * 1000 / u64::from(self.rate.max(1))
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Mix retained channels for listening and convert 16 kHz to `rate`.
struct Converter {
    step: f64,
    position: f64,
    previous: f32,
}

impl Converter {
    fn new(rate: u32) -> Self {
        Self {
            step: f64::from(crate::audio_store::RATE) / f64::from(rate.max(1)),
            position: 0.0,
            previous: 0.0,
        }
    }

    /// Linear interpolation, continuous across calls.
    fn convert(
        &mut self,
        channels: &[Vec<f32>],
        skip: usize,
        mut emit: impl FnMut(f32) -> bool,
    ) -> bool {
        let frames = channels.first().map_or(0, Vec::len);
        let scale = (channels.len().max(1) as f32).sqrt();
        let mono = |frame: usize| -> f32 {
            (channels.iter().map(|channel| channel[frame]).sum::<f32>() / scale).clamp(-1.0, 1.0)
        };
        let count = frames.saturating_sub(skip);
        while self.position < count as f64 {
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            let before = if index == 0 {
                self.previous
            } else {
                mono(skip + index - 1)
            };
            let sample = before + (mono(skip + index) - before) * fraction;
            if !emit(sample) {
                return false;
            }
            self.position += self.step;
        }
        self.position -= count as f64;
        if count > 0 {
            self.previous = mono(frames - 1);
        }
        true
    }
}

fn feed(
    path: &Path,
    from_ms: u64,
    rate: u32,
    producer: &mut impl Producer<Item = f32>,
    stop: &AtomicBool,
) -> Result<()> {
    let start_frame = from_ms * u64::from(crate::audio_store::RATE) / 1000;
    let mut converter = Converter::new(rate);
    crate::audio_store::read(path, from_ms, |start, channels| {
        let skip = start_frame.saturating_sub(start) as usize;
        let more = converter.convert(&channels, skip, |sample| {
            while producer.try_push(sample).is_err() {
                if stop.load(Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        });
        Ok(more && !stop.load(Ordering::Relaxed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_mixes_channels_and_keeps_rate_continuous() {
        let mut converter = Converter::new(48_000);
        let channels = vec![vec![0.5; 160], vec![0.5; 160]];
        let mut out = Vec::new();
        assert!(converter.convert(&channels, 0, |s| {
            out.push(s);
            true
        }));
        assert!(converter.convert(&channels, 100, |s| {
            out.push(s);
            true
        }));
        // 160 + 60 input frames at three output samples each.
        assert_eq!(out.len(), 660);
        assert!(
            out[10..]
                .iter()
                .all(|s| (s - 0.5f32 * 2.0 / 2f32.sqrt()).abs() < 1e-4)
        );
        assert!(!converter.convert(&channels, 0, |_| false));
    }
}
