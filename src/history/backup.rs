//! Portable backups, restore and bulk Markdown export.
//!
//! A backup is one ZIP archive, optionally wrapped in passphrase encryption
//! (the age format with scrypt):
//!
//! - `manifest.json`: format name, archive version, record schema, counts.
//! - `history/<id>.json`: each saved session in the plaintext record format.
//! - `library.json`: vocabulary corrections and voice macros.
//! - `settings.json`: preferences without the Discord pairing key, model and
//!   tool paths, or vocabulary (which lives in `library.json`).
//! - `audio/<id>.audio`: retained session audio, only when the request opts
//!   in with `include_audio`. It is written without DPAPI protection; the
//!   archive passphrase protects it instead.
//!
//! Restore reads and validates the entire archive before changing anything.
//! It adds missing sessions and never silently replaces a local session that is
//! newer than or as new as the archived copy. Sessions deleted on this device
//! are reported, not restored.
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
/// Settings that are secret or tied to this machine never leave it.
pub const EXCLUDED_SETTINGS: &[&str] = &[
    "discord_pairing_key",
    "model_path",
    "assort",
    "vencord_source",
    "entries",
    "macros",
];
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = super::MAX_RECORDS + 16;
const MAX_SIDE_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Retained audio added to one archive, keeping it below `MAX_ARCHIVE_BYTES`.
const MAX_AUDIO_BYTES: u64 = 768 * 1024 * 1024;
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

/// A fully validated archive. Nothing has been changed yet.
pub struct Archive {
    pub sessions: Vec<Session>,
    pub settings: Option<serde_json::Value>,
    pub library: Option<String>,
    /// Retained audio by session identifier, in the portable form.
    pub audio: std::collections::BTreeMap<String, Vec<u8>>,
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "The backup is too large");
    Ok(bytes)
}

/// Read, decrypt and validate every part of an archive.
pub fn read(path: &Path, passphrase: Option<&str>) -> Result<Archive> {
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
        serde_json::from_slice(&read_limited(entry, MAX_SIDE_FILE_BYTES)?)
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
            let bytes = read_limited(entry, super::MAX_RECORD_BYTES as u64)?;
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("A saved session in the backup is damaged ({id})"))?;
            let session = parse_record(value, id)
                .with_context(|| format!("A saved session in the backup is invalid ({id})"))?;
            archive.sessions.push(session);
        } else if name == "settings.json" {
            let value: serde_json::Value =
                serde_json::from_slice(&read_limited(entry, MAX_SIDE_FILE_BYTES)?)
                    .context("The backed up settings are damaged")?;
            ensure!(value.is_object(), "The backed up settings are damaged");
            archive.settings = Some(portable_settings(value));
        } else if name == "library.json" {
            let text = String::from_utf8(read_limited(entry, MAX_SIDE_FILE_BYTES)?)
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
            let bytes = read_limited(entry, MAX_AUDIO_BYTES)?;
            archive.audio.insert(id.to_owned(), bytes);
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

/// Validate the whole archive, then merge its sessions into `history`.
pub fn restore(history: &History, request: &RestoreRequest) -> Result<Restored> {
    let archive = read(&request.source, request.passphrase.as_deref())?;
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
    let protect = history.protect.load(std::sync::atomic::Ordering::Acquire);
    for session in archive.sessions {
        let id = session.id.clone();
        let outcome = (|| -> Result<()> {
            if history.is_deleted(&id)? {
                restored.deleted_locally.push(id.clone());
                return Ok(());
            }
            let installed = match history.stored(&id)? {
                None => {
                    history.install(&session)?;
                    restored.added.push(id.clone());
                    true
                }
                Some(local) if local.updated_ms >= session.updated_ms => {
                    restored.kept_local.push(id.clone());
                    false
                }
                Some(_) if !request.replace_older => {
                    restored.newer_in_backup.push(id.clone());
                    false
                }
                Some(_) => {
                    history.install(&session)?;
                    restored.updated.push(id.clone());
                    true
                }
            };
            let destination = crate::audio_store::path(&history.directory, &id);
            if let Some(bytes) = audio.remove(&id)
                && session.audio.is_some()
                && (installed || !destination.exists())
            {
                crate::audio_store::install(&bytes, &destination, protect)?;
                restored.audio.push(id.clone());
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
    let skipped = history.for_each_session(|session| {
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
                assert!(read(&path, None).is_err());
                assert!(read(&path, Some("wrong passphrase")).is_err());
            }
            let archive = read(&path, passphrase).unwrap();
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
        assert!(read(&path, None).is_err());
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
        assert!(read(&plain, None).unwrap().audio.is_empty());
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
        assert_eq!(restored.audio, [call.id.clone()]);
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
