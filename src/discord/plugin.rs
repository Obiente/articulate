//! Authenticated, opt-in loopback transport for the Vencord companion.
//!
//! Protocol version 2. Each TCP connection starts with a mutual HMAC-SHA256
//! challenge-response over the shared pairing key: the companion sends a nonce,
//! Articulate answers with its own nonce and a proof of the key, and only after
//! verifying that proof does the companion answer with its own proof. The key
//! itself never crosses the socket, a rogue listener learns nothing useful, and
//! the companion sends no voice metadata or audio before the server has proven
//! possession. Requests then flow over the same keep-alive connection, bound to
//! the session that authenticated it.
use super::*;
use std::{net::TcpListener, sync::atomic::AtomicUsize, time::SystemTime};

const ENDPOINT: &str = "127.0.0.1:9223";
/// Current protocol version. Companions must send exactly this version.
pub const PROTOCOL_VERSION: u32 = 2;
const BODY_LIMIT: usize = 128 * 1024;
const HEADER_LIMIT: usize = 4096;
const BATCH_PACKETS: usize = 64;
const STALE: Duration = Duration::from_millis(750);
const MAX_CONNECTIONS: usize = 8;
/// A connection with no request at all is closed after this long.
const IDLE_LIMIT: Duration = Duration::from_secs(15);
/// Once a request starts, all of its bytes must arrive within this long.
const REQUEST_LIMIT: Duration = Duration::from_millis(500);
const SERVER_LABEL: &[u8] = b"articulate-companion-v2/server";
const CLIENT_LABEL: &[u8] = b"articulate-companion-v2/client";

pub fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fills `bytes` from the system random generator.
pub fn random_bytes(bytes: &mut [u8]) -> Result<()> {
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
            unsafe {
                BCryptGenRandom(
                    std::ptr::null_mut(),
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    2,
                )
            } >= 0,
            "Could not generate random bytes"
        );
    }
    #[cfg(not(windows))]
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)?;
    Ok(())
}

pub fn new_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    random_bytes(&mut bytes).context("Could not generate a pairing key")?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str, expected: usize) -> Result<Vec<u8>> {
    ensure!(
        text.len() == expected * 2 && text.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid hexadecimal field"
    );
    (0..expected)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(Into::into))
        .collect()
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0_u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |different, (a, b)| different | (a ^ b))
            == 0
}

/// Binds the fixed loopback endpoint so that no other local socket can share it.
#[cfg(windows)]
fn bind_exclusive(address: std::net::SocketAddrV4) -> Result<TcpListener> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, IN_ADDR, IN_ADDR_0, INVALID_SOCKET, IPPROTO_TCP, SO_EXCLUSIVEADDRUSE, SOCK_STREAM,
        SOCKADDR, SOCKADDR_IN, SOL_SOCKET, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED,
        WSADATA, WSASocketW, WSAStartup, bind, listen, setsockopt,
    };
    unsafe {
        let mut data = std::mem::zeroed::<WSADATA>();
        ensure!(
            WSAStartup(0x202, &mut data) == 0,
            "Networking is unavailable"
        );
        let socket = WSASocketW(
            i32::from(AF_INET),
            SOCK_STREAM,
            IPPROTO_TCP,
            std::ptr::null(),
            0,
            WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
        );
        ensure!(
            socket != INVALID_SOCKET,
            "Could not create the Discord plugin socket"
        );
        // Owns the socket from here on, so every failure below closes it.
        let listener = TcpListener::from_raw_socket(socket as _);
        let exclusive: i32 = 1;
        ensure!(
            setsockopt(
                socket,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&exclusive as *const i32).cast(),
                std::mem::size_of::<i32>() as i32,
            ) == 0,
            "Could not reserve the Discord plugin port exclusively"
        );
        let name = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: address.port().to_be(),
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    S_addr: u32::from(*address.ip()).to_be(),
                },
            },
            sin_zero: [0; 8],
        };
        ensure!(
            bind(
                socket,
                (&name as *const SOCKADDR_IN).cast::<SOCKADDR>(),
                std::mem::size_of::<SOCKADDR_IN>() as i32,
            ) == 0,
            "The Discord plugin port is already in use"
        );
        ensure!(
            listen(socket, 16) == 0,
            "Could not listen for the Discord plugin"
        );
        Ok(listener)
    }
}

#[cfg(not(windows))]
fn bind_exclusive(address: std::net::SocketAddrV4) -> Result<TcpListener> {
    TcpListener::bind(address).context("The Discord plugin port is already in use")
}

