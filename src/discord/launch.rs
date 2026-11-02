//! Explicit, user-triggered relaunch of an installed Discord desktop client.
//!
//! Articulate launches Discord itself, so `--remote-debugging-pipe` was
//! evaluated first. Electron does honor it on Windows: Chromium reads CDP from
//! C-runtime file descriptor 3 and writes to descriptor 4, which a parent can
//! supply through the undocumented `STARTUPINFO.lpReserved2` handle block that
//! Node and Puppeteer also populate. Two consequences ruled it out here:
//! Electron quits the whole application when that pipe disconnects, so closing
//! or crashing Articulate would end the user's Discord call, and the pipe speaks
//! the browser-level flattened session protocol rather than the page endpoint
//! this observer uses. The debugger port therefore stays, but it is random for
//! every launch, and a normal relaunch is offered to close it again.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Installed Discord editions. Stable is preferred when several are present.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    Stable,
    Ptb,
    Canary,
}

impl Flavor {
    pub const ALL: [Flavor; 3] = [Flavor::Stable, Flavor::Ptb, Flavor::Canary];

    pub fn id(self) -> &'static str {
        match self {
            Flavor::Stable => "stable",
            Flavor::Ptb => "ptb",
            Flavor::Canary => "canary",
        }
    }
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|flavor| flavor.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Flavor::Stable => "Discord",
            Flavor::Ptb => "Discord PTB",
            Flavor::Canary => "Discord Canary",
        }
    }
    /// Folder beneath `%LOCALAPPDATA%` and the process name without extension.
    pub fn directory(self) -> &'static str {
        match self {
            Flavor::Stable => "Discord",
            Flavor::Ptb => "DiscordPTB",
            Flavor::Canary => "DiscordCanary",
        }
    }
    fn executable(self) -> String {
        format!("{}.exe", self.directory())
    }
    /// Renderer origins reported by the debugger target list.
    pub fn renderer_hosts(self) -> [&'static str; 2] {
        match self {
            Flavor::Stable => ["discord.com", "discord.com:443"],
            Flavor::Ptb => ["ptb.discord.com", "ptb.discord.com:443"],
            Flavor::Canary => ["canary.discord.com", "canary.discord.com:443"],
        }
    }
    pub fn for_renderer_host(host: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|flavor| flavor.renderer_hosts().contains(&host))
    }
}

/// Result of a debugger relaunch: which edition opened and its loopback port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Launched {
    pub flavor: Flavor,
    pub port: u16,
}

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

fn installation(root: &Path, flavor: Flavor) -> Result<Installation> {
    let name = flavor.executable();
    let root = root.canonicalize().with_context(|| {
        format!(
            "{} is not installed in its usual location. Install the desktop app first.",
            flavor.label()
        )
    })?;
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&root).context("Could not inspect the Discord installation.")? {
        let entry = entry?;
        let Some(version) = version(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let executable = entry.path().join(&name);
        if !executable.is_file() {
            continue;
        }
        let executable = executable.canonicalize()?;
        // Do not follow an app directory or executable redirected outside this installation.
        if executable.parent().and_then(Path::parent) != Some(root.as_path()) {
            continue;
        }
        if executable.file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
            continue;
        }
        candidates.push((version, executable));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let executable = candidates
        .last()
        .with_context(|| {
            format!(
                "No installed {} desktop executable was found.",
                flavor.label()
            )
        })?
        .1
        .clone();
    Ok(Installation {
        executable,
        allowed_executables: candidates.into_iter().map(|(_, path)| path).collect(),
    })
}

fn local_app_data() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("Windows app data is unavailable.")
}

/// Editions with at least one installed executable, in preference order.
pub fn installed() -> Vec<Flavor> {
    let Ok(local) = local_app_data() else {
        return Vec::new();
    };
    installed_beneath(&local)
}

fn installed_beneath(local: &Path) -> Vec<Flavor> {
    Flavor::ALL
        .into_iter()
        .filter(|flavor| installation(&local.join(flavor.directory()), *flavor).is_ok())
        .collect()
}

/// Chooses the edition to open. An explicit request must be installed. Without
/// one, stable wins, then a single installed edition; two test editions without
/// stable need an explicit choice.
pub fn choose(requested: Option<Flavor>, installed: &[Flavor]) -> Result<Flavor> {
    if let Some(flavor) = requested {
        ensure!(
            installed.contains(&flavor),
            "{} is not installed. Choose an installed Discord edition.",
            flavor.label()
        );
        return Ok(flavor);
    }
    match installed {
        [] => {
            bail!("Discord is not installed in its usual location. Install the desktop app first.")
        }
        [only] => Ok(*only),
        many if many.contains(&Flavor::Stable) => Ok(Flavor::Stable),
        _ => bail!(
            "Several Discord editions are installed. Choose Discord PTB or Discord Canary in Settings before relaunching."
        ),
    }
}

