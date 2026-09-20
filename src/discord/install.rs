//! Explicit companion deployment into a verified Vencord source checkout.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SOURCE_REVISION: &str = "59a54286542651fff5ea53f0ce6cadf2a6aa7521";
const SOURCE_URL: &str =
    "https://codeload.github.com/Vendicated/Vencord/zip/59a54286542651fff5ea53f0ce6cadf2a6aa7521";
const SOURCE_SHA256: &str = "8b58c3eef5c93ecd94b8950a5ef47001e4da3c22e90175b02eb736141b3ece57";
const INSTALLER_URL: &str =
    "https://github.com/Vencord/Installer/releases/download/v1.4.2/VencordInstaller.exe";
const INSTALLER_SHA256: &str = "1cb4115a99a8b7a0b0371dddb2ac7dde38b9acc23f53a69540319b4b7ca6c2da";
#[cfg(windows)]
const NODE_FOLDER: &str = "node-v22.23.2-win-x64";
#[cfg(windows)]
const NODE_URL: &str = "https://nodejs.org/dist/v22.23.2/node-v22.23.2-win-x64.zip";
#[cfg(windows)]
const NODE_SHA256: &str = "1177b4137ba5adaa56354ae40f1080c7450e8ae09cecb47da459d1c52ac99f97";
const MANIFEST: &str = ".articulate-companion.json";
const NATIVE_FILES: [(&str, &[u8]); 5] = [
    (
        "articulate_discord_audio.node",
        include_bytes!(concat!(env!("OUT_DIR"), "/articulate_discord_audio.node")),
    ),
    (
        "articulate-audio-preload.cjs",
        include_bytes!("../../plugins/discord-native/preload.cjs"),
    ),
    (
        "licenses/articulate-native/MinHook.txt",
        include_bytes!(concat!(env!("OUT_DIR"), "/MinHook.txt")),
    ),
    (
        "licenses/articulate-native/Node-API-Headers.txt",
        include_bytes!(concat!(env!("OUT_DIR"), "/Node-API-Headers.txt")),
    ),
    (
        "licenses/articulate-native/Articulate-AGPL.txt",
        include_bytes!("../../LICENSE"),
    ),
];
const CORE_ARTIFACTS: [&str; 4] = ["patcher.js", "preload.js", "renderer.js", "renderer.css"];

pub fn native_payload_available() -> bool {
    cfg!(windows) && NATIVE_FILES.iter().all(|(_, bytes)| !bytes.is_empty())
}

pub fn native_audio_enabled(source: &Path) -> bool {
    configured_native_audio(source).unwrap_or(false)
}

fn configured_native_audio(source: &Path) -> Result<bool> {
    let path = source.join("dist/.articulate-build.json");
    if !path.exists() {
        return Ok(false);
    }
    ordinary(&path)?;
    let manifest: Manifest = serde_json::from_slice(&bounded_read(&path, 16384)?)?;
    Ok(manifest.files.contains_key(NATIVE_FILES[0].0))
}

fn current_native_payload(source: &Path) -> Result<bool> {
    if !source.join("dist/.articulate-build.json").exists() {
        return Ok(false);
    }
    let manifest: Manifest = serde_json::from_slice(&bounded_read(
        &source.join("dist/.articulate-build.json"),
        16384,
    )?)?;
    Ok(native_payload_available()
        && NATIVE_FILES
            .iter()
            .all(|(name, bytes)| manifest.files.get(*name) == Some(&hash(bytes))))
}
const FILES: [(&str, &[u8]); 3] = [
    (
        "index.ts",
        include_bytes!("../../plugins/vencord/articulate/index.ts"),
    ),
    (
        "native.ts",
        include_bytes!("../../plugins/vencord/articulate/native.ts"),
    ),
    (
        "LICENSE",
        include_bytes!("../../plugins/vencord/articulate/LICENSE"),
    ),
];

#[derive(Clone, Debug, Default)]
pub struct Detection {
    pub installed: bool,
    pub source: Option<PathBuf>,
    /// A current Discord installation's injector points at this source's dist.
    pub selected_active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginStatus {
    Missing,
    Current,
    UpdateAvailable,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub source: PathBuf,
    pub status: PluginStatus,
    pub can_build: bool,
}

#[derive(Debug)]
pub struct InstallReport {
    pub changed: bool,
    pub built: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u8,
    revision: String,
    files: BTreeMap<String, String>,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn bundled_manifest() -> Manifest {
    let files: BTreeMap<_, _> = FILES
        .iter()
        .map(|(name, bytes)| ((*name).into(), hash(bytes)))
        .collect();
    Manifest {
        schema: 1,
        revision: hash(&serde_json::to_vec(&files).unwrap()),
        files,
    }
}

fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "File is larger than expected");
    Ok(bytes)
}

