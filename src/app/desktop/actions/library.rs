//! Saved documents: opening, editing, filing, deleting, notes and insights.

use crate::app::App;
use crate::calls;
use serde::Deserialize;

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
        }
        Ok(())
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
