//! Optional retained audio beside a saved session (off unless the user keeps it).
//!
//! Format: 16 kHz, 16-bit FLAC. Speech recognition already runs at 16 kHz, so
//! nothing a re-transcription needs is lost, and lossless FLAC (pure Rust
//! encoder, no native codec) roughly halves the 32 KB/s of raw 16-bit PCM per
//! channel for speech. Calls keep two channels, the microphone and the call
//! audio, so playback and re-transcription can still tell them apart.
//!
//! The file `<history>/<id>.audio` starts with `MAGIC`, followed by records of
//! a little-endian `u32` length and a payload. Each payload is one segment of
//! at most `SEGMENT_SECONDS`: a header and an independent FLAC stream. Payloads
//! are sealed with the same Windows DPAPI protection as history records when
//! encryption at rest is on; sealed and plain records can be mixed while a
//! protection change is in progress. A recording that stops abruptly keeps
//! every complete segment.
use crate::history::protect;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::JoinHandle,
};

pub const FORMAT: &str = "flac-segments-1";
pub const RATE: u32 = 16_000;
const MAGIC: &[u8] = b"ARTICULATE-AUDIO-1\n";
const SEGMENT_MAGIC: &[u8] = b"SEG1";
const SEGMENT_SECONDS: usize = 30;
const SEGMENT_FRAMES: usize = SEGMENT_SECONDS * RATE as usize;
const MAX_RECORD_BYTES: usize = 32 * 1024 * 1024;
const QUEUE: usize = 512;

/// Stored with the session when audio is retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub format: String,
    pub duration_ms: u64,
    pub bytes: u64,
    /// What each channel holds: "microphone", "call" or "recording".
    pub channels: Vec<String>,
    /// Some audio could not be written (the writer fell behind or failed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
}

/// The side file of a session.
pub fn path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.audio"))
}

/// Where audio is written while recording. Protection changes and deletion
/// sweeps leave it alone until the recording finishes and it is renamed.
fn recording_path(path: &Path) -> PathBuf {
    path.with_extension("audio.recording")
}

/// Complete recordings interrupted by an earlier exit. Call once at startup,
/// before any recording starts. Returns how many were recovered.
pub fn recover_interrupted(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut recovered = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".audio.recording"))
        else {
            continue;
        };
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let complete = path(directory, id);
        if complete.exists() || fs::rename(entry.path(), &complete).is_err() {
            let _ = fs::remove_file(entry.path());
        } else {
            recovered += 1;
        }
    }
    recovered
}

fn to_i16(sample: f32) -> i32 {
    if sample.is_finite() {
        (sample.clamp(-1.0, 1.0) * 32767.0).round() as i32
    } else {
        0
    }
}

