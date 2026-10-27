use ai_revit_core::{
    Command, Document, EngineeringSourceIdentity, FoundationLoadSource,
    FoundationStructuralCommand, FoundationStructuralInput, NATIVE_FORMAT_ID, Transaction,
    analyze_foundation_structural,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_REQUEST_BYTES: usize = 4 << 20;
const HASH_BUFFER_BYTES: usize = 64 << 10;

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

    fn from_kernel_error<E>(error: E) -> Self
    where
        E: Serialize + std::fmt::Display,
    {
        let message = error.to_string();
        match serde_json::to_value(&error) {
            Ok(details) => match details.get("code").and_then(Value::as_str) {
                Some(code) => Self {
                    code: code.to_owned(),
                    message,
                    details: Some(details),
                },
                None => Self {
                    code: "kernel_error_code_missing".into(),
                    message,
                    details: Some(details),
                },
            },
            Err(serialization_error) => Self {
                code: "kernel_error_serialization_error".into(),
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
struct CliSourceIdentity {
    source_manifest_sha256: Option<String>,
    executable_sha256: String,
    solver_version: String,
    source_label: String,
}

impl CliSourceIdentity {
    fn engineering_producer(&self) -> CliResult<EngineeringSourceIdentity> {
        let source_manifest_sha256 = self.source_manifest_sha256.clone().ok_or_else(|| {
            CliError::new(
                "source_manifest_binding_required",
                "analysis requires a valid compile-time AI_REVIT_FS_COMPILED_SOURCE_MANIFEST_SHA256 binding",
            )
        })?;
        Ok(EngineeringSourceIdentity {
            source_manifest_sha256,
            executable_sha256: Some(self.executable_sha256.clone()),
            solver_version: self.solver_version.clone(),
            source_label: "compiled source-manifest binding; not engineering approval".into(),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Foundation {
        input: foundation_core::RigidMatInput,
    },
    Structural {
        model: structural_core::StructuralModel,
        target: structural_core::AnalysisTarget,
    },
    CreateDemo {
        path: String,
        name: Option<String>,
    },
    Document {
        path: String,
        action: DocumentAction,
        output_path: Option<String>,
        area_m2: Option<f64>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DocumentAction {
    Inspect,
    Analyze,
    EditSection,
    Undo,
    Redo,
}

impl DocumentAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Analyze => "analyze",
            Self::EditSection => "edit_section",
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }
}

fn main() {
    let identity = runtime_identity();
    let (response, failed) = match identity {
        Ok(identity) => match execute(&identity) {
            Ok(response) => (response, false),
            Err(error) => (error_response(error, Some(&identity)), true),
        },
        Err(error) => (error_response(error, None), true),
    };

    let stdout = io::stdout();
    let mut output = stdout.lock();
    let _ = serde_json::to_writer(&mut output, &response);
    let _ = writeln!(output);
    if failed {
        std::process::exit(1);
    }
}

fn execute(identity: &CliSourceIdentity) -> CliResult<Value> {
    let mut args = env::args_os();
    let _program = args.next();
    let request_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::new("usage", "expected one request.json argument"))?;
    if args.next().is_some() {
        return Err(CliError::new(
            "usage",
            "expected exactly one request.json argument",
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
            "request.json exceeds the 4 MiB input limit",
        ));
    }
    let request: Request = serde_json::from_slice(&request_bytes)?;
    process_request(request, identity)
}

fn process_request(request: Request, identity: &CliSourceIdentity) -> CliResult<Value> {
    match request {
        Request::Foundation { input } => {
            let result = foundation_core::solve(&input).map_err(CliError::from_kernel_error)?;
            success_response(
                "foundation",
                None,
                identity,
                json!({ "foundation_result": result }),
            )
        }
        Request::Structural { model, target } => {
            let result =
                structural_core::analyze(&model, target).map_err(CliError::from_kernel_error)?;
            success_response(
                "structural",
                None,
                identity,
                json!({ "structural_result": result }),
            )
        }
        Request::CreateDemo { path, name } => create_demo(&path, name, identity),
        Request::Document {
            path,
            action,
            output_path,
            area_m2,
        } => document_operation(&path, action, output_path.as_deref(), area_m2, identity),
    }
}

fn create_demo(path: &str, name: Option<String>, identity: &CliSourceIdentity) -> CliResult<Value> {
    let output_path = checked_output_path(path, None)?;
    let input = FoundationStructuralInput::stage1_demo()
        .map_err(|error| CliError::new("invalid_fixture", error.to_string()))?;
    let mut document =
        Document::new(name.unwrap_or_else(|| "Foundation and structural stage 1 fixture".into()))
            .map_err(|error| CliError::new("document_error", error.to_string()))?;
    let transaction = Transaction::new(command_id("create_demo"), document_revision(&document)?)
        .with(Command::FoundationStructural(
            FoundationStructuralCommand::ReplaceInput { input },
        ));
    let receipt = document
        .commit(transaction)
        .map_err(|error| CliError::new("document_error", error.to_string()))?;
    document
        .save(&output_path)
        .map_err(|error| CliError::new("document_save_error", error.to_string()))?;

    success_response(
        "create_demo",
        Some(&document),
        identity,
        json!({ "commit": receipt, "saved_to": output_path }),
    )
}

fn document_operation(
    path: &str,
    action: DocumentAction,
    output_path: Option<&str>,
    area_m2: Option<f64>,
    identity: &CliSourceIdentity,
) -> CliResult<Value> {
    let source_path = fs::canonicalize(path)?;
    let mut document = Document::open(&source_path)
        .map_err(|error| CliError::new("document_open_error", error.to_string()))?;
    let mut outputs = json!({ "action": action.as_str() });

    match action {
        DocumentAction::Inspect => {}
        DocumentAction::Analyze => {
            let producer = identity.engineering_producer()?;
            let input = document.foundation_structural().input().ok_or_else(|| {
                CliError::new(
                    "missing_input",
                    "document has no structural/foundation input",
                )
            })?;
            let snapshot = analyze_foundation_structural(input, producer)
                .map_err(|error| CliError::new("analysis_error", error.to_string()))?;
            let returned_snapshot = snapshot.clone();
            let transaction =
                Transaction::new(command_id("analyze"), document_revision(&document)?).with(
                    Command::FoundationStructural(
                        FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot },
                    ),
                );
            let receipt = document
                .commit(transaction)
                .map_err(|error| CliError::new("document_commit_error", error.to_string()))?;
            outputs["snapshot"] = serde_json::to_value(returned_snapshot)?;
            outputs["commit"] = serde_json::to_value(receipt)?;
        }
        DocumentAction::EditSection => {
            let area_m2 = area_m2
                .ok_or_else(|| CliError::new("missing_area", "edit_section requires area_m2"))?;
            if !area_m2.is_finite() || area_m2 <= 0.0 {
                return Err(CliError::new(
                    "invalid_area",
                    "area_m2 must be finite and positive",
                ));
            }
            let mut input = document
                .foundation_structural()
                .input()
                .ok_or_else(|| {
                    CliError::new(
                        "missing_input",
                        "document has no structural/foundation input",
                    )
                })?
                .definition
                .clone();
            let section = input.structural_model.sections.first_mut().ok_or_else(|| {
                CliError::new("missing_section", "input model has no structural section")
            })?;
            let previous_area_m2 = section.area_m2;
            section.area_m2 = area_m2;
            let transaction =
                Transaction::new(command_id("edit_section"), document_revision(&document)?).with(
                    Command::FoundationStructural(FoundationStructuralCommand::ReplaceInput {
                        input,
                    }),
                );
            let receipt = document
                .commit(transaction)
                .map_err(|error| CliError::new("document_commit_error", error.to_string()))?;
            outputs["area_m2_before"] = json!(previous_area_m2);
            outputs["area_m2_after"] = json!(area_m2);
            outputs["commit"] = serde_json::to_value(receipt)?;
        }
        DocumentAction::Undo => {
            let receipt = document
                .undo()
                .map_err(|error| CliError::new("document_undo_error", error.to_string()))?;
            outputs["history_receipt"] = serde_json::to_value(receipt)?;
        }
        DocumentAction::Redo => {
            let receipt = document
                .redo()
                .map_err(|error| CliError::new("document_redo_error", error.to_string()))?;
            outputs["history_receipt"] = serde_json::to_value(receipt)?;
        }
    }

    if let Some(output_path) = output_path {
        let checked_path = checked_output_path(output_path, Some(&source_path))?;
        document
            .save(&checked_path)
            .map_err(|error| CliError::new("document_save_error", error.to_string()))?;
        outputs["saved_to"] = json!(checked_path);
    } else {
        outputs["saved_to"] = Value::Null;
    }

    success_response("document", Some(&document), identity, outputs)
}

fn checked_output_path(path: &str, source_path: Option<&Path>) -> CliResult<PathBuf> {
    if path.trim().is_empty() {
        return Err(CliError::new("invalid_output_path", "output path is empty"));
    }
    let root = env::var_os("AI_REVIT_FS_OUTPUT_ROOT").ok_or_else(|| {
        CliError::new(
            "output_root_required",
            "AI_REVIT_FS_OUTPUT_ROOT must name the allowed output directory",
        )
    })?;
    let root = fs::canonicalize(PathBuf::from(root))?;
    if !root.is_dir() {
        return Err(CliError::new(
            "invalid_output_root",
            "AI_REVIT_FS_OUTPUT_ROOT is not a directory",
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
            "output parent is outside AI_REVIT_FS_OUTPUT_ROOT",
        ));
    }
    let candidate = canonical_parent.join(file_name);
    if let Some(source_path) = source_path {
        if candidate.as_path() == source_path {
            return Err(CliError::new(
                "source_overwrite_rejected",
                "output_path must differ from the source document",
            ));
        }
    }

    match fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CliError::new(
                    "invalid_output_target",
                    "existing output target must be a regular file, not a link or directory",
                ));
            }
            let existing = fs::canonicalize(&candidate)?;
            if !existing.starts_with(&root) {
                return Err(CliError::new(
                    "output_path_escape",
                    "existing output target resolves outside AI_REVIT_FS_OUTPUT_ROOT",
                ));
            }
            if source_path.is_some_and(|source| source == existing.as_path()) {
                return Err(CliError::new(
                    "source_overwrite_rejected",
                    "output_path resolves to the source document",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(candidate)
}

fn document_revision(document: &Document) -> CliResult<u64> {
    let serialized = serde_json::to_value(document)?;
    serialized
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CliError::new(
                "document_metadata_error",
                "serialized document has no revision",
            )
        })
}

fn success_response(
    operation: &str,
    document: Option<&Document>,
    identity: &CliSourceIdentity,
    outputs: Value,
) -> CliResult<Value> {
    let metadata = document.map(document_metadata).transpose()?;
    Ok(json!({
        "ok": true,
        "operation": operation,
        "document": metadata,
        "source_identity": identity,
        "pid": std::process::id(),
        "outputs": outputs,
    }))
}

fn document_metadata(document: &Document) -> CliResult<Value> {
    let serialized = serde_json::to_value(document)?;
    let revision = serialized
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CliError::new(
                "document_metadata_error",
                "serialized document has no revision",
            )
        })?;
    let native_format_version = serialized
        .get("format_version")
        .and_then(Value::as_u64)
        .and_then(|version| u16::try_from(version).ok());
    let serialized_domain = serde_json::to_value(document.foundation_structural())?;
    Ok(json!({
        "document_uuid": document.document_uuid.to_string(),
        "revision": revision,
        "native_format": {
            "id": NATIVE_FORMAT_ID,
            "version": native_format_version,
        },
        "serialized_domain": serialized_domain,
    }))
}

fn runtime_identity() -> CliResult<CliSourceIdentity> {
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
    let source_manifest_sha256 = option_env!("AI_REVIT_FS_COMPILED_SOURCE_MANIFEST_SHA256")
        .filter(|value| is_sha256(value) && value.bytes().any(|byte| byte != b'0'))
        .map(|value| value.to_owned());
    let source_label = if source_manifest_sha256.is_some() {
        "compiled source-manifest binding; identity only; not engineering approval"
    } else {
        "no valid compile-time source-manifest binding; identity only; not engineering approval"
    };

    Ok(CliSourceIdentity {
        source_manifest_sha256,
        executable_sha256,
        solver_version: env!("CARGO_PKG_VERSION").to_owned(),
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
        "foundation-structural-cli-{action}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    )
}

fn error_response(error: CliError, identity: Option<&CliSourceIdentity>) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": error.code,
            "message": error.message,
            "details": error.details,
        },
        "source_identity": identity,
        "pid": std::process::id(),
    })
}
