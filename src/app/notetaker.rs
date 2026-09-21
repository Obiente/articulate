use super::*;
use crate::history::{Kind, Session, Summary};

#[derive(Default)]
pub(super) struct State {
    pending: std::collections::VecDeque<PendingFiling>,
    routing: Option<Routing>,
    pub moving: bool,
    pub status: String,
}

struct PendingFiling {
    session: Session,
    summarized: bool,
    retry_at: Option<Instant>,
}

struct Routing {
    id: String,
    events: Receiver<Result<crate::topics::Destination, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Routing {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl App {
    pub(super) fn notetaker_source_moved(&mut self, moved: crate::topics::Moved) {
        self.notetaker.moving = false;
        let was_filed = moved.previous_topic_id.is_some();
        self.notetaker_forget_filing(&moved.source.id);
        for current in [
            &mut self.history.call,
            &mut self.history.dictation,
            &mut self.history.selected,
        ]
        .into_iter()
        .flatten()
        {
            if current.id == moved.source.id {
                current.clone_from(&moved.source);
            }
        }
        self.history.items.retain(|item| item.id != moved.source.id);
        self.history.items.insert(0, Summary::from(&moved.source));
        self.notetaker_refresh_collections(moved.collections);
        self.notetaker.status = if moved.source.topic_id.is_some() && was_filed {
            "Moved to its new topic. Your original recording is kept."
        } else if moved.source.topic_id.is_some() {
            "Filed into its topic. You can move this recording at any time."
        } else {
            "This recording is now kept separately."
        }
        .into();
        if let Some(worker) = &self.history.worker {
            worker.list();
        }
    }

    pub(super) fn notetaker_refresh_collections(&mut self, collections: Vec<Session>) {
        for mut collection in collections {
            self.brain.forget(&collection.id);
            if collection.sources.is_empty() {
                if let Some(previous) = collection.generated_summary.take() {
                    collection.personal_notes = collection
                        .personal_notes
                        .split("\n\n")
                        .filter(|paragraph| {
                            !previous.items.iter().any(|item| item.text == *paragraph)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                }
            } else {
                self.brain_queue_session(collection.clone());
            }
            // Persist pruning immediately even when the model is not installed.
            if let Some(worker) = &self.history.worker {
                worker.save(collection.clone());
            }
            if self
                .history
                .selected
                .as_ref()
                .is_some_and(|selected| selected.id == collection.id)
            {
                self.history.selected = Some(collection.clone());
            }
            self.history.items.retain(|item| item.id != collection.id);
            self.history.items.insert(0, Summary::from(&collection));
        }
    }

    pub(super) fn notetaker_routing_busy(&self) -> bool {
        self.notetaker.routing.is_some() || self.notetaker.moving
    }

    pub(super) fn notetaker_queue_filing(&mut self, session: Session) {
        if session.is_collection || session.topic_id.is_some() || session.text.trim().is_empty() {
            return;
        }
        if self
            .notetaker
            .pending
            .iter()
            .any(|item| item.session.id == session.id)
        {
            return;
        }
        if self.notetaker.pending.len() >= 32 {
            self.notetaker.status =
                "This recording is saved separately. Open it to file it into a topic.".into();
            return;
        }
        let summarized = self.brain_source_current(&session)
            || session
                .generated_summary
                .as_ref()
                .is_some_and(|draft| draft.validate(&super::brain_ui::source(&session)).is_ok());
        self.notetaker.pending.push_back(PendingFiling {
            session,
            summarized,
            retry_at: None,
        });
        if !crate::polish::profile_installed(crate::polish::ModelProfile::Summary) {
            self.notetaker.status = "Saved separately. Download the notes model in Settings to organize your spoken notes.".into();
        }
    }

    pub(super) fn notetaker_summary_ready(&mut self, session: &Session) {
        if let Some(pending) = self
            .notetaker
            .pending
            .iter_mut()
            .find(|p| p.session.id == session.id)
        {
            pending.session = session.clone();
            pending.summarized = true;
        }
    }

    pub(super) fn notetaker_forget_filing(&mut self, id: &str) {
        self.notetaker.pending.retain(|p| p.session.id != id);
        if self
            .notetaker
            .routing
            .as_ref()
            .is_some_and(|job| job.id == id)
        {
            self.notetaker.routing = None;
        }
    }

    pub(super) fn notetaker_poll_filing(&mut self) {
        if let Some(job) = &self.notetaker.routing {
            let result = match job.events.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Topic filing stopped. Your recording is saved separately.".into(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                let id = job.id.clone();
                self.notetaker.routing = None;
                match result {
                    Ok(destination) => {
                        self.history_save_personal_notes();
                        if let Some(pending) =
                            self.notetaker.pending.iter().find(|p| p.session.id == id)
                            && let Some(worker) = &self.history.worker
                        {
                            self.notetaker.moving = true;
                            worker.move_source(
                                id.clone(),
                                destination,
                                pending.session.topic_id.clone(),
                            );
                        }
                        self.notetaker.pending.retain(|p| p.session.id != id);
                        self.notetaker.status = "Saving your recording into its topic…".into();
                    }
                    Err(error) => {
                        self.notetaker.status = error;
                        if let Some(pending) = self
                            .notetaker
                            .pending
                            .iter_mut()
                            .find(|p| p.session.id == id)
                        {
                            pending.retry_at = Some(Instant::now() + Duration::from_secs(45));
                        }
                    }
                }
            }
        }
        if self.notetaker_routing_busy()
            || self.history.worker.is_none()
            || self.brain_working()
            || self.polish_working()
            || self.call.is_some()
            || self.recording.is_some()
            || self.busy
            || self.loading
            || !crate::polish::profile_installed(crate::polish::ModelProfile::Summary)
        {
            return;
        }
        let Some(pending) = self
            .notetaker
            .pending
            .iter()
            .find(|p| p.retry_at.is_none_or(|at| at <= Instant::now()))
        else {
            return;
        };
        if !pending.summarized {
            self.brain_queue_session(pending.session.clone());
            return;
        }
        let session = pending.session.clone();
        let candidates = self
            .history
            .items
            .iter()
            .filter(|item| item.can_receive_sources)
            .take(crate::topics::MAX_CANDIDATES)
            .map(|item| crate::topics::Candidate {
                id: item.id.clone(),
                title: item.title.clone(),
                preview: item.preview.clone(),
            })
            .collect::<Vec<_>>();
        let (sender, events) = mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_cancel = cancel.clone();
        let id = session.id.clone();
        match std::thread::Builder::new()
            .name("note-topic-filing".into())
            .spawn(move || {
                let result = crate::brain::route_topic(&session, &candidates, &task_cancel)
                    .map_err(|e| e.to_string());
                let _ = sender.send(result);
            }) {
            Ok(_) => {
                self.notetaker.routing = Some(Routing { id, events, cancel });
                self.notetaker.status = "Finding the right topic for your note…".into();
            }
            Err(error) => self.notetaker.status = error.to_string(),
        }
    }

    pub(super) fn is_note_capture(&self) -> bool {
        self.history
            .call
            .as_ref()
            .is_some_and(|s| s.kind == Kind::Note)
    }

    pub(super) fn notetaker_toggle_capture(&mut self) -> Result<(), String> {
        if self.call.is_some() && self.is_note_capture() {
            return self.notetaker_stop_capture();
        }
        self.notetaker_start_capture()
    }

    pub(super) fn notetaker_start_capture(&mut self) -> Result<(), String> {
        if !self.ready || self.loading || self.downloading.is_some() {
            return Err("Wait for the speech model before recording a note.".into());
        }
        if self.recording.is_some() || self.call.is_some() || self.busy || self.preview_inflight {
            return Err("Finish the current recording before starting a spoken note.".into());
        }
        self.history_save_personal_notes();
        if !self.start_stream_capture(true) {
            return Err(self.call_status.clone());
        }
        self.history.selected = self.history.call.clone();
        self.history.open_requested = None;
        self.history.notetaker_tab = 0;
        self.page = 6;
        Ok(())
    }

    pub(super) fn notetaker_stop_capture(&mut self) -> Result<(), String> {
        if !self.is_note_capture() {
            return Err("There is no spoken note recording to finish.".into());
        }
        if let Some(control) = &self.call {
            control.stop();
            self.call_status = "Finishing your spoken note…".into();
        }
        Ok(())
    }

    pub(super) fn notetaker_hub(&mut self) {
        self.history_save_personal_notes();
        self.history.selected = None;
        self.history.open_requested = None;
        self.history.renaming = false;
        self.history.confirm_delete = false;
        self.page = 6;
    }

    pub(super) fn notetaker_new_note(&mut self) {
        self.history_save_personal_notes();
        let mut session = Session::new(Kind::Note);
        session.title = "Untitled note".into();
        self.history.selected = Some(session);
        self.history.selected_dirty = Some(Instant::now());
        self.history.open_requested = None;
        self.history.notetaker_tab = 0;
        self.history.detail_search.clear();
        self.history.notice.clear();
        self.history.confirm_delete = false;
        self.page = 6;
        self.history_save_personal_notes();
    }

    pub(super) fn notetaker_open(&mut self, id: String) {
        self.history_save_personal_notes();
        self.history.detail_search.clear();
        self.history.notice.clear();
        self.history.confirm_delete = false;
        if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
            self.notetaker_return_to_capture();
            return;
        }
        self.history.notetaker_tab = 0;
        if let Some(session) = &self.history.call
            && session.id == id
        {
            self.history.selected = Some(session.clone());
            self.history.open_requested = None;
            return;
        }
        self.history.selected = None;
        self.history.loading = true;
        self.history.open_requested = Some(id.clone());
        if let Some(worker) = &self.history.worker {
            worker.load(id);
        }
    }

    pub(super) fn notetaker_return_to_capture(&mut self) {
        self.history_save_personal_notes();
        self.history.open_requested = None;
        self.page = 3;
        self.call_tab = 3;
    }

    /// Mirror personal edits immediately, before a concurrent ASR snapshot can
    /// enqueue its next save. Transcript fields remain independent.
    #[allow(
        dead_code,
        reason = "Document editing and complete note export retained for the Tauri workspace"
    )]
    fn notetaker_changed(&mut self, session: &Session) {
        self.brain.remember(session);
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.personal_notes.clone_from(&session.personal_notes);
            current
                .generated_summary
                .clone_from(&session.generated_summary);
            current
                .protected_note_items
                .clone_from(&session.protected_note_items);
            current.title.clone_from(&session.title);
            current.title_is_manual = session.title_is_manual;
            self.history_call_changed();
        }
        self.history.selected_dirty.get_or_insert_with(Instant::now);
    }

    pub(super) fn history_save_personal_notes(&mut self) {
        if self.history.selected_dirty.is_none() {
            return;
        }
        let Some(session) = self.history.selected.clone() else {
            self.history.selected_dirty = None;
            return;
        };
        self.history.selected_dirty = None;
        self.brain.remember(&session);
        if self.history.pending_deletions.contains(&session.id) {
            return;
        }
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.personal_notes.clone_from(&session.personal_notes);
            current
                .generated_summary
                .clone_from(&session.generated_summary);
            current
                .protected_note_items
                .clone_from(&session.protected_note_items);
            current.title.clone_from(&session.title);
            current.title_is_manual = session.title_is_manual;
            self.history_save_call();
            self.history.selected = self.history.call.clone();
        } else if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
        self.history.items.retain(|item| item.id != session.id);
        self.history.items.insert(0, Summary::from(&session));
    }
}

