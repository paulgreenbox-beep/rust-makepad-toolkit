//! Small suite orchestrator. Test execution is injected, so suite logic never reaches internals.
use crate::{
    runner::{Failure, ScriptReport},
    script::Script,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Suite {
    pub name: String,
    pub tests: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub max_sessions: Option<usize>,
    #[serde(default)]
    pub max_test_duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SuiteReport {
    pub status: String,
    pub suite: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub failures: Vec<Failure>,
    pub evidence_refs: Vec<String>,
    pub tests: Vec<TestSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestSummary {
    pub name: String,
    pub status: String,
    pub report_path: Option<String>,
}

pub fn load(path: impl AsRef<Path>) -> Result<Suite, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("invalid suite JSON: {e}"))
}

pub fn validate(suite: &Suite, root: impl AsRef<Path>) -> Result<Vec<PathBuf>, Vec<String>> {
    let root = root.as_ref();
    let mut errors = Vec::new();
    let mut paths = Vec::new();
    if suite.name.trim().is_empty() {
        errors.push("suite.name must not be empty".into());
    }
    if suite.tests.is_empty() {
        errors.push("suite.tests must not be empty".into());
    }
    for test in &suite.tests {
        let path = root.join(test);
        if !path.is_file() {
            errors.push(format!("suite test does not exist: {}", path.display()));
        } else {
            paths.push(path);
        }
    }
    if errors.is_empty() {
        Ok(paths)
    } else {
        Err(errors)
    }
}

pub fn aggregate<F>(
    suite: &Suite,
    scripts: &[Script],
    mut execute: F,
) -> Result<SuiteReport, String>
where
    F: FnMut(&Script) -> Result<ScriptReport, String>,
{
    let mut failures = Vec::new();
    let mut evidence_refs = Vec::new();
    let mut tests = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let skipped = 0;
    for script in scripts {
        match execute(script) {
            Ok(report) => {
                if report.status == "PASSED" {
                    passed += 1;
                } else {
                    failed += 1;
                }
                failures.extend(report.failures);
                evidence_refs.extend(report.evidence_refs);
                tests.push(TestSummary {
                    name: script.name.clone(),
                    status: report.status,
                    report_path: None,
                });
            }
            Err(error) => {
                failed += 1;
                failures.push(Failure {
                    test: script.name.clone(),
                    step: "suite".into(),
                    operation: "suite.execute".into(),
                    category: "INFRASTRUCTURE_FAILURE".into(),
                    error_code: "SUITE_EXECUTION_FAILED".into(),
                    failed_phase: None,
                    action_id: None,
                    evidence_ref: None,
                    runtime_healthy: None,
                    suggested_next_action: error,
                });
                tests.push(TestSummary {
                    name: script.name.clone(),
                    status: "FAILED".into(),
                    report_path: None,
                });
            }
        }
    }
    let status = if failed == 0 && skipped == 0 {
        "PASSED"
    } else {
        "FAILED"
    };
    Ok(SuiteReport {
        status: status.into(),
        suite: suite.name.clone(),
        total: scripts.len(),
        passed,
        failed,
        skipped,
        failures,
        evidence_refs,
        tests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runner::{ScriptReport, Summary},
        script::{Application, ScriptOptions},
    };
    use serde_json::Map;
    #[test]
    fn suite_aggregates_independent_failures() {
        let suite = Suite {
            name: "s".into(),
            tests: vec!["a".into()],
            tags: vec![],
            max_sessions: None,
            max_test_duration_ms: None,
        };
        let script = Script {
            version: 1,
            name: "a".into(),
            application: Application::default(),
            options: ScriptOptions::default(),
            vars: Map::new(),
            setup: vec![],
            steps: vec![],
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
        let report = aggregate(&suite, &[script], |s| {
            Ok(ScriptReport {
                status: "PASSED".into(),
                test: s.name.clone(),
                summary: Summary {
                    total: 0,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                },
                evidence_ref: None,
                screenshot_ref: None,
                report_ref: None,
                failures: vec![],
                steps: vec![],
                evidence_refs: vec![],
                markers: vec![],
            })
        })
        .unwrap();
        assert_eq!(report.status, "PASSED");
        assert_eq!(report.passed, 1);
    }
}
