//! Local text history.
//!
//! Audio is not part of the record. When the user chooses to keep a
//! recording, it is stored beside the record as `<id>.audio` (see
//! `crate::audio_store`), sealed like the record when encryption at rest is
//! on, and removed with every other copy when the session is deleted.
//!
//! Each session is one JSON file named by its identifier. Writes are atomic
//! (temporary file, backup, rename) and recovery prefers the newest complete
//! copy. Records that cannot be parsed are moved aside ("quarantined") rather
//! than deleted, and can be listed, exported or restored. Records that cannot
//! be read right now (another program holds them open) are skipped and retried.
pub mod backup;
pub mod protect;
pub mod retention;
pub mod search_index;

use crate::{calls::Row, notes::Notes};
use anyhow::{Context, Result, ensure};
use search_index::SearchIndex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque, hash_map::RandomState},
    fs::{self, File, OpenOptions},
    hash::BuildHasher,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Record format version.
///
/// Compatibility policy:
/// - Adding an optional field with a serde default does not change `SCHEMA`.
///   Every version ignores fields it does not know when reading. Unknown
///   top-level session fields are kept in [`Session::unknown`] and written back
///   unchanged, so an older version editing a newer record does not drop them.
/// - Changing the meaning or shape of an existing field increments `SCHEMA`.
///   Versions that do not understand it never load, overwrite or quarantine
///   those records; they are reported as saved by a newer version.
const SCHEMA: u32 = 5;
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Encrypted records carry a small DPAPI envelope around the JSON.
const MAX_STORED_BYTES: u64 = MAX_BYTES as u64 + 64 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
const MAX_RECORDS: usize = 100_000;
const QUEUE_CAPACITY: usize = 32;
/// Tombstones only guard against stale queued saves and interrupted cleanup.
/// Once every copy is gone and this much time has passed, neither can happen.
const TOMBSTONE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_SAVE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dictation,
    Call,
    Note,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub title: String,
    /// Missing ownership in legacy files is conservative until migration.
    #[serde(default = "manual_title_default")]
    pub title_is_manual: bool,
    pub kind: Kind,
    pub text: String,
    pub original: String,
    pub rows: Vec<Row>,
    /// Recoverable audio interruptions reported by this call's capture worker.
    #[serde(default)]
    pub audio_packets_lost: u64,
    pub speaker_names: Vec<String>,
    pub notes: Option<Notes>,
    /// Personal writing is independent of source-quoted highlights.
    #[serde(default)]
    pub personal_notes: String,
    #[serde(default)]
    pub metrics: crate::insights::DictationMetrics,
    #[serde(default)]
    pub generated_summary: Option<crate::brain::Draft>,
    /// Generated paragraphs removed or rewritten by the user, retained across updates.
    #[serde(default)]
    pub protected_note_items: Vec<crate::brain::Item>,
    /// Membership belongs to the independently stored source, not copied prose.
    #[serde(default)]
    pub topic_id: Option<String>,
    #[serde(default)]
    pub auto_file_pending: bool,
    #[serde(default)]
    pub is_collection: bool,
    #[serde(default)]
    pub source_revision: String,
    /// Pinned sessions are exempt from retention. Like topic membership, the
    /// stored value wins over a stale editor copy; change it with `set_pinned`.
    #[serde(default)]
    pub pinned: bool,
    /// Recent speaker corrections that can be undone, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_edits: Vec<crate::speaker_edits::Edit>,
    /// Timestamped gaps in the captured audio.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interruptions: Vec<crate::calls::Interruption>,
    /// Retained audio beside this record, when the user chose to keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<crate::audio_store::Info>,
    /// Transcripts replaced by re-transcription, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript_versions: Vec<TranscriptVersion>,
    /// Fields written by a newer version, preserved when this version saves.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub unknown: serde_json::Map<String, serde_json::Value>,
    /// Rebuilt from member sessions when a topic is opened; never persisted.
    #[serde(skip)]
    pub sources: Vec<crate::topics::Source>,
}

fn manual_title_default() -> bool {
    true
}

/// Previous transcripts kept after re-transcription.
pub const MAX_TRANSCRIPT_VERSIONS: usize = 2;

/// A transcript replaced by re-transcription, kept so it can be restored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptVersion {
    /// When this transcript was replaced.
    pub replaced_ms: u64,
    /// The recognizer that produced it, when known.
    #[serde(default)]
    pub model: String,
    pub text: String,
    #[serde(default)]
    pub original: String,
    #[serde(default)]
    pub rows: Vec<Row>,
    #[serde(default)]
    pub speaker_names: Vec<String>,
}

impl TranscriptVersion {
    /// Capture the current transcript of `session`.
    pub fn of(session: &Session, model: &str) -> Self {
        Self {
            replaced_ms: now_ms(),
            model: model.to_owned(),
            text: session.text.clone(),
            original: session.original.clone(),
            rows: session.rows.clone(),
            speaker_names: session.speaker_names.clone(),
        }
    }
}

/// The history folder in the local data directory.
pub fn directory() -> PathBuf {
    crate::model::data_dir().join("history")
}

impl Session {
    pub fn can_receive_sources(&self) -> bool {
        self.is_collection
            || (self.kind == Kind::Note
                && self.topic_id.is_none()
                && self.rows.is_empty()
                && self.text.trim().is_empty())
    }
    pub fn new(kind: Kind) -> Self {
        let now = now_ms();
        Self {
            id: new_id(),
            created_ms: now,
            updated_ms: now,
            title: if kind == Kind::Call {
                "Conversation".into()
            } else {
                String::new()
            },
            title_is_manual: false,
            kind,
            text: String::new(),
            original: String::new(),
            rows: Vec::new(),
            audio_packets_lost: 0,
            speaker_names: crate::calls::empty_speaker_names(),
            notes: None,
            personal_notes: String::new(),
            metrics: Default::default(),
            generated_summary: None,
            protected_note_items: Vec::new(),
            topic_id: None,
            auto_file_pending: false,
            is_collection: false,
            source_revision: String::new(),
            pinned: false,
            speaker_edits: Vec::new(),
            interruptions: Vec::new(),
            audio: None,
            transcript_versions: Vec::new(),
            unknown: serde_json::Map::new(),
            sources: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub id: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub title: String,
    pub kind: Kind,
    pub preview: String,
    pub duration_ms: u64,
    pub topic_id: Option<String>,
    pub is_collection: bool,
    pub can_receive_sources: bool,
    pub pinned: bool,
}

impl From<&Session> for Summary {
    fn from(session: &Session) -> Self {
        let source = if !session.personal_notes.trim().is_empty() {
            session.personal_notes.as_str()
        } else if session.text.trim().is_empty() {
            session.rows.first().map_or("", |row| row.text.as_str())
        } else {
            &session.text
        };
        Self {
            id: session.id.clone(),
            topic_id: session.topic_id.clone(),
            is_collection: session.is_collection,
            can_receive_sources: session.can_receive_sources(),
            created_ms: session.created_ms,
            updated_ms: session.updated_ms,
            title: session.title.clone(),
            kind: session.kind,
            pinned: session.pinned,
            duration_ms: session.rows.iter().map(|row| row.end_ms).max().unwrap_or(0),
            preview: source
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(160)
                .collect(),
        }
    }
}

/// Unknown top-level fields are tolerated; see `SCHEMA`.
#[derive(Serialize, Deserialize)]
struct Stored {
    schema: u32,
    session: Session,
}

#[derive(Debug)]
struct UnsupportedSchema;
impl std::fmt::Display for UnsupportedSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This history item was saved by a newer version of Articulate.")
    }
}
impl std::error::Error for UnsupportedSchema {}

#[derive(Debug)]
struct DamagedRecord;
impl std::fmt::Display for DamagedRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("A damaged history item was isolated; other items are available")
    }
}
impl std::error::Error for DamagedRecord {}

/// The record exists but could not be read right now, for example because
/// antivirus, backup or indexing software holds it open, or because it is
/// encrypted for another Windows account. It is never quarantined.
#[derive(Debug)]
struct Unavailable(String);
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "This history item is temporarily unavailable and will be retried: {}",
            self.0
        )
    }
}
impl std::error::Error for Unavailable {}

fn unavailable(error: impl std::fmt::Display) -> anyhow::Error {
    Unavailable(error.to_string()).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// Could not be parsed or validated. The file was moved to quarantine.
    Damaged,
    /// Could not be read right now. It stays in place and is retried.
    Unavailable,
    /// Saved by a newer version. Left untouched.
    NewerVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordWarning {
    pub id: String,
    pub kind: WarningKind,
    pub message: String,
}

/// The result of one history pass.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub items: Vec<Summary>,
    /// Records skipped in this pass, one per identifier.
    pub warnings: Vec<RecordWarning>,
    /// Isolated files currently kept beside the history (see `quarantined`).
    pub quarantined: usize,
}

