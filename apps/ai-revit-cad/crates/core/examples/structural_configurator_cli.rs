use ai_revit_core::{
    Command, ConfiguratorAnalysisReadiness, ConfiguratorAnalysisRequest, Document,
    EngineeringSnapshotStatus, EngineeringSourceIdentity, FoundationAssignmentGroup,
    STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION, StructuralConfiguratorCommand,
    analyze_configured_building, configured_analysis_readiness,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use structural_configurator::{
    BuildingConfig, ConfiguratorError, FixtureKind, IssueSeverity, RoofHeightUpdate, SemanticId,
    connected_analysis_scope, dense_id_map, diff_generated, fixture, generate_building,
    set_section_eave, to_structural_model_for_scope, validate_config,
};

const MAX_REQUEST_BYTES: usize = 4 << 20;
const HASH_BUFFER_BYTES: usize = 64 << 10;
const MAX_DOCUMENT_SESSION_ACTIONS: usize = 8;

type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
struct CliError {
    code: String,
    message: String,
    details: Option<Value>,
}

impl CliError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    fn from_configurator(error: ConfiguratorError) -> Self {
        let message = error.to_string();
        match serde_json::to_value(&error) {
            Ok(details) => match details.get("code").and_then(Value::as_str) {
                Some(code) => Self {
                    code: code.to_owned(),
                    message,
                    details: Some(details),
                },
                None => Self {
                    code: "configurator_error_code_missing".into(),
                    message,
                    details: Some(details),
                },
            },
            Err(serialization_error) => Self {
                code: "configurator_error_serialization_error".into(),
                message: format!(
                    "{message}; typed error serialization failed: {serialization_error}"
                ),
                details: None,
            },
        }
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::new("io_error", error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("invalid_request", error.to_string())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct SourceIdentity {
    source_manifest_sha256: Option<String>,
    executable_sha256: String,
    package_version: String,
    source_label: String,
}

impl SourceIdentity {
    fn engineering_producer(&self) -> CliResult<EngineeringSourceIdentity> {
        let source_manifest_sha256 = self.source_manifest_sha256.clone().ok_or_else(|| {
            CliError::new(
                "source_manifest_binding_required",
                "analysis requires a valid compile-time AI_REVIT_SC_COMPILED_SOURCE_MANIFEST_SHA256",
            )
        })?;
        Ok(EngineeringSourceIdentity {
            source_manifest_sha256,
            executable_sha256: Some(self.executable_sha256.clone()),
            solver_version: self.package_version.clone(),
            source_label: "CLI executable bound to its compiled source manifest; identity only; no engineering approval".into(),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Generate {
        config: BuildingConfig,
        output_path: Option<String>,
    },
    Validate {
        config: BuildingConfig,
        output_path: Option<String>,
    },
    Fixture {
        kind: FixtureKind,
        output_path: Option<String>,
    },
    ProjectModel {
        config: BuildingConfig,
        scope_seed: Option<String>,
        output_path: Option<String>,
    },
    CreateFixtureDocument {
        kind: FixtureKind,
        path: String,
        name: Option<String>,
        #[serde(default)]
        foundation_groups: Option<Vec<FoundationAssignmentGroup>>,
    },
    Document {
        path: String,
        action: DocumentAction,
        output_path: Option<String>,
        section_id: Option<String>,
        eave_abs_m: Option<f64>,
        height_update: Option<RoofHeightUpdate>,
        analysis_request: Option<ConfiguratorAnalysisRequest>,
    },
    DocumentSession {
        path: String,
        actions: Vec<DocumentSessionAction>,
        output_path: Option<String>,
    },
}

/// A bounded ordinary editing session on one opened Document. Analysis and
/// archived snapshot writes are deliberately outside this request contract.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum DocumentSessionAction {
    Regenerate,
    EditSectionHeight {
        section_id: String,
        eave_abs_m: f64,
        height_update: Option<RoofHeightUpdate>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DocumentAction {
    Inspect,
    Regenerate,
    EditSectionHeight,
    Analyze,
    Undo,
    Redo,
    Save,
}

impl DocumentAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Regenerate => "regenerate",
            Self::EditSectionHeight => "edit_section_height",
            Self::Analyze => "analyze",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Save => "save",
        }
    }
}

fn main() {
    // Native history validation traverses nested, bounded document DTOs. Windows
    // starts console mains with a smaller stack than Rust's test threads. Use one
    // explicitly bounded worker; this does not alter document or solver limits.
    let worker = std::thread::Builder::new()
        .name("structural-configurator-cli".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(run_request);
    let (response, failed) = match worker {
        Ok(worker) => match worker.join() {
            Ok(response) => response,
            Err(_) => (
                error_response(
                    CliError::new("worker_panicked", "bounded CLI worker panicked"),
                    None,
                ),
                true,
            ),
        },
        Err(error) => (
            error_response(
                CliError::new("worker_start_failed", error.to_string()),
                None,
            ),
            true,
        ),
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let _ = serde_json::to_writer(&mut output, &response);
    let _ = writeln!(output);
    if failed {
        std::process::exit(1);
    }
}

fn run_request() -> (Value, bool) {
    let identity = runtime_identity();
    match identity {
        Ok(identity) => match execute(&identity) {
            Ok(response) => (response, false),
            Err(error) => (error_response(error, Some(&identity)), true),
        },
        Err(error) => (error_response(error, None), true),
    }
}

fn execute(identity: &SourceIdentity) -> CliResult<Value> {
    let mut args = env::args_os();
    let _program = args.next();
    let flag = args
        .next()
        .ok_or_else(|| CliError::new("usage", "expected --request-file <path>"))?;
    if flag.as_os_str() != OsStr::new("--request-file") {
        return Err(CliError::new("usage", "expected --request-file <path>"));
    }
    let request_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::new("usage", "missing request file path"))?;
    if args.next().is_some() {
        return Err(CliError::new(
            "usage",
            "expected exactly --request-file <path>",
        ));
    }

    let mut request_file = File::open(request_path)?;
    let mut request_bytes = Vec::with_capacity(64 * 1024);
    Read::by_ref(&mut request_file)
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut request_bytes)?;
    if request_bytes.len() > MAX_REQUEST_BYTES {
        return Err(CliError::new(
            "request_too_large",
            "request file exceeds the 4 MiB input limit",
        ));
    }
    let request: Request = serde_json::from_slice(&request_bytes)?;
    process_request(request, identity)
}

fn process_request(request: Request, identity: &SourceIdentity) -> CliResult<Value> {
    match request {
        Request::Generate {
            config,
            output_path,
        } => {
            let generated = generate_building(&config).map_err(CliError::from_configurator)?;
            let ids = dense_id_map(&generated).map_err(CliError::from_configurator)?;
            let result = json!({
                "building": config,
                "generated_building": generated,
                "dense_id_map": ids,
            });
            result_response("generate", result, output_path.as_deref(), identity)
        }
        Request::Validate {
            config,
            output_path,
        } => {
            let issues = validate_config(&config);
            let error_count = issues
                .iter()
                .filter(|issue| matches!(issue.severity, IssueSeverity::Error))
                .count();
            let generation_limit_count = issues
                .iter()
                .filter(|issue| matches!(issue.severity, IssueSeverity::GenerationLimit))
                .count();
            let warning_count = issues
                .iter()
                .filter(|issue| matches!(issue.severity, IssueSeverity::Warning))
                .count();
            let result = json!({
                "issue_counts": {
                    "error": error_count,
                    "generation_limit": generation_limit_count,
                    "warning": warning_count,
                },
                "issues": issues,
            });
            result_response("validate", result, output_path.as_deref(), identity)
        }
        Request::Fixture { kind, output_path } => {
            let config = fixture(kind);
            let generated = generate_building(&config).map_err(CliError::from_configurator)?;
            let ids = dense_id_map(&generated).map_err(CliError::from_configurator)?;
            let result = json!({
                "fixture_kind": kind,
                "building": config,
                "generated_building": generated,
                "dense_id_map": ids,
            });
            result_response("fixture", result, output_path.as_deref(), identity)
        }
        Request::ProjectModel {
            config,
            scope_seed,
            output_path,
        } => {
            let result = project_model(config, scope_seed)?;
            result_response("project_model", result, output_path.as_deref(), identity)
        }
        Request::CreateFixtureDocument {
            kind,
            path,
            name,
            foundation_groups,
        } => create_fixture_document(kind, &path, name, foundation_groups, identity),
        Request::Document {
            path,
            action,
            output_path,
            section_id,
            eave_abs_m,
            height_update,
            analysis_request,
        } => document_operation(
            &path,
            action,
            output_path.as_deref(),
            section_id.as_deref(),
            eave_abs_m,
            height_update,
            analysis_request,
            identity,
        ),
        Request::DocumentSession {
            path,
            actions,
            output_path,
        } => document_session(&path, actions, output_path.as_deref(), identity),
    }
}

fn project_model(config: BuildingConfig, scope_seed: Option<String>) -> CliResult<Value> {
    let generated = generate_building(&config).map_err(CliError::from_configurator)?;
    let dense_ids = dense_id_map(&generated).map_err(CliError::from_configurator)?;
    let seed_text = scope_seed
        .or_else(|| {
            config
                .sections
                .first()
                .map(|section| section.id.as_str().to_owned())
        })
        .ok_or_else(|| {
            CliError::new(
                "missing_scope_seed",
                "building has no section to seed a connected model projection",
            )
        })?;
    let seed = SemanticId::new(seed_text)
        .map_err(|message| CliError::new("invalid_semantic_id", message))?;
    let scope =
        connected_analysis_scope(&generated, seed.as_str()).map_err(CliError::from_configurator)?;
    let structural_model =
        to_structural_model_for_scope(&generated, &scope).map_err(CliError::from_configurator)?;
    let counts = json!({
        "nodes": structural_model.nodes.len(),
        "members": structural_model.members.len(),
        "materials": structural_model.materials.len(),
        "sections": structural_model.sections.len(),
        "supports": structural_model.supports.len(),
        "load_cases": structural_model.load_cases.len(),
        "combinations": structural_model.combinations.len(),
    });
    Ok(json!({
        "building": config,
        "generated_building": generated,
        "dense_id_map": dense_ids,
        "connected_scope": scope,
        "structural_model": structural_model,
        "projected_counts": counts,
        "structural_core_limits": {
            "nodes": structural_core::MAX_NODES,
            "members": structural_core::MAX_MEMBERS,
            "load_cases": structural_core::MAX_LOAD_CASES,
            "combinations": structural_core::MAX_COMBINATIONS,
        },
    }))
}

fn result_response(
    operation: &str,
    mut result: Value,
    output_path: Option<&str>,
    identity: &SourceIdentity,
) -> CliResult<Value> {
    if let Some(output_path) = output_path {
        let artifact_path = checked_output_path(output_path, None)?;
        let artifact = json!({ "operation": operation, "result": result });
        let bytes = json_bytes(&artifact)?;
        write_new_file(&artifact_path, &bytes)?;
        result["saved_output"] = json!({
            "path": artifact_path.to_string_lossy(),
            "sha256": lower_hex(&Sha256::digest(&bytes)),
        });
    }
    success_response(operation, None, identity, result)
}

fn create_fixture_document(
    kind: FixtureKind,
    path: &str,
    name: Option<String>,
    foundation_groups: Option<Vec<FoundationAssignmentGroup>>,
    identity: &SourceIdentity,
) -> CliResult<Value> {
    let output_path = checked_output_path(path, None)?;
    let (building, default_groups) =
        ai_revit_core::configured_fixture_input(kind).map_err(core_error)?;
    let foundation_groups = match foundation_groups {
        Some(groups) => groups,
        None => default_groups,
    };
    let mut document =
        Document::new(name.unwrap_or_else(|| format!("Structural configurator {kind:?} fixture")))
            .map_err(core_error)?;
    let update = StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
        building,
        foundation_groups: foundation_groups.clone(),
    };
    let transaction = document
        .begin_transaction(command_id("create_fixture"), document.revision())
        .with(Command::StructuralConfigurator(update));
    let receipt = document.commit(transaction).map_err(core_error)?;
    let saved_sha256 = save_document_new(&document, &output_path)?;
    success_response(
        "create_fixture_document",
        Some(&document),
        identity,
        json!({
            "fixture_kind": kind,
            "foundation_group_count": foundation_groups.len(),
            "commit": receipt,
            "saved_to": output_path.to_string_lossy(),
            "saved_sha256": saved_sha256,
        }),
    )
}

fn document_operation(
    path: &str,
    action: DocumentAction,
    output_path: Option<&str>,
    section_id: Option<&str>,
    eave_abs_m: Option<f64>,
    height_update: Option<RoofHeightUpdate>,
    analysis_request: Option<ConfiguratorAnalysisRequest>,
    identity: &SourceIdentity,
) -> CliResult<Value> {
    let source_path = fs::canonicalize(path)?;
    let mut document = Document::open(&source_path).map_err(core_error)?;
    let mut result = json!({ "action": action.as_str() });

    match action {
        DocumentAction::Inspect | DocumentAction::Save => {}
        DocumentAction::Regenerate => {
            result = regenerate_document(&mut document)?;
        }
        DocumentAction::EditSectionHeight => {
            result =
                edit_document_section_height(&mut document, section_id, eave_abs_m, height_update)?;
        }
        DocumentAction::Analyze => {
            let request = analysis_request.ok_or_else(|| {
                CliError::new(
                    "missing_analysis_request",
                    "analyze requires analysis_request",
                )
            })?;
            let current = document
                .structural_configurator()
                .current()
                .cloned()
                .ok_or_else(|| {
                    CliError::new(
                        "missing_building",
                        "document has no current configured building",
                    )
                })?;
            let readiness =
                configured_analysis_readiness(&current, &request).map_err(core_error)?;
            let readiness_json = serde_json::to_value(&readiness)
                .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
            if matches!(&readiness, ConfiguratorAnalysisReadiness::Ready { .. }) {
                let producer = identity.engineering_producer()?;
                let snapshot =
                    analyze_configured_building(&current, request, producer).map_err(core_error)?;
                let snapshot_json = serde_json::to_value(&snapshot)
                    .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
                let transaction = document
                    .begin_transaction(command_id("analyze"), document.revision())
                    .with(Command::StructuralConfigurator(
                        StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot },
                    ));
                let receipt = document.commit(transaction).map_err(core_error)?;
                result["analysis_outcome"] = json!("snapshot_saved");
                result["snapshot"] = snapshot_json;
                result["commit"] = serde_json::to_value(receipt)
                    .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
            } else {
                result["analysis_outcome"] = json!("not_run_readiness_hold");
                result["snapshot"] = Value::Null;
                result["commit"] = Value::Null;
            }
            result["readiness"] = readiness_json;
        }
        DocumentAction::Undo => {
            let receipt = document.undo().map_err(core_error)?;
            result["history_receipt"] = serde_json::to_value(receipt)
                .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
        }
        DocumentAction::Redo => {
            let receipt = document.redo().map_err(core_error)?;
            result["history_receipt"] = serde_json::to_value(receipt)
                .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
        }
    }

    if let Some(output_path) = output_path {
        let destination = checked_output_path(output_path, Some(&source_path))?;
        let saved_sha256 = save_document_new(&document, &destination)?;
        result["saved_to"] = json!(destination.to_string_lossy());
        result["saved_sha256"] = json!(saved_sha256);
    } else {
        if matches!(action, DocumentAction::Save) {
            return Err(CliError::new(
                "output_path_required",
                "document save requires an explicit output_path",
            ));
        }
        result["saved_to"] = Value::Null;
    }

    success_response("document", Some(&document), identity, result)
}

fn document_session(
    path: &str,
    actions: Vec<DocumentSessionAction>,
    output_path: Option<&str>,
    identity: &SourceIdentity,
) -> CliResult<Value> {
    if actions.is_empty() || actions.len() > MAX_DOCUMENT_SESSION_ACTIONS {
        return Err(CliError::new(
            "invalid_session_action_count",
            "document_session requires between 1 and 8 actions",
        ));
    }
    let source_path = fs::canonicalize(path)?;
    let destination = output_path
        .map(|path| checked_output_path(path, Some(&source_path)))
        .transpose()?;
    let mut document = Document::open(&source_path).map_err(core_error)?;
    let mut steps = Vec::with_capacity(actions.len());
    for (index, action) in actions.into_iter().enumerate() {
        let result = match action {
            DocumentSessionAction::Regenerate => regenerate_document(&mut document)?,
            DocumentSessionAction::EditSectionHeight {
                section_id,
                eave_abs_m,
                height_update,
            } => edit_document_section_height(
                &mut document,
                Some(&section_id),
                Some(eave_abs_m),
                height_update,
            )?,
        };
        let current = document
            .structural_configurator()
            .current()
            .ok_or_else(|| {
                CliError::new(
                    "missing_building",
                    "session update produced no configured building",
                )
            })?;
        steps.push(json!({
            "step": index + 1,
            "action": result["action"],
            "revision": document.revision(),
            "input_fingerprint": current.input_fingerprint,
            "source_revision": current.source_revision,
            "runtime_generation_diagnostics": runtime_generation_diagnostics(&document),
        }));
    }
    let mut result = json!({
        "action_count": steps.len(),
        "steps": steps,
        "saved_to": Value::Null,
    });
    if let Some(destination) = destination {
        let saved_sha256 = save_document_new(&document, &destination)?;
        result["saved_to"] = json!(destination.to_string_lossy());
        result["saved_sha256"] = json!(saved_sha256);
    }
    success_response("document_session", Some(&document), identity, result)
}

fn regenerate_document(document: &mut Document) -> CliResult<Value> {
    let current = document
        .structural_configurator()
        .current()
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                "missing_building",
                "document has no current configured building",
            )
        })?;
    let groups = current.foundation_groups.values().cloned().collect();
    let update = commit_building_update(
        document,
        current.building.clone(),
        groups,
        Some(current.generated),
        "regenerate",
    )?;
    Ok(json!({ "action": "regenerate", "update": update }))
}

