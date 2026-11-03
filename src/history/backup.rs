//! Portable backups, restore and bulk Markdown export.
//!
//! A backup is one ZIP archive, optionally wrapped in passphrase encryption
//! (the age format with scrypt):
//!
//! - `manifest.json`: format name, archive version, record schema, counts.
//! - `history/<id>.json`: each saved session in the plaintext record format.
//! - `library.json`: vocabulary corrections and voice macros.
//! - `settings.json`: preferences without the Discord pairing key, model and
//!   tool paths, settings tied to this device or a running session (see
//!   `EXCLUDED_SETTINGS`), or vocabulary (which lives in `library.json`).
//! - `audio/<id>.audio`: retained session audio, only when the request opts
//!   in with `include_audio`. It is written without DPAPI protection; the
//!   archive passphrase protects it instead.
//!
//! Restore reads and validates the entire archive, including every audio
//! entry, before changing anything. Expanded entries count against a total
//! size budget and the parts kept in memory against a smaller one; audio is
//! staged in files rather than memory. Restore adds missing sessions and never
//! silently replaces a local session that is newer than or as new as the
//! archived copy. Each session is installed together with its audio, or not
//! at all. Sessions deleted on this device are reported, not restored.
use super::{History, RecordWarning, Session, encode_record, parse_record};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const FORMAT: &str = "articulate-backup";
pub const VERSION: u32 = 1;
/// Settings that are secret, tied to this machine or its devices, or describe
/// a live session never leave it, and a restore keeps this device's values.
/// Encryption and retention change only through their own controls.
pub const EXCLUDED_SETTINGS: &[&str] = &[
    "discord_pairing_key",
    "discord_debugger_port",
    "discord_flavor",
    "discord_standard_configured",
    "model_path",
    "assort",
    "vencord_source",
    "microphone",
    "output",
    "start_with_windows",
    "encrypt_at_rest",
    "retention",
    "entries",
    "macros",
];
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = super::MAX_RECORDS + 16;
const MAX_SIDE_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Retained audio added to one archive, keeping it below `MAX_ARCHIVE_BYTES`.
const MAX_AUDIO_BYTES: u64 = 768 * 1024 * 1024;
/// Everything expanded from one archive, audio included.
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Sessions, preferences and vocabulary held in memory while restoring.
const MAX_RETAINED_BYTES: u64 = 512 * 1024 * 1024;
const AGE_MAGIC: &[u8] = b"age-encryption.org/v1";

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    record_schema: u32,
    created_ms: u64,
    app_version: String,
    sessions: usize,
}

