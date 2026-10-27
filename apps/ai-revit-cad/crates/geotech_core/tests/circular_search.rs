use geotech_core::circular::*;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

fn family() -> Vec<CircularAnalysisInput> {
    let data: Value = serde_json::from_str(include_str!(
        "fixtures/family-inputs.json"
    )).unwrap();
    serde_json::from_value(data["inputs"].clone()).unwrap()
}
fn axis(min: f64, max: f64, intervals: u32) -> InclusiveAxis {
    InclusiveAxis { min, max, intervals }
}
fn request() -> SearchPublicRequestV1 {
    let input = family().remove(0);
    let mut config = CircularAnalysisConfig::default();
    config.slices.target_slices = 64;
    config.bishop.step_abs = 1e-12;
    config.bishop.step_rel = 1e-12;
    config.bishop.residual_abs = 1e-12;
    config.bishop.residual_rel = 1e-12;
    SearchPublicRequestV1::from_specified(&input, &config, SearchDomain {
        center_x: axis(-0.125, 0.125, 1), center_z: axis(4.875, 5.125, 1),
        radius: axis(4.875, 5.125, 1), allowed_entry_x: None, allowed_exit_x: None,
    })
}
fn singleton() -> SearchPublicRequestV1 {
    let mut r = request();
    r.domain.center_x = axis(-0.125, -0.125, 0);
    r.domain.center_z = axis(5.125, 5.125, 0);
    r.domain.radius = axis(4.875, 4.875, 0);
    r
}
fn assert_no_minimum(r: &SearchResultV1) {
    assert!(!r.accepted);
    assert!(r.minima.fellenius.is_none() && r.minima.bishop.is_none());
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() <= 2e-9 + 2e-10 * expected.abs(),
        "actual={actual:.17e}, independent={expected:.17e}");
}

#[test]
fn search_eight_corner_reference_uses_all_common_final_tables() {
    let r = search_critical_circle(&request()).unwrap();
    assert_eq!(r.status, SearchRunStatus::CompletedMinimum, "{:?}", r.terminal_reason);
    assert!(r.accepted && !r.diagnostic_only);
    assert_eq!(r.execution_policy, "SerialOnly");
    assert!(r.arithmetic_environment.accepted);
    assert_eq!(r.candidates.len(), 8);
    assert_eq!(r.counters.jointly_eligible, 8);
    assert_eq!(r.examined_family.main_candidate_ids.len(), 8);
    assert_eq!(r.examined_family.eligible_candidate_ids.len(), 8);
    let ff = r.minima.fellenius.as_ref().unwrap();
    let fb = r.minima.bishop.as_ref().unwrap();
    assert_eq!(ff.candidate_id, 2);
    assert_eq!(fb.candidate_id, 2);
    near(ff.fine_factor, 6.854460903003482);
    near(fb.fine_factor, 7.2892325441301615);
    assert!(ff.boundary.original_box.iter().any(|v| *v));
    assert!(fb.boundary.original_box.iter().any(|v| *v));

    let expected: Value = serde_json::from_str(include_str!(
        "fixtures/candidate-factors-256-512.json"
    )).unwrap();
    // Every final pass, not just the selected C02, must retain bound evidence.
    let mut visited = std::collections::BTreeSet::new();
    for table in &r.retained_tables {
        if table.branch != "main" { continue; }
        let n = table.config.slices.target_slices;
        if n != 256 && n != 512 { continue; }
        if !visited.insert((table.candidate_id, n)) { continue; }
        let pass = &table.evaluation;
        assert!(pass.closure_passed);
        let ff = pass.fellenius.as_ref().unwrap();
        let fb = pass.bishop.as_ref().unwrap();
        assert_eq!(fb.whole_arc_contact.as_ref().unwrap().status,
            WholeArcContactStatus::PassedDeclaredModel);
        assert!(!fb.whole_arc_contact.as_ref().unwrap().eligible_for_critical_minimum);
        assert_eq!(table.source_fingerprint, r.source_fingerprint);
        assert!(!table.config.slices.verify_refinement);
        assert_eq!(pass.slices.len(), n + 4);
        let name = format!("C{:02}", table.candidate_id);
        let rows = expected["candidates"].as_array().expect("independent factor fixture candidates");
        let candidate = rows.iter().find(|row| row["candidate_id"] == name)
            .expect("frozen candidate");
        let row = &candidate["resolutions"][n.to_string()];
        near(ff.factor_of_safety, row["fellenius_factor_of_safety"].as_f64().unwrap());
        near(fb.factor_of_safety, row["bishop_factor_of_safety"].as_f64().unwrap());
    }
    assert_eq!(visited.len(), 16);
    assert!(r.counters.work.slice_rows > 0 && r.counters.work.bishop_slice_terms > 0);
    assert!(r.warnings.iter().any(|v| v.contains("SEARCH_DOMAIN_LIMITED")));
    assert_eq!(serde_json::to_vec(&r).unwrap().len() as u64, r.counters.output_bytes);
}