fn edit_document_section_height(
    document: &mut Document,
    section_id: Option<&str>,
    eave_abs_m: Option<f64>,
    height_update: Option<RoofHeightUpdate>,
) -> CliResult<Value> {
    let section_id = section_id.ok_or_else(|| {
        CliError::new(
            "missing_section_id",
            "edit_section_height requires section_id",
        )
    })?;
    let semantic_id = SemanticId::new(section_id.to_owned())
        .map_err(|message| CliError::new("invalid_semantic_id", message))?;
    let eave_abs_m = eave_abs_m.ok_or_else(|| {
        CliError::new(
            "missing_eave_height",
            "edit_section_height requires eave_abs_m",
        )
    })?;
    if !eave_abs_m.is_finite() {
        return Err(CliError::new(
            "invalid_eave_height",
            "eave_abs_m must be finite",
        ));
    }
    let height_update = height_update.unwrap_or(RoofHeightUpdate::PreserveRise);
    let current = document
        .structural_configurator()
        .current()
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                "missing_building",
                "document has no current configured building",
            )
        })?;
    let mut building = current.building.clone();
    set_section_eave(
        &mut building,
        semantic_id.as_str(),
        eave_abs_m,
        height_update,
    )
    .map_err(CliError::from_configurator)?;
    let groups = current.foundation_groups.values().cloned().collect();
    let update = commit_building_update(
        document,
        building,
        groups,
        Some(current.generated),
        "edit_section_height",
    )?;
    Ok(json!({
        "action": "edit_section_height",
        "section_id": semantic_id,
        "eave_abs_m": eave_abs_m,
        "height_update": height_update,
        "update": update,
    }))
}

