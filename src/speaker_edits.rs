//! Speaker corrections for call transcripts: rename, merge, reassign and undo.
//!
//! A correction rewrites the speaker identity of transcript rows (the
//! microphone flag, acoustic speaker numbers and named participants) and the
//! session's acoustic speaker names. Every label derived from rows (transcript
//! text, exports, search, notes citations) therefore shows the corrected
//! speaker through the existing paths, including in older versions of the app
//! that do not know about corrections.
//!
//! Each edit keeps a compact inverse: the previous identity of the rows it
//! changed and, when they changed, the previous acoustic names. The most recent
//! edits are saved with the session so they can be undone after reopening it.
//! Named participants are restored without their Discord identifiers, which are
//! never written to history.
use crate::{
    calls::Row,
    discord_attribution::{Attribution, NamedSpeaker},
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Undo depth kept per session.
pub const MAX_UNDO: usize = 20;
/// Longest accepted speaker name, in characters.
pub const MAX_NAME_CHARS: usize = 80;

/// One voice in a row. A row with several voices is overlapping speech.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Voice {
    You,
    /// Session-local diarizer speaker; 0 is other audio, outside 1..=8 is uncertain.
    Acoustic(i32),
    /// A named participant: Discord activity or a name chosen by the user.
    Named(NamedSpeaker),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    You,
    /// A numbered voice from acoustic speaker identification.
    Voice,
    /// A Discord participant matched by speaking activity.
    Participant,
    /// A name the user assigned.
    Person,
    /// Other audio or an uncertain speaker: not one person.
    Other,
}

/// One distinct speaker in a transcript, in order of first appearance.
#[derive(Clone, Debug)]
pub struct Speaker {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub turns: usize,
    pub voice: Voice,
}

impl Speaker {
    /// Whether this speaker can be renamed as one person.
    pub fn renamable(&self) -> bool {
        matches!(self.kind, Kind::Voice | Kind::Participant | Kind::Person)
    }
}

/// The previous identity of one changed row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RowBefore {
    pub index: usize,
    /// Guards against restoring onto a different row.
    pub start_ms: u64,
    pub microphone: bool,
    pub speakers: Vec<i32>,
    pub discord: Option<Attribution>,
}

impl RowBefore {
    fn of(index: usize, row: &Row) -> Self {
        Self {
            index,
            start_ms: row.start_ms,
            microphone: row.microphone,
            speakers: row.speakers.clone(),
            discord: row.discord.clone(),
        }
    }
}

/// A rule applied to rows that arrive later in a live call, so a merge or a
/// participant rename also covers speech that has not been transcribed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRule {
    pub from: String,
    pub to: Voice,
}

/// The inverse of one correction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edit {
    /// What the edit did, for the undo control: "Renamed Speaker 2 to Morgan".
    pub summary: String,
    pub at_ms: u64,
    /// Acoustic speaker names before the edit, when it changed them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowBefore>,
    /// Only meaningful while the call is recording.
    #[serde(skip)]
    pub rule: Option<LiveRule>,
    /// A known-people change made with this edit: the Discord user ID and the
    /// entry before it, restored on undo.
    #[serde(skip)]
    pub registry: Option<(String, Option<crate::people::Person>)>,
}

/// What selected rows are reassigned to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Existing(String),
    New(String),
}

