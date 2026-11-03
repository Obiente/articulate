//! Vocabulary entries and snippets.

use crate::app::App;
use crate::{dictionary, macros};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    DictionarySoundAlike {
        enabled: bool,
    },
    DictionarySave {
        heard: String,
        wanted: String,
        app: Option<String>,
        cues: Option<String>,
        sound_alike: Option<bool>,
        index: Option<usize>,
        expected_heard: Option<String>,
        expected_entry: Option<Value>,
    },
    DictionaryDelete {
        index: usize,
        expected_heard: String,
        expected_entry: Option<Value>,
    },
    MacroSave {
        trigger: String,
        expansion: String,
        app: Option<String>,
        index: Option<usize>,
        expected_trigger: Option<String>,
        expected_macro: Option<Value>,
    },
    MacroDelete {
        index: usize,
        expected_trigger: String,
        expected_macro: Option<Value>,
    },
}

impl App {
    pub(crate) fn desktop_vocabulary_action(&mut self, action: Action) -> Result<(), String> {
        self.desktop_vocabulary_save(action, Self::save_preferences)
    }

    fn desktop_vocabulary_save(
        &mut self,
        action: Action,
        save: impl FnOnce(&Self) -> anyhow::Result<()>,
    ) -> Result<(), String> {
        let entries = self.settings.entries.clone();
        let macros = self.settings.macros.clone();
        match action {
            Action::DictionarySoundAlike { enabled } => {
                let previous = self.settings.vocabulary_sound_alike;
                self.settings.vocabulary_sound_alike = enabled;
                if let Err(error) = save(self) {
                    self.settings.vocabulary_sound_alike = previous;
                    return Err(error.to_string());
                }
            }
            Action::DictionarySave {
                heard,
                wanted,
                app,
                cues,
                sound_alike,
                index,
                expected_heard,
                expected_entry,
            } => {
                let original = if let Some(index) = index {
                    Some(
                        self.settings
                            .entries
                            .get(index)
                            .filter(|e| {
                                Some(e.heard.as_str()) == expected_heard.as_deref()
                                    && serde_json::to_value(e).ok().as_ref()
                                        == expected_entry.as_ref()
                            })
                            .cloned()
                            .ok_or("Vocabulary changed. Refresh and try again.")?,
                    )
                } else {
                    None
                };
                let mut entry = dictionary::validate(&heard, &wanted).map_err(|e| e.to_string())?;
                entry.app = dictionary::app_scope(app.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                entry.cues = dictionary::cue_words(cues.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                if let Some(original) = &original {
                    entry.enabled = original.enabled;
                    entry.ignore_case = original.ignore_case;
                    entry.contexts.clone_from(&original.contexts);
                    if cues.is_none() {
                        entry.cues.clone_from(&original.cues);
                    }
                    if app.is_none() {
                        entry.app.clone_from(&original.app);
                    }
                }
                entry.sound_alike = sound_alike
                    .or_else(|| original.as_ref().map(|e| e.sound_alike))
                    .unwrap_or(entry.sound_alike)
                    && crate::sound_alike::is_distinctive(&entry.wanted);
                dictionary::save(&mut self.settings.entries, entry, original.as_ref());
                if let Err(error) = save(self) {
                    self.settings.entries = entries;
                    self.settings.macros = macros;
                    return Err(error.to_string());
                }
            }
            Action::DictionaryDelete {
                index,
                expected_heard,
                expected_entry,
            } => {
                if self.settings.entries.get(index).is_none_or(|e| {
                    e.heard != expected_heard
                        || serde_json::to_value(e).ok().as_ref() != expected_entry.as_ref()
                }) {
                    return Err("Vocabulary changed. Refresh and try again.".into());
                }
                self.settings.entries.remove(index);
                if let Err(error) = save(self) {
                    self.settings.entries = entries;
                    self.settings.macros = macros;
                    return Err(error.to_string());
                }
            }
            Action::MacroSave {
                trigger,
                expansion,
                app,
                index,
                expected_trigger,
                expected_macro,
            } => {
                let mut item = macros::validate(&trigger, &expansion, app.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                if let Some(index) = index {
                    if self.settings.macros.get(index).is_none_or(|m| {
                        Some(m.trigger.as_str()) != expected_trigger.as_deref()
                            || serde_json::to_value(m).ok().as_ref() != expected_macro.as_ref()
                    }) {
                        return Err("Snippets changed. Refresh and try again.".into());
                    }
                    if self
                        .settings
                        .macros
                        .iter()
                        .enumerate()
                        .any(|(i, m)| i != index && m.trigger == item.trigger && m.app == item.app)
                    {
                        return Err("A snippet already uses this trigger in that app.".into());
                    }
                    item.enabled = self.settings.macros[index].enabled;
                    self.settings.macros[index] = item;
                } else if let Some(old) = self
                    .settings
                    .macros
                    .iter_mut()
                    .find(|m| m.trigger == item.trigger && m.app == item.app)
                {
                    item.enabled = old.enabled;
                    *old = item;
                } else {
                    self.settings.macros.push(item);
                }
                if let Err(error) = save(self) {
                    self.settings.entries = entries;
                    self.settings.macros = macros;
                    return Err(error.to_string());
                }
            }
            Action::MacroDelete {
                index,
                expected_trigger,
                expected_macro,
            } => {
                if self.settings.macros.get(index).is_none_or(|m| {
                    m.trigger != expected_trigger
                        || serde_json::to_value(m).ok().as_ref() != expected_macro.as_ref()
                }) {
                    return Err("Snippets changed. Refresh and try again.".into());
                }
                self.settings.macros.remove(index);
                if let Err(error) = save(self) {
                    self.settings.entries = entries;
                    self.settings.macros = macros;
                    return Err(error.to_string());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sound_settings_migrate_roundtrip_and_save_transactionally() {
        let legacy: crate::app::Settings = serde_json::from_str("{}").unwrap();
        assert!(legacy.vocabulary_sound_alike);
        let (mut app, _) = crate::app::tests::app();
        assert!(
            app.desktop_vocabulary_save(
                Action::DictionarySoundAlike { enabled: false },
                |_| anyhow::bail!("synthetic save failure")
            )
            .is_err()
        );
        assert!(app.settings.vocabulary_sound_alike);
        app.desktop_vocabulary_save(Action::DictionarySoundAlike { enabled: false }, |_| Ok(()))
            .unwrap();
        let restored: crate::app::Settings =
            serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
        assert!(!restored.vocabulary_sound_alike);
        let action = serde_json::from_value(json!({"type":"dictionary_save", "heard":"obi ante", "wanted":"Obiente", "sound_alike":false})).unwrap();
        app.desktop_vocabulary_save(action, |_| Ok(())).unwrap();
        assert!(!app.settings.entries[0].sound_alike);
        let original = serde_json::to_value(&app.settings.entries[0]).unwrap();
        let action = serde_json::from_value(json!({"type":"dictionary_save", "index":0, "expected_heard":"obi ante", "expected_entry":original, "heard":"obi ante", "wanted":"Obiente", "sound_alike":true})).unwrap();
        app.desktop_vocabulary_save(action, |_| Ok(())).unwrap();
        assert!(app.settings.entries[0].sound_alike);
        let action = serde_json::from_value(json!({"type":"dictionary_save", "heard":"pro ject", "wanted":"Project", "sound_alike":true})).unwrap();
        app.desktop_vocabulary_save(action, |_| Ok(())).unwrap();
        assert!(!app.settings.entries[1].sound_alike);
        assert!(
            serde_json::from_value::<Action>(
                json!({"type":"dictionary_sound_alike", "enabled":"true"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Action>(
                json!({"type":"dictionary_save", "heard":"word", "wanted":"term", "sound_alike":0})
            )
            .is_err()
        );
    }
    #[test]
    fn failed_preferences_save_restores_vocabulary_and_snippets() {
        let (mut app, _) = crate::app::tests::app();
        app.settings.entries = vec![dictionary::validate("heard", "wanted").unwrap()];
        app.settings.macros = vec![macros::validate("trigger", "expansion", "").unwrap()];
        let entries = serde_json::to_value(&app.settings.entries).unwrap();
        let snippets = serde_json::to_value(&app.settings.macros).unwrap();
        for action in [
            json!({"type":"dictionary_save", "heard":"new", "wanted":"word"}),
            json!({"type":"dictionary_delete", "index":0, "expected_heard":"heard", "expected_entry":entries[0]}),
            json!({"type":"macro_save", "trigger":"new", "expansion":"words"}),
            json!({"type":"macro_delete", "index":0, "expected_trigger":"trigger", "expected_macro":snippets[0]}),
        ] {
            let action = serde_json::from_value(action).unwrap();
            assert!(
                app.desktop_vocabulary_save(action, |_| anyhow::bail!("synthetic save failure"))
                    .unwrap_err()
                    .contains("synthetic save failure")
            );
            assert_eq!(
                serde_json::to_value(&app.settings.entries).unwrap(),
                entries
            );
            assert_eq!(
                serde_json::to_value(&app.settings.macros).unwrap(),
                snippets
            );
        }
    }
}
