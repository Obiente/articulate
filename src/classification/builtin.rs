//! Small pretrained classifiers embedded in the executable, independent of ASR downloads.
use super::{InstalledPackage, digest_file};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

include!(concat!(env!("OUT_DIR"), "/assort_bundle.rs"));

#[derive(Clone, Copy)]
pub enum Task {
    Notes,
    Corrections,
}
impl Task {
    fn name(self) -> &'static str {
        match self {
            Self::Notes => "notes",
            Self::Corrections => "corrections",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema: u32,
    models: Vec<Model>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    task: String,
    id: String,
    files: Vec<Asset>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    path: String,
    sha256: String,
}

static BUNDLE: OnceLock<Result<Bundle, String>> = OnceLock::new();
static EXTRACTION: Mutex<()> = Mutex::new(());
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() < 160
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
}

fn parse(manifest: &[u8], files: &[(&str, &[u8])]) -> Result<Bundle> {
    ensure!(
        !manifest.is_empty(),
        "This development build does not contain pretrained Assort models"
    );
    ensure!(
        manifest.len() <= 65536 && files.len() <= 32,
        "Invalid embedded classifier bundle size"
    );
    let bundle: Bundle = serde_json::from_slice(manifest)?;
    ensure!(
        bundle.schema == 1 && (1..=2).contains(&bundle.models.len()),
        "Unsupported embedded classifier bundle"
    );
    let mut paths = HashSet::new();
    let mut tasks = HashSet::new();
    let mut ids = HashSet::new();
    for model in &bundle.models {
        ensure!(
            matches!(model.task.as_str(), "notes" | "corrections")
                && tasks.insert(&model.task)
                && ids.insert(&model.id)
                && !model.id.is_empty()
                && model.id.len() <= 64
                && model
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
            "Invalid embedded classifier identity"
        );
        for asset in &model.files {
            ensure!(
                relative_path(&asset.path)
                    && asset.path.starts_with(&format!("{}/", model.task))
                    && paths.insert(asset.path.as_str()),
                "Invalid embedded classifier path"
            );
            let bytes = files
                .iter()
                .find(|(path, _)| *path == asset.path)
                .context("An embedded classifier file is missing")?
                .1;
            ensure!(
                !bytes.is_empty()
                    && bytes.len() <= 128 * 1024 * 1024
                    && asset.sha256.len() == 64
                    && format!("{:x}", Sha256::digest(bytes)) == asset.sha256,
                "An embedded classifier file failed verification"
            );
        }
        for required in [
            "checkpoint/manifest.json",
            "checkpoint/weights.mpk",
            "tokenizer.json",
            "inference-limits.json",
            "evaluation.json",
            "LICENSE.txt",
            if model.task == "notes" {
                "checkpoint/transcript-pipeline.json"
            } else {
                "checkpoint/articulate-task.json"
            },
        ] {
            ensure!(
                model
                    .files
                    .iter()
                    .any(|asset| asset.path == format!("{}/{required}", model.task)),
                "An embedded classifier is missing {required}"
            );
        }
    }
    ensure!(
        paths.len() == files.len() && files.iter().all(|(path, _)| paths.contains(path)),
        "Unexpected embedded classifier files"
    );
    Ok(bundle)
}