/// A free loopback port outside the well-known range, chosen at random for each
/// launch. A local process that knows the previous port cannot rely on it again.
fn random_free_port() -> Result<u16> {
    use std::net::TcpListener;
    for _ in 0..32 {
        let mut bytes = [0_u8; 2];
        super::plugin::random_bytes(&mut bytes)?;
        let port = 49152 + (u16::from_le_bytes(bytes) % (65535 - 49152));
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Ends only processes belonging to the chosen installed edition, then opens
/// that client with a debugger restricted to loopback on a random port.
///
/// This is blocking and must run on a worker thread after an explicit UI action.
/// It does not change the installation or make the debugger setting permanent.
#[cfg(windows)]
pub fn relaunch(requested: Option<Flavor>) -> Result<Launched> {
    let flavor = choose(requested, &installed())?;
    let port = random_free_port()?;
    open(
        flavor,
        &[
            format!("--remote-debugging-port={port}"),
            "--remote-debugging-address=127.0.0.1".into(),
        ],
    )?;
    Ok(Launched { flavor, port })
}

/// Closes the chosen edition and opens it again without any debugger access.
#[cfg(windows)]
pub fn relaunch_normally(flavor: Flavor) -> Result<()> {
    open(flavor, &[])
}

#[cfg(windows)]
fn open(flavor: Flavor, arguments: &[String]) -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let installation = installation(&local_app_data()?.join(flavor.directory()), flavor)?;
    stop_installed_processes(flavor, &installation.allowed_executables)?;
    Command::new(&installation.executable)
        .args(arguments)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW; Discord opens its own app window.
        .spawn()
        .with_context(|| {
            format!(
                "{} could not be opened. Try opening it normally and retrying.",
                flavor.label()
            )
        })?;
    Ok(())
}

#[cfg(not(windows))]
pub fn relaunch(_requested: Option<Flavor>) -> Result<Launched> {
    bail!("Automatic Discord relaunch is currently available on Windows.")
}

#[cfg(not(windows))]
pub fn relaunch_normally(_flavor: Flavor) -> Result<()> {
    bail!("Automatic Discord relaunch is currently available on Windows.")
}

// All command text is fixed. Installation paths and the process name are passed
// through child-only environment variables, never interpolated into PowerShell.
#[cfg(windows)]
const STOP_INSTALLED: &str = r#"
$ErrorActionPreference = 'Stop'
$allowed = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($path in (ConvertFrom-Json -InputObject $env:ARTICULATE_DISCORD_PATHS)) {
    [void]$allowed.Add([System.IO.Path]::GetFullPath($path))
}
$name = $env:ARTICULATE_DISCORD_PROCESS
if ($name -notmatch '^[A-Za-z]+$') { throw 'Invalid process name.' }
$deadline = [DateTime]::UtcNow.AddSeconds(8)
do {
    $found = $false
    foreach ($process in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
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
fn stop_installed_processes(flavor: Flavor, paths: &[PathBuf]) -> Result<()> {
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
        .env("ARTICULATE_DISCORD_PROCESS", flavor.directory())
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

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "articulate-discord-launch-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

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
        let root = fixture("stable");
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
        let found = installation(&root, Flavor::Stable).unwrap();
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
        // A stable layout never satisfies a test edition and vice versa.
        assert!(installation(&root, Flavor::Canary).is_err());
        // This test creates and removes its own bounded temporary fixture only.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editions_use_their_own_folders_executables_and_renderer_hosts() {
        let local = fixture("editions");
        for (flavor, present) in [(Flavor::Ptb, true), (Flavor::Canary, true)] {
            let app = local.join(flavor.directory()).join("app-1.0.50");
            std::fs::create_dir_all(&app).unwrap();
            if present {
                std::fs::write(app.join(flavor.executable()), b"fixture").unwrap();
            }
        }
        // A stable folder with a PTB executable inside is not a stable installation.
        let wrong = local.join("Discord").join("app-1.0.1");
        std::fs::create_dir_all(&wrong).unwrap();
        std::fs::write(wrong.join("DiscordPTB.exe"), b"fixture").unwrap();
        assert_eq!(installed_beneath(&local), vec![Flavor::Ptb, Flavor::Canary]);
        assert_eq!(
            Flavor::for_renderer_host("canary.discord.com"),
            Some(Flavor::Canary)
        );
        assert_eq!(
            Flavor::for_renderer_host("ptb.discord.com:443"),
            Some(Flavor::Ptb)
        );
        assert_eq!(
            Flavor::for_renderer_host("discord.com"),
            Some(Flavor::Stable)
        );
        assert_eq!(Flavor::for_renderer_host("discord.com.example"), None);
        assert_eq!(Flavor::parse("ptb"), Some(Flavor::Ptb));
        assert_eq!(Flavor::parse("beta"), None);
        std::fs::remove_dir_all(local).unwrap();
    }

    #[test]
    fn choice_prefers_stable_and_demands_an_explicit_choice_between_test_editions() {
        assert!(choose(None, &[]).is_err());
        assert_eq!(choose(None, &[Flavor::Canary]).unwrap(), Flavor::Canary);
        assert_eq!(
            choose(None, &[Flavor::Stable, Flavor::Canary]).unwrap(),
            Flavor::Stable
        );
        let ambiguous = choose(None, &[Flavor::Ptb, Flavor::Canary]).unwrap_err();
        assert!(ambiguous.to_string().contains("Choose"));
        assert_eq!(
            choose(Some(Flavor::Ptb), &[Flavor::Ptb, Flavor::Canary]).unwrap(),
            Flavor::Ptb
        );
        let missing = choose(Some(Flavor::Canary), &[Flavor::Stable]).unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("Discord Canary is not installed")
        );
    }

    #[test]
    fn debugger_ports_are_random_free_and_outside_the_well_known_range() {
        let first = random_free_port().unwrap();
        let second = random_free_port().unwrap();
        assert!(first >= 49152 && second >= 49152);
        assert_ne!(first, 9222);
        // Two draws almost never collide; a collision here would be a fixed port.
        assert!(first != second || random_free_port().unwrap() != first);
        assert!(std::net::TcpListener::bind(("127.0.0.1", first)).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn empty_process_allowlist_never_stops_any_process() {
        // Exercises the real fixed helper syntax without authorizing any process.
        for flavor in Flavor::ALL {
            stop_installed_processes(flavor, &[]).unwrap();
        }
    }
}