fn ordinary(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "Linked source paths are not supported"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Redirected source paths are not supported"
        );
    }
    Ok(())
}

fn source_root(source: &Path) -> Result<PathBuf> {
    ordinary(source).context("Choose an existing Vencord source folder")?;
    let source = source.canonicalize()?;
    for relative in [
        "package.json",
        "pnpm-lock.yaml",
        "src",
        "src/plugins",
        "scripts",
        "scripts/build",
        "scripts/build/build.mjs",
    ] {
        ordinary(&source.join(relative))
            .context("This is not a complete Vencord source checkout")?;
    }
    let package: serde_json::Value =
        serde_json::from_slice(&bounded_read(&source.join("package.json"), 128 * 1024)?)?;
    ensure!(
        package["name"] == "vencord",
        "Choose the Vencord source folder, not its installed dist folder"
    );
    Ok(source)
}

fn managed_source() -> PathBuf {
    crate::model::data_dir().join("vencord-source")
}

pub fn detect(source_hint: Option<&Path>) -> Detection {
    let targets = installed_targets();
    let installed = !targets.is_empty()
        || std::env::var_os("APPDATA").is_some_and(|path| {
            PathBuf::from(path)
                .join("Vencord/dist/patcher.js")
                .is_file()
        });
    let source = source_hint
        .and_then(|path| source_root(path).ok())
        .or_else(|| {
            targets
                .iter()
                .filter_map(|path| path.parent()?.parent())
                .find_map(|path| source_root(path).ok())
        })
        .or_else(|| {
            std::env::var_os("VENCORD_USER_DATA_DIR")
                .and_then(|path| source_root(Path::new(&path)).ok())
        })
        .or_else(|| source_root(&managed_source()).ok());
    let selected_active = source.as_ref().is_some_and(|source| {
        let expected = source.join("dist/patcher.js");
        targets.iter().any(|target| same_path(target, &expected))
    });
    Detection {
        installed,
        source,
        selected_active,
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => {
            #[cfg(windows)]
            {
                left.to_string_lossy()
                    .eq_ignore_ascii_case(&right.to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                left == right
            }
        }
        _ => false,
    }
}

fn installed_targets() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for name in [
        "Discord",
        "DiscordPTB",
        "DiscordCanary",
        "DiscordDevelopment",
    ] {
        let root = PathBuf::from(&local).join(name);
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let latest = entries
            .filter_map(Result::ok)
            .take(100)
            .filter_map(|entry| {
                let name = entry.file_name();
                let version = semver::Version::parse(name.to_str()?.strip_prefix("app-")?).ok()?;
                Some((version, entry.path()))
            })
            .max_by(|a, b| a.0.cmp(&b.0));
        if let Some((_, directory)) = latest
            && let Ok(bytes) = bounded_read(&directory.join("resources/app.asar"), 64 * 1024)
            && let Some(target) = injector_target(&bytes)
        {
            targets.push(target);
        }
    }
    targets
}

// Read only the tiny official injector's index.js. Never load or execute ASAR code.
fn injector_target(bytes: &[u8]) -> Option<PathBuf> {
    let word =
        |offset| Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?) as usize);
    if word(0)? != 4 {
        return None;
    }
    let data_start = 8_usize.checked_add(word(4)?)?;
    let header_length = word(12)?;
    let header: serde_json::Value =
        serde_json::from_slice(bytes.get(16..16_usize.checked_add(header_length)?)?).ok()?;
    let entry = &header["files"]["index.js"];
    let offset: usize = entry["offset"].as_str()?.parse().ok()?;
    let size = usize::try_from(entry["size"].as_u64()?).ok()?;
    let start = data_start.checked_add(offset)?;
    let code = std::str::from_utf8(bytes.get(start..start.checked_add(size)?)?)
        .ok()?
        .trim();
    let argument = code.strip_prefix("require(")?.strip_suffix(')')?;
    let path: String = serde_json::from_str(argument).ok()?;
    let path = PathBuf::from(path);
    (path.is_absolute() && path.file_name()?.to_str()? == "patcher.js").then_some(path)
}

