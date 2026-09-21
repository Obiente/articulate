use super::*;
use crate::history::{Event as HistoryEvent, Kind, Session, Summary, Worker};

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
}

impl State {
    pub fn start() -> Self {
        Self {
            worker: Some(Worker::start()),
            loading: true,
            ..Default::default()
        }
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
                HistoryEvent::Listed(items) => {
                    self.history.items = items;
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
                HistoryEvent::Saved { id, updated_ms } => {
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
                    if let Some(worker) = &self.history.worker {
                        worker.list();
                    }
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
            }
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
        if self
            .history
            .call_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
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