#[test]
fn search_cancellation_before_core_commit_clears_every_branch_minimum() {
    let mut q = singleton();
    q.sensitivity.additional_slice_pairs.push(FinalComparisonPair {
        coarse_requested: 512, fine_requested: 1024,
    });
    let token = AtomicBool::new(false);
    let mut cancelled_before_commit = false;
    let mut terminal = Vec::new();
    let r = search_critical_circle_cancellable(&q, &token, &mut |p: SearchProgress| {
        if p.stage == "publication_ready" {
            assert!(p.evaluated_candidates >= 1 && p.completed_evaluations > 1);
            cancelled_before_commit = true;
            token.store(true, Ordering::Relaxed);
        }
        if let Some(status) = p.terminal_status { terminal.push(status); }
    }).unwrap();
    assert!(cancelled_before_commit);
    assert_eq!(r.status, SearchRunStatus::Cancelled);
    assert_no_minimum(&r);
    assert!(!r.sensitivity.is_empty());
    assert!(r.sensitivity.iter().all(|branch|
        branch.minima.fellenius.is_none() && branch.minima.bishop.is_none()));
    assert_eq!(terminal, vec![SearchRunStatus::Cancelled]);
    assert_eq!(serde_json::to_vec(&r).unwrap().len() as u64, r.counters.output_bytes);
}

#[test]
fn search_cancellation_after_evaluated_candidate_preserves_no_accepted_minimum() {
    let token = AtomicBool::new(false);
    let mut callback_seen = false;
    let mut evaluated_before_cancel = 0;
    let r = search_critical_circle_cancellable(&request(), &token, &mut |p: SearchProgress| {
        if !callback_seen && p.evaluated_candidates >= 1 {
            callback_seen = true;
            evaluated_before_cancel = p.evaluated_candidates;
            assert!(p.completed_evaluations > 0 && p.actual_slice_rows > 0);
            token.store(true, Ordering::Relaxed);
        }
    }).unwrap();
    assert!(callback_seen && evaluated_before_cancel >= 1);
    assert_eq!(r.status, SearchRunStatus::Cancelled);
    assert_no_minimum(&r);
    assert!(r.counters.work.slice_rows > 0);
    assert!(r.counters.work.bishop_function_evaluations > 0);
    assert!(r.counters.evaluated_candidates >= 1);
    eprintln!("CANCEL_EVIDENCE {}", serde_json::json!({
        "source_fingerprint": r.source_fingerprint, "solver_version": r.solver_version,
        "status": r.status, "accepted": r.accepted,
        "callback_after_evaluated_candidates": evaluated_before_cancel,
        "completed_evaluations": r.counters.candidate_evaluations,
        "evaluated_candidates": r.counters.evaluated_candidates,
        "work": r.counters.work, "accepted_minima_absent":
            r.minima.fellenius.is_none() && r.minima.bishop.is_none(),
    }));
}

