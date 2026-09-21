use crate::classification::InstalledPackage;
use crate::classification::builtin::{self, Task as ModelTask};
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Configuration {
    pub checkpoint: String,
    pub tokenizer: String,
    pub limits: String,
    pub local_preview: bool,
    pub use_imported_notes: bool,
    pub correction_checkpoint: String,
    pub correction_tokenizer: String,
    pub correction_limits: String,
    pub correction_preview: bool,
    pub use_imported_corrections: bool,
}

#[allow(
    dead_code,
    reason = "Assort model configuration retained for the renderer integration"
)]
impl Configuration {
    fn available(&self, task: ModelTask) -> bool {
        match task {
            ModelTask::Notes if self.use_imported_notes => self.local_preview,
            ModelTask::Corrections if self.use_imported_corrections => self.correction_preview,
            _ => builtin::available(task),
        }
    }

    fn package(&self, task: ModelTask) -> anyhow::Result<InstalledPackage> {
        let imported = match task {
            ModelTask::Notes => self.use_imported_notes,
            ModelTask::Corrections => self.use_imported_corrections,
        };
        if !imported {
            return builtin::package(task);
        }
        let (checkpoint, tokenizer, limits, enabled) = match task {
            ModelTask::Notes => (
                &self.checkpoint,
                &self.tokenizer,
                &self.limits,
                self.local_preview,
            ),
            ModelTask::Corrections => (
                &self.correction_checkpoint,
                &self.correction_tokenizer,
                &self.correction_limits,
                self.correction_preview,
            ),
        };
        anyhow::ensure!(enabled, "Enable suggestions for the imported model first");
        Ok(InstalledPackage {
            profile: "local-preview".into(),
            local_preview: true,
            checkpoint: checkpoint.trim().into(),
            tokenizer: tokenizer.trim().into(),
            limits: limits.trim().into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_configuration_selects_bundled_tasks_without_manual_paths() {
        let config = Configuration::default();
        assert!(!config.use_imported_notes && !config.use_imported_corrections);
        assert!(config.checkpoint.is_empty() && config.correction_checkpoint.is_empty());
        assert_eq!(
            config.available(ModelTask::Notes),
            builtin::available(ModelTask::Notes)
        );
        assert_eq!(
            config.available(ModelTask::Corrections),
            builtin::available(ModelTask::Corrections)
        );
        let imported = Configuration {
            use_imported_notes: true,
            ..Default::default()
        };
        assert!(!imported.available(ModelTask::Notes));
        assert!(imported.package(ModelTask::Notes).is_err());
    }
}
