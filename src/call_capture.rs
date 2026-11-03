use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
    },
    time::Instant,
};

use crate::calls::interruptions::{Cause, Interruption, Source};

/// Why a track stopped delivering audio, recorded before the capture thread sees it.
const FAULT_DEVICE: u8 = 1;
const FAULT_OVERFLOW: u8 = 2;

/// The first fault since the track last recovered. Written by the audio and
/// error callbacks with atomics only; read by the capture thread.
#[derive(Default)]
struct Fault {
    /// Milliseconds since the capture origin, plus one; zero when clear.
    at: AtomicU64,
    kind: AtomicU8,
}

impl Fault {
    fn note(&self, kind: u8, ms: u64) {
        if self
            .at
            .compare_exchange(0, ms + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.kind.store(kind, Ordering::Relaxed);
        }
    }

    fn take(&self) -> Option<(u64, u8)> {
        let at = self.at.swap(0, Ordering::Relaxed);
        (at != 0).then(|| (at - 1, self.kind.load(Ordering::Relaxed)))
    }
}

const BLOCK: usize = 512;
struct Packet {
    at: u64,
    len: usize,
    pcm: [f32; BLOCK],
}

pub struct Control {
    origin: OnceLock<Instant>,
    discord: Mutex<Option<Arc<crate::discord::Connection>>>,
    pub active_devices: Mutex<(Option<String>, Option<String>)>,
    pub stop_ns: AtomicU64,
    pub abort: AtomicBool,
}
impl Control {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            origin: OnceLock::new(),
            discord: Mutex::new(None),
            active_devices: Mutex::new((None, None)),
            stop_ns: AtomicU64::new(0),
            abort: AtomicBool::new(false),
        })
    }
    pub fn stop(&self) {
        let _ = self.stop_ns.compare_exchange(
            0,
            self.elapsed().as_nanos() as u64 + 1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    /// Replace metadata independently of audio capture, including during a call.
    /// This mutex is never accessed by the real-time audio callback.
    pub fn set_discord(&self, connection: Option<Arc<crate::discord::Connection>>) {
        let previous = std::mem::replace(
            &mut *self.discord.lock().unwrap_or_else(|e| e.into_inner()),
            connection,
        );
        drop(previous);
    }
    pub fn discord(&self) -> Option<Arc<crate::discord::Connection>> {
        self.discord
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn elapsed(&self) -> std::time::Duration {
        self.origin.get().map(Instant::elapsed).unwrap_or_default()
    }
    /// Audio sample positions and external activity observations share this clock.
    pub fn origin(&self) -> Option<Instant> {
        self.origin.get().copied()
    }
    pub fn end_seconds(&self) -> f64 {
        let stop = self.stop_ns.load(Ordering::SeqCst);
        if stop == 0 {
            self.elapsed().as_secs_f64()
        } else {
            (stop - 1) as f64 / 1e9
        }
    }
}

pub struct Track {
    _stream: cpal::Stream,
    consumer: HeapCons<Packet>,
    pending: VecDeque<Packet>,
    rate: u32,
    cursor: u64,
    converter: crate::audio::StreamingResampler,
    control: Arc<Control>,
    failed: Arc<AtomicBool>,
    fault: Arc<Fault>,
    pub level: Arc<AtomicU32>,
    selected: Option<String>,
    output: bool,
    pub gaps: u64,
    recovering: bool,
    /// The interruption in progress while the device is being reopened.
    open_gap: Option<Interruption>,
    /// Interruptions not yet collected with `take_interruptions`.
    reported: Vec<Interruption>,
    pub device: String,
}

impl Track {
    pub fn open(selected: Option<&str>, output: bool, control: Arc<Control>) -> Result<Self> {
        control.origin.get_or_init(Instant::now);
        let host = cpal::default_host();
        let device = if let Some(name) = selected {
            let mut devices = if output {
                host.output_devices()?
            } else {
                host.input_devices()?
            };
            devices
                .find(|d| d.name().is_ok_and(|n| n == name))
                .or_else(|| {
                    if output {
                        host.default_output_device()
                    } else {
                        host.default_input_device()
                    }
                })
                .context("No audio device is available")?
        } else if output {
            host.default_output_device()
                .context("No speaker or headphone output found")?
        } else {
            host.default_input_device().context("No microphone found")?
        };
        let device_name = device
            .name()
            .unwrap_or_else(|_| "Default audio device".into());
        let config = if output {
            device.default_output_config()?
        } else {
            device.default_input_config()?
        };
        let rate = config.sample_rate().0;
        let (producer, consumer) = HeapRb::<Packet>::new(8192).split();
        let failed = Arc::new(AtomicBool::new(false));
        let fault = Arc::new(Fault::default());
        let level = Arc::new(AtomicU32::new(0));
        let state = Writer {
            producer,
            channels: config.channels() as usize,
            rate,
            control: control.clone(),
            failed: failed.clone(),
            fault: fault.clone(),
            level: level.clone(),
            next: None,
        };
        // On Windows, CPAL enables WASAPI loopback for input streams opened on
        // render endpoints. Shared mode does not interrupt existing playback.
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config.into(), state)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config.into(), state)?,
            cpal::SampleFormat::I32 => build::<i32>(&device, &config.into(), state)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config.into(), state)?,
            _ => anyhow::bail!("Unsupported call audio format"),
        };
        stream.play()?;
        let mut reported = Vec::new();
        if let Some(notice) = crate::audio::device_notice(selected, &device_name) {
            let ms = control.elapsed().as_millis() as u64;
            let mut gap = Interruption::new(
                ms,
                ms,
                if output {
                    Source::SystemAudio
                } else {
                    Source::Microphone
                },
                Cause::DeviceLost,
            );
            gap.device_notice = Some(notice);
            reported.push(gap);
        }
        {
            let mut active = control
                .active_devices
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if output {
                active.1 = Some(device_name.clone());
            } else {
                active.0 = Some(device_name.clone());
            }
        }
        Ok(Self {
            _stream: stream,
            consumer,
            pending: VecDeque::new(),
            rate,
            cursor: 0,
            converter: crate::audio::StreamingResampler::new(rate)?,
            control,
            failed,
            fault,
            level,
            selected: selected.map(str::to_owned),
            output,
            gaps: 0,
            recovering: false,
            open_gap: None,
            reported,
            device: device_name,
        })
    }

    fn source(&self) -> Source {
        if self.output {
            Source::SystemAudio
        } else {
            Source::Microphone
        }
    }

    /// Interruptions since the last call, including one still in progress
    /// (marked ongoing, ending at `now` seconds).
    pub fn take_interruptions(&mut self, now: f64) -> Vec<Interruption> {
        let mut found = std::mem::take(&mut self.reported);
        if let Some(open) = &self.open_gap {
            let mut current = open.clone();
            current.end_ms = current.end_ms.max((now * 1000.0) as u64);
            found.push(current);
        }
        found
    }

    /// Record the start of an interruption noticed at `seconds`.
    fn begin_gap(&mut self, seconds: f64) {
        // The callback records when the fault happened; without it, the audio
        // is missing from the previous refresh onward.
        let now_ms = (seconds * 1000.0) as u64;
        let cursor_ms = self.cursor * 1000 / u64::from(self.rate.max(1));
        let (start_ms, cause) = match self.fault.take() {
            Some((at, FAULT_OVERFLOW)) => (at, Cause::Overflow),
            Some((at, _)) => (at, Cause::DeviceLost),
            None => (cursor_ms, Cause::DeviceLost),
        };
        let mut gap = Interruption::new(start_ms.min(now_ms), now_ms, self.source(), cause);
        gap.ongoing = true;
        self.reported.push(gap.clone());
        self.open_gap = Some(gap);
    }

    pub fn take_until(&mut self, seconds: f64) -> Result<Vec<f32>> {
        let interrupted = self.failed.swap(false, Ordering::Relaxed);
        if interrupted && !self.recovering {
            self.gaps += 1;
            self.recovering = true;
            self.begin_gap(seconds);
        }
        while let Some(packet) = self.consumer.try_pop() {
            self.pending.push_back(packet);
        }
        let end = (seconds * self.rate as f64).round() as u64;
        anyhow::ensure!(
            end >= self.cursor && end - self.cursor <= self.rate as u64 * 40,
            "Call processing cannot keep up. Capture has stopped."
        );
        let mut pcm = vec![0.0; (end - self.cursor) as usize];
        while let Some(packet) = self.pending.front() {
            if packet.at >= end {
                break;
            }
            let a = packet.at.max(self.cursor);
            let b = (packet.at + packet.len as u64).min(end);
            if a < b {
                pcm[(a - self.cursor) as usize..(b - self.cursor) as usize].copy_from_slice(
                    &packet.pcm[(a - packet.at) as usize..(b - packet.at) as usize],
                );
            }
            if packet.at + packet.len as u64 > end {
                break;
            }
            self.pending.pop_front();
        }
        self.cursor = end;
        let mut output = Vec::new();
        self.converter.append(&pcm, &mut output)?;
        if self.control.stop_ns.load(Ordering::SeqCst) != 0 && seconds >= self.control.end_seconds()
        {
            self.converter.finish(&mut output)?;
        }
        if interrupted && self.control.stop_ns.load(Ordering::SeqCst) == 0 {
            self.converter.finish(&mut output)?;
            match Self::open(self.selected.as_deref(), self.output, self.control.clone()) {
                Ok(mut replacement) => {
                    replacement.cursor = (seconds * f64::from(replacement.rate)).round() as u64;
                    replacement.gaps = self.gaps;
                    replacement.reported = std::mem::take(&mut self.reported);
                    if let Some(mut gap) = self.open_gap.take() {
                        gap.end_ms = gap.end_ms.max((seconds * 1000.0) as u64);
                        gap.ongoing = false;
                        gap.device_notice = Some(format!(
                            "Audio device reconnected. Now using {}.",
                            replacement.device
                        ));
                        replacement.reported.push(gap);
                    }
                    *self = replacement;
                }
                Err(_) => {
                    // Preserve the other track and retry at the next refresh.
                    self.failed.store(true, Ordering::Relaxed);
                    self.converter = crate::audio::StreamingResampler::new(self.rate)?;
                }
            }
        }
        Ok(output)
    }
}