/// A file moved aside because it could not be parsed or validated.
#[derive(Clone, Debug, Serialize)]
pub struct Quarantined {
    /// Opaque handle for restore, export and discard.
    pub file_name: String,
    pub id: String,
    pub bytes: u64,
    pub modified_ms: u64,
    /// Whether the file is a complete record that `restore_quarantined` accepts.
    pub restorable: bool,
    pub title: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    pub protect: bool,
    pub changed: usize,
    pub failed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::symlink_metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

/// Cached facts about one readable record, keyed by its file's (mtime, len).
struct Indexed {
    stamp: Option<Stamp>,
    summary: Summary,
    pending_filing: bool,
    sample: crate::insights::Sample,
}

#[derive(Default)]
struct Index {
    entries: HashMap<String, Indexed>,
    unreadable: BTreeMap<String, WarningKind>,
}

/// Files found for one identifier in a single directory pass.
#[derive(Default)]
struct Files {
    json: bool,
    tmp: bool,
    bak: bool,
    deleted: bool,
    deleted_at: Option<SystemTime>,
    corrupt: Vec<PathBuf>,
}

impl Files {
    fn has_record(&self) -> bool {
        self.json || self.tmp || self.bak
    }
}

pub struct History {
    directory: PathBuf,
    index: Mutex<Index>,
    search: Option<SearchIndex>,
    protect: AtomicBool,
    quarantined_now: AtomicUsize,
}

impl History {
    pub fn open(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory).context("Could not create local history")?;
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "History must be a local directory"
        );
        Ok(Self {
            directory,
            index: Mutex::new(Index::default()),
            search: None,
            protect: AtomicBool::new(false),
            quarantined_now: AtomicUsize::new(0),
        })
    }

    /// Keep `index` updated with every record this store reads, saves or deletes.
    pub fn with_search(mut self, index: SearchIndex) -> Self {
        self.search = Some(index);
        self
    }

    /// Whether new writes are encrypted. Use `set_protection` to migrate.
    pub fn with_protection(self, protect: bool) -> Self {
        self.protect.store(protect, Ordering::Release);
        self
    }

    fn lock_index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One directory enumeration per pass. Every later step uses this result
    /// instead of scanning again.
    fn scan(&self) -> Result<BTreeMap<String, Files>> {
        let mut files: BTreeMap<String, Files> = BTreeMap::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some((id, suffix)) = name.split_once('.') else {
                continue;
            };
            if !valid_id(id) {
                continue;
            }
            let known = matches!(suffix, "json" | "tmp" | "bak" | "deleted")
                || suffix.strip_prefix("corrupt-").is_some_and(valid_id);
            if !known {
                continue;
            }
            if !files.contains_key(id) {
                ensure!(
                    files.len() < MAX_RECORDS,
                    "Local history contains too many items"
                );
            }
            let found = files.entry(id.to_owned()).or_default();
            match suffix {
                "json" => found.json = true,
                "tmp" => found.tmp = true,
                "bak" => found.bak = true,
                "deleted" => {
                    found.deleted = true;
                    found.deleted_at = entry.metadata().ok().and_then(|m| m.modified().ok());
                }
                _ => found.corrupt.push(entry.path()),
            }
        }
        Ok(files)
    }

    /// Bring the index up to date. Unchanged records are not read again.
    fn refresh(&self) -> Result<Vec<RecordWarning>> {
        let files = self.scan()?;
        let removed: Vec<String> = {
            let mut index = self.lock_index();
            let removed = index
                .entries
                .keys()
                .filter(|id| {
                    files
                        .get(*id)
                        .is_none_or(|found| found.deleted || !found.has_record())
                })
                .cloned()
                .collect::<Vec<_>>();
            for id in &removed {
                index.entries.remove(id);
            }
            removed
        };
        for id in &removed {
            self.forget_search(id);
        }
        let mut warnings = Vec::new();
        let mut unreadable = BTreeMap::new();
        let mut warn = |id: &str, kind, message: String| {
            unreadable.insert(id.to_owned(), kind);
            warnings.push(RecordWarning {
                id: id.to_owned(),
                kind,
                message,
            });
        };
        for (id, found) in &files {
            if found.deleted {
                if let Err(error) = self.purge_deleted(id, found) {
                    warn(id, WarningKind::Unavailable, error.to_string());
                }
                continue;
            }
            if !found.has_record() {
                continue;
            }
            let main = self.path(id, "json")?;
            let before = if found.json { stamp(&main) } else { None };
            if !found.tmp
                && !found.bak
                && before.is_some()
                && self
                    .lock_index()
                    .entries
                    .get(id)
                    .is_some_and(|entry| entry.stamp == before)
            {
                continue;
            }
            match self.recover(id) {
                Ok(Some(session)) => {
                    let after = stamp(&main);
                    let trusted = after == before || found.tmp || found.bak;
                    self.remember(&session, if trusted { after } else { None });
                }
                Ok(None) => self.forget(id),
                Err(error) => {
                    let kind = if error.is::<UnsupportedSchema>() {
                        WarningKind::NewerVersion
                    } else if error.is::<DamagedRecord>() {
                        WarningKind::Damaged
                    } else {
                        WarningKind::Unavailable
                    };
                    // A transiently unreadable record keeps its last known
                    // summary so it does not vanish from the list.
                    if kind != WarningKind::Unavailable {
                        self.forget(id);
                    }
                    warn(id, kind, error.to_string());
                }
            }
        }
        self.lock_index().unreadable = unreadable;
        if let Some(search) = &self.search {
            search.set_ready(warnings.len());
        }
        Ok(warnings)
    }

    /// Finish an interrupted delete and compact old tombstones.
    fn purge_deleted(&self, id: &str, found: &Files) -> Result<()> {
        self.remove_copies(id, Some(&found.corrupt))?;
        let old = found.deleted_at.is_some_and(|at| {
            SystemTime::now()
                .duration_since(at)
                .is_ok_and(|age| age >= TOMBSTONE_RETENTION)
        });
        if old {
            remove_if_present(&self.path(id, "deleted")?)?;
        }
        Ok(())
    }

    fn remember(&self, session: &Session, stamp: Option<Stamp>) {
        let entry = Indexed {
            stamp,
            summary: Summary::from(session),
            pending_filing: session.auto_file_pending
                && !session.is_collection
                && session.topic_id.is_none()
                && !session.text.trim().is_empty(),
            sample: crate::insights::Sample::from(session),
        };
        {
            let mut index = self.lock_index();
            index.unreadable.remove(&session.id);
            index.entries.insert(session.id.clone(), entry);
        }
        if let Some(search) = &self.search {
            let _ = search.upsert(session);
        }
    }

    fn forget(&self, id: &str) {
        self.lock_index().entries.remove(id);
        self.forget_search(id);
    }

    fn forget_search(&self, id: &str) {
        if let Some(search) = &self.search {
            let _ = search.remove(id);
        }
    }

    fn quarantine_count(&self) -> usize {
        fs::read_dir(&self.directory)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .and_then(parse_quarantine_name)
                            .is_some()
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn insights(&self) -> Result<crate::insights::Report> {
        self.refresh()?;
        let mut aggregate = crate::insights::Aggregate::new(now_ms());
        for entry in self.lock_index().entries.values() {
            aggregate.push_sample(&entry.sample);
        }
        Ok(aggregate.finish())
    }

    #[cfg(test)]
    pub fn list(&self) -> Result<Vec<Summary>> {
        self.list_with_pending().map(|(listing, _)| listing.items)
    }

    #[cfg(test)]
    pub fn listing(&self) -> Result<Listing> {
        self.list_with_pending().map(|(listing, _)| listing)
    }

    fn list_with_pending(&self) -> Result<(Listing, Vec<Session>)> {
        self.quarantined_now.store(0, Ordering::Relaxed);
        let warnings = self.refresh()?;
        let (mut items, mut pending_ids) = {
            let index = self.lock_index();
            (
                index
                    .entries
                    .values()
                    .map(|entry| entry.summary.clone())
                    .collect::<Vec<_>>(),
                index
                    .entries
                    .iter()
                    .filter(|(_, entry)| entry.pending_filing)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>(),
            )
        };
        items.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        pending_ids.sort();
        let mut pending = Vec::new();
        let mut pending_bytes = 0_usize;
        for id in pending_ids {
            if pending.len() >= 32 {
                break;
            }
            let Ok(Some(session)) = self.recover(&id) else {
                continue;
            };
            if !session.auto_file_pending || session.topic_id.is_some() || session.is_collection {
                continue;
            }
            let bytes = session
                .text
                .len()
                .saturating_add(session.original.len())
                .saturating_add(session.rows.iter().map(|row| row.text.len()).sum::<usize>());
            if pending_bytes.saturating_add(bytes) <= MAX_TEXT_BYTES {
                pending_bytes += bytes;
                pending.push(session);
            }
        }
        Ok((
            Listing {
                items,
                warnings,
                quarantined: self.quarantine_count(),
            },
            pending,
        ))
    }

    /// Visit every readable record once. Used by backup and bulk export.
    pub(crate) fn for_each_session(
        &self,
        mut visit: impl FnMut(Session) -> Result<()>,
    ) -> Result<Vec<RecordWarning>> {
        let warnings = self.refresh()?;
        let mut ids: Vec<String> = self.lock_index().entries.keys().cloned().collect();
        ids.sort();
        for id in ids {
            match self.recover(&id) {
                Ok(Some(session)) => visit(session)?,
                Ok(None) => {}
                Err(error) if error.is::<Unavailable>() => {
                    anyhow::bail!("{error} Try again in a moment.")
                }
                Err(error) => return Err(error),
            }
        }
        Ok(warnings)
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        let session = self
            .recover(id)?
            .context("This history item is no longer available")?;
        if session.is_collection {
            self.collection(session)
        } else {
            Ok(session)
        }
    }

    /// Read only the committed file. Independent search workers must never
    /// recover, remove, or quarantine another worker's in-flight save files.
    #[allow(
        dead_code,
        reason = "Preserve isolated history snapshots for background search and review"
    )]
    pub(crate) fn snapshot(&self, id: &str) -> Result<Session> {
        let deleted = self.path(id, "deleted")?;
        ensure!(!deleted.exists(), "This history item was deleted");
        let session = read_session(&self.path(id, "json")?, id)?;
        ensure!(!deleted.exists(), "This history item was deleted");
        Ok(session)
    }

    pub fn save(&self, session: Session) -> Result<Session> {
        self.save_inner(session, true)
    }

    fn save_inner(&self, mut session: Session, preserve_membership: bool) -> Result<Session> {
        validate(&session)?;
        ensure!(
            !self.path(&session.id, "deleted")?.exists(),
            "This history item was deleted"
        );
        if let Some(existing) = self.recover(&session.id)? {
            ensure!(
                session.is_collection == existing.is_collection
                    || (session.is_collection && existing.can_receive_sources()),
                "A source cannot be replaced with a topic document."
            );
            if preserve_membership {
                session.topic_id = existing.topic_id.clone();
                session.auto_file_pending = existing.auto_file_pending;
                session.pinned = existing.pinned;
            }
            if existing.is_collection {
                let current = self.collection(existing.clone())?;
                ensure!(
                    session.source_revision == current.source_revision,
                    "The topic sources changed. Reopen the note before saving."
                );
            }
            session.created_ms = existing.created_ms;
            session.updated_ms = now_ms()
                .max(existing.updated_ms.saturating_add(1))
                .max(session.created_ms);
        } else {
            session.updated_ms = now_ms().max(session.created_ms);
        }
        if session.kind == Kind::Call && session.title.trim().is_empty() {
            session.title = "Conversation".into();
        }
        if session.title.trim().is_empty() {
            let source = if !session.personal_notes.trim().is_empty() {
                session.personal_notes.as_str()
            } else if session.text.trim().is_empty() {
                session.rows.first().map_or("", |row| row.text.as_str())
            } else {
                &session.text
            };
            session.title = source
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(72)
                .collect();
            if session.title.is_empty() {
                session.title = match session.kind {
                    Kind::Dictation => "Dictation",
                    Kind::Call => "Call transcript",
                    Kind::Note => "Untitled note",
                }
                .into();
            }
        }
        let mut stored_session = session.clone();
        if stored_session.is_collection {
            stored_session.rows.clear();
            stored_session.text.clear();
            stored_session.original.clear();
            stored_session.notes = None;
        }
        stored_session.sources.clear();
        self.write_record(&stored_session)?;
        Ok(session)
    }

    /// Atomically replace the stored record exactly as given and index it.
    fn write_record(&self, session: &Session) -> Result<()> {
        let bytes = serde_json::to_vec(&Stored {
            schema: SCHEMA,
            session: session.clone(),
        })?;
        ensure!(
            bytes.len() <= MAX_BYTES,
            "This transcript is too large to save as one history item"
        );
        let bytes = protect::seal_if(self.protect.load(Ordering::Acquire), bytes)?;
        let main = self.path(&session.id, "json")?;
        let temporary = self.path(&session.id, "tmp")?;
        let backup = self.path(&session.id, "bak")?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        drop(file);
        if main.exists() {
            fs::rename(&main, &backup)?;
        }
        if let Err(error) = fs::rename(&temporary, &main) {
            if backup.exists() {
                let _ = fs::rename(&backup, &main);
            }
            return Err(error.into());
        }
        // The durable replacement is now installed. Keeping a leftover backup is safe.
        let _ = remove_if_present(&backup);
        self.remember(session, stamp(&main));
        Ok(())
    }

    /// Install a complete record without changing its timestamps. Used by
    /// restore paths after the caller has decided it must not lose newer data.
    pub(crate) fn install(&self, session: &Session) -> Result<()> {
        validate(session)?;
        ensure!(
            !self.path(&session.id, "deleted")?.exists(),
            "This history item was deleted"
        );
        self.write_record(session)
    }

    /// The stored version of a record, if any, without hydrating topics.
    pub(crate) fn stored(&self, id: &str) -> Result<Option<Session>> {
        self.recover(id)
    }

    pub(crate) fn is_deleted(&self, id: &str) -> Result<bool> {
        Ok(self.path(id, "deleted")?.exists())
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<Summary> {
        let mut session = self
            .recover(id)?
            .context("This history item is no longer available")?;
        if session.pinned != pinned {
            session.pinned = pinned;
            self.write_record(&session)?;
        }
        Ok(Summary::from(&session))
    }

    fn collection(&self, session: Session) -> Result<Session> {
        self.refresh()?;
        let (members, unreadable) = {
            let index = self.lock_index();
            let mut members: Vec<String> = index
                .entries
                .iter()
                .filter(|(_, entry)| {
                    !entry.summary.is_collection
                        && entry.summary.topic_id.as_deref() == Some(session.id.as_str())
                })
                .map(|(id, _)| id.clone())
                .collect();
            members.sort();
            (members, index.unreadable.clone())
        };
        for (id, kind) in unreadable {
            match kind {
                // Recovery already quarantined damaged records, just as the
                // history list does. They cannot prevent intact recordings opening.
                WarningKind::Damaged => {}
                // A newer or temporarily unreadable unrelated recording must not
                // disable every topic. Preserve the error whenever its ownership
                // is unknown or points here, so saving cannot erase a member.
                WarningKind::NewerVersion
                    if !self.unreadable_record_is_unrelated(&id, &session.id) =>
                {
                    return Err(UnsupportedSchema.into());
                }
                WarningKind::Unavailable
                    if !self.unreadable_record_is_unrelated(&id, &session.id) =>
                {
                    return Err(unavailable(
                        "A recording in this topic could not be read. Try again in a moment.",
                    ));
                }
                _ => {}
            }
        }
        let mut sources = Vec::new();
        let mut bytes = 0_usize;
        for id in members {
            let recovered = match self.recover(&id) {
                Ok(source) => source,
                Err(error) if error.is::<DamagedRecord>() => continue,
                Err(error) => return Err(error),
            };
            if let Some(source) = recovered
                && !source.is_collection
                && source.topic_id.as_deref() == Some(session.id.as_str())
            {
                bytes = bytes
                    .saturating_add(source.text.len())
                    .saturating_add(source.original.len())
                    .saturating_add(source.rows.iter().map(|row| row.text.len()).sum::<usize>());
                ensure!(
                    bytes <= MAX_TEXT_BYTES && sources.len() < crate::topics::MAX_SOURCES,
                    "This topic is too large. Move some recordings into a separate note."
                );
                sources.push(source);
            }
        }
        crate::topics::hydrate(session, sources)
    }

    fn unreadable_record_is_unrelated(&self, id: &str, topic_id: &str) -> bool {
        let mut inspected = false;
        for suffix in ["json", "tmp", "bak"] {
            let Ok(path) = self.path(id, suffix) else {
                return false;
            };
            if !path.exists() {
                continue;
            }
            let Ok(value) = read_session_value(&path) else {
                return false;
            };
            if value["session"]["id"].as_str() != Some(id) {
                return false;
            }
            match value["session"].get("topic_id") {
                Some(serde_json::Value::Null) => {}
                Some(serde_json::Value::String(parent))
                    if valid_id(parent) && parent != topic_id => {}
                _ => return false,
            }
            inspected = true;
        }
        inspected
    }

    pub fn move_source(
        &self,
        id: &str,
        destination: crate::topics::Destination,
        expected_topic_id: Option<String>,
    ) -> Result<crate::topics::Moved> {
        let mut source = self.load(id)?;
        ensure!(
            !source.is_collection,
            "Move a source recording, not a topic document."
        );
        let source_bytes = source
            .text
            .len()
            .saturating_add(source.original.len())
            .saturating_add(source.rows.iter().map(|row| row.text.len()).sum::<usize>());
        if !matches!(destination, crate::topics::Destination::Unfiled) {
            ensure!(
                source_bytes <= MAX_TEXT_BYTES,
                "This recording is too large to file into one topic."
            );
        }
        let previous_topic_id = source.topic_id.clone();
        if let crate::topics::Destination::Existing(target) = &destination
            && source.topic_id.as_ref() == Some(target)
        {
            return Ok(crate::topics::Moved {
                source,
                previous_topic_id,
                collections: vec![self.load(target)?],
            });
        }
        ensure!(
            source.topic_id == expected_topic_id,
            "This recording was moved elsewhere. Refresh before moving it again."
        );
        let target = match destination {
            crate::topics::Destination::Existing(id) => {
                ensure!(id != source.id, "A note cannot contain itself.");
                let mut target = self.load(&id)?;
                let target_bytes = target
                    .sources
                    .iter()
                    .map(|source| {
                        source
                            .text
                            .len()
                            .saturating_add(source.original.len())
                            .saturating_add(
                                source.rows.iter().map(|row| row.text.len()).sum::<usize>(),
                            )
                    })
                    .sum::<usize>();
                ensure!(
                    target_bytes.saturating_add(source_bytes) <= MAX_TEXT_BYTES
                        && target.rows.len().saturating_add(source.rows.len()) <= MAX_ROWS,
                    "This topic is too large. Choose another note."
                );
                ensure!(
                    target.can_receive_sources(),
                    "Choose a topic or written note."
                );
                if !target.is_collection {
                    target.is_collection = true;
                    target = crate::topics::hydrate(target, Vec::new())?;
                    self.save(target.clone())?;
                }
                ensure!(
                    target.sources.len() < crate::topics::MAX_SOURCES,
                    "This topic has too many sources."
                );
                Some(id)
            }
            crate::topics::Destination::New(title) => {
                ensure!(
                    !title.trim().is_empty()
                        && title.chars().count() <= 100
                        && !title.chars().any(char::is_control),
                    "Choose a short readable topic title."
                );
                let mut topic = Session::new(Kind::Note);
                topic.is_collection = true;
                topic.title = title.trim().into();
                topic.title_is_manual = true;
                let topic = crate::topics::hydrate(topic, Vec::new())?;
                Some(self.save(topic)?.id)
            }
            crate::topics::Destination::Unfiled => None,
        };
        source.topic_id.clone_from(&target);
        source.auto_file_pending = false;
        let source = self.save_inner(source, false)?;
        let mut collections = Vec::new();
        for id in previous_topic_id.iter().chain(target.iter()) {
            if !collections.iter().any(|topic: &Session| &topic.id == id) {
                collections.push(self.load(id)?);
            }
        }
        Ok(crate::topics::Moved {
            source,
            previous_topic_id,
            collections,
        })
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if let Some(session) = self.recover(id)?
            && session.is_collection
        {
            ensure!(
                self.collection(session)?.sources.is_empty(),
                "Move this note's recordings before deleting it."
            );
        }
        let marker = self.path(id, "deleted")?;
        // Persist deletion before removing copies. Recovery and stale queued saves
        // honor the marker, so an interrupted delete cannot resurrect the entry.
        if !marker.exists() {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(marker)?;
            file.sync_all()?;
        }
        self.forget(id);
        self.remove_copies(id, None)
    }

    /// Remove every copy of a deleted record. `corrupt` lists its quarantined
    /// files when the caller already enumerated the directory.
    fn remove_copies(&self, id: &str, corrupt: Option<&[PathBuf]>) -> Result<()> {
        for suffix in ["json", "bak", "tmp", "audio", "audio.recording"] {
            remove_if_present(&self.path(id, suffix)?)?;
        }
        match corrupt {
            Some(paths) => {
                for path in paths {
                    remove_if_present(path)?;
                }
            }
            None => {
                for entry in fs::read_dir(&self.directory)? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_str()
                        .and_then(parse_quarantine_name)
                        .is_some_and(|owner| owner == id)
                    {
                        remove_if_present(&entry.path())?;
                    }
                }
            }
        }
        Ok(())
    }

    fn path(&self, id: &str, suffix: &str) -> Result<PathBuf> {
        ensure!(valid_id(id), "Invalid history identifier");
        Ok(self.directory.join(format!("{id}.{suffix}")))
    }

    fn recover(&self, id: &str) -> Result<Option<Session>> {
        if self.path(id, "deleted")?.exists() {
            self.remove_copies(id, None)?;
            return Ok(None);
        }
        let main = self.path(id, "json")?;
        let temporary = self.path(id, "tmp")?;
        let backup = self.path(id, "bak")?;
        let mut invalid = Vec::new();
        for candidate in [&main, &temporary, &backup] {
            if !candidate.exists() {
                continue;
            }
            match read_session(candidate, id) {
                Ok(session) => {
                    if candidate != &main {
                        if main.exists() {
                            self.quarantine(&main, id)?;
                        }
                        fs::rename(candidate, &main)?;
                    }
                    remove_if_present(&temporary)?;
                    remove_if_present(&backup)?;
                    return Ok(Some(session));
                }
                // Never decide anything about a record from a partial view. An
                // unreadable copy may be the only good one.
                Err(error) if error.is::<UnsupportedSchema>() || error.is::<Unavailable>() => {
                    return Err(error);
                }
                Err(_) => invalid.push(candidate.clone()),
            }
        }
        if invalid.is_empty() {
            return Ok(None);
        }
        for path in invalid {
            self.quarantine(&path, id)?;
        }
        Err(DamagedRecord.into())
    }

    fn quarantine(&self, path: &Path, id: &str) -> Result<()> {
        fs::rename(
            path,
            self.directory.join(format!("{id}.corrupt-{}", new_id())),
        )?;
        self.quarantined_now.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn quarantine_file(&self, file_name: &str) -> Result<(PathBuf, String)> {
        let id = parse_quarantine_name(file_name).context("Unknown isolated history file")?;
        let path = self.directory.join(file_name);
        let metadata =
            fs::symlink_metadata(&path).context("This isolated file no longer exists")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Unknown isolated history file"
        );
        Ok((path, id.to_owned()))
    }

    /// Files moved aside because they could not be parsed or validated.
    pub fn quarantined(&self) -> Result<Vec<Quarantined>> {
        let mut found = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(id) = parse_quarantine_name(&name) else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let (restorable, title, reason) = match read_session(&entry.path(), id) {
                Ok(session) => (true, Some(session.title), "Complete record".to_owned()),
                Err(error) => (
                    false,
                    read_session_value(&entry.path())
                        .ok()
                        .and_then(|value| value["session"]["title"].as_str().map(str::to_owned)),
                    error.to_string(),
                ),
            };
            found.push(Quarantined {
                file_name: name.clone(),
                id: id.to_owned(),
                bytes: metadata.len(),
                modified_ms: metadata
                    .modified()
                    .ok()
                    .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |at| at.as_millis().min(u64::MAX as u128) as u64),
                restorable,
                title,
                reason,
            });
            if found.len() >= 1000 {
                break;
            }
        }
        found.sort_by(|a, b| {
            b.modified_ms
                .cmp(&a.modified_ms)
                .then_with(|| a.file_name.cmp(&b.file_name))
        });
        Ok(found)
    }

    /// Put an isolated record back. Refuses to replace a newer saved version.
    pub fn restore_quarantined(&self, file_name: &str) -> Result<Session> {
        let (path, id) = self.quarantine_file(file_name)?;
        let session = read_session(&path, &id).map_err(|error| {
            anyhow::anyhow!(
                "This file cannot be restored automatically ({error}). Export it to inspect its contents."
            )
        })?;
        ensure!(
            !self.is_deleted(&id)?,
            "This item was deleted. Export the isolated copy instead."
        );
        if let Some(existing) = self.recover(&id)? {
            ensure!(
                existing.updated_ms < session.updated_ms,
                "A newer saved version of this item exists. Export the isolated copy instead."
            );
        }
        self.write_record(&session)?;
        remove_if_present(&path)?;
        Ok(session)
    }

    /// Write the isolated file's contents, decrypted when possible, to `destination`.
    pub fn export_quarantined(&self, file_name: &str, destination: &Path) -> Result<()> {
        let (path, _) = self.quarantine_file(file_name)?;
        let metadata = fs::metadata(&path)?;
        ensure!(
            metadata.len() <= MAX_STORED_BYTES,
            "This isolated file is too large to export"
        );
        let bytes = fs::read(&path)?;
        let bytes = protect::open(bytes.clone()).unwrap_or(bytes);
        protect::write_atomic(destination, &bytes)?;
        Ok(())
    }

    pub fn discard_quarantined(&self, file_name: &str) -> Result<()> {
        let (path, _) = self.quarantine_file(file_name)?;
        remove_if_present(&path)
    }

    /// Rewrite every stored record, backup and isolated file so it is encrypted
    /// (`protect`) or plaintext. Each file is replaced atomically, and readers
    /// accept both forms, so an interrupted migration is safe and resumes when
    /// this is called again. In-flight `.tmp` files are left to recovery.
    pub fn set_protection(&self, protect: bool) -> Result<MigrationReport> {
        self.protect.store(protect, Ordering::Release);
        let mut report = MigrationReport {
            protect,
            ..Default::default()
        };
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some((id, suffix)) = name.split_once('.') else {
                continue;
            };
            if valid_id(id) && suffix == "audio" {
                // Retained audio is sealed segment by segment.
                match crate::audio_store::reprotect(&entry.path(), protect) {
                    Ok(true) => report.changed += 1,
                    Ok(false) => {}
                    Err(_) => report.failed += 1,
                }
                continue;
            }
            let eligible = valid_id(id)
                && (matches!(suffix, "json" | "bak") || parse_quarantine_name(&name).is_some());
            if !eligible {
                continue;
            }
            let path = entry.path();
            let before = stamp(&path);
            match protect::migrate_file(&path, protect, MAX_STORED_BYTES) {
                Ok(true) => {
                    report.changed += 1;
                    if suffix == "json" {
                        // Same content, new bytes: keep the cached entry valid.
                        let mut index = self.lock_index();
                        if let Some(cached) = index.entries.get_mut(id)
                            && cached.stamp == before
                        {
                            cached.stamp = stamp(&path);
                        }
                    }
                }
                Ok(false) => {}
                Err(_) => report.failed += 1,
            }
        }
        Ok(report)
    }

    /// Sessions the policy would delete now. Nothing is changed.
    pub fn retention_preview(
        &self,
        policy: &retention::Policy,
        active: &[String],
    ) -> Result<Vec<Summary>> {
        policy.validate()?;
        self.refresh()?;
        let now = now_ms();
        let mut affected: Vec<Summary> = self
            .lock_index()
            .entries
            .values()
            .filter(|entry| {
                retention::eligible(
                    policy,
                    &retention::Facts {
                        id: &entry.summary.id,
                        kind: entry.summary.kind,
                        updated_ms: entry.summary.updated_ms,
                        pinned: entry.summary.pinned,
                        is_collection: entry.summary.is_collection,
                        topic_id: entry.summary.topic_id.as_deref(),
                    },
                    now,
                    active,
                )
            })
            .map(|entry| entry.summary.clone())
            .collect();
        affected.sort_by(|a, b| {
            a.updated_ms
                .cmp(&b.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(affected)
    }

    /// Delete what the policy selects. With `confirmed`, only sessions from that
    /// earlier preview are deleted, and only if they still qualify.
    pub fn apply_retention(
        &self,
        policy: &retention::Policy,
        confirmed: Option<&[String]>,
        active: &[String],
    ) -> Result<retention::Applied> {
        let eligible: Vec<String> = self
            .retention_preview(policy, active)?
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        let mut applied = retention::Applied::default();
        let selected: Vec<String> = match confirmed {
            Some(confirmed) => {
                for id in confirmed {
                    if !eligible.contains(id) {
                        applied.skipped.push(id.clone());
                    }
                }
                eligible
                    .into_iter()
                    .filter(|id| confirmed.contains(id))
                    .collect()
            }
            None => eligible,
        };
        for id in selected {
            match self.delete(&id) {
                Ok(()) => applied.deleted.push(id),
                Err(error) => applied.failed.push((id, error.to_string())),
            }
        }
        Ok(applied)
    }
}

/// `{id}.corrupt-{suffix}` names the record a quarantined file belongs to.
fn parse_quarantine_name(name: &str) -> Option<&str> {
    let (id, suffix) = name.split_once('.')?;
    (valid_id(id) && suffix.strip_prefix("corrupt-").is_some_and(valid_id)).then_some(id)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Plaintext bytes of a stored file. I/O failures and decryption failures are
/// `Unavailable`; a wrong file type or size is damage.
fn read_record_bytes(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(unavailable)?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_STORED_BYTES,
        "Invalid history file"
    );
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(unavailable)?
        .take(MAX_STORED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unavailable)?;
    ensure!(
        bytes.len() as u64 <= MAX_STORED_BYTES,
        "History item exceeds size limit"
    );
    let bytes = protect::open(bytes).map_err(|error| {
        if error.is::<protect::Undecryptable>() {
            unavailable(error)
        } else {
            error
        }
    })?;
    ensure!(bytes.len() <= MAX_BYTES, "History item exceeds size limit");
    Ok(bytes)
}

fn read_session_value(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&read_record_bytes(path)?)?)
}

