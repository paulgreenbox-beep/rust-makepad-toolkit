//! Compile-time opt-in native dispatcher instrumentation.
//!
//! There is no API method or socket control surface. A disposable test process
//! receives one local directory at startup. Gate release uses explicit files,
//! every gate has a maximum lifetime, and the default delivery omits this module.

use crate::agent_api::ApiCommand;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_CONTROL_BYTES: u64 = 4096;
const MAX_GATE_MS: u64 = 45_000;
const DEFAULT_REPLY_MS: u64 = 30_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    schema_version: u64,
    run_id: String,
    #[serde(default = "default_reply_ms")]
    ui_reply_timeout_ms: u64,
    #[serde(default = "default_gate_ms")]
    max_gate_wait_ms: u64,
}

fn default_reply_ms() -> u64 { DEFAULT_REPLY_MS }
fn default_gate_ms() -> u64 { MAX_GATE_MS }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    gate_id: String,
    #[serde(default)]
    request_id: Option<Value>,
}

impl Gate {
    fn matches_after_reply(&self, request_id: &Value, method: &str) -> bool {
        crate::circular_owner_order::after_reply_gate_method_allowed(method) && self.request_id.as_ref() == Some(request_id)
    }
}

struct GateState {
    id: String,
    first_seen: Instant,
    finished: bool,
}

struct EventWriter {
    file: File,
    sequence: u64,
}

struct Harness {
    directory: PathBuf,
    configuration: Configuration,
    origin: Instant,
    events: Mutex<EventWriter>,
    dispatch_gate: Mutex<Option<GateState>>,
    started_gates: Mutex<std::collections::BTreeSet<String>>,
    after_reply_gates: Mutex<std::collections::BTreeSet<String>>,
}

static HARNESS: OnceLock<Result<Option<Harness>, String>> = OnceLock::new();

