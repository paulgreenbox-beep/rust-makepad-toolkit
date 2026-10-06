use serde_json::{Value, json};

pub const SCHEMA_VERSION: u32 = 2;
pub const CONTRACT_VERSION: &str = "reliability-v2";
pub const TOOL_VERSION: &str = "ai-ui-test-tool-auto-probe";

const ERROR_CODES: &[&str] = &[
    "INVALID_ARGUMENT",
    "INVALID_RELATIVE_COORDINATE",
    "INVALID_SPATIAL_BOUNDS",
    "WINDOW_SESSION_MISMATCH",
    "ACTION_NOT_SUPPORTED_FOR_TARGET",
    "ELEMENT_NOT_INTERACTABLE",
    "ELEMENT_NOT_VISIBLE",
    "CANVAS_UNHEALTHY",
    "APPLICATION_RUNTIME_FAILURE",
    "SEQUENCE_FAILED",
    "HOST_PROTOCOL_ERROR",
];

pub fn canonicalize_response(
    operation: &str,
    mut response: Value,
    action_id: Option<&str>,
) -> Value {
    let nested = if response.get("success").and_then(Value::as_bool) == Some(true) {
        response.get("result").and_then(find_nested_error)
    } else {
        None
    };

    if let Some(error) = nested {
        let dispatched = error
            .get("details")
            .and_then(|v| v.get("dispatched"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        response = json!({
            "success": false,
            "operation": operation,
            "action_id": action_id,
            "phase": "complete",
            "failed_phase": failed_phase_for_code(error["code"].as_str().unwrap_or("")),
            "dispatched": dispatched,
            "target_state_verified": false,
            "postcondition_verified": false,
            "verification_mode": "unverified",
            "application_healthy": true,
            "retry_safe": retry_safe_for("validation", false),
            "idempotency": idempotency_for(operation),
            "error": error,
        });
        return response;
    }

    let success = response.get("success").and_then(Value::as_bool) == Some(true);
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    let result_object = result.as_object();
    let mut object = response.as_object().cloned().unwrap_or_default();
    object.insert("operation".into(), Value::String(operation.into()));
    if let Some(action_id) = action_id {
        object.insert("action_id".into(), Value::String(action_id.into()));
    }
    if success {
        object.insert("phase".into(), Value::String("complete".into()));
        object.insert(
            "dispatched".into(),
            result_object
                .and_then(|v| v.get("dispatched"))
                .cloned()
                .unwrap_or(Value::Bool(false)),
        );
        object.insert(
            "target_state_verified".into(),
            result_object
                .and_then(|v| v.get("target_state_verified"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert(
            "postcondition_verified".into(),
            result_object
                .and_then(|v| v.get("postcondition_verified"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert(
            "verification_mode".into(),
            result_object
                .and_then(|v| v.get("verification_mode"))
                .cloned()
                .unwrap_or_else(|| Value::String("unverified".into())),
        );
        object.insert(
            "verification_strength".into(),
            result_object
                .and_then(|v| v.get("verification_strength"))
                .cloned()
                .unwrap_or_else(|| Value::String("dispatch_only".into())),
        );
        object.insert(
            "application_healthy".into(),
            result_object
                .and_then(|v| v.get("application_health"))
                .and_then(|v| v.get("healthy"))
                .or_else(|| {
                    result_object
                        .and_then(|v| v.get("runtime_health"))
                        .and_then(|v| v.get("healthy"))
                })
                .cloned()
                .unwrap_or(Value::Bool(true)),
        );
        object.insert(
            "retry_safe".into(),
            Value::Bool(retry_safe_for(
                "complete",
                object["dispatched"].as_bool().unwrap_or(false),
            )),
        );
        object.insert("idempotency".into(), idempotency_for(operation));
    } else {
        let error = object.get("error").cloned().unwrap_or_else(|| {
            json!({"code":"HOST_PROTOCOL_ERROR","message":"operation failed without canonical error","details":{}})
        });
        let code = error["code"].as_str().unwrap_or("HOST_PROTOCOL_ERROR");
        object.insert("phase".into(), Value::String("complete".into()));
        object.insert(
            "failed_phase".into(),
            Value::String(failed_phase_for_code(code).into()),
        );
        let dispatched = error
            .get("details")
            .and_then(|v| v.get("dispatched"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        object.insert("dispatched".into(), Value::Bool(dispatched));
        object.insert("target_state_verified".into(), Value::Bool(false));
        object.insert("postcondition_verified".into(), Value::Bool(false));
        object.insert(
            "verification_mode".into(),
            Value::String("unverified".into()),
        );
        let healthy = error
            .get("details")
            .and_then(|v| v.get("runtime_health"))
            .and_then(|v| v.get("healthy"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        object.insert("application_healthy".into(), Value::Bool(healthy));
        object.insert(
            "retry_safe".into(),
            Value::Bool(retry_safe_for(
                object["failed_phase"].as_str().unwrap_or("complete"),
                false,
            )),
        );
        object.insert("idempotency".into(), idempotency_for(operation));
    }
    Value::Object(object)
}

pub fn validate_response(response: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let Some(object) = response.as_object() else {
        return Err(vec!["response must be an object".into()]);
    };
    let success = object.get("success").and_then(Value::as_bool);
    if success.is_none() {
        errors.push("success must be boolean".into());
    }
    if success == Some(true) {
        if object.get("error").is_some_and(|v| !v.is_null()) {
            errors.push("success=true cannot contain error".into());
        }
        if object.get("application_healthy").and_then(Value::as_bool) == Some(false) {
            errors.push("success=true cannot have application_healthy=false".into());
        }
        if object
            .get("postcondition_required")
            .and_then(Value::as_bool)
            == Some(true)
            && object
                .get("postcondition_verified")
                .and_then(Value::as_bool)
                != Some(true)
        {
            errors.push("required postcondition is not verified".into());
        }
    } else if success == Some(false) {
        if object
            .get("error")
            .and_then(Value::as_object)
            .and_then(|v| v.get("code"))
            .and_then(Value::as_str)
            .is_none()
        {
            errors.push("success=false requires error.code".into());
        }
        if object.get("failed_phase").and_then(Value::as_str) == Some("validation")
            && object.get("dispatched").and_then(Value::as_bool) == Some(true)
        {
            errors.push("validation failure cannot be dispatched".into());
        }
    }
    if object.get("status").and_then(Value::as_str) == Some("AI_UI_SPATIAL_PASSED") {
        let required = object
            .get("required_markers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let passed = object
            .get("passed_markers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if required.iter().any(|marker| !passed.contains(marker)) {
            errors.push("full pass status has missing required marker".into());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn validate_evidence_bundle(bundle: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for key in ["schema_version", "tool_version", "contract_version"] {
        if bundle.get(key).is_none() {
            errors.push(format!("missing top-level {key}"));
        }
    }
    if let Some(actions) = bundle.get("actions").and_then(Value::as_array) {
        for (index, action) in actions.iter().enumerate() {
            if action.get("action_id").and_then(Value::as_str).is_none() {
                errors.push(format!("actions[{index}] missing action_id"));
            }
            if let Some(result) = action.get("result") {
                if let Err(result_errors) = validate_response(result) {
                    errors.extend(
                        result_errors
                            .into_iter()
                            .map(|error| format!("actions[{index}]: {error}")),
                    );
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn find_nested_error(value: &Value) -> Option<Value> {
    if let Some(object) = value.as_object() {
        if object.get("success").and_then(Value::as_bool) == Some(false) {
            return object
                .get("error")
                .cloned()
                .or_else(|| Some(Value::Object(object.clone())));
        }
        if let Some(code) = object.get("code").and_then(Value::as_str) {
            if ERROR_CODES.contains(&code)
                || code.starts_with("APPLICATION_")
                || code.starts_with("INVALID_")
            {
                return Some(json!({
                    "code": code,
                    "message": object.get("message").and_then(Value::as_str).unwrap_or("nested operation failed"),
                    "details": object.get("details").cloned().unwrap_or_else(|| json!({}))
                }));
            }
        }
        for child in object.values() {
            if let Some(error) = find_nested_error(child) {
                return Some(error);
            }
        }
    }
    if let Some(array) = value.as_array() {
        for child in array {
            if let Some(error) = find_nested_error(child) {
                return Some(error);
            }
        }
    }
    None
}

fn failed_phase_for_code(code: &str) -> &'static str {
    if code.starts_with("INVALID_")
        || code == "WINDOW_SESSION_MISMATCH"
        || code == "ACTION_NOT_SUPPORTED_FOR_TARGET"
    {
        "validation"
    } else if code == "ELEMENT_NOT_INTERACTABLE"
        || code == "ELEMENT_NOT_VISIBLE"
        || code == "CANVAS_UNHEALTHY"
    {
        "precondition"
    } else if code == "APPLICATION_RUNTIME_FAILURE" {
        "runtime_check"
    } else if code == "SEQUENCE_FAILED" {
        "verification"
    } else {
        "complete"
    }
}

fn retry_safe_for(phase: &str, dispatched: bool) -> bool {
    !dispatched && matches!(phase, "validation" | "precondition" | "resolution")
}

fn idempotency_for(operation: &str) -> Value {
    if matches!(
        operation,
        "inspect_ui"
            | "inspect_semantic"
            | "inspect_spatial_target"
            | "read"
            | "get_runtime_health"
            | "get_runtime_stderr"
            | "capture_region"
    ) {
        Value::String("idempotent".into())
    } else if matches!(
        operation,
        "click_relative"
            | "double_click_relative"
            | "right_click_relative"
            | "drag_relative"
            | "select_region"
            | "pointer_down"
            | "pointer_up"
            | "wheel_relative"
            | "click"
            | "input"
            | "press"
            | "press_repeat"
            | "type_text"
    ) {
        Value::String("non_idempotent".into())
    } else {
        Value::String("unknown".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_success_with_nested_error_is_rejected() {
        let response = canonicalize_response(
            "move_pointer",
            json!({"success":true,"result":{"dispatched":true,"result":{"code":"INVALID_ARGUMENT","message":"x"}}}),
            Some("a1"),
        );
        assert_eq!(response["success"], false);
        assert_eq!(response["error"]["code"], "INVALID_ARGUMENT");
        assert_eq!(response["dispatched"], false);
        assert_eq!(response["failed_phase"], "validation");
        assert!(validate_response(&response).is_ok());
    }

    #[test]
    fn validation_failure_cannot_claim_dispatch() {
        let response = canonicalize_response(
            "click_relative",
            json!({"success":false,"error":{"code":"INVALID_RELATIVE_COORDINATE","message":"x","details":{}}}),
            Some("a2"),
        );
        assert_eq!(response["dispatched"], false);
        assert!(validate_response(&response).is_ok());
    }

    #[test]
    fn malformed_evidence_fixtures_are_rejected() {
        let malformed = json!({
            "schema_version": 2,
            "tool_version": TOOL_VERSION,
            "contract_version": CONTRACT_VERSION,
            "actions": [{
                "action_id": "a1",
                "result": {"success":true,"error":{"code":"BAD"}}
            }]
        });
        assert!(validate_evidence_bundle(&malformed).is_err());
    }
}
