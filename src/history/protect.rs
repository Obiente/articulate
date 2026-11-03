//! Optional encryption at rest for history records and preferences.
//!
//! Protected files hold a short header followed by a Windows DPAPI blob bound to
//! the current Windows account. Readers always detect the header, so plaintext
//! and protected files can coexist while a migration is interrupted or resumed.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

/// Never valid JSON, so a protected file is never mistaken for plaintext.
pub const MAGIC: &[u8] = b"\0ARTICULATE-DPAPI-1\n";
/// Binds blobs to this purpose; other software calling DPAPI without it cannot
/// decrypt the files by accident.
#[cfg(windows)]
const ENTROPY: &[u8] = b"Articulate local data protection v1";

pub fn is_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Whether the file at `path` starts with the protected header. Unreadable or
/// missing files count as not protected.
pub fn is_sealed_file(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0; MAGIC.len()];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| header == MAGIC)
}

/// The file could be read, but this Windows account cannot decrypt it. It is not
/// damaged and must never be quarantined or replaced because of this error.
#[derive(Debug)]
pub struct Undecryptable;
impl std::fmt::Display for Undecryptable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "This file is encrypted for a different Windows account or could not be decrypted",
        )
    }
}
impl std::error::Error for Undecryptable {}

/// Encrypt for the current Windows account.
pub fn seal(plain: &[u8]) -> Result<Vec<u8>> {
    let blob = dpapi::protect(plain).context("Could not encrypt local data")?;
    let mut sealed = Vec::with_capacity(MAGIC.len() + blob.len());
    sealed.extend_from_slice(MAGIC);
    sealed.extend_from_slice(&blob);
    Ok(sealed)
}

/// Encrypt only when `protect` is set; otherwise return the plaintext.
pub fn seal_if(protect: bool, plain: Vec<u8>) -> Result<Vec<u8>> {
    if protect { seal(&plain) } else { Ok(plain) }
}

/// Return plaintext for either form. Plain bytes pass through unchanged.
pub fn open(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let Some(blob) = bytes.strip_prefix(MAGIC) else {
        return Ok(bytes);
    };
    dpapi::unprotect(blob).map_err(|_| Undecryptable.into())
}

/// Convert stored bytes to the requested form. Returns `None` if already there.
pub fn convert(bytes: Vec<u8>, protect: bool) -> Result<Option<Vec<u8>>> {
    match (is_sealed(&bytes), protect) {
        (true, true) | (false, false) => Ok(None),
        (false, true) => seal(&bytes).map(Some),
        (true, false) => open(bytes).map(Some),
    }
}

/// Replace `destination` with `bytes` without exposing a partial file. The
/// temporary is unique, synced and renamed over the destination.
pub fn write_atomic(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut attempt = 0;
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(
            ".{name}.{}-{}.partial",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 64 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Rewrite one existing file in the requested form. Missing files are ignored.
/// Returns whether the file was changed.
pub fn migrate_file(path: &Path, protect: bool, max_bytes: u64) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Not a regular file"
    );
    if metadata.len() > max_bytes {
        bail!("File exceeds the size limit");
    }
    let bytes = std::fs::read(path)?;
    let Some(converted) = convert(bytes, protect)? else {
        return Ok(false);
    };
    write_atomic(path, &converted)?;
    Ok(true)
}

#[cfg(windows)]
mod dpapi {
    use anyhow::{Result, ensure};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    fn blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len())?,
            pbData: bytes.as_ptr().cast_mut(),
        })
    }

    fn take(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI allocated `cbData` bytes at `pbData` with LocalAlloc.
        unsafe {
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast());
            bytes
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob(plain)?;
        let entropy = blob(super::ENTROPY)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: input buffers outlive the call; output is owned and freed by `take`.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        ensure!(ok != 0, "{}", std::io::Error::last_os_error());
        Ok(take(output))
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = blob(sealed)?;
        let entropy = blob(super::ENTROPY)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: as above.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        ensure!(ok != 0, "{}", std::io::Error::last_os_error());
        Ok(take(output))
    }
}

#[cfg(not(windows))]
mod dpapi {
    use anyhow::{Result, bail};
    pub fn protect(_: &[u8]) -> Result<Vec<u8>> {
        bail!("Encryption at rest requires Windows")
    }
    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
        bail!("Encryption at rest requires Windows")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn sealed_bytes_roundtrip_and_never_contain_plaintext() {
        let plain = br#"{"session":{"text":"synthetic private phrase"}}"#.to_vec();
        let sealed = seal(&plain).unwrap();
        assert!(is_sealed(&sealed));
        assert!(serde_json::from_slice::<serde_json::Value>(&sealed).is_err());
        assert!(
            !sealed
                .windows(b"synthetic private phrase".len())
                .any(|window| window == b"synthetic private phrase")
        );
        assert_eq!(open(sealed.clone()).unwrap(), plain);
        assert_eq!(open(plain.clone()).unwrap(), plain);
        assert!(convert(sealed.clone(), true).unwrap().is_none());
        assert_eq!(convert(sealed, false).unwrap().unwrap(), plain);
    }

    #[test]
    fn tampered_blob_is_reported_as_undecryptable_not_damaged() {
        let mut sealed = seal(b"synthetic").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(open(sealed).unwrap_err().is::<Undecryptable>());
    }

    #[test]
    fn atomic_write_replaces_without_leaving_partials() {
        let directory = std::env::temp_dir().join(format!(
            "articulate-protect-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("record.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(migrate_file(&path, true, 1024).unwrap());
        assert!(is_sealed(&std::fs::read(&path).unwrap()));
        assert!(!migrate_file(&path, true, 1024).unwrap());
        assert!(migrate_file(&path, false, 1024).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
