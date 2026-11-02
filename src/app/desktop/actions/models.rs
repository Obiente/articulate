//! Speech, notes, cleanup, speaker and audio context model setup.

use crate::app::{App, Event};
use crate::model;
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    ModelDownload,
    ModelLoad,
    SummaryDownload,
    SummaryDownloadCancel,
    CleanupDownload,
    CleanupDownloadCancel,
    ContextDownload,
    SpeakersDownload,
    VerifierDownload,
}

impl App {
    pub(crate) fn desktop_models_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::ModelDownload => {
                if self.downloading.is_some()
                    || self.recording.is_some()
                    || self.call.is_some()
                    || self.busy
                    || self.loading
                {
                    return Err("Finish the current task before downloading a model.".into());
                }
                self.download();
            }
            Action::ModelLoad => {
                if self.recording.is_some() || self.call.is_some() || self.busy || self.loading {
                    return Err("Finish the current task first.".into());
                }
                self.load();
            }
            Action::SummaryDownload => self.desktop_summary_download()?,
            Action::SummaryDownloadCancel => self.desktop_summary_download_cancel(),
            Action::CleanupDownload => self.desktop_cleanup_download()?,
            Action::CleanupDownloadCancel => self.desktop_cleanup_cancel(),
            Action::ContextDownload => self.desktop_context_download()?,
            Action::SpeakersDownload => self.desktop_speakers_download()?,
            Action::VerifierDownload => self.desktop_verifier_download()?,
        }
        Ok(())
    }

    fn desktop_context_download(&mut self) -> Result<(), String> {
        if self.context_progress.is_some() || self.call.is_some() || self.recording.is_some() {
            return Err("Finish recording or the current audio context download first.".into());
        }
        self.context_progress = Some(0.0);
        self.context_status = "Downloading audio context".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("audio-context-install".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = crate::sensevoice::install(|progress| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::ContextProgress(progress));
                        last = Instant::now();
                    }
                })
                .map_err(|error| error.to_string());
                let _ = sender.send(Event::ContextInstalled(result));
            })
            .map_err(|error| {
                self.context_progress = None;
                error.to_string()
            })?;
        Ok(())
    }

    fn desktop_speakers_download(&mut self) -> Result<(), String> {
        if self.speakers_downloading.is_some() || self.call.is_some() || self.recording.is_some() {
            return Err("Finish the current recording or speaker download first.".into());
        }
        self.speakers_downloading = Some(0.0);
        self.speakers_download_status = "Downloading speaker identification…".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("speaker-model-download".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = model::download_speakers(|progress| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::SpeakersProgress(progress));
                        last = Instant::now();
                    }
                });
                let _ = sender.send(match result {
                    Ok(_) => Event::SpeakersDownloaded,
                    Err(error) => Event::SpeakersDownloadFailed(error.to_string()),
                });
            })
            .map_err(|error| {
                self.speakers_downloading = None;
                error.to_string()
            })?;
        Ok(())
    }

    fn desktop_verifier_download(&mut self) -> Result<(), String> {
        if self.verifier_downloading.is_some()
            || self.call.is_some()
            || self.recording.is_some()
            || self.busy
            || self.loading
        {
            return Err("Finish the current recording or model download first.".into());
        }
        self.verifier_downloading = Some(0.0);
        self.verifier_download_status = "Downloading second speech model…".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("verifier-model-download".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = model::download_verifier(|progress| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::VerifierProgress(progress));
                        last = Instant::now();
                    }
                });
                let _ = sender.send(match result {
                    Ok(_) => Event::VerifierDownloaded,
                    Err(error) => Event::VerifierDownloadFailed(error.to_string()),
                });
            })
            .map_err(|error| {
                self.verifier_downloading = None;
                error.to_string()
            })?;
        Ok(())
    }
}