/// Encode equal-length channels starting at `start` frames as one payload.
fn encode_segment(channels: &[Vec<f32>], start: u64) -> Result<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let frames = channels.first().map_or(0, Vec::len);
    ensure!(
        channels.iter().all(|channel| channel.len() == frames) && frames > 0,
        "Audio segment channels differ in length"
    );
    let mut interleaved = Vec::with_capacity(frames * channels.len());
    for frame in 0..frames {
        for channel in channels {
            interleaved.push(to_i16(channel[frame]));
        }
    }
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|_| anyhow::anyhow!("Audio encoder settings are invalid"))?;
    let source =
        flacenc::source::MemSource::from_samples(&interleaved, channels.len(), 16, RATE as usize);
    let mut stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|error| anyhow::anyhow!("Could not encode audio: {error:?}"))?;
    // The encoder counts the short final block as the minimum block size.
    // FLAC excludes it, and strict decoders reject frames that disagree.
    stream
        .stream_info_mut()
        .set_block_sizes(config.block_size, config.block_size)
        .map_err(|error| anyhow::anyhow!("Could not encode audio: {error:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|_| anyhow::anyhow!("Could not encode audio"))?;
    let mut payload = Vec::with_capacity(17 + sink.as_slice().len());
    payload.extend_from_slice(SEGMENT_MAGIC);
    payload.extend_from_slice(&start.to_le_bytes());
    payload.extend_from_slice(&(frames as u32).to_le_bytes());
    payload.push(channels.len() as u8);
    payload.extend_from_slice(sink.as_slice());
    Ok(payload)
}

/// A decoded segment header.
struct Header {
    start: u64,
    frames: u32,
    channels: u8,
}

fn parse_segment(payload: &[u8]) -> Result<(Header, &[u8])> {
    ensure!(
        payload.len() > 17 && payload.starts_with(SEGMENT_MAGIC),
        "Retained audio is damaged"
    );
    let start = u64::from_le_bytes(payload[4..12].try_into().expect("eight bytes"));
    let frames = u32::from_le_bytes(payload[12..16].try_into().expect("four bytes"));
    let channels = payload[16];
    ensure!(
        (1..=8).contains(&channels) && frames as usize <= SEGMENT_FRAMES,
        "Retained audio is damaged"
    );
    Ok((
        Header {
            start,
            frames,
            channels,
        },
        &payload[17..],
    ))
}

/// Read the next record, `None` at the end or at a truncated final record.
fn next_record(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_RECORD_BYTES, "Retained audio is damaged");
    let mut record = vec![0; length];
    match reader.read_exact(&mut record) {
        Ok(()) => Ok(Some(record)),
        // A recording stopped mid-write keeps every complete segment.
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn open_reader(path: &Path) -> Result<BufReader<File>> {
    let mut reader = BufReader::new(File::open(path).context("The retained audio is missing")?);
    let mut magic = vec![0; MAGIC.len()];
    reader
        .read_exact(&mut magic)
        .context("Retained audio is damaged")?;
    ensure!(magic == MAGIC, "Retained audio is damaged");
    Ok(reader)
}

fn write_record(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(bytes)?;
    Ok(())
}

/// Visit decoded segments that end after `from_ms`, in order. Each call gets
/// the segment's start in frames and one 16 kHz vector per channel. Return
/// `false` to stop.
pub fn read(
    path: &Path,
    from_ms: u64,
    mut each: impl FnMut(u64, Vec<Vec<f32>>) -> Result<bool>,
) -> Result<()> {
    let from = from_ms * u64::from(RATE) / 1000;
    let mut reader = open_reader(path)?;
    while let Some(record) = next_record(&mut reader)? {
        let payload = protect::open(record)?;
        let (header, flac) = parse_segment(&payload)?;
        if header.start + u64::from(header.frames) <= from {
            continue;
        }
        let decoded = crate::media::decode_flac(
            flac.to_vec(),
            crate::media::Layout::Planar(usize::from(header.channels)),
        )?;
        let mut channels = decoded.channels;
        for channel in &mut channels {
            channel.truncate(header.frames as usize);
            channel.resize(header.frames as usize, 0.0);
        }
        if !each(header.start, channels)? {
            break;
        }
    }
    Ok(())
}

/// Whole retained audio, one vector per channel, for tests.
#[cfg(test)]
pub fn read_all(path: &Path) -> Result<Vec<Vec<f32>>> {
    let mut result: Vec<Vec<f32>> = Vec::new();
    read(path, 0, |start, channels| {
        if result.is_empty() {
            result = vec![Vec::new(); channels.len()];
        }
        for (index, channel) in channels.into_iter().enumerate() {
            if let Some(target) = result.get_mut(index) {
                target.resize(start as usize, 0.0);
                target.extend(channel);
            }
        }
        Ok(true)
    })?;
    Ok(result)
}

/// Rewrite every record sealed or plain. Returns whether the file changed.
pub fn reprotect(path: &Path, protect_records: bool) -> Result<bool> {
    let mut reader = open_reader(path)?;
    let mut records = Vec::new();
    let mut changed = false;
    while let Some(record) = next_record(&mut reader)? {
        changed |= protect::is_sealed(&record) != protect_records;
        records.push(record);
    }
    drop(reader);
    if !changed {
        return Ok(false);
    }
    let temporary = path.with_extension(format!("audio.{}.partial", std::process::id()));
    let result = (|| -> Result<()> {
        let mut writer = BufWriter::new(File::create(&temporary)?);
        writer.write_all(MAGIC)?;
        for record in records {
            let plain = protect::open(record)?;
            write_record(&mut writer, &protect::seal_if(protect_records, plain)?)?;
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| true)
}

/// Write the retained audio without DPAPI protection, for a portable backup.
pub fn copy_plain(path: &Path, writer: &mut impl Write) -> Result<u64> {
    let mut reader = open_reader(path)?;
    writer.write_all(MAGIC)?;
    let mut bytes = MAGIC.len() as u64;
    while let Some(record) = next_record(&mut reader)? {
        let plain = protect::open(record)?;
        parse_segment(&plain)?;
        write_record(writer, &plain)?;
        bytes += 4 + plain.len() as u64;
    }
    Ok(bytes)
}

/// Install retained audio from a backup, sealing it when `protect_records`.
pub fn install(bytes: &[u8], path: &Path, protect_records: bool) -> Result<()> {
    ensure!(bytes.starts_with(MAGIC), "The backed up audio is damaged");
    let mut reader = &bytes[MAGIC.len()..];
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(MAGIC);
    while let Some(record) = next_record(&mut reader)? {
        ensure!(
            !protect::is_sealed(&record),
            "The backed up audio is damaged"
        );
        parse_segment(&record)?;
        write_record(&mut output, &protect::seal_if(protect_records, record)?)?;
    }
    ensure!(reader.is_empty(), "The backed up audio is damaged");
    protect::write_atomic(path, &output)?;
    Ok(())
}

enum Message {
    Audio(usize, Vec<f32>),
    Finish,
}

/// Writes retained audio on its own thread so capture never waits for disk
/// or encoding. Channels are aligned by sample position from the start.
pub struct Recorder {
    sender: SyncSender<Message>,
    thread: std::sync::Mutex<Option<JoinHandle<Result<Info>>>>,
    dropped: Arc<AtomicBool>,
    path: PathBuf,
}

impl Recorder {
    /// Start writing the audio for `path`. `channels` names each channel;
    /// `protect` seals segments. The file appears at `path` when finished.
    pub fn start(path: PathBuf, channels: Vec<String>, protect_records: bool) -> Result<Self> {
        ensure!(
            (1..=2).contains(&channels.len()),
            "Retained audio has one or two channels"
        );
        let writing = recording_path(&path);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&writing)
            .context("Could not create the retained audio file")?;
        file.write_all(MAGIC)?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let dropped = Arc::new(AtomicBool::new(false));
        let writer_dropped = dropped.clone();
        let thread = std::thread::Builder::new()
            .name("retained-audio".into())
            .spawn(move || {
                write_segments(
                    file,
                    writing,
                    channels,
                    protect_records,
                    receiver,
                    writer_dropped,
                )
            })
            .context("Could not start the audio writer")?;
        Ok(Self {
            sender,
            thread: std::sync::Mutex::new(Some(thread)),
            dropped,
            path,
        })
    }

    /// Queue 16 kHz audio for one channel. Never blocks; audio that cannot be
    /// queued marks the recording incomplete.
    pub fn push(&self, channel: usize, pcm: Vec<f32>) {
        if pcm.is_empty() {
            return;
        }
        match self.sender.try_send(Message::Audio(channel, pcm)) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Write the remaining audio and return what was stored. Capture may
    /// still hold a reference; anything it pushes afterwards is ignored.
    pub fn finish(&self) -> Result<Info> {
        let Some(thread) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            bail!("The audio writer already finished");
        };
        let _ = self.sender.send(Message::Finish);
        let written = thread
            .join()
            .map_err(|_| anyhow::anyhow!("The audio writer stopped unexpectedly"))?;
        let writing = recording_path(&self.path);
        let mut info = match written {
            Ok(info) => info,
            Err(error) => {
                let _ = fs::remove_file(&writing);
                return Err(error);
            }
        };
        fs::rename(&writing, &self.path).context("Could not save the retained audio")?;
        info.incomplete |= self.dropped.load(Ordering::Relaxed);
        Ok(info)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn write_segments(
    file: File,
    path: PathBuf,
    names: Vec<String>,
    protect_records: bool,
    receiver: mpsc::Receiver<Message>,
    dropped: Arc<AtomicBool>,
) -> Result<Info> {
    let mut writer = BufWriter::new(file);
    let mut queues: Vec<Vec<f32>> = vec![Vec::new(); names.len()];
    let mut written: u64 = 0;
    let mut failed = None;
    let mut flush = |queues: &mut Vec<Vec<f32>>, all: bool, written: &mut u64| -> Result<()> {
        loop {
            let longest = queues.iter().map(Vec::len).max().unwrap_or(0);
            let shortest = queues.iter().map(Vec::len).min().unwrap_or(0);
            // A channel that stops arriving is treated as silence once the
            // other is two segments ahead, keeping memory bounded.
            let ready = if all {
                longest.min(SEGMENT_FRAMES)
            } else if shortest >= SEGMENT_FRAMES || longest >= 2 * SEGMENT_FRAMES {
                SEGMENT_FRAMES
            } else {
                0
            };
            if ready == 0 {
                return Ok(());
            }
            let segment: Vec<Vec<f32>> = queues
                .iter_mut()
                .map(|queue| {
                    let take = ready.min(queue.len());
                    let mut channel: Vec<f32> = queue.drain(..take).collect();
                    channel.resize(ready, 0.0);
                    channel
                })
                .collect();
            let payload = encode_segment(&segment, *written)?;
            write_record(&mut writer, &protect::seal_if(protect_records, payload)?)?;
            writer.flush()?;
            *written += ready as u64;
        }
    };
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Audio(channel, pcm) => {
                if failed.is_some() {
                    continue;
                }
                if let Some(queue) = queues.get_mut(channel) {
                    queue.extend(pcm);
                }
                if let Err(error) = flush(&mut queues, false, &mut written) {
                    failed = Some(error);
                    dropped.store(true, Ordering::Relaxed);
                }
            }
            Message::Finish => break,
        }
    }
    if failed.is_none()
        && let Err(error) = flush(&mut queues, true, &mut written)
    {
        failed = Some(error);
    }
    drop(flush);
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    if let Some(error) = failed
        && written == 0
    {
        return Err(error);
    }
    Ok(Info {
        format: FORMAT.into(),
        duration_ms: written * 1000 / u64::from(RATE),
        bytes: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        channels: names,
        incomplete: dropped.load(Ordering::Relaxed),
    })
}

/// Mixes separately delivered participant audio into one 16 kHz timeline for
/// retention. Each participant keeps a running position so small callback
/// jitter does not shift their audio.
#[derive(Default)]
pub struct Mixdown {
    origin: u64,
    buffer: Vec<f32>,
    next: HashMap<String, u64>,
}

/// At most this much audio waits in the mixdown before it is taken.
const MIXDOWN_FRAMES: usize = 120 * RATE as usize;

impl Mixdown {
    /// Add interleaved 16-bit audio that starts `start` seconds into the call.
    pub fn add(&mut self, voice: &str, start: f64, rate: u32, channels: u16, samples: &[i16]) {
        if rate == 0 || channels == 0 || samples.is_empty() {
            return;
        }
        let mono: Vec<f32> = samples
            .chunks_exact(usize::from(channels))
            .map(|frame| {
                frame.iter().map(|&s| f32::from(s) / 32768.0).sum::<f32>() / f32::from(channels)
            })
            .collect();
        let ratio = f64::from(rate) / f64::from(RATE);
        let count = (mono.len() as f64 / ratio).round() as usize;
        let converted: Vec<f32> = (0..count)
            .map(|index| {
                // Average the source span of each output sample (a simple
                // low-pass when downsampling).
                let from = ((index as f64 * ratio) as usize).min(mono.len() - 1);
                let to = (((index + 1) as f64 * ratio) as usize).clamp(from + 1, mono.len());
                mono[from..to].iter().sum::<f32>() / (to - from) as f32
            })
            .collect();
        let mut position = (start.max(0.0) * f64::from(RATE)).round() as u64;
        if let Some(&expected) = self.next.get(voice)
            && expected.abs_diff(position) <= u64::from(RATE) * 30 / 1000
        {
            position = expected;
        }
        self.next.insert(voice.to_owned(), position + count as u64);
        let mut skip = 0;
        if position < self.origin {
            skip = (self.origin - position) as usize;
            position = self.origin;
        }
        if skip >= converted.len() {
            return;
        }
        let offset = (position - self.origin) as usize;
        let end = offset + converted.len() - skip;
        if end > MIXDOWN_FRAMES {
            return;
        }
        if self.buffer.len() < end {
            self.buffer.resize(end, 0.0);
        }
        for (target, sample) in self.buffer[offset..end].iter_mut().zip(&converted[skip..]) {
            *target += sample;
        }
    }

    /// Take the mixed timeline up to `end` seconds, silent where nobody spoke.
    pub fn take_until(&mut self, end: f64) -> Vec<f32> {
        let end = (end.max(0.0) * f64::from(RATE)).round() as u64;
        if end <= self.origin {
            return Vec::new();
        }
        let count = (end - self.origin) as usize;
        self.buffer.resize(self.buffer.len().max(count), 0.0);
        let taken: Vec<f32> = self.buffer.drain(..count).collect();
        self.origin = end;
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::tests::tone;

    fn directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "articulate-audio-{label}-{}-{}",
            std::process::id(),
            crate::history::now_ms()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn recorded_channels_round_trip_losslessly_at_16_bit() {
        let folder = directory("roundtrip");
        let file = path(&folder, "0123456789abcdef0123456789abcdef");
        let recorder = Recorder::start(
            file.clone(),
            vec!["microphone".into(), "call".into()],
            false,
        )
        .unwrap();
        let microphone = tone(65.0, RATE, 220.0);
        let call = tone(64.0, RATE, 330.0);
        // Capture delivers both tracks a refresh at a time.
        for (index, chunk) in microphone.chunks(32_000).enumerate() {
            recorder.push(0, chunk.to_vec());
            if let Some(remote) = call.chunks(32_000).nth(index) {
                recorder.push(1, remote.to_vec());
            }
        }
        assert!(!file.exists() && recording_path(&file).exists());
        let info = recorder.finish().unwrap();
        assert!(file.exists() && !recording_path(&file).exists());
        assert_eq!(info.format, FORMAT);
        assert_eq!(info.duration_ms, 65_000);
        assert!(!info.incomplete);
        assert!(
            info.bytes < (65 * 16_000 * 2 * 2) as u64,
            "FLAC is smaller than raw PCM: {}",
            info.bytes
        );
        let channels = read_all(&file).unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].len(), microphone.len());
        for (stored, original) in channels[0].iter().zip(&microphone) {
            assert!((stored - original).abs() <= 1.0 / 32767.0 + 1e-6);
        }
        assert!((channels[1][1000] - call[1000]).abs() <= 1.0 / 32767.0 + 1e-6);
        // The shorter channel is padded with silence.
        assert!(channels[1][call.len()..].iter().all(|s| *s == 0.0));
        // Reading from a time skips earlier segments without decoding them.
        let mut starts = Vec::new();
        read(&file, 45_000, |start, _| {
            starts.push(start);
            Ok(true)
        })
        .unwrap();
        assert_eq!(starts, [30 * 16_000, 60 * 16_000]);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_truncated_recording_keeps_complete_segments() {
        let folder = directory("truncated");
        let file = path(&folder, "0123456789abcdef0123456789abcdef");
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, tone(40.0, RATE, 200.0));
        recorder.finish().unwrap();
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..bytes.len() - 100]).unwrap();
        let channels = read_all(&file).unwrap();
        assert_eq!(channels[0].len(), SEGMENT_FRAMES);
        fs::write(&file, b"not retained audio").unwrap();
        assert!(read_all(&file).is_err());
        // A recording interrupted by an exit is completed at the next start.
        fs::remove_file(&file).unwrap();
        fs::write(recording_path(&file), &bytes).unwrap();
        assert_eq!(recover_interrupted(&folder), 1);
        assert_eq!(read_all(&file).unwrap()[0].len(), 40 * 16_000);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn protection_seals_every_segment_and_backups_carry_plain_audio() {
        let folder = directory("protect");
        let file = path(&folder, "0123456789abcdef0123456789abcdef");
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], true).unwrap();
        let pcm = tone(35.0, RATE, 250.0);
        recorder.push(0, pcm.clone());
        recorder.finish().unwrap();
        let sealed = fs::read(&file).unwrap();
        let mut reader = &sealed[MAGIC.len()..];
        let mut records = 0;
        while let Some(record) = next_record(&mut reader).unwrap() {
            assert!(protect::is_sealed(&record));
            records += 1;
        }
        assert_eq!(records, 2);
        assert_eq!(read_all(&file).unwrap()[0].len(), pcm.len());
        assert!(reprotect(&file, false).unwrap());
        assert!(!reprotect(&file, false).unwrap());
        assert!(!protect::is_sealed(
            &fs::read(&file).unwrap()[MAGIC.len() + 4..]
        ));
        assert!(reprotect(&file, true).unwrap());
        let mut plain = Vec::new();
        copy_plain(&file, &mut plain).unwrap();
        let restored = folder.join("restored.audio");
        install(&plain, &restored, true).unwrap();
        assert_eq!(read_all(&restored).unwrap()[0].len(), pcm.len());
        assert!(install(b"bad", &restored, false).is_err());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn mixdown_places_participants_on_one_timeline() {
        let mut mix = Mixdown::default();
        let samples = vec![16_384i16; 960 * 2];
        mix.add("a", 0.5, 48_000, 2, &samples);
        // Jitter within 30 ms continues the same participant's audio.
        mix.add("a", 0.52, 48_000, 2, &samples);
        mix.add("b", 0.5, 48_000, 2, &samples);
        let taken = mix.take_until(1.0);
        assert_eq!(taken.len(), 16_000);
        assert_eq!(taken[7_999], 0.0);
        assert!((taken[8_000] - 1.0).abs() < 1e-4, "two voices add up");
        assert!((taken[8_400] - 0.5).abs() < 1e-4);
        assert_eq!(taken[8_640], 0.0);
        // Late audio before the taken range is dropped, not shifted.
        mix.add("c", 0.9, 16_000, 1, &[16_384; 3_200]);
        let next = mix.take_until(1.5);
        assert_eq!(next.len(), 8_000);
        assert!((next[0] - 0.5).abs() < 1e-4 && next[1_600] == 0.0);
    }
}
