//! Native intents -> the SAME Document transaction dispatcher and circular host.
//! CHEAPEST_VERIFICATION_PATH: focused rustfmt/source only; root owns wiring/build.
//!
//! Required App field: #[rust] circular_native: CircularNativeBridge.
//! Root registers this module and imports CircularNativeBridge. API owner must
//! expose circular_ensure_host/circular_context as pub(super), without changing
//! their logic. Timer calls circular_native_poll_jobs(cx) IN PLACE OF the plain
//! circular_poll_jobs call; no second timer/worker/HTTP/Document is introduced.
//! During a synchronous native save, common circular catalog/observe refresh
//! must defer while circular_native_commit_in_progress(); mutation invalidation
//! MUST still run immediately. Typed save acknowledgement precedes UI refresh.
//!
//! One UI ticket -> one host sequence, retaining its ORIGINAL bounded full
//! submission for exact retry/unknown-reply recovery. One display clone lives in
//! CircularPanel only. Cache encoded bytes <=8 MiB are checked before any result
//! copy; the core's bounded finite/node canonical audit runs on borrowed output.
//! These are explicit serialization/node/copy limits, NOT a process RSS bound.
//! No geometry preparation, solver, input_fingerprint, or report formatting runs
//! here. archived_input_digest is pure version-bound serialization, not math.

use super::*;
use crate::geotech_circular_ui::{
    CalculationIdentity, CircularCaseEditorProjection, CircularUiIntent, DocumentBinding,
    EditorContext, JobKind, JobTicket, PhysicalStatus, RecordLoadMode, RecordLoadTicket,
    ReportTicket, ResultReview, SaveCaseProposal, SaveGroundProposal, SearchBox,
};
use crate::{circular_host as host, circular_jobs as jobs};
use ai_revit_core::{CommandId, CommitReceipt, engineering::EngineeringCommand};
use geotech_core::{InputUnits, circular::*};
use serde::Serialize;
use std::{
    io::{self, Write},
    sync::Arc,
};

const SOURCE_BYTES: usize = (1 << 20) - (16 << 10);
const DISPLAY_BYTES: usize = 8 << 20;
const RECEIPT_BYTES: usize = 1 << 20;

pub(super) struct CircularNativeJobBinding {
    pub(super) ticket: JobTicket,
    pub(super) host_sequence: host::DecimalU64,
    pub(super) submission: host::Submission,
    pub(super) draft_generation: u64,
    pub(super) load_generation: u64,
    pub(super) delivered: bool,
    cancel_requested: bool,
    last_progress: u64,
}
struct CircularNativeArchiveBinding {
    context: EditorContext,
    analysis_id: ai_revit_core::AnalysisCaseId,
    sequence: u64,
}
#[derive(Default)]
pub(super) struct CircularNativeBridge {
    pub(super) current: Option<CircularNativeJobBinding>,
    committing: bool,
    /// Explicit root owner decisions after its accepted math/source audit.
    /// Presence of a result, source, or accepted FP environment cannot grant them.
    pub(super) analysis_admitted: bool,
    pub(super) search_admitted: bool,
    pub(super) display_serialized_budget_used: usize,
    archive: Option<CircularNativeArchiveBinding>,
    archive_view_key: Option<crate::geotech_circular_archive_view::ArchiveKey>,
}

struct ByteBudget(usize);
impl Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("native circular byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded<T: Serialize + ?Sized>(value: &T, cap: usize) -> Result<(), String> {
    serde_json::to_writer(ByteBudget(cap), value)
        .map_err(|_| "NATIVE_CIRCULAR_BYTE_BUDGET".to_owned())
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string().chars().take(512).collect()
}
fn same_epoch(a: &DocumentBinding, b: &DocumentBinding) -> bool {
    a.document_uuid == b.document_uuid
        && a.session_id == b.session_id
        && a.open_generation == b.open_generation
}
fn same_identity(a: &CalculationIdentity, b: &CalculationIdentity) -> bool {
    same_epoch(&a.document, &b.document)
        && a.ground_id == b.ground_id
        && a.analysis_id == b.analysis_id
        && a.ground_source_operation_id == b.ground_source_operation_id
        && a.case_source_operation_id == b.case_source_operation_id
        && a.config == b.config
        && a.source_revision == b.source_revision
        && a.input_fingerprint == b.input_fingerprint
        && a.source_fingerprint == b.source_fingerprint
        && a.solver_version == b.solver_version
}
fn source_limits() -> geotech_report::ReportLimits {
    geotech_report::ReportLimits {
        input_bytes: SOURCE_BYTES,
        ..Default::default()
    }
}
fn report_review(result: &CircularSlopeResult, config: &CircularAnalysisConfig) -> ResultReview {
    let mut review = ResultReview::default(); // independently_verified ALWAYS false.
    let yes = PhysicalStatus::Admissible;
    let no = PhysicalStatus::Inadmissible;
    if result.accepted && !result.diagnostic_only {
        review.fellenius.physical_status = yes;
        review.fellenius.explanation="Core specified-circle acceptance; dry static finite model only; engineering review required".into();
    }
    let b = &result.bishop_result;
    review.bishop.physical_status = if b.convergence_status == BishopConvergenceStatus::Converged
        && b.accepted_factor_of_safety.is_some()
        && b.physical_assessment.status == BishopPhysicalStatus::Admissible
    {
        yes
    } else {
        no
    };
    review.bishop.explanation = format!(
        "Core {:?}, {:?}; independent review NOT_GRANTED",
        b.convergence_status, b.physical_assessment.status
    );
    review.finite_table.physical_status = match b.physical_assessment.status {
        BishopPhysicalStatus::Admissible => yes,
        BishopPhysicalStatus::NumericallyIndeterminate => PhysicalStatus::NotAssessed,
        _ => no,
    };
    if let Some(endpoint) = &b.endpoint_assessment {
        review.geometry_endpoints.physical_status = if endpoint.passed { yes } else { no };
        review.geometry_endpoints.explanation =
            "Actual core endpoint assessment; no whole-arc inference".into();
    }
    if let Some(arc) = &b.whole_arc_contact {
        let bound = arc.source_fingerprint == result.source_fingerprint
            && arc.config == config.contact
            && arc.coverage_complete
            && !arc.scope.is_empty()
            && !arc.root_range_scope.is_empty()
            && !arc.contact_input_fingerprint.is_empty();
        review.whole_arc.physical_status = match arc.status {
            WholeArcContactStatus::PassedDeclaredModel if bound => yes,
            WholeArcContactStatus::Violation => no,
            _ => PhysicalStatus::NotAssessed,
        };
        review.whole_arc_applicability = bound.then(|| arc.scope.clone());
        review.whole_arc.explanation="Actual worker core conditional declared model; RN/gradual underflow checked by worker; independent acceptance NOT_GRANTED".into();
    }
    review
}

