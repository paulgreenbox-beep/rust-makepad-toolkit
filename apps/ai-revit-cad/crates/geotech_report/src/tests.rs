use super::*;
use super::test_fixtures::{search, search_request, specified};
fn current(bundle: &ReportBundle) -> CurrentSource {
    let c = &bundle.context;
    CurrentSource { project_id: c.project_id.clone(), case_id: c.case_id, ground_id: c.ground_id,
        ground_source_operation: c.ground_source_operation, case_source_operation: c.case_source_operation,
        solver_version: c.archived_solver_version.clone(),
        solver_source_fingerprint: c.archived_solver_source_fingerprint.clone(),
        calculation_digest: bundle.calculation_digest().into() }
}

#[test]
fn report_actual_typed_specified_and_search_roundtrip_examples_no_recalculation() {
    // Actual public core is called only by these fixture constructors. Formatter
    // modules do not import a solver, preparation helper, or current FP inspector.
    let fixtures = [specified(), search()];
    for (name, bundle) in ["specified", "search"].into_iter().zip(fixtures) {
        let archive = create_archive(bundle.clone(), ReportLimits::default()).unwrap();
        let bytes = encode_archive(&archive, ReportLimits::default()).unwrap();
        let opened = decode_archive(&bytes, ReportLimits::default()).unwrap();
        assert_eq!(opened.markdown, archive.markdown);
        assert_eq!(opened.bundle.currentness(&current(bundle)), Currentness::Current);
        assert!(!archive.markdown.contains("VERIFIED=true"));
        assert!(archive.markdown.contains("ENGINEERING_REVIEW_REQUIRED"));
        assert!(archive.markdown.contains("Сухая статическая 2D-модель"));
        assert!(archive.markdown.contains("UNKNOWN / отсутствует"));
        for section in 1..=20 { assert!(archive.markdown.contains(&format!("## {section}. "))); }
        // The generated typed value is immutable setup shared by both suites;
        // it is not an independent numerical expected result.
        eprintln!("REPORT_SIZE {name} typed_bundle_bytes={} markdown_bytes={} archive_bytes={} archive_digest={}",
            serde_json::to_vec(bundle).unwrap().len(), archive.markdown.len(), bytes.len(), archive.archive_digest);
    }
}

#[test]
fn report_all_slice_rows_iterations_and_negative_signed_force_survive() {
    let bundle = specified();
    let ReportEvidence::Specified { result, .. } = &bundle.evidence else { unreachable!() };
    let report = format_report(bundle, ReportLimits::default()).unwrap();
    let negative = result.fellenius_result.slice_contributions.iter()
        .find(|s| s.driving_kn_per_m < 0.0).expect("actual fixture must have signed negative force");
    assert!(report.contains(&format!("| {} | {} | {} |", negative.slice_id,
        negative.normal_kn_per_m, negative.driving_kn_per_m)));
    for s in &result.slices {
        assert!(report.contains(&format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.slice_id, s.x_left, s.x_right, s.horizontal_width_m, s.total_area_m2,
            s.weight_kn_per_m, s.base_inclination_rad, s.base_length_m, s.arc_length_m)));
    }
    for i in &result.bishop_result.iterations {
        assert!(report.contains(&format!("| {} | {:?} | {} | {} |", i.iteration, i.step, i.factor_before, i.factor_after)));
    }
    assert!(report.contains("independent_whole_arc_acceptance"));
    assert!(!result.numerically_verified && result.engineering_review_required);
}

#[test]
fn report_archive_tamper_input_config_result_context_and_report_all_fail() {
    let good = create_archive(specified().clone(), ReportLimits::default()).unwrap();
    for mutation in 0..5 {
        let mut bad = good.clone();
        let ReportEvidence::Specified { input, config, result } = &mut bad.bundle.evidence else { unreachable!() };
        match mutation {
            0 => input.ground.terrain_polyline[0].z += 0.01,
            1 => config.bishop.step_abs *= 2.0,
            2 => result.fellenius_result.factor_of_safety += 0.01,
            3 => bad.bundle.context.captured_global_revision += 1,
            _ => bad.markdown.push_str("\nForged approval"),
        }
        assert!(verify_archive(&bad, ReportLimits::default()).is_err(), "mutation {mutation}");
    }
    // A report checksum recomputed after tampering still fails deterministic
    // formatter binding, even if the envelope checksum is also recomputed.
    let mut bad = good;
    bad.markdown.push_str("\nForged approval");
    bad.report_digest = format!("sha256:{:x}", Sha256::digest(bad.markdown.as_bytes()));
    bad.archive_digest = digest(&(ARCHIVE_FORMAT, FORMATTER_VERSION, INPUT_DIGEST_ALGORITHM,
        &bad.bundle_digest, &bad.report_digest), ReportLimits::default().input_bytes).unwrap();
    assert_eq!(verify_archive(&bad, ReportLimits::default()), Err(ReportError::Binding("formatter/report binding")));
}

