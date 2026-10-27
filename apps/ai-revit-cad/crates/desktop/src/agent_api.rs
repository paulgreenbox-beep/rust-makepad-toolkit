//! Loopback JSON-line API transport for the AI Revit desktop app.
//!
//! Each TCP connection carries one UTF-8 JSON object terminated by a newline
//! and receives one JSON response line. Requests are dispatched to the UI
//! thread through `ApiCommand`; this module does not perform model changes.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 48728;
const PROTOCOL_VERSION: u64 = 1;
const MAX_REQUEST_BYTES: usize = 1_048_576;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const UI_REPLY_TIMEOUT: Duration = Duration::from_secs(30);
// New circular/ground/material routes reserve a bounded response wrapper. Keep
// legacy ingress contracts unchanged; this guard runs before queue admission.
const CIRCULAR_MAX_ID_BYTES: usize = 4096;
const CIRCULAR_MAX_TOKEN_BYTES: usize = 4096;
const CIRCULAR_MAX_METHOD_BYTES: usize = 128;

/// Returns the loopback address used by the API server.
///
/// Invalid or zero port values fall back to the default, since an ephemeral
/// port would not be discoverable by an external client through this API.
pub fn api_addr() -> String {
    #[cfg(target_os = "linux")]
    { return crate::linux_lifecycle_wire::ENDPOINT.into(); }
    #[cfg(not(target_os = "linux"))]
    {
    let port = std::env::var("AI_REVIT_API_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT);
    format!("127.0.0.1:{port}")
    }
}

/// A request handed from the transport thread to the desktop UI thread.
///
/// The UI handler should send one JSON response envelope through `reply`.
/// The transport overwrites its `id` with the original request ID before
/// writing it to the client.
pub struct ApiCommand {
    pub id: Value,
    pub method: String,
    pub params: Value,
    pub token: Option<String>,
    pub reply: Sender<Value>,
    deadline: Instant,
    state: Arc<AtomicU8>,
}

impl ApiCommand {
    /// Cancellation and dispatch compete for one transition before any UI effect.
    pub(super) fn try_start(&self) -> bool {
        if Instant::now() >= self.deadline {
            let expired = self
                .state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            #[cfg(feature = "native-boundary-harness")]
            self.trace(
                if expired {
                    "expired_before_start"
                } else {
                    "dispatch_refused"
                },
                json!({"state":self.state.load(Ordering::Acquire),"deadline_elapsed":true}),
            );
            #[cfg(not(feature = "native-boundary-harness"))]
            let _ = expired;
            return false;
        }
        let started = self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        #[cfg(feature = "native-boundary-harness")]
        self.trace(
            if started {
                "started"
            } else {
                "dispatch_refused"
            },
            json!({"state":self.state.load(Ordering::Acquire),"deadline_elapsed":false}),
        );
        started
    }
    pub(super) fn complete(&self) {
        self.state.store(3, Ordering::Release);
        #[cfg(feature = "native-boundary-harness")]
        self.trace("completed", json!({"state":3}));
    }
    #[cfg(feature = "native-boundary-harness")]
    fn trace(&self, event: &str, details: Value) {
        crate::queue_harness::request_event(event, &self.id, &self.method, details);
    }
}

/// Starts the loopback API listener and returns its UI-thread request receiver.
///
/// The listener handles one connection at a time. Read, UI reply, and write
/// waits are all bounded, so a stalled client or handler cannot block the
/// listener forever.
pub fn start_server() -> Result<Receiver<ApiCommand>, String> {
    #[cfg(target_os = "linux")]
    {
        crate::linux_lifecycle_wire::trusted()?;
        if let Ok(port) = std::env::var("AI_REVIT_API_PORT") {
            crate::linux_lifecycle_wire::ensure(port == "48728", "LIFECYCLE_PORT_OVERRIDE_REFUSED")?;
        }
    }
    #[cfg(feature = "native-boundary-harness")]
    crate::queue_harness::initialize()?;
    let address = api_addr();
    let listener = TcpListener::bind(&address)
        .map_err(|error| format!("cannot bind AI Revit API at {address}: {error}"))?;
    let (sender, receiver) = mpsc::sync_channel(8);

    std::thread::Builder::new()
        .name("ai-revit-api".to_owned())
        .spawn(move || {
            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => handle_client(stream, &sender),
                    Err(error) => {
                        eprintln!("AI Revit API accept failed: {error}");
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
        .map_err(|error| format!("cannot start AI Revit API: {error}"))?;

    Ok(receiver)
}

struct ApiRequest {
    id: Value,
    method: String,
    params: Value,
    token: Option<String>,
}

struct ApiFailure {
    id: Value,
    message: String,
}

/// Count the ENCODED id without allocating another arbitrary caller JSON value.
struct CircularIdBudget(usize);
impl Write for CircularIdBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("circular request id budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn circular_ingress_guard(object: &serde_json::Map<String, Value>) -> Result<(), ApiFailure> {
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    if ![
        "geotech.circular.",
        "geotech.ground.",
        "geotech.soil_material.",
    ]
    .iter()
    .any(|prefix| method.starts_with(prefix))
    {
        return Ok(());
    }
    // Check before id cloning AND version/token validation: no earlier failure
    // may reflect a giant new-route id into its response envelope.
    if object.get("id").is_some_and(|id| {
        serde_json::to_writer(CircularIdBudget(CIRCULAR_MAX_ID_BYTES), id).is_err()
    }) {
        return Err(ApiFailure {
            id: Value::Null,
            message: "CircularIngressIdBudget: encoded request id exceeds 4096 bytes".into(),
        });
    }
    let message = if method.len() > CIRCULAR_MAX_METHOD_BYTES {
        Some("CircularIngressMethodBudget: method exceeds 128 UTF-8 bytes")
    } else if object
        .get("token")
        .and_then(Value::as_str)
        .is_some_and(|token| token.len() > CIRCULAR_MAX_TOKEN_BYTES)
    {
        Some("CircularIngressTokenBudget: token exceeds 4096 UTF-8 bytes")
    } else {
        None
    };
    if let Some(message) = message {
        return Err(ApiFailure {
            id: object.get("id").cloned().unwrap_or(Value::Null),
            message: message.into(),
        });
    }
    Ok(())
}

fn handle_client(stream: TcpStream, commands: &SyncSender<ApiCommand>) {
    #[cfg(feature = "native-boundary-harness")]
    let reply_timeout = crate::queue_harness::reply_timeout(UI_REPLY_TIMEOUT);
    #[cfg(not(feature = "native-boundary-harness"))]
    let reply_timeout = UI_REPLY_TIMEOUT;
    handle_client_with_timeout(stream, commands, reply_timeout);
}

fn handle_client_with_timeout(
    mut stream: TcpStream,
    commands: &SyncSender<ApiCommand>,
    reply_timeout: Duration,
) {
    if let Err(error) = stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(WRITE_TIMEOUT)))
    {
        eprintln!("AI Revit API could not set socket timeouts: {error}");
        return;
    }

    let response = match read_request(&stream) {
        Ok(request) => {
            let request_id = request.id.clone();
            #[cfg(feature = "native-boundary-harness")]
            let request_method = request.method.clone();
            let (reply, reply_receiver) = mpsc::channel();
            let state = Arc::new(AtomicU8::new(0));
            let command = ApiCommand {
                id: request.id,
                method: request.method,
                params: request.params,
                token: request.token,
                reply,
                deadline: Instant::now() + reply_timeout,
                state: Arc::clone(&state),
            };

            #[cfg(feature = "native-boundary-harness")]
            command.trace("queued", json!({"state":0,"admission":"pending","reply_timeout_ms":reply_timeout.as_millis()}));
            if let Err(error) = commands.try_send(command) {
                #[cfg(feature = "native-boundary-harness")]
                crate::queue_harness::request_event(
                    "queue_rejected",
                    &request_id,
                    &request_method,
                    json!({"reason":if matches!(&error,mpsc::TrySendError::Full(_)){"full"}else{"disconnected"},"executed":false}),
                );
                failure(
                    request_id,
                    match error {
                        mpsc::TrySendError::Full(_) => {
                            "AI Revit request queue is full; request was not executed"
                        }
                        mpsc::TrySendError::Disconnected(_) => "AI Revit UI is unavailable",
                    },
                )
            } else {
                #[cfg(feature = "native-boundary-harness")]
                crate::queue_harness::request_event(
                    "queue_admitted",
                    &request_id,
                    &request_method,
                    json!({"state":state.load(Ordering::Acquire),"capacity":8}),
                );
                match reply_receiver.recv_timeout(reply_timeout) {
                    Ok(reply) => normalize_reply(reply, request_id),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        #[cfg(feature = "native-boundary-harness")]
                        crate::queue_harness::request_event(
                            "reply_wait_elapsed",
                            &request_id,
                            &request_method,
                            json!({"state":state.load(Ordering::Acquire),"wait_number":1}),
                        );
                        if state
                            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                            || state.load(Ordering::Acquire) == 2
                        {
                            #[cfg(feature = "native-boundary-harness")]
                            crate::queue_harness::request_event(
                                "expired_before_start",
                                &request_id,
                                &request_method,
                                json!({"state":2,"origin":"transport","no_ui_effect_dispatched":true}),
                            );
                            failure(
                                request_id,
                                "AI Revit request expired before execution; no UI effect was dispatched",
                            )
                        } else {
                            // An executing atomic transaction cannot be cancelled or
                            // truthfully reported as rejected. Bound the additional wait.
                            match reply_receiver.recv_timeout(reply_timeout) {
                                Ok(reply) => normalize_reply(reply, request_id),
                                Err(_) => {
                                    #[cfg(feature = "native-boundary-harness")]
                                    crate::queue_harness::request_event(
                                        "started_outcome_unknown",
                                        &request_id,
                                        &request_method,
                                        json!({"state":state.load(Ordering::Acquire),"cancellation_guaranteed":false}),
                                    );
                                    let mut value = failure(
                                        request_id,
                                        "execution already started; outcome is not yet known; retry the original command_id for its authoritative receipt",
                                    );
                                    value["request_status"] =
                                        json!("execution_started_outcome_unknown");
                                    value
                                }
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        failure(request_id, "AI Revit UI did not return a reply")
                    }
                }
            }
        }
        Err(error) => failure(error.id, error.message),
    };

    let encoded = match serde_json::to_vec(&response) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("AI Revit API response serialization failed: {error}");
            return;
        }
    };

    let deadline = Instant::now() + WRITE_TIMEOUT;
    if let Err(error) = bounded_write(&mut stream, &encoded, deadline)
        .and_then(|_| bounded_write(&mut stream, b"\n", deadline))
    {
        eprintln!("AI Revit API response write failed: {error}");
    }
}

fn read_request(stream: &TcpStream) -> Result<ApiRequest, ApiFailure> {
    let mut copy = stream.try_clone().map_err(|error| ApiFailure {
        id: Value::Null,
        message: format!("cannot read AI Revit API socket: {error}"),
    })?;
    let line = bounded_request_line(&mut copy, Instant::now() + READ_TIMEOUT).map_err(|error| {
        ApiFailure {
            id: Value::Null,
            message: format!("cannot read AI Revit API request: {error}"),
        }
    })?;

    let has_newline = line.last() == Some(&b'\n');
    let payload_len = if has_newline {
        line.len() - 1
    } else {
        line.len()
    };
    if payload_len > MAX_REQUEST_BYTES {
        return Err(ApiFailure {
            id: Value::Null,
            message: "AI Revit API request exceeds 1 MiB".to_owned(),
        });
    }
    if line.is_empty() {
        return Err(ApiFailure {
            id: Value::Null,
            message: "AI Revit API request is empty".to_owned(),
        });
    }
    if !has_newline {
        return Err(ApiFailure {
            id: Value::Null,
            message: "AI Revit API request must end with a newline".to_owned(),
        });
    }

    let request: Value =
        serde_json::from_slice(&line[..payload_len]).map_err(|error| ApiFailure {
            id: Value::Null,
            message: format!("invalid AI Revit API JSON request: {error}"),
        })?;
    let object = request.as_object().ok_or_else(|| ApiFailure {
        id: Value::Null,
        message: "AI Revit API request must be a JSON object".to_owned(),
    })?;
    #[cfg(target_os = "linux")]
    if object.get("method").and_then(Value::as_str)
        .is_some_and(|method| method.starts_with("diagnostic.lifecycle.")) {
        if line.len() > crate::linux_lifecycle_wire::REQUEST_CAP {
            return Err(ApiFailure { id: Value::Null, message: "LIFECYCLE_STRICT_4096_REQUEST_REFUSED".into() });
        }
        let strict = serde_json::from_slice::<crate::linux_lifecycle_wire::LifecycleRequest>(&line[..payload_len])
            .ok().filter(|request| request.validate().is_ok());
        if strict.is_none() {
            return Err(ApiFailure { id: Value::Null, message: "LIFECYCLE_STRICT_4096_REQUEST_REFUSED".into() });
        }
    }
    #[cfg(target_os = "linux")]
    if object.get("method").and_then(Value::as_str)
        .is_some_and(|method| method.starts_with("diagnostic.gui.")) {
        if line.len() > crate::linux_geotech_gui_wire::REQUEST_CAP {
            return Err(ApiFailure { id: Value::Null, message: "GUI_STRICT_4096_REQUEST_REFUSED".into() });
        }
        let strict = crate::linux_geotech_gui::strict_job_original(
            object.get("method").and_then(Value::as_str).unwrap_or(""),&line[..payload_len])
            .unwrap_or_else(||serde_json::from_slice::<crate::linux_geotech_gui_wire::GuiRequest>(&line[..payload_len])
                .is_ok_and(|request|request.validate().is_ok()));
        if !strict {
            return Err(ApiFailure { id: Value::Null, message: "GUI_STRICT_4096_REQUEST_REFUSED".into() });
        }
    }
    circular_ingress_guard(object)?;
    let id = object.get("id").cloned().ok_or_else(|| ApiFailure {
        id: Value::Null,
        message: "AI Revit API request needs an id".to_owned(),
    })?;
    if let Some(version) = object.get("version") {
        #[cfg(target_os = "linux")]
        let expected_version = crate::linux_geotech_persistence_adapter_guard::protocol_version(
            object.get("method").and_then(Value::as_str).unwrap_or(""));
        #[cfg(not(target_os = "linux"))]
        let expected_version = PROTOCOL_VERSION;
        if version.as_u64() != Some(expected_version) {
            return Err(ApiFailure {
                id,
                message: format!("unsupported AI Revit API version; expected {expected_version}"),
            });
        }
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| !method.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiFailure {
            id: id.clone(),
            message: "AI Revit API request needs a non-empty method".to_owned(),
        })?;
    let token = match object.get("token") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => Some(token.clone()),
        Some(_) => {
            return Err(ApiFailure {
                id,
                message: "AI Revit API token must be a string".to_owned(),
            });
        }
    };

    Ok(ApiRequest {
        id,
        method,
        params: object.get("params").cloned().unwrap_or(Value::Null),
        token,
    })
}

