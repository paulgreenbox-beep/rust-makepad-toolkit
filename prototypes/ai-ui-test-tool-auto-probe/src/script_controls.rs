//! Restricted local Script API controls.
//!
//! This module deliberately supports bounded, declarative controls. It never
//! evaluates code, dispatches arbitrary HTTP, or repeats UI mutations. Its
//! one click-and-wait control dispatches one predeclared semantic click and
//! then performs only read-only postcondition observation.

use crate::{
    api::{AiUiClient, Transport},
    catalog,
};
use serde_json::{Map, Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};

pub const MAX_REPEAT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_REPEAT_ITERATIONS: u64 = 1_000;
/// Long-running application workflows may legitimately contain several
/// bounded provider requests. This remains finite and read-only.
pub const MAX_OUTCOME_TIMEOUT_MS: u64 = 14_400_000;
pub const MAX_OUTCOME_ITERATIONS: u64 = 14_400;
const DEFAULT_REPEAT_INTERVAL_MS: u64 = 50;
const MIN_REPEAT_INTERVAL_MS: u64 = 10;
const MAX_REPEAT_INTERVAL_MS: u64 = 1_000;

const READ_ONLY_OBSERVATION_OPERATIONS: &[&str] = &[
    "get_task_state",
    "get_runtime_milestones",
    "get_runtime_timeline",
    "get_api_request_state",
    "get_application_logs",
    "read",
];

#[derive(Clone, Debug)]
pub(crate) enum JsonPathSegment {
    Field(String),
    Index(usize),
}

#[derive(Clone, Debug)]
enum ValueMatcher {
    Equals(Value),
    NotEquals(Value),
    In(Vec<Value>),
    ContainsAny(Vec<String>),
}