#[test]
fn report_deep_nonfinite_is_rejected_before_json_digest_or_format() {
    for mutation in 0..4 {
        let mut bundle = specified().clone();
        let ReportEvidence::Specified { input, config, result } = &mut bundle.evidence else { unreachable!() };
        match mutation {
            0 => result.bishop_result.iterations[0].factor_after = f64::NAN,
            1 => result.slices[0].numerical_quality.estimated_area_roundoff_m2 = f64::INFINITY,
            2 => input.ground.materials[0].properties.effective_cohesion_kpa = f64::NEG_INFINITY,
            _ => config.bishop.residual_rel = f64::NAN,
        }
        assert_eq!(format_report(&bundle, ReportLimits::default()), Err(ReportError::NonFinite));
    }
    let mut bundle = search().clone();
    let ReportEvidence::Search { result, .. } = &mut bundle.evidence else { unreachable!() };
    result.retained_tables[0].evaluation.bishop.as_mut().unwrap()
        .slice_contributions[0].base_forces.vertical_residual_bound_kn_per_m = f64::NAN;
    assert_eq!(format_report(&bundle, ReportLimits::default()), Err(ReportError::NonFinite));
}

#[test]
fn report_old_solver_archive_integrity_is_separate_from_live_currentness() {
    let live = current(specified());
    let mut historical = specified().clone();
    historical.context.archived_solver_version = "historical-version-serialization-fixture".into();
    let ReportEvidence::Specified { input, config, result } = &mut historical.evidence else { unreachable!() };
    result.solver_version = historical.context.archived_solver_version.clone();
    // Controlled serialization fixture, not evidence that this old solver ran.
    result.input_fingerprint = archived_input_digest(&result.solver_version, input, config, ReportLimits::default()).unwrap();
    let archive = create_archive(historical, ReportLimits::default()).unwrap();
    verify_archive(&archive, ReportLimits::default()).unwrap();
    assert_eq!(archive.bundle.currentness(&live), Currentness::Stale);
    let mut unchanged = specified().clone();
    unchanged.context.captured_global_revision += 300; // unrelated BIM provenance
    assert_eq!(unchanged.currentness(&live), Currentness::Current);
    let mut edited = live.clone(); edited.ground_source_operation.0 += 1;
    assert_eq!(specified().currentness(&edited), Currentness::Stale);
}

#[test]
fn report_missing_winner_table_and_search_binding_corruption_fail_explicitly() {
    let mut missing = search().clone();
    let ReportEvidence::Search { result, .. } = &mut missing.evidence else { unreachable!() };
    let winner = result.minima.bishop.as_ref().unwrap().candidate_id;
    result.retained_tables.clear();
    assert_eq!(format_report(&missing, ReportLimits::default()), Err(ReportError::MissingWinnerTable(winner)));
    for mutation in 0..4 {
        let mut corrupted = search().clone();
        let ReportEvidence::Search { request, result } = &mut corrupted.evidence else { unreachable!() };
        match mutation {
            0 => request.max_candidates += 1,
            1 => result.retained_tables[0].evaluation.total_weight_kn_per_m += 0.01,
            2 => result.retained_tables[0].config.bishop.step_rel *= 2.0,
            _ => result.minima.bishop.as_mut().unwrap().fine_factor += 0.1,
        }
        assert!(format_report(&corrupted, ReportLimits::default()).is_err());
    }
}

