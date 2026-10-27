use super::*;
use ai_revit_core::{Command, EngineeringCommand, CircularCaseDefinition, GroundDefinition,
    GroundSectionId, CommitReceipt, CircularSearchSpecV1};
use std::time::{Duration, Instant};

fn context() -> HostContext { HostContext { session_id: "authenticated-test-session".into(), open_generation: DecimalU64(1) } }
fn apply(doc: &mut Document, key: &str, command: Command) -> CommitReceipt {
    doc.commit(doc.begin_transaction(key, doc.revision()).with(command)).unwrap()
}
fn fixture() -> (Document, GroundSectionId, AnalysisCaseId, GroundSectionId, AnalysisCaseId) {
    assert_eq!(source_fingerprint(), "sha256:dbb13bc6773d4ba6c6a3aaed2d3d9e71407ddc09458c98955fd32588cb1963e8");
    let raw: serde_json::Value = serde_json::from_str(include_str!("../fixtures/family-inputs.json")).unwrap();
    let input: CircularAnalysisInput = serde_json::from_value(raw["inputs"][0].clone()).unwrap();
    let g = input.ground;
    let definition = GroundDefinition { coordinate_system: g.coordinate_system, terrain_polyline: g.terrain_polyline,
        layer_interfaces: g.layer_interfaces, soil_regions: g.soil_regions, bottom_boundary: g.bottom_boundary, materials: g.materials };
    let mut doc = Document::new("Actual durable03 host adapter fixture").unwrap();
    let first = apply(&mut doc, "ground-a", Command::Engineering(EngineeringCommand::CreateGround { definition: definition.clone(), name: "Ground A".into() })).delta.engineering.created_ground_sections[0];
    let other = apply(&mut doc, "ground-b", Command::Engineering(EngineeringCommand::CreateGround { definition, name: "Ground B".into() })).delta.engineering.created_ground_sections[0];
    let mut config = CircularAnalysisConfig::default(); config.slices.target_slices = 64; config.slices.verify_refinement = false;
    config.bishop.step_abs = 1e-12; config.bishop.step_rel = 1e-12; config.bishop.residual_abs = 1e-12; config.bishop.residual_rel = 1e-12;
    let make = |ground| CircularCaseDefinition { ground_section_id: ground, slip_surface: input.slip_surface.clone(), basis: input.basis,
        units: input.units, extrusion_width_m: input.extrusion_width_m, config, search_spec: Some(search_spec()) };
    let a = apply(&mut doc, "case-a", Command::Engineering(EngineeringCommand::CreateCircular { definition: make(first), name: "Case A".into() })).delta.engineering.created_cases[0];
    let b = apply(&mut doc, "case-b", Command::Engineering(EngineeringCommand::CreateCircular { definition: make(other), name: "Case B".into() })).delta.engineering.created_cases[0];
    (doc, first, a, other, b)
}
fn finish(host: &mut CircularHostController, doc: &Document, ctx: &HostContext) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !host.is_quiescent() { host.poll(doc, ctx); assert!(Instant::now() < deadline); std::thread::yield_now(); }
}
fn edit(doc: &mut Document, ground: GroundSectionId, key: &str) {
    let mut definition = doc.ground_section(ground).unwrap().definition.clone();
    definition.materials[0].properties.effective_cohesion_kpa += 0.25;
    apply(doc, key, Command::Engineering(EngineeringCommand::UpdateGround { ground_section_id: ground, definition, name: None }));
}
fn search_spec() -> CircularSearchSpecV1 {
    let axis = |v| InclusiveAxis { min: v, max: v, intervals: 0 };
    CircularSearchSpecV1 { schema_version: 1,
        domain: SearchDomain { center_x: axis(-0.125), center_z: axis(5.125), radius: axis(4.875), allowed_entry_x: None, allowed_exit_x: None },
        family_policy: FamilyPolicy::JointlyEligibleV1, target_method: TargetMethod::Both,
        exploration_slices: ExplorationSlices { initial_requested: 64, max_passes: 3, require_joint_consecutive_convergence: true },
        final_pair: FinalComparisonPair { coarse_requested: 256, fine_requested: 512 },
        refinement: NeighborhoodRefinement { levels: 0, promising_per_method: 2 },
        sensitivity: SensitivityPlan { uniform_grid_bisections: 0, additional_slice_pairs: vec![] },
        core_policy_profile: CorePolicyProfile::ProvisionalSerialV1, output_mode: SearchOutputMode::Strict,
        max_candidates: 4096, max_wall_time_ms: 120_000, near_tie_abs: 1e-10, near_tie_rel: 1e-10 }
}

