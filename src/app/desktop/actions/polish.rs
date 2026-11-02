use crate::app::App;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    PolishStart,
    PolishApply,
    PolishCancel,
}
impl App {
    pub(crate) fn desktop_polish_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::PolishStart => self.polish_start(),
            Action::PolishApply => self.polish_apply(),
            Action::PolishCancel => self.desktop_cleanup_cancel(),
        }
        Ok(())
    }
}
