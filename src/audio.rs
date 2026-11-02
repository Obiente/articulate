use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{
    HeapCons, HeapRb,
    traits::{Consumer, Producer, Split},
};
use rubato::{FftFixedInOut, Resampler};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

pub const CAPTURE_BUFFER_SECONDS: usize = 90;
pub const WINDOW_SECONDS: usize = 20;

pub struct Recording {
    stream: cpal::Stream,
    consumer: HeapCons<f32>,
    rate: u32,
    captured: Vec<f32>,
    converter: StreamingResampler,
    pub samples: Arc<AtomicUsize>,
    #[allow(
        dead_code,
        reason = "Preserve microphone metering for React recording feedback"
    )]
    pub level: Arc<AtomicU32>,
    pub failed: Arc<AtomicBool>,
    pub full: Arc<AtomicBool>,
    pub device: String,
}

impl Recording {
    pub fn start(selected: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match selected {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().is_ok_and(|n| n == name))
                .or_else(|| host.default_input_device())
                .context("No microphone found. Connect a microphone and try again.")?,
            None => host
                .default_input_device()
                .context("No microphone found. Connect a microphone and try again.")?,
        };
        let name = device
            .name()
            .unwrap_or_else(|_| "Default microphone".into());
        let config = device
            .default_input_config()
            .context("Could not open the default microphone")?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let capacity = rate as usize * CAPTURE_BUFFER_SECONDS;
        let (producer, consumer) = HeapRb::<f32>::new(capacity).split();
        let samples = Arc::new(AtomicUsize::new(0));
        let level = Arc::new(AtomicU32::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let full = Arc::new(AtomicBool::new(false));
        let count = samples.clone();
        let meter = level.clone();
        let limit = full.clone();
        let state = Capture {
            producer,
            channels,
            count,
            meter,
            limit,
            error: failed.clone(),
        };
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), state)?,
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), state)?,
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config.into(), state)?,
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), state)?,
            _ => bail!(
                "This microphone format is unsupported. Choose a different Windows default microphone or import a WAV file."
            ),
        };
        stream.play()?;
        Ok(Self {
            stream,
            consumer,
            rate,
            captured: Vec::with_capacity(16000 * CAPTURE_BUFFER_SECONDS),
            converter: StreamingResampler::new(rate)?,
            samples,
            level,
            failed,
            full,
            device: name,
        })
    }

    pub fn seconds(&self) -> f32 {
        self.samples.load(Ordering::Relaxed) as f32 / self.rate as f32
    }

    pub fn snapshot(&mut self) -> Result<Vec<f32>> {
        self.drain();
        Ok(self.captured[..self.captured.len().min(16000 * WINDOW_SECONDS)].to_vec())
    }
    pub fn drain(&mut self) {
        let input: Vec<_> = self.consumer.pop_iter().collect();
        if self.converter.append(&input, &mut self.captured).is_err() {
            self.failed.store(true, Ordering::Relaxed);
        }
        if self.captured.len() >= 16000 * CAPTURE_BUFFER_SECONDS {
            self.full.store(true, Ordering::Relaxed);
        }
    }

    pub fn window_seconds(&self) -> f32 {
        self.captured.len() as f32 / 16000.0
    }

    pub fn take_window(&mut self) -> Result<Option<Vec<f32>>> {
        self.drain();
        if self.captured.len() < 16000 * WINDOW_SECONDS {
            return Ok(None);
        }
        // Prefer a quiet boundary in the final two seconds of each window.
        let end = WINDOW_SECONDS * 16000;
        let cut = quiet_boundary(&self.captured, 16000, end);
        let chunk: Vec<_> = self.captured.drain(..cut.min(end)).collect();
        Ok(Some(chunk))
    }

    pub fn stop(self) -> Result<CapturedAudio> {
        let Self {
            stream,
            mut consumer,
            mut converter,
            failed,
            full,
            mut captured,
            ..
        } = self;
        drop(stream);
        let tail: Vec<_> = consumer.pop_iter().collect();
        converter.append(&tail, &mut captured)?;
        converter.finish(&mut captured)?;
        Ok(CapturedAudio {
            pcm: captured,
            interruption: interruption(
                failed.load(Ordering::Relaxed),
                full.load(Ordering::Relaxed),
            ),
        })
    }
}

