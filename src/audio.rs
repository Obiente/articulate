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

pub const CAPTURE_BUFFER_SECONDS: usize = 30;
pub const WINDOW_SECONDS: usize = 20;

pub struct Recording {
    stream: cpal::Stream,
    consumer: HeapCons<f32>,
    rate: u32,
    captured: Vec<f32>,
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
            Some(name) => host.input_devices()?.find(|d| d.name().is_ok_and(|n| n == name))
                .context("The selected microphone is unavailable. Choose another microphone in Settings.")?,
            None => host.default_input_device().context("No microphone found. Connect a microphone and try again.")?,
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
            captured: Vec::with_capacity(capacity),
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
        resample(
            &self.captured[..self.captured.len().min(self.rate as usize * WINDOW_SECONDS)],
            self.rate,
        )
    }

    fn drain(&mut self) {
        while let Some(sample) = self.consumer.try_pop() {
            self.captured.push(sample);
        }
    }

    pub fn window_seconds(&self) -> f32 {
        self.captured.len() as f32 / self.rate as f32
    }

    pub fn take_window(&mut self) -> Result<Option<Vec<f32>>> {
        self.drain();
        // Bound backlog explicitly. Never silently discard speech on a slow PC.
        anyhow::ensure!(
            self.captured.len() <= self.rate as usize * 90,
            "Transcription cannot keep up with capture. Recording stopped to avoid losing audio."
        );
        if self.captured.len() < self.rate as usize * WINDOW_SECONDS {
            return Ok(None);
        }
        // Prefer a quiet boundary in the final two seconds of each window.
        let end = WINDOW_SECONDS * self.rate as usize;
        let cut = quiet_boundary(&self.captured, self.rate as usize, end);
        let chunk: Vec<_> = self.captured.drain(..cut.min(end)).collect();
        Ok(Some(resample(&chunk, self.rate)?))
    }

    pub fn stop(self) -> Result<Vec<f32>> {
        let Self {
            stream,
            mut consumer,
            rate,
            failed,
            mut captured,
            ..
        } = self;
        drop(stream);
        anyhow::ensure!(
            !failed.load(Ordering::Relaxed),
            "Microphone capture failed. Please check your input device and record again."
        );
        while let Some(sample) = consumer.try_pop() {
            captured.push(sample);
        }
        resample(&captured, rate)
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
        move |data: &[T], _| {
            // No heap allocation, mutex, I/O or inference in the audio callback.
            let mut peak = 0.0f32;
            let mut added = 0;
            for frame in data.chunks_exact(capture.channels) {
                let mono = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>()
                    / capture.channels as f32;
                peak = peak.max(mono.abs());
                if capture.producer.try_push(mono).is_err() {
                    capture.limit.store(true, Ordering::Relaxed);
                    capture.error.store(true, Ordering::Relaxed);
                    break;
                }
                added += 1;
            }
            capture.count.fetch_add(added, Ordering::Relaxed);
            capture.meter.store(peak.to_bits(), Ordering::Relaxed);
        },
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

pub fn resample(input: &[f32], rate: u32) -> Result<Vec<f32>> {
    anyhow::ensure!((8000..=192000).contains(&rate), "Unsupported sample rate");
    anyhow::ensure!(
        input.iter().all(|x| x.is_finite()),
        "Audio contains invalid samples"
    );
    if rate == 16000 || input.is_empty() {
        return Ok(input.to_vec());
    }
    // Band-limited conversion avoids aliasing from simple sample decimation.
    let mut resampler = FftFixedInOut::<f32>::new(rate as usize, 16000, 1024, 1)?;
    let block = resampler.input_frames_next();
    let delay = resampler.output_delay();
    let expected = (input.len() as u64 * 16000 / rate as u64) as usize;
    let mut output = Vec::with_capacity(expected + delay + 2048);
    for chunk in input.chunks(block) {
        let mut padded = vec![0.0; block];
        padded[..chunk.len()].copy_from_slice(chunk);
        output.extend(resampler.process(&[padded], None)?.remove(0));
    }
    while output.len() < expected + delay {
        output.extend(resampler.process(&[vec![0.0; block]], None)?.remove(0));
    }
    Ok(output[delay..delay + expected].to_vec())
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
    fn short_and_invalid_inputs() {
        assert!(resample(&[], 48000).unwrap().is_empty());
        assert_eq!(resample(&[0.5; 48], 48000).unwrap().len(), 16);
        assert!(resample(&[f32::NAN], 16000).is_err());
        assert!(resample(&[0.1], 0).is_err());
    }
}
