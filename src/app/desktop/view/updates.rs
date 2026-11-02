//! Application update status.

use crate::app::App;
use crate::update::Status;
use serde::Serialize;

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum UpdaterView<'a> {
    Idle,
    Checking,
    Latest,
    Available { version: &'a str, size: u64 },
    Downloading { version: &'a str, progress: f32 },
    Ready { version: &'a str },
    Error { error: &'a str },
}

#[derive(Serialize)]
pub(crate) struct Updater<'a> {
    updater: UpdaterView<'a>,
}

pub(crate) fn updater(app: &App) -> Updater<'_> {
    Updater {
        updater: match &app.updater.status {
            Status::Idle => UpdaterView::Idle,
            Status::Checking => UpdaterView::Checking,
            Status::Latest => UpdaterView::Latest,
            Status::Available(release) => UpdaterView::Available {
                version: &release.version,
                size: release.size,
            },
            Status::Downloading { version, progress } => UpdaterView::Downloading {
                version,
                progress: *progress,
            },
            Status::Ready(installer) => UpdaterView::Ready {
                version: &installer.version,
            },
            Status::Error(error) => UpdaterView::Error { error },
        },
    }
}
