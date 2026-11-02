//! Renderer state publication: named sections, revisions and row splices.
//!
//! Every value the renderer reads belongs to exactly one section. A pass
//! serializes each section's typed view and compares the bytes with those last
//! published. Only sections whose bytes changed receive a new revision and are
//! sent. Live transcript rows are compared row by row and sent as splices
//! relative to the previous revision of their section.

use super::view::rows::RowView;
use crate::calls;
use serde::Serialize;
use serde_json::value::RawValue;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

/// Background sections are compared at least this often.
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(1);

/// When a section is compared with its last published value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cadence {
    /// Small values, compared on every publish tick.
    Live,
    /// Large lists and document bodies. Compared about once a second, after
    /// every action, and when an earlier section asks for it in the same pass.
    Background,
}

/// Which sections a pass compares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pass {
    /// Periodic: live sections, plus background sections when due.
    Tick,
    /// After an action or lifecycle transition: every section.
    Full,
}

/// Immutable, cheaply shared JSON text produced by serializing a view.
#[derive(Clone)]
pub(crate) struct Json(Arc<RawValue>);

impl Json {
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let text = String::from_utf8(bytes).ok()?;
        RawValue::from_string(text)
            .ok()
            .map(|raw| Self(Arc::from(raw)))
    }

    fn of(value: &impl Serialize) -> Option<Self> {
        serde_json::to_vec(value).ok().and_then(Self::from_bytes)
    }

    #[cfg(test)]
    pub(crate) fn value(&self) -> serde_json::Value {
        serde_json::from_str(self.0.get()).unwrap()
    }
}

impl Serialize for Json {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// Replace rows from `start` onward with `rows`.
#[derive(Clone, Serialize)]
pub(crate) struct Splice {
    pub start: usize,
    pub rows: Vec<Json>,
}

/// One section in a renderer update.
///
/// `data` is the complete section: an object whose keys merge into renderer
/// state. Row sections may instead carry `splices` that apply on top of the
/// renderer's copy at revision `base`.
#[derive(Clone, Serialize)]
pub(crate) struct Section {
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub splices: Option<BTreeMap<&'static str, Splice>>,
}

/// Payload of the `desktop-state` event and the `desktop_snapshot` command.
#[derive(Clone, Serialize)]
pub(crate) struct StateUpdate {
    /// The newest revision of any section.
    pub revision: u64,
    pub sections: BTreeMap<&'static str, Section>,
}

impl StateUpdate {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

enum Change {
    Data {
        name: &'static str,
        revision: u64,
        data: Json,
    },
    Rows {
        name: &'static str,
        revision: u64,
        base: u64,
        splices: BTreeMap<&'static str, Splice>,
    },
}

/// Sections that changed during one pass.
#[derive(Default)]
pub(crate) struct Changes(Vec<Change>);

impl Changes {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

enum Content {
    Data(Json),
    Rows {
        streams: BTreeMap<&'static str, Vec<Json>>,
        last: Option<(u64, BTreeMap<&'static str, Splice>)>,
    },
}

struct Entry {
    revision: u64,
    content: Content,
}

/// The latest published sections, shared with IPC handlers.
#[derive(Default)]
pub(crate) struct Published {
    revision: u64,
    sections: BTreeMap<&'static str, Entry>,
}

impl Published {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Store a pass's changes and return them as a renderer update.
    pub(crate) fn apply(&mut self, changes: Changes) -> StateUpdate {
        let mut sections = BTreeMap::new();
        for change in changes.0 {
            match change {
                Change::Data {
                    name,
                    revision,
                    data,
                } => {
                    self.revision = self.revision.max(revision);
                    sections.insert(
                        name,
                        Section {
                            revision,
                            data: Some(data.clone()),
                            base: None,
                            splices: None,
                        },
                    );
                    self.sections.insert(
                        name,
                        Entry {
                            revision,
                            content: Content::Data(data),
                        },
                    );
                }
                Change::Rows {
                    name,
                    revision,
                    base,
                    splices,
                } => {
                    self.revision = self.revision.max(revision);
                    let entry = self.sections.entry(name).or_insert_with(|| Entry {
                        revision: 0,
                        content: Content::Rows {
                            streams: BTreeMap::new(),
                            last: None,
                        },
                    });
                    if !matches!(entry.content, Content::Rows { .. }) {
                        entry.content = Content::Rows {
                            streams: BTreeMap::new(),
                            last: None,
                        };
                    }
                    let Content::Rows { streams, last } = &mut entry.content else {
                        unreachable!("row content was just ensured");
                    };
                    for (key, splice) in &splices {
                        let stream = streams.entry(key).or_default();
                        stream.truncate(splice.start);
                        stream.extend(splice.rows.iter().cloned());
                    }
                    entry.revision = revision;
                    *last = Some((base, splices.clone()));
                    sections.insert(
                        name,
                        Section {
                            revision,
                            data: None,
                            base: Some(base),
                            splices: Some(splices),
                        },
                    );
                }
            }
        }
        StateUpdate {
            revision: self.revision,
            sections,
        }
    }

