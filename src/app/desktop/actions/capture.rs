//! Starting and finishing dictation, calls and spoken notes.

use crate::app::App;
use serde::Deserialize;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallSource {
    #[default]
    System,
    Discord,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    DictationStart,
    DictationStop,
    DictationCancel,
    CallStart {
        #[serde(default)]
        source: CallSource,
        #[serde(default)]
        language_preference: Option<String>,
        #[serde(default)]
        audio_context: Option<bool>,
    },
    CallStop,
    NoteCaptureStart,
    NoteCaptureStop,
}

impl App {
    pub(crate) fn desktop_capture_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::DictationStart => {
                self.desktop_capture_idle()?;
                self.toggle(None);
                if self.recording.is_none() {
                    return Err(self.status.clone());
                }
            }
            Action::DictationStop => {
                if self.recording.is_some() {
                    self.toggle(None);
                }
            }
            Action::DictationCancel => {
                if self.call.is_some() {
                    return Err("Finish the call using its recording control.".into());
                }
                self.cancel_dictation();
            }
            Action::CallStart {
                source,
                language_preference,
                audio_context,
            } => {
                self.desktop_capture_idle()?;
                if language_preference.as_ref().is_some_and(|language| {
                    !self
                        .speech_languages
                        .iter()
                        .any(|supported| supported == language)
                }) {
                    return Err("Choose a supported call language preference.".into());
                }
                if audio_context == Some(true) && !crate::sensevoice::installed() {
                    return Err("Install sound and tone cues in Settings first.".into());
                }
                if !self.start_manual_call_capture(
                    matches!(source, CallSource::Discord),
                    language_preference,
                    audio_context,
                ) {
                    return Err(self.call_status.clone());
                }
            }
            Action::CallStop => {
                self.discord_launch.automation.manual_finish();
                if let Some(call) = &self.call {
                    call.stop();
                }
            }
            Action::NoteCaptureStart => self.notetaker_start_capture()?,
            Action::NoteCaptureStop => self.notetaker_stop_capture()?,
        }
        Ok(())
    }
}