fn valid_identifier(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn read_control<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| format!("cannot open native harness control: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_CONTROL_BYTES {
        return Err("native harness control exceeds 4096 bytes".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONTROL_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_CONTROL_BYTES {
        return Err("native harness control exceeds 4096 bytes".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid native harness control: {e}"))
}

/// Called before binding the API socket. An invalid requested harness fails closed.
pub(super) fn initialize() -> Result<(), String> {
    let result = HARNESS.get_or_init(|| {
        let Some(directory) = std::env::var_os("AI_REVIT_NATIVE_HARNESS_DIR") else {
            return Ok(None);
        };
        let directory = std::fs::canonicalize(directory).map_err(|e| e.to_string())?;
        if !directory.is_dir() { return Err("native harness directory is not a directory".into()); }
        let configuration: Configuration = read_control(&directory.join("native-boundary-harness.json"))?;
        if configuration.schema_version != 1 || !valid_identifier(&configuration.run_id) {
            return Err("native harness requires schema 1 and a safe run_id".into());
        }
        if !(200..=DEFAULT_REPLY_MS).contains(&configuration.ui_reply_timeout_ms) {
            return Err("native harness UI timeout must be 200..30000 ms".into());
        }
        if !(1..=MAX_GATE_MS).contains(&configuration.max_gate_wait_ms) {
            return Err("native harness gate lifetime must be 1..45000 ms".into());
        }
        let file = OpenOptions::new().write(true).create_new(true)
            .open(directory.join(format!("queue-events-{}.jsonl", std::process::id())))
            .map_err(|e| format!("cannot create fresh native harness events: {e}"))?;
        Ok(Some(Harness {
            directory, configuration, origin: Instant::now(),
            events: Mutex::new(EventWriter { file, sequence: 0 }),
            dispatch_gate: Mutex::new(None),
            started_gates: Mutex::new(std::collections::BTreeSet::new()),
            after_reply_gates: Mutex::new(std::collections::BTreeSet::new()),
        }))
    });
    match result {
        Ok(Some(harness)) => {
            harness.record("harness_initialized", None, json!({
                "ui_reply_timeout_ms":harness.configuration.ui_reply_timeout_ms,
                "max_gate_wait_ms":harness.configuration.max_gate_wait_ms,
                "measurement_scope":"feature-gated local native API/UI dispatcher",
                "production_default_timeout_ms":DEFAULT_REPLY_MS,
            }));
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

fn harness() -> Option<&'static Harness> {
    HARNESS.get().and_then(|result| result.as_ref().ok()).and_then(Option::as_ref)
}

pub(super) fn reply_timeout(default: Duration) -> Duration {
    harness().map_or(default, |h| Duration::from_millis(h.configuration.ui_reply_timeout_ms))
}

fn safe_request_id(id: &Value) -> Value {
    match id {
        Value::Null | Value::Bool(_) | Value::Number(_) => id.clone(),
        Value::String(text) if text.len() <= 128 => id.clone(),
        _ => json!("request_id_omitted_over_size_or_complex"),
    }
}

impl Harness {
    fn record(&self, event: &str, request: Option<(&Value, &str)>, details: Value) {
        let Ok(mut writer) = self.events.lock() else { return; };
        writer.sequence = writer.sequence.saturating_add(1);
        let value = json!({
            "schema_version":1,"run_id":self.configuration.run_id,
            "pid":std::process::id(),"session_id":crate::agent_handler::api_session_id(),
            "sequence":writer.sequence,"event":event,
            "at_unix_us":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros(),
            "at_monotonic_us":self.origin.elapsed().as_micros(),
            "request_id":request.map(|(id,_)|safe_request_id(id)),
            "method":request.map(|(_,method)|method.chars().take(128).collect::<String>()),
            "details":details,
        });
        if serde_json::to_writer(&mut writer.file, &value).is_err()
            || writer.file.write_all(b"\n").is_err() || writer.file.flush().is_err()
        {
            eprintln!("native queue harness event write failed");
        }
    }

    fn gate(&self, name: &str) -> Option<Gate> {
        let path = self.directory.join(name);
        if !path.exists() { return None; }
        match read_control::<Gate>(&path) {
            Ok(gate) if valid_identifier(&gate.gate_id) => Some(gate),
            _ => {
                // Invalid or partially published controls never block the UI.
                None
            }
        }
    }

    fn released(&self, gate: &Gate) -> bool {
        self.directory.join(format!("{}.release", gate.gate_id)).is_file()
    }
}

pub(super) fn request_event(event: &str, id: &Value, method: &str, details: Value) {
    if let Some(harness) = harness() { harness.record(event, Some((id, method)), details); }
}

/// A dispatch gate skips queue draining, leaving the native Makepad UI live.
pub(super) fn dispatch_held() -> bool {
    let Some(harness) = harness() else { return false; };
    let Some(gate) = harness.gate("dispatch-gate.json") else { return false; };
    let Ok(mut state) = harness.dispatch_gate.lock() else { return false; };
    if state.as_ref().is_none_or(|old| old.id != gate.gate_id) {
        *state = Some(GateState { id: gate.gate_id.clone(), first_seen: Instant::now(), finished: false });
        harness.record("dispatch_gate_held", None, json!({"gate_id":gate.gate_id}));
    }
    let current = state.as_mut().expect("gate state set");
    if current.finished { return false; }
    let released = harness.released(&gate);
    let expired = current.first_seen.elapsed() >= Duration::from_millis(harness.configuration.max_gate_wait_ms);
    if released || expired {
        current.finished = true;
        harness.record(if released {"dispatch_gate_released"} else {"dispatch_gate_timed_out"}, None,
            json!({"gate_id":gate.gate_id,"elapsed_ms":current.first_seen.elapsed().as_millis()}));
        return false;
    }
    true
}

/// Only the test build can pause a claimed request. Explicit release and its
/// observed event establish ordering; polling duration alone is not evidence.
pub(super) fn after_start(request: &ApiCommand) {
    let Some(harness) = harness() else { return; };
    let Some(gate) = harness.gate("started-gate.json") else { return; };
    if gate.request_id.as_ref() != Some(&request.id) { return; }
    let Ok(mut used) = harness.started_gates.lock() else { return; };
    if !used.insert(gate.gate_id.clone()) { return; }
    drop(used);
    let started = Instant::now();
    harness.record("started_gate_held", Some((&request.id, &request.method)), json!({"gate_id":gate.gate_id}));
    loop {
        let released = harness.released(&gate);
        let expired = started.elapsed() >= Duration::from_millis(harness.configuration.max_gate_wait_ms);
        if released || expired {
            harness.record(if released {"started_gate_released"} else {"started_gate_timed_out"},
                Some((&request.id, &request.method)),
                json!({"gate_id":gate.gate_id,"elapsed_ms":started.elapsed().as_millis()}));
            return;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

pub(super) fn canonical_document_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The reply has already crossed the UI channel. The transport remains live and
/// may admit the follow-up RPC while this exact native poll is held before Draw.
/// Only an explicit local release returns true; timeout is not a proven ordering.
pub(super) fn after_reply(request: &ApiCommand, capture: impl FnOnce() -> Value) -> bool {
    let Some(harness) = harness() else { return false; };
    let Some(gate) = harness.gate("after-reply-gate.json") else { return false; };
    if !gate.matches_after_reply(&request.id, &request.method) { return false; }
    let Ok(mut used) = harness.after_reply_gates.lock() else { return false; };
    if used.contains(&gate.gate_id) { return false; }
    if used.len() >= 64 {
        harness.record("after_reply_gate_refused", Some((&request.id, &request.method)),
            json!({"gate_id":gate.gate_id,"reason":"bounded 64-gate run limit"}));
        return false;
    }
    used.insert(gate.gate_id.clone());
    drop(used);
    let started = Instant::now();
    harness.record("after_reply_gate_held", Some((&request.id, &request.method)),
        json!({"gate_id":gate.gate_id,"native_state":capture(),"reply_channel_send_completed":true,
            "measurement_scope":"UI handler after response send and before next API poll or Draw"}));
    loop {
        let released = harness.released(&gate);
        let expired = started.elapsed() >= Duration::from_millis(harness.configuration.max_gate_wait_ms);
        if released || expired {
            harness.record(if released {"after_reply_gate_released"} else {"after_reply_gate_timed_out"},
                Some((&request.id, &request.method)),
                json!({"gate_id":gate.gate_id,"elapsed_ms":started.elapsed().as_millis()}));
            return released;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

// Read scalar identity and the process counter only. Never refresh owners here.
pub(super) fn owner_order_capture(document: Option<&ai_revit_core::Document>, open_generation: u64) -> Option<Value> {
    let _ = harness()?; // A disabled harness has no capture/identity side effects.
    let serial = crate::circular_owner_order::snapshot();
    Some(json!({
        "counter_scope":crate::circular_owner_order::COUNTER_SCOPE,
        "owner_poll_serial":serial.map(|v|v.to_string()),
        "serial_state":if serial.is_some(){"KNOWN"}else{"UNKNOWN_OVERFLOW"},
        "api_session_id":crate::agent_handler::api_session_id(),
        "open_generation":open_generation.to_string(),
        "document_uuid":document.map(|d|d.document_uuid.to_string()),
        "runtime_instance_id":document.map(|d|d.runtime_instance_id()),
        "revision":document.map(|d|d.revision().to_string())
    }))
}

pub(super) fn handler_finished(request: &ApiCommand, response: &Value, revision_before: u64, revision_after: u64,
    owner_order_before: Option<Value>, owner_order_after: Option<Value>) {
    request_event(if response.get("ok") == Some(&Value::Bool(true)) && revision_after > revision_before {
        "committed"
    } else { "handler_finished" }, &request.id, &request.method,
        json!({"ok":response.get("ok"),"revision_before":revision_before,"revision_after":revision_after,
            "owner_order_before":owner_order_before,"owner_order_after":owner_order_after}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_identifiers_cannot_escape_the_run_directory() {
        for invalid in ["", "../release", "C:\\temp", "x/y", "x.y", "a b"] {
            assert!(!valid_identifier(invalid), "{invalid}");
        }
        assert!(valid_identifier("expiry-real-30s_01"));
        assert!(!valid_identifier(&"x".repeat(65)));
    }

    #[test]
    fn evidence_omits_complex_or_oversized_request_ids() {
        assert_eq!(safe_request_id(&json!("case-a")), json!("case-a"));
        assert_eq!(safe_request_id(&json!({"nested":"not logged"})), json!("request_id_omitted_over_size_or_complex"));
        assert_eq!(safe_request_id(&json!("x".repeat(129))), json!("request_id_omitted_over_size_or_complex"));
    }

    #[test]
    fn after_reply_gate_requires_exact_typed_request_id_and_mutating_method() {
        let gate = Gate { gate_id:"predraw-01".into(), request_id:Some(json!(u64::MAX)) };
        assert!(gate.matches_after_reply(&json!(u64::MAX), "command.apply"));
        assert!(!gate.matches_after_reply(&json!(u64::MAX.to_string()), "command.apply"));
        assert!(!gate.matches_after_reply(&json!(u64::MAX-1), "command.apply"));
        assert!(!gate.matches_after_reply(&json!(u64::MAX), "drawing.ui.state"));
        let unbound = Gate { gate_id:"predraw-02".into(), request_id:None };
        assert!(!unbound.matches_after_reply(&Value::Null, "command.apply"));
    }
}
