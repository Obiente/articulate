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
//!
//! This module owns every audio file of a session: `<id>.audio`, the
//! `<id>.audio.recording` being written, and `<id>.audio.*.partial`
//! temporaries of a rewrite. Deletion (`remove`), protection changes
//! (`set_protection`, `Recorder::set_protection`) and startup recovery
//! (`recover_interrupted`) handle all of them, so no copy outlives its session
//! or keeps an old protection.
use crate::history::protect;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
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
const RECORDING: &str = "audio.recording";

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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The side file of a session.
pub fn path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.audio"))
}

/// Where audio is written while recording. Protection changes made during
/// the recording are applied to it by its recorder; history sweeps leave it
/// alone until it is renamed.
fn recording_path(path: &Path) -> PathBuf {
    path.with_extension(RECORDING)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Audio,
    Recording,
    Temporary,
}

/// The session and role of a file in the history folder, if it is retained
/// audio. Temporaries are named `<target>.<n>.partial`; atomic writes
/// elsewhere add a leading dot.
fn classify(name: &str) -> Option<(&str, Role)> {
    let (dotted, name) = match name.strip_prefix('.') {
        Some(name) => (true, name),
        None => (false, name),
    };
    let (id, rest) = name.split_once('.')?;
    if !valid_id(id) {
        return None;
    }
    let role = match rest {
        "audio" if !dotted => Role::Audio,
        RECORDING if !dotted => Role::Recording,
        _ if rest.starts_with("audio.") && rest.ends_with(".partial") => Role::Temporary,
        _ => return None,
    };
    Some((id, role))
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Finished audio paths an operation in this process is changing.
static EXCLUSIVE: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
static RELEASED: Condvar = Condvar::new();

/// Exclusive use of one session's audio files within this process, so a
/// deletion never interleaves with a rewrite that would bring a file back.
struct Exclusive(PathBuf);

impl Exclusive {
    fn acquire(audio: &Path) -> Self {
        let mut held = lock(&EXCLUSIVE);
        while held.contains(audio) {
            held = RELEASED.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        held.insert(audio.to_owned());
        Self(audio.to_owned())
    }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        lock(&EXCLUSIVE).remove(&self.0);
        RELEASED.notify_all();
    }
}

/// Temporaries being written now. Sweeps remove only the others, which an
/// earlier run left behind.
static WRITING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// A uniquely named temporary beside `target`, removed unless installed.
struct Temporary {
    path: PathBuf,
    installed: bool,
}

impl Temporary {
    fn create(target: &Path) -> Result<(Self, File)> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for _ in 0..64 {
            let path = target.with_file_name(format!(
                "{name}.{}-{}.partial",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            // Registered before it exists, so a sweep never takes it for a leftover.
            lock(&WRITING).insert(path.clone());
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            path,
                            installed: false,
                        },
                        file,
                    ));
                }
                Err(error) => {
                    lock(&WRITING).remove(&path);
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(error.into());
                    }
                }
            }
        }
        bail!("Could not create a temporary audio file")
    }

    fn install(mut self, target: &Path) -> Result<()> {
        fs::rename(&self.path, target)?;
        self.installed = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.installed {
            let _ = fs::remove_file(&self.path);
        }
        lock(&WRITING).remove(&self.path);
    }
}

/// Remove a temporary unless this process is writing it.
fn remove_stale(path: &Path) -> io::Result<()> {
    let writing = lock(&WRITING);
    if writing.contains(path) {
        return Ok(());
    }
    remove_if_present(path)
}