#[derive(Clone, Debug)]
struct RepeatPlan {
    operation: String,
    arguments: Map<String, Value>,
    path: String,
    matcher: ValueMatcher,
    timeout_ms: u64,
    max_iterations: u64,
    interval_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutcomeKind {
    Success,
    Failure,
}

impl OutcomeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

#[derive(Clone, Debug)]
struct OutcomeProbe {
    kind: OutcomeKind,
    id: String,
    path: String,
    matcher: ValueMatcher,
    visible: Option<bool>,
}

#[derive(Clone, Debug)]
struct OutcomePlan {
    outcomes: Vec<OutcomeProbe>,
    timeout_ms: u64,
    max_iterations: u64,
    interval_ms: u64,
}

/// Normalize the two block syntaxes into ordinary declarative steps before
/// deserialization. A when block distributes one safe condition over its
/// direct children; it does not introduce a general-purpose control language.
pub fn normalize_script_controls(mut value: Value) -> Result<Value, Vec<String>> {
    let Some(object) = value.as_object_mut() else {
        return Ok(value);
    };
    let mut errors = Vec::new();
    for phase in ["setup", "steps", "teardown"] {
        let Some(raw_steps) = object.get(phase).cloned() else {
            continue;
        };
        let Some(raw_steps) = raw_steps.as_array() else {
            continue;
        };
        match normalize_steps(raw_steps, phase) {
            Ok(steps) => {
                object.insert(phase.into(), Value::Array(steps));
            }
            Err(mut phase_errors) => errors.append(&mut phase_errors),
        }
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(errors)
    }
}

fn normalize_steps(raw_steps: &[Value], phase: &str) -> Result<Vec<Value>, Vec<String>> {
    let mut normalized = Vec::new();
    let mut errors = Vec::new();
    for (index, raw) in raw_steps.iter().enumerate() {
        let Some(object) = raw.as_object() else {
            normalized.push(raw.clone());
            continue;
        };
        let has_operation = object.contains_key("op");
        if !has_operation && object.contains_key("when") && object.contains_key("steps") {
            let allowed = ["when", "steps"];
            let unexpected = object
                .keys()
                .filter(|key| !allowed.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !unexpected.is_empty() {
                errors.push(format!(
                    "{phase}[{index}] when block only accepts when and steps; unexpected: {}",
                    unexpected.join(", ")
                ));
                continue;
            }
            let Some(children) = object.get("steps").and_then(Value::as_array) else {
                errors.push(format!("{phase}[{index}] when.steps must be an array"));
                continue;
            };
            let condition = object.get("when").cloned().unwrap_or(Value::Null);
            match normalize_steps(children, phase) {
                Ok(children) => {
                    for mut child in children {
                        let Some(child_object) = child.as_object_mut() else {
                            errors.push(format!(
                                "{phase}[{index}] when.steps may contain only step objects"
                            ));
                            continue;
                        };
                        if child_object.contains_key("when") {
                            errors.push(format!(
                                "{phase}[{index}] nested when conditions are not supported"
                            ));
                            continue;
                        }
                        child_object.insert("when".into(), condition.clone());
                        normalized.push(child);
                    }
                }
                Err(mut nested) => errors.append(&mut nested),
            }
            continue;
        }
        if !has_operation && object.contains_key("repeat_until") {
            if object.contains_key("steps") {
                errors.push(format!(
                    "{phase}[{index}] repeat_until observes one read-only operation and cannot contain steps"
                ));
                continue;
            }
            let mut control = object.clone();
            control.insert("op".into(), Value::String("repeat_until".into()));
            normalized.push(Value::Object(control));
            continue;
        }
        normalized.push(raw.clone());
    }
    if errors.is_empty() {
        Ok(normalized)
    } else {
        Err(errors)
    }
}

pub fn validate_step_controls(operation: &str, arguments: &Map<String, Value>) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(condition) = arguments.get("when") {
        if let Err(error) = validate_when(condition) {
            errors.push(error);
        }
    }
    if let Some(specification) = arguments.get("extract") {
        if let Err(error) = validate_extraction(specification) {
            errors.push(error);
        }
    }
    if arguments.contains_key("steps") {
        errors.push("nested steps are allowed only in a when wrapper".into());
    }
    match operation {
        "repeat_until" => match arguments.get("repeat_until") {
            Some(config) => {
                if let Err(error) = validate_repeat_until(config) {
                    errors.push(error);
                }
            }
            None => errors.push("repeat_until requires a repeat_until object".into()),
        },
        "wait_for_outcome" => {
            if let Err(error) = validate_wait_for_outcome(arguments) {
                errors.push(error);
            }
        }
        "click_once_and_wait_for_outcome" => {
            if let Err(error) = validate_click_once_and_wait_for_outcome(arguments) {
                errors.push(error);
            }
        }
        _ => {
            if arguments.contains_key("repeat_until") {
                errors.push(
                    "repeat_until may be used only by the local repeat_until operation".into(),
                );
            }
            if arguments.contains_key("outcomes") {
                errors.push(
                    "outcomes may be used only by wait_for_outcome or click_once_and_wait_for_outcome".into(),
                );
            }
            if arguments.contains_key("accept_ambiguous_error_codes") {
                errors.push(
                    "accept_ambiguous_error_codes may be used only by click_once_and_wait_for_outcome".into(),
                );
            }
        }
    }
    errors
}

pub fn evaluate_when(condition: &Value, vars: &Map<String, Value>) -> Result<bool, String> {
    validate_when(condition)?;
    let object = condition
        .as_object()
        .ok_or_else(|| "when must be an object".to_owned())?;
    let variable = object
        .get("var")
        .and_then(Value::as_str)
        .ok_or_else(|| "when.var must be a non-empty string".to_owned())?;
    let actual = resolve_variable(variable, vars);
    if let Some(expected) = object.get("equals") {
        return Ok(actual == Some(resolve_references(expected, vars)));
    }
    if let Some(expected) = object.get("not_equals") {
        return Ok(actual != Some(resolve_references(expected, vars)));
    }
    if let Some(expected) = object.get("exists").and_then(Value::as_bool) {
        return Ok(actual.is_some() == expected);
    }
    let expected = object
        .get("in")
        .and_then(Value::as_array)
        .ok_or_else(|| "when.in must be a non-empty array".to_owned())?;
    Ok(actual.is_some_and(|value| {
        expected
            .iter()
            .map(|candidate| resolve_references(candidate, vars))
            .any(|candidate| candidate == value)
    }))
}

pub fn extract_values(source: &Value, specification: &Value) -> Result<Map<String, Value>, Value> {
    validate_extraction(specification).map_err(|message| {
        json!({
            "code":"VARIABLE_EXTRACTION_FAILED",
            "message":message,
            "details":{"source":"response.result","last_value":source}
        })
    })?;
    let object = specification
        .as_object()
        .expect("extraction validation guarantees object");
    let mut extracted = Map::new();
    for (name, path) in object {
        let path = path
            .as_str()
            .expect("extraction validation guarantees a JSONPath string");
        match json_path_select(source, path) {
            Ok(Some(value)) => {
                extracted.insert(name.clone(), value);
            }
            Ok(None) => {
                return Err(json!({
                    "code":"VARIABLE_EXTRACTION_FAILED",
                    "message":"configured extraction path is absent from the response",
                    "details":{"source":"response.result","path":path,"last_value":source}
                }));
            }
            Err(error) => {
                return Err(json!({
                    "code":"VARIABLE_EXTRACTION_FAILED",
                    "message":error,
                    "details":{"source":"response.result","path":path,"last_value":source}
                }));
            }
        }
    }
    Ok(extracted)
}

/// Execute a bounded polling construct around an allowlisted, read-only
/// observation operation. The output is canonicalized as repeat_until rather
/// than leaking an inner operation result through the public Script API.
pub fn execute_repeat_until<T: Transport>(
    client: &mut AiUiClient<T>,
    arguments: &Map<String, Value>,
) -> Value {
    let Some(config) = arguments.get("repeat_until") else {
        return local_failure(
            "INVALID_ARGUMENT",
            "repeat_until requires a repeat_until object",
            Value::Null,
        );
    };
    let mut plan = match parse_repeat_plan(config) {
        Ok(plan) => plan,
        Err(error) => {
            return local_failure("INVALID_ARGUMENT", &error, json!({"repeat_until":config}));
        }
    };
    if let Some(session_id) = arguments.get("session_id") {
        plan.arguments
            .entry("session_id")
            .or_insert_with(|| session_id.clone());
    }
    let started = Instant::now();
    let mut last_response = Value::Null;
    for iteration in 1..=plan.max_iterations {
        let response = client.call(&plan.operation, Value::Object(plan.arguments.clone()));
        let success = response.get("success").and_then(Value::as_bool) == Some(true);
        if success {
            let source = response.get("result").unwrap_or(&response);
            match json_path_select(source, &plan.path) {
                Ok(Some(value))
                    if let Some(matched_token) = matched_token(&value, &plan.matcher) =>
                {
                    return json!({
                        "operation":"repeat_until",
                        "success":true,
                        "result":{
                            "iterations":iteration,
                            "elapsed_ms":started.elapsed().as_millis(),
                            "observation_operation":plan.operation,
                            "path":plan.path,
                            "last_value":value,
                            "matched_token":matched_token,
                            "last_response":response
                        }
                    });
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(error) => {
                    return local_failure(
                        "REPEAT_UNTIL_PATH_MISSING",
                        &error,
                        json!({"path":plan.path,"last_response":response}),
                    );
                }
            }
        } else {
            let error_code = response
                .pointer("/error/code")
                .and_then(Value::as_str)
                .unwrap_or("OBSERVATION_FAILED");
            if !matches!(error_code, "TASK_NOT_FOUND" | "EXTERNAL_REQUEST_NOT_FOUND") {
                return local_failure(
                    "REPEAT_UNTIL_OBSERVATION_FAILED",
                    "inner observation failed before its condition was reached",
                    json!({"inner_error":response.get("error"),"last_response":response}),
                );
            }
        }
        last_response = response;
        if iteration == plan.max_iterations
            || started.elapsed() >= Duration::from_millis(plan.timeout_ms)
        {
            break;
        }
        thread::sleep(Duration::from_millis(plan.interval_ms));
    }
    local_failure(
        "REPEAT_UNTIL_TIMEOUT",
        "bounded repeat_until condition was not reached",
        json!({
            "timeout_ms":plan.timeout_ms,
            "max_iterations":plan.max_iterations,
            "elapsed_ms":started.elapsed().as_millis(),
            "observation_operation":plan.operation,
            "path":plan.path,
            "last_response":last_response
        }),
    )
}

/// Wait for one of the declared semantic outcomes without sending a new host
/// operation. Failure outcomes are checked before success outcomes so a
/// published application failure is never hidden by a stale confirmation.
pub fn execute_wait_for_outcome<T: Transport>(
    client: &mut AiUiClient<T>,
    arguments: &Map<String, Value>,
) -> Value {
    let plan = match parse_wait_for_outcome(arguments) {
        Ok(plan) => plan,
        Err(error) => {
            return outcome_failure(
                "INVALID_ARGUMENT",
                &error,
                json!({"wait_for_outcome":arguments}),
            );
        }
    };
    let session_id = arguments.get("session_id").cloned();
    let started = Instant::now();
    let mut last_responses = Map::new();
    for iteration in 1..=plan.max_iterations {
        for kind in [OutcomeKind::Failure, OutcomeKind::Success] {
            for probe in plan.outcomes.iter().filter(|probe| probe.kind == kind) {
                let mut read_arguments =
                    Map::from_iter([("id".into(), Value::String(probe.id.clone()))]);
                if let Some(session_id) = &session_id {
                    read_arguments.insert("session_id".into(), session_id.clone());
                }
                let response = client.call("read", Value::Object(read_arguments));
                last_responses.insert(probe.id.clone(), response.clone());
                if response.get("success").and_then(Value::as_bool) == Some(true) {
                    let source = response.get("result").unwrap_or(&response);
                    match json_path_select(source, &probe.path) {
                        Ok(Some(value)) => {
                            let actual_visible = source.get("visible").and_then(Value::as_bool);
                            if probe
                                .visible
                                .is_some_and(|expected| actual_visible != Some(expected))
                            {
                                continue;
                            }
                            if let Some(matched_token) = matched_token(&value, &probe.matcher) {
                                let details = json!({
                                    "outcome":probe.kind.as_str(),
                                    "iterations":iteration,
                                    "elapsed_ms":started.elapsed().as_millis(),
                                    "id":probe.id,
                                    "path":probe.path,
                                    "last_value":value,
                                    "matched_token":matched_token,
                                    "visible":actual_visible,
                                    "last_response":response
                                });
                                if probe.kind == OutcomeKind::Failure {
                                    return outcome_failure(
                                        "WAIT_FOR_OUTCOME_FAILURE",
                                        "application published a configured failure outcome",
                                        details,
                                    );
                                }
                                return json!({
                                    "operation":"wait_for_outcome",
                                    "success":true,
                                    "result":details
                                });
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return outcome_failure(
                                "WAIT_FOR_OUTCOME_PATH_MISSING",
                                &error,
                                json!({
                                    "id":probe.id,
                                    "path":probe.path,
                                    "last_response":response
                                }),
                            );
                        }
                    }
                } else {
                    let error_code = response
                        .pointer("/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("OBSERVATION_FAILED");
                    if error_code != "ELEMENT_NOT_FOUND" {
                        return outcome_failure(
                            "WAIT_FOR_OUTCOME_OBSERVATION_FAILED",
                            "semantic observation failed before an outcome was reached",
                            json!({
                                "id":probe.id,
                                "inner_error":response.get("error"),
                                "last_response":response
                            }),
                        );
                    }
                }
            }
        }
        if iteration == plan.max_iterations
            || started.elapsed() >= Duration::from_millis(plan.timeout_ms)
        {
            break;
        }
        thread::sleep(Duration::from_millis(plan.interval_ms));
    }
    outcome_failure(
        "WAIT_FOR_OUTCOME_TIMEOUT",
        "bounded wait_for_outcome did not reach a configured outcome",
        json!({
            "timeout_ms":plan.timeout_ms,
            "max_iterations":plan.max_iterations,
            "elapsed_ms":started.elapsed().as_millis(),
            "last_responses":last_responses
        }),
    )
}

/// Dispatch exactly one semantic click. An explicitly marked ambiguous Invoke
/// error is never retried; only the declared semantic outcomes can establish
/// success afterwards.
pub fn execute_click_once_and_wait_for_outcome<T: Transport>(
    client: &mut AiUiClient<T>,
    arguments: &Map<String, Value>,
) -> Value {
    let accepted_codes = match parse_click_once_and_wait_for_outcome(arguments) {
        Ok(codes) => codes,
        Err(error) => {
            return click_once_failure(
                "INVALID_ARGUMENT",
                &error,
                json!({"click_once_and_wait_for_outcome":arguments}),
            );
        }
    };
    let dispatch = client.call("click", Value::Object(click_once_arguments(arguments)));
    let dispatch_success = dispatch.get("success").and_then(Value::as_bool) == Some(true);
    let dispatch_indeterminate =
        !dispatch_success && is_accepted_indeterminate_dispatch(&dispatch, &accepted_codes);
    if !dispatch_success && !dispatch_indeterminate {
        return click_once_failure(
            "CLICK_ONCE_DISPATCH_FAILED",
            "semantic click was not accepted and is not an explicitly marked indeterminate Invoke",
            json!({"dispatch":dispatch}),
        );
    }
    let wait = execute_wait_for_outcome(client, &wait_for_outcome_arguments(arguments));
    let dispatch_status = if dispatch_indeterminate {
        "indeterminate_observed"
    } else {
        "accepted"
    };
    if wait.get("success").and_then(Value::as_bool) == Some(true) {
        let outcome = wait.get("result").cloned().unwrap_or(Value::Null);
        let matched_token = outcome.get("matched_token").cloned().unwrap_or(Value::Null);
        return json!({
            "operation":"click_once_and_wait_for_outcome",
            "success":true,
            "result":{
                "dispatch_status":dispatch_status,
                "dispatch":dispatch,
                "matched_token":matched_token,
                "outcome":outcome
            }
        });
    }
    let error = wait.get("error").cloned().unwrap_or_else(|| {
        json!({
            "code":"CLICK_ONCE_OBSERVATION_FAILED",
            "message":"postcondition observation did not return an error payload",
            "details":{"wait":wait}
        })
    });
    json!({
        "operation":"click_once_and_wait_for_outcome",
        "success":false,
        "error":error,
        "dispatch_status":dispatch_status,
        "dispatch":dispatch
    })
}

fn click_once_failure(code: &str, message: &str, details: Value) -> Value {
    json!({
        "operation":"click_once_and_wait_for_outcome",
        "success":false,
        "error":{"code":code,"message":message,"details":details}
    })
}

fn is_accepted_indeterminate_dispatch(response: &Value, accepted_codes: &[String]) -> bool {
    let Some(code) = response.pointer("/error/code").and_then(Value::as_str) else {
        return false;
    };
    accepted_codes.iter().any(|candidate| candidate == code)
        && response.pointer("/error/details/activation_attempted") == Some(&Value::Bool(true))
        && response.pointer("/error/details/attempted_pattern")
            == Some(&Value::String("InvokePattern".into()))
        && response.pointer("/error/details/delivery") == Some(&Value::String("unknown".into()))
        && response.pointer("/error/details/fallback_suppressed") == Some(&Value::Bool(true))
}

fn click_once_arguments(arguments: &Map<String, Value>) -> Map<String, Value> {
    selected_arguments(
        arguments,
        &[
            "id",
            "session_id",
            "input_lock_token",
            "_aiui_redact_fields",
            "_aiui_redact_response",
        ],
    )
}

fn wait_for_outcome_arguments(arguments: &Map<String, Value>) -> Map<String, Value> {
    selected_arguments(
        arguments,
        &[
            "outcomes",
            "timeout_ms",
            "max_iterations",
            "interval_ms",
            "session_id",
            "_aiui_redact_fields",
            "_aiui_redact_response",
        ],
    )
}

fn selected_arguments(arguments: &Map<String, Value>, names: &[&str]) -> Map<String, Value> {
    names
        .iter()
        .filter_map(|name| {
            arguments
                .get(*name)
                .map(|value| ((*name).into(), value.clone()))
        })
        .collect()
}

fn outcome_failure(code: &str, message: &str, details: Value) -> Value {
    json!({
        "operation":"wait_for_outcome",
        "success":false,
        "error":{"code":code,"message":message,"details":details}
    })
}

fn local_failure(code: &str, message: &str, details: Value) -> Value {
    json!({
        "operation":"repeat_until",
        "success":false,
        "error":{"code":code,"message":message,"details":details}
    })
}

fn parse_repeat_plan(config: &Value) -> Result<RepeatPlan, String> {
    validate_repeat_until(config)?;
    let object = config
        .as_object()
        .ok_or_else(|| "repeat_until must be an object".to_owned())?;
    let timeout_ms = object
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| "repeat_until.timeout_ms is required".to_owned())?;
    let max_iterations = object
        .get("max_iterations")
        .and_then(Value::as_u64)
        .ok_or_else(|| "repeat_until.max_iterations is required".to_owned())?;
    let interval_ms = object
        .get("interval_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_REPEAT_INTERVAL_MS);
    if let Some(observation) = object.get("observation") {
        let observation = observation
            .as_object()
            .ok_or_else(|| "repeat_until.observation must be an object".to_owned())?;
        let states = observation
            .get("state_in")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "repeat_until.observation.state_in must be a non-empty array".to_owned()
            })?
            .clone();
        if let Some(external) = observation.get("external").and_then(Value::as_str) {
            return Ok(RepeatPlan {
                operation: "get_api_request_state".into(),
                arguments: Map::from_iter([(
                    "observer_alias".into(),
                    Value::String(external.into()),
                )]),
                path: "$.last_observation.request.state".into(),
                matcher: ValueMatcher::In(states),
                timeout_ms,
                max_iterations,
                interval_ms,
            });
        }
        if let Some(task) = observation.get("task").and_then(Value::as_str) {
            return Ok(RepeatPlan {
                operation: "get_task_state".into(),
                arguments: Map::from_iter([("operation_id".into(), Value::String(task.into()))]),
                path: "$.last_observation.task.state".into(),
                matcher: ValueMatcher::In(states),
                timeout_ms,
                max_iterations,
                interval_ms,
            });
        }
        return Err("repeat_until.observation requires external or task".into());
    }
    let operation = object
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| "repeat_until.operation is required".to_owned())?;
    let arguments = object
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "repeat_until.path is required".to_owned())?
        .to_owned();
    Ok(RepeatPlan {
        operation: operation.into(),
        arguments,
        path,
        matcher: matcher_from(object)?,
        timeout_ms,
        max_iterations,
        interval_ms,
    })
}

