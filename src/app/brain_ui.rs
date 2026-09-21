use super::App;
use crate::{
    brain,
    classification::{Segment, Transcript},
    history::Session,
    polish::{self, ModelProfile},
};

#[derive(Default)]
pub(super) struct State {
    job: Option<ActiveSummary>,
    live_call: Option<String>,
    final_calls: std::collections::VecDeque<Session>,
    sessions: std::collections::HashMap<String, SessionState>,
    download: Option<polish::Job>,
    download_status: String,
    download_progress: Option<f32>,
}

struct ActiveSummary {
    source_id: String,
    job: brain::Job,
    snapshot: Option<Transcript>,
    automatic: bool,
}

#[derive(Default)]
struct SessionState {
    draft: Option<brain::Draft>,
    pending_save: bool,
    save_acknowledged: bool,
    status: String,
    progress: Option<f32>,
    live_seen: bool,
    spoken_note: bool,
    paused: bool,
    last_attempt: Option<std::time::Instant>,
    retry_pending: bool,
    last_source: Option<Transcript>,
    automatic_ready: bool,
    owner: Option<Session>,
}

impl State {
    pub(super) fn remember(&mut self, session: &Session) {
        if let Some(queued) = self
            .final_calls
            .iter_mut()
            .find(|queued| queued.id == session.id)
        {
            queued.clone_from(session);
        }
        if self.working_for(&session.id) {
            self.sessions.entry(session.id.clone()).or_default().owner = Some(session.clone());
        }
    }

    fn working_for(&self, id: &str) -> bool {
        self.job
            .as_ref()
            .is_some_and(|active| active.source_id == id)
    }

    fn start(&mut self, id: String, job: brain::Job) {
        let session = self.sessions.entry(id.clone()).or_default();
        session.draft = None;
        session.automatic_ready = false;
        session.status = "Preparing the local summary…".into();
        session.progress = None;
        session.retry_pending = false;
        self.job = Some(ActiveSummary {
            source_id: id,
            job,
            snapshot: None,
            automatic: false,
        });
    }

    fn cancel(&mut self, id: &str, message: &str) {
        if self.working_for(id) {
            self.job = None;
            let session = self.sessions.entry(id.into()).or_default();
            session.status = message.into();
            session.last_source = None;
            session.retry_pending = true;
            session.progress = None;
        }
    }

    pub(super) fn forget(&mut self, id: &str) {
        if self.working_for(id) {
            self.job = None;
        }
        self.sessions.remove(id);
        self.final_calls.retain(|session| session.id != id);
    }

    fn queue_final(&mut self, session: Session) {
        let state = self.sessions.entry(session.id.clone()).or_default();
        if state.paused {
            return;
        }
        self.final_calls.retain(|queued| queued.id != session.id);
        // Retain bounded final snapshots only until completed or cancelled.
        if self.final_calls.len() == 8
            && let Some(oldest) = self.final_calls.pop_front()
            && let Some(state) = self.sessions.get_mut(&oldest.id)
        {
            state.status =
                "Automatic notes are busy. Open this conversation and choose Update notes.".into();
        }
        self.final_calls.push_back(session);
    }

    fn final_candidate(&mut self, now: std::time::Instant) -> Option<Session> {
        let mut index = 0;
        while index < self.final_calls.len() {
            let session = &self.final_calls[index];
            let input = source(session);
            let state = self.sessions.entry(session.id.clone()).or_default();
            let complete = state
                .last_source
                .as_ref()
                .is_some_and(|old| old.segments == input.segments)
                && !self
                    .job
                    .as_ref()
                    .is_some_and(|job| job.source_id == session.id);
            if state.paused || complete || input.segments.is_empty() {
                self.final_calls.remove(index);
                continue;
            }
            if notes_due(state, &input, false, now) {
                return Some(session.clone());
            }
            index += 1;
        }
        None
    }

