//! Full-text index of saved history.
//!
//! The index lives only in memory and is rebuilt from the saved records each
//! time Articulate starts, then updated as records are saved or deleted. It is
//! never written to disk, so it cannot leak plaintext when encryption at rest is
//! enabled, and it cannot become stale or corrupt across versions. The cost is a
//! rebuild at startup (the history list pass reads every record anyway) and
//! memory roughly proportional to the saved text.
//!
//! SQLite FTS5 with the trigram tokenizer matches any substring of three or
//! more characters in any script, ignoring case and diacritics. Shorter terms
//! fall back to a substring filter.
use super::{Kind, Session};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, InterruptHandle, params_from_iter, types::Value};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const MAX_HITS: usize = 200;
const MAX_QUERY_CHARS: usize = 160;

/// Stops one search. Cancelling also interrupts its SQLite statement, so a
/// superseded search releases the index promptly and never delays saves.
#[derive(Clone, Default)]
pub struct Cancel(Arc<CancelState>);

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    /// Set only while this search holds the index lock.
    running: Mutex<Option<InterruptHandle>>,
}

impl Cancel {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        if let Some(handle) = self.running().as_ref() {
            handle.interrupt();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    fn check(&self) -> Result<()> {
        ensure!(!self.is_cancelled(), Cancelled);
        Ok(())
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Option<InterruptHandle>> {
        self.0.running.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The search was cancelled before it finished.
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Search cancelled.")
    }
}
impl std::error::Error for Cancelled {}

/// Clears the interrupt handle before the index lock is released, so a late
/// cancel can never interrupt another caller's statement.
struct Running<'a>(&'a Cancel);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.0.running() = None;
    }
}

/// Highlight markers for one query. Each holds a random token, so stored text
/// cannot imitate them; a result that still does not align with the stored
/// text is shown without highlights rather than misread.
struct Markers {
    open: String,
    close: String,
}

impl Markers {
    fn new() -> Self {
        let token = super::new_id();
        Self {
            open: format!("\u{2}{token}["),
            close: format!("]{token}\u{3}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Title,
    /// Edited dictation or note text.
    Text,
    /// Recognized text before cleanup or manual edits.
    Original,
    PersonalNotes,
    /// Generated notes, highlights and action items.
    GeneratedNotes,
    CallTranscript,
    SpeakerNames,
}

impl Field {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Text => "text",
            Self::Original => "original",
            Self::PersonalNotes => "personal_notes",
            Self::GeneratedNotes => "generated_notes",
            Self::CallTranscript => "transcript",
            Self::SpeakerNames => "speaker_names",
        }
    }
    fn parse(key: &str) -> Option<Self> {
        [
            Self::Title,
            Self::Text,
            Self::Original,
            Self::PersonalNotes,
            Self::GeneratedNotes,
            Self::CallTranscript,
            Self::SpeakerNames,
        ]
        .into_iter()
        .find(|field| field.key() == key)
    }
}

pub(crate) fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Dictation => "dictation",
        Kind::Call => "call",
        Kind::Note => "note",
    }
}

fn parse_kind(key: &str) -> Kind {
    match key {
        "call" => Kind::Call,
        "note" => Kind::Note,
        _ => Kind::Dictation,
    }
}

/// Optional restrictions. Dates are inclusive Unix milliseconds of creation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub kind: Option<Kind>,
    /// Case-insensitive substring of a speaker label. Transcript hits must be
    /// spoken by a matching speaker; other hits must come from a session in
    /// which a matching speaker took part.
    pub speaker: Option<String>,
    /// Destination app basename recorded with a dictation, case-insensitive.
    pub app: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub session_id: String,
    pub title: String,
    pub kind: Kind,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub field: Field,
    /// Exact substring of the saved field around the first match.
    pub snippet: String,
    /// Byte offset of `snippet` within the full field text.
    pub snippet_start: usize,
    /// Byte ranges of matches within `snippet`.
    pub matches: Vec<(usize, usize)>,
    pub row: Option<usize>,
    pub speaker: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    /// More hits exist than were returned.
    pub partial: bool,
    pub indexed_sessions: usize,
    /// Records that could not be read and are therefore not searchable.
    pub unreadable_sessions: usize,
}

