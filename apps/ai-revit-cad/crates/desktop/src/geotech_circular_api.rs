//! Circular API owner adapter. Root must register these routes BEFORE legacy
//! `geotech.*`, provide `App.circular_host: Option<host::CircularHostController>`,
//! and reexport the promoted crates as `crate::{circular_host,circular_jobs}`.
//! CHEAPEST_VERIFICATION_PATH: source/rustfmt only until root combines those hooks.
//!
//! All public owner IDs/revisions/sequences are canonical decimal strings.
//! `context` bootstraps epoch guards; every other route requires that exact
//! expected_session_id/document_id/runtime_id/open_generation. Catalog pages
//! also require expected_revision, and use after_id (materials: offset).
//! Mutation params additionally require command_id, expected_revision and
//! transaction_mode="atomic". The existing dispatcher owns allocation/history.
//!
//! Quote returns a FULL immutable submission or OutputBudget, never a truncated
//! input. Analyze/search accept only that full `submission`; preserve it for an
//! unknown-response retry. Revision is checked by host only for NEW admission.
//! Result pages carry host-owned exact digest/cursor bindings, never full clones.
//! Token checks are API-only; native UI uses the shared command dispatcher.
//!
//! Root hooks still required: every intermediate commit/undo/redo/open calls
//! circular_after_mutation before another poll; the existing 50ms timer calls
//! circular_poll_jobs; close retains Document/controller/timer until quiescent.
//! Root must cap the outer RPC id/error wrapper even when this adapter rejects
//! an oversized id (the existing agent_handler owns the final response).
//! No HTTP runner, timer, Document, report/archive formatter or filesystem export.

use super::*;
use crate::{circular_host as host, circular_jobs as jobs};
use ai_revit_core::engineering::{
    AnalysisCaseId, CircularCaseDefinition, EngineeringCommand, GroundDefinition, GroundSectionId,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::{self, Write},
    ops::Bound::{Excluded, Unbounded},
};

type Failure = agent_handler::RequestFailure;
const PAGE_BYTES: usize = 64 << 10;
const WRAPPER_BYTES: usize = 16 << 10;
const INPUT_BYTES: usize = (1 << 20) - WRAPPER_BYTES;
const PAGE_ROWS: usize = 64;

/// Exact-identity gate for a separate bounded Root admission decision.
/// Independent review02 (2026-10-20) is at
/// `[local profile path omitted]
/// It finds evidence sufficient to support a later Root decision for supported
/// dry-static 2D circular calculations; the report itself grants no runtime
/// admission. Saved Root evidence cited there includes 37 circular library,
/// 33 integration and 12 search tests, plus one fixed CLI comparison with
/// 11,756 checks, zero failures and 260 slices. Preserve the existing unverified,
/// engineering-review-required result semantics and per-result topology/material,
/// finite-value, signed-driving, root/residual/physical/contact, refinement,
/// closure, resource/cancellation/staleness, source-binding and finite-search
/// guards. The evidence establishes no full published/Golden validation,
/// engineering approval, current native parity or product/release acceptance.
/// The fingerprint binds compiled `include_str!` source and is cached once;
/// it does not detect later on-disk edits. A changed compiled version or math
/// fingerprint fails shut.
pub(super) fn reviewed_circular_source() -> bool {
    geotech_core::circular::CIRCULAR_SOLVER_VERSION == "geotech-circular-0.2.0-gate8-geometry-precision01-v1"
        && geotech_core::circular::source_fingerprint()
            == "sha256:dbb13bc6773d4ba6c6a3aaed2d3d9e71407ddc09458c98955fd32588cb1963e8"
}

/// Only implemented routes: no report/snapshot/apply-candidate capability yet.
pub(super) const CIRCULAR_API_METHODS: &[&str] = &[
    "geotech.circular.context",
    "geotech.ground.list",
    "geotech.ground.create",
    "geotech.ground.inspect",
    "geotech.ground.update",
    "geotech.soil_material.list",
    "geotech.soil_material.inspect",
    "geotech.circular.list",
    "geotech.circular.create",
    "geotech.circular.inspect",
    "geotech.circular.archive_status",
    "geotech.circular.update",
    "geotech.circular.quote",
    "geotech.circular.analyze",
    "geotech.circular.search",
    "geotech.circular.job_status",
    "geotech.circular.search_status",
    "geotech.circular.cancel",
    "geotech.circular.result_page",
    "geotech.circular.slices",
    "geotech.circular.bishop_iterations",
    "geotech.circular.candidates",
    "geotech.circular.report.create",
    "geotech.circular.report.status",
    "geotech.circular.report.export",
];

pub(super) fn circular_api_requires_token(method: &str) -> bool {
    matches!(
        method,
        "geotech.ground.create" | "geotech.ground.update"
        | "geotech.circular.create" | "geotech.circular.update"
        | "geotech.circular.analyze" | "geotech.circular.search" | "geotech.circular.cancel"
        // Archive-byte paging is an explicit in-memory export; no FilePolicy bypass.
        | "geotech.circular.result_page"
        | "geotech.circular.report.create" | "geotech.circular.report.export"
    )
}

fn fail(code: &'static str, message: &str) -> Failure {
    // No request/token/source JSON is interpolated into diagnostics.
    let message: String = message.chars().take(512).collect();
    Failure::geometry_error(message, json!({"module":"geotech_circular","code":code}))
}

