//! Windows adapter for safe observation operations.
//!
//! The generic contracts live in observation.rs. This adapter only provides
//! trusted telemetry from the active Makepad accessibility tree, owned
//! screenshot captures, and session-owned file roots.

use super::{
    HostError, HostResult, ReliabilityManager, call_host, host_error, invalid, required, to_value,
};
use crate::observation::{
    self, AsyncOperation, AsyncOperationState, ExternalObserversConfig, ExternalRequestPhase,
    ExternalRequestState, ObservationError, ObservationResult, ObservationType, PixelRegion,
    PortableEvidenceRef, TelemetrySnapshot, TraceEvent, TrustedRoots,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const TELEMETRY_NODE_SUFFIX: &str = "telemetry.snapshot";
const LOG_LIMIT: usize = 200;
const LOG_LINE_LIMIT: usize = 1024;

impl ReliabilityManager {
    pub(super) fn observation_operation(
        &mut self,
        session_id: &str,
        operation: &str,
        arguments: &Value,
    ) -> HostResult<Option<Value>> {
        let result = match operation {
            "get_task_state" => self.get_task_state(session_id, arguments),
            "wait_for_task" => self.wait_for_task(session_id, arguments),
            "wait_for_task_state" => self.wait_for_task_state(session_id, arguments),
            "wait_for_task_complete" => self.wait_for_task_complete(session_id, arguments),
            "assert_task_success" => self.assert_task_success(session_id, arguments),
            "get_runtime_milestones" => self.get_runtime_milestones(session_id, arguments),
            "wait_for_runtime_milestone" => self.wait_for_runtime_milestone(session_id, arguments),
            "assert_runtime_milestone" => self.assert_runtime_milestone(session_id, arguments),
            "get_runtime_timeline" => self.get_runtime_timeline(session_id, arguments),
            "get_api_request_state" => self.get_api_request_state(session_id, arguments),
            "wait_for_api_state" => self.wait_for_api_state(session_id, arguments),
            "wait_for_api_response" => self.wait_for_api_response(session_id, arguments),
            "assert_api_success" => self.assert_api_success(session_id, arguments),
            "assert_api_failure" => self.assert_api_failure(session_id, arguments),
            "wait_for_file" => self.wait_for_file_observation(session_id, arguments),
            "assert_file_exists" => self.assert_file_exists_observation(session_id, arguments),
            "assert_file_not_exists" => {
                self.assert_file_not_exists_observation(session_id, arguments)
            }
            "assert_file_size" => self.assert_file_size_observation(session_id, arguments),
            "assert_file_count" => self.assert_file_count_observation(session_id, arguments),
            "assert_file_hash" => self.assert_file_hash_observation(session_id, arguments),
            "wait_for_file_stable" => self.wait_for_file_stable_observation(session_id, arguments),
            "assert_not_blank" => self.assert_not_blank_observation(session_id, arguments),
            "assert_region_changed" => {
                self.assert_region_changed_observation(session_id, arguments)
            }
            "assert_region_unchanged" => {
                self.assert_region_unchanged_observation(session_id, arguments)
            }
            "assert_similarity" | "compare_screenshot" => {
                self.assert_similarity_observation(session_id, arguments)
            }
            "get_makepad_visual_diagnostics" => {
                self.get_makepad_visual_diagnostics(session_id, arguments)
            }
            "get_application_logs" => self.get_application_logs(session_id, arguments),
            "wait_for_log" => self.wait_for_log(session_id, arguments),
            "assert_log_contains" => self.assert_log_contains(session_id, arguments),
            "assert_log_not_contains" => self.assert_log_not_contains(session_id, arguments),
            "assert_trace_event" => self.assert_trace_event(session_id, arguments),
            "assert_no_stale_window" => self.assert_no_stale_window(session_id, arguments),
            "assert_no_pending_task" => self.assert_no_pending_task(session_id, arguments),
            "assert_no_phantom_semantic_node" => {
                self.assert_no_phantom_semantic_node(session_id, arguments)
            }
            _ => return Ok(None),
        }?;
        Ok(Some(result))
    }

    fn observation_success(
        &self,
        observation_type: ObservationType,
        state: impl Into<String>,
        started: Instant,
        last_observation: Value,
        evidence_ref: Option<PortableEvidenceRef>,
    ) -> HostResult<Value> {
        let screenshot_ref = if matches!(&observation_type, ObservationType::Visual) {
            evidence_ref.clone()
        } else {
            None
        };
        let result = ObservationResult::success(
            observation_type,
            state,
            started.elapsed().as_millis(),
            last_observation,
            evidence_ref,
        )
        .with_screenshot_ref(screenshot_ref);
        to_value(result)
    }

    fn telemetry_snapshot(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<TelemetrySnapshot> {
        let telemetry_selector = arguments
            .get("telemetry_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(TELEMETRY_NODE_SUFFIX);
        let telemetry_id = self
            .resolve_control(session_id, telemetry_selector)?
            .automation_id;
        let state = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "read", json!({"id":telemetry_id}))?
        };
        let raw = state
            .get("value")
            .or_else(|| state.get("text"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                host_error(
                    "TELEMETRY_NOT_PUBLISHED",
                    "trusted Makepad telemetry node has no readable JSON value",
                    json!({"telemetry_id":telemetry_id,"state":state}),
                )
            })?;
        let snapshot = TelemetrySnapshot::parse(raw).map_err(observation_host_error)?;
        let observers = ExternalObserversConfig::built_in();
        for request in snapshot.external_requests.values() {
            observers
                .observer(&request.observer_alias)
                .map_err(observation_host_error)?;
        }
        Ok(snapshot)
    }

    fn observation_roots(&self, session_id: &str) -> HostResult<TrustedRoots> {
        let output = self.session(session_id)?.host.output_dir().to_path_buf();
        let workspace = std::env::current_dir()
            .map_err(|error| host_error("FILE_ROOT_NOT_ALLOWED", error.to_string(), Value::Null))?;
        Ok(TrustedRoots::new([
            ("workspace".into(), workspace),
            ("output".into(), output.clone()),
            ("fixture".into(), output.join("fixture")),
            ("temp".into(), output.join("temp")),
        ]))
    }

    fn observation_timeout_ms(&self, arguments: &Value) -> HostResult<u64> {
        let timeout_ms = arguments
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(self.config.default_timeout_ms);
        if timeout_ms == 0 || timeout_ms > observation::MAX_OBSERVATION_TIMEOUT_MS {
            return Err(invalid(format!(
                "timeout_ms must be within 1..={}",
                observation::MAX_OBSERVATION_TIMEOUT_MS
            )));
        }
        Ok(timeout_ms)
    }

    fn wait_snapshot<F>(
        &mut self,
        session_id: &str,
        arguments: &Value,
        observation_type: ObservationType,
        timeout_code: &str,
        timeout_message: &str,
        mut predicate: F,
    ) -> HostResult<Value>
    where
        F: FnMut(&TelemetrySnapshot) -> HostResult<Option<(String, Value)>>,
    {
        let started = Instant::now();
        let timeout_ms = self.observation_timeout_ms(arguments)?;
        loop {
            let snapshot = self.telemetry_snapshot(session_id, arguments)?;
            if let Some((state, observation)) = predicate(&snapshot)? {
                return self.observation_success(
                    observation_type,
                    state,
                    started,
                    observation,
                    None,
                );
            }
            let last_observation = serde_json::to_value(&snapshot).unwrap_or(Value::Null);
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(host_error(
                    timeout_code,
                    timeout_message,
                    json!({"timeout_ms":timeout_ms,"last_observation":last_observation}),
                ));
            }
            thread::sleep(Duration::from_millis(self.config.default_poll_ms.min(250)));
        }
    }

    fn get_task_state(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let operation_id = required(arguments, "operation_id")?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let task = snapshot
            .task(&operation_id)
            .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::RuntimeTask,
            state_name(task.state),
            started,
            json!({"task":task,"telemetry_version":snapshot.version}),
            None,
        )
    }

    fn wait_for_task(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let operation_id = required(arguments, "operation_id")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::RuntimeTask,
            "TASK_NOT_FOUND",
            "runtime task did not appear before the bounded timeout",
            move |snapshot| {
                Ok(snapshot.tasks.get(&operation_id).map(|task| {
                    (
                        state_name(task.state),
                        json!({"task":task,"telemetry_version":snapshot.version}),
                    )
                }))
            },
        )
    }

    fn wait_for_task_state(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let operation_id = required(arguments, "operation_id")?;
        let expected = parse_state(arguments, "state")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::RuntimeTask,
            "TASK_STATE_TIMEOUT",
            "runtime task did not reach the requested state before the bounded timeout",
            move |snapshot| {
                Ok(snapshot.tasks.get(&operation_id).and_then(|task| {
                    (task.state == expected).then(|| {
                        (
                            state_name(task.state),
                            json!({"task":task,"expected_state":expected}),
                        )
                    })
                }))
            },
        )
    }

    fn wait_for_task_complete(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let operation_id = required(arguments, "operation_id")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::RuntimeTask,
            "TASK_STATE_TIMEOUT",
            "runtime task did not enter a terminal state before the bounded timeout",
            move |snapshot| {
                let Some(task) = snapshot.tasks.get(&operation_id) else {
                    return Ok(None);
                };
                if !task.state.terminal() {
                    return Ok(None);
                }
                if task.state == AsyncOperationState::Completed {
                    Ok(Some((
                        state_name(task.state),
                        json!({"task":task,"terminal":true}),
                    )))
                } else {
                    Err(host_error(
                        "TASK_FAILED",
                        "runtime task completed unsuccessfully",
                        json!({"task":task}),
                    ))
                }
            },
        )
    }

    fn assert_task_success(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let operation_id = required(arguments, "operation_id")?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let task = snapshot
            .task(&operation_id)
            .map_err(observation_host_error)?;
        if task.state == AsyncOperationState::Completed && task.error.is_none() {
            self.observation_success(
                ObservationType::RuntimeTask,
                state_name(task.state),
                started,
                json!({"task":task,"accepted":true}),
                None,
            )
        } else {
            Err(host_error(
                "TASK_FAILED",
                "runtime task is not a successful completion",
                json!({"task":task}),
            ))
        }
    }

    fn get_runtime_milestones(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        self.observation_success(
            ObservationType::RuntimeMilestone,
            "observed",
            started,
            json!({"milestones":snapshot.milestones,"telemetry_version":snapshot.version}),
            None,
        )
    }

    fn wait_for_runtime_milestone(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let name = required(arguments, "name")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::RuntimeMilestone,
            "RUNTIME_MILESTONE_TIMEOUT",
            "runtime milestone did not appear before the bounded timeout",
            move |snapshot| {
                Ok(snapshot
                    .milestones
                    .iter()
                    .find(|milestone| milestone.name == name)
                    .map(|milestone| {
                        (
                            "observed".into(),
                            json!({"milestone":milestone,"timeline":snapshot.milestones}),
                        )
                    }))
            },
        )
    }

    fn assert_runtime_milestone(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let name = required(arguments, "name")?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let milestone = snapshot
            .milestones
            .iter()
            .find(|milestone| milestone.name == name)
            .ok_or_else(|| {
                host_error(
                    "RUNTIME_MILESTONE_NOT_FOUND",
                    "runtime milestone is absent from trusted telemetry",
                    json!({"name":name,"timeline":snapshot.milestones}),
                )
            })?;
        self.observation_success(
            ObservationType::RuntimeMilestone,
            "observed",
            started,
            json!({"milestone":milestone,"timeline":snapshot.milestones}),
            None,
        )
    }

    fn get_runtime_timeline(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        self.observation_success(
            ObservationType::RuntimeMilestone,
            "observed",
            started,
            json!({
                "milestones":snapshot.milestones,
                "logs":snapshot.logs.iter().map(bounded_trace).collect::<Vec<_>>()
            }),
            None,
        )
    }

    fn get_api_request_state(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let selector = external_request_selector(arguments)?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let request = select_external_request(&snapshot, &selector)?;
        self.observation_success(
            ObservationType::ExternalRequest,
            api_state_name(request),
            started,
            json!({
                "request":request,
                "transport_is_not_application_acceptance":true,
                "selection":selector.describe()
            }),
            None,
        )
    }

    fn wait_for_api_state(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let selector = external_request_selector(arguments)?;
        let expected = parse_api_state(arguments, "state")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::ExternalRequest,
            "EXTERNAL_REQUEST_TIMEOUT",
            "external request did not reach the requested state before the bounded timeout",
            move |snapshot| match select_external_request(snapshot, &selector) {
                Ok(request) if api_state_matches(request, expected) => Ok(Some((
                    api_state_name(request),
                    json!({
                        "request":request,
                        "expected_state":expected.as_str(),
                        "selection":selector.describe()
                    }),
                ))),
                Ok(_) => Ok(None),
                Err(error) if error.code == "EXTERNAL_REQUEST_NOT_FOUND" => Ok(None),
                Err(error) => Err(error),
            },
        )
    }

    fn wait_for_api_response(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let selector = external_request_selector(arguments)?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::ExternalRequest,
            "EXTERNAL_REQUEST_TIMEOUT",
            "external response was not received before the bounded timeout",
            move |snapshot| {
                let request = match select_external_request(snapshot, &selector) {
                    Ok(request) => request,
                    Err(error) if error.code == "EXTERNAL_REQUEST_NOT_FOUND" => return Ok(None),
                    Err(error) => return Err(error),
                };
                if request_timed_out(request) {
                    return Err(host_error(
                        "EXTERNAL_REQUEST_TIMEOUT",
                        "external request timed out",
                        json!({"request":request,"selection":selector.describe()}),
                    ));
                }
                if let Some(code) = application_failure_code(request) {
                    let message = request
                        .error
                        .as_ref()
                        .map(|error| error.message.clone())
                        .unwrap_or_else(|| {
                            "external response failed application validation".into()
                        });
                    return Err(host_error(
                        code,
                        message,
                        json!({"request":request,"selection":selector.describe()}),
                    ));
                }
                if request_failed(request) {
                    return Err(host_error(
                        "EXTERNAL_REQUEST_FAILED",
                        "external request failed before a response was available",
                        json!({"request":request,"selection":selector.describe()}),
                    ));
                }
                Ok(request.response_received.then(|| {
                    (
                        api_state_name(request),
                        json!({
                            "request":request,
                            "response_received":true,
                            "selection":selector.describe()
                        }),
                    )
                }))
            },
        )
    }

    fn assert_api_success(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let selector = external_request_selector(arguments)?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let request = select_external_request(&snapshot, &selector)?;
        if request_timed_out(request) {
            return Err(host_error(
                "EXTERNAL_REQUEST_TIMEOUT",
                "external request timed out",
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        if let Some(code) = application_failure_code(request) {
            let message = request
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "external response failed application validation".into());
            return Err(host_error(
                code,
                message,
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        if request_failed(request) {
            return Err(host_error(
                "EXTERNAL_REQUEST_FAILED",
                "external request failed",
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        if !request.transport_completed || !request.response_received {
            return Err(host_error(
                "EXTERNAL_RESPONSE_INVALID",
                "transport did not produce a complete response",
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        if request.status_code.is_some_and(|status| status >= 400) {
            return Err(host_error(
                "EXTERNAL_REQUEST_FAILED",
                "external transport returned an unsuccessful status",
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        if !request.application_accepted_response {
            return Err(host_error(
                "EXTERNAL_RESPONSE_NOT_ACCEPTED",
                "application did not accept the completed external response",
                json!({"request":request,"selection":selector.describe()}),
            ));
        }
        self.observation_success(
            ObservationType::ExternalRequest,
            api_state_name(request),
            started,
            json!({
                "request":request,
                "application_accepted_response":true,
                "selection":selector.describe()
            }),
            None,
        )
    }

    fn assert_api_failure(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let selector = external_request_selector(arguments)?;
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let request = select_external_request(&snapshot, &selector)?;
        let failed = request_failed(request)
            || request.status_code.is_some_and(|status| status >= 400)
            || (request.response_received && !request.application_accepted_response);
        if failed {
            self.observation_success(
                ObservationType::ExternalRequest,
                api_state_name(request),
                started,
                json!({
                    "request":request,
                    "failure_observed":true,
                    "selection":selector.describe()
                }),
                None,
            )
        } else {
            Err(host_error(
                "EXTERNAL_REQUEST_FAILED",
                "expected an external request failure but request remains successful or pending",
                json!({"request":request,"selection":selector.describe()}),
            ))
        }
    }

    fn wait_for_file_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let roots = self.observation_roots(session_id)?;
        let state = observation::wait_for_file(
            &roots,
            &root,
            &path,
            self.observation_timeout_ms(arguments)?,
        )
        .map_err(observation_host_error)?;
        let evidence = file_evidence(&roots, &state)?;
        self.observation_success(
            ObservationType::File,
            "exists",
            started,
            json!({"file":state}),
            evidence,
        )
    }

    fn assert_file_exists_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let roots = self.observation_roots(session_id)?;
        let state = observation::assert_file_exists(&roots, &root, &path)
            .map_err(observation_host_error)?;
        let evidence = file_evidence(&roots, &state)?;
        self.observation_success(
            ObservationType::File,
            "exists",
            started,
            json!({"file":state}),
            evidence,
        )
    }

    fn assert_file_not_exists_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let roots = self.observation_roots(session_id)?;
        let state = observation::assert_file_not_exists(&roots, &root, &path)
            .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::File,
            "not_exists",
            started,
            json!({"file":state}),
            None,
        )
    }

    fn assert_file_size_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let expected_bytes = arguments
            .get("expected_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("expected_bytes must be an unsigned integer"))?;
        let roots = self.observation_roots(session_id)?;
        let state = observation::assert_file_size(&roots, &root, &path, expected_bytes)
            .map_err(observation_host_error)?;
        let evidence = file_evidence(&roots, &state)?;
        self.observation_success(
            ObservationType::File,
            "size_matches",
            started,
            json!({"file":state}),
            evidence,
        )
    }

    fn assert_file_count_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let root = required(arguments, "root")?;
        let path = required(arguments, "path")?;
        let expected_count = arguments
            .get("count")
            .or_else(|| arguments.get("expected_count"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| invalid("count must be an unsigned integer"))?;
        let pattern = arguments.get("pattern").and_then(Value::as_str);
        let roots = self.observation_roots(session_id)?;
        let result = observation::assert_file_count(&roots, &root, &path, pattern, expected_count)
            .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::File,
            "count_matches",
            started,
            json!({"directory":result}),
            None,
        )
    }

    fn assert_file_hash_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let expected_sha256 = arguments
            .get("sha256")
            .or_else(|| arguments.get("expected_sha256"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| invalid("sha256 must be a non-empty SHA-256 string"))?
            .to_owned();
        let roots = self.observation_roots(session_id)?;
        let state = observation::assert_file_hash(&roots, &root, &path, &expected_sha256)
            .map_err(observation_host_error)?;
        let evidence = file_evidence(&roots, &state)?;
        self.observation_success(
            ObservationType::File,
            "hash_matches",
            started,
            json!({"file":state}),
            evidence,
        )
    }

    fn wait_for_file_stable_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let (root, path) = file_target(arguments)?;
        let stable_for_ms = arguments
            .get("unchanged_for_ms")
            .or_else(|| arguments.get("stable_for_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(500);
        let roots = self.observation_roots(session_id)?;
        let state = observation::wait_for_file_stable(
            &roots,
            &root,
            &path,
            self.observation_timeout_ms(arguments)?,
            stable_for_ms,
        )
        .map_err(observation_host_error)?;
        let evidence = file_evidence(&roots, &state)?;
        self.observation_success(
            ObservationType::File,
            "stable",
            started,
            json!({"file":state,"unchanged_for_ms":stable_for_ms}),
            evidence,
        )
    }

    fn assert_not_blank_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let roots = self.observation_roots(session_id)?;
        let (path, evidence) = self.current_capture(session_id, arguments, &roots)?;
        let region = pixel_region(arguments, "region")?;
        let masks = pixel_masks(arguments)?;
        let minimum_variance = arguments
            .get("minimum_variance")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let metrics = observation::assert_not_blank(&path, region, &masks, minimum_variance)
            .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::Visual,
            "not_blank",
            started,
            json!({"metrics":metrics}),
            Some(evidence),
        )
    }

    fn assert_region_changed_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let roots = self.observation_roots(session_id)?;
        let baseline = self.baseline_capture(arguments, &roots)?;
        let (current, evidence) = self.current_capture(session_id, arguments, &roots)?;
        let region = pixel_region(arguments, "region")?;
        let masks = pixel_masks(arguments)?;
        let minimum_changed_fraction = arguments
            .get("minimum_changed_fraction")
            .and_then(Value::as_f64)
            .unwrap_or(0.01);
        let metrics = observation::assert_region_changed(
            &baseline,
            &current,
            region,
            &masks,
            minimum_changed_fraction,
        )
        .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::Visual,
            "changed",
            started,
            json!({"metrics":metrics,"baseline":baseline,"current":current}),
            Some(evidence),
        )
    }

    fn assert_region_unchanged_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let roots = self.observation_roots(session_id)?;
        let baseline = self.baseline_capture(arguments, &roots)?;
        let (current, evidence) = self.current_capture(session_id, arguments, &roots)?;
        let region = pixel_region(arguments, "region")?;
        let masks = pixel_masks(arguments)?;
        let maximum_changed_fraction = arguments
            .get("maximum_changed_fraction")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let metrics = observation::assert_region_unchanged(
            &baseline,
            &current,
            region,
            &masks,
            maximum_changed_fraction,
        )
        .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::Visual,
            "unchanged",
            started,
            json!({"metrics":metrics,"baseline":baseline,"current":current}),
            Some(evidence),
        )
    }

    fn assert_similarity_observation(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let roots = self.observation_roots(session_id)?;
        let baseline = self.baseline_capture(arguments, &roots)?;
        let (current, evidence) = self.current_capture(session_id, arguments, &roots)?;
        let region = pixel_region(arguments, "region")?;
        let masks = pixel_masks(arguments)?;
        let minimum_similarity = arguments
            .get("minimum_similarity")
            .and_then(Value::as_f64)
            .unwrap_or(0.99);
        let metrics =
            observation::assert_similarity(&baseline, &current, region, &masks, minimum_similarity)
                .map_err(observation_host_error)?;
        self.observation_success(
            ObservationType::Visual,
            "similar",
            started,
            json!({"metrics":metrics,"baseline":baseline,"current":current}),
            Some(evidence),
        )
    }

    fn get_makepad_visual_diagnostics(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let roots = self.observation_roots(session_id)?;
        let (path, evidence) = self.current_capture(session_id, arguments, &roots)?;
        let region = pixel_region(arguments, "region")?;
        let masks = pixel_masks(arguments)?;
        let metrics = observation::assert_not_blank(&path, region, &masks, -1.0)
            .map_err(observation_host_error)?;
        let tree = self.inspect_compact(session_id, &json!({"compact":true,"max_depth":8}))?;
        // Telemetry is optional for diagnostics: a missing bridge must be explicit
        // rather than turning screenshot/tree evidence into fabricated runtime state.
        let telemetry = self.telemetry_snapshot(session_id, arguments).ok();
        let runtime = makepad_visual_diagnostics(telemetry.as_ref());
        self.observation_success(
            ObservationType::Visual,
            "diagnosed",
            started,
            json!({"capture":path,"metrics":metrics,"tree":tree,"runtime":runtime}),
            Some(evidence),
        )
    }

    fn get_application_logs(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let logs = self.observed_logs(session_id, arguments)?;
        self.observation_success(
            ObservationType::Trace,
            "observed",
            started,
            json!({"logs":logs.iter().map(bounded_trace).collect::<Vec<_>>()}),
            None,
        )
    }

    fn wait_for_log(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let needle = required(arguments, "contains")?;
        self.wait_snapshot(
            session_id,
            arguments,
            ObservationType::Trace,
            "TRACE_TIMEOUT",
            "expected log content did not appear before the bounded timeout",
            move |snapshot| {
                Ok(snapshot
                    .logs
                    .iter()
                    .find(|trace| trace_matches(trace, &needle))
                    .map(|trace| ("matched".into(), json!({"log":bounded_trace(trace)}))))
            },
        )
    }

    fn assert_log_contains(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let needle = required(arguments, "contains")?;
        let logs = self.observed_logs(session_id, arguments)?;
        let trace = logs.iter().find(|trace| trace_matches(trace, &needle)).ok_or_else(|| {
            host_error(
                "TRACE_NOT_FOUND",
                "expected log content is absent",
                json!({"contains":needle,"logs":logs.iter().map(bounded_trace).collect::<Vec<_>>() }),
            )
        })?;
        self.observation_success(
            ObservationType::Trace,
            "matched",
            started,
            json!({"log":bounded_trace(trace)}),
            None,
        )
    }

    fn assert_log_not_contains(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let needle = required(arguments, "contains")?;
        let logs = self.observed_logs(session_id, arguments)?;
        if let Some(trace) = logs.iter().find(|trace| trace_matches(trace, &needle)) {
            return Err(host_error(
                "TRACE_UNEXPECTED",
                "unexpected log content is present",
                json!({"contains":needle,"log":bounded_trace(trace)}),
            ));
        }
        self.observation_success(
            ObservationType::Trace,
            "absent",
            started,
            json!({"contains":needle}),
            None,
        )
    }

    fn assert_trace_event(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let expected = required(arguments, "event")?;
        let logs = self.observed_logs(session_id, arguments)?;
        let trace = logs.iter().find(|trace| trace.event == expected).ok_or_else(|| {
            host_error(
                "TRACE_NOT_FOUND",
                "expected structured trace event is absent",
                json!({"event":expected,"logs":logs.iter().map(bounded_trace).collect::<Vec<_>>() }),
            )
        })?;
        self.observation_success(
            ObservationType::Trace,
            "matched",
            started,
            json!({"trace":bounded_trace(trace)}),
            None,
        )
    }

    fn assert_no_stale_window(
        &mut self,
        session_id: &str,
        _arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        self.refresh_session_windows(session_id)?;
        let health = self
            .session_mut(session_id)?
            .host
            .test_host_mut()
            .get_runtime_health();
        let session = self.session(session_id)?.state.clone();
        let stale = !health.healthy
            || session.processes.iter().any(|process| !process.alive)
            || session_has_stale_window(&session);
        if stale {
            return Err(host_error(
                "STALE_WINDOW",
                "session has a stale process, hidden window, or unhealthy runtime",
                json!({"runtime_health":health,"session":session}),
            ));
        }
        self.observation_success(
            ObservationType::Ui,
            "healthy",
            started,
            json!({"runtime_health":health,"windows":session.windows}),
            None,
        )
    }

    fn assert_no_pending_task(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let started = Instant::now();
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let pending = snapshot
            .tasks
            .values()
            .filter(|task| !task.state.terminal())
            .cloned()
            .collect::<Vec<AsyncOperation>>();
        if !pending.is_empty() {
            return Err(host_error(
                "PENDING_TASKS",
                "trusted telemetry still reports non-terminal tasks",
                json!({"pending_tasks":pending}),
            ));
        }
        self.observation_success(
            ObservationType::RuntimeTask,
            "no_pending_tasks",
            started,
            json!({"tasks":snapshot.tasks}),
            None,
        )
    }

    fn assert_no_phantom_semantic_node(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let started = Instant::now();
        let id = required(arguments, "id")?;
        match self.resolve_control(session_id, &id) {
            Ok(control) => Err(host_error(
                "PHANTOM_SEMANTIC_NODE",
                "semantic node unexpectedly exists",
                json!({"id":id,"control":control}),
            )),
            Err(error) if error.code == "ELEMENT_NOT_FOUND" => self.observation_success(
                ObservationType::Ui,
                "absent",
                started,
                json!({"id":id}),
                None,
            ),
            Err(error) => Err(error),
        }
    }

    fn current_capture(
        &mut self,
        session_id: &str,
        arguments: &Value,
        roots: &TrustedRoots,
    ) -> HostResult<(PathBuf, PortableEvidenceRef)> {
        if let Some(path) = arguments
            .get("current_path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let evidence = roots
                .portable_evidence_ref(Path::new(path))
                .map_err(observation_host_error)?;
            return Ok((PathBuf::from(&evidence.resolved_path), evidence));
        }
        let capture = self.screenshot_window(session_id)?;
        let path = capture
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("screenshot capture returned no path"))?;
        let evidence = roots
            .portable_evidence_ref(Path::new(path))
            .map_err(observation_host_error)?;
        Ok((PathBuf::from(&evidence.resolved_path), evidence))
    }

    fn baseline_capture(&self, arguments: &Value, roots: &TrustedRoots) -> HostResult<PathBuf> {
        let baseline = required(arguments, "baseline_path")?;
        let evidence = roots
            .portable_evidence_ref(Path::new(&baseline))
            .map_err(observation_host_error)?;
        Ok(PathBuf::from(evidence.resolved_path))
    }

    fn observed_logs(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<Vec<TraceEvent>> {
        let snapshot = self.telemetry_snapshot(session_id, arguments)?;
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(LOG_LIMIT)
            .min(LOG_LIMIT);
        let start = snapshot.logs.len().saturating_sub(limit);
        Ok(snapshot.logs[start..].to_vec())
    }
}

fn makepad_visual_diagnostics(snapshot: Option<&TelemetrySnapshot>) -> Value {
    let runtime = snapshot.map(|snapshot| &snapshot.runtime);
    let visual = runtime.and_then(|runtime| runtime.get("visual"));
    let source = match runtime
        .and_then(|runtime| runtime.get("source"))
        .and_then(Value::as_str)
    {
        Some("makepad_accessibility_bridge") | Some("makepad") => "makepad",
        Some("native") => "native",
        Some("inferred") => "inferred",
        _ => "unknown",
    };
    json!({
        "source":source,
        "telemetry_published":snapshot.is_some(),
        "viewport":visual_field(visual, "viewport"),
        "clip":visual_field(visual, "clip"),
        "z_order":visual_field(visual, "z_order"),
        "draw_called":visual_field(visual, "draw_called"),
        "scroll_offset":visual_field(visual, "scroll_offset"),
        "zoom":visual_field(visual, "zoom"),
        "layout_generation":visual_field(visual, "layout_generation"),
        "canvas_health":visual_field(visual, "canvas_health"),
    })
}

fn visual_field(visual: Option<&Value>, field: &str) -> Value {
    visual
        .and_then(|visual| visual.get(field))
        .cloned()
        .unwrap_or_else(|| Value::String("Unknown".into()))
}

fn observation_host_error(error: ObservationError) -> HostError {
    host_error(&error.code, error.message, error.details)
}

fn session_has_stale_window(session: &super::AutomationSession) -> bool {
    let mut primary_windows = 0usize;
    for window in &session.windows {
        if !is_primary_session_window(window) {
            continue;
        }
        primary_windows = primary_windows.saturating_add(1);
        if !window.visible {
            return true;
        }
    }
    primary_windows == 0
}

fn is_primary_session_window(window: &super::SessionWindow) -> bool {
    !window.automation_id.trim().is_empty() || window.control_type.eq_ignore_ascii_case("Window")
}

fn state_name(state: AsyncOperationState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Unknown".into())
}

#[derive(Clone, Debug)]
enum ExternalRequestSelector {
    RequestId(String),
    ObserverAlias(String),
}

impl ExternalRequestSelector {
    fn describe(&self) -> Value {
        match self {
            Self::RequestId(request_id) => json!({"request_id":request_id}),
            Self::ObserverAlias(observer_alias) => json!({"observer_alias":observer_alias}),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ApiExpectedState {
    Normalized(AsyncOperationState),
    Phase(ExternalRequestPhase),
}

impl ApiExpectedState {
    fn as_str(self) -> String {
        match self {
            Self::Normalized(state) => state_name(state),
            Self::Phase(phase) => serde_json::to_value(phase)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "Unknown".into()),
        }
    }
}

fn external_request_selector(arguments: &Value) -> HostResult<ExternalRequestSelector> {
    if let Some(request_id) = arguments
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(ExternalRequestSelector::RequestId(request_id.into()));
    }
    if let Some(observer_alias) = arguments
        .get("observer_alias")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(ExternalRequestSelector::ObserverAlias(
            observer_alias.into(),
        ));
    }
    Err(invalid("request_id or observer_alias is required"))
}

fn select_external_request<'a>(
    snapshot: &'a TelemetrySnapshot,
    selector: &ExternalRequestSelector,
) -> HostResult<&'a ExternalRequestState> {
    match selector {
        ExternalRequestSelector::RequestId(request_id) => snapshot
            .external_request(request_id)
            .map_err(observation_host_error),
        ExternalRequestSelector::ObserverAlias(observer_alias) => {
            let requests = snapshot
                .external_requests
                .values()
                .filter(|request| request.observer_alias == *observer_alias)
                .collect::<Vec<_>>();
            match requests.as_slice() {
                [request] => Ok(*request),
                [] => Err(host_error(
                    "EXTERNAL_REQUEST_NOT_FOUND",
                    "trusted observer alias has no published request",
                    json!({"observer_alias":observer_alias}),
                )),
                _ => Err(host_error(
                    "EXTERNAL_REQUEST_AMBIGUOUS",
                    "trusted observer alias has multiple published requests; use request_id",
                    json!({
                        "observer_alias":observer_alias,
                        "request_ids":requests.iter().map(|request| &request.request_id).collect::<Vec<_>>()
                    }),
                )),
            }
        }
    }
}

fn parse_api_state(arguments: &Value, key: &str) -> HostResult<ApiExpectedState> {
    let value = required(arguments, key)?;
    if let Ok(state) = serde_json::from_value(Value::String(value.clone())) {
        return Ok(ApiExpectedState::Normalized(state));
    }
    serde_json::from_value(Value::String(value.clone()))
        .map(ApiExpectedState::Phase)
        .map_err(|error| {
            host_error(
                "INVALID_ARGUMENT",
                format!("{key} must be an AsyncOperationState or ExternalRequestPhase: {error}"),
                json!({"value":value}),
            )
        })
}

fn api_state_matches(request: &ExternalRequestState, expected: ApiExpectedState) -> bool {
    match expected {
        ApiExpectedState::Normalized(state) => request.state == state,
        ApiExpectedState::Phase(phase) => request.phase == phase,
    }
}

fn api_state_name(request: &ExternalRequestState) -> String {
    if request.phase != ExternalRequestPhase::Unknown {
        serde_json::to_value(request.phase)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| state_name(request.state))
    } else {
        state_name(request.state)
    }
}

