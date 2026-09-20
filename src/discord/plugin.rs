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
        pcm: super::pcm::Hub::default(),
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
                // Winsock accept inherits the listener's nonblocking mode.
                // Request reads use bounded blocking timeouts; otherwise a
                // fragmented header/body is rejected as soon as read would block.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                let result = receive(&mut stream, token);
                let mut response = String::new();
                let status = match result {
                    Ok(Incoming::Voice(observation, timestamp))
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
                    Ok(Incoming::Voice(_, _)) => "429 Too Many Requests",
                    Ok(Incoming::Control) => {
                        let observation = shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .snapshot
                            .observation
                            .clone();
                        response = shared.pcm.control(observation.as_ref()).to_string();
                        "200 OK"
                    }
                    Ok(Incoming::Pcm(bytes)) => {
                        let now = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_micros() as u64);
                        if now.is_ok_and(|now| {
                            shared.pcm.receive(&bytes, now, Instant::now()).is_ok()
                        }) {
                            "204 No Content"
                        } else {
                            "400 Bad Request"
                        }
                    }
                    Err(_) => "400 Bad Request",
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                let _ = stream.write_all(response.as_bytes());
                finish_response(&mut stream);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_client(&listener, shared)
            }
            Err(_) => {
                shared.unavailable("The Discord plugin connection stopped.");
                break;
            }
        }
    }
}

fn finish_response(stream: &mut TcpStream) {
    // Header/authentication failures can leave request-body bytes unread. On
    // Windows, dropping that socket immediately sends RST and may discard the
    // HTTP error response. Send FIN for our completed response first, then
    // consume bounded pending input until the client closes its side.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(25)));
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut remaining = HEADER_LIMIT + BODY_LIMIT;
    let mut buffer = [0_u8; 4096];
    while remaining > 0 && Instant::now() < deadline {
        let amount = remaining.min(buffer.len());
        match stream.read(&mut buffer[..amount]) {
            Ok(0) | Err(_) => break,
            Ok(count) => remaining -= count,
        }
    }
}

fn wait_for_client(listener: &TcpListener, shared: &Shared) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{POLLRDNORM, WSAPOLLFD, WSAPoll};
        let mut socket = WSAPOLLFD {
            fd: listener.as_raw_socket() as usize,
            events: POLLRDNORM,
            revents: 0,
        };
        // Wake immediately for the next PCM packet, with bounded shutdown latency.
        // A fixed sleep between connections caps sequential audio throughput.
        if unsafe { WSAPoll(&mut socket, 1, 50) } < 0 {
            shared.wait(Duration::from_millis(20));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = listener;
        shared.wait(Duration::from_millis(1));
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Voice,
    Pcm,
    Control,
}
enum Incoming {
    Voice(Observation, u64),
    Pcm(Vec<u8>),
    Control,
}
fn request_headers(headers: &str, token: &str) -> Result<(Route, usize)> {
    let mut lines = headers.split("\r\n");
    let route = match lines.next() {
        Some("POST /voice HTTP/1.1") => Route::Voice,
        Some("POST /pcm HTTP/1.1") => Route::Pcm,
        Some("GET /capture HTTP/1.1") => Route::Control,
        _ => bail!("Unsupported request"),
    };
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
        fields.get("content-type")
            == Some(&if route == Route::Pcm {
                "application/octet-stream"
            } else {
                "application/json"
            }),
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
    ensure!(
        match route {
            Route::Voice => (1..=BODY_LIMIT).contains(&length),
            Route::Pcm => (64..=super::pcm::MAX_PACKET).contains(&length),
            Route::Control => length == 0,
        },
        "Payload limit exceeded"
    );
    Ok((route, length))
}

