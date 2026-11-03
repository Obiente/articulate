//! Copying and exporting text.

use crate::platform;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    InsertText {
        text: String,
        #[serde(default)]
        line_breaks: std::collections::BTreeMap<String, platform::LineBreak>,
    },
    Copy {
        text: String,
    },
    Export {
        text: String,
        extension: String,
    },
}

pub(crate) fn run(action: Action) -> Result<(), String> {
    match action {
        Action::InsertText { text, line_breaks } => insert(&text, &line_breaks),
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
        Action::InsertText { text, line_breaks } => Some(insert(text, line_breaks)),
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

fn insert(
    text: &str,
    line_breaks: &std::collections::BTreeMap<String, platform::LineBreak>,
) -> Result<(), String> {
    if line_breaks.len() > 100 {
        return Err("Use at most 100 app profiles.".into());
    }
    if text.is_empty() || text.len() > 16000 {
        return Err("Choose text up to 16,000 bytes to insert.".into());
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    let target = platform::target()
        .filter(|target| target.is_external())
        .ok_or("Focus a text field in another app and try again.")?;
    let (tx, rx) = crate::integration::start(|| {});
    let app = platform::app_name(target);
    let line_break = app
        .as_deref()
        .and_then(|app| {
            line_breaks
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(app))
                .map(|(_, mode)| *mode)
        })
        .unwrap_or_else(|| crate::writing_style::default_line_break(app.as_deref()));
    tx.send(crate::integration::Action::LineBreak(line_break))
        .map_err(|_| "Insertion stopped.")?;
    tx.send(crate::integration::Action::Arm(0, target, false))
        .map_err(|_| "Insertion stopped.")?;
    let cancel = transcribe_cpp::CancelToken::new();
    tx.send(crate::integration::Action::Insert {
        id: 0,
        target,
        text: text.into(),
        learn: false,
        cancel: cancel.clone(),
    })
    .map_err(|_| "Insertion stopped.")?;
    let result = match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(crate::integration::Event::Inserted(_, result)) => result.map(|_| ()),
        _ => Err("Insertion could not finish. Your text is ready to copy.".into()),
    };
    cancel.cancel();
    result
}