#[test]
fn search_method_failure_keeps_fellenius_and_exposes_joint_family_restriction() {
    let mut q = singleton();
    q.shared_analysis.bishop.max_iterations = 1;
    let r = search_critical_circle(&q).unwrap();
    assert_eq!(r.status, SearchRunStatus::CompletedNoEligibleCandidates);
    assert_no_minimum(&r);
    let candidate = &r.candidates[0];
    assert!(candidate.fellenius.raw_factor.unwrap().is_finite());
    assert_eq!(candidate.fellenius.own_prerequisites.state, CheckState::Passed);
    assert_ne!(candidate.bishop.own_prerequisites.state, CheckState::Passed);
    assert_ne!(candidate.fellenius.selected_family_eligibility.state, CheckState::Passed);
    assert_eq!(r.counters.fellenius_own_eligible, 1);
    assert_eq!(r.counters.jointly_eligible, 0);
}

#[test]
fn search_unresolved_contact_is_not_eligible_and_raw_observations_survive() {
    let mut q = singleton();
    q.shared_analysis.contact.max_intervals = 1;
    let r = search_critical_circle(&q).unwrap();
    assert_no_minimum(&r);
    assert_eq!(r.status, SearchRunStatus::CompletedNoEligibleCandidates);
    assert!(r.candidates[0].fellenius.raw_factor.is_some());
    assert!(r.candidates[0].bishop.raw_factor.is_some());
    assert_ne!(r.candidates[0].bishop.physical.state, CheckState::Passed);
}

#[test]
fn search_contact_x_interval_wire_contract_preserves_legacy_unavailability() {
    for enclosure in [serde_json::Value::Null,
        serde_json::json!({"lower_m": -4.1, "upper_m": -3.9})] {
        let legacy = serde_json::json!({
            "nominal": {"x": -4.0, "z": 2.0}, "x_enclosure": enclosure,
        });
        let decoded: ResolvedContact = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(decoded.contact_x_interval, ContactXInterval::Unavailable);
        assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
    }
    let computed = serde_json::json!({
        "nominal": {"x": -4.0, "z": 2.0},
        "x_enclosure": {"lower_m": -4.1, "upper_m": -3.9},
        "contact_x_interval": {"status": "COMPUTED",
            "interval": {"lower_m": -4.1, "upper_m": -3.9}},
    });
    let decoded: ResolvedContact = serde_json::from_value(computed.clone()).unwrap();
    assert!(matches!(decoded.contact_x_interval, ContactXInterval::Computed { .. }));
    assert_eq!(serde_json::to_value(decoded).unwrap(), computed);
    for state in ["NOT_REQUESTED", "UNRESOLVED"] {
        let mut noncomputed = computed.clone();
        noncomputed["contact_x_interval"] = serde_json::json!({"status": state});
        assert!(serde_json::from_value::<ResolvedContact>(noncomputed.clone()).is_err());
        noncomputed["x_enclosure"] = Value::Null;
        let decoded: ResolvedContact = serde_json::from_value(noncomputed.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), noncomputed);
    }
    for invalid in [
        serde_json::json!({"lower_m": -4.2, "upper_m": -3.9}),
        serde_json::json!({"lower_m": -3.9, "upper_m": -4.1}),
        serde_json::json!({"lower_m": -3.9, "upper_m": -3.8}),
        Value::Null,
    ] {
        let mut inconsistent = computed.clone();
        inconsistent["contact_x_interval"]["interval"] = invalid;
        assert!(serde_json::from_value::<ResolvedContact>(inconsistent).is_err());
    }
    let mut missing_projection = computed;
    missing_projection["x_enclosure"] = Value::Null;
    assert!(serde_json::from_value::<ResolvedContact>(missing_projection).is_err());
}

#[test]
fn search_contact_x_interval_not_requested_without_filters() {
    let r = search_critical_circle(&singleton()).unwrap();
    let contacts = r.candidates[0].contacts.as_ref().unwrap();
    for contact in [&contacts.entry, &contacts.exit] {
        assert_eq!(contact.contact_x_interval, ContactXInterval::NotRequested);
        assert_eq!(contact.x_enclosure, None);
    }
    let wire = serde_json::to_value(&r).unwrap();
    assert_eq!(wire["candidates"][0]["contacts"]["entry"]["contact_x_interval"]["status"],
        "NOT_REQUESTED");
    let decoded: SearchResultV1 = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.candidates, r.candidates);
}

