//! Copying and exporting text.

use crate::platform;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Copy { text: String },
    Export { text: String, extension: String },
}

pub(crate) fn run(action: Action) -> Result<(), String> {
    match action {
        Action::Copy { text } => {
            if text.len() > 8 * 1024 * 1024 {
                return Err("Text is too long.".into());
            }
            platform::copy(&text).map_err(|error| error.to_string())
        }
        Action::Export { text, extension } => export(&text, &extension),
    }
}

/// The save dialog can stay open while recording continues, so exports run
/// on the IPC thread instead of the controller. Returns `None` for actions
/// the controller handles.
pub(crate) fn run_outside_controller(action: &Action) -> Option<Result<(), String>> {
    match action {
        Action::Export { text, extension } => Some(export(text, extension)),
        Action::Copy { .. } => None,
    }
}

fn export(text: &str, extension: &str) -> Result<(), String> {
    if text.len() > 16 * 1024 * 1024 || !["txt", "md", "srt", "vtt"].contains(&extension) {
        return Err("Unsupported export.".into());
    }
    crate::export_file::save(text, extension).map_err(|error| error.to_string())?;
    Ok(())
}
