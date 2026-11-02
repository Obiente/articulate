use anyhow::Result;
use std::path::PathBuf;

/// What is being saved. Selects the dialog title and the suggested file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Document {
    Dictation,
    Call,
    Notes,
    /// An export whose kind the caller does not know.
    Export,
    /// A portable history backup archive.
    Backup,
}

impl Document {
    pub fn from_kind(kind: crate::history::Kind) -> Self {
        match kind {
            crate::history::Kind::Dictation => Self::Dictation,
            crate::history::Kind::Call => Self::Call,
            crate::history::Kind::Note => Self::Notes,
        }
    }

    fn dialog_title(self) -> &'static str {
        match self {
            Self::Dictation => "Save dictation",
            Self::Call => "Save call transcript",
            Self::Notes => "Save notes",
            Self::Export => "Save export",
            Self::Backup => "Save Articulate backup",
        }
    }

    fn fallback_name(self) -> &'static str {
        match self {
            Self::Dictation => "Articulate dictation",
            Self::Call => "Articulate call",
            Self::Notes => "Articulate notes",
            Self::Export => "Articulate export",
            Self::Backup => "Articulate backup",
        }
    }
}

pub const BACKUP_EXTENSION: &str = "articulate-backup";

/// A file name for `title`, safe on Windows: no reserved characters or names,
/// no trailing dots or spaces, and at most 80 characters before the extension.
pub fn suggested_name(document: Document, title: Option<&str>, extension: &str) -> String {
    let mut stem: String = title
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    stem = stem.trim_end_matches(['.', ' ']).to_owned();
    let reserved = {
        let base = stem
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((base.starts_with("COM") || base.starts_with("LPT"))
                && base.len() == 4
                && base.as_bytes()[3].is_ascii_digit())
    };
    if stem.is_empty() || reserved {
        stem = document.fallback_name().to_owned();
    }
    format!("{stem}.{extension}")
}

/// The window that owns this process's dialogs: the foreground window only if
/// it belongs to Articulate, otherwise none, so another app is never the owner.
#[cfg(windows)]
fn own_window() -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::{
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    };
    // SAFETY: plain Win32 queries with a valid out-pointer.
    unsafe {
        let window = GetForegroundWindow();
        let mut process = 0u32;
        if !window.is_null()
            && GetWindowThreadProcessId(window, &mut process) != 0
            && process == GetCurrentProcessId()
        {
            window
        } else {
            std::ptr::null_mut()
        }
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Ask for a file path. `open` selects an existing file; otherwise the user
/// chooses where to save, with an overwrite prompt.
#[cfg(windows)]
fn choose_path(
    document: Document,
    suggested: &str,
    extension: &str,
    open: bool,
    title: &str,
) -> Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;

    let mut name = vec![0u16; 32768];
    if !open {
        for (to, from) in name.iter_mut().zip(suggested.encode_utf16()) {
            *to = from;
        }
    }
    let label = if document == Document::Backup {
        "Articulate backup".to_owned()
    } else {
        format!("{} files", extension.to_uppercase())
    };
    let filter: Vec<u16> = format!("{label}\0*.{extension}\0All files\0*.*\0\0")
        .encode_utf16()
        .collect();
    let suffix = wide(extension);
    let title = wide(title);
    // SAFETY: every buffer outlives the call and is NUL-terminated.
    unsafe {
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = own_window();
        dialog.lpstrFile = name.as_mut_ptr();
        dialog.nMaxFile = name.len() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrDefExt = suffix.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        let accepted = if open {
            dialog.Flags |= OFN_FILEMUSTEXIST;
            GetOpenFileNameW(&mut dialog)
        } else {
            dialog.Flags |= OFN_OVERWRITEPROMPT;
            GetSaveFileNameW(&mut dialog)
        };
        if accepted == 0 {
            let error = CommDlgExtendedError();
            anyhow::ensure!(error == 0, "Could not open the file dialog ({error})");
            return Ok(None);
        }
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &name[..end],
    ))))
}

/// Save with a neutral title and name. Prefer `save_document`.
#[cfg(windows)]
pub fn save(text: &str, extension: &str) -> Result<bool> {
    save_document(text, extension, Document::Export, None)
}

/// Save `text` after asking where, with a title and suggested name matching
/// the document. Returns `false` if the user cancelled.
#[cfg(windows)]
pub fn save_document(
    text: &str,
    extension: &str,
    document: Document,
    title: Option<&str>,
) -> Result<bool> {
    let suggested = suggested_name(document, title, extension);
    let Some(path) = choose_path(
        document,
        &suggested,
        extension,
        false,
        document.dialog_title(),
    )?
    else {
        return Ok(false);
    };
    write_atomic(&path, |file| {
        use std::io::Write;
        file.write_all(text.as_bytes())
    })?;
    Ok(true)
}

/// Ask where to save a file of any type. `None` if cancelled.
#[cfg(windows)]
pub fn choose_save_path(
    document: Document,
    suggested: &str,
    extension: &str,
) -> Result<Option<PathBuf>> {
    choose_path(
        document,
        suggested,
        extension,
        false,
        document.dialog_title(),
    )
}

#[cfg(not(windows))]
pub fn choose_save_path(_: Document, _: &str, _: &str) -> Result<Option<PathBuf>> {
    anyhow::bail!("Saving files is not available on this platform.")
}

#[cfg(not(windows))]
pub fn choose_backup_destination(_: &str) -> Result<Option<PathBuf>> {
    anyhow::bail!("Backups are not available on this platform.")
}

#[cfg(not(windows))]
pub fn choose_backup_source() -> Result<Option<PathBuf>> {
    anyhow::bail!("Backups are not available on this platform.")
}

/// Ask where to save a backup archive. `None` if cancelled.
#[cfg(windows)]
pub fn choose_backup_destination(suggested_title: &str) -> Result<Option<PathBuf>> {
    let suggested = suggested_name(Document::Backup, Some(suggested_title), BACKUP_EXTENSION);
    choose_path(
        Document::Backup,
        &suggested,
        BACKUP_EXTENSION,
        false,
        Document::Backup.dialog_title(),
    )
}

/// Ask which backup archive to restore. `None` if cancelled.
#[cfg(windows)]
pub fn choose_backup_source() -> Result<Option<PathBuf>> {
    choose_path(
        Document::Backup,
        "",
        BACKUP_EXTENSION,
        true,
        "Restore Articulate backup",
    )
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

#[cfg(not(windows))]
pub fn save_document(_: &str, _: &str, _: Document, _: Option<&str>) -> Result<bool> {
    anyhow::bail!("Use Copy to export this transcript on this platform.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    #[test]
    fn suggested_names_follow_the_document_and_are_safe_on_windows() {
        assert_eq!(
            suggested_name(Document::Call, Some("Q3 plan: \"final\"/v2"), "md"),
            "Q3 plan final v2.md"
        );
        assert_eq!(
            suggested_name(Document::Dictation, None, "txt"),
            "Articulate dictation.txt"
        );
        assert_eq!(
            suggested_name(Document::Notes, Some("  ...  "), "md"),
            "Articulate notes.md"
        );
        assert_eq!(
            suggested_name(Document::Notes, Some("con"), "md"),
            "Articulate notes.md"
        );
        assert_eq!(
            suggested_name(Document::Export, Some("Ends with dots..."), "srt"),
            "Ends with dots.srt"
        );
        let long = "é".repeat(200);
        assert_eq!(
            suggested_name(Document::Call, Some(&long), "vtt")
                .chars()
                .count(),
            84
        );
    }

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
