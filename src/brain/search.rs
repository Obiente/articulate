//! Bounded local phrase search. No embeddings, remote requests, or generated answers.
use crate::history::{History, Kind, Session};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::SystemTime,
};

const MAX_FILES: usize = 500;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;
const MAX_SCANNED_BYTES: usize = 32 * 1024 * 1024;
const MAX_MATCHES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Dictation,
    PersonalNotes,
    CallTranscript,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub session_id: String,
    pub title: String,
    pub created_ms: u64,
    pub field: Field,
    /// Exact substring of the saved text, never generated or paraphrased.
    pub excerpt: String,
    pub row: Option<usize>,
    pub speaker: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct Results {
    pub query: String,
    pub hits: Vec<Hit>,
    pub scanned_sessions: usize,
    pub unreadable_sessions: usize,
    /// More files/content/results exist than this bounded scan could inspect.
    pub partial: bool,
}

pub struct Job {
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<Results, String>>,
}
impl Job {
    pub fn try_recv(&self) -> std::result::Result<Result<Results, String>, TryRecvError> {
        self.result.try_recv()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(query: String) -> Result<Job> {
    let query = query.trim().to_owned();
    ensure!(
        !query.is_empty() && query.chars().count() <= 160 && !query.chars().any(char::is_control),
        "Search for a word or phrase of up to 160 characters."
    );
    let directory = crate::model::data_dir().join("history");
    let (tx, result) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("saved-content-search".into())
        .spawn(move || {
            let answer = scan(directory, query, &worker_cancel).map_err(|e| e.to_string());
            let _ = tx.send(answer);
        })?;
    Ok(Job { cancel, result })
}

fn scan(directory: PathBuf, query: String, cancel: &AtomicBool) -> Result<Results> {
    let mut result = Results {
        query: query.clone(),
        ..Default::default()
    };
    if !directory.exists() {
        return Ok(result);
    }
    let history = History::open(directory.clone())?;
    let mut files = Vec::new();
    for (index, entry) in fs::read_dir(&directory)?.enumerate() {
        ensure!(!cancel.load(Ordering::Acquire), "Search cancelled.");
        if index >= MAX_DIRECTORY_ENTRIES {
            result.partial = true;
            break;
        }
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                continue;
            }
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        files.push((
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            id.to_owned(),
        ));
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    result.partial |= files.len() > MAX_FILES;
    let mut scanned = 0;
    for (_, id) in files.into_iter().take(MAX_FILES) {
        ensure!(!cancel.load(Ordering::Acquire), "Search cancelled.");
        if result.hits.len() >= MAX_MATCHES {
            result.partial = true;
            break;
        }
        let session = match history.snapshot(&id) {
            Ok(session) => session,
            Err(_) => {
                result.unreadable_sessions += 1;
                continue;
            }
        };
        let bytes = session
            .title
            .len()
            .saturating_add(session.personal_notes.len())
            .saturating_add(if session.kind == Kind::Call {
                session.rows.iter().map(|r| r.text.len()).sum()
            } else {
                session.text.len()
            });
        if scanned + bytes > MAX_SCANNED_BYTES {
            result.partial = true;
            break;
        }
        scanned += bytes;
        result.scanned_sessions += 1;
        search_session(&session, &query, &mut result);
    }
    Ok(result)
}

fn search_session(session: &Session, query: &str, results: &mut Results) {
    let mut add = |text: &str, field: Field, row: Option<usize>| {
        if results.hits.len() >= MAX_MATCHES {
            results.partial = true;
            return;
        }
        let Some(excerpt) = excerpt(text, query) else {
            return;
        };
        let turn = row.and_then(|index| session.rows.get(index));
        results.hits.push(Hit {
            session_id: session.id.clone(),
            title: session.title.clone(),
            created_ms: session.created_ms,
            field,
            excerpt,
            row,
            speaker: turn.map(|turn| crate::calls::label(turn, &session.speaker_names)),
            start_ms: turn.map(|turn| turn.start_ms),
            end_ms: turn.map(|turn| turn.end_ms),
        });
    };
    add(&session.title, Field::Title, None);
    add(&session.personal_notes, Field::PersonalNotes, None);
    if session.kind == Kind::Call {
        for (index, row) in session.rows.iter().enumerate() {
            add(&row.text, Field::CallTranscript, Some(index));
        }
    } else {
        add(&session.text, Field::Dictation, None);
    }
}

fn excerpt(text: &str, query: &str) -> Option<String> {
    let folded = text.to_lowercase();
    let query = query.to_lowercase();
    let offset = folded.find(&query)?;
    // Lowercasing can change UTF-8 width, so folded offsets must never slice
    // the original directly (for example capital dotted I).
    let mut folded_at = 0;
    let mut match_start = 0;
    let mut match_end = 0;
    for (original_at, c) in text.char_indices() {
        let next = folded_at + c.to_lowercase().map(char::len_utf8).sum::<usize>();
        if folded_at <= offset && next > offset {
            match_start = original_at;
        }
        if folded_at < offset + query.len() {
            match_end = original_at + c.len_utf8();
        }
        folded_at = next;
        if folded_at >= offset + query.len() {
            break;
        }
    }
    let start = text[..match_start]
        .char_indices()
        .rev()
        .nth(69)
        .map_or(0, |(at, _)| at);
    let end = text[match_end..]
        .char_indices()
        .nth(120)
        .map_or(text.len(), |(at, _)| match_end + at);
    Some(text[start..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_excerpts_survive_casefold_expansion_and_unicode_boundaries() {
        let text = "İstanbul café planning. Send the résumé tomorrow.";
        for query in ["İSTANBUL", "CAFÉ", "résumé"] {
            let found = excerpt(text, query).unwrap();
            assert!(text.contains(&found));
            assert!(found.to_lowercase().contains(&query.to_lowercase()));
        }
        assert!(excerpt(text, "absent").is_none());
    }
    #[test]
    fn saved_personal_notes_and_call_turns_return_separate_source_hits() {
        let mut session = Session::new(Kind::Call);
        session.title = "Planning".into();
        session.personal_notes = "Ask about the zephyr launch.".into();
        session.rows.push(crate::calls::Row {
            start_ms: 1500,
            end_ms: 4500,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "The zephyr launch is Friday.".into(),
        });
        let mut results = Results::default();
        search_session(&session, "zephyr", &mut results);
        assert_eq!(results.hits.len(), 2);
        assert_eq!(results.hits[0].field, Field::PersonalNotes);
        assert_eq!(results.hits[1].field, Field::CallTranscript);
        assert_eq!(results.hits[1].start_ms, Some(1500));
        assert_eq!(results.hits[1].row, Some(0));
    }

    #[test]
    fn full_saved_text_beyond_preview_is_found_and_deletion_removes_it() {
        let temporary = std::env::temp_dir();
        let directory = temporary.join(format!(
            "articulate-search-test-{}",
            crate::discord::plugin::new_token().unwrap()
        ));
        assert_eq!(directory.parent(), Some(temporary.as_path()));
        let history = History::open(directory.clone()).unwrap();
        let mut session = Session::new(Kind::Dictation);
        session.title = "A synthetic session".into();
        session.text = format!(
            "{}The zephyr release is Friday.",
            "Earlier discussion. ".repeat(30)
        );
        history.save(session.clone()).unwrap();
        let result = scan(directory.clone(), "zephyr".into(), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].session_id, session.id);
        assert!(session.text.contains(&result.hits[0].excerpt));
        assert!(result.hits[0].excerpt.contains("zephyr"));
        assert!(!result.partial);
        // A save can have staged temporary/backup files beside the committed
        // JSON. Searching must neither consume nor remove those files.
        let temporary_save = directory.join(format!("{}.tmp", session.id));
        let backup = directory.join(format!("{}.bak", session.id));
        fs::write(&temporary_save, b"an in-flight save").unwrap();
        fs::write(&backup, b"the writer's backup").unwrap();
        let result = scan(directory.clone(), "zephyr".into(), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(fs::read(&temporary_save).unwrap(), b"an in-flight save");
        assert_eq!(fs::read(&backup).unwrap(), b"the writer's backup");
        history.delete(&session.id).unwrap();
        let result = scan(directory.clone(), "zephyr".into(), &AtomicBool::new(false)).unwrap();
        assert!(result.hits.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