fn parse_wait_for_outcome(arguments: &Map<String, Value>) -> Result<OutcomePlan, String> {
    validate_wait_for_outcome(arguments)?;
    let outcomes = arguments
        .get("outcomes")
        .and_then(Value::as_array)
        .expect("wait_for_outcome validation guarantees outcomes");
    let outcomes = outcomes
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .expect("wait_for_outcome validation guarantees outcome object");
            let kind = match object
                .get("kind")
                .and_then(Value::as_str)
                .expect("wait_for_outcome validation guarantees kind")
            {
                "success" => OutcomeKind::Success,
                "failure" => OutcomeKind::Failure,
                _ => unreachable!("wait_for_outcome validation guarantees kind"),
            };
            Ok(OutcomeProbe {
                kind,
                id: object
                    .get("id")
                    .and_then(Value::as_str)
                    .expect("wait_for_outcome validation guarantees id")
                    .to_owned(),
                path: object
                    .get("path")
                    .and_then(Value::as_str)
                    .expect("wait_for_outcome validation guarantees path")
                    .to_owned(),
                matcher: matcher_from(object)?,
                visible: object.get("visible").and_then(Value::as_bool),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(OutcomePlan {
        outcomes,
        timeout_ms: arguments
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .expect("wait_for_outcome validation guarantees timeout_ms"),
        max_iterations: arguments
            .get("max_iterations")
            .and_then(Value::as_u64)
            .expect("wait_for_outcome validation guarantees max_iterations"),
        interval_ms: arguments
            .get("interval_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_REPEAT_INTERVAL_MS),
    })
}

fn parse_click_once_and_wait_for_outcome(
    arguments: &Map<String, Value>,
) -> Result<Vec<String>, String> {
    validate_click_once_and_wait_for_outcome(arguments)?;
    Ok(arguments
        .get("accept_ambiguous_error_codes")
        .and_then(Value::as_array)
        .expect("click_once_and_wait validation guarantees codes")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}

fn validate_click_once_and_wait_for_outcome(arguments: &Map<String, Value>) -> Result<(), String> {
    for key in arguments.keys() {
        if !matches!(
            key.as_str(),
            "id" | "outcomes"
                | "timeout_ms"
                | "max_iterations"
                | "interval_ms"
                | "accept_ambiguous_error_codes"
                | "when"
                | "extract"
                | "session_id"
                | "input_lock_token"
                | "_aiui_redact_fields"
                | "_aiui_redact_response"
        ) {
            return Err(format!(
                "click_once_and_wait_for_outcome has unsupported field {key}"
            ));
        }
    }
    if !arguments
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
    {
        return Err("click_once_and_wait_for_outcome.id must be a non-empty AutomationId".into());
    }
    validate_wait_for_outcome(&wait_for_outcome_arguments(arguments))?;
    let codes = arguments
        .get("accept_ambiguous_error_codes")
        .and_then(Value::as_array)
        .filter(|codes| !codes.is_empty() && codes.len() <= 4)
        .ok_or_else(|| {
            "click_once_and_wait_for_outcome.accept_ambiguous_error_codes must contain 1..=4 error codes"
                .to_owned()
        })?;
    let mut unique = Vec::new();
    for (index, value) in codes.iter().enumerate() {
        let code = value.as_str().ok_or_else(|| {
            format!(
                "click_once_and_wait_for_outcome.accept_ambiguous_error_codes[{index}] must be a string"
            )
        })?;
        if code.is_empty()
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        {
            return Err(format!(
                "click_once_and_wait_for_outcome.accept_ambiguous_error_codes[{index}] must be SCREAMING_SNAKE_CASE"
            ));
        }
        if unique.iter().any(|existing: &String| existing == code) {
            return Err(format!(
                "click_once_and_wait_for_outcome.accept_ambiguous_error_codes contains duplicate {code}"
            ));
        }
        unique.push(code.to_owned());
    }
    Ok(())
}

fn validate_wait_for_outcome(arguments: &Map<String, Value>) -> Result<(), String> {
    for key in arguments.keys() {
        if !matches!(
            key.as_str(),
            "outcomes"
                | "timeout_ms"
                | "max_iterations"
                | "interval_ms"
                | "when"
                | "extract"
                | "session_id"
                | "_aiui_redact_fields"
                | "_aiui_redact_response"
        ) {
            return Err(format!("wait_for_outcome has unsupported field {key}"));
        }
    }
    let timeout_ms = arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= MAX_OUTCOME_TIMEOUT_MS)
        .ok_or_else(|| {
            format!(
                "wait_for_outcome.timeout_ms is required and must be within 1..={MAX_OUTCOME_TIMEOUT_MS}"
            )
        })?;
    let max_iterations = arguments
        .get("max_iterations")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= MAX_OUTCOME_ITERATIONS)
        .ok_or_else(|| {
            format!(
                "wait_for_outcome.max_iterations is required and must be within 1..={MAX_OUTCOME_ITERATIONS}"
            )
        })?;
    let _ = (timeout_ms, max_iterations);
    if let Some(interval_ms) = arguments.get("interval_ms").and_then(Value::as_u64) {
        if !(MIN_REPEAT_INTERVAL_MS..=MAX_REPEAT_INTERVAL_MS).contains(&interval_ms) {
            return Err(format!(
                "wait_for_outcome.interval_ms must be within {MIN_REPEAT_INTERVAL_MS}..={MAX_REPEAT_INTERVAL_MS}"
            ));
        }
    } else if arguments.contains_key("interval_ms") {
        return Err("wait_for_outcome.interval_ms must be an integer".into());
    }
    let outcomes = arguments
        .get("outcomes")
        .and_then(Value::as_array)
        .filter(|outcomes| !outcomes.is_empty() && outcomes.len() <= 8)
        .ok_or_else(|| "wait_for_outcome.outcomes must contain 1..=8 outcome objects".to_owned())?;
    for (index, outcome) in outcomes.iter().enumerate() {
        let object = outcome
            .as_object()
            .ok_or_else(|| format!("wait_for_outcome.outcomes[{index}] must be an object"))?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "kind"
                    | "id"
                    | "path"
                    | "equals"
                    | "not_equals"
                    | "in"
                    | "contains_any"
                    | "visible"
            ) {
                return Err(format!(
                    "wait_for_outcome.outcomes[{index}] has unsupported field {key}"
                ));
            }
        }
        if !matches!(
            object.get("kind").and_then(Value::as_str),
            Some("success" | "failure")
        ) {
            return Err(format!(
                "wait_for_outcome.outcomes[{index}].kind must be success or failure"
            ));
        }
        if !object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        {
            return Err(format!(
                "wait_for_outcome.outcomes[{index}].id must be a non-empty AutomationId"
            ));
        }
        let path = object.get("path").and_then(Value::as_str).ok_or_else(|| {
            format!("wait_for_outcome.outcomes[{index}].path must be a JSONPath string")
        })?;
        parse_json_path(path)
            .map_err(|error| format!("wait_for_outcome.outcomes[{index}].path: {error}"))?;
        if object
            .get("visible")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(format!(
                "wait_for_outcome.outcomes[{index}].visible must be boolean"
            ));
        }
        matcher_from(object)
            .map_err(|error| format!("wait_for_outcome.outcomes[{index}]: {error}"))?;
    }
    Ok(())
}