/// Parse a stored record, tolerating unknown fields (see `SCHEMA`).
pub(crate) fn parse_record(value: serde_json::Value, expected_id: &str) -> Result<Session> {
    if value["schema"]
        .as_u64()
        .is_some_and(|schema| schema > SCHEMA as u64)
    {
        return Err(UnsupportedSchema.into());
    }
    let legacy_title = value["session"].get("title_is_manual").is_none();
    let mut stored: Stored = serde_json::from_value(value)?;
    ensure!(
        (1..=SCHEMA).contains(&stored.schema) && stored.session.id == expected_id,
        "Invalid history schema or identity"
    );
    if legacy_title && stored.session.kind == Kind::Call {
        let session = &mut stored.session;
        let old_excerpt: String = session
            .rows
            .first()
            .map_or("", |row| row.text.as_str())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(72)
            .collect();
        if session.title.is_empty()
            || session.title == "Conversation"
            || session.title == "Call transcript"
            || session.title == old_excerpt
        {
            session.title_is_manual = false;
            if session.title.is_empty() {
                session.title = "Conversation".into();
            }
        }
    }
    validate(&stored.session)?;
    Ok(stored.session)
}

/// Serialize a record in the stored format, for portable archives.
pub(crate) fn encode_record(session: &Session) -> Result<Vec<u8>> {
    let mut session = session.clone();
    session.sources.clear();
    let bytes = serde_json::to_vec(&Stored {
        schema: SCHEMA,
        session,
    })?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "This transcript is too large to save as one history item"
    );
    Ok(bytes)
}

pub(crate) const RECORD_SCHEMA: u32 = SCHEMA;
pub(crate) const MAX_RECORD_BYTES: usize = MAX_BYTES;