fn application_failure_code(request: &ExternalRequestState) -> Option<&str> {
    let code = request.error.as_ref()?.code.as_str();
    matches!(
        code,
        "EXTERNAL_RESPONSE_INVALID" | "EXTERNAL_RESPONSE_NOT_ACCEPTED"
    )
    .then_some(code)
}

fn request_timed_out(request: &ExternalRequestState) -> bool {
    request.state == AsyncOperationState::TimedOut
        || request.phase == ExternalRequestPhase::TimedOut
}

fn request_failed(request: &ExternalRequestState) -> bool {
    matches!(
        request.state,
        AsyncOperationState::Failed
            | AsyncOperationState::Cancelled
            | AsyncOperationState::TimedOut
    ) || matches!(
        request.phase,
        ExternalRequestPhase::Failed
            | ExternalRequestPhase::Cancelled
            | ExternalRequestPhase::TimedOut
    ) || request.error.is_some()
}

fn parse_state(arguments: &Value, key: &str) -> HostResult<AsyncOperationState> {
    let value = required(arguments, key)?;
    serde_json::from_value(Value::String(value.clone())).map_err(|error| {
        host_error(
            "INVALID_ARGUMENT",
            format!("{key} must be an AsyncOperationState: {error}"),
            json!({"value":value}),
        )
    })
}

