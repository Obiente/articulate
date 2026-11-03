//! Local model installation, downloads and note generation status.

use crate::app::App;
use crate::app::{brain_ui::NotesStatus, polish_ui::CleanupStatus};
use crate::model;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub(crate) struct ModelStatus<'a> {
    speech_installed: bool,
    backend: &'a str,
    /// The file name only; the folder can identify the user.
    model_name: Option<String>,
    speech_languages: &'a [String],
    model_custom: bool,
    speakers_installed: bool,
    speakers_working: bool,
    speakers_progress: Option<f32>,
    speakers_status: &'a str,
    context_installed: bool,
    context_progress: Option<f32>,
    context_status: &'a str,
    context_supported: bool,
    verifier_installed: bool,
    verifier_working: bool,
    verifier_progress: Option<f32>,
    verifier_status: &'a str,
    // Reported by the Polish module.
    cleanup_installed: bool,
    cleanup_working: bool,
    cleanup_status: &'a str,
    cleanup_progress: Option<f32>,
}

#[derive(Serialize)]
pub(crate) struct Models<'a> {
    managed_models: Vec<model::ModelInfo>,
    cleanup: CleanupStatus<'a>,
    model_status: ModelStatus<'a>,
    /// Speech model download progress from 0 to 1.
    downloading: Option<f32>,
}

pub(crate) fn models(app: &App) -> Models<'_> {
    let cleanup = app.desktop_cleanup_status();
    let path = Path::new(&app.settings.model_path);
    let model_name = (path.is_absolute()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf")))
    .then(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
    .flatten();
    Models {
        managed_models: model::list_models()
            .into_iter()
            .map(|mut model| {
                model.path = model
                    .path
                    .file_name()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default();
                model
            })
            .collect(),
        cleanup: app.desktop_cleanup_status(),
        model_status: ModelStatus {
            speech_installed: path.is_file(),
            backend: &app.backend,
            model_name,
            speech_languages: &app.speech_languages,
            model_custom: path != model::default_path(),
            speakers_installed: crate::speakers::path().is_file(),
            speakers_working: app.speakers_downloading.is_some(),
            speakers_progress: app.speakers_downloading,
            speakers_status: &app.speakers_download_status,
            context_installed: crate::sensevoice::installed(),
            context_progress: app.context_progress,
            context_status: &app.context_status,
            context_supported: cfg!(all(windows, target_arch = "x86_64")),
            verifier_installed: model::verifier_path().is_file(),
            verifier_working: app.verifier_downloading.is_some(),
            verifier_progress: app.verifier_downloading,
            verifier_status: &app.verifier_download_status,
            cleanup_installed: cleanup.installed,
            cleanup_working: cleanup.working,
            cleanup_status: cleanup.status,
            cleanup_progress: cleanup.progress,
        },
        downloading: app.downloading,
    }
}

/// Note generation status, each in the shape reported by the notes module.
#[derive(Serialize)]
pub(crate) struct Notes<'a> {
    /// The selected document, or the current call when nothing is selected.
    notes: NotesStatus<'a>,
    selected_notes: NotesStatus<'a>,
    call_notes: NotesStatus<'a>,
    note_notes: NotesStatus<'a>,
}

pub(crate) fn notes(app: &App) -> Notes<'_> {
    let selected = app.history.selected.as_ref().map(|s| s.id.as_str());
    let current = app.history.call.as_ref().map(|s| s.id.as_str());
    let note_capture = app.is_note_capture();
    Notes {
        notes: app.desktop_notes_status(selected.or(current).unwrap_or("")),
        selected_notes: app.desktop_notes_status(selected.unwrap_or("")),
        call_notes: app.desktop_notes_status(current.unwrap_or("")),
        note_notes: app.desktop_notes_status(current.filter(|_| note_capture).unwrap_or("")),
    }
}
