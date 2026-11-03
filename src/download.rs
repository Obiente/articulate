//! Pinned artifact transfers retain their saved, length-checked prefix across reconnects.
fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cancel.load(std::sync::atomic::Ordering::Acquire),
        "Download cancelled"
    );
    Ok(())
}
#[path = "polish/transport.rs"]
mod transport;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

static TRANSFERS: std::sync::RwLock<()> = std::sync::RwLock::new(());

struct DirectoryLock {
    file: Option<fs::File>,
    path: std::path::PathBuf,
}
impl DirectoryLock {
    fn acquire(path: &Path) -> Result<Self> {
        let path = path.to_owned();
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let file = options
            .open(&path)
            .context("Another download or cleanup may be active in this folder")?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}
impl Drop for DirectoryLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

/// Remove abandoned prefixes only while no managed transfer is running.
pub fn prune_stale(directory: &Path, age: Duration) -> Result<()> {
    let _guard = cleanup_guard(&TRANSFERS)?;
    prune_files(directory, age)
}
fn cleanup_guard(lock: &std::sync::RwLock<()>) -> Result<std::sync::RwLockWriteGuard<'_, ()>> {
    lock.try_write().or_else(|error| match error {
        std::sync::TryLockError::Poisoned(error) => Ok(error.into_inner()),
        std::sync::TryLockError::WouldBlock => Err(anyhow::anyhow!("A download is active")),
    })
}
fn prune_files(directory: &Path, age: Duration) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && (path.extension().is_some_and(|e| e == "part")
                || path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".invalid-")))
            && metadata
                .modified()?
                .elapsed()
                .is_ok_and(|elapsed| elapsed > age)
        {
            let Some(name) = path.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            let base = name.rsplit_once('.').map_or(name, |(base, _)| base);
            let Ok(_lock) =
                DirectoryLock::acquire(&directory.join(format!("{base}.download.lock")))
            else {
                continue;
            };
            ordinary(&path)?;
            fs::remove_file(&path)?;
            let _ = fs::remove_file(path.with_extension("identity"));
        }
    }
    Ok(())
}