struct Writer {
    producer: HeapProd<Packet>,
    channels: usize,
    rate: u32,
    control: Arc<Control>,
    failed: Arc<AtomicBool>,
    fault: Arc<Fault>,
    level: Arc<AtomicU32>,
    next: Option<u64>,
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut writer: Writer,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let failed = writer.failed.clone();
    let fault = writer.fault.clone();
    let control = writer.control.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            if writer.control.stop_ns.load(Ordering::Relaxed) != 0
                || writer.control.abort.load(Ordering::Relaxed)
            {
                return;
            }
            let timestamp = info.timestamp();
            let latency = timestamp
                .callback
                .duration_since(&timestamp.capture)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let wall = writer.control.elapsed().as_secs_f64();
            let clock_at = ((wall - latency).max(0.0) * writer.rate as f64).round() as u64;
            // Use sample continuity for small callback jitter; preserve actual gaps
            // when a loopback endpoint stops delivering packets during silence.
            let at = writer
                .next
                .filter(|n| n.abs_diff(clock_at) < writer.rate as u64 / 20)
                .unwrap_or(clock_at);
            let mut frames = data.chunks_exact(writer.channels);
            let mut offset = 0;
            let mut peak = 0.0f32;
            loop {
                let mut packet = Packet {
                    at: at + offset,
                    len: 0,
                    pcm: [0.0; BLOCK],
                };
                for frame in frames.by_ref().take(BLOCK) {
                    let sample = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>()
                        / writer.channels as f32;
                    packet.pcm[packet.len] = sample;
                    packet.len += 1;
                    peak = peak.max(sample.abs());
                }
                if packet.len == 0 {
                    break;
                }
                offset += packet.len as u64;
                if writer.producer.try_push(packet).is_err() {
                    writer
                        .fault
                        .note(FAULT_OVERFLOW, (wall * 1000.0).max(0.0) as u64);
                    writer.failed.store(true, Ordering::Relaxed);
                    break;
                }
            }
            writer.next = Some(at + offset);
            writer.level.store(peak.to_bits(), Ordering::Relaxed);
        },
        move |_| {
            fault.note(FAULT_DEVICE, control.elapsed().as_millis() as u64);
            failed.store(true, Ordering::Relaxed);
        },
        None,
    )?)
}

pub fn outputs() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}
