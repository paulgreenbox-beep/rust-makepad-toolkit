//! Explicit, opt-in fixture producer for archive/persistence tests.
//!
//! Synthetic geometry comes from the small public family-inputs source fixture.
//! Initialization performs one specified calculation and one bounded search;
//! archive reads, Document::open and inspections do not invoke a solver.
//! Generated numbers are test setup, never an independent numerical golden.
//! This module is compiled only for tests or the `test-fixtures` feature.
use crate::*;
use geotech_core::circular as c;
use geotech_report::{
    self, CaseId, CurrentSource, Currentness, GroundId, ProjectId, ReportBundle, ReportContext,
    ReportEvidence, ReportLimits, SourceOperationId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

const EXPECTED_SOLVER_VERSION: &str = c::CIRCULAR_SOLVER_VERSION;

/// Explicitly initialize portable fixtures once per test process, before any
/// archive read under test. The UUID directory is owned by this producer and is
/// outside the checkout. Failed setup is preserved for inspection, never reused
/// as a successful fixture set. There is no environment or private-path fallback.
pub fn ensure_circular_archive_fixtures() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "ai-revit-circular-archive-tests-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        produce_circular_archive_fixtures(&root).unwrap_or_else(|error| {
            panic!("explicit synthetic archive fixture setup at {}: {error}", root.display())
        });
        root
    }).as_path()
}

#[derive(Deserialize)]
struct FamilyInputsFixture {
    inputs: Vec<c::CircularAnalysisInput>,
}

#[derive(Deserialize)]
struct NativeHeader {
    format_version: u16,
    required_features: Vec<String>,
}

#[derive(Serialize)]
struct FileEvidence {
    name: String,
    artifact_kind: String,
    sha256: String,
    size_bytes: usize,
    native_format_version: Option<u16>,
    required_features: Vec<String>,
    document_revision: Option<u64>,
    analysis_id: Option<u64>,
    ground_id: Option<u64>,
    ground_source_operation: Option<u64>,
    case_source_operation: Option<u64>,
    solver_version: Option<String>,
    solver_source_fingerprint: Option<String>,
    archive_digest: Option<String>,
    currentness: Option<String>,
}

#[derive(Serialize)]
struct ProducerReceipt {
    schema: &'static str,
    expected_solver_version: &'static str,
    expected_source_fingerprint: String,
    specified_solver_calls: u8,
    search_solver_calls: u8,
    files: Vec<FileEvidence>,
    evidence_boundary: &'static str,
}

#[derive(Serialize)]
struct ProducerSummary {
    status: &'static str,
    file_count: usize,
    receipt: String,
    receipt_sha256: String,
    solver_version: &'static str,
    source_fingerprint: String,
    specified_solver_calls: u8,
    search_solver_calls: u8,
}

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn report<T>(result: Result<T, geotech_report::ReportError>) -> Result<T, Box<dyn Error>> {
    result.map_err(|error| format!("geotech report error: {error:?}").into())
}

fn fixture_input() -> Result<c::CircularAnalysisInput, Box<dyn Error>> {
    let fixture: FamilyInputsFixture = serde_json::from_str(include_str!(
        "../../geotech_report/fixtures/family-inputs.json"
    ))?;
    fixture
        .inputs
        .into_iter()
        .find(|input| input.analysis_id == "C00")
        .ok_or_else(|| "typed family-inputs fixture has no C00 input".into())
}

fn report_limits() -> ReportLimits {
    ReportLimits {
        input_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        output_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        archive_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        ..ReportLimits::default()
    }
}

fn analysis_config() -> c::CircularAnalysisConfig {
    let mut config = c::CircularAnalysisConfig::default();
    config.slices.target_slices = 32;
    config.slices.verify_refinement = false;
    config
}