pub fn quiet_boundary(pcm: &[f32], rate: usize, end: usize) -> usize {
    let end = end.min(pcm.len());
    if end == 0 {
        return 0;
    }
    let stride = (rate / 20).max(1);
    let begin = end.saturating_sub(rate * 2).max(1).min(end);
    let cut = (begin..end)
        .step_by(stride)
        .min_by(|&a, &b| {
            let energy = |i: usize| {
                pcm[i..(i + stride).min(end)]
                    .iter()
                    .map(|v| v * v)
                    .sum::<f32>()
            };
            energy(a).total_cmp(&energy(b))
        })
        .unwrap_or(end);
    (cut + stride / 2).min(end)
}

struct Capture {
    producer: ringbuf::HeapProd<f32>,
    channels: usize,
    count: Arc<AtomicUsize>,
    meter: Arc<AtomicU32>,
    limit: Arc<AtomicBool>,
    error: Arc<AtomicBool>,
}

impl Capture {
    fn write<T: cpal::SizedSample>(&mut self, data: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        if self.limit.load(Ordering::Relaxed) || self.error.load(Ordering::Relaxed) {
            return;
        }
        // No heap allocation, mutex, I/O or inference in the audio callback.
        let mut peak = 0.0f32;
        let mut added = 0;
        for frame in data.chunks_exact(self.channels) {
            let mono =
                frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / self.channels as f32;
            peak = peak.max(mono.abs());
            if self.producer.try_push(mono).is_err() {
                self.limit.store(true, Ordering::Relaxed);
                break;
            }
            added += 1;
        }
        self.count.fetch_add(added, Ordering::Relaxed);
        self.meter.store(peak.to_bits(), Ordering::Relaxed);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut capture: Capture,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let error = capture.error.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| capture.write(data),
        move |_| {
            error.store(true, Ordering::Relaxed);
        },
        None,
    )?)
}

pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// Stateful band-limited conversion. Partial blocks stay queued until more audio arrives.
pub struct StreamingResampler {
    rate: u32,
    filter: Option<FftFixedInOut<f32>>,
    pending: Vec<f32>,
    block: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    skip: usize,
    input_count: usize,
    output_count: usize,
}
impl StreamingResampler {
    pub fn new(rate: u32) -> Result<Self> {
        anyhow::ensure!((8000..=192000).contains(&rate), "Unsupported sample rate");
        let filter = if rate == 16000 {
            None
        } else {
            Some(FftFixedInOut::<f32>::new(rate as usize, 16000, 1024, 1)?)
        };
        let size = filter.as_ref().map_or(0, |f| f.input_frames_next());
        let output = filter
            .as_ref()
            .map_or_else(Vec::new, |f| f.output_buffer_allocate(true));
        let skip = filter.as_ref().map_or(0, |f| f.output_delay());
        Ok(Self {
            rate,
            filter,
            pending: Vec::new(),
            block: vec![vec![0.0; size]],
            output,
            skip,
            input_count: 0,
            output_count: 0,
        })
    }
    pub fn append(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        anyhow::ensure!(
            input.iter().all(|x| x.is_finite()),
            "Audio contains invalid samples"
        );
        self.input_count += input.len();
        if self.filter.is_none() {
            out.extend_from_slice(input);
            self.output_count += input.len();
            return Ok(());
        }
        self.pending.extend_from_slice(input);
        let size = self.block[0].len();
        let complete = self.pending.len() / size * size;
        for start in (0..complete).step_by(size) {
            self.block[0].copy_from_slice(&self.pending[start..start + size]);
            self.process(out)?;
        }
        self.pending.drain(..complete);
        Ok(())
    }
    fn process(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let (_, produced) = self
            .filter
            .as_mut()
            .expect("conversion filter")
            .process_into_buffer(&self.block, &mut self.output, None)?;
        let skip = self.skip.min(produced);
        self.skip -= skip;
        let expected = self.input_count as u64 * 16000 / u64::from(self.rate);
        let count = (produced - skip).min((expected as usize).saturating_sub(self.output_count));
        out.extend_from_slice(&self.output[0][skip..skip + count]);
        self.output_count += count;
        Ok(())
    }
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        if self.filter.is_none() {
            return Ok(());
        }
        self.block[0].fill(0.0);
        self.block[0][..self.pending.len()].copy_from_slice(&self.pending);
        self.pending.clear();
        let expected = (self.input_count as u64 * 16000 / u64::from(self.rate)) as usize;
        while self.output_count < expected {
            self.process(out)?;
            self.block[0].fill(0.0);
        }
        Ok(())
    }
}
pub fn resample(input: &[f32], rate: u32) -> Result<Vec<f32>> {
    let mut converter = StreamingResampler::new(rate)?;
    let mut output = Vec::new();
    converter.append(input, &mut output)?;
    converter.finish(&mut output)?;
    Ok(output)
}