fn validate_when(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "when must be an object".to_owned())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "var" | "equals" | "not_equals" | "exists" | "in"
        ) {
            return Err(format!("when has unsupported field {key}"));
        }
    }
    if !object
        .get("var")
        .and_then(Value::as_str)
        .is_some_and(|variable| !variable.trim().is_empty())
    {
        return Err("when.var must be a non-empty string".into());
    }
    let comparisons = usize::from(object.contains_key("equals"))
        + usize::from(object.contains_key("not_equals"))
        + usize::from(object.contains_key("exists"))
        + usize::from(object.contains_key("in"));
    if comparisons != 1 {
        return Err("when requires exactly one of equals, not_equals, exists, or in".into());
    }
    if object
        .get("exists")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("when.exists must be boolean".into());
    }
    if !object
        .get("in")
        .is_none_or(|value| value.as_array().is_some_and(|items| !items.is_empty()))
    {
        return Err("when.in must be a non-empty array".into());
    }
    Ok(())
}

fn validate_extraction(value: &Value) -> Result<(), String> {
    let object = value.as_object().ok_or_else(|| {
        "extract must be an object of variable names to JSONPath strings".to_owned()
    })?;
    if object.is_empty() {
        return Err("extract must not be empty".into());
    }
    for (name, path) in object {
        if !valid_variable_name(name) {
            return Err(format!("extract variable name is not safe: {name}"));
        }
        let path = path
            .as_str()
            .ok_or_else(|| format!("extract.{name} must be a JSONPath string"))?;
        parse_json_path(path).map_err(|error| format!("extract.{name}: {error}"))?;
    }
    Ok(())
}

