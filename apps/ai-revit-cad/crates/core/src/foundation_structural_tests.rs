use crate::*;

fn input() -> FoundationStructuralInput {
    let mut foundation = foundation_core::RigidMatInput::default();
    foundation.mesh = foundation_core::MeshSpec { nx: 4, ny: 4 };
    foundation.self_weight_policy = foundation_core::SelfWeightPolicy::IncludeOnce {
        source_id: "test-raft-own-weight".into(),
    };
    FoundationStructuralInput {
        structural_model: structural_core::fixtures::braced_single_storey_10x12(),
        analysis_target: structural_core::AnalysisTarget::Combination(
            structural_core::LoadCombinationId::new(1).unwrap(),
        ),
        foundation: Some(foundation),
        foundation_load_source: FoundationLoadSource::StructuralReactions,
        foundation_reference_point_m: [5.0, 6.0, 0.0],
    }
}
fn identity() -> EngineeringSourceIdentity {
    EngineeringSourceIdentity {
        source_manifest_sha256: "a".repeat(64),
        executable_sha256: None,
        solver_version: "stage1-test".into(),
        source_label: "Source-only synthetic fixture, no engineering approval".into(),
    }
}
fn commit(d: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut tx = d.begin_transaction(id, d.revision());
    for command in commands {
        tx.push(command);
    }
    d.commit(tx).unwrap()
}
fn replace(input: FoundationStructuralInput) -> Command {
    Command::FoundationStructural(FoundationStructuralCommand::ReplaceInput { input })
}
fn compute_and_save(d: &mut Document, id: &str) -> EngineeringSnapshotId {
    let snapshot =
        analyze_foundation_structural(d.foundation_structural().input().unwrap(), identity())
            .unwrap();
    let receipt = commit(
        d,
        id,
        vec![Command::FoundationStructural(
            FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot },
        )],
    );
    assert_eq!(
        receipt.delta.foundation_structural.created_snapshots.len(),
        1
    );
    receipt.delta.foundation_structural.created_snapshots[0]
}

#[test]
fn foundation_structural_mixed_transaction_rolls_back_all_domains_and_ids() {
    let mut d = Document::new("Atomic single document").unwrap();
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("mixed-invalid", d.revision())
        .with(replace(input()))
        .with(Command::CreateLevel {
            name: "Invalid level".into(),
            elevation_m: f64::NAN,
            height_m: None,
        });
    assert!(d.commit(tx).is_err());
    assert!(d.foundation_structural().is_empty());
    assert_eq!(d.to_json_string().unwrap(), before);
    let receipt = commit(&mut d, "after-rollback", vec![replace(input())]);
    assert_eq!(receipt.operation_ids, vec![OperationId(1)]);
    assert_eq!(
        d.foundation_structural()
            .input()
            .unwrap()
            .source_revision
            .operation_id,
        OperationId(1)
    );
    assert!(
        receipt
            .affected_objects
            .contains(&ProjectObjectRef::FoundationStructuralInput)
    );
}

#[test]
fn foundation_structural_archives_become_stale_and_undo_redo_never_recompute() {
    let mut d = Document::new("Immutable runs").unwrap();
    commit(&mut d, "source-v1", vec![replace(input())]);
    let first = compute_and_save(&mut d, "run-v1");
    let archived = d.foundation_structural().snapshot(first).unwrap().clone();
    assert_eq!(
        d.foundation_structural().snapshot_status(first),
        Some(EngineeringSnapshotStatus::Current)
    );
    assert_eq!(
        archived.engineering_approval,
        EngineeringApprovalStatus::NotImplemented
    );
    assert!(
        !archived
            .effective_foundation_input
            .as_ref()
            .unwrap()
            .soil
            .engineering_site_verified
    );
    assert!(archived.producer.executable_sha256.is_none());
    // Reaction mode must replace the default manual 300 t reference, reverse
    // the actual support actions, and retain individual transferred reactions.
    let effective = archived.effective_foundation_input.as_ref().unwrap();
    assert_eq!(
        effective.load.original_structural_reactions.len(),
        archived.structural_result.reactions.len()
    );
    assert!((effective.load.n_kn - foundation_core::reference_load_300_tonnes().n_kn).abs() > 1.0);
    for (action, reaction) in effective
        .load
        .original_structural_reactions
        .iter()
        .zip(&archived.structural_result.reactions)
    {
        assert_eq!(action.external_force_kn[2], -reaction.force_moment[2]);
        assert_eq!(
            action.location_m,
            [
                reaction.position_m[0] - 5.0,
                reaction.position_m[1] - 6.0,
                reaction.position_m[2]
            ]
        );
    }
    let mut changed = input();
    changed.structural_model.sections[0].area_m2 *= 1.1;
    commit(&mut d, "section-v2", vec![replace(changed)]);
    assert_eq!(
        d.foundation_structural().snapshot_status(first),
        Some(EngineeringSnapshotStatus::Stale)
    );
    assert_eq!(d.foundation_structural().snapshot(first), Some(&archived));
    d.undo().unwrap();
    assert_eq!(
        d.foundation_structural().snapshot_status(first),
        Some(EngineeringSnapshotStatus::Current)
    );
    d.redo().unwrap();
    assert_eq!(
        d.foundation_structural().snapshot_status(first),
        Some(EngineeringSnapshotStatus::Stale)
    );
    let second = compute_and_save(&mut d, "run-v2");
    assert_ne!(first, second);
    assert_eq!(d.foundation_structural().snapshots().len(), 2);
    let second_archive = d.foundation_structural().snapshot(second).unwrap().clone();
    d.undo().unwrap();
    assert!(d.foundation_structural().snapshot(second).is_none());
    d.redo().unwrap();
    assert_eq!(
        d.foundation_structural().snapshot(second),
        Some(&second_archive)
    );
    assert_eq!(d.foundation_structural().snapshot(first), Some(&archived));
    let reopened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.foundation_structural(), d.foundation_structural());
    assert_eq!(reopened.document_uuid, d.document_uuid);
}