/// The voices of a row, in display order. Empty for a row without attribution.
pub fn voices(row: &Row) -> Vec<Voice> {
    if row.microphone {
        return vec![Voice::You];
    }
    if let Some(attribution) = &row.discord
        && !attribution.speakers.is_empty()
    {
        return attribution
            .speakers
            .iter()
            .cloned()
            .map(Voice::Named)
            .collect();
    }
    row.speakers.iter().map(|&id| Voice::Acoustic(id)).collect()
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A stable, opaque key for a voice within one transcript. Named keys are
/// digests so Discord identifiers never reach the renderer.
pub fn key(voice: &Voice) -> String {
    match voice {
        Voice::You => "you".into(),
        Voice::Acoustic(0) => "other".into(),
        Voice::Acoustic(id @ 1..=8) => format!("s{id}"),
        Voice::Acoustic(_) => "uncertain".into(),
        Voice::Named(named) => {
            let identity = if !named.id.is_empty() {
                format!("id:{}", named.id)
            } else if let Some(avatar) = &named.avatar {
                format!("avatar:{}", avatar.user_id)
            } else {
                format!("name:{}", named.name)
            };
            format!("n:{}", digest(&identity))
        }
    }
}

/// The label a voice shows; matches `calls::speaker_labels`.
pub fn label(voice: &Voice, names: &[String]) -> String {
    match voice {
        Voice::You => "You".into(),
        Voice::Acoustic(0) => "Other audio".into(),
        Voice::Acoustic(id @ 1..=8) => names
            .get(*id as usize - 1)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Speaker {id}")),
        Voice::Acoustic(_) => "Uncertain speaker".into(),
        Voice::Named(named) => named.name.clone(),
    }
}

fn kind(voice: &Voice) -> Kind {
    match voice {
        Voice::You => Kind::You,
        Voice::Acoustic(1..=8) => Kind::Voice,
        Voice::Acoustic(_) => Kind::Other,
        Voice::Named(named) if !named.id.is_empty() || named.avatar.is_some() => Kind::Participant,
        Voice::Named(_) => Kind::Person,
    }
}

/// Distinct speakers in order of first appearance.
pub fn roster(rows: &[Row], names: &[String]) -> Vec<Speaker> {
    let mut speakers: Vec<Speaker> = Vec::new();
    for row in rows {
        for voice in voices(row) {
            let key = key(&voice);
            if let Some(existing) = speakers.iter_mut().find(|s| s.key == key) {
                existing.turns += 1;
            } else {
                speakers.push(Speaker {
                    label: label(&voice, names),
                    kind: kind(&voice),
                    key,
                    turns: 1,
                    voice,
                });
            }
        }
    }
    speakers
}

/// The keys of a row's voices, parallel to `calls::speaker_labels`.
pub fn row_keys(row: &Row) -> Vec<String> {
    voices(row).iter().map(key).collect()
}

/// Replace a row's identity with `voices`, keeping overlap explicit.
fn set_voices(
    row: &mut Row,
    voices: Vec<Voice>,
    names: &[String],
    rows_like: Option<&Attribution>,
) {
    let mut unique: Vec<Voice> = Vec::new();
    for voice in voices {
        if !unique.iter().any(|existing| key(existing) == key(&voice)) {
            unique.push(voice);
        }
    }
    if unique == [Voice::You] {
        row.microphone = true;
        row.speakers.clear();
        row.discord = None;
        return;
    }
    row.microphone = false;
    if unique
        .iter()
        .all(|voice| matches!(voice, Voice::Acoustic(_)))
    {
        let mut ids: Vec<i32> = unique
            .iter()
            .map(|voice| match voice {
                Voice::Acoustic(id) => *id,
                _ => unreachable!("checked above"),
            })
            .collect();
        ids.sort_unstable();
        ids.truncate(4);
        row.speakers = ids;
        row.discord = None;
        return;
    }
    // Mixed or named voices: keep each person by their current label.
    let speakers: Vec<NamedSpeaker> = unique
        .iter()
        .map(|voice| match voice {
            Voice::Named(named) => named.clone(),
            other => NamedSpeaker {
                id: String::new(),
                name: label(other, names),
                avatar: None,
            },
        })
        .collect();
    row.speakers.clear();
    row.discord = Some(match rows_like {
        // Reuse the existing attribution so later live turns from the same
        // participant still continue this row.
        Some(existing) if existing.speakers == speakers => existing.clone(),
        _ => Attribution {
            generation: 0,
            channel_id: String::new(),
            speakers,
        },
    });
}

/// Validate and normalize a name typed by the user.
pub fn clean_name(name: &str) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        !name.is_empty() && name.chars().count() <= MAX_NAME_CHARS,
        "Use a name from 1 to {MAX_NAME_CHARS} characters."
    );
    ensure!(
        !name.chars().any(char::is_control),
        "Use a name without control characters."
    );
    ensure!(
        !["you", "other audio", "uncertain speaker"].contains(&name.to_lowercase().as_str()),
        "Choose a different name; that label is reserved."
    );
    Ok(name)
}

