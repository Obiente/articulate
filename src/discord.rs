//! Opt-in observer of the user's loopback Discord renderer debugger.
//!
//! The renderer observer reads only current voice membership and speaking state.
//! Its lease removes its own listeners even if this process disappears.
pub mod launch;
pub mod plugin;

use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, client::client_with_config, protocol::WebSocketConfig};

const ADDRESS: &str = "127.0.0.1:9222";
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const HISTORY_LIMIT: usize = 8192;
const GROUP: &str = "articulate-voice-observer";

#[derive(Clone, Debug, Deserialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub speaking: bool,
    pub is_self: bool,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub at: Instant,
    pub generation: u64,
    pub channel_id: Option<String>,
    pub participants: Vec<Participant>,
    pub valid: bool,
}

#[derive(Clone, Debug)]
pub enum Status {
    Connecting,
    Ready,
    Unavailable(String),
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub observation: Option<Observation>,
}

struct State {
    snapshot: Snapshot,
    history: VecDeque<Observation>,
    generation: u64,
    channel: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    stopped: AtomicBool,
    wake: Condvar,
    wait_lock: Mutex<()>,
}

/// The worker owns `Shared`, never `Connection`, so dropping the last handle stops it.
pub struct Connection {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Connection {
    pub fn start() -> Arc<Self> {
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
            .name("discord-voice".into())
            .spawn(move || run(worker_shared));
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(_) => {
                shared.unavailable("Could not start the Discord observer.");
                None
            }
        };
        Arc::new(Self {
            shared,
            worker: Mutex::new(worker),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
    }

    /// Includes the most recent state at or before `from` for interval attribution.
    pub fn history(&self, from: Instant, to: Instant) -> Vec<Observation> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        history_between(&state.history, from, to)
    }

    pub fn disconnect(&self) {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shared.wake.notify_all();
        self.shared.unavailable("Discord speaker names are off.");
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

fn history_between(
    history: &VecDeque<Observation>,
    from: Instant,
    to: Instant,
) -> Vec<Observation> {
    if from > to {
        return Vec::new();
    }
    let start = history
        .iter()
        .rposition(|item| item.at <= from)
        .unwrap_or(0);
    history
        .iter()
        .skip(start)
        .take_while(|item| item.at <= to)
        .cloned()
        .collect()
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn wait(&self, duration: Duration) {
        let guard = self.wait_lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self
            .wake
            .wait_timeout_while(guard, duration, |_| !self.stopped());
    }

    fn unavailable(&self, message: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.snapshot = Snapshot {
            status: Status::Unavailable(message.into()),
            observation: None,
        };
        state.channel = None;
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        push_history(
            &mut state.history,
            Observation {
                at: Instant::now(),
                generation,
                channel_id: None,
                participants: Vec::new(),
                valid: false,
            },
        );
    }

    fn publish(&self, mut observation: Observation) {
        if self.stopped() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped() {
            return;
        }
        if observation.channel_id != state.channel {
            state.generation = state.generation.wrapping_add(1);
            state.channel = observation.channel_id.clone();
        }
        observation.generation = state.generation;
        if !observation.valid {
            observation.participants.clear();
            observation.channel_id = None;
        }
        // Never reorder activity when a new calibration follows a debugger stall.
        if state
            .history
            .back()
            .is_some_and(|last| last.at > observation.at)
        {
            return;
        }
        state.snapshot = if observation.valid {
            Snapshot {
                status: Status::Ready,
                observation: Some(observation.clone()),
            }
        } else {
            Snapshot {
                status: Status::Unavailable("Discord activity is temporarily unavailable.".into()),
                observation: None,
            }
        };
        push_history(&mut state.history, observation);
    }
}

fn push_history(history: &mut VecDeque<Observation>, observation: Observation) {
    history.push_back(observation);
    while history.len() > HISTORY_LIMIT {
        history.pop_front();
    }
    // Keep five minutes and one predecessor. At normal cadence this is ~2,000 entries.
    let cutoff = Instant::now().checked_sub(Duration::from_secs(300));
    while history.len() > 2 && cutoff.is_some_and(|cutoff| history[1].at < cutoff) {
        history.pop_front();
    }
}

fn run(shared: Arc<Shared>) {
    let mut backoff = Duration::from_secs(1);
    while !shared.stopped() {
        let result = observe(&shared);
        if shared.stopped() {
            break;
        }
        // CDP errors can include client internals. Map failures to fixed product messages.
        let debugger_closed = result.as_ref().err().is_some_and(|error| {
            error.chain().any(|cause| {
                cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::NotConnected
                            | std::io::ErrorKind::AddrNotAvailable
                    )
                })
            })
        });
        shared.unavailable(if debugger_closed {
            "Discord is unavailable. Open its local debugger on port 9222 to connect."
        } else {
            "Discord's voice interface is unavailable. Retrying automatically."
        });
        shared.wait(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(8));
    }
}