fn file_target(arguments: &Value) -> HostResult<(String, String)> {
    Ok((required(arguments, "root")?, required(arguments, "path")?))
}

fn file_evidence(
    roots: &TrustedRoots,
    state: &observation::FileState,
) -> HostResult<Option<PortableEvidenceRef>> {
    if state.exists {
        roots
            .portable_evidence_ref(Path::new(&state.resolved_path))
            .map(Some)
            .map_err(observation_host_error)
    } else {
        Ok(None)
    }
}

fn pixel_region(arguments: &Value, key: &str) -> HostResult<Option<PixelRegion>> {
    arguments
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| invalid(format!("{key} must be a pixel region object: {error}")))
        })
        .transpose()
}

fn pixel_masks(arguments: &Value) -> HostResult<Vec<PixelRegion>> {
    arguments
        .get("masks")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| invalid(format!("masks must be pixel region objects: {error}")))
        })
        .transpose()
        .map(|masks| masks.unwrap_or_default())
}

fn bounded_trace(trace: &TraceEvent) -> Value {
    let mut value = scrub_secrets(
        serde_json::to_value(trace).unwrap_or_else(|_| json!({"event":"trace_encode_failed"})),
    );
    let shortened = value
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| message.chars().count() > LOG_LINE_LIMIT)
        .map(|message| message.chars().take(LOG_LINE_LIMIT).collect::<String>());
    if let Some(shortened) = shortened {
        value["message"] = Value::String(format!("{shortened}…"));
    }
    value
}

