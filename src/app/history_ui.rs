use super::*;
use crate::history::{
    Event as HistoryEvent, Kind, MigrationReport, Quarantined, RecordWarning, Session, Summary,
    Worker, backup,
};

/// Preview-only call changes (revisable text, cues, packet counts) save at most
/// this often. Newly committed rows save after `CALL_COMMIT_SAVE`. Finishing a
/// call or closing the app always flushes immediately.
const CALL_PREVIEW_SAVE: Duration = Duration::from_secs(4);
const CALL_COMMIT_SAVE: Duration = Duration::from_secs(1);
/// Insights are recomputed from cached samples, at most this often.
const INSIGHTS_REFRESH: Duration = Duration::from_secs(3);
const RETENTION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Identifies the committed call rows already handed to the history worker.
fn committed_fingerprint(rows: &[calls::Row]) -> (usize, u64, usize) {
    rows.last()
        .map_or((0, 0, 0), |row| (rows.len(), row.end_ms, row.text.len()))
}

#[derive(Default)]
pub(super) struct State {
    pub worker: Option<Worker>,
    pub items: Vec<Summary>,
    pub selected: Option<Session>,
    pub selected_dirty: Option<Instant>,
    pub open_requested: Option<String>,

    pub notetaker_retry: bool,
    pub notetaker_tab: usize,
    pub dictation: Option<Session>,
    pub call: Option<Session>,
    pub dictation_dirty: Option<Instant>,
    pub call_dirty: Option<Instant>,
    pub dictation_deleted: bool,
    pub call_deleted: bool,
    pub pending_deletions: std::collections::HashSet<String>,

    pub detail_search: String,
    pub error: Option<String>,
    pub notice: String,
    pub rename: String,
    pub renaming: bool,
    pub confirm_delete: bool,
    pub detail_tab: usize,
    pub loading: bool,

    /// Records skipped by the latest history pass (damaged, locked, newer).
    pub warnings: Vec<RecordWarning>,
    /// Isolated files kept beside the history. Review them with `quarantine`.
    pub quarantined: usize,
    pub quarantine: Vec<Quarantined>,
    pub retention_preview: Option<Result<Vec<Summary>, String>>,
    pub protection: Option<Result<MigrationReport, String>>,
    pub backup: Option<Result<backup::Created, String>>,
    pub restored: Option<Result<Box<backup::Restored>, String>>,
    pub markdown_export: Option<Result<backup::Exported, String>>,
    /// Progress of storage operations, file dialogs and library notices.
    pub storage: Storage,
    call_saved_committed: (usize, u64, usize),
    insights_requested: Option<Instant>,
    retention_applied: Option<Instant>,
}

/// Work to finish on the controller after a file dialog closes.
pub(super) type Continuation = Box<dyn FnOnce(&mut App) + Send>;

/// The stage of an operation that asks for a file first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Progress {
    #[default]
    Idle,
    /// A file or folder dialog is open.
    Choosing,
    /// The history worker is writing or reading.
    Working,
}

impl Progress {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Choosing => "choosing",
            Self::Working => "working",
        }
    }
}

/// A one-time message for the library, such as a finished export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notice {
    /// Increases with every notice, so the renderer shows each one once.
    pub sequence: u64,
    pub error: bool,
    pub message: String,
}

/// What a restore changed in this app's preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoredSettings {
    /// Shortcuts changed and were registered again.
    pub shortcuts: bool,
    /// The speech model's language or processor changed; reload it to apply.
    pub model_reload: bool,
}

/// File dialogs, replaceable in tests so no dialog ever opens there.
#[derive(Clone, Copy)]
pub(super) struct Pickers {
    pub save_text:
        fn(&str, &str, crate::export_file::Document, Option<&str>) -> anyhow::Result<bool>,
    pub save_path: fn(crate::export_file::Document, &str, &str) -> anyhow::Result<Option<PathBuf>>,
    pub backup_destination: fn(&str) -> anyhow::Result<Option<PathBuf>>,
    pub backup_source: fn() -> anyhow::Result<Option<PathBuf>>,
    pub folder: fn(&str) -> anyhow::Result<Option<PathBuf>>,
}

impl Pickers {
    /// The Windows file dialogs.
    #[cfg_attr(test, allow(dead_code, reason = "Tests never open real file dialogs"))]
    fn system() -> Self {
        use crate::export_file as files;
        Self {
            save_text: files::save_document,
            save_path: files::choose_save_path,
            backup_destination: files::choose_backup_destination,
            backup_source: files::choose_backup_source,
            folder: files::choose_folder,
        }
    }
}

impl Default for Pickers {
    #[cfg(not(test))]
    fn default() -> Self {
        Self::system()
    }
    /// Tests cancel every dialog unless they install their own picker.
    #[cfg(test)]
    fn default() -> Self {
        Self {
            save_text: |_, _, _, _| Ok(false),
            save_path: |_, _, _| Ok(None),
            backup_destination: |_| Ok(None),
            backup_source: || Ok(None),
            folder: |_| Ok(None),
        }
    }
}

#[derive(Default)]
pub(super) struct Storage {
    pub encryption_working: bool,
    pub retention_preview_working: bool,
    /// The policy `State::retention_preview` was computed for.
    pub retention_previewed: Option<crate::history::retention::Policy>,
    pub backup: Progress,
    pub restore: Progress,
    pub markdown: Progress,
    /// Set after a restore that included preferences.
    pub restored_settings: Option<RestoredSettings>,
    /// Whether the isolated files have been listed since the app started.
    pub quarantine_listed: bool,
    /// One file dialog at a time.
    pub dialog_open: bool,
    pub notice: Option<Notice>,
    pub pickers: Pickers,
    dialogs: Option<(mpsc::Sender<Continuation>, mpsc::Receiver<Continuation>)>,
}

