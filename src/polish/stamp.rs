use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path, sync::atomic::AtomicBool};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    size: u64,
    modified: u128,
    identity: (u32, u64),
    sha256: String,
}
#[cfg(windows)]
fn identity(file: &File) -> Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut Information,
        ) -> i32;
    }
    let mut info = Information::default();
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
        "Could not inspect model identity"
    );
    Ok((
        info.volume,
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}
#[cfg(not(windows))]
fn identity(file: &File) -> Result<(u32, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok((m.dev() as u32, m.ino()))
}
fn stamp(file: &File, sha: &str) -> Result<Stamp> {
    let m = file.metadata()?;
    Ok(Stamp {
        size: m.len(),
        modified: m
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        identity: identity(file)?,
        sha256: sha.into(),
    })
}
pub(super) fn verified(path: &Path, size: u64, sha: &str, cancel: &AtomicBool) -> Result<()> {
    super::check_cancel(cancel)?;
    super::install::ordinary(path)?;
    let file = File::open(path)?;
    let before = stamp(&file, sha)?;
    ensure!(before.size == size, "Model size changed");
    let location = path.with_extension("verified.json");
    if std::fs::read(&location)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stamp>(&bytes).ok())
        .as_ref()
        == Some(&before)
    {
        return Ok(());
    }
    super::install::verified(path, size, sha, cancel)?;
    ensure!(
        before == stamp(&File::open(path)?, sha)?,
        "Model changed during verification"
    );
    let temporary = location.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(&before)?)?;
    std::fs::rename(temporary, location)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    #[test]
    fn replacement_invalidates_verified_stamp() {
        let dir = std::env::temp_dir().join(crate::discord::plugin::new_token().unwrap());
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("model");
        std::fs::write(&path, b"verified").unwrap();
        let sha = format!("{:x}", Sha256::digest(b"verified"));
        let cancel = AtomicBool::new(false);
        verified(&path, 8, &sha, &cancel).unwrap();
        verified(&path, 8, &sha, &cancel).unwrap();
        std::fs::write(dir.join("replacement"), b"modified").unwrap();
        std::fs::rename(dir.join("replacement"), &path).unwrap();
        assert!(verified(&path, 8, &sha, &cancel).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