#[test]
fn host_actual_shared_ground_edit_undo_before_any_poll_stays_permanently_stale() {
    let (mut doc, ground, case, _, _) = fixture();
    let shared_definition = doc.circular_case(case).unwrap().definition.clone();
    let shared = apply(&mut doc, "shared-case", Command::Engineering(EngineeringCommand::CreateCircular { definition: shared_definition, name: "Shared A".into() })).delta.engineering.created_cases[0];
    let ctx = context(); let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let original = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &original).unwrap(); finish(&mut host, &doc, &ctx);
    let pending = host.quote_specified(&doc, &ctx, DecimalU64(shared.get())).unwrap();
    host.submit(&doc, &ctx, &pending).unwrap();
    edit(&mut doc, ground, "ground-edit"); host.after_mutation(&doc, &ctx);
    doc.undo().unwrap(); host.after_mutation(&doc, &ctx); // no poll between edit/undo
    finish(&mut host, &doc, &ctx);
    for id in [original.sequence, pending.sequence] {
        assert_eq!(host.status(&doc, &ctx, id).unwrap().phase, JobPhase::Stale);
        assert!(host.result_page(&doc, &ctx, id, PageKind::ArchiveBytes, None, 1, 8192).is_err());
    }
    // Original request recovery is allowed after restored or changed source;
    // receipt remains stale and no sequence/new worker is created.
    assert_eq!(host.submit(&doc, &ctx, &original).unwrap().phase, JobPhase::Stale);
    assert_eq!(host.next_sequence().0, 3); assert!(host.is_quiescent());
}

#[test]
fn host_noop_rename_unrelated_ground_keep_result_but_rebind_invalidates() {
    let (mut doc, ground, case, unrelated, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let quote = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &quote).unwrap(); finish(&mut host, &doc, &ctx);
    for (key, name) in [("rename", Some("Renamed ground".into())), ("noop", None)] {
        let definition = doc.ground_section(ground).unwrap().definition.clone();
        apply(&mut doc, key, Command::Engineering(EngineeringCommand::UpdateGround { ground_section_id: ground, definition, name }));
        host.after_mutation(&doc, &ctx);
        assert_eq!(host.status(&doc, &ctx, quote.sequence).unwrap().phase, JobPhase::Completed);
    }
    let definition = doc.circular_case(case).unwrap().definition.clone();
    apply(&mut doc, "case-rename", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: Some("Renamed case".into()) }));
    host.after_mutation(&doc, &ctx);
    edit(&mut doc, unrelated, "other-ground-edit"); host.after_mutation(&doc, &ctx);
    assert!(host.status(&doc, &ctx, quote.sequence).unwrap().result_available);
    let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.ground_section_id = unrelated;
    apply(&mut doc, "rebind", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    host.after_mutation(&doc, &ctx);
    assert_eq!(host.status(&doc, &ctx, quote.sequence).unwrap().phase, JobPhase::Stale);
}

