//! The companion pairing file.
//!
//! The Vencord companion runs under the same Windows account as Articulate and
//! needs the pairing key to authenticate its loopback connection. It used to
//! read the key out of `settings.json`, which becomes a DPAPI envelope once
//! encryption at rest is on. The key is therefore published in a small
//! dedicated file that mirrors the current key: written whenever the key
//! changes, removed when the pairing is forgotten, and kept in step at startup.
//!
//! The file is plaintext on purpose. The key authorizes nothing but the
//! loopback connection between two processes of the same account; user-scope
//! DPAPI cannot keep it from a process running as that account, and the
//! companion has no PowerShell-free way to call DPAPI without a native module.
//! Encryption at rest protects transcripts, which the key never unlocks. The
//! rationale is spelled out in SECURITY.md.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// File name inside Articulate's data directory.
pub const FILE_NAME: &str = "discord-pairing.json";
const SCHEMA: u32 = 1;
/// Far larger than a real file; bounds a damaged or foreign one.
const SIZE_LIMIT: u64 = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairing {
    version: u32,
    key: String,
}

/// The key the file currently publishes, if it exists and is well formed.
pub fn read(directory: &Path) -> Result<Option<String>> {
    let path = directory.join(FILE_NAME);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "The pairing file is not a regular file"
    );
    ensure!(
        metadata.len() <= SIZE_LIMIT,
        "The pairing file is too large"
    );
    let pairing: Pairing =
        serde_json::from_slice(&std::fs::read(&path)?).context("The pairing file is damaged")?;
    ensure!(
        pairing.version == SCHEMA && super::plugin::valid_token(&pairing.key),
        "The pairing file holds no usable key"
    );
    Ok(Some(pairing.key.to_ascii_lowercase()))
}

/// Makes the file publish exactly `key`, removing it when the key is empty.
/// Returns whether anything on disk changed.
pub fn sync(directory: &Path, key: &str) -> Result<bool> {
    let path = directory.join(FILE_NAME);
    if !super::plugin::valid_token(key) {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("Could not remove the companion pairing file"),
        };
    }
    let key = key.to_ascii_lowercase();
    if read(directory).ok().flatten().as_deref() == Some(key.as_str()) {
        return Ok(false);
    }
    std::fs::create_dir_all(directory)?;
    let bytes = serde_json::to_vec_pretty(&Pairing {
        version: SCHEMA,
        key,
    })?;
    crate::history::protect::write_atomic(&path, &bytes)
        .context("Could not write the companion pairing file")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "articulate-pairing-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn pairing_file_mirrors_the_key_and_is_removed_with_it() {
        let directory = directory("mirror");
        assert_eq!(read(&directory).unwrap(), None);
        assert!(!sync(&directory, "").unwrap(), "nothing to remove");
        let key = "A".repeat(64);
        assert!(sync(&directory, &key).unwrap());
        assert_eq!(
            read(&directory).unwrap().as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert!(
            !sync(&directory, &key).unwrap(),
            "an unchanged key is not rewritten"
        );
        let bytes = std::fs::read(directory.join(FILE_NAME)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["key"], "a".repeat(64));
        assert_eq!(
            value.as_object().unwrap().len(),
            2,
            "only the key is published"
        );
        let rotated = "b".repeat(64);
        assert!(sync(&directory, &rotated).unwrap());
        assert_eq!(read(&directory).unwrap().as_deref(), Some(rotated.as_str()));
        assert!(sync(&directory, "").unwrap());
        assert!(!directory.join(FILE_NAME).exists());
        assert_eq!(read(&directory).unwrap(), None);
        assert_eq!(
            std::fs::read_dir(&directory).unwrap().count(),
            0,
            "no partial files remain"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn damaged_or_foreign_pairing_files_are_reported_and_replaced() {
        let directory = directory("damaged");
        let path = directory.join(FILE_NAME);
        for bytes in [
            &b"garbage"[..],
            br#"{"version":2,"key":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
            br#"{"version":1,"key":"short"}"#,
            br#"{"version":1,"key":"0000000000000000000000000000000000000000000000000000000000000000","extra":1}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read(&directory).is_err(), "{}", String::from_utf8_lossy(bytes));
            assert!(sync(&directory, &"c".repeat(64)).unwrap());
            assert_eq!(read(&directory).unwrap().as_deref(), Some("c".repeat(64).as_str()));
        }
        std::fs::write(&path, vec![b' '; SIZE_LIMIT as usize + 1]).unwrap();
        assert!(
            read(&directory)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