fn runtime_generation_diagnostics(document: &Document) -> Value {
    json!({
        "authoritative": false,
        "scope": "last_successful_explicit_generation_in_this_in_memory_document",
        "last_successful_explicit_generation": document.configurator_generation_diagnostics(),
    })
}

fn commit_building_update(
    document: &mut Document,
    building: BuildingConfig,
    foundation_groups: Vec<FoundationAssignmentGroup>,
    previous_generated: Option<structural_configurator::GeneratedBuilding>,
    action: &str,
) -> CliResult<Value> {
    let command = StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
        building,
        foundation_groups,
    };
    let transaction = document
        .begin_transaction(command_id(action), document.revision())
        .with(Command::StructuralConfigurator(command));
    let receipt = document.commit(transaction).map_err(core_error)?;
    let generation_delta = match (
        previous_generated,
        document.structural_configurator().current(),
    ) {
        (Some(before), Some(after)) => {
            serde_json::to_value(diff_generated(&before, &after.generated))
                .map_err(|error| CliError::new("serialization_error", error.to_string()))?
        }
        _ => Value::Null,
    };
    let current = document
        .structural_configurator()
        .current()
        .ok_or_else(|| {
            CliError::new(
                "missing_building",
                "Core update produced no current configured building",
            )
        })?;
    Ok(json!({
        "commit": receipt,
        "generation_delta": generation_delta,
        "input_fingerprint": current.input_fingerprint,
        "source_revision": current.source_revision,
    }))
}

