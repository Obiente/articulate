//! Saved documents: the history list, the open document and insights.

use super::rows::RowView;
use crate::app::App;
use crate::history::{Kind, Quarantined, RecordWarning, Session, retention::Policy};
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
    pinned: bool,
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
                pinned: item.pinned,
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

/// Document metadata, without notes, transcript text or rows.
#[derive(Serialize)]
pub(crate) struct SessionMeta<'a> {
    id: &'a str,
    title: &'a str,
    kind: Kind,
    audio_packets_lost: u64,
    created_ms: u64,
    updated_ms: u64,
    topic_id: Option<&'a str>,
    is_collection: bool,
    can_receive_sources: bool,
    sources: Vec<SourceView<'a>>,
    pinned: bool,
    /// Action items kept in the notes, ready to export.
    follow_ups: usize,
    /// Why subtitle exports are unavailable, or `None` when they are.
    subtitles_unavailable: Option<&'static str>,
    #[serde(flatten)]
    calls: super::calls::SessionCalls<'a>,
}

/// Mirrors the checks in `call_export::session` without formatting anything.
fn subtitles_unavailable(session: &Session) -> Option<&'static str> {
    if session.rows.is_empty() {
        if session.text.trim().is_empty() {
            Some("There is no transcript to turn into subtitles.")
        } else if session.metrics.audio_duration_ms.unwrap_or(0) == 0 {
            Some("This item has no recorded timing.")
        } else {
            None
        }
    } else if session
        .rows
        .iter()
        .any(|row| row.end_ms > row.start_ms && !row.text.trim().is_empty())
    {
        None
    } else {
        Some("This transcript has no timed sections.")
    }
}

impl<'a> SessionMeta<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        Self {
            id: &session.id,
            title: &session.title,
            kind: session.kind,
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
            pinned: session.pinned,
            follow_ups: crate::brain::followups::reviewed_count(session),
            subtitles_unavailable: subtitles_unavailable(session),
            calls: super::calls::SessionCalls::new(session),
        }
    }
}

/// Transcript text and rows of a saved document.
#[derive(Serialize)]
pub(crate) struct SessionBody<'a> {
    personal_notes: &'a str,
    id: &'a str,
    text: &'a str,
    original: &'a str,
    rows: Vec<RowView<'a>>,
}

impl<'a> SessionBody<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        Self {
            id: &session.id,
            personal_notes: &session.personal_notes,
            text: &session.text,
            original: &session.original,
            rows: RowView::saved(&session.rows, &session.speaker_names),
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

/// Hits sent to the renderer; the index may return more.
const MAX_SEARCH_HITS: usize = 100;
/// Retention preview items sent to the renderer; the total is always sent.
const MAX_PREVIEW_ITEMS: usize = 100;

#[derive(Serialize)]
pub(crate) struct FiltersView<'a> {
    from_ms: Option<u64>,
    to_ms: Option<u64>,
    kind: Option<Kind>,
    speaker: Option<&'a str>,
    app: Option<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct HitView<'a> {
    session_id: &'a str,
    title: &'a str,
    kind: Kind,
    created_ms: u64,
    field: &'static str,
    excerpt: &'a str,
    /// UTF-8 byte ranges of matched terms within `excerpt`.
    matches: &'a [(usize, usize)],
    speaker: Option<&'a str>,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ResultsView<'a> {
    query: &'a str,
    hits: Vec<HitView<'a>>,
    /// More hits exist than were sent.
    more: bool,
    indexed: bool,
    partial: bool,
    scanned_sessions: usize,
    unreadable_sessions: usize,
}

#[derive(Serialize)]
pub(crate) struct SearchView<'a> {
    query: &'a str,
    filters: FiltersView<'a>,
    working: bool,
    error: Option<&'a str>,
    results: Option<ResultsView<'a>>,
}

#[derive(Serialize)]
pub(crate) struct Search<'a> {
    search: SearchView<'a>,
}