struct Inner {
    db: Mutex<Connection>,
    ready: AtomicBool,
    unreadable: AtomicUsize,
}

/// A shared handle. Cloning is cheap; all clones see the same index.
#[derive(Clone)]
pub struct SearchIndex(Arc<Inner>);

impl SearchIndex {
    pub fn in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().context("Could not create the search index")?;
        db.execute_batch(
            "CREATE TABLE fields(
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL,
                title TEXT NOT NULL,
                app TEXT,
                speakers TEXT NOT NULL,
                field TEXT NOT NULL,
                row_index INTEGER,
                speaker TEXT,
                speaker_folded TEXT,
                start_ms INTEGER,
                end_ms INTEGER
            );
            CREATE INDEX fields_session ON fields(session_id);
            CREATE VIRTUAL TABLE body USING fts5(text, tokenize = 'trigram remove_diacritics 1');",
        )
        .context("Could not create the search index")?;
        Ok(Self(Arc::new(Inner {
            db: Mutex::new(db),
            ready: AtomicBool::new(false),
            unreadable: AtomicUsize::new(0),
        })))
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set after the first complete history pass. Before that, results would
    /// silently miss records, so callers should fall back to a direct scan.
    pub fn is_ready(&self) -> bool {
        self.0.ready.load(Ordering::Acquire)
    }

    pub(crate) fn set_ready(&self, unreadable: usize) {
        self.0.unreadable.store(unreadable, Ordering::Release);
        self.0.ready.store(true, Ordering::Release);
    }

    /// Replace everything indexed for this session.
    pub fn upsert(&self, session: &Session) -> Result<()> {
        let mut db = self.db();
        let transaction = db.transaction()?;
        remove_in(&transaction, &session.id)?;
        if !session.is_collection || !session.personal_notes.is_empty() {
            insert_session(&transaction, session)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let db = self.db();
        remove_in(&db, id)
    }

    pub fn clear(&self) -> Result<()> {
        self.0.ready.store(false, Ordering::Release);
        self.db()
            .execute_batch("DELETE FROM fields; DELETE FROM body;")?;
        Ok(())
    }

    #[cfg(test)]
    pub fn search(&self, query: &str, filters: &Filters) -> Result<Found> {
        self.search_cancellable(query, filters, &Cancel::default())
    }

    /// Search until `cancel` is triggered. A cancelled search returns
    /// `Cancelled` and stops its SQLite work as soon as possible.
    pub fn search_cancellable(
        &self,
        query: &str,
        filters: &Filters,
        cancel: &Cancel,
    ) -> Result<Found> {
        let terms = terms(query)?;
        cancel.check()?;
        let db = self.db();
        // Publish the handle before checking again: a cancel that raced the
        // lock is either seen here or interrupts the statement below.
        *cancel.running() = Some(db.get_interrupt_handle());
        let _running = Running(cancel);
        cancel.check()?;
        let markers = Markers::new();
        let interrupted = |error: rusqlite::Error| -> anyhow::Error {
            if cancel.is_cancelled() {
                Cancelled.into()
            } else {
                error.into()
            }
        };
        let indexed_sessions: usize = db
            .query_row("SELECT COUNT(DISTINCT session_id) FROM fields", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(interrupted)? as usize;
        let mut sql = String::from(
            "SELECT f.session_id, f.title, f.kind, f.created_ms, f.updated_ms, f.field,
                    f.row_index, f.speaker, f.start_ms, f.end_ms, body.text, ",
        );
        let long: Vec<&String> = terms.iter().filter(|t| t.chars().count() >= 3).collect();
        let short: Vec<&String> = terms.iter().filter(|t| t.chars().count() < 3).collect();
        let mut values: Vec<Value> = Vec::new();
        if long.is_empty() {
            sql.push_str("NULL FROM body JOIN fields f ON f.id = body.rowid WHERE 1");
        } else {
            sql.push_str(
                "highlight(body, 0, ?, ?) FROM body JOIN fields f ON f.id = body.rowid WHERE body MATCH ?",
            );
            values.push(Value::Text(markers.open.clone()));
            values.push(Value::Text(markers.close.clone()));
            values.push(Value::Text(
                long.iter()
                    .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            ));
        }
        for term in &short {
            sql.push_str(" AND body.text LIKE ? ESCAPE '\\'");
            values.push(Value::Text(format!("%{}%", escape_like(term))));
        }
        if let Some(from) = filters.from_ms {
            sql.push_str(" AND f.created_ms >= ?");
            values.push(Value::Integer(clamp(from)));
        }
        if let Some(to) = filters.to_ms {
            sql.push_str(" AND f.created_ms <= ?");
            values.push(Value::Integer(clamp(to)));
        }
        if let Some(kind) = filters.kind {
            sql.push_str(" AND f.kind = ?");
            values.push(Value::Text(kind_key(kind).into()));
        }
        if let Some(app) = filters
            .app
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            sql.push_str(" AND f.app = ?");
            values.push(Value::Text(app.to_lowercase()));
        }
        if let Some(speaker) = filters
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let folded = speaker.to_lowercase();
            sql.push_str(
                " AND ((f.row_index IS NOT NULL AND instr(f.speaker_folded, ?) > 0)
                    OR (f.row_index IS NULL AND instr(f.speakers, ?) > 0))",
            );
            values.push(Value::Text(folded.clone()));
            values.push(Value::Text(folded));
        }
        sql.push_str(if long.is_empty() {
            " ORDER BY f.updated_ms DESC, f.id"
        } else {
            " ORDER BY bm25(body), f.updated_ms DESC, f.id"
        });
        sql.push_str(&format!(" LIMIT {}", MAX_HITS + 1));
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement
            .query(params_from_iter(values))
            .map_err(interrupted)?;
        let mut found = Found {
            indexed_sessions,
            unreadable_sessions: self.0.unreadable.load(Ordering::Acquire),
            ..Default::default()
        };
        while let Some(row) = rows.next().map_err(interrupted)? {
            cancel.check()?;
            if found.hits.len() == MAX_HITS {
                found.partial = true;
                break;
            }
            let text: String = row.get(10)?;
            let highlighted: Option<String> = row.get(11)?;
            let mut ranges = highlighted
                .as_deref()
                .and_then(|marked| marked_ranges(marked, &text, &markers))
                .unwrap_or_default();
            for term in &short {
                ranges.extend(find_all(&text, term));
            }
            ranges.sort_unstable();
            ranges.dedup();
            let (snippet_start, snippet_end) = window(&text, ranges.first().copied());
            let matches = ranges
                .iter()
                .filter(|(start, end)| *start >= snippet_start && *end <= snippet_end)
                .map(|(start, end)| (start - snippet_start, end - snippet_start))
                .collect();
            let field: String = row.get(5)?;
            let kind: String = row.get(2)?;
            found.hits.push(Hit {
                session_id: row.get(0)?,
                title: row.get(1)?,
                kind: parse_kind(&kind),
                created_ms: row.get::<_, i64>(3)?.max(0) as u64,
                updated_ms: row.get::<_, i64>(4)?.max(0) as u64,
                field: Field::parse(&field).unwrap_or(Field::Text),
                snippet: text[snippet_start..snippet_end].to_owned(),
                snippet_start,
                matches,
                row: row
                    .get::<_, Option<i64>>(6)?
                    .map(|index| index.max(0) as usize),
                speaker: row.get(7)?,
                start_ms: row.get::<_, Option<i64>>(8)?.map(|ms| ms.max(0) as u64),
                end_ms: row.get::<_, Option<i64>>(9)?.map(|ms| ms.max(0) as u64),
            });
        }
        Ok(found)
    }
}