fn success_response(
    operation: &str,
    document: Option<&Document>,
    identity: &SourceIdentity,
    result: Value,
) -> CliResult<Value> {
    let metadata = document.map(document_metadata).transpose()?;
    Ok(json!({
        "ok": true,
        "operation": operation,
        "document": metadata,
        "pid": std::process::id(),
        "source_identity": identity,
        "result": result,
    }))
}

fn document_metadata(document: &Document) -> CliResult<Value> {
    let serialized_document = serde_json::to_value(document)
        .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
    let format_id = serialized_document.get("format_id").and_then(Value::as_str);
    let format_version = serialized_document
        .get("format_version")
        .and_then(Value::as_u64);
    let domain = document.structural_configurator();
    let serialized_domain = serde_json::to_value(domain)
        .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
    let snapshot_statuses: Vec<Value> = domain
        .snapshots()
        .map(|(id, snapshot)| {
            let status = match domain.snapshot_status(id) {
                Some(EngineeringSnapshotStatus::Current) => "current",
                Some(EngineeringSnapshotStatus::Stale) => "stale",
                None => "unavailable",
            };
            json!({
                "snapshot_id": id.get(),
                "status": status,
                "input_fingerprint": snapshot.source.input_fingerprint,
                "content_fingerprint": snapshot.content_fingerprint,
                "producer": snapshot.structural.producer,
            })
        })
        .collect();
    let current = match domain.current() {
        Some(state) => {
            let dense_ids = dense_id_map(&state.generated).map_err(CliError::from_configurator)?;
            json!({
                "building": state.building,
                "generated": state.generated,
                "foundation_groups": state.foundation_groups,
                "source_revision": state.source_revision,
                "input_fingerprint": state.input_fingerprint,
                "dense_id_map": dense_ids,
                "semantic_ids": {
                    "nodes": state.generated.nodes.keys().collect::<Vec<_>>(),
                    "members": state.generated.members.keys().collect::<Vec<_>>(),
                    "surfaces": state.generated.surfaces.keys().collect::<Vec<_>>(),
                    "sections": state.generated.outlines.keys().collect::<Vec<_>>(),
                },
                "counts": {
                    "nodes": state.generated.nodes.len(),
                    "members": state.generated.members.len(),
                    "surfaces": state.generated.surfaces.len(),
                    "sections": state.generated.outlines.len(),
                    "max_structural_nodes": structural_core::MAX_NODES,
                    "max_structural_members": structural_core::MAX_MEMBERS,
                },
            })
        }
        None => Value::Null,
    };
    Ok(json!({
        "document_uuid": document.document_uuid.to_string(),
        "revision": document.revision(),
        "native_format": {
            "id": format_id,
            "version": format_version,
            "configurator_version": STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION,
        },
        "current": current,
        "runtime_generation_diagnostics": runtime_generation_diagnostics(document),
        "snapshot_statuses": snapshot_statuses,
        "serialized_domain": serialized_domain,
    }))
}