/// Clean up after an earlier exit. Call once at startup, before any
/// recording starts. An unfinished recording of a saved session keeps its
/// complete segments and becomes that session's audio. Recordings and audio
/// whose session was never saved or was deleted are removed, as are
/// temporaries an interrupted rewrite left (they may hold audio in an old
/// protection). Returns how many recordings were recovered.
pub fn recover_interrupted(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut saved = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut audio = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some((id, role)) = classify(&name) {
            audio.push((id.to_owned(), role, entry.path()));
            continue;
        }
        let Some((id, suffix)) = name.split_once('.') else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        // History records: the file, its backup, an unfinished write or a
        // copy set aside as unreadable (which can still be restored).
        if suffix == "deleted" {
            deleted.insert(id.to_owned());
        } else if matches!(suffix, "json" | "tmp" | "bak") || suffix.starts_with("corrupt-") {
            saved.insert(id.to_owned());
        }
    }
    let saved = |id: &str| saved.contains(id) && !deleted.contains(id);
    let mut recovered = 0;
    for (id, role, file) in audio {
        match role {
            Role::Temporary => {
                let _ = remove_stale(&file);
            }
            Role::Audio if !saved(&id) => {
                let _ = remove_if_present(&file);
            }
            Role::Audio => {}
            Role::Recording => {
                let complete = path(directory, &id);
                if saved(&id) && !complete.exists() && fs::rename(&file, &complete).is_ok() {
                    recovered += 1;
                } else {
                    let _ = fs::remove_file(&file);
                }
            }
        }
    }
    recovered
}

