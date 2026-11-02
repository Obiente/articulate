//! Explicit, user-triggered updates from the project's public GitHub releases.
use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const API: &str = "https://api.github.com/repos/Obiente/articulate/releases/latest";
pub const RELEASES: &str = "https://github.com/Obiente/articulate/releases";
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;
const MAX_METADATA: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub size: u64,
    url: String,
    name: String,
    digest: [u8; 32],
}

#[derive(Default, Debug)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    Latest,
    Available(Release),
    Downloading {
        version: String,
        progress: f32,
    },
    Ready(InstallRequest),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct InstallRequest {
    pub version: String,
    path: PathBuf,
    size: u64,
    digest: [u8; 32],
    manifest: Vec<u8>,
    signature: Vec<u8>,
}

impl InstallRequest {
    /// Call only after capture has stopped and durable application state is saved.
    /// Recheck the downloaded bytes immediately before running the installer.
    pub fn launch(&self) -> Result<()> {
        verify_manifest(
            &self.manifest,
            &self.signature,
            &self.version,
            self.path
                .file_name()
                .and_then(|v| v.to_str())
                .context("Invalid installer name")?,
            self.size,
            &self.digest,
        )?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Deny concurrent replacement/writes while verifying and creating the process.
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&self.path)
                .context("The downloaded update is missing. Download it again.")?;
            verify_reader(&mut file, self.size, &self.digest)?;
            std::process::Command::new(&self.path).spawn().context(
                "Windows could not open the installer. Try again or download it from Releases.",
            )?;
            Ok(())
        }
        #[cfg(not(windows))]
        anyhow::bail!("In-app installation is available on Windows only.")
    }
}

pub struct State {
    pub status: Status,
    pending: Option<Receiver<Event>>,
}

impl Default for State {
    fn default() -> Self {
        static CLEANUP: std::sync::Once = std::sync::Once::new();
        CLEANUP.call_once(|| {
            let _ = prune_downloads(&crate::model::data_dir().join("updates"), None);
        });
        Self {
            status: Status::Idle,
            pending: None,
        }
    }
}

/// Call before starting update work; preserve only the directory of the pending installer.
pub fn prune_downloads(root: &std::path::Path, pending: Option<&std::path::Path>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut first_error = None;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if pending == Some(path.as_path()) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                continue;
            }
        }
        if metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && let Err(error) = fs::remove_dir_all(path)
        {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = first_error {
        return Err(error.into());
    }
    Ok(())
}

enum Event {
    Checked(Result<Option<Release>, String>),
    Progress(f32),
    Downloaded(Result<InstallRequest, String>),
}

impl State {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn poll(&mut self) {
        let Some(receiver) = self.pending.as_ref() else {
            return;
        };
        let mut done = false;
        loop {
            match receiver.try_recv() {
                Ok(Event::Checked(result)) => {
                    self.status = match result {
                        Ok(Some(release)) => Status::Available(release),
                        Ok(None) => Status::Latest,
                        Err(error) => Status::Error(error),
                    };
                    done = true;
                    break;
                }
                Ok(Event::Downloaded(result)) => {
                    self.status = match result {
                        Ok(installer) => Status::Ready(installer),
                        Err(error) => Status::Error(error),
                    };
                    done = true;
                    break;
                }
                Ok(Event::Progress(value)) => {
                    if let Status::Downloading { progress, .. } = &mut self.status {
                        *progress = value;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status =
                        Status::Error("The update task stopped. Please try again.".into());
                    done = true;
                    break;
                }
            }
        }
        if done {
            self.pending = None;
        }
    }

    pub fn check(&mut self) {
        if self.busy() {
            return;
        }
        self.status = Status::Checking;
        self.start("update-check", |tx| {
            let result = check_release().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Checked(result));
        });
    }

