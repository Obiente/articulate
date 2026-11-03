//! Conversation tools: audio import and re-transcription progress, playback
//! of retained audio, known people, and the per-document parts of a session
//! that belong to calls (interruptions, retained audio, corrections).

use crate::app::App;
use crate::history::Session;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ImportView<'a> {
    /// "idle", "ready" (a file waits for a choice), "working", "done" or "failed".
    state: &'static str,
    /// Files are being dragged over the window.
    dragging: bool,
    file_name: Option<&'a str>,
    progress: Option<f32>,
    stage: &'static str,
    message: &'a str,
    /// The saved session of a finished import or re-transcription.
    session_id: Option<&'a str>,
    /// The session being re-transcribed.
    rebuilding: Option<&'a str>,
    /// Imported conversations can be attributed to speakers.
    speakers_available: bool,
}

#[derive(Serialize)]
pub(crate) struct PlaybackView<'a> {
    session_id: Option<&'a str>,
    playing: bool,
    position_ms: u64,
    error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct CallTools<'a> {
    audio_import: ImportView<'a>,
    playback: PlaybackView<'a>,
}

pub(crate) fn call_tools(app: &App) -> CallTools<'_> {
    let import = &app.call_tools.import;
    let state = if import.working {
        "working"
    } else if import.pending.is_some() {
        "ready"
    } else if import.failed {
        "failed"
    } else if import.session_id.is_some() {
        "done"
    } else {
        "idle"
    };
    let player = app.call_tools.player.as_ref();
    CallTools {
        audio_import: ImportView {
            state,
            dragging: app.calls_dragging(),
            file_name: import.file_name.as_deref(),
            progress: import.progress,
            stage: import.stage,
            message: &import.message,
            session_id: import.session_id.as_deref(),
            rebuilding: import.rebuilding.as_deref(),
            speakers_available: crate::speakers::path().is_file(),
        },
        playback: PlaybackView {
            session_id: player.map(|player| player.session_id.as_str()),
            playing: player.is_some_and(|player| player.playing()),
            position_ms: player.map_or(0, |player| player.position_ms()),
            error: player
                .and_then(|player| player.error())
                .or_else(|| app.call_tools.playback_error.clone()),
        },
    }
}

#[derive(Serialize)]
pub(crate) struct PersonView<'a> {
    id: &'a str,
    name: &'a str,
    /// "discord" for remembered participants, "name" for quick picks.
    source: &'static str,
    discord_name: Option<&'a str>,
    /// A `bytes://discord-avatar/...` reference, never a URL.
    avatar: Option<String>,
    used_ms: u64,
}

#[derive(Serialize)]
pub(crate) struct People<'a> {
    people: Vec<PersonView<'a>>,
    people_error: Option<&'a str>,
}

pub(crate) fn people(app: &App) -> People<'_> {
    let directory = &app.call_tools.people;
    let mut people: Vec<PersonView<'_>> = directory
        .list()
        .iter()
        .map(|person| PersonView {
            id: &person.id,
            name: &person.name,
            source: if person.discord.is_some() {
                "discord"
            } else {
                "name"
            },
            discord_name: person.discord.as_ref().map(|link| link.name.as_str()),
            avatar: person
                .discord
                .as_ref()
                .and_then(|link| link.avatar.as_ref())
                .filter(|avatar| avatar.valid())
                .map(|avatar| avatar.uri()),
            used_ms: person.used_ms,
        })
        .collect();
    people.sort_by(|a, b| b.used_ms.cmp(&a.used_ms).then_with(|| a.name.cmp(b.name)));
    People {
        people,
        people_error: directory.error(),
    }
}

#[derive(Serialize)]
pub(crate) struct InterruptionView {
    start_ms: u64,
    end_ms: u64,
    source: crate::calls::interruptions::Source,
    cause: crate::calls::interruptions::Cause,
    ongoing: bool,
    /// "Audio interrupted 12:03–12:07: microphone disconnected".
    message: String,
}

#[derive(Serialize)]
pub(crate) struct AudioView<'a> {
    duration_ms: u64,
    bytes: u64,
    channels: &'a [String],
    incomplete: bool,
}

#[derive(Serialize)]
pub(crate) struct VersionView<'a> {
    index: usize,
    replaced_ms: u64,
    model: &'a str,
    preview: &'a str,
}

/// Call-specific parts of a document's metadata.
#[derive(Serialize)]
pub(crate) struct SessionCalls<'a> {
    interruptions: Vec<InterruptionView>,
    /// What the next undo reverts, if anything.
    speaker_undo: Option<&'a str>,
    audio: Option<AudioView<'a>>,
    transcript_versions: Vec<VersionView<'a>>,
}

fn preview(text: &str) -> &str {
    text.char_indices()
        .nth(160)
        .map_or(text, |(index, _)| &text[..index])
}

impl<'a> SessionCalls<'a> {
    pub(crate) fn new(session: &'a Session) -> Self {
        Self {
            interruptions: session
                .interruptions
                .iter()
                .map(|interruption| InterruptionView {
                    start_ms: interruption.start_ms,
                    end_ms: interruption.end_ms,
                    source: interruption.source,
                    cause: interruption.cause,
                    ongoing: interruption.ongoing,
                    message: interruption.message(),
                })
                .collect(),
            speaker_undo: session
                .speaker_edits
                .last()
                .map(|edit| edit.summary.as_str()),
            audio: session.audio.as_ref().map(|audio| AudioView {
                duration_ms: audio.duration_ms,
                bytes: audio.bytes,
                channels: &audio.channels,
                incomplete: audio.incomplete,
            }),
            transcript_versions: session
                .transcript_versions
                .iter()
                .enumerate()
                .map(|(index, version)| VersionView {
                    index,
                    replaced_ms: version.replaced_ms,
                    model: &version.model,
                    preview: preview(&version.text),
                })
                .collect(),
        }
    }
}