fn clamp(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn remove_in(db: &Connection, id: &str) -> Result<()> {
    db.execute(
        "DELETE FROM body WHERE rowid IN (SELECT id FROM fields WHERE session_id = ?1)",
        [id],
    )?;
    db.execute("DELETE FROM fields WHERE session_id = ?1", [id])?;
    Ok(())
}

struct Entry<'a> {
    field: Field,
    text: std::borrow::Cow<'a, str>,
    row: Option<usize>,
    speaker: Option<String>,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
}

fn entries(session: &Session) -> Vec<Entry<'_>> {
    fn plain(field: Field, text: &str) -> Entry<'_> {
        Entry {
            field,
            text: text.into(),
            row: None,
            speaker: None,
            start_ms: None,
            end_ms: None,
        }
    }
    let mut entries = Vec::new();
    entries.push(plain(Field::Title, &session.title));
    if session.rows.is_empty() || session.kind != Kind::Call {
        entries.push(plain(Field::Text, &session.text));
    }
    if session.original.trim() != session.text.trim() && session.kind != Kind::Call {
        entries.push(plain(Field::Original, &session.original));
    }
    entries.push(plain(Field::PersonalNotes, &session.personal_notes));
    let mut generated = Vec::new();
    if let Some(summary) = &session.generated_summary {
        generated.extend(summary.items.iter().map(|item| item.text.as_str()));
    }
    if let Some(notes) = &session.notes {
        generated.extend(
            notes
                .highlights
                .iter()
                .chain(&notes.actions)
                .map(|quote| quote.text.as_str()),
        );
    }
    if !generated.is_empty() {
        entries.push(Entry {
            text: generated.join("\n").into(),
            ..plain(Field::GeneratedNotes, "")
        });
    }
    for (index, row) in session.rows.iter().enumerate() {
        entries.push(Entry {
            field: Field::CallTranscript,
            text: row.text.as_str().into(),
            row: Some(index),
            speaker: Some(crate::calls::label(row, &session.speaker_names)),
            start_ms: Some(row.start_ms),
            end_ms: Some(row.end_ms),
        });
    }
    let names = speaker_names(session);
    if !names.is_empty() {
        entries.push(Entry {
            text: names.join("\n").into(),
            ..plain(Field::SpeakerNames, "")
        });
    }
    entries.retain(|entry| !entry.text.trim().is_empty());
    entries
}