fn find<'a>(speakers: &'a [Speaker], key: &str) -> Result<&'a Speaker> {
    speakers
        .iter()
        .find(|speaker| speaker.key == key)
        .ok_or_else(|| anyhow::anyhow!("That speaker is no longer in this transcript."))
}

/// A single-speaker attribution already used for `voice`, if any.
fn existing_attribution<'a>(rows: &'a [Row], voice: &Voice) -> Option<&'a Attribution> {
    let Voice::Named(named) = voice else {
        return None;
    };
    rows.iter()
        .filter_map(|row| row.discord.as_ref())
        .find(|attribution| attribution.speakers.len() == 1 && &attribution.speakers[0] == named)
}

/// Replace `from` by `to` in every row that contains it.
fn substitute(
    rows: &mut [Row],
    names: &[String],
    from: &str,
    to: &Voice,
    before: &mut Vec<RowBefore>,
) {
    let like = existing_attribution(rows, to).cloned();
    for (index, row) in rows.iter_mut().enumerate() {
        let current = voices(row);
        if !current.iter().any(|voice| key(voice) == from) {
            continue;
        }
        before.push(RowBefore::of(index, row));
        let next = current
            .into_iter()
            .map(|voice| {
                if key(&voice) == from {
                    to.clone()
                } else {
                    voice
                }
            })
            .collect();
        set_voices(row, next, names, like.as_ref());
    }
}

fn edit(summary: String, names: Option<Vec<String>>, rows: Vec<RowBefore>) -> Edit {
    Edit {
        summary,
        at_ms: crate::history::now_ms(),
        names,
        rows,
        rule: None,
        registry: None,
    }
}

/// Rename one speaker everywhere in the transcript.
pub fn rename(
    rows: &mut [Row],
    names: &mut Vec<String>,
    speaker: &str,
    name: &str,
) -> Result<Edit> {
    let name = clean_name(name)?;
    let speakers = roster(rows, names);
    let current = find(&speakers, speaker)?.clone();
    ensure!(
        current.renamable(),
        match current.kind {
            Kind::You => "Your microphone is always labelled You.",
            _ => "This label covers more than one voice. Reassign its turns instead.",
        }
    );
    ensure!(current.label != name, "The speaker already has that name.");
    let summary = format!("Renamed {} to {name}", current.label);
    match &current.voice {
        Voice::Acoustic(id) => {
            let previous = names.clone();
            names.resize(names.len().max(8), String::new());
            names[*id as usize - 1] = name;
            Ok(edit(summary, Some(previous), Vec::new()))
        }
        Voice::Named(named) => {
            let renamed = Voice::Named(NamedSpeaker {
                name,
                ..named.clone()
            });
            let mut before = Vec::new();
            substitute(rows, names, &current.key, &renamed, &mut before);
            let mut edit = edit(summary, None, before);
            edit.rule = Some(LiveRule {
                from: current.key.clone(),
                to: renamed,
            });
            Ok(edit)
        }
        Voice::You => unreachable!("checked as not renamable"),
    }
}

/// Merge `from` into `into`: every turn of `from` becomes a turn of `into`.
pub fn merge(rows: &mut [Row], names: &mut [String], from: &str, into: &str) -> Result<Edit> {
    ensure!(from != into, "Choose two different speakers to merge.");
    let speakers = roster(rows, names);
    let source = find(&speakers, from)?.clone();
    let target = find(&speakers, into)?.clone();
    let mut before = Vec::new();
    substitute(rows, names, &source.key, &target.voice, &mut before);
    let mut edit = edit(
        format!("Merged {} into {}", source.label, target.label),
        None,
        before,
    );
    edit.rule = Some(LiveRule {
        from: source.key,
        to: target.voice,
    });
    Ok(edit)
}