#[test]
fn host_unknown_reply_original_envelope_retry_after_edit_and_mutated_request_gone() {
    let (mut doc, ground, case, _, _) = fixture(); let ctx = context();
    let mut limits = HostLimits::default(); limits.jobs.max_receipts = 1;
    let mut host = CircularHostController::new(limits).unwrap();
    let original = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    let admitted = host.submit(&doc, &ctx, &original).unwrap(); // admission reply treated as lost
    edit(&mut doc, ground, "edited"); host.after_mutation(&doc, &ctx);
    let retry = host.submit(&doc, &ctx, &original).unwrap();
    assert_eq!(admitted.request_digest, retry.request_digest); assert_eq!(retry.phase, JobPhase::Stale);
    assert_eq!(host.next_sequence().0, 2);
    assert_eq!(host.submit(&doc, &ctx, &original).unwrap().phase, JobPhase::Stale);
    let mut changed = original.clone();
    if let FullRequest::Search(q) = &mut changed.request { q.near_tie_abs *= 2.0; }
    assert!(matches!(host.submit(&doc, &ctx, &changed), Err(HostError::Job(jobs::JobError::SequenceReused))));
    let mut changed = original.clone(); changed.expected_revision.0 += 1;
    assert!(matches!(host.submit(&doc, &ctx, &changed), Err(HostError::Job(jobs::JobError::SequenceReused))));
    finish(&mut host, &doc, &ctx);
    let next = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &next).unwrap(); finish(&mut host, &doc, &ctx);
    assert!(matches!(host.submit(&doc, &ctx, &original), Err(HostError::Job(jobs::JobError::Gone))));
    assert_eq!(host.next_sequence().0, 3);
}

#[test]
fn host_current_epoch_rejects_old_doc_runtime_session_and_open_generation() {
    let (doc, _, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let original = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &original).unwrap(); finish(&mut host, &doc, &ctx);
    let mut changed_ctx = ctx.clone(); changed_ctx.open_generation.0 += 1;
    host.after_mutation(&doc, &changed_ctx);
    assert!(matches!(host.submit(&doc, &changed_ctx, &original), Err(HostError::ContextMismatch)));
    let mut changed_ctx = ctx.clone(); changed_ctx.session_id.push_str("-different");
    assert!(matches!(host.submit(&doc, &changed_ctx, &original), Err(HostError::ContextMismatch)));
    let reopened = Document::from_json_str(&doc.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.document_uuid, doc.document_uuid); assert_ne!(reopened.runtime_instance_id(), doc.runtime_instance_id());
    assert!(matches!(host.submit(&reopened, &ctx, &original), Err(HostError::ContextMismatch)));
    let other = Document::new("Another actual document").unwrap();
    assert!(matches!(host.submit(&other, &ctx, &original), Err(HostError::ContextMismatch)));
    assert_eq!(host.next_sequence().0, 2);
}

#[test]
fn host_new_admission_strict_full_source_config_revision_and_readiness() {
    let (mut doc, ground, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let original = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    for mutation in 0..4 {
        let mut altered = original.clone();
        match mutation {
            0 => altered.expected_revision.0 -= 1,
            1 => altered.binding.ground_source_operation.0 += 1,
            2 => altered.resolved_case_config.slices.target_slices += 1,
            _ => if let FullRequest::Search(q) = &mut altered.request { q.shared_analysis.bishop.step_abs *= 2.; },
        }
        assert!(host.submit(&doc, &ctx, &altered).is_err()); assert_eq!(host.next_sequence().0, 1);
    }
    apply(&mut doc, "draft-ground", Command::Engineering(EngineeringCommand::UpdateGround { ground_section_id: ground,
        definition: GroundDefinition::default(), name: None }));
    assert!(matches!(host.quote_specified(&doc, &ctx, DecimalU64(case.get())), Err(HostError::NotReady)));
    assert!(host.is_quiescent());
}

#[test]
fn host_bounded_pages_exact_opaque_cursors_and_completed_cache_staleness() {
    let (mut doc, ground, case, _, _) = fixture(); let ctx = context();
    let mut limits = HostLimits::default(); limits.max_cursors = 1;
    let mut host = CircularHostController::new(limits).unwrap();
    let quote = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &quote).unwrap(); finish(&mut host, &doc, &ctx);
    let kind = PageKind::Slices(TableSelector::Specified);
    let first = host.result_page(&doc, &ctx, quote.sequence, kind.clone(), None, 1, 8192).unwrap();
    assert_eq!(first.rows, 1); assert!(first.utf8.len() <= 8192);
    let cursor = first.next_cursor.unwrap(); assert_eq!(cursor.len(), 71);
    let second = host.result_page(&doc, &ctx, quote.sequence, kind.clone(), Some(&cursor), 1, 8192).unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first.utf8).unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second.utf8).unwrap();
    assert_eq!(first_json["offset"], "0"); assert_eq!(second_json["offset"], "1");
    assert_eq!(first_json["sequence"], quote.sequence.0.to_string());
    assert!(matches!(host.result_page(&doc, &ctx, quote.sequence, kind.clone(), Some(&cursor), 1, 8192), Err(HostError::CursorGone)));
    let current_cursor = second.next_cursor.unwrap();
    assert!(matches!(host.result_page(&doc, &ctx, quote.sequence, PageKind::ArchiveBytes, Some(&current_cursor), 1, 8192), Err(HostError::CursorMismatch)));
    assert!(matches!(host.result_page(&doc, &ctx, quote.sequence, kind.clone(), None, 65, 8192), Err(HostError::InvalidPage)));
    assert!(matches!(host.result_page(&doc, &ctx, quote.sequence, kind.clone(), None, 64, 1024), Err(HostError::PageBudget)));
    edit(&mut doc, ground, "stale-page"); host.after_mutation(&doc, &ctx);
    assert!(host.result_page(&doc, &ctx, quote.sequence, kind, Some(&current_cursor), 1, 8192).is_err());
    assert_eq!(host.status(&doc, &ctx, quote.sequence).unwrap().phase, JobPhase::Stale);
    host.begin_shutdown(); assert!(host.is_quiescent());
}

