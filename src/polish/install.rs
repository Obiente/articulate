use super::{Event, check_cancel, progress};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

pub const MODEL_NAME: &str = "Qwen3.5-0.8B-Q8_0.gguf";
pub const MODEL_BYTES: u64 = 833_592_096;
const MODEL_SHA: &str = "37ae482d336108d23516fa35e8e0c4126688d81018b87178a18d752a1357814f";
const MODEL_URL: &str = "https://huggingface.co/ggml-org/Qwen3.5-0.8B-GGUF/resolve/8fea620810c4afa23dd6443f999a48574c1611a3/Qwen3.5-0.8B-Q8_0.gguf";
const RUNTIME_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10964/llama-b10964-bin-win-cpu-x64.zip";
const RUNTIME_SHA: &str = "917f39c076402c421224824607397af20f53625a60defc20e8dd22446bf4c5d7";
const RUNTIME_BYTES: u64 = 18_427_629;
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
pub(super) fn root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("ARTICULATE_POLISH_TEST_DIR") {
        return PathBuf::from(root);
    }
    crate::model::data_dir()
        .join("polish")
        .join("qwen35-08b-q8-llama-b10964")
}
pub(super) fn model() -> PathBuf {
    root().join(MODEL_NAME)
}
pub(super) fn executable() -> PathBuf {
    root().join("runtime").join("llama-server.exe")
}

pub fn installed() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
        && model().metadata().is_ok_and(|m| m.len() == MODEL_BYTES)
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

fn ordinary(path: &Path) -> Result<()> {
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
fn verified(path: &Path, size: u64, expected: &str, cancel: &AtomicBool) -> Result<()> {
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
pub(super) fn verify(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "The managed local editor currently requires Windows x64."
    );
    ensure!(installed(), "Download the local editor in Settings first.");
    verified(&model(), MODEL_BYTES, MODEL_SHA, cancel)?;
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
pub(super) fn download(cancel: &AtomicBool, events: &Sender<Event>) -> Result<()> {
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
    let model = model();
    if verified(&model, MODEL_BYTES, MODEL_SHA, cancel).is_err() {
        fetch(
            MODEL_URL,
            &model,
            MODEL_BYTES,
            MODEL_SHA,
            "Downloading editing model",
            cancel,
            events,
        )?;
    }
    let archive = root().join("runtime.zip");
    if verified(&archive, RUNTIME_BYTES, RUNTIME_SHA, cancel).is_err() {
        fetch(
            RUNTIME_URL,
            &archive,
            RUNTIME_BYTES,
            RUNTIME_SHA,
            "Downloading local editing tools",
            cancel,
            events,
        )?;
    }
    progress(events, "Preparing local editing tools", None);
    let stage = root().join(format!("stage-{}", crate::discord::plugin::new_token()?));
    fs::create_dir(&stage)?;
    let result = (|| -> Result<()> {
        let mut zip = zip::ZipArchive::new(File::open(&archive)?)?;
        let expected = artifacts()?;
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
        check_cancel(cancel)?;
        let runtime = root().join("runtime");
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
        fs::write(root().join("ready"), RUNTIME_SHA)?;
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
    cancel: &AtomicBool,
    events: &Sender<Event>,
) -> Result<()> {
    check_cancel(cancel)?;
    let part = destination.with_extension(format!("part-{}", crate::discord::plugin::new_token()?));
    let result = (|| -> Result<()> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(1800)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_body(Some(Duration::from_secs(5)))
            .build()
            .into();
        // These fixed public requests carry no dictation, vocabulary or user ID.
        let mut response = agent
            .get(url)
            .call()
            .context("Could not download the local editor. Check your connection and retry.")?;
        let mut reader = response.body_mut().as_reader();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        let mut buffer = vec![0; 256 * 1024];
        let mut received = 0u64;
        let mut last = 0u64;
        loop {
            check_cancel(cancel)?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            ensure!(
                received <= size,
                "The editor download exceeded its expected size."
            );
            file.write_all(&buffer[..count])?;
            if received - last >= 1024 * 1024 || received == size {
                progress(events, stage, Some(received as f32 / size as f32));
                last = received;
            }
        }
        file.sync_all()?;
        drop(file);
        verified(&part, size, sha, cancel)?;
        // Windows rename cannot replace an existing file. A broken model is
        // preserved for diagnosis instead of deleting a potentially open file.
        if destination.exists() {
            let backup = destination
                .with_extension(format!("replaced-{}", crate::discord::plugin::new_token()?));
            fs::rename(destination, &backup)?;
            if let Err(error) = fs::rename(&part, destination) {
                let _ = fs::rename(&backup, destination);
                return Err(error.into());
            }
            let _ = fs::remove_file(backup);
        } else {
            fs::rename(&part, destination)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_inventory_rejects_paths_and_has_server_without_rpc() {
        let files = artifacts().unwrap();
        assert!(
            files
                .iter()
                .all(|f| safe_name(&f.name) && f.sha256.len() == 64 && f.bytes > 0)
        );
        assert_eq!(files.iter().filter(|f| f.name.ends_with(".exe")).count(), 1);
        assert!(files.iter().any(|f| f.name == "llama-server.exe"));
        assert!(!files.iter().any(|f| f.name.contains("rpc")));
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