pub(crate) fn search(app: &App) -> Search<'_> {
    let state = &app.saved_search;
    Search {
        search: SearchView {
            query: &state.query,
            filters: FiltersView {
                from_ms: state.filters.from_ms,
                to_ms: state.filters.to_ms,
                kind: state.filters.kind,
                speaker: state.filters.speaker.as_deref(),
                app: state.filters.app.as_deref(),
            },
            working: state.working(),
            error: state.error.as_deref(),
            results: state.results.as_ref().map(|results| ResultsView {
                query: &results.query,
                hits: results
                    .hits
                    .iter()
                    .take(MAX_SEARCH_HITS)
                    .map(|hit| HitView {
                        session_id: &hit.session_id,
                        title: &hit.title,
                        kind: hit.kind,
                        created_ms: hit.created_ms,
                        field: hit.field.key(),
                        excerpt: &hit.excerpt,
                        matches: &hit.matches,
                        speaker: hit.speaker.as_deref(),
                        start_ms: hit.start_ms,
                        end_ms: hit.end_ms,
                    })
                    .collect(),
                more: results.hits.len() > MAX_SEARCH_HITS,
                indexed: results.indexed,
                partial: results.partial,
                scanned_sessions: results.scanned_sessions,
                unreadable_sessions: results.unreadable_sessions,
            }),
        },
    }
}

#[derive(Serialize)]
pub(crate) struct EncryptionResultView<'a> {
    error: Option<&'a str>,
    protect: bool,
    changed: usize,
    failed: usize,
}

#[derive(Serialize)]
pub(crate) struct PreviewItemView<'a> {
    id: &'a str,
    title: &'a str,
    kind: Kind,
    updated_ms: u64,
}

#[derive(Serialize)]
pub(crate) struct RetentionPreviewView<'a> {
    policy: Option<&'a Policy>,
    error: Option<&'a str>,
    total: usize,
    items: Vec<PreviewItemView<'a>>,
}

#[derive(Serialize)]
pub(crate) struct BackupResultView<'a> {
    error: Option<&'a str>,
    path: Option<String>,
    sessions: usize,
    encrypted: bool,
    skipped: usize,
}

#[derive(Serialize)]
pub(crate) struct RestoreResultView<'a> {
    error: Option<&'a str>,
    sessions_in_archive: usize,
    added: usize,
    updated: usize,
    kept_local: usize,
    /// Older local items left in place because "replace older" was off.
    newer_in_backup: usize,
    deleted_locally: usize,
    failed: usize,
    settings_restored: bool,
    shortcuts_changed: bool,
    model_reload: bool,
}

#[derive(Serialize)]
pub(crate) struct MarkdownResultView<'a> {
    error: Option<&'a str>,
    folder: Option<String>,
    files: usize,
    skipped: usize,
}

#[derive(Serialize)]
pub(crate) struct StorageView<'a> {
    encrypt_at_rest: bool,
    encryption_working: bool,
    encryption_result: Option<EncryptionResultView<'a>>,
    retention: &'a Policy,
    retention_preview_working: bool,
    retention_preview: Option<RetentionPreviewView<'a>>,
    backup: &'static str,
    backup_result: Option<BackupResultView<'a>>,
    restore: &'static str,
    restore_result: Option<RestoreResultView<'a>>,
    markdown: &'static str,
    markdown_result: Option<MarkdownResultView<'a>>,
    dialog_open: bool,
}

#[derive(Serialize)]
pub(crate) struct NoticeView<'a> {
    sequence: u64,
    error: bool,
    message: &'a str,
}

#[derive(Serialize)]
pub(crate) struct Storage<'a> {
    storage: StorageView<'a>,
    library_notice: Option<NoticeView<'a>>,
}