    /// Sections newer than the renderer's revisions, or every section when the
    /// renderer has none. Row sections are sent as splices when the renderer is
    /// exactly one revision behind, otherwise in full.
    pub(crate) fn since(&self, known: Option<&HashMap<String, u64>>) -> StateUpdate {
        let mut sections = BTreeMap::new();
        for (name, entry) in &self.sections {
            let renderer = known.and_then(|known| known.get(*name)).copied();
            if renderer == Some(entry.revision) {
                continue;
            }
            let section = match &entry.content {
                Content::Data(data) => Section {
                    revision: entry.revision,
                    data: Some(data.clone()),
                    base: None,
                    splices: None,
                },
                Content::Rows {
                    last: Some((base, splices)),
                    ..
                } if renderer == Some(*base) => Section {
                    revision: entry.revision,
                    data: None,
                    base: Some(*base),
                    splices: Some(splices.clone()),
                },
                Content::Rows { streams, .. } => Section {
                    revision: entry.revision,
                    data: Json::of(streams),
                    base: None,
                    splices: None,
                },
            };
            sections.insert(*name, section);
        }
        StateUpdate {
            revision: self.revision,
            sections,
        }
    }
}

/// A live transcript row list published under `key`.
pub(crate) struct RowStream<'a> {
    pub key: &'static str,
    pub rows: &'a [calls::Row],
    pub names: &'a [String],
    pub provisional: &'a dyn Fn(usize) -> bool,
}

#[derive(Default)]
struct RowCache {
    names: Vec<String>,
    rows: Vec<(calls::Row, bool)>,
}

fn same_row(left: &calls::Row, right: &calls::Row) -> bool {
    left.start_ms == right.start_ms
        && left.end_ms == right.end_ms
        && left.microphone == right.microphone
        && left.speakers == right.speakers
        && left.discord == right.discord
        && left.text == right.text
        && left.cues == right.cues
}

/// Controller-side change detection. Owned by the controller thread.
#[derive(Default)]
pub(crate) struct Tracker {
    revision: u64,
    bytes: HashMap<&'static str, Vec<u8>>,
    revisions: HashMap<&'static str, u64>,
    rows: HashMap<&'static str, RowCache>,
    scratch: Vec<u8>,
    background_checked: Option<Instant>,
}

impl Tracker {
    /// Begin a comparison pass.
    pub(crate) fn pass(&mut self, pass: Pass) -> Collector<'_> {
        let background = pass == Pass::Full
            || self
                .background_checked
                .is_none_or(|checked| checked.elapsed() >= BACKGROUND_INTERVAL);
        if background {
            self.background_checked = Some(Instant::now());
        }
        Collector {
            sink: Sink::Track {
                tracker: self,
                changes: Changes::default(),
            },
            background,
        }
    }

    /// Compare one section outside a view pass, such as controller health.
    pub(crate) fn single<T: Serialize>(&mut self, name: &'static str, value: &T) -> Changes {
        let mut changes = Changes::default();
        self.compare(name, value, &mut changes);
        changes
    }

    fn compare<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
        changes: &mut Changes,
    ) -> bool {
        self.scratch.clear();
        if let Err(error) = serde_json::to_writer(&mut self.scratch, value) {
            log::error!("Could not serialize the {name} section: {error}");
            return false;
        }
        let previous = self.bytes.entry(name).or_default();
        if self.revisions.contains_key(name) && *previous == self.scratch {
            return false;
        }
        std::mem::swap(previous, &mut self.scratch);
        let Some(data) = Json::from_bytes(previous.clone()) else {
            log::error!("The {name} section produced invalid JSON");
            return false;
        };
        self.revision += 1;
        self.revisions.insert(name, self.revision);
        changes.0.push(Change::Data {
            name,
            revision: self.revision,
            data,
        });
        true
    }