fn validate_repeat_until(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "repeat_until must be an object".to_owned())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "observation"
                | "operation"
                | "arguments"
                | "path"
                | "equals"
                | "not_equals"
                | "in"
                | "contains_any"
                | "timeout_ms"
                | "max_iterations"
                | "interval_ms"
        ) {
            return Err(format!("repeat_until has unsupported field {key}"));
        }
    }
    let timeout_ms = object
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= MAX_REPEAT_TIMEOUT_MS)
        .ok_or_else(|| {
            format!(
                "repeat_until.timeout_ms is required and must be within 1..={MAX_REPEAT_TIMEOUT_MS}"
            )
        })?;
    let max_iterations = object
        .get("max_iterations")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0 && *value <= MAX_REPEAT_ITERATIONS)
        .ok_or_else(|| {
            format!(
                "repeat_until.max_iterations is required and must be within 1..={MAX_REPEAT_ITERATIONS}"
            )
        })?;
    let _ = (timeout_ms, max_iterations);
    if let Some(interval_ms) = object.get("interval_ms").and_then(Value::as_u64) {
        if !(MIN_REPEAT_INTERVAL_MS..=MAX_REPEAT_INTERVAL_MS).contains(&interval_ms) {
            return Err(format!(
                "repeat_until.interval_ms must be within {MIN_REPEAT_INTERVAL_MS}..={MAX_REPEAT_INTERVAL_MS}"
            ));
        }
    } else if object.contains_key("interval_ms") {
        return Err("repeat_until.interval_ms must be an integer".into());
    }
    let has_observation = object.contains_key("observation");
    let has_operation = object.contains_key("operation");
    if has_observation == has_operation {
        return Err("repeat_until requires exactly one of observation or operation".into());
    }
    if has_observation {
        let observation = object
            .get("observation")
            .and_then(Value::as_object)
            .ok_or_else(|| "repeat_until.observation must be an object".to_owned())?;
        for key in observation.keys() {
            if !matches!(key.as_str(), "external" | "task" | "state_in") {
                return Err(format!(
                    "repeat_until.observation has unsupported field {key}"
                ));
            }
        }
        let targets = usize::from(observation.contains_key("external"))
            + usize::from(observation.contains_key("task"));
        if targets != 1 {
            return Err("repeat_until.observation requires exactly one of external or task".into());
        }
        for key in ["external", "task"] {
            if observation.contains_key(key)
                && !observation
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(format!(
                    "repeat_until.observation.{key} must be a non-empty string"
                ));
            }
        }
        let states = observation
            .get("state_in")
            .and_then(Value::as_array)
            .filter(|states| !states.is_empty())
            .ok_or_else(|| {
                "repeat_until.observation.state_in must be a non-empty array".to_owned()
            })?;
        if states.iter().any(|state| !state.is_string()) {
            return Err("repeat_until.observation.state_in must contain strings".into());
        }
        return Ok(());
    }
    let operation = object
        .get("operation")
        .and_then(Value::as_str)
        .filter(|operation| !operation.is_empty())
        .ok_or_else(|| "repeat_until.operation must be a non-empty string".to_owned())?;
    if !READ_ONLY_OBSERVATION_OPERATIONS.contains(&operation)
        || !catalog::operation(operation)
            .is_some_and(|spec| !spec.physical_input && !spec.mutation && !spec.local_only)
    {
        return Err(
            "repeat_until.operation must be an allowlisted read-only observation operation".into(),
        );
    }
    if object
        .get("arguments")
        .is_some_and(|value| !value.is_object())
    {
        return Err("repeat_until.arguments must be an object".into());
    }
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "repeat_until.path must be a JSONPath string".to_owned())?;
    parse_json_path(path).map_err(|error| format!("repeat_until.path: {error}"))?;
    matcher_from(object)?;
    Ok(())
}

