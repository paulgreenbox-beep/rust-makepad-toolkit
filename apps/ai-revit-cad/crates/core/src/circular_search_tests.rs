//! Persistence and resolution regressions for circular-search specifications.
//! These tests do not run a solver or establish engineering acceptance.
use crate::*;
use geotech_core::circular as c;
use geotech_core::{InputUnits, Point2 as GPoint, SoilMaterial};

fn ground() -> GroundDefinition {
    GroundDefinition {
        coordinate_system: c::SectionCoordinateSystem::CartesianXzMetres,
        terrain_polyline: vec![GPoint::new(-6.0, 3.0), GPoint::new(6.0, 0.0)],
        layer_interfaces: vec![],
        soil_regions: vec![c::SoilRegion {
            region_id: "region-a".into(),
            material_id: "soil-a".into(),
        }],
        bottom_boundary: vec![GPoint::new(-6.0, -3.0), GPoint::new(6.0, -3.0)],
        materials: vec![c::GroundMaterial {
            material_id: "soil-a".into(),
            properties: SoilMaterial {
                name: "Search specification fixture soil".into(),
                unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 0.0,
                effective_friction_angle_deg: 28.0,
                provenance: "Explicit test fixture; not engineering soil acceptance".into(),
            },
        }],
    }
}

fn search_spec() -> CircularSearchSpecV1 {
    CircularSearchSpecV1 {
        schema_version: 1,
        domain: c::SearchDomain {
            center_x: c::InclusiveAxis {
                min: 0.0,
                max: 0.0,
                intervals: 0,
            },
            center_z: c::InclusiveAxis {
                min: 5.0,
                max: 5.0,
                intervals: 0,
            },
            radius: c::InclusiveAxis {
                min: 4.875,
                max: 4.875,
                intervals: 0,
            },
            allowed_entry_x: None,
            allowed_exit_x: None,
        },
        family_policy: c::FamilyPolicy::JointlyEligibleV1,
        target_method: c::TargetMethod::Both,
        exploration_slices: c::ExplorationSlices {
            initial_requested: 8,
            max_passes: 3,
            require_joint_consecutive_convergence: true,
        },
        final_pair: c::FinalComparisonPair {
            coarse_requested: 32,
            fine_requested: 64,
        },
        refinement: c::NeighborhoodRefinement {
            levels: 0,
            promising_per_method: 2,
        },
        sensitivity: c::SensitivityPlan {
            uniform_grid_bisections: 0,
            additional_slice_pairs: vec![],
        },
        core_policy_profile: c::CorePolicyProfile::ProvisionalSerialV1,
        output_mode: c::SearchOutputMode::Strict,
        max_candidates: 4096,
        max_wall_time_ms: 120_000,
        near_tie_abs: 1e-10,
        near_tie_rel: 1e-10,
    }
}

fn circle(
    ground_section_id: GroundSectionId,
    search_spec: Option<CircularSearchSpecV1>,
) -> CircularCaseDefinition {
    CircularCaseDefinition {
        ground_section_id,
        slip_surface: c::CircularSlipSurface {
            slip_id: "explicit-slip".into(),
            center: GPoint::new(0.0, 5.0),
            radius_m: 4.875,
            arc_branch: c::ArcBranch::Lower,
            slide_direction: c::SlideDirection::PositiveX,
            selected_endpoints: None,
        },
        basis: c::AnalysisBasis::DryStatic,
        units: InputUnits::default(),
        extrusion_width_m: 1.0,
        config: c::CircularAnalysisConfig::default(),
        search_spec,
    }
}

fn create_ground(definition: GroundDefinition) -> Command {
    Command::Engineering(EngineeringCommand::CreateGround {
        definition,
        name: "Ground".into(),
    })
}

fn create_circle(id: GroundSectionId, spec: Option<CircularSearchSpecV1>) -> Command {
    Command::Engineering(EngineeringCommand::CreateCircular {
        definition: circle(id, spec),
        name: "Circle".into(),
    })
}

