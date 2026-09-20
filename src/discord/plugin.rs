//! Authenticated, opt-in loopback metadata transport for the Vencord companion.
use super::*;
use std::{net::TcpListener, time::SystemTime};

const ENDPOINT: &str = "127.0.0.1:9223";
const BODY_LIMIT: usize = 128 * 1024;
const HEADER_LIMIT: usize = 4096;
const STALE: Duration = Duration::from_millis(750);

pub fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn new_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                algorithm: *mut std::ffi::c_void,
                bytes: *mut u8,
                count: u32,
                flags: u32,
            ) -> i32;
        }
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG, with no algorithm handle.
        ensure!(
            unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 32, 2) } >= 0,
            "Could not generate a pairing key"
        );
    }
    #[cfg(not(windows))]
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn start(token: String) -> Result<Arc<Connection>> {
    ensure!(valid_token(&token), "Invalid pairing key");
    let listener =
        TcpListener::bind(ENDPOINT).context("The Discord plugin port is already in use")?;
    listener.set_nonblocking(true)?;
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            snapshot: Snapshot {
                status: Status::Connecting,
                observation: None,
            },
            history: VecDeque::new(),
            generation: 0,
            channel: None,
        }),
        stopped: AtomicBool::new(false),
        wake: Condvar::new(),
        wait_lock: Mutex::new(()),
    });
    let worker_shared = shared.clone();
    let worker = thread::Builder::new()
        .name("discord-plugin".into())
        .spawn(move || serve(listener, &token, &worker_shared))?;
    Ok(Arc::new(Connection {
        shared,
        worker: Mutex::new(Some(worker)),
    }))
}

fn serve(listener: TcpListener, token: &str, shared: &Shared) {
    let mut last_good: Option<Instant> = None;
    let mut last_timestamp = 0;
    let mut stale = false;
    while !shared.stopped() {
        if last_good.is_some_and(|at| at.elapsed() > STALE) && !stale {
            shared.unavailable("Waiting for the Discord plugin.");
            stale = true;
        }
        match listener.accept() {
            Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                let result = receive(&mut stream, token);
                let status = match result {
                    Ok((observation, timestamp))
                        if timestamp > last_timestamp
                            && last_good
                                .is_none_or(|at| at.elapsed() >= Duration::from_millis(40)) =>
                    {
                        last_timestamp = timestamp;
                        last_good = Some(Instant::now());
                        stale = false;
                        shared.publish(observation);
                        "204 No Content"
                    }
                    Ok(_) => "429 Too Many Requests",
                    Err(_) => "400 Bad Request",
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                shared.wait(Duration::from_millis(20))
            }
            Err(_) => {
                shared.unavailable("The Discord plugin connection stopped.");
                break;
            }
        }
    }
}

fn token_matches(left: &str, right: &str) -> bool {
    if left.len() != 64 || right.len() != 64 {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |different, (a, b)| different | (a ^ b))
        == 0
}

fn parse_headers(headers: &str, token: &str) -> Result<usize> {
    let mut lines = headers.split("\r\n");
    ensure!(
        lines.next() == Some("POST /voice HTTP/1.1"),
        "Unsupported request"
    );
    let mut fields = std::collections::HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').context("Malformed header")?;
        ensure!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid header"
        );
        ensure!(
            fields
                .insert(name.to_ascii_lowercase(), value.trim())
                .is_none(),
            "Duplicate header"
        );
    }
    ensure!(fields.get("host") == Some(&ENDPOINT), "Invalid host");
    ensure!(
        !fields.contains_key("origin") && !fields.contains_key("transfer-encoding"),
        "Unsupported client"
    );
    ensure!(
        fields.get("content-type") == Some(&"application/json"),
        "Invalid content type"
    );
    let supplied = fields
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    ensure!(token_matches(supplied, token), "Invalid pairing key");
    let length = fields
        .get("content-length")
        .context("Missing content length")?
        .parse::<usize>()?;
    ensure!((1..=BODY_LIMIT).contains(&length), "Payload limit exceeded");
    Ok(length)
}

fn receive(stream: &mut TcpStream, token: &str) -> Result<(Observation, u64)> {
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut bytes = Vec::new();
    let (split, length) = loop {
        ensure!(
            Instant::now() < deadline && bytes.len() < HEADER_LIMIT,
            "Header limit exceeded"
        );
        let mut buffer = [0_u8; 1024];
        let count = stream.read(&mut buffer)?;
        ensure!(count > 0, "Incomplete request");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            ensure!(split <= HEADER_LIMIT, "Header limit exceeded");
            let length = parse_headers(std::str::from_utf8(&bytes[..split])?, token)?;
            break (split + 4, length);
        }
    };
    while bytes.len() < split + length {
        ensure!(Instant::now() < deadline, "Request expired");
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer)?;
        ensure!(count > 0, "Incomplete request");
        bytes.extend_from_slice(&buffer[..count]);
    }
    ensure!(bytes.len() == split + length, "Unexpected request bytes");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis() as u64;
    decode(&bytes[split..], now, Instant::now())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    observed_ms: u64,
    channel_id: Option<String>,
    participants: Vec<PluginParticipant>,
    valid: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginParticipant {
    id: String,
    name: String,
    speaking: bool,
    is_self: bool,
}