impl State {
    pub fn start(protect: bool) -> Self {
        Self {
            worker: Some(Worker::start_protected(protect)),
            loading: true,
            ..Default::default()
        }
    }

    /// Insert or replace one summary without reloading the whole history.
    fn upsert(&mut self, summary: Summary) {
        if self.pending_deletions.contains(&summary.id) {
            return;
        }
        self.items.retain(|item| item.id != summary.id);
        let at = self
            .items
            .iter()
            .position(|item| {
                item.updated_ms < summary.updated_ms
                    || (item.updated_ms == summary.updated_ms && item.id > summary.id)
            })
            .unwrap_or(self.items.len());
        self.items.insert(at, summary);
    }

    fn deleted(&mut self, id: &str) {
        self.pending_deletions.remove(id);
        self.items.retain(|item| item.id != id);
        if self
            .selected
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            self.selected = None;
        }
        if self
            .dictation
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            self.dictation = None;
            self.dictation_deleted = true;
        }
        if self.call.as_ref().is_some_and(|session| session.id == id) {
            self.call = None;
            self.call_deleted = true;
        }
        self.confirm_delete = false;
        self.notice = "Deleted from this device".into();
    }
}

impl App {
    pub(super) fn history_delete(&mut self, id: &str) -> bool {
        if self.history.worker.is_none() {
            return false;
        }
        self.brain.forget(id);
        self.notetaker_forget_filing(id);
        self.history.pending_deletions.insert(id.into());
        self.history.worker.as_ref().unwrap().delete(id.into());
        true
    }