fn bundle() -> Result<&'static Bundle> {
    BUNDLE
        .get_or_init(|| parse(MANIFEST, FILES).map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub fn available(task: Task) -> bool {
    bundle().is_ok_and(|bundle| bundle.models.iter().any(|model| model.task == task.name()))
}

pub(super) fn contains(id: &str) -> bool {
    bundle().is_ok_and(|bundle| bundle.models.iter().any(|model| model.id == id))
}

fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !reparse(&metadata),
            "The classifier cache contains an unsafe directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)?,
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn install(model: &Model, root: &Path) -> Result<InstalledPackage> {
    fs::create_dir_all(root)?;
    directory(root)?;
    for asset in &model.files {
        let relative = asset
            .path
            .strip_prefix(&format!("{}/", model.task))
            .context("Invalid classifier path")?;
        let path = root.join(relative);
        let mut parent = root.to_path_buf();
        let components: Vec<_> = relative.split('/').collect();
        for component in &components[..components.len() - 1] {
            parent.push(component);
            directory(&parent)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            ensure!(
                metadata.is_file() && !reparse(&metadata),
                "The classifier cache contains an unsafe file"
            );
            if digest_file(&path, 128 * 1024 * 1024).is_ok_and(|hash| hash == asset.sha256) {
                continue;
            }
        }
        let bytes = FILES
            .iter()
            .find(|(name, _)| *name == asset.path)
            .context("Missing classifier payload")?
            .1;
        let temporary = parent.join(format!(
            ".assort-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> Result<()> {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            output.write_all(bytes)?;
            output.sync_all()?;
            drop(output);
            if path.exists() {
                fs::remove_file(&path)?;
            }
            fs::rename(&temporary, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
    }
    Ok(InstalledPackage {
        profile: model.id.clone(),
        local_preview: false,
        checkpoint: root.join("checkpoint"),
        tokenizer: root.join("tokenizer.json"),
        limits: root.join("inference-limits.json"),
    })
}

pub fn package(task: Task) -> Result<InstalledPackage> {
    let _lock = EXTRACTION.lock().unwrap_or_else(|error| error.into_inner());
    let model = bundle()?
        .models
        .iter()
        .find(|model| model.task == task.name())
        .context("This build does not contain the requested Assort model")?;
    let digest = format!("{:x}", Sha256::digest(MANIFEST));
    let mut root = crate::model::data_dir();
    fs::create_dir_all(&root)?;
    for component in ["assort", &digest[..24], task.name()] {
        root.push(component);
        directory(&root)?;
    }
    let package = install(model, &root)?;
    super::verify_task_package(&package, None, matches!(task, Task::Corrections))?;
    Ok(package)
}

/// Release checks verify embedded bytes without writing a cache or loading weights.
pub(super) fn verify_embedded() -> Result<()> {
    let bundle = bundle()?;
    for task in [Task::Notes, Task::Corrections] {
        let model = bundle
            .models
            .iter()
            .find(|model| model.task == task.name())
            .context("The release must include both pretrained Assort tasks")?;
        let json = |name: &str| -> Result<serde_json::Value> {
            let path = format!("{}/{name}", model.task);
            let bytes = FILES
                .iter()
                .find(|(file, _)| *file == path)
                .context("Missing embedded model metadata")?
                .1;
            Ok(serde_json::from_slice(bytes)?)
        };
        let manifest = json("checkpoint/manifest.json")?;
        ensure!(
            manifest["format_version"] == 1
                && manifest["architecture"] == "assort-candidate-scoring-v1"
                && manifest["burn_version"] == "0.21.0"
                && manifest["weights_status"] == "trained",
            "An embedded model is incompatible or untrained"
        );
        for (name, expected) in [
            ("checkpoint/weights.mpk", &manifest["weights_sha256"]),
            ("tokenizer.json", &manifest["tokenizer"]["fingerprint"]),
        ] {
            let asset = model
                .files
                .iter()
                .find(|asset| asset.path == format!("{}/{name}", model.task))
                .context("Missing embedded model file")?;
            ensure!(
                expected.as_str() == Some(asset.sha256.as_str()),
                "Embedded weights or tokenizer do not match their training manifest"
            );
        }
        ensure!(
            manifest["tokenizer"]["kind"] == "huggingface-json-v1"
                && manifest["tokenizer"]["pad_id"] == 0,
            "Unsupported embedded tokenizer contract"
        );
        match task {
            Task::Corrections => ensure!(
                json("checkpoint/articulate-task.json")? == super::correction_contract(),
                "Embedded correction task does not match Articulate"
            ),
            Task::Notes => {
                let pipeline = json("checkpoint/transcript-pipeline.json")?;
                ensure!(
                    pipeline["version"] == 1
                        && pipeline["category_ids"]
                            == serde_json::json!(["decision", "action", "key_fact", "background"])
                        && pipeline["training_label_smoothing"]
                            .as_f64()
                            .is_some_and(|v| v.is_finite() && (0.0..1.0).contains(&v)),
                    "Embedded notes task is incompatible"
                );
            }
        }
        let limits = json("inference-limits.json")?;
        for (field, max) in [
            ("max_batch_size", 32),
            ("max_questions", 32),
            ("max_candidates", 32),
            ("max_state_tokens", 4096),
            ("max_question_tokens", 4096),
            ("max_candidate_tokens", 1024),
            ("max_padded_tokens", 1_048_576),
        ] {
            ensure!(
                limits[field].as_u64().is_some_and(|v| v > 0 && v <= max),
                "Unsupported embedded inference limit: {field}"
            );
        }
    }
    Ok(())
}

/// Both the parent and isolated worker verify every file against the executable.
pub(super) fn verify(package: &InstalledPackage, corrections: bool) -> Result<bool> {
    let Some(model) = bundle().ok().and_then(|bundle| {
        bundle
            .models
            .iter()
            .find(|model| model.id == package.profile)
    }) else {
        return Ok(false);
    };
    ensure!(
        model.task == if corrections { "corrections" } else { "notes" },
        "The bundled classifier does not support this task"
    );
    ensure!(
        !package.local_preview,
        "A bundled classifier cannot use the import policy"
    );
    let root = package
        .checkpoint
        .parent()
        .context("Invalid classifier cache location")?;
    ensure!(
        root.is_absolute()
            && package.checkpoint == root.join("checkpoint")
            && package.tokenizer == root.join("tokenizer.json")
            && package.limits == root.join("inference-limits.json"),
        "The bundled classifier paths do not match"
    );
    for asset in &model.files {
        let path = root.join(
            asset
                .path
                .strip_prefix(&format!("{}/", model.task))
                .context("Invalid embedded classifier path")?,
        );
        ensure!(
            digest_file(&path, 128 * 1024 * 1024)? == asset.sha256,
            "The bundled classifier failed file verification"
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundle_rejects_traversal_duplicate_assets_and_modified_payloads() {
        assert!(!relative_path("notes/../other"));
        assert!(!relative_path("notes/C:\\data"));
        assert!(!relative_path("/notes/model"));
        let manifest=br#"{"schema":1,"models":[{"task":"notes","id":"test","files":[{"path":"notes/x","sha256":"bad"}]}]}"#;
        assert!(parse(manifest, &[("notes/x", b"weights")]).is_err());
        assert!(parse(b"", &[]).is_err());
        let names = [
            "notes/checkpoint/manifest.json",
            "notes/checkpoint/weights.mpk",
            "notes/tokenizer.json",
            "notes/inference-limits.json",
            "notes/evaluation.json",
            "notes/LICENSE.txt",
            "notes/checkpoint/transcript-pipeline.json",
        ];
        let payloads: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (*name, b"synthetic parser fixture".as_slice()))
            .collect();
        let assets: Vec<_> = payloads.iter().map(|(path,bytes)|serde_json::json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))})).collect();
        let valid = serde_json::json!({"schema":1,"models":[{"task":"notes","id":"parser-test","files":assets}]});
        assert!(parse(&serde_json::to_vec(&valid).unwrap(), &payloads).is_ok());
        let mut duplicate = valid.clone();
        duplicate["models"][0]["files"]
            .as_array_mut()
            .unwrap()
            .push(valid["models"][0]["files"][0].clone());
        assert!(parse(&serde_json::to_vec(&duplicate).unwrap(), &payloads).is_err());
        let mut changed = payloads;
        changed[0].1 = b"modified";
        assert!(parse(&serde_json::to_vec(&valid).unwrap(), &changed).is_err());
    }

    #[test]
    fn bundled_files_and_models_have_complete_contracts_when_present() {
        if MANIFEST.is_empty() {
            return;
        }
        let bundle = parse(MANIFEST, FILES).unwrap();
        assert!(!bundle.models.is_empty());
        for model in &bundle.models {
            assert!(contains(&model.id));
        }
    }

    #[test]
    fn installed_bundle_rejects_tampering_and_repairs_only_its_files() {
        if MANIFEST.is_empty() {
            return;
        }
        verify_embedded().unwrap();
        let bundle = bundle().unwrap();
        let root = std::env::temp_dir().join(format!(
            "articulate-assort-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for model in &bundle.models {
            let task_root = root.join(&model.task);
            let package = install(model, &task_root).unwrap();
            let corrections = model.task == "corrections";
            assert!(verify(&package, corrections).unwrap());
            assert!(verify(&package, !corrections).is_err());
            super::super::verify_task_package(&package, None, corrections).unwrap();
            fs::write(task_root.join("unrelated.txt"), b"preserve").unwrap();
            fs::write(package.checkpoint.join("weights.mpk"), b"modified").unwrap();
            assert!(verify(&package, corrections).is_err());
            let repaired = install(model, &task_root).unwrap();
            assert!(verify(&repaired, corrections).unwrap());
            assert_eq!(
                fs::read(task_root.join("unrelated.txt")).unwrap(),
                b"preserve"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