    fn receive(&mut self, id: &str, event: brain::Event) {
        // Only the active job's owner may receive these events, even after navigation.
        if !self.working_for(id) {
            return;
        }
        let session = self.sessions.entry(id.into()).or_default();
        if let brain::Event::Progress { completed, total } = event {
            session.status =
                format!("Summarizing on this device · {completed} of {total} sections");
            session.progress = Some(completed as f32 / total.max(1) as f32);
            return;
        }
        let active = self.job.take().expect("active owner checked");
        session.progress = None;
        match event {
            brain::Event::Complete(draft) if draft.source_id == id => {
                if let Some(snapshot) = active.snapshot.as_ref()
                    && draft.validate(snapshot).is_err()
                {
                    session.last_source = None;
                    session.retry_pending = true;
                    session.status = "The notes did not match their transcript. Try again.".into();
                    return;
                }
                session.automatic_ready = active.automatic;
                session.draft = Some(draft);
                session.status = "Review the summary and its sources before saving.".into();
            }
            brain::Event::Complete(_) => {
                session.last_source = None;
                session.retry_pending = true;
                session.status = "The summary did not match this transcript. Try again.".into()
            }
            brain::Event::Failed(error) => {
                session.last_source = None;
                session.retry_pending = true;
                session.status = error;
            }
            brain::Event::Cancelled => {
                session.last_source = None;
                session.retry_pending = true;
                session.status = "Summary cancelled. Your notes are unchanged.".into()
            }
            brain::Event::Progress { .. } => unreachable!(),
        }
    }

    fn settle_saves(&mut self, settled: bool) {
        if !settled {
            return;
        }
        for session in self.sessions.values_mut() {
            if session.pending_save && session.save_acknowledged {
                session.pending_save = false;
                session.save_acknowledged = false;
                if session.status == "Saving your reviewed summary…" {
                    session.status = "Summary saved on this device.".into();
                }
            }
        }
    }
}

impl App {
    pub(super) fn desktop_notes_status(&self, id: &str) -> serde_json::Value {
        let state = self.brain.sessions.get(id);
        serde_json::json!({"ready": polish::profile_installed(ModelProfile::Summary),
            "acceleration_installed": polish::gpu_available(),
            "working": self.brain.working_for(id), "status": state.map(|s|s.status.as_str()).unwrap_or(""),
            "progress": state.and_then(|s|s.progress), "download_status": self.brain.download_status,
            "downloading": self.brain.download.is_some()})
    }

    pub(super) fn desktop_notes_generate(&mut self, session: Session) -> Result<(), String> {
        if self.recording.is_some()
            || self.busy
            || self.loading
            || self.polish_working()
            || self.brain.job.is_some()
            || self.notetaker_routing_busy()
            || self.brain.download.is_some()
        {
            return Err("Wait for the current task before updating notes.".into());
        }
        if !polish::profile_installed(ModelProfile::Summary) {
            return Err("Download the summary model in Settings first.".into());
        }
        let transcript = source(&session);
        if transcript.segments.is_empty() {
            return Err("This document has no transcript to summarize.".into());
        }
        self.release_polish_runtime();
        let job = brain::start(transcript.clone()).map_err(|e| e.to_string())?;
        self.brain.start(session.id.clone(), job);
        let active = self.brain.job.as_mut().unwrap();
        active.snapshot = Some(transcript.clone());
        active.automatic = true;
        let state = self.brain.sessions.get_mut(&session.id).unwrap();
        state.owner = Some(session);
        state.last_source = Some(transcript);
        state.last_attempt = Some(std::time::Instant::now());
        Ok(())
    }

    pub(super) fn desktop_summary_download(&mut self) -> Result<(), String> {
        if self.brain.download.is_some() {
            return Err("The summary model is already downloading.".into());
        }
        self.brain.download = Some(polish::download_profile(ModelProfile::Summary));
        self.brain.download_status = "Downloading summary model".into();
        Ok(())
    }

    pub(super) fn desktop_summary_download_cancel(&mut self) {
        if let Some(job) = &self.brain.download {
            job.cancel();
        }
    }

    pub(super) fn brain_queue_finished_call(&mut self) {
        if polish::profile_installed(ModelProfile::Summary)
            && let Some(session) = &self.history.call
            && !self.history.pending_deletions.contains(&session.id)
            && !self.history.call_deleted
        {
            self.brain.queue_final(session.clone());
        }
    }

    pub(super) fn brain_queue_session(&mut self, session: Session) {
        self.brain.queue_final(session);
    }

    pub(super) fn brain_source_current(&self, session: &Session) -> bool {
        !self.brain.working_for(&session.id)
            && self
                .brain
                .sessions
                .get(&session.id)
                .and_then(|state| state.last_source.as_ref())
                .is_some_and(|last| last.segments == source(session).segments)
    }

    pub(super) fn brain_saved(&mut self, id: &str) {
        if let Some(session) = self.brain.sessions.get_mut(id)
            && session.pending_save
        {
            session.save_acknowledged = true;
        }
    }
    pub(super) fn brain_working(&self) -> bool {
        self.brain.job.is_some()
    }