    pub fn download(&mut self, release: Release) {
        if self.busy() {
            return;
        }
        self.status = Status::Downloading {
            version: release.version.clone(),
            progress: 0.0,
        };
        self.start("update-download", move |tx| {
            let result = download_release(&release, |p| {
                let _ = tx.send(Event::Progress(p));
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Downloaded(result));
        });
    }

    fn start(&mut self, name: &str, work: impl FnOnce(Sender<Event>) + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name(name.into())
            .spawn(move || work(tx))
        {
            Ok(_) => self.pending = Some(rx),
            Err(error) => {
                self.status = Status::Error(format!("Could not start the update task: {error}"))
            }
        }
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ApiAsset>,
}
#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
    state: String,
}

fn parse_digest(value: &str) -> Result<[u8; 32]> {
    let hex = value
        .strip_prefix("sha256:")
        .context("The release has no SHA-256 checksum. Use Releases to report this.")?;
    ensure!(
        hex.len() == 64 && hex.bytes().all(|v| v.is_ascii_hexdigit()),
        "The release checksum is invalid."
    );
    let mut digest = [0u8; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(digest)
}

fn select_release(release: ApiRelease, current: &str) -> Result<Option<Release>> {
    ensure!(
        !release.draft && !release.prerelease,
        "The update service returned an unpublished or preview release."
    );
    let tag_version = release
        .tag_name
        .strip_prefix('v')
        .context("The release version is invalid.")?;
    let version = Version::parse(tag_version).context("The release version is invalid.")?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "Only stable releases can be installed here."
    );
    if version <= Version::parse(current)? {
        return Ok(None);
    }
    let name = format!("Articulate-{version}-windows-x86_64-setup.exe");
    let mut matches = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .context("This release does not have a Windows installer yet. Try again later.")?;
    ensure!(
        matches.next().is_none(),
        "The release has duplicate installers."
    );
    ensure!(
        asset.state == "uploaded",
        "The Windows installer is not ready yet."
    );
    ensure!(
        asset.size > 0 && asset.size <= MAX_INSTALLER,
        "The installer has an unexpected size."
    );
    let url = format!("{RELEASES}/download/v{version}/{name}");
    ensure!(
        asset.browser_download_url == url,
        "The installer address is outside the Articulate release."
    );
    let digest = parse_digest(
        asset
            .digest
            .as_deref()
            .context("This release has no checksum. Try again later.")?,
    )?;
    Ok(Some(Release {
        version: version.to_string(),
        size: asset.size,
        url,
        name,
        digest,
    }))
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(seconds)))
        .build()
        .into()
}

fn check_release() -> Result<Option<Release>> {
    check_release_for(env!("CARGO_PKG_VERSION"))
}