fn search_spec(input: &c::CircularAnalysisInput) -> CircularSearchSpecV1 {
    let axis = |value: f64| c::InclusiveAxis {
        min: value,
        max: value,
        intervals: 0,
    };
    CircularSearchSpecV1 {
        schema_version: 1,
        domain: c::SearchDomain {
            center_x: axis(input.slip_surface.center.x),
            center_z: axis(input.slip_surface.center.z),
            radius: axis(input.slip_surface.radius_m),
            allowed_entry_x: None,
            allowed_exit_x: None,
        },
        family_policy: c::FamilyPolicy::JointlyEligibleV1,
        target_method: c::TargetMethod::Both,
        exploration_slices: c::ExplorationSlices {
            initial_requested: 8,
            max_passes: 2,
            require_joint_consecutive_convergence: true,
        },
        final_pair: c::FinalComparisonPair {
            coarse_requested: 16,
            fine_requested: 32,
        },
        refinement: c::NeighborhoodRefinement {
            levels: 0,
            promising_per_method: 2,
        },
        sensitivity: c::SensitivityPlan {
            uniform_grid_bisections: 0,
            additional_slice_pairs: Vec::new(),
        },
        core_policy_profile: c::CorePolicyProfile::ProvisionalSerialV1,
        output_mode: c::SearchOutputMode::Strict,
        max_candidates: 4096,
        max_wall_time_ms: 120_000,
        near_tie_abs: 1e-10,
        near_tie_rel: 1e-10,
    }
}

fn ground_definition(input: &c::CircularAnalysisInput) -> GroundDefinition {
    GroundDefinition {
        coordinate_system: input.ground.coordinate_system,
        terrain_polyline: input.ground.terrain_polyline.clone(),
        layer_interfaces: input.ground.layer_interfaces.clone(),
        soil_regions: input.ground.soil_regions.clone(),
        bottom_boundary: input.ground.bottom_boundary.clone(),
        materials: input.ground.materials.clone(),
    }
}

fn apply(
    document: &mut Document,
    command_id: &str,
    command: Command,
) -> Result<CommitReceipt, CoreError> {
    let transaction = document
        .begin_transaction(command_id, document.revision())
        .with(command);
    document.commit(transaction)
}

fn build_document(
    source_input: &c::CircularAnalysisInput,
    with_search_spec: bool,
) -> Result<(Document, GroundSectionId, AnalysisCaseId), Box<dyn Error>> {
    let mut document = Document::new("Gate8 circular calculation archive fixture")?;
    let ground_receipt = apply(
        &mut document,
        if with_search_spec {
            "search-create-ground"
        } else {
            "specified-create-ground"
        },
        Command::Engineering(EngineeringCommand::CreateGround {
            definition: ground_definition(source_input),
            name: "Typed family fixture ground".into(),
        }),
    )?;
    let ground_operation = *ground_receipt
        .operation_ids
        .first()
        .ok_or("ground creation returned no source operation")?;
    let ground_id = GroundSectionId::new(ground_operation.get());

    let definition = CircularCaseDefinition {
        ground_section_id: ground_id,
        slip_surface: source_input.slip_surface.clone(),
        basis: source_input.basis,
        units: source_input.units,
        extrusion_width_m: source_input.extrusion_width_m,
        config: analysis_config(),
        search_spec: with_search_spec.then(|| search_spec(source_input)),
    };
    let case_receipt = apply(
        &mut document,
        if with_search_spec {
            "search-create-case"
        } else {
            "specified-create-case"
        },
        Command::Engineering(EngineeringCommand::CreateCircular {
            definition,
            name: if with_search_spec {
                "Bounded search fixture".into()
            } else {
                "Specified calculation fixture".into()
            },
        }),
    )?;
    let case_operation = *case_receipt
        .operation_ids
        .first()
        .ok_or("circular case creation returned no source operation")?;
    let case_id = AnalysisCaseId::new(case_operation.get());
    if case_operation.get() <= ground_operation.get() {
        return Err("case source operation did not follow the ground source".into());
    }
    Ok((document, ground_id, case_id))
}

