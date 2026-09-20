//! Explicit, user-triggered relaunch of the installed stable Discord client.
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Version([u32; 3]);

fn version(directory: &str) -> Option<Version> {
    let mut parts = directory.strip_prefix("app-")?.split('.');
    let mut numbers = [0; 3];
    for number in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(Version(numbers))
}

struct Installation {
    executable: PathBuf,
    allowed_executables: Vec<PathBuf>,
}

fn installation(root: &Path) -> Result<Installation> {
    let root = root.canonicalize().context(
        "Discord is not installed in its usual location. Install the desktop app first.",
    )?;
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&root).context("Could not inspect the Discord installation.")? {
        let entry = entry?;
        let Some(version) = version(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let executable = entry.path().join("Discord.exe");
        if !executable.is_file() {
            continue;
        }
        let executable = executable.canonicalize()?;
        // Do not follow an app directory or executable redirected outside this installation.
        if executable.parent().and_then(Path::parent) != Some(root.as_path()) {
            continue;
        }
        if executable.file_name().and_then(|name| name.to_str()) != Some("Discord.exe") {
            continue;
        }
        candidates.push((version, executable));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let executable = candidates
        .last()
        .context("No installed Discord desktop executable was found.")?
        .1
        .clone();
    Ok(Installation {
        executable,
        allowed_executables: candidates.into_iter().map(|(_, path)| path).collect(),
    })
}

/// Ends only stable Discord processes belonging to the discovered installation,
/// then opens that client with a debugger restricted to loopback.
///
/// This is blocking and must run on a worker thread after an explicit UI action.
/// It does not change the installation or make the debugger setting permanent.
#[cfg(windows)]
pub fn relaunch() -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let local = std::env::var_os("LOCALAPPDATA").context("Windows app data is unavailable.")?;
    let installation = installation(&PathBuf::from(local).join("Discord"))?;
    stop_installed_processes(&installation.allowed_executables)?;
    Command::new(&installation.executable)
        .args([
            "--remote-debugging-port=9222",
            "--remote-debugging-address=127.0.0.1",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW; Discord opens its own app window.
        .spawn()
        .context("Discord could not be opened. Try opening it normally and retrying.")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn relaunch() -> Result<()> {
    bail!("Automatic Discord relaunch is currently available on Windows.")
}

// All command text is fixed. Installation paths are passed as JSON through a
// child-only environment variable, never interpolated into PowerShell source.
#[cfg(windows)]
const STOP_INSTALLED: &str = r#"
$ErrorActionPreference = 'Stop'
$allowed = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($path in (ConvertFrom-Json -InputObject $env:ARTICULATE_DISCORD_PATHS)) {
    [void]$allowed.Add([System.IO.Path]::GetFullPath($path))
}
$deadline = [DateTime]::UtcNow.AddSeconds(8)
do {
    $found = $false
    foreach ($process in @(Get-Process -Name Discord -ErrorAction SilentlyContinue)) {
        try {
            # Cache the actual process handle before reading its path. Kill then
            # acts on that process, even if its PID is later reused.
            $null = $process.Handle
            $path = [System.IO.Path]::GetFullPath($process.Path)
            if (-not $allowed.Contains($path)) { continue }
            $found = $true
            $process.Kill()
            if (-not $process.WaitForExit(1000)) {
                throw 'Discord did not finish closing.'
            }
        } catch {
            if (-not $process.HasExited) { throw }
        } finally {
            $process.Dispose()
        }
    }
    if (-not $found) { exit 0 }
} while ([DateTime]::UtcNow -lt $deadline)
throw 'Discord is still running. Quit it from the tray and try again.'
"#;

#[cfg(windows)]
fn stop_installed_processes(paths: &[PathBuf]) -> Result<()> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    // Canonical Windows paths use the extended prefix; Process.Path does not.
    let paths: Vec<_> = paths
        .iter()
        .map(|path| {
            path.to_string_lossy()
                .strip_prefix(r"\\?\")
                .unwrap_or(&path.to_string_lossy())
                .to_owned()
        })
        .collect();
    let system_root =
        std::env::var_os("SystemRoot").context("Windows directory is unavailable.")?;
    let powershell =
        PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut child = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            STOP_INSTALLED,
        ])
        .env("ARTICULATE_DISCORD_PATHS", serde_json::to_string(&paths)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .context("Could not start the Discord relaunch helper.")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Discord could not be closed. Quit it from the tray and try again."
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Discord took too long to close. Quit it from the tray and try again.");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_numeric_and_strict() {
        assert!(version("app-1.0.10000") > version("app-1.0.9999"));
        for invalid in [
            "app-1.0",
            "app-1.0.1.2",
            "app-1.0.-1",
            "app-1.0.1-beta",
            "Discord",
            "app-1.0.+1",
        ] {
            assert_eq!(version(invalid), None);
        }
    }

    #[test]
    fn discovery_selects_latest_and_targets_only_installed_executables() {
        let root = std::env::temp_dir().join(format!(
            "articulate-discord-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for directory in [
            "app-1.0.9999",
            "app-1.0.10000",
            "app-1.0.10001",
            "app-malformed",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
            if directory != "app-1.0.10001" {
                std::fs::write(root.join(directory).join("Discord.exe"), b"fixture").unwrap();
            }
        }
        std::fs::write(root.join("Discord.exe"), b"not an installed app version").unwrap();
        let found = installation(&root).unwrap();
        assert!(
            found
                .executable
                .ends_with(Path::new("app-1.0.10000/Discord.exe"))
        );
        assert_eq!(found.allowed_executables.len(), 2);
        assert!(
            found
                .allowed_executables
                .iter()
                .all(|path| path.file_name().unwrap() == "Discord.exe")
        );
        // This test creates and removes its own bounded temporary fixture only.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn empty_process_allowlist_never_stops_any_process() {
        // Exercises the real fixed helper syntax without authorizing any process.
        stop_installed_processes(&[]).unwrap();
    }
}
