use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spelling {
    heard: String,
    wanted: String,
    #[serde(default)]
    cues: String,
}

impl App {
    pub(super) fn edit_saved_dictation(
        &mut self,
        id: &str,
        expected: &str,
        text: String,
        spelling: Option<Spelling>,
    ) -> Result<(), String> {
        if self.history.worker.is_none() {
            return Err("Local history is unavailable.".into());
        }
        if self.notetaker_routing_busy() || self.history.pending_deletions.contains(id) {
            return Err("Wait for this dictation to finish saving or moving.".into());
        }
        let latest = self.history.dictation.as_ref().is_some_and(|s| s.id == id);
        if latest
            && (self.recording.is_some()
                || self.busy
                || self.preview_inflight
                || self.integration_inflight
                || self.integration_pending.is_some())
        {
            return Err("Finish dictating before editing the text.".into());
        }
        let mut session = self
            .history
            .dictation
            .as_ref()
            .filter(|s| s.id == id)
            .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
            .cloned()
            .ok_or("Open the dictation before editing it.")?;
        if session.kind != crate::history::Kind::Dictation
            || !session.rows.is_empty()
            || session.is_collection
        {
            return Err("Choose a finished dictation to edit.".into());
        }
        if session.text != expected || (latest && self.text != expected) {
            return Err("This dictation changed. Reopen the editor to use its latest text.".into());
        }
        if text.trim().is_empty() || text.len() > 8 * 1024 * 1024 || text.contains('\0') {
            return Err("Enter some text, up to 8 MB.".into());
        }
        // Remember only an explicitly reviewed spelling. General prose edits
        // must not silently become global vocabulary replacements.
        if let Some(spelling) = spelling {
            let mut entry = dictionary::validate(&spelling.heard, &spelling.wanted)
                .map_err(|e| e.to_string())?;
            if !expected.contains(&entry.heard) || !text.contains(&entry.wanted) {
                return Err("The spelling must appear in the original and corrected text.".into());
            }
            entry.cues = dictionary::cue_words(&spelling.cues).map_err(|e| e.to_string())?;
            let previous = self.settings.entries.clone();
            learning::Change::apply(&mut self.settings.entries, entry);
            if let Err(error) = self.save_preferences() {
                self.settings.entries = previous;
                return Err(error.to_string());
            }
        }
        if session.original.is_empty() {
            session.original.clone_from(&session.text);
        }
        session.text = text;
        // Original recognition, timestamps, manually written notes and metrics
        // remain independent of the user's corrected transcript.
        self.brain.remember(&session);
        if latest {
            self.text.clone_from(&session.text);
            self.raw.clone_from(&session.original);
            self.edit_baseline.clone_from(&session.text);
            self.edited_at = None;
            self.history.dictation_dirty = None;
            self.history.dictation = Some(session.clone());
        }
        if let Some(selected) = &mut self.history.selected
            && selected.id == id
        {
            selected.text.clone_from(&session.text);
            selected.original.clone_from(&session.original);
        }
        self.history.worker.as_ref().unwrap().save(session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_preserves_original_and_other_dictations_and_rejects_stale_saves() {
        let (mut app, _) = super::super::tests::app();
        app.history.worker = Some(crate::history::Worker::start());
        let mut session = crate::history::Session::new(crate::history::Kind::Dictation);
        session.text = "Start with a kit. Considering a kit is a sample project.".into();
        session.original = session.text.clone();
        let original = session.text.clone();
        let id = session.id.clone();
        app.history.selected = Some(session.clone());
        app.history.dictation = Some(session);
        app.text = original.clone();
        let corrected = "Start with AcmeKit. Considering AcmeKit is a sample project.";
        assert!(
            app.edit_saved_dictation(&id, &original, corrected.into(), None)
                .is_err()
        );
        assert_eq!(app.text, original);
        app.preview_inflight = false;
        app.edit_saved_dictation(
            &id,
            &original,
            corrected.into(),
            Some(Spelling {
                heard: "a kit".into(),
                wanted: "AcmeKit".into(),
                cues: "sample".into(),
            }),
        )
        .unwrap();
        assert_eq!(app.text, corrected);
        assert_eq!(app.history.selected.as_ref().unwrap().original, original);
        assert_eq!(app.history.dictation.as_ref().unwrap().text, corrected);
        assert_eq!(app.settings.entries.len(), 1);
        assert!(
            app.edit_saved_dictation(&id, &original, "Stale draft".into(), None)
                .is_err()
        );
        assert_eq!(app.text, corrected);
        let mut another = crate::history::Session::new(crate::history::Kind::Dictation);
        another.text = "Another dictation.".into();
        app.history.dictation = Some(another);
        app.text = "Another dictation.".into();
        app.edit_saved_dictation(&id, corrected, "Corrected older dictation.".into(), None)
            .unwrap();
        assert_eq!(app.text, "Another dictation.");
        assert_eq!(app.history.selected.as_ref().unwrap().original, original);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app
            .history
            .worker
            .as_ref()
            .unwrap()
            .saves_settled()
            .unwrap()
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let store =
            crate::history::History::open(crate::model::data_dir().join("history")).unwrap();
        let restored = store.load(&id).unwrap();
        assert_eq!(restored.text, "Corrected older dictation.");
        assert_eq!(restored.original, original);
    }

    #[test]
    fn confirmed_spelling_extends_existing_context_without_duplicates() {
        let (mut app, _) = super::super::tests::app();
        app.history.worker = Some(crate::history::Worker::start());
        let mut entry = dictionary::validate("a kit", "AcmeKit").unwrap();
        entry.cues = vec!["sample".into()];
        app.settings.entries.push(entry);
        let mut session = crate::history::Session::new(crate::history::Kind::Dictation);
        session.text = "The project is a kit.".into();
        let id = session.id.clone();
        app.history.selected = Some(session);
        app.edit_saved_dictation(
            &id,
            "The project is a kit.",
            "The project is AcmeKit.".into(),
            Some(Spelling {
                heard: "a kit".into(),
                wanted: "AcmeKit".into(),
                cues: "project".into(),
            }),
        )
        .unwrap();
        assert_eq!(app.settings.entries.len(), 1);
        assert!(app.settings.entries[0].cues.contains(&"sample".into()));
        assert!(app.settings.entries[0].cues.contains(&"project".into()));
        assert_eq!(
            app.history.selected.as_ref().unwrap().original,
            "The project is a kit."
        );
    }
}