fn host_failure(error: host::HostError, next: host::DecimalU64) -> Failure {
    use host::HostError as H;
    use jobs::JobError as J;
    let code = match &error {
        H::InvalidContext => "InvalidContext",
        H::ContextMismatch => "ContextMismatch",
        H::InvalidInput => "InvalidInput",
        H::InputBudget => "InputBudget",
        H::RevisionConflict => "RevisionConflict",
        H::SourceMismatch => "SourceMismatch",
        H::NotReady => "NotReady",
        H::SearchSpecNotConfigured => "SearchSpecNotConfigured",
        H::UnknownCase => "UnknownCase",
        H::InvalidEnvelope => "InvalidEnvelope",
        H::PageBudget => "PageBudget",
        H::TransportBudget => "TransportBudget",
        H::InvalidPage => "InvalidPage",
        H::CursorGone => "CursorGone",
        H::CursorMismatch => "CursorMismatch",
        H::Job(J::Gone) => "Gone",
        H::Job(J::SequenceReused) => "SequenceReused",
        H::Job(J::SequenceExpected(_)) => "SequenceExpected",
        H::Job(J::Busy) => "Busy",
        H::Job(J::ShuttingDown) => "ShuttingDown",
        H::Job(J::NotCompleted) => "NotCompleted",
        H::Job(J::InputBudget) => "InputBudget",
        H::Job(J::InvalidInput) => "InvalidInput",
        H::Job(J::InvalidBinding) => "InvalidBinding",
        H::Job(J::InvalidLimits) => "InvalidLimits",
        H::Job(J::SpawnFailed) => "SpawnFailed",
    };
    let expected = match error {
        H::Job(J::SequenceExpected(value)) => Some(host::DecimalU64(value)),
        _ => None,
    };
    Failure::geometry_error(
        format!("Circular request rejected: {code}"),
        json!({
            "module":"geotech_circular", "code":code, "next_sequence":next,
            "expected_sequence":expected
        }),
    )
}

struct BoundedBytes {
    bytes: Vec<u8>,
    cap: usize,
}
impl BoundedBytes {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
        }
    }
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("circular API byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
// Count without cloning a large caller id/params or allocating encoded strings.
struct Count {
    used: usize,
    cap: usize,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.used = self
            .used
            .checked_add(bytes.len())
            .filter(|v| *v <= self.cap)
            .ok_or_else(|| io::Error::other("circular API byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn measure<T: Serialize + ?Sized>(value: &T, cap: usize) -> Result<(), Failure> {
    serde_json::to_writer(&mut Count { used: 0, cap }, value).map_err(|_| {
        fail(
            "TransportBudget",
            "Circular request exceeds the transport budget",
        )
    })
}
fn encoded<T: Serialize + ?Sized>(value: &T, cap: usize) -> Result<Vec<u8>, Failure> {
    let mut sink = BoundedBytes::new(cap);
    serde_json::to_writer(&mut sink, value).map_err(|_| {
        fail(
            "OutputBudget",
            "Circular response exceeds its byte budget; no partial response is returned",
        )
    })?;
    Ok(sink.bytes)
}
fn bounded_value<T: Serialize + ?Sized>(value: &T, cap: usize) -> Result<Value, Failure> {
    serde_json::from_slice(&encoded(value, cap)?).map_err(|_| {
        fail(
            "EncodingError",
            "Could not encode the typed circular response",
        )
    })
}
fn decode<T: DeserializeOwned>(params: &Value, key: &str) -> Result<T, Failure> {
    let value = params
        .get(key)
        .ok_or_else(|| fail("InvalidParams", "Missing required typed field"))?;
    let mut sink = BoundedBytes::new(INPUT_BYTES);
    serde_json::to_writer(&mut sink, value)
        .map_err(|_| fail("InputBudget", "Typed input exceeds the input budget"))?;
    serde_json::from_slice(&sink.bytes).map_err(|_| {
        fail(
            "InvalidParams",
            "Invalid typed field or unknown field in its definition",
        )
    })
}
fn text<'a>(params: &'a Value, key: &str, cap: usize) -> Result<&'a str, Failure> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= cap)
        .ok_or_else(|| fail("InvalidParams", "Missing, empty or oversized string field"))
}
fn decimal(params: &Value, key: &str) -> Result<host::DecimalU64, Failure> {
    decode(params, key)
}
fn owner_id(params: &Value, key: &str) -> Result<host::DecimalU64, Failure> {
    let id = decimal(params, key)?;
    if id.0 == 0 {
        Err(fail(
            "InvalidParams",
            "Owner IDs must be positive decimal strings",
        ))
    } else {
        Ok(id)
    }
}
fn optional_decimal(params: &Value, key: &str) -> Result<host::DecimalU64, Failure> {
    if params.get(key).is_some() {
        decimal(params, key)
    } else {
        Ok(host::DecimalU64(0))
    }
}
fn rows(params: &Value) -> Result<usize, Failure> {
    let count = if params.get("rows").is_some() {
        decimal(params, "rows")?.0
    } else {
        PAGE_ROWS as u64
    };
    if count == 0 || count > PAGE_ROWS as u64 {
        return Err(fail(
            "InvalidPage",
            "rows must be a decimal string from 1 to 64",
        ));
    }
    Ok(count as usize)
}
fn allowed(params: &Value, fields: &[&str]) -> Result<(), Failure> {
    const COMMON: &[&str] = &[
        "expected_session_id",
        "expected_document_id",
        "expected_runtime_id",
        "expected_open_generation",
    ];
    let object = params
        .as_object()
        .ok_or_else(|| fail("InvalidParams", "params must be an object"))?;
    if object
        .keys()
        .any(|key| !COMMON.contains(&key.as_str()) && !fields.contains(&key.as_str()))
    {
        return Err(fail(
            "InvalidParams",
            "Unknown parameter for this circular method",
        ));
    }
    Ok(())
}
fn revision_guard(doc: &Document, params: &Value) -> Result<(), Failure> {
    if decimal(params, "expected_revision")?.0 != doc.revision() {
        Err(fail(
            "RevisionConflict",
            "The catalog revision changed; restart paging",
        ))
    } else {
        Ok(())
    }
}

fn archive_status_params(doc: &Document, params: &Value) -> Result<(AnalysisCaseId, bool), Failure> {
    allowed(params, &["analysis_id", "expected_revision", "require_current"])?;
    revision_guard(doc, params)?;
    let require_current = params.get("require_current").and_then(Value::as_bool)
        .ok_or_else(|| fail("InvalidParams", "require_current must be a boolean"))?;
    let id = AnalysisCaseId::new(owner_id(params, "analysis_id")?.0);
    if doc.circular_case(id).is_none() {
        return Err(fail("UnknownCase", "Unknown circular analysis"));
    }
    Ok((id, require_current))
}

fn check_circular_context(doc: &Document, generation: u64, params: &Value) -> Result<(), Failure> {
    if text(params, "expected_session_id", 256)? != agent_handler::api_session_id()
        || text(params, "expected_document_id", 64)? != doc.document_uuid.to_string()
        || text(params, "expected_runtime_id", 256)? != doc.runtime_instance_id()
        || decimal(params, "expected_open_generation")?.0 != generation
    {
        return Err(fail("ContextMismatch", "The document, application session or open generation changed"));
    }
    Ok(())
}
#[derive(Serialize)]
struct ContextView {
    document_id: String,
    runtime_id: String,
    session_id: &'static str,
    open_generation: host::DecimalU64,
    revision: host::DecimalU64,
}
fn view(doc: &Document, generation: u64) -> ContextView {
    ContextView {
        document_id: doc.document_uuid.to_string(),
        runtime_id: doc.runtime_instance_id(),
        session_id: agent_handler::api_session_id(),
        open_generation: host::DecimalU64(generation),
        revision: host::DecimalU64(doc.revision()),
    }
}
#[derive(Serialize)]
struct GroundSummary<'a> {
    ground_section_id: GroundSectionId,
    name: &'a str,
    source_operation_id: host::DecimalU64,
    material_count: host::DecimalU64,
    terrain_point_count: host::DecimalU64,
    interface_count: host::DecimalU64,
    region_count: host::DecimalU64,
}
fn ground_summary(ground: &ai_revit_core::engineering::GroundSection) -> GroundSummary<'_> {
    GroundSummary {
        ground_section_id: ground.ground_section_id,
        name: &ground.name,
        source_operation_id: host::DecimalU64(ground.source_operation_id.get()),
        material_count: host::DecimalU64(ground.definition.materials.len() as u64),
        terrain_point_count: host::DecimalU64(ground.definition.terrain_polyline.len() as u64),
        interface_count: host::DecimalU64(ground.definition.layer_interfaces.len() as u64),
        region_count: host::DecimalU64(ground.definition.soil_regions.len() as u64),
    }
}
#[derive(Serialize)]
struct CaseSummary<'a> {
    analysis_id: AnalysisCaseId,
    name: &'a str,
    ground_section_id: GroundSectionId,
    source_operation_id: host::DecimalU64,
    search_spec_configured: bool,
}
fn case_summary(case: &ai_revit_core::engineering::CircularAnalysisCase) -> CaseSummary<'_> {
    CaseSummary {
        analysis_id: case.analysis_id,
        name: &case.name,
        ground_section_id: case.definition.ground_section_id,
        source_operation_id: host::DecimalU64(case.source_operation_id.get()),
        search_spec_configured: case.definition.search_spec.is_some(),
    }
}