    pub(super) fn brain_poll(&mut self) {
        self.brain.live_call = self
            .history
            .call
            .as_ref()
            .filter(|_| self.call.is_some())
            .map(|session| session.id.clone());
        for session in [
            &self.history.call,
            &self.history.selected,
            &self.history.dictation,
        ]
        .into_iter()
        .flatten()
        {
            self.brain.remember(session);
        }
        if (self.recording.is_some() || self.loading)
            && let Some(active) = &self.brain.job
        {
            let id = active.source_id.clone();
            self.brain.cancel(&id, "Summary cancelled to keep speech recognition responsive. Generate it again when recording and model setup have finished.");
        }
        while let Some(active) = &self.brain.job {
            let id = active.source_id.clone();
            match active.job.try_recv() {
                Ok(event) => self.brain.receive(&id, event),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.brain.receive(
                        &id,
                        brain::Event::Failed("The summary worker stopped. Try again.".into()),
                    );
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        self.brain_live_notes();
        let settled = self.history.selected_dirty.is_none()
            && self.history.call_dirty.is_none()
            && self.history.dictation_dirty.is_none()
            && self
                .history
                .worker
                .as_ref()
                .is_some_and(|worker| worker.saves_settled() == Ok(true));
        self.brain.settle_saves(settled);
        loop {
            let event = self.brain.download.as_ref().map(polish::Job::try_recv);
            match event {
                Some(Ok(polish::Event::Progress { stage, fraction })) => {
                    self.brain.download_status = stage;
                    self.brain.download_progress = fraction;
                }
                Some(Ok(event)) => {
                    self.brain.download = None;
                    self.brain.download_progress = None;
                    self.brain.download_status = match event {
                        polish::Event::Installed => "Summary model ready on this device.".into(),
                        polish::Event::Failed(error) => error,
                        polish::Event::Cancelled => {
                            "Download paused. Choose Download to resume.".into()
                        }
                        _ => "Summary model setup finished.".into(),
                    };
                    break;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.brain.download = None;
                    self.brain.download_progress = None;
                    self.brain.download_status =
                        "Download stopped. Choose Download to resume.".into();
                    break;
                }
                _ => break,
            }
        }
    }

    fn brain_live_notes(&mut self) {
        // Completed updates are persisted by source identity, including a call
        // that has already been replaced by a new recording in the workspace.
        let ready: Vec<_> = self
            .brain
            .sessions
            .iter()
            .filter(|(_, state)| state.automatic_ready)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            if self.history.pending_deletions.contains(&id) {
                self.brain.forget(&id);
                continue;
            }
            let state = self.brain.sessions.get_mut(&id).unwrap();
            let owner = self
                .history
                .call
                .as_ref()
                .filter(|s| s.id == id)
                .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
                .or_else(|| self.history.dictation.as_ref().filter(|s| s.id == id))
                .or(state.owner.as_ref())
                .cloned();
            if let Some(mut session) = owner
                && let Some(draft) = state.draft.take()
            {
                merge_document(&mut session, &draft);
                state.automatic_ready = false;
                state.owner = None;
                if let Some(queued) = self
                    .brain
                    .final_calls
                    .iter_mut()
                    .find(|queued| queued.id == id)
                {
                    queued.clone_from(&session);
                }
                state.status = "Notes updated. Your edits are kept.".into();
                for slot in [
                    &mut self.history.call,
                    &mut self.history.selected,
                    &mut self.history.dictation,
                ] {
                    if slot.as_ref().is_some_and(|s| s.id == id) {
                        *slot = Some(session.clone());
                    }
                }
                self.notetaker_summary_ready(&session);
                if let Some(worker) = &self.history.worker {
                    worker.save(session);
                }
            }
        }
        // Polling continues while inference and capture run. Do not repeatedly
        // clone long call transcripts or scan queued sources until work can start.
        if self.brain.job.is_some()
            || self.notetaker_routing_busy()
            || self.recording.is_some()
            || self.loading
            || self.busy
            || self.polish_working()
            || !polish::profile_installed(ModelProfile::Summary)
        {
            return;
        }
        let final_session = self.brain.final_candidate(std::time::Instant::now());
        let finalizing = final_session.is_some();
        let Some(mut session) = final_session.or_else(|| self.history.call.clone()) else {
            return;
        };
        if self.history.pending_deletions.contains(&session.id) {
            return;
        }
        let recording = !finalizing && self.call.is_some();
        let state = self.brain.sessions.entry(session.id.clone()).or_default();
        state.live_seen |= recording;
        state.spoken_note = session.kind == crate::history::Kind::Note;
        if (!state.live_seen && !finalizing) || state.paused {
            return;
        }
        // Summarize committed speech only. Revisable ASR previews remain in the transcript.
        if recording {
            session.rows.clone_from(&self.call_committed);
            session.text = crate::calls::text(&session.rows, &session.speaker_names);
        }
        let transcript = source(&session);
        if !notes_due(state, &transcript, recording, std::time::Instant::now()) {
            return;
        }
        self.release_polish_runtime();
        let state = self.brain.sessions.get_mut(&session.id).unwrap();
        state.last_attempt = Some(std::time::Instant::now());
        match brain::start(transcript.clone()) {
            Ok(job) => {
                self.brain.start(session.id.clone(), job);
                let active = self.brain.job.as_mut().unwrap();
                active.snapshot = Some(transcript.clone());
                active.automatic = true;
                let state = self.brain.sessions.get_mut(&session.id).unwrap();
                state.owner = Some(session);
                state.last_source = Some(transcript);
            }
            Err(error) => {
                let state = self.brain.sessions.get_mut(&session.id).unwrap();
                state.retry_pending = true;
                state.status = error.to_string();
            }
        }
    }
}

fn notes_due(
    state: &SessionState,
    input: &Transcript,
    recording: bool,
    now: std::time::Instant,
) -> bool {
    // The live cadence protects capture responsiveness. Once capture ends,
    // finish any remaining speech immediately; only failed attempts back off.
    let interval = if state.spoken_note && !state.retry_pending {
        15
    } else {
        45
    };
    if input.segments.is_empty()
        || ((recording || state.retry_pending)
            && state
                .last_attempt
                .is_some_and(|at| now.duration_since(at).as_secs() < interval))
    {
        return false;
    }
    let words = |t: &Transcript| {
        t.segments
            .iter()
            .map(|s| s.text.split_whitespace().count())
            .sum::<usize>()
    };
    let changed = state
        .last_source
        .as_ref()
        .is_none_or(|old| old.segments != input.segments);
    let added = words(input).saturating_sub(state.last_source.as_ref().map_or(0, words));
    changed && (!recording || added >= if state.spoken_note { 12 } else { 40 })
}

fn merge_document(session: &mut Session, draft: &brain::Draft) {
    if (matches!(
        session.kind,
        crate::history::Kind::Call | crate::history::Kind::Dictation
    ) || (session.kind == crate::history::Kind::Note && !session.rows.is_empty()))
        && !session.title_is_manual
        && let Some(title) = &draft.title
    {
        session.title.clone_from(title);
    }
    if draft.items.is_empty() {
        return;
    }
    let previous = session.generated_summary.as_ref();
    let mut blocks: Vec<String> = session
        .personal_notes
        .split("\n\n")
        .map(str::to_owned)
        .collect();
    // Keep a durable record of generated wording the user changed or removed.
    // Comparing only the preceding draft loses that intent when a model update
    // omits a point and a later update includes it again.
    if let Some(old) = previous {
        for prior in &old.items {
            if !blocks.iter().any(|text| text == &prior.text)
                && !session.protected_note_items.contains(prior)
            {
                session.protected_note_items.push(prior.clone());
            }
        }
    }
    let mut used = std::collections::HashSet::new();
    for (index, next) in draft.items.iter().enumerate() {
        if session
            .protected_note_items
            .iter()
            .any(|prior| same_note_source(prior, next))
        {
            used.insert(index);
        }
    }
    if let Some(old) = previous {
        for prior in &old.items {
            if session
                .protected_note_items
                .iter()
                .any(|edited| same_note_source(edited, prior))
            {
                continue;
            }
            let related: Vec<_> = draft
                .items
                .iter()
                .enumerate()
                .filter(|(index, next)| !used.contains(index) && same_note_source(prior, next))
                .collect();
            for (index, _) in &related {
                used.insert(*index);
            }
            if let Some(position) = blocks.iter().position(|text| text == &prior.text) {
                blocks[position] = related
                    .iter()
                    .map(|(_, item)| item.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
        }
    }
    for (index, item) in draft.items.iter().enumerate() {
        if !used.contains(&index) && !blocks.iter().any(|text| text == &item.text) {
            blocks.push(item.text.clone());
        }
    }
    session.personal_notes = blocks
        .into_iter()
        .filter(|b| !b.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    session.generated_summary = Some(draft.clone());
}

fn same_note_source(prior: &brain::Item, next: &brain::Item) -> bool {
    next.text == prior.text
        || next.sources.iter().any(|a| {
            prior.sources.iter().any(|b| {
                a.source_id == b.source_id
                    && a.start_byte == b.start_byte
                    && a.end_byte == b.end_byte
                    && a.excerpt == b.excerpt
                    && a.speaker == b.speaker
            })
        })
}

pub(super) fn source(session: &Session) -> Transcript {
    if session.is_collection {
        return crate::topics::transcript(session);
    }
    let segments = if !session.rows.is_empty() {
        session
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.text.trim().is_empty())
            .map(|(index, row)| Segment {
                id: format!("row-{index}"),
                start_ms: row.start_ms,
                end_ms: row.end_ms,
                speaker: Some(crate::calls::label(row, &session.speaker_names)),
                text: row.text.clone(),
            })
            .collect()
    } else if !session.text.trim().is_empty() {
        vec![Segment {
            id: "dictation".into(),
            start_ms: 0,
            end_ms: session.metrics.audio_duration_ms.unwrap_or(0),
            speaker: None,
            text: session.text.clone(),
        }]
    } else {
        Vec::new()
    };
    Transcript {
        id: session.id.clone(),
        title: session.title.clone(),
        goal: "Summarize facts, decisions and explicit actions with sources.".into(),
        segments,
    }
}
#[allow(
    dead_code,
    reason = "Source-attributed summary export retained for document export"
)]
fn kind(kind: brain::Kind) -> &'static str {
    match kind {
        brain::Kind::Fact => "Key point",
        brain::Kind::Decision => "Decision",
        brain::Kind::Action => "Action",
    }
}
#[allow(
    dead_code,
    reason = "Source-attributed summary export retained for document export"
)]
fn time(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, (ms / 1000) % 60)
}
#[allow(
    dead_code,
    reason = "Source-attributed summary export retained for document export"
)]
pub(super) fn summary_text(draft: &brain::Draft) -> String {
    let mut text = String::new();
    for item in &draft.items {
        text.push_str(&format!("{}: {}\n", kind(item.kind), item.text));
        for citation in &item.sources {
            text.push_str(&format!(
                "  [{}] {}: {}\n",
                time(citation.start_ms),
                citation.speaker.as_deref().unwrap_or("Transcript"),
                citation.excerpt
            ));
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
impl App {
    pub(super) fn prepare_active_summary_owner_for_test(&mut self, session: &Session) {
        let (job, _events) = brain::Job::test_channel();
        self.brain.start(session.id.clone(), job);
        self.brain.remember(session);
    }

    pub(super) fn active_summary_owner_notes_for_test(&self, id: &str) -> String {
        self.brain.sessions[id]
            .owner
            .as_ref()
            .unwrap()
            .personal_notes
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, row: &str) -> brain::Item {
        brain::Item {
            kind: brain::Kind::Fact,
            text: text.into(),
            section: 1,
            sources: vec![brain::Citation {
                source_id: row.into(),
                start_byte: 0,
                end_byte: 10,
                start_ms: 0,
                end_ms: 1000,
                speaker: None,
                excerpt: "A decision".into(),
            }],
        }
    }

    #[test]
    fn model_titles_update_transcripts_but_preserve_manual_names_and_notes() {
        let mut call = session();
        call.kind = crate::history::Kind::Call;
        call.title = "Conversation".into();
        let mut next = draft(&call);
        next.title = Some("Release schedule and ownership".into());
        next.items = vec![item("Ship on Friday.", "row-0")];
        merge_document(&mut call, &next);
        assert_eq!(call.title, "Release schedule and ownership");
        next.title = Some("Revised release schedule".into());
        merge_document(&mut call, &next);
        assert_eq!(call.title, "Revised release schedule");
        call.title = "My project sync".into();
        call.title_is_manual = true;
        merge_document(&mut call, &next);
        assert_eq!(call.title, "My project sync");
        let mut dictation = session();
        dictation.title = "My dictation".into();
        merge_document(&mut dictation, &next);
        assert_eq!(dictation.title, "Revised release schedule");
        dictation.title = "My custom title".into();
        dictation.title_is_manual = true;
        merge_document(&mut dictation, &next);
        assert_eq!(dictation.title, "My custom title");
        let mut note = session();
        note.kind = crate::history::Kind::Note;
        note.title = "My written note".into();
        merge_document(&mut note, &next);
        assert_eq!(note.title, "My written note");
    }

    #[test]
    fn document_updates_preserve_written_and_deleted_paragraphs() {
        let mut session = session();
        let mut old = draft(&session);
        old.items = vec![
            item("Ship on Friday.", "row-0"),
            item("Use the blue cover.", "row-1"),
        ];
        merge_document(&mut session, &old);
        session.personal_notes = "My private reminder.\n\nShip on Friday after my review.".into();
        let mut next = old.clone();
        next.items
            .push(item("Taylor will send the invitation.", "row-2"));
        merge_document(&mut session, &next);
        assert_eq!(
            session.personal_notes,
            "My private reminder.\n\nShip on Friday after my review.\n\nTaylor will send the invitation."
        );
        session.personal_notes.clear();
        merge_document(&mut session, &next);
        assert!(
            session.personal_notes.is_empty(),
            "Deleting notes must not restore them"
        );
    }

    #[test]
    fn document_edits_survive_omitted_generation_and_history_reload() {
        let mut session = session();
        let directory =
            std::env::temp_dir().join(format!("articulate-protected-notes-{}", session.id));
        let history = crate::history::History::open(directory.clone()).unwrap();
        let mut old = draft(&session);
        old.items = vec![item("Ship on Friday.", "row-0"), item("Use blue.", "row-1")];
        merge_document(&mut session, &old);
        session.personal_notes = "Ship after my own review.".into();
        let mut omitted = old.clone();
        omitted.items = vec![item("Taylor owns the invitation.", "row-2")];
        merge_document(&mut session, &omitted);
        history.save(session.clone()).unwrap();
        session = history.load(&session.id).unwrap();
        let mut returned = omitted.clone();
        returned.items.extend([
            item("Ship the release on Friday.", "row-0"),
            item("The chosen cover is blue.", "row-1"),
        ]);
        merge_document(&mut session, &returned);
        assert_eq!(
            session.personal_notes,
            "Ship after my own review.\n\nTaylor owns the invitation."
        );
        assert!(session.protected_note_items.len() >= 2);
        drop(history);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn untouched_notes_can_return_after_model_omission() {
        let mut session = session();
        let mut first = draft(&session);
        first.items = vec![item("Ship Friday.", "row-0"), item("Use blue.", "row-1")];
        merge_document(&mut session, &first);
        let mut omitted = first.clone();
        omitted.items.truncate(1);
        merge_document(&mut session, &omitted);
        merge_document(&mut session, &first);
        assert_eq!(session.personal_notes, "Ship Friday.\n\nUse blue.");
        assert!(session.protected_note_items.is_empty());
    }

    #[test]
    fn combined_generated_paragraph_is_not_duplicated() {
        let mut session = session();
        let mut old = draft(&session);
        old.items = vec![item("Ship on Friday.", "row-0"), item("Use blue.", "row-0")];
        merge_document(&mut session, &old);
        session.personal_notes.push_str("\n\nMy reminder.");
        let mut next = old.clone();
        next.items = vec![item("Ship the blue cover on Friday.", "row-0")];
        merge_document(&mut session, &next);
        assert_eq!(
            session.personal_notes,
            "Ship the blue cover on Friday.\n\nMy reminder."
        );
    }

    #[test]
    fn live_notes_are_bounded_and_finish_with_short_remaining_speech() {
        let mut session = session();
        session.text = "word ".repeat(40);
        let input = source(&session);
        let now = std::time::Instant::now();
        let mut state = SessionState::default();
        assert!(notes_due(&state, &input, true, now));
        state.last_source = Some(input.clone());
        state.last_attempt = Some(now);
        session.text.push_str("Another sentence.");
        let next = source(&session);
        assert!(notes_due(&state, &next, false, now));
        assert!(!notes_due(&state, &next, true, now));
        let later = now + std::time::Duration::from_secs(46);
        assert!(!notes_due(&state, &next, true, later));
        assert!(notes_due(&state, &next, false, later));
        assert!(!notes_due(&state, &input, false, later));
    }

    #[test]
    fn spoken_thoughts_update_sooner_without_tight_failure_retries() {
        let mut session = session();
        session.text = "word ".repeat(12);
        let input = source(&session);
        let now = std::time::Instant::now();
        let mut state = SessionState {
            spoken_note: true,
            ..Default::default()
        };
        assert!(notes_due(&state, &input, true, now));
        state.last_source = Some(input);
        state.last_attempt = Some(now);
        session.text.push_str(&"next ".repeat(12));
        let next = source(&session);
        assert!(!notes_due(
            &state,
            &next,
            true,
            now + std::time::Duration::from_secs(14)
        ));
        assert!(notes_due(
            &state,
            &next,
            true,
            now + std::time::Duration::from_secs(15)
        ));
        state.retry_pending = true;
        assert!(!notes_due(
            &state,
            &next,
            true,
            now + std::time::Duration::from_secs(16)
        ));
    }

    #[test]
    fn final_tail_bypasses_live_cooldown_and_survives_a_new_call() {
        let mut state = State::default();
        let mut finished = session();
        let original = source(&finished);
        let now = std::time::Instant::now();
        let previous = state.sessions.entry(finished.id.clone()).or_default();
        previous.last_source = Some(original);
        previous.last_attempt = Some(now);
        previous.live_seen = true;
        finished.text.push_str(" The final deadline is Monday.");
        state.queue_final(finished.clone());
        state.live_call = Some(session().id);
        let candidate = state.final_candidate(now).unwrap();
        assert_eq!(candidate.id, finished.id);
        assert!(candidate.text.ends_with("Monday."));
        state.sessions.get_mut(&finished.id).unwrap().last_source = Some(source(&finished));
        assert!(
            state
                .final_candidate(now + std::time::Duration::from_secs(46))
                .is_none()
        );
        assert!(state.final_calls.is_empty());
    }

    #[test]
    fn final_queue_preserves_edits_and_drops_cancelled_deleted_or_excess_calls() {
        let mut state = State::default();
        let now = std::time::Instant::now();
        let mut finished = session();
        state.queue_final(finished.clone());
        finished.personal_notes = "My edited notes".into();
        state.remember(&finished);
        assert_eq!(
            state.final_candidate(now).unwrap().personal_notes,
            finished.personal_notes
        );
        state.sessions.get_mut(&finished.id).unwrap().paused = true;
        assert!(state.final_candidate(now).is_none());
        state.sessions.get_mut(&finished.id).unwrap().paused = false;
        state.queue_final(finished.clone());
        state.forget(&finished.id);
        assert!(state.final_calls.is_empty());
        for _ in 0..10 {
            state.queue_final(session());
        }
        assert_eq!(state.final_calls.len(), 8);
    }

    #[test]
    fn failed_live_generation_can_retry_unchanged_speech() {
        let mut state = State::default();
        let session = session();
        let (job, _events) = brain::Job::test_channel();
        state.start(session.id.clone(), job);
        state.sessions.get_mut(&session.id).unwrap().last_source = Some(source(&session));
        state.receive(&session.id, brain::Event::Failed("Temporary error".into()));
        assert!(notes_due(
            &state.sessions[&session.id],
            &source(&session),
            false,
            std::time::Instant::now()
        ));
    }

    #[test]
    fn failed_final_summary_keeps_retry_backoff() {
        let mut state = State::default();
        let finished = session();
        let now = std::time::Instant::now();
        state.queue_final(finished.clone());
        let (job, _events) = brain::Job::test_channel();
        state.start(finished.id.clone(), job);
        let owner = state.sessions.get_mut(&finished.id).unwrap();
        owner.last_source = Some(source(&finished));
        owner.last_attempt = Some(now);
        state.receive(&finished.id, brain::Event::Failed("Temporary error".into()));
        assert!(state.final_candidate(now).is_none());
        assert!(
            state
                .final_candidate(now + std::time::Duration::from_secs(46))
                .is_some()
        );
    }

    #[test]
    fn title_only_result_updates_title_without_changing_notes() {
        let mut dictation = session();
        dictation.personal_notes = "Keep this reminder.".into();
        let mut next = draft(&dictation);
        next.title = Some("Release planning".into());
        merge_document(&mut dictation, &next);
        assert_eq!(dictation.title, "Release planning");
        assert_eq!(dictation.personal_notes, "Keep this reminder.");
    }

    #[test]
    fn automatic_notes_save_to_their_owner_after_navigation() {
        let (mut app, _) = super::super::tests::app();
        let mut a = session();
        a.kind = crate::history::Kind::Call;
        let b = session();
        let directory = std::env::temp_dir().join(format!("articulate-live-notes-{}", a.id));
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        let (job, events) = brain::Job::test_channel();
        app.brain.start(a.id.clone(), job);
        app.brain.remember(&a);
        let active = app.brain.job.as_mut().unwrap();
        active.snapshot = Some(source(&a));
        active.automatic = true;
        app.history.selected = Some(b.clone());
        let mut result = draft(&a);
        result.title = Some("Release approval".into());
        result.items = vec![brain::Item {
            kind: brain::Kind::Decision,
            text: "The team approved the release.".into(),
            section: 1,
            sources: vec![brain::Citation {
                source_id: "dictation".into(),
                start_byte: 0,
                end_byte: a.text.len(),
                start_ms: 0,
                end_ms: 0,
                speaker: None,
                excerpt: a.text.clone(),
            }],
        }];
        result.validate(&source(&a)).unwrap();
        events.send(brain::Event::Complete(result)).unwrap();
        app.brain_poll();
        let worker = app.history.worker.as_ref().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while worker.saves_settled() != Ok(true) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let store = crate::history::History::open(directory.clone()).unwrap();
        assert_eq!(store.load(&a.id).unwrap().personal_notes, a.text);
        assert_eq!(store.load(&a.id).unwrap().title, "Release approval");
        assert_eq!(app.history.selected.as_ref().unwrap().title, b.title);
        assert_eq!(app.history.selected.as_ref().unwrap().id, b.id);
        assert!(
            app.history
                .selected
                .as_ref()
                .unwrap()
                .personal_notes
                .is_empty()
        );
        drop(app);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn session() -> Session {
        let mut session = Session::new(crate::history::Kind::Dictation);
        session.text = "The team approved the release.".into();
        session
    }
    fn draft(session: &Session) -> brain::Draft {
        brain::Draft {
            title: None,
            schema: 1,
            source_id: session.id.clone(),
            source_hash: brain::source_hash_for_test(&source(session)),
            model: brain::MODEL_LABEL.into(),
            sections: 1,
            elapsed_ms: 10,
            items: vec![],
        }
    }

    #[test]
    fn completion_uses_job_owner_and_preserves_both_unsaved_drafts() {
        let (mut app, _) = super::super::tests::app();
        let a = session();
        let b = session();
        let (job, events) = brain::Job::test_channel();
        app.brain.start(a.id.clone(), job);
        app.history.selected = Some(b.clone());
        events.send(brain::Event::Complete(draft(&a))).unwrap();
        app.brain_poll();
        assert!(!app.brain.sessions.contains_key(&b.id));
        let (job, events) = brain::Job::test_channel();
        app.brain.start(b.id.clone(), job);
        events.send(brain::Event::Complete(draft(&b))).unwrap();
        app.brain_poll();
        for session in [&a, &b] {
            app.brain.sessions[&session.id]
                .draft
                .as_ref()
                .unwrap()
                .validate(&source(session))
                .unwrap();
        }
        let mut changed = a.clone();
        changed.text.push_str(" New context.");
        assert!(
            app.brain.sessions[&a.id]
                .draft
                .as_ref()
                .unwrap()
                .validate(&source(&changed))
                .is_err()
        );
    }

    #[test]
    fn deleted_and_cancelled_jobs_cannot_repopulate_or_replace_another_session() {
        let (mut app, _) = super::super::tests::app();
        let a = session();
        let b = session();
        let (job, old_events) = brain::Job::test_channel();
        app.brain.start(a.id.clone(), job);
        app.brain.forget(&a.id);
        assert!(old_events.send(brain::Event::Complete(draft(&a))).is_err());
        let (job, events) = brain::Job::test_channel();
        app.brain.start(b.id.clone(), job);
        app.brain.receive(&a.id, brain::Event::Complete(draft(&a)));
        assert!(!app.brain.sessions.contains_key(&a.id));
        assert!(app.brain.working_for(&b.id));
        events.send(brain::Event::Complete(draft(&a))).unwrap();
        app.brain_poll();
        assert!(app.brain.sessions[&b.id].draft.is_none());
        assert!(app.brain.sessions[&b.id].status.contains("did not match"));
    }

    #[test]
    fn delayed_save_ack_is_scoped_and_never_overwrites_newer_work() {
        let (mut app, _) = super::super::tests::app();
        let a = session();
        let b = session();
        for session in [&a, &b] {
            app.brain.sessions.insert(
                session.id.clone(),
                SessionState {
                    pending_save: true,
                    status: "Saving your reviewed summary…".into(),
                    ..Default::default()
                },
            );
        }
        app.brain_saved(&a.id);
        app.brain.settle_saves(false);
        assert!(app.brain.sessions[&a.id].pending_save);
        app.brain.settle_saves(true);
        assert_eq!(
            app.brain.sessions[&a.id].status,
            "Summary saved on this device."
        );
        assert!(app.brain.sessions[&b.id].pending_save);
        let (job, _events) = brain::Job::test_channel();
        app.brain.start(b.id.clone(), job);
        app.brain_saved(&b.id);
        app.brain.settle_saves(true);
        assert_eq!(
            app.brain.sessions[&b.id].status,
            "Preparing the local summary…"
        );
    }
}
