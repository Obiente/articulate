//! Decoding recorded audio files for import and retained-audio playback.
//!
//! Containers and codecs come from Symphonia's pure Rust implementations:
//! WAV, FLAC, MP3, AAC and ALAC in MP4/M4A, Vorbis in Ogg, and the audio track
//! of MP4 and Matroska/WebM files when it uses one of those codecs. Opus and
//! video codecs are not included. Audio is converted to 16 kHz as it is
//! decoded, so memory stays proportional to the 16 kHz result.
use crate::audio::StreamingResampler;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Read, Seek},
    path::Path,
    sync::Arc,
};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    errors::Error,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::MetadataOptions,
};

/// Asked while a file is read and decoded; `true` stops with "Import cancelled".
pub type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;

/// A `Cancel` that never stops.
pub fn uncancelled() -> Cancel {
    Arc::new(|| false)
}

const CANCELLED: &str = "Import cancelled";

/// A file whose reads fail once decoding is cancelled, so probing or
/// scanning a large file stops too, not only the packet loop.
struct Source {
    file: std::fs::File,
    cancel: Cancel,
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancel)() {
            return Err(std::io::Error::other(CANCELLED));
        }
        self.file.read(buffer)
    }
}

impl Seek for Source {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        if (self.cancel)() {
            return Err(std::io::Error::other(CANCELLED));
        }
        self.file.seek(position)
    }
}

impl MediaSource for Source {
    fn is_seekable(&self) -> bool {
        self.file.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.file.byte_len()
    }
}

/// What a stream must hold, checked while it is decoded: a retained audio
/// segment is never decoded past the length its header states.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub rate: u32,
    pub channels: usize,
    pub frames: u64,
}

/// File extensions offered for import.
pub const EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "flac", "ogg", "oga", "mp4", "mkv", "webm", "aac",
];

/// Longest accepted import, in seconds (about four hours).
pub const MAX_SECONDS: u64 = 4 * 60 * 60;

/// Whether a path has an importable extension.
pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Decoded 16 kHz audio, one vector per requested output channel.
pub struct Decoded {
    pub channels: Vec<Vec<f32>>,
}

impl Decoded {
    #[cfg(test)]
    pub fn duration_ms(&self) -> u64 {
        self.channels.first().map_or(0, |pcm| pcm.len() as u64 / 16)
    }
}

/// How source channels map to output channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Average every source channel into one.
    Mono,
    /// Keep up to `n` source channels separately; missing ones are silent.
    Planar(usize),
}

/// Decode an entire file. `progress` receives 0..=1 when the length is known.
pub fn decode_file(
    path: &Path,
    layout: Layout,
    cancel: &Cancel,
    progress: impl FnMut(f32),
) -> Result<Decoded> {
    let mut channels: Vec<Vec<f32>> = Vec::new();
    stream_file(path, layout, cancel, progress, |chunk| {
        channels.resize(chunk.len(), Vec::new());
        for (target, part) in channels.iter_mut().zip(chunk) {
            target.extend(part);
        }
        Ok(())
    })?;
    Ok(Decoded { channels })
}

/// Decode a file in chunks of 16 kHz audio, one vector per output channel,
/// without holding the whole recording. Returns the source channel count.
pub fn stream_file(
    path: &Path,
    layout: Layout,
    cancel: &Cancel,
    progress: impl FnMut(f32),
    sink: impl FnMut(Vec<Vec<f32>>) -> Result<()>,
) -> Result<usize> {
    ensure!(!cancel(), CANCELLED);
    let file = std::fs::File::open(path).context("Could not open the audio file")?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let source = Source {
        file,
        cancel: cancel.clone(),
    };
    decode(
        MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default()),
        hint,
        layout,
        None,
        &**cancel,
        progress,
        sink,
    )
}

/// Decode an in-memory FLAC stream (one retained audio segment) that must
/// fit `limit`; anything else is an error.
pub fn decode_flac(bytes: Vec<u8>, limit: Limit) -> Result<Decoded> {
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut channels: Vec<Vec<f32>> = Vec::new();
    decode(
        MediaSourceStream::new(
            Box::new(std::io::Cursor::new(bytes)),
            MediaSourceStreamOptions::default(),
        ),
        hint,
        Layout::Planar(limit.channels),
        Some(limit),
        &|| false,
        |_| {},
        |chunk| {
            channels.resize(chunk.len(), Vec::new());
            for (target, part) in channels.iter_mut().zip(chunk) {
                target.extend(part);
            }
            Ok(())
        },
    )?;
    Ok(Decoded { channels })
}