#[cfg(test)]
fn parse_headers(headers: &str, token: &str) -> Result<usize> {
    let (route, length) = request_headers(headers, token)?;
    ensure!(route == Route::Voice, "Not a voice request");
    Ok(length)
}
fn receive(stream: &mut TcpStream, token: &str) -> Result<Incoming> {
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut bytes = Vec::new();
    let (split, route, length) = loop {
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
            let (route, length) = request_headers(std::str::from_utf8(&bytes[..split])?, token)?;
            break (split + 4, route, length);
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
    match route {
        Route::Voice => {
            decode(&bytes[split..], now, Instant::now()).map(|(o, t)| Incoming::Voice(o, t))
        }
        Route::Pcm => Ok(Incoming::Pcm(bytes[split..].to_vec())),
        Route::Control => Ok(Incoming::Control),
    }
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
    #[serde(default)]
    avatar: Option<super::avatar::Avatar>,
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
                avatar: p.avatar,
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
    fn avatar_metadata_is_optional_and_bound_to_the_participant() {
        let mut body = json!({"version":1,"observed_ms":1000,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Example","speaking":true,"is_self":false,
                "avatar":{"user_id":"456","hash":"0123456789abcdef0123456789abcdef"}}]});
        let (observation, _) = decode(body.to_string().as_bytes(), 1100, Instant::now()).unwrap();
        assert_eq!(
            observation.participants[0].avatar.as_ref().unwrap().user_id,
            "456"
        );
        body["participants"][0]["avatar"]["user_id"] = json!("789");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body["participants"][0]["avatar"]["user_id"] = json!("456");
        body["participants"][0]["avatar"]["url"] = json!("https://example.invalid/avatar.png");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body["participants"][0]
            .as_object_mut()
            .unwrap()
            .remove("avatar");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_ok());
    }

    #[test]
    fn authenticated_http_publishes_expires_and_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let shared = Arc::new(Shared {
            pcm: super::super::pcm::Hub::default(),
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
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let request = format!(
            "POST /voice HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
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

    #[test]
    fn real_http_pcm_requires_auth_arming_nonce_and_remote_identity() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let shared = Arc::new(Shared {
            pcm: super::super::pcm::Hub::default(),
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
        // RAII stops the socket worker even if an assertion below fails.
        let connection = Connection {
            shared,
            worker: Mutex::new(Some(worker)),
        };
        let exchange_parts = |method: &str,
                              route: &str,
                              auth: &str,
                              body: &[u8],
                              fragmented: bool| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.set_nodelay(true).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let content_type = if route == "/pcm" {
                "application/octet-stream"
            } else {
                "application/json"
            };
            let mut request = format!("{method} {route} HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Bearer {auth}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
            let header_length = request.len();
            request.extend_from_slice(body);
            if fragmented {
                // Exercise delayed header and body delivery separately. Windows
                // accepted sockets inherit nonblocking mode unless reset by serve.
                stream.write_all(&request[..13]).unwrap();
                thread::sleep(Duration::from_millis(15));
                stream.write_all(&request[13..header_length]).unwrap();
                thread::sleep(Duration::from_millis(15));
                stream.write_all(&request[header_length..]).unwrap();
            } else {
                stream.write_all(&request).unwrap();
            }
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        };
        let exchange = |method: &str, route: &str, auth: &str, body: &[u8]| {
            exchange_parts(method, route, auth, body, false)
        };
        let control = || {
            let response = exchange("GET", "/capture", &token, &[]);
            assert!(response.starts_with("HTTP/1.1 200"));
            serde_json::from_str::<Value>(response.split_once("\r\n\r\n").unwrap().1).unwrap()
        };
        let packet = |capture: u64, user: u64, sequence: u64| {
            // One realistic 20ms, 48kHz mono callback per HTTP request.
            let mut bytes = vec![0_u8; 64 + 960 * 2];
            bytes[..4].copy_from_slice(b"APCM");
            for (at, value) in [(4, 1_u16), (6, 64), (44, 1)] {
                bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64;
            for (at, value) in [
                (8, 1_u64),
                (16, sequence),
                (24, now),
                (32, user),
                (52, capture),
            ] {
                bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
            }
            bytes[40..44].copy_from_slice(&48_000_u32.to_le_bytes());
            bytes[48..52].copy_from_slice(&960_u32.to_le_bytes());
            bytes[64..68].copy_from_slice(&[1, 0, 255, 255]);
            bytes
        };
        let invalid_key = "z".repeat(64);
        assert!(exchange("GET", "/capture", &invalid_key, &[]).starts_with("HTTP/1.1 400"));
        assert!(!connection.shared.pcm.ready());
        assert!(
            exchange("POST", "/pcm", &invalid_key, &packet(1, 456, 1)).starts_with("HTTP/1.1 400")
        );
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let body = json!({"version":1,"observed_ms":now,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Remote example","speaking":true,"is_self":false},
                {"id":"789","name":"Local example","speaking":true,"is_self":true}]})
        .to_string();
        assert!(exchange("POST", "/voice", &token, body.as_bytes()).starts_with("HTTP/1.1 204"));
        assert_eq!(control()["active"], false);
        assert!(
            connection.native_audio_ready(),
            "Native readiness must precede arming and PCM"
        );
        assert!(exchange("POST", "/pcm", &token, &packet(1, 456, 1)).starts_with("HTTP/1.1 400"));
        let observation = connection.snapshot().observation.unwrap();
        let capture = connection.shared.pcm.begin(&observation).unwrap();
        let armed = control();
        assert_eq!(armed["active"], true);
        assert_eq!(armed["channel_id"], "123");
        assert_eq!(armed["participants"], json!(["456"]));
        let nonce = armed["capture_id"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_ne!(nonce, 0);
        assert!(
            exchange("POST", "/pcm", &token, &packet(nonce ^ 1, 456, 1))
                .starts_with("HTTP/1.1 400")
        );
        assert!(
            exchange("POST", "/pcm", &token, &packet(nonce, 789, 1)).starts_with("HTTP/1.1 400")
        );
        let transfer_started = Instant::now();
        for sequence in 1..=200 {
            let response = exchange_parts(
                "POST",
                "/pcm",
                &token,
                &packet(nonce, 456, sequence),
                sequence == 1,
            );
            assert!(
                response.starts_with("HTTP/1.1 204"),
                "PCM packet {sequence} was rejected: {response}"
            );
        }
        assert!(
            transfer_started.elapsed() < Duration::from_secs(2),
            "Sequential PCM transport cannot keep up with live audio"
        );
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 200);
        assert_eq!(frames[0].samples.len(), 960);
        assert_eq!(&frames[0].samples[..2], &[1, -1]);
        assert_eq!(frames[0].attribution.speakers[0].name, "Remote example");
        assert_eq!(frames[0].attribution.speakers[0].id, "456");
        assert!(
            exchange("POST", "/pcm", &token, &packet(nonce, 456, 200)).starts_with("HTTP/1.1 400")
        );
        assert!(
            capture.drain().is_err(),
            "Replayed sequence must stop capture"
        );
        drop(capture);
        assert_eq!(control()["active"], false);
        assert!(
            exchange("POST", "/pcm", &token, &packet(nonce, 456, 201)).starts_with("HTTP/1.1 400")
        );
        drop(connection);
        assert!(TcpStream::connect(address).is_err());
    }
}