fn speaker_names(session: &Session) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|known| known == name) && names.len() < 256 {
            names.push(name.to_owned());
        }
    };
    for name in &session.speaker_names {
        add(name);
    }
    for row in &session.rows {
        if let Some(attribution) = &row.discord {
            for speaker in &attribution.speakers {
                add(&speaker.name);
            }
        }
    }
    names
}

fn insert_session(db: &Connection, session: &Session) -> Result<()> {
    let mut speakers = speaker_names(session);
    for row in &session.rows {
        let label = crate::calls::label(row, &session.speaker_names);
        if !speakers.contains(&label) && speakers.len() < 512 {
            speakers.push(label);
        }
    }
    let speakers = speakers.join("\n").to_lowercase();
    let app = session.metrics.app.as_deref().map(str::to_lowercase);
    let mut field_statement = db.prepare_cached(
        "INSERT INTO fields(session_id, kind, created_ms, updated_ms, title, app, speakers,
            field, row_index, speaker, speaker_folded, start_ms, end_ms)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    let mut body_statement = db.prepare_cached("INSERT INTO body(rowid, text) VALUES(?1, ?2)")?;
    for entry in entries(session) {
        field_statement.execute(rusqlite::params![
            session.id,
            kind_key(session.kind),
            clamp(session.created_ms),
            clamp(session.updated_ms),
            session.title,
            app,
            speakers,
            entry.field.key(),
            entry.row.map(|row| row as i64),
            entry.speaker,
            entry.speaker.as_deref().map(str::to_lowercase),
            entry.start_ms.map(clamp),
            entry.end_ms.map(clamp),
        ])?;
        body_statement.execute(rusqlite::params![db.last_insert_rowid(), entry.text])?;
    }
    Ok(())
}

/// Split a query into terms. A query in double quotes is one exact phrase.
fn terms(query: &str) -> Result<Vec<String>> {
    let query = query.trim();
    ensure!(
        !query.is_empty()
            && query.chars().count() <= MAX_QUERY_CHARS
            && !query.chars().any(char::is_control),
        "Search for a word or phrase of up to 160 characters."
    );
    if let Some(phrase) = query
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::trim)
        .filter(|phrase| !phrase.is_empty())
    {
        return Ok(vec![phrase.to_owned()]);
    }
    Ok(query.split_whitespace().map(str::to_owned).collect())
}