#[derive(Deserialize)]
struct Batch {
    now: f64,
    overflow: bool,
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    time: f64,
    channel_id: Option<String>,
    participants: Vec<Participant>,
    valid: bool,
}

struct Calibration {
    renderer_ms: f64,
    local: Instant,
}

impl Calibration {
    fn new(renderer_ms: f64, before: Instant, after: Instant) -> Option<Self> {
        let elapsed = after.checked_duration_since(before)?;
        if !renderer_ms.is_finite() || renderer_ms < 0.0 || elapsed > Duration::from_millis(150) {
            return None;
        }
        Some(Self {
            renderer_ms,
            local: before + elapsed / 2,
        })
    }

    fn map(&self, renderer_ms: f64) -> Option<Instant> {
        let delta = renderer_ms - self.renderer_ms;
        if !delta.is_finite() || delta.abs() > 10_000.0 {
            return None;
        }
        let duration = Duration::from_secs_f64(delta.abs() / 1000.0);
        if delta >= 0.0 {
            self.local.checked_add(duration)
        } else {
            self.local.checked_sub(duration)
        }
    }
}

fn observe(shared: &Shared) -> Result<()> {
    let mut cdp = Cdp::connect()?;
    let result = (|| -> Result<()> {
        let bridge = cdp.discover(shared)?;
        cdp.bridge = Some(bridge.clone());
        let mut previous_clock: Option<Calibration> = None;
        while !shared.stopped() {
            let before = Instant::now();
            let value = cdp.call("Runtime.callFunctionOn", json!({
                "objectId": bridge, "functionDeclaration": "function () { return this.drain(); }",
                "returnByValue": true, "objectGroup": GROUP,
            }))?;
            let after = Instant::now();
            let batch: Batch = serde_json::from_value(value["result"]["value"].clone())?;
            ensure!(batch.samples.len() <= 512, "Activity buffer exceeded");
            let clock = Calibration::new(batch.now, before, after);
            let consistent = clock.as_ref().is_some_and(|clock| {
                previous_clock.as_ref().is_none_or(|previous| {
                    previous.map(batch.now).is_some_and(|predicted| {
                        instant_distance(predicted, clock.local) <= Duration::from_millis(50)
                    })
                })
            });
            if !consistent || batch.overflow {
                shared.publish(Observation {
                    at: after,
                    generation: 0,
                    channel_id: None,
                    participants: Vec::new(),
                    valid: false,
                });
            } else if let Some(clock) = &clock {
                for sample in batch.samples {
                    let Some(at) = clock.map(sample.time) else {
                        continue;
                    };
                    if at > after || after.duration_since(at) > Duration::from_secs(4) {
                        continue;
                    }
                    let valid = sample.valid && valid_sample(&sample);
                    shared.publish(Observation {
                        at,
                        generation: 0,
                        channel_id: sample.channel_id,
                        participants: sample.participants,
                        valid,
                    });
                }
            }
            previous_clock = clock;
            shared.wait(Duration::from_millis(150));
        }
        Ok(())
    })();
    cdp.cleanup();
    result
}

