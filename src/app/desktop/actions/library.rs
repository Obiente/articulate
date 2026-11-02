//! Saved documents: opening, editing, filing, deleting, notes and insights.

use crate::app::App;
use crate::brain::search::Filters;
use crate::calls;
use crate::history::{Kind, Session, retention::Policy};
use serde::Deserialize;

/// Search restrictions from the renderer. Dates are inclusive Unix milliseconds.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SearchFilters {
    from_ms: Option<u64>,
    to_ms: Option<u64>,
    kind: Option<Kind>,
    speaker: Option<String>,
    app: Option<String>,
}

impl SearchFilters {
    fn into_filters(self) -> Result<Filters, String> {
        let text = |value: Option<String>| {
            value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let speaker = text(self.speaker);
        let app = text(self.app);
        if [&speaker, &app].into_iter().flatten().any(|value| {
            value.chars().count() > 160 || value.chars().any(char::is_control)
        }) {
            return Err("Use a speaker or app name of up to 160 characters.".into());
        }
        if let (Some(from), Some(to)) = (self.from_ms, self.to_ms)
            && from > to
        {
            return Err("Choose a start date before the end date.".into());
        }
        Ok(Filters {
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            kind: self.kind,
            speaker,
            app,
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportFormat {
    Text,
    Markdown,
    Srt,
    Vtt,
}

impl From<ExportFormat> for crate::call_export::Format {
    fn from(format: ExportFormat) -> Self {
        match format {
            ExportFormat::Text => Self::Text,
            ExportFormat::Markdown => Self::Markdown,
            ExportFormat::Srt => Self::Srt,
            ExportFormat::Vtt => Self::WebVtt,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FollowUpFormat {
    Markdown,
    Ics,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    HistoryRefresh,
    HistoryOpen {
        id: String,
    },
    HistoryClose,
    HistoryRetry,
    NoteNew,
    SourceMove {
        id: String,
        topic_id: Option<String>,
        expected_topic_id: Option<String>,
    },
    SourceFile {
        id: String,
    },
    SessionPatch {
        id: String,
        title: Option<String>,
        personal_notes: Option<String>,
    },
    SessionDelete {
        id: String,
    },
    DictationEdit {
        id: String,
        expected_text: String,
        text: String,
        correction: Option<crate::app::transcript_editor::Spelling>,
    },
    NotesGenerate {
        id: String,
    },
    InsightsRefresh,
    /// Full-text search of saved items. An empty query clears the results.
    HistorySearch {
        query: String,
        #[serde(default)]
        filters: SearchFilters,
    },
    SessionPin {
        id: String,
        pinned: bool,
    },
    SessionExport {
        id: String,
        format: ExportFormat,
    },
    FollowupsExport {
        id: String,
        format: FollowUpFormat,
        reminder_ms: Option<u64>,
    },
    StorageEncryption {
        enabled: bool,
    },
    RetentionPreview {
        policy: Policy,
    },
    RetentionSave {
        policy: Policy,
    },
    QuarantineList,
    QuarantineRestore {
        file_name: String,
    },
    QuarantineExport {
        file_name: String,
    },
    QuarantineDiscard {
        file_name: String,
    },
    BackupCreate {
        passphrase: Option<String>,
    },
    BackupRestore {
        passphrase: Option<String>,
        replace_older: bool,
        include_settings: bool,
    },
    HistoryExportMarkdown,
}

impl App {
    pub(crate) fn desktop_library_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::HistoryRefresh => {
                if let Some(worker) = &self.history.worker {
                    worker.list();
                }
            }
            Action::HistoryOpen { id } => {
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    self.history_save_personal_notes();
                    self.history.selected = self.history.call.clone();
                    self.history.open_requested = None;
                } else {
                    self.notetaker_open(id);
                }
            }
            Action::HistoryClose => self.notetaker_hub(),
            Action::HistoryRetry => self.history_retry(),
            Action::NoteNew => self.notetaker_new_note(),
            Action::SourceMove {
                id,
                topic_id,
                expected_topic_id,
            } => {
                if self.call.is_some()
                    || self.recording.is_some()
                    || self.busy
                    || self.brain_working()
                    || self.notetaker_routing_busy()
                {
                    return Err(
                        "Wait for recording and notes to finish before moving this source.".into(),
                    );
                }
                self.history_save_personal_notes();
                self.notetaker_forget_filing(&id);
                let destination = topic_id
                    .map(crate::topics::Destination::Existing)
                    .unwrap_or(crate::topics::Destination::Unfiled);
                self.history
                    .worker
                    .as_ref()
                    .ok_or("Local history is unavailable.")?
                    .move_source(id, destination, expected_topic_id);
                self.notetaker.moving = true;
                self.notetaker.status = "Moving the recording and updating its notes…".into();
            }
            Action::SourceFile { id } => {
                let session = self
                    .history
                    .selected
                    .as_ref()
                    .filter(|session| session.id == id)
                    .or_else(|| {
                        self.history
                            .call
                            .as_ref()
                            .filter(|session| session.id == id)
                    })
                    .cloned()
                    .ok_or("Open the recording before filing it.")?;
                self.notetaker_queue_filing(session);
            }
            Action::SessionPatch {
                id,
                title,
                personal_notes,
            } => self.desktop_patch_session(&id, title, personal_notes)?,
            Action::SessionDelete { id } => {
                if self.notetaker_routing_busy() {
                    return Err(
                        "Wait for topic filing to finish before deleting a recording.".into(),
                    );
                }
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    return Err("Finish recording before deleting this call.".into());
                }
                self.history_save_personal_notes();
                if !self.history_delete(&id) {
                    return Err("Local history is unavailable.".into());
                }
            }
            Action::DictationEdit {
                id,
                expected_text,
                text,
                correction,
            } => self.edit_saved_dictation(&id, &expected_text, text, correction)?,
            Action::NotesGenerate { id } => {
                if self.history.pending_deletions.contains(&id) {
                    return Err("This document is being deleted.".into());
                }
                let mut session = self
                    .history
                    .call
                    .as_ref()
                    .filter(|s| s.id == id)
                    .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
                    .cloned()
                    .ok_or("Open the document before updating notes.")?;
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    session.rows.clone_from(&self.call_committed);
                    session.text = calls::text(&session.rows, &session.speaker_names);
                }
                self.desktop_notes_generate(session)?;
            }
            Action::InsightsRefresh => {
                if let Some(worker) = &self.history.worker {
                    worker.insights();
                    self.insights.loading = true;
                }
            }
            Action::HistorySearch { query, filters } => {
                let filters = filters.into_filters()?;
                self.saved_search(&query, filters);
            }
            Action::SessionPin { id, pinned } => self.history_set_pinned(id, pinned)?,
            Action::SessionExport { id, format } => {
                let session = self.desktop_export_source(&id)?;
                self.history_export_session(&session, format.into())?;
            }
            Action::FollowupsExport {
                id,
                format,
                reminder_ms,
            } => {
                let session = self.desktop_export_source(&id)?;
                let calendar = matches!(format, FollowUpFormat::Ics);
                if !calendar && reminder_ms.is_some() {
                    return Err("Reminders are only available for calendar files.".into());
                }
                self.history_export_followups(&session, calendar, reminder_ms)?;
            }
            Action::StorageEncryption { enabled } => self.history_set_encryption(enabled)?,
            Action::RetentionPreview { policy } => self.history_retention_preview(policy)?,
            Action::RetentionSave { policy } => self.history_save_retention(policy)?,
            Action::QuarantineList => self.history_list_quarantine()?,
            Action::QuarantineRestore { file_name } => {
                self.history_restore_quarantined(file_name)?;
            }
            Action::QuarantineExport { file_name } => self.history_export_quarantined(file_name)?,
            Action::QuarantineDiscard { file_name } => {
                self.history_discard_quarantined(file_name)?;
            }
            Action::BackupCreate { passphrase } => self.history_backup(passphrase)?,
            Action::BackupRestore {
                passphrase,
                replace_older,
                include_settings,
            } => self.history_restore(passphrase, replace_older, include_settings)?,
            Action::HistoryExportMarkdown => self.history_export_all_markdown()?,
        }
        Ok(())
    }

    /// The current version of an open, captured or selected document.
    fn desktop_export_source(&mut self, id: &str) -> Result<Session, String> {
        if self.history.pending_deletions.contains(id) {
            return Err("This document is being deleted.".into());
        }
        self.history_save_personal_notes();
        let mut session = self
            .history
            .call
            .as_ref()
            .filter(|s| s.id == id)
            .or_else(|| self.history.dictation.as_ref().filter(|s| s.id == id))
            .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
            .cloned()
            .ok_or("Open the document before exporting it.")?;
        if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
            session.rows.clone_from(&self.call_rows);
            session.speaker_names.clone_from(&self.speaker_names);
            session.text = calls::text(&session.rows, &session.speaker_names);
        }
        Ok(session)
    }

    /// Patch only the title and personal notes of an open document.
    fn desktop_patch_session(
        &mut self,
        id: &str,
        title: Option<String>,
        personal_notes: Option<String>,
    ) -> Result<(), String> {
        let mut session = self
            .history
            .call
            .as_ref()
            .filter(|s| s.id == id)
            .or_else(|| self.history.dictation.as_ref().filter(|s| s.id == id))
            .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
            .cloned()
            .ok_or("Open the document before editing it.")?;
        if let Some(reason) = self.desktop_edit_blocked(&session) {
            return Err(reason.into());
        }
        if let Some(title) = title {
            let title = title.trim();
            if title.is_empty()
                || title.chars().count() > 160
                || title.chars().any(char::is_control)
            {
                return Err("Use a title from 1 to 160 characters.".into());
            }
            session.title = title.into();
            session.title_is_manual = true;
        }
        if let Some(notes) = personal_notes {
            if notes.len() > 8 * 1024 * 1024 {
                return Err("This note is too long.".into());
            }
            session.personal_notes = notes;
        }
        let worker = self
            .history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?;
        worker.save(session.clone());
        self.brain.remember(&session);
        for current in [
            &mut self.history.call,
            &mut self.history.dictation,
            &mut self.history.selected,
        ]
        .into_iter()
        .flatten()
        {
            if current.id == id {
                current.title.clone_from(&session.title);
                current.title_is_manual = session.title_is_manual;
                current.personal_notes.clone_from(&session.personal_notes);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Action as Request;
    use crate::app::App;
    use crate::history::{Kind, Session, Worker};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    thread_local! {
        /// Paths and text handed to the synthetic file dialogs on this thread.
        static CHOSEN: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
        static SAVED: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }

    fn chosen() -> Option<PathBuf> {
        CHOSEN.with(|path| path.borrow().clone())
    }

    fn act(app: &mut App, value: Value) -> Result<(), String> {
        let request: Request = serde_json::from_value(value).map_err(|e| e.to_string())?;
        app.desktop_action(request)
    }

    /// An app with a history worker in a new temporary directory.
    fn library_app(name: &str) -> (App, PathBuf) {
        let (mut app, _) = crate::app::tests::app();
        let temp = std::env::temp_dir();
        let directory = temp.join(format!(
            "articulate-library-{name}-{}",
            Session::new(Kind::Note).id
        ));
        assert_eq!(directory.parent(), Some(temp.as_path()));
        app.history.worker = Some(Worker::test_directory(directory.join("history")));
        app.history.storage.pickers.save_text = |text, extension, _, _| {
            SAVED.with(|saved| saved.borrow_mut().push((text.into(), extension.into())));
            Ok(true)
        };
        app.history.storage.pickers.backup_destination = |_| Ok(chosen());
        app.history.storage.pickers.backup_source = || Ok(chosen());
        app.history.storage.pickers.folder = |_| Ok(chosen());
        (app, directory)
    }

    fn wait(app: &mut App, what: &str, mut done: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(app) {
            app.history_poll();
            app.poll_saved_search();
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn finish(app: App, directory: &Path) {
        drop(app);
        let _ = std::fs::remove_dir_all(directory);
    }

    fn saved_session(app: &mut App, kind: Kind, text: &str) -> Session {
        let mut session = Session::new(kind);
        session.title = "Quarterly planning".into();
        session.text = text.into();
        app.history.worker.as_ref().unwrap().save(session.clone());
        let id = session.id.clone();
        wait(app, "the saved session", |app| {
            app.history.items.iter().any(|item| item.id == id)
        });
        session
    }

    #[test]
    fn search_reports_highlighted_hits_for_the_current_query_and_filters() {
        let (mut app, directory) = library_app("search");
        let session = saved_session(&mut app, Kind::Note, "We reviewed the café budget together.");
        wait(&mut app, "the search index", |app| {
            app.history
                .worker
                .as_ref()
                .and_then(Worker::search_index)
                .is_some_and(|index| index.is_ready())
        });
        assert!(act(&mut app, json!({"type": "history_search", "query": "café", "filters": {"from_ms": 5, "to_ms": 1}})).unwrap_err().contains("start date"));
        assert!(act(&mut app, json!({"type": "history_search", "query": "café", "filters": {"colour": "red"}})).is_err());
        act(&mut app, json!({"type": "history_search", "query": " café ", "filters": {"kind": "note"}})).unwrap();
        assert_eq!(app.desktop_snapshot()["search"]["working"], true);
        wait(&mut app, "search results", |app| app.saved_search.results.is_some());
        let state = app.desktop_snapshot();
        let search = &state["search"];
        assert_eq!(search["query"], "café");
        assert_eq!(search["filters"]["kind"], "note");
        assert_eq!(search["working"], false);
        assert_eq!(search["results"]["indexed"], true);
        let hit = &search["results"]["hits"][0];
        assert_eq!(hit["session_id"], session.id.as_str());
        let excerpt = hit["excerpt"].as_str().unwrap();
        let range = &hit["matches"][0];
        let (start, end) = (range[0].as_u64().unwrap() as usize, range[1].as_u64().unwrap() as usize);
        assert_eq!(excerpt[start..end].to_lowercase(), "café");
        // Another kind filter finds nothing; clearing removes the results.
        act(&mut app, json!({"type": "history_search", "query": "café", "filters": {"kind": "call"}})).unwrap();
        wait(&mut app, "filtered results", |app| !app.saved_search.working());
        assert_eq!(app.desktop_snapshot()["search"]["results"]["hits"], json!([]));
        act(&mut app, json!({"type": "history_search", "query": ""})).unwrap();
        assert_eq!(app.desktop_snapshot()["search"]["results"], Value::Null);
        finish(app, &directory);
    }

    #[test]
    fn pins_update_the_open_document_and_the_list() {
        let (mut app, directory) = library_app("pin");
        let session = saved_session(&mut app, Kind::Note, "Keep this one.");
        app.history.selected = Some(session.clone());
        act(&mut app, json!({"type": "session_pin", "id": session.id, "pinned": true})).unwrap();
        assert_eq!(app.desktop_snapshot()["selected"]["pinned"], true);
        wait(&mut app, "the pinned summary", |app| {
            app.history.items.iter().any(|item| item.id == session.id && item.pinned)
        });
        assert_eq!(app.desktop_snapshot()["history"][0]["pinned"], true);
        act(&mut app, json!({"type": "session_pin", "id": session.id, "pinned": false})).unwrap();
        wait(&mut app, "the unpinned summary", |app| {
            app.history.items.iter().any(|item| item.id == session.id && !item.pinned)
        });
        finish(app, &directory);
    }

    #[test]
    fn exports_refuse_unavailable_formats_before_opening_a_dialog() {
        let (mut app, directory) = library_app("export");
        let mut dictation = Session::new(Kind::Dictation);
        dictation.title = "Untimed".into();
        dictation.text = "No recorded timing here.".into();
        app.history.selected = Some(dictation.clone());
        let state = app.desktop_snapshot();
        assert!(state["selected"]["subtitles_unavailable"].as_str().unwrap().contains("timing"));
        assert_eq!(state["selected"]["follow_ups"], 0);
        let refused = act(&mut app, json!({"type": "session_export", "id": dictation.id, "format": "srt"})).unwrap_err();
        assert!(refused.contains("timing"), "{refused}");
        assert!(!app.history.storage.dialog_open);
        assert!(act(&mut app, json!({"type": "session_export", "id": "missing", "format": "text"})).is_err());
        act(&mut app, json!({"type": "session_export", "id": dictation.id, "format": "markdown"})).unwrap();
        assert!(app.history.storage.dialog_open);
        app.history_poll();
        assert!(!app.history.storage.dialog_open);
        let (text, extension) = SAVED.with(|saved| saved.borrow().last().cloned()).unwrap();
        assert_eq!(extension, "md");
        assert!(text.starts_with("# Untimed"));
        let notice = &app.desktop_snapshot()["library_notice"];
        assert_eq!(notice["message"], "Exported as Markdown");
        assert_eq!(notice["error"], false);
        let first = notice["sequence"].as_u64().unwrap();
        // A cancelled dialog leaves no notice.
        app.history.storage.pickers.save_text = |_, _, _, _| Ok(false);
        act(&mut app, json!({"type": "session_export", "id": dictation.id, "format": "text"})).unwrap();
        app.history_poll();
        assert_eq!(app.desktop_snapshot()["library_notice"]["sequence"].as_u64(), Some(first));
        finish(app, &directory);
    }

    #[test]
    fn follow_ups_export_only_kept_action_items() {
        use crate::brain::{Draft, Item, Kind as ItemKind};
        let (mut app, directory) = library_app("followups");
        let mut call = Session::new(Kind::Call);
        call.title = "Launch sync".into();
        let item = |kind, text: &str| Item { kind, text: text.into(), section: 1, sources: vec![] };
        call.generated_summary = Some(Draft {
            schema: 1,
            source_id: call.id.clone(),
            source_hash: String::new(),
            model: String::new(),
            sections: 1,
            elapsed_ms: 0,
            items: vec![
                item(ItemKind::Fact, "Launch is on Friday."),
                item(ItemKind::Action, "Send the release notes, then plan; review."),
            ],
            title: None,
        });
        app.history.selected = Some(call.clone());
        assert!(act(&mut app, json!({"type": "followups_export", "id": call.id, "format": "markdown"})).unwrap_err().contains("no follow-ups"));
        call.personal_notes = "Launch is on Friday.\n\nSend the release notes, then plan; review.".into();
        app.history.selected = Some(call.clone());
        assert_eq!(app.desktop_snapshot()["selected"]["follow_ups"], 1);
        act(&mut app, json!({"type": "followups_export", "id": call.id, "format": "markdown"})).unwrap();
        app.history_poll();
        let (text, extension) = SAVED.with(|saved| saved.borrow().last().cloned()).unwrap();
        assert_eq!((text.as_str(), extension.as_str()), ("- [ ] Send the release notes, then plan; review.\n", "md"));
        assert!(act(&mut app, json!({"type": "followups_export", "id": call.id, "format": "markdown", "reminder_ms": 1})).is_err());
        assert!(act(&mut app, json!({"type": "followups_export", "id": call.id, "format": "ics", "reminder_ms": 1})).unwrap_err().contains("future"));
        let reminder = crate::history::now_ms() + 86_400_000;
        act(&mut app, json!({"type": "followups_export", "id": call.id, "format": "ics", "reminder_ms": reminder})).unwrap();
        app.history_poll();
        let (text, extension) = SAVED.with(|saved| saved.borrow().last().cloned()).unwrap();
        assert_eq!(extension, "ics");
        assert!(text.contains("SUMMARY:Send the release notes\\, then plan\\; review."));
        assert!(text.contains("DUE:"));
        assert_eq!(app.desktop_snapshot()["library_notice"]["message"], "Exported 1 follow-up");
        finish(app, &directory);
    }

    #[test]
    fn turning_retention_on_requires_a_preview_of_the_same_policy() {
        let (mut app, directory) = library_app("retention");
        saved_session(&mut app, Kind::Dictation, "Recent words.");
        let policy = json!({"enabled": true, "days": 30, "dictations": true, "calls": false, "notes": false});
        assert!(act(&mut app, json!({"type": "retention_save", "policy": policy})).unwrap_err().contains("Review"));
        assert!(act(&mut app, json!({"type": "retention_preview", "policy": {"days": 0}})).is_err());
        act(&mut app, json!({"type": "retention_preview", "policy": policy})).unwrap();
        assert_eq!(app.desktop_snapshot()["storage"]["retention_preview_working"], true);
        wait(&mut app, "the retention preview", |app| app.history.retention_preview.is_some());
        let state = app.desktop_snapshot();
        assert_eq!(state["storage"]["retention_preview"]["total"], 0);
        assert_eq!(state["storage"]["retention_preview"]["policy"], policy);
        let mut other = policy.clone();
        other["days"] = json!(31);
        assert!(act(&mut app, json!({"type": "retention_save", "policy": other})).is_err());
        act(&mut app, json!({"type": "retention_save", "policy": policy})).unwrap();
        assert!(app.settings.retention.enabled);
        assert_eq!(app.settings.retention.days, 30);
        assert_eq!(app.desktop_snapshot()["storage"]["retention"], policy);
        assert!(app.history.retention_preview.is_none());
        // Turning it off needs no preview.
        act(&mut app, json!({"type": "retention_save", "policy": {"enabled": false, "days": 30}})).unwrap();
        assert!(!app.settings.retention.enabled);
        finish(app, &directory);
    }

    #[test]
    fn encryption_changes_only_through_its_action_and_reports_progress() {
        let (mut app, directory) = library_app("encryption");
        saved_session(&mut app, Kind::Note, "Private words.");
        assert!(act(&mut app, json!({"type": "settings_patch", "encrypt_at_rest": true})).is_err());
        act(&mut app, json!({"type": "storage_encryption", "enabled": true})).unwrap();
        assert!(app.settings.encrypt_at_rest);
        assert_eq!(app.desktop_snapshot()["storage"]["encryption_working"], true);
        assert!(act(&mut app, json!({"type": "storage_encryption", "enabled": false})).is_err());
        wait(&mut app, "encryption", |app| !app.history.storage.encryption_working);
        let result = &app.desktop_snapshot()["storage"]["encryption_result"];
        assert_eq!(result["protect"], true);
        assert_eq!(result["error"], Value::Null);
        act(&mut app, json!({"type": "storage_encryption", "enabled": false})).unwrap();
        wait(&mut app, "decryption", |app| !app.history.storage.encryption_working);
        assert!(!app.settings.encrypt_at_rest);
        finish(app, &directory);
    }

    #[test]
    fn backup_restore_and_markdown_export_use_the_chosen_locations() {
        let (mut app, directory) = library_app("backup");
        saved_session(&mut app, Kind::Note, "Back me up.");
        std::fs::create_dir_all(&directory).unwrap();
        let archive = directory.join("library.articulate-backup");
        // A cancelled dialog leaves everything idle.
        CHOSEN.with(|path| *path.borrow_mut() = None);
        act(&mut app, json!({"type": "backup_create"})).unwrap();
        assert_eq!(app.desktop_snapshot()["storage"]["backup"], "choosing");
        app.history_poll();
        assert_eq!(app.desktop_snapshot()["storage"]["backup"], "idle");
        assert!(act(&mut app, json!({"type": "backup_create", "passphrase": "short"})).unwrap_err().contains("8 characters"));
        CHOSEN.with(|path| *path.borrow_mut() = Some(archive.clone()));
        act(&mut app, json!({"type": "backup_create", "passphrase": "correct horse"})).unwrap();
        app.history_poll();
        assert_eq!(app.desktop_snapshot()["storage"]["backup"], "working");
        assert!(act(&mut app, json!({"type": "history_export_markdown"})).is_err(), "one operation at a time");
        wait(&mut app, "the backup", |app| app.history.backup.is_some());
        let state = app.desktop_snapshot();
        let backup = &state["storage"]["backup_result"];
        assert_eq!(backup["error"], Value::Null, "{backup}");
        assert_eq!(backup["encrypted"], true);
        assert_eq!(backup["sessions"], 1);
        assert!(archive.is_file());

        let encrypted = app.settings.encrypt_at_rest;
        act(&mut app, json!({"type": "backup_restore", "passphrase": "wrong passphrase", "replace_older": false, "include_settings": true})).unwrap();
        app.history_poll();
        wait(&mut app, "the failed restore", |app| app.history.restored.is_some());
        assert!(app.desktop_snapshot()["storage"]["restore_result"]["error"].is_string());
        act(&mut app, json!({"type": "backup_restore", "passphrase": "correct horse", "replace_older": false, "include_settings": true})).unwrap();
        app.history_poll();
        wait(&mut app, "the restore", |app| app.history.restored.is_some());
        let state = app.desktop_snapshot();
        let restored = &state["storage"]["restore_result"];
        assert_eq!(restored["error"], Value::Null, "{restored}");
        assert_eq!(restored["sessions_in_archive"], 1);
        assert_eq!(restored["settings_restored"], true);
        assert_eq!(restored["model_reload"], false);
        assert_eq!(app.settings.encrypt_at_rest, encrypted);

        let folder = directory.join("markdown");
        std::fs::create_dir_all(&folder).unwrap();
        CHOSEN.with(|path| *path.borrow_mut() = Some(folder.clone()));
        act(&mut app, json!({"type": "history_export_markdown"})).unwrap();
        app.history_poll();
        wait(&mut app, "the Markdown export", |app| app.history.markdown_export.is_some());
        let state = app.desktop_snapshot();
        assert_eq!(state["storage"]["markdown_result"]["files"], 1);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        finish(app, &directory);
    }

    #[test]
    fn quarantine_actions_need_a_listed_file() {
        let (mut app, directory) = library_app("quarantine");
        act(&mut app, json!({"type": "quarantine_list"})).unwrap();
        wait(&mut app, "the isolated files", |app| app.history.storage.quarantine_listed);
        let state = app.desktop_snapshot();
        assert_eq!(state["recovery"]["listed"], true);
        assert_eq!(state["recovery"]["quarantine"], json!([]));
        for kind in ["quarantine_restore", "quarantine_export", "quarantine_discard"] {
            assert!(
                act(&mut app, json!({"type": kind, "file_name": "missing.corrupt-1"}))
                    .unwrap_err()
                    .contains("no longer isolated")
            );
        }
        finish(app, &directory);
    }
}