/// Remove every audio file of session `id`: its audio, an unfinished
/// recording and any temporaries. Used when the session or its audio is
/// deleted.
pub fn remove(directory: &Path, id: &str) -> Result<()> {
    ensure!(valid_id(id), "Invalid history identifier");
    let audio = path(directory, id);
    let _exclusive = Exclusive::acquire(&audio);
    remove_if_present(&audio)?;
    remove_if_present(&recording_path(&audio))?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(classify)
            .is_some_and(|(owner, role)| owner == id && role == Role::Temporary)
        {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

/// The outcome of sealing or unsealing all retained audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    pub changed: usize,
    pub failed: usize,
}

/// Seal (`protect`) or unseal the retained audio of every session in
/// `directory`, and remove temporaries an interrupted rewrite left behind.
/// Recordings in progress follow `Recorder::set_protection` instead.
pub fn set_protection(directory: &Path, protect_records: bool) -> Result<Migration> {
    let mut migration = Migration::default();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let Some(role) = entry
            .file_name()
            .to_str()
            .and_then(classify)
            .map(|(_, role)| role)
        else {
            continue;
        };
        let path = entry.path();
        match role {
            Role::Audio => match reprotect(&path, protect_records) {
                Ok(true) => migration.changed += 1,
                Ok(false) => {}
                Err(_) => migration.failed += 1,
            },
            Role::Temporary => {
                if remove_stale(&path).is_err() {
                    migration.failed += 1;
                }
            }
            Role::Recording => {}
        }
    }
    Ok(migration)
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
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_RECORD_BYTES, "Retained audio is damaged");
    let mut record = vec![0; length];
    match reader.read_exact(&mut record) {
        Ok(()) => Ok(Some(record)),
        // A recording stopped mid-write keeps every complete segment.
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
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

/// Decode one segment. Its header bounds the work: the FLAC stream inside
/// must have the header's channels at 16 kHz and is never decoded past the
/// header's length, whatever the stream itself claims.
fn decode_segment(payload: &[u8]) -> Result<(u64, Vec<Vec<f32>>)> {
    let (header, flac) = parse_segment(payload)?;
    let decoded = crate::media::decode_flac(
        flac.to_vec(),
        crate::media::Limit {
            rate: RATE,
            channels: usize::from(header.channels),
            frames: u64::from(header.frames),
        },
    )
    .context("Retained audio is damaged")?;
    let mut channels = decoded.channels;
    for channel in &mut channels {
        channel.truncate(header.frames as usize);
        channel.resize(header.frames as usize, 0.0);
    }
    Ok((header.start, channels))
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
        let (header, _) = parse_segment(&payload)?;
        if header.start + u64::from(header.frames) <= from {
            continue;
        }
        let (start, channels) = decode_segment(&payload)?;
        if !each(start, channels)? {
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

/// Rewrite the records of `path` sealed or plain, streaming through a
/// temporary. Returns whether anything changed and how many records the file
/// holds.
fn rewrite(path: &Path, protect_records: bool) -> Result<(bool, usize)> {
    let mut reader = open_reader(path)?;
    let mut records = 0;
    let mut mismatched = false;
    while let Some(record) = next_record(&mut reader)? {
        mismatched |= protect::is_sealed(&record) != protect_records;
        records += 1;
    }
    drop(reader);
    if !mismatched {
        return Ok((false, records));
    }
    let (temporary, file) = Temporary::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(MAGIC)?;
    let mut reader = open_reader(path)?;
    let mut records = 0;
    while let Some(record) = next_record(&mut reader)? {
        let record = if protect::is_sealed(&record) == protect_records {
            record
        } else {
            protect::seal_if(protect_records, protect::open(record)?)?
        };
        write_record(&mut writer, &record)?;
        records += 1;
    }
    drop(reader);
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    temporary.install(path)?;
    Ok((true, records))
}

/// Rewrite every record sealed or plain. Returns whether the file changed.
pub fn reprotect(path: &Path, protect_records: bool) -> Result<bool> {
    let _exclusive = Exclusive::acquire(path);
    Ok(rewrite(path, protect_records)?.0)
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
    let _exclusive = Exclusive::acquire(path);
    let (temporary, mut file) = Temporary::create(path)?;
    file.write_all(&output)?;
    file.sync_all()?;
    drop(file);
    temporary.install(path)
}

/// Whether everything a recorder wrote has the protection last asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protection {
    Settled,
    /// The writer has not rewritten earlier segments yet.
    Pending,
    /// Earlier segments could not be rewritten.
    Failed(String),
}

/// Records written so far, by protection.
#[derive(Default)]
struct Written {
    sealed: usize,
    plain: usize,
    error: Option<String>,
}

struct Shared {
    protect: AtomicBool,
    /// Counts protection requests, so a failed rewrite is retried once per request.
    request: AtomicU64,
    written: Mutex<Written>,
    dropped: AtomicBool,
}

enum Message {
    /// Audio for `channel` starting `start` frames into that channel.
    Audio {
        channel: usize,
        start: u64,
        pcm: Vec<f32>,
    },
    /// The requested protection changed.
    Protect,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Recording,
    Finished,
    Discarded,
}

/// Writes retained audio on its own thread so capture never waits for disk
/// or encoding. Every chunk carries its position in its channel, so audio
/// that could not be queued becomes silence instead of shifting what follows.
///
/// A recording is published at `path` only by `finish`. `discard`, or
/// dropping the last handle before `finish`, removes everything it wrote.
pub struct Recorder {
    sender: SyncSender<Message>,
    thread: Mutex<Option<JoinHandle<Result<Info>>>>,
    shared: Arc<Shared>,
    /// Frames pushed so far per channel.
    positions: Vec<AtomicU64>,
    state: Mutex<State>,
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
        let shared = Arc::new(Shared {
            protect: AtomicBool::new(protect_records),
            request: AtomicU64::new(0),
            written: Mutex::new(Written::default()),
            dropped: AtomicBool::new(false),
        });
        let positions = channels.iter().map(|_| AtomicU64::new(0)).collect();
        let writer = Writer {
            queues: vec![Vec::new(); channels.len()],
            names: channels,
            file: Some(BufWriter::new(file)),
            recording: writing,
            audio: path.clone(),
            shared: shared.clone(),
            written: 0,
            failed: None,
            gaps: false,
            attempted: None,
        };
        let thread = std::thread::Builder::new()
            .name("retained-audio".into())
            .spawn(move || writer.run(receiver));
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                let _ = fs::remove_file(recording_path(&path));
                return Err(error).context("Could not start the audio writer");
            }
        };
        Ok(Self {
            sender,
            thread: Mutex::new(Some(thread)),
            shared,
            positions,
            state: Mutex::new(State::Recording),
            path,
        })
    }

    /// Queue 16 kHz audio for one channel, continuing where its previous
    /// chunk ended. Never blocks; audio that cannot be queued is kept as
    /// silence and marks the recording incomplete.
    pub fn push(&self, channel: usize, pcm: Vec<f32>) {
        let Some(position) = self.positions.get(channel) else {
            return;
        };
        if pcm.is_empty() {
            return;
        }
        let start = position.fetch_add(pcm.len() as u64, Ordering::Relaxed);
        if self
            .sender
            .try_send(Message::Audio {
                channel,
                start,
                pcm,
            })
            .is_err()
        {
            self.shared.dropped.store(true, Ordering::Relaxed);
        }
    }

    /// Seal or unseal this recording, including segments already written,
    /// as encryption at rest changes while it records. See `protection`.
    pub fn set_protection(&self, protect_records: bool) {
        self.shared
            .protect
            .store(protect_records, Ordering::Release);
        self.shared.request.fetch_add(1, Ordering::AcqRel);
        lock(&self.shared.written).error = None;
        // A full queue wakes the writer anyway.
        let _ = self.sender.try_send(Message::Protect);
    }

    /// Whether the stored audio has the protection last asked for.
    pub fn protection(&self) -> Protection {
        if *lock(&self.state) == State::Discarded {
            return Protection::Settled;
        }
        let protect_records = self.shared.protect.load(Ordering::Acquire);
        let written = lock(&self.shared.written);
        let mismatched = if protect_records {
            written.plain
        } else {
            written.sealed
        };
        match &written.error {
            _ if mismatched == 0 => Protection::Settled,
            Some(error) => Protection::Failed(error.clone()),
            None if *lock(&self.state) == State::Finished => {
                Protection::Failed("The retained audio could not be converted.".into())
            }
            None => Protection::Pending,
        }
    }

    /// Write the remaining audio, publish the file and return what was
    /// stored. Capture may still hold a reference; anything it pushes
    /// afterwards is ignored. On failure nothing is kept.
    pub fn finish(&self) -> Result<Info> {
        let Some(thread) = lock(&self.thread).take() else {
            bail!("The audio writer already finished");
        };
        let _ = self.sender.send(Message::Finish);
        let result = thread
            .join()
            .map_err(|_| anyhow::anyhow!("The audio writer stopped unexpectedly"))
            .and_then(|written| written)
            .and_then(|mut info| {
                let _exclusive = Exclusive::acquire(&self.path);
                fs::rename(recording_path(&self.path), &self.path)
                    .context("Could not save the retained audio")?;
                info.incomplete |= self.shared.dropped.load(Ordering::Relaxed);
                Ok(info)
            });
        match &result {
            Ok(_) => *lock(&self.state) = State::Finished,
            Err(_) => self.remove_files(),
        }
        result
    }

    /// Stop writing and remove everything this recording stored, including
    /// a finished file: for audio whose session will not be saved.
    pub fn discard(&self) {
        if let Some(thread) = lock(&self.thread).take() {
            let _ = self.sender.send(Message::Finish);
            let _ = thread.join();
        }
        self.remove_files();
    }

    fn remove_files(&self) {
        let _exclusive = Exclusive::acquire(&self.path);
        let _ = remove_if_present(&recording_path(&self.path));
        let _ = remove_if_present(&self.path);
        *lock(&self.state) = State::Discarded;
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Never publish audio nobody committed to keeping.
        if *lock(&self.state) == State::Recording {
            self.discard();
        }
    }
}