fn report_context(
    document: &Document,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
    ground_source: OperationId,
    case_source: OperationId,
) -> ReportContext {
    ReportContext {
        project_id: ProjectId(document.document_uuid.to_string()),
        case_id: CaseId(analysis_id.get()),
        ground_id: GroundId(ground_id.get()),
        captured_global_revision: document.revision(),
        ground_source_operation: SourceOperationId(ground_source.get()),
        case_source_operation: SourceOperationId(case_source.get()),
        archived_solver_version: c::CIRCULAR_SOLVER_VERSION.into(),
        archived_solver_source_fingerprint: c::source_fingerprint(),
        saved_calculation_worker_arithmetic: None,
    }
}

fn write_report_archive(
    archive: &geotech_report::ReportArchive,
    output: &Path,
    artifact_kind: &str,
) -> Result<FileEvidence, Box<dyn Error>> {
    if output.exists() {
        return Err(format!("Refusing to replace {}", output.display()).into());
    }
    let limits = report_limits();
    let encoded = report(geotech_report::encode_archive(archive, limits))?;
    let decoded = report(geotech_report::decode_archive(&encoded, limits))?;
    if decoded.archive_digest != archive.archive_digest
        || decoded.bundle.calculation_digest() != archive.bundle.calculation_digest()
    {
        return Err("typed report archive changed during bounded encode/decode".into());
    }
    fs::write(output, &encoded)?;
    let saved = fs::read(output)?;
    let reopened = report(geotech_report::decode_archive(&saved, limits))?;
    if reopened.archive_digest != archive.archive_digest {
        return Err("saved report archive digest changed after reopen".into());
    }
    let context = &reopened.bundle.context;
    Ok(FileEvidence {
        name: output
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        artifact_kind: artifact_kind.into(),
        sha256: hash(&saved),
        size_bytes: saved.len(),
        native_format_version: None,
        required_features: Vec::new(),
        document_revision: Some(context.captured_global_revision),
        analysis_id: Some(context.case_id.0),
        ground_id: Some(context.ground_id.0),
        ground_source_operation: Some(context.ground_source_operation.0),
        case_source_operation: Some(context.case_source_operation.0),
        solver_version: Some(context.archived_solver_version.clone()),
        solver_source_fingerprint: Some(context.archived_solver_source_fingerprint.clone()),
        archive_digest: Some(reopened.archive_digest),
        currentness: Some("current".into()),
    })
}

fn write_native_document(
    document: &Document,
    output: &Path,
    artifact_kind: &str,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
    currentness: Option<Currentness>,
) -> Result<(Document, FileEvidence), Box<dyn Error>> {
    if output.exists() {
        return Err(format!("Refusing to replace {}", output.display()).into());
    }
    document.save(output)?;
    let bytes = fs::read(output)?;
    let header: NativeHeader = serde_json::from_slice(&bytes)?;
    let mut expected_features = vec![
        "typed_drafting_v1".to_owned(),
        "geotech_slope_v1".to_owned(),
        "geotech_circular_v1".to_owned(),
    ];
    if document.circular_archive_retention_usage()?.snapshot_occurrences != 0 {
        expected_features.push("geotech_circular_archive_v1".to_owned());
    }
    if header.format_version != CIRCULAR_NATIVE_FORMAT_VERSION
        || header.required_features != expected_features
    {
        return Err("saved circular fixture is not exact Native4".into());
    }
    let opened = Document::open(output)?;
    opened.verify_index_consistency()?;
    if opened.revision() != document.revision() {
        return Err("native save/open changed the document revision".into());
    }
    let opened_case = opened
        .circular_case(analysis_id)
        .ok_or("saved native document lost its circular case")?;
    if opened_case.definition.ground_section_id != ground_id {
        return Err("saved native document changed the case ground binding".into());
    }
    let opened_currentness = opened.circular_snapshot_currentness(analysis_id)?;
    if opened_currentness != currentness {
        return Err("native file currentness differs from the requested fixture state".into());
    }
    let snapshot_archive = opened_case
        .circular_snapshot
        .as_ref()
        .map(|snapshot| snapshot.archive());
    let snapshot_context = snapshot_archive.map(|archive| &archive.bundle.context);
    let evidence = FileEvidence {
            name: output
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            artifact_kind: artifact_kind.into(),
            sha256: hash(&bytes),
            size_bytes: bytes.len(),
            native_format_version: Some(header.format_version),
            required_features: header.required_features,
            document_revision: Some(document.revision()),
            analysis_id: Some(analysis_id.get()),
            ground_id: Some(ground_id.get()),
            ground_source_operation: Some(
                document
                    .ground_section(ground_id)
                    .ok_or("native document ground missing")?
                    .source_operation_id
                    .get(),
            ),
            case_source_operation: Some(
                document
                    .circular_case(analysis_id)
                    .ok_or("native document case missing")?
                    .source_operation_id
                    .get(),
            ),
            solver_version: snapshot_context.map(|context| context.archived_solver_version.clone()),
            solver_source_fingerprint: snapshot_context
                .map(|context| context.archived_solver_source_fingerprint.clone()),
            archive_digest: snapshot_archive.map(|archive| archive.archive_digest.clone()),
            currentness: opened_currentness.map(|value| match value {
                Currentness::Current => "current".to_owned(),
                Currentness::Stale => "stale".to_owned(),
            }),
        };
    Ok((opened, evidence))
}

