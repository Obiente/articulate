use anyhow::{Context, Result};
#[path = "download.rs"]
#[allow(dead_code)]
pub mod download;
use std::sync::{Arc, atomic::AtomicBool};
use std::{fs, path::PathBuf};

pub const NAME: &str = "Qwen3-ASR-1.7B-Q8_0.gguf";
pub const SIZE: u64 = 2_185_030_624;
pub const SHA256: &str = "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7";
const URL: &str = "https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf/resolve/3555bd238a8572bbace3ebf60d23b036dc0a5dbe/Qwen3-ASR-1.7B-Q8_0.gguf";
pub const VERIFIER_NAME: &str = "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";
const VERIFIER_SIZE: u64 = 751_094_240;
const VERIFIER_SHA256: &str = "b94545b313b3223fda7b2857a52681da813935c2127643d1e9ff0c23d988089c";
const VERIFIER_URL: &str = "https://huggingface.co/handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/resolve/8139c4ec14bdc45c361adf8d57c27c28e7478272/nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";

pub fn data_dir() -> PathBuf {
    // Controller tests can save preferences. Never let those writes touch a real profile.
    if cfg!(test) {
        if let Some(root) = std::env::var_os("ARTICULATE_BRAIN_TEST_DIR") {
            let root = PathBuf::from(root);
            assert!(
                root.is_absolute(),
                "Use an absolute isolated model test directory"
            );
            return root.join("TranscribeLocal");
        }
        return std::env::temp_dir().join(format!("articulate-tests-{}", std::process::id()));
    }
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TranscribeLocal");
    static CLEANUP: std::sync::Once = std::sync::Once::new();
    CLEANUP.call_once(|| {
        let _ = download::prune_stale(
            &directory.join("models"),
            std::time::Duration::from_secs(7 * 86400),
        );
    });
    directory
}
pub fn default_path() -> PathBuf {
    data_dir().join("models").join(NAME)
}
pub fn verifier_path() -> PathBuf {
    data_dir().join("models").join(VERIFIER_NAME)
}
pub fn download_verifier(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_file(
        VERIFIER_NAME,
        VERIFIER_SIZE,
        VERIFIER_SHA256,
        VERIFIER_URL,
        &mut progress,
    )
}

// No audio, text, IDs, settings or dictionary data are included in this request.
pub fn download(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_file(NAME, SIZE, SHA256, URL, &mut progress)
}

#[allow(
    dead_code,
    reason = "Preserve speaker-model download for React audio setup"
)]
pub fn download_speakers(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_speakers_cancellable(&Arc::new(AtomicBool::new(false)), &mut progress)
}

