use super::{Event, progress};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};
pub(super) struct Artifact<'a> {
    pub url: &'a str,
    pub destination: &'a Path,
    pub size: u64,
    pub sha: &'a str,
    pub stage: &'a str,
}
pub(super) fn fetch(
    a: Artifact<'_>,
    cancel: &Arc<AtomicBool>,
    events: &Sender<Event>,
) -> anyhow::Result<()> {
    crate::model::download::fetch(
        crate::model::download::Artifact {
            url: a.url,
            destination: a.destination,
            size: a.size,
            sha: a.sha,
            stage: a.stage,
        },
        cancel,
        &mut |stage, fraction| progress(events, stage, fraction),
    )
}
