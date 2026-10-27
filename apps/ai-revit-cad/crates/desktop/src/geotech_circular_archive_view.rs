//! Read-only archived evidence projection. Hash/report verification belongs to
//! the existing owned report worker; navigation reads this immutable cache.
//! No solver, current-job receipt, persistence authority or engineering approval.
use crate::circular_host::HostContext;
use ai_revit_core::{
    CircularCalculationSnapshot, CircularSnapshotInspection, Document,
    engineering::AnalysisCaseId,
};
use geotech_core::circular::{
    BishopConvergenceStatus, BishopPhysicalStatus, CIRCULAR_SOLVER_VERSION,
    SearchConclusion, SearchRunStatus, WholeArcContactStatus, source_fingerprint,
};
use geotech_report::{Currentness, ReportEvidence};
use serde::Serialize;

pub(super) const ARCHIVE_COPY_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum ArchiveState {
    NoResult,
    ArchivedCurrent,
    ArchivedStale,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ArchivePresentation {
    pub state: ArchiveState,
    pub origin: &'static str,
    pub calculated_in_this_runtime: bool,
    pub current_job_result: bool,
    pub archive: Option<ArchiveDescriptor>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ArchiveDescriptor {
    pub project_id: String,
    pub case_id: String,
    pub ground_id: String,
    pub captured_global_revision: String,
    pub ground_source_operation_id: String,
    pub case_source_operation_id: String,
    pub source_revision: String,
    pub calculation_fingerprint: String,
    pub archived_solver_version: String,
    pub archived_solver_source_fingerprint: String,
    pub current_solver_version: &'static str,
    pub current_solver_source_fingerprint: String,
    pub archive_digest: String,
    pub report_digest: String,
    pub saved_calculation_worker_arithmetic_accepted: Option<bool>,
    pub saved_calculation_worker_arithmetic_scope: Option<String>,
    pub result: StoredSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum StoredSummary {
    Specified {
        slice_count: String,
        area_m2: f64,
        weight_kn_per_m: f64,
        fellenius_fs: f64,
        bishop_equation_fs: f64,
        bishop_accepted_fs: Option<f64>,
        bishop_equation_status: BishopConvergenceStatus,
        bishop_equation_residual: f64,
        finite_contact_status: BishopPhysicalStatus,
        minimum_normal_kn_per_m: f64,
        tensile_slice_count: String,
        indeterminate_slice_count: String,
        core_accepted: bool,
        diagnostic_only: bool,
        numerically_verified: bool,
        engineering_review_required: bool,
    },
    Search {
        status: SearchRunStatus,
        conclusion: SearchConclusion,
        core_accepted: bool,
        diagnostic_only: bool,
        candidate_count: String,
        retained_table_count: String,
        fellenius_minimum: Option<MinimumSummary>,
        bishop_minimum: Option<MinimumSummary>,
        classification: &'static str,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct MinimumSummary {
    pub candidate_id: String,
    pub factor_of_safety: f64,
    pub table_hash: String,
    pub evaluation_key: String,
    pub retained_table_available: bool,
    pub bishop_equation_status: Option<BishopConvergenceStatus>,
    pub finite_contact_status: Option<BishopPhysicalStatus>,
    pub whole_arc_status: Option<WholeArcContactStatus>,
    pub minimum_normal_kn_per_m: Option<f64>,
}

/// Immutable display data only. Native views borrow the exact stored evidence
/// through an Arc; no PublishedResult or fake computation sequence is created.
#[derive(Debug)]
pub(super) struct ArchiveInspection {
    pub analysis_id: AnalysisCaseId,
    pub presentation: ArchivePresentation,
    pub snapshot: Option<CircularCalculationSnapshot>,
    pub report_preview: String,
}

/// Owner metadata fence. Capturing/comparing it never hashes, resolves numerical
/// inputs, formats reports or clones an archive/Document/history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArchiveKey {
    pub analysis_id: AnalysisCaseId,
    pub archive_digest: Option<String>,
    document_uuid: String,
    runtime_id: String,
    session_id: String,
    open_generation: u64,
    ground_id: u64,
    ground_source_operation: Option<u64>,
    case_source_operation: u64,
    captured_owner_revision: u64,
}

impl ArchiveKey {
    pub(super) fn capture(doc: &Document, ctx: &HostContext, id: AnalysisCaseId) -> Result<Self, String> {
        let case = doc.circular_case(id).ok_or("UNKNOWN_CIRCULAR_CASE")?;
        let ground_id = case.definition.ground_section_id;
        Ok(Self {
            analysis_id: id,
            archive_digest: case.circular_snapshot.as_ref().map(|snapshot| snapshot.archive().archive_digest.clone()),
            document_uuid: doc.document_uuid.to_string(),
            runtime_id: doc.runtime_instance_id(),
            session_id: ctx.session_id.clone(),
            open_generation: ctx.open_generation.0,
            ground_id: ground_id.get(),
            ground_source_operation: doc.ground_section(ground_id).map(|ground| ground.source_operation_id.get()),
            case_source_operation: case.source_operation_id.get(),
            captured_owner_revision: doc.revision(),
        })
    }

    pub(super) fn matches_document(&self, doc: &Document, ctx: &HostContext) -> bool {
        self.document_uuid == doc.document_uuid.to_string()
            && self.runtime_id == doc.runtime_instance_id()
            && self.session_id == ctx.session_id
            && self.open_generation == ctx.open_generation.0
            && self.captured_owner_revision <= doc.revision()
            && doc.circular_case(self.analysis_id).is_some_and(|case| {
                case.source_operation_id.get() == self.case_source_operation
                    && case.definition.ground_section_id.get() == self.ground_id
                    && case.circular_snapshot.as_ref().map(|snapshot| snapshot.archive().archive_digest.as_str())
                        == self.archive_digest.as_deref()
                    && doc.ground_section(case.definition.ground_section_id).map(|ground| ground.source_operation_id.get())
                        == self.ground_source_operation
            })
    }
}

fn minimum(
    value: Option<&geotech_core::circular::MethodMinimum>,
    search: &geotech_core::circular::SearchResultV1,
) -> Option<MinimumSummary> {
    value.map(|value| {
        let mut tables = search.retained_tables.iter().filter(|table| {
            table.candidate_id == value.candidate_id && table.table_hash == value.table_hash
                && table.evaluation_key == value.evaluation_key && table.branch == "main"
                && table.config.slices.target_slices == value.final_pair.fine_requested as usize
        });
        let table = tables.next();
        let table = if tables.next().is_none() { table } else { None };
        let bishop = table.and_then(|table| table.evaluation.bishop.as_ref().ok());
        MinimumSummary {
        candidate_id: value.candidate_id.to_string(),
        factor_of_safety: value.fine_factor,
        table_hash: value.table_hash.clone(),
        evaluation_key: value.evaluation_key.clone(),
        retained_table_available: table.is_some(),
        bishop_equation_status: bishop.map(|value| value.convergence_status.clone()),
        finite_contact_status: bishop.map(|value| value.physical_assessment.status.clone()),
        whole_arc_status: bishop.and_then(|value| value.whole_arc_contact.as_ref().map(|contact| contact.status.clone())),
        minimum_normal_kn_per_m: bishop.map(|value| value.physical_assessment.minimum_normal_kn_per_m),
    }})
}

/// Consume the one admitted snapshot copy on the existing worker. This is the
/// only archived-view entry point that verifies hashes/deterministic reports.
pub(super) fn inspect(
    id: AnalysisCaseId,
    captured: Option<CircularSnapshotInspection>,
) -> Result<ArchiveInspection, String> {
    let mut view = ArchiveInspection {
        analysis_id: id,
        presentation: ArchivePresentation {
            state: ArchiveState::NoResult,
            origin: "document_archive",
            calculated_in_this_runtime: false,
            current_job_result: false,
            archive: None,
        },
        snapshot: None,
        report_preview: String::new(),
    };
    let Some(captured) = captured else { return Ok(view); };
    let (currentness, snapshot) = captured.inspect().map_err(|error| {
        format!("ARCHIVE_INVALID: {error}").chars().take(512).collect::<String>()
    })?;
    let archive = snapshot.archive();
    let context = &archive.bundle.context;
    let (source_revision, result) = match &archive.bundle.evidence {
        ReportEvidence::Specified { result, .. } => (
            result.source_revision.clone(),
            StoredSummary::Specified {
                slice_count: result.slices.len().to_string(),
                area_m2: result.total_area_m2,
                weight_kn_per_m: result.total_weight_kn_per_m,
                fellenius_fs: result.fellenius_result.factor_of_safety,
                bishop_equation_fs: result.bishop_result.factor_of_safety,
                bishop_accepted_fs: result.bishop_result.accepted_factor_of_safety,
                bishop_equation_status: result.bishop_result.convergence_status.clone(),
                bishop_equation_residual: result.bishop_result.equation_residual,
                finite_contact_status: result.bishop_result.physical_assessment.status.clone(),
                minimum_normal_kn_per_m: result.bishop_result.physical_assessment.minimum_normal_kn_per_m,
                tensile_slice_count: result.bishop_result.physical_assessment.tensile_slice_ids.len().to_string(),
                indeterminate_slice_count: result.bishop_result.physical_assessment.indeterminate_slice_ids.len().to_string(),
                core_accepted: result.accepted,
                diagnostic_only: result.diagnostic_only,
                numerically_verified: result.numerically_verified,
                engineering_review_required: result.engineering_review_required,
            },
        ),
        ReportEvidence::Search { request, result } => (
            request.source_revision.clone(),
            StoredSummary::Search {
                status: result.status.clone(),
                conclusion: result.conclusion.clone(),
                core_accepted: result.accepted,
                diagnostic_only: result.diagnostic_only,
                candidate_count: result.candidates.len().to_string(),
                retained_table_count: result.retained_tables.len().to_string(),
                fellenius_minimum: minimum(result.minima.fellenius.as_ref(), result),
                bishop_minimum: minimum(result.minima.bishop.as_ref(), result),
                classification: "MINIMUM_WITHIN_EXAMINED_FAMILY_ONLY",
            },
        ),
    };
    view.presentation.state = match currentness {
        Currentness::Current => ArchiveState::ArchivedCurrent,
        Currentness::Stale => ArchiveState::ArchivedStale,
    };
    view.presentation.archive = Some(ArchiveDescriptor {
        project_id: context.project_id.0.clone(),
        case_id: context.case_id.0.to_string(),
        ground_id: context.ground_id.0.to_string(),
        captured_global_revision: context.captured_global_revision.to_string(),
        ground_source_operation_id: context.ground_source_operation.0.to_string(),
        case_source_operation_id: context.case_source_operation.0.to_string(),
        source_revision,
        calculation_fingerprint: archive.bundle.calculation_digest().to_owned(),
        archived_solver_version: context.archived_solver_version.clone(),
        archived_solver_source_fingerprint: context.archived_solver_source_fingerprint.clone(),
        current_solver_version: CIRCULAR_SOLVER_VERSION,
        current_solver_source_fingerprint: source_fingerprint(),
        archive_digest: archive.archive_digest.clone(),
        report_digest: archive.report_digest.clone(),
        saved_calculation_worker_arithmetic_accepted: context.saved_calculation_worker_arithmetic.as_ref().map(|value| value.accepted),
        saved_calculation_worker_arithmetic_scope: context.saved_calculation_worker_arithmetic.as_ref().map(|value| value.scope.clone()),
        result,
    });
    let mut end = archive.markdown.len().min(60 << 10);
    while !archive.markdown.is_char_boundary(end) { end -= 1; }
    view.report_preview = archive.markdown[..end].to_owned();
    if end < archive.markdown.len() {
        view.report_preview.push_str("\n[Предпросмотр сохранённого отчёта сокращён.]\n");
    }
    view.snapshot = Some(snapshot);
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_revit_core::{Command, EngineeringCommand};

    pub(super) fn fixture_document(name: &str) -> Document {
        let path = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures().join(name);
        Document::open(path).unwrap_or_else(|error| panic!("saved fixture {name}: {error}"))
    }

    #[test]
    fn archived_view_preserves_specified_and_search_values_without_current_job() {
        for name in ["specified.native4.json", "search.native4.json"] {
            let document = fixture_document(name);
            let id = *document.circular_cases().keys().next().unwrap();
            let before = document.to_json_string().unwrap();
            let expected = document.circular_snapshot_currentness(id).unwrap().unwrap();
            let saved = document.circular_case(id).unwrap().circular_snapshot.as_ref().unwrap();
            let view = inspect(id, document.capture_circular_snapshot_inspection(id, ARCHIVE_COPY_BYTES).unwrap()).unwrap();
            assert_eq!(view.analysis_id, id);
            assert_eq!(view.presentation.state, match expected {
                Currentness::Current => ArchiveState::ArchivedCurrent,
                Currentness::Stale => ArchiveState::ArchivedStale,
            });
            assert!(!view.presentation.calculated_in_this_runtime);
            assert!(!view.presentation.current_job_result);
            assert_eq!(view.presentation.origin, "document_archive");
            assert_eq!(serde_json::to_vec(&view.snapshot.as_ref().unwrap().archive().bundle.evidence).unwrap(),
                serde_json::to_vec(&saved.archive().bundle.evidence).unwrap());
            let metadata = view.presentation.archive.as_ref().unwrap();
            assert_eq!(metadata.archive_digest, saved.archive().archive_digest);
            assert_eq!(metadata.saved_calculation_worker_arithmetic_accepted,
                saved.archive().bundle.context.saved_calculation_worker_arithmetic.as_ref().map(|value| value.accepted));
            match (&saved.archive().bundle.evidence, &metadata.result) {
                (ReportEvidence::Specified { result, .. }, StoredSummary::Specified {
                    area_m2, weight_kn_per_m, fellenius_fs, bishop_equation_fs,
                    bishop_accepted_fs, numerically_verified, engineering_review_required, ..
                }) => {
                    assert_eq!(*area_m2, result.total_area_m2);
                    assert_eq!(*weight_kn_per_m, result.total_weight_kn_per_m);
                    assert_eq!(*fellenius_fs, result.fellenius_result.factor_of_safety);
                    assert_eq!(*bishop_equation_fs, result.bishop_result.factor_of_safety);
                    assert_eq!(*bishop_accepted_fs, result.bishop_result.accepted_factor_of_safety);
                    assert_eq!(*numerically_verified, result.numerically_verified);
                    assert_eq!(*engineering_review_required, result.engineering_review_required);
                }
                (ReportEvidence::Search { result, .. }, StoredSummary::Search {
                    candidate_count, retained_table_count, fellenius_minimum, bishop_minimum, classification, ..
                }) => {
                    assert_eq!(candidate_count, &result.candidates.len().to_string());
                    assert_eq!(retained_table_count, &result.retained_tables.len().to_string());
                    assert_eq!(*classification, "MINIMUM_WITHIN_EXAMINED_FAMILY_ONLY");
                    for (projected, original) in [
                        (fellenius_minimum.as_ref(), result.minima.fellenius.as_ref()),
                        (bishop_minimum.as_ref(), result.minima.bishop.as_ref()),
                    ] {
                        assert_eq!(projected.map(|value| value.factor_of_safety), original.map(|value| value.fine_factor));
                        assert_eq!(projected.map(|value| value.candidate_id.as_str()), original.map(|value| value.candidate_id.to_string()).as_deref());
                    }
                }
                _ => panic!("saved mode changed during projection"),
            }
            assert!(view.report_preview.len() <= 64 << 10);
            assert!(serde_json::to_vec(&view.presentation).unwrap().len() <= 64 << 10);
            assert_eq!(document.to_json_string().unwrap(), before);
        }
    }

    #[test]
    fn archive_view_absent_is_explicit_and_unknown_case_is_rejected() {
        let document = fixture_document("specified.unsaved.native4.json");
        let id = *document.circular_cases().keys().next().unwrap();
        let view = inspect(id, document.capture_circular_snapshot_inspection(id, ARCHIVE_COPY_BYTES).unwrap()).unwrap();
        assert_eq!(view.presentation.state, ArchiveState::NoResult);
        assert!(view.snapshot.is_none() && view.presentation.archive.is_none());
        assert!(document.capture_circular_snapshot_inspection(AnalysisCaseId::new(u64::MAX), ARCHIVE_COPY_BYTES).is_err());
    }

    #[test]
    fn archive_key_rejects_new_runtime_epochs_and_source_edits_but_allows_rename() {
        let mut document = fixture_document("specified.native4.json");
        let id = *document.circular_cases().keys().next().unwrap();
        let mut context = HostContext { session_id: "inspection-test-session".into(), open_generation: crate::circular_host::DecimalU64(7) };
        let key = ArchiveKey::capture(&document, &context, id).unwrap();
        assert!(key.matches_document(&document, &context));
        let reopened = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
        assert!(!key.matches_document(&reopened, &context));
        context.open_generation.0 += 1;
        assert!(!key.matches_document(&document, &context));
        context.open_generation.0 -= 1;
        context.session_id.push('x');
        assert!(!key.matches_document(&document, &context));
        context.session_id.pop();
        let mut definition = document.circular_case(id).unwrap().definition.clone();
        let mut transaction = document.begin_transaction("archive-view-rename", document.revision());
        transaction.push(Command::Engineering(EngineeringCommand::UpdateCircular {
            analysis_id: id, definition: definition.clone(), name: Some("Historical view rename".into())
        }));
        document.commit(transaction).unwrap();
        assert!(key.matches_document(&document, &context));
        definition.config.bishop.initial_factor += 0.125;
        let mut transaction = document.begin_transaction("archive-view-source-edit", document.revision());
        transaction.push(Command::Engineering(EngineeringCommand::UpdateCircular {
            analysis_id: id, definition, name: None
        }));
        document.commit(transaction).unwrap();
        assert!(!key.matches_document(&document, &context));
        let view = inspect(id, document.capture_circular_snapshot_inspection(id, ARCHIVE_COPY_BYTES).unwrap()).unwrap();
        assert_eq!(view.presentation.state, ArchiveState::ArchivedStale);
        assert!(view.snapshot.is_some());
    }
}