fn instant_distance(a: Instant, b: Instant) -> Duration {
    if a >= b {
        a.duration_since(b)
    } else {
        b.duration_since(a)
    }
}

fn valid_sample(sample: &Sample) -> bool {
    fn valid_id(id: &str) -> bool {
        !id.is_empty() && id.len() <= 24 && id.bytes().all(|c| c.is_ascii_digit())
    }
    if sample.channel_id.as_deref().is_some_and(|id| !valid_id(id))
        || sample.participants.len() > 256
    {
        return false;
    }
    if sample.channel_id.is_none() && !sample.participants.is_empty() {
        return false;
    }
    let mut ids = std::collections::HashSet::new();
    sample.participants.iter().all(|p| {
        valid_id(&p.id)
            && ids.insert(&p.id)
            && !p.name.is_empty()
            && p.name.chars().count() <= 128
            && !p.name.chars().any(char::is_control)
    })
}

struct Cdp {
    socket: WebSocket<TcpStream>,
    next_id: u64,
    bridge: Option<String>,
}

impl Cdp {
    fn connect() -> Result<Self> {
        let endpoint = discover_endpoint()?;
        let stream = loopback_stream()?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_PAYLOAD))
            .max_frame_size(Some(MAX_PAYLOAD));
        let (socket, _) = client_with_config(endpoint.as_str(), stream, Some(config))
            .context("Debugger handshake failed")?;
        Ok(Self {
            socket,
            next_id: 0,
            bridge: None,
        })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.socket.send(Message::Text(
            json!({"id": id, "method": method, "params": params})
                .to_string()
                .into(),
        ))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            ensure!(Instant::now() < deadline, "Debugger response timed out");
            match self.socket.read()? {
                Message::Text(text) => {
                    let response: Value = serde_json::from_str(&text)?;
                    if response["id"].as_u64() != Some(id) {
                        continue;
                    }
                    ensure!(response.get("error").is_none(), "Debugger command failed");
                    let result = response.get("result").context("Missing debugger result")?;
                    ensure!(
                        result.get("exceptionDetails").is_none(),
                        "Voice interface changed"
                    );
                    return Ok(result.clone());
                }
                Message::Close(_) => bail!("Debugger disconnected"),
                Message::Ping(_) | Message::Pong(_) => {}
                _ => bail!("Unexpected debugger response"),
            }
        }
    }

    fn properties(&mut self, object_id: &str) -> Result<Value> {
        self.call(
            "Runtime.getProperties",
            json!({"objectId": object_id, "ownProperties": true, "generatePreview": false}),
        )
    }

    fn discover(&mut self, shared: &Shared) -> Result<String> {
        let loader = self.call(
            "Runtime.evaluate",
            json!({
                "expression": "window.webpackChunkdiscord_app.push", "returnByValue": false,
                "objectGroup": GROUP, "timeout": 1000,
            }),
        )?;
        let loader_id = object_id(&loader["result"])?;
        let bound = self.properties(&loader_id)?;
        let target_id = internal_id(&bound, "[[TargetFunction]]").unwrap_or(loader_id);
        let target = self.properties(&target_id)?;
        let scopes_id = internal_id(&target, "[[Scopes]]").context("Loader scopes unavailable")?;
        let scopes = self.properties(&scopes_id)?;
        let scopes = scopes["result"]
            .as_array()
            .context("Loader scopes invalid")?;
        let mut inspected = 0;
        for scope in scopes.iter().take(16) {
            ensure!(!shared.stopped(), "Observer stopped");
            if !scope["value"]["description"]
                .as_str()
                .is_some_and(|name| name.starts_with("Closure"))
            {
                continue;
            }
            let variables = self.properties(&object_id(&scope["value"])?)?;
            let Some(variables) = variables["result"].as_array() else {
                continue;
            };
            for variable in variables.iter().take(256) {
                ensure!(!shared.stopped(), "Observer stopped");
                if variable["value"]["type"] != "object" || variable["value"]["subtype"] == "null" {
                    continue;
                }
                let Ok(candidate) = object_id(&variable["value"]) else {
                    continue;
                };
                inspected += 1;
                ensure!(inspected <= 64, "Loader discovery bound exceeded");
                let shape = self.call("Runtime.callFunctionOn", json!({
                    "objectId": candidate, "returnByValue": true, "objectGroup": GROUP,
                    "functionDeclaration": "function () { const keys = Object.keys(this).slice(0, 32); let count = 0; for (const key of keys) { const value = Object.getOwnPropertyDescriptor(this, key)?.value; if (/^\\d+$/.test(key) && value && typeof value === 'object' && Object.prototype.hasOwnProperty.call(value, 'exports')) count++; } return count >= 3; }",
                }))?;
                if shape["result"]["value"] != true {
                    continue;
                }
                let bridge = self.call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": candidate, "returnByValue": false, "objectGroup": GROUP,
                        "functionDeclaration": include_str!("discord/discover.js"),
                    }),
                )?;
                return object_id(&bridge["result"]);
            }
        }
        bail!("Voice module cache unavailable")
    }

    fn cleanup(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            let _ = self.call("Runtime.callFunctionOn", json!({
                "objectId": bridge, "functionDeclaration": "function () { this.stop(); }", "returnByValue": true,
            }));
        }
        let _ = self.call("Runtime.releaseObjectGroup", json!({"objectGroup": GROUP}));
        let _ = self.socket.close(None);
    }
}