/// Routine editor inspection borrows only authoritative editor/source fields.
/// An attached calculation archive is neither copied, serialized nor inspected
/// for integrity/currentness here; archive access belongs to explicit routes.
#[derive(Serialize)]
struct CircularCaseEditorWire<'a> {
    analysis_id: host::DecimalU64,
    name: &'a str,
    definition: &'a CircularCaseDefinition,
    source_operation_id: host::DecimalU64,
}
fn case_editor_wire(
    case: &ai_revit_core::engineering::CircularAnalysisCase,
) -> CircularCaseEditorWire<'_> {
    CircularCaseEditorWire {
        analysis_id: host::DecimalU64(case.analysis_id.get()),
        name: &case.name,
        definition: &case.definition,
        source_operation_id: host::DecimalU64(case.source_operation_id.get()),
    }
}
fn circular_case_inspection(
    doc: &Document,
    generation: u64,
    case: &ai_revit_core::engineering::CircularAnalysisCase,
) -> Result<Value, Failure> {
    #[derive(Serialize)]
    struct CaseView<'a> {
        format: &'static str,
        context: ContextView,
        case: CircularCaseEditorWire<'a>,
    }
    bounded_value(
        &CaseView {
            format: "circular-case-editor-projection-v1",
            context: view(doc, generation),
            case: case_editor_wire(case),
        },
        PAGE_BYTES,
    )
}

impl App {
    pub(super) fn circular_context(&self) -> host::HostContext {
        host::HostContext {
            session_id: agent_handler::api_session_id().to_owned(),
            open_generation: host::DecimalU64(self.api_document_generation),
        }
    }
    fn circular_check_context(&self, params: &Value) -> Result<(), Failure> {
        let doc = self
            .document
            .as_ref()
            .ok_or_else(|| fail("NoDocument", "No current document"))?;
        check_circular_context(doc, self.api_document_generation, params)
    }
    pub(super) fn circular_ensure_host(&mut self) -> Result<(), Failure> {
        if self.circular_host.is_none() {
            self.circular_host = Some(
                host::CircularHostController::new(host::HostLimits::default())
                    .map_err(|e| host_failure(e, host::DecimalU64(1)))?,
            );
        }
        Ok(())
    }

