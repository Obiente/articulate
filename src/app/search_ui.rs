//! Full-text search over saved history for the library.
//!
//! A new search replaces the previous one; dropping its job cancels it, so a
//! slower, older search can never overwrite the results of the latest query.

use super::*;
use crate::brain::search::{self, Filters, Job, Results};

/// Longest query accepted, in characters.
const MAX_QUERY_CHARS: usize = 160;

#[derive(Default)]
pub(super) struct State {
    /// The trimmed query of the current or latest search.
    pub query: String,
    /// Date, kind, speaker and app restrictions of the current search.
    pub filters: Filters,
    pub results: Option<Results>,
    job: Option<Job>,
    pub error: Option<String>,
}

impl State {
    fn receive(&mut self, result: Result<Results, String>) {
        self.job = None;
        match result {
            Ok(results) if results.query == self.query => {
                self.results = Some(results);
                self.error = None;
            }
            Ok(_) => {}
            Err(error) => {
                self.results = None;
                self.error = Some(error);
            }
        }
    }

    pub(super) fn working(&self) -> bool {
        self.job.is_some()
    }
}

impl App {
    pub(super) fn poll_saved_search(&mut self) {
        if let Some(job) = &self.saved_search.job {
            match job.try_recv() {
                Ok(result) => self.saved_search.receive(result),
                Err(mpsc::TryRecvError::Disconnected) => self
                    .saved_search
                    .receive(Err("Search stopped unexpectedly. Try again.".into())),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    /// Search saved items, replacing any search in progress. An empty query
    /// clears the results. Invalid queries are reported in the search state
    /// rather than as an action error, so typing never raises an alert.
    pub(super) fn saved_search(&mut self, query: &str, filters: Filters) {
        // Dropping the job cancels it.
        self.saved_search.job = None;
        self.saved_search.error = None;
        self.saved_search.query = query.trim().to_owned();
        self.saved_search.filters = filters;
        if self.saved_search.query.is_empty() {
            self.saved_search.results = None;
            return;
        }
        if self.saved_search.query.chars().count() > MAX_QUERY_CHARS
            || self.saved_search.query.chars().any(char::is_control)
        {
            self.saved_search.results = None;
            self.saved_search.error =
                Some("Search for a word or phrase of up to 160 characters.".into());
            return;
        }
        let index = self
            .history
            .worker
            .as_ref()
            .and_then(crate::history::Worker::search_index);
        match search::start(
            self.saved_search.query.clone(),
            self.saved_search.filters.clone(),
            index,
        ) {
            Ok(job) => self.saved_search.job = Some(job),
            Err(error) => self.saved_search.error = Some(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_search_results_never_replace_current_query() {
        let mut state = State {
            query: "new phrase".into(),
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
    fn empty_and_invalid_queries_clear_results_without_starting_a_job() {
        let (mut app, _) = super::super::tests::app();
        app.saved_search.results = Some(Results {
            query: "earlier".into(),
            ..Default::default()
        });
        app.saved_search("   ", Filters::default());
        assert!(app.saved_search.results.is_none());
        assert!(!app.saved_search.working());
        assert!(app.saved_search.error.is_none());
        app.saved_search(&"a".repeat(161), Filters::default());
        assert!(!app.saved_search.working());
        assert!(app.saved_search.error.is_some());
        app.saved_search("bell\u{7}", Filters::default());
        assert!(app.saved_search.error.is_some());
    }
}
