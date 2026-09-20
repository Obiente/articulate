//! Pinned artifact transfers retain their saved, length-checked prefix across reconnects.
use super::{Event, check_cancel, progress};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
    time::{Duration, Instant},
};

pub(super) struct Artifact<'a> {
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
pub(super) fn fetch(
    artifact: Artifact<'_>,
    cancel: &Arc<AtomicBool>,
    events: &Sender<Event>,
) -> Result<()> {
    transfer(artifact, cancel, events, Policy::default())
}
fn transfer(
    a: Artifact<'_>,
    cancel: &Arc<AtomicBool>,
    events: &Sender<Event>,
    policy: Policy,
) -> Result<()> {
    check_cancel(cancel)?;
    ensure!(
        a.size > 0 && a.sha.len() == 64 && a.sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid download manifest."
    );
    let part = a.destination.with_extension(format!("{}.part", a.sha));
    if part.exists() {
        super::install::ordinary(&part)?;
    }
    if part.metadata().is_ok_and(|m| m.len() > a.size) {
        fs::rename(
            &part,
            part.with_extension(format!("invalid-{}", crate::discord::plugin::new_token()?)),
        )?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&part)?;
    let began = Instant::now();
    let mut last_error = String::new();
    for attempt in 0..policy.attempts {
        check_cancel(cancel)?;
        let offset = file.metadata()?.len();
        if offset == a.size {
            break;
        }
        ensure!(
            began.elapsed() < Duration::from_secs(3600),
            "The download paused after one hour. Downloaded data was saved; choose Download again to resume."
        );
        if attempt > 0 {
            progress(
                events,
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
        let agent =
            super::transport::agent(cancel.clone(), policy.idle, a.url.starts_with("https://"));
        let mut request = agent
            .get(a.url)
            .header("Accept-Encoding", "identity")
            .header("Connection", "close");
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
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
        let reset = validate_headers(status, response.headers(), offset, a.size)?;
        if reset {
            file.set_len(0)?;
        }
        file.seek(SeekFrom::End(0))?;
        let mut received = if reset { 0 } else { offset };
        let mut last = received;
        progress(events, a.stage, Some(received as f32 / a.size as f32));
        let mut exceeded_size = false;
        let outcome = (|| -> Result<()> {
            let mut reader = response.body_mut().as_reader();
            let mut buffer = vec![0; 256 * 1024];
            loop {
                check_cancel(cancel)?;
                ensure!(
                    began.elapsed() < Duration::from_secs(3600),
                    "The transfer time limit was reached"
                );
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
                    progress(events, a.stage, Some(received as f32 / a.size as f32));
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
    progress(events, "Verifying downloaded files", None);
    if let Err(error) = super::install::verified(&part, a.size, a.sha, cancel) {
        check_cancel(cancel)?;
        // Keep a failed artifact for diagnosis, but permit a clean retry next time.
        let invalid =
            part.with_extension(format!("invalid-{}", crate::discord::plugin::new_token()?));
        fs::rename(&part, invalid)?;
        return Err(error).context(
            "Downloaded data failed verification. Choose Download again to fetch a fresh copy.",
        );
    }
    if a.destination.exists() {
        let backup = a
            .destination
            .with_extension(format!("replaced-{}", crate::discord::plugin::new_token()?));
        fs::rename(a.destination, &backup)?;
        if let Err(error) = fs::rename(&part, a.destination) {
            let _ = fs::rename(&backup, a.destination);
            return Err(error.into());
        }
        let _ = fs::remove_file(backup);
    } else {
        fs::rename(&part, a.destination)?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };
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
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data[..1000]).unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).contains("range: bytes=1000-"));
            write!(socket,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 1000-{}/{}\r\nConnection: close\r\n\r\n",size-1000,size-1,size).unwrap();
            socket.write_all(&data[1000..]).unwrap();
        });
        let path = directory.join("model");
        let (tx, _) = mpsc::channel();
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &tx,
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).contains("range: bytes=400-"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data).unwrap();
        });
        let (tx, _) = mpsc::channel();
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &tx,
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(socket,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",size-400,size-1,size).unwrap();
        });
        let (tx, _) = mpsc::channel();
        let result = transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &tx,
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
        let (ready, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
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
        let (tx, _) = mpsc::channel();
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
            &tx,
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
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            first.write_all(&data[..400]).unwrap();
            let (mut second, _) = listener.accept().unwrap();
            assert!(request(&mut second).contains("range: bytes=400-"));
            write!(second,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 400-{}/{}\r\nConnection: close\r\n\r\n",size-400,size-1,size).unwrap();
            second.write_all(&data[400..]).unwrap();
        });
        let (tx, _) = mpsc::channel();
        transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &tx,
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
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            socket.write_all(&data).unwrap();
        });
        let (tx, _) = mpsc::channel();
        let result = transfer(
            Artifact {
                url: &url,
                destination: &path,
                size: size as u64,
                sha: &sha,
                stage: "Testing",
            },
            &Arc::new(AtomicBool::new(false)),
            &tx,
            policy(),
        );
        server.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("verification"));
        assert!(!path.exists());
        assert!(!path.with_extension(format!("{sha}.part")).exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