fn read_session(path: &Path, expected_id: &str) -> Result<Session> {
    parse_record(read_session_value(path)?, expected_id)
}

fn validate(session: &Session) -> Result<()> {
    if let Some(app) = &session.metrics.app {
        ensure!(
            crate::dictionary::app_scope(app)?.is_some(),
            "Invalid dictation app metadata"
        );
    }
    ensure!(valid_id(&session.id), "Invalid history identifier");
    ensure!(
        session
            .topic_id
            .as_deref()
            .is_none_or(|id| valid_id(id) && id != session.id),
        "Invalid topic reference"
    );
    ensure!(
        !session.is_collection || (session.kind == Kind::Note && session.topic_id.is_none()),
        "Topic documents cannot be nested"
    );
    ensure!(
        session.title.len() <= 2048 && !session.title.contains('\0'),
        "History title is too long or invalid"
    );
    ensure!(
        session.text.len() <= MAX_TEXT_BYTES
            && session.original.len() <= MAX_TEXT_BYTES
            && session.personal_notes.len() <= MAX_TEXT_BYTES
            && session.rows.len() <= MAX_ROWS,
        "Transcript exceeds history size limit"
    );
    ensure!(
        session.speaker_names.len() <= 8
            && session.speaker_names.iter().all(|name| name.len() <= 512),
        "Speaker name exceeds size limit"
    );
    ensure!(
        session.speaker_edits.len() <= 4 * crate::speaker_edits::MAX_UNDO
            && session
                .speaker_edits
                .iter()
                .all(|edit| edit.rows.len() <= MAX_ROWS && edit.summary.len() <= 1024),
        "Speaker corrections exceed size limit"
    );
    ensure!(
        session.interruptions.len() <= 2 * crate::calls::interruptions::MAX_STORED
            && session
                .interruptions
                .iter()
                .all(|interruption| interruption.end_ms >= interruption.start_ms),
        "Invalid audio interruption"
    );
    ensure!(
        session
            .audio
            .as_ref()
            .is_none_or(|audio| audio.channels.len() <= 8
                && audio.format.len() <= 64
                && audio.channels.iter().all(|name| name.len() <= 64)),
        "Invalid retained audio details"
    );
    ensure!(
        session.transcript_versions.len() <= 2 * MAX_TRANSCRIPT_VERSIONS,
        "Too many saved transcript versions"
    );
    let mut text_bytes = session
        .text
        .len()
        .saturating_add(session.original.len())
        .saturating_add(session.personal_notes.len());
    if let Some(summary) = &session.generated_summary {
        let bytes = serde_json::to_vec(summary)?.len();
        ensure!(
            bytes <= MAX_TEXT_BYTES,
            "Generated summary exceeds history size limit"
        );
        text_bytes = text_bytes.saturating_add(bytes);
    }
    let protection_bytes = serde_json::to_vec(&session.protected_note_items)?.len();
    ensure!(
        protection_bytes <= MAX_TEXT_BYTES,
        "Note edit history exceeds size limit"
    );
    text_bytes = text_bytes.saturating_add(protection_bytes);
    for version in &session.transcript_versions {
        ensure!(
            version.rows.len() <= MAX_ROWS
                && version.text.len() <= MAX_TEXT_BYTES
                && version.original.len() <= MAX_TEXT_BYTES
                && version.speaker_names.len() <= 8,
            "Saved transcript version exceeds size limit"
        );
        text_bytes = text_bytes
            .saturating_add(version.text.len())
            .saturating_add(version.original.len())
            .saturating_add(version.rows.iter().map(|row| row.text.len()).sum());
    }
    for row in &session.rows {
        ensure!(
            row.end_ms >= row.start_ms && row.speakers.len() <= 4,
            "Invalid transcript section"
        );
        if let Some(attribution) = &row.discord {
            ensure!(
                attribution.speakers.len() <= 256
                    && attribution
                        .speakers
                        .iter()
                        .all(|speaker| speaker.name.len() <= 512
                            && speaker.avatar.as_ref().is_none_or(|avatar| avatar.valid())),
                "Invalid saved speaker labels"
            );
        }
        text_bytes = text_bytes.saturating_add(row.text.len());
    }
    if let Some(notes) = &session.notes {
        ensure!(
            notes.highlights.len() <= 1000 && notes.actions.len() <= 1000,
            "Notes exceed size limit"
        );
        for quote in notes.highlights.iter().chain(&notes.actions) {
            ensure!(
                quote.row < session.rows.len()
                    && quote.end_ms >= quote.start_ms
                    && quote.speakers.len() <= 4,
                "Invalid saved note reference"
            );
            text_bytes = text_bytes.saturating_add(quote.text.len());
            if let Some(attribution) = &quote.discord {
                ensure!(
                    attribution.speakers.len() <= 256
                        && attribution
                            .speakers
                            .iter()
                            .all(|speaker| speaker.name.len() <= 512
                                && speaker.avatar.as_ref().is_none_or(|avatar| avatar.valid())),
                    "Invalid saved note labels"
                );
            }
        }
    }
    ensure!(
        text_bytes <= MAX_BYTES,
        "Transcript exceeds history size limit"
    );
    Ok(())
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn new_id() -> String {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let seed = (
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        sequence,
    );
    let a = RandomState::new().hash_one(seed);
    let b = RandomState::new().hash_one((seed, a));
    format!("{a:016x}{b:016x}")
}

/// Remove temporary call-audio WAV files left behind when an earlier run of
/// Articulate stopped during audio analysis. Audio is never meant to persist.
/// Files written by this process are kept, since analysis may be running.
/// Returns how many files were removed.
pub fn remove_leftover_call_audio(directory: &Path) -> usize {
    let current = format!("{}-", std::process::id());
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".wav") else {
            continue;
        };
        let generated = stem.split_once('-').is_some_and(|(pid, serial)| {
            !pid.is_empty()
                && !serial.is_empty()
                && pid.bytes().all(|b| b.is_ascii_digit())
                && serial.bytes().all(|b| b.is_ascii_digit())
        });
        let is_file = entry
            .file_type()
            .is_ok_and(|kind| kind.is_file() && !kind.is_symlink());
        if generated
            && is_file
            && !name.starts_with(&current)
            && fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

pub enum Event {
    Insights(crate::insights::Report),
    InsightsFailed(String),
    Listed(Listing),
    Loaded(Box<Session>),
    SourceMoved(Box<crate::topics::Moved>),
    SourceMoveFailed {
        id: String,
        error: String,
    },
    CollectionsChanged(Vec<Session>),
    FilingPending(Vec<Session>),
    /// `summary` reflects the stored record, so lists update without a reload.
    Saved {
        id: String,
        updated_ms: u64,
        summary: Box<Summary>,
    },
    Deleted {
        id: String,
    },
    DeleteFailed {
        id: String,
        error: String,
    },
    Failed(String),
    Quarantine(Vec<Quarantined>),
    QuarantineRestored(Box<Session>),
    QuarantineExported {
        file_name: String,
        path: PathBuf,
    },
    QuarantineDiscarded {
        file_name: String,
    },
    QuarantineFailed {
        file_name: String,
        error: String,
    },
    Pinned(Box<Summary>),
    ProtectionChanged(Result<MigrationReport, String>),
    RetentionPreview(Result<Vec<Summary>, String>),
    RetentionApplied(Result<retention::Applied, String>),
    BackupCreated(Result<backup::Created, String>),
    RestoreFinished(Result<Box<backup::Restored>, String>),
    MarkdownExported(Result<backup::Exported, String>),
}

enum Command {
    Insights,
    List,
    Load(String),
    MoveSource(String, crate::topics::Destination, Option<String>),
    Save(PendingSave),
    Delete(String),
    Quarantined,
    RestoreQuarantined(String),
    ExportQuarantined(String, PathBuf),
    DiscardQuarantined(String),
    SetPinned(String, bool),
    SetProtection(bool),
    RetentionPreview(retention::Policy, Vec<String>),
    ApplyRetention(retention::Policy, Option<Vec<String>>, Vec<String>),
    Backup(Box<backup::Request>),
    Restore(Box<backup::RestoreRequest>),
    ExportMarkdown(PathBuf),
}

#[derive(Clone)]
struct PendingSave {
    revision: u64,
    session: Box<Session>,
}

impl std::ops::Deref for PendingSave {
    type Target = Session;
    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

struct WorkState {
    commands: VecDeque<Command>,
    events: VecDeque<Event>,
    stopped: bool,
    failed_saves: BTreeMap<String, PendingSave>,
    active_save: Option<String>,
    active_operation: bool,
}
struct Shared {
    state: Mutex<WorkState>,
    wake: Condvar,
}

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    search: Option<SearchIndex>,
}

impl Worker {
    #[cfg(test)]
    pub(crate) fn test_directory(directory: PathBuf) -> Self {
        Self::start_directory(directory)
    }

    /// Start the history worker with plaintext writes.
    #[cfg(test)]
    pub fn start() -> Self {
        Self::start_protected(false)
    }

    /// Start the history worker. `protect` selects encryption for new writes
    /// and finishes any interrupted encryption migration before listing.
    pub fn start_protected(protect: bool) -> Self {
        let data = crate::model::data_dir();
        remove_leftover_call_audio(&data.join("audio-context-temp"));
        crate::audio_store::recover_interrupted(&directory());
        Self::start_directory_protected(directory(), protect)
    }

    #[cfg(test)]
    fn start_directory(directory: PathBuf) -> Self {
        Self::start_directory_protected(directory, false)
    }

    fn start_directory_protected(directory: PathBuf, protect: bool) -> Self {
        let search = SearchIndex::in_memory().ok();
        let shared = Arc::new(Shared {
            state: Mutex::new(WorkState {
                commands: VecDeque::from([Command::SetProtection(protect), Command::List]),
                events: VecDeque::new(),
                stopped: false,
                failed_saves: BTreeMap::new(),
                active_save: None,
                active_operation: false,
            }),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let worker_search = search.clone();
        let thread = thread::Builder::new()
            .name("local-history".into())
            .spawn(move || history_worker(worker_shared, directory, worker_search, protect));
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(_) => {
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .events
                    .push_back(Event::Failed("Could not start local history.".into()));
                None
            }
        };
        Self {
            shared,
            thread,
            search,
        }
    }

    /// The live full-text index maintained by this worker, if it could be
    /// created. Check `is_ready` before relying on complete results.
    pub fn search_index(&self) -> Option<SearchIndex> {
        self.search.clone()
    }

    pub fn insights(&self) {
        self.enqueue(Command::Insights);
    }

    pub fn list(&self) {
        self.enqueue(Command::List);
    }
    pub fn load(&self, id: String) {
        self.enqueue(Command::Load(id));
    }
    pub fn move_source(
        &self,
        id: String,
        destination: crate::topics::Destination,
        expected_topic_id: Option<String>,
    ) {
        self.enqueue(Command::MoveSource(id, destination, expected_topic_id));
    }
    pub fn save(&self, session: Session) {
        self.enqueue(Command::Save(PendingSave {
            revision: NEXT_SAVE.fetch_add(1, Ordering::Relaxed),
            session: Box::new(session),
        }));
    }
    pub fn delete(&self, id: String) {
        self.enqueue(Command::Delete(id));
    }
    /// Replies with `Event::Quarantine`.
    pub fn quarantined(&self) {
        self.enqueue(Command::Quarantined);
    }
    /// Replies with `QuarantineRestored` or `QuarantineFailed`.
    pub fn restore_quarantined(&self, file_name: String) {
        self.enqueue(Command::RestoreQuarantined(file_name));
    }
    /// Replies with `QuarantineExported` or `QuarantineFailed`.
    pub fn export_quarantined(&self, file_name: String, destination: PathBuf) {
        self.enqueue(Command::ExportQuarantined(file_name, destination));
    }
    /// Replies with `QuarantineDiscarded` or `QuarantineFailed`.
    pub fn discard_quarantined(&self, file_name: String) {
        self.enqueue(Command::DiscardQuarantined(file_name));
    }
    /// Replies with `Pinned` or `Failed`.
    pub fn set_pinned(&self, id: String, pinned: bool) {
        self.enqueue(Command::SetPinned(id, pinned));
    }
    /// Replies with `ProtectionChanged`.
    pub fn set_protection(&self, protect: bool) {
        self.enqueue(Command::SetProtection(protect));
    }
    /// Replies with `RetentionPreview`. `active` sessions are always kept.
    pub fn retention_preview(&self, policy: retention::Policy, active: Vec<String>) {
        self.enqueue(Command::RetentionPreview(policy, active));
    }
    /// Replies with `RetentionApplied`, preceded by `Deleted` for each removal.
    pub fn apply_retention(
        &self,
        policy: retention::Policy,
        confirmed: Option<Vec<String>>,
        active: Vec<String>,
    ) {
        self.enqueue(Command::ApplyRetention(policy, confirmed, active));
    }
    /// Replies with `BackupCreated`.
    pub fn backup(&self, request: backup::Request) {
        self.enqueue(Command::Backup(Box::new(request)));
    }
    /// Replies with `RestoreFinished`, then a fresh `Listed`.
    pub fn restore(&self, request: backup::RestoreRequest) {
        self.enqueue(Command::Restore(Box::new(request)));
    }
    /// Replies with `MarkdownExported`.
    pub fn export_markdown(&self, folder: PathBuf) {
        self.enqueue(Command::ExportMarkdown(folder));
    }
    pub fn retry(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let sessions: Vec<_> = state
            .failed_saves
            .values()
            .filter(|session| {
                state.active_save.as_deref() != Some(session.id.as_str())
                    && !state.commands.iter().any(|command| match command {
                        Command::Save(pending) => pending.id == session.id,
                        Command::Delete(id) => *id == session.id,
                        _ => false,
                    })
            })
            .cloned()
            .collect();
        for session in sessions {
            if state.commands.len() >= QUEUE_CAPACITY || state.stopped {
                break;
            }
            state.commands.push_back(Command::Save(session));
        }
        self.shared.wake.notify_one();
    }
    pub fn drain(&self) -> Vec<Event> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .drain(..)
            .collect()
    }

    /// Poll before a deliberate app shutdown. The caller must prevent new edits
    /// between a successful result and closing, and surface errors without exiting.
    pub fn saves_settled(&self) -> Result<bool, String> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped
            || self
                .thread
                .as_ref()
                .is_none_or(|thread| thread.is_finished())
        {
            return Err(
                "Local history is unavailable. Save or export your text before updating.".into(),
            );
        }
        if !state.failed_saves.is_empty() {
            return Err(
                "Some changes could not be saved. Retry saving in History before updating.".into(),
            );
        }
        Ok(!state.active_operation && state.commands.is_empty())
    }

    fn enqueue(&self, command: Command) {
        let insights = matches!(command, Command::Insights);
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match &command {
            Command::Save(session) => {
                if let Some(queued) = state
                    .commands
                    .iter_mut()
                    .rev()
                    .find(|queued| matches!(queued, Command::Save(old) if old.id == session.id))
                {
                    *queued = command;
                    return;
                }
            }
            Command::List
                if state
                    .commands
                    .iter()
                    .any(|command| matches!(command, Command::List)) =>
            {
                return;
            }
            Command::Insights
                if state
                    .commands
                    .iter()
                    .any(|command| matches!(command, Command::Insights)) =>
            {
                return;
            }
            Command::Delete(id) => state
                .commands
                .retain(|command| !matches!(command, Command::Save(session) if session.id == *id)),
            _ => {}
        }
        if state.stopped || state.commands.len() >= QUEUE_CAPACITY {
            let busy = busy_event(&command);
            if let Command::Save(session) = command
                && (state.failed_saves.len() < QUEUE_CAPACITY
                    || state.failed_saves.contains_key(&session.id))
            {
                state.failed_saves.insert(session.id.clone(), session);
            }
            if state.events.len() >= 128 {
                state.events.pop_front();
            }
            state.events.push_back(if insights {
                Event::InsightsFailed("History is busy. Refresh Insights shortly.".into())
            } else {
                busy
            });
        } else {
            state.commands.push_back(command);
            self.shared.wake.notify_one();
        }
    }
}