pub fn plan(source: &Path) -> Result<Plan> {
    let source = source_root(source)?;
    let plugins = source.join("src/userplugins");
    if plugins.exists() {
        ordinary(&plugins)?;
    }
    let plugin = plugins.join("articulate");
    let expected = bundled_manifest();
    let status = if !plugin.exists() {
        PluginStatus::Missing
    } else {
        ordinary(&plugin)?;
        let entries: Vec<_> = fs::read_dir(&plugin)?.collect::<std::io::Result<_>>()?;
        for entry in &entries {
            ordinary(&entry.path())?;
            ensure!(
                entry.file_type()?.is_file()
                    && (entry.file_name() == MANIFEST
                        || FILES.iter().any(|(name, _)| entry.file_name() == *name)),
                "The Articulate plugin folder contains unrecognized files. Keep a backup and choose a clean folder; nothing was changed."
            );
        }
        let current: BTreeMap<_, _> = FILES
            .iter()
            .map(|(name, _)| {
                Ok((
                    (*name).to_owned(),
                    hash(&bounded_read(&plugin.join(name), 256 * 1024)?),
                ))
            })
            .collect::<Result<_>>()
            .context("Existing Articulate plugin files are incomplete; nothing was changed")?;
        if plugin.join(MANIFEST).exists() {
            let manifest: Manifest =
                serde_json::from_slice(&bounded_read(&plugin.join(MANIFEST), 8192)?)?;
            ensure!(
                manifest.schema == 1 && manifest.files.keys().eq(expected.files.keys()),
                "Unknown companion ownership manifest"
            );
            ensure!(
                current == manifest.files || current == expected.files,
                "The Articulate plugin was edited outside the app. Back up your changes before updating; nothing was changed."
            );
        } else {
            ensure!(
                current == expected.files,
                "An unrecognized Articulate plugin already exists. It will not be overwritten."
            );
        }
        let needs_native = configured_native_audio(&source)?
            || (native_payload_available() && source.join("dist").exists());
        if current == expected.files && (!needs_native || current_native_payload(&source)?) {
            PluginStatus::Current
        } else {
            PluginStatus::UpdateAvailable
        }
    };
    Ok(Plan {
        can_build: cfg!(windows) || node_binary().is_some(),
        source,
        status,
    })
}