pub struct Request {
    pub destination: PathBuf,
    /// Encrypt the archive with this passphrase when set.
    pub passphrase: Option<String>,
    /// The current preferences as JSON. Excluded keys are removed here.
    pub settings: serde_json::Value,
    /// `crate::library::export` output for the current vocabulary and macros.
    pub library: String,
    /// Include retained session audio. Off by default: audio is large and
    /// more sensitive than text.
    pub include_audio: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Created {
    pub path: PathBuf,
    pub sessions: usize,
    pub encrypted: bool,
    /// Records that could not be read and are therefore missing from the archive.
    pub skipped: Vec<RecordWarning>,
    /// Sessions whose retained audio was included.
    pub audio: usize,
    /// Sessions whose retained audio was left out (unreadable or over the size limit).
    pub audio_skipped: Vec<String>,
}

pub struct RestoreRequest {
    pub source: PathBuf,
    pub passphrase: Option<String>,
    /// Replace local sessions that are older than the archived copy. When
    /// false, those are listed in `Restored::newer_in_backup` instead.
    pub replace_older: bool,
    /// Return the archived preferences and vocabulary for the caller to merge.
    pub include_settings: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Restored {
    pub sessions_in_archive: usize,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    /// The local copy is as new or newer, so it was kept.
    pub kept_local: Vec<String>,
    /// The archived copy is newer but `replace_older` was not set.
    pub newer_in_backup: Vec<String>,
    /// Deleted on this device; not restored.
    pub deleted_locally: Vec<String>,
    pub failed: Vec<(String, String)>,
    /// Portable preferences for the caller to merge. Never applied here.
    pub settings: Option<serde_json::Value>,
    /// Validated library JSON for the caller to merge. Never applied here.
    pub library: Option<String>,
    /// Sessions whose retained audio was restored.
    pub audio: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Exported {
    pub folder: PathBuf,
    pub files: usize,
    pub skipped: Vec<RecordWarning>,
}

/// Remove secret and machine-specific preferences.
pub fn portable_settings(mut settings: serde_json::Value) -> serde_json::Value {
    if let Some(object) = settings.as_object_mut() {
        for key in EXCLUDED_SETTINGS {
            object.remove(*key);
        }
    }
    settings
}

fn now_ms() -> u64 {
    super::now_ms()
}

/// A sibling temporary that is removed unless committed.
struct Partial {
    path: PathBuf,
    committed: bool,
}
impl Drop for Partial {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn partial_beside(destination: &Path) -> Result<(Partial, File)> {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..64 {
        let path = parent.join(format!(
            ".articulate-backup-{}-{}.partial",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok((
                    Partial {
                        path,
                        committed: false,
                    },
                    file,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Could not create a temporary backup file")
}

struct Written {
    sessions: usize,
    skipped: Vec<RecordWarning>,
    audio: usize,
    audio_skipped: Vec<String>,
}

fn write_archive<W: Write>(history: &History, request: &Request, sink: W) -> Result<(W, Written)> {
    let mut zip = zip::ZipWriter::new_stream(sink);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    // FLAC is already compressed.
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let mut sessions = 0;
    let mut audio = 0;
    let mut audio_bytes = 0;
    let mut audio_skipped = Vec::new();
    let skipped = history.for_each_session(|session| {
        zip.start_file(format!("history/{}.json", session.id), options)?;
        zip.write_all(&encode_record(&session)?)?;
        sessions += 1;
        if request.include_audio && session.audio.is_some() {
            let path = crate::audio_store::path(&history.directory, &session.id);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut plain = Vec::new();
            if size == 0
                || audio_bytes + size > MAX_AUDIO_BYTES
                || crate::audio_store::copy_plain(&path, &mut plain).is_err()
            {
                audio_skipped.push(session.id.clone());
            } else {
                zip.start_file(format!("audio/{}.audio", session.id), stored)?;
                zip.write_all(&plain)?;
                audio_bytes += plain.len() as u64;
                audio += 1;
            }
        }
        Ok(())
    })?;
    crate::library::parse(&request.library).context("The vocabulary could not be backed up")?;
    zip.start_file("library.json", options)?;
    zip.write_all(request.library.as_bytes())?;
    zip.start_file("settings.json", options)?;
    zip.write_all(&serde_json::to_vec_pretty(&portable_settings(
        request.settings.clone(),
    ))?)?;
    zip.start_file("manifest.json", options)?;
    zip.write_all(&serde_json::to_vec_pretty(&Manifest {
        format: FORMAT.into(),
        version: VERSION,
        record_schema: super::RECORD_SCHEMA,
        created_ms: now_ms(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        sessions,
    })?)?;
    Ok((
        zip.finish()?.into_inner(),
        Written {
            sessions,
            skipped,
            audio,
            audio_skipped,
        },
    ))
}

/// Write a complete archive to `request.destination`, replacing it atomically.
pub fn create(history: &History, request: &Request) -> Result<Created> {
    let passphrase = request
        .passphrase
        .as_deref()
        .filter(|passphrase| !passphrase.is_empty());
    if let Some(passphrase) = passphrase {
        ensure!(
            passphrase.chars().count() >= 8,
            "Use a passphrase of at least 8 characters."
        );
    }
    let (mut partial, file) = partial_beside(&request.destination)?;
    let buffered = BufWriter::new(file);
    let (file, written) = match passphrase {
        Some(passphrase) => {
            let encryptor = age::Encryptor::with_user_passphrase(age::secrecy::SecretString::from(
                passphrase.to_owned(),
            ));
            let encrypted = encryptor.wrap_output(buffered)?;
            let (encrypted, written) = write_archive(history, request, encrypted)?;
            (encrypted.finish()?, written)
        }
        None => write_archive(history, request, buffered)?,
    };
    let file = file.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&partial.path, &request.destination)?;
    partial.committed = true;
    Ok(Created {
        path: request.destination.clone(),
        sessions: written.sessions,
        encrypted: passphrase.is_some(),
        skipped: written.skipped,
        audio: written.audio,
        audio_skipped: written.audio_skipped,
    })
}

/// A fully validated archive. No session has been changed yet.
pub struct Archive {
    pub sessions: Vec<Session>,
    pub settings: Option<serde_json::Value>,
    pub library: Option<String>,
    /// Retained audio by session identifier, validated and staged as files
    /// in the form history uses.
    pub audio: std::collections::BTreeMap<String, Staged>,
}

/// Validated retained audio waiting beside the history. The file is removed
/// unless it is installed.
pub struct Staged {
    path: PathBuf,
}
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Where restore stages audio and in which form.
pub struct Staging<'a> {
    pub directory: &'a Path,
    pub protect: bool,
}

const STAGED_PREFIX: &str = ".restore-";
const STAGED_SUFFIX: &str = ".audio.partial";
const PREVIOUS_SUFFIX: &str = ".audio.previous";

/// Finish after a restore that was interrupted: staged audio is removed, and
/// audio set aside for replacement is put back unless its replacement is in
/// place.
fn remove_abandoned_staging(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(STAGED_PREFIX) else {
            continue;
        };
        if rest.ends_with(STAGED_SUFFIX) {
            let _ = fs::remove_file(entry.path());
        } else if let Some(id) = rest
            .strip_suffix(PREVIOUS_SUFFIX)
            .filter(|id| super::valid_id(id))
        {
            let current = crate::audio_store::path(directory, id);
            if current.exists() {
                let _ = fs::remove_file(entry.path());
            } else {
                let _ = fs::rename(entry.path(), current);
            }
        }
    }
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "The backup is too large");
    Ok(bytes)
}

/// Cumulative limits for one archive. Each entry is checked against what is
/// left before it is read, by its declared size and again while reading, so
/// a compressed or mislabelled entry can never expand past the budget.
struct Budget {
    expanded: u64,
    retained: u64,
    audio: u64,
}

impl Budget {
    fn new() -> Self {
        Self {
            expanded: MAX_EXPANDED_BYTES,
            retained: MAX_RETAINED_BYTES,
            audio: MAX_AUDIO_BYTES,
        }
    }

    /// Read one entry that is kept in memory.
    fn retain(&mut self, entry: impl Read, declared: u64, limit: u64) -> Result<Vec<u8>> {
        let allowed = limit.min(self.expanded).min(self.retained);
        ensure!(declared <= allowed, "The backup is too large");
        let bytes = read_limited(entry, allowed)?;
        self.expanded -= bytes.len() as u64;
        self.retained -= bytes.len() as u64;
        Ok(bytes)
    }

    /// Room left for one audio entry, which is streamed to a file.
    fn audio_allowance(&self, declared: u64) -> Result<u64> {
        let allowed = self.audio.min(self.expanded);
        ensure!(declared <= allowed, "The backup is too large");
        Ok(allowed)
    }

    fn spend_audio(&mut self, bytes: u64) {
        self.audio -= bytes;
        self.expanded -= bytes;
    }
}

/// Validate one portable audio entry and stage it as a file in the form
/// history uses. Only this entry is held in memory, and only until it is
/// written; staged audio never accumulates in memory.
fn stage_audio(entry: impl Read, limit: u64, staging: &Staging) -> Result<(Staged, u64)> {
    let bytes = read_limited(entry, limit)?;
    let staged = Staged {
        path: staging
            .directory
            .join(format!("{STAGED_PREFIX}{}{STAGED_SUFFIX}", super::new_id())),
    };
    crate::audio_store::install(&bytes, &staged.path, staging.protect)
        .context("The backed up audio is damaged")?;
    Ok((staged, bytes.len() as u64))
}

/// Read, decrypt and validate every part of an archive. Audio is staged in
/// `staging`; nothing else is written.
pub fn read(path: &Path, passphrase: Option<&str>, staging: &Staging) -> Result<Archive> {
    read_within(path, passphrase, staging, Budget::new())
}

fn read_within(
    path: &Path,
    passphrase: Option<&str>,
    staging: &Staging,
    mut budget: Budget,
) -> Result<Archive> {
    let metadata = fs::metadata(path).context("Could not open the backup")?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_ARCHIVE_BYTES,
        "This is not an Articulate backup or it is too large"
    );
    let bytes = read_limited(File::open(path)?, MAX_ARCHIVE_BYTES)?;
    let bytes = if bytes.starts_with(AGE_MAGIC) {
        let Some(passphrase) = passphrase.filter(|p| !p.is_empty()) else {
            bail!("This backup is protected. Enter its passphrase.");
        };
        let decryptor = age::Decryptor::new_buffered(&bytes[..])
            .map_err(|_| anyhow::anyhow!("This backup is damaged"))?;
        let identity =
            age::scrypt::Identity::new(age::secrecy::SecretString::from(passphrase.to_owned()));
        let reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| anyhow::anyhow!("The passphrase is incorrect or the backup is damaged"))?;
        read_limited(reader, MAX_ARCHIVE_BYTES)
            .context("The passphrase is incorrect or the backup is damaged")?
    } else {
        bytes
    };
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| anyhow::anyhow!("This is not an Articulate backup"))?;
    ensure!(zip.len() <= MAX_ENTRIES, "This backup has too many entries");
    let manifest: Manifest = {
        let entry = zip
            .by_name("manifest.json")
            .map_err(|_| anyhow::anyhow!("This is not an Articulate backup"))?;
        let declared = entry.size();
        serde_json::from_slice(&budget.retain(entry, declared, MAX_SIDE_FILE_BYTES)?)
            .context("The backup manifest is damaged")?
    };
    ensure!(
        manifest.format == FORMAT,
        "This is not an Articulate backup"
    );
    ensure!(
        manifest.version <= VERSION && manifest.record_schema <= super::RECORD_SCHEMA,
        "This backup was made by a newer version of Articulate. Update Articulate to restore it."
    );
    ensure!(
        manifest.sessions <= super::MAX_RECORDS,
        "This backup has too many entries"
    );
    let mut archive = Archive {
        sessions: Vec::new(),
        settings: None,
        library: None,
        audio: std::collections::BTreeMap::new(),
    };
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        let declared = entry.size();
        if name == "manifest.json" || entry.is_dir() {
            continue;
        }
        if let Some(id) = name
            .strip_prefix("history/")
            .and_then(|rest| rest.strip_suffix(".json"))
        {
            ensure!(
                super::valid_id(id) && seen.insert(id.to_owned()),
                "The backup contains an invalid history entry"
            );
            let bytes = budget.retain(entry, declared, super::MAX_RECORD_BYTES as u64)?;
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("A saved session in the backup is damaged ({id})"))?;
            let session = parse_record(value, id)
                .with_context(|| format!("A saved session in the backup is invalid ({id})"))?;
            archive.sessions.push(session);
        } else if name == "settings.json" {
            ensure!(
                archive.settings.is_none(),
                "The backed up settings are damaged"
            );
            let value: serde_json::Value =
                serde_json::from_slice(&budget.retain(entry, declared, MAX_SIDE_FILE_BYTES)?)
                    .context("The backed up settings are damaged")?;
            ensure!(value.is_object(), "The backed up settings are damaged");
            archive.settings = Some(portable_settings(value));
        } else if name == "library.json" {
            ensure!(
                archive.library.is_none(),
                "The backed up vocabulary is damaged"
            );
            let text = String::from_utf8(budget.retain(entry, declared, MAX_SIDE_FILE_BYTES)?)
                .context("The backed up vocabulary is damaged")?;
            crate::library::parse(&text).context("The backed up vocabulary is invalid")?;
            archive.library = Some(text);
        } else if let Some(id) = name
            .strip_prefix("audio/")
            .and_then(|rest| rest.strip_suffix(".audio"))
        {
            ensure!(
                super::valid_id(id) && !archive.audio.contains_key(id),
                "The backup contains invalid audio"
            );
            let allowed = budget.audio_allowance(declared)?;
            let (staged, bytes) = stage_audio(entry, allowed, staging)?;
            budget.spend_audio(bytes);
            archive.audio.insert(id.to_owned(), staged);
        } else {
            bail!("The backup contains an unexpected file: {name}");
        }
    }
    ensure!(
        archive
            .audio
            .keys()
            .all(|id| archive.sessions.iter().any(|session| &session.id == id)),
        "The backup contains audio without its session"
    );
    ensure!(
        archive.sessions.len() == manifest.sessions,
        "The backup is incomplete"
    );
    Ok(archive)
}

/// What happens to one archived session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Add,
    Replace,
    KeepLocal,
    NewerInBackup,
}