#[test]
fn search_direction_owned_filters_use_outward_contact_enclosures() {
    let mut q = singleton();
    q.domain.allowed_entry_x = Some(ClosedXRange { min_m: -5.0, max_m: -3.0 });
    q.domain.allowed_exit_x = Some(ClosedXRange { min_m: 2.0, max_m: 4.0 });
    let r = search_critical_circle(&q).unwrap();
    assert!(r.accepted, "{:?}", r.candidates[0].reasons);
    let contacts = r.candidates[0].contacts.as_ref().unwrap();
    assert!(contacts.entry.x_enclosure.is_some() && contacts.exit.x_enclosure.is_some());
    for contact in [&contacts.entry, &contacts.exit] {
        assert_eq!(contact.contact_x_interval,
            ContactXInterval::Computed { interval: contact.x_enclosure.unwrap() });
    }
    assert!(contacts.entry.nominal.x < contacts.exit.nominal.x);
    let decoded: SearchResultV1 = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
    assert_eq!(decoded.candidates, r.candidates);

    q.direction = SlideDirection::NegativeX;
    q.domain.allowed_entry_x = Some(ClosedXRange { min_m: 2.0, max_m: 4.0 });
    q.domain.allowed_exit_x = Some(ClosedXRange { min_m: -5.0, max_m: -3.0 });
    let r = search_critical_circle(&q).unwrap();
    let c = &r.candidates[0];
    assert_eq!(c.entry_filter.state, CheckState::Passed);
    assert_eq!(c.exit_filter.state, CheckState::Passed);
    let contacts = c.contacts.as_ref().unwrap();
    assert!(contacts.entry.nominal.x > contacts.exit.nominal.x);
    assert!(matches!(contacts.entry.contact_x_interval, ContactXInterval::Computed { .. }));
    assert!(matches!(contacts.exit.contact_x_interval, ContactXInterval::Computed { .. }));
    let decoded: SearchResultV1 = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
    assert_eq!(decoded.candidates, r.candidates);

    let mut q = singleton();
    let input = family().remove(2);
    let g = build_failure_mass(&input, &q.shared_analysis.fixed(64)).unwrap();
    q.domain.allowed_entry_x = Some(ClosedXRange {
        min_m: g.circle.entry_point.x, max_m: g.circle.entry_point.x,
    });
    let r = search_critical_circle(&q).unwrap();
    assert_no_minimum(&r);
    assert_eq!(r.candidates[0].entry_filter.state, CheckState::Unresolved);
}

#[test]
fn search_refinement_dedup_and_sensitivity_keep_separate_families() {
    let mut q = singleton();
    q.domain.radius = axis(4.875, 5.125, 1);
    q.refinement.levels = 1;
    q.sensitivity.uniform_grid_bisections = 1;
    q.sensitivity.additional_slice_pairs.push(FinalComparisonPair {
        coarse_requested: 512, fine_requested: 1024,
    });
    let r = search_critical_circle(&q).unwrap();
    assert!(r.accepted, "{:?}", r.terminal_reason);
    assert_eq!(r.refinement.len(), 1);
    assert!(r.refinement[0].seeds.len() >= 2);
    assert!(r.counters.duplicate_proposals > 0);
    assert_eq!(r.examined_family.main_candidate_ids.len(), 3);
    assert!(r.counters.unique_candidates >= 3);
    assert!(r.sensitivity.len() >= 2);
    assert!(r.sensitivity.iter().any(|s| s.final_pair.fine_requested == 1024));
    for s in &r.sensitivity {
        assert!(!s.observations.is_empty());
        assert!(!s.eligible_set_hash.is_empty());
    }
    assert_eq!(r.minima.bishop.as_ref().unwrap().final_pair.fine_requested, 512);
}