fn check_release_for(current: &str) -> Result<Option<Release>> {
    let mut response = agent(30)
        .get(API)
        .header(
            "User-Agent",
            concat!("Articulate/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .context("Could not check for updates. Check your connection and try again.")?;
    ensure!(
        response.status().as_u16() == 200,
        "The update service returned an unexpected response."
    );
    let bytes = read_limited(response.body_mut().as_reader(), MAX_METADATA)?;
    select_release(
        serde_json::from_slice(&bytes)
            .context("The update service returned invalid release information.")?,
        current,
    )
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "The update response is too large."
    );
    Ok(bytes)
}

fn permitted_redirect(url: &str, original: &str) -> bool {
    !url.bytes().any(|b| b.is_ascii_control() || b == b'\\')
        && (url == original
            || url.starts_with("https://release-assets.githubusercontent.com/")
            || url.starts_with("https://objects.githubusercontent.com/"))
}

fn download_release(release: &Release, mut progress: impl FnMut(f32)) -> Result<InstallRequest> {
    let base = format!("{RELEASES}/download/v{}", release.version);
    let (manifest, signature) = if cfg!(feature = "signed-updates") {
        (
            fetch_metadata(&format!("{base}/update-manifest.json"))?,
            fetch_metadata(&format!("{base}/update-manifest.sig"))?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    verify_manifest(
        &manifest,
        &signature,
        &release.version,
        &release.name,
        release.size,
        &release.digest,
    )?;
    let root = crate::model::data_dir().join("updates");
    fs::create_dir_all(&root).context("Could not create the update folder.")?;
    let unique = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let directory = root.join(unique);
    fs::create_dir(&directory)?;
    let part = directory.join("installer.part");
    let path = directory.join(&release.name);
    let result = (|| {
        let client = agent(900);
        let mut url = release.url.clone();
        let mut response = None;
        for _ in 0..5 {
            ensure!(
                permitted_redirect(&url, &release.url),
                "The update download was redirected outside GitHub's release storage."
            );
            let res = client
                .get(&url)
                .header("User-Agent", "Articulate")
                .call()
                .context("The update download failed. Check your connection and retry.")?;
            if res.status().is_redirection() {
                url = res
                    .headers()
                    .get("location")
                    .context("The download redirect has no address.")?
                    .to_str()
                    .context("The download redirect is invalid.")?
                    .to_owned();
            } else {
                ensure!(
                    res.status().as_u16() == 200,
                    "The installer download returned an unexpected response."
                );
                response = Some(res);
                break;
            }
        }
        let mut response = response.context("The update download redirected too many times.")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        let mut reader = response.body_mut().as_reader();
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut last_percent = 0;
        let mut buffer = [0u8; 65536];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            ensure!(
                bytes <= release.size && bytes <= MAX_INSTALLER,
                "The downloaded installer is larger than expected."
            );
            file.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
            let percent = bytes * 100 / release.size;
            if percent > last_percent {
                progress(bytes as f32 / release.size as f32);
                last_percent = percent;
            }
        }
        ensure!(
            bytes == release.size && <[u8; 32]>::from(hash.finalize()) == release.digest,
            "The update failed its integrity check. Download it again."
        );
        file.sync_all()?;
        drop(file);
        fs::rename(&part, &path)?;
        Ok(InstallRequest {
            version: release.version.clone(),
            path,
            size: release.size,
            digest: release.digest,
            manifest,
            signature,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
        let _ = fs::remove_dir(&directory);
    }
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedManifest {
    version: String,
    asset: String,
    size: u64,
    sha256: String,
}
fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N]> {
    ensure!(
        value.len() == N * 2 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid signing key or signature encoding"
    );
    let mut output = [0; N];
    for (i, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    Ok(output)
}
fn verify_signature(manifest: &[u8], signature: &[u8], public_key: &str) -> Result<()> {
    ensure!(
        manifest.len() <= 16384 && signature.len() <= 256,
        "Update signing metadata is too large"
    );
    let key = ed25519_dalek::VerifyingKey::from_bytes(&decode_hex::<32>(public_key.trim())?)?;
    let signature = ed25519_dalek::Signature::from_bytes(&decode_hex::<64>(
        std::str::from_utf8(signature)?.trim(),
    )?);
    key.verify_strict(manifest, &signature)
        .context("The update signature is invalid")
}
fn verify_manifest(
    manifest: &[u8],
    signature: &[u8],
    version: &str,
    asset: &str,
    size: u64,
    digest: &[u8; 32],
) -> Result<()> {
    if !cfg!(feature = "signed-updates") {
        return Ok(());
    }
    verify_manifest_with_key(
        manifest,
        signature,
        version,
        asset,
        size,
        digest,
        include_str!("../assets/update-signing.pub"),
    )
}
fn verify_manifest_with_key(
    manifest: &[u8],
    signature: &[u8],
    version: &str,
    asset: &str,
    size: u64,
    digest: &[u8; 32],
    key: &str,
) -> Result<()> {
    verify_signature(manifest, signature, key)?;
    let signed: SignedManifest = serde_json::from_slice(manifest)?;
    ensure!(
        signed.version == version
            && signed.asset == asset
            && signed.size == size
            && decode_hex::<32>(&signed.sha256)? == *digest,
        "The signed release manifest does not match the installer"
    );
    Ok(())
}
fn fetch_metadata(original: &str) -> Result<Vec<u8>> {
    let mut url = original.to_owned();
    for _ in 0..5 {
        ensure!(
            permitted_redirect(&url, original),
            "Invalid signing metadata redirect"
        );
        let mut response = agent(30)
            .get(&url)
            .header("User-Agent", "Articulate")
            .call()?;
        if response.status().is_redirection() {
            url = response
                .headers()
                .get("location")
                .context("Missing signing metadata redirect")?
                .to_str()?
                .to_owned();
        } else {
            ensure!(
                response.status().as_u16() == 200,
                "Release signing metadata is missing"
            );
            return read_limited(response.body_mut().as_reader(), 16384);
        }
    }
    anyhow::bail!("Too many signing metadata redirects")
}

fn verify_reader(reader: &mut impl Read, size: u64, digest: &[u8; 32]) -> Result<()> {
    ensure!(
        size > 0 && size <= MAX_INSTALLER,
        "The update size is invalid."
    );
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        received += count as u64;
        ensure!(
            received <= size,
            "The downloaded update changed. Download it again."
        );
        hash.update(&buffer[..count]);
    }
    ensure!(
        received == size && <[u8; 32]>::from(hash.finalize()) == *digest,
        "The downloaded update changed. Download it again."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_keeps_only_the_pending_installer_directory() {
        let root = std::env::temp_dir().join(crate::discord::plugin::new_token().unwrap());
        let pending = root.join("pending");
        let stale = root.join("stale");
        fs::create_dir_all(&pending).unwrap();
        fs::create_dir(&stale).unwrap();
        fs::write(pending.join("setup.exe"), b"synthetic").unwrap();
        fs::write(stale.join("setup.part"), b"partial").unwrap();
        prune_downloads(&root, Some(&pending)).unwrap();
        assert!(pending.join("setup.exe").exists());
        assert!(!stale.exists());
        prune_downloads(&root, None).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn signature_switch_preserves_legacy_updates_by_default() {
        let result = verify_manifest(b"", b"", "1.0.0", "setup.exe", 42, &[0; 32]);
        assert_eq!(result.is_err(), cfg!(feature = "signed-updates"));
    }
    #[test]
    fn signed_manifest_binds_every_installer_field() {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let digest = [0xab; 32];
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version":"1.2.3", "asset":"setup.exe", "size":42, "sha256":hex(&digest)
        }))
        .unwrap();
        let signature = hex(&key.sign(&manifest).to_bytes());
        let public = hex(&key.verifying_key().to_bytes());
        let verify = |version, asset, size, digest| {
            verify_manifest_with_key(
                &manifest,
                signature.as_bytes(),
                version,
                asset,
                size,
                digest,
                &public,
            )
        };
        assert!(verify("1.2.3", "setup.exe", 42, &digest).is_ok());
        assert!(verify("1.2.4", "setup.exe", 42, &digest).is_err());
        assert!(verify("1.2.3", "other.exe", 42, &digest).is_err());
        assert!(verify("1.2.3", "setup.exe", 43, &digest).is_err());
        assert!(verify("1.2.3", "setup.exe", 42, &[0; 32]).is_err());
    }
    #[test]
    fn signatures_reject_tampering_missing_values_and_placeholder() {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let manifest = b"synthetic release";
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let signature = hex(&key.sign(manifest).to_bytes());
        let public = hex(&key.verifying_key().to_bytes());
        assert!(verify_signature(manifest, signature.as_bytes(), &public).is_ok());
        assert!(verify_signature(b"changed", signature.as_bytes(), &public).is_err());
        assert!(verify_signature(manifest, b"", &public).is_err());
        assert!(verify_signature(manifest, signature.as_bytes(), "PLACEHOLDER").is_err());
    }
    #[test]
    #[ignore = "Downloads the latest public installer from GitHub; never launches it"]
    fn published_installer_download_passes_the_real_update_path() {
        let release = check_release_for("0.0.0")
            .expect("fetch public release")
            .expect("a public release newer than 0.0.0");
        let request = download_release(&release, |_| {}).expect("download verified installer");
        let mut file = fs::File::open(&request.path).expect("download exists");
        verify_reader(&mut file, request.size, &request.digest).expect("installer unchanged");
        drop(file);
        fs::remove_file(&request.path).expect("remove this test's installer");
        fs::remove_dir(request.path.parent().unwrap()).expect("remove empty test folder");
    }
    fn fixture(version: &str) -> ApiRelease {
        let name = format!("Articulate-{version}-windows-x86_64-setup.exe");
        ApiRelease {
            tag_name: format!("v{version}"),
            draft: false,
            prerelease: false,
            assets: vec![ApiAsset {
                browser_download_url: format!("{RELEASES}/download/v{version}/{name}"),
                name,
                size: 42,
                digest: Some(format!("sha256:{}", "ab".repeat(32))),
                state: "uploaded".into(),
            }],
        }
    }
    #[test]
    fn versions_use_semver_ordering_and_never_downgrade() {
        assert!(
            select_release(fixture("0.10.0"), "0.9.0")
                .unwrap()
                .is_some()
        );
        assert!(
            select_release(fixture("0.9.0"), "0.10.0")
                .unwrap()
                .is_none()
        );
        assert!(select_release(fixture("0.1.0"), "0.1.0").unwrap().is_none());
        assert!(select_release(fixture("0.2.0-beta.1"), "0.1.0").is_err());
    }
    #[test]
    fn only_the_exact_repository_asset_url_is_accepted() {
        for url in [
            "https://github.com/evil/articulate/file.exe",
            "http://github.com/Obiente/articulate/file.exe",
            "https://github.com@evil.test/file.exe",
            "https://github.com.evil.test/file.exe",
            "file:///tmp/setup.exe",
        ] {
            let mut release = fixture("0.2.0");
            release.assets[0].browser_download_url = url.into();
            assert!(select_release(release, "0.1.0").is_err());
        }
        let mut release = fixture("0.2.0");
        release.assets[0].browser_download_url.push_str("?anything");
        assert!(select_release(release, "0.1.0").is_err());
    }
    #[test]
    fn checksums_are_required_and_strict() {
        for digest in [
            "",
            "sha1:abc",
            "sha256:abc",
            &format!("sha256:{}", "zz".repeat(32)),
        ] {
            assert!(parse_digest(digest).is_err());
        }
        assert!(parse_digest(&format!("sha256:{}", "aB".repeat(32))).is_ok());
        let mut release = fixture("0.2.0");
        release.assets[0].digest = None;
        assert!(select_release(release, "0.1.0").is_err());
    }
    #[test]
    fn rejects_invalid_sizes_and_unpublished_installers() {
        for size in [0, MAX_INSTALLER + 1] {
            let mut release = fixture("0.2.0");
            release.assets[0].size = size;
            assert!(select_release(release, "0.1.0").is_err());
        }
        let mut release = fixture("0.2.0");
        release.draft = true;
        assert!(select_release(release, "0.1.0").is_err());
        let mut release = fixture("0.2.0");
        release.assets[0].state = "starter".into();
        assert!(select_release(release, "0.1.0").is_err());
    }
    #[test]
    fn redirects_cannot_escape_https_github_storage() {
        for url in [
            "https://release-assets.githubusercontent.com.evil.test/a",
            "https://release-assets.githubusercontent.com@evil.test/a",
            "http://release-assets.githubusercontent.com/a",
            "https://evil.test/a",
            "https://release-assets.githubusercontent.com\\@evil.test/a",
            "https://release-assets.githubusercontent.com/a\n",
        ] {
            assert!(!permitted_redirect(
                url,
                "https://github.com/Obiente/articulate/a"
            ));
        }
        assert!(permitted_redirect(
            "https://release-assets.githubusercontent.com/github-production-release-asset/a?signature=test",
            "unused"
        ));
    }
    #[test]
    fn verifies_exact_size_digest_and_limits_metadata() {
        let bytes = b"synthetic installer";
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        assert!(verify_reader(&mut bytes.as_slice(), bytes.len() as u64, &digest).is_ok());
        assert!(verify_reader(&mut bytes.as_slice(), bytes.len() as u64 - 1, &digest).is_err());
        assert!(verify_reader(&mut bytes.as_slice(), bytes.len() as u64 + 1, &digest).is_err());
        assert!(verify_reader(&mut bytes.as_slice(), bytes.len() as u64, &[0; 32]).is_err());
        assert!(read_limited(bytes.as_slice(), 3).is_err());
        assert_eq!(
            read_limited(bytes.as_slice(), bytes.len() as u64).unwrap(),
            bytes
        );
    }
}
