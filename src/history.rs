//! Local text history. Audio is never part of the saved format.
use crate::{calls::Row, notes::Notes};
use anyhow::{Context, Result, bail, ensure};
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

const SCHEMA: u32 = 2;
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
    pub kind: Kind,
    pub text: String,
    pub original: String,
    pub rows: Vec<Row>,
    pub speaker_names: [String; 4],
    pub notes: Option<Notes>,
    /// Personal writing is independent of source-quoted highlights.
    #[serde(default)]
    pub personal_notes: String,
    #[serde(default)]
    pub metrics: crate::insights::DictationMetrics,
    #[serde(default)]
    pub generated_summary: Option<crate::brain::Draft>,
}

impl Session {
    pub fn new(kind: Kind) -> Self {
        let now = now_ms();
        Self {
            id: new_id(),
            created_ms: now,
            updated_ms: now,
            title: String::new(),
            kind,
            text: String::new(),
            original: String::new(),
            rows: Vec::new(),
            speaker_names: Default::default(),
            notes: None,
            personal_notes: String::new(),
            metrics: Default::default(),
            generated_summary: None,
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

    pub fn list(&self) -> Result<Vec<Summary>> {
        let ids = self.ids()?;
        // A damaged individual item cannot prevent other recordings from opening.
        let mut summaries: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                self.recover(&id)
                    .ok()
                    .flatten()
                    .map(|session| Summary::from(&session))
            })
            .collect();
        summaries.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(summaries)
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        self.recover(id)?
            .context("This history item is no longer available")
    }

    /// Read only the committed file. Independent search workers must never
    /// recover, remove, or quarantine another worker's in-flight save files.
    pub(crate) fn snapshot(&self, id: &str) -> Result<Session> {
        let deleted = self.path(id, "deleted")?;
        ensure!(!deleted.exists(), "This history item was deleted");
        let session = read_session(&self.path(id, "json")?, id)?;
        ensure!(!deleted.exists(), "This history item was deleted");
        Ok(session)
    }

    pub fn save(&self, mut session: Session) -> Result<Session> {
        validate(&session)?;
        ensure!(
            !self.path(&session.id, "deleted")?.exists(),
            "This history item was deleted"
        );
        if let Some(existing) = self.recover(&session.id)? {
            session.created_ms = existing.created_ms;
            session.updated_ms = now_ms()
                .max(existing.updated_ms.saturating_add(1))
                .max(session.created_ms);
        } else {
            session.updated_ms = now_ms().max(session.created_ms);
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
        let bytes = serde_json::to_vec(&Stored {
            schema: SCHEMA,
            session: session.clone(),
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

    pub fn delete(&self, id: &str) -> Result<()> {
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
        bail!("A damaged history item was isolated; other items are available")
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

fn read_session(path: &Path, expected_id: &str) -> Result<Session> {
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
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value["schema"]
        .as_u64()
        .is_some_and(|schema| schema > SCHEMA as u64)
    {
        return Err(UnsupportedSchema.into());
    }
    let stored: Stored = serde_json::from_value(value)?;
    ensure!(
        (1..=SCHEMA).contains(&stored.schema) && stored.session.id == expected_id,
        "Invalid history schema or identity"
    );
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
        session.speaker_names.iter().all(|name| name.len() <= 512),
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
    Saved { id: String, updated_ms: u64 },
    Deleted { id: String },
    Failed(String),
}

enum Command {
    Insights,
    List,
    Load(String),
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
        let retry = match &command {
            Command::Save(session) => Some(session.clone()),
            _ => None,
        };
        let result = match &history {
            Ok(history) => match command {
                Command::Insights => history.insights().map(Event::Insights),
                Command::List => history.list().map(Event::Listed),
                Command::Load(id) => history
                    .load(&id)
                    .map(|session| Event::Loaded(Box::new(session))),
                Command::Save(pending) => {
                    history.save(*pending.session).map(|session| Event::Saved {
                        id: session.id,
                        updated_ms: session.updated_ms,
                    })
                }
                Command::Delete(id) => history.delete(&id).map(|()| Event::Deleted { id }),
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

    fn meeting() -> Session {
        let mut session = Session::new(Kind::Call);
        session.title = "Café meeting 東京".into();
        session.rows.push(Row {
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
        for field in ["personal_notes", "metrics", "generated_summary"] {
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