fn escape_like(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Byte ranges of `text` from FTS5 highlight output. `None` unless removing
/// the markers reproduces `text` exactly, so every range is a valid slice of
/// the stored text whatever characters it contains.
fn marked_ranges(marked: &str, text: &str, markers: &Markers) -> Option<Vec<(usize, usize)>> {
    let mut ranges = Vec::new();
    let mut rest = marked;
    let mut offset = 0;
    // Each plain or highlighted piece must equal the stored text at `offset`.
    let consume = |piece: &str, offset: &mut usize| -> Option<()> {
        (text.get(*offset..*offset + piece.len())? == piece).then_some(())?;
        *offset += piece.len();
        Some(())
    };
    while let Some(at) = rest.find(&markers.open) {
        consume(&rest[..at], &mut offset)?;
        rest = &rest[at + markers.open.len()..];
        let end = rest.find(&markers.close)?;
        let inside = &rest[..end];
        if inside.contains(&markers.open) {
            return None;
        }
        let start = offset;
        consume(inside, &mut offset)?;
        ranges.push((start, offset));
        rest = &rest[end + markers.close.len()..];
    }
    if rest.contains(&markers.close) {
        return None;
    }
    consume(rest, &mut offset)?;
    (offset == text.len()).then_some(ranges)
}

/// Case-insensitive occurrences of `needle`, as byte ranges of `text`.
fn find_all(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let mut matched = 0;
        let mut cursor = index;
        'outer: while cursor < chars.len() && matched < needle.len() {
            for lower in chars[cursor].1.to_lowercase() {
                if matched >= needle.len() || lower != needle[matched] {
                    break 'outer;
                }
                matched += 1;
            }
            cursor += 1;
        }
        if matched == needle.len() && cursor > index {
            let end = chars.get(cursor).map_or(text.len(), |(at, _)| *at);
            ranges.push((chars[index].0, end));
            index = cursor;
        } else {
            index += 1;
        }
    }
    ranges
}