fn decode(bytes: &[u8], now: u64, received: Instant) -> Result<(Observation, u64)> {
    let payload: Payload = serde_json::from_slice(bytes)?;
    ensure!(
        payload.version == 1
            && payload.observed_ms <= now + 50
            && now.saturating_sub(payload.observed_ms) <= 250,
        "Stale plugin snapshot"
    );
    let sample = Sample {
        time: 0.0,
        channel_id: payload.channel_id,
        participants: payload
            .participants
            .into_iter()
            .map(|p| Participant {
                id: p.id,
                name: p.name,
                speaking: p.speaking,
                is_self: p.is_self,
            })
            .collect(),
        valid: payload.valid,
    };
    ensure!(valid_sample(&sample), "Invalid voice metadata");
    ensure!(
        sample.participants.iter().filter(|p| p.is_self).count() <= 1,
        "Ambiguous local identity"
    );
    ensure!(
        sample.valid || (sample.channel_id.is_none() && sample.participants.is_empty()),
        "Invalid unavailable snapshot"
    );
    Ok((
        Observation {
            at: received
                .checked_sub(Duration::from_millis(
                    now.saturating_sub(payload.observed_ms),
                ))
                .unwrap_or(received),
            generation: 0,
            channel_id: sample.channel_id,
            participants: sample.participants,
            valid: sample.valid,
        },
        payload.observed_ms,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_keys_are_random_and_valid() {
        let first = new_token().unwrap();
        assert!(valid_token(&first));
        assert_ne!(first, new_token().unwrap());
        assert!(token_matches(&first, &first));
        assert!(!token_matches(&first, &"0".repeat(64)));
        assert!(!valid_token("short"));
    }

    #[test]
    fn rejects_browser_remote_unauthenticated_and_ambiguous_requests() {
        let token = "a".repeat(64);
        let header = format!(
            "POST /voice HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 20"
        );
        assert_eq!(parse_headers(&header, &token).unwrap(), 20);
        for altered in [
            header.replace(ENDPOINT, "localhost:9223"),
            format!("{header}\r\nOrigin: https://discord.com"),
            format!("{header}\r\nContent-Length: 20"),
            format!("{header}\r\nTransfer-Encoding: chunked"),
            header.replace(&token, &"b".repeat(64)),
            header.replace("Content-Length: 20", "Content-Length: 9999999"),
        ] {
            assert!(parse_headers(&altered, &token).is_err());
        }
    }

    #[test]
    fn metadata_rejects_stale_duplicate_and_unknown_fields() {
        let mut body = json!({"version":1,"observed_ms":1000,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Example","speaking":true,"is_self":false}]});
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_ok());
        assert!(decode(body.to_string().as_bytes(), 1300, Instant::now()).is_err());
        let duplicate = body["participants"][0].clone();
        body["participants"].as_array_mut().unwrap().push(duplicate);
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body["participants"].as_array_mut().unwrap().pop();
        body["audio"] = json!("unaccepted");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
    }

    #[test]
    fn authenticated_http_publishes_expires_and_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                snapshot: Snapshot {
                    status: Status::Connecting,
                    observation: None,
                },
                history: VecDeque::new(),
                generation: 0,
                channel: None,
            }),
            stopped: AtomicBool::new(false),
            wake: Condvar::new(),
            wait_lock: Mutex::new(()),
        });
        let token = new_token().unwrap();
        let worker_token = token.clone();
        let worker_shared = shared.clone();
        let worker = thread::spawn(move || serve(listener, &worker_token, &worker_shared));
        let connection = Connection {
            shared,
            worker: Mutex::new(Some(worker)),
        };
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let body = json!({"version":1,"observed_ms":now,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Example","speaking":true,"is_self":false}]})
        .to_string();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        write!(stream, "POST /voice HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 204"));
        assert!(matches!(connection.snapshot().status, Status::Ready));
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(connection.snapshot().status, Status::Ready) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connection.snapshot().observation.is_none());
        let shutdown = Instant::now();
        drop(connection);
        assert!(shutdown.elapsed() < Duration::from_secs(1));
        assert!(TcpStream::connect(address).is_err());
    }
}