/// The writer thread's state.
struct Writer {
    names: Vec<String>,
    file: Option<BufWriter<File>>,
    recording: PathBuf,
    /// The finished path, which names this session's audio for `Exclusive`.
    audio: PathBuf,
    shared: Arc<Shared>,
    /// Audio after `written` frames, per channel.
    queues: Vec<Vec<f32>>,
    written: u64,
    failed: Option<anyhow::Error>,
    /// Silence stood in for audio that never arrived.
    gaps: bool,
    /// The protection request whose rewrite failed; retried at the end.
    attempted: Option<u64>,
}

impl Writer {
    fn run(mut self, receiver: mpsc::Receiver<Message>) -> Result<Info> {
        while let Ok(message) = receiver.recv() {
            match message {
                Message::Audio {
                    channel,
                    start,
                    pcm,
                } => {
                    // After a write failure audio is ignored, but a
                    // protection change still applies to what was written.
                    if self.failed.is_none()
                        && let Err(error) = self.place(channel, start, &pcm)
                    {
                        self.failed = Some(error);
                        self.shared.dropped.store(true, Ordering::Relaxed);
                    }
                    self.follow_protection(false);
                }
                Message::Protect => self.follow_protection(false),
                Message::Finish => break,
            }
        }
        if self.failed.is_none()
            && let Err(error) = self.flush(true)
        {
            self.failed = Some(error);
        }
        self.follow_protection(true);
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            file.get_ref().sync_all()?;
        }
        let incomplete =
            self.gaps || self.failed.is_some() || self.shared.dropped.load(Ordering::Relaxed);
        if self.written == 0
            && let Some(error) = self.failed
        {
            return Err(error);
        }
        Ok(Info {
            format: FORMAT.into(),
            duration_ms: self.written * 1000 / u64::from(RATE),
            bytes: fs::metadata(&self.recording).map(|m| m.len()).unwrap_or(0),
            channels: self.names,
            incomplete,
        })
    }

    /// Queue `pcm`, which starts `start` frames into `channel`. Audio that
    /// never arrived before it is written as silence; audio for a time that
    /// is already queued or written is dropped rather than shifted.
    fn place(&mut self, channel: usize, start: u64, pcm: &[f32]) -> Result<()> {
        if channel >= self.queues.len() {
            return Ok(());
        }
        let next = |writer: &Self| writer.written + writer.queues[channel].len() as u64;
        while next(self) < start {
            self.gaps = true;
            // A segment at a time keeps memory bounded however long the gap.
            let step = (start - next(self)).min(SEGMENT_FRAMES as u64) as usize;
            let queue = &mut self.queues[channel];
            queue.resize(queue.len() + step, 0.0);
            self.flush(false)?;
        }
        let skip = (next(self) - start) as usize;
        if skip < pcm.len() {
            self.queues[channel].extend_from_slice(&pcm[skip..]);
        }
        self.flush(false)
    }

    /// Encode and write every complete segment, or everything with `all`.
    fn flush(&mut self, all: bool) -> Result<()> {
        loop {
            let longest = self.queues.iter().map(Vec::len).max().unwrap_or(0);
            let shortest = self.queues.iter().map(Vec::len).min().unwrap_or(0);
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
            let segment: Vec<Vec<f32>> = self
                .queues
                .iter_mut()
                .map(|queue| {
                    let take = ready.min(queue.len());
                    let mut channel: Vec<f32> = queue.drain(..take).collect();
                    channel.resize(ready, 0.0);
                    channel
                })
                .collect();
            let protect_records = self.shared.protect.load(Ordering::Acquire);
            let payload = encode_segment(&segment, self.written)?;
            let record = protect::seal_if(protect_records, payload)?;
            let file = self.file.as_mut().context("The audio file is closed")?;
            write_record(file, &record)?;
            file.flush()?;
            let mut written = lock(&self.shared.written);
            if protect_records {
                written.sealed += 1;
            } else {
                written.plain += 1;
            }
            drop(written);
            self.written += ready as u64;
        }
    }

    /// Bring segments already written to the requested protection. A failed
    /// rewrite is not repeated for the same request until the last call.
    fn follow_protection(&mut self, last: bool) {
        let protect_records = self.shared.protect.load(Ordering::Acquire);
        let request = self.shared.request.load(Ordering::Acquire);
        let mismatched = {
            let written = lock(&self.shared.written);
            if protect_records {
                written.plain
            } else {
                written.sealed
            }
        };
        if mismatched == 0 || (!last && self.attempted == Some(request)) {
            return;
        }
        let result = self.rewrite(protect_records);
        let mut written = lock(&self.shared.written);
        match result {
            Ok(records) => {
                *written = Written::default();
                if protect_records {
                    written.sealed = records;
                } else {
                    written.plain = records;
                }
                self.attempted = None;
            }
            Err(error) => {
                written.error = Some(format!("{error:#}"));
                self.attempted = Some(request);
            }
        }
    }

    fn rewrite(&mut self, protect_records: bool) -> Result<usize> {
        let _exclusive = Exclusive::acquire(&self.audio);
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            file.get_ref().sync_all()?;
        }
        let result = rewrite(&self.recording, protect_records);
        // Keep appending whether or not the rewrite succeeded.
        match OpenOptions::new().append(true).open(&self.recording) {
            Ok(file) => self.file = Some(BufWriter::new(file)),
            Err(error) => {
                let error = anyhow::Error::from(error).context("Could not reopen the audio file");
                let message = format!("{error:#}");
                self.failed.get_or_insert(error);
                bail!(message);
            }
        }
        result.map(|(_, records)| records)
    }
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
pub(crate) mod tests {
    use super::*;
    use crate::media::tests::tone;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    /// A FLAC stream of `channels` as a segment holds it, without its header.
    pub(crate) fn flac(channels: &[Vec<f32>]) -> Vec<u8> {
        encode_segment(channels, 0).unwrap()[17..].to_vec()
    }

