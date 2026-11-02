//! Saved documents: the history list, the open document and insights.

use super::rows::RowView;
use crate::app::App;
use crate::history::{Kind, Session};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct SummaryView<'a> {
    id: &'a str,
    title: &'a str,
    kind: Kind,
    created_ms: u64,
    updated_ms: u64,
    preview: &'a str,
    duration_ms: u64,
    topic_id: Option<&'a str>,
    is_collection: bool,
    can_receive_sources: bool,
}

#[derive(Serialize)]
pub(crate) struct History<'a> {
    history: Vec<SummaryView<'a>>,
}

pub(crate) fn history(app: &App) -> History<'_> {
    History {
        history: app
            .history
            .items
            .iter()
            .map(|item| SummaryView {
                id: &item.id,
                title: &item.title,
                kind: item.kind,
                created_ms: item.created_ms,
                updated_ms: item.updated_ms,
                preview: &item.preview,
                duration_ms: item.duration_ms,
                topic_id: item.topic_id.as_deref(),
                is_collection: item.is_collection,
                can_receive_sources: item.can_receive_sources,
            })
            .collect(),
    }
}

#[derive(Serialize)]
pub(crate) struct SourceView<'a> {
    id: &'a str,
    title: &'a str,
    kind: Kind,
    created_ms: u64,
    preview: &'a str,
}

/// The first 160 characters, without allocating.
fn preview(text: &str) -> &str {
    text.char_indices()
        .nth(160)
        .map_or(text, |(index, _)| &text[..index])
}

/// Document metadata and personal notes, without transcript text or rows.
#[derive(Serialize)]
pub(crate) struct SessionMeta<'a> {
    id: &'a str,
    title: &'a str,
    kind: Kind,
    personal_notes: &'a str,
    audio_packets_lost: u64,
    created_ms: u64,
    updated_ms: u64,
    topic_id: Option<&'a str>,
    is_collection: bool,
    can_receive_sources: bool,
    sources: Vec<SourceView<'a>>,
    #[serde(flatten)]
    calls: super::calls::SessionCalls<'a>,
}

impl<'a> SessionMeta<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        Self {
            id: &session.id,
            title: &session.title,
            kind: session.kind,
            personal_notes: &session.personal_notes,
            audio_packets_lost: session.audio_packets_lost,
            created_ms: session.created_ms,
            updated_ms: session.updated_ms,
            topic_id: session.topic_id.as_deref(),
            is_collection: session.is_collection,
            can_receive_sources: session.can_receive_sources(),
            sources: session
                .sources
                .iter()
                .map(|source| SourceView {
                    id: &source.id,
                    title: &source.title,
                    kind: source.kind,
                    created_ms: source.created_ms,
                    preview: preview(&source.text),
                })
                .collect(),
            calls: super::calls::SessionCalls::new(session),
        }
    }
}

/// Transcript text and rows of a saved document.
#[derive(Serialize)]
pub(crate) struct SessionBody<'a> {
    id: &'a str,
    text: &'a str,
    original: &'a str,
    rows: Vec<RowView<'a>>,
}

impl<'a> SessionBody<'a> {
    fn new(session: &'a Session) -> Self {
        Self {
            id: &session.id,
            text: &session.text,
            original: &session.original,
            rows: RowView::saved(&session.rows, &session.speaker_names),
        }
    }
}

/// A complete document: metadata, notes, text and rows.
#[derive(Serialize)]
pub(crate) struct SessionView<'a> {
    #[serde(flatten)]
    meta: SessionMeta<'a>,
    text: &'a str,
    original: &'a str,
    rows: Vec<RowView<'a>>,
}

impl<'a> SessionView<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        let body = SessionBody::new(session);
        Self {
            meta: SessionMeta::new(session),
            text: body.text,
            original: body.original,
            rows: body.rows,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Document<'a> {
    selected: Option<SessionMeta<'a>>,
}

pub(crate) fn document(app: &App) -> Document<'_> {
    Document {
        selected: app.history.selected.as_ref().map(SessionMeta::new),
    }
}

/// Sent separately so note edits never resend a long transcript.
#[derive(Serialize)]
pub(crate) struct DocumentBody<'a> {
    selected_body: Option<SessionBody<'a>>,
}

pub(crate) fn document_body(app: &App) -> DocumentBody<'_> {
    DocumentBody {
        selected_body: app.history.selected.as_ref().map(SessionBody::new),
    }
}

#[derive(Serialize)]
pub(crate) struct AppUsageView<'a> {
    app: Option<&'a str>,
    words: u64,
    sessions: u64,
}

#[derive(Serialize)]
pub(crate) struct DayView<'a> {
    date: &'a str,
    words: u64,
    sessions: u64,
}

#[derive(Serialize)]
pub(crate) struct InsightsView<'a> {
    words: u64,
    sessions: u64,
    words_per_minute: Option<f64>,
    current_streak: u64,
    longest_streak: u64,
    active_days: u64,
    dictionary_replacements: u64,
    cleanup_edits: u64,
    apps: Vec<AppUsageView<'a>>,
    days: Vec<DayView<'a>>,
}

#[derive(Serialize)]
pub(crate) struct Insights<'a> {
    insights: Option<InsightsView<'a>>,
}

pub(crate) fn insights(app: &App) -> Insights<'_> {
    Insights {
        insights: app.insights.report.as_ref().map(|report| InsightsView {
            words: report.words,
            sessions: report.sessions,
            words_per_minute: report.words_per_minute,
            current_streak: report.current_streak,
            longest_streak: report.longest_streak,
            active_days: report.active_days,
            dictionary_replacements: report.dictionary_replacements,
            cleanup_edits: report.cleanup_edits,
            apps: report
                .apps
                .iter()
                .map(|usage| AppUsageView {
                    app: usage.app.as_deref(),
                    words: usage.words,
                    sessions: usage.sessions,
                })
                .collect(),
            days: report
                .days
                .iter()
                .map(|day| DayView {
                    date: &day.date,
                    words: day.words,
                    sessions: day.sessions,
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_previews_stop_at_a_character_boundary() {
        let text = "é".repeat(200);
        assert_eq!(super::preview(&text).chars().count(), 160);
        assert_eq!(super::preview("short"), "short");
    }
}
