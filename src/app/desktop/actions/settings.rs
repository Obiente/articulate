//! Preferences. A patch is validated completely before anything changes.

use crate::app::{App, Settings};
use crate::discord::launch::Flavor;
use crate::{integration, meetings, platform};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    SettingsPatch {
        cpu: Option<bool>,
        model_path: Option<String>,
        transcription_language: Option<String>,
        clean_speech: Option<bool>,
        insert: Option<bool>,
        live_insert: Option<bool>,
        learn_corrections: Option<bool>,
        audio_feedback: Option<bool>,
        audio_context: Option<bool>,
        microphone: Option<String>,
        output: Option<String>,
        hotkey_mode: Option<platform::HotkeyMode>,
        hotkey: Option<platform::Hotkey>,
        quick_note_hotkey: Option<platform::Hotkey>,
        discord_auto_connect: Option<bool>,
        discord_auto_transcribe: Option<bool>,
        vencord_auto_update: Option<bool>,
        /// `stable`, `ptb`, `canary` or empty for the preferred installed edition.
        discord_flavor: Option<String>,
        discord_avatars: Option<bool>,
        meetings_teams: Option<bool>,
        meetings_zoom: Option<bool>,
        meetings_meet: Option<bool>,
        meetings_start: Option<meetings::StartMode>,
    },
}

/// Saved preference values from a patch, already normalized.
#[derive(Default)]
struct Preferences {
    cpu: Option<bool>,
    model_path: Option<String>,
    transcription_language: Option<String>,
    clean_speech: Option<bool>,
    insert: Option<bool>,
    live_insert: Option<bool>,
    learn_corrections: Option<bool>,
    audio_feedback: Option<bool>,
    audio_context: Option<bool>,
    microphone: Option<Option<String>>,
    output: Option<Option<String>>,
    hotkey_mode: Option<platform::HotkeyMode>,
    discord_auto_connect: Option<bool>,
    discord_auto_transcribe: Option<bool>,
    vencord_auto_update: Option<bool>,
    discord_flavor: Option<String>,
    discord_avatars: Option<bool>,
    meetings_teams: Option<bool>,
    meetings_zoom: Option<bool>,
    meetings_meet: Option<bool>,
    meetings_start: Option<meetings::StartMode>,
}

impl Preferences {
    /// Exchange each provided value with the current setting. Afterwards
    /// `self` holds the previous values, so a second call restores them.
    fn exchange(&mut self, settings: &mut Settings) {
        fn swap<T>(value: &mut Option<T>, current: &mut T) {
            if let Some(value) = value {
                std::mem::swap(value, current);
            }
        }
        swap(&mut self.cpu, &mut settings.cpu);
        swap(&mut self.model_path, &mut settings.model_path);
        swap(
            &mut self.transcription_language,
            &mut settings.transcription_language,
        );
        swap(&mut self.clean_speech, &mut settings.clean_speech);
        swap(&mut self.insert, &mut settings.insert);
        swap(&mut self.live_insert, &mut settings.live_insert);
        swap(&mut self.learn_corrections, &mut settings.learn_corrections);
        swap(&mut self.audio_feedback, &mut settings.audio_feedback);
        swap(&mut self.audio_context, &mut settings.audio_context);
        swap(&mut self.microphone, &mut settings.microphone);
        swap(&mut self.output, &mut settings.output);
        swap(&mut self.hotkey_mode, &mut settings.hotkey_mode);
        swap(
            &mut self.discord_auto_connect,
            &mut settings.discord_auto_connect,
        );
        swap(
            &mut self.discord_auto_transcribe,
            &mut settings.discord_auto_transcribe,
        );
        swap(
            &mut self.vencord_auto_update,
            &mut settings.vencord_auto_update,
        );
        swap(&mut self.discord_flavor, &mut settings.discord_flavor);
        swap(&mut self.discord_avatars, &mut settings.discord_avatars);
        swap(&mut self.meetings_teams, &mut settings.meetings.teams);
        swap(&mut self.meetings_zoom, &mut settings.meetings.zoom);
        swap(&mut self.meetings_meet, &mut settings.meetings.meet);
        swap(&mut self.meetings_start, &mut settings.meetings.start);
    }
}

fn device(name: String) -> Option<String> {
    (!name.is_empty()).then_some(name)
}

