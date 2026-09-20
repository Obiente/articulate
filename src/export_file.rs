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
    std::fs::write(path, text)?;
    Ok(true)
}

#[cfg(not(windows))]
pub fn save(_: &str, _: &str) -> Result<bool> {
    anyhow::bail!("Use Copy to export this transcript on this platform.")
}