    pub(super) fn dispatch_circular_request(
        &mut self,
        cx: &mut Cx,
        request: &agent_api::ApiCommand,
    ) -> Result<Value, Failure> {
        if !CIRCULAR_API_METHODS.contains(&request.method.as_str()) {
            return Err(fail("UnknownMethod", "Unknown circular method"));
        }
        #[derive(Serialize)]
        struct Wrapper<'a> {
            id: &'a Value,
            ok: bool,
            result: Option<()>,
        }
        measure(
            &Wrapper {
                id: &request.id,
                ok: true,
                result: None,
            },
            WRAPPER_BYTES - 1024,
        )?;
        measure(&request.params, INPUT_BYTES)?;
        if circular_api_requires_token(&request.method) {
            self.require_api_token(request)?;
        }
        if matches!(request.method.as_str(), "geotech.circular.analyze" | "geotech.circular.search")
            && !reviewed_circular_source() {
            return Err(fail("NumericalReviewPending", "This numerical source has no bounded owner admission"));
        }
        if request.method == "geotech.circular.context" {
            allowed(&request.params, &[])?;
            self.circular_ensure_host()?;
            let doc = self
                .document
                .as_ref()
                .ok_or_else(|| fail("NoDocument", "No current document"))?;
            #[derive(Serialize)]
            struct Bootstrap {
                context: ContextView,
                next_sequence: host::DecimalU64,
                max_page_rows: host::DecimalU64,
                max_page_bytes: host::DecimalU64,
                max_result_bytes: host::DecimalU64,
            }
            return bounded_value(
                &Bootstrap {
                    context: view(doc, self.api_document_generation),
                    next_sequence: self.circular_host.as_ref().unwrap().next_sequence(),
                    max_page_rows: host::DecimalU64(PAGE_ROWS as u64),
                    max_page_bytes: host::DecimalU64(PAGE_BYTES as u64),
                    max_result_bytes: host::DecimalU64(8 << 20),
                },
                PAGE_BYTES,
            );
        }
        self.circular_check_context(&request.params)?;
        if request.method.starts_with("geotech.circular.report.") {
            return self.circular_report_dispatch(request);
        }
        match request.method.as_str() {
            "geotech.ground.create"
            | "geotech.ground.update"
            | "geotech.circular.create"
            | "geotech.circular.update" => {
                self.circular_mutate(cx, &request.method, &request.params)
            }
            "geotech.ground.list" | "geotech.circular.list" => {
                self.circular_catalog(&request.method, &request.params)
            }
            "geotech.ground.inspect"
            | "geotech.circular.inspect"
            | "geotech.soil_material.list"
            | "geotech.soil_material.inspect" => {
                self.circular_inspect(&request.method, &request.params)
            }
            "geotech.circular.archive_status" => {
                // The outer gate checked all four actual lifecycle identities.
                // Source/report integrity runs once on the shared owned worker;
                // status polls only read its metadata-fenced immutable cache.
                let (id, require_current) = archive_status_params(
                    self.document.as_ref().unwrap(), &request.params,
                )?;
                let archive = self.circular_archive_status(id, require_current).map_err(|message| {
                    let code = match message.as_str() {
                        "ARCHIVED_RESULT_STALE" => "ArchivedResultStale",
                        "ARCHIVE_INSPECTION_PENDING" => "ArchiveInspectionPending",
                        "ARCHIVE_INSPECTION_BUSY" | "ARCHIVE_INSPECTION_DEFERRED_CURRENT_CALCULATED" => "Busy",
                        "UNKNOWN_CIRCULAR_CASE" => "UnknownCase",
                        _ => "ArchiveInvalid",
                    };
                    fail(code, &message)
                })?;
                #[derive(Serialize)]
                struct ArchivedView {
                    context: ContextView,
                    archive: crate::geotech_circular_archive_view::ArchivePresentation,
                }
                bounded_value(&ArchivedView {
                    context: view(self.document.as_ref().unwrap(), self.api_document_generation),
                    archive,
                }, PAGE_BYTES)
            }
            "geotech.circular.quote" => {
                allowed(&request.params, &["analysis_id", "mode"])?;
                let id = owner_id(&request.params, "analysis_id")?;
                let mode = text(&request.params, "mode", 16)?;
                self.circular_ensure_host()?;
                let ctx = self.circular_context();
                let doc = self.document.as_ref().unwrap();
                let controller = self.circular_host.as_ref().unwrap();
                let result = match mode {
                    "specified" => controller.quote_specified(doc, &ctx, id),
                    "search" => controller.quote_search(doc, &ctx, id),
                    _ => {
                        return Err(fail(
                            "InvalidParams",
                            "quote mode must be specified or search",
                        ));
                    }
                }
                .map_err(|e| host_failure(e, controller.next_sequence()))?;
                // Full submission is the result itself, not reconstructed by analyze.
                bounded_value(&result, PAGE_BYTES)
            }
            "geotech.circular.analyze" | "geotech.circular.search" => {
                self.circular_submit(&request.method, &request.params)
            }
            "geotech.circular.job_status"
            | "geotech.circular.search_status"
            | "geotech.circular.cancel" => {
                allowed(&request.params, &["sequence"])?;
                let sequence = owner_id(&request.params, "sequence")?;
                self.circular_ensure_host()?;
                let ctx = self.circular_context();
                let doc = self.document.as_ref().unwrap();
                let controller = self.circular_host.as_mut().unwrap();
                controller.after_mutation(doc, &ctx);
                let status = if request.method == "geotech.circular.cancel" {
                    controller.cancel(doc, &ctx, sequence)
                } else {
                    controller.status(doc, &ctx, sequence)
                }
                .map_err(|e| host_failure(e, controller.next_sequence()))?;
                self.circular_validate_report_compute();
                bounded_value(&status, PAGE_BYTES)
            }
            _ => self.circular_page(&request.method, &request.params),
        }
    }

    fn circular_submit(&mut self, method: &str, params: &Value) -> Result<Value, Failure> {
        allowed(params, &["submission"])?;
        let source = params.get("submission").ok_or_else(|| {
            fail(
                "InvalidEnvelope",
                "A complete immutable submission is required",
            )
        })?;
        let bytes = encoded(source, INPUT_BYTES)?;
        self.circular_ensure_host()?;
        let next = self.circular_host.as_ref().unwrap().next_sequence();
        let envelope = host::decode_submission_bytes(&bytes).map_err(|e| host_failure(e, next))?;
        let proper_mode = matches!(
            (&envelope.request, &envelope.binding.mode, method),
            (
                host::FullRequest::Specified { .. },
                host::SubmissionMode::Specified,
                "geotech.circular.analyze"
            ) | (
                host::FullRequest::Search(_),
                host::SubmissionMode::Search,
                "geotech.circular.search"
            )
        );
        if !proper_mode {
            return Err(fail(
                "InvalidEnvelope",
                "Submission mode does not match the requested computation",
            ));
        }
        let ctx = self.circular_context();
        let doc = self.document.as_ref().unwrap();
        let controller = self.circular_host.as_mut().unwrap();
        controller.after_mutation(doc, &ctx);
        let status = controller
            .submit(doc, &ctx, &envelope)
            .map_err(|e| host_failure(e, controller.next_sequence()))?;
        self.circular_validate_report_compute();
        bounded_value(&status, PAGE_BYTES)
    }

    fn circular_page(&mut self, method: &str, params: &Value) -> Result<Value, Failure> {
        allowed(
            params,
            &["sequence", "rows", "bytes_cap", "cursor", "table"],
        )?;
        let sequence = owner_id(params, "sequence")?;
        let count = rows(params)?;
        let cap = if params.get("bytes_cap").is_some() {
            decimal(params, "bytes_cap")?.0
        } else {
            PAGE_BYTES as u64
        };
        if !(1024..=PAGE_BYTES as u64).contains(&cap) {
            return Err(fail(
                "InvalidPage",
                "bytes_cap must be a decimal string from 1024 to 65536",
            ));
        }
        let cursor = if params.get("cursor").is_some() {
            Some(text(params, "cursor", 71)?)
        } else {
            None
        };
        let kind = match method {
            "geotech.circular.result_page" => host::PageKind::ArchiveBytes,
            "geotech.circular.candidates" => host::PageKind::SearchCandidates,
            "geotech.circular.slices" => host::PageKind::Slices(decode(params, "table")?),
            "geotech.circular.bishop_iterations" => {
                host::PageKind::BishopIterations(decode(params, "table")?)
            }
            _ => return Err(fail("UnknownMethod", "Unknown circular page method")),
        };
        if params.get("table").is_some()
            && !matches!(
                &kind,
                host::PageKind::Slices(_) | host::PageKind::BishopIterations(_)
            )
        {
            return Err(fail(
                "InvalidPage",
                "This page kind does not accept a table selector",
            ));
        }
        self.circular_ensure_host()?;
        let ctx = self.circular_context();
        let doc = self.document.as_ref().unwrap();
        let controller = self.circular_host.as_mut().unwrap();
        let page = controller
            .result_page(doc, &ctx, sequence, kind, cursor, count, cap as usize)
            .map_err(|e| host_failure(e, controller.next_sequence()))?;
        // Host encoded a complete bounded page; decode ONLY that page for the
        // existing response wrapper. No whole result/canonical archive clone.
        serde_json::from_slice(&page.utf8)
            .map_err(|_| fail("EncodingError", "Invalid encoded host page"))
    }

    fn circular_mutate(
        &mut self,
        cx: &mut Cx,
        method: &str,
        params: &Value,
    ) -> Result<Value, Failure> {
        let id_field = match method {
            "geotech.ground.update" => "ground_section_id",
            "geotech.circular.update" => "analysis_id",
            _ => "",
        };
        let mut fields = vec![
            "command_id",
            "expected_revision",
            "transaction_mode",
            "definition",
            "name",
        ];
        if !id_field.is_empty() {
            fields.push(id_field);
        }
        allowed(params, &fields)?;
        let command_id = text(params, "command_id", 256)?;
        let revision = decimal(params, "expected_revision")?;
        if text(params, "transaction_mode", 16)? != "atomic" {
            return Err(fail(
                "InvalidParams",
                "transaction_mode must explicitly be atomic",
            ));
        }
        let name = if params.get("name").is_some() {
            Some(text(params, "name", 256)?.to_owned())
        } else {
            None
        };
        let command = match method {
            "geotech.ground.create" => EngineeringCommand::CreateGround {
                definition: decode::<GroundDefinition>(params, "definition")?,
                name: name.ok_or_else(|| fail("InvalidParams", "create requires a name"))?,
            },
            "geotech.ground.update" => EngineeringCommand::UpdateGround {
                ground_section_id: GroundSectionId::new(owner_id(params, id_field)?.0),
                definition: decode::<GroundDefinition>(params, "definition")?,
                name,
            },
            "geotech.circular.create" => EngineeringCommand::CreateCircular {
                definition: decode::<CircularCaseDefinition>(params, "definition")?,
                name: name.ok_or_else(|| fail("InvalidParams", "create requires a name"))?,
            },
            "geotech.circular.update" => EngineeringCommand::UpdateCircular {
                analysis_id: AnalysisCaseId::new(owner_id(params, id_field)?.0),
                definition: decode::<CircularCaseDefinition>(params, "definition")?,
                name,
            },
            _ => return Err(fail("UnknownMethod", "Unknown circular mutation")),
        };
        // Decimal wire -> legacy dispatcher u64 fields, losslessly, with no f64
        // conversion or generated command ID/revision/identity fallback.
        let apply = json!({"command_id":command_id, "expected_revision":revision.0,
            "expected_session_id":text(params,"expected_session_id",256)?,
            "expected_document_id":text(params,"expected_document_id",64)?,
            "expected_open_generation":decimal(params,"expected_open_generation")?.0,
            "transaction_mode":"atomic", "commands":[Command::Engineering(command)]});
        let outcome = self.api_command_apply(cx, &apply);
        // Also runs if the common path committed but failed later while forming
        // its reply. Root's common hook is still mandatory for all OTHER owners.
        self.circular_after_mutation();
        let receipt = outcome.map_err(|e| fail("CommandRejected", &e.message))?;
        self.circular_receipt_summary(&receipt)
    }

    fn circular_receipt_summary(&self, receipt: &Value) -> Result<Value, Failure> {
        // A ground edit can affect many cases: do not echo its unbounded receipt
        // delta. The authoritative full receipt remains in the shared Document.
        fn native_u64(value: Option<&Value>) -> Result<u64, Failure> {
            value
                .and_then(Value::as_u64)
                .ok_or_else(|| fail("ReceiptError", "Invalid shared command receipt"))
        }
        fn ids(value: Option<&Value>) -> Result<Vec<host::DecimalU64>, Failure> {
            let values = value
                .and_then(Value::as_array)
                .ok_or_else(|| fail("ReceiptError", "Invalid shared command receipt identities"))?;
            if values.len() > 1 {
                return Err(fail(
                    "ReceiptError",
                    "Single-command receipt contains multiple allocated owners",
                ));
            }
            values
                .iter()
                .map(|v| {
                    if let Some(n) = v.as_u64() {
                        Ok(host::DecimalU64(n))
                    } else {
                        serde_json::from_value(v.clone())
                            .map_err(|_| fail("ReceiptError", "Invalid shared receipt identity"))
                    }
                })
                .collect()
        }
        let delta = receipt.pointer("/delta/engineering");
        let doc = self
            .document
            .as_ref()
            .ok_or_else(|| fail("NoDocument", "No current document"))?;
        let ground_ids = ids(delta
            .and_then(|v| v.get("created_ground_sections"))
            .or_else(|| Some(&EMPTY_ARRAY)))?;
        let case_ids = ids(delta
            .and_then(|v| v.get("created_cases"))
            .or_else(|| Some(&EMPTY_ARRAY)))?;
        let owner_current = ground_ids
            .iter()
            .all(|id| doc.ground_section(GroundSectionId::new(id.0)).is_some())
            && case_ids
                .iter()
                .all(|id| doc.circular_case(AnalysisCaseId::new(id.0)).is_some());
        let operation_ids = ids(receipt.get("operation_ids"))?;
        bounded_value(
            &json!({"format":"circular-command-receipt-summary-v1",
                "transaction_id":receipt.get("transaction_id"),
                "revision_before":host::DecimalU64(native_u64(receipt.get("revision_before"))?),
                "revision_after":host::DecimalU64(native_u64(receipt.get("revision_after"))?),
                "replayed":receipt.get("replayed").and_then(Value::as_bool).unwrap_or(false),
                "operation_ids":operation_ids, "created_ground_sections":ground_ids, "created_cases":case_ids,
                "created_owners_present":(!ground_ids.is_empty() || !case_ids.is_empty()).then_some(owner_current),
                "context":view(doc,self.api_document_generation),
                "affected_case_count":host::DecimalU64(delta.and_then(|v|v.get("affected_cases")).and_then(Value::as_array).map_or(0,|v|v.len()) as u64),
                "affected_ground_count":host::DecimalU64(delta.and_then(|v|v.get("affected_ground_sections")).and_then(Value::as_array).map_or(0,|v|v.len()) as u64)
            }),
            PAGE_BYTES,
        )
    }

    fn circular_catalog(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        allowed(params, &["expected_revision", "after_id", "rows"])?;
        let doc = self.document.as_ref().unwrap();
        revision_guard(doc, params)?;
        let count = rows(params)?;
        let after = optional_decimal(params, "after_id")?.0;
        #[derive(Serialize)]
        struct Catalog<T> {
            context: ContextView,
            rows: Vec<T>,
            next_after_id: Option<host::DecimalU64>,
        }
        if method == "geotech.ground.list" {
            let mut iter = doc
                .ground_sections()
                .range((Excluded(GroundSectionId::new(after)), Unbounded));
            let entries: Vec<_> = iter
                .by_ref()
                .take(count)
                .map(|(_, v)| ground_summary(v))
                .collect();
            let next = iter.next().and_then(|_| {
                entries
                    .last()
                    .map(|v| host::DecimalU64(v.ground_section_id.get()))
            });
            bounded_value(
                &Catalog {
                    context: view(doc, self.api_document_generation),
                    rows: entries,
                    next_after_id: next,
                },
                PAGE_BYTES,
            )
        } else {
            let mut iter = doc
                .circular_cases()
                .range((Excluded(AnalysisCaseId::new(after)), Unbounded));
            let entries: Vec<_> = iter
                .by_ref()
                .take(count)
                .map(|(_, v)| case_summary(v))
                .collect();
            let next = iter.next().and_then(|_| {
                entries
                    .last()
                    .map(|v| host::DecimalU64(v.analysis_id.get()))
            });
            bounded_value(
                &Catalog {
                    context: view(doc, self.api_document_generation),
                    rows: entries,
                    next_after_id: next,
                },
                PAGE_BYTES,
            )
        }
    }

    fn circular_inspect(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        let doc = self.document.as_ref().unwrap();
        if method == "geotech.circular.inspect" {
            allowed(params, &["analysis_id"])?;
            let id = AnalysisCaseId::new(owner_id(params, "analysis_id")?.0);
            let case = doc
                .circular_case(id)
                .ok_or_else(|| fail("UnknownCase", "Unknown circular case"))?;
            return circular_case_inspection(doc, self.api_document_generation, case);
        }
        let fields: &[&str] = match method {
            "geotech.ground.inspect" => &["ground_section_id"],
            "geotech.soil_material.list" => {
                &["ground_section_id", "expected_revision", "offset", "rows"]
            }
            _ => &["ground_section_id", "material_id"],
        };
        allowed(params, fields)?;
        let id = GroundSectionId::new(owner_id(params, "ground_section_id")?.0);
        let ground = doc
            .ground_section(id)
            .ok_or_else(|| fail("UnknownGround", "Unknown ground section"))?;
        if method == "geotech.ground.inspect" {
            // Bound borrowed source BEFORE readiness traversal, no solver call.
            measure(ground, PAGE_BYTES)?;
            let readiness = doc
                .ground_readiness(id)
                .map_err(|_| fail("InvalidGround", "Could not inspect ground readiness"))?;
            #[derive(Serialize)]
            struct GroundView<'a> {
                context: ContextView,
                ground: &'a ai_revit_core::engineering::GroundSection,
                readiness: ai_revit_core::engineering::GroundReadinessReport,
            }
            return bounded_value(
                &GroundView {
                    context: view(doc, self.api_document_generation),
                    ground,
                    readiness,
                },
                PAGE_BYTES,
            );
        }
        if method == "geotech.soil_material.inspect" {
            let material_id = geotech_core::circular::SoilMaterialId(
                text(params, "material_id", 256)?.to_owned(),
            );
            let material = doc.ground_material(id, &material_id).ok_or_else(|| {
                fail("UnknownMaterial", "Unknown material in this ground section")
            })?;
            #[derive(Serialize)]
            struct MaterialView<'a> {
                context: ContextView,
                ground_section_id: GroundSectionId,
                source_operation_id: host::DecimalU64,
                material: &'a geotech_core::circular::GroundMaterial,
            }
            return bounded_value(
                &MaterialView {
                    context: view(doc, self.api_document_generation),
                    ground_section_id: id,
                    source_operation_id: host::DecimalU64(ground.source_operation_id.get()),
                    material,
                },
                PAGE_BYTES,
            );
        }
        revision_guard(doc, params)?;
        let offset = usize::try_from(optional_decimal(params, "offset")?.0)
            .map_err(|_| fail("InvalidPage", "Material offset overflow"))?;
        let count = rows(params)?;
        if offset > ground.definition.materials.len() {
            return Err(fail(
                "InvalidPage",
                "Material offset exceeds the collection",
            ));
        }
        let end = offset
            .saturating_add(count)
            .min(ground.definition.materials.len());
        #[derive(Serialize)]
        struct Materials<'a> {
            context: ContextView,
            ground_section_id: GroundSectionId,
            source_operation_id: host::DecimalU64,
            offset: host::DecimalU64,
            total: host::DecimalU64,
            next_offset: Option<host::DecimalU64>,
            materials: &'a [geotech_core::circular::GroundMaterial],
        }
        bounded_value(
            &Materials {
                context: view(doc, self.api_document_generation),
                ground_section_id: id,
                source_operation_id: host::DecimalU64(ground.source_operation_id.get()),
                offset: host::DecimalU64(offset as u64),
                total: host::DecimalU64(ground.definition.materials.len() as u64),
                next_offset: (end < ground.definition.materials.len())
                    .then_some(host::DecimalU64(end as u64)),
                materials: &ground.definition.materials[offset..end],
            },
            PAGE_BYTES,
        )
    }

    /// Common refresh must invoke this for EACH intermediate mutation (ABA).
    pub(super) fn circular_after_mutation(&mut self) {
        let ctx = self.circular_context();
        if let Some(doc) = self.document.as_ref() {
            self.circular_report.after_mutation(doc, &ctx);
        } else {
            self.circular_report.begin_shutdown();
        }
        if let Some(controller) = self.circular_host.as_mut() {
            if let Some(doc) = self.document.as_ref() {
                controller.after_mutation(doc, &ctx);
            } else {
                controller.begin_shutdown();
            }
        }
        self.circular_validate_report_compute();
        self.circular_native_validate_archive_view();
    }
    /// Existing owner timer only. No implicit compute/report, no blocking join.
    pub(super) fn circular_poll_jobs(&mut self) {
        #[cfg(feature = "native-boundary-harness")]
        crate::circular_owner_order::enter_poll();
        let ctx = self.circular_context();
        if let Some(doc) = self.document.as_ref() {
            self.circular_report.poll(doc, &ctx);
        }
        if let Some(controller) = self.circular_host.as_mut() {
            if let Some(doc) = self.document.as_ref() {
                controller.poll(doc, &ctx);
            }
            // Losing Document before quiescence is a host lifecycle violation:
            // stop admission, but do not claim that the worker has been reaped.
            else {
                controller.begin_shutdown();
            }
        }
        self.circular_validate_report_compute();
    }
    pub(super) fn circular_begin_shutdown(&mut self) {
        self.circular_report.begin_shutdown();
        if let Some(controller) = self.circular_host.as_mut() {
            controller.begin_shutdown();
        }
    }
    pub(super) fn circular_is_quiescent(&self) -> bool {
        self.circular_report.is_quiescent() && self.circular_host
            .as_ref()
            .is_none_or(|controller| controller.is_quiescent())
    }
    /// Typed borrowed access for native UI/report adoption. The closure cannot
    /// retain a result borrow across owner/Document mutation. Its arbitrary work
    /// and any copies it makes are CALLER-owned budgets, not bounded here.
    pub(super) fn circular_with_current_result<R>(
        &mut self,
        sequence: host::DecimalU64,
        inspect: impl for<'a> FnOnce(&'a jobs::PublishedResult) -> R,
    ) -> Result<R, Failure> {
        let ctx = self.circular_context();
        let doc = self
            .document
            .as_ref()
            .ok_or_else(|| fail("NoDocument", "No current document"))?;
        let controller = self
            .circular_host
            .as_mut()
            .ok_or_else(|| fail("Gone", "No circular job receipt is retained"))?;
        let next = controller.next_sequence();
        let result = controller
            .current_result(doc, &ctx, sequence)
            .map_err(|e| host_failure(e, next))?;
        Ok(inspect(result))
    }
}

