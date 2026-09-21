use super::*;
use crate::brain::search::{self, Field, Hit, Job, Results};

#[derive(Default)]
pub(super) struct State {
    #[allow(
        dead_code,
        reason = "Saved search query retained for the Tauri search flow"
    )]
    pub query: String,
    pub results: Option<Results>,
    job: Option<Job>,
    submitted: String,
    pub error: Option<String>,
    opening: Option<Hit>,
}

impl State {
    fn receive(&mut self, result: Result<Results, String>) {
        self.job = None;
        match result {
            Ok(results) if results.query == self.submitted => {
                self.results = Some(results);
                self.error = None;
            }
            Ok(_) => {}
            Err(error) => self.error = Some(error),
        }
    }
    fn cancel(&mut self) {
        self.job = None;
    }
}

impl App {
    pub(super) fn poll_saved_search(&mut self) {
        if self.page != 8 {
            self.saved_search.cancel();
        }
        if let Some(job) = &self.saved_search.job {
            match job.try_recv() {
                Ok(result) => self.saved_search.receive(result),
                Err(mpsc::TryRecvError::Disconnected) => self
                    .saved_search
                    .receive(Err("Search stopped unexpectedly. Try again.".into())),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(hit) = self.saved_search.opening.clone() else {
            return;
        };
        if !matches!(self.page, 3 | 5) {
            self.saved_search.opening = None;
            return;
        }
        if self
            .history
            .selected
            .as_ref()
            .is_some_and(|session| session.id == hit.session_id)
        {
            self.saved_search.opening = None;
            let session = self.history.selected.as_ref().unwrap();
            self.page = if session.kind == crate::history::Kind::Dictation {
                5
            } else {
                6
            };
            self.history.notetaker_tab = if hit.field == Field::CallTranscript {
                2
            } else {
                0
            };
            if hit.field == Field::CallTranscript {
                let current = hit.row.and_then(|index| session.rows.get(index));
                if current.is_some_and(|row| row.text.contains(&hit.excerpt)) {
                    self.history.detail_search = hit.excerpt;
                } else {
                    self.history.notice = "This transcript has changed since the search. Search again for its latest text.".into();
                }
            }
        } else if self.page == 3
            && self
                .history
                .call
                .as_ref()
                .is_some_and(|session| session.id == hit.session_id)
        {
            self.saved_search.opening = None;
            if hit.field == Field::CallTranscript {
                self.call_tab = 0;
                self.call_search = hit.excerpt;
            }
        } else if !self.history.loading && self.history.error.is_some() {
            self.saved_search.opening = None;
            self.saved_search.error = Some("Could not open that saved item. It may have been deleted. Refresh the search or retry in History.".into());
            self.page = 8;
        }
    }

    #[allow(
        dead_code,
        reason = "Saved transcript search retained for the Tauri search flow"
    )]
    fn start_saved_search(&mut self) {
        self.saved_search.cancel();
        self.saved_search.error = None;
        self.saved_search.submitted = self.saved_search.query.trim().to_owned();
        match search::start(self.saved_search.submitted.clone()) {
            Ok(job) => self.saved_search.job = Some(job),
            Err(error) => self.saved_search.error = Some(error.to_string()),
        }
    }
}

#[allow(
    dead_code,
    reason = "Saved transcript search retained for the Tauri search flow"
)]
fn metadata(hit: &Hit) -> String {
    let field = match hit.field {
        Field::Title => "Title",
        Field::Dictation => "Dictation",
        Field::PersonalNotes => "My notes",
        Field::CallTranscript => "Transcript",
    };
    let when =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(hit.created_ms) * 1_000_000)
            .ok()
            .map(|timestamp| {
                let local = timestamp.to_offset(
                    time::UtcOffset::local_offset_at(timestamp).unwrap_or(time::UtcOffset::UTC),
                );
                format!(
                    "{} · {:02}:{:02}",
                    local.date(),
                    local.hour(),
                    local.minute()
                )
            })
            .unwrap_or_else(|| "Date unavailable".into());
    let mut details = format!("{field} · {when}");
    if let Some(speaker) = &hit.speaker {
        details.push_str(&format!(" · {speaker}"));
    }
    if let (Some(start), Some(end)) = (hit.start_ms, hit.end_ms) {
        details.push_str(&format!(" · {} to {}", stamp(start), stamp(end)));
    }
    details
}
#[allow(
    dead_code,
    reason = "Saved transcript search retained for the Tauri search flow"
)]
fn stamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn superseded_search_results_never_replace_current_query() {
        let mut state = State {
            submitted: "new phrase".into(),
            ..Default::default()
        };
        state.receive(Ok(Results {
            query: "old phrase".into(),
            ..Default::default()
        }));
        assert!(state.results.is_none());
        state.receive(Ok(Results {
            query: "new phrase".into(),
            ..Default::default()
        }));
        assert_eq!(state.results.unwrap().query, "new phrase");
    }
    #[test]
    fn result_metadata_preserves_source_turn_times() {
        let hit = Hit {
            session_id: "synthetic".into(),
            title: "Plan".into(),
            created_ms: 0,
            field: Field::CallTranscript,
            excerpt: "Exact source words.".into(),
            row: Some(0),
            speaker: Some("Casey".into()),
            start_ms: Some(90_000),
            end_ms: Some(125_000),
        };
        let label = metadata(&hit);
        assert!(label.contains("Casey"));
        assert!(label.contains("01:30 to 02:05"));
    }

    #[test]
    fn source_open_waits_for_exact_loaded_id_and_selects_transcript() {
        let (mut app, _) = super::super::tests::app();
        let mut hit = Hit {
            session_id: "synthetic-session".into(),
            title: "Planning the next release".into(),
            created_ms: 0,
            field: Field::CallTranscript,
            excerpt: "Review the release together on Thursday.".into(),
            row: Some(0),
            speaker: Some("Casey".into()),
            start_ms: Some(95_000),
            end_ms: Some(108_000),
        };
        let mut session = crate::history::Session::new(crate::history::Kind::Call);
        hit.session_id = session.id.clone();
        session.rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 95_000,
            end_ms: 108_000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: hit.excerpt.clone(),
        });
        app.page = 5;
        app.saved_search.opening = Some(hit.clone());
        app.history.selected = Some(crate::history::Session::new(crate::history::Kind::Call));
        app.poll_saved_search();
        assert_eq!(app.page, 5);
        assert!(app.saved_search.opening.is_some());
        app.history.selected = Some(session);
        app.poll_saved_search();
        assert_eq!(app.page, 6);
        assert_eq!(app.history.notetaker_tab, 2);
        assert_eq!(app.history.detail_search, hit.excerpt);
        assert!(app.saved_search.opening.is_none());
    }
}