fn verify_saved_history(
    document: &Document,
    analysis_id: AnalysisCaseId,
    save_revision: u64,
) -> Result<(), Box<dyn Error>> {
    let replayed = document.replay_revision(save_revision)?;
    replayed.verify_index_consistency()?;
    if replayed
        .circular_case(analysis_id)
        .and_then(|case| case.circular_snapshot.as_ref())
        .is_none()
        || replayed.circular_snapshot_currentness(analysis_id)? != Some(Currentness::Current)
    {
        return Err("replay at snapshot-save revision did not restore a current archive".into());
    }
    Ok(())
}

fn current_source(
    document: &Document,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
    ground_source: OperationId,
    case_source: OperationId,
    calculation_digest: &str,
) -> CurrentSource {
    CurrentSource {
        project_id: ProjectId(document.document_uuid.to_string()),
        case_id: CaseId(analysis_id.get()),
        ground_id: GroundId(ground_id.get()),
        ground_source_operation: SourceOperationId(ground_source.get()),
        case_source_operation: SourceOperationId(case_source.get()),
        solver_version: c::CIRCULAR_SOLVER_VERSION.into(),
        solver_source_fingerprint: c::source_fingerprint(),
        calculation_digest: calculation_digest.into(),
    }
}

fn make_specified_bundle(
    document: &Document,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
) -> Result<ReportBundle, Box<dyn Error>> {
    let resolved = document.resolve_circular_input(analysis_id)?;
    let context = report_context(
        document,
        analysis_id,
        ground_id,
        resolved.ground_source_operation_id,
        resolved.case_source_operation_id,
    );
    // The producer's sole specified-solver invocation.
    let result = c::analyze_specified_circle(&resolved.input, &resolved.config)?;
    if result.solver_version != EXPECTED_SOLVER_VERSION
        || result.source_fingerprint != c::source_fingerprint()
    {
        return Err("specified solver output has unexpected version or source fingerprint".into());
    }
    Ok(ReportBundle {
        context,
        evidence: ReportEvidence::Specified {
            input: resolved.input,
            config: resolved.config,
            result,
        },
        source_references: Vec::new(),
    })
}

fn make_search_bundle(
    document: &Document,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
) -> Result<ReportBundle, Box<dyn Error>> {
    let resolved = document.resolve_circular_search(analysis_id)?;
    let context = report_context(
        document,
        analysis_id,
        ground_id,
        resolved.ground_source_operation_id,
        resolved.case_source_operation_id,
    );
    let request: c::SearchPublicRequestV1 = resolved.request;
    // The producer's sole bounded-search solver invocation.
    let result = c::search_critical_circle(&request)?;
    if result.solver_version != EXPECTED_SOLVER_VERSION
        || result.source_fingerprint != c::source_fingerprint()
        || result.request.public != request
    {
        return Err(
            "search solver output has unexpected version, source, or public request".into(),
        );
    }
    Ok(ReportBundle {
        context,
        evidence: ReportEvidence::Search { request, result },
        source_references: Vec::new(),
    })
}