fn trace_matches(trace: &TraceEvent, needle: &str) -> bool {
    serde_json::to_string(&bounded_trace(trace))
        .map(|value| value.contains(needle))
        .unwrap_or(false)
}

fn scrub_secrets(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(scrub_secrets).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let sensitive = ["token", "secret", "password", "authorization"]
                        .iter()
                        .any(|needle| key.to_ascii_lowercase().contains(needle));
                    (
                        key,
                        if sensitive {
                            Value::String("[REDACTED]".into())
                        } else {
                            scrub_secrets(value)
                        },
                    )
                })
                .collect(),
        ),
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_diagnostics_keep_telemetry_facts_and_mark_gaps_unknown() {
        let snapshot = TelemetrySnapshot {
            runtime: json!({"source":"makepad_accessibility_bridge","visual":{"viewport":{"width":800,"height":600},"draw_called":false}}),
            ..TelemetrySnapshot::default()
        };
        let diagnostics = makepad_visual_diagnostics(Some(&snapshot));
        assert_eq!(diagnostics["source"], "makepad");
        assert_eq!(diagnostics["telemetry_published"], true);
        assert_eq!(diagnostics["draw_called"], false);
        assert_eq!(diagnostics["clip"], "Unknown");

        let unavailable = makepad_visual_diagnostics(None);
        assert_eq!(unavailable["source"], "unknown");
        assert_eq!(unavailable["viewport"], "Unknown");
        assert_eq!(unavailable["telemetry_published"], false);
    }

    #[test]
    fn api_application_failure_codes_are_not_collapsed() {
        for code in [
            "EXTERNAL_RESPONSE_INVALID",
            "EXTERNAL_RESPONSE_NOT_ACCEPTED",
        ] {
            let request = ExternalRequestState {
                error: Some(crate::observation::ObservationFailure {
                    code: code.into(),
                    message: "fixture failure".into(),
                    details: Value::Null,
                }),
                ..ExternalRequestState::default()
            };
            assert_eq!(application_failure_code(&request), Some(code));
        }
        let generic = ExternalRequestState {
            error: Some(crate::observation::ObservationFailure {
                code: "EXTERNAL_REQUEST_FAILED".into(),
                message: "transport failure".into(),
                details: Value::Null,
            }),
            ..ExternalRequestState::default()
        };
        assert_eq!(application_failure_code(&generic), None);
    }

    #[test]
    fn stale_window_ignores_hidden_auxiliary_panes_but_requires_a_visible_primary_window() {
        let primary_window = |visible| super::super::SessionWindow {
            session_id: "session:fixture".into(),
            process_id: 7,
            window_handle: "0x7".into(),
            window_id: "window:main".into(),
            name: "Fixture".into(),
            control_type: "Window".into(),
            automation_id: "window.main".into(),
            bounds: toolkit_windows_automation::Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            visible,
        };
        let hidden_ime_pane = super::super::SessionWindow {
            session_id: "session:fixture".into(),
            process_id: 7,
            window_handle: "0x8".into(),
            window_id: "window:ime".into(),
            name: "Default IME".into(),
            control_type: "Pane".into(),
            automation_id: String::new(),
            bounds: toolkit_windows_automation::Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            visible: false,
        };
        let session = |windows| super::super::AutomationSession {
            session_id: "session:fixture".into(),
            processes: Vec::new(),
            windows,
            active_window: Some("window.main".into()),
            action_log: Vec::new(),
            evidence: Vec::new(),
        };

        assert!(!session_has_stale_window(&session(vec![
            primary_window(true),
            hidden_ime_pane.clone(),
        ])));
        assert!(session_has_stale_window(&session(vec![
            primary_window(false),
            hidden_ime_pane.clone(),
        ])));
        assert!(session_has_stale_window(&session(vec![hidden_ime_pane])));
    }
}