#[test]
fn search_original_eight_corner_refinement_preserves_all_21_candidates() {
    let mut q = request();
    q.refinement.levels = 1;
    let r = search_critical_circle(&q).unwrap();
    assert!(r.accepted, "{:?}", r.terminal_reason);
    assert_eq!(r.counters.generated_proposals, 89);
    assert_eq!(r.counters.duplicate_proposals, 68);
    assert_eq!(r.counters.unique_candidates, 21);
    assert_eq!(r.examined_family.main_candidate_ids.len(), 21);
    let base: Vec<_> = r.proposal_outcomes.iter().filter_map(|proposal| {
        matches!(proposal.origin, ProposalOrigin::BaseGrid { .. }).then_some(proposal.proposal_id)
    }).collect();
    assert_eq!(base, (0..8).collect::<Vec<_>>());
    assert_eq!(r.refinement.len(), 1);
    assert_eq!(r.refinement[0].seeds, vec![2, 3, 6]);
    assert_eq!(r.refinement[0].proposal_count, 81);
    for minimum in [&r.minima.fellenius, &r.minima.bishop] {
        assert!(minimum.as_ref().unwrap().label.contains("within_jointly_eligible_examined_family"));
    }
}

#[test]
fn search_near_ties_keep_all_eight_candidates_in_deterministic_order() {
    // A separate narrow eight-corner control exercises near-tie reporting;
    // it does not replace the original eight-corner/refinement domain above.
    let mut q = request();
    q.domain.center_x = axis(-1e-8, 1e-8, 1);
    q.domain.center_z = axis(5.0 - 1e-8, 5.0 + 1e-8, 1);
    q.domain.radius = axis(5.0 - 1e-8, 5.0 + 1e-8, 1);
    q.near_tie_abs = 1e-5;
    q.near_tie_rel = 0.0;
    let first = search_critical_circle(&q).unwrap();
    let second = search_critical_circle(&q).unwrap();
    assert!(first.accepted && second.accepted);
    assert_eq!(first.counters.unique_candidates, 8);
    assert_eq!(first.candidates, second.candidates);
    assert_eq!(first.proposal_outcomes, second.proposal_outcomes);
    assert_eq!(first.minima, second.minima);
    for minimum in [&first.minima.fellenius, &first.minima.bishop] {
        assert_eq!(minimum.as_ref().unwrap().near_ties, (0..8).collect::<Vec<_>>());
    }
}

#[test]
fn search_repeat_is_deterministic_without_claiming_parallel_acceptance() {
    let q = singleton();
    let before = q.clone();
    let first = search_critical_circle(&q).unwrap();
    let second = search_critical_circle(&q).unwrap();
    assert_eq!(q, before);
    assert_eq!(first.request_fingerprint, second.request_fingerprint);
    assert_eq!(first.examined_family, second.examined_family);
    assert_eq!(first.minima, second.minima);
    assert_eq!(first.candidates, second.candidates);
    assert_eq!(first.counters.work, second.counters.work);
    assert_eq!(first.parallel_determinism, "NotTested");
}

#[test]
fn search_admission_and_output_limits_do_not_accept_partial_minima() {
    let mut q = request();
    q.max_candidates = 7;
    assert!(search_critical_circle(&q).is_err());
    let mut q = request();
    q.domain.center_x.intervals = u32::MAX;
    q.domain.center_z.intervals = u32::MAX;
    q.domain.radius.intervals = u32::MAX;
    assert!(search_critical_circle(&q).is_err());
    let mut q = request();
    q.domain.center_x = axis(1.0e16, 1.0e16 + 2.0, 4);
    assert!(search_critical_circle(&q).is_err());
    let mut q = singleton();
    q.shared_analysis.limits.max_result_bytes = 1024;
    match search_critical_circle(&q) { Ok(r) => assert_no_minimum(&r), Err(e) =>
        assert_eq!(e.code, CircularErrorCode::ResourceLimit) }
    let mut q = singleton();
    q.final_pair.fine_requested = 513;
    assert!(search_critical_circle(&q).is_err());
    let mut value = serde_json::to_value(singleton()).unwrap();
    value["eligible_for_critical_minimum"] = true.into();
    assert!(serde_json::from_value::<SearchPublicRequestV1>(value).is_err());
}