    #[allow(
        dead_code,
        reason = "Persistence operations retained for dictation editing and document updates"
    )]
    pub(super) fn history_dictation_changed(&mut self) {
        self.history
            .dictation_dirty
            .get_or_insert_with(Instant::now);
    }

    pub(super) fn history_call_changed(&mut self) {
        self.history.call_dirty.get_or_insert_with(Instant::now);
    }

    pub(super) fn history_save_dictation(&mut self) {
        self.history.dictation_dirty = None;
        if self.history.dictation_deleted
            || self
                .history
                .dictation
                .as_ref()
                .is_some_and(|s| self.history.pending_deletions.contains(&s.id))
        {
            return;
        }
        if self.text.trim().is_empty() && self.history.dictation.is_none() {
            return;
        }
        let session = self
            .history
            .dictation
            .get_or_insert_with(|| Session::new(Kind::Dictation));
        if session.title.is_empty() {
            session.title = title(&self.text, "Dictation");
        }
        session.text.clone_from(&self.text);
        session.original.clone_from(&self.raw);
        session.metrics.clone_from(&self.dictation_metrics);
        if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    pub(super) fn history_save_call(&mut self) {
        self.history.call_dirty = None;
        self.history.call_saved_committed = committed_fingerprint(&self.call_committed);
        if self.history.call_deleted
            || self
                .history
                .call
                .as_ref()
                .is_some_and(|s| self.history.pending_deletions.contains(&s.id))
        {
            return;
        }
        if self.call_rows.is_empty() && self.history.call.is_none() {
            return;
        }
        let session = self
            .history
            .call
            .get_or_insert_with(|| Session::new(Kind::Call));
        if session.title.is_empty() {
            session.title = "Conversation".into();
        }
        session.rows.clone_from(&self.call_rows);
        session.speaker_names.clone_from(&self.speaker_names);
        session.notes.clone_from(&self.call_notes);
        session.text = calls::text(&self.call_rows, &self.speaker_names);
        if session.kind == Kind::Note {
            // The evolving document is separate from what was actually spoken.
            session.original.clone_from(&session.text);
        }
        if let Some(selected) = &mut self.history.selected
            && selected.id == session.id
        {
            selected.clone_from(session);
        }
        if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    pub(super) fn history_poll(&mut self) {
        let events = self
            .history
            .worker
            .as_ref()
            .map(Worker::drain)
            .unwrap_or_default();
        for event in events {
            match event {
                HistoryEvent::SourceMoved(moved) => self.notetaker_source_moved(*moved),
                HistoryEvent::CollectionsChanged(collections) => {
                    self.notetaker_refresh_collections(collections)
                }
                HistoryEvent::FilingPending(sessions) => {
                    for session in sessions {
                        if self.call.is_some()
                            && self
                                .history
                                .call
                                .as_ref()
                                .is_some_and(|current| current.id == session.id)
                        {
                            continue;
                        }
                        self.notetaker_queue_filing(session);
                    }
                }
                HistoryEvent::SourceMoveFailed { id, error } => {
                    self.notetaker.moving = false;
                    self.notetaker_forget_filing(&id);
                    self.notetaker.status.clone_from(&error);
                    self.history.error = Some(error);
                }
                HistoryEvent::Insights(report) => {
                    self.insights.report = Some(report);
                    self.insights.loading = false;
                    self.insights.error = None;
                }
                HistoryEvent::InsightsFailed(error) => {
                    self.insights.error = Some(error);
                    self.insights.loading = false;
                }
                HistoryEvent::Listed(listing) => {
                    self.history.items = listing.items;
                    let pending = &self.history.pending_deletions;
                    self.history
                        .items
                        .retain(|item| !pending.contains(&item.id));
                    self.history.warnings = listing.warnings;
                    self.history.quarantined = listing.quarantined;
                    self.insights.stale = true;
                    self.history.loading = false;
                }
                HistoryEvent::Loaded(mut session) => {
                    if self.history.open_requested.as_deref() != Some(session.id.as_str()) {
                        continue;
                    }
                    self.history.open_requested = None;
                    if let Some(current) = &self.history.call
                        && current.id == session.id
                    {
                        *session = current.clone();
                    }
                    self.history.notetaker_tab = 0;
                    self.history.rename.clone_from(&session.title);
                    self.history.selected = Some(*session);
                    self.history.renaming = false;
                    self.history.confirm_delete = false;
                    self.history.detail_tab = 0;
                    self.history.loading = false;
                }
                HistoryEvent::Saved {
                    id,
                    updated_ms,
                    summary,
                } => {
                    self.brain_saved(&id);
                    for session in [
                        &mut self.history.dictation,
                        &mut self.history.call,
                        &mut self.history.selected,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if session.id == id {
                            session.updated_ms = updated_ms;
                        }
                    }
                    // The saved summary replaces one entry; nothing is re-read.
                    if summary.kind == Kind::Dictation {
                        self.insights.stale = true;
                    }
                    self.history.upsert(*summary);
                }
                HistoryEvent::Deleted { id } => {
                    self.brain.forget(&id);
                    self.history.deleted(&id);
                    self.insights.stale = true;
                }
                HistoryEvent::DeleteFailed { id, error } => {
                    self.history.pending_deletions.remove(&id);
                    self.history.error = Some(error);
                    self.history.loading = false;
                }
                HistoryEvent::Failed(error) => {
                    self.history.error = Some(error);
                    self.history.loading = false;
                }
                HistoryEvent::Quarantine(files) => {
                    self.history.quarantined = files.len();
                    self.history.quarantine = files;
                    self.history.storage.quarantine_listed = true;
                }
                HistoryEvent::QuarantineRestored(session) => {
                    let before = self.history.quarantine.len();
                    self.history.quarantine.retain(|file| file.id != session.id);
                    let removed = before - self.history.quarantine.len();
                    self.history.quarantined = self.history.quarantined.saturating_sub(removed);
                    self.history_notify(
                        format!("Restored \u{201c}{}\u{201d}", session.title),
                        false,
                    );
                    self.history.upsert(Summary::from(&*session));
                    self.insights.stale = true;
                    if let Some(worker) = &self.history.worker {
                        worker.list();
                    }
                }
                HistoryEvent::QuarantineExported { file_name, path } => {
                    self.history_notify(
                        format!("Exported {file_name} to {}", path.display()),
                        false,
                    );
                }
                HistoryEvent::QuarantineDiscarded { file_name } => {
                    self.history
                        .quarantine
                        .retain(|file| file.file_name != file_name);
                    self.history.quarantined = self.history.quarantined.saturating_sub(1);
                    self.history_notify("Discarded the damaged file", false);
                    if let Some(worker) = &self.history.worker {
                        worker.list();
                    }
                }
                HistoryEvent::QuarantineFailed { file_name, error } => {
                    self.history_notify(format!("{error} ({file_name})"), true);
                }
                HistoryEvent::Pinned(summary) => self.history.upsert(*summary),
                // Reported once known people and recordings in progress
                // have converted too (see call_tools).
                HistoryEvent::ProtectionChanged(result) => self.calls_protection_changed(result),
                HistoryEvent::RetentionPreview(result) => {
                    self.history.storage.retention_preview_working = false;
                    self.history.retention_preview = Some(result);
                }
                HistoryEvent::RetentionApplied(result) => {
                    match &result {
                        Ok(applied) if !applied.failed.is_empty() => {
                            self.history_notify(
                                format!(
                                    "{} old items could not be removed. They will be retried later.",
                                    applied.failed.len()
                                ),
                                true,
                            );
                        }
                        Ok(applied) if !applied.deleted.is_empty() => {
                            let count = applied.deleted.len();
                            self.history_notify(
                                format!(
                                    "Removed {count} {} older than your retention period",
                                    if count == 1 { "item" } else { "items" }
                                ),
                                false,
                            );
                        }
                        Ok(_) => {}
                        Err(error) => self.history_notify(error.clone(), true),
                    }
                    if let Ok(applied) = &result {
                        for id in &applied.deleted {
                            self.brain.forget(id);
                            self.history.items.retain(|item| &item.id != id);
                        }
                    }
                    self.history.retention_preview = None;
                    self.history.storage.retention_previewed = None;
                    self.insights.stale = true;
                }
                HistoryEvent::BackupCreated(result) => {
                    self.history.storage.backup = Progress::Idle;
                    self.history.backup = Some(result);
                }
                HistoryEvent::RestoreFinished(mut result) => {
                    self.history.storage.restore = Progress::Idle;
                    self.history.storage.restored_settings = None;
                    if let Ok(restored) = &mut result
                        && (restored.settings.is_some() || restored.library.is_some())
                    {
                        let library = restored.library.take();
                        match merge_restored_settings(
                            &self.settings,
                            restored.settings.take(),
                            library.as_deref(),
                        ) {
                            Ok(settings) => {
                                let applied = self.history_apply_restored_settings(settings);
                                self.history.storage.restored_settings = Some(applied);
                            }
                            Err(error) => {
                                self.history_notify(
                                    format!("Restored preferences were not applied: {error}"),
                                    true,
                                );
                            }
                        }
                    }
                    self.insights.stale = true;
                    self.history.restored = Some(result);
                }
                HistoryEvent::MarkdownExported(result) => {
                    self.history.storage.markdown = Progress::Idle;
                    self.history.markdown_export = Some(result);
                }
            }
        }
        self.history_poll_dialogs();
        if self.insights.stale
            && !self.insights.loading
            && self
                .history
                .insights_requested
                .is_none_or(|at| at.elapsed() >= INSIGHTS_REFRESH)
            && let Some(worker) = &self.history.worker
        {
            worker.insights();
            self.insights.stale = false;
            self.insights.loading = true;
            self.history.insights_requested = Some(Instant::now());
        }
        if self.settings.retention.enabled
            && !self.history.loading
            && self.history.worker.is_some()
            && self
                .history
                .retention_applied
                .is_none_or(|at| at.elapsed() >= RETENTION_INTERVAL)
        {
            let active = self.history_active_ids();
            if let Some(worker) = &self.history.worker {
                worker.apply_retention(self.settings.retention.clone(), None, active);
            }
            self.history.retention_applied = Some(Instant::now());
        }
        if self.history.notetaker_retry {
            self.history.notetaker_retry = false;
            self.history_retry();
        }
        if self
            .history
            .selected_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(700))
        {
            self.history_save_personal_notes();
        }
        if self
            .history
            .dictation_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(800))
        {
            self.history_save_dictation();
        }
        let call_delay =
            if committed_fingerprint(&self.call_committed) != self.history.call_saved_committed {
                CALL_COMMIT_SAVE
            } else {
                CALL_PREVIEW_SAVE
            };
        if self
            .history
            .call_dirty
            .is_some_and(|at| at.elapsed() >= call_delay)
        {
            self.history_save_call();
        }
    }

    pub(super) fn history_retry(&mut self) {
        self.history.error = None;
        self.history_save_personal_notes();
        if self.history.dictation.is_some() {
            self.history_save_dictation();
        }
        if self.history.call.is_some() {
            self.history_save_call();
        }
        if let Some(worker) = &self.history.worker {
            worker.retry();
            if let Some(id) = &self.history.open_requested {
                worker.load(id.clone());
            }
            worker.list();
        }
    }

    #[allow(
        dead_code,
        reason = "Persistence operations retained for dictation editing and document updates"
    )]
    fn history_rename(&mut self, session: &mut Session, title: String) {
        if self.history.pending_deletions.contains(&session.id) {
            return;
        }
        session.title = title.clone();
        session.title_is_manual = true;
        self.brain.remember(session);
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.title = title;
            current.title_is_manual = true;
            self.history_save_call();
            session.clone_from(self.history.call.as_ref().unwrap());
        } else if let Some(current) = &mut self.history.dictation
            && current.id == session.id
        {
            current.title = title;
            current.title_is_manual = true;
            self.history_save_dictation();
            session.clone_from(self.history.dictation.as_ref().unwrap());
        } else if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    #[allow(
        dead_code,
        reason = "Persistence operations retained for dictation editing and document updates"
    )]
    fn history_save_notes_document(&mut self, session: &Session) {
        if self.history.pending_deletions.contains(&session.id) {
            return;
        }
        if let Some(current) = &mut self.history.dictation
            && current.id == session.id
        {
            current.personal_notes.clone_from(&session.personal_notes);
            current
                .generated_summary
                .clone_from(&session.generated_summary);
            current
                .protected_note_items
                .clone_from(&session.protected_note_items);
        }
        self.brain.remember(session);
        if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }
}