fn produce_specified(
    output: &Path,
    source_input: &c::CircularAnalysisInput,
    files: &mut Vec<FileEvidence>,
) -> Result<(), Box<dyn Error>> {
    let (mut document, ground_id, analysis_id) = build_document(source_input, false)?;
    let resolved = document.resolve_circular_input(analysis_id)?;
    let ground_source = resolved.ground_source_operation_id;
    let case_source = resolved.case_source_operation_id;

    let (opened_unsaved, row) = write_native_document(
        &document,
        &output.join("specified.unsaved.native4.json"),
        "specified_sources_without_saved_calculation",
        analysis_id,
        ground_id,
        None,
    )?;
    if opened_unsaved
        .circular_snapshot_currentness(analysis_id)?
        .is_some()
    {
        return Err("unsaved specified source fixture unexpectedly has an archive".into());
    }
    files.push(row);

    let bundle = make_specified_bundle(&document, analysis_id, ground_id)?;
    let calculation_digest = bundle.calculation_digest().to_owned();
    let current = current_source(
        &document,
        analysis_id,
        ground_id,
        ground_source,
        case_source,
        &calculation_digest,
    );
    if bundle.currentness(&current) != Currentness::Current {
        return Err("specified report bundle does not match its real source operations".into());
    }
    let archive = archive_bundle_for_document(bundle, &document, analysis_id, ground_id)?;
    files.push(write_report_archive(
        &archive,
        &output.join("specified.archive.json"),
        "current_specified_report_archive",
    )?);

    let snapshot = document.prepare_circular_snapshot(analysis_id, archive.clone())?;
    let save_receipt = apply(
        &mut document,
        "specified-save-archive",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id,
            snapshot,
        }),
    )?;
    let save_revision = save_receipt.revision;
    if document.circular_snapshot_currentness(analysis_id)? != Some(Currentness::Current) {
        return Err("specified archive save did not become current".into());
    }
    verify_saved_history(&document, analysis_id, save_revision)?;
    let (opened, row) = write_native_document(
        &document,
        &output.join("specified.native4.json"),
        "saved_specified_result_and_report",
        analysis_id,
        ground_id,
        Some(Currentness::Current),
    )?;
    if opened.circular_snapshot_currentness(analysis_id)? != Some(Currentness::Current) {
        return Err("specified snapshot did not remain current after native open".into());
    }
    verify_saved_history(&opened, analysis_id, save_revision)?;
    files.push(row);

    document
        .undo()?
        .ok_or("specified archive save could not be undone")?;
    if document
        .circular_case(analysis_id)
        .and_then(|case| case.circular_snapshot.as_ref())
        .is_some()
        || document
            .circular_snapshot_currentness(analysis_id)?
            .is_some()
    {
        return Err("undo left a live specified snapshot in the case".into());
    }
    let (retained, row) = write_native_document(
        &document,
        &output.join("retained-only.native4.json"),
        "specified_snapshot_retained_only_in_undo_redo_history",
        analysis_id,
        ground_id,
        None,
    )?;
    if retained
        .circular_case(analysis_id)
        .and_then(|case| case.circular_snapshot.as_ref())
        .is_some()
        || retained
            .circular_snapshot_currentness(analysis_id)?
            .is_some()
    {
        return Err("retained-only native file exposes a live snapshot".into());
    }
    verify_saved_history(&retained, analysis_id, save_revision)?;
    files.push(row);
    Ok(())
}