fn checked_output_path(path: &str, source_path: Option<&Path>) -> CliResult<PathBuf> {
    if path.trim().is_empty() {
        return Err(CliError::new("invalid_output_path", "output path is empty"));
    }
    let root = env::var_os("AI_REVIT_SC_OUTPUT_ROOT").ok_or_else(|| {
        CliError::new(
            "output_root_required",
            "AI_REVIT_SC_OUTPUT_ROOT must name the allowed output directory",
        )
    })?;
    let root = fs::canonicalize(PathBuf::from(root))?;
    if !root.is_dir() {
        return Err(CliError::new(
            "invalid_output_root",
            "AI_REVIT_SC_OUTPUT_ROOT is not a directory",
        ));
    }

    let requested = PathBuf::from(path);
    let file_name = requested
        .file_name()
        .ok_or_else(|| CliError::new("invalid_output_path", "output path has no file name"))?;
    let requested_parent = requested
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(requested_parent)?;
    if !canonical_parent.starts_with(&root) {
        return Err(CliError::new(
            "output_path_escape",
            "output parent is outside AI_REVIT_SC_OUTPUT_ROOT",
        ));
    }
    let destination = canonical_parent.join(file_name);
    if let Some(source_path) = source_path {
        let source_path = fs::canonicalize(source_path)?;
        if destination == source_path
            || fs::canonicalize(&destination).is_ok_and(|existing| existing == source_path)
        {
            return Err(CliError::new(
                "source_overwrite_rejected",
                "output_path must not overwrite the opened source document",
            ));
        }
    }
    Ok(destination)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> CliResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                CliError::new(
                    "output_exists",
                    "output_path already exists; overwrites are rejected",
                )
            } else {
                CliError::from(error)
            }
        })?;
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(CliError::from(error));
    }
    Ok(())
}

