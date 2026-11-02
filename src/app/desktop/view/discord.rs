//! Discord connection and companion status.

use crate::app::App;
use serde::Serialize;
use serde_json::Value;

/// The status object is built by the Discord module, which owns its fields.
/// The pairing key is never part of it.
#[derive(Serialize)]
pub(crate) struct Discord {
    discord: Value,
}

pub(crate) fn discord(app: &App) -> Discord {
    Discord {
        discord: app.desktop_discord_status(),
    }
}