pub fn install(request: &Plan, rebuild: bool, progress: impl Fn(&str)) -> Result<InstallReport> {
    let root = source_root(&request.source)?;
    let scratch = root.join(".articulate-companion");
    fs::create_dir_all(&scratch)?;
    ordinary(&scratch)?;
    let mut lock_options = fs::OpenOptions::new();
    lock_options.write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        lock_options.share_mode(0);
    }
    let _lock = lock_options
        .open(scratch.join("operation.lock"))
        .context("Another companion installation is using this source folder")?;
    let native = native_payload_available() || configured_native_audio(&root)?;
    ensure!(
        !native || native_payload_available(),
        "This app build does not contain the separate-audio adapter. Use a packaged build."
    );
    let checked = plan(&request.source)?;
    ensure!(
        checked.status == request.status,
        "The plugin changed since it was checked. Check again before installing."
    );
    let changed = checked.status != PluginStatus::Current;
    let mut source_backup = None;
    if changed {
        progress("Installing the Articulate companion source…");
        let scratch = checked.source.join(".articulate-companion");
        fs::create_dir_all(&scratch)?;
        ordinary(&scratch)?;
        let identity = super::plugin::new_token()?;
        let stage = scratch.join(format!("staging-{identity}"));
        fs::create_dir(&stage)?;
        for (name, bytes) in FILES {
            write_new(&stage.join(name), bytes)?;
        }
        write_new(
            &stage.join(MANIFEST),
            &serde_json::to_vec_pretty(&bundled_manifest())?,
        )?;
        let parent = checked.source.join("src/userplugins");
        fs::create_dir_all(&parent)?;
        ordinary(&parent)?;
        let target = parent.join("articulate");
        let backup = scratch.join(format!("backup-{identity}"));
        let had_previous = target.exists();
        if had_previous {
            fs::rename(&target, &backup)?;
        }
        if let Err(error) = fs::rename(&stage, &target) {
            if had_previous {
                let _ = fs::rename(&backup, &target);
            }
            return Err(error.into());
        }
        source_backup = Some((target, had_previous.then_some(backup), scratch, identity));
    } else if !checked
        .source
        .join("src/userplugins/articulate")
        .join(MANIFEST)
        .exists()
    {
        write_new(
            &checked
                .source
                .join("src/userplugins/articulate")
                .join(MANIFEST),
            &serde_json::to_vec_pretty(&bundled_manifest())?,
        )?;
    }
    if rebuild && let Err(error) = build(&checked.source, native, &progress) {
        if let Some((target, previous, scratch, identity)) = source_backup {
            fs::rename(&target, scratch.join(format!("failed-source-{identity}")))?;
            if let Some(previous) = previous {
                fs::rename(previous, target)?;
            }
        }
        return Err(error);
    }
    Ok(InstallReport {
        changed,
        built: rebuild,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn node_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

fn npm_cli(node: &Path) -> Option<PathBuf> {
    let canonical = node.canonicalize().ok()?;
    let root = external_path(canonical.parent()?);
    [
        root.join("node_modules/npm/bin/npm-cli.js"),
        root.join("../lib/node_modules/npm/bin/npm-cli.js"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn external_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path.to_owned()
}

fn usable_node(node: &Path) -> bool {
    let mut command = Command::new(node);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.output().ok().is_some_and(|output| {
        output.status.success()
            && std::str::from_utf8(&output.stdout)
                .ok()
                .and_then(|version| {
                    semver::Version::parse(version.trim().trim_start_matches('v')).ok()
                })
                .is_some_and(|version| version.major >= 22)
    })
}

fn ensure_node(progress: &impl Fn(&str)) -> Result<PathBuf> {
    if let Some(node) = node_binary().filter(|node| usable_node(node)) {
        return Ok(node);
    }
    #[cfg(windows)]
    {
        let runtimes = crate::model::data_dir().join("runtimes");
        fs::create_dir_all(&runtimes)?;
        ordinary(&runtimes)?;
        let installed = runtimes.join(NODE_FOLDER);
        let node = installed.join("node.exe");
        if installed.exists() {
            ordinary(&installed)?;
            ordinary(&node)?;
            ensure!(
                usable_node(&node),
                "The local Node.js runtime is incomplete. Remove it and retry setup."
            );
            return Ok(node);
        }
        progress("Downloading the verified local Node.js build tools…");
        let archive = runtimes.join(format!("{NODE_FOLDER}.zip"));
        download(NODE_URL, NODE_SHA256, &archive, 128 * 1024 * 1024)?;
        let stage = runtimes.join(format!("extract-{}", super::plugin::new_token()?));
        fs::create_dir(&stage)?;
        extract_archive(&archive, &stage)?;
        ordinary(&stage.join(NODE_FOLDER))?;
        ensure!(
            usable_node(&stage.join(NODE_FOLDER).join("node.exe")),
            "The downloaded Node.js runtime did not start"
        );
        fs::rename(stage.join(NODE_FOLDER), installed)?;
        Ok(node)
    }
    #[cfg(not(windows))]
    {
        let _ = progress;
        bail!("Install Node.js 22 or newer, then build again")
    }
}

fn with_node_path(command: &mut Command, node: &Path) -> Result<()> {
    let root = node.parent().context("Node.js directory unavailable")?;
    let current = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(root.to_owned()).chain(std::env::split_paths(&current));
    command.env("PATH", std::env::join_paths(paths)?);
    Ok(())
}

fn build(source: &Path, native: bool, progress: &impl Fn(&str)) -> Result<()> {
    let package: serde_json::Value =
        serde_json::from_slice(&bounded_read(&source.join("package.json"), 128 * 1024)?)?;
    let manager = package["packageManager"]
        .as_str()
        .context("The Vencord source has no pinned package manager")?;
    let version = manager
        .strip_prefix("pnpm@")
        .and_then(|value| value.split('+').next())
        .context("Unsupported Vencord package manager")?;
    let version = semver::Version::parse(version)?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "Use a stable pinned pnpm version"
    );
    let node = ensure_node(progress)?;
    let log = crate::model::data_dir().join("logs/vencord-build.log");
    fs::create_dir_all(log.parent().unwrap())?;
    if !source.join("node_modules/esbuild/package.json").is_file() {
        progress("Installing Vencord's pinned build dependencies…");
        let npm = npm_cli(&node).context("This Node.js installation does not include npm")?;
        let mut command = Command::new(&node);
        with_node_path(&mut command, &node)?;
        command.arg(npm).args([
            "exec",
            "--yes",
            &format!("--package=pnpm@{version}"),
            "--",
            "pnpm",
            "install",
            "--frozen-lockfile",
        ]);
        run(command, source, &log, Duration::from_secs(1800))?;
    }
    progress("Building Vencord with the Articulate companion…");
    let scratch = source.join(".articulate-companion");
    fs::create_dir_all(&scratch)?;
    ordinary(&scratch)?;
    let identity = super::plugin::new_token()?;
    let stage = scratch.join(format!("build-{identity}"));
    fs::create_dir(&stage)?;
    let mut copied = 0;
    for name in ["src", "scripts", "package.json", "tsconfig.json"] {
        copy_tree(&source.join(name), &stage.join(name), &mut copied)?;
    }
    let mut command = Command::new(&node);
    with_node_path(&mut command, &node)?;
    command.args([
        "--require=./scripts/suppressExperimentalWarnings.js",
        "scripts/build/build.mjs",
    ]);
    if same_path(source, &managed_source()) {
        command
            .arg("--disable-updater")
            .env("VENCORD_HASH", &SOURCE_REVISION[..7])
            .env("VENCORD_REMOTE", "Vendicated/Vencord");
    }
    run(command, &stage, &log, Duration::from_secs(600))?;
    for name in CORE_ARTIFACTS {
        ensure!(
            stage.join("dist").join(name).is_file(),
            "Vencord did not produce a complete desktop build"
        );
    }
    ensure!(
        bounded_read(&stage.join("dist/renderer.js"), 16 * 1024 * 1024)?
            .windows(b"Articulate".len())
            .any(|bytes| bytes == b"Articulate"),
        "The Vencord build does not contain the companion"
    );
    if native {
        stage_native_audio(&stage.join("dist"))?;
    }
    let mut artifacts = BTreeMap::new();
    let names = CORE_ARTIFACTS.into_iter().chain(
        NATIVE_FILES
            .iter()
            .filter(|_| native)
            .map(|(name, _)| *name),
    );
    for name in names {
        artifacts.insert(
            name.to_owned(),
            hash(&bounded_read(
                &stage.join("dist").join(name),
                16 * 1024 * 1024,
            )?),
        );
    }
    write_new(
        &stage.join("dist/.articulate-build.json"),
        &serde_json::to_vec_pretty(&Manifest {
            schema: 1,
            revision: bundled_manifest().revision,
            files: artifacts,
        })?,
    )?;
    let target = source.join("dist");
    let backup = scratch.join(format!("backup-dist-{identity}"));
    let had_previous = target.exists();
    if had_previous {
        ordinary(&target)?;
        fs::rename(&target, &backup)?;
    }
    if let Err(error) = fs::rename(stage.join("dist"), &target) {
        if had_previous {
            let _ = fs::rename(&backup, &target);
        }
        return Err(error.into());
    }
    progress("Companion built. Install this custom build, then restart Discord.");
    Ok(())
}

fn stage_native_audio(dist: &Path) -> Result<()> {
    ensure!(
        native_payload_available(),
        "Experimental audio adapter unavailable in this app build"
    );
    for (name, bytes) in NATIVE_FILES {
        let path = dist.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        ordinary(path.parent().unwrap())?;
        write_new(&path, bytes)?;
    }
    // Keep the original built preload intact and append our guarded loader only
    // in the staging directory. Discord's own preload has already been required.
    let preload = dist.join("preload.js");
    let mut bytes = bounded_read(&preload, 16 * 1024 * 1024)?;
    bytes.extend_from_slice(b"\n;try { require(require('node:path').join(__dirname, 'articulate-audio-preload.cjs')); } catch {}\n");
    fs::write(preload, bytes)?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path, total: &mut u64) -> Result<()> {
    ordinary(source)?;
    let metadata = fs::metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()), total)?;
        }
    } else {
        ensure!(metadata.is_file(), "Unsupported file in the Vencord source");
        *total = total.saturating_add(metadata.len());
        ensure!(
            *total <= 512 * 1024 * 1024,
            "Vencord source exceeds build staging size limit"
        );
        fs::copy(source, target)?;
    }
    Ok(())
}