fn object_id(value: &Value) -> Result<String> {
    Ok(value["objectId"]
        .as_str()
        .context("Missing runtime reference")?
        .into())
}

fn internal_id(properties: &Value, name: &str) -> Option<String> {
    properties["internalProperties"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)?["value"]["objectId"]
        .as_str()
        .map(str::to_owned)
}

fn loopback_stream() -> Result<TcpStream> {
    let stream =
        TcpStream::connect_timeout(&ADDRESS.parse::<SocketAddr>()?, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn discover_endpoint() -> Result<String> {
    // Direct TCP avoids system proxies, redirects, DNS, or accidental remote endpoints.
    let mut stream = loopback_stream()?;
    stream.write_all(
        b"GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:9222\r\nConnection: close\r\n\r\n",
    )?;
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buffer = [0_u8; 8192];
    let mut expected_length = None;
    loop {
        ensure!(
            Instant::now() < deadline,
            "Debugger target request timed out"
        );
        let length = stream.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        ensure!(
            bytes.len() + length <= MAX_PAYLOAD,
            "Debugger target list exceeds limit"
        );
        bytes.extend_from_slice(&buffer[..length]);
        if expected_length.is_none() {
            expected_length = http_response_length(&bytes)?;
        }
        if expected_length.is_some_and(|expected| bytes.len() >= expected) {
            break;
        }
    }
    ensure!(
        expected_length == Some(bytes.len()),
        "Incomplete debugger HTTP response"
    );
    let split = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .context("Invalid debugger HTTP response")?;
    let headers = std::str::from_utf8(&bytes[..split])?;
    ensure!(
        headers.lines().next().is_some_and(
            |line| line.starts_with("HTTP/1.1 200 ") || line.starts_with("HTTP/1.0 200 ")
        ),
        "Debugger target request failed"
    );
    ensure!(
        !headers.to_ascii_lowercase().contains("transfer-encoding:"),
        "Unsupported debugger response framing"
    );
    select_endpoint(&serde_json::from_slice(&bytes[split + 4..])?)
}

fn http_response_length(bytes: &[u8]) -> Result<Option<usize>> {
    let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
        ensure!(bytes.len() <= 16384, "Debugger HTTP headers exceed limit");
        return Ok(None);
    };
    ensure!(split <= 16384, "Debugger HTTP headers exceed limit");
    let headers = std::str::from_utf8(&bytes[..split])?;
    let mut lengths = headers.lines().skip(1).filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then_some(value.trim())
    });
    let length: usize = lengths
        .next()
        .context("Missing debugger response length")?
        .parse()?;
    ensure!(
        lengths.next().is_none(),
        "Ambiguous debugger response length"
    );
    let total = split
        .checked_add(4)
        .and_then(|prefix| prefix.checked_add(length))
        .context("Debugger response exceeds limit")?;
    ensure!(total <= MAX_PAYLOAD, "Debugger response exceeds limit");
    Ok(Some(total))
}

