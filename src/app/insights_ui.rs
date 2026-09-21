use crate::insights::Report;

#[derive(Default)]
pub(super) struct State {
    pub report: Option<Report>,
    pub loading: bool,
    pub error: Option<String>,
    pub stale: bool,
}
