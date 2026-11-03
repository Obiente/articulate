//! Small, opt-in-by-connection avatar cache. Only Discord avatar identifiers cross
//! the bridge; arbitrary URLs, cookies, tokens and transcript text never do.
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_BYTES: usize = 256 * 1024;
const MAX_MEMORY: usize = 256;
const MAX_DISK: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Avatar {
    pub user_id: String,
    pub hash: String,
}

impl Avatar {
    pub fn valid(&self) -> bool {
        let hash = self.hash.strip_prefix("a_").unwrap_or(&self.hash);
        !self.user_id.is_empty()
            && self.user_id.len() <= 20
            && self.user_id.bytes().all(|byte| byte.is_ascii_digit())
            && hash.len() == 32
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    pub fn uri(&self) -> String {
        format!("bytes://discord-avatar/{}/{}.png", self.user_id, self.hash)
    }

    fn url(&self) -> Result<String> {
        ensure!(self.valid(), "Invalid Discord avatar identity");
        Ok(format!(
            "https://cdn.discordapp.com/avatars/{}/{}.png?size=64",
            self.user_id, self.hash
        ))
    }

    fn filename(&self) -> String {
        format!("{:x}.png", Sha256::digest(self.uri().as_bytes()))
    }

    /// Parses the identifier form produced by `uri`, refusing anything else.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix("bytes://discord-avatar/")?;
        let (user_id, file) = rest.split_once('/')?;
        let hash = file.strip_suffix(".png")?;
        let avatar = Self {
            user_id: user_id.into(),
            hash: hash.into(),
        };
        avatar.valid().then_some(avatar)
    }
}

type Images = Arc<Mutex<HashMap<Avatar, Option<Arc<[u8]>>>>>;

/// Bounded participant picture cache: at most `MAX_MEMORY` entries in memory
/// and `MAX_DISK` PNG files on disk, each named by a digest of its identifier.
pub struct Cache {
    pending: mpsc::SyncSender<Avatar>,
    images: Images,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new(crate::model::data_dir().join("discord-avatars"))
    }
}

impl Cache {
    /// Caches under `directory`, which is created on first save.
    pub fn new(directory: PathBuf) -> Self {
        let (pending, receiver) = mpsc::sync_channel::<Avatar>(MAX_MEMORY);
        let images: Images = Arc::default();
        let worker_images = images.clone();
        let _ = std::thread::Builder::new()
            .name("discord-avatars".into())
            .spawn(move || {
                while let Ok(avatar) = receiver.recv() {
                    // Failure leaves initials in place and is not retried every frame.
                    let image = load(&directory, &avatar).ok().map(Arc::from);
                    if let Ok(mut images) = worker_images.lock() {
                        images.insert(avatar, image);
                    }
                }
            });
        Self { pending, images }
    }

    /// Called only for participants displayed in a connected or saved session.
    pub fn request(&self, avatar: &Avatar) {
        if !avatar.valid() {
            return;
        }
        if let Ok(mut images) = self.images.lock() {
            if images.contains_key(avatar) || images.len() >= MAX_MEMORY {
                return;
            }
            images.insert(avatar.clone(), None);
            if self.pending.try_send(avatar.clone()).is_err() {
                images.remove(avatar);
            }
        }
    }

    pub fn bytes(&self, avatar: &Avatar) -> Option<Arc<[u8]>> {
        self.images.lock().ok()?.get(avatar)?.clone()
    }

    /// Resolves a `bytes://discord-avatar/...` identifier to cached PNG bytes.
    /// Unknown identifiers are queued and return `None` until loaded.
    pub fn bytes_for_uri(&self, uri: &str) -> Option<Arc<[u8]>> {
        let avatar = Avatar::from_uri(uri)?;
        let bytes = self.bytes(&avatar);
        if bytes.is_none() {
            self.request(&avatar);
        }
        bytes
    }
}

fn png(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() >= 33 && bytes.len() <= MAX_BYTES,
        "Invalid avatar size"
    );
    ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            && bytes[8..12] == 13_u32.to_be_bytes()
            && &bytes[12..16] == b"IHDR",
        "Avatar is not a PNG"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        (1..=128).contains(&width) && (1..=128).contains(&height),
        "Avatar dimensions exceed limits"
    );
    Ok(())
}

fn read_png(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    png(&bytes)?;
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?;
    Ok(bytes)
}

