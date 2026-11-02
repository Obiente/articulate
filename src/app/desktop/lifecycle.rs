//! Shutdown, capture readiness and document edit locks.

use crate::app::App;
use crate::history::Session;
use std::time::{Duration, Instant};

/// A download that closing the app would interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Download {
    pub name: &'static str,
    /// Settings offers a cancel control for this download.
    pub cancellable: bool,
}

fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Explain which downloads must finish before closing or updating.
pub(crate) fn downloads_message(downloads: &[Download]) -> String {
    let names: Vec<_> = downloads.iter().map(|download| download.name).collect();
    let plural = if downloads.len() == 1 {
        "download is"
    } else {
        "downloads are"
    };
    let mut message = format!(
        "The {} {plural} still in progress. Closing now would interrupt {}. Wait for {} to finish",
        list(&names),
        if downloads.len() == 1 { "it" } else { "them" },
        if downloads.len() == 1 { "it" } else { "them" },
    );
    let cancellable: Vec<_> = downloads
        .iter()
        .filter(|download| download.cancellable)
        .map(|download| download.name)
        .collect();
    if cancellable.is_empty() {
        message.push('.');
    } else {
        message.push_str(&format!(
            ", or cancel the {} download in Settings.",
            list(&cancellable)
        ));
    }
    message
}

impl App {
    /// Downloads that write model or installer files.
    pub(crate) fn desktop_active_downloads(&self) -> Vec<Download> {
        let mut active = Vec::new();
        let mut add = |running: bool, name, cancellable| {
            if running {
                active.push(Download { name, cancellable });
            }
        };
        add(self.downloading.is_some(), "speech model", true);
        add(
            self.verifier_downloading.is_some(),
            "second speech model",
            true,
        );
        add(
            self.speakers_downloading.is_some(),
            "speaker identification",
            true,
        );
        add(self.context_progress.is_some(), "audio context", true);
        add(
            self.desktop_notes_status("").downloading,
            "notes model",
            true,
        );
        add(
            matches!(
                self.updater.status,
                crate::update::Status::Downloading { .. }
            ),
            "app update",
            false,
        );
        active
    }

    pub(crate) fn desktop_capture_idle(&self) -> Result<(), String> {
        if !self.ready
            || self.loading
            || self.busy
            || self.recording.is_some()
            || self.call.is_some()
            || self.downloading.is_some()
            || self.verifier_downloading.is_some()
            || self.preview_inflight
        {
            Err("Wait for the speech model and finish the current recording first.".into())
        } else {
            Ok(())
        }
    }

    /// Save everything and confirm the history worker settled. Used before
    /// closing and before launching an update installer.
    pub(crate) fn desktop_shutdown(&mut self) -> Result<(), String> {
        if self.recording.is_some()
            || self.call.is_some()
            || self.busy
            || self.preview_inflight
            || self.integration_inflight
            || self.brain_working()
            || self.polish_working()
            || self.notetaker_routing_busy()
        {
            return Err(
                "Finish recording and wait for processing before closing Articulate.".into(),
            );
        }
        let downloads = self.desktop_active_downloads();
        if !downloads.is_empty() {
            return Err(downloads_message(&downloads));
        }
        self.history_save_personal_notes();
        self.history_save_dictation();
        self.history_save_call();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.history_poll();
            if self.install_saves_settled()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Changes are still saving. Try closing again in a moment.".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Why an edit to `session` must wait, if a move or deletion affects it.
    pub(crate) fn desktop_edit_blocked(&self, session: &Session) -> Option<&'static str> {
        if self.notetaker.moving {
            return Some(
                "A recording is moving between topics. Try saving your edit again in a moment.",
            );
        }
        if self.history.pending_deletions.contains(&session.id) {
            return Some("This document is being deleted.");
        }
        for id in &self.history.pending_deletions {
            let member = session.sources.iter().any(|source| source.id == *id)
                || self.history.items.iter().any(|item| {
                    item.id == *id && item.topic_id.as_deref() == Some(session.id.as_str())
                });
            if member {
                return Some(
                    "A recording in this topic is being deleted. Try saving your edit again in a moment.",
                );
            }
            if session.topic_id.as_deref() == Some(id.as_str()) {
                return Some(
                    "The topic for this recording is being deleted. Try saving your edit again in a moment.",
                );
            }
        }
        None
    }

    /// Lock open editors while a move or deletion can replace their document.
    pub(crate) fn desktop_editors_locked(&self) -> bool {
        self.notetaker.moving
            || [
                &self.history.selected,
                &self.history.call,
                &self.history.dictation,
            ]
            .into_iter()
            .flatten()
            .any(|session| self.desktop_edit_blocked(session).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_messages_name_each_download_and_cancellable_ones() {
        let speech = Download {
            name: "speech model",
            cancellable: false,
        };
        let notes = Download {
            name: "notes model",
            cancellable: true,
        };
        assert_eq!(
            downloads_message(&[speech]),
            "The speech model download is still in progress. Closing now would interrupt it. Wait for it to finish."
        );
        assert_eq!(
            downloads_message(&[speech, notes]),
            "The speech model and notes model downloads are still in progress. Closing now would interrupt them. Wait for them to finish, or cancel the notes model download in Settings."
        );
    }
}