    fn compare_rows(
        &mut self,
        name: &'static str,
        streams: &[RowStream<'_>],
        changes: &mut Changes,
    ) -> bool {
        let first = !self.revisions.contains_key(name);
        let mut splices = BTreeMap::new();
        for stream in streams {
            let cache = self.rows.entry(stream.key).or_default();
            let renamed = cache.names.as_slice() != stream.names;
            let start = if renamed {
                0
            } else {
                cache
                    .rows
                    .iter()
                    .zip(stream.rows)
                    .enumerate()
                    .position(|(index, ((old, provisional), new))| {
                        !same_row(old, new) || *provisional != (stream.provisional)(index)
                    })
                    .unwrap_or_else(|| cache.rows.len().min(stream.rows.len()))
            };
            if !first && !renamed && start == cache.rows.len() && start == stream.rows.len() {
                continue;
            }
            cache.rows.truncate(start);
            let mut rows = Vec::with_capacity(stream.rows.len() - start);
            for (offset, row) in stream.rows[start..].iter().enumerate() {
                let provisional = (stream.provisional)(start + offset);
                let Some(json) = Json::of(&RowView::new(row, stream.names, provisional)) else {
                    // Force a complete resend once serialization recovers.
                    log::error!("Could not serialize a transcript row");
                    cache.names.clear();
                    cache.rows.clear();
                    return false;
                };
                rows.push(json);
                cache.rows.push((row.clone(), provisional));
            }
            if renamed {
                cache.names = stream.names.to_vec();
            }
            splices.insert(stream.key, Splice { start, rows });
        }
        if splices.is_empty() {
            return false;
        }
        let base = self.revisions.get(name).copied().unwrap_or(0);
        self.revision += 1;
        self.revisions.insert(name, self.revision);
        changes.0.push(Change::Rows {
            name,
            revision: self.revision,
            base,
            splices,
        });
        true
    }
}

enum Sink<'a> {
    Track {
        tracker: &'a mut Tracker,
        changes: Changes,
    },
    /// Builds every section without change detection, for contract tests.
    #[cfg(test)]
    Snapshot(Vec<(&'static str, serde_json::Value)>),
}

/// Receives each section's view during a pass. See `view::collect`.
pub(crate) struct Collector<'a> {
    sink: Sink<'a>,
    background: bool,
}

impl<'a> Collector<'a> {
    /// Build every section without change detection. Used by contract tests.
    #[cfg(test)]
    pub(crate) fn snapshot() -> Self {
        Self {
            sink: Sink::Snapshot(Vec::new()),
            background: true,
        }
    }

    /// Offer a section. `view` runs only when the section is compared in this
    /// pass. Returns whether the section changed.
    pub(crate) fn section<T: Serialize>(
        &mut self,
        name: &'static str,
        cadence: Cadence,
        view: impl FnOnce() -> T,
    ) -> bool {
        if cadence == Cadence::Background && !self.background {
            return false;
        }
        match &mut self.sink {
            Sink::Track { tracker, changes } => tracker.compare(name, &view(), changes),
            #[cfg(test)]
            Sink::Snapshot(sections) => {
                sections.push((name, serde_json::to_value(view()).unwrap()));
                true
            }
        }
    }

    /// Offer a section of live transcript rows, published as splices.
    pub(crate) fn rows(&mut self, name: &'static str, streams: &[RowStream<'_>]) -> bool {
        match &mut self.sink {
            Sink::Track { tracker, changes } => tracker.compare_rows(name, streams, changes),
            #[cfg(test)]
            Sink::Snapshot(sections) => {
                let mut data = serde_json::Map::new();
                for stream in streams {
                    let rows = stream
                        .rows
                        .iter()
                        .enumerate()
                        .map(|(index, row)| {
                            serde_json::to_value(RowView::new(
                                row,
                                stream.names,
                                (stream.provisional)(index),
                            ))
                            .unwrap()
                        })
                        .collect();
                    data.insert(stream.key.into(), serde_json::Value::Array(rows));
                }
                sections.push((name, serde_json::Value::Object(data)));
                true
            }
        }
    }

    /// Compare background sections for the rest of this pass, for example
    /// because the open document changed.
    pub(crate) fn include_background(&mut self) {
        self.background = true;
    }

