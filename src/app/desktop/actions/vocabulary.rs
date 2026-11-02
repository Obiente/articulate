//! Vocabulary entries and snippets.

use crate::app::App;
use crate::{dictionary, macros};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    DictionarySave {
        heard: String,
        wanted: String,
        app: Option<String>,
        cues: Option<String>,
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
        match action {
            Action::DictionarySave {
                heard,
                wanted,
                app,
                cues,
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
                dictionary::save(&mut self.settings.entries, entry, original.as_ref());
                self.save_preferences().map_err(|e| e.to_string())?;
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
                self.save_preferences().map_err(|e| e.to_string())?;
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
                self.save_preferences().map_err(|e| e.to_string())?;
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
                self.save_preferences().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