/// The failure reported when a command cannot be queued.
fn busy_event(command: &Command) -> Event {
    let busy = "History is busy. Please try again.".to_owned();
    match command {
        Command::Delete(id) => Event::DeleteFailed {
            id: id.clone(),
            error: "History is busy. Please try deleting again.".into(),
        },
        Command::MoveSource(id, _, _) => Event::SourceMoveFailed {
            id: id.clone(),
            error: "History is busy. Please try moving again.".into(),
        },
        Command::RestoreQuarantined(file_name)
        | Command::ExportQuarantined(file_name, _)
        | Command::DiscardQuarantined(file_name) => Event::QuarantineFailed {
            file_name: file_name.clone(),
            error: busy,
        },
        Command::SetProtection(_) => Event::ProtectionChanged(Err(busy)),
        Command::RetentionPreview(..) => Event::RetentionPreview(Err(busy)),
        Command::ApplyRetention(..) => Event::RetentionApplied(Err(busy)),
        Command::Backup(_) => Event::BackupCreated(Err(busy)),
        Command::Restore(_) => Event::RestoreFinished(Err(busy)),
        Command::ExportMarkdown(_) => Event::MarkdownExported(Err(busy)),
        _ => Event::Failed("History is busy. Please try saving again.".into()),
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopped = true;
        self.shared.wake.notify_one();
        // Drain accepted saves before exit. The worker never owns this handle.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_history(directory: &Path, search: &Option<SearchIndex>, protect: bool) -> Result<History> {
    let mut history = History::open(directory.to_path_buf())?.with_protection(protect);
    if let Some(search) = search {
        // A reopened store starts from an empty index and rebuilds it.
        let _ = search.clear();
        history = history.with_search(search.clone());
    }
    Ok(history)
}

fn history_worker(
    shared: Arc<Shared>,
    directory: PathBuf,
    search: Option<SearchIndex>,
    mut protect: bool,
) {
    let mut history = open_history(&directory, &search, protect);
    loop {
        let command = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.commands.is_empty() && !state.stopped {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            let Some(command) = state.commands.pop_front() else {
                return;
            };
            state.active_operation = true;
            state.active_save = match &command {
                Command::Save(session) => Some(session.id.clone()),
                _ => None,
            };
            command
        };
        if let Command::SetProtection(value) = &command {
            protect = *value;
        }
        if history.is_err() {
            history = open_history(&directory, &search, protect);
        }
        let insights = matches!(command, Command::Insights);
        let deleting = match &command {
            Command::Delete(id) => Some(id.clone()),
            _ => None,
        };
        let moving = match &command {
            Command::MoveSource(id, _, _) => Some(id.clone()),
            _ => None,
        };
        let retry = match &command {
            Command::Save(session) => Some(session.clone()),
            _ => None,
        };
        let failure = busy_event(&command);
        let mut refreshed_collections = Vec::new();
        let mut pending_filing = None;
        let mut preceding = Vec::new();
        let mut relist = false;
        let result = match &history {
            Ok(history) => match command {
                Command::Insights => history.insights().map(Event::Insights),
                Command::List => history.list_with_pending().map(|(listed, pending)| {
                    pending_filing = Some(pending);
                    Event::Listed(listed)
                }),
                Command::Load(id) => history
                    .load(&id)
                    .map(|session| Event::Loaded(Box::new(session))),
                Command::Save(pending) => {
                    history.save(*pending.session).map(|session| Event::Saved {
                        id: session.id.clone(),
                        updated_ms: session.updated_ms,
                        summary: Box::new(Summary::from(&session)),
                    })
                }
                Command::Delete(id) => {
                    let parent = history
                        .recover(&id)
                        .ok()
                        .flatten()
                        .and_then(|session| session.topic_id);
                    history.delete(&id).map(|()| {
                        if let Some(parent) = parent
                            && let Ok(collection) = history.load(&parent)
                        {
                            refreshed_collections.push(collection);
                        }
                        Event::Deleted { id }
                    })
                }
                Command::MoveSource(id, destination, expected) => history
                    .move_source(&id, destination, expected)
                    .map(|moved| Event::SourceMoved(Box::new(moved))),
                Command::Quarantined => history.quarantined().map(Event::Quarantine),
                Command::RestoreQuarantined(file_name) => {
                    relist = true;
                    Ok(match history.restore_quarantined(&file_name) {
                        Ok(session) => Event::QuarantineRestored(Box::new(session)),
                        Err(error) => Event::QuarantineFailed {
                            file_name,
                            error: error.to_string(),
                        },
                    })
                }
                Command::ExportQuarantined(file_name, path) => {
                    Ok(match history.export_quarantined(&file_name, &path) {
                        Ok(()) => Event::QuarantineExported { file_name, path },
                        Err(error) => Event::QuarantineFailed {
                            file_name,
                            error: error.to_string(),
                        },
                    })
                }
                Command::DiscardQuarantined(file_name) => {
                    relist = true;
                    Ok(match history.discard_quarantined(&file_name) {
                        Ok(()) => Event::QuarantineDiscarded { file_name },
                        Err(error) => Event::QuarantineFailed {
                            file_name,
                            error: error.to_string(),
                        },
                    })
                }
                Command::SetPinned(id, pinned) => history
                    .set_pinned(&id, pinned)
                    .map(|summary| Event::Pinned(Box::new(summary))),
                Command::SetProtection(protect) => Ok(Event::ProtectionChanged(
                    history.set_protection(protect).map_err(|e| e.to_string()),
                )),
                Command::RetentionPreview(policy, active) => Ok(Event::RetentionPreview(
                    history
                        .retention_preview(&policy, &active)
                        .map_err(|e| e.to_string()),
                )),
                Command::ApplyRetention(policy, confirmed, active) => {
                    let applied = history
                        .apply_retention(&policy, confirmed.as_deref(), &active)
                        .map_err(|e| e.to_string());
                    if let Ok(applied) = &applied {
                        preceding.extend(
                            applied
                                .deleted
                                .iter()
                                .map(|id| Event::Deleted { id: id.clone() }),
                        );
                    }
                    Ok(Event::RetentionApplied(applied))
                }
                Command::Backup(request) => Ok(Event::BackupCreated(
                    backup::create(history, &request).map_err(|e| e.to_string()),
                )),
                Command::Restore(request) => {
                    relist = true;
                    Ok(Event::RestoreFinished(
                        backup::restore(history, &request)
                            .map(Box::new)
                            .map_err(|e| e.to_string()),
                    ))
                }
                Command::ExportMarkdown(folder) => Ok(Event::MarkdownExported(
                    backup::export_markdown(history, &folder).map_err(|e| e.to_string()),
                )),
            },
            Err(_) => Err(anyhow::anyhow!(
                "Could not open local history. Check that the local data folder is writable."
            )),
        };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active_save = None;
        state.active_operation = false;
        match (&result, retry) {
            (Err(_), Some(session)) => {
                if (state.failed_saves.len() < QUEUE_CAPACITY
                    || state.failed_saves.contains_key(&session.id))
                    && state
                        .failed_saves
                        .get(&session.id)
                        .is_none_or(|existing| existing.revision <= session.revision)
                {
                    state.failed_saves.insert(session.id.clone(), session);
                }
            }
            (Ok(Event::Saved { id, .. }), Some(saved)) => {
                if state
                    .failed_saves
                    .get(id)
                    .is_some_and(|failed| failed.revision <= saved.revision)
                {
                    state.failed_saves.remove(id);
                }
            }
            (Ok(Event::Deleted { id }), _) => {
                state.failed_saves.remove(id);
            }
            _ => {}
        }
        for event in &preceding {
            if let Event::Deleted { id } = event {
                state.failed_saves.remove(id);
            }
        }
        let event = result.unwrap_or_else(|error| {
            let error = error.to_string();
            if insights {
                Event::InsightsFailed(error)
            } else if let Some(id) = deleting {
                Event::DeleteFailed { id, error }
            } else if let Some(id) = moving {
                Event::SourceMoveFailed { id, error }
            } else {
                match failure {
                    Event::QuarantineFailed { file_name, .. } => {
                        Event::QuarantineFailed { file_name, error }
                    }
                    _ => Event::Failed(error),
                }
            }
        });
        // Keep important acknowledgements, coalesce replaceable list responses.
        if matches!(event, Event::Listed(_)) {
            state.events.retain(|old| !matches!(old, Event::Listed(_)));
        }
        if matches!(event, Event::Insights(_) | Event::InsightsFailed(_)) {
            state
                .events
                .retain(|old| !matches!(old, Event::Insights(_) | Event::InsightsFailed(_)));
        }
        for event in preceding.into_iter().chain(Some(event)) {
            if state.events.len() >= 128 {
                state.events.pop_front();
            }
            state.events.push_back(event);
        }
        if !refreshed_collections.is_empty() {
            state
                .events
                .push_back(Event::CollectionsChanged(refreshed_collections));
        }
        if let Some(pending) = pending_filing {
            state
                .events
                .retain(|event| !matches!(event, Event::FilingPending(_)));
            state.events.push_back(Event::FilingPending(pending));
        }
        if relist
            && !state.stopped
            && !state
                .commands
                .iter()
                .any(|command| matches!(command, Command::List))
        {
            state.commands.push_back(Command::List);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord_attribution::{Attribution, NamedSpeaker};
    use std::collections::BTreeSet;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("articulate-history-test-{}", new_id())))
        }
        fn history(&self) -> History {
            History::open(self.0.clone()).unwrap()
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn topic_move_is_atomic_preserves_sources_and_stale_saves_cannot_undo_it() {
        use crate::topics::Destination;
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut first = Session::new(Kind::Dictation);
        first.text = "A synthetic launch plan.".into();
        first.original = "Uh a synthetic launch plan.".into();
        let first = history.save(first).unwrap();
        let mut second = Session::new(Kind::Note);
        second.text = "An independent timeline.".into();
        let second = history.save(second).unwrap();
        let moved = history
            .move_source(&first.id, Destination::New("Launch".into()), None)
            .unwrap();
        let topic_id = moved.source.topic_id.clone().unwrap();
        history
            .move_source(&second.id, Destination::Existing(topic_id.clone()), None)
            .unwrap();
        let mut topic = history.load(&topic_id).unwrap();
        topic.personal_notes = "Human-written context stays with this topic.".into();
        history.save(topic.clone()).unwrap();
        let old_revision = topic.source_revision.clone();
        let moved = history
            .move_source(
                &first.id,
                Destination::New("Budget".into()),
                Some(topic_id.clone()),
            )
            .unwrap();
        let destination = moved.source.topic_id.clone().unwrap();
        assert_eq!(moved.source.original, first.original);
        assert_eq!(moved.source.text, first.text);
        let remaining = history.load(&topic_id).unwrap();
        assert_eq!(remaining.sources.len(), 1);
        assert_eq!(remaining.sources[0].id, second.id);
        assert_eq!(
            remaining.personal_notes,
            "Human-written context stays with this topic."
        );
        assert_ne!(remaining.source_revision, old_revision);
        // The durable source owns membership even if an earlier editor saves.
        history.save(first.clone()).unwrap();
        assert_eq!(
            history.load(&first.id).unwrap().topic_id.as_deref(),
            Some(destination.as_str())
        );
        assert!(history.save(topic).is_err());
        // Repeating the exact move is harmless; moving based on an obsolete
        // parent is rejected before creating another topic or touching data.
        history
            .move_source(
                &first.id,
                Destination::Existing(destination.clone()),
                Some(topic_id.clone()),
            )
            .unwrap();
        assert!(
            history
                .move_source(
                    &first.id,
                    Destination::New("Duplicate".into()),
                    Some(topic_id.clone())
                )
                .is_err()
        );
        history
            .move_source(
                &first.id,
                Destination::Existing(topic_id.clone()),
                Some(destination.clone()),
            )
            .unwrap();
        assert_eq!(history.load(&topic_id).unwrap().sources.len(), 2);
        assert!(history.load(&destination).unwrap().sources.is_empty());
        assert!(history.delete(&topic_id).is_err());
        let disk: serde_json::Value =
            serde_json::from_slice(&fs::read(history.path(&topic_id, "json").unwrap()).unwrap())
                .unwrap();
        assert!(disk["session"].get("sources").is_none());
        assert_eq!(disk["session"]["text"], "");
    }

    #[test]
    fn topics_skip_unrelated_future_records_but_preserve_unreadable_members() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut source = Session::new(Kind::Note);
        source.text = "A valid recorded thought.".into();
        let source = history.save(source).unwrap();
        let topic_id = history
            .move_source(
                &source.id,
                crate::topics::Destination::New("Project".into()),
                None,
            )
            .unwrap()
            .source
            .topic_id
            .unwrap();
        let mut topic = history.load(&topic_id).unwrap();
        topic.personal_notes = "Preserve my independent prose.".into();
        history.save(topic).unwrap();
        let future = Session::new(Kind::Note);
        let future_path = history.path(&future.id, "json").unwrap();
        let mut value = serde_json::json!({"schema": SCHEMA + 1, "session": future});
        let unrelated_bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&future_path, &unrelated_bytes).unwrap();
        let loaded = history.load(&topic_id).unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.sources[0].id, source.id);
        history.save(loaded.clone()).unwrap();
        assert_eq!(fs::read(&future_path).unwrap(), unrelated_bytes);

        value["session"]["topic_id"] = serde_json::json!(topic_id);
        fs::write(&future_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let topic_path = history.path(&topic_id, "json").unwrap();
        let preserved = fs::read(&topic_path).unwrap();
        assert!(history.load(&topic_id).is_err());
        assert!(history.save(loaded.clone()).is_err());
        assert_eq!(fs::read(&topic_path).unwrap(), preserved);

        value["session"].as_object_mut().unwrap().remove("topic_id");
        fs::write(&future_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(history.load(&topic_id).is_err());
        assert_eq!(fs::read(&topic_path).unwrap(), preserved);

        // Completely damaged unrelated files are quarantined once without
        // blocking otherwise intact collections.
        fs::write(&future_path, b"invalid json").unwrap();
        let loaded = history.load(&topic_id).unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.personal_notes, "Preserve my independent prose.");
        assert!(!future_path.exists());
    }

    #[test]
    fn pending_filing_survives_reload_but_explicit_unfiling_clears_it() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut source = Session::new(Kind::Note);
        source.text = "A thought waiting for the local model.".into();
        source.auto_file_pending = true;
        let source = history.save(source).unwrap();
        let reopened = directory.history();
        let (_, pending) = reopened.list_with_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, source.id);
        reopened
            .move_source(&source.id, crate::topics::Destination::Unfiled, None)
            .unwrap();
        reopened.save(source.clone()).unwrap();
        assert!(!reopened.load(&source.id).unwrap().auto_file_pending);
        assert!(reopened.list_with_pending().unwrap().1.is_empty());
    }

    #[test]
    fn deleting_a_source_refreshes_its_collection_without_removing_manual_prose() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut source = Session::new(Kind::Dictation);
        source.text = "A removable recording.".into();
        let source = history.save(source).unwrap();
        let topic_id = history
            .move_source(
                &source.id,
                crate::topics::Destination::New("Project".into()),
                None,
            )
            .unwrap()
            .source
            .topic_id
            .unwrap();
        let mut topic = history.load(&topic_id).unwrap();
        topic.personal_notes = "My independent notes.".into();
        history.save(topic).unwrap();
        let worker = Worker::test_directory(directory.0.clone());
        worker.delete(source.id.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut deleted = false;
        let mut refreshed = false;
        while !deleted || !refreshed {
            for event in worker.drain() {
                match event {
                    Event::Deleted { id } if id == source.id => deleted = true,
                    Event::CollectionsChanged(collections) => {
                        assert_eq!(collections.len(), 1);
                        assert_eq!(collections[0].id, topic_id);
                        assert_eq!(collections[0].personal_notes, "My independent notes.");
                        assert!(collections[0].sources.is_empty());
                        refreshed = true;
                    }
                    Event::Failed(error) | Event::DeleteFailed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "History delete did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn pre_topic_history_files_default_to_independent_sources() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = Session::new(Kind::Dictation);
        session.text = "An earlier recording.".into();
        let mut value = serde_json::to_value(&session).unwrap();
        for field in [
            "topic_id",
            "auto_file_pending",
            "is_collection",
            "source_revision",
        ] {
            value.as_object_mut().unwrap().remove(field);
        }
        fs::write(
            history.path(&session.id, "json").unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema":4,"session":value})).unwrap(),
        )
        .unwrap();
        let restored = history.load(&session.id).unwrap();
        assert_eq!(restored.text, session.text);
        assert!(restored.topic_id.is_none());
        assert!(!restored.is_collection);
        assert!(restored.sources.is_empty());
    }

    #[test]
    fn written_notes_accept_sources_without_replacing_manual_prose() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut note = Session::new(Kind::Note);
        note.title = "Project notes".into();
        note.personal_notes = "My original written plan.".into();
        let note = history.save(note).unwrap();
        assert!(Summary::from(&note).can_receive_sources);
        let mut source = Session::new(Kind::Dictation);
        source.text = "A recorded follow-up.".into();
        let source = history.save(source).unwrap();
        history
            .move_source(
                &source.id,
                crate::topics::Destination::Existing(note.id.clone()),
                None,
            )
            .unwrap();
        let promoted = history.load(&note.id).unwrap();
        assert!(promoted.is_collection);
        assert_eq!(promoted.personal_notes, note.personal_notes);
        assert_eq!(promoted.title, note.title);
        assert_eq!(promoted.sources[0].id, source.id);
    }

    #[test]
    fn topic_hydration_removes_only_departed_generated_paragraphs() {
        use crate::topics::Destination;
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut a = Session::new(Kind::Dictation);
        a.text = "First source.".into();
        let a = history.save(a).unwrap();
        let mut b = Session::new(Kind::Dictation);
        b.text = "Second source.".into();
        let b = history.save(b).unwrap();
        let topic_id = history
            .move_source(&a.id, Destination::New("Project".into()), None)
            .unwrap()
            .source
            .topic_id
            .unwrap();
        history
            .move_source(&b.id, Destination::Existing(topic_id.clone()), None)
            .unwrap();
        let mut topic = history.load(&topic_id).unwrap();
        let items = [&a, &b]
            .iter()
            .enumerate()
            .map(|(index, source)| crate::brain::Item {
                kind: crate::brain::Kind::Fact,
                text: format!("Generated paragraph {}.", index + 1),
                section: 1,
                sources: vec![crate::brain::Citation {
                    source_id: format!("{}:text", source.id),
                    start_byte: 0,
                    end_byte: source.text.len(),
                    start_ms: 0,
                    end_ms: 0,
                    speaker: None,
                    excerpt: source.text.clone(),
                    context: None,
                }],
            })
            .collect();
        topic.generated_summary = Some(crate::brain::Draft {
            schema: 1,
            source_id: topic_id.clone(),
            source_hash: String::new(),
            model: crate::brain::MODEL_LABEL.into(),
            sections: 1,
            elapsed_ms: 0,
            items,
            title: None,
        });
        topic.personal_notes="Manual context.\n\nGenerated paragraph 1.\n\nGenerated paragraph 2.\n\nMy edited version of paragraph 1.\n\nGenerated paragraph 1.".into();
        history.save(topic).unwrap();
        history
            .move_source(&a.id, Destination::Unfiled, Some(topic_id.clone()))
            .unwrap();
        let topic = history.load(&topic_id).unwrap();
        assert_eq!(
            topic.personal_notes,
            "Manual context.\n\nGenerated paragraph 2.\n\nMy edited version of paragraph 1.\n\nGenerated paragraph 1."
        );
        let transcript = crate::topics::transcript(&topic);
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].id, format!("{}:text", b.id));
    }

    fn meeting() -> Session {
        let mut session = Session::new(Kind::Call);
        session.title = "Café meeting 東京".into();
        session.rows.push(Row {
            cues: Vec::new(),
            start_ms: 1000,
            end_ms: 8000,
            microphone: false,
            speakers: vec![1],
            discord: Some(Attribution {
                generation: 987654,
                channel_id: "private-channel-fixture".into(),
                speakers: vec![NamedSpeaker {
                    avatar: None,
                    id: "private-user-fixture".into(),
                    name: "Élodie".into(),
                }],
            }),
            text: "We agreed to review the café plan. I'll send the résumé.".into(),
        });
        session.text = session.rows[0].text.clone();
        session.original = session.text.clone();
        session.speaker_names[0] = "Élodie".into();
        session.notes = Some(Notes::build(&session.rows));
        session
    }

    #[test]
    fn four_name_sessions_load_without_losing_speaker_identity() {
        let session = meeting();
        let mut saved = serde_json::to_value(&session).unwrap();
        saved["speaker_names"] = serde_json::json!(["Élodie", "", "", ""]);
        let restored: Session = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.speaker_names.len(), 4);
        assert_eq!(
            crate::calls::label(&restored.rows[0], &restored.speaker_names),
            "Élodie"
        );
        assert_eq!(
            crate::calls::label(
                &crate::calls::Row {
                    speakers: vec![8],
                    discord: None,
                    ..restored.rows[0].clone()
                },
                &restored.speaker_names
            ),
            "Speaker 8"
        );
    }

    #[test]
    fn call_audio_interruptions_persist_and_legacy_sessions_default_to_zero() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut interrupted = meeting();
        interrupted.audio_packets_lost = 3;
        history.save(interrupted.clone()).unwrap();
        let restored = history.load(&interrupted.id).unwrap();
        assert_eq!(restored.audio_packets_lost, 3);
        assert_eq!(restored.text, interrupted.text);
        assert_eq!(restored.original, interrupted.original);
        let clean = meeting();
        history.save(clean.clone()).unwrap();
        assert_eq!(history.load(&clean.id).unwrap().audio_packets_lost, 0);
        let mut legacy = serde_json::to_value(&interrupted).unwrap();
        legacy.as_object_mut().unwrap().remove("audio_packets_lost");
        assert_eq!(
            serde_json::from_value::<Session>(legacy)
                .unwrap()
                .audio_packets_lost,
            0
        );
    }

    #[test]
    fn call_title_ownership_roundtrips_and_legacy_excerpt_migrates() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = meeting();
        session.title = "Conversation".into();
        assert!(!history.save(session.clone()).unwrap().title_is_manual);
        session.title = "My planning meeting".into();
        session.title_is_manual = true;
        history.save(session.clone()).unwrap();
        assert!(history.load(&session.id).unwrap().title_is_manual);
        for (title, automatic) in [
            (session.rows[0].text.clone(), true),
            ("My planning meeting".into(), false),
        ] {
            let mut stored = serde_json::to_value(Stored {
                schema: 2,
                session: session.clone(),
            })
            .unwrap();
            stored["session"]["title"] = serde_json::json!(title);
            stored["session"]
                .as_object_mut()
                .unwrap()
                .remove("title_is_manual");
            let path = history.path(&session.id, "json").unwrap();
            fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
            let loaded = read_session(&path, &session.id).unwrap();
            assert_eq!(loaded.title_is_manual, !automatic);
            assert_eq!(loaded.title, title);
        }
        let mut fresh = Session::new(Kind::Call);
        fresh.text = "Just a greeting, not a topic.".into();
        fresh.title.clear();
        assert_eq!(history.save(fresh).unwrap().title, "Conversation");
    }

    #[test]
    fn unicode_transcript_and_notes_roundtrip_without_runtime_identifiers() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = history.save(meeting()).unwrap();
        let saved = fs::read_to_string(history.path(&session.id, "json").unwrap()).unwrap();
        assert!(!saved.contains("private-channel-fixture"));
        assert!(!saved.contains("private-user-fixture"));
        assert!(!saved.contains("987654"));
        assert!(!saved.contains("channel_id"));
        let loaded = history.load(&session.id).unwrap();
        assert_eq!(loaded.text, session.text);
        assert_eq!(loaded.title, "Café meeting 東京");
        assert_eq!(
            loaded.rows[0].discord.as_ref().unwrap().speakers[0].name,
            "Élodie"
        );
        assert!(
            loaded.rows[0].discord.as_ref().unwrap().speakers[0]
                .id
                .is_empty()
        );
        assert!(loaded.notes.as_ref().unwrap().is_current(&loaded.rows));
        assert_eq!(
            loaded.notes.unwrap().text(&loaded.speaker_names),
            session.notes.unwrap().text(&session.speaker_names)
        );
        assert_eq!(history.list().unwrap().len(), 1);
    }

    #[test]
    fn same_id_updates_in_place_and_preserves_creation_date() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = history.save(meeting()).unwrap();
        let created = session.created_ms;
        let previous = session.updated_ms;
        session.text = "Corrected transcript.".into();
        session.created_ms = 42;
        let session = history.save(session).unwrap();
        assert_eq!(session.created_ms, created);
        assert!(session.updated_ms > previous);
        assert_eq!(
            history.load(&session.id).unwrap().text,
            "Corrected transcript."
        );
        assert_eq!(history.list().unwrap().len(), 1);
    }

    #[test]
    fn interrupted_replacement_recovers_complete_temporary_then_backup() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = history.save(meeting()).unwrap();
        let main = history.path(&session.id, "json").unwrap();
        let temporary = history.path(&session.id, "tmp").unwrap();
        let backup = history.path(&session.id, "bak").unwrap();
        fs::rename(&main, &backup).unwrap();
        session.text = "Completed replacement.".into();
        fs::write(
            &temporary,
            serde_json::to_vec(&Stored {
                schema: SCHEMA,
                session: session.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            history.load(&session.id).unwrap().text,
            "Completed replacement."
        );
        assert!(!backup.exists());
        fs::rename(&main, &backup).unwrap();
        fs::write(&temporary, b"{incomplete").unwrap();
        assert_eq!(
            history.load(&session.id).unwrap().text,
            "Completed replacement."
        );
        assert!(!temporary.exists());
    }

    #[test]
    fn an_uncommitted_temporary_does_not_replace_valid_main() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = history.save(meeting()).unwrap();
        let original = session.text.clone();
        session.text = "Not committed.".into();
        fs::write(
            history.path(&session.id, "tmp").unwrap(),
            serde_json::to_vec(&Stored {
                schema: SCHEMA,
                session: session.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(history.load(&session.id).unwrap().text, original);
    }

    #[test]
    fn damaged_item_is_isolated_without_blocking_valid_sessions() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = history.save(meeting()).unwrap();
        let bad_id = new_id();
        fs::write(history.path(&bad_id, "json").unwrap(), b"invalid-json").unwrap();
        let list = history.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, session.id);
        assert!(!history.path(&bad_id, "json").unwrap().exists());
        assert!(fs::read_dir(&directory.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{bad_id}.corrupt-"))
        }));
    }

    #[test]
    fn deletion_survives_stale_saves_and_interrupted_cleanup() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = history.save(meeting()).unwrap();
        let bytes = fs::read(history.path(&session.id, "json").unwrap()).unwrap();
        history.delete(&session.id).unwrap();
        assert!(history.save(session.clone()).is_err());
        fs::write(history.path(&session.id, "bak").unwrap(), bytes).unwrap();
        assert!(history.list().unwrap().is_empty());
        assert!(!history.path(&session.id, "bak").unwrap().exists());
        assert!(history.load(&session.id).is_err());
        history.delete(&session.id).unwrap();
    }

    #[test]
    fn traversal_and_future_schema_are_rejected_without_overwrite() {
        let directory = TestDirectory::new();
        let history = directory.history();
        for id in ["../outside", "..\\outside", "C:\\outside", "a/b", "", "123"] {
            assert!(history.load(id).is_err());
            assert!(history.delete(id).is_err());
        }
        let session = meeting();
        let path = history.path(&session.id, "json").unwrap();
        let future = serde_json::to_vec(&Stored {
            schema: SCHEMA + 1,
            session: session.clone(),
        })
        .unwrap();
        fs::write(&path, &future).unwrap();
        assert!(history.load(&session.id).is_err());
        assert!(history.save(session).is_err());
        assert_eq!(fs::read(&path).unwrap(), future);
    }

    #[cfg(windows)]
    #[test]
    fn locked_records_are_skipped_and_retried_never_quarantined() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = history.save(meeting()).unwrap();
        let path = history.path(&session.id, "json").unwrap();
        let reopened = directory.history();
        // No sharing: every other open fails, as with antivirus or backup tools.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let listing = reopened.listing().unwrap();
        assert!(listing.items.is_empty());
        assert_eq!(listing.warnings.len(), 1);
        assert_eq!(listing.warnings[0].kind, WarningKind::Unavailable);
        assert_eq!(listing.quarantined, 0);
        assert!(reopened.load(&session.id).is_err());
        drop(lock);
        assert!(path.exists());
        let listing = reopened.listing().unwrap();
        assert_eq!(listing.items.len(), 1);
        assert!(listing.warnings.is_empty());
        assert_eq!(reopened.load(&session.id).unwrap().text, session.text);
    }

    #[test]
    fn damaged_records_are_reported_listed_exported_and_discarded() {
        let directory = TestDirectory::new();
        let history = directory.history();
        history.save(meeting()).unwrap();
        let bad_id = new_id();
        fs::write(
            history.path(&bad_id, "json").unwrap(),
            b"{\"schema\": 5, trunc",
        )
        .unwrap();
        let listing = history.listing().unwrap();
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.warnings.len(), 1);
        assert_eq!(listing.warnings[0].id, bad_id);
        assert_eq!(listing.warnings[0].kind, WarningKind::Damaged);
        assert_eq!(listing.quarantined, 1);
        // The next pass no longer warns, but the isolated file stays counted.
        let listing = history.listing().unwrap();
        assert!(listing.warnings.is_empty());
        assert_eq!(listing.quarantined, 1);
        let files = history.quarantined().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, bad_id);
        assert!(!files[0].restorable);
        assert!(history.restore_quarantined(&files[0].file_name).is_err());
        let exported = directory.0.join("exported.json");
        history
            .export_quarantined(&files[0].file_name, &exported)
            .unwrap();
        assert_eq!(fs::read(&exported).unwrap(), b"{\"schema\": 5, trunc");
        for invalid in ["../x", "settings.json", &format!("{bad_id}.json")] {
            assert!(history.discard_quarantined(invalid).is_err());
        }
        history.discard_quarantined(&files[0].file_name).unwrap();
        assert!(history.quarantined().unwrap().is_empty());
        assert_eq!(history.listing().unwrap().quarantined, 0);
    }

    #[test]
    fn complete_quarantined_records_restore_without_replacing_newer_copies() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = meeting();
        session.updated_ms = 1000;
        let isolated = format!("{}.corrupt-{}", session.id, new_id());
        fs::write(
            directory.0.join(&isolated),
            serde_json::to_vec(&Stored {
                schema: SCHEMA,
                session: session.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let mut newer = session.clone();
        newer.text = "A newer saved version.".into();
        let newer = history.save(newer).unwrap();
        let files = history.quarantined().unwrap();
        assert!(files[0].restorable);
        assert_eq!(files[0].title.as_deref(), Some("Café meeting 東京"));
        assert!(history.restore_quarantined(&isolated).is_err());
        assert_eq!(history.load(&newer.id).unwrap().text, newer.text);
        history.delete(&newer.id).unwrap();
        let other = meeting();
        let isolated = format!("{}.corrupt-{}", other.id, new_id());
        fs::write(
            directory.0.join(&isolated),
            serde_json::to_vec(&Stored {
                schema: SCHEMA,
                session: other.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let restored = history.restore_quarantined(&isolated).unwrap();
        assert_eq!(restored.id, other.id);
        assert_eq!(history.list().unwrap()[0].id, other.id);
        assert!(!directory.0.join(&isolated).exists());
    }

    /// Schema compatibility policy (see `SCHEMA`): unknown fields from newer
    /// versions are ignored on read and top-level session fields survive a
    /// save; a newer schema number is reported and never touched.
    #[test]
    fn newer_optional_fields_are_tolerated_and_preserved_newer_schemas_untouched() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = meeting();
        let path = history.path(&session.id, "json").unwrap();
        let mut stored = serde_json::to_value(Stored {
            schema: SCHEMA,
            session: session.clone(),
        })
        .unwrap();
        stored["written_by"] = serde_json::json!("a later version");
        stored["session"]["future_field"] = serde_json::json!({"nested": [1, 2]});
        stored["session"]["metrics"]["future_metric"] = serde_json::json!(7);
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        let mut loaded = history.load(&session.id).unwrap();
        assert_eq!(loaded.text, session.text);
        assert_eq!(loaded.unknown["future_field"]["nested"][1], 2);
        loaded.title = "Edited by this version".into();
        history.save(loaded).unwrap();
        let disk: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(disk["session"]["future_field"]["nested"][0], 1);
        assert_eq!(disk["session"]["title"], "Edited by this version");
        assert!(disk["session"].get("unknown").is_none());

        let future = meeting();
        let future_path = history.path(&future.id, "json").unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": SCHEMA + 1,
            "session": {"id": future.id, "shape": "different"},
        }))
        .unwrap();
        fs::write(&future_path, &bytes).unwrap();
        let listing = history.listing().unwrap();
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.warnings[0].kind, WarningKind::NewerVersion);
        assert_eq!(listing.quarantined, 0);
        assert_eq!(fs::read(&future_path).unwrap(), bytes);
    }

    #[test]
    fn tombstones_are_cleaned_in_one_pass_and_old_ones_compacted() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let kept = history.save(meeting()).unwrap();
        let mut deleted = Vec::new();
        for _ in 0..50 {
            let session = history.save(meeting()).unwrap();
            history.delete(&session.id).unwrap();
            deleted.push(session.id);
        }
        // An interrupted cleanup left a copy and a quarantined file behind.
        fs::write(history.path(&deleted[0], "bak").unwrap(), b"stale").unwrap();
        fs::write(
            directory
                .0
                .join(format!("{}.corrupt-{}", deleted[0], new_id())),
            b"stale",
        )
        .unwrap();
        let old = SystemTime::now() - TOMBSTONE_RETENTION - Duration::from_secs(60);
        for id in &deleted[..10] {
            File::options()
                .write(true)
                .open(history.path(id, "deleted").unwrap())
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let listing = history.listing().unwrap();
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.items[0].id, kept.id);
        assert_eq!(listing.quarantined, 0);
        let remaining: Vec<String> = fs::read_dir(&directory.0)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), 1 + 40);
        assert!(
            remaining
                .iter()
                .filter(|name| name.ends_with(".deleted"))
                .count()
                == 40
        );
        for id in &deleted[..10] {
            assert!(!history.path(id, "deleted").unwrap().exists());
        }
    }

    #[test]
    fn summaries_are_cached_until_the_file_changes() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = history.save(meeting()).unwrap();
        assert_eq!(history.list().unwrap()[0].title, "Café meeting 東京");
        // A change by another process is noticed through size and time.
        session.title = "Changed elsewhere with a longer title".into();
        let path = history.path(&session.id, "json").unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&Stored {
                schema: SCHEMA,
                session: session.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(history.list().unwrap()[0].title, session.title);
        // Removal by another process is noticed too.
        fs::remove_file(&path).unwrap();
        assert!(history.list().unwrap().is_empty());
    }

    #[test]
    fn worker_reports_saved_summaries_without_listing_again() {
        let directory = TestDirectory::new();
        let worker = Worker::test_directory(directory.0.clone());
        let mut session = meeting();
        session.title = "A saved summary".into();
        worker.save(session.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut listed = 0;
        loop {
            let mut saved = None;
            for event in worker.drain() {
                match event {
                    Event::Listed(_) => listed += 1,
                    Event::Saved { summary, .. } => saved = Some(summary),
                    Event::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
            if let Some(summary) = saved {
                assert_eq!(summary.id, session.id);
                assert_eq!(summary.title, "A saved summary");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        // Only the startup listing ran.
        assert!(listed <= 1);
    }

    #[test]
    fn index_search_follows_saves_deletes_and_restart() {
        let directory = TestDirectory::new();
        let index = SearchIndex::in_memory().unwrap();
        let history = directory.history().with_search(index.clone());
        let mut session = meeting();
        session.personal_notes = "Remember the zephyr budget.".into();
        let session = history.save(session).unwrap();
        let filters = search_index::Filters::default();
        assert_eq!(index.search("zephyr", &filters).unwrap().hits.len(), 1);
        history.delete(&session.id).unwrap();
        assert!(index.search("zephyr", &filters).unwrap().hits.is_empty());
        let other = history.save(meeting()).unwrap();
        // A fresh index is rebuilt from disk by the first listing.
        let rebuilt = SearchIndex::in_memory().unwrap();
        let reopened = directory.history().with_search(rebuilt.clone());
        assert!(!rebuilt.is_ready());
        reopened.listing().unwrap();
        assert!(rebuilt.is_ready());
        let found = rebuilt.search("résumé", &filters).unwrap();
        assert!(found.hits.iter().all(|hit| hit.session_id == other.id));
        assert!(!found.hits.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn encryption_migrates_both_ways_without_plaintext_on_disk() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = meeting();
        session.personal_notes = "synthetic-private-note".into();
        let session = history.save(session).unwrap();
        let bad = new_id();
        fs::write(
            history.path(&bad, "json").unwrap(),
            b"synthetic-private-damage",
        )
        .unwrap();
        history.listing().unwrap();
        let contains_secret = |directory: &Path| {
            fs::read_dir(directory).unwrap().flatten().any(|entry| {
                let bytes = fs::read(entry.path()).unwrap();
                String::from_utf8_lossy(&bytes).contains("synthetic-private")
            })
        };
        assert!(contains_secret(&directory.0));
        let report = history.set_protection(true).unwrap();
        assert_eq!((report.changed, report.failed), (2, 0));
        assert!(!contains_secret(&directory.0));
        assert_eq!(
            history.load(&session.id).unwrap().personal_notes,
            "synthetic-private-note"
        );
        // New saves stay protected, and a fresh store reads them.
        let mut edited = history.load(&session.id).unwrap();
        edited.personal_notes = "synthetic-private-edit".into();
        history.save(edited).unwrap();
        assert!(!contains_secret(&directory.0));
        let reopened = directory.history();
        assert_eq!(reopened.list().unwrap().len(), 1);
        assert_eq!(reopened.set_protection(true).unwrap().changed, 0);
        let files = reopened.quarantined().unwrap();
        let exported = directory.0.join("damage.txt");
        reopened
            .export_quarantined(&files[0].file_name, &exported)
            .unwrap();
        assert_eq!(fs::read(&exported).unwrap(), b"synthetic-private-damage");
        fs::remove_file(&exported).unwrap();
        reopened.set_protection(false).unwrap();
        let disk = fs::read_to_string(history.path(&session.id, "json").unwrap()).unwrap();
        assert!(disk.contains("synthetic-private-edit"));
    }

    #[test]
    fn retention_previews_then_deletes_only_eligible_unpinned_sessions() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let old = now_ms() - 40 * 24 * 60 * 60 * 1000;
        let install = |kind, pinned| {
            let mut session = Session::new(kind);
            session.text = "Synthetic retained words.".into();
            session.created_ms = old;
            session.updated_ms = old;
            session.pinned = pinned;
            history.install(&session).unwrap();
            session.id
        };
        let dictation = install(Kind::Dictation, false);
        let pinned = install(Kind::Dictation, true);
        let call = install(Kind::Call, false);
        let active = install(Kind::Note, false);
        let recent = history.save(meeting()).unwrap().id;
        let policy = retention::Policy {
            enabled: true,
            days: 30,
            calls: false,
            ..Default::default()
        };
        let preview = history
            .retention_preview(&policy, std::slice::from_ref(&active))
            .unwrap();
        assert_eq!(
            preview.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            std::slice::from_ref(&dictation)
        );
        assert_eq!(history.list().unwrap().len(), 5);
        // A stale editor copy cannot unpin a pinned session.
        let mut stale = history.load(&pinned).unwrap();
        stale.pinned = false;
        history.save(stale).unwrap();
        assert!(history.load(&pinned).unwrap().pinned);
        let applied = history
            .apply_retention(&policy, Some(&[dictation.clone(), recent.clone()]), &[])
            .unwrap();
        assert_eq!(applied.deleted, std::slice::from_ref(&dictation));
        assert_eq!(applied.skipped, std::slice::from_ref(&recent));
        assert!(history.load(&dictation).is_err());
        for id in [&pinned, &call, &active, &recent] {
            assert!(history.load(id).is_ok());
        }
        // Pinning is not an edit: it leaves the last-changed time alone.
        let before = history.load(&pinned).unwrap().updated_ms;
        history.set_pinned(&pinned, false).unwrap();
        assert_eq!(history.load(&pinned).unwrap().updated_ms, before);
        assert!(!history.load(&pinned).unwrap().pinned);
        let preview = history.retention_preview(&policy, &[]).unwrap();
        assert!(preview.iter().all(|s| s.id != call));
    }

    #[test]
    fn leftover_call_audio_is_removed_but_current_and_unrelated_files_kept() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let other = directory.0.join("99999999-3.wav");
        let current = directory.0.join(format!("{}-1.wav", std::process::id()));
        let unrelated = directory.0.join("notes.wav");
        for path in [&other, &current, &unrelated] {
            fs::write(path, b"RIFF").unwrap();
        }
        assert_eq!(remove_leftover_call_audio(&directory.0), 1);
        assert!(!other.exists() && current.exists() && unrelated.exists());
        assert_eq!(remove_leftover_call_audio(&directory.0.join("missing")), 0);
    }

    #[test]
    fn session_ids_are_safe_and_unique() {
        let ids: BTreeSet<_> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|id| valid_id(id)));
    }

    #[test]
    fn worker_drains_accepted_saves_when_dropped() {
        let directory = TestDirectory::new();
        let session = meeting();
        let worker = Worker::start_directory(directory.0.clone());
        worker.save(session.clone());
        drop(worker);
        assert_eq!(
            directory.history().load(&session.id).unwrap().text,
            session.text
        );
    }

    #[test]
    fn settled_saves_are_already_on_disk() {
        let directory = TestDirectory::new();
        let session = meeting();
        let worker = Worker::start_directory(directory.0.clone());
        worker.save(session.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !worker.saves_settled().unwrap() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            directory.history().load(&session.id).unwrap().text,
            session.text
        );
    }

    #[test]
    fn settled_saves_report_write_failures_and_recover_after_retry() {
        let directory = TestDirectory::new();
        fs::write(&directory.0, b"synthetic obstructing file").unwrap();
        let worker = Worker::start_directory(directory.0.clone());
        let session = meeting();
        worker.save(session.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match worker.saves_settled() {
                Err(_) => break,
                Ok(false) => {}
                Ok(true) => panic!("A failed save must not permit shutdown"),
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        fs::remove_file(&directory.0).unwrap();
        worker.retry();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while worker.saves_settled() != Ok(true) {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            directory.history().load(&session.id).unwrap().text,
            session.text
        );
    }

    #[test]
    fn queued_delete_then_stale_save_never_resurrects() {
        let directory = TestDirectory::new();
        let session = meeting();
        let worker = Worker::start_directory(directory.0.clone());
        worker.save(session.clone());
        worker.delete(session.id.clone());
        worker.save(session.clone());
        drop(worker);
        let history = directory.history();
        assert!(history.list().unwrap().is_empty());
        assert!(history.load(&session.id).is_err());
        assert!(history.path(&session.id, "deleted").unwrap().exists());
    }

    #[test]
    fn retry_reopens_history_after_initial_filesystem_failure() {
        let directory = TestDirectory::new();
        fs::write(&directory.0, b"synthetic obstructing file").unwrap();
        let worker = Worker::start_directory(directory.0.clone());
        let session = meeting();
        worker.save(session.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if worker
                .shared
                .state
                .lock()
                .unwrap()
                .failed_saves
                .contains_key(&session.id)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Expected a retryable failed save"
            );
            thread::sleep(std::time::Duration::from_millis(5));
        }
        fs::remove_file(&directory.0).unwrap();
        worker.retry();
        drop(worker);
        assert_eq!(
            directory.history().load(&session.id).unwrap().text,
            session.text
        );
    }
    #[test]
    fn personal_note_survives_worker_shutdown_and_restart() {
        let directory = TestDirectory::new();
        let mut session = Session::new(Kind::Note);
        session.title = "Ideas for Thursday".into();
        session.personal_notes = "Question for Casey\nKeep the first step short.".into();
        let worker = Worker::start_directory(directory.0.clone());
        worker.save(session.clone());
        drop(worker);
        let reopened = directory.history().load(&session.id).unwrap();
        assert_eq!(reopened.kind, Kind::Note);
        assert_eq!(reopened.personal_notes, session.personal_notes);
        assert!(reopened.rows.is_empty());
        assert!(reopened.notes.is_none());
        assert!(
            Summary::from(&reopened)
                .preview
                .contains("Question for Casey")
        );
    }

    #[test]
    fn old_history_without_personal_notes_remains_readable() {
        let directory = TestDirectory::new();
        let session = meeting();
        let history = directory.history();
        let mut stored = serde_json::to_value(Stored {
            schema: SCHEMA,
            session: session.clone(),
        })
        .unwrap();
        stored["schema"] = serde_json::json!(1);
        for field in [
            "personal_notes",
            "metrics",
            "generated_summary",
            "title_is_manual",
            "protected_note_items",
        ] {
            stored["session"].as_object_mut().unwrap().remove(field);
        }
        fs::write(
            history.path(&session.id, "json").unwrap(),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let reopened = history.load(&session.id).unwrap();
        assert!(reopened.personal_notes.is_empty());
        assert_eq!(reopened.rows[0].text, session.rows[0].text);
    }
    #[test]
    fn insights_counts_retained_ids_once_and_removes_deleted_sessions() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = Session::new(Kind::Dictation);
        session.text = "These are saved words".into();
        session.metrics = crate::insights::DictationMetrics {
            recognized_words: Some(4),
            audio_duration_ms: Some(2000),
            app: Some("editor.exe".into()),
            dictionary_replacements: Some(2),
            cleanup_edits: Some(0),
        };
        history.save(session.clone()).unwrap();
        session.text = "Manually edited to a much longer text afterwards".into();
        history.save(session.clone()).unwrap();
        let report = history.insights().unwrap();
        assert_eq!((report.sessions, report.words), (1, 4));
        assert_eq!(report.words_per_minute, Some(120.0));
        assert_eq!(report.dictionary_replacements, 2);
        assert_eq!(report.correction_sessions, 1);
        assert_eq!(report.apps[0].app.as_deref(), Some("editor.exe"));
        history.delete(&session.id).unwrap();
        assert_eq!(history.insights().unwrap().sessions, 0);
    }

    #[test]
    fn legacy_metrics_remain_unknown_and_aggregate_off_worker() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = Session::new(Kind::Dictation);
        session.text = "Three saved words".into();
        let mut stored = serde_json::to_value(Stored {
            schema: SCHEMA,
            session: session.clone(),
        })
        .unwrap();
        stored["session"].as_object_mut().unwrap().remove("metrics");
        fs::write(
            history.path(&session.id, "json").unwrap(),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let loaded = history.load(&session.id).unwrap();
        assert!(loaded.metrics.audio_duration_ms.is_none());
        let worker = Worker::start_directory(directory.0.clone());
        worker.insights();
        let start = std::time::Instant::now();
        loop {
            for event in worker.drain() {
                match event {
                    Event::Insights(report) => {
                        assert_eq!((report.sessions, report.words), (1, 3));
                        assert!(report.words_per_minute.is_none());
                        assert_eq!(report.correction_sessions, 0);
                        assert!(report.apps[0].app.is_none());
                        return;
                    }
                    Event::InsightsFailed(error) => panic!("{error}"),
                    _ => {}
                }
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Calls gained optional fields (speaker corrections, interruptions,
    /// retained audio, earlier transcripts) without changing `SCHEMA`.
    #[test]
    fn call_fields_are_optional_and_round_trip_without_a_schema_change() {
        use crate::calls::interruptions::{Cause, Source};
        let directory = TestDirectory::new();
        let history = directory.history();
        let session = meeting();
        let stored = serde_json::to_value(Stored {
            schema: SCHEMA,
            session: session.clone(),
        })
        .unwrap();
        for field in [
            "speaker_edits",
            "interruptions",
            "audio",
            "transcript_versions",
        ] {
            assert!(stored["session"].get(field).is_none(), "{field} is omitted");
        }
        let path = history.path(&session.id, "json").unwrap();
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        let mut loaded = history.load(&session.id).unwrap();
        assert!(loaded.speaker_edits.is_empty() && loaded.audio.is_none());
        assert!(loaded.interruptions.is_empty() && loaded.transcript_versions.is_empty());
        let mut rows = loaded.rows.clone();
        let mut names = loaded.speaker_names.clone();
        let key = crate::speaker_edits::roster(&rows, &names)[0].key.clone();
        let edit = crate::speaker_edits::rename(&mut rows, &mut names, &key, "Morgan").unwrap();
        loaded.rows = rows;
        loaded.speaker_names = names;
        loaded.speaker_edits.push(edit);
        loaded.interruptions.push(crate::calls::Interruption::new(
            1000,
            2000,
            Source::Microphone,
            Cause::DeviceLost,
        ));
        loaded.audio = Some(crate::audio_store::Info {
            format: crate::audio_store::FORMAT.into(),
            duration_ms: 3000,
            bytes: 10,
            channels: vec!["microphone".into(), "call".into()],
            incomplete: false,
        });
        loaded
            .transcript_versions
            .push(TranscriptVersion::of(&session, "model.gguf"));
        history.save(loaded.clone()).unwrap();
        let disk: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(disk["schema"], SCHEMA);
        let text = disk.to_string();
        assert!(
            !text.contains("private-user-fixture") && !text.contains("private-channel-fixture")
        );
        let reloaded = history.load(&session.id).unwrap();
        assert_eq!(
            reloaded.speaker_edits[0].summary,
            "Renamed Élodie to Morgan"
        );
        assert_eq!(
            crate::calls::label(&reloaded.rows[0], &reloaded.speaker_names),
            "Morgan"
        );
        assert_eq!(reloaded.interruptions, loaded.interruptions);
        assert_eq!(reloaded.audio, loaded.audio);
        assert_eq!(reloaded.transcript_versions[0].text, session.text);
        assert_eq!(reloaded.transcript_versions[0].model, "model.gguf");
        // Undo after reopening restores the participant name.
        let mut rows = reloaded.rows.clone();
        let mut names = reloaded.speaker_names.clone();
        crate::speaker_edits::undo(&mut rows, &mut names, &reloaded.speaker_edits[0]);
        assert_eq!(crate::calls::label(&rows[0], &names), "Élodie");
    }

    #[test]
    fn retained_audio_is_sealed_with_history_and_removed_by_retention() {
        let directory = TestDirectory::new();
        let history = directory.history();
        let mut session = meeting();
        session.created_ms = 1;
        session.updated_ms = 1;
        let audio = crate::audio_store::path(&directory.0, &session.id);
        let recorder =
            crate::audio_store::Recorder::start(audio.clone(), vec!["recording".into()], false)
                .unwrap();
        recorder.push(0, vec![0.1; 16_000]);
        session.audio = Some(recorder.finish().unwrap());
        history.install(&session).unwrap();
        let plain = fs::read(&audio).unwrap();
        let report = history.set_protection(true).unwrap();
        assert!(report.changed >= 2, "record and audio: {report:?}");
        let sealed = fs::read(&audio).unwrap();
        assert_ne!(plain, sealed);
        assert!(protect::is_sealed(&sealed[23..]));
        assert_eq!(
            crate::audio_store::read_all(&audio).unwrap()[0].len(),
            16_000
        );
        history.set_protection(false).unwrap();
        assert_eq!(fs::read(&audio).unwrap(), plain);
        let policy = retention::Policy {
            enabled: true,
            days: 30,
            ..Default::default()
        };
        let applied = history.apply_retention(&policy, None, &[]).unwrap();
        assert_eq!(applied.deleted, std::slice::from_ref(&session.id));
        assert!(!audio.exists(), "retention removes retained audio");
        // A recording interrupted before it finished is also removed with its session.
        let other = history.save(meeting()).unwrap();
        let partial = directory.0.join(format!("{}.audio.recording", other.id));
        fs::write(&partial, b"partial").unwrap();
        history.delete(&other.id).unwrap();
        assert!(!partial.exists());
    }
}
