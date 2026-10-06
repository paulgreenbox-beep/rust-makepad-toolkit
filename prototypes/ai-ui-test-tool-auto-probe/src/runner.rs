//! Generic script runner. It depends only on the public client transport.
use crate::{
    api::{AiUiClient, Transport},
    fixtures::{self, FixtureStore},
    observation::PortableEvidenceRef,
    script::{Script, substitute},
    script_controls,
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    path::PathBuf,
    time::Instant,
};

#[derive(Clone, Debug, serde::Serialize)]
pub struct Failure {
    pub test: String,
    pub step: String,
    pub operation: String,
    pub category: String,
    pub error_code: String,
    pub failed_phase: Option<String>,
    pub action_id: Option<String>,
    pub evidence_ref: Option<String>,
    pub runtime_healthy: Option<bool>,
    pub suggested_next_action: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct StepReport {
    pub index: usize,
    pub step: String,
    pub operation: String,
    pub status: String,
    pub response: Value,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ScriptReport {
    pub status: String,
    pub test: String,
    pub summary: Summary,
    pub failures: Vec<Failure>,
    pub steps: Vec<StepReport>,
    pub evidence_refs: Vec<String>,
    pub markers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<PortableEvidenceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_ref: Option<PortableEvidenceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_ref: Option<PortableEvidenceRef>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Summary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug)]
struct AutoGestureLock {
    token: String,
    button: String,
}

#[derive(Clone, Debug)]
enum AutoInputLock {
    Acquired { session_id: String, token: String },
    Retained { session_id: String },
}

#[derive(Clone, Debug)]
struct CleanupCall {
    operation: &'static str,
    response: Value,
}

pub fn run<T: Transport>(
    client: &mut AiUiClient<T>,
    script: &Script,
    output: impl AsRef<Path>,
) -> Result<ScriptReport, String> {
    run_with_fixtures(client, script, output, &FixtureStore::default())
}

pub fn run_with_fixtures<T: Transport>(
    client: &mut AiUiClient<T>,
    script: &Script,
    output: impl AsRef<Path>,
    fixture_store: &FixtureStore,
) -> Result<ScriptReport, String> {
    let started = Instant::now();
    let mut vars = script.vars.clone();
    fixture_store.merge_for_script(script, &mut vars)?;
    let secret_literals = fixture_store.secret_literals(script, &vars);
    let mut sessions: BTreeMap<String, String> = BTreeMap::new();
    let mut current_session: Option<String> = None;
    let mut auto_gesture_locks: BTreeMap<String, AutoGestureLock> = BTreeMap::new();
    let mut completed: BTreeMap<String, bool> = BTreeMap::new();
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    let mut evidence_refs = Vec::new();
    let mut failed = false;
    let mut all_steps = Vec::new();
    all_steps.extend(script.setup.iter().cloned());
    all_steps.extend(script.steps.iter().cloned());
    for (index, step) in all_steps.iter().enumerate() {
        let step_name = step
            .step_id
            .clone()
            .unwrap_or_else(|| format!("step-{index}"));
        let dependency_failed = step
            .depends_on
            .iter()
            .any(|id| completed.get(id) == Some(&false));
        if dependency_failed {
            reports.push(StepReport {
                index,
                step: step_name.clone(),
                operation: step.op.clone(),
                status: "SKIPPED_DEPENDENCY_FAILED".into(),
                response: json!({"operation":step.op,"success":false,"skipped":true,"reason":"dependency failed"}),
            });
            completed.insert(step_name, false);
            continue;
        }
        if let Some(condition) = step.arguments.get("when") {
            let should_run = script_controls::evaluate_when(condition, &vars)
                .map_err(|error| format!("invalid when control at {step_name}: {error}"))?;
            if !should_run {
                reports.push(StepReport {
                    index,
                    step: step_name.clone(),
                    operation: step.op.clone(),
                    status: "SKIPPED_CONDITION".into(),
                    response: json!({
                        "operation":step.op,
                        "success":true,
                        "skipped":true,
                        "reason":"when condition evaluated false"
                    }),
                });
                completed.insert(step_name, true);
                continue;
            }
        }
        let mut args = Map::new();
        for (key, value) in &step.arguments {
            args.insert(key.clone(), substitute(value, &vars));
        }
        let extract_spec = args.remove("extract");
        args.remove("when");
        let selected_session = step
            .session
            .as_ref()
            .and_then(|alias| sessions.get(alias))
            .cloned()
            .or_else(|| current_session.clone());
        if step.op != "session.open" {
            if let Some(session_id) = selected_session.clone() {
                args.insert("session_id".into(), Value::String(session_id));
            }
        }
        let auto_lock = match prepare_auto_input_lock(
            client,
            &step.op,
            selected_session.as_deref(),
            &mut args,
            &auto_gesture_locks,
        ) {
            Ok(lock) => lock,
            Err(lock) => {
                reports.push(StepReport {
                    index,
                    step: step_name.clone(),
                    operation: step.op.clone(),
                    status: "FAIL".into(),
                    response: lock.clone(),
                });
                completed.insert(step_name.clone(), false);
                failed = true;
                failures.push(input_lock_failure(
                    &script.name,
                    &step_name,
                    &step.op,
                    &lock,
                ));
                if script.options.stop_on_error && !script.options.collect_failures {
                    break;
                }
                continue;
            }
        };
        let secret_fields = fixtures::secret_argument_fields(&args, &secret_literals);
        let redact_response = step.redact || !secret_fields.is_empty();
        if !secret_fields.is_empty() {
            args.insert("_aiui_redact_fields".into(), json!(secret_fields));
        }
        if redact_response {
            args.insert("_aiui_redact_response".into(), Value::Bool(true));
        }
        let pointer_button = args
            .get("button")
            .and_then(Value::as_str)
            .unwrap_or("left")
            .to_owned();
        let pre_close_cleanup = cleanup_before_session_close(
            client,
            &step.op,
            args.get("session_id").and_then(Value::as_str),
            &mut auto_gesture_locks,
        );
        let pre_cleanup_ok = append_cleanup_reports(
            &mut reports,
            &mut failures,
            &script.name,
            index,
            &step_name,
            pre_close_cleanup,
        );
        if !pre_cleanup_ok {
            failed = true;
        }
        let raw_response = match step.op.as_str() {
            "repeat_until" => script_controls::execute_repeat_until(client, &args),
            "wait_for_outcome" => script_controls::execute_wait_for_outcome(client, &args),
            "click_once_and_wait_for_outcome" => {
                script_controls::execute_click_once_and_wait_for_outcome(client, &args)
            }
            _ => client.call(&step.op, Value::Object(args)),
        };
        let mut response = fixtures::redact_value(&raw_response, &secret_literals, step.redact);
        let success = apply_extraction(&mut response, &step.op, extract_spec.as_ref(), &mut vars);
        let post_action_cleanup = finish_auto_input_lock(
            client,
            &step.op,
            success,
            auto_lock,
            &pointer_button,
            &mut auto_gesture_locks,
        );
        let actual_error = response
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let expected_error = step.expect_error.as_deref();
        let expected_ok = expected_error
            .is_some_and(|expected| !success && actual_error.as_deref() == Some(expected));
        let action_passed = expected_error.map_or(success, |_| expected_ok);
        if !action_passed {
            if let Some(expected) = expected_error {
                response["expectation_error"] = json!({
                    "code":"EXPECTED_ERROR_NOT_OBSERVED",
                    "expected_error":expected,
                    "observed_error":actual_error,
                    "observed_success":success
                });
            }
        }
        let status = if expected_ok {
            "PASS_EXPECTED_ERROR"
        } else if action_passed {
            "PASS"
        } else {
            "FAIL"
        };
        let post_cleanup_ok = append_cleanup_reports(
            &mut reports,
            &mut failures,
            &script.name,
            index,
            &step_name,
            post_action_cleanup,
        );
        if !post_cleanup_ok {
            failed = true;
        }
        let passed = action_passed && pre_cleanup_ok && post_cleanup_ok;
        if let Some(name) = &step.save_as {
            vars.insert(
                name.clone(),
                response
                    .get("result")
                    .cloned()
                    .unwrap_or_else(|| response.clone()),
            );
        }
        if let Some(alias) = &step.save_session_as {
            if let Some(id) = response
                .get("result")
                .and_then(|v| v.get("session_id"))
                .and_then(Value::as_str)
            {
                sessions.insert(alias.clone(), id.into());
                current_session = Some(id.into());
            }
        } else if step.op == "session.open" {
            if let Some(id) = response
                .get("result")
                .and_then(|v| v.get("session_id"))
                .and_then(Value::as_str)
            {
                current_session = Some(id.into());
            }
        }
        if success && step.op == "session.close" {
            if let Some(closed) = selected_session.as_deref() {
                sessions.retain(|_, session_id| session_id != closed);
                if current_session.as_deref() == Some(closed) {
                    current_session = None;
                }
            }
        }
        reports.push(StepReport {
            index,
            step: step_name.clone(),
            operation: step.op.clone(),
            status: if action_passed && !passed {
                "FAIL_CLEANUP".into()
            } else {
                status.into()
            },
            response: response.clone(),
        });
        completed.insert(step_name.clone(), passed);
        if !action_passed {
            failed = true;
            let code = if expected_error.is_some() {
                "EXPECTED_ERROR_NOT_OBSERVED".to_string()
            } else {
                actual_error.unwrap_or_else(|| "SCRIPT_ERROR".to_string())
            };
            let category = category_for(&code, &step.op);
            failures.push(Failure {
                test: script.name.clone(),
                step: step_name,
                operation: step.op.clone(),
                category,
                error_code: code,
                failed_phase: response
                    .get("failed_phase")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                action_id: response
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                evidence_ref: response
                    .get("error")
                    .and_then(|v| v.get("details"))
                    .and_then(|v| v.get("evidence_path"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                runtime_healthy: response.get("application_healthy").and_then(Value::as_bool),
                suggested_next_action: "inspect evidence and last observation".into(),
            });
            if script.options.evidence_on_failure {
                if let Some(session_id) = current_session.clone() {
                    let evidence = client.call(
                        "windows_export_evidence",
                        json!({"session_id":session_id,"reason":format!("script-failure-{index}")}),
                    );
                    if let Some(path) = evidence
                        .get("result")
                        .and_then(|v| v.get("path"))
                        .and_then(Value::as_str)
                    {
                        evidence_refs.push(path.into());
                    }
                }
            }
            if script.options.stop_on_error && !script.options.collect_failures {
                break;
            }
        }
    }
    for (offset, step) in script.teardown.iter().enumerate() {
        let teardown_index = all_steps.len() + offset;
        let teardown_step_name = step
            .step_id
            .clone()
            .unwrap_or_else(|| format!("teardown-{offset}"));
        let mut args = Map::new();
        for (key, value) in &step.arguments {
            args.insert(key.clone(), substitute(value, &vars));
        }
        let extract_spec = args.remove("extract");
        args.remove("when");
        let selected_session = step
            .session
            .as_ref()
            .and_then(|alias| sessions.get(alias))
            .cloned()
            .or_else(|| current_session.clone());
        if step.op != "session.open" {
            if let Some(id) = selected_session.clone() {
                args.entry("session_id").or_insert(Value::String(id));
            }
        }
        let auto_lock = match prepare_auto_input_lock(
            client,
            &step.op,
            selected_session.as_deref(),
            &mut args,
            &auto_gesture_locks,
        ) {
            Ok(lock) => lock,
            Err(lock) => {
                reports.push(StepReport {
                    index: teardown_index,
                    step: teardown_step_name.clone(),
                    operation: step.op.clone(),
                    status: "TEARDOWN_FAILURE".into(),
                    response: lock.clone(),
                });
                failures.push(input_lock_failure(
                    &script.name,
                    &teardown_step_name,
                    &step.op,
                    &lock,
                ));
                failed = true;
                continue;
            }
        };
        let secret_fields = fixtures::secret_argument_fields(&args, &secret_literals);
        let redact_response = step.redact || !secret_fields.is_empty();
        if !secret_fields.is_empty() {
            args.insert("_aiui_redact_fields".into(), json!(secret_fields));
        }
        if redact_response {
            args.insert("_aiui_redact_response".into(), Value::Bool(true));
        }
        let pointer_button = args
            .get("button")
            .and_then(Value::as_str)
            .unwrap_or("left")
            .to_owned();
        let closing_session_id = if step.op == "session.close" {
            args.get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let pre_close_cleanup = cleanup_before_session_close(
            client,
            &step.op,
            closing_session_id.as_deref(),
            &mut auto_gesture_locks,
        );
        let pre_cleanup_ok = append_cleanup_reports(
            &mut reports,
            &mut failures,
            &script.name,
            teardown_index,
            &teardown_step_name,
            pre_close_cleanup,
        );
        if !pre_cleanup_ok {
            failed = true;
        }
        let raw_response = match step.op.as_str() {
            "repeat_until" => script_controls::execute_repeat_until(client, &args),
            "wait_for_outcome" => script_controls::execute_wait_for_outcome(client, &args),
            "click_once_and_wait_for_outcome" => {
                script_controls::execute_click_once_and_wait_for_outcome(client, &args)
            }
            _ => client.call(&step.op, Value::Object(args)),
        };
        let mut response = fixtures::redact_value(&raw_response, &secret_literals, step.redact);
        let success = apply_extraction(&mut response, &step.op, extract_spec.as_ref(), &mut vars);
        let post_action_cleanup = finish_auto_input_lock(
            client,
            &step.op,
            success,
            auto_lock,
            &pointer_button,
            &mut auto_gesture_locks,
        );
        let post_cleanup_ok = append_cleanup_reports(
            &mut reports,
            &mut failures,
            &script.name,
            teardown_index,
            &teardown_step_name,
            post_action_cleanup,
        );
        if !post_cleanup_ok {
            failed = true;
        }
        if success {
            if let Some(closed) = closing_session_id.as_deref() {
                sessions.retain(|_, session_id| session_id != closed);
                if current_session.as_deref() == Some(closed) {
                    current_session = None;
                }
            }
        }
        reports.push(StepReport {
            index: teardown_index,
            step: teardown_step_name.clone(),
            operation: step.op.clone(),
            status: if !success {
                "TEARDOWN_FAILURE".into()
            } else if !pre_cleanup_ok || !post_cleanup_ok {
                "CLEANUP_FAILURE".into()
            } else {
                "PASS".into()
            },
            response: response.clone(),
        });
        if !success {
            failed = true;
            failures.push(Failure {
                test: script.name.clone(),
                step: teardown_step_name,
                operation: step.op.clone(),
                category: category_for(
                    response
                        .get("error")
                        .and_then(|error| error.get("code"))
                        .and_then(Value::as_str)
                        .unwrap_or("SCRIPT_ERROR"),
                    &step.op,
                ),
                error_code: response
                    .get("error")
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("SCRIPT_ERROR")
                    .into(),
                failed_phase: response
                    .get("failed_phase")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                action_id: response
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                evidence_ref: response
                    .get("error")
                    .and_then(|error| error.get("details"))
                    .and_then(|details| details.get("evidence_path"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                runtime_healthy: response.get("application_healthy").and_then(Value::as_bool),
                suggested_next_action: "inspect teardown evidence and session state".into(),
            });
            if script.options.evidence_on_failure {
                if let Some(session_id) = selected_session.as_deref() {
                    let evidence = client.call(
                        "windows_export_evidence",
                        json!({"session_id":session_id,"reason":format!("teardown-failure-{offset}")}),
                    );
                    if let Some(path) = evidence
                        .get("result")
                        .and_then(|value| value.get("path"))
                        .and_then(Value::as_str)
                    {
                        evidence_refs.push(path.into());
                    }
                }
            }
        }
    }
    let final_gesture_cleanup = cleanup_all_retained_gestures(client, &mut auto_gesture_locks);
    if !append_cleanup_reports(
        &mut reports,
        &mut failures,
        &script.name,
        all_steps.len() + script.teardown.len(),
        "final-gesture-cleanup",
        final_gesture_cleanup,
    ) {
        failed = true;
    }
    let mut remaining_sessions: BTreeSet<String> = sessions.values().cloned().collect();
    if let Some(id) = current_session.take() {
        remaining_sessions.insert(id);
    }
    for (offset, id) in remaining_sessions.into_iter().enumerate() {
        let response = client.call("session.close", json!({"session_id":id}));
        let success = response.get("success").and_then(Value::as_bool) == Some(true);
        reports.push(StepReport {
            index: all_steps.len() + script.teardown.len() + offset,
            step: format!("auto-session-close-{offset}"),
            operation: "session.close".into(),
            status: if success {
                "PASS_AUTO_CLEANUP".into()
            } else {
                "CLEANUP_FAILURE".into()
            },
            response: response.clone(),
        });
        if !success {
            failed = true;
            evidence_refs.push(
                response
                    .get("error")
                    .and_then(|error| error.get("details"))
                    .and_then(|details| details.get("evidence_path"))
                    .and_then(Value::as_str)
                    .unwrap_or("session-close-failed")
                    .into(),
            );
            failures.push(Failure {
                test: script.name.clone(),
                step: format!("auto-session-close-{offset}"),
                operation: "session.close".into(),
                category: "CLEANUP_FAILURE".into(),
                error_code: response
                    .get("error")
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("SESSION_CLOSE_FAILED")
                    .into(),
                failed_phase: response
                    .get("failed_phase")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                action_id: response
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                evidence_ref: response
                    .get("error")
                    .and_then(|error| error.get("details"))
                    .and_then(|details| details.get("evidence_path"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                runtime_healthy: response.get("application_healthy").and_then(Value::as_bool),
                suggested_next_action: "inspect session teardown and child process ownership"
                    .into(),
            });
        }
    }
    let passed = reports
        .iter()
        .filter(|r| r.status.starts_with("PASS"))
        .count();
    let skipped = reports
        .iter()
        .filter(|r| r.status.starts_with("SKIPPED"))
        .count();
    let mut markers = Vec::new();
    let api_parity = !failed
        && reports.iter().all(|report| {
            report.response.get("operation").and_then(Value::as_str)
                == Some(report.operation.as_str())
        });
    if api_parity {
        markers.push("AI_UI_SCRIPT_API_PARITY_PASSED".to_string());
        markers.push("AI_UI_SCRIPT_RUNNER_PARITY_PASSED".to_string());
    }
    if script.steps.iter().any(|step| step.expect_error.is_some()) && !failed {
        markers.push("AI_UI_SCRIPT_NEGATIVE_TESTS_PASSED".to_string());
    }
    if script.options.collect_failures && failed && skipped > 0 {
        markers.push("AI_UI_SCRIPT_ERROR_COLLECTION_PASSED".to_string());
    }
    if script.generated_by.is_some() && !failed {
        markers.push("AI_UI_GENERATED_SCRIPT_VALIDATED".to_string());
    }
    if !script.secret_vars.is_empty() {
        markers.push("AI_UI_SECRET_REDACTION_PASSED".to_string());
    }
    if script.name.to_ascii_lowercase().contains("self-test") && !failed {
        markers.push("AI_UI_SCRIPT_SELF_TEST_PASSED".to_string());
    }
    append_observation_markers(&mut markers, script, &reports, failed);
    markers.sort();
    markers.dedup();
    let dir = output.as_ref();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let evidence_ref = primary_evidence_ref(&reports, dir);
    let screenshot_ref = observation_artifact_ref(&reports, "screenshot_ref");
    let report_ref = Some(output_artifact_ref(dir, "report.json")?);
    let summary = ScriptReport {
        status: if failed {
            "FAILED".into()
        } else {
            "PASSED".into()
        },
        test: script.name.clone(),
        summary: Summary {
            total: reports.len(),
            passed,
            failed: failures.len(),
            skipped,
        },
        failures,
        steps: reports,
        evidence_refs,
        markers,
        evidence_ref,
        screenshot_ref,
        report_ref,
    };
    let ai = json!({"summary":summary.summary,"failures":summary.failures,"status":summary.status,"markers":summary.markers,"evidence_ref":summary.evidence_ref,"screenshot_ref":summary.screenshot_ref,"report_ref":summary.report_ref,"duration_ms":started.elapsed().as_millis()});
    fs::write(
        dir.join("ai-report.json"),
        serde_json::to_vec_pretty(&ai).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        dir.join("report.json"),
        serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(dir.join("report.md"), report_markdown(&summary)).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Compact, secret-safe handoff report written beside the machine artifacts.
///
/// Detailed step payloads intentionally remain only in `report.json`.
pub fn report_markdown(report: &ScriptReport) -> String {
    let failures = if report.failures.is_empty() {
        "None.".to_owned()
    } else {
        let json =
            serde_json::to_string_pretty(&report.failures).unwrap_or_else(|_| "[]".to_owned());
        format!("```json\n{json}\n```")
    };
    let evidence_refs =
        serde_json::to_string_pretty(&report.evidence_refs).unwrap_or_else(|_| "[]".to_owned());
    let markers = serde_json::to_string_pretty(&report.markers).unwrap_or_else(|_| "[]".to_owned());
    let artifact_refs = serde_json::to_string_pretty(&json!({
        "evidence_ref": &report.evidence_ref,
        "screenshot_ref": &report.screenshot_ref,
        "report_ref": &report.report_ref,
    }))
    .unwrap_or_else(|_| "{}".to_owned());
    format!(
        "# AI UI Test Report\n\nStatus: **{}**\n\nTest: `{}`\n\n## Summary\n\n- Total: {}\n- Passed: {}\n- Failed: {}\n- Skipped: {}\n\n## Failures\n\n{}\n\n## Artifact refs\n\n```json\n{}\n```\n\n## Evidence refs\n\n```json\n{}\n```\n\n## Markers\n\n```json\n{}\n```\n",
        report.status,
        serde_json::to_string(&report.test).unwrap_or_else(|_| "\"unknown\"".to_owned()),
        report.summary.total,
        report.summary.passed,
        report.summary.failed,
        report.summary.skipped,
        failures,
        artifact_refs,
        evidence_refs,
        markers,
    )
}

fn observation_artifact_ref(reports: &[StepReport], field: &str) -> Option<PortableEvidenceRef> {
    reports
        .iter()
        .rev()
        .filter(|report| {
            report.status.starts_with("PASS")
                && report.response.get("success").and_then(Value::as_bool) == Some(true)
        })
        .find_map(|report| {
            report
                .response
                .get("result")
                .and_then(|result| result.get(field))
                .cloned()
        })
        .and_then(|value| serde_json::from_value(value).ok())
}

fn primary_evidence_ref(reports: &[StepReport], output: &Path) -> Option<PortableEvidenceRef> {
    exported_evidence_ref(reports, output)
        .or_else(|| observation_artifact_ref(reports, "evidence_ref"))
}

fn exported_evidence_ref(reports: &[StepReport], output: &Path) -> Option<PortableEvidenceRef> {
    reports
        .iter()
        .rev()
        .filter(|report| {
            report.operation == "windows_export_evidence"
                && report.status.starts_with("PASS")
                && report.response.get("success").and_then(Value::as_bool) == Some(true)
        })
        .find_map(|report| {
            report
                .response
                .get("result")
                .and_then(|result| result.get("path"))
                .and_then(Value::as_str)
                .and_then(|path| existing_artifact_ref(path, output))
        })
}

fn existing_artifact_ref(path: &str, output: &Path) -> Option<PortableEvidenceRef> {
    let resolved = fs::canonicalize(path).ok()?;
    let absolute_path = resolved.to_string_lossy().into_owned();
    let relative_path = fs::canonicalize(output)
        .ok()
        .and_then(|root| resolved.strip_prefix(root).ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            resolved
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("artifact"))
        })
        .to_string_lossy()
        .trim_start_matches(['\\', '/'])
        .replace('\\', "/");
    Some(PortableEvidenceRef {
        relative_path,
        resolved_path: absolute_path.clone(),
        absolute_path: Some(absolute_path),
        environment_path: None,
        host_artifact_path: None,
    })
}

fn output_artifact_ref(output: &Path, name: &str) -> Result<PortableEvidenceRef, String> {
    let candidate = output.join(name);
    let absolute: PathBuf = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve report output root: {error}"))?
            .join(candidate)
    };
    let absolute_path = absolute.to_string_lossy().into_owned();
    Ok(PortableEvidenceRef {
        relative_path: name.into(),
        resolved_path: absolute_path.clone(),
        absolute_path: Some(absolute_path),
        environment_path: None,
        host_artifact_path: None,
    })
}

/// Derive milestone markers from successful public operations, never from tags
/// alone. Expected-error responses have `success: false`, so negative scripts
/// cannot receive a positive observation-domain marker.
fn append_observation_markers(
    markers: &mut Vec<String>,
    script: &Script,
    reports: &[StepReport],
    failed: bool,
) {
    if failed
        || !script
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("observation"))
    {
        return;
    }
    let successful_reports = reports
        .iter()
        .filter(|report| {
            report.status.starts_with("PASS")
                && report.response.get("success").and_then(Value::as_bool) == Some(true)
        })
        .collect::<Vec<_>>();
    let successful_operations = successful_reports
        .iter()
        .map(|report| report.operation.as_str())
        .collect::<BTreeSet<_>>();
    let has_operations = |operations: &[&str]| {
        operations
            .iter()
            .all(|operation| successful_operations.contains(operation))
    };
    let has_successful_control = |key: &str| {
        script
            .setup
            .iter()
            .chain(script.steps.iter())
            .chain(script.teardown.iter())
            .filter(|step| step.arguments.contains_key(key))
            .any(|step| {
                step.step_id.as_deref().is_some_and(|step_id| {
                    successful_reports
                        .iter()
                        .any(|report| report.step == step_id)
                })
            })
    };
    if has_operations(&["wait_for_task_state", "assert_task_success"]) {
        markers.push("AI_UI_RUNTIME_TASK_OBSERVATION_PASSED".into());
    }
    if has_operations(&["wait_for_runtime_milestone"]) {
        markers.push("AI_UI_RUNTIME_MILESTONE_PASSED".into());
    }
    if has_operations(&[
        "get_api_request_state",
        "wait_for_api_response",
        "assert_api_success",
    ]) {
        markers.push("AI_UI_EXTERNAL_REQUEST_OBSERVATION_PASSED".into());
    }
    if has_operations(&["wait_for_file", "assert_file_exists"]) {
        markers.push("AI_UI_FILE_POSTCONDITION_PASSED".into());
    }
    if has_operations(&["wait_for_file_stable"]) {
        markers.push("AI_UI_FILE_STABILITY_PASSED".into());
    }
    if has_operations(&[
        "assert_region_changed",
        "assert_not_blank",
        "get_makepad_visual_diagnostics",
    ]) {
        markers.push("AI_UI_VISUAL_ASSERTIONS_PASSED".into());
    }
    if has_operations(&["get_application_logs", "assert_trace_event"]) {
        markers.push("AI_UI_TRACE_ASSERTIONS_PASSED".into());
    }
    if has_successful_control("extract") {
        markers.push("AI_UI_VARIABLE_EXTRACTION_PASSED".into());
    }
    if has_successful_control("when") && has_successful_control("repeat_until") {
        markers.push("AI_UI_LIMITED_WORKFLOW_PASSED".into());
    }
    let async_markers = [
        "AI_UI_RUNTIME_TASK_OBSERVATION_PASSED",
        "AI_UI_RUNTIME_MILESTONE_PASSED",
        "AI_UI_EXTERNAL_REQUEST_OBSERVATION_PASSED",
        "AI_UI_FILE_POSTCONDITION_PASSED",
        "AI_UI_FILE_STABILITY_PASSED",
        "AI_UI_VISUAL_ASSERTIONS_PASSED",
        "AI_UI_TRACE_ASSERTIONS_PASSED",
        "AI_UI_VARIABLE_EXTRACTION_PASSED",
        "AI_UI_LIMITED_WORKFLOW_PASSED",
    ];
    if has_operations(&[
        "windows_export_evidence",
        "assert_no_pending_task",
        "assert_no_stale_window",
    ]) && async_markers
        .iter()
        .all(|marker| markers.iter().any(|actual| actual == marker))
    {
        markers.push("AI_UI_ASYNC_WORKFLOW_E2E_PASSED".into());
    }
}

fn apply_extraction(
    response: &mut Value,
    operation: &str,
    specification: Option<&Value>,
    vars: &mut Map<String, Value>,
) -> bool {
    if response.get("success").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let Some(specification) = specification else {
        return true;
    };
    let source = response
        .get("result")
        .cloned()
        .unwrap_or_else(|| response.clone());
    match script_controls::extract_values(&source, specification) {
        Ok(extracted) => {
            vars.extend(extracted);
            true
        }
        Err(error) => {
            *response = json!({
                "operation":operation,
                "success":false,
                "failed_phase":"extraction",
                "error":error
            });
            false
        }
    }
}

fn is_physical_operation(operation: &str) -> bool {
    crate::catalog::operation(operation).is_some_and(|spec| spec.physical_input)
}
fn prepare_auto_input_lock<T: Transport>(
    client: &mut AiUiClient<T>,
    operation: &str,
    selected_session: Option<&str>,
    arguments: &mut Map<String, Value>,
    retained: &BTreeMap<String, AutoGestureLock>,
) -> Result<Option<AutoInputLock>, Value> {
    if !is_physical_operation(operation)
        || arguments.contains_key("input_lock_token")
        || selected_session.is_none()
    {
        return Ok(None);
    }
    let session_id = selected_session.expect("selected_session was checked");
    if let Some(gesture) = retained.get(session_id) {
        arguments.insert(
            "input_lock_token".into(),
            Value::String(gesture.token.clone()),
        );
        return Ok(Some(AutoInputLock::Retained {
            session_id: session_id.to_owned(),
        }));
    }
    let lock = client.call("acquire_input_lock", json!({"session_id":session_id}));
    if lock["success"] != true {
        return Err(lock);
    }
    let Some(token) = lock["result"]["token"]
        .as_str()
        .filter(|token| !token.is_empty())
    else {
        return Err(json!({
            "operation":"acquire_input_lock",
            "success":false,
            "error":{
                "code":"INPUT_LOCK_TOKEN_MISSING",
                "message":"input lock acquisition succeeded without a token",
                "details":lock
            }
        }));
    };
    let token = token.to_owned();
    arguments.insert("input_lock_token".into(), Value::String(token.clone()));
    Ok(Some(AutoInputLock::Acquired {
        session_id: session_id.to_owned(),
        token,
    }))
}

fn finish_auto_input_lock<T: Transport>(
    client: &mut AiUiClient<T>,
    operation: &str,
    action_success: bool,
    auto_lock: Option<AutoInputLock>,
    pointer_button: &str,
    retained: &mut BTreeMap<String, AutoGestureLock>,
) -> Vec<CleanupCall> {
    match auto_lock {
        Some(AutoInputLock::Acquired { session_id, token })
            if operation == "pointer_down" && action_success =>
        {
            retained.insert(
                session_id,
                AutoGestureLock {
                    token,
                    button: pointer_button.to_owned(),
                },
            );
            Vec::new()
        }
        Some(AutoInputLock::Acquired { session_id, token }) => vec![CleanupCall {
            operation: "release_input_lock",
            response: client.call(
                "release_input_lock",
                json!({"session_id":session_id,"token":token}),
            ),
        }],
        Some(AutoInputLock::Retained { session_id })
            if operation == "pointer_up" && action_success =>
        {
            let Some(gesture) = retained.remove(&session_id) else {
                return Vec::new();
            };
            vec![CleanupCall {
                operation: "release_input_lock",
                response: client.call(
                    "release_input_lock",
                    json!({"session_id":session_id,"token":gesture.token}),
                ),
            }]
        }
        Some(AutoInputLock::Retained { session_id })
            if operation == "pointer_up" || !action_success =>
        {
            let Some(gesture) = retained.remove(&session_id) else {
                return Vec::new();
            };
            release_retained_gesture(client, &session_id, gesture)
        }
        _ => Vec::new(),
    }
}

fn cleanup_before_session_close<T: Transport>(
    client: &mut AiUiClient<T>,
    operation: &str,
    session_id: Option<&str>,
    retained: &mut BTreeMap<String, AutoGestureLock>,
) -> Vec<CleanupCall> {
    if operation != "session.close" {
        return Vec::new();
    }
    let Some(session_id) = session_id else {
        return Vec::new();
    };
    let Some(gesture) = retained.remove(session_id) else {
        return Vec::new();
    };
    release_retained_gesture(client, session_id, gesture)
}

fn cleanup_all_retained_gestures<T: Transport>(
    client: &mut AiUiClient<T>,
    retained: &mut BTreeMap<String, AutoGestureLock>,
) -> Vec<CleanupCall> {
    let retained = std::mem::take(retained);
    retained
        .into_iter()
        .flat_map(|(session_id, gesture)| release_retained_gesture(client, &session_id, gesture))
        .collect()
}

fn release_retained_gesture<T: Transport>(
    client: &mut AiUiClient<T>,
    session_id: &str,
    gesture: AutoGestureLock,
) -> Vec<CleanupCall> {
    let token = gesture.token;
    let button = gesture.button;
    vec![
        CleanupCall {
            operation: "pointer_up",
            response: client.call(
                "pointer_up",
                json!({"session_id":session_id,"input_lock_token":token.clone(),"button":button}),
            ),
        },
        CleanupCall {
            operation: "release_input_lock",
            response: client.call(
                "release_input_lock",
                json!({"session_id":session_id,"token":token}),
            ),
        },
    ]
}

fn append_cleanup_reports(
    reports: &mut Vec<StepReport>,
    failures: &mut Vec<Failure>,
    test: &str,
    index: usize,
    context: &str,
    calls: Vec<CleanupCall>,
) -> bool {
    let mut clean = true;
    for cleanup in calls {
        let success = cleanup_response_succeeded(&cleanup);
        reports.push(StepReport {
            index,
            step: format!("{context}:auto-cleanup"),
            operation: cleanup.operation.into(),
            status: if success {
                "PASS_AUTO_CLEANUP".into()
            } else {
                "CLEANUP_FAILURE".into()
            },
            response: cleanup.response.clone(),
        });
        if !success {
            clean = false;
            failures.push(Failure {
                test: test.into(),
                step: format!("{context}:auto-cleanup"),
                operation: cleanup.operation.into(),
                category: "CLEANUP_FAILURE".into(),
                error_code: cleanup
                    .response
                    .get("error")
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or(if cleanup.operation == "release_input_lock" {
                        "INPUT_LOCK_RELEASE_FAILED"
                    } else {
                        "POINTER_CLEANUP_FAILED"
                    })
                    .into(),
                failed_phase: cleanup
                    .response
                    .get("failed_phase")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                action_id: cleanup
                    .response
                    .get("action_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                evidence_ref: cleanup
                    .response
                    .get("error")
                    .and_then(|error| error.get("details"))
                    .and_then(|details| details.get("evidence_path"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                runtime_healthy: cleanup
                    .response
                    .get("application_healthy")
                    .and_then(Value::as_bool),
                suggested_next_action: "inspect session teardown and input lock ownership".into(),
            });
        }
    }
    clean
}

fn cleanup_response_succeeded(cleanup: &CleanupCall) -> bool {
    cleanup.response.get("success").and_then(Value::as_bool) == Some(true)
        && (cleanup.operation != "release_input_lock"
            || cleanup.response.get("result").and_then(Value::as_bool) == Some(true))
}

fn input_lock_failure(test: &str, step: &str, operation: &str, response: &Value) -> Failure {
    Failure {
        test: test.into(),
        step: step.into(),
        operation: operation.into(),
        category: "INFRASTRUCTURE_FAILURE".into(),
        error_code: response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("INPUT_LOCK_FAILURE")
            .into(),
        failed_phase: response
            .get("failed_phase")
            .and_then(Value::as_str)
            .map(str::to_owned),
        action_id: response
            .get("action_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        evidence_ref: None,
        runtime_healthy: response.get("application_healthy").and_then(Value::as_bool),
        suggested_next_action: "resolve physical input lock ownership".into(),
    }
}

fn category_for(code: &str, operation: &str) -> String {
    if code.starts_with("ELEMENT_") {
        "ELEMENT_NOT_FOUND".into()
    } else if code.contains("POSTCONDITION") {
        "POSTCONDITION_FAILED".into()
    } else if code.contains("RUNTIME") {
        "RUNTIME_FAILURE".into()
    } else if operation.contains("relative") || operation.contains("drag") {
        "SPATIAL_FAILURE".into()
    } else if code == "SCRIPT_ERROR" {
        "SCRIPT_ERROR".into()
    } else {
        "ASSERTION_FAILED".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{AiUiClient, InProcessTransport},
        script::{Application, ScriptOptions, Step},
    };
    #[test]
    fn expected_error_is_pass_and_dependency_is_skipped() {
        let transport = InProcessTransport::new(|op: &str, _: Value| {
            if op == "bad" {
                json!({"success":false,"error":{"code":"INVALID_ARGUMENT"}})
            } else {
                json!({"success":true,"result":{"session_id":"s"}})
            }
        });
        let mut client = AiUiClient::new(transport);
        let script = Script {
            version: 1,
            name: "x".into(),
            application: Application::default(),
            options: ScriptOptions {
                stop_on_error: false,
                collect_failures: true,
                evidence_on_failure: false,
                ..Default::default()
            },
            vars: Map::new(),
            setup: vec![],
            steps: vec![
                Step {
                    op: "bad".into(),
                    arguments: Map::new(),
                    save_as: None,
                    save_session_as: None,
                    session: None,
                    depends_on: vec![],
                    expect_error: Some("INVALID_ARGUMENT".into()),
                    step_id: Some("a".into()),
                    redact: false,
                },
                Step {
                    op: "inspect_ui".into(),
                    arguments: Map::new(),
                    save_as: None,
                    save_session_as: None,
                    session: None,
                    depends_on: vec!["a".into()],
                    expect_error: None,
                    step_id: Some("b".into()),
                    redact: false,
                },
            ],
            teardown: vec![],
            cases: vec![],
            tags: vec![],
            generated_by: None,
            generated: false,
            generator_version: None,
            inventory_version: None,
            generated_from: None,
            intent: None,
            regression: None,
            secret_vars: vec![],
        };
        let report = run(&mut client, &script, "target/script-test").unwrap();
        assert_eq!(report.failures.len(), 0);
        assert_eq!(report.summary.skipped, 0);
    }

    #[test]
    fn expected_error_that_succeeds_is_a_failure() {
        let transport = InProcessTransport::new(
            |operation: &str, _: Value| json!({"operation":operation,"success":true,"result":{}}),
        );
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version":1,
            "name":"expected-error-missing",
            "options":{"evidence_on_failure":false},
            "steps":[{
                "op":"unexpected_success",
                "expect_error":"INVALID_ARGUMENT",
                "step_id":"must-fail"
            }]
        }))
        .unwrap();
        let output = std::env::temp_dir().join(format!(
            "ai-ui-expected-error-success-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&output);
        let report = run(&mut client, &script, &output).unwrap();
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.steps[0].status, "FAIL");
        assert_eq!(report.failures[0].error_code, "EXPECTED_ERROR_NOT_OBSERVED");
        assert_eq!(
            report.steps[0].response["expectation_error"]["expected_error"],
            "INVALID_ARGUMENT"
        );
        let _ = std::fs::remove_dir_all(output);
    }

    #[test]
    fn secret_fixture_is_resolved_but_never_persisted() {
        use crate::fixtures::{FixtureStore, FixtureValue};
        use std::collections::BTreeMap;

        let transport = InProcessTransport::new(|op: &str, args: Value| {
            if op == "session.open" {
                json!({
                    "operation":op,
                    "success":true,
                    "result":{"session_id":"secret-session"}
                })
            } else {
                json!({
                    "operation":op,
                    "success":true,
                    "result":{"value":args["value"],"echo":args}
                })
            }
        });
        let mut client = AiUiClient::new(transport);
        let script = Script {
            version: 1,
            name: "secret-redaction".into(),
            application: Application::default(),
            options: ScriptOptions {
                evidence_on_failure: false,
                ..Default::default()
            },
            vars: Map::new(),
            setup: vec![],
            steps: vec![
                Step {
                    op: "session.open".into(),
                    arguments: Map::new(),
                    save_as: None,
                    save_session_as: Some("app".into()),
                    session: None,
                    depends_on: vec![],
                    expect_error: None,
                    step_id: Some("open".into()),
                    redact: false,
                },
                Step {
                    op: "set_value".into(),
                    arguments: Map::from_iter([
                        ("id".into(), json!("login.password")),
                        ("value".into(), json!("${password}")),
                    ]),
                    save_as: None,
                    save_session_as: None,
                    session: Some("app".into()),
                    depends_on: vec!["open".into()],
                    expect_error: None,
                    step_id: Some("password".into()),
                    redact: true,
                },
            ],
            teardown: vec![],
            cases: vec![],
            tags: vec![],
            generated_by: Some("test".into()),
            generated: true,
            generator_version: Some("1".into()),
            inventory_version: Some(1),
            generated_from: Some("inventory.json".into()),
            intent: Some("redaction proof".into()),
            regression: None,
            secret_vars: vec!["password".into()],
        };
        let fixtures = FixtureStore {
            version: 1,
            fixtures: BTreeMap::from_iter([(
                "password".into(),
                FixtureValue {
                    value: json!("not-for-reports-123"),
                    secret: true,
                    description: None,
                },
            )]),
        };
        let output =
            std::env::temp_dir().join(format!("ai-ui-secret-redaction-{}", std::process::id()));
        let report = run_with_fixtures(&mut client, &script, &output, &fixtures).unwrap();
        let serialized = serde_json::to_string(&report).unwrap();
        let disk = fs::read_to_string(output.join("report.json")).unwrap();
        let human = fs::read_to_string(output.join("report.md")).unwrap();
        assert!(!human.contains("not-for-reports-123"));
        assert!(human.contains("# AI UI Test Report"));
        assert!(human.contains("AI_UI_SECRET_REDACTION_PASSED"));
        assert!(!serialized.contains("not-for-reports-123"));
        assert!(!disk.contains("not-for-reports-123"));
        let report_ref = report.report_ref.as_ref().expect("report ref");
        assert_eq!(report_ref.relative_path, "report.json");
        assert!(report_ref.absolute_path.is_some());
        assert!(report.screenshot_ref.is_none());
        assert!(report.evidence_ref.is_none());
        assert!(serialized.contains("[REDACTED]"));
        assert!(
            report
                .markers
                .iter()
                .any(|marker| marker == "AI_UI_SECRET_REDACTION_PASSED")
        );
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn explicit_teardown_close_is_not_repeated_or_reported_as_failed_evidence() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            time::{SystemTime, UNIX_EPOCH},
        };

        let close_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = close_calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, _: Value| match operation {
            "session.open" => json!({
                "operation": operation,
                "success": true,
                "result": {"session_id": "session:explicit-close"}
            }),
            "session.close" => {
                let call = transport_calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    json!({
                        "operation": operation,
                        "success": true,
                        "result": {"closed": true}
                    })
                } else {
                    json!({
                        "operation": operation,
                        "success": false,
                        "error": {
                            "code": "SESSION_NOT_FOUND",
                            "message": "session was already closed",
                            "details": null
                        }
                    })
                }
            }
            other => panic!("unexpected operation: {other}"),
        });
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version": 1,
            "name": "explicit-close-regression",
            "options": {"evidence_on_failure": false},
            "steps": [{
                "op": "session.open",
                "save_session_as": "app"
            }],
            "teardown": [{
                "op": "session.close",
                "session": "app"
            }]
        }))
        .unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "ai-ui-explicit-close-{}-{stamp}",
            std::process::id()
        ));

        let report = run(&mut client, &script, &output).unwrap();

        assert_eq!(report.status, "PASSED");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert!(report.evidence_refs.is_empty());
        let _ = fs::remove_dir_all(output);
    }
    #[test]
    fn pointer_gesture_keeps_auto_lock_until_pointer_up() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
            captured_calls
                .lock()
                .unwrap()
                .push((operation.into(), arguments.clone()));
            let result = match operation {
                "session.open" => json!({"session_id":"gesture-session"}),
                "acquire_input_lock" => json!({"token":"gesture-lock"}),
                "pointer_down" | "pointer_up" => json!({"dispatched":true}),
                "release_input_lock" => json!(true),
                "session.close" => json!({"closed":true}),
                other => panic!("unexpected operation: {other}"),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version": 1,
            "name": "gesture-lock-regression",
            "options": {"evidence_on_failure": false},
            "steps": [
                {"op":"session.open","save_session_as":"app"},
                {"op":"pointer_down","session":"app","button":"left"},
                {"op":"pointer_up","session":"app","button":"left"}
            ],
            "teardown": [{"op":"session.close","session":"app"}]
        }))
        .unwrap();
        let output =
            std::env::temp_dir().join(format!("ai-ui-gesture-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);

        let report = run(&mut client, &script, &output).unwrap();

        assert_eq!(report.status, "PASSED");
        let calls = calls.lock().unwrap();
        let operations: Vec<_> = calls
            .iter()
            .map(|(operation, _)| operation.as_str())
            .collect();
        assert_eq!(
            operations,
            vec![
                "session.open",
                "acquire_input_lock",
                "pointer_down",
                "pointer_up",
                "release_input_lock",
                "session.close"
            ]
        );
        assert_eq!(calls[2].1["input_lock_token"], "gesture-lock");
        assert_eq!(calls[3].1["input_lock_token"], "gesture-lock");
        assert!(report.steps.iter().any(
            |step| step.status == "PASS_AUTO_CLEANUP" && step.operation == "release_input_lock"
        ));
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn final_cleanup_releases_unpaired_pointer_and_every_open_session() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let captured_calls = calls.clone();
        let mut opened = 0usize;
        let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
            captured_calls
                .lock()
                .unwrap()
                .push((operation.into(), arguments.clone()));
            let result = match operation {
                "session.open" => {
                    opened += 1;
                    json!({"session_id":format!("session-{opened}")})
                }
                "acquire_input_lock" => json!({"token":"retained-lock"}),
                "pointer_down" | "pointer_up" => json!({"dispatched":true}),
                "release_input_lock" => json!(true),
                "session.close" => json!({"closed":true}),
                other => panic!("unexpected operation: {other}"),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version": 1,
            "name": "final-cleanup-regression",
            "options": {"evidence_on_failure": false},
            "steps": [
                {"op":"session.open","save_session_as":"first"},
                {"op":"session.open","save_session_as":"second"},
                {"op":"pointer_down","session":"first","button":"right"}
            ]
        }))
        .unwrap();
        let output =
            std::env::temp_dir().join(format!("ai-ui-final-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);

        let report = run(&mut client, &script, &output).unwrap();

        assert_eq!(report.status, "PASSED");
        let calls = calls.lock().unwrap();
        let operations: Vec<_> = calls
            .iter()
            .map(|(operation, _)| operation.as_str())
            .collect();
        assert_eq!(
            operations,
            vec![
                "session.open",
                "session.open",
                "acquire_input_lock",
                "pointer_down",
                "pointer_up",
                "release_input_lock",
                "session.close",
                "session.close"
            ]
        );
        assert_eq!(calls[4].1["button"], "right");
        assert_eq!(calls[4].1["input_lock_token"], "retained-lock");
        assert_eq!(calls[6].1["session_id"], "session-1");
        assert_eq!(calls[7].1["session_id"], "session-2");
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn failed_auto_release_is_a_script_cleanup_failure() {
        let transport = InProcessTransport::new(|operation: &str, _: Value| {
            let result = match operation {
                "session.open" => json!({"session_id":"release-session"}),
                "acquire_input_lock" => json!({"token":"release-lock"}),
                "click" => json!({"dispatched":true}),
                "release_input_lock" => json!(false),
                "session.close" => json!({"closed":true}),
                other => panic!("unexpected operation: {other}"),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version": 1,
            "name": "release-failure-regression",
            "options": {"evidence_on_failure": false},
            "steps": [
                {"op":"session.open","save_session_as":"app"},
                {"op":"click","session":"app","id":"button.create"}
            ]
        }))
        .unwrap();
        let output =
            std::env::temp_dir().join(format!("ai-ui-release-failure-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);

        let report = run(&mut client, &script, &output).unwrap();

        assert_eq!(report.status, "FAILED");
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.category == "CLEANUP_FAILURE"
                    && failure.error_code == "INPUT_LOCK_RELEASE_FAILED")
        );
        assert!(report.steps.iter().any(|step| {
            step.operation == "release_input_lock" && step.status == "CLEANUP_FAILURE"
        }));
        let _ = fs::remove_dir_all(output);
    }
    #[test]
    fn wait_for_outcome_is_executed_locally_and_reads_only_semantic_targets() {
        use std::sync::{Arc, Mutex};

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_calls = calls.clone();
        let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
            captured_calls.lock().unwrap().push(operation.into());
            let result = match operation {
                "session.open" => json!({"session_id":"outcome-session"}),
                "read" => {
                    assert_eq!(arguments["id"], "workflow.status");
                    assert_eq!(arguments["session_id"], "outcome-session");
                    json!({"value":"Последовательный расчёт завершён"})
                }
                "session.close" => json!({"closed":true}),
                other => panic!("wait_for_outcome must not dispatch {other}"),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let script: Script = serde_json::from_value(json!({
            "version":1,
            "name":"wait-for-outcome-local",
            "options":{"evidence_on_failure":false},
            "steps":[
                {"op":"session.open","save_session_as":"app"},
                {
                    "op":"wait_for_outcome",
                    "session":"app",
                    "outcomes":[{
                        "kind":"success",
                        "id":"workflow.status",
                        "path":"$.value",
                        "contains_any":["Последовательный расчёт завершён"]
                    }],
                    "timeout_ms":100,
                    "max_iterations":1
                }
            ],
            "teardown":[{"op":"session.close","session":"app"}]
        }))
        .unwrap();
        let output = std::env::temp_dir().join(format!(
            "ai-ui-wait-for-outcome-local-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&output);

        let report = run(&mut client, &script, &output).unwrap();

        assert_eq!(report.status, "PASSED");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["session.open", "read", "session.close"]
        );
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn observation_markers_require_successful_public_operations() {
        let script: Script = serde_json::from_value(json!({
            "version": 1,
            "name": "observation-marker-proof",
            "tags": ["observation", "async", "e2e"],
            "steps": [
                {"op":"wait_for_task_state","step_id":"task-state"},
                {"op":"assert_task_success","step_id":"task-success"},
                {"op":"wait_for_runtime_milestone","step_id":"milestone"},
                {"op":"get_api_request_state","step_id":"api-state"},
                {"op":"wait_for_api_response","step_id":"api-response"},
                {"op":"assert_api_success","step_id":"api-success"},
                {"op":"wait_for_file","step_id":"file"},
                {"op":"assert_file_exists","step_id":"file-exists"},
                {"op":"wait_for_file_stable","step_id":"file-stable"},
                {"op":"assert_region_changed","step_id":"visual-change"},
                {"op":"assert_not_blank","step_id":"visual-not-blank"},
                {"op":"get_makepad_visual_diagnostics","step_id":"visual-diagnostics"},
                {"op":"get_application_logs","step_id":"logs"},
                {"op":"assert_trace_event","step_id":"trace"},
                {"op":"get_runtime_timeline","step_id":"extract","extract":{"state":"$.state"}},
                {"op":"get_runtime_timeline","step_id":"when","when":{"var":"state","exists":true}},
                {"op":"get_runtime_timeline","step_id":"repeat","repeat_until":{"timeout_ms":1,"max_iterations":1}},
                {"op":"assert_no_pending_task","step_id":"cleanup-task"},
                {"op":"assert_no_stale_window","step_id":"cleanup-window"},
                {"op":"windows_export_evidence","step_id":"evidence"}
            ]
        }))
        .unwrap();
        let mut reports = script
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| StepReport {
                index,
                step: step.step_id.clone().unwrap(),
                operation: step.op.clone(),
                status: "PASS".into(),
                response: json!({"success":true}),
            })
            .collect::<Vec<_>>();
        let mut markers = Vec::new();
        append_observation_markers(&mut markers, &script, &reports, false);
        assert!(
            markers
                .iter()
                .any(|marker| marker == "AI_UI_ASYNC_WORKFLOW_E2E_PASSED")
        );
        assert!(
            markers
                .iter()
                .any(|marker| marker == "AI_UI_VARIABLE_EXTRACTION_PASSED")
        );
        reports
            .iter_mut()
            .find(|report| report.operation == "assert_api_success")
            .unwrap()
            .response = json!({"success":false});
        let mut rejected_markers = Vec::new();
        append_observation_markers(&mut rejected_markers, &script, &reports, false);
        assert!(
            !rejected_markers
                .iter()
                .any(|marker| marker == "AI_UI_EXTERNAL_REQUEST_OBSERVATION_PASSED")
        );
        assert!(
            !rejected_markers
                .iter()
                .any(|marker| marker == "AI_UI_ASYNC_WORKFLOW_E2E_PASSED")
        );
    }

    #[test]
    fn exported_evidence_bundle_is_preferred_over_visual_capture() {
        let root =
            std::env::temp_dir().join(format!("ai-ui-primary-evidence-{}", std::process::id()));
        let bundle = root.join("bundle");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&bundle).unwrap();
        let visual = PortableEvidenceRef {
            relative_path: "capture.png".into(),
            resolved_path: "C:/capture.png".into(),
            absolute_path: Some("C:/capture.png".into()),
            environment_path: None,
            host_artifact_path: None,
        };
        let reports = vec![
            StepReport {
                index: 0,
                step: "visual".into(),
                operation: "get_makepad_visual_diagnostics".into(),
                status: "PASS".into(),
                response: json!({"success":true,"result":{"evidence_ref":visual}}),
            },
            StepReport {
                index: 1,
                step: "export".into(),
                operation: "windows_export_evidence".into(),
                status: "PASS".into(),
                response: json!({"success":true,"result":{"path":bundle.to_string_lossy().to_string()}}),
            },
        ];
        let reference = primary_evidence_ref(&reports, &root).expect("export ref");
        let expected = fs::canonicalize(&bundle)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_ne!(reference.relative_path, "capture.png");
        assert_eq!(reference.relative_path, "bundle");
        assert_eq!(reference.absolute_path.as_deref(), Some(expected.as_str()));
        fs::remove_dir_all(root).unwrap();
    }
}