    /// The changes found in this pass.
    pub(crate) fn finish(self) -> Changes {
        match self.sink {
            Sink::Track { changes, .. } => changes,
            #[cfg(test)]
            Sink::Snapshot(_) => Changes::default(),
        }
    }

    /// Every section built by a snapshot collector, in registry order.
    #[cfg(test)]
    pub(crate) fn into_sections(self) -> Vec<(&'static str, serde_json::Value)> {
        match self.sink {
            Sink::Snapshot(sections) => sections,
            Sink::Track { .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start_ms: u64, text: &str) -> calls::Row {
        calls::Row {
            start_ms,
            end_ms: start_ms + 1000,
            microphone: true,
            speakers: Vec::new(),
            discord: None,
            text: text.into(),
            cues: Vec::new(),
        }
    }

    #[test]
    fn unchanged_sections_keep_their_revision_and_are_not_resent() {
        let mut tracker = Tracker::default();
        let mut published = Published::default();
        let first = published.apply(tracker.single("shell", &serde_json::json!({"ready": true})));
        assert_eq!(first.sections["shell"].revision, 1);
        let again = published.apply(tracker.single("shell", &serde_json::json!({"ready": true})));
        assert!(again.is_empty());
        let changed =
            published.apply(tracker.single("shell", &serde_json::json!({"ready": false})));
        assert_eq!(changed.sections["shell"].revision, 2);
        let since = published.since(Some(&HashMap::from([("shell".to_owned(), 2)])));
        assert!(since.is_empty());
        assert_eq!(published.since(None).sections.len(), 1);
    }

    #[test]
    fn background_sections_wait_for_their_cadence_or_a_full_pass() {
        let mut tracker = Tracker::default();
        let mut pass = tracker.pass(Pass::Full);
        assert!(pass.section("history", Cadence::Background, || 1));
        pass.finish();
        let mut pass = tracker.pass(Pass::Tick);
        let mut built = false;
        assert!(!pass.section("history", Cadence::Background, || {
            built = true;
            2
        }));
        assert!(!built, "a background view is not built before it is due");
        pass.include_background();
        assert!(pass.section("history", Cadence::Background, || 2));
    }

    #[test]
    fn live_rows_are_sent_as_splices_and_resynchronize_in_full() {
        let mut tracker = Tracker::default();
        let mut published = Published::default();
        let names = calls::empty_speaker_names();
        let committed = |_: usize| false;
        let mut rows = vec![row(0, "First."), row(1000, "Second.")];
        let publish = |tracker: &mut Tracker, published: &mut Published, rows: &[calls::Row]| {
            let mut pass = tracker.pass(Pass::Full);
            pass.rows(
                "live_rows",
                &[RowStream {
                    key: "call_rows",
                    rows,
                    names: &names,
                    provisional: &committed,
                }],
            );
            published.apply(pass.finish())
        };
        let first = publish(&mut tracker, &mut published, &rows);
        let section = &first.sections["live_rows"];
        assert_eq!(section.base, Some(0));
        assert_eq!(section.splices.as_ref().unwrap()["call_rows"].rows.len(), 2);
        rows[1].text = "Second, revised.".into();
        rows.push(row(2000, "Third."));
        let second = publish(&mut tracker, &mut published, &rows);
        let section = &second.sections["live_rows"];
        let splice = &section.splices.as_ref().unwrap()["call_rows"];
        assert_eq!(
            (section.base, splice.start, splice.rows.len()),
            (Some(1), 1, 2)
        );
        assert_eq!(splice.rows[0].value()["text"], "Second, revised.");
        assert!(publish(&mut tracker, &mut published, &rows).is_empty());
        let one_behind = published.since(Some(&HashMap::from([("live_rows".to_owned(), 1)])));
        assert!(one_behind.sections["live_rows"].splices.is_some());
        let stale = published.since(Some(&HashMap::from([("live_rows".to_owned(), 0)])));
        let full = stale.sections["live_rows"].data.as_ref().unwrap().value();
        assert_eq!(full["call_rows"].as_array().unwrap().len(), 3);
        assert_eq!(full["call_rows"][2]["text"], "Third.");
        rows.clear();
        let cleared = publish(&mut tracker, &mut published, &rows);
        let splice = &cleared.sections["live_rows"].splices.as_ref().unwrap()["call_rows"];
        assert_eq!((splice.start, splice.rows.len()), (0, 0));
    }
}