fn unsupported(error: Error) -> anyhow::Error {
    match error {
        Error::Unsupported(_) => anyhow::anyhow!(
            "This file's format or audio codec is not supported. Use WAV, MP3, M4A, FLAC or Ogg Vorbis."
        ),
        other => anyhow::anyhow!("Could not read this audio file: {other}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn decode(
    stream: MediaSourceStream,
    hint: Hint,
    layout: Layout,
    limit: Option<Limit>,
    cancel: &dyn Fn() -> bool,
    mut progress: impl FnMut(f32),
    mut sink: impl FnMut(Vec<Vec<f32>>) -> Result<()>,
) -> Result<usize> {
    // A read failed by cancellation surfaces as an I/O error from Symphonia.
    let failed = |error: Error| {
        if cancel() {
            anyhow::anyhow!(CANCELLED)
        } else {
            unsupported(error)
        }
    };
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(failed)?;
    let track = format
        .default_track(TrackType::Audio)
        .context("This file has no audio track")?;
    let track_id = track.id;
    let total_frames = track.num_frames;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|parameters| parameters.audio())
        .context("This file's audio codec is not supported")?
        .clone();
    if let Some(limit) = limit {
        ensure!(
            parameters.sample_rate.is_none_or(|rate| rate == limit.rate)
                && parameters
                    .channels
                    .as_ref()
                    .is_none_or(|channels| channels.count() == limit.channels)
                && total_frames.is_none_or(|total| total <= limit.frames),
            "The audio stream does not match its stated format"
        );
    }
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&parameters, &AudioDecoderOptions::default())
        .map_err(failed)?;
    let outputs = match layout {
        Layout::Mono => 1,
        Layout::Planar(count) => count.max(1),
    };
    let mut channels: Vec<Vec<f32>> = vec![Vec::new(); outputs];
    let mut converters: Vec<StreamingResampler> = Vec::new();
    let mut rate = 0;
    let mut source_channels = 0;
    let mut decoded_frames = 0u64;
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut mixed = Vec::new();
    loop {
        ensure!(!cancel(), CANCELLED);
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::IoError(_)) if cancel() => bail!(CANCELLED),
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(Error::ResetRequired) => bail!("This file changes audio format part way through"),
            Err(error) => return Err(failed(error)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
            Err(error) => return Err(failed(error)),
        };
        let spec_rate = buffer.spec().rate();
        let spec_channels = buffer.spec().channels().count().max(1);
        if let Some(limit) = limit {
            ensure!(
                spec_rate == limit.rate && spec_channels == limit.channels,
                "The audio stream does not match its stated format"
            );
        }
        if converters.is_empty() {
            rate = spec_rate;
            source_channels = spec_channels;
            for _ in 0..outputs {
                converters.push(
                    StreamingResampler::new(rate)
                        .context("This file's sample rate is not supported (8 kHz to 192 kHz)")?,
                );
            }
        }
        ensure!(
            spec_rate == rate,
            "This file changes sample rate part way through"
        );
        buffer.copy_to_vecs_planar::<f32>(&mut planes);
        let frames = planes.first().map_or(0, Vec::len);
        decoded_frames += frames as u64;
        if let Some(limit) = limit {
            ensure!(
                decoded_frames <= limit.frames,
                "The audio stream is longer than its stated length"
            );
        }
        ensure!(
            decoded_frames / u64::from(rate.max(1)) <= MAX_SECONDS,
            "Recordings longer than four hours cannot be imported"
        );
        let sanitize = |sample: f32| if sample.is_finite() { sample } else { 0.0 };
        match layout {
            Layout::Mono => {
                mixed.clear();
                mixed.extend((0..frames).map(|frame| {
                    sanitize(planes.iter().map(|plane| plane[frame]).sum::<f32>())
                        / planes.len().max(1) as f32
                }));
                converters[0].append(&mixed, &mut channels[0])?;
            }
            Layout::Planar(_) => {
                for (index, converter) in converters.iter_mut().enumerate() {
                    mixed.clear();
                    match planes.get(index) {
                        Some(plane) => mixed.extend(plane.iter().copied().map(sanitize)),
                        None => mixed.resize(frames, 0.0),
                    }
                    converter.append(&mixed, &mut channels[index])?;
                }
            }
        }
        // Hand over about a second at a time.
        if channels[0].len() >= 16_000 {
            sink(std::mem::replace(&mut channels, vec![Vec::new(); outputs]))?;
        }
        if let Some(total) = total_frames.filter(|total| *total > 0) {
            progress((decoded_frames as f32 / total as f32).min(1.0));
        }
    }
    ensure!(
        !converters.is_empty(),
        "This file has no audio that could be decoded"
    );
    for (index, converter) in converters.iter_mut().enumerate() {
        converter.finish(&mut channels[index])?;
    }
    if !channels[0].is_empty() {
        sink(channels)?;
    }
    progress(1.0);
    Ok(source_channels)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tone(seconds: f32, rate: u32, frequency: f32) -> Vec<f32> {
        (0..(seconds * rate as f32) as usize)
            .map(|i| (i as f32 / rate as f32 * frequency * std::f32::consts::TAU).sin() * 0.4)
            .collect()
    }

    fn directory(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "articulate-media-{label}-{}-{}",
            std::process::id(),
            crate::history::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn wav_files_decode_to_16khz_mono_or_planar_channels() {
        let folder = directory("wav");
        let path = folder.join("stereo.wav");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        let left = tone(1.5, 48_000, 440.0);
        for sample in &left {
            writer.write_sample((sample * 32767.0) as i16).unwrap();
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let mut seen = Vec::new();
        let mono = decode_file(&path, Layout::Mono, &uncancelled(), |p| seen.push(p)).unwrap();
        assert_eq!(mono.channels.len(), 1);
        assert_eq!(mono.channels[0].len(), 24_000);
        assert_eq!(mono.duration_ms(), 1500);
        let peak = mono.channels[0].iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(
            (0.15..0.25).contains(&peak),
            "mono averages channels: {peak}"
        );
        assert_eq!(seen.last(), Some(&1.0));
        let planar = decode_file(&path, Layout::Planar(3), &uncancelled(), |_| {}).unwrap();
        assert_eq!(planar.channels.len(), 3);
        assert!(planar.channels[1].iter().all(|s| s.abs() < 1e-3));
        assert!(planar.channels[2].iter().all(|s| *s == 0.0));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn the_job_cancellation_stops_probing_and_decoding() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let folder = directory("cancel");
        let path = folder.join("long.wav");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in tone(20.0, 16_000, 300.0) {
            writer.write_sample((sample * 32767.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
        // Cancelled before probing: no format is examined.
        let cancelled: Cancel = Arc::new(|| true);
        let error = decode_file(&path, Layout::Mono, &cancelled, |_| {})
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "Import cancelled");
        // Cancelled part way: the job's flag is seen while reading packets.
        let flag = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let observed = (flag.clone(), checks.clone());
        let cancel: Cancel = Arc::new(move || {
            observed.1.fetch_add(1, Ordering::Relaxed);
            observed.0.load(Ordering::Relaxed)
        });
        let mut chunks = 0;
        let error = stream_file(
            &path,
            Layout::Mono,
            &cancel,
            |_| {},
            |_| {
                chunks += 1;
                flag.store(true, Ordering::Relaxed);
                Ok(())
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "Import cancelled");
        assert_eq!(chunks, 1, "decoding stopped after the cancellation");
        assert!(checks.load(Ordering::Relaxed) > 1);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_limited_stream_must_match_and_never_decodes_past_its_length() {
        let limit = |frames| Limit {
            rate: 16_000,
            channels: 1,
            frames,
        };
        let one = crate::audio_store::tests::flac(&[tone(1.25, 16_000, 220.0)]);
        let decoded = decode_flac(one.clone(), limit(20_000)).unwrap();
        assert_eq!(decoded.channels.len(), 1);
        assert_eq!(decoded.channels[0].len(), 20_000);
        assert!(decode_flac(one, limit(8_000)).is_err());
        let two = crate::audio_store::tests::flac(&[vec![0.1; 8_000], vec![0.2; 8_000]]);
        assert!(decode_flac(two, limit(8_000)).is_err());
    }

    #[test]
    fn unsupported_and_empty_files_report_a_readable_error() {
        let folder = directory("bad");
        let path = folder.join("notes.mp3");
        std::fs::write(&path, b"this is not audio at all").unwrap();
        let error = decode_file(&path, Layout::Mono, &uncancelled(), |_| {})
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("not supported") || error.contains("Could not read"),
            "{error}"
        );
        assert!(supported(Path::new("a.M4A")) && !supported(Path::new("a.exe")));
        let _ = std::fs::remove_dir_all(folder);
    }
}