    fn directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "articulate-audio-{label}-{}-{}",
            std::process::id(),
            crate::history::now_ms()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn records(file: &Path) -> Vec<bool> {
        let bytes = fs::read(file).unwrap();
        let mut reader = &bytes[MAGIC.len()..];
        let mut sealed = Vec::new();
        while let Some(record) = next_record(&mut reader).unwrap() {
            sealed.push(protect::is_sealed(&record));
        }
        sealed
    }

    #[test]
    fn recorded_channels_round_trip_losslessly_at_16_bit() {
        let folder = directory("roundtrip");
        let file = path(&folder, ID);
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
        // Dropping a finished recorder keeps its audio.
        drop(recorder);
        assert!(file.exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_truncated_recording_keeps_complete_segments() {
        let folder = directory("truncated");
        let file = path(&folder, ID);
        fs::write(folder.join(format!("{ID}.json")), b"{}").unwrap();
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
    fn recovery_keeps_only_audio_of_saved_sessions_and_removes_leftovers() {
        let folder = directory("recovery");
        let saved = "11111111111111111111111111111111";
        let unsaved = "22222222222222222222222222222222";
        let deleted = "33333333333333333333333333333333";
        let quarantined = "44444444444444444444444444444444";
        fs::write(folder.join(format!("{saved}.json")), b"{}").unwrap();
        fs::write(folder.join(format!("{deleted}.json")), b"{}").unwrap();
        fs::write(folder.join(format!("{deleted}.deleted")), b"").unwrap();
        fs::write(
            folder.join(format!("{quarantined}.corrupt-{saved}")),
            b"damaged",
        )
        .unwrap();
        let recorder =
            Recorder::start(path(&folder, saved), vec!["recording".into()], false).unwrap();
        recorder.push(0, vec![0.25; 16_000]);
        recorder.finish().unwrap();
        let audio = fs::read(path(&folder, saved)).unwrap();
        for id in [unsaved, deleted, quarantined] {
            fs::write(path(&folder, id), &audio).unwrap();
        }
        fs::write(recording_path(&path(&folder, unsaved)), &audio).unwrap();
        // A crash while decrypting leaves plaintext beside the sealed audio.
        fs::write(folder.join(format!("{saved}.audio.4242.partial")), &audio).unwrap();
        fs::write(folder.join(format!(".{saved}.audio.7-1.partial")), &audio).unwrap();
        fs::write(folder.join("unrelated.audio"), b"kept").unwrap();
        assert_eq!(recover_interrupted(&folder), 0);
        assert_eq!(
            names(&folder),
            [
                format!("{saved}.audio"),
                format!("{saved}.json"),
                format!("{deleted}.deleted"),
                format!("{deleted}.json"),
                format!("{quarantined}.audio"),
                format!("{quarantined}.corrupt-{saved}"),
                "unrelated.audio".to_owned(),
            ]
        );
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn protection_seals_every_segment_and_backups_carry_plain_audio() {
        let folder = directory("protect");
        let file = path(&folder, ID);
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], true).unwrap();
        let pcm = tone(35.0, RATE, 250.0);
        recorder.push(0, pcm.clone());
        recorder.finish().unwrap();
        assert_eq!(records(&file), [true, true]);
        assert_eq!(read_all(&file).unwrap()[0].len(), pcm.len());
        assert!(reprotect(&file, false).unwrap());
        assert!(!reprotect(&file, false).unwrap());
        assert_eq!(records(&file), [false, false]);
        assert!(reprotect(&file, true).unwrap());
        let mut plain = Vec::new();
        copy_plain(&file, &mut plain).unwrap();
        let restored = folder.join("restored.audio");
        install(&plain, &restored, true).unwrap();
        assert_eq!(read_all(&restored).unwrap()[0].len(), pcm.len());
        assert!(install(b"bad", &restored, false).is_err());
        assert_eq!(
            names(&folder),
            [format!("{ID}.audio"), "restored.audio".into()]
        );
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_recording_follows_protection_changes_made_while_it_records() {
        let folder = directory("follow");
        let file = path(&folder, ID);
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, tone(31.0, RATE, 200.0));
        // The first segment is on disk unsealed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while records(&recording_path(&file)).is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(recorder.protection(), Protection::Settled);
        recorder.set_protection(true);
        // A history sweep during the recording leaves the recording to it.
        assert_eq!(set_protection(&folder, true).unwrap(), Migration::default());
        while recorder.protection() != Protection::Settled {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(records(&recording_path(&file)), [true]);
        recorder.push(0, tone(10.0, RATE, 300.0));
        let info = recorder.finish().unwrap();
        assert_eq!(info.duration_ms, 41_000);
        assert_eq!(records(&file), [true, true]);
        assert_eq!(read_all(&file).unwrap()[0].len(), 41 * 16_000);
        // And back, applied when the recording finishes.
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], true).unwrap();
        recorder.push(0, tone(31.0, RATE, 200.0));
        recorder.set_protection(false);
        recorder.finish().unwrap();
        assert_eq!(recorder.protection(), Protection::Settled);
        assert_eq!(records(&file), [false, false]);
        assert_eq!(names(&folder), [format!("{ID}.audio")]);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn discarded_or_dropped_recordings_leave_no_audio() {
        let folder = directory("discard");
        let file = path(&folder, ID);
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, tone(35.0, RATE, 200.0));
        drop(recorder);
        assert!(names(&folder).is_empty(), "{:?}", names(&folder));
        // Discarding after finishing removes the published file too.
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, vec![0.1; 16_000]);
        recorder.finish().unwrap();
        assert!(file.exists());
        recorder.discard();
        assert!(names(&folder).is_empty());
        assert!(recorder.finish().is_err());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn missing_chunks_become_silence_and_later_audio_keeps_its_time() {
        let folder = directory("gaps");
        let file = path(&folder, ID);
        let recorder = Recorder::start(
            file.clone(),
            vec!["microphone".into(), "call".into()],
            false,
        )
        .unwrap();
        // The writer receives a chunk that starts two seconds after the
        // previous one ended, as when the queue was full in between.
        recorder.push(0, vec![0.5; 16_000]);
        recorder.positions[0].fetch_add(32_000, Ordering::Relaxed);
        recorder.push(0, vec![0.25; 16_000]);
        recorder.push(1, vec![0.125; 64_000]);
        let info = recorder.finish().unwrap();
        assert!(info.incomplete);
        assert_eq!(info.duration_ms, 4_000);
        let channels = read_all(&file).unwrap();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(channels[0][15_999], 0.5));
        assert!(channels[0][16_000..48_000].iter().all(|s| *s == 0.0));
        assert!(near(channels[0][48_000], 0.25) && near(channels[0][63_999], 0.25));
        assert!(near(channels[1][63_999], 0.125));
        // Overlapping audio is dropped rather than shifting what follows.
        let mut writer = Writer {
            names: vec!["recording".into()],
            file: Some(BufWriter::new(
                File::create(folder.join("overlap.audio.recording")).unwrap(),
            )),
            recording: folder.join("overlap.audio.recording"),
            audio: folder.join("overlap.audio"),
            shared: Arc::new(Shared {
                protect: AtomicBool::new(false),
                request: AtomicU64::new(0),
                written: Mutex::new(Written::default()),
                dropped: AtomicBool::new(false),
            }),
            queues: vec![Vec::new()],
            written: 0,
            failed: None,
            gaps: false,
            attempted: None,
        };
        writer.place(0, 0, &[1.0; 100]).unwrap();
        writer.place(0, 50, &[2.0; 100]).unwrap();
        assert_eq!(writer.queues[0].len(), 150);
        assert_eq!(writer.queues[0][99], 1.0);
        assert_eq!(writer.queues[0][100], 2.0);
        assert!(!writer.gaps);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_segment_never_decodes_beyond_its_stated_length() {
        // A two-second FLAC stream under a header that claims one second.
        let long = encode_segment(&[vec![0.25; 32_000]], 0).unwrap();
        let mut forged = long.clone();
        forged[12..16].copy_from_slice(&16_000u32.to_le_bytes());
        let error = decode_segment(&forged).unwrap_err();
        assert!(format!("{error:#}").contains("damaged"), "{error:#}");
        // Or a different channel count than the header says.
        forged.clone_from(&long);
        forged[16] = 2;
        assert!(decode_segment(&forged).is_err());
        let (start, channels) = decode_segment(&long).unwrap();
        assert_eq!((start, channels.len(), channels[0].len()), (0, 1, 32_000));
    }

    #[test]
    fn removal_and_migration_cover_every_owned_file() {
        let folder = directory("owned");
        let file = path(&folder, ID);
        let recorder = Recorder::start(file.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, vec![0.1; 16_000]);
        recorder.finish().unwrap();
        let stale = folder.join(format!("{ID}.audio.999-1.partial"));
        fs::copy(&file, &stale).unwrap();
        fs::write(folder.join("unrelated.txt"), b"kept").unwrap();
        let migration = set_protection(&folder, true).unwrap();
        assert_eq!(
            migration,
            Migration {
                changed: 1,
                failed: 0
            }
        );
        assert!(!stale.exists(), "leftovers in the old form are removed");
        assert_eq!(records(&file), [true]);
        // A temporary being written now is left alone.
        let (writing, handle) = Temporary::create(&file).unwrap();
        drop(handle);
        set_protection(&folder, false).unwrap();
        assert!(writing.path.exists());
        drop(writing);
        fs::copy(&file, &stale).unwrap();
        fs::copy(&file, recording_path(&file)).unwrap();
        remove(&folder, ID).unwrap();
        assert_eq!(names(&folder), ["unrelated.txt"]);
        assert!(remove(&folder, "../escape").is_err());
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
