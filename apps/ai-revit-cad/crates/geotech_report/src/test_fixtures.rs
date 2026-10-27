//! Explicit in-memory setup for formatter/integrity/currentness tests.
//!
//! setup_generated_not_independent_golden: generated outputs only supply typed
//! archive content to roundtrip, corruption, provenance and budget assertions.
//! They are not expected answers for a numerical solver test. Existing numerical
//! and source pins stay in the tests. No Core dependency, file output, external
//! document, user directory or machine-path seed participates in this setup.
use super::*;
use serde_json::Value;
use std::sync::OnceLock;
fn inputs() -> (ReportContext, CircularAnalysisInput, CircularAnalysisConfig) {
    assert_eq!(source_fingerprint(), "sha256:dbb13bc6773d4ba6c6a3aaed2d3d9e71407ddc09458c98955fd32588cb1963e8");
    let raw: Value = serde_json::from_str(include_str!("../fixtures/family-inputs.json")).unwrap();
    let mut input: CircularAnalysisInput = serde_json::from_value(raw["inputs"][0].clone()).unwrap();
    let context = ReportContext {
        project_id: ProjectId("fixture-project-not-native-document".into()),
        case_id: CaseId(1), ground_id: GroundId(2), captured_global_revision: 41,
        ground_source_operation: SourceOperationId(2), case_source_operation: SourceOperationId(3),
        archived_solver_version: CIRCULAR_SOLVER_VERSION.into(),
        archived_solver_source_fingerprint: source_fingerprint(),
        saved_calculation_worker_arithmetic: None,
    };
    input.analysis_id = "1".into(); input.ground.section_id = GroundSectionId("2".into());
    input.ground.model_revision = 2; input.source_revision = context.source_revision();
    let mut config = CircularAnalysisConfig::default();
    config.slices.target_slices = 64; config.slices.verify_refinement = false;
    config.bishop.step_abs = 1e-12; config.bishop.step_rel = 1e-12;
    config.bishop.residual_abs = 1e-12; config.bishop.residual_rel = 1e-12;
    (context, input, config)
}
pub(super) fn specified() -> &'static ReportBundle {
    static FIXTURE: OnceLock<ReportBundle> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let (context, input, config) = inputs();
        let result = analyze_specified_circle(&input, &config).unwrap();
        ReportBundle { context, evidence: ReportEvidence::Specified { input, config, result },
            source_references: Vec::new() }
    })
}
pub(super) fn search_request() -> (ReportContext, SearchPublicRequestV1) {
    let (context, input, config) = inputs();
    let axis = |v| InclusiveAxis { min: v, max: v, intervals: 0 };
    (context, SearchPublicRequestV1::from_specified(&input, &config, SearchDomain {
        center_x: axis(-0.125), center_z: axis(5.125), radius: axis(4.875),
        allowed_entry_x: None, allowed_exit_x: None }))
}
pub(super) fn search() -> &'static ReportBundle {
    static FIXTURE: OnceLock<ReportBundle> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let (context, request) = search_request();
        let result = search_critical_circle(&request).unwrap();
        assert_eq!(result.status, SearchRunStatus::CompletedMinimum);
        ReportBundle { context, evidence: ReportEvidence::Search { request, result },
            source_references: Vec::new() }
    })
}

/// Explicit setup happens once; subsequent archive tests clone the saved typed
/// value, and format/decode/verify functions do not acquire a solver.
pub(super) fn specified_archive() -> &'static ReportArchive {
    static ARCHIVE: OnceLock<ReportArchive> = OnceLock::new();
    ARCHIVE.get_or_init(|| {
        eprintln!("REPORT_TEST_FIXTURE setup_generated_not_independent_golden mode=specified");
        create_archive(specified().clone(), ReportLimits::default()).unwrap()
    })
}

pub(super) fn search_archive() -> &'static ReportArchive {
    static ARCHIVE: OnceLock<ReportArchive> = OnceLock::new();
    ARCHIVE.get_or_init(|| {
        eprintln!("REPORT_TEST_FIXTURE setup_generated_not_independent_golden mode=search");
        create_archive(search().clone(), ReportLimits::default()).unwrap()
    })
}