pub fn read_wav(path: impl AsRef<std::path::Path>) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).context("Could not read WAV file")?;
    let spec = reader.spec();
    anyhow::ensure!(spec.channels > 0, "WAV has no channels");
    let interleaved = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            anyhow::ensure!(
                (1..=32).contains(&spec.bits_per_sample),
                "Unsupported PCM depth"
            );
            let scale = 2f32.powi(spec.bits_per_sample as i32 - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 / scale))
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    anyhow::ensure!(
        interleaved.len() % spec.channels as usize == 0,
        "WAV contains an incomplete frame"
    );
    let mono: Vec<f32> = interleaved
        .chunks_exact(spec.channels as usize)
        .map(|f| f.iter().sum::<f32>() / spec.channels as f32)
        .collect();
    resample(&mono, spec.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversion_preserves_duration_and_passband() {
        for rate in [44100, 48000, 96000] {
            let signal: Vec<_> = (0..rate)
                .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate as f32).sin() * 0.5)
                .collect();
            let result = resample(&signal, rate).unwrap();
            assert_eq!(result.len(), 16000);
            let rms = (result[500..15500].iter().map(|v| v * v).sum::<f32>() / 15000.0).sqrt();
            assert!((rms - 0.35355).abs() < 0.01, "{rate}: {rms}");
        }
    }
    #[test]
    fn suppresses_above_nyquist() {
        let signal: Vec<_> = (0..48000)
            .map(|i| (std::f32::consts::TAU * 12000.0 * i as f32 / 48000.0).sin())
            .collect();
        let result = resample(&signal, 48000).unwrap();
        let rms = (result[500..15500].iter().map(|v| v * v).sum::<f32>() / 15000.0).sqrt();
        assert!(rms < 0.01, "Aliased high frequency: {rms}");
    }
    #[test]
    fn streaming_chunks_match_contiguous_conversion() {
        for rate in [16000, 44100, 48000, 96000] {
            let signal: Vec<_> = (0..rate + 17)
                .map(|i| (i as f32 * 0.137).sin() * 0.5)
                .collect();
            let expected = resample(&signal, rate).unwrap();
            for chunk in [1, 319, 2048] {
                let mut stream = StreamingResampler::new(rate).unwrap();
                let mut actual = Vec::new();
                for samples in signal.chunks(chunk) {
                    stream.append(samples, &mut actual).unwrap();
                }
                stream.finish(&mut actual).unwrap();
                assert_eq!(actual, expected, "rate {rate}, chunk {chunk}");
            }
        }
    }
    #[test]
    fn short_and_invalid_inputs() {
        assert!(resample(&[], 48000).unwrap().is_empty());
        assert_eq!(resample(&[0.5; 48], 48000).unwrap().len(), 16);
        assert!(resample(&[f32::NAN], 16000).is_err());
        assert!(resample(&[0.1], 0).is_err());
    }
}

/// A valid prefix remains usable even when capture was interrupted.
pub struct CapturedAudio {
    pub pcm: Vec<f32>,
    pub interruption: Option<&'static str>,
}
fn interruption(failed: bool, full: bool) -> Option<&'static str> {
    if full {
        Some("Incomplete transcript: transcription fell behind. Captured audio was retained.")
    } else if failed {
        Some("Incomplete transcript: the microphone was interrupted. Captured audio was retained.")
    } else {
        None
    }
}