/// Install `session`, and `audio` when given, as one change: if either step
/// fails, the previous record and audio are left as they were.
fn install_pair(history: &History, session: &Session, audio: Option<&Staged>) -> Result<()> {
    let Some(staged) = audio else {
        return history.install(session);
    };
    let destination = crate::audio_store::path(&history.directory, &session.id);
    let aside = history
        .directory
        .join(format!("{STAGED_PREFIX}{}{PREVIOUS_SUFFIX}", session.id));
    let had_audio = destination.exists();
    if had_audio {
        fs::rename(&destination, &aside)?;
    }
    let put_back = || {
        let _ = fs::remove_file(&destination);
        if had_audio {
            let _ = fs::rename(&aside, &destination);
        }
    };
    if let Err(error) = fs::rename(&staged.path, &destination) {
        put_back();
        return Err(error.into());
    }
    if let Err(error) = history.install(session) {
        put_back();
        return Err(error);
    }
    if had_audio {
        let _ = fs::remove_file(&aside);
    }
    Ok(())
}

/// Validate the whole archive, then merge its sessions into `history`.
pub fn restore(history: &History, request: &RestoreRequest) -> Result<Restored> {
    let protect = history.protect.load(std::sync::atomic::Ordering::Acquire);
    remove_abandoned_staging(&history.directory);
    let archive = read(
        &request.source,
        request.passphrase.as_deref(),
        &Staging {
            directory: &history.directory,
            protect,
        },
    )?;
    let (settings, library) = if request.include_settings {
        (archive.settings, archive.library)
    } else {
        (None, None)
    };
    let mut restored = Restored {
        sessions_in_archive: archive.sessions.len(),
        settings,
        library,
        ..Default::default()
    };
    let mut audio = archive.audio;
    for mut session in archive.sessions {
        let id = session.id.clone();
        // Dropping unused staged audio removes it.
        let staged = audio.remove(&id);
        let outcome = (|| -> Result<()> {
            if history.is_deleted(&id)? {
                restored.deleted_locally.push(id.clone());
                return Ok(());
            }
            let local = history.stored(&id)?;
            let outcome = match &local {
                None => Outcome::Add,
                Some(local) if local.updated_ms >= session.updated_ms => Outcome::KeepLocal,
                Some(_) if !request.replace_older => Outcome::NewerInBackup,
                Some(_) => Outcome::Replace,
            };
            let destination = crate::audio_store::path(&history.directory, &id);
            match outcome {
                Outcome::KeepLocal | Outcome::NewerInBackup => {
                    // The local record stays, and so does its audio choice.
                    // Only a recording it still refers to, and that is
                    // missing here, is filled in from the archive.
                    let local = local.as_ref().expect("a local record exists");
                    if let Some(staged) = &staged
                        && local.audio.is_some()
                        && local.audio == session.audio
                        && !destination.exists()
                    {
                        fs::rename(&staged.path, &destination)?;
                        restored.audio.push(id.clone());
                    }
                    if outcome == Outcome::KeepLocal {
                        restored.kept_local.push(id.clone());
                    } else {
                        restored.newer_in_backup.push(id.clone());
                    }
                }
                Outcome::Add | Outcome::Replace => {
                    // Audio is installed only when the restored record refers
                    // to it. A replaced record without audio keeps a local
                    // recording rather than hiding or deleting it.
                    let audio = staged.as_ref().filter(|_| session.audio.is_some());
                    if session.audio.is_none()
                        && let Some(local) = &local
                        && local.audio.is_some()
                        && destination.exists()
                    {
                        session.audio.clone_from(&local.audio);
                    }
                    install_pair(history, &session, audio)?;
                    if audio.is_some() {
                        restored.audio.push(id.clone());
                    }
                    if outcome == Outcome::Add {
                        restored.added.push(id.clone());
                    } else {
                        restored.updated.push(id.clone());
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            restored.failed.push((id, error.to_string()));
        }
    }
    Ok(restored)
}

/// Write every session as a Markdown file into `folder`. Existing files are
/// never replaced; a numbered name is chosen instead.
pub fn export_markdown(history: &History, folder: &Path) -> Result<Exported> {
    fs::create_dir_all(folder).context("Could not create the export folder")?;
    let mut files = 0;
    let mut unavailable = Vec::new();
    let mut skipped = history.for_each_session(|session| {
        // Topic documents store only their own notes; their transcript is
        // assembled from the filed recordings, as when one is opened.
        let session = if session.is_collection {
            let id = session.id.clone();
            match history.collection(session) {
                Ok(session) => session,
                Err(error) => {
                    unavailable.push(RecordWarning {
                        id,
                        kind: super::WarningKind::Unavailable,
                        message: error.to_string(),
                    });
                    return Ok(());
                }
            }
        } else {
            session
        };
        let text = crate::call_export::session(&session, crate::call_export::Format::Markdown)?;
        let date: String = crate::call_export::local_time(session.created_ms)
            .replace(':', "-")
            .chars()
            .take(16)
            .collect();
        let title = format!(
            "{date} {} {}",
            if session.title.trim().is_empty() {
                crate::call_export::default_title(session.kind)
            } else {
                session.title.as_str()
            },
            &session.id[..8]
        );
        let base = crate::export_file::suggested_name(
            crate::export_file::Document::from_kind(session.kind),
            Some(&title),
            "md",
        );
        let stem = base.strip_suffix(".md").unwrap_or(&base).to_owned();
        let mut attempt = 1;
        loop {
            let name = if attempt == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} ({attempt}).md")
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(folder.join(&name))
            {
                Ok(mut file) => {
                    file.write_all(text.as_bytes())?;
                    file.sync_all()?;
                    break;
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 =>
                {
                    attempt += 1;
                }
                Err(error) => return Err(error.into()),
            }
        }
        files += 1;
        Ok(())
    })?;
    skipped.extend(unavailable);
    Ok(Exported {
        folder: folder.to_path_buf(),
        files,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Kind;

    struct Directory(PathBuf);
    impl Directory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "articulate-backup-test-{label}-{}",
                super::super::new_id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn stage(directory: &Path) -> Staging<'_> {
        Staging {
            directory,
            protect: false,
        }
    }

    fn request(destination: PathBuf, passphrase: Option<&str>) -> Request {
        Request {
            destination,
            passphrase: passphrase.map(str::to_owned),
            settings: serde_json::json!({
                "discord_pairing_key": "synthetic-secret-pairing-key",
                "model_path": "C:/synthetic/model.gguf",
                "assort": {"checkpoint": "C:/synthetic/checkpoint"},
                "entries": [],
                "hotkey_mode": "Hold",
            }),
            library: crate::library::export(&[], &[]).unwrap(),
            include_audio: false,
        }
    }

    #[test]
    fn backup_roundtrips_and_excludes_secrets_and_paths() {
        let source = Directory::new("source");
        let history = History::open(source.0.join("history")).unwrap();
        let mut dictation = Session::new(Kind::Dictation);
        dictation.text = "A synthetic backed up thought.".into();
        let dictation = history.save(dictation).unwrap();
        let mut call = Session::new(Kind::Call);
        call.personal_notes = "Synthetic call notes.".into();
        let call = history.save(call).unwrap();
        for passphrase in [None, Some("correct horse battery")] {
            let path = source.0.join("archive.articulate-backup");
            let created = create(&history, &request(path.clone(), passphrase)).unwrap();
            assert_eq!(created.sessions, 2);
            let raw = fs::read(&path).unwrap();
            let needle = b"synthetic-secret-pairing-key";
            assert!(!raw.windows(needle.len()).any(|window| window == needle));
            if passphrase.is_some() {
                assert!(raw.starts_with(AGE_MAGIC));
                assert!(read(&path, None, &stage(&source.0)).is_err());
                assert!(read(&path, Some("wrong passphrase"), &stage(&source.0)).is_err());
            }
            let archive = read(&path, passphrase, &stage(&source.0)).unwrap();
            assert_eq!(archive.sessions.len(), 2);
            let settings = archive.settings.unwrap();
            for key in EXCLUDED_SETTINGS {
                assert!(settings.get(*key).is_none(), "{key} must be excluded");
            }
            assert_eq!(settings["hotkey_mode"], "Hold");
            assert!(archive.library.is_some());

            let target = Directory::new("target");
            let restored_history = History::open(target.0.clone()).unwrap();
            let restored = restore(
                &restored_history,
                &RestoreRequest {
                    source: path.clone(),
                    passphrase: passphrase.map(str::to_owned),
                    replace_older: false,
                    include_settings: true,
                },
            )
            .unwrap();
            assert_eq!(restored.added.len(), 2);
            assert_eq!(
                restored_history.load(&dictation.id).unwrap().text,
                dictation.text
            );
            assert_eq!(
                restored_history.load(&call.id).unwrap().updated_ms,
                call.updated_ms
            );
        }
        // No temporary files remain beside the archive.
        assert!(
            fs::read_dir(&source.0)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        );
    }

    #[test]
    fn restore_never_silently_replaces_newer_or_deleted_sessions() {
        let source = Directory::new("conflict");
        let history = History::open(source.0.join("history")).unwrap();
        let mut older_locally = Session::new(Kind::Dictation);
        older_locally.text = "Archived newer text.".into();
        let mut newer_locally = Session::new(Kind::Dictation);
        newer_locally.text = "Archived older text.".into();
        let deleted = history.save(Session::new(Kind::Note)).unwrap();
        let mut deleted_copy = deleted.clone();
        deleted_copy.personal_notes = "Deleted here.".into();
        let deleted_copy = history.save(deleted_copy).unwrap();
        let older_locally = history.save(older_locally).unwrap();
        let newer_locally = history.save(newer_locally).unwrap();
        let path = source.0.join("archive.articulate-backup");
        create(&history, &request(path.clone(), None)).unwrap();

        let target = Directory::new("conflict-target");
        let local = History::open(target.0.clone()).unwrap();
        let mut stale = older_locally.clone();
        stale.text = "Local older text.".into();
        stale.updated_ms = older_locally.updated_ms - 1;
        local.install(&stale).unwrap();
        let mut fresh = newer_locally.clone();
        fresh.text = "Local newer text.".into();
        fresh.updated_ms = newer_locally.updated_ms + 1000;
        local.install(&fresh).unwrap();
        local.install(&deleted_copy).unwrap();
        local.delete(&deleted.id).unwrap();

        let request = |replace_older| RestoreRequest {
            source: path.clone(),
            passphrase: None,
            replace_older,
            include_settings: false,
        };
        let first = restore(&local, &request(false)).unwrap();
        assert_eq!(
            first.newer_in_backup,
            std::slice::from_ref(&older_locally.id)
        );
        assert_eq!(first.kept_local, std::slice::from_ref(&newer_locally.id));
        assert_eq!(first.deleted_locally, std::slice::from_ref(&deleted.id));
        assert_eq!(
            local.load(&older_locally.id).unwrap().text,
            "Local older text."
        );
        let second = restore(&local, &request(true)).unwrap();
        assert_eq!(second.updated, std::slice::from_ref(&older_locally.id));
        assert_eq!(
            local.load(&older_locally.id).unwrap().text,
            "Archived newer text."
        );
        assert_eq!(
            local.load(&newer_locally.id).unwrap().text,
            "Local newer text."
        );
        assert!(local.load(&deleted.id).is_err());
    }

    #[test]
    fn invalid_archives_are_rejected_before_any_change() {
        let directory = Directory::new("invalid");
        let history = History::open(directory.0.join("history")).unwrap();
        let path = directory.0.join("bad.articulate-backup");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(
            &serde_json::to_vec(&Manifest {
                format: FORMAT.into(),
                version: VERSION,
                record_schema: super::super::RECORD_SCHEMA,
                created_ms: 0,
                app_version: String::new(),
                sessions: 2,
            })
            .unwrap(),
        )
        .unwrap();
        let good = Session::new(Kind::Note);
        zip.start_file(format!("history/{}.json", good.id), options)
            .unwrap();
        zip.write_all(&encode_record(&good).unwrap()).unwrap();
        let bad = Session::new(Kind::Note);
        zip.start_file(format!("history/{}.json", bad.id), options)
            .unwrap();
        zip.write_all(b"{damaged").unwrap();
        zip.finish().unwrap();
        let result = restore(
            &history,
            &RestoreRequest {
                source: path.clone(),
                passphrase: None,
                replace_older: true,
                include_settings: true,
            },
        );
        assert!(result.is_err());
        assert!(history.list().unwrap().is_empty());
        fs::write(&path, b"not a zip").unwrap();
        assert!(read(&path, None, &stage(&directory.0)).is_err());
    }

    #[test]
    fn audio_is_backed_up_only_on_request_and_restored_with_its_session() {
        let source = Directory::new("audio-source");
        let folder = source.0.join("history");
        let history = History::open(folder.clone()).unwrap();
        let mut call = Session::new(Kind::Call);
        call.text = "Synthetic call with retained audio.".into();
        let recorder = crate::audio_store::Recorder::start(
            crate::audio_store::path(&folder, &call.id),
            vec!["recording".into()],
            false,
        )
        .unwrap();
        recorder.push(0, vec![0.25; 16_000]);
        call.audio = Some(recorder.finish().unwrap());
        let call = history.save(call).unwrap();
        let plain = source.0.join("text-only.articulate-backup");
        assert_eq!(
            create(&history, &request(plain.clone(), None))
                .unwrap()
                .audio,
            0
        );
        assert!(
            read(&plain, None, &stage(&source.0))
                .unwrap()
                .audio
                .is_empty()
        );
        let with_audio = source.0.join("with-audio.articulate-backup");
        let mut wanted = request(with_audio.clone(), Some("correct horse battery"));
        wanted.include_audio = true;
        assert_eq!(create(&history, &wanted).unwrap().audio, 1);
        let target = Directory::new("audio-target");
        let restored_folder = target.0.join("history");
        let restored_history = History::open(restored_folder.clone())
            .unwrap()
            .with_protection(true);
        let restored = restore(
            &restored_history,
            &RestoreRequest {
                source: with_audio,
                passphrase: Some("correct horse battery".into()),
                replace_older: false,
                include_settings: false,
            },
        )
        .unwrap();
        assert_eq!(restored.audio, std::slice::from_ref(&call.id));
        let audio = crate::audio_store::path(&restored_folder, &call.id);
        assert!(super::super::protect::is_sealed(
            &fs::read(&audio).unwrap()[23..]
        ));
        assert_eq!(
            crate::audio_store::read_all(&audio).unwrap()[0].len(),
            16_000
        );
        // Deleting the session, as retention does, removes its audio.
        restored_history.delete(&call.id).unwrap();
        assert!(!audio.exists());
    }

    /// A minimal archive written entry by entry, with optional compression.
    fn archive_with(path: &Path, sessions: usize, entries: &[(&str, Vec<u8>)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(
            &serde_json::to_vec(&Manifest {
                format: FORMAT.into(),
                version: VERSION,
                record_schema: super::super::RECORD_SCHEMA,
                created_ms: 0,
                app_version: String::new(),
                sessions,
            })
            .unwrap(),
        )
        .unwrap();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn staged_files(directory: &Path) -> usize {
        fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(".restore-"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn compressible_archives_cannot_expand_past_the_budgets() {
        let directory = Directory::new("budget");
        let path = directory.0.join("bomb.articulate-backup");
        // Two megabytes of spaces compress to a few kilobytes.
        let mut settings = b"{}".to_vec();
        settings.resize(2 * 1024 * 1024, b' ');
        archive_with(&path, 0, &[("settings.json", settings.clone())]);
        assert!(fs::metadata(&path).unwrap().len() < 64 * 1024);
        let small = |expanded, retained, audio| Budget {
            expanded,
            retained,
            audio,
        };
        let stage = stage(&directory.0);
        let error = read_within(&path, None, &stage, small(1 << 30, 1 << 20, 1 << 20))
            .err()
            .unwrap();
        assert!(error.to_string().contains("too large"), "{error:#}");
        let error = read_within(&path, None, &stage, small(1 << 20, 1 << 30, 1 << 20))
            .err()
            .unwrap();
        assert!(error.to_string().contains("too large"), "{error:#}");
        assert!(read_within(&path, None, &stage, small(1 << 30, 1 << 30, 0)).is_ok());

        // The total counts every entry, not just the largest.
        let mut first = Session::new(Kind::Note);
        first.personal_notes = " ".repeat(600 * 1024);
        let mut second = Session::new(Kind::Note);
        second.personal_notes = " ".repeat(600 * 1024);
        archive_with(
            &path,
            2,
            &[
                (
                    &format!("history/{}.json", first.id),
                    encode_record(&first).unwrap(),
                ),
                (
                    &format!("history/{}.json", second.id),
                    encode_record(&second).unwrap(),
                ),
            ],
        );
        assert!(read_within(&path, None, &stage, small(1 << 30, 1 << 30, 0)).is_ok());
        assert!(read_within(&path, None, &stage, small(1 << 30, 1 << 20, 0)).is_err());

        // Audio counts against its own and the total budget before anything
        // is kept, and a rejected archive leaves no staged audio behind.
        let mut audio = b"ARTICULATE-AUDIO-1\n".to_vec();
        audio.resize(2 * 1024 * 1024, 0);
        let call = Session::new(Kind::Call);
        archive_with(
            &path,
            1,
            &[
                (
                    &format!("history/{}.json", call.id),
                    encode_record(&call).unwrap(),
                ),
                (&format!("audio/{}.audio", call.id), audio),
            ],
        );
        let error = read_within(&path, None, &stage, small(1 << 30, 1 << 30, 1 << 20))
            .err()
            .unwrap();
        assert!(error.to_string().contains("too large"), "{error:#}");
        let error = read_within(&path, None, &stage, small(1 << 20, 1 << 30, 1 << 30))
            .err()
            .unwrap();
        assert!(error.to_string().contains("too large"), "{error:#}");
        assert_eq!(staged_files(&directory.0), 0);
    }

    /// A one second synthetic recording for `id` in `folder`.
    fn record_audio(folder: &Path, id: &str) -> crate::audio_store::Info {
        fs::create_dir_all(folder).unwrap();
        let recorder = crate::audio_store::Recorder::start(
            crate::audio_store::path(folder, id),
            vec!["recording".into()],
            false,
        )
        .unwrap();
        recorder.push(0, vec![0.25; 16_000]);
        recorder.finish().unwrap()
    }

    #[test]
    fn restore_keeps_the_local_choice_to_delete_audio() {
        let source = Directory::new("deleted-audio");
        let folder = source.0.join("history");
        let history = History::open(folder.clone()).unwrap();
        let mut call = Session::new(Kind::Call);
        call.text = "Synthetic call.".into();
        call.audio = Some(record_audio(&folder, &call.id));
        let call = history.save(call).unwrap();
        let path = source.0.join("audio.articulate-backup");
        let mut wanted = request(path.clone(), None);
        wanted.include_audio = true;
        assert_eq!(create(&history, &wanted).unwrap().audio, 1);
        // Delete the audio afterwards, as the "Delete audio" action does.
        fs::remove_file(crate::audio_store::path(&folder, &call.id)).unwrap();
        let mut without = history.load(&call.id).unwrap();
        without.audio = None;
        history.save(without).unwrap();
        for replace_older in [false, true] {
            let restored = restore(
                &history,
                &RestoreRequest {
                    source: path.clone(),
                    passphrase: None,
                    replace_older,
                    include_settings: false,
                },
            )
            .unwrap();
            assert_eq!(restored.kept_local, std::slice::from_ref(&call.id));
            assert!(restored.audio.is_empty());
            assert!(!crate::audio_store::path(&folder, &call.id).exists());
            assert!(history.load(&call.id).unwrap().audio.is_none());
            assert_eq!(staged_files(&folder), 0);
        }
    }

    #[test]
    fn damaged_audio_is_rejected_before_any_session_changes() {
        let directory = Directory::new("damaged-audio");
        let folder = directory.0.join("history");
        let history = History::open(folder.clone()).unwrap();
        let mut local = Session::new(Kind::Call);
        local.text = "Local text.".into();
        local.audio = Some(record_audio(&folder, &local.id));
        let local = history.save(local).unwrap();
        let audio_before = fs::read(crate::audio_store::path(&folder, &local.id)).unwrap();
        let mut newer = local.clone();
        newer.text = "Archived newer text.".into();
        newer.updated_ms += 1000;
        let mut damaged = b"ARTICULATE-AUDIO-1\n".to_vec();
        damaged.extend_from_slice(&8u32.to_le_bytes());
        damaged.extend_from_slice(b"garbage!");
        let path = directory.0.join("damaged.articulate-backup");
        archive_with(
            &path,
            1,
            &[
                (
                    &format!("history/{}.json", newer.id),
                    encode_record(&newer).unwrap(),
                ),
                (&format!("audio/{}.audio", newer.id), damaged),
            ],
        );
        let result = restore(
            &history,
            &RestoreRequest {
                source: path,
                passphrase: None,
                replace_older: true,
                include_settings: false,
            },
        );
        assert!(result.is_err());
        assert_eq!(history.load(&local.id).unwrap().text, "Local text.");
        assert_eq!(
            fs::read(crate::audio_store::path(&folder, &local.id)).unwrap(),
            audio_before
        );
        assert_eq!(staged_files(&folder), 0);
    }

    #[test]
    fn replacing_a_session_installs_its_audio_with_it() {
        let source = Directory::new("replace-audio");
        let source_folder = source.0.join("history");
        let archived = History::open(source_folder.clone()).unwrap();
        let mut call = Session::new(Kind::Call);
        call.text = "Archived newer text.".into();
        call.audio = Some(record_audio(&source_folder, &call.id));
        let call = archived.save(call).unwrap();
        let path = source.0.join("replace.articulate-backup");
        let mut wanted = request(path.clone(), None);
        wanted.include_audio = true;
        create(&archived, &wanted).unwrap();

        let target = Directory::new("replace-audio-target");
        let folder = target.0.join("history");
        let local = History::open(folder.clone()).unwrap();
        let mut older = call.clone();
        older.text = "Local older text.".into();
        older.audio = None;
        older.updated_ms -= 1000;
        local.install(&older).unwrap();
        let restored = restore(
            &local,
            &RestoreRequest {
                source: path,
                passphrase: None,
                replace_older: true,
                include_settings: false,
            },
        )
        .unwrap();
        assert_eq!(restored.updated, std::slice::from_ref(&call.id));
        assert_eq!(restored.audio, std::slice::from_ref(&call.id));
        let session = local.load(&call.id).unwrap();
        assert_eq!(session.text, "Archived newer text.");
        assert_eq!(session.audio, call.audio);
        assert_eq!(
            crate::audio_store::read_all(&crate::audio_store::path(&folder, &call.id)).unwrap()[0]
                .len(),
            16_000
        );
        assert_eq!(staged_files(&folder), 0);
    }

    #[test]
    fn machine_specific_settings_are_never_backed_up() {
        let settings = portable_settings(serde_json::json!({
            "discord_debugger_port": 9229,
            "discord_flavor": "canary",
            "microphone": "Synthetic microphone",
            "encrypt_at_rest": true,
            "hotkey_mode": "Hold",
        }));
        for key in EXCLUDED_SETTINGS {
            assert!(settings.get(*key).is_none(), "{key} must be excluded");
        }
        assert_eq!(settings["hotkey_mode"], "Hold");
    }

    #[test]
    fn markdown_export_includes_topic_transcripts() {
        let directory = Directory::new("markdown-topic");
        let history = History::open(directory.0.join("history")).unwrap();
        let mut source = Session::new(Kind::Dictation);
        source.text = "Synthetic filed words.".into();
        let source = history.save(source).unwrap();
        history
            .move_source(
                &source.id,
                crate::topics::Destination::New("Synthetic topic".into()),
                None,
            )
            .unwrap();
        let folder = directory.0.join("export");
        let exported = export_markdown(&history, &folder).unwrap();
        assert_eq!(exported.files, 2);
        let topic = fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("Synthetic topic")
            })
            .unwrap();
        let text = fs::read_to_string(topic.path()).unwrap();
        assert!(text.contains("Synthetic filed words"), "{text}");
    }

    #[test]
    fn markdown_export_writes_one_file_per_session_without_overwriting() {
        let directory = Directory::new("markdown");
        let history = History::open(directory.0.join("history")).unwrap();
        let mut note = Session::new(Kind::Note);
        note.title = "Plans: \"Q3\"".into();
        note.personal_notes = "Synthetic plan.".into();
        history.save(note).unwrap();
        let mut dictation = Session::new(Kind::Dictation);
        dictation.text = "Synthetic dictation.".into();
        history.save(dictation).unwrap();
        let folder = directory.0.join("export");
        assert_eq!(export_markdown(&history, &folder).unwrap().files, 2);
        assert_eq!(export_markdown(&history, &folder).unwrap().files, 2);
        let names: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 4);
        assert!(names.iter().all(|name| name.ends_with(".md")));
        assert!(names.iter().any(|name| name.contains("Plans Q3")));
        assert!(names.iter().any(|name| name.ends_with(" (2).md")));
    }
}
