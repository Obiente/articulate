//! Starting and finishing dictation, calls and spoken notes.

use crate::app::App;
use serde::Deserialize;

#[derive(Default, serde::Serialize)]
pub(crate) struct MicrophoneCheck {
    pub running: bool,
    pub result: Option<crate::audio::AudioCheck>,
    pub error: Option<String>,
}

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
    MicrophoneCheck,
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
            Action::MicrophoneCheck => {
                if self.recording.is_some() || self.call.is_some() || self.busy {
                    return Err("Finish recording before testing the microphone.".into());
                }
                if self.microphone_check.running {
                    return Err("A microphone test is already running.".into());
                }
                self.microphone_check = MicrophoneCheck {
                    running: true,
                    ..Default::default()
                };
                let selected = self.settings.microphone.clone();
                let tx = self.event_tx.clone();
                std::thread::spawn(move || {
                    let result = crate::audio::check_microphone(selected.as_deref(), 3)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(crate::app::Event::MicrophoneChecked(result));
                });
            }
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