/// Assign the selected rows to one speaker. Each selection is a row index and
/// the start time the renderer saw, so a changed transcript is never edited.
pub fn reassign(
    rows: &mut [Row],
    names: &mut [String],
    selection: &[(usize, u64)],
    target: &Target,
) -> Result<Edit> {
    ensure!(!selection.is_empty(), "Select at least one turn.");
    ensure!(
        selection.len() <= rows.len(),
        "Select turns from this transcript."
    );
    for &(index, start_ms) in selection {
        ensure!(
            rows.get(index).is_some_and(|row| row.start_ms == start_ms),
            "The transcript changed. Select the turns again."
        );
    }
    let speakers = roster(rows, names);
    let voice = match target {
        Target::Existing(key) => find(&speakers, key)?.voice.clone(),
        Target::New(name) => {
            let name = clean_name(name)?;
            match speakers.iter().find(|speaker| {
                speaker.renamable() && speaker.label.to_lowercase() == name.to_lowercase()
            }) {
                Some(existing) => existing.voice.clone(),
                None => Voice::Named(NamedSpeaker {
                    id: String::new(),
                    name,
                    avatar: None,
                }),
            }
        }
    };
    let label = label(&voice, names);
    let like = existing_attribution(rows, &voice).cloned();
    let mut before = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for &(index, _) in selection {
        if !seen.insert(index) {
            continue;
        }
        let row = &mut rows[index];
        if voices(row) == [voice.clone()] {
            continue;
        }
        before.push(RowBefore::of(index, row));
        set_voices(row, vec![voice.clone()], names, like.as_ref());
    }
    if before.is_empty() {
        bail!("The selected turns already belong to {label}.");
    }
    let count = before.len();
    Ok(edit(
        format!(
            "Assigned {count} {} to {label}",
            if count == 1 { "turn" } else { "turns" }
        ),
        None,
        before,
    ))
}

/// Restore what `edit` changed. Rows that no longer match are left alone;
/// returns how many rows were restored.
pub fn undo(rows: &mut [Row], names: &mut Vec<String>, edit: &Edit) -> usize {
    if let Some(previous) = &edit.names {
        names.clone_from(previous);
    }
    let mut restored = 0;
    for before in edit.rows.iter().rev() {
        if let Some(row) = rows.get_mut(before.index)
            && row.start_ms == before.start_ms
        {
            row.microphone = before.microphone;
            row.speakers.clone_from(&before.speakers);
            row.discord.clone_from(&before.discord);
            restored += 1;
        }
    }
    restored
}

/// Push an edit, keeping the most recent `MAX_UNDO`.
pub fn record(history: &mut Vec<Edit>, edit: Edit) {
    history.push(edit);
    if history.len() > MAX_UNDO {
        let excess = history.len() - MAX_UNDO;
        history.drain(..excess);
    }
}

/// Apply live rules to rows that arrive after a correction.
pub fn apply_rules(rows: &mut [Row], rules: &[LiveRule], names: &[String]) {
    if rules.is_empty() {
        return;
    }
    for row in rows {
        let mut current = voices(row);
        let mut changed = false;
        for rule in rules {
            for voice in &mut current {
                if key(voice) == rule.from {
                    *voice = rule.to.clone();
                    changed = true;
                }
            }
        }
        if changed {
            set_voices(row, current, names, None);
        }
    }
}

