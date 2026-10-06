use crate::{
    environment::ExecutionEnvironmentRequirement,
    model::{
        ApplicationSpec, ArtifactSpec, ExecutionRequirements, RetryPolicy, TEST_JOB_VERSION,
        TestJob, WorkerCapabilities,
    },
    worker::capabilities_match,
};
use ai_ui_test_tool_auto_probe::{
    catalog,
    script::{self, Script},
    suite,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const APPLICATION_CATALOG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedApplication {
    pub key: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_application_parallelism")]
    pub max_parallel: usize,
    /// Declarative script command spellings accepted for this trusted entry.
    /// These aliases are matched only; they are never executed.
    #[serde(default)]
    pub script_command_aliases: Vec<String>,
}

fn default_application_parallelism() -> usize {
    1
}

impl TrustedApplication {
    pub fn as_spec(&self) -> ApplicationSpec {
        ApplicationSpec {
            key: self.key.clone(),
            executable: self.executable.clone(),
            args: self.args.clone(),
            max_parallel: self.max_parallel,
        }
    }

    fn matches_script_command(&self, command: &str) -> bool {
        path_text_eq(command, &self.executable.to_string_lossy())
            || self
                .script_command_aliases
                .iter()
                .any(|alias| path_text_eq(command, alias))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedApplicationCatalog {
    pub version: u32,
    pub applications: Vec<TrustedApplication>,
}

impl TrustedApplicationCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let catalog: Self = serde_json::from_str(&raw)
            .map_err(|error| format!("invalid trusted application catalog: {error}"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != APPLICATION_CATALOG_VERSION {
            return Err(format!(
                "trusted application catalog version must be {APPLICATION_CATALOG_VERSION}"
            ));
        }
        if self.applications.is_empty() {
            return Err("trusted application catalog must not be empty".into());
        }
        let mut keys = BTreeSet::new();
        for application in &self.applications {
            if application.key.trim().is_empty() {
                return Err("trusted application key must not be empty".into());
            }
            if application.executable.as_os_str().is_empty() {
                return Err(format!(
                    "trusted application {} has an empty executable",
                    application.key
                ));
            }
            if application.max_parallel == 0 {
                return Err(format!(
                    "trusted application {} max_parallel must be greater than zero",
                    application.key
                ));
            }
            if !keys.insert(application.key.to_ascii_lowercase()) {
                return Err(format!(
                    "duplicate trusted application key: {}",
                    application.key
                ));
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        script: &Script,
        explicit_key: Option<&str>,
    ) -> Result<ApplicationSpec, String> {
        let command = script.application.command.as_deref().ok_or_else(|| {
            format!(
                "script {} has no application.command to match against trusted config",
                script.name
            )
        })?;
        let application = if let Some(key) = explicit_key {
            self.applications
                .iter()
                .find(|application| application.key.eq_ignore_ascii_case(key))
                .ok_or_else(|| format!("trusted application key not found: {key}"))?
        } else {
            let matches = self
                .applications
                .iter()
                .filter(|application| application.matches_script_command(command))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [application] => *application,
                [] => {
                    return Err(format!(
                        "script application.command is not trusted: {command}"
                    ));
                }
                _ => {
                    return Err(format!(
                        "script application.command matches multiple trusted entries: {command}"
                    ));
                }
            }
        };
        if !application.matches_script_command(command) {
            return Err(format!(
                "script command {command} does not match trusted application {}",
                application.key
            ));
        }
        Ok(application.as_spec())
    }
}

fn path_text_eq(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .trim_matches('"')
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

#[derive(Clone, Debug)]
pub struct SubmissionOptions {
    pub run_id: String,
    pub priority: i32,
    pub timeout_ms: u64,
    pub retry: RetryPolicy,
    pub output_root: PathBuf,
    pub fixtures: Option<PathBuf>,
    pub application_key: Option<String>,
    pub execution_environment: ExecutionEnvironmentRequirement,
    pub selected_environment_capabilities: Option<WorkerCapabilities>,
    pub metadata: BTreeMap<String, Value>,
}

impl SubmissionOptions {
    pub fn for_run(run_id: impl Into<String>, output_root: impl Into<PathBuf>) -> Self {
        Self {
            run_id: run_id.into(),
            priority: 0,
            timeout_ms: 120_000,
            retry: RetryPolicy::default(),
            output_root: output_root.into(),
            fixtures: None,
            application_key: None,
            execution_environment: ExecutionEnvironmentRequirement::default(),
            selected_environment_capabilities: None,
            metadata: BTreeMap::new(),
        }
    }
}

pub fn submit_source(
    source: impl AsRef<Path>,
    applications: &TrustedApplicationCatalog,
    options: &SubmissionOptions,
    now_ms: u64,
) -> Result<Vec<TestJob>, String> {
    applications.validate()?;
    let script_paths = expand_source(source.as_ref())?;
    let mut jobs = Vec::with_capacity(script_paths.len());
    for (index, path) in script_paths.iter().enumerate() {
        let script = script::load(path).map_err(|errors| {
            format!("invalid script {}: {}", path.display(), errors.join("; "))
        })?;
        let application = applications.resolve(&script, options.application_key.as_deref())?;
        let mut requirements = analyze_requirements(&script)?;
        requirements.execution_environment = options.execution_environment.clone();
        apply_selected_environment(&script, &mut requirements, options)?;
        let retry_safe = script_retry_safe(&script);
        let job_id = format!(
            "job:{}:{index:04}:{}",
            safe_id(&options.run_id),
            safe_id(&script.name)
        );
        let mut metadata = options.metadata.clone();
        metadata.insert("script_name".into(), Value::String(script.name.clone()));
        metadata.insert(
            "source_path".into(),
            Value::String(path.to_string_lossy().into_owned()),
        );
        let job = TestJob {
            version: TEST_JOB_VERSION,
            job_id: job_id.clone(),
            run_id: options.run_id.clone(),
            application,
            source: crate::model::JobSource::Script { path: path.clone() },
            fixtures: options.fixtures.clone(),
            priority: options.priority,
            timeout_ms: options.timeout_ms,
            retry: options.retry.clone(),
            requirements,
            artifacts: ArtifactSpec {
                output_dir: options.output_root.join("jobs").join(safe_id(&job_id)),
            },
            metadata,
            created_at_ms: now_ms,
            retry_safe,
        };
        job.validate()
            .map_err(|errors| format!("invalid generated TestJob: {}", errors.join("; ")))?;
        jobs.push(job);
    }
    Ok(jobs)
}

fn apply_selected_environment(
    script: &Script,
    requirements: &mut ExecutionRequirements,
    options: &SubmissionOptions,
) -> Result<(), String> {
    let Some(capabilities) = &options.selected_environment_capabilities else {
        return Ok(());
    };
    if script_is_environment_probe(script) {
        requirements.platform = capabilities.platform.clone();
        requirements.interactive_desktop = false;
        requirements.gpu = false;
        requirements.semantic_ui = false;
        requirements.physical_input = false;
        requirements.makepad = false;
        requirements.semantic_only = true;
    }
    if capabilities_match(capabilities, requirements) {
        Ok(())
    } else {
        Err(format!(
            "trusted environment {} does not satisfy script {} requirements",
            requirements.execution_environment.environment_key, script.name
        ))
    }
}

fn script_is_environment_probe(script: &Script) -> bool {
    let operations = script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .collect::<Vec<_>>();
    !operations.is_empty()
        && operations
            .iter()
            .all(|step| step.op == "get_runtime_health")
}

pub fn analyze_requirements(script: &Script) -> Result<ExecutionRequirements, String> {
    let mut physical_input = false;
    for step in script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
    {
        let operation = catalog::operation(&step.op)
            .ok_or_else(|| format!("unknown public operation in script: {}", step.op))?;
        physical_input |= operation.physical_input;
    }
    if script.options.semantic_only && physical_input {
        return Err(format!(
            "script {} declares semantic_only but contains physical operations",
            script.name
        ));
    }
    // A job is background-safe only when the script explicitly promises it and
    // the catalog independently confirms that no operation requires physical input.
    let semantic_only = script.options.semantic_only && !physical_input;
    Ok(ExecutionRequirements {
        platform: "windows".into(),
        interactive_desktop: !semantic_only,
        gpu: true,
        semantic_ui: true,
        physical_input,
        makepad: true,
        semantic_only,
        execution_environment: ExecutionEnvironmentRequirement::default(),
    })
}

pub fn script_retry_safe(script: &Script) -> bool {
    script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .all(|step| {
            if matches!(step.op.as_str(), "session.open" | "session.close") {
                return true;
            }
            catalog::operation(&step.op).is_some_and(|operation| {
                operation.idempotency.as_deref() == Some("idempotent")
                    || (!operation.mutation && !operation.physical_input)
            })
        })
}

pub fn expand_source(source: &Path) -> Result<Vec<PathBuf>, String> {
    if !source.exists() {
        return Err(format!(
            "submission source does not exist: {}",
            source.display()
        ));
    }
    let mut candidates = Vec::new();
    collect_json_files(source, &mut candidates)?;
    candidates.sort();

    let mut scripts = BTreeSet::new();
    for path in candidates {
        let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let value: Value = serde_json::from_str(&raw)
            .map_err(|error| format!("invalid JSON {}: {error}", path.display()))?;
        if value.get("tests").is_some() {
            let suite = suite::load(&path)?;
            let root = path.parent().unwrap_or_else(|| Path::new("."));
            for script_path in suite::validate(&suite, root).map_err(|errors| errors.join("; "))? {
                scripts.insert(script_path);
            }
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".aiui.json"))
        {
            scripts.insert(path);
        }
    }
    if scripts.is_empty() {
        return Err("submission source contains no .aiui.json scripts".into());
    }
    Ok(scripts.into_iter().collect())
}

fn collect_json_files(source: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if source.is_file() {
        output.push(source.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_json_files(&path, output)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("json") {
            output.push(path);
        }
    }
    Ok(())
}

fn safe_id(value: &str) -> String {
    let output = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    output.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_ui_test_tool_auto_probe::script;

    fn parsed_script(semantic_only: bool, operation: &str) -> Script {
        script::from_value(serde_json::json!({
            "version": 1,
            "name": "submission proof",
            "application": {
                "command": "fixtures/advanced-demo.exe",
                "window_title": "Advanced Demo"
            },
            "options": {
                "semantic_only": semantic_only
            },
            "steps": [{
                "op": "session.open",
                "save_session_as": "app"
            }, {
                "op": operation,
                "id": "button.create"
            }],
            "teardown": [{
                "op": "session.close",
                "session": "app"
            }]
        }))
        .unwrap()
    }

    #[test]
    fn catalog_analysis_proves_semantic_or_conservatively_leases_desktop() {
        let semantic = parsed_script(true, "assert_visible");
        let requirements = analyze_requirements(&semantic).unwrap();
        assert!(requirements.semantic_only);
        assert!(!requirements.interactive_desktop);
        assert!(!requirements.physical_input);
        assert!(script_retry_safe(&semantic));

        let physical = parsed_script(false, "click");
        let requirements = analyze_requirements(&physical).unwrap();
        assert!(!requirements.semantic_only);
        assert!(requirements.interactive_desktop);
        assert!(requirements.physical_input);
        assert!(!script_retry_safe(&physical));
    }

    #[test]
    fn window_frame_catalog_operations_have_the_expected_desktop_requirement() {
        let script_for = |operation: &str, arguments: serde_json::Value, semantic_only: bool| {
            let mut action = arguments.as_object().cloned().unwrap_or_default();
            action.insert("op".into(), serde_json::Value::String(operation.into()));
            script::from_value(serde_json::json!({
                "version": 1,
                "name": "window frame scheduler proof",
                "application": {
                    "command": "fixtures/advanced-demo.exe",
                    "window_title": "Advanced Demo"
                },
                "options": {
                    "semantic_only": semantic_only
                },
                "steps": [{
                    "op": "session.open",
                    "save_session_as": "app"
                }, serde_json::Value::Object(action)],
                "teardown": [{
                    "op": "session.close",
                    "session": "app"
                }]
            }))
            .unwrap()
        };

        for (operation, arguments) in [
            (
                "windows_drag_title",
                serde_json::json!({"delta_x": 12, "delta_y": 8}),
            ),
            (
                "windows_resize_frame",
                serde_json::json!({"target": "bottom_right", "delta_x": 12, "delta_y": 8}),
            ),
            ("windows_double_click_title", serde_json::json!({})),
        ] {
            let requirements =
                analyze_requirements(&script_for(operation, arguments, false)).unwrap();
            assert!(requirements.interactive_desktop, "{operation}");
            assert!(requirements.physical_input, "{operation}");
            assert!(!requirements.semantic_only, "{operation}");
        }

        for (operation, arguments) in [
            ("inspect_window_interaction", serde_json::json!({})),
            (
                "windows_set_window_bounds",
                serde_json::json!({"width": 900, "height": 640}),
            ),
            (
                "windows_set_window_state",
                serde_json::json!({"state": "normal"}),
            ),
        ] {
            let requirements =
                analyze_requirements(&script_for(operation, arguments, true)).unwrap();
            assert!(!requirements.interactive_desktop, "{operation}");
            assert!(!requirements.physical_input, "{operation}");
        }
    }
    #[test]
    fn contradictory_semantic_only_script_is_rejected() {
        let script = parsed_script(true, "click");
        assert!(analyze_requirements(&script).is_err());
    }

    #[test]
    fn trusted_catalog_matches_alias_but_never_exposes_command_fields() {
        let catalog = TrustedApplicationCatalog {
            version: APPLICATION_CATALOG_VERSION,
            applications: vec![TrustedApplication {
                key: "advanced".into(),
                executable: "C:/apps/advanced-demo.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
                script_command_aliases: vec!["fixtures/advanced-demo.exe".into()],
            }],
        };
        let script = parsed_script(true, "assert_visible");
        let application = catalog.resolve(&script, None).unwrap();
        assert_eq!(application.key, "advanced");
        assert_eq!(
            application.executable,
            PathBuf::from("C:/apps/advanced-demo.exe")
        );
    }
}