fn matcher_from(object: &Map<String, Value>) -> Result<ValueMatcher, String> {
    let comparisons = usize::from(object.contains_key("equals"))
        + usize::from(object.contains_key("not_equals"))
        + usize::from(object.contains_key("in"))
        + usize::from(object.contains_key("contains_any"));
    if comparisons != 1 {
        return Err(
            "repeat_until requires exactly one of equals, not_equals, in, or contains_any".into(),
        );
    }
    if let Some(value) = object.get("equals") {
        return Ok(ValueMatcher::Equals(value.clone()));
    }
    if let Some(value) = object.get("not_equals") {
        return Ok(ValueMatcher::NotEquals(value.clone()));
    }
    if let Some(value) = object.get("contains_any") {
        let values = value
            .as_array()
            .filter(|values| !values.is_empty())
            .ok_or_else(|| "repeat_until.contains_any must be a non-empty array".to_owned())?;
        let tokens = values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        "repeat_until.contains_any must contain non-empty strings".to_owned()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ValueMatcher::ContainsAny(tokens));
    }
    let values = object
        .get("in")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| "repeat_until.in must be a non-empty array".to_owned())?;
    Ok(ValueMatcher::In(values.clone()))
}

fn matched_token(value: &Value, matcher: &ValueMatcher) -> Option<Value> {
    match matcher {
        ValueMatcher::Equals(expected) if value == expected => Some(value.clone()),
        ValueMatcher::NotEquals(expected) if value != expected => Some(value.clone()),
        ValueMatcher::In(expected) => expected
            .iter()
            .find(|candidate| *candidate == value)
            .cloned(),
        ValueMatcher::ContainsAny(tokens) => value.as_str().and_then(|value| {
            tokens
                .iter()
                .find(|token| value.contains(token.as_str()))
                .cloned()
                .map(Value::String)
        }),
        _ => None,
    }
}

fn resolve_variable(reference: &str, vars: &Map<String, Value>) -> Option<Value> {
    let reference = reference
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(reference);
    if let Some(value) = vars.get(reference) {
        return Some(value.clone());
    }
    let (root, path) = reference.split_once('.')?;
    let mut current = vars.get(root)?;
    for segment in path.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current.clone())
}