fn select_endpoint(value: &Value) -> Result<String> {
    let targets = value.as_array().context("Invalid debugger targets")?;
    ensure!(targets.len() <= 64, "Too many debugger targets");
    let mut matches = Vec::new();
    for target in targets {
        if target["type"] != "page" {
            continue;
        }
        let Some(url) = target["url"].as_str() else {
            continue;
        };
        let Ok(uri) = url.parse::<tungstenite::http::Uri>() else {
            continue;
        };
        if uri.scheme_str() != Some("https")
            || !uri.authority().is_some_and(|authority| {
                matches!(authority.as_str(), "discord.com" | "discord.com:443")
            })
        {
            continue;
        }
        let endpoint = target["webSocketDebuggerUrl"]
            .as_str()
            .context("Missing renderer debugger endpoint")?;
        let ws = endpoint.parse::<tungstenite::http::Uri>()?;
        ensure!(
            ws.scheme_str() == Some("ws")
                && ws
                    .authority()
                    .is_some_and(|authority| authority.as_str() == ADDRESS)
                && ws.path().starts_with("/devtools/page/")
                && ws.query().is_none(),
            "Debugger must be loopback only"
        );
        matches.push(endpoint.to_owned());
    }
    ensure!(matches.len() == 1, "Expected one Discord renderer");
    Ok(matches.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_call_sees_later_connections_and_disconnect_without_stopping_audio() {
        fn connection() -> Arc<Connection> {
            Arc::new(Connection {
                shared: Arc::new(Shared {
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
                }),
                worker: Mutex::new(None),
            })
        }
        let control = crate::call_capture::Control::new();
        let worker_control = control.clone();
        assert!(worker_control.discord().is_none());
        let first = connection();
        control.set_discord(Some(first.clone()));
        assert!(Arc::ptr_eq(&worker_control.discord().unwrap(), &first));
        first.shared.publish(Observation {
            at: Instant::now(),
            generation: 0,
            channel_id: Some("123".into()),
            valid: true,
            participants: vec![Participant {
                id: "456".into(),
                name: "Example".into(),
                speaking: true,
                is_self: false,
            }],
        });
        assert_eq!(
            worker_control
                .discord()
                .unwrap()
                .snapshot()
                .observation
                .unwrap()
                .participants
                .len(),
            1
        );
        control.set_discord(None);
        first.disconnect();
        assert!(worker_control.discord().is_none());
        let second = connection();
        control.set_discord(Some(second.clone()));
        assert!(Arc::ptr_eq(&worker_control.discord().unwrap(), &second));
        assert!(
            worker_control
                .discord()
                .unwrap()
                .snapshot()
                .observation
                .is_none()
        );
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
        assert!(!control.abort.load(Ordering::Relaxed));
    }

    #[test]
    fn endpoint_rejects_remote_and_ambiguous_targets() {
        let target = json!({"type":"page", "url":"https://discord.com/channels/@me", "webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/page/example"});
        assert!(select_endpoint(&json!([target.clone()])).is_ok());
        assert!(select_endpoint(&json!([target.clone(), target.clone()])).is_err());
        for endpoint in [
            "ws://localhost:9222/devtools/page/example",
            "ws://127.0.0.1:9999/devtools/page/example",
            "ws://example.test:9222/devtools/page/example",
            "ws://user@127.0.0.1:9222/devtools/page/example",
        ] {
            let mut changed = target.clone();
            changed["webSocketDebuggerUrl"] = endpoint.into();
            assert!(select_endpoint(&json!([changed])).is_err());
        }
    }

    #[test]
    fn debugger_http_uses_length_without_waiting_for_connection_close() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length:2\r\n\r\n[]";
        assert_eq!(
            http_response_length(response).unwrap(),
            Some(response.len())
        );
        assert!(
            http_response_length(
                b"HTTP/1.1 200 OK\r\nContent-Length:2\r\nContent-Length:3\r\n\r\n[]"
            )
            .is_err()
        );
        assert!(
            http_response_length(b"HTTP/1.1 200 OK\r\nContent-Length:999999999\r\n\r\n").is_err()
        );
        assert!(
            http_response_length(b"HTTP/1.1 200 OK\r\nContent-Length:2\r\n")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn history_keeps_predecessor_and_end_boundary() {
        let now = Instant::now();
        let history: VecDeque<_> = (0..5)
            .map(|index| Observation {
                at: now + Duration::from_secs(index),
                generation: 1,
                channel_id: None,
                participants: Vec::new(),
                valid: true,
            })
            .collect();
        let rows = history_between(
            &history,
            now + Duration::from_millis(1500),
            now + Duration::from_secs(3),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].at, now + Duration::from_secs(1));
        assert_eq!(rows[2].at, now + Duration::from_secs(3));
        assert!(history_between(&history, now + Duration::from_secs(4), now).is_empty());
    }

    #[test]
    fn calibration_rejects_stalls_and_out_of_range_events() {
        let now = Instant::now();
        assert!(Calibration::new(1000.0, now, now + Duration::from_millis(151)).is_none());
        assert!(Calibration::new(f64::NAN, now, now).is_none());
        let clock = Calibration::new(1000.0, now, now + Duration::from_millis(20)).unwrap();
        assert_eq!(clock.map(1010.0), Some(now + Duration::from_millis(20)));
        assert!(clock.map(20000.0).is_none());
    }

    #[test]
    fn metadata_rejects_duplicates_and_names_without_channel() {
        let participant = Participant {
            id: "123".into(),
            name: "Example".into(),
            speaking: true,
            is_self: false,
        };
        let mut sample = Sample {
            time: 0.0,
            channel_id: Some("456".into()),
            participants: vec![participant.clone()],
            valid: true,
        };
        assert!(valid_sample(&sample));
        sample.participants.push(participant);
        assert!(!valid_sample(&sample));
        sample.participants.pop();
        sample.channel_id = None;
        assert!(!valid_sample(&sample));
    }

    #[test]
    fn cdp_ignores_events_and_rejects_runtime_exceptions() {
        use std::net::TcpListener;
        use tungstenite::protocol::Role;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = WebSocket::from_raw_socket(stream, Role::Server, None);
            let request = socket.read().unwrap().into_text().unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            socket
                .send(Message::Text(
                    json!({"method":"Runtime.executionContextCreated","params":{}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            socket.send(Message::Text(json!({"id":request["id"],"result":{"exceptionDetails":{"text":"Synthetic failure"}}}).to_string().into())).unwrap();
        });
        let mut cdp = Cdp {
            socket: WebSocket::from_raw_socket(stream, Role::Client, None),
            next_id: 0,
            bridge: None,
        };
        let result = cdp.call("Runtime.evaluate", json!({"expression":"void 0"}));
        assert_eq!(result.unwrap_err().to_string(), "Voice interface changed");
        server.join().unwrap();
    }

    #[test]
    fn disconnect_clears_current_identity_and_marks_history_invalid() {
        let now = Instant::now();
        let shared = Shared {
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
        };
        shared.publish(Observation {
            at: now,
            generation: 0,
            channel_id: Some("123".into()),
            participants: vec![Participant {
                id: "456".into(),
                name: "Example".into(),
                speaking: true,
                is_self: false,
            }],
            valid: true,
        });
        let first_generation = shared.state.lock().unwrap().generation;
        shared.unavailable("Disconnected");
        let state = shared.state.lock().unwrap();
        assert!(state.snapshot.observation.is_none());
        assert!(!state.history.back().unwrap().valid);
        assert!(state.history.back().unwrap().participants.is_empty());
        assert!(state.generation > first_generation);
    }
}
