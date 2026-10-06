use crate::{
    environment::{EnvironmentHealth, ExecutionEnvironmentKind},
    model::{AttemptRecord, FailureClassification, RuntimeDiagnostics, TestJob},
    scheduler::{AttemptOutcome, LeaseGrant},
};
use ai_ui_test_tool_auto_probe::script;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub trait ExecutionEnvironment: Send + Sync {
    fn kind(&self) -> ExecutionEnvironmentKind {
        ExecutionEnvironmentKind::LocalProcess
    }

    fn health(&self) -> EnvironmentHealth {
        EnvironmentHealth::healthy()
    }

    fn execute(&self, request: &ExecutionRequest, control: &ExecutionControl) -> AttemptOutcome;
}

#[derive(Clone, Debug)]
pub struct ExecutionRequest {
    pub job: TestJob,
    pub attempt: AttemptRecord,
}

impl From<&LeaseGrant> for ExecutionRequest {
    fn from(grant: &LeaseGrant) -> Self {
        Self {
            job: grant.job.clone(),
            attempt: grant.attempt.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExecutionControl {
    cancelled: Arc<AtomicBool>,
}

impl ExecutionControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct LocalProcessEnvironment {
    /// Fixed trusted public runner executable (normally ai-ui-test.exe).
    pub runner_executable: PathBuf,
    pub working_dir: PathBuf,
    pub poll_interval_ms: u64,
}

impl LocalProcessEnvironment {
    pub fn new(runner_executable: impl Into<PathBuf>, working_dir: impl Into<PathBuf>) -> Self {
        Self {
            runner_executable: runner_executable.into(),
            working_dir: working_dir.into(),
            poll_interval_ms: 25,
        }
    }

    fn execute_inner(
        &self,
        request: &ExecutionRequest,
        control: &ExecutionControl,
    ) -> Result<AttemptOutcome, String> {
        if !self.runner_executable.is_file() {
            return Err(format!(
                "public ai-ui-test runner not found: {}",
                self.runner_executable.display()
            ));
        }
        if !self.working_dir.is_dir() {
            return Err(format!(
                "runner working directory not found: {}",
                self.working_dir.display()
            ));
        }
        if !request.job.application.args.is_empty() {
            return Err(
                "LocalProcessEnvironment cannot pass application args until the Public Script API supports them"
                    .into(),
            );
        }

        let attempt_dir = request
            .job
            .artifacts
            .output_dir
            .join(format!("attempt-{:03}", request.attempt.number));
        fs::create_dir_all(&attempt_dir)
            .map_err(|error| format!("create attempt output: {error}"))?;
        let staged_script = stage_trusted_script(&request.job, &attempt_dir)?;

        let stdout_path = attempt_dir.join("runner-stdout.log");
        let stderr_path = attempt_dir.join("runner-stderr.log");
        let stdout =
            File::create(&stdout_path).map_err(|error| format!("create runner stdout: {error}"))?;
        let stderr =
            File::create(&stderr_path).map_err(|error| format!("create runner stderr: {error}"))?;

        let mut command = Command::new(&self.runner_executable);
        command
            .current_dir(&self.working_dir)
            .arg("run")
            .arg(&staged_script)
            .arg("--output")
            .arg(&attempt_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        if let Some(fixtures) = &request.job.fixtures {
            command.arg("--fixtures").arg(fixtures);
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("start public ai-ui-test runner: {error}"))?;
        let runner_pid = child.id();
        fs::write(
            attempt_dir.join("runner-process.json"),
            serde_json::to_vec_pretty(&json!({
                "runner_pid": runner_pid,
                "attempt_id": request.attempt.attempt_id,
                "public_entrypoint": "ai-ui-test run"
            }))
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("write runner ownership metadata: {error}"))?;

        let started = Instant::now();
        let poll = Duration::from_millis(self.poll_interval_ms.max(1));
        loop {
            if control.is_cancelled() {
                terminate_owned_process_tree(&mut child);
                return Ok(outcome_with_diagnostics(
                    FailureClassification::Cancelled,
                    runner_pid,
                    None,
                    stdout_path,
                    stderr_path,
                    "public runner cancelled",
                ));
            }
            if started.elapsed().as_millis() >= u128::from(request.job.timeout_ms) {
                terminate_owned_process_tree(&mut child);
                return Ok(outcome_with_diagnostics(
                    FailureClassification::Timeout,
                    runner_pid,
                    None,
                    stdout_path,
                    stderr_path,
                    "scheduler attempt timeout",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(outcome_from_public_runner(
                        &attempt_dir,
                        runner_pid,
                        status,
                        stdout_path,
                        stderr_path,
                    ));
                }
                Ok(None) => thread::sleep(poll),
                Err(error) => {
                    terminate_owned_process_tree(&mut child);
                    return Err(format!("poll public runner: {error}"));
                }
            }
        }
    }
}

impl ExecutionEnvironment for LocalProcessEnvironment {
    fn execute(&self, request: &ExecutionRequest, control: &ExecutionControl) -> AttemptOutcome {
        self.execute_inner(request, control)
            .unwrap_or_else(|error| AttemptOutcome {
                classification: Some(FailureClassification::InfrastructureFailure),
                script_report: None,
                evidence_refs: Vec::new(),
                runtime: Some(RuntimeDiagnostics {
                    runner_pid: None,
                    exit_code: None,
                    stdout_path: None,
                    stderr_path: None,
                    error_code: None,
                    message: Some(error),
                }),
                execution_environment: None,
                environment_report: None,
            })
    }
}

fn stage_trusted_script(job: &TestJob, attempt_dir: &Path) -> Result<PathBuf, String> {
    let source = job.source.script_path();
    let mut script = script::load(source)
        .map_err(|errors| format!("invalid source script: {}", errors.join("; ")))?;
    // The executable is copied from trusted scheduler configuration. The
    // untrusted scenario cannot inject a shell, runner, or executable argument.
    script.application.command = Some(job.application.executable.to_string_lossy().into_owned());
    let path = attempt_dir.join("resolved-script.aiui.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&script).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write resolved script: {error}"))?;
    Ok(path)
}

fn outcome_from_public_runner(
    attempt_dir: &Path,
    runner_pid: u32,
    status: ExitStatus,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
) -> AttemptOutcome {
    let script_report = find_script_report(attempt_dir);
    let report = script_report
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let classification = match report.as_ref() {
        Some(report)
            if report.get("status").and_then(Value::as_str) == Some("PASSED")
                && status.success() =>
        {
            None
        }
        Some(report) => Some(classify_public_report(report)),
        None => Some(FailureClassification::InfrastructureFailure),
    };
    let evidence_refs = report
        .as_ref()
        .map_or_else(Vec::new, collect_report_evidence);
    let message = if report.is_none() {
        Some("public runner exited without a script report".into())
    } else if !status.success() {
        Some(format!(
            "public runner exited with code {:?}",
            status.code()
        ))
    } else {
        None
    };
    AttemptOutcome {
        classification,
        script_report,
        evidence_refs,
        runtime: Some(RuntimeDiagnostics {
            runner_pid: Some(runner_pid),
            exit_code: status.code(),
            stdout_path: Some(stdout_path),
            stderr_path: Some(stderr_path),
            error_code: None,
            message,
        }),
        execution_environment: None,
        environment_report: None,
    }
}

pub(crate) fn collect_report_evidence(report: &Value) -> Vec<PathBuf> {
    let mut refs = BTreeSet::new();
    if let Some(values) = report.get("evidence_refs").and_then(Value::as_array) {
        refs.extend(values.iter().filter_map(Value::as_str).map(PathBuf::from));
    }
    if let Some(failures) = report.get("failures").and_then(Value::as_array) {
        refs.extend(
            failures
                .iter()
                .filter_map(|failure| failure.get("evidence_ref"))
                .filter_map(Value::as_str)
                .map(PathBuf::from),
        );
    }
    refs.into_iter().collect()
}

pub fn verify_public_api_boundary() -> Result<(), String> {
    let source = include_str!("execution.rs");
    let forbidden = [
        concat!("reliability", "::"),
        concat!("Windows", "Automation"),
        concat!("generator", "::"),
        concat!("windows", "_uia"),
        concat!("makepad", "_widgets"),
    ];
    for forbidden in forbidden {
        if source.contains(forbidden) {
            return Err(format!(
                "execution boundary contains forbidden dependency: {forbidden}"
            ));
        }
    }
    if !source.contains("ai-ui-test run") || !source.contains("script::load") {
        return Err("execution boundary does not use the public script runner contract".into());
    }
    Ok(())
}

pub fn verify_scheduler_environment_boundary() -> Result<(), String> {
    let source = include_str!("scheduler.rs");
    let forbidden = [
        "docker",
        "container_id",
        "sandbox_id",
        "--mount",
        "CreateDesktop",
        "CreateWindowStation",
    ];
    for forbidden in forbidden {
        if source.contains(forbidden) {
            return Err(format!(
                "core Scheduler contains environment-specific mechanism: {forbidden}"
            ));
        }
    }
    Ok(())
}

pub fn classify_public_report(report: &Value) -> FailureClassification {
    let infrastructure_codes = [
        "APP_NOT_FOUND",
        "APP_START_FAILED",
        "WINDOW_NOT_FOUND",
        "WINDOW_AMBIGUOUS",
        "PROCESS_NOT_FOUND",
        "SESSION_NOT_FOUND",
        "INPUT_LOCK_FAILURE",
        "WINDOW_FOCUS_CONFLICT",
        "EVIDENCE_CONSISTENCY_FAILED",
        "SUITE_EXECUTION_FAILED",
    ];
    let infrastructure = report
        .get("failures")
        .and_then(Value::as_array)
        .is_some_and(|failures| {
            failures.iter().any(|failure| {
                let category = failure.get("category").and_then(Value::as_str);
                let code = failure.get("error_code").and_then(Value::as_str);
                category == Some("INFRASTRUCTURE_FAILURE")
                    || code.is_some_and(|code| {
                        infrastructure_codes.contains(&code)
                            || code.starts_with("RUNNER_")
                            || code.starts_with("IO_")
                            || code.starts_with("PROTOCOL_")
                    })
            })
        });
    if infrastructure {
        FailureClassification::InfrastructureFailure
    } else {
        // Assertion/postcondition and application runtime regressions are test
        // failures once the public runner produced a valid script report.
        FailureClassification::TestFailure
    }
}

pub(crate) fn find_script_report(root: &Path) -> Option<PathBuf> {
    let mut reports = Vec::new();
    collect_named(root, "report.json", &mut reports);
    reports.sort();
    reports.into_iter().next()
}

fn collect_named(root: &Path, name: &str, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named(&path, name, output);
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            output.push(path);
        }
    }
}

fn outcome_with_diagnostics(
    classification: FailureClassification,
    runner_pid: u32,
    exit_code: Option<i32>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    message: &str,
) -> AttemptOutcome {
    AttemptOutcome {
        classification: Some(classification),
        script_report: None,
        evidence_refs: Vec::new(),
        runtime: Some(RuntimeDiagnostics {
            runner_pid: Some(runner_pid),
            exit_code,
            stdout_path: Some(stdout_path),
            stderr_path: Some(stderr_path),
            error_code: None,
            message: Some(message.into()),
        }),
        execution_environment: None,
        environment_report: None,
    }
}

#[cfg(windows)]
fn terminate_owned_process_tree(child: &mut Child) {
    let pid = child.id().to_string();
    // PID-scoped tree termination is used only for the runner process owned by
    // this attempt. No process-name or global kill is permitted.
    let _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(windows))]
fn terminate_owned_process_tree(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_classification_separates_assertion_and_infrastructure() {
        let assertion = json!({
            "status": "FAILED",
            "failures": [{
                "category": "ASSERTION_FAILED",
                "error_code": "POSTCONDITION_FAILED"
            }]
        });
        let infrastructure = json!({
            "status": "FAILED",
            "failures": [{
                "category": "ASSERTION_FAILED",
                "error_code": "APP_NOT_FOUND"
            }]
        });
        assert_eq!(
            classify_public_report(&assertion),
            FailureClassification::TestFailure
        );
        assert_eq!(
            classify_public_report(&infrastructure),
            FailureClassification::InfrastructureFailure
        );
    }

    #[test]
    fn worker_execution_source_has_no_low_level_bypass() {
        verify_public_api_boundary().unwrap();
        crate::container::verify_container_public_api_boundary().unwrap();
    }

    #[test]
    fn core_scheduler_has_no_container_or_desktop_mechanism() {
        verify_scheduler_environment_boundary().unwrap();
    }
}
