//! Conversations and speakers: speaker corrections, known people, audio
//! import, retained audio playback and re-transcription.

use crate::app::{
    App,
    call_tools::{self, ImportKind, SpeakerOp},
};
use crate::speaker_edits::Target;
use serde::Deserialize;

/// A transcript row as the renderer saw it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RowRef {
    index: usize,
    start_ms: u64,
}

/// What the delete confirmation showed: the session and its audio size.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioConfirmation {
    id: String,
    bytes: u64,
}

/// What the forget-everyone confirmation showed: how many people.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PeopleConfirmation {
    count: usize,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    SpeakerRename {
        id: String,
        speaker: String,
        name: String,
    },
    SpeakerMerge {
        id: String,
        from: String,
        into: String,
    },
    /// Assign rows to an existing `speaker` key or a new `name`.
    SpeakerReassign {
        id: String,
        rows: Vec<RowRef>,
        #[serde(default)]
        speaker: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    SpeakerUndo {
        id: String,
    },
    PersonSave {
        #[serde(default)]
        id: Option<String>,
        name: String,
    },
    PersonDelete {
        id: String,
    },
    PeopleClear {
        confirm: PeopleConfirmation,
    },
    AudioImportChoose,
    AudioImportStart {
        kind: ImportKind,
        #[serde(default)]
        keep_audio: bool,
    },
    AudioImportCancel,
    AudioImportDismiss,
    AudioPlay {
        id: String,
        #[serde(default)]
        from_ms: u64,
    },
    AudioStop,
    AudioDelete {
        id: String,
        confirm: AudioConfirmation,
    },
    TranscriptRebuild {
        id: String,
    },
    /// `version` is a `transcript_versions[].id`; `expected` the document's
    /// `transcript_revision` when the choice was made.
    TranscriptRestore {
        id: String,
        version: String,
        expected: String,
    },
}

/// The open dialog must not pause capture, so it runs on the IPC thread.
pub(crate) fn run_outside_controller(action: &Action) -> Option<Result<(), String>> {
    match action {
        Action::AudioImportChoose => Some(call_tools::choose_file()),
        _ => None,
    }
}

impl App {
    pub(crate) fn desktop_calls_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::SpeakerRename { id, speaker, name } => {
                self.calls_speaker_edit(&id, SpeakerOp::Rename { speaker, name })
            }
            Action::SpeakerMerge { id, from, into } => {
                self.calls_speaker_edit(&id, SpeakerOp::Merge { from, into })
            }
            Action::SpeakerReassign {
                id,
                rows,
                speaker,
                name,
            } => {
                let target = match (speaker, name) {
                    (Some(speaker), None) => Target::Existing(speaker),
                    (None, Some(name)) => Target::New(name),
                    _ => return Err("Choose one speaker or type a new name.".into()),
                };
                if rows.len() > 10_000 {
                    return Err("Select fewer turns at once.".into());
                }
                let rows = rows
                    .into_iter()
                    .map(|row| (row.index, row.start_ms))
                    .collect();
                self.calls_speaker_edit(&id, SpeakerOp::Reassign { rows, target })
            }
            Action::SpeakerUndo { id } => self.calls_speaker_edit(&id, SpeakerOp::Undo),
            Action::PersonSave { id, name } => self.calls_people_save(id.as_deref(), &name),
            Action::PersonDelete { id } => self.calls_people_delete(&id),
            Action::PeopleClear { confirm } => self.calls_people_clear(confirm.count),
            // Handled outside the controller; nothing to do here.
            Action::AudioImportChoose => Ok(()),
            Action::AudioImportStart { kind, keep_audio } => {
                self.calls_import_start(kind, keep_audio)
            }
            Action::AudioImportCancel => {
                self.calls_import_cancel();
                Ok(())
            }
            Action::AudioImportDismiss => {
                self.calls_import_dismiss();
                Ok(())
            }
            Action::AudioPlay { id, from_ms } => self.calls_play(&id, from_ms),
            Action::AudioStop => {
                self.calls_stop_playback();
                Ok(())
            }
            Action::AudioDelete { id, confirm } => {
                self.calls_delete_audio(&id, &confirm.id, confirm.bytes)
            }
            Action::TranscriptRebuild { id } => self.calls_rebuild(&id),
            Action::TranscriptRestore {
                id,
                version,
                expected,
            } => self.calls_restore_version(&id, &version, &expected),
        }
    }
}
