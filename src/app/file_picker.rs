use std::path::PathBuf;

#[cfg(windows)]
pub(super) fn open(title: &str, extension: &str) -> anyhow::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::{Controls::Dialogs::*, WindowsAndMessaging::GetForegroundWindow};
    let mut name = vec![0u16; 32768];
    let filter: Vec<u16> = format!(
        "{} files\0*.{}\0All files\0*.*\0\0",
        extension.to_uppercase(),
        extension
    )
    .encode_utf16()
    .collect();
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = GetForegroundWindow();
        dialog.lpstrFile = name.as_mut_ptr();
        dialog.nMaxFile = name.len() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut dialog) == 0 {
            let error = CommDlgExtendedError();
            anyhow::ensure!(error == 0, "Could not open the file picker ({error})");
            return Ok(None);
        }
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Ok(Some(std::ffi::OsString::from_wide(&name[..end]).into()))
}

#[cfg(not(windows))]
pub(super) fn open(_: &str, _: &str) -> anyhow::Result<Option<PathBuf>> {
    anyhow::bail!("Enter the path to your file on this platform.")
}