/// A readable window around the first match: up to 70 characters before and
/// 120 after, always on character boundaries.
fn window(text: &str, first: Option<(usize, usize)>) -> (usize, usize) {
    let (match_start, match_end) = first
        .filter(|(start, end)| {
            start <= end && text.is_char_boundary(*start) && text.is_char_boundary(*end)
        })
        .unwrap_or((0, 0));
    let start = text[..match_start]
        .char_indices()
        .rev()
        .nth(69)
        .map_or(0, |(at, _)| at);
    let end = text[match_end..]
        .char_indices()
        .nth(120)
        .map_or(text.len(), |(at, _)| match_end + at);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calls::Row;

    fn row(start_ms: u64, speaker: i32, text: &str) -> Row {
        Row {
            cues: Vec::new(),
            start_ms,
            end_ms: start_ms + 2000,
            microphone: false,
            speakers: vec![speaker],
            discord: None,
            text: text.into(),
        }
    }

    fn sample() -> (SearchIndex, Session, Session) {
        let index = SearchIndex::in_memory().unwrap();
        let mut call = Session::new(Kind::Call);
        call.title = "Weekly planning".into();
        call.created_ms = 2_000;
        call.speaker_names[0] = "Élodie".into();
        call.speaker_names[1] = "Casey".into();
        call.rows = vec![
            row(0, 1, "The zephyr launch moves to Friday."),
            row(5000, 2, "I will draft the café résumé."),
        ];
        call.personal_notes = "Ask about budget for zephyr.".into();
        index.upsert(&call).unwrap();
        let mut dictation = Session::new(Kind::Dictation);
        dictation.created_ms = 1_000;
        dictation.title = "Message".into();
        dictation.text = "Send the Zephyr notes to the team.".into();
        dictation.original = "Uh send the zefir notes to the team.".into();
        dictation.metrics.app = Some("Editor.exe".into());
        index.upsert(&dictation).unwrap();
        index.set_ready(2);
        (index, call, dictation)
    }

    #[test]
    fn searches_every_field_with_exact_snippets_and_offsets() {
        let (index, call, dictation) = sample();
        let found = index.search("zephyr", &Filters::default()).unwrap();
        assert_eq!(found.unreadable_sessions, 2);
        assert_eq!(found.indexed_sessions, 2);
        let fields: Vec<_> = found.hits.iter().map(|hit| hit.field).collect();
        assert!(fields.contains(&Field::CallTranscript));
        assert!(fields.contains(&Field::PersonalNotes));
        assert!(fields.contains(&Field::Text));
        for hit in &found.hits {
            assert!(!hit.matches.is_empty());
            for (start, end) in &hit.matches {
                assert!(hit.snippet[*start..*end].eq_ignore_ascii_case("zephyr"));
            }
        }
        let turn = found
            .hits
            .iter()
            .find(|hit| hit.field == Field::CallTranscript)
            .unwrap();
        assert_eq!(turn.session_id, call.id);
        assert_eq!(turn.speaker.as_deref(), Some("Élodie"));
        assert_eq!(
            (turn.start_ms, turn.end_ms, turn.row),
            (Some(0), Some(2000), Some(0))
        );
        let original = index.search("zefir", &Filters::default()).unwrap();
        assert_eq!(original.hits.len(), 1);
        assert_eq!(original.hits[0].field, Field::Original);
        assert_eq!(original.hits[0].session_id, dictation.id);
        let names = index.search("elodie", &Filters::default()).unwrap();
        assert!(
            names
                .hits
                .iter()
                .any(|hit| hit.field == Field::SpeakerNames)
        );
        // Diacritics and case are folded; short terms still match as substrings.
        assert_eq!(
            index
                .search("CAFE resume", &Filters::default())
                .unwrap()
                .hits
                .len(),
            1
        );
        assert!(
            !index
                .search("to", &Filters::default())
                .unwrap()
                .hits
                .is_empty()
        );
    }

    #[test]
    fn filters_restrict_kind_dates_speakers_and_apps() {
        let (index, call, dictation) = sample();
        let only = |filters: Filters| {
            let mut ids: Vec<_> = index
                .search("zephyr", &filters)
                .unwrap()
                .hits
                .into_iter()
                .map(|hit| hit.session_id)
                .collect();
            ids.dedup();
            ids
        };
        assert_eq!(
            only(Filters {
                kind: Some(Kind::Dictation),
                ..Default::default()
            }),
            std::slice::from_ref(&dictation.id)
        );
        assert_eq!(
            only(Filters {
                from_ms: Some(1_500),
                ..Default::default()
            }),
            std::slice::from_ref(&call.id)
        );
        assert_eq!(
            only(Filters {
                to_ms: Some(1_500),
                ..Default::default()
            }),
            std::slice::from_ref(&dictation.id)
        );
        assert_eq!(
            only(Filters {
                app: Some("editor.EXE".into()),
                ..Default::default()
            }),
            std::slice::from_ref(&dictation.id)
        );
        let spoken = index
            .search(
                "zephyr",
                &Filters {
                    speaker: Some("casey".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        // Casey did not say "zephyr", but took part in the call with the note.
        assert!(
            spoken
                .hits
                .iter()
                .all(|hit| hit.field != Field::CallTranscript)
        );
        assert!(
            spoken
                .hits
                .iter()
                .any(|hit| hit.field == Field::PersonalNotes)
        );
    }

    #[test]
    fn upsert_replaces_and_remove_deletes_previous_text() {
        let (index, mut call, _) = sample();
        call.personal_notes = "Nothing relevant.".into();
        call.rows.clear();
        index.upsert(&call).unwrap();
        assert!(
            index
                .search("zephyr", &Filters::default())
                .unwrap()
                .hits
                .iter()
                .all(|hit| hit.session_id != call.id)
        );
        index.remove(&call.id).unwrap();
        assert!(
            index
                .search("relevant", &Filters::default())
                .unwrap()
                .hits
                .is_empty()
        );
    }

    #[test]
    fn query_syntax_cannot_inject_fts_operators() {
        let (index, ..) = sample();
        for query in [
            "\"",
            "zephyr OR",
            "NEAR(",
            "a\"b",
            "*",
            "col:zephyr",
            "%",
            "_",
        ] {
            index.search(query, &Filters::default()).unwrap();
        }
        assert!(index.search("", &Filters::default()).is_err());
        assert!(index.search("bad\u{1}", &Filters::default()).is_err());
        let phrase = index
            .search("\"launch moves\"", &Filters::default())
            .unwrap();
        assert_eq!(phrase.hits.len(), 1);
    }

    #[test]
    fn helpers_keep_utf8_boundaries() {
        assert_eq!(find_all("İstanbul and istanbul", "is"), vec![(14, 16)]);
        assert_eq!(find_all("Café CAFÉ", "café"), vec![(0, 5), (6, 11)]);
        let text = "é".repeat(300);
        let (start, end) = window(&text, Some((300, 302)));
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        let markers = Markers::new();
        let marked = format!("a{}bc{}d", markers.open, markers.close);
        assert_eq!(marked_ranges(&marked, "abcd", &markers), Some(vec![(1, 3)]));
        // Output that does not reproduce the stored text is never trusted.
        assert_eq!(marked_ranges(&marked, "abc", &markers), None);
        assert_eq!(marked_ranges(&marked, "xbcd", &markers), None);
        assert_eq!(window("abc", Some((1, 9))), (0, 3));
    }

    #[test]
    fn stored_control_characters_never_break_highlighting() {
        let index = SearchIndex::in_memory().unwrap();
        let mut session = Session::new(Kind::Dictation);
        session.title = "Controls".into();
        session.text = "\u{2}\u{e9}abc \u{3}abc\u{2}".into();
        index.upsert(&session).unwrap();
        index.set_ready(0);
        for query in ["abc", "\u{e9}ab", "abc ab"] {
            let found = index.search(query, &Filters::default()).unwrap();
            let hit = found
                .hits
                .iter()
                .find(|hit| hit.field == Field::Text)
                .unwrap();
            assert_eq!(hit.snippet, session.text);
            assert!(!hit.matches.is_empty(), "{query}");
            for (start, end) in &hit.matches {
                assert!(hit.snippet.get(*start..*end).is_some());
            }
        }
        let found = index.search("abc", &Filters::default()).unwrap();
        let hit = found
            .hits
            .iter()
            .find(|hit| hit.field == Field::Text)
            .unwrap();
        let matched: Vec<&str> = hit
            .matches
            .iter()
            .map(|(start, end)| &hit.snippet[*start..*end])
            .collect();
        assert_eq!(matched, ["abc", "abc"]);
    }

    #[test]
    fn cancelled_searches_stop_before_and_during_the_query() {
        let (index, _, _) = sample();
        let cancel = Cancel::default();
        cancel.cancel();
        let error = index
            .search_cancellable("zephyr", &Filters::default(), &cancel)
            .unwrap_err();
        assert!(error.is::<Cancelled>());
        // A long query holding the index is interrupted, and the lock is free
        // again for saves straight away.
        for i in 0..200 {
            let mut session = Session::new(Kind::Dictation);
            session.title = format!("Bulk {i}");
            session.text = "zephyr ".repeat(1000);
            index.upsert(&session).unwrap();
        }
        let cancel = Cancel::default();
        let running = {
            let index = index.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                index.search_cancellable("zep", &Filters::default(), &cancel)
            })
        };
        let started = std::time::Instant::now();
        while cancel.running().is_none() && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::yield_now();
        }
        cancel.cancel();
        let result = running.join().unwrap();
        if let Err(error) = &result {
            assert!(error.is::<Cancelled>(), "{error:#}");
        }
        assert!(cancel.running().is_none());
        let mut late = Session::new(Kind::Note);
        late.title = "After cancel".into();
        index.upsert(&late).unwrap();
        assert!(
            !index
                .search("after cancel", &Filters::default())
                .unwrap()
                .hits
                .is_empty()
        );
    }
}