#[allow(dead_code)]
pub fn download_speakers_cancellable(
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<PathBuf> {
    download_file_cancellable(
        crate::speakers::NAME,
        236606560,
        "62faec7b99ad23e323087597604b50728abe85089b6364970b019a845547bf99",
        "https://huggingface.co/handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf/resolve/ae4afbb5c3d33b71cf2dbf600022b655ee706dd0/diar_streaming_sortformer_4spk-v2.1-F16.gguf",
        cancel,
        &mut progress,
    )
}

pub(crate) fn download_file(
    name: &str,
    size: u64,
    expected_hash: &str,
    url: &str,
    progress: &mut impl FnMut(f32),
) -> Result<PathBuf> {
    download_file_cancellable(
        name,
        size,
        expected_hash,
        url,
        &Arc::new(AtomicBool::new(false)),
        progress,
    )
}

#[allow(dead_code)]
pub fn download_file_cancellable(
    name: &str,
    size: u64,
    expected_hash: &str,
    url: &str,
    cancel: &Arc<AtomicBool>,
    progress: &mut impl FnMut(f32),
) -> Result<PathBuf> {
    anyhow::ensure!(
        std::path::Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            == Some(name)
            && !name.contains(['/', '\\', ':']),
        "Invalid model filename"
    );
    let _lease = match name {
        NAME => Some(acquire(ModelId::Speech)),
        VERIFIER_NAME => Some(acquire(ModelId::Verifier)),
        crate::speakers::NAME => Some(acquire(ModelId::Speaker)),
        "sensevoice-small-q8.gguf" => Some(acquire(ModelId::SoundCues)),
        _ => None,
    };
    let path = data_dir().join("models").join(name);
    fs::create_dir_all(path.parent().context("Missing model directory")?)?;
    download::fetch(
        download::Artifact {
            url,
            destination: &path,
            size,
            sha: expected_hash,
            stage: "Downloading model",
        },
        cancel,
        &mut |_, fraction| {
            if let Some(value) = fraction {
                progress(value)
            }
        },
    )?;
    Ok(path)
}

#[allow(dead_code)]
pub fn download_cancellable(
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<PathBuf> {
    download_file_cancellable(NAME, SIZE, SHA256, URL, cancel, &mut progress)
}
#[allow(dead_code)]
pub fn download_verifier_cancellable(
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<PathBuf> {
    download_file_cancellable(
        VERIFIER_NAME,
        VERIFIER_SIZE,
        VERIFIER_SHA256,
        VERIFIER_URL,
        cancel,
        &mut progress,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelId {
    Speech,
    Verifier,
    Speaker,
    SoundCues,
    Polish,
    Notes,
}
#[derive(serde::Serialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub installed: bool,
    pub disk_size: u64,
    pub path: PathBuf,
    pub in_use: bool,
}
static MODEL_USERS: std::sync::Mutex<Vec<ModelId>> = std::sync::Mutex::new(Vec::new());
/// Retain this lease for the complete lifetime of an inference engine or download.
pub struct ModelLease(ModelId);
impl Drop for ModelLease {
    fn drop(&mut self) {
        let mut users = MODEL_USERS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = users.iter().position(|id| *id == self.0) {
            users.remove(index);
        }
    }
}
#[allow(dead_code)]
pub fn acquire(id: ModelId) -> ModelLease {
    MODEL_USERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(id);
    ModelLease(id)
}
fn managed_path(id: ModelId) -> PathBuf {
    match id {
        ModelId::Speech => data_dir().join("models").join(NAME),
        ModelId::Verifier => verifier_path(),
        ModelId::Speaker => data_dir().join("models").join(crate::speakers::NAME),
        ModelId::SoundCues => data_dir().join("models").join("sensevoice-small-q8.gguf"),
        ModelId::Polish => crate::polish::model_path(crate::polish::ModelProfile::Polish),
        ModelId::Notes => crate::polish::model_path(crate::polish::ModelProfile::Summary),
    }
}
#[allow(dead_code)]
pub fn list_models() -> Vec<ModelInfo> {
    let users = MODEL_USERS.lock().unwrap_or_else(|e| e.into_inner());
    [
        ModelId::Speech,
        ModelId::Verifier,
        ModelId::Speaker,
        ModelId::SoundCues,
        ModelId::Polish,
        ModelId::Notes,
    ]
    .into_iter()
    .map(|id| {
        let path = managed_path(id);
        let disk_size = path.metadata().map_or(0, |m| m.len());
        let installed = match id {
            ModelId::Speech => disk_size == SIZE,
            ModelId::Verifier => disk_size == VERIFIER_SIZE,
            ModelId::Speaker => disk_size == 236_606_560,
            ModelId::SoundCues => crate::sensevoice::installed(),
            ModelId::Polish => {
                crate::polish::profile_installed(crate::polish::ModelProfile::Polish)
            }
            ModelId::Notes => {
                crate::polish::profile_installed(crate::polish::ModelProfile::Summary)
            }
        };
        ModelInfo {
            id,
            installed,
            disk_size,
            path,
            in_use: users.contains(&id),
        }
    })
    .collect()
}
#[allow(dead_code)]
pub fn delete_model(id: ModelId) -> Result<()> {
    let users = MODEL_USERS.lock().unwrap_or_else(|e| e.into_inner());
    anyhow::ensure!(!users.contains(&id), "This model is in use");
    let path = managed_path(id);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let _ = fs::remove_file(path.with_extension("verified.json"));
    crate::polish::invalidate_install_state();
    Ok(())
}

pub fn acquire_path(path: &std::path::Path) -> ModelLease {
    let canonical = path.canonicalize().ok();
    let id = [
        ModelId::Speech,
        ModelId::Verifier,
        ModelId::Speaker,
        ModelId::SoundCues,
        ModelId::Polish,
        ModelId::Notes,
    ]
    .into_iter()
    .find(|id| {
        let managed = managed_path(*id);
        path == managed
            || canonical
                .as_ref()
                .is_some_and(|path| managed.canonicalize().ok().as_ref() == Some(path))
    })
    .unwrap_or(ModelId::Speech);
    acquire(id)
}

#[cfg(test)]
mod management_tests {
    use super::*;
    #[test]
    fn active_lease_prevents_deletion() {
        let lease = acquire(ModelId::Speech);
        assert!(
            list_models()
                .iter()
                .find(|m| m.id == ModelId::Speech)
                .unwrap()
                .in_use
        );
        assert!(delete_model(ModelId::Speech).is_err());
        drop(lease);
    }
}