fn save_document_new(document: &Document, destination: &Path) -> CliResult<String> {
    let parent = destination
        .parent()
        .ok_or_else(|| CliError::new("invalid_output_path", "document output has no parent"))?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| CliError::new("invalid_output_path", "document output has no file name"))?
        .to_string_lossy();
    let mut staging_directory = None;
    for attempt in 0..128_u32 {
        let candidate = parent.join(format!(
            ".{file_name}.staging-{}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                staging_directory = Some(candidate);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CliError::from(error)),
        }
    }
    let staging_directory = staging_directory.ok_or_else(|| {
        CliError::new(
            "staging_path_unavailable",
            "could not reserve a unique document staging directory",
        )
    })?;
    let staging_file = staging_directory.join("document.native.json");
    let saved = document.save(&staging_file).map_err(core_error);
    if let Err(error) = saved {
        let _ = fs::remove_dir_all(&staging_directory);
        return Err(error);
    }
    let copied = copy_new_file(&staging_file, destination);
    let _ = fs::remove_dir_all(&staging_directory);
    copied
}

fn copy_new_file(source_path: &Path, destination: &Path) -> CliResult<String> {
    let mut source = File::open(source_path)?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                CliError::new(
                    "output_exists",
                    "output_path already exists; overwrites are rejected",
                )
            } else {
                CliError::from(error)
            }
        })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let copy_result = (|| -> io::Result<()> {
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            destination_file.write_all(&buffer[..count])?;
            hasher.update(&buffer[..count]);
        }
        destination_file.flush()?;
        destination_file.sync_all()
    })();
    drop(destination_file);
    if let Err(error) = copy_result {
        let _ = fs::remove_file(destination);
        return Err(CliError::from(error));
    }
    Ok(lower_hex(&hasher.finalize()))
}

