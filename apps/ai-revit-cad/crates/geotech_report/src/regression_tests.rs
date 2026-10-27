//! Formatter/integrity regressions after explicit in-memory synthetic setup.
//! setup_generated_not_independent_golden: acquisition is separate from the pure
//! operations under test; numerical golden assertions are not replaced.
use super::*;
fn specified() -> ReportArchive { super::test_fixtures::specified_archive().clone() }
fn search() -> ReportArchive { super::test_fixtures::search_archive().clone() }
fn result_mut(bundle: &mut ReportBundle) -> &mut SearchResultV1 {
    let ReportEvidence::Search { result, .. } = &mut bundle.evidence else { unreachable!() }; result
}
fn current(bundle: &ReportBundle) -> CurrentSource {
    let c = &bundle.context;
    CurrentSource { project_id: c.project_id.clone(), case_id: c.case_id, ground_id: c.ground_id,
        ground_source_operation: c.ground_source_operation, case_source_operation: c.case_source_operation,
        solver_version: c.archived_solver_version.clone(), solver_source_fingerprint: c.archived_solver_source_fingerprint.clone(),
        calculation_digest: bundle.calculation_digest().into() }
}
fn rehash(archive: &mut ReportArchive) {
    let cap = ReportLimits::default().input_bytes;
    archive.bundle_digest = digest(&(ARCHIVE_FORMAT, INPUT_DIGEST_ALGORITHM, &archive.bundle), cap).unwrap();
    archive.report_digest = format!("sha256:{:x}", Sha256::digest(archive.markdown.as_bytes()));
    archive.archive_digest = digest(&(ARCHIVE_FORMAT, FORMATTER_VERSION, INPUT_DIGEST_ALGORITHM,
        &archive.bundle_digest, &archive.report_digest), cap).unwrap();
}

#[test]
fn report02_generated_archives_roundtrip_and_source_pin_after_explicit_setup() {
    for archived in [specified(), search()] {
        verify_archive(&archived, ReportLimits::default()).unwrap();
        let bytes = encode_archive(&archived, ReportLimits::default()).unwrap();
        let opened = decode_archive(&bytes, ReportLimits::default()).unwrap();
        assert_eq!(opened.markdown, archived.markdown);
        assert_eq!(opened.bundle.calculation_digest(), archived.bundle.calculation_digest());
        assert_eq!(format_report(&opened.bundle, ReportLimits::default()).unwrap(), archived.markdown);
    }
    assert_eq!(source_fingerprint(), "sha256:dbb13bc6773d4ba6c6a3aaed2d3d9e71407ddc09458c98955fd32588cb1963e8");
}

