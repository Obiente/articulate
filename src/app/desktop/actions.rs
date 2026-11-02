//! Renderer actions, grouped by feature.
//!
//! Each feature module defines its own `Action` enum (tagged by `type`) and an
//! `App` handler. `Action` here routes a request to the feature that declares
//! its `type`. To add a feature, create a module, add a variant below and one
//! entry to `FEATURES` and `App::desktop_action`. ui/src/actions.js lists every
//! action type; a test keeps both lists in sync.

use crate::app::App;
use serde::{Deserialize, Deserializer, de::Error as _};
use serde_json::Value;

pub(super) mod calls;
pub(super) mod capture;
pub(super) mod discord;
pub(super) mod library;
pub(super) mod models;
pub(super) mod output;
pub(super) mod settings;
pub(super) mod updates;
pub(super) mod vocabulary;

pub(crate) enum Action {
    Library(library::Action),
    Capture(capture::Action),
    Output(output::Action),
    Vocabulary(vocabulary::Action),
    Settings(settings::Action),
    Models(models::Action),
    Discord(discord::Action),
    Updates(updates::Action),
    Calls(calls::Action),
}

type Parser = fn(&Value) -> Result<Action, serde_json::Error>;

/// Feature parsers in routing order.
const FEATURES: &[(&str, Parser)] = &[
    ("library", |value| {
        library::Action::deserialize(value).map(Action::Library)
    }),
    ("capture", |value| {
        capture::Action::deserialize(value).map(Action::Capture)
    }),
    ("output", |value| {
        output::Action::deserialize(value).map(Action::Output)
    }),
    ("vocabulary", |value| {
        vocabulary::Action::deserialize(value).map(Action::Vocabulary)
    }),
    ("settings", |value| {
        settings::Action::deserialize(value).map(Action::Settings)
    }),
    ("models", |value| {
        models::Action::deserialize(value).map(Action::Models)
    }),
    ("discord", |value| {
        discord::Action::deserialize(value).map(Action::Discord)
    }),
    ("updates", |value| {
        updates::Action::deserialize(value).map(Action::Updates)
    }),
    ("calls", |value| {
        calls::Action::deserialize(value).map(Action::Calls)
    }),
];

/// Whether a feature rejected the action's type, rather than its fields.
fn unknown_type(error: &serde_json::Error, kind: &str) -> bool {
    error
        .to_string()
        .starts_with(&format!("unknown variant `{kind}`"))
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return Err(D::Error::custom("An action needs a type."));
        };
        for (_, parse) in FEATURES {
            match parse(&value) {
                Ok(action) => return Ok(action),
                // The feature owns this type, but the fields are invalid.
                Err(error) if !unknown_type(&error, kind) => {
                    return Err(D::Error::custom(error));
                }
                Err(_) => {}
            }
        }
        Err(D::Error::custom(format!("Unsupported action `{kind}`.")))
    }
}

impl Action {
    pub(crate) fn installs_update(&self) -> bool {
        matches!(self, Self::Updates(updates::Action::UpdateInstall))
    }

    /// Run actions that must not wait on the controller thread, such as a
    /// save dialog that must not pause capture and autosave. Returns `None`
    /// for actions the controller handles.
    pub(crate) fn run_outside_controller(&self) -> Option<Result<(), String>> {
        match self {
            Self::Output(action) => output::run_outside_controller(action),
            Self::Calls(action) => calls::run_outside_controller(action),
            _ => None,
        }
    }
}

impl App {
    pub(crate) fn desktop_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Library(action) => self.desktop_library_action(action),
            Action::Capture(action) => self.desktop_capture_action(action),
            Action::Output(action) => output::run(action),
            Action::Vocabulary(action) => self.desktop_vocabulary_action(action),
            Action::Settings(action) => self.desktop_settings_action(action),
            Action::Models(action) => self.desktop_models_action(action),
            Action::Discord(action) => self.desktop_discord_action(action),
            Action::Updates(action) => self.desktop_updates_action(action),
            Action::Calls(action) => self.desktop_calls_action(action),
        }
    }
}

/// Every action type each feature accepts, read from serde's own error for an
/// unknown type.
#[cfg(test)]
pub(super) fn action_types() -> Vec<(&'static str, Vec<String>)> {
    FEATURES
        .iter()
        .map(|(feature, parse)| {
            let error = match parse(&serde_json::json!({"type": "__probe__"})) {
                Ok(_) => panic!("a probe type must not parse"),
                Err(error) => error.to_string(),
            };
            // "expected one of `a`, `b`", "expected `a` or `b`" or "expected `a`".
            let expected = error
                .split_once("expected ")
                .map(|(_, list)| list)
                .unwrap_or_default();
            let types = expected
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect();
            (*feature, types)
        })
        .collect()
}
