use anyhow::Result;

#[cfg(windows)]
pub fn save(text: &str, extension: &str) -> Result<bool> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::{Controls::Dialogs::*, WindowsAndMessaging::GetForegroundWindow};

    let mut name = vec![0u16; 32768];
    for (to, from) in name
        .iter_mut()
        .zip(format!("Articulate-call.{extension}").encode_utf16())
    {
        *to = from;
    }
    let filter: Vec<u16> = format!(
        "{} files\0*.{}\0All files\0*.*\0\0",
        extension.to_uppercase(),
        extension
    )
    .encode_utf16()
    .collect();
    let suffix: Vec<u16> = extension.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Save call transcript"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = GetForegroundWindow();
        dialog.lpstrFile = name.as_mut_ptr();
        dialog.nMaxFile = name.len() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrDefExt = suffix.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetSaveFileNameW(&mut dialog) == 0 {
            let error = CommDlgExtendedError();
            anyhow::ensure!(error == 0, "Could not open the save dialog ({error})");
            return Ok(false);
        }
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&name[..end]));
    write_atomic(&path, |file| {
        use std::io::Write;
        file.write_all(text.as_bytes())
    })?;
    Ok(true)
}

#[cfg(any(windows, test))]
fn write_atomic(
    destination: &std::path::Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::{
        fs::{File, OpenOptions},
        io,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);
    struct Temporary {
        path: PathBuf,
        file: Option<File>,
        committed: bool,
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            // Windows cannot remove an open file. Only remove the temporary
            // which this operation created, never the existing destination.
            self.file.take();
            if !self.committed {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = None;
    for _ in 0..64 {
        let path = parent.join(format!(
            ".articulate-export-{}-{}.tmp",
            std::process::id(),
            NEXT_EXPORT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some(Temporary {
                    path,
                    file: Some(file),
                    committed: false,
                });
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let mut temporary = temporary.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Could not create an export temporary file",
        )
    })?;
    let file = temporary.file.as_mut().expect("New export owns its file");
    write(file)?;
    file.sync_all()?;
    temporary.file.take();
    // The sibling is on the same filesystem. Rust replaces an existing file
    // with rename on Windows and Unix, without truncating it before success.
    std::fs::rename(&temporary.path, destination)?;
    temporary.committed = true;
    Ok(())
}

#[cfg(not(windows))]
pub fn save(_: &str, _: &str) -> Result<bool> {
    anyhow::bail!("Use Copy to export this transcript on this platform.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    #[test]
    fn failed_partial_export_keeps_existing_file_and_cleans_own_temporary() {
        let parent = std::env::temp_dir();
        let directory = parent.join(format!(
            "articulate-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("notes.txt");
        let unrelated = directory.join("keep.tmp");
        fs::write(&destination, "Previous complete notes").unwrap();
        fs::write(&unrelated, "Unrelated file").unwrap();

        let result = write_atomic(&destination, |file| {
            file.write_all(b"Incomplete replacement")?;
            Err(std::io::Error::other(
                "Simulated disk full after partial write",
            ))
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "Previous complete notes"
        );
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "Unrelated file");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);

        write_atomic(&destination, |file| file.write_all(b"New complete notes")).unwrap();
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "New complete notes"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);

        let blocked = directory.join("existing-directory");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("keep.txt"), "Keep this").unwrap();
        assert!(write_atomic(&blocked, |file| file.write_all(b"Replacement")).is_err());
        assert_eq!(
            fs::read_to_string(blocked.join("keep.txt")).unwrap(),
            "Keep this"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 3);

        assert_eq!(directory.parent(), Some(parent.as_path()));
        fs::remove_dir_all(directory).unwrap();
    }
}