pub fn start(token: String) -> Result<Arc<Connection>> {
    ensure!(valid_token(&token), "Invalid pairing key");
    let listener = bind_exclusive(ENDPOINT.parse()?)?;
    listener.set_nonblocking(true)?;
    let shared = Arc::new(Shared {
        pcm: if cfg!(test) {
            super::pcm::Hub::default()
        } else {
            super::pcm::Hub::with_diagnostics(
                crate::model::data_dir().join("logs/discord-audio.json"),
            )
        },
        state: Mutex::new(State {
            snapshot: Snapshot {
                audio_status: None,
                companion_revision: None,
                companion_protocol: None,
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

/// Listener-wide state shared by every connection thread.
struct Listener<'a> {
    shared: &'a Shared,
    key: Vec<u8>,
    voice: Mutex<VoiceGate>,
    connections: AtomicUsize,
}

#[derive(Default)]
struct VoiceGate {
    last_good: Option<Instant>,
}

fn serve(listener: TcpListener, token: &str, shared: &Shared) {
    let key = match unhex(token, 32) {
        Ok(key) => key,
        Err(_) => {
            shared.unavailable("Invalid pairing key.");
            return;
        }
    };
    let server = Listener {
        shared,
        key,
        voice: Mutex::new(VoiceGate::default()),
        connections: AtomicUsize::new(0),
    };
    let mut stale = false;
    thread::scope(|scope| {
        while !shared.stopped() {
            let last_good = server
                .voice
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_good;
            if last_good.is_some_and(|at| at.elapsed() > STALE) && !stale {
                shared.unavailable("Waiting for the Discord plugin.");
                stale = true;
            } else if stale && last_good.is_some_and(|at| at.elapsed() <= STALE) {
                stale = false;
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
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    if server.connections.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                        let _ = stream.write_all(
                            Reply::new("503 Service Unavailable", String::new(), true)
                                .render()
                                .as_bytes(),
                        );
                        finish_response(&mut stream);
                        continue;
                    }
                    server.connections.fetch_add(1, Ordering::AcqRel);
                    let server = &server;
                    let spawned = thread::Builder::new()
                        .name("discord-plugin-connection".into())
                        .spawn_scoped(scope, move || {
                            serve_connection(&mut stream, server);
                            server.connections.fetch_sub(1, Ordering::AcqRel);
                        });
                    if spawned.is_err() {
                        server.connections.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    wait_for_client(&listener, shared);
                }
                Err(_) => {
                    shared.unavailable("The Discord plugin connection stopped.");
                    break;
                }
            }
        }
        // Connection threads notice the stop flag within one read timeout and
        // the scope joins them before this function returns.
    });
}

/// Per-connection authentication state. A connection is trusted only after the
/// companion has verified Articulate's proof and returned its own.
#[derive(Default)]
struct Session {
    id: String,
    expected_client_proof: Option<[u8; 32]>,
    authenticated: bool,
    voice_sequence: u64,
    voice_last: Option<Instant>,
}

/// Serves requests until the client closes, an error closes, or the listener stops.
fn serve_connection(stream: &mut TcpStream, server: &Listener) {
    let mut session = Session::default();
    let mut pending = Vec::new();
    while !server.shared.stopped() {
        let received = match receive(stream, &mut pending, server.shared) {
            Ok(Some(received)) => received,
            Ok(None) => break,
            Err(_) => {
                let _ = stream.write_all(
                    Reply::new("400 Bad Request", String::new(), true)
                        .render()
                        .as_bytes(),
                );
                finish_response(stream);
                return;
            }
        };
        let reply = handle(&mut session, received, server);
        if stream.write_all(reply.render().as_bytes()).is_err() {
            break;
        }
        if reply.close {
            finish_response(stream);
            return;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
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
        // Wake immediately for the next connection, with bounded shutdown latency.
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

struct Reply {
    status: &'static str,
    body: String,
    close: bool,
}

impl Reply {
    fn new(status: &'static str, body: String, close: bool) -> Self {
        Self {
            status,
            body,
            close,
        }
    }
    fn render(&self) -> String {
        format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n{}",
            self.status,
            self.body.len(),
            if self.close { "close" } else { "keep-alive" },
            self.body
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    Session,
    Confirm,
    Diagnostics,
    Voice,
    Pcm,
    PcmBatch,
    Control,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Auth {
    None,
    Session(String),
    /// A protocol version 1 companion still sending its key outright.
    Legacy(String),
}

#[derive(Debug)]
struct Head {
    route: Route,
    length: usize,
    auth: Auth,
}

struct Received {
    head: Head,
    body: Vec<u8>,
}

fn request_head(headers: &str) -> Result<Head> {
    let mut lines = headers.split("\r\n");
    let route = match lines.next() {
        Some("POST /session HTTP/1.1") => Route::Session,
        Some("POST /session/confirm HTTP/1.1") => Route::Confirm,
        Some("POST /voice HTTP/1.1") => Route::Voice,
        Some("POST /pcm HTTP/1.1") => Route::Pcm,
        Some("POST /pcm-batch HTTP/1.1") => Route::PcmBatch,
        Some("GET /capture HTTP/1.1") => Route::Control,
        Some("POST /diagnostics HTTP/1.1") => Route::Diagnostics,
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
            == Some(&if matches!(route, Route::Pcm | Route::PcmBatch) {
                "application/octet-stream"
            } else {
                "application/json"
            }),
        "Invalid content type"
    );
    let auth = match fields.get("authorization") {
        None => Auth::None,
        Some(value) if value.starts_with("Bearer ") => {
            Auth::Legacy(value["Bearer ".len()..].trim().to_owned())
        }
        Some(value) => {
            let id = value
                .strip_prefix("Articulate-Session ")
                .context("Unsupported authorization")?;
            ensure!(
                id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid session"
            );
            Auth::Session(id.to_owned())
        }
    };
    let length = fields
        .get("content-length")
        .context("Missing content length")?
        .parse::<usize>()?;
    ensure!(
        match route {
            Route::Session | Route::Confirm => (1..=1024).contains(&length),
            Route::Voice => (1..=BODY_LIMIT).contains(&length),
            Route::Pcm => (64..=super::pcm::MAX_PACKET).contains(&length),
            Route::PcmBatch => (68..=BODY_LIMIT).contains(&length),
            Route::Control => length == 0,
            Route::Diagnostics => (1..=4096).contains(&length),
        },
        "Payload limit exceeded"
    );
    Ok(Head {
        route,
        length,
        auth,
    })
}

/// Reads one complete request. `Ok(None)` means the client closed between
/// requests, the idle limit passed, or the listener stopped.
fn receive(
    stream: &mut TcpStream,
    pending: &mut Vec<u8>,
    shared: &Shared,
) -> Result<Option<Received>> {
    let idle_deadline = Instant::now() + IDLE_LIMIT;
    let mut request_deadline: Option<Instant> =
        (!pending.is_empty()).then(|| Instant::now() + REQUEST_LIMIT);
    let mut bytes = std::mem::take(pending);
    let (split, head) = loop {
        if let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            ensure!(split <= HEADER_LIMIT, "Header limit exceeded");
            let head = request_head(std::str::from_utf8(&bytes[..split])?)?;
            break (split + 4, head);
        }
        ensure!(bytes.len() < HEADER_LIMIT, "Header limit exceeded");
        if shared.stopped() || Instant::now() >= idle_deadline {
            return Ok(None);
        }
        ensure!(
            request_deadline.is_none_or(|deadline| Instant::now() < deadline),
            "Request expired"
        );
        let mut buffer = [0_u8; 1024];
        match stream.read(&mut buffer) {
            Ok(0) => {
                ensure!(bytes.is_empty(), "Incomplete request");
                return Ok(None);
            }
            Ok(count) => {
                request_deadline.get_or_insert(Instant::now() + REQUEST_LIMIT);
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    };
    let request_deadline = request_deadline.unwrap_or_else(|| Instant::now() + REQUEST_LIMIT);
    while bytes.len() < split + head.length {
        ensure!(Instant::now() < request_deadline, "Request expired");
        ensure!(!shared.stopped(), "Stopped");
        let mut buffer = [0_u8; 4096];
        match stream.read(&mut buffer) {
            Ok(0) => bail!("Incomplete request"),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Pipelined bytes belong to the next request on this connection.
    *pending = bytes.split_off(split + head.length);
    let body = bytes.split_off(split);
    Ok(Some(Received { head, body }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRequest {
    version: u32,
    client_nonce: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmRequest {
    version: u32,
    session: String,
    client_proof: String,
}

fn handle(session: &mut Session, received: Received, server: &Listener) -> Reply {
    let Received { head, body } = received;
    let shared = server.shared;
    // Compatibility errors are answered on this connection and close it. They
    // reach the shared state only as an advisory notice, which never replaces
    // a paired companion's observation (see `Shared::protocol_mismatch`).
    if let Auth::Legacy(token) = &head.auth {
        // A version 1 companion proves itself the old way, by sending the key.
        // Only a key that matches ours may raise the update notice; any other
        // client is refused without learning or changing anything.
        if unhex(token, 32).is_ok_and(|key| constant_time_equal(&key, &server.key)) {
            shared.protocol_mismatch("outdated");
        }
        *session = Session::default();
        return Reply::new(
            "426 Upgrade Required",
            json!({"version": PROTOCOL_VERSION, "min_version": PROTOCOL_VERSION}).to_string(),
            true,
        );
    }
    match head.route {
        Route::Session => {
            let request: SessionRequest = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(_) => return Reply::new("400 Bad Request", String::new(), true),
            };
            if request.version != PROTOCOL_VERSION {
                // Unauthenticated, so advisory only while nothing is paired.
                shared.protocol_mismatch(if request.version < PROTOCOL_VERSION {
                    "outdated"
                } else {
                    "newer"
                });
                *session = Session::default();
                return Reply::new(
                    "426 Upgrade Required",
                    json!({"version": PROTOCOL_VERSION, "min_version": PROTOCOL_VERSION})
                        .to_string(),
                    true,
                );
            }
            let Ok(client_nonce) = unhex(&request.client_nonce, 32) else {
                return Reply::new("400 Bad Request", String::new(), true);
            };
            let mut server_nonce = [0_u8; 32];
            let mut id = [0_u8; 16];
            if random_bytes(&mut server_nonce).is_err() || random_bytes(&mut id).is_err() {
                return Reply::new("503 Service Unavailable", String::new(), true);
            }
            let server_proof =
                hmac_sha256(&server.key, &[SERVER_LABEL, &client_nonce, &server_nonce]);
            *session = Session {
                id: hex(&id),
                expected_client_proof: Some(hmac_sha256(
                    &server.key,
                    &[CLIENT_LABEL, &client_nonce, &server_nonce],
                )),
                ..Session::default()
            };
            Reply::new(
                "200 OK",
                json!({"version": PROTOCOL_VERSION, "session": session.id,
                    "server_nonce": hex(&server_nonce), "server_proof": hex(&server_proof)})
                .to_string(),
                false,
            )
        }
        Route::Confirm => {
            let request: ConfirmRequest = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(_) => return Reply::new("400 Bad Request", String::new(), true),
            };
            let Some(expected) = session.expected_client_proof.take() else {
                return Reply::new("401 Unauthorized", String::new(), true);
            };
            let proof = unhex(&request.client_proof, 32).unwrap_or_default();
            let valid = request.version == PROTOCOL_VERSION
                && constant_time_equal(request.session.as_bytes(), session.id.as_bytes())
                && constant_time_equal(&proof, &expected);
            if !valid {
                *session = Session::default();
                return Reply::new("403 Forbidden", String::new(), true);
            }
            session.authenticated = true;
            Reply::new("204 No Content", String::new(), false)
        }
        route => {
            if !session.authenticated || head.auth != Auth::Session(session.id.clone()) {
                *session = Session::default();
                return Reply::new("401 Unauthorized", String::new(), true);
            }
            authenticated(session, route, &body, server)
        }
    }
}

fn authenticated(session: &mut Session, route: Route, body: &[u8], server: &Listener) -> Reply {
    let shared = server.shared;
    let now_us = || {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
    };
    match route {
        Route::Voice => {
            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let Ok((observation, sequence, revision, audio_status)) =
                decode(body, now_ms, Instant::now())
            else {
                return Reply::new("400 Bad Request", String::new(), false);
            };
            // Ordering uses the connection-scoped sequence, never wall-clock time,
            // so a clock rollback cannot stall observations.
            if sequence <= session.voice_sequence {
                return Reply::new("409 Conflict", String::new(), false);
            }
            if session
                .voice_last
                .is_some_and(|at| at.elapsed() < Duration::from_millis(40))
            {
                return Reply::new("429 Too Many Requests", String::new(), false);
            }
            session.voice_sequence = sequence;
            session.voice_last = Some(Instant::now());
            server
                .voice
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_good = Some(Instant::now());
            shared.publish_companion(observation, revision, audio_status, Some("current"));
            Reply::new("204 No Content", String::new(), false)
        }
        Route::Control => {
            let observation = shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .snapshot
                .observation
                .clone();
            Reply::new(
                "200 OK",
                shared.pcm.control(observation.as_ref()).to_string(),
                false,
            )
        }
        Route::Diagnostics => {
            if shared.pcm.report_diagnostics(body).is_ok() {
                Reply::new("204 No Content", String::new(), false)
            } else {
                Reply::new("400 Bad Request", String::new(), false)
            }
        }
        Route::Pcm => {
            if now_us().is_ok_and(|now| shared.pcm.receive(body, now, Instant::now()).is_ok()) {
                Reply::new("204 No Content", String::new(), false)
            } else {
                Reply::new("400 Bad Request", String::new(), false)
            }
        }
        Route::PcmBatch => {
            let accepted = now_us().is_ok_and(|now| {
                pcm_batch(body).is_ok_and(|packets| {
                    packets
                        .into_iter()
                        .all(|packet| shared.pcm.receive(packet, now, Instant::now()).is_ok())
                })
            });
            if accepted {
                Reply::new("204 No Content", String::new(), false)
            } else {
                Reply::new("400 Bad Request", String::new(), false)
            }
        }
        Route::Session | Route::Confirm => unreachable!("handled before authentication"),
    }
}

fn pcm_batch(mut bytes: &[u8]) -> Result<Vec<&[u8]>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= BODY_LIMIT,
        "Invalid PCM batch size"
    );
    let mut packets = Vec::new();
    while !bytes.is_empty() {
        ensure!(
            packets.len() < BATCH_PACKETS && bytes.len() >= 4,
            "Invalid PCM batch framing"
        );
        let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        bytes = &bytes[4..];
        ensure!(
            (64..=super::pcm::MAX_PACKET).contains(&length) && length <= bytes.len(),
            "Invalid PCM batch packet size"
        );
        packets.push(&bytes[..length]);
        bytes = &bytes[length..];
    }
    Ok(packets)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u32,
    sequence: u64,
    #[serde(default)]
    companion_revision: Option<String>,
    #[serde(default)]
    audio_status: Option<String>,
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

pub(super) const AUDIO_STATES: &[&str] = &[
    "disabled",
    "addon-unavailable",
    "preload-unavailable",
    "waiting",
    "waiting-for-voice-engine",
    "unsupported-native-build",
    "native-hook-unavailable",
    "waiting-for-articulate",
    "ready",
    "capturing",
    "control-unavailable",
    "audio-transport-unavailable",
    "update-required",
];

type DecodedVoice = (Observation, u64, Option<String>, Option<String>);
/// Validates a voice snapshot. Wall-clock time is used only for the bounded
/// freshness window; ordering is the caller's sequence number.
fn decode(bytes: &[u8], now: u64, received: Instant) -> Result<DecodedVoice> {
    let payload: Payload = serde_json::from_slice(bytes)?;
    ensure!(
        payload
            .audio_status
            .as_deref()
            .is_none_or(|status| AUDIO_STATES.contains(&status)),
        "Invalid audio status"
    );
    ensure!(
        payload
            .companion_revision
            .as_ref()
            .is_none_or(|revision| revision.len() == 64
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
        "Invalid companion revision"
    );
    ensure!(
        payload.version == PROTOCOL_VERSION
            && payload.sequence > 0
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
        payload.sequence,
        payload.companion_revision,
        payload.audio_status,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch_bytes(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for packet in packets {
            bytes.extend_from_slice(&(packet.len() as u32).to_le_bytes());
            bytes.extend_from_slice(packet);
        }
        bytes
    }

    fn fresh_shared() -> Arc<Shared> {
        Arc::new(Shared {
            pcm: super::super::pcm::Hub::default(),
            state: Mutex::new(State {
                snapshot: Snapshot {
                    audio_status: None,
                    companion_revision: None,
                    companion_protocol: None,
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
        })
    }

    /// A listener on an ephemeral port with the real worker and RAII shutdown.
    fn listener(token: &str) -> (Connection, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let shared = fresh_shared();
        let worker_token = token.to_owned();
        let worker_shared = shared.clone();
        let worker = thread::spawn(move || serve(listener, &worker_token, &worker_shared));
        (
            Connection {
                shared,
                worker: Mutex::new(Some(worker)),
            },
            address,
        )
    }

    /// Minimal keep-alive HTTP client speaking the companion protocol.
    struct Client {
        stream: TcpStream,
        session: Option<String>,
        sequence: u64,
    }

    impl Client {
        fn connect(address: std::net::SocketAddr) -> Self {
            let stream = TcpStream::connect(address).unwrap();
            stream.set_nodelay(true).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            Self {
                stream,
                session: None,
                sequence: 0,
            }
        }

        fn raw(&mut self, request: &[u8], fragmented: bool) -> (u16, Vec<u8>) {
            if fragmented {
                // Exercise delayed header and body delivery separately. Windows
                // accepted sockets inherit nonblocking mode unless reset by serve.
                let header_end = request
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                self.stream.write_all(&request[..13]).unwrap();
                thread::sleep(Duration::from_millis(15));
                self.stream.write_all(&request[13..header_end]).unwrap();
                thread::sleep(Duration::from_millis(15));
                self.stream.write_all(&request[header_end..]).unwrap();
            } else {
                self.stream.write_all(request).unwrap();
            }
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            let (split, length) = loop {
                if let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..split]).unwrap().to_owned();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    break (split + 4, length);
                }
                let count = self.stream.read(&mut buffer).unwrap();
                assert!(count > 0, "Connection closed before a reply");
                bytes.extend_from_slice(&buffer[..count]);
            };
            while bytes.len() < split + length {
                let count = self.stream.read(&mut buffer).unwrap();
                assert!(count > 0, "Connection closed inside a reply");
                bytes.extend_from_slice(&buffer[..count]);
            }
            let status = std::str::from_utf8(&bytes[9..12]).unwrap().parse().unwrap();
            (status, bytes[split..split + length].to_vec())
        }

        fn request(&mut self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
            self.request_parts(method, path, body, self.session.clone(), false)
        }

        fn request_parts(
            &mut self,
            method: &str,
            path: &str,
            body: &[u8],
            auth: Option<String>,
            fragmented: bool,
        ) -> (u16, Vec<u8>) {
            let content_type = if matches!(path, "/pcm" | "/pcm-batch") {
                "application/octet-stream"
            } else {
                "application/json"
            };
            let auth = auth
                .map(|id| format!("Authorization: Articulate-Session {id}\r\n"))
                .unwrap_or_default();
            let mut request = format!(
                "{method} {path} HTTP/1.1\r\nHost: {ENDPOINT}\r\n{auth}Content-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            request.extend_from_slice(body);
            self.raw(&request, fragmented)
        }

        /// Performs the mutual handshake the way the companion does, verifying
        /// Articulate's proof before revealing anything that needs the key.
        fn handshake(&mut self, key_hex: &str) -> Result<String> {
            let key = unhex(key_hex, 32).unwrap();
            let mut client_nonce = [0_u8; 32];
            random_bytes(&mut client_nonce).unwrap();
            let (status, body) = self.request_parts(
                "POST",
                "/session",
                json!({"version": PROTOCOL_VERSION, "client_nonce": hex(&client_nonce)})
                    .to_string()
                    .as_bytes(),
                None,
                false,
            );
            ensure!(status == 200, "Session request failed: {status}");
            let reply: Value = serde_json::from_slice(&body)?;
            let server_nonce = unhex(reply["server_nonce"].as_str().unwrap_or(""), 32)?;
            let proof = unhex(reply["server_proof"].as_str().unwrap_or(""), 32)?;
            ensure!(
                constant_time_equal(
                    &proof,
                    &hmac_sha256(&key, &[SERVER_LABEL, &client_nonce, &server_nonce])
                ),
                "Articulate did not prove the pairing key"
            );
            let id = reply["session"].as_str().unwrap().to_owned();
            let client_proof = hmac_sha256(&key, &[CLIENT_LABEL, &client_nonce, &server_nonce]);
            let (status, _) = self.request_parts(
                "POST",
                "/session/confirm",
                json!({"version": PROTOCOL_VERSION, "session": id, "client_proof": hex(&client_proof)})
                    .to_string()
                    .as_bytes(),
                None,
                false,
            );
            ensure!(status == 204, "Confirmation failed: {status}");
            self.session = Some(id.clone());
            self.sequence = 0;
            Ok(id)
        }

        fn voice(&mut self, mut body: Value) -> u16 {
            self.sequence += 1;
            body["sequence"] = json!(self.sequence);
            self.request("POST", "/voice", body.to_string().as_bytes())
                .0
        }
    }

    fn voice_body(participants: Value) -> Value {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        json!({"version":PROTOCOL_VERSION,"observed_ms":now,"channel_id":"123","valid":true,
            "participants":participants})
    }

    fn packet(capture: u64, user: u64, sequence: u64) -> Vec<u8> {
        // One realistic 20ms, 48kHz mono callback.
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
    }

    #[test]
    fn hmac_matches_the_published_test_vector_and_hex_round_trips() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let bytes: Vec<u8> = (0..32).collect();
        assert_eq!(unhex(&hex(&bytes), 32).unwrap(), bytes);
        assert!(unhex("zz", 1).is_err());
        assert!(unhex("abc", 2).is_err());
        assert!(constant_time_equal(b"same", b"same"));
        assert!(!constant_time_equal(b"same", b"sane"));
        assert!(!constant_time_equal(b"same", b"same "));
    }

    #[test]
    fn batches_preserve_order_and_reject_truncated_oversized_or_excess_frames() {
        let packets = vec![vec![1; 64], vec![2; 128], vec![3; 64]];
        let bytes = batch_bytes(&packets);
        assert_eq!(
            pcm_batch(&bytes).unwrap(),
            packets.iter().map(Vec::as_slice).collect::<Vec<_>>()
        );
        assert!(pcm_batch(&[]).is_err());
        assert!(pcm_batch(&bytes[..bytes.len() - 1]).is_err());
        assert!(pcm_batch(&batch_bytes(&[vec![0; 63]])).is_err());
        assert!(pcm_batch(&batch_bytes(&[vec![0; super::super::pcm::MAX_PACKET + 1]])).is_err());
        assert!(pcm_batch(&batch_bytes(&vec![vec![0; 64]; BATCH_PACKETS + 1])).is_err());
        assert!(pcm_batch(&vec![0; BODY_LIMIT + 1]).is_err());
        let session = "a".repeat(32);
        let header = format!(
            "POST /pcm-batch HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Articulate-Session {session}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}",
            bytes.len()
        );
        let head = request_head(&header).unwrap();
        assert_eq!(head.route, Route::PcmBatch);
        assert_eq!(head.auth, Auth::Session(session.clone()));
        assert!(request_head(&format!("{header}\r\nOrigin: https://example.test")).is_err());
        assert!(request_head(&header.replace(&session, "short")).is_err());
    }

    #[test]
    fn diagnostics_require_json_and_a_small_body() {
        let header = format!(
            "POST /diagnostics HTTP/1.1\r\nHost: {ENDPOINT}\r\nContent-Type: application/json\r\nContent-Length: 128"
        );
        let head = request_head(&header).unwrap();
        assert_eq!(
            (head.route, head.length, head.auth),
            (Route::Diagnostics, 128, Auth::None)
        );
        assert!(request_head(&format!("{header}\r\nOrigin: https://example.invalid")).is_err());
        assert!(request_head(&header.replace("128", "4097")).is_err());
        assert!(request_head(&header.replace("application/json", "text/plain")).is_err());
    }

    #[test]
    fn running_revision_is_optional_for_old_companions_and_strict_when_present() {
        let mut body = json!({"version":PROTOCOL_VERSION,"sequence":1,"observed_ms":1000,"channel_id":null,"participants":[],"valid":true});
        assert!(
            decode(body.to_string().as_bytes(), 1000, Instant::now())
                .unwrap()
                .2
                .is_none()
        );
        let revision = "a".repeat(64);
        body["companion_revision"] = json!(revision);
        assert_eq!(
            decode(body.to_string().as_bytes(), 1000, Instant::now())
                .unwrap()
                .2
                .as_deref(),
            Some(revision.as_str())
        );
        for invalid in ["short".into(), "A".repeat(64), "g".repeat(64)] {
            body["companion_revision"] = json!(invalid);
            assert!(decode(body.to_string().as_bytes(), 1000, Instant::now()).is_err());
        }
        body["companion_revision"] = json!(revision);
        body["audio_status"] = json!("native-hook-unavailable");
        assert_eq!(
            decode(body.to_string().as_bytes(), 1000, Instant::now())
                .unwrap()
                .3
                .as_deref(),
            Some("native-hook-unavailable")
        );
        body["audio_status"] = json!("unbounded diagnostic text");
        assert!(decode(body.to_string().as_bytes(), 1000, Instant::now()).is_err());
    }

    #[test]
    fn pairing_keys_are_random_and_valid() {
        let first = new_token().unwrap();
        assert!(valid_token(&first));
        assert_ne!(first, new_token().unwrap());
        assert!(!valid_token("short"));
    }

    #[test]
    fn rejects_browser_remote_legacy_and_ambiguous_requests() {
        let session = "a".repeat(32);
        let header = format!(
            "POST /voice HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Articulate-Session {session}\r\nContent-Type: application/json\r\nContent-Length: 20"
        );
        assert_eq!(request_head(&header).unwrap().length, 20);
        for altered in [
            header.replace(ENDPOINT, "localhost:9223"),
            format!("{header}\r\nOrigin: https://discord.com"),
            format!("{header}\r\nContent-Length: 20"),
            format!("{header}\r\nTransfer-Encoding: chunked"),
            header.replace("Content-Length: 20", "Content-Length: 9999999"),
            header.replace("Articulate-Session", "Basic"),
        ] {
            assert!(request_head(&altered).is_err(), "{altered}");
        }
        // A version 1 companion is recognised so the user can be told to update.
        let legacy = header.replace(
            &format!("Articulate-Session {session}"),
            &format!("Bearer {}", "a".repeat(64)),
        );
        assert_eq!(
            request_head(&legacy).unwrap().auth,
            Auth::Legacy("a".repeat(64))
        );
    }

    #[test]
    fn metadata_rejects_stale_duplicate_wrong_version_and_unknown_fields() {
        let mut body = json!({"version":PROTOCOL_VERSION,"sequence":5,"observed_ms":1000,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Example","speaking":true,"is_self":false}]});
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_ok());
        assert!(decode(body.to_string().as_bytes(), 1300, Instant::now()).is_err());
        let duplicate = body["participants"][0].clone();
        body["participants"].as_array_mut().unwrap().push(duplicate);
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body["participants"].as_array_mut().unwrap().pop();
        body["audio"] = json!("unaccepted");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body.as_object_mut().unwrap().remove("audio");
        body["version"] = json!(1);
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body["version"] = json!(PROTOCOL_VERSION);
        body["sequence"] = json!(0);
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
        body.as_object_mut().unwrap().remove("sequence");
        assert!(decode(body.to_string().as_bytes(), 1100, Instant::now()).is_err());
    }

    #[test]
    fn avatar_metadata_is_optional_and_bound_to_the_participant() {
        let mut body = json!({"version":PROTOCOL_VERSION,"sequence":1,"observed_ms":1000,"channel_id":"123","valid":true,
            "participants":[{"id":"456","name":"Example","speaking":true,"is_self":false,
                "avatar":{"user_id":"456","hash":"0123456789abcdef0123456789abcdef"}}]});
        let (observation, _, _, _) =
            decode(body.to_string().as_bytes(), 1100, Instant::now()).unwrap();
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
    fn handshake_proves_the_key_both_ways_and_sessions_bind_to_their_connection() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        // Nothing is accepted before a handshake, and no secret is needed to ask.
        let mut client = Client::connect(address);
        let (status, _) = client.request(
            "POST",
            "/voice",
            voice_body(json!([])).to_string().as_bytes(),
        );
        assert_eq!(status, 401);
        // A listener holding the wrong key cannot produce a valid server proof.
        let mut client = Client::connect(address);
        let wrong = client.handshake(&"0".repeat(64)).unwrap_err();
        assert!(wrong.to_string().contains("did not prove"));
        // A client holding the wrong key is refused and learns nothing.
        let mut client = Client::connect(address);
        let (status, body) = client.request_parts(
            "POST",
            "/session",
            json!({"version": PROTOCOL_VERSION, "client_nonce": "1".repeat(64)})
                .to_string()
                .as_bytes(),
            None,
            false,
        );
        assert_eq!(status, 200);
        let reply: Value = serde_json::from_slice(&body).unwrap();
        let (status, _) = client.request_parts(
            "POST",
            "/session/confirm",
            json!({"version": PROTOCOL_VERSION, "session": reply["session"], "client_proof": "2".repeat(64)})
                .to_string()
                .as_bytes(),
            None,
            false,
        );
        assert_eq!(status, 403);
        // A correct handshake authenticates only its own connection.
        let mut client = Client::connect(address);
        let id = client.handshake(&token).unwrap();
        assert_eq!(client.voice(voice_body(json!([]))), 204);
        let mut other = Client::connect(address);
        other.session = Some(id);
        let (status, _) = other.request("GET", "/capture", &[]);
        assert_eq!(status, 401);
        // The session identifier must match the one that authenticated.
        let (status, _) = client.request_parts("GET", "/capture", &[], Some("b".repeat(32)), false);
        assert_eq!(status, 401);
        assert!(matches!(connection.snapshot().status, Status::Ready));
        assert_eq!(
            connection.snapshot().companion_protocol.as_deref(),
            Some("current")
        );
        drop(connection);
    }

    #[test]
    fn version_mismatches_are_reported_so_the_user_can_update() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        let mut client = Client::connect(address);
        let (status, body) = client.request_parts(
            "POST",
            "/session",
            json!({"version": 1, "client_nonce": "1".repeat(64)})
                .to_string()
                .as_bytes(),
            None,
            false,
        );
        assert_eq!(status, 426);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["min_version"],
            PROTOCOL_VERSION
        );
        assert_eq!(
            connection.snapshot().companion_protocol.as_deref(),
            Some("outdated")
        );
        assert!(matches!(
            connection.snapshot().status,
            Status::Unavailable(_)
        ));
        let mut client = Client::connect(address);
        let (status, _) = client.request_parts(
            "POST",
            "/session",
            json!({"version": PROTOCOL_VERSION + 1, "client_nonce": "1".repeat(64)})
                .to_string()
                .as_bytes(),
            None,
            false,
        );
        assert_eq!(status, 426);
        assert_eq!(
            connection.snapshot().companion_protocol.as_deref(),
            Some("newer")
        );
        // A version 1 companion that still sends a bearer key is told to update
        // too, but only a bearer key that matches ours may say so.
        assert_eq!(legacy_request(address, &"0".repeat(64)), 426);
        assert_eq!(
            connection.snapshot().companion_protocol.as_deref(),
            Some("newer"),
            "an unknown key changes nothing"
        );
        assert_eq!(legacy_request(address, &token), 426);
        assert_eq!(
            connection.snapshot().companion_protocol.as_deref(),
            Some("outdated")
        );
        drop(connection);
    }

    /// One protocol version 1 request with `Authorization: Bearer`.
    fn legacy_request(address: std::net::SocketAddr, token: &str) -> u16 {
        let mut legacy = TcpStream::connect(address).unwrap();
        legacy
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        legacy
            .write_all(
                format!(
                    "GET /capture HTTP/1.1\r\nHost: {ENDPOINT}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        legacy.read_to_string(&mut response).unwrap();
        response
            .strip_prefix("HTTP/1.1 ")
            .and_then(|rest| rest[..3].parse().ok())
            .unwrap()
    }

    #[test]
    fn compatibility_errors_never_replace_a_paired_companion() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        let mut client = Client::connect(address);
        client.handshake(&token).unwrap();
        assert_eq!(
            client.voice(voice_body(
                json!([{"id":"456","name":"Example","speaking":true,"is_self":false}])
            )),
            204
        );
        let paired = connection.snapshot();
        assert!(matches!(paired.status, Status::Ready));
        assert_eq!(paired.companion_protocol.as_deref(), Some("current"));
        let published = Instant::now();
        let history = connection.history(published - Duration::from_secs(1), published);
        assert_eq!(history.len(), 1);
        assert!(history[0].valid);
        // Unauthenticated peers speaking the wrong version, an unknown bearer
        // key and even the real key over the old protocol are refused on their
        // own connection without touching the paired state.
        for version in [1, PROTOCOL_VERSION + 1] {
            let mut other = Client::connect(address);
            let (status, _) = other.request_parts(
                "POST",
                "/session",
                json!({"version": version, "client_nonce": "1".repeat(64)})
                    .to_string()
                    .as_bytes(),
                None,
                false,
            );
            assert_eq!(status, 426);
            drop(other); // Lets the receiver finish that connection at once.
        }
        assert_eq!(legacy_request(address, &"0".repeat(64)), 426);
        assert_eq!(legacy_request(address, &token), 426);
        let snapshot = connection.snapshot();
        assert!(matches!(snapshot.status, Status::Ready));
        assert_eq!(snapshot.companion_protocol.as_deref(), Some("current"));
        let observation = snapshot.observation.expect("the observation survives");
        assert_eq!(observation.participants.len(), 1);
        let history = connection.history(published - Duration::from_secs(1), Instant::now());
        assert_eq!(history.len(), 1, "no gap was recorded");
        // The paired connection keeps working, including a capture control
        // response built from that observation.
        let (status, body) = client.request("GET", "/capture", &[]);
        assert_eq!(status, 200);
        assert!(serde_json::from_slice::<Value>(&body).is_ok());
        thread::sleep(Duration::from_millis(50)); // Clears the snapshot rate limit.
        assert_eq!(
            client.voice(voice_body(
                json!([{"id":"456","name":"Example","speaking":false,"is_self":false}])
            )),
            204
        );
        drop(connection);
    }

    #[test]
    fn authenticated_http_publishes_expires_and_stops() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        let mut client = Client::connect(address);
        client.handshake(&token).unwrap();
        let mut body =
            voice_body(json!([{"id":"456","name":"Example","speaking":true,"is_self":false}]));
        body["companion_revision"] = json!("a".repeat(64));
        assert_eq!(client.voice(body), 204);
        assert!(matches!(connection.snapshot().status, Status::Ready));
        assert_eq!(
            connection.snapshot().companion_revision.as_deref(),
            Some("a".repeat(64).as_str())
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(connection.snapshot().status, Status::Ready) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(connection.snapshot().observation.is_none());
        assert!(connection.snapshot().companion_revision.is_none());
        let shutdown = Instant::now();
        drop(connection);
        assert!(shutdown.elapsed() < Duration::from_secs(1));
        assert!(TcpStream::connect(address).is_err());
    }

    #[test]
    fn voice_ordering_uses_connection_sequence_not_wall_clock() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        let mut client = Client::connect(address);
        client.handshake(&token).unwrap();
        let participants = json!([{"id":"456","name":"Example","speaking":true,"is_self":false}]);
        assert_eq!(client.voice(voice_body(participants.clone())), 204);
        thread::sleep(Duration::from_millis(45));
        // A replayed or reordered snapshot on this connection is refused.
        let mut stale = voice_body(participants.clone());
        stale["sequence"] = json!(1);
        let (status, _) = client.request("POST", "/voice", stale.to_string().as_bytes());
        assert_eq!(status, 409);
        // Observed time moving backwards within the freshness window still publishes.
        let mut earlier = voice_body(participants.clone());
        earlier["observed_ms"] = json!(earlier["observed_ms"].as_u64().unwrap() - 100);
        assert_eq!(client.voice(earlier), 204);
        // Snapshots faster than 40 ms apart are throttled, not treated as errors.
        assert_eq!(client.voice(voice_body(participants.clone())), 429);
        assert!(matches!(connection.snapshot().status, Status::Ready));
        // A new connection starts its own sequence from one.
        let mut second = Client::connect(address);
        second.handshake(&token).unwrap();
        thread::sleep(Duration::from_millis(45));
        assert_eq!(second.voice(voice_body(participants)), 204);
        drop(connection);
    }

    #[test]
    fn real_http_pcm_requires_arming_nonce_and_remote_identity_over_concurrent_connections() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        // Metadata and audio arrive on separate persistent connections, as in Discord.
        let mut metadata = Client::connect(address);
        metadata.handshake(&token).unwrap();
        let mut audio = Client::connect(address);
        audio.handshake(&token).unwrap();
        let control = |audio: &mut Client| {
            let (status, body) = audio.request("GET", "/capture", &[]);
            assert_eq!(status, 200);
            serde_json::from_slice::<Value>(&body).unwrap()
        };
        assert!(!connection.shared.pcm.ready());
        assert_eq!(
            audio.request("POST", "/pcm", &packet(1, 456, 1)).0,
            400,
            "Audio before arming is refused"
        );
        assert_eq!(
            metadata.voice(voice_body(json!([
                {"id":"456","name":"Remote example","speaking":true,"is_self":false},
                {"id":"901","name":"Second remote example","speaking":true,"is_self":false},
                {"id":"789","name":"Local example","speaking":true,"is_self":true}]))),
            204
        );
        assert_eq!(control(&mut audio)["active"], false);
        assert!(
            connection.native_audio_ready(),
            "Native readiness must precede arming and PCM"
        );
        let observation = connection.snapshot().observation.unwrap();
        let capture = connection.shared.pcm.begin(&observation).unwrap();
        let armed = control(&mut audio);
        assert_eq!(armed["active"], true);
        assert_eq!(armed["channel_id"], "123");
        assert_eq!(armed["participants"], json!(["456", "901"]));
        let nonce = armed["capture_id"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_ne!(nonce, 0);
        assert_eq!(
            audio.request("POST", "/pcm", &packet(nonce ^ 1, 456, 1)).0,
            400
        );
        assert_eq!(audio.request("POST", "/pcm", &packet(nonce, 789, 1)).0, 400);
        let transfer_started = Instant::now();
        for sequence in 1..=200 {
            let (status, _) = audio.request_parts(
                "POST",
                "/pcm",
                &packet(nonce, 456, sequence),
                audio.session.clone(),
                sequence == 1,
            );
            assert_eq!(status, 204, "PCM packet {sequence} was rejected");
            if sequence % 50 == 0 {
                // Metadata keeps flowing on its own connection meanwhile.
                thread::sleep(Duration::from_millis(45));
                assert_eq!(
                    metadata.voice(voice_body(json!([
                        {"id":"456","name":"Remote example","speaking":true,"is_self":false},
                        {"id":"901","name":"Second remote example","speaking":true,"is_self":false},
                        {"id":"789","name":"Local example","speaking":true,"is_self":true}]))),
                    204
                );
            }
        }
        assert!(
            transfer_started.elapsed() < Duration::from_secs(2),
            "Persistent PCM transport cannot keep up with live audio"
        );
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 200);
        assert_eq!(frames[0].samples.len(), 960);
        assert_eq!(&frames[0].samples[..2], &[1, -1]);
        assert_eq!(frames[0].attribution.speakers[0].name, "Remote example");
        assert_eq!(frames[0].attribution.speakers[0].id, "456");
        let packets: Vec<_> = (201..=264)
            .map(|sequence| packet(nonce, if sequence % 2 == 0 { 901 } else { 456 }, sequence))
            .collect();
        let mut malformed = batch_bytes(&packets[..1]);
        malformed.extend_from_slice(&[1, 2, 3]);
        assert_eq!(audio.request("POST", "/pcm-batch", &malformed).0, 400);
        assert!(
            capture.drain().unwrap().is_empty(),
            "Malformed batch framing must not partially deliver audio"
        );
        let batch = batch_bytes(&packets);
        let batch_started = Instant::now();
        let (status, _) =
            audio.request_parts("POST", "/pcm-batch", &batch, audio.session.clone(), true);
        assert_eq!(status, 204);
        assert!(
            batch_started.elapsed() < Duration::from_millis(600),
            "Two participants' 640 ms of PCM must arrive faster than real time"
        );
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 64);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame.attribution.speakers[0].id,
                if index % 2 == 0 { "456" } else { "901" }
            );
            assert_eq!(frame.samples.len(), 960);
        }
        // A protocol rejection is a normal reply; the connection stays usable.
        assert_eq!(
            audio.request("POST", "/pcm", &packet(nonce, 456, 264)).0,
            400
        );
        assert!(
            capture.drain().is_err(),
            "Replayed sequence must stop capture"
        );
        drop(capture);
        assert_eq!(control(&mut audio)["active"], false);
        assert_eq!(
            audio.request("POST", "/pcm", &packet(nonce, 456, 201)).0,
            400
        );
        drop(connection);
        assert!(TcpStream::connect(address).is_err());
    }

    #[test]
    fn connections_are_bounded_and_idle_or_slow_requests_are_closed() {
        let token = new_token().unwrap();
        let (connection, address) = listener(&token);
        let mut clients: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| {
                let mut client = Client::connect(address);
                client.handshake(&token).unwrap();
                client
            })
            .collect();
        // One connection too many is refused without disturbing the others.
        let mut extra = TcpStream::connect(address).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = String::new();
        let _ = extra.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 503"), "{response}");
        assert_eq!(clients[0].request("GET", "/capture", &[]).0, 200);
        // A request that stalls after its first bytes is dropped on the deadline
        // while other connections keep working.
        let mut slow = clients.pop().unwrap();
        slow.stream.write_all(b"GET /capture HTTP/1.1\r\n").unwrap();
        thread::sleep(REQUEST_LIMIT + Duration::from_millis(250));
        assert_eq!(clients[1].request("GET", "/capture", &[]).0, 200);
        let mut rest = Vec::new();
        slow.stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let _ = slow.stream.read_to_end(&mut rest);
        assert!(String::from_utf8_lossy(&rest).starts_with("HTTP/1.1 400"));
        drop(clients);
        drop(connection);
    }

    #[cfg(windows)]
    #[test]
    fn plugin_port_is_reserved_exclusively() {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            AF_INET, IN_ADDR, IN_ADDR_0, IPPROTO_TCP, SO_REUSEADDR, SOCK_STREAM, SOCKADDR,
            SOCKADDR_IN, SOL_SOCKET, WSASocketW, bind, closesocket, setsockopt,
        };
        let listener = bind_exclusive("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 0);
        assert_ne!(listener.as_raw_socket(), 0);
        // A plain bind fails as usual.
        assert!(TcpListener::bind(("127.0.0.1", port)).is_err());
        // Even a socket asking to share the address is refused.
        unsafe {
            let socket = WSASocketW(
                i32::from(AF_INET),
                SOCK_STREAM,
                IPPROTO_TCP,
                std::ptr::null(),
                0,
                0,
            );
            let one: i32 = 1;
            assert_eq!(
                setsockopt(
                    socket,
                    SOL_SOCKET,
                    SO_REUSEADDR,
                    (&one as *const i32).cast(),
                    4
                ),
                0
            );
            let name = SOCKADDR_IN {
                sin_family: AF_INET,
                sin_port: port.to_be(),
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from(std::net::Ipv4Addr::LOCALHOST).to_be(),
                    },
                },
                sin_zero: [0; 8],
            };
            let bound = bind(
                socket,
                (&name as *const SOCKADDR_IN).cast::<SOCKADDR>(),
                std::mem::size_of::<SOCKADDR_IN>() as i32,
            );
            closesocket(socket);
            assert_ne!(bound, 0, "SO_REUSEADDR must not hijack the plugin port");
        }
    }
}