#[allow(
    dead_code,
    reason = "Document editing and complete note export retained for the Tauri workspace"
)]
fn full_note(session: &Session) -> String {
    let mut text = format!(
        "# {}\n\n{}\n\n## Notes\n\n{}\n",
        session.title,
        full_time(session.created_ms),
        session.personal_notes
    );
    // Older sessions can have a saved summary without a notes document.
    // Once the document exists, it is the authoritative edited version.
    if session.personal_notes.trim().is_empty()
        && let Some(summary) = &session.generated_summary
    {
        let stale = summary.validate(&super::brain_ui::source(session)).is_err();
        text.push_str(&format!(
            "\n## Summary{}\n\n{}\n",
            if stale {
                " (earlier transcript version)"
            } else {
                ""
            },
            super::brain_ui::summary_text(summary)
        ));
    }
    if let Some(notes) = &session.notes {
        text.push_str(&format!(
            "\n## Highlights\n\n{}\n",
            notes.text(&session.speaker_names)
        ));
    }
    if !session.text.is_empty() {
        text.push_str(&format!("\n## Transcript\n\n{}\n", session.text));
    }
    text
}
#[allow(
    dead_code,
    reason = "Document editing and complete note export retained for the Tauri workspace"
)]
fn local_time(ms: u64) -> time::OffsetDateTime {
    let utc = time::OffsetDateTime::from_unix_timestamp((ms / 1000).min(i64::MAX as u64) as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    utc.to_offset(time::UtcOffset::local_offset_at(utc).unwrap_or(time::UtcOffset::UTC))
}
#[allow(
    dead_code,
    reason = "Document editing and complete note export retained for the Tauri workspace"
)]
fn full_time(ms: u64) -> String {
    let time = local_time(ms);
    format!(
        "{:04}-{:02}-{:02} · {:02}:{:02}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_before_navigation_refreshes_active_summary_owner() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Latest user edit".into();
        // The owner-cache regression is exercised through its public test helper.
        app.prepare_active_summary_owner_for_test(&session);
        session.personal_notes = "Revised immediately before leaving".into();
        app.history.selected = Some(session.clone());
        app.history.selected_dirty = Some(Instant::now());
        app.notetaker_hub();
        assert_eq!(
            app.active_summary_owner_notes_for_test(&session.id),
            session.personal_notes
        );
    }

    #[test]
    fn personal_note_needs_no_model_or_audio_and_does_not_touch_dictation() {
        let (mut app, commands) = super::super::tests::app();
        app.ready = false;
        app.text = "Unrelated dictation".into();
        app.notetaker_new_note();
        assert_eq!(app.page, 6);
        assert_eq!(app.history.selected.as_ref().unwrap().kind, Kind::Note);
        assert!(app.call.is_none());
        assert!(app.recording.is_none());
        assert_eq!(app.text, "Unrelated dictation");
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn spoken_note_uses_only_microphone_and_keeps_dictation_separate() {
        let (mut app, commands) = super::super::tests::app();
        app.preview_inflight = false;
        app.settings.audio_feedback = false;
        app.settings.discord_companion = true;
        app.settings.insert = true;
        app.settings.output = Some("Unrelated output".into());
        app.text = "An earlier dictation".into();
        app.notetaker_start_capture().unwrap();
        let Command::Call(request, _) = commands.try_recv().unwrap() else {
            panic!("Spoken notes must use streaming capture");
        };
        assert!(request.microphone_only);
        assert!(!request.native_audio);
        assert!(request.output.is_none());
        assert!(app.target.is_none());
        assert!(app.recording.is_none());
        assert!(app.is_note_capture());
        assert_eq!(app.history.selected.as_ref().unwrap().kind, Kind::Note);
        assert_eq!(app.text, "An earlier dictation");
        app.notetaker_toggle_capture().unwrap();
        assert!(request.control.stop_ns.load(Ordering::SeqCst) != 0);
    }

    #[test]
    fn spoken_note_source_updates_leave_written_document_untouched() {
        let (mut app, _) = super::super::tests::app();
        let mut note = Session::new(Kind::Note);
        note.title = "Spoken note".into();
        note.personal_notes = "A reminder I wrote myself.".into();
        app.history.call = Some(note.clone());
        app.history.selected = Some(note.clone());
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: Vec::new(),
            discord: None,
            text: "I want to plan a workshop.".into(),
        });
        app.history_save_call();
        let saved = app.history.call.as_ref().unwrap();
        assert_eq!(saved.personal_notes, note.personal_notes);
        assert_eq!(saved.original, saved.text);
        assert!(saved.text.contains("plan a workshop"));
        assert_eq!(app.history.selected.as_ref().unwrap().rows.len(), 1);
    }

    #[test]
    fn quick_note_cannot_start_over_another_capture() {
        let (mut app, commands) = super::super::tests::app();
        app.call = Some(call_capture::Control::new());
        app.history.call = Some(Session::new(Kind::Call));
        assert!(app.notetaker_toggle_capture().is_err());
        assert!(commands.try_recv().is_err());
        assert_eq!(app.history.call.as_ref().unwrap().kind, Kind::Call);
    }

    #[test]
    fn empty_topic_after_move_keeps_manual_paragraphs_and_source_identity() {
        let (mut app, _) = super::super::tests::app();
        let mut collection = Session::new(Kind::Note);
        collection.is_collection = true;
        collection.title = "Workshop".into();
        collection.personal_notes = "Generated workshop detail.\n\nMy own reminder.".into();
        collection.generated_summary = Some(crate::brain::Draft {
            title: None,
            schema: 1,
            source_id: collection.id.clone(),
            source_hash: "synthetic".into(),
            model: crate::brain::MODEL_LABEL.into(),
            sections: 1,
            elapsed_ms: 1,
            items: vec![crate::brain::Item {
                kind: crate::brain::Kind::Fact,
                text: "Generated workshop detail.".into(),
                section: 1,
                sources: Vec::new(),
            }],
        });
        app.history.selected = Some(collection.clone());
        let mut source = Session::new(Kind::Note);
        source.text = "Original independent recording.".into();
        source.original = source.text.clone();
        app.history.call = Some(source.clone());
        app.notetaker_source_moved(crate::topics::Moved {
            source: source.clone(),
            previous_topic_id: Some(collection.id.clone()),
            collections: vec![collection],
        });
        let selected = app.history.selected.as_ref().unwrap();
        assert_eq!(selected.personal_notes, "My own reminder.");
        assert!(selected.generated_summary.is_none());
        assert_eq!(app.history.call.as_ref().unwrap().id, source.id);
        assert_eq!(app.history.call.as_ref().unwrap().original, source.original);
    }

    #[test]
    fn deleting_queued_source_prevents_later_automatic_filing() {
        let (mut app, _) = super::super::tests::app();
        let mut source = Session::new(Kind::Note);
        source.text = "Save this thought separately.".into();
        app.notetaker_queue_filing(source.clone());
        assert_eq!(app.notetaker.pending.len(), 1);
        app.notetaker_forget_filing(&source.id);
        assert!(app.notetaker.pending.is_empty());
    }

    #[test]
    fn returning_to_active_note_does_not_restart_or_stop_capture() {
        let (mut app, commands) = super::super::tests::app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Keep this thought.".into();
        app.history.call = Some(session);
        app.notetaker_hub();
        app.notetaker_return_to_capture();
        assert_eq!(app.page, 3);
        assert_eq!(app.call_tab, 3);
        assert!(std::sync::Arc::ptr_eq(app.call.as_ref().unwrap(), &control));
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(
            app.history.call.as_ref().unwrap().personal_notes,
            "Keep this thought."
        );
    }

    #[test]
    fn active_capture_snapshots_preserve_new_personal_writing() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Old thought".into();
        app.history.call = Some(session.clone());
        session.personal_notes = "Revised personal thought".into();
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "Words from the conversation.".into(),
        });
        app.history_save_call();
        app.history_save_personal_notes();
        let saved = app.history.call.as_ref().unwrap();
        assert_eq!(saved.personal_notes, "Revised personal thought");
        assert_eq!(saved.rows.len(), 1);
        assert!(saved.text.contains("Words from the conversation."));
        assert_eq!(
            app.history.selected.as_ref().unwrap().personal_notes,
            saved.personal_notes
        );
    }

    #[test]
    fn reviewed_summary_survives_call_snapshot_and_personal_note_save() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.generated_summary = Some(crate::brain::Draft {
            title: None,
            schema: 1,
            source_id: session.id.clone(),
            source_hash: "synthetic".into(),
            model: crate::brain::MODEL_LABEL.into(),
            sections: 0,
            elapsed_ms: 1,
            items: vec![],
        });
        let mut previous = session.clone();
        previous.generated_summary = None;
        app.history.call = Some(previous);
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.history_save_call();
        app.history_save_personal_notes();
        assert!(
            app.history
                .call
                .as_ref()
                .unwrap()
                .generated_summary
                .is_some()
        );
        assert!(
            app.history
                .selected
                .as_ref()
                .unwrap()
                .generated_summary
                .is_some()
        );
    }

    #[test]
    fn full_export_keeps_saved_notes_and_legacy_excerpts() {
        let mut session = Session::new(Kind::Call);
        session.title = "Planning".into();
        session.personal_notes = "A private idea not spoken.".into();
        session.rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "I will send the update tomorrow.".into(),
        });
        session.text = calls::text(&session.rows, &session.speaker_names);
        session.notes = Some(crate::notes::Notes::build(&session.rows));
        assert_eq!(session.personal_notes, "A private idea not spoken.");
        let exported = full_note(&session);
        assert!(exported.contains("## Notes\n\nA private idea not spoken."));
        assert!(exported.contains("## Transcript"));
        assert!(exported.contains("## Highlights"));
    }
    #[test]
    fn edited_document_exports_without_duplicate_generated_baseline() {
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "The updated plan, including my edits.".into();
        session.generated_summary = Some(crate::brain::Draft {
            title: None,
            schema: 1,
            source_id: session.id.clone(),
            source_hash: "synthetic".into(),
            model: crate::brain::MODEL_LABEL.into(),
            sections: 0,
            elapsed_ms: 1,
            items: vec![],
        });
        let exported = full_note(&session);
        assert!(exported.contains(&session.personal_notes));
        assert!(!exported.contains("## Summary"));
    }

    #[test]
    fn active_note_edits_and_later_transcript_survive_restart_in_order() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Original thought".into();
        let id = session.id.clone();
        let temp = std::env::temp_dir();
        let directory = temp.join(format!("articulate-notetaker-test-{id}"));
        assert_eq!(directory.parent(), Some(temp.as_path()));
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        app.history.call = Some(session.clone());
        app.history_save_call();
        session.personal_notes = "My revised thought, independent of speech.".into();
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "New words from the call.".into(),
        });
        // The ASR autosave can run before the editor debounce expires.
        app.history_save_call();
        app.notetaker_hub();
        drop(app);
        let reopened = crate::history::History::open(directory.clone())
            .unwrap()
            .load(&id)
            .unwrap();
        assert_eq!(
            reopened.personal_notes,
            "My revised thought, independent of speech."
        );
        assert_eq!(reopened.rows[0].text, "New words from the call.");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