// Empty receipts omit optional engineering vectors. A constant avoids an input
// clone/default identity; it represents only an empty collection.
static EMPTY_ARRAY: Value = Value::Array(Vec::new());

#[cfg(test)]
mod case_projection_tests {
    use super::*;

    fn archive_fixture() -> Document {
        let fixtures = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures();
        Document::open(fixtures.join("specified.native4.json")).unwrap()
    }

    #[test]
    fn archive_status_checks_all_lifecycle_identities_revision_canonical_ids_and_strict_boolean() {
        let document = archive_fixture();
        let id = *document.circular_cases().keys().next().unwrap();
        let params = json!({
            "expected_session_id":agent_handler::api_session_id(),
            "expected_document_id":document.document_uuid.to_string(),
            "expected_runtime_id":document.runtime_instance_id(),
            "expected_open_generation":"9",
            "expected_revision":document.revision().to_string(),
            "analysis_id":id.get().to_string(),
            "require_current":false,
        });
        assert!(check_circular_context(&document, 9, &params).is_ok());
        assert!(archive_status_params(&document, &params).is_ok());
        for field in ["expected_session_id", "expected_document_id", "expected_runtime_id", "expected_open_generation"] {
            let mut changed = params.clone();
            changed[field] = Value::String(if field == "expected_open_generation" { "10" } else { "changed" }.into());
            assert!(check_circular_context(&document, 9, &changed).is_err());
        }
        for invalid in [json!("false"), json!(0), Value::Null] {
            let mut changed = params.clone();
            changed["require_current"] = invalid;
            assert!(archive_status_params(&document, &changed).is_err());
        }
        let mut changed = params.clone();
        changed.as_object_mut().unwrap().remove("require_current");
        assert!(archive_status_params(&document, &changed).is_err());
        changed = params.clone();
        changed["expected_revision"] = json!((document.revision() + 1).to_string());
        assert!(archive_status_params(&document, &changed).is_err());
        for invalid in [json!(format!("0{}", id.get())), json!(id.get()), json!(u64::MAX.to_string())] {
            let mut changed = params.clone();
            changed["analysis_id"] = invalid;
            assert!(archive_status_params(&document, &changed).is_err());
        }
        changed = params.clone();
        changed["unexpected"] = json!(true);
        assert!(archive_status_params(&document, &changed).is_err());
        assert_eq!(owner_id(&json!({"analysis_id":"9007199254740993"}), "analysis_id").unwrap_or_else(|error| panic!("{}", error.message)).0, 9_007_199_254_740_993);
    }