fn bounded_request_line(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut line = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or("absolute request deadline exceeded")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let budget = (MAX_REQUEST_BYTES + 1).saturating_sub(line.len());
        if budget == 0 {
            return Err("AI Revit API request exceeds 1 MiB".into());
        }
        let capacity = budget.min(chunk.len());
        let n = stream
            .read(&mut chunk[..capacity])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(line);
        }
        if let Some(end) = chunk[..n].iter().position(|v| *v == b'\n') {
            line.extend_from_slice(&chunk[..=end]);
            return Ok(line);
        }
        line.extend_from_slice(&chunk[..n]);
    }
}

fn bounded_write(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> Result<(), String> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or("absolute response deadline exceeded")?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let n = stream.write(bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("response stream closed".into());
        }
        bytes = &bytes[n..];
    }
    Ok(())
}

#[cfg(test)]
mod unified_transport_tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    fn command(deadline: Instant) -> ApiCommand {
        let (reply, _) = mpsc::channel();
        ApiCommand {
            id: json!(1),
            method: "command.apply".into(),
            params: json!({}),
            token: None,
            reply,
            deadline,
            state: Arc::new(AtomicU8::new(0)),
        }
    }
    #[test]
    fn expired_requests_cannot_later_claim_and_live_claim_is_exclusive() {
        let expired = command(Instant::now() - Duration::from_millis(1));
        assert!(!expired.try_start());
        assert_eq!(expired.state.load(Ordering::Acquire), 2);
        assert!(!expired.try_start());
        let live = command(Instant::now() + Duration::from_secs(1));
        let winners = std::thread::scope(|scope| {
            let a = scope.spawn(|| live.try_start());
            let b = scope.spawn(|| live.try_start());
            usize::from(a.join().unwrap()) + usize::from(b.join().unwrap())
        });
        assert_eq!(winners, 1);
        assert!(
            live.state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
    }

    fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn response_line(client: TcpStream) -> Value {
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn real_transport_circular_ingress_rejects_oversized_encoded_id_before_queue() {
        for prefix in [
            "geotech.circular.",
            "geotech.ground.",
            "geotech.soil_material.",
        ] {
            let (mut client, server) = loopback_pair();
            let (sender, receiver) = mpsc::sync_channel(8);
            let transport = std::thread::spawn(move || {
                handle_client_with_timeout(server, &sender, Duration::from_millis(200));
            });
            // Decoded id is only 2048 bytes, but encoded JSON is 4098 bytes.
            // Also supply a bad version: the oversized id must never be echoed
            // by that earlier legacy validation failure, either.
            let id = "\n".repeat(CIRCULAR_MAX_ID_BYTES / 2);
            assert_eq!(
                serde_json::to_vec(&id).unwrap().len(),
                CIRCULAR_MAX_ID_BYTES + 2
            );
            let secret = "not-for-response-circular-secret";
            let mut bytes = serde_json::to_vec(&json!({"id":id,"version":99,
                "method":format!("{prefix}inspect"),"params":{},"token":secret}))
            .unwrap();
            bytes.push(b'\n');
            client.write_all(&bytes).unwrap();
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            transport.join().unwrap();
            assert!(line.len() < 512, "rejected id must not inflate the reply");
            assert!(!line.contains(secret));
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], Value::Null);
            assert_eq!(response["ok"], json!(false));
            assert!(
                response["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("CircularIngressIdBudget:")
            );
            // Sender has exited: an empty disconnected queue proves there was
            // no ApiCommand for a UI owner to claim or execute.
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));
        }
    }

    #[test]
    fn real_transport_circular_ingress_preserves_u64_max_id() {
        let (mut client, server) = loopback_pair();
        let (sender, receiver) = mpsc::sync_channel(8);
        let transport = std::thread::spawn(move || {
            handle_client_with_timeout(server, &sender, Duration::from_secs(1));
        });
        let mut bytes = serde_json::to_vec(&json!({"id":u64::MAX,"version":1,
            "method":"geotech.circular.job_status","params":{"sequence":"18446744073709551615"},
            "token":"not-for-response-positive-secret"}))
        .unwrap();
        bytes.push(b'\n');
        client.write_all(&bytes).unwrap();
        let pending = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(pending.id.as_u64(), Some(u64::MAX));
        assert_eq!(pending.params["sequence"], json!(u64::MAX.to_string()));
        assert!(pending.try_start());
        pending.complete();
        // Transport must replace a handler's reply id with the exact original.
        pending
            .reply
            .send(success(json!(0), json!({"observed":true})))
            .unwrap();
        let response = response_line(client);
        assert_eq!(response["id"].as_u64(), Some(u64::MAX));
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["result"]["observed"], json!(true));
        assert!(
            !response
                .to_string()
                .contains("not-for-response-positive-secret")
        );
        assert!(!pending.try_start());
        transport.join().unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn real_transport_circular_ingress_rejects_large_method_or_token_before_queue() {
        for prefix in [
            "geotech.circular.",
            "geotech.ground.",
            "geotech.soil_material.",
        ] {
            for oversized_method in [true, false] {
                let (mut client, server) = loopback_pair();
                let (sender, receiver) = mpsc::sync_channel(8);
                let transport = std::thread::spawn(move || {
                    handle_client_with_timeout(server, &sender, Duration::from_millis(200));
                });
                let method = if oversized_method {
                    format!(
                        "{prefix}{}",
                        "m".repeat(CIRCULAR_MAX_METHOD_BYTES + 1 - prefix.len())
                    )
                } else {
                    format!("{prefix}inspect")
                };
                // Multi-byte UTF-8 checks byte count, not character count.
                let token = if oversized_method {
                    "redacted-secret".into()
                } else {
                    "é".repeat(CIRCULAR_MAX_TOKEN_BYTES / 2 + 1)
                };
                let mut bytes = serde_json::to_vec(&json!({"id":"bounded-id","method":method,
                    "params":{},"token":token}))
                .unwrap();
                bytes.push(b'\n');
                client.write_all(&bytes).unwrap();
                let response = response_line(client);
                transport.join().unwrap();
                assert_eq!(response["id"], json!("bounded-id"));
                assert_eq!(response["ok"], json!(false));
                let error = response["error"].as_str().unwrap();
                assert!(error.starts_with(if oversized_method {
                    "CircularIngressMethodBudget:"
                } else {
                    "CircularIngressTokenBudget:"
                }));
                assert!(response.to_string().len() < 512);
                assert!(!response.to_string().contains(&token));
                assert!(matches!(
                    receiver.try_recv(),
                    Err(mpsc::TryRecvError::Disconnected)
                ));
            }
        }
    }

    #[test]
    fn loopback_circular_ingress_exact_byte_caps_and_legacy_routes_unchanged() {
        for circular in [true, false] {
            let (mut client, server) = loopback_pair();
            let prefix = if circular {
                "geotech.circular."
            } else {
                "legacy.route."
            };
            let extra = usize::from(!circular);
            let id = "i".repeat(CIRCULAR_MAX_ID_BYTES - 2 + extra);
            let method = format!(
                "{prefix}{}",
                "m".repeat(CIRCULAR_MAX_METHOD_BYTES - prefix.len() + extra)
            );
            let token = "é".repeat(CIRCULAR_MAX_TOKEN_BYTES / 2 + extra);
            assert_eq!(
                serde_json::to_vec(&id).unwrap().len(),
                CIRCULAR_MAX_ID_BYTES + extra
            );
            let mut bytes =
                serde_json::to_vec(&json!({"id":id,"method":method,"token":token,"params":{}}))
                    .unwrap();
            bytes.push(b'\n');
            client.write_all(&bytes).unwrap();
            let request = match read_request(&server) {
                Ok(value) => value,
                Err(_) => panic!("accepted byte boundary or legacy route was rejected"),
            };
            assert_eq!(request.id, json!(id));
            assert_eq!(request.method, method);
            assert_eq!(request.token.as_deref(), Some(token.as_str()));
        }
    }

    #[test]
    fn real_transport_expiry_cannot_be_claimed_after_client_rejection() {
        let (mut client, server) = loopback_pair();
        let (sender, receiver) = mpsc::sync_channel(8);
        let transport = std::thread::spawn(move || {
            handle_client_with_timeout(server, &sender, Duration::from_millis(200));
        });
        client
            .write_all(b"{\"id\":\"expiry\",\"method\":\"command.apply\",\"params\":{}}\n")
            .unwrap();
        let pending = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        // Receiving the completed transport reply, rather than a sleep, proves
        // the cancellation won before we attempt the delayed UI claim.
        let response = response_line(client);
        assert_eq!(response["id"], json!("expiry"));
        assert_eq!(response["ok"], json!(false));
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("expired before execution")
        );
        assert_eq!(pending.state.load(Ordering::Acquire), 2);
        assert!(!pending.try_start());
        assert!(!pending.try_start());
        transport.join().unwrap();
    }

    #[test]
    fn real_transport_started_request_returns_final_reply_after_deadline() {
        let (mut client, server) = loopback_pair();
        let (sender, receiver) = mpsc::sync_channel(8);
        let transport = std::thread::spawn(move || {
            handle_client_with_timeout(server, &sender, Duration::from_millis(200));
        });
        client
            .write_all(b"{\"id\":\"started\",\"method\":\"command.apply\",\"params\":{}}\n")
            .unwrap();
        let pending = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(pending.try_start());
        while Instant::now() <= pending.deadline {
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(pending.state.load(Ordering::Acquire), 1);
        pending.complete();
        pending
            .reply
            .send(json!({"id":null,"ok":true,"result":{"committed":true}}))
            .unwrap();
        let response = response_line(client);
        assert_eq!(response["id"], json!("started"));
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["result"]["committed"], json!(true));
        assert!(response.get("request_status").is_none());
        assert!(!pending.try_start());
        transport.join().unwrap();
    }

    #[test]
    fn real_transport_full_queue_rejects_without_replacing_admitted_request() {
        let (mut client, server) = loopback_pair();
        let (sender, receiver) = mpsc::sync_channel(1);
        let admitted = command(Instant::now() + Duration::from_secs(2));
        let admitted_state = Arc::clone(&admitted.state);
        assert!(sender.try_send(admitted).is_ok());
        let transport = std::thread::spawn(move || {
            handle_client_with_timeout(server, &sender, Duration::from_millis(200));
        });
        client
            .write_all(b"{\"id\":\"overflow\",\"method\":\"command.apply\",\"params\":{}}\n")
            .unwrap();
        let response = response_line(client);
        assert_eq!(response["id"], json!("overflow"));
        assert_eq!(response["ok"], json!(false));
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("queue is full")
        );
        let retained = receiver.try_recv().unwrap();
        assert!(Arc::ptr_eq(&retained.state, &admitted_state));
        assert_eq!(retained.state.load(Ordering::Acquire), 0);
        assert!(receiver.try_recv().is_err());
        transport.join().unwrap();
    }
    #[test]
    fn absolute_request_deadline_cannot_be_extended_by_more_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"{}\n").unwrap();
        let error = bounded_request_line(&mut server, Instant::now() - Duration::from_millis(1))
            .unwrap_err();
        assert_eq!(error, "absolute request deadline exceeded");
        assert_eq!(
            bounded_request_line(&mut server, Instant::now() + Duration::from_secs(1)).unwrap(),
            b"{}\n"
        );
    }
    #[test]
    fn loopback_parser_keeps_maximum_u64_and_rejects_wrong_token_type() {
        for (token, valid) in [(json!("secret"), true), (json!(17), false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (server, _) = listener.accept().unwrap();
            let mut bytes=serde_json::to_vec(&json!({"id":u64::MAX,"version":1,"method":"drawing.entities","params":{"drawing_id":"18446744073709551615","offset":u64::MAX},"token":token})).unwrap();
            bytes.push(b'\n');
            client.write_all(&bytes).unwrap();
            match read_request(&server) {
                Ok(request) => {
                    assert!(valid);
                    assert_eq!(request.id.as_u64(), Some(u64::MAX));
                    assert_eq!(request.params["offset"].as_u64(), Some(u64::MAX));
                }
                Err(_) => assert!(!valid),
            }
        }
    }
}

fn normalize_reply(mut reply: Value, id: Value) -> Value {
    let Some(object) = reply.as_object_mut() else {
        return failure(id, "AI Revit UI returned an invalid API reply");
    };
    if object.get("ok").and_then(Value::as_bool).is_none() {
        return failure(id, "AI Revit UI returned an invalid API reply");
    }
    object.insert("id".to_owned(), id);
    reply
}

/// Creates a successful response envelope for an API request.
pub fn success(id: Value, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}

/// Creates a failed response envelope for an API request.
pub fn failure(id: Value, error: impl Into<String>) -> Value {
    json!({"id": id, "ok": false, "error": error.into()})
}
