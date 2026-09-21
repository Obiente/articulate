use super::{Event, check_cancel, progress};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

pub const MODEL_NAME: &str = "Qwen3.5-0.8B-Q8_0.gguf";
pub const MODEL_BYTES: u64 = 833_592_096;
const MODEL_SHA: &str = "37ae482d336108d23516fa35e8e0c4126688d81018b87178a18d752a1357814f";
const MODEL_URL: &str = "https://huggingface.co/ggml-org/Qwen3.5-0.8B-GGUF/resolve/8fea620810c4afa23dd6443f999a48574c1611a3/Qwen3.5-0.8B-Q8_0.gguf";
const RUNTIME_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10964/llama-b10964-bin-win-cpu-x64.zip";
const RUNTIME_SHA: &str = "917f39c076402c421224824607397af20f53625a60defc20e8dd22446bf4c5d7";
const RUNTIME_BYTES: u64 = 18_427_629;
const VULKAN_RUNTIME_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10964/llama-b10964-bin-win-vulkan-x64.zip";
const VULKAN_RUNTIME_SHA: &str = "1ee3ad952f4ba71f438bd6d7bebef19e1c7af04adcaa35d08b4ddabb27d4c642";
const VULKAN_RUNTIME_BYTES: u64 = 31_674_542;
pub const DOWNLOAD_BYTES: u64 = MODEL_BYTES + RUNTIME_BYTES;
pub const SUMMARY_BYTES: u64 = 3_143_656_608;
pub const SUMMARY_DOWNLOAD_BYTES: u64 = SUMMARY_BYTES + RUNTIME_BYTES + VULKAN_RUNTIME_BYTES;
const SUMMARY_NAME: &str = "Qwen3.5-4B-Q5_K_M.gguf";
const SUMMARY_SHA: &str = "8814232b85594dcd46c50e5b8b29324a7efe9e746edbe8a3d1df3d3fce7aad39";
const SUMMARY_URL: &str = "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q5_K_M.gguf";
use super::ModelProfile;
fn manifest(profile: ModelProfile) -> (&'static str, u64, &'static str, &'static str) {
    match profile {
        ModelProfile::Polish => (MODEL_NAME, MODEL_BYTES, MODEL_SHA, MODEL_URL),
        ModelProfile::Summary => (SUMMARY_NAME, SUMMARY_BYTES, SUMMARY_SHA, SUMMARY_URL),
    }
}
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
struct Artifact {
    name: String,
    bytes: u64,
    sha256: String,
}
fn artifacts() -> Result<Vec<Artifact>> {
    Ok(serde_json::from_str(include_str!("runtime-files.json"))?)
}
fn gpu_artifacts() -> Result<Vec<Artifact>> {
    Ok(serde_json::from_str(include_str!(
        "runtime-vulkan-files.json"
    ))?)
}
pub(super) fn root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("ARTICULATE_POLISH_TEST_DIR") {
        return PathBuf::from(root);
    }
    crate::model::data_dir()
        .join("polish")
        .join("qwen35-08b-q8-llama-b10964")
}
pub(super) fn model_for(profile: ModelProfile) -> PathBuf {
    root().join(manifest(profile).0)
}
pub(super) fn executable() -> PathBuf {
    root().join("runtime").join("llama-server.exe")
}
pub(super) fn executable_for_gpu() -> PathBuf {
    root().join("runtime-vulkan").join("llama-server.exe")
}

// This indicates a complete downloaded runtime, not a usable graphics device.
// The caller must verify its hashes before trying it and fall back to the CPU
// runtime if Vulkan initialization or model loading fails.
pub(super) fn gpu_available() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
        && fs::read_to_string(root().join("ready-vulkan"))
            .is_ok_and(|marker| marker == VULKAN_RUNTIME_SHA)
        && gpu_artifacts().is_ok_and(|files| {
            files.iter().all(|file| {
                root()
                    .join("runtime-vulkan")
                    .join(&file.name)
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.len() == file.bytes)
            })
        })
}

pub(super) fn verify_gpu(cancel: &AtomicBool) -> Result<()> {
    ensure!(gpu_available(), "The local GPU tools are not installed.");
    for file in gpu_artifacts()? {
        verified(
            &root().join("runtime-vulkan").join(file.name),
            file.bytes,
            &file.sha256,
            cancel,
        )?;
    }
    Ok(())
}

pub fn installed() -> bool {
    profile_installed(ModelProfile::Polish)
}
pub fn profile_installed(profile: ModelProfile) -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
        && model_for(profile)
            .metadata()
            .is_ok_and(|m| m.len() == manifest(profile).1)
        && executable().is_file()
        && root().join("ready").is_file()
        && artifacts().is_ok_and(|files| {
            files.iter().all(|file| {
                root()
                    .join("runtime")
                    .join(&file.name)
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.len() == file.bytes)
            })
        })
}