#[test]
fn foundation_structural_replay_and_idempotent_receipts_keep_original_ids() {
    let mut d = Document::new("One journal").unwrap();
    let commands = vec![
        replace(input()),
        Command::CreateLevel {
            name: "Same transaction".into(),
            elevation_m: 0.0,
            height_m: Some(4.0),
        },
    ];
    let original = commit(&mut d, "mixed-source", commands.clone());
    compute_and_save(&mut d, "run");
    let source_revision = d
        .foundation_structural()
        .input()
        .unwrap()
        .source_revision
        .clone();
    let mut tx = d.begin_transaction("mixed-source", 0);
    for command in commands.clone() {
        tx.push(command);
    }
    let replayed = d.commit(tx).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.operation_ids, original.operation_ids);
    assert_eq!(
        d.foundation_structural().input().unwrap().source_revision,
        source_revision
    );
    let source_state = d.replay_revision(1).unwrap();
    assert_eq!(
        source_state
            .foundation_structural()
            .input()
            .unwrap()
            .source_revision,
        source_revision
    );
    assert_eq!(source_state.foundation_structural().snapshots().len(), 0);
    let mut before_source = d.replay_revision(0).unwrap();
    assert!(before_source.foundation_structural().is_empty());
    let mut future_retry = before_source.begin_transaction("mixed-source", 0);
    for command in commands {
        future_retry.push(command);
    }
    assert!(before_source.commit(future_retry).is_err());
    let reopened = Document::from_json_str(&source_state.to_json_string().unwrap()).unwrap();
    assert_eq!(
        reopened.foundation_structural(),
        source_state.foundation_structural()
    );
}

#[test]
fn foundation_structural_native5_exact_features_reject_downgrade_and_forgery() {
    let mut d = Document::new("Required typed domain").unwrap();
    commit(&mut d, "source", vec![replace(input())]);
    let source = d.to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert_eq!(value["format_version"], 5);
    assert_eq!(
        value["required_features"],
        serde_json::json!([
            "typed_drafting_v1",
            "geotech_slope_v1",
            "geotech_circular_v1",
            "foundation_structural_v1"
        ])
    );
    let mut downgraded = value.clone();
    downgraded["format_version"] = 4.into();
    downgraded["document"]["format_version"] = 4.into();
    downgraded["required_features"] = serde_json::json!([
        "typed_drafting_v1",
        "geotech_slope_v1",
        "geotech_circular_v1"
    ]);
    assert!(Document::from_json_str(&downgraded.to_string()).is_err());
    let mut forged = value.clone();
    forged["required_features"] = serde_json::json!([
        "typed_drafting_v1",
        "geotech_slope_v1",
        "geotech_circular_v1"
    ]);
    assert!(Document::from_json_str(&forged.to_string()).is_err());
    assert!(Document::from_json_str(&value["document"].to_string()).is_err());
    let mut forged_id = value;
    forged_id["document"]["foundation_structural"]["current_input"]["definition"]["structural_model"]
        ["nodes"][0]["id"] = "01".into();
    assert!(Document::from_json_str(&forged_id.to_string()).is_err());
}

