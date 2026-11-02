use std::path::Path;
pub(crate) trait RunKey {
    fn set(&self, command: Option<&str>) -> Result<(), String>;
}
pub(crate) fn configure(key: &impl RunKey, enabled: bool, executable: &Path) -> Result<(), String> {
    let path = executable
        .to_str()
        .ok_or("The executable path is not valid text")?;
    if path.contains(['"', '\n', '\r']) {
        return Err("Invalid executable path".into());
    }
    let command = format!("\"{path}\" --login");
    key.set(enabled.then_some(command.as_str()))
}
pub(crate) struct WindowsRunKey;
impl RunKey for WindowsRunKey {
    fn set(&self, command: Option<&str>) -> Result<(), String> {
        #[cfg(all(windows, not(test)))]
        {
            use std::os::windows::process::CommandExt;
            let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
            let mut process = std::process::Command::new("reg.exe");
            if let Some(command) = command {
                process.args([
                    "add",
                    key,
                    "/v",
                    "Articulate",
                    "/t",
                    "REG_SZ",
                    "/d",
                    command,
                    "/f",
                ]);
            } else {
                let exists = std::process::Command::new("reg.exe")
                    .args(["query", key, "/v", "Articulate"])
                    .creation_flags(0x08000000)
                    .output()
                    .map_err(|e| e.to_string())?;
                if !exists.status.success() {
                    return Ok(());
                }
                process.args(["delete", key, "/v", "Articulate", "/f"]);
            }
            let output = process
                .creation_flags(0x08000000)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err("Could not update Windows startup registration".into());
            }
            Ok(())
        }
        #[cfg(any(not(windows), test))]
        {
            let _ = command;
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(std::cell::RefCell<Option<String>>);
    impl RunKey for Fake {
        fn set(&self, command: Option<&str>) -> Result<(), String> {
            *self.0.borrow_mut() = command.map(str::to_owned);
            Ok(())
        }
    }
    #[test]
    fn quotes_executable_and_removes_only_own_value() {
        let key = Fake(Default::default());
        let executable = Path::new("C:/Program Files/Articulate/articulate.exe");
        configure(&key, true, executable).unwrap();
        assert_eq!(
            key.0.borrow().as_deref(),
            Some("\"C:/Program Files/Articulate/articulate.exe\" --login")
        );
        configure(&key, false, executable).unwrap();
        assert!(key.0.borrow().is_none());
    }
}