pub(super) fn ordinary(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "The local editor contains an unexpected file type."
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "The local editor contains an unexpected linked file."
        );
    }
    Ok(())
}
pub(super) fn verified(path: &Path, size: u64, expected: &str, cancel: &AtomicBool) -> Result<()> {
    ordinary(path)?;
    let mut file = File::open(path)?;
    ensure!(
        file.metadata()?.len() == size,
        "A local editor file has the wrong size. Download it again."
    );
    let mut buffer = vec![0; 256 * 1024];
    let mut hash = Sha256::new();
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == expected,
        "A local editor file failed verification. Download it again."
    );
    Ok(())
}
pub(super) fn verify_profile(profile: ModelProfile, cancel: &AtomicBool) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "The managed local editor currently requires Windows x64."
    );
    ensure!(
        profile_installed(profile),
        "Download this local model in Settings first."
    );
    let (_, size, sha, _) = manifest(profile);
    verified(&model_for(profile), size, sha, cancel)?;
    for file in artifacts()? {
        verified(
            &root().join("runtime").join(file.name),
            file.bytes,
            &file.sha256,
            cancel,
        )?;
    }
    Ok(())
}

struct DownloadLock;
impl Drop for DownloadLock {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::Release);
    }
}
#[cfg(test)]
pub(super) fn download(cancel: &Arc<AtomicBool>, events: &Sender<Event>) -> Result<()> {
    download_profile(ModelProfile::Polish, cancel, events)
}
pub(super) fn download_profile(
    profile: ModelProfile,
    cancel: &Arc<AtomicBool>,
    events: &Sender<Event>,
) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "The managed local editor currently requires Windows x64."
    );
    ensure!(
        !DOWNLOADING.swap(true, Ordering::AcqRel),
        "The local editor download is already running."
    );
    let _lock = DownloadLock;
    fs::create_dir_all(root())?;
    let model = model_for(profile);
    let (_, model_size, model_sha, model_url) = manifest(profile);
    if verified(&model, model_size, model_sha, cancel).is_err() {
        fetch(
            model_url,
            &model,
            model_size,
            model_sha,
            if profile == ModelProfile::Polish {
                "Downloading editing model"
            } else {
                "Downloading summary model"
            },
            cancel,
            events,
        )?;
    }
    install_runtime(false, cancel, events)?;
    if profile == ModelProfile::Summary
        && let Err(error) = install_runtime(true, cancel, events)
    {
        // GPU support is optional. Preserve a working CPU installation if the
        // additional archive is unavailable, but always honor cancellation.
        check_cancel(cancel)?;
        progress(
            events,
            &format!("Notes are ready on CPU. GPU tools could not be prepared: {error:#}"),
            None,
        );
    }
    Ok(())
}

