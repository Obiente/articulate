//! Local text history. Audio is never part of the saved format.
use crate::{calls::Row, notes::Notes};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque, hash_map::RandomState},
    fs::{self, File, OpenOptions},
    hash::BuildHasher,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 5;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
const QUEUE_CAPACITY: usize = 32;
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
#[serde(deny_unknown_fields)]
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
    /// Rebuilt from member sessions when a topic is opened; never persisted.
    #[serde(skip)]
    pub sources: Vec<crate::topics::Source>,
}

fn manual_title_default() -> bool {
    true
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
            sources: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

pub struct History {
    directory: PathBuf,
}

impl History {
    pub fn open(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory).context("Could not create local history")?;
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "History must be a local directory"
        );
        Ok(Self { directory })
    }

    fn ids(&self) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            for suffix in [".json", ".bak", ".tmp", ".deleted"] {
                if let Some(id) = name.strip_suffix(suffix)
                    && valid_id(id)
                {
                    ids.insert(id.to_owned());
                }
            }
            ensure!(
                ids.len() <= 100_000,
                "Local history contains too many items"
            );
        }
        Ok(ids)
    }

    pub fn insights(&self) -> Result<crate::insights::Report> {
        let mut aggregate = crate::insights::Aggregate::new(now_ms());
        for id in self.ids()? {
            if let Some(session) = self.recover(&id).ok().flatten() {
                aggregate.push(&session);
            }
        }
        Ok(aggregate.finish())
    }

    #[cfg(test)]
    pub fn list(&self) -> Result<Vec<Summary>> {
        self.list_with_pending().map(|(summaries, _)| summaries)
    }

    fn list_with_pending(&self) -> Result<(Vec<Summary>, Vec<Session>)> {
        let ids = self.ids()?;
        let mut summaries = Vec::new();
        let mut pending = Vec::new();
        let mut pending_bytes = 0_usize;
        // A damaged individual item cannot prevent other recordings from opening.
        for id in ids {
            if let Some(session) = self.recover(&id).ok().flatten() {
                summaries.push(Summary::from(&session));
                if session.auto_file_pending
                    && !session.is_collection
                    && session.topic_id.is_none()
                    && !session.text.trim().is_empty()
                    && pending.len() < 32
                {
                    let bytes = session
                        .text
                        .len()
                        .saturating_add(session.original.len())
                        .saturating_add(
                            session.rows.iter().map(|row| row.text.len()).sum::<usize>(),
                        );
                    if pending_bytes.saturating_add(bytes) <= MAX_TEXT_BYTES {
                        pending_bytes += bytes;
                        pending.push(session);
                    }
                }
            }
        }
        summaries.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok((summaries, pending))
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
        let bytes = serde_json::to_vec(&Stored {
            schema: SCHEMA,
            session: stored_session,
        })?;
        ensure!(
            bytes.len() <= MAX_BYTES,
            "This transcript is too large to save as one history item"
        );
        let main = self.path(&session.id, "json")?;
        let temporary = self.path(&session.id, "tmp")?;
        let backup = self.path(&session.id, "bak")?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
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
        Ok(session)
    }

    fn collection(&self, session: Session) -> Result<Session> {
        let mut sources = Vec::new();
        let mut bytes = 0_usize;
        for id in self.ids()? {
            let recovered = match self.recover(&id) {
                Ok(source) => source,
                Err(error) if error.is::<UnsupportedSchema>() => {
                    // A newer unrelated recording must not disable every topic.
                    // Preserve the error whenever its ownership is unknown or
                    // points here, so saving cannot erase an unreadable member.
                    if !self.unreadable_record_is_unrelated(&id, &session.id) {
                        return Err(error);
                    }
                    continue;
                }
                // Recovery quarantines damaged records, just as the history
                // list does. They cannot prevent intact recordings opening.
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
        self.remove_copies(id)
    }

    fn remove_copies(&self, id: &str) -> Result<()> {
        for suffix in ["json", "bak", "tmp"] {
            remove_if_present(&self.path(id, suffix)?)?;
        }
        let prefix = format!("{id}.corrupt-");
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix(&prefix))
                .is_some_and(valid_id)
            {
                remove_if_present(&entry.path())?;
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
            self.remove_copies(id)?;
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
                Err(error) if error.is::<UnsupportedSchema>() => return Err(error),
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
        Ok(())
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn read_session_value(path: &Path) -> Result<serde_json::Value> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_BYTES as u64,
        "Invalid history file"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "History item exceeds size limit");
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_session(path: &Path, expected_id: &str) -> Result<Session> {
    let value = read_session_value(path)?;
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

fn now_ms() -> u64 {
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

pub enum Event {
    Insights(crate::insights::Report),
    InsightsFailed(String),
    Listed(Vec<Summary>),
    Loaded(Box<Session>),
    SourceMoved(Box<crate::topics::Moved>),
    SourceMoveFailed { id: String, error: String },
    CollectionsChanged(Vec<Session>),
    FilingPending(Vec<Session>),
    Saved { id: String, updated_ms: u64 },
    Deleted { id: String },
    DeleteFailed { id: String, error: String },
    Failed(String),
}

enum Command {
    Insights,
    List,
    Load(String),
    MoveSource(String, crate::topics::Destination, Option<String>),
    Save(PendingSave),
    Delete(String),
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
}

impl Worker {
    #[cfg(test)]
    pub(crate) fn test_directory(directory: PathBuf) -> Self {
        Self::start_directory(directory)
    }

    pub fn start() -> Self {
        Self::start_directory(crate::model::data_dir().join("history"))
    }

    fn start_directory(directory: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(WorkState {
                commands: VecDeque::from([Command::List]),
                events: VecDeque::new(),
                stopped: false,
                failed_saves: BTreeMap::new(),
                active_save: None,
                active_operation: false,
            }),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("local-history".into())
            .spawn(move || history_worker(worker_shared, directory));
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
        Self { shared, thread }
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
            let deleting = match &command {
                Command::Delete(id) => Some(id.clone()),
                _ => None,
            };
            let moving = match &command {
                Command::MoveSource(id, _, _) => Some(id.clone()),
                _ => None,
            };
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
            } else if let Some(id) = deleting {
                Event::DeleteFailed {
                    id,
                    error: "History is busy. Please try deleting again.".into(),
                }
            } else if let Some(id) = moving {
                Event::SourceMoveFailed {
                    id,
                    error: "History is busy. Please try moving again.".into(),
                }
            } else {
                Event::Failed("History is busy. Please try saving again.".into())
            });
        } else {
            state.commands.push_back(command);
            self.shared.wake.notify_one();
        }
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

fn history_worker(shared: Arc<Shared>, directory: PathBuf) {
    let mut history = History::open(directory.clone());
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
        if history.is_err() {
            history = History::open(directory.clone());
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
        let mut refreshed_collections = Vec::new();
        let mut pending_filing = None;
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
                        id: session.id,
                        updated_ms: session.updated_ms,
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
        let event = result.unwrap_or_else(|error| {
            if insights {
                Event::InsightsFailed(error.to_string())
            } else if let Some(id) = deleting {
                Event::DeleteFailed {
                    id,
                    error: error.to_string(),
                }
            } else if let Some(id) = moving {
                Event::SourceMoveFailed {
                    id,
                    error: error.to_string(),
                }
            } else {
                Event::Failed(error.to_string())
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
        if state.events.len() >= 128 {
            state.events.pop_front();
        }
        state.events.push_back(event);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord_attribution::{Attribution, NamedSpeaker};

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
}