fn run(mut command: Command, directory: &Path, log: &Path, timeout: Duration) -> Result<()> {
    let stdout = fs::OpenOptions::new().create(true).append(true).open(log)?;
    command
        .current_dir(external_path(directory))
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stdout);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(windows)]
    let job = ToolJob::new()?;
    let mut child = ToolChild(
        command
            .spawn()
            .context("Could not start the Vencord build tool")?,
    );
    #[cfg(windows)]
    job.attach(&child.0)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.0.try_wait()? {
            ensure!(
                status.success(),
                "Vencord build failed. Details are in Articulate's local logs/vencord-build.log file."
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            let _ = child.0.wait();
            bail!("Vencord build timed out. Try again after checking its build dependencies.");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

struct ToolChild(std::process::Child);
impl Drop for ToolChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
struct ToolJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl ToolJob {
    fn new() -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(!handle.is_null(), "Could not create an isolated build job");
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32
                ) != 0,
                "Could not configure build process cleanup"
            );
            Ok(job)
        }
    }
    fn attach(&self, child: &std::process::Child) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        ensure!(
            unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    self.0,
                    child.as_raw_handle(),
                )
            } != 0,
            "Could not isolate the Vencord build process"
        );
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for ToolJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn download(url: &str, expected: &str, destination: &Path, limit: u64) -> Result<()> {
    if destination.exists() {
        ordinary(destination)?;
        ensure!(
            hash(&bounded_read(destination, limit)?) == expected,
            "A cached download has changed. Remove it before retrying."
        );
        return Ok(());
    }
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build();
    let mut response = config
        .new_agent()
        .get(url)
        .call()
        .context("The verified download could not be fetched")?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit && hash(&bytes) == expected,
        "Downloaded file failed size or SHA-256 verification"
    );
    write_new(destination, &bytes)
}