#[test]
fn report02_escaped_semantic_root_key_budget_and_duplicate_guards_predecode() {
    let archive = specified(); let bytes = serde_json::to_vec(&archive).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    let escaped = raw.replacen("\"bundle\":", "\"\\u0062undle\":", 1);
    let decoded: ReportArchive = serde_json::from_str(&escaped).unwrap();
    assert_eq!(serde_json::to_vec(&decoded.bundle).unwrap(), serde_json::to_vec(&archive.bundle).unwrap());
    let bundle_bytes = serde_json::to_vec(&archive.bundle).unwrap().len();
    let limits = ReportLimits { input_bytes: bundle_bytes - 1, ..ReportLimits::default() };
    assert_eq!(preflight::check(&bytes, limits), Err(ReportError::InputBudget));
    assert_eq!(preflight::check(escaped.as_bytes(), limits), Err(ReportError::InputBudget));
    assert!(matches!(decode_archive(escaped.as_bytes(), limits), Err(ReportError::InputBudget)));
    let generous = ReportLimits { input_bytes: bundle_bytes + 512, ..ReportLimits::default() };
    decode_archive(escaped.as_bytes(), generous).unwrap();
    for aliased in [r#"{"bundle":{},"\u0062undle":{}}"#,
        r#"{"markdown":"x","mark\u0064own":"y"}"#,
        r#"{"formatter":"x","forma\u0074ter":"y"}"#] {
        assert_eq!(preflight::check(aliased.as_bytes(), ReportLimits::default()), Err(ReportError::InvalidArchive));
    }
    assert!(preflight::check(br#"{"\ud83d\ude80":0}"#, ReportLimits::default()).is_ok());
    assert_eq!(preflight::check(br#"{"\ud83d":0}"#, ReportLimits::default()), Err(ReportError::InvalidArchive));
    assert_eq!(preflight::check(br#"{"\ude80":0}"#, ReportLimits::default()), Err(ReportError::InvalidArchive));
    eprintln!("PREDECODE_CONTROL bundle={bundle_bytes} cap={} plain={} escaped={}", limits.input_bytes, bytes.len(), escaped.len());
}

#[test]
fn report02_standalone_format_enforces_actual_encoded_numeric_heavy_bundle_cap() {
    let archive = specified(); let bytes = serde_json::to_vec(&archive.bundle).unwrap().len();
    let limits = ReportLimits { input_bytes: bytes - 1, ..ReportLimits::default() };
    // Controls the original distinction: allocation-free scalar traversal alone
    // succeeds, while complete encoded typed input is one byte over budget.
    audit(&archive.bundle, limits).unwrap();
    assert_eq!(format_report(&archive.bundle, limits), Err(ReportError::InputBudget));
    assert!(format_report(&archive.bundle, ReportLimits { input_bytes: bytes, ..limits }).is_ok());
}

#[test]
fn report02_main_status_family_pair_and_coarse_binding_negative_cases() {
    for mutation in 0..9 {
        let mut bundle = search().bundle;
        let r = result_mut(&mut bundle);
        match mutation {
            0 => { r.examined_family.main_candidate_ids.clear(); r.examined_family.eligible_candidate_ids.clear(); }
            1 => r.minima.fellenius.as_mut().unwrap().final_pair.coarse_requested = 128,
            2 => { r.minima.fellenius = None; r.minima.bishop = None; }
            3 => r.minima.fellenius = None,
            4 => r.candidates[0].main_family_member = false,
            5 => r.candidates[0].final_status = CandidateStatus::MethodsRejected,
            6 => {
                let key = r.minima.bishop.as_ref().unwrap().evaluation_key.clone();
                r.retained_tables.iter_mut().find(|t| t.evaluation_key == key).unwrap().branch = "uniform_grid_bisection_1".into();
            }
            7 => r.candidates[0].fellenius.slice_passes[0].input_fingerprint = "sha256:incorrect-coarse".into(),
            _ => r.minima.bishop.as_mut().unwrap().near_ties.push(999),
        }
        assert!(matches!(create_archive(bundle, ReportLimits::default()), Err(ReportError::Binding(_))), "mutation {mutation}");
    }
}

fn synthetic_shared_baseline() -> ReportBundle {
    // SYNTHETIC STRUCTURAL fixture explicitly authorized by coordinator: reuse
    // saved arithmetic and exact tables, changing metadata only. Not a claim
    // that a new sensitivity run, factor or engineering result was computed.
    let mut bundle = search().bundle;
    let ReportEvidence::Search { request, result: r } = &mut bundle.evidence else { unreachable!() };
    request.sensitivity.uniform_grid_bisections = 1;
    r.request.public = request.clone();
    r.request_fingerprint = digest(&(SEARCH_BINDING, &r.request), ReportLimits::default().input_bytes).unwrap();
    let c = &r.candidates[0];
    r.sensitivity.push(SensitivityResult {
        branch: "uniform_grid_baseline".into(), kind: "controlled_original_uniform_grid".into(), status: "Completed".into(),
        final_pair: request.final_pair, candidate_ids: vec![c.id], eligible_candidate_ids: vec![c.id],
        eligible_set_hash: digest(&(SEARCH_BINDING, vec![c.id]), ReportLimits::default().input_bytes).unwrap(),
        minima: r.minima.clone(), observations: vec![BranchCandidateObservation { candidate_id: c.id,
            fellenius: c.fellenius.clone(), bishop: c.bishop.clone(), status: CandidateStatus::JointlyEligible }],
        winner_changed: ByMethod { fellenius: None, bishop: None }, minimum_factor_delta: ByMethod { fellenius: None, bishop: None },
        eligibility_changed: false, common_candidate_ids: vec![c.id], warnings: vec!["SYNTHETIC_STRUCTURAL_TEST_NO_NEW_ARITHMETIC".into()],
    });
    bundle
}

#[test]
fn report02_sensitivity_scope_positive_shared_main_table_and_negative_bindings() {
    let shared = synthetic_shared_baseline();
    let original = search();
    let ReportEvidence::Search { result: original, .. } = &original.bundle.evidence else { unreachable!() };
    let ReportEvidence::Search { result: shared_result, .. } = &shared.evidence else { unreachable!() };
    assert_eq!(serde_json::to_vec(&shared_result.retained_tables).unwrap(), serde_json::to_vec(&original.retained_tables).unwrap());
    create_archive(shared.clone(), ReportLimits::default()).unwrap();
    for mutation in 0..5 {
        let mut changed = shared.clone(); let r = result_mut(&mut changed);
        match mutation {
            0 => r.sensitivity[0].minima.bishop.as_mut().unwrap().final_pair.coarse_requested = 128,
            1 => r.sensitivity[0].eligible_candidate_ids.clear(),
            2 => r.sensitivity[0].observations[0].status = CandidateStatus::MethodsRejected,
            3 => r.sensitivity[0].minima.fellenius = None,
            _ => { r.sensitivity[0].kind = "complete_nested_uniform_grid".into(); r.sensitivity[0].branch = "uniform_grid_bisection_1".into(); },
        }
        assert!(create_archive(changed, ReportLimits::default()).is_err(), "sensitivity mutation {mutation}");
    }
    // Coarse table is optional retention; required coarse input/pass binding is
    // checked independently and must not demand a second retained full table.
    let mut without_coarse = search().bundle; let r = result_mut(&mut without_coarse);
    r.retained_tables.retain(|t| t.config.slices.target_slices == r.request.public.final_pair.fine_requested as usize);
    create_archive(without_coarse, ReportLimits::default()).unwrap();
}

#[test]
fn report02_old_version_integrity_and_eight_currentness_bindings_stay_separate() {
    let live_bundle = specified().bundle; let live = current(&live_bundle);
    let mut older = live_bundle.clone(); older.context.archived_solver_version = "archived-old-version-serialization-control".into();
    let ReportEvidence::Specified { input, config, result } = &mut older.evidence else { unreachable!() };
    result.solver_version = older.context.archived_solver_version.clone();
    result.input_fingerprint = archived_input_digest(&result.solver_version, input, config, ReportLimits::default()).unwrap();
    let archive = create_archive(older, ReportLimits::default()).unwrap();
    let bytes = encode_archive(&archive, ReportLimits::default()).unwrap();
    let opened = decode_archive(&bytes, ReportLimits::default()).unwrap();
    assert_eq!(opened.bundle.currentness(&live), Currentness::Stale);
    for field in 0..8 {
        let mut changed = live.clone();
        match field {
            0 => changed.project_id.0.push('x'), 1 => changed.case_id.0 += 1, 2 => changed.ground_id.0 += 1,
            3 => changed.ground_source_operation.0 += 1, 4 => changed.case_source_operation.0 += 1,
            5 => changed.solver_version.push('x'), 6 => changed.solver_source_fingerprint.push('x'),
            _ => changed.calculation_digest.push('x'),
        }
        assert_eq!(live_bundle.currentness(&changed), Currentness::Stale);
    }
    let mut same = live_bundle; same.context.captured_global_revision += 1000;
    assert_eq!(same.currentness(&live), Currentness::Current);
}

#[test]
fn report02_rehashed_text_and_internal_binding_tampering_fail() {
    let mut changed = specified(); changed.markdown.push_str("\nFORGED APPROVAL"); rehash(&mut changed);
    assert_eq!(verify_archive(&changed, ReportLimits::default()), Err(ReportError::Binding("formatter/report binding")));
    let mut changed = specified();
    let ReportEvidence::Specified { config, .. } = &mut changed.bundle.evidence else { unreachable!() };
    config.bishop.step_abs *= 2.; rehash(&mut changed);
    assert!(verify_archive(&changed, ReportLimits::default()).is_err());
    let mut changed = search(); result_mut(&mut changed.bundle).minima.bishop.as_mut().unwrap().final_pair.coarse_requested = 128;
    rehash(&mut changed); assert!(verify_archive(&changed, ReportLimits::default()).is_err());
}

#[test]
fn report02_nonfinite_and_all_caps_fail_closed_without_truncation() {
    let mut bad = specified().bundle;
    let ReportEvidence::Specified { result, .. } = &mut bad.evidence else { unreachable!() };
    result.bishop_result.iterations[0].factor_after = f64::NAN;
    assert_eq!(format_report(&bad, ReportLimits::default()), Err(ReportError::NonFinite));
    let archive = specified(); let limits = ReportLimits::default();
    for bound in [ReportLimits { string_bytes: 8, ..limits }, ReportLimits { collection_items: 4, ..limits },
        ReportLimits { nesting: 2, ..limits }] {
        assert_eq!(format_report(&archive.bundle, bound), Err(ReportError::InputBudget));
    }
    assert_eq!(format_report(&archive.bundle, ReportLimits { output_bytes: archive.markdown.len()-1, ..limits }), Err(ReportError::OutputBudget));
    assert!(matches!(create_archive(archive.bundle, ReportLimits { archive_bytes: 1, ..limits }), Err(ReportError::ArchiveBudget)));
    let bytes = serde_json::to_vec(&search()).unwrap();
    assert!(matches!(decode_archive(&bytes, ReportLimits { archive_bytes: bytes.len()-1, ..limits }), Err(ReportError::ArchiveBudget)));
}