fn resolve_references(value: &Value, vars: &Map<String, Value>) -> Value {
    match value {
        Value::String(text) if text.starts_with("${") && text.ends_with('}') => {
            resolve_variable(text, vars).unwrap_or_else(|| value.clone())
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| resolve_references(value, vars))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), resolve_references(value, vars)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn valid_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn json_path_select(source: &Value, path: &str) -> Result<Option<Value>, String> {
    let segments = parse_json_path(path)?;
    let mut current = source;
    for segment in segments {
        current = match segment {
            JsonPathSegment::Field(name) => {
                let Some(object) = current.as_object() else {
                    return Ok(None);
                };
                let Some(value) = object.get(&name) else {
                    return Ok(None);
                };
                value
            }
            JsonPathSegment::Index(index) => {
                let Some(array) = current.as_array() else {
                    return Ok(None);
                };
                let Some(value) = array.get(index) else {
                    return Ok(None);
                };
                value
            }
        };
    }
    Ok(Some(current.clone()))
}

pub(crate) fn parse_json_path(path: &str) -> Result<Vec<JsonPathSegment>, String> {
    let bytes = path.as_bytes();
    if bytes.first() != Some(&b'$') {
        return Err("JSONPath must start with $".into());
    }
    let mut position = 1;
    let mut segments = Vec::new();
    while position < bytes.len() {
        match bytes[position] {
            b'.' => {
                position += 1;
                let start = position;
                while position < bytes.len()
                    && (bytes[position].is_ascii_alphanumeric()
                        || matches!(bytes[position], b'_' | b'-'))
                {
                    position += 1;
                }
                if start == position {
                    return Err("JSONPath field segment is empty or unsafe".into());
                }
                segments.push(JsonPathSegment::Field(path[start..position].into()));
            }
            b'[' => {
                position += 1;
                let start = position;
                while position < bytes.len() && bytes[position].is_ascii_digit() {
                    position += 1;
                }
                if start == position || bytes.get(position) != Some(&b']') {
                    return Err("JSONPath array index must be a non-negative integer".into());
                }
                let index = path[start..position]
                    .parse::<usize>()
                    .map_err(|_| "JSONPath array index is too large".to_owned())?;
                position += 1;
                segments.push(JsonPathSegment::Index(index));
            }
            _ => return Err("JSONPath supports only .field and [index] segments".into()),
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AiUiClient, InProcessTransport};
    use serde_json::json;

    #[test]
    fn when_wrapper_normalizes_to_guarded_steps() {
        let script = json!({
            "steps":[
                {"when":{"var":"kind","equals":"pdf"},"steps":[{"op":"inspect_ui"}]}
            ]
        });
        let normalized = normalize_script_controls(script).unwrap();
        assert_eq!(normalized["steps"][0]["op"], "inspect_ui");
        assert_eq!(normalized["steps"][0]["when"]["var"], "kind");
    }

    #[test]
    fn extraction_supports_field_and_array_paths() {
        let extracted = extract_values(
            &json!({"job":{"items":[{"id":"first"}]}}),
            &json!({"job_id":"$.job.items[0].id"}),
        )
        .unwrap();
        assert_eq!(extracted["job_id"], "first");
    }

    #[test]
    fn missing_extraction_path_is_typed() {
        let error = extract_values(&json!({"job":{}}), &json!({"job_id":"$.job.id"})).unwrap_err();
        assert_eq!(error["code"], "VARIABLE_EXTRACTION_FAILED");
    }

    #[test]
    fn repeat_until_refuses_missing_bounds_and_mutations() {
        let missing_bounds =
            json!({"operation":"get_task_state","path":"$.state","equals":"Completed"});
        assert!(
            !validate_step_controls(
                "repeat_until",
                &Map::from_iter([("repeat_until".into(), missing_bounds)])
            )
            .is_empty()
        );
        let mutation = json!({
            "operation":"click",
            "arguments":{"id":"button"},
            "path":"$.state",
            "equals":"Completed",
            "timeout_ms":100,
            "max_iterations":2
        });
        assert!(
            !validate_step_controls(
                "repeat_until",
                &Map::from_iter([("repeat_until".into(), mutation)])
            )
            .is_empty()
        );
    }

    #[test]
    fn repeat_until_allows_public_read_observation() {
        let read = json!({
            "operation":"read",
            "arguments":{"id":"workflow.status"},
            "path":"$.value",
            "equals":"ready",
            "timeout_ms":100,
            "max_iterations":2
        });
        assert!(
            validate_step_controls(
                "repeat_until",
                &Map::from_iter([("repeat_until".into(), read)])
            )
            .is_empty()
        );
    }

    #[test]
    fn repeat_until_contains_any_is_validated_and_exclusive() {
        let contains_any = json!({
            "operation":"read",
            "arguments":{"id":"dialog.confirmation.message"},
            "path":"$.value",
            "contains_any":["Распознать таблицы выбранных страниц", "ИИ · общая спецификация"],
            "timeout_ms":100,
            "max_iterations":2
        });
        assert!(
            validate_step_controls(
                "repeat_until",
                &Map::from_iter([("repeat_until".into(), contains_any.clone())])
            )
            .is_empty()
        );

        for invalid in [
            json!({
                "operation":"read",
                "arguments":{"id":"dialog.confirmation.message"},
                "path":"$.value",
                "contains_any":[],
                "timeout_ms":100,
                "max_iterations":2
            }),
            json!({
                "operation":"read",
                "arguments":{"id":"dialog.confirmation.message"},
                "path":"$.value",
                "contains_any":["ready", 1],
                "timeout_ms":100,
                "max_iterations":2
            }),
            json!({
                "operation":"read",
                "arguments":{"id":"dialog.confirmation.message"},
                "path":"$.value",
                "contains_any":["ready"],
                "in":["ready"],
                "timeout_ms":100,
                "max_iterations":2
            }),
        ] {
            assert!(
                !validate_step_controls(
                    "repeat_until",
                    &Map::from_iter([("repeat_until".into(), invalid)])
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn repeat_until_contains_any_returns_matching_token() {
        let transport = InProcessTransport::new(|operation: &str, _arguments: Value| {
            json!({
                "operation":operation,
                "success":true,
                "result":{"value":"«ИИ · общая спецификация» передаст подготовленные данные"}
            })
        });
        let mut client = AiUiClient::new(transport);
        let result = execute_repeat_until(
            &mut client,
            &Map::from_iter([(
                "repeat_until".into(),
                json!({
                    "operation":"read",
                    "arguments":{"id":"dialog.confirmation.message"},
                    "path":"$.value",
                    "contains_any":["Распознать таблицы выбранных страниц", "ИИ · общая спецификация"],
                    "timeout_ms":100,
                    "max_iterations":2
                }),
            )]),
        );
        assert_eq!(result["success"], true);
        assert_eq!(
            result["result"]["last_value"],
            "«ИИ · общая спецификация» передаст подготовленные данные"
        );
        assert_eq!(result["result"]["matched_token"], "ИИ · общая спецификация");
    }

    #[test]
    fn wait_for_outcome_requires_bounded_semantic_outcomes() {
        let valid = Map::from_iter([
            (
                "outcomes".into(),
                json!([
                    {
                        "kind":"success",
                        "id":"dialog.confirmation.message",
                        "path":"$.value",
                        "contains_any":["Собрать общую спецификацию"],
                        "visible":true
                    },
                    {
                        "kind":"failure",
                        "id":"workflow.status",
                        "path":"$.value",
                        "contains_any":["остановлена"]
                    }
                ]),
            ),
            ("timeout_ms".into(), json!(1_000)),
            ("max_iterations".into(), json!(2)),
            ("interval_ms".into(), json!(10)),
        ]);
        assert!(validate_step_controls("wait_for_outcome", &valid).is_empty());

        for invalid in [
            Map::from_iter([
                ("outcomes".into(), json!([])),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
            Map::from_iter([
                (
                    "outcomes".into(),
                    json!([{
                        "kind":"unknown",
                        "id":"workflow.status",
                        "path":"$.value",
                        "contains_any":["failed"]
                    }]),
                ),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
            Map::from_iter([
                (
                    "outcomes".into(),
                    json!([{
                        "kind":"success",
                        "id":"workflow.status",
                        "path":"$.value",
                        "contains_any":["ready"],
                        "equals":"ready"
                    }]),
                ),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
            Map::from_iter([
                (
                    "outcomes".into(),
                    json!([{
                        "kind":"success",
                        "id":"workflow.status",
                        "path":"$.value",
                        "contains_any":["ready"],
                        "visible":"yes"
                    }]),
                ),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
        ] {
            assert!(!validate_step_controls("wait_for_outcome", &invalid).is_empty());
        }
    }

    #[test]
    fn wait_for_outcome_returns_success_and_failure_is_prioritized() {
        let success_transport = InProcessTransport::new(|operation: &str, arguments: Value| {
            assert_eq!(operation, "read");
            assert_eq!(arguments["id"], "dialog.confirmation.message");
            json!({
                "operation":operation,
                "success":true,
                "result":{"value":"Собрать общую спецификацию","visible":true}
            })
        });
        let mut success_client = AiUiClient::new(success_transport);
        let success = execute_wait_for_outcome(
            &mut success_client,
            &Map::from_iter([
                (
                    "outcomes".into(),
                    json!([{
                        "kind":"success",
                        "id":"dialog.confirmation.message",
                        "path":"$.value",
                        "contains_any":["Собрать общую спецификацию"],
                        "visible":true
                    }]),
                ),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
        );
        assert_eq!(success["success"], true);
        assert_eq!(success["result"]["outcome"], "success");

        let failure_transport = InProcessTransport::new(|operation: &str, arguments: Value| {
            assert_eq!(operation, "read");
            let value = match arguments["id"].as_str() {
                Some("workflow.status") => "Сборка спецификации остановлена",
                Some("dialog.confirmation.message") => "Собрать общую спецификацию",
                other => panic!("unexpected semantic id: {other:?}"),
            };
            json!({"operation":operation,"success":true,"result":{"value":value,"visible":true}})
        });
        let mut failure_client = AiUiClient::new(failure_transport);
        let failure = execute_wait_for_outcome(
            &mut failure_client,
            &Map::from_iter([
                (
                    "outcomes".into(),
                    json!([
                        {
                            "kind":"success",
                            "id":"dialog.confirmation.message",
                            "path":"$.value",
                            "contains_any":["Собрать общую спецификацию"],
                            "visible":true
                        },
                        {
                            "kind":"failure",
                            "id":"workflow.status",
                            "path":"$.value",
                            "contains_any":["остановлена"],
                            "visible":true
                        }
                    ]),
                ),
                ("timeout_ms".into(), json!(100)),
                ("max_iterations".into(), json!(1)),
            ]),
        );
        assert_eq!(failure["success"], false);
        assert_eq!(failure["error"]["code"], "WAIT_FOR_OUTCOME_FAILURE");
        assert_eq!(failure["error"]["details"]["id"], "workflow.status");
    }

    #[test]
    fn wait_for_outcome_ignores_hidden_matching_confirmation() {
        let hidden_transport = InProcessTransport::new(|operation: &str, arguments: Value| {
            assert_eq!(operation, "read");
            assert_eq!(arguments["id"], "dialog.confirmation.message");
            json!({
                "operation":operation,
                "success":true,
                "result":{"value":"Собрать общую спецификацию","visible":false}
            })
        });
        let mut hidden_client = AiUiClient::new(hidden_transport);
        let arguments = Map::from_iter([
            (
                "outcomes".into(),
                json!([{
                    "kind":"success",
                    "id":"dialog.confirmation.message",
                    "path":"$.value",
                    "contains_any":["Собрать общую спецификацию"],
                    "visible":true
                }]),
            ),
            ("timeout_ms".into(), json!(100)),
            ("max_iterations".into(), json!(1)),
        ]);
        let hidden = execute_wait_for_outcome(&mut hidden_client, &arguments);
        assert_eq!(hidden["success"], false);
        assert_eq!(hidden["error"]["code"], "WAIT_FOR_OUTCOME_TIMEOUT");

        let visible_transport = InProcessTransport::new(|operation: &str, arguments: Value| {
            assert_eq!(operation, "read");
            assert_eq!(arguments["id"], "dialog.confirmation.message");
            json!({
                "operation":operation,
                "success":true,
                "result":{"value":"Собрать общую спецификацию","visible":true}
            })
        });
        let mut visible_client = AiUiClient::new(visible_transport);
        let visible = execute_wait_for_outcome(&mut visible_client, &arguments);
        assert_eq!(visible["success"], true);
        assert_eq!(visible["result"]["visible"], true);
    }

    #[test]
    fn wait_for_outcome_distinguishes_pending_elements_from_observation_errors() {
        let pending_transport = InProcessTransport::new(
            |operation: &str, _: Value| json!({"operation":operation,"success":false,"error":{"code":"ELEMENT_NOT_FOUND"}}),
        );
        let mut pending_client = AiUiClient::new(pending_transport);
        let arguments = Map::from_iter([
            (
                "outcomes".into(),
                json!([{
                    "kind":"success",
                    "id":"dialog.confirmation.message",
                    "path":"$.value",
                    "contains_any":["ready"]
                }]),
            ),
            ("timeout_ms".into(), json!(1)),
            ("max_iterations".into(), json!(1)),
        ]);
        let timeout = execute_wait_for_outcome(&mut pending_client, &arguments);
        assert_eq!(timeout["error"]["code"], "WAIT_FOR_OUTCOME_TIMEOUT");

        let failed_transport = InProcessTransport::new(
            |operation: &str, _: Value| json!({"operation":operation,"success":false,"error":{"code":"SESSION_NOT_FOUND"}}),
        );
        let mut failed_client = AiUiClient::new(failed_transport);
        let observation = execute_wait_for_outcome(&mut failed_client, &arguments);
        assert_eq!(
            observation["error"]["code"],
            "WAIT_FOR_OUTCOME_OBSERVATION_FAILED"
        );
    }
    fn click_once_wait_arguments() -> Map<String, Value> {
        Map::from_iter([
            ("id".into(), json!("dialog.confirmation.accept")),
            (
                "outcomes".into(),
                json!([{
                    "kind":"success",
                    "id":"dialog.confirmation.message",
                    "path":"$.value",
                    "contains_any":["Собрать общую спецификацию"],
                    "visible":true
                }]),
            ),
            ("timeout_ms".into(), json!(100)),
            ("max_iterations".into(), json!(1)),
            (
                "accept_ambiguous_error_codes".into(),
                json!(["INVOCATION_FAILED"]),
            ),
        ])
    }

    #[test]
    fn click_once_waits_after_exactly_one_successful_dispatch() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
            captured_calls.lock().unwrap().push(operation.into());
            let result = match operation {
                "click" => {
                    assert_eq!(arguments["id"], "dialog.confirmation.accept");
                    json!({"accepted":true})
                }
                "read" => json!({
                    "value":"Собрать общую спецификацию",
                    "visible":true
                }),
                other => panic!("click_once must not dispatch {other}"),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let result =
            execute_click_once_and_wait_for_outcome(&mut client, &click_once_wait_arguments());
        assert_eq!(result["success"], true);
        assert_eq!(result["result"]["dispatch_status"], "accepted");
        assert_eq!(
            result["result"]["matched_token"],
            "Собрать общую спецификацию"
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["click", "read"]);
    }

    #[test]
    fn click_once_observes_marked_indeterminate_invoke_without_retry() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, _: Value| {
            captured_calls.lock().unwrap().push(operation.into());
            match operation {
                "click" => json!({
                    "operation":"click",
                    "success":false,
                    "error":{
                        "code":"INVOCATION_FAILED",
                        "details":{
                            "activation_attempted":true,
                            "attempted_pattern":"InvokePattern",
                            "delivery":"unknown",
                            "fallback_suppressed":true
                        }
                    }
                }),
                "read" => json!({
                    "operation":"read",
                    "success":true,
                    "result":{"value":"Собрать общую спецификацию","visible":true}
                }),
                other => panic!("click_once must not dispatch {other}"),
            }
        });
        let mut client = AiUiClient::new(transport);
        let result =
            execute_click_once_and_wait_for_outcome(&mut client, &click_once_wait_arguments());
        assert_eq!(result["success"], true);
        assert_eq!(
            result["result"]["dispatch_status"],
            "indeterminate_observed"
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["click", "read"]);
    }

    #[test]
    fn click_once_rejects_unmarked_dispatch_error_without_read() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, _: Value| {
            captured_calls.lock().unwrap().push(operation.into());
            json!({
                "operation":operation,
                "success":false,
                "error":{"code":"INVOCATION_FAILED","details":{"activation_attempted":true}}
            })
        });
        let mut client = AiUiClient::new(transport);
        let result =
            execute_click_once_and_wait_for_outcome(&mut client, &click_once_wait_arguments());
        assert_eq!(result["success"], false);
        assert_eq!(result["error"]["code"], "CLICK_ONCE_DISPATCH_FAILED");
        assert_eq!(calls.lock().unwrap().as_slice(), ["click"]);
    }

    #[test]
    fn click_once_timeout_never_sends_a_second_click() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, _: Value| {
            captured_calls.lock().unwrap().push(operation.into());
            match operation {
                "click" => json!({
                    "operation":"click",
                    "success":false,
                    "error":{
                        "code":"INVOCATION_FAILED",
                        "details":{
                            "activation_attempted":true,
                            "attempted_pattern":"InvokePattern",
                            "delivery":"unknown",
                            "fallback_suppressed":true
                        }
                    }
                }),
                "read" => json!({
                    "operation":"read",
                    "success":false,
                    "error":{"code":"ELEMENT_NOT_FOUND"}
                }),
                other => panic!("click_once must not dispatch {other}"),
            }
        });
        let mut client = AiUiClient::new(transport);
        let result =
            execute_click_once_and_wait_for_outcome(&mut client, &click_once_wait_arguments());
        assert_eq!(result["success"], false);
        assert_eq!(result["error"]["code"], "WAIT_FOR_OUTCOME_TIMEOUT");
        assert_eq!(calls.lock().unwrap().as_slice(), ["click", "read"]);
    }
}