#[test]
fn search_diagnostic_output_never_grants_a_minimum() {
    let mut q = singleton();
    q.output_mode = SearchOutputMode::Diagnostic;
    let r = search_critical_circle(&q).unwrap();
    assert!(r.diagnostic_only);
    assert_no_minimum(&r);
    assert!(r.counters.jointly_eligible > 0);
    assert!(r.diagnostic_best_so_far.fellenius.is_some());
    assert!(r.diagnostic_best_so_far.bishop.is_some());
    let encoded = serde_json::to_vec(&r).unwrap();
    let decoded: SearchResultV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.request_fingerprint, r.request_fingerprint);
}

#[test]
fn search_arithmetic_guard_observes_the_actual_worker_without_changing_controls() {
    let first = inspect_arithmetic_environment();
    let second = inspect_arithmetic_environment();
    assert!(first.accepted && first.round_to_nearest && first.gradual_underflow_probe_passed);
    assert!(!first.flush_to_zero && !first.denormals_are_zero);
    assert_eq!(first.mxcsr, second.mxcsr);
    assert_eq!(first.x87_control_word, second.x87_control_word);
}

#[test]
fn search_independent_callers_are_deterministic_across_thread_start_orders() {
    use std::sync::{Arc, mpsc};
    fn run(q: Arc<SearchPublicRequestV1>, order: [usize; 3]) -> Vec<Result<SearchResultV1, CircularError>> {
        let mut starters = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..3 {
            let (start, ready) = mpsc::channel();
            starters.push(start);
            let q = Arc::clone(&q);
            workers.push(std::thread::spawn(move || {
                ready.recv().unwrap();
                search_critical_circle(&q)
            }));
        }
        for index in order { starters[index].send(()).unwrap(); }
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    }
    let q = Arc::new(singleton());
    let one = run(Arc::clone(&q), [0, 1, 2]);
    let two = run(Arc::clone(&q), [2, 0, 1]);
    let expected = one[0].as_ref().unwrap();
    for result in one.iter().chain(&two) {
        let result = result.as_ref().unwrap();
        assert!(result.accepted && result.arithmetic_environment.accepted);
        assert_eq!(result.status, SearchRunStatus::CompletedMinimum);
        assert_eq!(result.request_fingerprint, expected.request_fingerprint);
        assert_eq!(result.examined_family, expected.examined_family);
        assert_eq!(result.proposal_outcomes, expected.proposal_outcomes);
        assert_eq!(result.candidates, expected.candidates);
        assert_eq!(result.minima, expected.minima);
        assert_eq!(result.counters.work, expected.counters.work);
    }
    let mut invalid = singleton();
    invalid.max_candidates = 0;
    let invalid = Arc::new(invalid);
    let errors_one = run(Arc::clone(&invalid), [1, 0, 2]);
    let errors_two = run(invalid, [2, 1, 0]);
    let expected = errors_one[0].as_ref().unwrap_err();
    for result in errors_one.iter().chain(&errors_two) {
        let error = result.as_ref().unwrap_err();
        assert_eq!(error.code, expected.code);
        assert_eq!(error.message, expected.message);
    }
    eprintln!("CALLER_ORDER_EVIDENCE {}", serde_json::json!({
        "source_fingerprint": one[0].as_ref().unwrap().source_fingerprint,
        "completed_callers": one.len() + two.len(), "start_orders": [[0,1,2],[2,0,1]],
        "equal_numerical_candidates_families_minima": true,
        "equal_error_callers": errors_one.len() + errors_two.len(),
        "execution_policy": "SerialOnly independent calling threads",
        "internal_parallel_executor_tested": false,
    }));
}