#[test]
fn host_actual_search_worker_fp_exact_request_and_nonblocking_shutdown() {
    let (doc, _, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    assert_eq!(host.limits.jobs.max_output_bytes, 8 << 20);
    let envelope = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    let admitted = host.submit(&doc, &ctx, &envelope).unwrap();
    finish(&mut host, &doc, &ctx);
    let status = host.status(&doc, &ctx, envelope.sequence).unwrap();
    assert_eq!(status.phase, JobPhase::Completed, "{:?}", status.failure);
    assert_eq!(status.core_status, Some(SearchRunStatus::CompletedMinimum));
    assert!(status.worker_arithmetic.unwrap().accepted);
    assert_eq!(admitted.request_digest, status.request_digest);
    let JobRequest::Search(request) = host.manager.result(envelope.sequence.0).unwrap().request() else { unreachable!() };
    let FullRequest::Search(original) = &envelope.request else { unreachable!() }; assert_eq!(request, original);
    let page = host.result_page(&doc, &ctx, envelope.sequence, PageKind::SearchCandidates, None, 1, 32 << 10).unwrap();
    assert_eq!(page.rows, 1); assert!(page.utf8.len() <= 32 << 10);
    host.begin_shutdown(); assert!(host.is_quiescent());
}

#[test]
fn host_decimal_transport_and_deep_finite_budget_rejection() {
    let max = DecimalU64(u64::MAX); assert_eq!(serde_json::to_string(&max).unwrap(), "\"18446744073709551615\"");
    assert_eq!(serde_json::from_str::<DecimalU64>("\"9007199254740993\"").unwrap().0, 9_007_199_254_740_993);
    for invalid in ["18446744073709551615", "\"01\"", "\"1e3\"", "\"18446744073709551616\"", "1.0"] {
        assert!(serde_json::from_str::<DecimalU64>(invalid).is_err());
    }
    let (doc, _, case, _, _) = fixture(); let ctx = context();
    let host = CircularHostController::new(HostLimits::default()).unwrap();
    let envelope = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    let id = serde_json::json!("request-id");
    let line = encode_submission_line(&envelope, TransportHeader { id: &id, method: "geotech.circular.analyze", token: Some("test-only-auth-not-logged") }).unwrap();
    assert!(line.len() <= transport::MAX_REQUEST_LINE_BYTES && line.ends_with(b"\n"));
    let parsed: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert!(parsed["params"]["request"]["request"]["input"]["ground"]["model_revision"].is_string());
    let decoded: Submission = serde_json::from_value(parsed["params"].clone()).unwrap(); assert_eq!(decoded, envelope);
    let mut wide = envelope.clone();
    if let FullRequest::Specified { input, .. } = &mut wide.request { input.ground.model_revision = u64::MAX; }
    wide.binding.ground_source_operation = DecimalU64(u64::MAX);
    let bytes = serde_json::to_vec(&wide).unwrap();
    assert_eq!(decode_submission_bytes(&bytes).unwrap(), wide);
    let huge = serde_json::Value::String("x".repeat(transport::WRAPPER_RESERVE_BYTES + 1));
    assert!(encode_submission_line(&envelope, TransportHeader { id: &huge, method: "m", token: None }).is_err());
    let mut nonfinite = envelope.clone();
    if let FullRequest::Specified { input, .. } = &mut nonfinite.request { input.ground.materials[0].properties.effective_cohesion_kpa = f64::NAN; }
    assert!(matches!(host.audit_envelope(&nonfinite), Err(HostError::InvalidInput)));
    assert!(encode_submission_line(&nonfinite, TransportHeader { id: &id, method: "m", token: None }).is_err());
}

#[test]
fn host02_current_result_borrows_exact_saved_typed_request_output_and_fp() {
    let (doc, _, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let envelope = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &envelope).unwrap(); finish(&mut host, &doc, &ctx);
    let address = host.manager.result(envelope.sequence.0).unwrap() as *const jobs::PublishedResult;
    let expected = doc.resolve_circular_search(case).unwrap();
    {
        let saved = host.current_result(&doc, &ctx, envelope.sequence).unwrap();
        assert!(std::ptr::eq(address, saved)); // exact cache borrow; no copied result
        assert_eq!(saved.binding.ground_source_operation.0, expected.ground_source_operation_id.get());
        assert!(saved.worker_arithmetic.accepted);
        let JobRequest::Search(request) = saved.request() else { unreachable!() };
        assert_eq!(request, &expected.request);
        let jobs::JobOutput::Search(result) = saved.output() else { unreachable!() };
        assert_eq!(result.status, SearchRunStatus::CompletedMinimum);
        assert!(result.warnings.iter().any(|v| v == "SEARCH_DOMAIN_LIMITED"));
    }
    assert_eq!(host.next_sequence().0, 2); host.begin_shutdown(); assert!(host.is_quiescent());
}

#[test]
fn host02_current_result_rejects_ground_rebind_aba_open_and_runtime_changes() {
    for change in 0..5 {
        let (mut doc, ground, case, other, _) = fixture(); let mut ctx = context();
        let mut host = CircularHostController::new(HostLimits::default()).unwrap();
        let envelope = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
        host.submit(&doc, &ctx, &envelope).unwrap(); finish(&mut host, &doc, &ctx);
        match change {
            0 => edit(&mut doc, ground, "edit-before-borrow"),
            1 => {
                let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.ground_section_id = other;
                apply(&mut doc, "rebind-before-borrow", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
            }
            2 => {
                edit(&mut doc, ground, "aba-before-borrow"); host.after_mutation(&doc, &ctx);
                doc.undo().unwrap(); host.after_mutation(&doc, &ctx); // no poll
            }
            3 => { ctx.open_generation.0 += 1; host.after_mutation(&doc, &ctx); }
            _ => { doc = Document::from_json_str(&doc.to_json_string().unwrap()).unwrap(); }
        }
        assert!(host.current_result(&doc, &ctx, envelope.sequence).is_err(), "change {change}");
        assert_eq!(host.next_sequence().0, 2); assert!(host.is_quiescent());
        assert!(!host.manager.status(envelope.sequence.0).unwrap().result_available);
    }
}

#[test]
fn host02_current_result_never_creates_result_after_cancel_or_actual_core_failure() {
    let (mut doc, _, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let search = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &search).unwrap(); host.cancel(&doc, &ctx, search.sequence).unwrap();
    assert!(host.current_result(&doc, &ctx, search.sequence).is_err());
    finish(&mut host, &doc, &ctx);
    assert_eq!(host.submit(&doc, &ctx, &search).unwrap().phase, JobPhase::Cancelled);
    assert!(host.current_result(&doc, &ctx, search.sequence).is_err());
    let mut definition = doc.circular_case(case).unwrap().definition.clone();
    definition.slip_surface.center = geotech_core::Point2::new(100., 100.); definition.slip_surface.radius_m = 1.;
    apply(&mut doc, "actual-invalid-circle", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    host.after_mutation(&doc, &ctx);
    let failed = host.quote_specified(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &failed).unwrap(); finish(&mut host, &doc, &ctx);
    assert_eq!(host.status(&doc, &ctx, failed.sequence).unwrap().phase, JobPhase::Failed);
    assert!(host.current_result(&doc, &ctx, failed.sequence).is_err());
    assert_eq!(host.submit(&doc, &ctx, &failed).unwrap().phase, JobPhase::Failed);
    assert_eq!(host.next_sequence().0, 3); assert!(host.is_quiescent());
}

#[test]
fn host02_saved_spec_required_exact_admission_and_retry_survives_spec_removal() {
    let (mut doc, _, case, _, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let original = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    let mut runtime_override = original.clone();
    if let FullRequest::Search(q) = &mut runtime_override.request { q.near_tie_abs *= 2.; }
    assert!(matches!(host.submit(&doc, &ctx, &runtime_override), Err(HostError::SourceMismatch)));
    assert_eq!(host.next_sequence().0, 1);
    let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.search_spec = None;
    apply(&mut doc, "clear-spec", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    assert!(matches!(host.quote_search(&doc, &ctx, DecimalU64(case.get())), Err(HostError::SearchSpecNotConfigured)));
    assert!(host.quote_specified(&doc, &ctx, DecimalU64(case.get())).is_ok());
    let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.search_spec = Some(search_spec());
    apply(&mut doc, "save-spec", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    let admitted = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &admitted).unwrap();
    let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.search_spec = None;
    apply(&mut doc, "clear-after-admission", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    host.after_mutation(&doc, &ctx);
    let replay = host.submit(&doc, &ctx, &admitted).unwrap();
    assert_eq!(replay.phase, JobPhase::Stale); assert_eq!(host.next_sequence().0, 2);
    finish(&mut host, &doc, &ctx);
    assert!(host.current_result(&doc, &ctx, admitted.sequence).is_err());
}

#[test]
fn host02_saved_spec_edit_undo_before_poll_is_permanent_and_rebind_resolves_actual_ground() {
    let (mut doc, _, case, other, _) = fixture(); let ctx = context();
    let mut host = CircularHostController::new(HostLimits::default()).unwrap();
    let old = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    host.submit(&doc, &ctx, &old).unwrap();
    let mut definition = doc.circular_case(case).unwrap().definition.clone();
    definition.search_spec.as_mut().unwrap().near_tie_abs *= 2.;
    apply(&mut doc, "edit-spec", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    host.after_mutation(&doc, &ctx); doc.undo().unwrap(); host.after_mutation(&doc, &ctx); // no poll
    finish(&mut host, &doc, &ctx);
    assert_eq!(host.submit(&doc, &ctx, &old).unwrap().phase, JobPhase::Stale);
    let mut definition = doc.circular_case(case).unwrap().definition.clone(); definition.ground_section_id = other;
    apply(&mut doc, "rebind-search", Command::Engineering(EngineeringCommand::UpdateCircular { analysis_id: case, definition, name: None }));
    host.after_mutation(&doc, &ctx);
    let new = host.quote_search(&doc, &ctx, DecimalU64(case.get())).unwrap();
    let FullRequest::Search(request) = &new.request else { unreachable!() };
    assert_eq!(request, &doc.resolve_circular_search(case).unwrap().request);
    assert_eq!(new.binding.ground_id.0, other.get());
    assert_ne!(new.binding.case_source_operation, old.binding.case_source_operation);
    host.submit(&doc, &ctx, &new).unwrap(); host.cancel(&doc, &ctx, new.sequence).unwrap(); finish(&mut host, &doc, &ctx);
    assert!(host.current_result(&doc, &ctx, old.sequence).is_err()); assert_eq!(host.next_sequence().0, 3);
}