#[test]
fn report_selected_winner_rows_use_exact_fine_table_and_deduplicate_shared_winner() {
    let bundle = search();
    let ReportEvidence::Search { result, .. } = &bundle.evidence else { unreachable!() };
    let report = format_report(bundle, ReportLimits::default()).unwrap();
    let mut selected = std::collections::BTreeSet::new();
    for minimum in [&result.minima.fellenius, &result.minima.bishop].into_iter().flatten() {
        selected.insert((minimum.candidate_id, &minimum.table_hash, &minimum.evaluation_key));
    }
    let mut expected_slice_ids = 0;
    for (id, hash, key) in selected {
        let table = result.retained_tables.iter().find(|t|
            t.candidate_id == id && &t.table_hash == hash && &t.evaluation_key == key).unwrap();
        // Slice JSON + FF contributions + Bishop contributions; no coarse table
        // substitution and no second copy when both methods have the same winner.
        expected_slice_ids += table.evaluation.slices.len()
            + table.evaluation.fellenius.as_ref().unwrap().slice_contributions.len()
            + table.evaluation.bishop.as_ref().unwrap().slice_contributions.len();
        assert!(report.contains(&text_candidate(id, key)));
    }
    assert_eq!(report.matches("\"slice_id\":").count(), expected_slice_ids);
    fn text_candidate(id: u64, key: &str) -> String { format!("Кандидат {id}; ветвь main; 512 slices requested; {key}") }
}

#[test]
fn report_real_diagnostic_and_empty_search_never_become_accepted_critical() {
    for diagnostic in [true, false] {
        let (context, mut request) = search_request();
        if diagnostic { request.output_mode = SearchOutputMode::Diagnostic; }
        else { request.domain.center_x = InclusiveAxis { min: 100., max: 100., intervals: 0 }; }
        // Fixture acquisition only, before the pure formatter/archive calls.
        let result = search_critical_circle(&request).unwrap();
        assert!(!result.accepted);
        let bundle = ReportBundle { context, evidence: ReportEvidence::Search { request, result },
            source_references: vec![SourceReference { title: "Licensed reference unavailable".into(),
                locator: None, publication_date: None, section_or_page: None,
                availability: ReferenceAvailability::Unavailable, note: None }] };
        let archive = create_archive(bundle, ReportLimits::default()).unwrap();
        assert_eq!(archive.bundle.conclusion(), EvidenceConclusion::NoAcceptedStabilityEvidence);
        assert!(archive.markdown.contains("NoAcceptedStabilityEvidence"));
        verify_archive(&archive, ReportLimits::default()).unwrap();
    }
}

#[test]
fn report_caps_are_typed_atomic_and_archive_preflight_precedes_deserialize() {
    let limits = ReportLimits::default();
    let full = format_report(specified(), limits).unwrap();
    assert_eq!(format_report(specified(), ReportLimits { output_bytes: full.len()-1, ..limits }),
        Err(ReportError::OutputBudget));
    assert_eq!(format_report(specified(), ReportLimits { string_bytes: 8, ..limits }),
        Err(ReportError::InputBudget));
    assert_eq!(format_report(specified(), ReportLimits { collection_items: 4, ..limits }),
        Err(ReportError::InputBudget));
    let archive = create_archive(specified().clone(), limits).unwrap();
    assert!(matches!(create_archive(specified().clone(), ReportLimits { archive_bytes: 1, ..limits }),
        Err(ReportError::ArchiveBudget)));
    let bytes = encode_archive(&archive, limits).unwrap();
    assert_eq!(encode_archive(&archive, ReportLimits { archive_bytes: bytes.len()-1, ..limits }),
        Err(ReportError::ArchiveBudget));
    assert!(matches!(decode_archive(&bytes, ReportLimits { archive_bytes: 1, ..limits }),
        Err(ReportError::ArchiveBudget)));
    let hostile = format!("{{\"bundle\":{{\"context\":{{\"project_id\":\"{}\"}}}}}}", "a".repeat(100_000));
    assert_eq!(preflight::check(hostile.as_bytes(), limits), Err(ReportError::InputBudget));
    assert_eq!(preflight::check(b"{\"x\":[1,2,3,4,5]}", ReportLimits { collection_items: 4, ..limits }),
        Err(ReportError::InputBudget));
    assert_eq!(preflight::check(b"{\"x\":[[[[0]]]]}", ReportLimits { nesting: 2, ..limits }),
        Err(ReportError::InputBudget));
    assert_eq!(preflight::check(b"{\"bundle\":{\"x\":\"1234567890\"}}", ReportLimits { input_bytes: 8, ..limits }),
        Err(ReportError::InputBudget));
    assert_eq!(preflight::check(b"{\"markdown\":\"12345\"}", ReportLimits { output_bytes: 4, ..limits }),
        Err(ReportError::InputBudget));
    assert_eq!(preflight::check(b"{\"x\":\"\\u20ac\\u20ac\"}", ReportLimits { string_bytes: 5, ..limits }),
        Err(ReportError::InputBudget));
}