fn load(directory: &Path, avatar: &Avatar) -> Result<Vec<u8>> {
    let url = avatar.url()?;
    let destination = directory.join(avatar.filename());
    if let Ok(file) = File::open(&destination)
        && let Ok(bytes) = read_png(file)
    {
        return Ok(bytes);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(4)))
        .build()
        .into();
    let mut response = agent.get(&url).header("Accept", "image/png").call()?;
    ensure!(
        response.status().as_u16() == 200,
        "Avatar request did not succeed"
    );
    if let Some(length) = response.headers().get("Content-Length") {
        ensure!(
            length.to_str()?.parse::<u64>()? <= MAX_BYTES as u64,
            "Avatar response is too large"
        );
    }
    let bytes = read_png(response.body_mut().as_reader())?;
    // A read-only/full disk must not prevent this session from displaying an image.
    let _ = save(directory, &destination, &bytes);
    Ok(bytes)
}

fn save(directory: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(directory)?;
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(directory)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            let stem = name.strip_suffix(".png")?;
            if stem.len() != 64 || !stem.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then_some((metadata.modified().ok()?, entry.path()))
        })
        .collect();
    files.sort_by_key(|entry| entry.0);
    let remove = files.len().saturating_sub(MAX_DISK - 1);
    for (_, path) in files.into_iter().take(remove) {
        fs::remove_file(path)?;
    }
    let temporary = destination.with_extension(format!("{}.part", std::process::id()));
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_only_urls_reject_paths_queries_and_hosts() {
        let valid = Avatar {
            user_id: "123456".into(),
            hash: "a_0123456789abcdef0123456789abcdef".into(),
        };
        assert_eq!(
            valid.url().unwrap(),
            "https://cdn.discordapp.com/avatars/123456/a_0123456789abcdef0123456789abcdef.png?size=64"
        );
        for user_id in [
            "",
            "../123",
            "123?token=x",
            "https://example.invalid",
            "123456789012345678901",
        ] {
            assert!(
                Avatar {
                    user_id: user_id.into(),
                    ..valid.clone()
                }
                .url()
                .is_err()
            );
        }
        for hash in [
            "",
            "../avatar",
            "0123456789abcdef0123456789abcdef?x",
            "0123456789abcdef0123456789abcdeFG",
        ] {
            assert!(
                Avatar {
                    hash: hash.into(),
                    ..valid.clone()
                }
                .url()
                .is_err()
            );
        }
    }

    #[test]
    fn cache_serves_local_files_by_identifier_without_a_network() {
        let directory = std::env::temp_dir().join(format!(
            "articulate-avatar-cache-{}",
            crate::discord::plugin::new_token().unwrap()
        ));
        let avatar = Avatar {
            user_id: "123456".into(),
            hash: "0123456789abcdef0123456789abcdef".into(),
        };
        assert_eq!(Avatar::from_uri(&avatar.uri()), Some(avatar.clone()));
        for uri in [
            "bytes://discord-avatar/123456/0123456789abcdef0123456789abcdef.svg",
            "bytes://discord-avatar/../x/0123456789abcdef0123456789abcdef.png",
            "https://cdn.discordapp.com/avatars/123456/0123456789abcdef0123456789abcdef.png",
            "bytes://discord-avatar/123456/0123456789abcdef0123456789abcdef.png?x=1",
        ] {
            assert!(Avatar::from_uri(uri).is_none(), "{uri}");
        }
        // The cached file name is a digest, never participant-controlled text.
        assert!(avatar.filename().len() == 68 && avatar.filename().ends_with(".png"));
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        save(&directory, &directory.join(avatar.filename()), &png).unwrap();
        let cache = Cache::new(directory.clone());
        assert!(
            cache.bytes_for_uri(&avatar.uri()).is_none(),
            "Loads happen off-thread"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while cache.bytes(&avatar).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            cache.bytes_for_uri(&avatar.uri()).as_deref(),
            Some(png.as_slice())
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dimensions_and_response_bytes_are_bounded_before_decode() {
        let mut header = vec![0; 33];
        header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        header[8..12].copy_from_slice(&13_u32.to_be_bytes());
        header[12..16].copy_from_slice(b"IHDR");
        header[16..20].copy_from_slice(&64_u32.to_be_bytes());
        header[20..24].copy_from_slice(&64_u32.to_be_bytes());
        assert!(png(&header).is_ok());
        header[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(png(&header).is_err());
        assert!(read_png(std::io::repeat(0).take(MAX_BYTES as u64 + 1)).is_err());
        assert!(png(b"<svg>not a PNG</svg>").is_err());
    }
}