fn json_bytes(value: &Value) -> CliResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| CliError::new("serialization_error", error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn runtime_identity() -> CliResult<SourceIdentity> {
    let executable_path = env::current_exe()?;
    let mut executable = File::open(executable_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    loop {
        let count = executable.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let executable_sha256 = lower_hex(&hasher.finalize());
    let source_manifest_sha256 = option_env!("AI_REVIT_SC_COMPILED_SOURCE_MANIFEST_SHA256")
        .filter(|value| is_sha256(value) && value.bytes().any(|byte| byte != b'0'))
        .map(|value| value.to_owned());
    let source_label = if source_manifest_sha256.is_some() {
        "compiled source-manifest binding; identity only; no engineering acceptance"
    } else {
        "no valid compile-time source-manifest binding; identity only; no engineering acceptance"
    };
    Ok(SourceIdentity {
        source_manifest_sha256,
        executable_sha256,
        package_version: env!("CARGO_PKG_VERSION").to_owned(),
        source_label: source_label.into(),
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn command_id(action: &str) -> String {
    format!(
        "structural-configurator-cli-{action}-{}",
        uuid::Uuid::new_v4()
    )
}

fn core_error(error: impl std::fmt::Display) -> CliError {
    CliError::new("core_error", error.to_string())
}

fn error_response(error: CliError, identity: Option<&SourceIdentity>) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": error.code,
            "message": error.message,
            "details": error.details,
        },
        "pid": std::process::id(),
        "source_identity": identity,
    })
}