    #[test]
    fn archive_status_projection_preserves_raw_summary_and_unchanged_page_cap() {
        let document = archive_fixture();
        let id = *document.circular_cases().keys().next().unwrap();
        let inspection = crate::geotech_circular_archive_view::inspect(id,
            document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap()).unwrap();
        let value = bounded_value(&inspection.presentation, PAGE_BYTES).unwrap_or_else(|error| panic!("{}", error.message));
        assert_eq!(value["archive"]["case_id"], id.get().to_string());
        assert_eq!(value["current_job_result"], false);
        assert!(serde_json::to_vec(&value).unwrap().len() <= PAGE_BYTES);
        let mut oversized = inspection.presentation;
        oversized.archive.as_mut().unwrap().saved_calculation_worker_arithmetic_scope = Some("x".repeat(PAGE_BYTES));
        assert!(bounded_value(&oversized, PAGE_BYTES).is_err());
        assert_eq!(PAGE_BYTES, 65_536);
    }

    #[test]
    fn circular_inspect_projects_archive_bearing_core_cases_within_page_budget() {
        // Explicit synthetic setup precedes the actual Core document open.
        // Projection preserves generated values and never calls the solver.
        let fixtures = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures();
        for name in ["specified.native4.json", "search.native4.json"] {
            let document = Document::open(fixtures.join(name))
                .unwrap_or_else(|_| panic!("Could not open saved calculation fixture {name}"));
            assert_eq!(document.circular_cases().len(), 1);
            let case = document.circular_cases().values().next().unwrap();
            assert!(case.circular_snapshot.is_some());
            // The old full-record response hits the unchanged 64KiB limit.
            // Count stops at the cap instead of allocating the full archive.
            assert!(measure(case, PAGE_BYTES).is_err());

            let borrowed = case_editor_wire(case);
            assert!(std::ptr::eq(borrowed.definition, &case.definition));
            assert!(std::ptr::eq(borrowed.name, case.name.as_str()));
            let response = circular_case_inspection(&document, 1, case)
                .unwrap_or_else(|_| panic!("Editor projection exceeds page budget for {name}"));
            assert_eq!(response["format"], "circular-case-editor-projection-v1");
            assert_eq!(response["case"]["analysis_id"], case.analysis_id.get().to_string());
            assert_eq!(response["case"]["name"], case.name);
            assert_eq!(
                response["case"]["source_operation_id"],
                case.source_operation_id.get().to_string()
            );
            assert_eq!(
                response["case"]["definition"],
                serde_json::to_value(&case.definition).unwrap()
            );
            let fields = response["case"].as_object().unwrap();
            assert_eq!(fields.len(), 4);
            assert!(!fields.contains_key("circular_snapshot"));
            assert!(case.circular_snapshot.is_some());
            assert!(serde_json::to_vec(&response).unwrap().len() <= PAGE_BYTES);
        }
    }
}
