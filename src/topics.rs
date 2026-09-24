//! Topic documents reference independently stored source sessions.
use crate::{
    classification::{Segment, Transcript},
    history::{Kind, Session},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_SOURCES: usize = 256;
pub const MAX_CANDIDATES: usize = 24;

#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub created_ms: u64,
    pub text: String,
    pub original: String,
    pub rows: Vec<crate::calls::Row>,
    pub speaker_names: Vec<String>,
}
impl From<Session> for Source {
    fn from(session: Session) -> Self {
        Self {
            id: session.id,
            title: session.title,
            kind: session.kind,
            created_ms: session.created_ms,
            text: session.text,
            original: session.original,
            rows: session.rows,
            speaker_names: session.speaker_names,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Destination {
    Existing(String),
    New(String),
    Unfiled,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct Moved {
    pub source: Session,
    pub previous_topic_id: Option<String>,
    pub collections: Vec<Session>,
}

/// Stable source IDs keep generated citations bound to their original recording
/// when another recording is moved, inserted, or reordered in the collection.
pub fn transcript(session: &Session) -> Transcript {
    let segments = session
        .sources
        .iter()
        .flat_map(|source| {
            if source.rows.is_empty() {
                if source.text.trim().is_empty() {
                    return Vec::new();
                }
                vec![Segment {
                    id: format!("{}:text", source.id),
                    start_ms: 0,
                    end_ms: 0,
                    speaker: None,
                    text: source.text.clone(),
                    context: None,
                }]
            } else {
                source
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| !row.text.trim().is_empty() || !row.cues.is_empty())
                    .map(|(index, _)| {
                        Segment::from_call_row(
                            &source.rows,
                            &source.speaker_names,
                            index,
                            format!("{}:row-{index}", source.id),
                        )
                    })
                    .collect()
            }
        })
        .collect();
    Transcript {
        id: session.id.clone(),
        title: session.title.clone(),
        goal: String::new(),
        segments,
    }
}

pub(crate) fn hydrate(mut collection: Session, mut sources: Vec<Session>) -> Result<Session> {
    ensure!(
        collection.is_collection && collection.kind == Kind::Note,
        "Choose a topic note."
    );
    ensure!(
        sources.len() <= MAX_SOURCES,
        "This topic has too many sources. Move some into another note."
    );
    sources.sort_by(|a, b| {
        a.created_ms
            .cmp(&b.created_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    collection.sources = sources.into_iter().map(Source::from).collect();
    let input = transcript(&collection);
    collection.source_revision =
        format!("{:x}", Sha256::digest(serde_json::to_vec(&input.segments)?));
    collection.rows = collection
        .sources
        .iter()
        .flat_map(|source| source.rows.clone())
        .collect();
    collection.text = collection
        .sources
        .iter()
        .map(|source| source.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    collection.original.clear();
    // Only remove exact, unedited generated paragraphs whose cited recording
    // left the topic. Human writing and all remaining sources stay untouched.
    if let Some(draft) = collection.generated_summary.as_mut() {
        let ids: std::collections::HashSet<_> =
            input.segments.iter().map(|s| s.id.as_str()).collect();
        let removed: Vec<_> = draft
            .items
            .iter()
            .filter(|item| {
                item.sources
                    .iter()
                    .any(|citation| !ids.contains(citation.source_id.as_str()))
            })
            .map(|item| item.text.as_str())
            .collect();
        let mut blocks: Vec<_> = collection
            .personal_notes
            .split("\n\n")
            .map(str::to_owned)
            .collect();
        let mut processed = std::collections::HashSet::new();
        for text in removed {
            // The generated baseline inserted at most one copy. An identical
            // paragraph the user copied elsewhere is still personal writing.
            let retained_elsewhere = draft.items.iter().any(|item| {
                item.text == text
                    && item
                        .sources
                        .iter()
                        .all(|citation| ids.contains(citation.source_id.as_str()))
            });
            if !retained_elsewhere
                && processed.insert(text)
                && let Some(index) = blocks.iter().position(|block| block == text)
            {
                blocks.remove(index);
            }
        }
        collection.personal_notes = blocks.join("\n\n");
        draft.items.retain(|item| {
            item.sources
                .iter()
                .all(|citation| ids.contains(citation.source_id.as_str()))
        });
    }
    Ok(collection)
}