enum DisplayCopy {
    Specified(Arc<CircularSlopeResult>, ResultReview),
    Search(Arc<SearchResultV1>),
}
fn display_copy(saved: &jobs::PublishedResult) -> Result<(DisplayCopy, usize), String> {
    let bytes = saved.canonical_bytes().len();
    if bytes > DISPLAY_BYTES {
        return Err("NATIVE_DISPLAY_OUTPUT_BUDGET_8_MIB".into());
    }
    if !saved.worker_arithmetic.accepted {
        return Err("WORKER_FP_ENVIRONMENT_NOT_ACCEPTED".into());
    }
    // Owner helper audits finite f64, <=1M canonical nodes, depth<=128 and
    // <=16MiB canonical payload. It drops temporary audit storage before clone.
    canonical_version_bound_digest(1, &saved.binding.solver_version, saved.output())
        .map_err(message)?;
    let copy = match (saved.request(), saved.output()) {
        (jobs::JobRequest::Specified { config, .. }, jobs::JobOutput::Specified(result)) => {
            if result.numerically_verified || !result.engineering_review_required {
                return Err("UNSUPPORTED_REVIEW_FLAGS".into());
            }
            DisplayCopy::Specified(Arc::new(result.clone()), report_review(result, config))
        }
        (jobs::JobRequest::Search(_), jobs::JobOutput::Search(result)) => {
            DisplayCopy::Search(Arc::new(result.clone()))
        }
        _ => return Err("NATIVE_RESULT_MODE_MISMATCH".into()),
    };
    Ok((copy, bytes))
}

impl App {
    pub(super) fn circular_native_commit_in_progress(&self) -> bool {
        self.circular_native.committing
    }