pub(crate) struct Artifact<'a> {
    pub url: &'a str,
    pub destination: &'a Path,
    pub size: u64,
    pub sha: &'a str,
    pub stage: &'a str,
}
#[derive(Clone, Copy)]
struct Policy {
    attempts: usize,
    idle: Duration,
    backoff: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            attempts: 6,
            idle: Duration::from_secs(30),
            backoff: Duration::from_secs(1),
        }
    }
}
pub(crate) fn fetch(
    artifact: Artifact<'_>,
    cancel: &Arc<AtomicBool>,
    progress: &mut impl FnMut(&str, Option<f32>),
) -> Result<()> {
    transfer(artifact, cancel, progress, Policy::default())
}
fn transfer(
    a: Artifact<'_>,
    cancel: &Arc<AtomicBool>,
    progress: &mut impl FnMut(&str, Option<f32>),
    policy: Policy,
) -> Result<()> {
    let _guard = TRANSFERS.read().unwrap_or_else(|error| error.into_inner());
    check_cancel(cancel)?;
    ensure!(
        a.url.starts_with("https://") || cfg!(test) && a.url.starts_with("http://127.0.0.1:"),
        "Downloads require HTTPS"
    );
    ensure!(
        a.size > 0 && a.sha.len() == 64 && a.sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid download manifest."
    );
    let part = a.destination.with_extension(format!("{}.part", a.sha));
    let _directory_lock = DirectoryLock::acquire(&a.destination.with_extension("download.lock"))?;
    if part.exists() {
        ordinary(&part)?;
    }
    if part.metadata().is_ok_and(|m| m.len() > a.size) {
        quarantine(&part)?;
    }
    let identity_path = part.with_extension("identity");
    let mut identity = fs::read_to_string(&identity_path).ok();
    if identity.is_none() && part.exists() {
        fs::remove_file(&part)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&part)?;
    let mut last_error = String::new();
    for attempt in 0..policy.attempts {
        check_cancel(cancel)?;
        if identity.is_none() && file.metadata()?.len() < a.size {
            file.set_len(0)?;
        }
        let offset = file.metadata()?.len();
        if offset == a.size {
            break;
        }
        if attempt > 0 {
            progress(
                &format!(
                    "Connection interrupted. Resuming download ({}/{})",
                    attempt + 1,
                    policy.attempts
                ),
                Some(offset as f32 / a.size as f32),
            );
            let wait = Instant::now();
            while wait.elapsed() < policy.backoff.saturating_mul((attempt as u32).min(5)) {
                check_cancel(cancel)?;
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let agent = transport::agent(cancel.clone(), policy.idle, a.url.starts_with("https://"));
        let mut request = agent
            .get(a.url)
            .header("Accept-Encoding", "identity")
            .header("Connection", "close");
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
            if let Some(value) = &identity {
                request = request.header("If-Range", value);
            }
        }
        let mut response = match request.call() {
            Ok(r) => r,
            Err(_) => {
                last_error = "The download server could not be reached".into();
                continue;
            }
        };
        let status = response.status().as_u16();
        if matches!(status, 408 | 429 | 500 | 502 | 503 | 504) {
            last_error = format!("The download server is temporarily unavailable ({status})");
            continue;
        }
        let remote = response
            .headers()
            .get("etag")
            .filter(|v| !v.as_bytes().starts_with(b"W/"))
            .or_else(|| response.headers().get("last-modified"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if status == 206 && offset > 0 && (identity.is_none() || remote != identity) {
            file.set_len(0)?;
            identity = None;
            let _ = fs::remove_file(&identity_path);
            continue;
        }
        let reset = validate_headers(status, response.headers(), offset, a.size)?;
        if reset {
            file.set_len(0)?;
        }
        identity = remote;
        if let Some(value) = &identity {
            fs::write(&identity_path, value)?;
        } else {
            let _ = fs::remove_file(&identity_path);
        }
        file.seek(SeekFrom::End(0))?;
        let mut received = if reset { 0 } else { offset };
        let mut last = received;
        progress(a.stage, Some(received as f32 / a.size as f32));
        let mut exceeded_size = false;
        let outcome = (|| -> Result<()> {
            let mut reader = response.body_mut().as_reader();
            let mut buffer = vec![0; 256 * 1024];
            loop {
                check_cancel(cancel)?;
                let count = reader
                    .read(&mut buffer)
                    .context("The download connection was interrupted")?;
                if count == 0 {
                    break;
                }
                exceeded_size = received + count as u64 > a.size;
                ensure!(!exceeded_size, "The download exceeded its pinned size.");
                file.write_all(&buffer[..count])?;
                received += count as u64;
                if received - last >= 1024 * 1024 || received == a.size {
                    progress(a.stage, Some(received as f32 / a.size as f32));
                    last = received;
                }
            }
            ensure!(
                received == a.size,
                "The download connection closed before the file was complete"
            );
            Ok(())
        })();
        file.sync_all()?;
        check_cancel(cancel)?;
        ensure!(
            !exceeded_size,
            "The download server sent more data than expected. The file was not installed."
        );
        if outcome.is_ok() {
            break;
        }
        last_error = "The download connection was interrupted".into();
    }
    file.sync_all()?;
    let received = file.metadata()?.len();
    drop(file);
    ensure!(
        received == a.size,
        "{last_error}. Saved {:.1} MB of {:.1} MB. Choose Download again to resume.",
        received as f64 / 1e6,
        a.size as f64 / 1e6
    );
    progress("Verifying downloaded files", None);
    if let Err(error) = verified(&part, a.size, a.sha, cancel) {
        check_cancel(cancel)?;
        // Keep a failed artifact for diagnosis, but permit a clean retry next time.
        quarantine(&part)?;
        let _ = fs::remove_file(&identity_path);
        return Err(error).context(
            "Downloaded data failed verification. Choose Download again to fetch a fresh copy.",
        );
    }
    let _ = fs::remove_file(identity_path);
    fs::rename(&part, a.destination)?;
    Ok(())
}
fn quarantine(part: &Path) -> Result<()> {
    let stem = part
        .file_stem()
        .context("Missing artifact name")?
        .to_string_lossy();
    let base = stem
        .rsplit_once('.')
        .map_or(stem.as_ref(), |(base, _)| base);
    for entry in fs::read_dir(part.parent().context("Missing artifact directory")?)? {
        let path = entry?.path();
        if path
            .file_stem()
            .and_then(|n| n.to_str())
            .and_then(|stem| stem.rsplit_once('.'))
            .is_some_and(|(candidate, _)| candidate == base)
            && path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().starts_with("invalid-"))
        {
            ordinary(&path)?;
            fs::remove_file(path)?;
        }
    }
    fs::rename(part, part.with_extension("invalid-latest"))?;
    Ok(())
}
fn validate_headers(
    status: u16,
    headers: &ureq::http::HeaderMap,
    offset: u64,
    size: u64,
) -> Result<bool> {
    ensure!(
        matches!(status, 200 | 206),
        "The download server returned HTTP {status}. Your partial download was kept; retry later."
    );
    ensure!(
        headers
            .get("content-encoding")
            .is_none_or(|v| v == "identity"),
        "The download server returned an unexpected encoding."
    );
    let reset = status == 200;
    let remaining = if reset { size } else { size - offset };
    if let Some(length) = headers.get("content-length") {
        ensure!(
            length.to_str()?.parse::<u64>()? == remaining,
            "The download server returned an unexpected file length. Your partial download was kept."
        );
    }
    if status == 206 {
        let expected = format!("bytes {offset}-{}/{}", size - 1, size);
        ensure!(
            headers.get("content-range").and_then(|v| v.to_str().ok()) == Some(expected.as_str()),
            "The download server returned the wrong byte range. Your partial download was kept."
        );
    } else {
        ensure!(
            !headers.contains_key("content-range"),
            "Unexpected byte range in a full download."
        );
    }
    Ok(reset)
}

pub(crate) fn ordinary(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "The download path contains an unexpected file type."
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "The download path contains an unexpected linked file."
        );
    }
    Ok(())
}
pub(crate) fn verified(path: &Path, size: u64, expected: &str, cancel: &AtomicBool) -> Result<()> {
    ordinary(path)?;
    let mut file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() == size,
        "The downloaded file has the wrong size. Download it again."
    );
    let mut buffer = vec![0; 256 * 1024];
    let mut hash = Sha256::new();
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == expected,
        "The downloaded file failed verification. Download it again."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        net::{TcpListener, TcpStream},
        thread,
    };
    #[test]
    fn cleanup_is_blocked_during_transfer_and_keeps_finished_files() {
        let (directory, _, _) = fixture();
        fs::write(directory.join("old.part"), b"partial").unwrap();
        fs::write(directory.join("model.hash.invalid-old"), b"invalid").unwrap();
        fs::write(directory.join("finished.gguf"), b"complete").unwrap();
        {
            let _guard = TRANSFERS.write().unwrap();
            assert!(prune_stale(&directory, Duration::ZERO).is_err());
            assert!(directory.join("old.part").exists());
        }
        std::thread::sleep(Duration::from_millis(20));
        let _guard = TRANSFERS.write().unwrap_or_else(|e| e.into_inner());
        prune_files(&directory, Duration::ZERO).unwrap();
        assert!(!directory.join("model.hash.invalid-old").exists());
        assert!(!directory.join("old.part").exists());
        assert!(directory.join("finished.gguf").exists());
        fs::remove_dir_all(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn directory_lock_excludes_another_handle() {
        let (directory, _, _) = fixture();
        let lock = DirectoryLock::acquire(&directory.join("model.download.lock")).unwrap();
        assert!(DirectoryLock::acquire(&directory.join("model.download.lock")).is_err());
        drop(lock);
        drop(DirectoryLock::acquire(&directory.join("model.download.lock")).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn poisoned_cleanup_lock_recovers() {
        let lock = Arc::new(std::sync::RwLock::new(()));
        let other = lock.clone();
        let _ = std::thread::spawn(move || {
            let _guard = other.write().unwrap();
            panic!("synthetic lock failure");
        })
        .join();
        assert!(cleanup_guard(&lock).is_ok());
    }
    #[test]
    fn quarantine_is_bounded() {
        let (directory, _, sha) = fixture();
        let part = directory.join(format!("fixture.{sha}.part"));
        fs::write(part.with_extension("invalid-old"), b"old").unwrap();
        fs::write(
            directory.join("fixture.previoushash.invalid-old"),
            b"old version",
        )
        .unwrap();
        let unrelated = directory.join("fixture.other.hash.invalid-old");
        fs::write(&unrelated, b"unrelated").unwrap();
        for _ in 0..3 {
            fs::write(&part, b"bad").unwrap();
            quarantine(&part).unwrap();
        }
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        assert!(unrelated.exists());
        fs::remove_dir_all(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn unrelated_artifacts_can_lock_concurrently() {
        let (directory, _, _) = fixture();
        let a = directory.join("a.download.lock");
        let b = directory.join("b.download.lock");
        let first = DirectoryLock::acquire(&a).unwrap();
        let second = DirectoryLock::acquire(&b).unwrap();
        assert!(DirectoryLock::acquire(&a).is_err());
        drop((first, second));
        fs::remove_dir_all(directory).unwrap();
    }
    fn request(socket: &mut TcpStream) -> String {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut one = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut one).unwrap();
            bytes.push(one[0]);
            assert!(bytes.len() < 8192);
        }
        String::from_utf8(bytes).unwrap().to_lowercase()
    }
    fn fixture() -> (std::path::PathBuf, Vec<u8>, String) {
        let directory = std::env::temp_dir().join(format!(
            "articulate-download-{}",
            crate::discord::plugin::new_token().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let data = b"A synthetic pinned download. ".repeat(300);
        let sha = format!("{:x}", Sha256::digest(&data));
        (directory, data, sha)
    }
    fn policy() -> Policy {
        Policy {
            attempts: 3,
            idle: Duration::from_millis(350),
            backoff: Duration::ZERO,
        }
    }
    #[test]
    fn truncated_download_resumes_exact_range_and_verifies() {
        let (directory, data, sha) = fixture();
        let expected = data.clone();
        let size = data.len();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(!request(&mut socket).contains("range:"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data[..1000]).unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).contains("range: bytes=1000-"));
            write!(socket,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 1000-{}/{}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n",size-1000,size-1,size).unwrap();
            socket.write_all(&data[1000..]).unwrap();
        });
        let path = directory.join("model");
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), expected);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn changed_remote_identity_restarts_before_appending() {
        let (directory, data, sha) = fixture();
        let path = directory.join("model");
        let part = path.with_extension(format!("{sha}.part"));
        fs::write(&part, b"old prefix").unwrap();
        fs::write(part.with_extension("identity"), "\"old\"").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let expected = data.clone();
        let size = data.len();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let headers = request(&mut first);
            assert!(headers.contains("if-range: \"old\""));
            write!(first,"HTTP/1.1 206 Partial Content\r\nETag: \"new\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            assert!(!request(&mut second).contains("range:"));
            write!(second,"HTTP/1.1 200 OK\r\nETag: \"new\"\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n").unwrap();
            second.write_all(&data).unwrap();
        });
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), expected);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn ignored_range_restarts_instead_of_appending() {
        let (directory, data, sha) = fixture();
        let size = data.len();
        let expected = data.clone();
        let path = directory.join("model");
        fs::write(path.with_extension(format!("{sha}.part")), &data[..400]).unwrap();
        fs::write(
            path.with_extension(format!("{sha}.part"))
                .with_extension("identity"),
            "\"fixture\"",
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).contains("range: bytes=400-"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data).unwrap();
        });
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), expected);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn wrong_range_leaves_partial_untouched() {
        let (directory, data, sha) = fixture();
        let size = data.len();
        let path = directory.join("model");
        let part = path.with_extension(format!("{sha}.part"));
        fs::write(&part, &data[..400]).unwrap();
        fs::write(part.with_extension("identity"), "\"fixture\"").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(socket,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n",size-400,size-1,size).unwrap();
        });
        let result = transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        );
        server.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("wrong byte range"));
        assert_eq!(fs::read(part).unwrap(), data[..400]);
        assert!(!path.exists());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn stalled_body_can_be_cancelled_without_losing_prefix() {
        let (directory, data, sha) = fixture();
        let size = data.len();
        let path = directory.join("model");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let (ready, wait) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data[..400]).unwrap();
            ready.send(()).unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let stopper = thread::spawn(move || {
            wait.recv().unwrap();
            thread::sleep(Duration::from_millis(100));
            flag.store(true, std::sync::atomic::Ordering::Release);
        });
        let began = Instant::now();
        let result = transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &cancel,
            &mut |_, _| {},
            Policy::default(),
        );
        assert!(result.is_err());
        assert!(began.elapsed() < Duration::from_secs(1));
        stopper.join().unwrap();
        assert_eq!(
            fs::metadata(path.with_extension(format!("{sha}.part")))
                .unwrap()
                .len(),
            400
        );
        server.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn idle_timeout_reconnects_and_keeps_prefix() {
        let (directory, data, sha) = fixture();
        let size = data.len();
        let expected = data.clone();
        let path = directory.join("model");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            request(&mut first);
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            first.write_all(&data[..400]).unwrap();
            let (mut second, _) = listener.accept().unwrap();
            assert!(request(&mut second).contains("range: bytes=400-"));
            write!(second,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 400-{}/{}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n",size-400,size-1,size).unwrap();
            second.write_all(&data[400..]).unwrap();
        });
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), expected);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn incorrect_digest_is_never_promoted_and_retry_can_start_fresh() {
        let (directory, data, _) = fixture();
        let size = data.len();
        let sha = "0".repeat(64);
        let path = directory.join("model");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data).unwrap();
        });
        let result = transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| {},
            policy(),
        );
        server.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("verification"));
        assert!(!path.exists());
        assert!(!path.with_extension(format!("{sha}.part")).exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        assert!(unrelated.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
