use crate::app::App;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::enum_variant_names,
    reason = "Variant names are the renderer's action types"
)]
pub(crate) enum Action {
    AssortStart { app: Option<String> },
    AssortApply { index: usize, saved_spelling: bool },
    AssortCancel,
}
impl App {
    pub(crate) fn desktop_assort_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::AssortStart { app } => {
                let context = app.or_else(|| self.dictation_app.clone());
                self.desktop_assort_start(context)?;
            }
            Action::AssortApply {
                index,
                saved_spelling,
            } => self.desktop_assort_apply(index, saved_spelling)?,
            Action::AssortCancel => self.desktop_assort_cancel(),
        }
        Ok(())
    }
}