fn archive_bundle_for_document(
    bundle: ReportBundle,
    document: &Document,
    analysis_id: AnalysisCaseId,
    ground_id: GroundSectionId,
) -> Result<geotech_report::ReportArchive, Box<dyn Error>> {
    let cse = document
        .circular_case(analysis_id)
        .ok_or("archive case missing")?;
    let ground = document
        .ground_section(ground_id)
        .ok_or("archive ground missing")?;
    let source = current_source(
        document,
        analysis_id,
        ground_id,
        ground.source_operation_id,
        cse.source_operation_id,
        bundle.calculation_digest(),
    );
    if bundle.currentness(&source) != Currentness::Current {
        return Err("report bundle currentness differs from the live source operations".into());
    }
    report(geotech_report::create_archive(bundle, report_limits()))
}

fn produce_search(
    output: &Path,
    source_input: &c::CircularAnalysisInput,
    files: &mut Vec<FileEvidence>,
) -> Result<(), Box<dyn Error>> {
    let (mut document, ground_id, analysis_id) = build_document(source_input, true)?;
    let resolved = document.resolve_circular_search(analysis_id)?;
    let ground_source = resolved.ground_source_operation_id;
    let case_source = resolved.case_source_operation_id;

    let (opened_unsaved, row) = write_native_document(
        &document,
        &output.join("search.unsaved.native4.json"),
        "search_sources_without_saved_calculation",
        analysis_id,
        ground_id,
        None,
    )?;
    if opened_unsaved
        .circular_snapshot_currentness(analysis_id)?
        .is_some()
    {
        return Err("unsaved search source fixture unexpectedly has an archive".into());
    }
    files.push(row);

    let bundle = make_search_bundle(&document, analysis_id, ground_id)?;
    let calculation_digest = bundle.calculation_digest().to_owned();
    let current = current_source(
        &document,
        analysis_id,
        ground_id,
        ground_source,
        case_source,
        &calculation_digest,
    );
    if bundle.currentness(&current) != Currentness::Current {
        return Err("search report bundle does not match its real source operations".into());
    }
    let archive = archive_bundle_for_document(bundle, &document, analysis_id, ground_id)?;
    files.push(write_report_archive(
        &archive,
        &output.join("search.archive.json"),
        "current_search_report_archive",
    )?);

    let snapshot = document.prepare_circular_snapshot(analysis_id, archive.clone())?;
    let save_receipt = apply(
        &mut document,
        "search-save-archive",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id,
            snapshot,
        }),
    )?;
    let save_revision = save_receipt.revision;
    if document.circular_snapshot_currentness(analysis_id)? != Some(Currentness::Current) {
        return Err("search archive save did not become current".into());
    }
    verify_saved_history(&document, analysis_id, save_revision)?;
    let (opened, row) = write_native_document(
        &document,
        &output.join("search.native4.json"),
        "saved_search_public_request_and_result_tuple",
        analysis_id,
        ground_id,
        Some(Currentness::Current),
    )?;
    if opened.circular_snapshot_currentness(analysis_id)? != Some(Currentness::Current) {
        return Err("search snapshot did not remain current after native open".into());
    }
    verify_saved_history(&opened, analysis_id, save_revision)?;
    files.push(row);
    Ok(())
}