    /// Caller must establish authority from the actual owned report/job first.
    /// Move the typed snapshot through the common transaction path; no JSON
    /// command tree, archive clone, or externally supplied bypass flag.
    pub(super) fn circular_native_commit_snapshot(
        &mut self,
        cx: &mut Cx,
        expected: &DocumentBinding,
        analysis_id: ai_revit_core::AnalysisCaseId,
        snapshot: ai_revit_core::CircularCalculationSnapshot,
    ) -> Result<CommitReceipt, String> {
        if self.circular_binding()? != *expected {
            return Err("STALE_NATIVE_SNAPSHOT_CONTEXT".into());
        }
        if self.next_command_sequence == u64::MAX {
            return Err("NATIVE_COMMAND_SEQUENCE_EXHAUSTED".into());
        }
        #[cfg(target_os="linux")]
        self.gui_job_snapshot_commit_before(cx,expected,analysis_id,&snapshot)?;
        let command_id = self.next_command_id("circular-snapshot");
        self.circular_native.committing = true;
        let outcome = self.apply_typed_commands(
            cx,
            command_id,
            expected.revision,
            vec![Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
                analysis_id,
                snapshot,
            })],
        );
        self.circular_native.committing = false;
        // Invalidation must happen even if the adapter/ack later fails.
        self.circular_after_mutation();
        outcome.map_err(|error| message(error.message))
    }

    fn circular_native_require_context(&self, expected: &EditorContext) -> Result<(), String> {
        if self.circular_ui.panel.editor_context()? != *expected
            || self.circular_binding()? != expected.document
        {
            return Err("STALE_NATIVE_EDITOR_CONTEXT".into());
        }
        Ok(())
    }
    fn circular_native_command(
        &mut self,
        cx: &mut Cx,
        label: &str,
        expected: &DocumentBinding,
        command: &EngineeringCommand,
    ) -> Result<(CommandId, CommitReceipt), String> {
        if self.circular_binding()? != *expected {
            return Err("STALE_NATIVE_TRANSACTION_CONTEXT".into());
        }
        bounded(command, SOURCE_BYTES)?;
        if self.next_command_sequence == u64::MAX {
            return Err("NATIVE_COMMAND_SEQUENCE_EXHAUSTED".into());
        }
        let command_id = self.next_command_id(label);
        let params = json!({"command_id":command_id,"expected_revision":expected.revision,
            "expected_session_id":agent_handler::api_session_id(),"expected_document_id":expected.document_uuid,
            "expected_open_generation":expected.open_generation,"transaction_mode":"atomic",
            "commands":[Command::Engineering(command.clone())]});
        self.circular_native.committing = true;
        let outcome = self.api_command_apply(cx, &params);
        self.circular_native.committing = false;
        // Common mutation hook must run even when later receipt formation fails.
        self.circular_after_mutation();
        let value = outcome.map_err(|e| message(e.message))?;
        bounded(&value, RECEIPT_BYTES).map_err(|_| {
            format!(
                "TRANSACTION_COMMITTED_UI_RECEIPT_BUDGET: {command_id}; reload current Document"
            )
        })?;
        let receipt: CommitReceipt = serde_json::from_value(value).map_err(message)?;
        Ok((command_id.into(), receipt))
    }

    fn circular_native_load(
        &mut self,
        cx: &mut Cx,
        ticket: RecordLoadTicket,
    ) -> Result<(), String> {
        self.circular_native_require_context(&ticket.context)?;
        self.circular_native.archive = None;
        self.circular_native.archive_view_key = None;
        let live = self.circular_binding()?;
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let ground = doc
            .ground_section(ticket.ground.0)
            .ok_or("UNKNOWN_GROUND")?;
        if ground.source_operation_id != ticket.ground.1 {
            return Err("STALE_LOAD_GROUND".into());
        }
        bounded(ground, SOURCE_BYTES)?;
        let ground = Arc::new(ground.clone());
        match ticket.mode {
            RecordLoadMode::Ground => {
                self.circular_ui.panel.load_ground(&self.ui, cx, &ticket, live, ground)?;
                self.circular_native_release_report_presentation();
            }
            RecordLoadMode::NewCase => {
                self.circular_ui.panel.new_case_draft(
                    &self.ui,
                    cx,
                    &ticket,
                    live,
                    CircularAnalysisConfig::default(),
                    AnalysisBasis::DryStatic,
                    InputUnits::default(),
                )?;
                self.circular_native_release_report_presentation();
                self.set_status(cx,"Новый case: полный стандартный CircularAnalysisConfig preset; DryStatic/SI; задайте собственную окружность");
            }
            RecordLoadMode::Case => {
                let (id, source) = ticket.case.ok_or("MISSING_CASE_LOAD_ID")?;
                let case = doc.circular_case(id).ok_or("UNKNOWN_CASE")?;
                if case.source_operation_id != source
                    || ticket.case_config != Some(case.definition.config)
                {
                    return Err("STALE_LOAD_CASE".into());
                }
                let case_view = CircularCaseEditorProjection::view(case);
                case_view.serialized_bytes()?;
                bounded(&case_view, SOURCE_BYTES)?;
                self.circular_ui.panel.load_case(
                    &self.ui,
                    cx,
                    &ticket,
                    live,
                    ground,
                    Arc::new(CircularCaseEditorProjection::from_case(case)?),
                )?;
                self.circular_native_release_report_presentation();
                self.circular_native_bind_editor(cx)?;
            }
        }
        self.circular_native.current = None;
        self.circular_native.display_serialized_budget_used = 0;
        self.circular_invalidate_display();
        Ok(())
    }

    /// Explicit editor replacement revokes the old report's native display/save
    /// capability. A running formatter remains owned until it actually returns;
    /// the pending archive uses the existing Busy/reap/retry path. Host jobs and
    /// their mathematical evidence are unaffected.
    fn circular_native_release_report_presentation(&mut self) {
        if self.circular_report.key().is_some() {
            self.circular_report.cancel();
        }
        self.circular_report_ui = Default::default();
    }

    /// Pure owner resolve + bounded archived tuple hash. No PreparedMass/slices.
    pub(super) fn circular_native_bind_editor(&mut self, cx: &mut Cx) -> Result<(), String> {
        let context = self.circular_ui.panel.editor_context()?;
        let Some((case_id, source)) = context.case_source else {
            return Ok(());
        };
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let case = doc.circular_case(case_id).ok_or("UNKNOWN_CASE")?;
        let ground = doc
            .ground_section(case.definition.ground_section_id)
            .ok_or("UNKNOWN_GROUND")?;
        if source != case.source_operation_id
            || context.ground_source != Some((ground.ground_section_id, ground.source_operation_id))
        {
            return Err("STALE_EDITOR_SOURCE".into());
        }
        bounded(&(&ground.definition, &case.definition), SOURCE_BYTES)?;
        if !doc
            .ground_readiness(ground.ground_section_id)
            .map_err(message)?
            .is_ready()
        {
            return self.circular_native_request_archive(cx);
        }
        let resolved = doc.resolve_circular_editor_input(case_id).map_err(message)?;
        let fingerprint = geotech_report::archived_input_digest(
            CIRCULAR_SOLVER_VERSION,
            &resolved.input,
            &resolved.config,
            source_limits(),
        )
        .map_err(|error| format!("REPORT_INPUT_DIGEST: {error:?}"))?;
        self.circular_ui
            .panel
            .bind_resolved(&context, Arc::new(resolved), fingerprint)?;
        if case.definition.search_spec.is_some() {
            let resolved = doc.resolve_circular_editor_search(case_id).map_err(message)?;
            bounded(&resolved, SOURCE_BYTES)?;
            let context = self.circular_ui.panel.editor_context()?;
            self.circular_ui
                .panel
                .set_search_request_template(&self.ui, cx, &context, resolved)?;
        }
        self.circular_native_request_archive(cx)
    }

    /// Shared API/native cache entry point. Integrity verification happens only
    /// on the existing owned report worker, never on an API/panel callback.
    pub(super) fn circular_archive_status(
        &mut self,
        id: ai_revit_core::AnalysisCaseId,
        require_current: bool,
    ) -> Result<crate::geotech_circular_archive_view::ArchivePresentation, String> {
        use crate::geotech_circular_archive_view::ArchiveState;
        use crate::geotech_circular_reportworker::ReportPhase;
        let ctx = self.circular_context();
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        if doc.circular_case(id).is_none() { return Err("UNKNOWN_CIRCULAR_CASE".into()); }
        let same = self.circular_report.archive_key().is_some_and(|key| key.analysis_id == id && key.matches_document(doc, &ctx));
        if same {
            if let Some(ready) = self.circular_report.archive_current(doc, &ctx) {
                if require_current && ready.presentation.state == ArchiveState::ArchivedStale {
                    return Err("ARCHIVED_RESULT_STALE".into());
                }
                return Ok(ready.presentation.clone());
            }
            return Err(match self.circular_report.phase() {
                ReportPhase::Running => "ARCHIVE_INSPECTION_PENDING",
                _ => "ARCHIVE_INSPECTION_FAILED",
            }.into());
        }
        // A current job/report keeps its real authority and retained cache.
        // An explicit historical read can retry once the owned worker is free.
        if self.circular_ui.panel.current_result().is_some() || self.circular_ui.panel.current_search_result().is_some()
            || (self.circular_report.key().is_some() && matches!(self.circular_report.phase(), ReportPhase::Running | ReportPhase::Completed)) {
            return Err("ARCHIVE_INSPECTION_DEFERRED_CURRENT_CALCULATED".into());
        }
        self.circular_report.start_archive(doc, &ctx, id)
            .map_err(|error| match error {
                crate::geotech_circular_reportworker::ReportWorkError::Busy => "ARCHIVE_INSPECTION_BUSY".into(),
                _ => format!("ARCHIVE_INSPECTION_REJECTED: {error:?}"),
            })?;
        Err("ARCHIVE_INSPECTION_PENDING".into())
    }

    fn circular_native_request_archive(&mut self, cx: &mut Cx) -> Result<(), String> {
        let Some(context) = self.circular_ui.panel.archive_load_context() else { return Ok(()); };
        let Some((analysis_id, _)) = context.case_source else { return Ok(()); };
        let request = self.circular_archive_status(analysis_id, false);
                // Polling adopts only the exact requested worker sequence and
                // repeats the local draft/source/session fence before display.
                let ctx = self.circular_context();
                let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
                let matches = self.circular_report.archive_key().is_some_and(|key| key.analysis_id == analysis_id && key.matches_document(doc, &ctx));
                if matches {
                    self.circular_native.archive = Some(CircularNativeArchiveBinding {
                        context, analysis_id, sequence: self.circular_report.sequence(),
                    });
                    self.circular_native_poll_archive(cx);
                } else {
                    let error = request.err().unwrap_or_else(|| "ARCHIVE_INSPECTION_NOT_ADMITTED".into());
                    if error == "ARCHIVE_INSPECTION_BUSY" {
                        self.circular_native.archive = Some(CircularNativeArchiveBinding {
                            context, analysis_id, sequence: 0,
                        });
                    } else {
                        self.circular_ui.panel.archive_load_failed(&context, &error);
                        self.circular_ui.panel.refresh(&self.ui, cx);
                    }
                }
        Ok(())
    }

    fn circular_native_poll_archive(&mut self, cx: &mut Cx) {
        use crate::geotech_circular_reportworker::ReportPhase;
        let Some(mut pending) = self.circular_native.archive.take() else { return; };
        if self.circular_ui.panel.archive_load_context().is_none_or(|context| {
            context.load_generation != pending.context.load_generation
                || context.draft_generation != pending.context.draft_generation
                || !same_epoch(&context.document, &pending.context.document)
                || context.ground_source != pending.context.ground_source
                || context.case_source != pending.context.case_source
        }) { return; }
        if pending.sequence == 0 {
            match self.circular_archive_status(pending.analysis_id, false) {
                Ok(_) => pending.sequence = self.circular_report.sequence(),
                Err(error) if error == "ARCHIVE_INSPECTION_PENDING" => pending.sequence = self.circular_report.sequence(),
                Err(error) if error == "ARCHIVE_INSPECTION_BUSY" => {
                    self.circular_native.archive = Some(pending);
                    return;
                }
                Err(error) => {
                    self.circular_ui.panel.archive_load_failed(&pending.context, &error);
                    self.circular_ui.panel.refresh(&self.ui, cx);
                    return;
                }
            }
        }
        let ctx = self.circular_context();
        let Some(doc) = self.document.as_ref() else { return; };
        if pending.sequence != self.circular_report.sequence()
            || self.circular_report.archive_key().is_none_or(|key| key.analysis_id != pending.analysis_id || !key.matches_document(doc, &ctx)) { return; }
        if let Some(saved) = self.circular_report.archive_current(doc, &ctx).cloned() {
            let review = match saved.snapshot.as_ref().map(|snapshot| &snapshot.archive().bundle.evidence) {
                Some(geotech_report::ReportEvidence::Specified { config, result, .. }) => report_review(result, config),
                _ => ResultReview::default(),
            };
            if self.circular_ui.panel.accept_archive(&pending.context, saved, review).is_ok() {
                self.circular_native.archive_view_key = self.circular_report.archive_key().cloned();
                self.circular_invalidate_display();
                self.refresh_circular(cx);
            }
        } else if self.circular_report.phase() == ReportPhase::Running {
            self.circular_native.archive = Some(pending);
        } else {
            self.circular_ui.panel.archive_load_failed(&pending.context, "ARCHIVE_INVALID / INSPECTION_FAILED");
            self.circular_ui.panel.refresh(&self.ui, cx);
        }
    }

    /// Metadata-only invalidation before draw/mutation refresh. The retained
    /// view cannot survive a runtime change or same-source archive replacement.
    pub(super) fn circular_native_validate_archive_view(&mut self) {
        let ctx = self.circular_context();
        let valid = self.circular_native.archive_view_key.as_ref().is_none_or(|key| {
            self.document.as_ref().is_some_and(|doc| key.matches_document(doc, &ctx))
        });
        if !valid {
            self.circular_native.archive_view_key = None;
            self.circular_ui.panel.invalidate_archive_view();
        }
    }

    /// Root calls after a fresh bounded catalog on Open. A single identified
    /// archive may load automatically; local drafts and multiple cases remain
    /// explicit choices. This performs no calculation or report formatting.
    pub(super) fn circular_native_restore_archive_after_open(&mut self, cx: &mut Cx) -> Result<(), String> {
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let selected = self.circular_ui.panel.catalog_record_ids().1;
        let mut archives = doc.circular_cases().values().filter(|case| case.circular_snapshot.is_some());
        let first = archives.next().map(|case| case.analysis_id);
        let multiple = archives.next().is_some();
        let chosen = selected.filter(|id| doc.circular_case(*id).is_some_and(|case| case.circular_snapshot.is_some()))
            .or_else(|| (!multiple).then_some(first).flatten());
        let Some(id) = chosen else {
            if multiple { self.set_status(cx, "Несколько сохранённых расчётов: выберите case и нажмите «Загрузить case»"); }
            return Ok(());
        };
        let Some(ticket) = self.circular_ui.panel.archive_autoload_ticket(id)? else {
            self.set_status(cx, "Сохранённый расчёт найден; локальный черновик сохранён. Загрузите case явно для просмотра архива");
            return Ok(());
        };
        self.circular_native_load(cx, ticket)
    }

    fn circular_native_save_ground(
        &mut self,
        cx: &mut Cx,
        proposal: SaveGroundProposal,
    ) -> Result<(), String> {
        self.circular_native_require_context(&proposal.context)?;
        if proposal.expected != proposal.context.document {
            return Err("GROUND_SAVE_CONTEXT_MISMATCH".into());
        }
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        match (
            &proposal.command,
            proposal.expected_ground,
            proposal.before.as_deref(),
        ) {
            (EngineeringCommand::CreateGround { .. }, None, None) => {}
            (
                EngineeringCommand::UpdateGround {
                    ground_section_id, ..
                },
                Some((id, source)),
                Some(before),
            ) if *ground_section_id == id
                && before.ground_section_id == id
                && before.source_operation_id == source
                && doc.ground_section(id) == Some(before) => {}
            _ => return Err("GROUND_SAVE_SOURCE_MISMATCH".into()),
        }
        let (command_id, receipt) = self.circular_native_command(
            cx,
            "circular-ground-save",
            &proposal.expected,
            &proposal.command,
        )?;
        let id = match &proposal.command {
            EngineeringCommand::CreateGround { .. } => *receipt
                .delta
                .engineering
                .created_ground_sections
                .first()
                .filter(|_| receipt.delta.engineering.created_ground_sections.len() == 1)
                .ok_or("GROUND_RECEIPT_OWNER_MISSING")?,
            EngineeringCommand::UpdateGround {
                ground_section_id, ..
            } => *ground_section_id,
            _ => return Err("GROUND_RECEIPT_COMMAND_MISMATCH".into()),
        };
        let live = self.circular_binding()?;
        let ground = self
            .document
            .as_ref()
            .and_then(|d| d.ground_section(id))
            .ok_or("GROUND_NOT_CURRENT_AFTER_COMMIT")?;
        bounded(ground, SOURCE_BYTES)?;
        self.circular_ui.panel.accept_ground_saved(
            &self.ui,
            cx,
            &proposal,
            &command_id,
            &receipt,
            live,
            Arc::new(ground.clone()),
        )?;
        self.circular_refresh_catalog(cx)?;
        self.circular_native_bind_editor(cx)?;
        self.circular_invalidate_display();
        Ok(())
    }

    fn circular_native_save_case(
        &mut self,
        cx: &mut Cx,
        proposal: SaveCaseProposal,
        reload: bool,
    ) -> Result<(), String> {
        self.circular_native_require_context(&proposal.context)?;
        if proposal.expected != proposal.context.document {
            return Err("CASE_SAVE_CONTEXT_MISMATCH".into());
        }
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let ground = doc
            .ground_section(proposal.expected_ground.0)
            .ok_or("UNKNOWN_GROUND")?;
        if ground != proposal.referenced_ground.as_ref()
            || ground.source_operation_id != proposal.expected_ground.1
        {
            return Err("CASE_SAVE_GROUND_MISMATCH".into());
        }
        match (
            &proposal.command,
            proposal.expected_case,
            proposal.before.as_deref(),
        ) {
            (EngineeringCommand::CreateCircular { definition, .. }, None, None)
                if definition.ground_section_id == ground.ground_section_id
                    && definition.search_spec.is_none() => {}
            (
                EngineeringCommand::UpdateCircular {
                    analysis_id,
                    definition,
                    name,
                },
                Some((id, source)),
                Some(before),
            ) if *analysis_id == id
                && before.analysis_id == id
                && before.source_operation_id == source
                && doc
                    .circular_case(id)
                    .is_some_and(|case| before.matches_case(case))
                && definition.ground_section_id == ground.ground_section_id =>
            {
                // Search policy is a separate explicit save. Ordinary circle or
                // config edits preserve it; a policy-only proposal preserves all
                // other saved definition fields and the saved name.
                if definition.search_spec != before.definition.search_spec {
                    let mut policy_only = before.definition.clone();
                    policy_only.search_spec = definition.search_spec.clone();
                    if definition != &policy_only || name.is_some() {
                        return Err("SEARCH_POLICY_SAVE_MUST_BE_EXPLICIT_AND_SEPARATE".into());
                    }
                }
            }
            _ => return Err("CASE_SAVE_SOURCE_MISMATCH".into()),
        }
        let (command_id, receipt) = self.circular_native_command(
            cx,
            "circular-case-save",
            &proposal.expected,
            &proposal.command,
        )?;
        let id = match &proposal.command {
            EngineeringCommand::CreateCircular { .. } => *receipt
                .delta
                .engineering
                .created_cases
                .first()
                .filter(|_| receipt.delta.engineering.created_cases.len() == 1)
                .ok_or("CASE_RECEIPT_OWNER_MISSING")?,
            EngineeringCommand::UpdateCircular { analysis_id, .. } => *analysis_id,
            _ => return Err("CASE_RECEIPT_COMMAND_MISMATCH".into()),
        };
        let live = self.circular_binding()?;
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let case = doc
            .circular_case(id)
            .ok_or("CASE_NOT_CURRENT_AFTER_COMMIT")?;
        let ground = doc
            .ground_section(case.definition.ground_section_id)
            .ok_or("GROUND_NOT_CURRENT_AFTER_COMMIT")?;
        let case_view = CircularCaseEditorProjection::view(case);
        case_view.serialized_bytes()?;
        bounded(&(ground, &case_view), SOURCE_BYTES)?;
        let ground = Arc::new(ground.clone());
        let case = Arc::new(CircularCaseEditorProjection::from_case(case)?);
        self.circular_ui.panel.accept_case_saved(
            &self.ui,
            cx,
            &proposal,
            &command_id,
            &receipt,
            live.clone(),
            ground.clone(),
            case.clone(),
        )?;
        if reload {
            // Applying a candidate must hydrate its actual full circle fields,
            // otherwise a later ordinary save could restore the previous text.
            let ticket = RecordLoadTicket {
                context: self.circular_ui.panel.editor_context()?,
                mode: RecordLoadMode::Case,
                ground: (ground.ground_section_id, ground.source_operation_id),
                case: Some((case.analysis_id, case.source_operation_id)),
                case_config: Some(case.definition.config),
            };
            self.circular_ui
                .panel
                .load_case(&self.ui, cx, &ticket, live, ground, case)?;
        }
        self.circular_refresh_catalog(cx)?;
        self.circular_native_bind_editor(cx)?;
        self.circular_invalidate_display();
        Ok(())
    }

    /// Root supplies owner acceptance explicitly; this never grants review.
    pub(super) fn circular_native_set_gates(
        &mut self,
        analyze: bool,
        search: bool,
    ) -> Result<(), String> {
        self.circular_native.analysis_admitted = analyze;
        self.circular_native.search_admitted = search;
        self.circular_native_refresh_gates()
    }
    fn circular_native_refresh_gates(&mut self) -> Result<(), String> {
        if let Ok(context) = self.circular_ui.panel.editor_context() {
            self.circular_ui.panel.set_host_gates(
                &context,
                self.circular_native.analysis_admitted,
                self.circular_native.search_admitted,
            )?;
        }
        Ok(())
    }
    pub(super) fn circular_native_set_search_draft(
        &mut self,
        cx: &mut Cx,
        context: &EditorContext,
        request: SearchPublicRequestV1,
    ) -> Result<(), String> {
        self.circular_native_require_context(context)?;
        bounded(&request, SOURCE_BYTES)?;
        self.circular_ui
            .panel
            .set_search_draft_template(&self.ui, cx, context, request)
    }

    /// Explicit user action only. The public builder contributes its documented
    /// full policy; singleton bounds use the actual saved circle, no invented
    /// geometry and no automatically saved/admitted/frozen search.
    pub(super) fn circular_native_prepare_basic_search_draft(
        &mut self,
        cx: &mut Cx,
        context: &EditorContext,
    ) -> Result<(), String> {
        self.circular_native_require_context(context)?;
        if self.circular_ui.panel.dirty || self.circular_ui.panel.stale {
            return Err("SAVE_CURRENT_CASE_BEFORE_SEARCH_PRESET".into());
        }
        let (case_id, case_source) = context.case_source.ok_or("SEARCH_CASE_NOT_SAVED")?;
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let case = doc.circular_case(case_id).ok_or("UNKNOWN_CASE")?;
        let ground = doc
            .ground_section(case.definition.ground_section_id)
            .ok_or("UNKNOWN_GROUND")?;
        if case.source_operation_id != case_source
            || context.ground_source != Some((ground.ground_section_id, ground.source_operation_id))
            || context.loaded_config != Some(case.definition.config)
        {
            return Err("STALE_SEARCH_PRESET_SOURCE".into());
        }
        bounded(&(&ground.definition, &case.definition), SOURCE_BYTES)?;
        let resolved = doc.resolve_circular_editor_input(case_id).map_err(message)?;
        bounded(&resolved, SOURCE_BYTES)?;
        let circle = &resolved.input.slip_surface;
        let axis = |value| InclusiveAxis {
            min: value,
            max: value,
            intervals: 0,
        };
        let domain = SearchDomain {
            center_x: axis(circle.center.x),
            center_z: axis(circle.center.z),
            radius: axis(circle.radius_m),
            allowed_entry_x: None,
            allowed_exit_x: None,
        };
        let mut request =
            SearchPublicRequestV1::from_specified(&resolved.input, &resolved.config, domain);
        // Explicit named root preset: only these public work ceilings differ
        // from the builder. Case limits and the 8 MiB output cap do not change.
        request.max_candidates = 1000;
        request.max_wall_time_ms = 120_000;
        let exploration = request.exploration_slices;
        let shift = exploration
            .max_passes
            .checked_sub(1)
            .and_then(|v| 1_u32.checked_shl(v))
            .ok_or("SEARCH_PRESET_EXPLORATION_OVERFLOW")?;
        let maximum_exploration = exploration
            .initial_requested
            .checked_mul(shift)
            .ok_or("SEARCH_PRESET_EXPLORATION_OVERFLOW")?;
        let pair = request.final_pair;
        if exploration.initial_requested == 0
            || !(2..=16).contains(&exploration.max_passes)
            || !exploration.require_joint_consecutive_convergence
            || pair.coarse_requested < maximum_exploration
            || pair.coarse_requested.checked_mul(2) != Some(pair.fine_requested)
            || pair.fine_requested as usize > resolved.config.limits.max_slices
        {
            return Err("SEARCH_PRESET_EXCEEDS_ACTUAL_CASE_SLICE_LIMITS: edit the full case config or choose an explicit policy; no limits changed".into());
        }
        self.circular_native_set_search_draft(cx, context, request)?;
        self.set_status(cx,"Базовые настройки поиска: одна сохранённая окружность; потолок 1000 candidates / 120 s; output cap 8 MiB может отклонить большой результат; полный policy показан в панели; сохранение и заморозка — отдельные действия");
        Ok(())
    }

    #[cfg(target_os="linux")]
    pub(crate) fn gui_job_native_view_currentness(&self)->serde_json::Value {
        let ctx=self.circular_context();
        if let(Some(doc),Some(key))=(self.document.as_ref(),self.circular_native.archive_view_key.as_ref()){
            if key.matches_document(doc,&ctx){
                let source=match doc.circular_snapshot_currentness(key.analysis_id){Ok(Some(geotech_report::Currentness::Current))=>"CURRENT",
                    Ok(Some(geotech_report::Currentness::Stale))=>"STALE",_=>"UNKNOWN"};
                return serde_json::json!({"kind":"SAVED_ARCHIVE_VIEW","source_currentness":source,"case_id":key.analysis_id.get(),
                    "display_verified_by_owned_archive_inspector":true,"current_compute_capability":false});
            }
        }
        if let Some(current)=self.circular_native.current.as_ref().filter(|current|current.delivered&&self.circular_ui.panel.current_result().is_some()){
            let same=self.circular_ui.panel.editor_context().is_ok_and(|editor|editor.resolved_identity.as_ref().is_some_and(|identity|
                same_identity(identity,&current.ticket.identity)))&&!self.circular_ui.panel.dirty&&!self.circular_ui.panel.stale;
            return serde_json::json!({"kind":"HOST_RESULT_DISPLAY","source_currentness":if same{"CURRENT"}else{"STALE"},
                "case_id":current.ticket.identity.analysis_id.get(),"native_delivered":current.delivered,
                "current_compute_capability":false,"authority":"REQUIRES_SEPARATE_OWNED_JOB_CURRENT_RESULT_READBACK"});
        }
        serde_json::json!({"kind":"NO_VERIFIED_RESULT_VIEW","source_currentness":"UNKNOWN","current_compute_capability":false})
    }

    fn circular_native_start(
        &mut self,
        ticket: JobTicket,
        search: Option<SearchBox>,
    ) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        let gui_job_profile=self.gui_persistence_enabled();
        #[cfg(target_os = "linux")]
        if gui_job_profile {
            crate::linux_geotech_gui::job_native_start(&mut self.gui_hooks,&ticket,search.as_ref())?;
        }
        let outcome: Result<(), String> = (|| {
            let context = self.circular_ui.panel.editor_context()?;
            let identity = context
                .resolved_identity
                .as_ref()
                .ok_or("NATIVE_SOURCE_NOT_RESOLVED")?;
            if !same_identity(identity, &ticket.identity)
                || !same_epoch(&self.circular_binding()?, &ticket.identity.document)
                || (ticket.kind == JobKind::Search
                    && context.search_generation != ticket.search_generation)
                || (ticket.kind == JobKind::Analyze && !self.circular_native.analysis_admitted)
                || (ticket.kind == JobKind::Search && !self.circular_native.search_admitted)
            {
                return Err("STALE_NATIVE_JOB_OR_HOST_GATE_NOT_ADMITTED".into());
            }
            self.circular_ensure_host()
                .map_err(|e| message(e.message))?;
            let ctx = self.circular_context();
            let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
            let controller = self.circular_host.as_mut().ok_or("HOST_NOT_AVAILABLE")?;
            let envelope = match ticket.kind {
                JobKind::Analyze => {
                    if search.is_some() || ticket.search_spec.is_some() {
                        return Err("SPECIFIED_MODE_MISMATCH".into());
                    }
                    controller.quote_specified(
                        doc,
                        &ctx,
                        host::DecimalU64(ticket.identity.analysis_id.get()),
                    )
                }
                JobKind::Search => controller.quote_search(
                    doc,
                    &ctx,
                    host::DecimalU64(ticket.identity.analysis_id.get()),
                ),
            }
            .map_err(message)?;
            if envelope.binding.ground_id.0 != ticket.identity.ground_id.get()
                || envelope.binding.ground_source_operation.0
                    != ticket.identity.ground_source_operation_id.get()
                || envelope.binding.case_source_operation.0
                    != ticket.identity.case_source_operation_id.get()
                || envelope.resolved_case_config != ticket.identity.config
                || envelope.binding.solver_source_fingerprint != ticket.identity.source_fingerprint
                || envelope.binding.solver_version != ticket.identity.solver_version
            {
                return Err("NATIVE_TICKET_SOURCE_MISMATCH".into());
            }
            match (&envelope.request, ticket.kind) {
                (host::FullRequest::Specified { input, config }, JobKind::Analyze) => {
                    let fingerprint = geotech_report::archived_input_digest(
                        &envelope.binding.solver_version,
                        input,
                        config,
                        source_limits(),
                    )
                    .map_err(|error| format!("REPORT_INPUT_DIGEST: {error:?}"))?;
                    if fingerprint != ticket.identity.input_fingerprint {
                        return Err("NATIVE_SPECIFIED_INPUT_MISMATCH".into());
                    }
                }
                (host::FullRequest::Search(actual), JobKind::Search) => {
                    let frozen = search.as_ref().ok_or("MISSING_FULL_FROZEN_SEARCH")?;
                    if ticket.search_spec.as_ref() != Some(frozen)
                        || actual != &frozen.request
                        || frozen.analysis_config != envelope.resolved_case_config
                    {
                        return Err("NATIVE_SAVED_SEARCH_SPEC_MISMATCH".into());
                    }
                }
                _ => return Err("NATIVE_SUBMISSION_MODE_MISMATCH".into()),
            }
            bounded(&envelope, SOURCE_BYTES)?;
            controller.after_mutation(doc, &ctx);
            if let Some(old) = self.circular_native.current.as_ref() {
                if controller
                    .status(doc, &ctx, old.host_sequence)
                    .is_ok_and(|s| s.worker_owned)
                {
                    return Err("BUSY_NATIVE_WORKER_NOT_REAPED".into());
                }
            }
            #[cfg(target_os = "linux")]
            if gui_job_profile {crate::linux_geotech_gui::job_submit_before(&mut self.gui_hooks,&ticket,&envelope)?;}
            let submitted = controller.submit(doc, &ctx, &envelope);
            #[cfg(target_os = "linux")]
            if gui_job_profile {crate::linux_geotech_gui::job_submit_returned(&mut self.gui_hooks,&ticket,&envelope,submitted.as_ref().ok())?;}
            let status = submitted.map_err(message)?;
            self.circular_native.current = Some(CircularNativeJobBinding {
                ticket: ticket.clone(),
                host_sequence: status.sequence,
                submission: envelope,
                draft_generation: context.draft_generation,
                load_generation: context.load_generation,
                delivered: false,
                cancel_requested: false,
                last_progress: 0,
            });
            self.circular_native.display_serialized_budget_used = 0;
            Ok(())
        })();
        self.circular_validate_report_compute();
        if let Err(error) = &outcome {
            self.circular_ui.panel.finish_error(&ticket, error);
        }
        outcome
    }

    /// Exact retained original admission: never quote/regenerate on retry.
    pub(super) fn circular_native_retry(&mut self) -> Result<host::HostStatus, String> {
        #[cfg(target_os = "linux")]
        if self.gui_persistence_enabled(){return Err("GUI_JOB_RETRY_NOT_ADMITTED".into());}
        let ctx = self.circular_context();
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let current = self
            .circular_native
            .current
            .as_ref()
            .ok_or("NATIVE_JOB_GONE")?;
        let outcome = self
            .circular_host
            .as_mut()
            .ok_or("HOST_NOT_AVAILABLE")?
            .submit(doc, &ctx, &current.submission)
            .map_err(message);
        self.circular_validate_report_compute();
        outcome
    }
    /// Lookup is metadata only. Callers must additionally check current_result
    /// with actual Document/HostContext before borrowing/copying saved evidence.
    pub(super) fn circular_native_sequence(
        &self,
        ticket: &JobTicket,
    ) -> Result<host::DecimalU64, String> {
        self.circular_native
            .current
            .as_ref()
            .filter(|v| &v.ticket == ticket)
            .map(|v| v.host_sequence)
            .ok_or_else(|| "NATIVE_TICKET_NOT_BOUND".into())
    }

    /// Report worker handoff: metadata lookup PLUS current owner/cache access.
    /// This grants no report/math/engineering acceptance and performs no copy.
    pub(super) fn circular_native_report_sequence(
        &mut self,
        ticket: &ReportTicket,
    ) -> Result<host::DecimalU64, String> {
        let editor = self.circular_ui.panel.editor_context()?;
        let current = self
            .circular_native
            .current
            .as_ref()
            .ok_or("NATIVE_REPORT_JOB_GONE")?;
        if !current.delivered
            || current.cancel_requested
            || self.circular_ui.panel.dirty
            || self.circular_ui.panel.stale
            || editor.search_generation != ticket.search_generation
            || !same_identity(&current.ticket.identity, &ticket.identity)
            || !editor
                .resolved_identity
                .as_ref()
                .is_some_and(|i| same_identity(i, &ticket.identity))
            || (current.ticket.kind == JobKind::Search
                && ticket.candidate_source.as_ref() != Some(&current.ticket))
            || (current.ticket.kind == JobKind::Analyze && ticket.candidate_source.is_some())
        {
            return Err("STALE_NATIVE_REPORT_TICKET".into());
        }
        let sequence = current.host_sequence;
        let ctx = self.circular_context();
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let saved = self
            .circular_host
            .as_mut()
            .ok_or("HOST_NOT_AVAILABLE")?
            .current_result(doc, &ctx, sequence)
            .map_err(message)?;
        if !saved.worker_arithmetic.accepted || saved.canonical_bytes().len() > DISPLAY_BYTES {
            return Err("NATIVE_REPORT_SOURCE_BUDGET_OR_ENVIRONMENT".into());
        }
        Ok(sequence)
    }

    fn circular_native_apply_candidate(
        &mut self,
        cx: &mut Cx,
        expected: DocumentBinding,
        search: JobTicket,
        id: CandidateId,
    ) -> Result<(), String> {
        if search.kind != JobKind::Search
            || self.circular_binding()? != expected
            || self.circular_ui.panel.dirty
            || self.circular_ui.panel.stale
        {
            return Err("STALE_CANDIDATE_APPLY_CONTEXT".into());
        }
        let sequence = self.circular_native_sequence(&search)?;
        let ctx = self.circular_context();
        let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
        let circle = {
            let saved = self
                .circular_host
                .as_mut()
                .ok_or("HOST_NOT_AVAILABLE")?
                .current_result(doc, &ctx, sequence)
                .map_err(message)?;
            if !saved.worker_arithmetic.accepted || !saved.accepted_minimum() {
                return Err("CANDIDATE_NOT_ACCEPTED_MAIN_EVIDENCE".into());
            }
            let (request, result) = match (saved.request(), saved.output()) {
                (jobs::JobRequest::Search(request), jobs::JobOutput::Search(result)) => {
                    (request, result)
                }
                _ => return Err("CANDIDATE_NOT_FROM_SEARCH".into()),
            };
            if result.status != SearchRunStatus::CompletedMinimum
                || !result.accepted
                || result.diagnostic_only
                || result.request.public != *request
                || result.request.public.output_mode != SearchOutputMode::Strict
            {
                return Err("DIAGNOSTIC_OR_INCOMPLETE_CANDIDATE".into());
            }
            let candidate = result
                .candidates
                .iter()
                .find(|c| c.id == id)
                .ok_or("CANDIDATE_NOT_RETAINED")?;
            if !candidate.main_family_member
                || candidate.final_status != CandidateStatus::JointlyEligible
                || !result.examined_family.main_candidate_ids.contains(&id)
                || !result.examined_family.eligible_candidate_ids.contains(&id)
                || [&candidate.fellenius, &candidate.bishop].iter().any(|m| {
                    m.selected_family_eligibility.state != CheckState::Passed
                        || !m.accepted_factor.is_some_and(|f| f.is_finite() && f > 0.0)
                })
            {
                return Err("CANDIDATE_NOT_JOINT_MAIN_FAMILY".into());
            }
            let config = request
                .shared_analysis
                .fixed(request.final_pair.fine_requested);
            let table = result
                .retained_tables
                .iter()
                .find(|table| {
                    table.candidate_id == id
                        && table.branch == "main"
                        && table.key == candidate.key
                        && table.source_fingerprint == result.source_fingerprint
                        && table.config == config
                        && candidate.final_input_fingerprint.as_ref()
                            == Some(&table.evaluation.input_fingerprint)
                })
                .ok_or("CANDIDATE_FINE_TABLE_NOT_RETAINED")?;
            let bishop = table
                .evaluation
                .bishop
                .as_ref()
                .map_err(|_| "CANDIDATE_BISHOP_NOT_ACCEPTED")?;
            let arc = bishop
                .whole_arc_contact
                .as_ref()
                .ok_or("CANDIDATE_WHOLE_ARC_NOT_ASSESSED")?;
            if !table.evaluation.closure_passed
                || table.evaluation.fellenius.as_ref().is_err()
                || bishop.convergence_status != BishopConvergenceStatus::Converged
                || bishop.physical_assessment.status != BishopPhysicalStatus::Admissible
                || !bishop
                    .accepted_factor_of_safety
                    .is_some_and(|f| f.is_finite() && f > 0.0)
                || !bishop
                    .endpoint_assessment
                    .as_ref()
                    .is_some_and(|e| e.passed)
                || arc.status != WholeArcContactStatus::PassedDeclaredModel
                || !arc.coverage_complete
                || arc.source_fingerprint != result.source_fingerprint
                || arc.config != config.contact
                || arc.scope.is_empty()
                || arc.root_range_scope.is_empty()
                || arc.contact_input_fingerprint.is_empty()
            {
                return Err("CANDIDATE_PHYSICAL_MODEL_NOT_ACCEPTED".into());
            }
            let surface = &table.evaluation.geometry.circle.surface;
            if surface.center != candidate.center
                || surface.radius_m != candidate.radius_m
                || surface.slide_direction != request.direction
            {
                return Err("CANDIDATE_FULL_CIRCLE_BINDING_MISMATCH".into());
            }
            // Exact saved circle, including selected endpoints and direction.
            // Never reconstruct from summary or substitute the original circle.
            table.evaluation.geometry.circle.surface.clone()
        };
        let context = self.circular_ui.panel.editor_context()?;
        if !context
            .resolved_identity
            .as_ref()
            .is_some_and(|i| same_identity(i, &search.identity))
        {
            return Err("CANDIDATE_EDITOR_SOURCE_MISMATCH".into());
        }
        let case = doc
            .circular_case(search.identity.analysis_id)
            .ok_or("UNKNOWN_CASE")?;
        let ground = doc
            .ground_section(case.definition.ground_section_id)
            .ok_or("UNKNOWN_GROUND")?;
        let case_view = CircularCaseEditorProjection::view(case);
        case_view.serialized_bytes()?;
        bounded(&(&case_view, ground), SOURCE_BYTES)?;
        let mut definition = case.definition.clone();
        definition.slip_surface = circle;
        let proposal = SaveCaseProposal {
            generation: context.case_generation,
            ground_generation: context.ground_generation,
            context,
            expected,
            expected_case: Some((case.analysis_id, case.source_operation_id)),
            before: Some(Arc::new(CircularCaseEditorProjection::from_case(case)?)),
            referenced_ground: Arc::new(ground.clone()),
            expected_ground: (ground.ground_section_id, ground.source_operation_id),
            command: EngineeringCommand::UpdateCircular {
                analysis_id: case.analysis_id,
                definition,
                name: None,
            },
        };
        self.circular_native_save_case(cx, proposal, true)
    }

    /// Called by the EXISTING owner timer instead of its plain host poll. Manager
    /// poll remains the only worker completion/publication/reap path.
    pub(super) fn circular_native_poll_jobs(&mut self, cx: &mut Cx) -> Result<(), String> {
        self.circular_poll_jobs();
        self.circular_native_validate_archive_view();
        self.circular_native_poll_archive(cx);
        self.circular_native_refresh_gates()?;
        let Some(mut current) = self.circular_native.current.take() else {
            return Ok(());
        };
        let ctx = self.circular_context();
        let Some(doc) = self.document.as_ref() else {
            self.circular_ui
                .panel
                .discard_host_result(&current.ticket, "NO_DOCUMENT");
            self.circular_native.display_serialized_budget_used = 0;
            self.circular_invalidate_display();
            return Ok(());
        };
        let Some(controller) = self.circular_host.as_mut() else {
            self.circular_ui
                .panel
                .discard_host_result(&current.ticket, "HOST_NOT_AVAILABLE");
            self.circular_native.display_serialized_budget_used = 0;
            self.circular_invalidate_display();
            return Ok(());
        };
        let editor_current = self.circular_ui.panel.editor_context().is_ok_and(|editor| {
            editor.load_generation == current.load_generation
                && editor.draft_generation == current.draft_generation
                && editor
                    .resolved_identity
                    .as_ref()
                    .is_some_and(|i| same_identity(i, &current.ticket.identity))
                && (current.ticket.kind != JobKind::Search
                    || editor.search_generation == current.ticket.search_generation)
                && !self.circular_ui.panel.dirty
                && !self.circular_ui.panel.stale
        });
        if !editor_current && !current.cancel_requested {
            let _ = controller.cancel(doc, &ctx, current.host_sequence);
            current.cancel_requested = true;
        }
        let status = match controller.status(doc, &ctx, current.host_sequence) {
            Ok(status) => status,
            Err(error) => {
                self.circular_ui
                    .panel
                    .discard_host_result(&current.ticket, &message(error));
                self.circular_native.display_serialized_budget_used = 0;
                self.circular_invalidate_display();
                self.refresh_circular(cx);
                return Ok(());
            }
        };
        if current.cancel_requested
            || !editor_current
            || matches!(
                status.phase,
                jobs::JobPhase::Failed | jobs::JobPhase::Cancelled | jobs::JobPhase::Stale
            )
        {
            let detail = format!(
                "{:?}; {:?}; worker_owned={}",
                status.phase, status.failure, status.worker_owned
            );
            self.circular_ui
                .panel
                .discard_host_result(&current.ticket, &message(detail));
            self.circular_native.display_serialized_budget_used = 0;
            if status.worker_owned {
                self.circular_native.current = Some(current);
            }
            self.circular_invalidate_display();
            self.refresh_circular(cx);
            return Ok(());
        }
        if status.phase == jobs::JobPhase::Completed && !current.delivered {
            let copied = controller
                .current_result(doc, &ctx, current.host_sequence)
                .map_err(message)
                .and_then(display_copy);
            let accepted = match copied {
                Ok((DisplayCopy::Specified(result, review), bytes)) => self
                    .circular_ui
                    .panel
                    .accept_analysis(&current.ticket, result, review)
                    .map(|_| bytes),
                Ok((DisplayCopy::Search(result), bytes)) => self
                    .circular_ui
                    .panel
                    .accept_search_result(&current.ticket, result)
                    .map(|_| bytes),
                Err(error) => Err(error),
            };
            match accepted {
                Ok(bytes) => {
                    current.delivered = true;
                    self.circular_native.display_serialized_budget_used = bytes;
                }
                Err(error) => {
                    let _ = controller.cancel(doc, &ctx, current.host_sequence);
                    self.circular_ui
                        .panel
                        .discard_host_result(&current.ticket, &error);
                    self.circular_native.display_serialized_budget_used = 0;
                    self.circular_invalidate_display();
                    self.refresh_circular(cx);
                    return Err(error);
                }
            }
            self.circular_invalidate_display();
            self.refresh_circular(cx);
        } else if !current.delivered && current.last_progress != status.progress_updates.0 {
            current.last_progress = status.progress_updates.0;
            if let Some(progress) = status.progress {
                self.set_status(
                    cx,
                    &format!(
                        "Круговой поиск · {} · выполнено {} · уникальных {}",
                        progress.stage.chars().take(80).collect::<String>(),
                        progress.completed_evaluations,
                        progress.unique_candidates
                    ),
                );
            }
        }
        self.circular_native.current = Some(current);
        Ok(())
    }

    pub(super) fn circular_dispatch_ui_intent(
        &mut self,
        cx: &mut Cx,
        intent: CircularUiIntent,
    ) -> Result<(), String> {
        if self.circular_apply_view_intent(cx, &intent)? {
            self.circular_native_refresh_gates()?;
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        self.gui_job_intent_begin(cx,&intent)?;
        let outcome = match intent {
            CircularUiIntent::LoadGround(ticket) if ticket.mode == RecordLoadMode::Ground => {
                self.circular_native_load(cx, ticket)
            }
            CircularUiIntent::LoadCase(ticket) if ticket.mode == RecordLoadMode::Case => {
                self.circular_native_load(cx, ticket)
            }
            CircularUiIntent::NewCase(ticket) if ticket.mode == RecordLoadMode::NewCase => {
                self.circular_native_load(cx, ticket)
            }
            CircularUiIntent::LoadGround(_)
            | CircularUiIntent::LoadCase(_)
            | CircularUiIntent::NewCase(_) => Err("NATIVE_LOAD_MODE_MISMATCH".into()),
            CircularUiIntent::EditFullConfig { context, config } => {
                self.circular_native_require_context(&context)?;
                // Local draft only. User's separate SaveCase action performs
                // the shared atomic transaction and preserves saved search_spec.
                self.circular_ui
                    .panel
                    .set_case_config(&self.ui, cx, &context, config)
            }
            CircularUiIntent::EditSearchPolicy { context, request } => {
                self.circular_native_require_context(&context)?;
                match request {
                    Some(request) => self.circular_native_set_search_draft(cx, &context, request),
                    None => self.circular_native_prepare_basic_search_draft(cx, &context),
                }
            }
            CircularUiIntent::SaveGround(proposal) => {
                self.circular_native_save_ground(cx, proposal)
            }
            CircularUiIntent::SaveCase(proposal) => {
                self.circular_native_save_case(cx, proposal, false)
            }
            CircularUiIntent::Analyze(ticket) => self.circular_native_start(ticket, None),
            CircularUiIntent::Search(ticket, search) => {
                self.circular_native_start(ticket, Some(search))
            }
            CircularUiIntent::Cancel(ticket) => {
                let sequence = self.circular_native_sequence(&ticket)?;
                let ctx = self.circular_context();
                let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
                let controller = self.circular_host.as_mut().ok_or("HOST_NOT_AVAILABLE")?;
                controller.after_mutation(doc, &ctx);
                let outcome = controller.cancel(doc, &ctx, sequence).map_err(message);
                self.circular_validate_report_compute();
                outcome?;
                if let Some(binding) = self.circular_native.current.as_mut() {
                    binding.cancel_requested = true;
                }
                Ok(())
            }
            CircularUiIntent::PreviewCircle(circle) => {
                let context = self.circular_ui.panel.editor_context()?;
                self.circular_ui.panel.set_circle_preview(&context, circle)
            }
            CircularUiIntent::SelectSlice(id) => {
                let context = self.circular_ui.panel.editor_context()?;
                self.circular_ui.panel.select_slice(&context, id)
            }
            CircularUiIntent::SelectCandidate { search, id } => {
                self.circular_native_sequence(&search)?;
                let context = self.circular_ui.panel.editor_context()?;
                self.circular_ui
                    .panel
                    .select_candidate(&context, &search, id)
            }
            CircularUiIntent::ApplyCandidate {
                expected,
                search,
                id,
            } => self.circular_native_apply_candidate(cx, expected, search, id),
            CircularUiIntent::Report(ticket) => self.circular_start_report(cx, ticket),
            CircularUiIntent::SaveSnapshot(ticket) => self.circular_save_owned_snapshot(cx, ticket),
            CircularUiIntent::ViewChanged | CircularUiIntent::Fit => Ok(()),
        };
        #[cfg(target_os = "linux")]
        self.gui_job_intent_end(cx,outcome.is_ok());
        self.circular_native_refresh_gates()?;
        self.refresh_circular(cx);
        outcome
    }
}