pub(crate) fn storage(app: &App) -> Storage<'_> {
    let history = &app.history;
    let state = &history.storage;
    Storage {
        storage: StorageView {
            encrypt_at_rest: app.settings.encrypt_at_rest,
            encryption_working: state.encryption_working,
            encryption_result: history.protection.as_ref().map(|result| match result {
                Ok(report) => EncryptionResultView {
                    error: None,
                    protect: report.protect,
                    changed: report.changed,
                    failed: report.failed,
                },
                Err(error) => EncryptionResultView {
                    error: Some(error),
                    protect: app.settings.encrypt_at_rest,
                    changed: 0,
                    failed: 0,
                },
            }),
            retention: &app.settings.retention,
            retention_preview_working: state.retention_preview_working,
            retention_preview: history.retention_preview.as_ref().map(|result| {
                let policy = state.retention_previewed.as_ref();
                match result {
                    Ok(items) => RetentionPreviewView {
                        policy,
                        error: None,
                        total: items.len(),
                        items: items
                            .iter()
                            .take(MAX_PREVIEW_ITEMS)
                            .map(|item| PreviewItemView {
                                id: &item.id,
                                title: &item.title,
                                kind: item.kind,
                                updated_ms: item.updated_ms,
                            })
                            .collect(),
                    },
                    Err(error) => RetentionPreviewView {
                        policy,
                        error: Some(error),
                        total: 0,
                        items: Vec::new(),
                    },
                }
            }),
            backup: state.backup.key(),
            backup_result: history.backup.as_ref().map(|result| match result {
                Ok(created) => BackupResultView {
                    error: None,
                    path: Some(created.path.display().to_string()),
                    sessions: created.sessions,
                    encrypted: created.encrypted,
                    skipped: created.skipped.len(),
                },
                Err(error) => BackupResultView {
                    error: Some(error),
                    path: None,
                    sessions: 0,
                    encrypted: false,
                    skipped: 0,
                },
            }),
            restore: state.restore.key(),
            restore_result: history.restored.as_ref().map(|result| {
                let applied = state.restored_settings.unwrap_or_default();
                match result {
                    Ok(restored) => RestoreResultView {
                        error: None,
                        sessions_in_archive: restored.sessions_in_archive,
                        added: restored.added.len(),
                        updated: restored.updated.len(),
                        kept_local: restored.kept_local.len(),
                        newer_in_backup: restored.newer_in_backup.len(),
                        deleted_locally: restored.deleted_locally.len(),
                        failed: restored.failed.len(),
                        settings_restored: state.restored_settings.is_some(),
                        shortcuts_changed: applied.shortcuts,
                        model_reload: applied.model_reload,
                    },
                    Err(error) => RestoreResultView {
                        error: Some(error),
                        sessions_in_archive: 0,
                        added: 0,
                        updated: 0,
                        kept_local: 0,
                        newer_in_backup: 0,
                        deleted_locally: 0,
                        failed: 0,
                        settings_restored: false,
                        shortcuts_changed: false,
                        model_reload: false,
                    },
                }
            }),
            markdown: state.markdown.key(),
            markdown_result: history.markdown_export.as_ref().map(|result| match result {
                Ok(exported) => MarkdownResultView {
                    error: None,
                    folder: Some(exported.folder.display().to_string()),
                    files: exported.files,
                    skipped: exported.skipped.len(),
                },
                Err(error) => MarkdownResultView {
                    error: Some(error),
                    folder: None,
                    files: 0,
                    skipped: 0,
                },
            }),
            dialog_open: state.dialog_open,
        },
        library_notice: state.notice.as_ref().map(|notice| NoticeView {
            sequence: notice.sequence,
            error: notice.error,
            message: &notice.message,
        }),
    }
}

#[derive(Serialize)]
pub(crate) struct RecoveryView<'a> {
    /// Records skipped by the latest history pass (damaged, locked, newer).
    warnings: &'a [RecordWarning],
    /// Isolated files kept beside the history.
    quarantined: usize,
    /// The isolated files, once listed with `quarantine_list`.
    quarantine: &'a [Quarantined],
    listed: bool,
}

#[derive(Serialize)]
pub(crate) struct Recovery<'a> {
    recovery: RecoveryView<'a>,
}

pub(crate) fn recovery(app: &App) -> Recovery<'_> {
    Recovery {
        recovery: RecoveryView {
            warnings: &app.history.warnings,
            quarantined: app.history.quarantined,
            quarantine: &app.history.quarantine,
            listed: app.history.storage.quarantine_listed,
        },
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