/// Bring copies of row identities (notes quotes, citation labels) in line
/// with the corrected rows.
pub fn refresh_derived(session: &mut crate::history::Session) {
    let rows = &session.rows;
    if let Some(notes) = &mut session.notes {
        for quote in notes.highlights.iter_mut().chain(notes.actions.iter_mut()) {
            if let Some(row) = rows.get(quote.row) {
                quote.microphone = row.microphone;
                quote.speakers.clone_from(&row.speakers);
                quote.discord.clone_from(&row.discord);
            }
        }
    }
    let label = |source_id: &str| {
        source_id
            .strip_prefix("row-")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| rows.get(index))
            .map(|row| crate::calls::label(row, &session.speaker_names))
    };
    let citations = session
        .generated_summary
        .iter_mut()
        .flat_map(|draft| draft.items.iter_mut())
        .chain(session.protected_note_items.iter_mut())
        .flat_map(|item| item.sources.iter_mut());
    for citation in citations {
        if let Some(label) = label(&citation.source_id) {
            citation.speaker = Some(label);
        }
    }
    session.text = crate::calls::text(&session.rows, &session.speaker_names);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start: u64, microphone: bool, speakers: &[i32], text: &str) -> Row {
        Row {
            cues: Vec::new(),
            start_ms: start,
            end_ms: start + 1000,
            microphone,
            speakers: speakers.to_vec(),
            discord: None,
            text: text.into(),
        }
    }

    fn named(start: u64, id: &str, name: &str) -> Row {
        let mut row = row(start, false, &[], "Named turn.");
        row.discord = Some(Attribution {
            generation: 3,
            channel_id: "synthetic-channel".into(),
            speakers: vec![NamedSpeaker {
                id: id.into(),
                name: name.into(),
                avatar: None,
            }],
        });
        row
    }

    fn sample() -> (Vec<Row>, Vec<String>) {
        (
            vec![
                row(0, false, &[1], "Hello."),
                row(2000, true, &[], "Hi there."),
                row(4000, false, &[2], "Me too."),
                row(6000, false, &[1, 2], "Both at once."),
                named(8000, "synthetic-user", "Zoë 李"),
                row(10000, false, &[0], "Music."),
            ],
            crate::calls::empty_speaker_names(),
        )
    }

    fn labels(rows: &[Row], names: &[String]) -> Vec<String> {
        rows.iter()
            .map(|row| crate::calls::label(row, names))
            .collect()
    }

    #[test]
    fn roster_lists_each_voice_once_with_opaque_keys() {
        let (rows, names) = sample();
        let speakers = roster(&rows, &names);
        let keys: Vec<_> = speakers.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(&keys[..3], ["s1", "you", "s2"]);
        assert_eq!(speakers[0].turns, 2);
        assert_eq!(speakers[3].kind, Kind::Participant);
        assert!(speakers[3].key.starts_with("n:"));
        assert!(!speakers[3].key.contains("synthetic-user"));
        assert_eq!(speakers[4].kind, Kind::Other);
        assert!(!speakers[4].renamable() && !speakers[1].renamable());
    }

    #[test]
    fn renaming_updates_every_label_and_undo_restores_it() {
        let (mut rows, mut names) = sample();
        let edit = rename(&mut rows, &mut names, "s2", "Morgan").unwrap();
        assert_eq!(edit.summary, "Renamed Speaker 2 to Morgan");
        assert_eq!(labels(&rows, &names)[2], "Morgan");
        assert_eq!(labels(&rows, &names)[3], "Overlap: Speaker 1 + Morgan");
        undo(&mut rows, &mut names, &edit);
        assert_eq!(labels(&rows, &names)[2], "Speaker 2");

        let key = roster(&rows, &names)[3].key.clone();
        let edit = rename(&mut rows, &mut names, &key, "Zoe").unwrap();
        assert_eq!(labels(&rows, &names)[4], "Zoe");
        // The live rule renames later turns from the same participant.
        let mut later = vec![named(20000, "synthetic-user", "Zoë 李")];
        apply_rules(&mut later, &[edit.rule.clone().unwrap()], &names);
        assert_eq!(labels(&later, &names), ["Zoe"]);
        undo(&mut rows, &mut names, &edit);
        assert_eq!(labels(&rows, &names)[4], "Zoë 李");
        assert!(rename(&mut rows, &mut names, "you", "Sam").is_err());
        assert!(rename(&mut rows, &mut names, "other", "Band").is_err());
        assert!(rename(&mut rows, &mut names, "s1", "  ").is_err());
        assert!(rename(&mut rows, &mut names, "s1", "You").is_err());
    }

    #[test]
    fn merging_replaces_a_voice_and_collapses_overlap() {
        let (mut rows, mut names) = sample();
        names[0] = "Alex".into();
        let edit = merge(&mut rows, &mut names, "s2", "s1").unwrap();
        assert_eq!(edit.summary, "Merged Speaker 2 into Alex");
        assert_eq!(rows[2].speakers, [1]);
        assert_eq!(rows[3].speakers, [1], "overlap with oneself is one voice");
        assert_eq!(labels(&rows, &names)[3], "Alex");
        assert!(!roster(&rows, &names).iter().any(|s| s.key == "s2"));
        let mut later = vec![row(30000, false, &[2], "Later.")];
        apply_rules(&mut later, &[edit.rule.clone().unwrap()], &names);
        assert_eq!(later[0].speakers, [1]);
        undo(&mut rows, &mut names, &edit);
        assert_eq!(rows[2].speakers, [2]);
        assert_eq!(rows[3].speakers, [1, 2]);
        assert!(merge(&mut rows, &mut names, "s1", "s1").is_err());
        assert!(merge(&mut rows, &mut names, "s7", "s1").is_err());
    }

    #[test]
    fn merging_an_acoustic_voice_into_a_participant_names_its_turns() {
        let (mut rows, mut names) = sample();
        let participant = roster(&rows, &names)[3].key.clone();
        merge(&mut rows, &mut names, "s2", &participant).unwrap();
        assert_eq!(labels(&rows, &names)[2], "Zoë 李");
        // The participant keeps one attribution, so live turns still continue it.
        assert_eq!(rows[2].discord, rows[4].discord);
        assert_eq!(labels(&rows, &names)[3], "Overlap: Speaker 1 + Zoë 李");
    }

    #[test]
    fn reassigning_turns_checks_the_selection_and_can_create_a_person() {
        let (mut rows, mut names) = sample();
        assert!(
            reassign(
                &mut rows,
                &mut names,
                &[(0, 999)],
                &Target::Existing("s2".into())
            )
            .is_err()
        );
        let edit = reassign(
            &mut rows,
            &mut names,
            &[(0, 0), (5, 10000)],
            &Target::Existing("s2".into()),
        )
        .unwrap();
        assert_eq!(edit.summary, "Assigned 2 turns to Speaker 2");
        assert_eq!(labels(&rows, &names)[0], "Speaker 2");
        assert_eq!(labels(&rows, &names)[5], "Speaker 2");
        let edit_two = reassign(
            &mut rows,
            &mut names,
            &[(2, 4000)],
            &Target::New("Casey".into()),
        )
        .unwrap();
        assert_eq!(labels(&rows, &names)[2], "Casey");
        assert_eq!(
            roster(&rows, &names)
                .iter()
                .find(|s| s.label == "Casey")
                .unwrap()
                .kind,
            Kind::Person
        );
        // A typed name that already exists joins that speaker.
        reassign(
            &mut rows,
            &mut names,
            &[(1, 2000)],
            &Target::New("casey".into()),
        )
        .unwrap();
        assert_eq!(labels(&rows, &names)[1], "Casey");
        assert!(
            reassign(
                &mut rows,
                &mut names,
                &[(1, 2000)],
                &Target::New("Casey".into())
            )
            .is_err()
        );
        let mut history = Vec::new();
        record(&mut history, edit);
        record(&mut history, edit_two);
        let last = history.pop().unwrap();
        undo(&mut rows, &mut names, &last);
        assert_eq!(labels(&rows, &names)[2], "Speaker 2");
        let first = history.pop().unwrap();
        undo(&mut rows, &mut names, &first);
        assert_eq!(labels(&rows, &names)[0], "Speaker 1");
        assert_eq!(labels(&rows, &names)[5], "Other audio");
    }

    #[test]
    fn undo_skips_rows_that_moved_and_history_is_bounded() {
        let (mut rows, mut names) = sample();
        let edit = reassign(
            &mut rows,
            &mut names,
            &[(0, 0)],
            &Target::Existing("you".into()),
        )
        .unwrap();
        assert!(rows[0].microphone);
        rows[0].start_ms = 1;
        assert_eq!(undo(&mut rows, &mut names, &edit), 0);
        assert!(rows[0].microphone);
        let mut history = Vec::new();
        for _ in 0..(MAX_UNDO + 5) {
            record(&mut history, edit.clone());
        }
        assert_eq!(history.len(), MAX_UNDO);
    }

    #[test]
    fn saved_edits_round_trip_without_discord_identifiers() {
        let (mut rows, mut names) = sample();
        let key = roster(&rows, &names)[3].key.clone();
        let edit = reassign(
            &mut rows,
            &mut names,
            &[(4, 8000)],
            &Target::Existing("s1".into()),
        )
        .unwrap();
        let json = serde_json::to_string(&edit).unwrap();
        assert!(!json.contains("synthetic-user") && !json.contains("synthetic-channel"));
        let restored: Edit = serde_json::from_str(&json).unwrap();
        undo(&mut rows, &mut names, &restored);
        assert_eq!(labels(&rows, &names)[4], "Zoë 李");
        // Without its identifier the participant gets a name-based key.
        assert_ne!(roster(&rows, &names)[3].key, key);
    }
}