fn install_runtime(gpu: bool, cancel: &Arc<AtomicBool>, events: &Sender<Event>) -> Result<()> {
    let (url, size, sha, archive_name, directory_name, marker_name) = if gpu {
        (
            VULKAN_RUNTIME_URL,
            VULKAN_RUNTIME_BYTES,
            VULKAN_RUNTIME_SHA,
            "runtime-vulkan.zip",
            "runtime-vulkan",
            "ready-vulkan",
        )
    } else {
        (
            RUNTIME_URL,
            RUNTIME_BYTES,
            RUNTIME_SHA,
            "runtime.zip",
            "runtime",
            "ready",
        )
    };
    let expected = if gpu { gpu_artifacts()? } else { artifacts()? };
    let runtime = root().join(directory_name);
    if fs::read_to_string(root().join(marker_name)).is_ok_and(|marker| marker == sha)
        && expected.iter().all(|artifact| {
            verified(
                &runtime.join(&artifact.name),
                artifact.bytes,
                &artifact.sha256,
                cancel,
            )
            .is_ok()
        })
    {
        check_cancel(cancel)?;
        return Ok(());
    }
    check_cancel(cancel)?;
    let archive = root().join(archive_name);
    if verified(&archive, size, sha, cancel).is_err() {
        fetch(
            url,
            &archive,
            size,
            sha,
            if gpu {
                "Downloading GPU tools"
            } else {
                "Downloading local editing tools"
            },
            cancel,
            events,
        )?;
    }
    progress(
        events,
        if gpu {
            "Preparing GPU tools"
        } else {
            "Preparing local editing tools"
        },
        None,
    );
    let stage = root().join(format!("stage-{}", crate::discord::plugin::new_token()?));
    fs::create_dir(&stage)?;
    let result = (|| -> Result<()> {
        let mut zip = zip::ZipArchive::new(File::open(&archive)?)?;
        let expected = if gpu { gpu_artifacts()? } else { artifacts()? };
        ensure!(zip.len() <= 100, "Unexpected editing runtime archive.");
        for artifact in &expected {
            check_cancel(cancel)?;
            ensure!(safe_name(&artifact.name), "Unexpected runtime file name.");
            let mut entry = zip
                .by_name(&artifact.name)
                .context("Editing runtime is incomplete.")?;
            ensure!(
                entry.size() == artifact.bytes
                    && !entry.is_dir()
                    && entry
                        .unix_mode()
                        .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "Unexpected runtime archive entry."
            );
            let path = stage.join(&artifact.name);
            let mut target = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            std::io::copy(&mut entry.by_ref().take(artifact.bytes + 1), &mut target)?;
            target.sync_all()?;
            drop(target);
            verified(&path, artifact.bytes, &artifact.sha256, cancel)?;
        }
        fs::write(
            stage.join("LICENSE-llama.txt"),
            include_bytes!("../../assets/polish/llama-LICENSE.txt"),
        )?;
        fs::write(
            stage.join("LICENSE-LLVM-OpenMP.txt"),
            include_bytes!("../../assets/polish/LLVM-OpenMP-LICENSE.txt"),
        )?;
        fs::write(
            root().join("LICENSE-model.txt"),
            include_bytes!("../../assets/polish/Qwen-LICENSE.txt"),
        )?;
        fs::write(
            root().join("NOTICE.md"),
            include_bytes!("../../assets/polish/NOTICE.md"),
        )?;
        check_cancel(cancel)?;
        let runtime = root().join(directory_name);
        let healthy = runtime.exists()
            && expected.iter().all(|artifact| {
                verified(
                    &runtime.join(&artifact.name),
                    artifact.bytes,
                    &artifact.sha256,
                    cancel,
                )
                .is_ok()
            });
        check_cancel(cancel)?;
        if healthy {
            remove_owned_directory(&stage)?;
        } else if runtime.exists() {
            let backup = root().join(format!("replaced-{}", crate::discord::plugin::new_token()?));
            fs::rename(&runtime,&backup).context("The editing tools are still in use. Close the preview and retry Repair, or restart Articulate first.")?;
            if let Err(error) = fs::rename(&stage, &runtime) {
                let _ = fs::rename(&backup, &runtime);
                return Err(error.into());
            }
            let _ = remove_owned_directory(&backup);
        } else {
            fs::rename(&stage, &runtime)?;
        }
        fs::write(root().join(marker_name), sha)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = remove_owned_directory(&stage);
    }
    result
}

fn remove_owned_directory(path: &Path) -> Result<()> {
    let parent = fs::canonicalize(root())?;
    let target = fs::canonicalize(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink()
            && target.parent() == Some(parent.as_path())
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("stage-") || n.starts_with("replaced-")),
        "Refusing to remove an unexpected editing directory."
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Refusing to remove a linked editing directory."
        );
    }
    fs::remove_dir_all(target)?;
    Ok(())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
fn fetch(
    url: &str,
    destination: &Path,
    size: u64,
    sha: &str,
    stage: &str,
    cancel: &Arc<AtomicBool>,
    events: &Sender<Event>,
) -> Result<()> {
    super::transfer::fetch(
        super::transfer::Artifact {
            url,
            destination,
            size,
            sha,
            stage,
        },
        cancel,
        events,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_inventory_rejects_paths_and_has_server_without_rpc() {
        for files in [artifacts().unwrap(), gpu_artifacts().unwrap()] {
            assert!(
                files
                    .iter()
                    .all(|f| safe_name(&f.name) && f.sha256.len() == 64 && f.bytes > 0)
            );
            assert_eq!(files.iter().filter(|f| f.name.ends_with(".exe")).count(), 1);
            assert!(files.iter().any(|f| f.name == "llama-server.exe"));
            assert!(!files.iter().any(|f| f.name.contains("rpc")));
            let names: std::collections::HashSet<_> = files.iter().map(|f| &f.name).collect();
            assert_eq!(names.len(), files.len());
        }
        assert!(
            gpu_artifacts()
                .unwrap()
                .iter()
                .any(|f| f.name == "ggml-vulkan.dll")
        );
        assert!(
            !artifacts()
                .unwrap()
                .iter()
                .any(|f| f.name.contains("vulkan"))
        );
        for name in [
            "../server.exe",
            "C:server.exe",
            "folder/server.exe",
            "folder\\server.exe",
            ".hidden",
        ] {
            assert!(!safe_name(name));
        }
    }
}