/// A synthetic old-identity control, not an archive from an earlier solver run.
/// Repackage the freshly generated specified output without changing its numbers.
/// This preserves a Stale/open/inspection regression without shipping a private
/// historical runtime blob or claiming historical numerical verification.
fn produce_historical_identity_control(
    output: &Path,
    files: &mut Vec<FileEvidence>,
) -> Result<(), Box<dyn Error>> {
    const VERSION: &str = "synthetic-historical-serialization-control-v1";
    const SOURCE: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000001";
    let mut document = Document::open(output.join("specified.unsaved.native4.json"))?;
    let analysis_id = *document.circular_cases().keys().next().ok_or("control case missing")?;
    let ground_id = document.circular_case(analysis_id).ok_or("control case missing")?
        .definition.ground_section_id;
    let mut archive = report(geotech_report::decode_archive(
        &fs::read(output.join("specified.archive.json"))?, report_limits()
    ))?;
    archive.bundle.context.archived_solver_version = VERSION.into();
    archive.bundle.context.archived_solver_source_fingerprint = SOURCE.into();
    let ReportEvidence::Specified { input, config, result } = &mut archive.bundle.evidence else {
        return Err("specified identity control changed mode".into());
    };
    let before = result.clone();
    result.solver_version = VERSION.into();
    result.source_fingerprint = SOURCE.into();
    result.input_fingerprint = report(geotech_report::archived_input_digest(
        VERSION, input, config, report_limits()
    ))?;
    if let Some(contact) = result.bishop_result.whole_arc_contact.as_mut() {
        contact.source_fingerprint = SOURCE.into();
        contact.contact_input_fingerprint = hash(&serde_json::to_vec(&(
            "whole-arc-contact-envelope-v1", input, config, contact.factor_lower,
            contact.factor_upper, &contact.root_range_scope, SOURCE,
        ))?);
    }
    let mut normalized = result.clone();
    normalized.solver_version = before.solver_version.clone();
    normalized.source_fingerprint = before.source_fingerprint.clone();
    normalized.input_fingerprint = before.input_fingerprint.clone();
    if let (Some(normalized_contact), Some(before_contact)) = (
        normalized.bishop_result.whole_arc_contact.as_mut(),
        before.bishop_result.whole_arc_contact.as_ref(),
    ) {
        normalized_contact.source_fingerprint = before_contact.source_fingerprint.clone();
        normalized_contact.contact_input_fingerprint = before_contact.contact_input_fingerprint.clone();
    }
    if normalized != before {
        return Err("synthetic identity control changed saved numerical evidence".into());
    }
    let archive = report(geotech_report::create_archive(archive.bundle, report_limits()))?;
    let snapshot = document.prepare_circular_snapshot(analysis_id, archive)?;
    apply(&mut document, "synthetic-historical-identity-save", Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot { analysis_id, snapshot }
    ))?;
    let (_, row) = write_native_document(
        &document, &output.join("historical.specified.native4.json"),
        "synthetic_historical_identity_only_not_historical_solver_evidence",
        analysis_id, ground_id, Some(Currentness::Stale),
    )?;
    files.push(row);
    Ok(())
}

/// Explicit synthetic test setup: one specified solve and one bounded search.
/// The output directory must be new. This is never called by Document::open.
pub fn produce_circular_archive_fixtures(output: &Path) -> Result<(), Box<dyn Error>> {
    if output.exists() {
        return Err("Output directory must be new; refusing to replace fixture evidence".into());
    }
    fs::create_dir_all(output)?;
    let source_input = fixture_input()?;
    let mut files = Vec::new();
    produce_specified(&output, &source_input, &mut files)?;
    produce_search(output, &source_input, &mut files)?;
    produce_historical_identity_control(output, &mut files)?;

    let receipt = ProducerReceipt {
        schema: "synthetic-circular-archive-test-fixtures-v2",
        expected_solver_version: EXPECTED_SOLVER_VERSION,
        expected_source_fingerprint: c::source_fingerprint(),
        specified_solver_calls: 1,
        search_solver_calls: 1,
        files,
        evidence_boundary: "Synthetic test input with freshly solver-generated typed outputs and real native4 save/open/history replay; the separate historical identity probe is a synthetic repack, not historical solver evidence. Currentness is provenance only; numerical goldens and engineering acceptance are separate.",
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt)?;
    let receipt_path = output.join("producer-receipt.json");
    if receipt_path.exists() {
        return Err("Refusing to replace producer receipt".into());
    }
    fs::write(&receipt_path, &receipt_bytes)?;
    let summary = ProducerSummary {
        status: "SAVED_SYNTHETIC_CIRCULAR_TEST_FIXTURES",
        file_count: receipt.files.len(),
        receipt: receipt_path.to_string_lossy().into_owned(),
        receipt_sha256: hash(&receipt_bytes),
        solver_version: EXPECTED_SOLVER_VERSION,
        source_fingerprint: c::source_fingerprint(),
        specified_solver_calls: 1,
        search_solver_calls: 1,
    };
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}