#[test]
fn foundation_structural_snapshot_rejects_stale_source_and_unrelated_manual_result() {
    let mut d = Document::new("Coherent result chain").unwrap();
    commit(&mut d, "source", vec![replace(input())]);
    let state = d.foundation_structural().input().unwrap().clone();
    let structural = structural_core::analyze(
        &state.definition.structural_model,
        state.definition.analysis_target,
    )
    .unwrap();
    let manual = foundation_core::solve(state.definition.foundation.as_ref().unwrap()).unwrap();
    assert!(
        FoundationStructuralSnapshot::new(&state, structural, Some(manual), identity()).is_err()
    );
    let snapshot = analyze_foundation_structural(&state, identity()).unwrap();
    let mut changed = input();
    changed.structural_model.sections[0].iy_m4 *= 2.0;
    commit(&mut d, "new-section", vec![replace(changed)]);
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("stale-save", d.revision())
        .with(Command::FoundationStructural(
            FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot },
        ));
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn foundation_structural_builder_never_relabels_old_result_for_a_new_section_or_load() {
    let mut d = Document::new("Analyze-produced result binding").unwrap();
    commit(&mut d, "source-v1", vec![replace(input())]);
    let original = d.foundation_structural().input().unwrap().clone();
    let old_result = structural_core::analyze(
        &original.definition.structural_model,
        original.definition.analysis_target,
    )
    .unwrap();
    let mut changed = input();
    changed.foundation = None;
    changed.structural_model.sections[0].area_m2 *= 1.1;
    commit(&mut d, "section-v2", vec![replace(changed)]);
    let state = d.foundation_structural().input().unwrap();
    assert!(
        FoundationStructuralSnapshot::new(state, old_result.clone(), None, identity()).is_err()
    );
    let mut changed_load = input();
    changed_load.foundation = None;
    changed_load.structural_model.load_cases[1].nodal_loads[0].force_moment[2] *= 2.0;
    commit(&mut d, "load-v3", vec![replace(changed_load)]);
    assert!(
        FoundationStructuralSnapshot::new(
            d.foundation_structural().input().unwrap(),
            old_result,
            None,
            identity()
        )
        .is_err()
    );
}

#[test]
fn foundation_structural_geotech_and_drafting_share_the_same_undo_boundary() {
    let mut d = Document::new("Mixed BIM drafting GEO structural").unwrap();
    let slope = geotech_core::SlopeAnalysisInput {
        section: geotech_core::triangular_section(10.0, 45.0, 30.0, 1.0).unwrap(),
        soil: geotech_core::SoilMaterial {
            name: "Existing GEO fixture".into(),
            unit_weight_kn_per_m3: 18.0,
            effective_cohesion_kpa: 0.0,
            effective_friction_angle_deg: 30.0,
            provenance: "Synthetic regression fixture, no site approval".into(),
        },
        conditions: geotech_core::AnalysisConditions::default(),
        units: geotech_core::InputUnits::default(),
        required_factor_of_safety: None,
        standard: None,
    };
    let receipt = commit(
        &mut d,
        "all-domains",
        vec![
            replace(input()),
            Command::Engineering(EngineeringCommand::CreateSlope {
                name: "Existing GEO".into(),
                input: slope,
            }),
            Command::Draft(DraftCommand::CreateDrawing {
                name: "Same document plan".into(),
                plane: DrawingPlane::default(),
            }),
            Command::CreateLevel {
                name: "BIM level".into(),
                elevation_m: 0.0,
                height_m: Some(4.0),
            },
        ],
    );
    assert_eq!(receipt.operation_ids.len(), 4);
    assert_eq!(d.engineering().slope_cases.len(), 1);
    assert_eq!(receipt.created_drawings.len(), 1);
    let persisted = d.to_json_string().unwrap();
    d.undo().unwrap();
    assert!(d.foundation_structural().is_empty());
    assert!(d.engineering().slope_cases.is_empty());
    assert!(d.drawing(receipt.created_drawings[0]).is_none());
    d.redo().unwrap();
    assert_eq!(d.engineering().slope_cases.len(), 1);
    assert!(d.drawing(receipt.created_drawings[0]).is_some());
    assert!(Document::from_json_str(&persisted).is_ok());
    assert!(Document::from_json_str(&d.to_json_string().unwrap()).is_ok());
}

#[test]
fn foundation_structural_retained_request_and_effect_forgery_are_rejected() {
    let mut d = Document::new("Retained authority").unwrap();
    commit(&mut d, "source", vec![replace(input())]);
    compute_and_save(&mut d, "run");
    let original: serde_json::Value = serde_json::from_str(&d.to_json_string().unwrap()).unwrap();
    let mut missing_original = original.clone();
    missing_original["document"]["command_receipts"]["run"]
        .as_object_mut()
        .unwrap()
        .remove("recorded_commands");
    assert!(Document::from_json_str(&missing_original.to_string()).is_err());
    let mut forged = original;
    forged["document"]["foundation_structural"]["current_input"]["input_fingerprint"] =
        "b".repeat(64).into();
    assert!(Document::from_json_str(&forged.to_string()).is_err());
}

#[test]
fn foundation_structural_empty_documents_keep_native2_feature_contract() {
    let d = Document::new("Legacy native2 remains native2").unwrap();
    let source = d.to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert_eq!(value["format_version"], NATIVE_FORMAT_VERSION);
    assert_eq!(
        value["required_features"],
        serde_json::json!(["typed_drafting_v1"])
    );
    assert!(value["document"].get("foundation_structural").is_none());
    assert!(
        Document::from_json_str(&source)
            .unwrap()
            .foundation_structural()
            .is_empty()
    );
}