#[derive(Debug, serde::Serialize)]
pub struct AudioCheck {
    pub device: String,
    pub peak: f32,
    pub rms: f32,
    pub clipping_ratio: f32,
    pub silent: bool,
}
impl AudioCheck {
    pub fn measure(device: String, pcm: &[f32]) -> Self {
        let count = pcm.len().max(1) as f64;
        let peak = pcm.iter().fold(0.0f32, |p, x| p.max(x.abs()));
        let rms = (pcm.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / count).sqrt() as f32;
        Self {
            device,
            peak,
            rms,
            clipping_ratio: pcm.iter().filter(|x| x.abs() >= 0.999).count() as f32 / count as f32,
            silent: rms < 0.0001,
        }
    }
}
/// Run on a worker thread. Capture only; never loads a speech model.
#[allow(dead_code, reason = "Backend API awaiting microphone check UI")]
pub fn check_microphone(selected: Option<&str>, seconds: u32) -> Result<AudioCheck> {
    anyhow::ensure!(
        (1..=30).contains(&seconds),
        "Choose a check duration from 1 to 30 seconds"
    );
    let mut recording = Recording::start(selected)?;
    let name = recording.device.clone();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(u64::from(seconds)) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        recording.drain();
        anyhow::ensure!(
            !recording.failed.load(Ordering::Relaxed),
            "Microphone check was interrupted"
        );
    }
    let captured = recording.stop()?;
    anyhow::ensure!(
        captured.interruption.is_none(),
        "Microphone check was interrupted"
    );
    Ok(AudioCheck::measure(name, &captured.pcm))
}
#[cfg(test)]
mod capture_tests {
    use super::*;
    #[test]
    fn overflow_keeps_prefix_and_stops_accepting_after_the_gap() {
        let (producer, mut consumer) = HeapRb::<f32>::new(3).split();
        let mut capture = Capture {
            producer,
            channels: 1,
            count: Arc::new(AtomicUsize::new(0)),
            meter: Arc::new(AtomicU32::new(0)),
            limit: Arc::new(AtomicBool::new(false)),
            error: Arc::new(AtomicBool::new(false)),
        };
        capture.write(&[0.1f32, 0.2, 0.3, 0.4]);
        assert!(capture.limit.load(Ordering::Relaxed));
        assert!(!capture.error.load(Ordering::Relaxed));
        assert_eq!(capture.count.load(Ordering::Relaxed), 3);
        assert_eq!(consumer.pop_iter().collect::<Vec<_>>(), vec![0.1, 0.2, 0.3]);
        capture.write(&[0.5f32]);
        assert!(consumer.try_pop().is_none());
    }
    #[test]
    fn interruption_distinguishes_backpressure_from_device_failure() {
        assert!(interruption(false, false).is_none());
        assert!(interruption(false, true).unwrap().contains("fell behind"));
        assert!(interruption(true, false).unwrap().contains("microphone"));
    }
    #[test]
    fn meter_reports_silence_level_and_clipping() {
        assert!(AudioCheck::measure("Test".into(), &[]).silent);
        assert!(AudioCheck::measure("Test".into(), &[0.0; 100]).silent);
        let check = AudioCheck::measure("Test".into(), &[1.0, -1.0, 0.0, 0.0]);
        assert_eq!(check.peak, 1.0);
        assert!((check.rms - 0.5f32.sqrt()).abs() < 1e-6);
        assert_eq!(check.clipping_ratio, 0.5);
    }
}

/// Check input and optionally a render endpoint on a worker thread, without inference.
#[allow(dead_code, reason = "Backend API awaiting capture check UI")]
pub fn check_devices(
    input: Option<&str>,
    output: Option<&str>,
    loopback: bool,
    seconds: u32,
) -> Result<Vec<AudioCheck>> {
    anyhow::ensure!(
        (1..=30).contains(&seconds),
        "Choose a check duration from 1 to 30 seconds"
    );
    let control = crate::call_capture::Control::new();
    let mut mic = crate::call_capture::Track::open(input, false, control.clone())?;
    let mut speaker = if loopback {
        Some(crate::call_capture::Track::open(
            output,
            true,
            control.clone(),
        )?)
    } else {
        None
    };
    let mut mic_pcm = Vec::new();
    let mut output_pcm = Vec::new();
    while control.end_seconds() < f64::from(seconds) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let end = control.end_seconds().min(f64::from(seconds));
        mic_pcm.extend(mic.take_until(end)?);
        if let Some(track) = &mut speaker {
            output_pcm.extend(track.take_until(end)?);
        }
    }
    control.stop();
    mic_pcm.extend(mic.take_until(control.end_seconds())?);
    anyhow::ensure!(mic.gaps == 0, "Microphone check was interrupted");
    let mut results = vec![AudioCheck::measure(mic.device, &mic_pcm)];
    if let Some(mut track) = speaker {
        output_pcm.extend(track.take_until(control.end_seconds())?);
        anyhow::ensure!(track.gaps == 0, "Output check was interrupted");
        results.push(AudioCheck::measure(track.device, &output_pcm));
    }
    Ok(results)
}