impl App {
    /// Sessions open in the editor or being captured. Retention never removes them.
    fn history_active_ids(&self) -> Vec<String> {
        [
            &self.history.dictation,
            &self.history.call,
            &self.history.selected,
        ]
        .into_iter()
        .flatten()
        .map(|session| session.id.clone())
        .collect()
    }
}

/// Storage operations for the library and Privacy & storage settings. Each
/// call returns immediately; file dialogs run on their own thread so they never
/// pause capture or autosave, and results arrive through `history_poll`.
impl App {
    /// Show a one-time library message.
    pub(super) fn history_notify(&mut self, message: impl Into<String>, error: bool) {
        let storage = &mut self.history.storage;
        let sequence = storage
            .notice
            .as_ref()
            .map_or(1, |notice| notice.sequence + 1);
        storage.notice = Some(Notice {
            sequence,
            error,
            message: message.into(),
        });
    }

    /// Run `choose` (which shows a file dialog) away from the controller, then
    /// finish with the continuation it returns on the controller.
    fn history_dialog(
        &mut self,
        choose: impl FnOnce(Pickers) -> Continuation + Send + 'static,
    ) -> Result<(), String> {
        if self.history.storage.dialog_open {
            return Err("Finish the open file window first.".into());
        }
        let pickers = self.history.storage.pickers;
        let sender = self
            .history
            .storage
            .dialogs
            .get_or_insert_with(mpsc::channel)
            .0
            .clone();
        let run = move || {
            let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| choose(pickers)))
                .unwrap_or_else(|_| {
                    Box::new(|app: &mut App| {
                        app.history_notify("The file window closed unexpectedly. Try again.", true);
                    })
                });
            let _ = sender.send(next);
        };
        // Tests use synthetic pickers and finish in the same poll.
        #[cfg(test)]
        run();
        #[cfg(not(test))]
        std::thread::Builder::new()
            .name("file-dialog".into())
            .spawn(run)
            .map_err(|error| format!("Could not open the file window: {error}"))?;
        self.history.storage.dialog_open = true;
        Ok(())
    }

    fn history_poll_dialogs(&mut self) {
        let ready: Vec<Continuation> = match &self.history.storage.dialogs {
            Some((_, receiver)) => receiver.try_iter().collect(),
            None => return,
        };
        for next in ready {
            self.history.storage.dialog_open = false;
            next(self);
        }
    }

    /// Turn encryption at rest on or off. Preferences are rewritten first, then
    /// every history file is migrated; `State::protection` reports the result.
    /// This is the only way `Settings::encrypt_at_rest` changes.
    pub(super) fn history_set_encryption(&mut self, enabled: bool) -> Result<(), String> {
        if self.history.storage.encryption_working {
            return Err("Wait for the current encryption change to finish.".into());
        }
        let Some(worker) = &self.history.worker else {
            return Err("Local history is unavailable.".into());
        };
        let previous = self.settings.encrypt_at_rest;
        self.settings.encrypt_at_rest = enabled;
        if let Err(error) = self.save_preferences() {
            self.settings.encrypt_at_rest = previous;
            return Err(format!("Could not save preferences: {error}"));
        }
        worker.set_protection(enabled);
        // Known people and recordings in progress follow the same protection;
        // retained audio is migrated with the history files.
        self.calls_set_protection(enabled);
        self.history.protection = None;
        self.history.storage.encryption_working = true;
        Ok(())
    }

    /// Dry run: fills `State::retention_preview` with what `policy` would delete.
    pub(super) fn history_retention_preview(
        &mut self,
        policy: crate::history::retention::Policy,
    ) -> Result<(), String> {
        policy.validate().map_err(|error| error.to_string())?;
        let active = self.history_active_ids();
        let worker = self
            .history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?;
        worker.retention_preview(policy.clone(), active);
        self.history.retention_preview = None;
        self.history.storage.retention_preview_working = true;
        self.history.storage.retention_previewed = Some(policy);
        Ok(())
    }

    /// Save a retention policy. Turning a policy on requires a finished
    /// preview of exactly that policy; the previewed items are deleted now,
    /// and later items are removed automatically once a day.
    pub(super) fn history_save_retention(
        &mut self,
        policy: crate::history::retention::Policy,
    ) -> Result<(), String> {
        policy.validate().map_err(|error| error.to_string())?;
        let confirmed = if policy.enabled {
            if self.history.storage.retention_preview_working
                || self.history.storage.retention_previewed.as_ref() != Some(&policy)
            {
                return Err("Review what this policy removes before turning it on.".into());
            }
            match &self.history.retention_preview {
                Some(Ok(items)) => items.iter().map(|item| item.id.clone()).collect(),
                Some(Err(error)) => return Err(error.clone()),
                None => {
                    return Err("Review what this policy removes before turning it on.".into());
                }
            }
        } else {
            Vec::new()
        };
        self.settings.retention = policy.clone();
        if let Err(error) = self.save_preferences() {
            return Err(format!("Could not save preferences: {error}"));
        }
        // The confirmed items are removed now; the next automatic pass waits a day.
        self.history.retention_applied = Some(Instant::now());
        self.history.retention_preview = None;
        self.history.storage.retention_previewed = None;
        if !confirmed.is_empty() {
            self.history_apply_retention(policy, confirmed);
        }
        Ok(())
    }

    /// Delete exactly the confirmed sessions from a preview, if they still qualify.
    fn history_apply_retention(
        &mut self,
        policy: crate::history::retention::Policy,
        confirmed: Vec<String>,
    ) {
        let active = self.history_active_ids();
        if let Some(worker) = &self.history.worker {
            worker.apply_retention(policy, Some(confirmed), active);
        }
    }

    pub(super) fn history_set_pinned(&mut self, id: String, pinned: bool) -> Result<(), String> {
        let worker = self
            .history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?;
        if self.history.pending_deletions.contains(&id) {
            return Err("This item is being deleted.".into());
        }
        worker.set_pinned(id.clone(), pinned);
        for session in [
            &mut self.history.dictation,
            &mut self.history.call,
            &mut self.history.selected,
        ]
        .into_iter()
        .flatten()
        {
            if session.id == id {
                session.pinned = pinned;
            }
        }
        Ok(())
    }

    /// Refresh `State::quarantine`.
    pub(super) fn history_list_quarantine(&mut self) -> Result<(), String> {
        self.history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?
            .quarantined();
        Ok(())
    }

    fn history_quarantined_file(&self, file_name: &str) -> Result<(), String> {
        if self.history.worker.is_none() {
            return Err("Local history is unavailable.".into());
        }
        if !self
            .history
            .quarantine
            .iter()
            .any(|file| file.file_name == file_name)
        {
            return Err("This file is no longer isolated. Refresh the list.".into());
        }
        Ok(())
    }

    pub(super) fn history_restore_quarantined(&mut self, file_name: String) -> Result<(), String> {
        self.history_quarantined_file(&file_name)?;
        if let Some(worker) = &self.history.worker {
            worker.restore_quarantined(file_name);
        }
        Ok(())
    }

    /// Ask where to save, then export the isolated file's raw (decrypted) bytes.
    pub(super) fn history_export_quarantined(&mut self, file_name: String) -> Result<(), String> {
        self.history_quarantined_file(&file_name)?;
        self.history_dialog(move |pickers| {
            let chosen = (pickers.save_path)(
                crate::export_file::Document::Export,
                &format!("{file_name}.json"),
                "json",
            );
            Box::new(move |app: &mut App| match chosen {
                Ok(Some(path)) => {
                    if let Some(worker) = &app.history.worker {
                        worker.export_quarantined(file_name, path);
                    }
                }
                Ok(None) => {}
                Err(error) => app.history_notify(error.to_string(), true),
            })
        })
    }

    pub(super) fn history_discard_quarantined(&mut self, file_name: String) -> Result<(), String> {
        self.history_quarantined_file(&file_name)?;
        if let Some(worker) = &self.history.worker {
            worker.discard_quarantined(file_name);
        }
        Ok(())
    }

    fn history_storage_idle(&self) -> Result<(), String> {
        let storage = &self.history.storage;
        if self.history.worker.is_none() {
            return Err("Local history is unavailable.".into());
        }
        if storage.backup != Progress::Idle
            || storage.restore != Progress::Idle
            || storage.markdown != Progress::Idle
        {
            return Err("Wait for the current backup, restore or export to finish.".into());
        }
        Ok(())
    }

    /// Ask where to save, then write a backup archive. Optional passphrase.
    pub(super) fn history_backup(&mut self, passphrase: Option<String>) -> Result<(), String> {
        self.history_storage_idle()?;
        let passphrase = passphrase.filter(|passphrase| !passphrase.is_empty());
        if passphrase
            .as_ref()
            .is_some_and(|passphrase| passphrase.chars().count() < 8)
        {
            return Err("Use a passphrase of at least 8 characters.".into());
        }
        let date = crate::call_export::local_time(crate::history::now_ms())
            .chars()
            .take(10)
            .collect::<String>();
        self.history_dialog(move |pickers| {
            let chosen = (pickers.backup_destination)(&format!("Articulate backup {date}"));
            Box::new(move |app: &mut App| {
                app.history.storage.backup = Progress::Idle;
                let destination = match chosen {
                    Ok(Some(destination)) => destination,
                    Ok(None) => return,
                    Err(error) => return app.history_notify(error.to_string(), true),
                };
                let request = serde_json::to_value(&app.settings)
                    .map_err(|error| error.to_string())
                    .and_then(|settings| {
                        crate::library::export(&app.settings.entries, &app.settings.macros)
                            .map(|library| (settings, library))
                            .map_err(|error| error.to_string())
                    });
                match (request, &app.history.worker) {
                    (Ok((settings, library)), Some(worker)) => {
                        worker.backup(backup::Request {
                            destination,
                            passphrase,
                            settings,
                            library,
                            include_audio: false,
                        });
                        app.history.backup = None;
                        app.history.storage.backup = Progress::Working;
                    }
                    (Err(error), _) => app.history.backup = Some(Err(error)),
                    (_, None) => {
                        app.history.backup = Some(Err("Local history is unavailable.".into()));
                    }
                }
            })
        })?;
        self.history.storage.backup = Progress::Choosing;
        Ok(())
    }

    /// Ask for an archive, then validate and merge it. Preferences and
    /// vocabulary are merged when `include_settings` is set.
    pub(super) fn history_restore(
        &mut self,
        passphrase: Option<String>,
        replace_older: bool,
        include_settings: bool,
    ) -> Result<(), String> {
        self.history_storage_idle()?;
        if self.recording.is_some() || self.call.is_some() || self.busy {
            return Err("Finish recording before restoring a backup.".into());
        }
        let passphrase = passphrase.filter(|passphrase| !passphrase.is_empty());
        self.history_dialog(move |pickers| {
            let chosen = (pickers.backup_source)();
            Box::new(move |app: &mut App| {
                app.history.storage.restore = Progress::Idle;
                let source = match chosen {
                    Ok(Some(source)) => source,
                    Ok(None) => return,
                    Err(error) => return app.history_notify(error.to_string(), true),
                };
                // Saving open documents first lets a restore compare their latest version.
                app.history_save_personal_notes();
                if let Some(worker) = &app.history.worker {
                    worker.restore(backup::RestoreRequest {
                        source,
                        passphrase,
                        replace_older,
                        include_settings,
                    });
                    app.history.restored = None;
                    app.history.storage.restored_settings = None;
                    app.history.storage.restore = Progress::Working;
                }
            })
        })?;
        self.history.storage.restore = Progress::Choosing;
        Ok(())
    }

    /// Apply merged preferences from a backup. Encryption and retention stay
    /// as chosen on this device: they change only through their own controls,
    /// so a restore never rewrites files or deletes items unexpectedly.
    /// Shortcuts are registered again; a changed speech setting needs a reload.
    fn history_apply_restored_settings(&mut self, mut settings: Settings) -> RestoredSettings {
        settings.encrypt_at_rest = self.settings.encrypt_at_rest;
        settings.retention = self.settings.retention.clone();
        let previous = std::mem::replace(&mut self.settings, settings);
        self.save();
        let mut applied = RestoredSettings {
            shortcuts: false,
            model_reload: self.settings.cpu != previous.cpu
                || self.settings.transcription_language != previous.transcription_language,
        };
        if self.settings.hotkey != previous.hotkey {
            applied.shortcuts = true;
            if self.hotkey_tx.send(self.settings.hotkey.clone()).is_ok() {
                self.hotkey_draft = self.settings.hotkey.clone();
                self.hotkey_pending = true;
            }
        }
        if self.settings.quick_note_hotkey != previous.quick_note_hotkey {
            applied.shortcuts = true;
            if self
                .quick_note_hotkey_tx
                .send(self.settings.quick_note_hotkey.clone())
                .is_ok()
            {
                self.quick_note_hotkey_pending = true;
                self.quick_note_shortcut_error = None;
            }
        }
        applied
    }

    /// Ask for a folder, then write every session there as Markdown.
    pub(super) fn history_export_all_markdown(&mut self) -> Result<(), String> {
        self.history_storage_idle()?;
        self.history_dialog(|pickers| {
            let chosen = (pickers.folder)("Choose a folder for your Markdown files");
            Box::new(move |app: &mut App| {
                app.history.storage.markdown = Progress::Idle;
                match chosen {
                    Ok(Some(folder)) => {
                        if let Some(worker) = &app.history.worker {
                            worker.export_markdown(folder);
                            app.history.markdown_export = None;
                            app.history.storage.markdown = Progress::Working;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => app.history_notify(error.to_string(), true),
                }
            })
        })?;
        self.history.storage.markdown = Progress::Choosing;
        Ok(())
    }

    /// Export one session with a title and file name matching its kind.
    /// Formats the session can't produce, such as subtitles without timing,
    /// are refused before any dialog opens.
    pub(super) fn history_export_session(
        &mut self,
        session: &Session,
        format: crate::call_export::Format,
    ) -> Result<(), String> {
        let text =
            crate::call_export::session(session, format).map_err(|error| error.to_string())?;
        let document = crate::export_file::Document::from_kind(session.kind);
        let title = session.title.clone();
        let label = match format {
            crate::call_export::Format::Text => "text",
            crate::call_export::Format::Markdown => "Markdown",
            crate::call_export::Format::Srt => "SRT subtitles",
            crate::call_export::Format::WebVtt => "WebVTT subtitles",
        };
        self.history_dialog(move |pickers| {
            let saved = (pickers.save_text)(&text, format.extension(), document, Some(&title));
            Box::new(move |app: &mut App| match saved {
                Ok(true) => app.history_notify(format!("Exported as {label}"), false),
                Ok(false) => {}
                Err(error) => app.history_notify(format!("Could not export: {error}"), true),
            })
        })
    }

    /// Export the action items kept in a document's notes as a Markdown
    /// checklist or a calendar file of tasks, optionally with a reminder.
    pub(super) fn history_export_followups(
        &mut self,
        session: &Session,
        calendar: bool,
        reminder_ms: Option<u64>,
    ) -> Result<(), String> {
        use crate::brain::followups;
        let items = followups::reviewed(session);
        if items.is_empty() {
            return Err(
                "There are no follow-ups in this document's notes. Update notes, then keep the action items you want to export."
                    .into(),
            );
        }
        let (text, extension) = if calendar {
            let now = crate::history::now_ms();
            let stamp = followups::utc_stamp(now).map_err(|error| error.to_string())?;
            let text = match reminder_ms {
                Some(reminder) if reminder <= now => {
                    return Err("Choose a reminder time in the future.".into());
                }
                Some(reminder) => {
                    let reminder =
                        followups::utc_stamp(reminder).map_err(|error| error.to_string())?;
                    followups::ics_with_reminder(&items, &session.id, &stamp, &reminder)
                }
                None => followups::ics(&items, &session.id, &stamp),
            }
            .map_err(|error| error.to_string())?;
            (text, "ics")
        } else {
            (followups::markdown(&items), "md")
        };
        let title = format!("{} follow-ups", session.title.trim());
        let count = items.len();
        self.history_dialog(move |pickers| {
            let saved = (pickers.save_text)(
                &text,
                extension,
                crate::export_file::Document::FollowUps,
                Some(&title),
            );
            Box::new(move |app: &mut App| match saved {
                Ok(true) => app.history_notify(
                    format!(
                        "Exported {count} {}",
                        if count == 1 {
                            "follow-up"
                        } else {
                            "follow-ups"
                        }
                    ),
                    false,
                ),
                Ok(false) => {}
                Err(error) => app.history_notify(format!("Could not export: {error}"), true),
            })
        })
    }
}

fn title(text: &str, fallback: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        fallback.into()
    } else {
        text.chars().take(72).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, updated_ms: u64) -> Summary {
        let mut session = Session::new(Kind::Note);
        session.id = id.into();
        session.updated_ms = updated_ms;
        Summary::from(&session)
    }

    #[test]
    fn saved_summaries_update_one_entry_in_order_without_listing() {
        let mut state = State::default();
        state.upsert(summary("b", 20));
        state.upsert(summary("a", 10));
        state.upsert(summary("c", 30));
        let ids = |state: &State| state.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&state), ["c", "b", "a"]);
        state.upsert(summary("a", 40));
        assert_eq!(ids(&state), ["a", "c", "b"]);
        state.pending_deletions.insert("b".into());
        state.upsert(summary("b", 50));
        assert_eq!(ids(&state), ["a", "c", "b"]);
        assert_eq!(state.items[2].updated_ms, 20);
    }

    #[test]
    fn preview_only_call_changes_wait_longer_than_committed_rows() {
        let (mut app, _) = super::super::tests::app();
        let row = |text: &str| calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 1000,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: text.into(),
        };
        app.call_committed = vec![row("Committed words.")];
        app.call_rows = app.call_committed.clone();
        app.history_save_call();
        // Only preview text changed: not saved after the commit interval.
        app.call_rows.push(row("A revisable preview."));
        app.history.call_dirty =
            Some(Instant::now() - CALL_COMMIT_SAVE - Duration::from_millis(10));
        app.history_poll();
        assert!(app.history.call_dirty.is_some());
        app.history.call_dirty = Some(Instant::now() - CALL_PREVIEW_SAVE);
        app.history_poll();
        assert!(app.history.call_dirty.is_none());
        // A newly committed row saves after the short interval.
        app.call_committed.push(row("More committed words."));
        app.call_rows.push(row("More committed words."));
        app.history.call_dirty =
            Some(Instant::now() - CALL_COMMIT_SAVE - Duration::from_millis(10));
        app.history_poll();
        assert!(app.history.call_dirty.is_none());
        assert_eq!(app.history.call.as_ref().unwrap().rows.len(), 3);
    }

    #[test]
    fn stale_insights_refresh_through_the_worker() {
        let (mut app, _) = super::super::tests::app();
        let directory = std::env::temp_dir().join(format!(
            "articulate-insights-refresh-{}",
            Session::new(Kind::Note).id
        ));
        app.history.worker = Some(Worker::test_directory(directory.clone()));
        app.insights.stale = true;
        app.history_poll();
        assert!(!app.insights.stale && app.insights.loading);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.insights.report.is_none() {
            app.history_poll();
            assert!(Instant::now() < deadline, "Insights did not refresh");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(app);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_deletion_clears_only_its_own_pending_request() {
        let (mut app, _) = super::super::tests::app();
        let temp = std::env::temp_dir();
        let directory = temp.join(format!(
            "articulate-delete-test-{}",
            Session::new(Kind::Note).id
        ));
        assert_eq!(directory.parent(), Some(temp.as_path()));
        app.history.worker = Some(Worker::test_directory(directory.clone()));
        app.history
            .pending_deletions
            .insert("another-request".into());
        assert!(app.history_delete("invalid/id"));
        assert!(app.history.pending_deletions.contains("invalid/id"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.history.pending_deletions.contains("invalid/id") {
            app.history_poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.history.pending_deletions.contains("another-request"));
        assert!(app.history.error.is_some());
        drop(app);
        if directory.exists() {
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn dictation_autosave_preserves_document_edits() {
        let (mut app, _) = super::super::tests::app();
        app.text = "The spoken transcript.".into();
        app.history_save_dictation();
        let mut edited = app.history.dictation.clone().unwrap();
        edited.personal_notes = "My edited notes, not spoken aloud.".into();
        app.history_save_notes_document(&edited);
        app.text.push_str(" More transcript words.");
        app.history_save_dictation();
        let saved = app.history.dictation.as_ref().unwrap();
        assert_eq!(saved.personal_notes, edited.personal_notes);
        assert!(saved.text.ends_with("More transcript words."));
    }

    #[test]
    fn manual_edits_preserve_capture_metrics_and_summary() {
        let (mut app, _) = super::super::tests::app();
        app.text = "Two words".into();
        app.raw = app.text.clone();
        app.dictation_metrics = crate::insights::DictationMetrics {
            recognized_words: Some(2),
            audio_duration_ms: Some(1500),
            app: Some("Editor.exe".into()),
            ..Default::default()
        };
        app.history_save_dictation();
        let id = app.history.dictation.as_ref().unwrap().id.clone();
        app.text = "Several manually added words for this note".into();
        app.history_save_dictation();
        let saved = app.history.dictation.as_ref().unwrap();
        assert_eq!(saved.id, id);
        assert_eq!(saved.metrics.recognized_words, Some(2));
        assert_eq!(saved.metrics.audio_duration_ms, Some(1500));
        assert_eq!(saved.original, "Two words");
    }

    #[test]
    fn cleared_preview_overwrites_saved_draft_without_creating_empty_sessions() {
        let (mut app, _) = super::super::tests::app();
        app.history_save_call();
        assert!(app.history.call.is_none());
        app.event_tx
            .send(Event::Call(calls::Update::Preview(vec![calls::Row {
                cues: Vec::new(),
                start_ms: 0,
                end_ms: 1000,
                microphone: false,
                speakers: vec![1],
                discord: None,
                text: "Please send the temporary draft.".into(),
            }])))
            .unwrap();
        app.receive();
        app.call_notes = Some(crate::notes::Notes::build(&app.call_rows));
        app.history_save_call();
        let original = app.history.call.clone().unwrap();
        let temp_root = std::env::temp_dir();
        let directory = temp_root.join(format!("articulate-empty-preview-test-{}", original.id));
        assert_eq!(directory.parent(), Some(temp_root.as_path()));
        let history = crate::history::History::open(directory.clone()).unwrap();
        history.save(original.clone()).unwrap();
        app.event_tx
            .send(Event::Call(calls::Update::Preview(Vec::new())))
            .unwrap();
        app.receive();
        app.history_save_call();
        let cleared = app.history.call.clone().unwrap();
        assert_eq!(cleared.id, original.id);
        assert!(cleared.rows.is_empty());
        assert!(cleared.text.is_empty());
        assert!(cleared.notes.as_ref().unwrap().actions.is_empty());
        history.save(cleared).unwrap();
        assert!(history.load(&original.id).unwrap().text.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rename_uses_current_draft_not_stale_history_content() {
        let (mut app, _) = super::super::tests::app();
        app.text = "First version".into();
        app.raw = "The original words".into();
        app.history_save_dictation();
        let mut old = app.history.dictation.clone().unwrap();
        app.text = "The user's corrected version".into();
        app.history_rename(&mut old, "Renamed note".into());
        assert_eq!(old.text, app.text);
        assert_eq!(old.original, app.raw);
        assert_eq!(old.title, "Renamed note");
        assert_eq!(old.id, app.history.dictation.as_ref().unwrap().id);
    }

    #[test]
    fn renaming_saved_call_keeps_new_rows_names_and_notes() {
        let (mut app, _) = super::super::tests::app();
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 1000,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: "First part.".into(),
        });
        app.history_save_call();
        let mut old = app.history.call.clone().unwrap();
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 1000,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "I will review this tomorrow.".into(),
        });
        app.speaker_names[0] = "Alex".into();
        app.call_notes = Some(crate::notes::Notes::build(&app.call_rows));
        app.history_rename(&mut old, "Review call".into());
        assert_eq!(old.rows.len(), 2);
        assert_eq!(old.speaker_names[0], "Alex");
        assert!(old.notes.is_some());
        assert_eq!(old.text, calls::text(&app.call_rows, &app.speaker_names));
        assert_eq!(old.id, app.history.call.as_ref().unwrap().id);
    }

    #[test]
    fn deletion_does_not_resurrect_visible_draft_on_later_edits() {
        let (mut app, _) = super::super::tests::app();
        app.text = "Saved words".into();
        app.history_save_dictation();
        let id = app.history.dictation.as_ref().unwrap().id.clone();
        app.history.deleted(&id);
        app.text.push_str(" with an edit");
        app.history_save_dictation();
        assert!(app.history.dictation.is_none());
        assert_eq!(app.text, "Saved words with an edit");
        app.history.dictation_deleted = false;
        app.history_save_dictation();
        assert_ne!(app.history.dictation.as_ref().unwrap().id, id);
    }
}