fn apply(document: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut transaction = document.begin_transaction(id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    document.commit(transaction).unwrap()
}

fn one(spec: Option<CircularSearchSpecV1>) -> (Document, GroundSectionId, AnalysisCaseId) {
    let mut document = Document::new("Circular search fixture").unwrap();
    let receipt = apply(&mut document, "ground", vec![create_ground(ground())]);
    let ground_id = receipt.delta.engineering.created_ground_sections[0];
    let receipt = apply(
        &mut document,
        "circle",
        vec![create_circle(ground_id, spec)],
    );
    (
        document,
        ground_id,
        receipt.delta.engineering.created_cases[0],
    )
}

fn wall() -> Command {
    Command::CreateElement {
        name: "Wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(5.0, 0.0),
            base_elevation_m: 0.0,
            height_m: 3.0,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    }
}

fn drawing() -> Command {
    Command::Draft(DraftCommand::CreateDrawing {
        name: "Section".into(),
        plane: DrawingPlane::default(),
    })
}

fn update_circle(
    id: AnalysisCaseId,
    definition: CircularCaseDefinition,
    name: Option<&str>,
) -> Command {
    Command::Engineering(EngineeringCommand::UpdateCircular {
        analysis_id: id,
        definition,
        name: name.map(str::to_owned),
    })
}

#[test]
fn circular_search_none_is_legacy_omitted_and_preserves_command_payload_bytes() {
    let (document, _ground_id, case_id) = one(None);
    let source = document.to_json_string().unwrap();
    let mut legacy: serde_json::Value = serde_json::from_str(&source).unwrap();
    let case_key = case_id.to_string();
    let definition =
        legacy["document"]["engineering"]["circular_cases"][case_key.as_str()]["definition"]
            .as_object_mut()
            .unwrap();
    // Model a native4 record written before search_spec existed.
    definition.remove("search_spec");
    let original_payload = legacy["document"]["command_receipts"]["circle"]["payload_json"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!original_payload.contains("search_spec"));

    let opened = Document::from_json_str(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(
        opened
            .circular_case(case_id)
            .unwrap()
            .definition
            .search_spec,
        None
    );
    let round_trip: serde_json::Value =
        serde_json::from_str(&opened.to_json_string().unwrap()).unwrap();
    assert!(
        round_trip["document"]["engineering"]["circular_cases"][case_key.as_str()]["definition"]
            .get("search_spec")
            .is_none()
    );
    assert_eq!(
        round_trip["document"]["command_receipts"]["circle"]["payload_json"],
        original_payload
    );

    let encoded = serde_json::to_value(search_spec()).unwrap();
    for forbidden in [
        "analysis_id",
        "ground_section_id",
        "search_id",
        "source_revision",
        "config",
        "source_operation_id",
        "basis",
        "units",
        "direction",
        "extrusion_width_m",
        "ground",
    ] {
        let mut forged = encoded.clone();
        forged[forbidden] = serde_json::json!("forged");
        assert!(
            serde_json::from_value::<CircularSearchSpecV1>(forged).is_err(),
            "accepted {forbidden}"
        );
    }
}

#[test]
fn circular_search_resolution_copies_saved_policy_and_uses_document_sources() {
    let (mut document, ground_id, case_id) = one(Some(search_spec()));
    let mut definition = document.circular_case(case_id).unwrap().definition.clone();
    let saved_spec = definition.search_spec.as_mut().unwrap();
    saved_spec.domain.allowed_entry_x = Some(c::ClosedXRange {
        min_m: -2.0,
        max_m: 2.0,
    });
    saved_spec.domain.allowed_exit_x = Some(c::ClosedXRange {
        min_m: -3.0,
        max_m: 3.0,
    });
    saved_spec.target_method = c::TargetMethod::Bishop;
    saved_spec.refinement = c::NeighborhoodRefinement {
        levels: 1,
        promising_per_method: 3,
    };
    saved_spec.sensitivity = c::SensitivityPlan {
        uniform_grid_bisections: 1,
        additional_slice_pairs: vec![c::FinalComparisonPair {
            coarse_requested: 64,
            fine_requested: 128,
        }],
    };
    saved_spec.output_mode = c::SearchOutputMode::Diagnostic;
    saved_spec.max_candidates = 17;
    saved_spec.max_wall_time_ms = 25_000;
    saved_spec.near_tie_abs = 2e-8;
    saved_spec.near_tie_rel = 3e-8;
    definition.config.tolerances.fs_abs = 2e-5;
    definition.config.limits.max_slices = 2048;
    definition.config.bishop.initial_factor = 1.2;
    definition.config.contact.max_depth = 39;
    definition.slip_surface.slide_direction = c::SlideDirection::NegativeX;
    definition.extrusion_width_m = 2.5;
    apply(
        &mut document,
        "set-nondefault-search-inputs",
        vec![update_circle(case_id, definition, None)],
    );

    let saved = document
        .circular_case(case_id)
        .unwrap()
        .definition
        .search_spec
        .as_ref()
        .unwrap();
    let actual_config = document.circular_case(case_id).unwrap().definition.config;
    let input = document.resolve_circular_input(case_id).unwrap();
    let resolved = document.resolve_circular_search(case_id).unwrap();
    let request = &resolved.request;
    assert_eq!(
        (
            request.schema_version,
            request.domain.clone(),
            request.family_policy,
            request.target_method,
            request.exploration_slices,
            request.final_pair,
        ),
        (
            saved.schema_version,
            saved.domain.clone(),
            saved.family_policy,
            saved.target_method,
            saved.exploration_slices,
            saved.final_pair,
        ),
    );
    assert_eq!(
        (
            request.refinement,
            request.sensitivity.clone(),
            request.core_policy_profile,
            request.output_mode,
            request.max_candidates,
            request.max_wall_time_ms,
            request.near_tie_abs,
        ),
        (
            saved.refinement,
            saved.sensitivity.clone(),
            saved.core_policy_profile,
            saved.output_mode,
            saved.max_candidates,
            saved.max_wall_time_ms,
            saved.near_tie_abs,
        ),
    );
    assert_eq!(request.near_tie_rel, saved.near_tie_rel);
    assert_eq!(
        request.shared_analysis,
        c::SearchAnalysisConfig {
            tolerances: actual_config.tolerances,
            limits: actual_config.limits,
            bishop: actual_config.bishop,
            contact: actual_config.contact,
        }
    );
    assert_eq!(
        resolved.ground_source_operation_id,
        input.ground_source_operation_id
    );
    assert_eq!(
        resolved.case_source_operation_id,
        input.case_source_operation_id
    );
    assert_eq!(request.search_id, case_id.to_string());
    assert_eq!(request.source_revision, input.input.source_revision);
    assert_eq!(request.ground, input.input.ground);
    assert_eq!(request.ground.section_id.0, ground_id.to_string());
    assert_eq!(
        request.ground.model_revision,
        resolved.ground_source_operation_id.get()
    );
    assert_eq!(request.basis, input.input.basis);
    assert_eq!(request.units, input.input.units);
    assert_eq!(request.direction, input.input.slip_surface.slide_direction);
    assert_eq!(request.extrusion_width_m, input.input.extrusion_width_m);

    let before = document.to_json_string().unwrap();
    let mut detached = resolved.request.clone();
    detached.domain.center_x.min = 100.0;
    detached.source_revision.push_str(":detached");
    detached.ground.materials[0]
        .properties
        .effective_cohesion_kpa = 99.0;
    assert_eq!(document.to_json_string().unwrap(), before);
    assert_eq!(
        document.resolve_circular_search(case_id).unwrap().request,
        resolved.request
    );
}

#[test]
fn circular_search_refuses_missing_policy_and_draft_ground() {
    let (ready, _ground_id, ready_case) = one(None);
    assert!(
        ready
            .resolve_circular_search(ready_case)
            .unwrap_err()
            .to_string()
            .contains("SEARCH_SPEC_NOT_CONFIGURED")
    );

    let mut draft = Document::new("Circular search draft refusal").unwrap();
    let receipt = apply(
        &mut draft,
        "draft-ground",
        vec![create_ground(GroundDefinition::default())],
    );
    let ground_id = receipt.delta.engineering.created_ground_sections[0];
    let receipt = apply(
        &mut draft,
        "draft-case",
        vec![create_circle(ground_id, Some(search_spec()))],
    );
    let case_id = receipt.delta.engineering.created_cases[0];
    assert!(
        draft
            .resolve_circular_search(case_id)
            .unwrap_err()
            .to_string()
            .contains("GROUND_NOT_READY")
    );
}

#[test]
fn circular_search_readiness_and_source_revisions_follow_spec_and_owner_changes() {
    let mut document = Document::new("Circular search provenance").unwrap();
    let grounds = apply(
        &mut document,
        "grounds",
        vec![create_ground(ground()), create_ground(ground())],
    );
    let (ground_a, ground_b) = (
        grounds.delta.engineering.created_ground_sections[0],
        grounds.delta.engineering.created_ground_sections[1],
    );
    let mut second = circle(ground_a, Some(search_spec()));
    second.slip_surface.slip_id = "second-slip".into();
    let cases = apply(
        &mut document,
        "cases",
        vec![
            create_circle(ground_a, Some(search_spec())),
            Command::Engineering(EngineeringCommand::CreateCircular {
                definition: second,
                name: "Shared case".into(),
            }),
        ],
    );
    let (case_id, shared_case_id) = (
        cases.delta.engineering.created_cases[0],
        cases.delta.engineering.created_cases[1],
    );
    let initial = document.resolve_circular_search(case_id).unwrap();
    let original_case_source = document.circular_case(case_id).unwrap().source_operation_id;
    let original_ground_source = document
        .ground_section(ground_a)
        .unwrap()
        .source_operation_id;

    let definition = document.circular_case(case_id).unwrap().definition.clone();
    apply(
        &mut document,
        "rename-case",
        vec![update_circle(case_id, definition.clone(), Some("Renamed"))],
    );
    apply(
        &mut document,
        "noop-case",
        vec![update_circle(case_id, definition.clone(), None)],
    );
    assert_eq!(
        document.circular_case(case_id).unwrap().source_operation_id,
        original_case_source
    );
    assert_eq!(
        document
            .resolve_circular_search(case_id)
            .unwrap()
            .request
            .source_revision,
        initial.request.source_revision
    );

    let mut changed_spec = definition.clone();
    changed_spec.search_spec.as_mut().unwrap().domain.center_x = c::InclusiveAxis {
        min: 0.25,
        max: 0.25,
        intervals: 0,
    };
    apply(
        &mut document,
        "change-search-spec",
        vec![update_circle(case_id, changed_spec.clone(), None)],
    );
    let after_spec = document.resolve_circular_search(case_id).unwrap();
    assert_ne!(
        document.circular_case(case_id).unwrap().source_operation_id,
        original_case_source
    );
    assert_ne!(
        after_spec.request.source_revision,
        initial.request.source_revision
    );

    // A valid no-op geology update preserves its source stamp; a shared edit changes it.
    let original_ground = document
        .ground_section(ground_a)
        .unwrap()
        .definition
        .clone();
    apply(
        &mut document,
        "noop-ground",
        vec![Command::Engineering(EngineeringCommand::UpdateGround {
            ground_section_id: ground_a,
            definition: original_ground.clone(),
            name: None,
        })],
    );
    assert_eq!(
        document
            .ground_section(ground_a)
            .unwrap()
            .source_operation_id,
        original_ground_source
    );
    let mut changed_ground = original_ground;
    changed_ground.materials[0]
        .properties
        .effective_cohesion_kpa = 2.0;
    let edit = apply(
        &mut document,
        "shared-ground-edit",
        vec![Command::Engineering(EngineeringCommand::UpdateGround {
            ground_section_id: ground_a,
            definition: changed_ground,
            name: None,
        })],
    );
    assert_eq!(
        edit.delta.engineering.affected_cases,
        vec![case_id, shared_case_id]
    );
    let after_ground = document.resolve_circular_search(case_id).unwrap();
    assert_ne!(
        after_ground.ground_source_operation_id,
        original_ground_source
    );
    assert_ne!(
        after_ground.request.source_revision,
        after_spec.request.source_revision
    );

    let mut rebound = document.circular_case(case_id).unwrap().definition.clone();
    rebound.ground_section_id = ground_b;
    apply(
        &mut document,
        "rebind-case",
        vec![update_circle(case_id, rebound, None)],
    );
    let after_rebind = document.resolve_circular_search(case_id).unwrap();
    assert_eq!(
        after_rebind.request.ground.section_id.0,
        ground_b.to_string()
    );
    assert_ne!(
        after_rebind.case_source_operation_id,
        after_ground.case_source_operation_id
    );
    assert_ne!(
        after_rebind.request.source_revision,
        after_ground.request.source_revision
    );
}

#[test]
fn circular_search_invalid_mixed_commit_rolls_back_and_changed_retry_is_rejected() {
    let (mut document, ground_id, case_id) = one(Some(search_spec()));
    let mut changed_ground = ground();
    changed_ground.materials[0]
        .properties
        .effective_cohesion_kpa = 4.0;
    let mut invalid_case = document.circular_case(case_id).unwrap().definition.clone();
    invalid_case.search_spec.as_mut().unwrap().schema_version = 2;
    let before = document.to_json_string().unwrap();
    let counters = document.allocation_counters();
    let indexes = document.indexes().clone();
    let revision = document.revision();
    let mut transaction = document.begin_transaction("invalid-mixed", revision);
    for command in [
        wall(),
        Command::Engineering(EngineeringCommand::UpdateGround {
            ground_section_id: ground_id,
            definition: changed_ground,
            name: None,
        }),
        drawing(),
        update_circle(case_id, invalid_case, None),
    ] {
        transaction.push(command);
    }
    assert!(document.commit(transaction).is_err());
    assert_eq!(document.to_json_string().unwrap(), before);
    assert_eq!(document.allocation_counters(), counters);
    assert_eq!(document.indexes(), &indexes);
    assert_eq!(document.revision(), revision);
    document.verify_index_consistency().unwrap();

    let mut first_definition = document.circular_case(case_id).unwrap().definition.clone();
    first_definition.search_spec.as_mut().unwrap().near_tie_abs = 1e-8;
    let first_command = update_circle(case_id, first_definition, None);
    let committed = apply(
        &mut document,
        "stable-spec-retry",
        vec![first_command.clone()],
    );
    assert!(!committed.replayed);
    let after_commit = document.to_json_string().unwrap();
    let mut changed_definition = document.circular_case(case_id).unwrap().definition.clone();
    changed_definition
        .search_spec
        .as_mut()
        .unwrap()
        .near_tie_abs = 2e-8;
    let changed = update_circle(case_id, changed_definition, None);
    assert!(matches!(
        document.commit(Transaction::new("stable-spec-retry", 0).with(changed)),
        Err(CoreError::CommandIdReused(_))
    ));
    assert_eq!(document.to_json_string().unwrap(), after_commit);
    let replay = document
        .commit(Transaction::new("stable-spec-retry", 0).with(first_command))
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(document.to_json_string().unwrap(), after_commit);
}

#[test]
fn circular_search_policy_bounds_fail_before_document_mutation() {
    let (mut document, _ground_id, case_id) = one(Some(search_spec()));
    let definition = document.circular_case(case_id).unwrap().definition.clone();
    let mut invalid = Vec::new();

    let mut spec = search_spec();
    spec.schema_version = 0;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.max_candidates = 0;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.max_candidates = 4097;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.max_wall_time_ms = 3_600_001;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.near_tie_abs = f64::NAN;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.domain.center_x = c::InclusiveAxis {
        min: 1.0,
        max: 0.0,
        intervals: 1,
    };
    invalid.push(spec);
    let mut spec = search_spec();
    spec.domain.center_z = c::InclusiveAxis {
        min: 5.0,
        max: 5.0,
        intervals: 1,
    };
    invalid.push(spec);
    let mut spec = search_spec();
    spec.domain.radius = c::InclusiveAxis {
        min: 4.875,
        max: 5.0,
        intervals: 4097,
    };
    invalid.push(spec);
    let mut spec = search_spec();
    spec.exploration_slices.initial_requested = u32::MAX;
    invalid.push(spec);
    let mut spec = search_spec();
    spec.final_pair.fine_requested = 8192;
    spec.final_pair.coarse_requested = 4096;
    invalid.push(spec);

    for (index, search_spec) in invalid.into_iter().enumerate() {
        let mut candidate = definition.clone();
        candidate.search_spec = Some(search_spec);
        let before = document.to_json_string().unwrap();
        let counters = document.allocation_counters();
        let indexes = document.indexes().clone();
        let command_id = format!("invalid-search-policy-{index}");
        let error = document
            .commit(
                Transaction::new(command_id, document.revision())
                    .with(update_circle(case_id, candidate, None)),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Circular search specification is invalid:"),
            "{error}"
        );
        assert_eq!(document.to_json_string().unwrap(), before);
        assert_eq!(document.allocation_counters(), counters);
        assert_eq!(document.indexes(), &indexes);
    }
}

#[test]
fn circular_search_native_corruption_and_history_round_trip_preserve_spec() {
    let (document, _ground_id, case_id) = one(Some(search_spec()));
    let mut corrupted: serde_json::Value =
        serde_json::from_str(&document.to_json_string().unwrap()).unwrap();
    let case_key = case_id.to_string();
    corrupted["document"]["engineering"]["circular_cases"][&case_key]["definition"]["search_spec"]
        ["near_tie_abs"] = serde_json::json!(1e-8);
    assert!(
        Document::from_json_str(&serde_json::to_string(&corrupted).unwrap()).is_err(),
        "current case must agree with its retained before/after journal"
    );

    let mut corrupted: serde_json::Value =
        serde_json::from_str(&document.to_json_string().unwrap()).unwrap();
    let payload = corrupted["document"]["command_receipts"]["circle"]["payload_json"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut commands: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert!(replace_first_key(
        &mut commands,
        "max_candidates",
        serde_json::json!(2048)
    ));
    corrupted["document"]["command_receipts"]["circle"]["payload_json"] =
        serde_json::Value::String(serde_json::to_string(&commands).unwrap());
    assert!(
        Document::from_json_str(&serde_json::to_string(&corrupted).unwrap()).is_err(),
        "stored command bytes must agree with their retained original command"
    );

    let (mut history_document, _history_ground_id, history_case_id) = one(None);
    let mut definition = history_document
        .circular_case(history_case_id)
        .unwrap()
        .definition
        .clone();
    definition.search_spec = Some(search_spec());
    apply(
        &mut history_document,
        "attach-search-to-history",
        vec![update_circle(history_case_id, definition, None)],
    );
    assert!(
        history_document
            .circular_case(history_case_id)
            .unwrap()
            .definition
            .search_spec
            .is_some()
    );
    history_document.undo().unwrap();
    assert!(
        history_document
            .circular_case(history_case_id)
            .unwrap()
            .definition
            .search_spec
            .is_none()
    );
    history_document.redo().unwrap();
    assert_eq!(
        history_document
            .circular_case(history_case_id)
            .unwrap()
            .definition
            .search_spec,
        Some(search_spec())
    );

    let mut retained = Document::new("Retained search specification").unwrap();
    let retained_ground = GroundSectionId::new(1);
    let creation = apply(
        &mut retained,
        "create-specification-source",
        vec![
            create_ground(ground()),
            create_circle(retained_ground, Some(search_spec())),
        ],
    );
    let retained_case = creation.delta.engineering.created_cases[0];
    let spec_revision = creation.revision;
    assert!(retained.ground_sections().contains_key(&retained_ground));
    assert!(
        retained
            .circular_case(retained_case)
            .unwrap()
            .definition
            .search_spec
            .is_some()
    );
    retained.undo().unwrap();
    assert!(retained.ground_sections().is_empty());
    assert!(retained.circular_cases().is_empty());
    apply(&mut retained, "abandon-retained-spec-branch", vec![wall()]);
    assert!(retained.ground_sections().is_empty());
    assert!(retained.circular_cases().is_empty());
    let source = retained.to_json_string().unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert_eq!(envelope["format_version"], CIRCULAR_NATIVE_FORMAT_VERSION);
    assert!(
        envelope["required_features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature == "geotech_circular_v1")
    );
    let reopened = Document::from_json_str(&source).unwrap();
    assert!(reopened.ground_sections().is_empty());
    assert!(reopened.circular_cases().is_empty());
    let historical = reopened.replay_revision(spec_revision).unwrap();
    assert_eq!(
        historical
            .circular_case(retained_case)
            .unwrap()
            .definition
            .search_spec,
        Some(search_spec())
    );
}

fn replace_first_key(
    value: &mut serde_json::Value,
    key: &str,
    replacement: serde_json::Value,
) -> bool {
    if let Some(object) = value.as_object_mut() {
        if object.contains_key(key) {
            object.insert(key.to_owned(), replacement);
            return true;
        }
        for child in object.values_mut() {
            if replace_first_key(child, key, replacement.clone()) {
                return true;
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        for child in array {
            if replace_first_key(child, key, replacement.clone()) {
                return true;
            }
        }
    }
    false
}