/// Creates a private, pinned source checkout. Does not touch Discord or installed Vencord.
#[cfg(windows)]
pub fn prepare_managed(progress: impl Fn(&str)) -> Result<Plan> {
    let destination = managed_source();
    if destination.exists() {
        return plan(&destination);
    }
    let cache = crate::model::data_dir().join("vencord-downloads");
    fs::create_dir_all(&cache)?;
    ordinary(&cache)?;
    let archive = cache.join(format!("Vencord-{SOURCE_REVISION}.zip"));
    progress("Downloading the verified Vencord source…");
    download(SOURCE_URL, SOURCE_SHA256, &archive, 32 * 1024 * 1024)?;
    let stage = cache.join(format!("extract-{}", super::plugin::new_token()?));
    fs::create_dir(&stage)?;
    extract_archive(&archive, &stage)?;
    let extracted = source_root(&stage.join(format!("Vencord-{SOURCE_REVISION}")))?;
    fs::rename(extracted, &destination)?;
    plan(&destination)
}

#[cfg(windows)]
fn extract_archive(archive: &Path, stage: &Path) -> Result<()> {
    let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$root = [System.IO.Path]::GetFullPath($env:ARTICULATE_VENCORD_STAGE) + [System.IO.Path]::DirectorySeparatorChar
$zip = [System.IO.Compression.ZipFile]::OpenRead($env:ARTICULATE_VENCORD_ARCHIVE)
try {
  if ($zip.Entries.Count -gt 50000) { throw 'Too many archive entries' }
  $total = 0L
  foreach ($entry in $zip.Entries) {
    $target = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($root, $entry.FullName))
    if (-not $target.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe archive path' }
    if ($entry.FullName.Contains(':') -or $entry.FullName.Contains('\') -or (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) { throw 'Unsupported archive entry' }
    $total += $entry.Length
    if ($total -gt 536870912) { throw 'Archive exceeds extraction limit' }
  }
} finally { $zip.Dispose() }
[System.IO.Compression.ZipFile]::ExtractToDirectory($env:ARTICULATE_VENCORD_ARCHIVE, $env:ARTICULATE_VENCORD_STAGE)
"#;
    let powershell =
        PathBuf::from(std::env::var_os("SystemRoot").context("Windows directory unavailable")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let log = crate::model::data_dir().join("logs/vencord-build.log");
    fs::create_dir_all(log.parent().unwrap())?;
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("ARTICULATE_VENCORD_ARCHIVE", external_path(archive))
        .env("ARTICULATE_VENCORD_STAGE", external_path(stage));
    run(
        command,
        stage.parent().context("Invalid extraction folder")?,
        &log,
        Duration::from_secs(120),
    )
}

#[cfg(not(windows))]
pub fn prepare_managed(_progress: impl Fn(&str)) -> Result<Plan> {
    bail!("Automatic Vencord setup is currently available on Windows")
}

/// Opens the official interactive installer in custom-build mode. Never patches Discord itself.
#[cfg(windows)]
pub fn open_installer(source: &Path, progress: impl Fn(&str)) -> Result<()> {
    let source = source_root(source)?;
    verify_build(&source)?;
    let cache = crate::model::data_dir().join("vencord-downloads");
    fs::create_dir_all(&cache)?;
    ordinary(&cache)?;
    let installer = cache.join("VencordInstaller-1.4.2.exe");
    progress("Downloading the verified Vencord installer…");
    download(
        INSTALLER_URL,
        INSTALLER_SHA256,
        &installer,
        32 * 1024 * 1024,
    )?;
    Command::new(&installer)
        .env("VENCORD_USER_DATA_DIR", external_path(&source))
        .env("VENCORD_DEV_INSTALL", "1")
        .spawn()
        .context("Windows could not open the Vencord installer")?;
    progress("Choose your Discord installation in the Vencord installer, then restart Discord.");
    Ok(())
}

fn verify_build(source: &Path) -> Result<()> {
    ensure!(
        plan(source)?.status == PluginStatus::Current,
        "Update and build the companion first"
    );
    let manifest: Manifest = serde_json::from_slice(
        &bounded_read(&source.join("dist/.articulate-build.json"), 8192)
            .context("Build the companion with Articulate before opening the installer")?,
    )?;
    ensure!(
        manifest.schema == 1
            && manifest.revision == bundled_manifest().revision
            && (manifest.files.len() == 4 || manifest.files.len() == 9),
        "The custom build is out of date. Build the companion again"
    );
    let native = manifest.files.len() == 9;
    let names = CORE_ARTIFACTS.into_iter().chain(
        NATIVE_FILES
            .iter()
            .filter(|_| native)
            .map(|(name, _)| *name),
    );
    for name in names {
        let artifact = source.join("dist").join(name);
        ordinary(&artifact)?;
        ensure!(
            manifest.files.get(name) == Some(&hash(&bounded_read(&artifact, 16 * 1024 * 1024)?)),
            "The custom build changed since it was prepared. Build the companion again"
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn open_installer(_source: &Path, _progress: impl Fn(&str)) -> Result<()> {
    bail!("Automatic Vencord installation is currently available on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "articulate-plugin-test-{}",
                super::super::plugin::new_token().unwrap()
            ));
            fs::create_dir_all(root.join("src/plugins")).unwrap();
            fs::create_dir_all(root.join("scripts/build")).unwrap();
            fs::write(
                root.join("package.json"),
                br#"{"name":"vencord","packageManager":"pnpm@11.9.0"}"#,
            )
            .unwrap();
            fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'").unwrap();
            fs::write(root.join("scripts/build/build.mjs"), "").unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn installs_idempotently_and_refuses_user_changes() {
        let fixture = Fixture::new();
        let request = plan(&fixture.0).unwrap();
        assert_eq!(request.status, PluginStatus::Missing);
        assert!(install(&request, false, |_| {}).unwrap().changed);
        assert_eq!(plan(&fixture.0).unwrap().status, PluginStatus::Current);
        assert!(
            !install(&plan(&fixture.0).unwrap(), false, |_| {})
                .unwrap()
                .changed
        );
        fs::write(
            fixture.0.join("src/userplugins/articulate/index.ts"),
            "user edits",
        )
        .unwrap();
        assert!(plan(&fixture.0).is_err());
    }
    #[test]
    fn managed_upgrade_keeps_backup_and_other_plugins() {
        let fixture = Fixture::new();
        install(&plan(&fixture.0).unwrap(), false, |_| {}).unwrap();
        let plugin = fixture.0.join("src/userplugins/articulate");
        let mut old = bundled_manifest();
        fs::write(plugin.join("index.ts"), "previous managed version").unwrap();
        old.files
            .insert("index.ts".into(), hash(b"previous managed version"));
        fs::write(plugin.join(MANIFEST), serde_json::to_vec(&old).unwrap()).unwrap();
        fs::write(fixture.0.join("src/userplugins/other.ts"), "preserve").unwrap();
        let request = plan(&fixture.0).unwrap();
        assert_eq!(request.status, PluginStatus::UpdateAvailable);
        install(&request, false, |_| {}).unwrap();
        assert_eq!(
            fs::read(fixture.0.join("src/userplugins/other.ts")).unwrap(),
            b"preserve"
        );
        assert!(
            fs::read_dir(fixture.0.join(".articulate-companion"))
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with("backup-"))
        );
    }
    #[test]
    fn installed_dist_and_unowned_plugin_are_not_source_or_managed() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("dist")).unwrap();
        fs::write(fixture.0.join("dist/package.json"), "{}").unwrap();
        assert!(plan(&fixture.0.join("dist")).is_err());
        let plugin = fixture.0.join("src/userplugins/articulate");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("index.ts"), "unknown").unwrap();
        assert!(plan(&fixture.0).is_err());
    }

    #[test]
    fn failed_preparation_restores_previous_plugin_and_leaves_dist_unchanged() {
        let fixture = Fixture::new();
        install(&plan(&fixture.0).unwrap(), false, |_| {}).unwrap();
        let plugin = fixture.0.join("src/userplugins/articulate");
        let mut previous = bundled_manifest();
        fs::write(plugin.join("index.ts"), "previous managed version").unwrap();
        previous
            .files
            .insert("index.ts".into(), hash(b"previous managed version"));
        fs::write(
            plugin.join(MANIFEST),
            serde_json::to_vec(&previous).unwrap(),
        )
        .unwrap();
        fs::create_dir(fixture.0.join("dist")).unwrap();
        fs::write(
            fixture.0.join("dist/renderer.js"),
            "original compiled output",
        )
        .unwrap();
        // Invalid package-manager selection fails before tools or downloads run.
        fs::write(
            fixture.0.join("package.json"),
            br#"{"name":"vencord","packageManager":"unexpected"}"#,
        )
        .unwrap();
        assert!(install(&plan(&fixture.0).unwrap(), true, |_| {}).is_err());
        assert_eq!(
            fs::read(plugin.join("index.ts")).unwrap(),
            b"previous managed version"
        );
        assert_eq!(
            fs::read(fixture.0.join("dist/renderer.js")).unwrap(),
            b"original compiled output"
        );
    }

    #[test]
    fn build_verification_rejects_missing_and_changed_artifacts() {
        let fixture = Fixture::new();
        install(&plan(&fixture.0).unwrap(), false, |_| {}).unwrap();
        assert!(verify_build(&fixture.0).is_err());
        fs::create_dir(fixture.0.join("dist")).unwrap();
        let mut files = BTreeMap::new();
        for name in ["patcher.js", "preload.js", "renderer.js", "renderer.css"] {
            fs::write(fixture.0.join("dist").join(name), "synthetic build").unwrap();
            files.insert(name.to_owned(), hash(b"synthetic build"));
        }
        if native_payload_available() {
            stage_native_audio(&fixture.0.join("dist")).unwrap();
            files.insert(
                "preload.js".into(),
                hash(&fs::read(fixture.0.join("dist/preload.js")).unwrap()),
            );
            for (name, bytes) in NATIVE_FILES {
                files.insert(name.into(), hash(bytes));
            }
        }
        fs::write(
            fixture.0.join("dist/.articulate-build.json"),
            serde_json::to_vec(&Manifest {
                schema: 1,
                revision: bundled_manifest().revision,
                files,
            })
            .unwrap(),
        )
        .unwrap();
        verify_build(&fixture.0).unwrap();
        fs::write(fixture.0.join("dist/renderer.js"), "changed").unwrap();
        assert!(verify_build(&fixture.0).is_err());
    }

    #[test]
    fn injector_detection_reads_exact_require_target_without_execution() {
        let path = std::env::temp_dir().join("synthetic-vencord/dist/patcher.js");
        let code = format!("require({})", serde_json::to_string(&path).unwrap());
        let header = serde_json::to_vec(
            &serde_json::json!({"files":{"index.js":{"offset":"0","size":code.len()}}}),
        )
        .unwrap();
        let aligned = (header.len() + 3) & !3;
        let mut bytes = Vec::new();
        for word in [4, aligned + 8, aligned + 4, header.len()] {
            bytes.extend_from_slice(&(word as u32).to_le_bytes());
        }
        bytes.extend_from_slice(&header);
        bytes.resize(16 + aligned, 0);
        bytes.extend_from_slice(code.as_bytes());
        assert_eq!(injector_target(&bytes), Some(path));
        assert!(injector_target(b"require('arbitrary code')").is_none());
        bytes.truncate(16);
        assert!(injector_target(&bytes).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn build_timeout_terminates_spawned_descendants() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                WaitForSingleObject,
            },
        };
        let fixture = Fixture::new();
        let pid_file = fixture.0.join("child-pid.txt");
        let log = fixture.0.join("synthetic-tool.log");
        let mut command = Command::new(
            PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
$child = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 30') -WindowStyle Hidden -PassThru
[System.IO.File]::WriteAllText($env:ARTICULATE_TOOL_TEST_PID, [string]$child.Id)
Start-Sleep -Seconds 30
"#]).env("ARTICULATE_TOOL_TEST_PID", &pid_file);
        let directory = fixture.0.clone();
        let running = thread::spawn(move || run(command, &directory, &log, Duration::from_secs(8)));
        let deadline = Instant::now() + Duration::from_secs(6);
        while !pid_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let pid: u32 = fs::read_to_string(&pid_file)
            .expect("Synthetic child should start")
            .parse()
            .unwrap();
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert!(!handle.is_null());
        assert!(running.join().unwrap().is_err());
        let stopped = unsafe { WaitForSingleObject(handle, 1000) };
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(
            stopped, WAIT_OBJECT_0,
            "The build job must terminate its descendant"
        );
    }

    #[test]
    #[ignore = "Downloads and builds the pinned Vencord source in an explicitly isolated LOCALAPPDATA; never injects Discord"]
    fn managed_bootstrap_build_smoke() {
        let expected = std::env::var_os("ARTICULATE_VENCORD_TEST_DATA")
            .expect("Set an isolated test data directory");
        assert!(crate::model::data_dir().starts_with(PathBuf::from(expected)));
        let request = prepare_managed(|message| println!("{message}")).unwrap();
        let report = install(&request, true, |message| println!("{message}")).unwrap();
        assert!(report.built);
        verify_build(&request.source).unwrap();
    }

    #[test]
    #[ignore = "Builds an explicitly isolated custom Vencord with the embedded native adapter; never injects Discord"]
    fn native_managed_bootstrap_build_smoke() {
        let expected =
            std::env::var_os("ARTICULATE_VENCORD_TEST_DATA").expect("Set isolated test data");
        assert!(crate::model::data_dir().starts_with(PathBuf::from(expected)));
        assert!(
            native_payload_available(),
            "Build tests with ARTICULATE_NATIVE_AUDIO_DIR set"
        );
        let request = prepare_managed(|message| println!("{message}")).unwrap();
        let report = install(&request, true, |message| println!("{message}")).unwrap();
        assert!(report.built && native_audio_enabled(&request.source));
        verify_build(&request.source).unwrap();
        let manifest: Manifest = serde_json::from_slice(
            &bounded_read(&request.source.join("dist/.articulate-build.json"), 16384).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.files.len(), 9);
        for (name, bytes) in NATIVE_FILES {
            assert_eq!(
                fs::read(request.source.join("dist").join(name)).unwrap(),
                bytes
            );
        }
        let preload = fs::read_to_string(request.source.join("dist/preload.js")).unwrap();
        assert!(preload.contains("articulate-audio-preload.cjs"));
    }
}