impl App {
    pub(crate) fn desktop_settings_action(&mut self, action: Action) -> Result<(), String> {
        let Action::SettingsPatch {
            cpu,
            model_path,
            transcription_language,
            clean_speech,
            insert,
            live_insert,
            learn_corrections,
            audio_feedback,
            audio_context,
            microphone,
            output,
            hotkey_mode,
            hotkey,
            quick_note_hotkey,
            discord_auto_connect,
            discord_auto_transcribe,
            vencord_auto_update,
            discord_flavor,
            discord_avatars,
            meetings_teams,
            meetings_zoom,
            meetings_meet,
            meetings_start,
        } = action;
        let mut preferences = Preferences {
            cpu,
            model_path: model_path.map(|path| path.trim().to_owned()),
            transcription_language,
            clean_speech,
            insert,
            live_insert,
            learn_corrections,
            audio_feedback,
            audio_context,
            microphone: microphone.map(device),
            output: output.map(device),
            hotkey_mode,
            discord_auto_connect,
            discord_auto_transcribe,
            vencord_auto_update,
            discord_flavor: discord_flavor.map(|flavor| flavor.trim().to_ascii_lowercase()),
            discord_avatars,
            meetings_teams,
            meetings_zoom,
            meetings_meet,
            meetings_start,
        };
        self.desktop_validate_settings(&preferences, hotkey.as_ref(), quick_note_hotkey.as_ref())?;
        let reload_model = preferences
            .transcription_language
            .as_ref()
            .is_some_and(|language| *language != self.settings.transcription_language)
            || preferences
                .cpu
                .is_some_and(|value| value != self.settings.cpu)
            || preferences
                .model_path
                .as_ref()
                .is_some_and(|path| *path != self.settings.model_path);
        preferences.exchange(&mut self.settings);
        if let Err(error) = self.save_preferences() {
            preferences.exchange(&mut self.settings);
            return Err(format!("Could not save preferences: {error}"));
        }
        if learn_corrections == Some(false) {
            self.desktop_learning_disabled();
        }
        if discord_avatars == Some(false) {
            self.desktop_avatars_disabled();
        }
        if let Some(hotkey) = quick_note_hotkey {
            self.quick_note_hotkey_tx
                .send(hotkey)
                .map_err(|_| "The quick-note shortcut service stopped.")?;
            self.quick_note_hotkey_pending = true;
            self.quick_note_shortcut_error = None;
        }
        if let Some(hotkey) = hotkey {
            self.hotkey_tx
                .send(hotkey.clone())
                .map_err(|_| "The shortcut service stopped.")?;
            self.hotkey_draft = hotkey;
            self.hotkey_pending = true;
        }
        if reload_model && PathBuf::from(&self.settings.model_path).is_file() {
            self.load();
        }
        Ok(())
    }

    fn desktop_validate_settings(
        &self,
        preferences: &Preferences,
        hotkey: Option<&platform::Hotkey>,
        quick_note_hotkey: Option<&platform::Hotkey>,
    ) -> Result<(), String> {
        if self.recording.is_some() || self.call.is_some() || self.busy {
            return Err("Finish recording before changing preferences.".into());
        }
        if (preferences.cpu.is_some()
            || preferences.model_path.is_some()
            || preferences.transcription_language.is_some())
            && self.loading
        {
            return Err("Wait for the current model to finish loading.".into());
        }
        if let Some(path) = &preferences.model_path {
            let path = Path::new(path);
            if !path.is_absolute()
                || !path.is_file()
                || !path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
            {
                return Err("Choose an existing local GGUF model file using its full path.".into());
            }
        }
        if let Some(language) = &preferences.transcription_language
            && !language.is_empty()
            && !self.speech_languages.contains(language)
        {
            return Err("Choose a language supported by the loaded speech model.".into());
        }
        let dictation = hotkey.unwrap_or(&self.settings.hotkey);
        let quick_note = quick_note_hotkey.unwrap_or(&self.settings.quick_note_hotkey);
        for requested in [hotkey, quick_note_hotkey].into_iter().flatten() {
            requested.validate()?;
        }
        if (hotkey.is_some() || quick_note_hotkey.is_some()) && dictation == quick_note {
            return Err("Choose different shortcuts for dictation and quick notes.".into());
        }
        if preferences.audio_context == Some(true) && !crate::sensevoice::installed() {
            return Err("Install the audio context model first.".into());
        }
        if let Some(flavor) = &preferences.discord_flavor
            && *flavor != self.settings.discord_flavor
        {
            let parsed = if flavor.is_empty() {
                None
            } else {
                Some(
                    Flavor::parse(flavor)
                        .ok_or("Choose Discord, Discord PTB or Discord Canary.")?,
                )
            };
            if self.settings.discord_debugger_port != 0 {
                return Err(
                    "Restart Discord without the debugger before choosing another edition.".into(),
                );
            }
            if self.desktop_discord_relaunching() {
                return Err("Wait for the Discord restart to finish.".into());
            }
            let installed = self.desktop_installed_flavors();
            if let Some(parsed) = parsed
                && !installed.is_empty()
                && !installed.contains(&parsed)
            {
                return Err(format!("{} is not installed.", parsed.label()));
            }
        }
        Ok(())
    }

    /// Turn learning on or off immediately, without saving preferences.
    #[cfg(test)]
    pub(crate) fn desktop_set_learning(&mut self, enabled: bool) {
        self.settings.learn_corrections = enabled;
        if !enabled {
            self.desktop_learning_disabled();
        }
    }

    /// Stop observing inserted text and discard an unsaved learned edit.
    fn desktop_learning_disabled(&mut self) {
        let _ = self.integration_tx.send(integration::Action::StopLearning);
        self.edited_at = None;
        self.edit_baseline.clone_from(&self.text);
    }
}
