use crate::*;
use geotech_core::{
    AnalysisConditions, InputUnits, SlopeAnalysisInput, SoilMaterial, triangular_section,
};
fn input() -> SlopeAnalysisInput {
    SlopeAnalysisInput {
        section: triangular_section(10.0, 45.0, 30.0, 1.0).unwrap(),
        soil: SoilMaterial {
            name: "Explicit test soil".into(),
            unit_weight_kn_per_m3: 18.0,
            effective_cohesion_kpa: 0.0,
            effective_friction_angle_deg: 30.0,
            provenance: "GEO-SLOPE analytical fixture, not design soil".into(),
        },
        conditions: AnalysisConditions::default(),
        units: InputUnits::default(),
        required_factor_of_safety: None,
        standard: None,
    }
}
fn create() -> Command {
    Command::Engineering(EngineeringCommand::CreateSlope {
        input: input(),
        name: "Slope test".into(),
    })
}
fn apply(d: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut t = d.begin_transaction(id, d.revision());
    for c in commands {
        t.push(c);
    }
    d.commit(t).unwrap()
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

#[test]
fn geotech_authoritative_cases_share_mixed_history_and_replay() {
    let mut d = Document::new("Geotechnical unified project").unwrap();
    let r = apply(
        &mut d,
        "mixed-geotech",
        vec![
            wall(),
            create(),
            Command::Draft(DraftCommand::CreateDrawing {
                name: "Section".into(),
                plane: DrawingPlane::default(),
            }),
        ],
    );
    let id = r.delta.engineering.created_cases[0];
    assert_eq!(id.get(), r.operation_ids[1].get());
    assert_eq!(r.revision, 1);
    assert_eq!(r.created_elements.len(), 1);
    assert_eq!(r.created_drawings.len(), 1);
    assert!(
        r.affected_objects
            .contains(&ProjectObjectRef::AnalysisCase { analysis_id: id })
    );
    let case = d.slope_case(id).unwrap().clone();
    assert_eq!(
        d.engineering().section(case.section_id),
        Some(&case.input.section)
    );
    assert_eq!(
        d.engineering().soil_material(case.soil_material_id),
        Some(&case.input.soil)
    );
    assert_eq!(
        d.engineering().slip_surface(case.slip_surface_id),
        Some(&case.input.section.slip_surface)
    );
    let before = d.to_json_string().unwrap();
    let rev = d.revision();
    let result = d.analyze_slope(id).unwrap();
    assert!((result.factor_of_safety - 1.0).abs() < 1e-13);
    assert_eq!(rev, d.revision());
    assert_eq!(before, d.to_json_string().unwrap());
    let undo = d.undo().unwrap().unwrap();
    assert_eq!(undo.delta.engineering.deleted_cases, vec![id]);
    assert!(d.slope_cases().is_empty());
    assert_eq!(d.elements().len(), 0);
    assert!(d.drawings().is_empty());
    let redo = d.redo().unwrap().unwrap();
    assert_eq!(redo.delta.engineering.created_cases, vec![id]);
    assert_eq!(d.slope_case(id), Some(&case));
    let historical = d.replay_revision(1).unwrap();
    assert_eq!(historical.slope_case(id), Some(&case));
    let opened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(opened.slope_case(id), Some(&case));
}
#[test]
fn geotech_failed_mixed_transaction_restores_exact_document_and_ids() {
    let mut d = Document::new("Atomic failures").unwrap();
    let before = d.to_json_string().unwrap();
    let bad = Command::Engineering(EngineeringCommand::UpdateSlope {
        analysis_id: AnalysisCaseId::new(999),
        input: input(),
        name: None,
    });
    let t = d
        .begin_transaction("fail", 0)
        .with(wall())
        .with(create())
        .with(bad);
    assert!(d.commit(t).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    let r = apply(&mut d, "success", vec![create()]);
    assert_eq!(
        r.delta.engineering.created_cases,
        vec![AnalysisCaseId::new(1)]
    );
    assert_eq!(r.operation_ids[0].get(), 1);
}
#[test]
fn geotech_update_invalidates_snapshot_and_undo_restores_it() {
    let mut d = Document::new("Snapshots").unwrap();
    let r = apply(&mut d, "create", vec![create()]);
    let id = r.delta.engineering.created_cases[0];
    apply(
        &mut d,
        "snapshot",
        vec![Command::Engineering(
            EngineeringCommand::SaveCalculationSnapshot { analysis_id: id },
        )],
    );
    let archived = d.slope_case(id).unwrap().clone();
    assert!(archived.calculation_snapshot.is_some());
    let mut changed = input();
    changed.soil.effective_cohesion_kpa = 10.0;
    apply(
        &mut d,
        "change",
        vec![Command::Engineering(EngineeringCommand::UpdateSlope {
            analysis_id: id,
            input: changed,
            name: Some("Revised".into()),
        })],
    );
    assert!(d.slope_case(id).unwrap().calculation_snapshot.is_none());
    assert!(d.analyze_slope(id).unwrap().factor_of_safety > 1.0);
    d.undo().unwrap();
    assert_eq!(d.slope_case(id), Some(&archived));
    let source = d.to_json_string().unwrap();
    let mut reopened = Document::from_json_str(&source).unwrap();
    assert_eq!(reopened.slope_case(id), Some(&archived));
    reopened.redo().unwrap();
    assert!(
        reopened
            .slope_case(id)
            .unwrap()
            .calculation_snapshot
            .is_none()
    );
}
#[test]
fn geotech_format_feature_guards_include_undone_history_and_plain_v2_import() {
    let mut d = Document::new("Feature envelopes").unwrap();
    let plain = d.to_json_string().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&plain).unwrap()["format_version"],
        2
    );
    Document::from_json_str(&plain).unwrap();
    apply(&mut d, "create", vec![create()]);
    d.undo().unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(data["format_version"], 3);
    assert_eq!(
        data["required_features"],
        serde_json::json!(["typed_drafting_v1", "geotech_slope_v1"])
    );
    Document::from_json_str(&data.to_string()).unwrap();
    data["format_version"] = serde_json::json!(2);
    data["document"]["format_version"] = serde_json::json!(2);
    data["required_features"] = serde_json::json!(["typed_drafting_v1"]);
    assert!(Document::from_json_str(&data.to_string()).is_err());
}
#[test]
fn geotech_snapshot_projection_and_receipt_corruption_are_rejected() {
    let mut d = Document::new("Corruption").unwrap();
    let r = apply(&mut d, "create", vec![create()]);
    let id = r.delta.engineering.created_cases[0];
    apply(
        &mut d,
        "snapshot",
        vec![Command::Engineering(
            EngineeringCommand::SaveCalculationSnapshot { analysis_id: id },
        )],
    );
    let source = d.to_json_string().unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&source).unwrap();
    data["document"]["engineering"]["slope_cases"][id.to_string()]["calculation_snapshot"]["result"]
        ["factor_of_safety"] = serde_json::json!(123.0);
    assert!(Document::from_json_str(&data.to_string()).is_err());
    let mut data: serde_json::Value = serde_json::from_str(&source).unwrap();
    data["document"]["revision_states"]["1"]["engineering"]["slope_cases"][id.to_string()]["name"] =
        serde_json::json!("Forged projection");
    assert!(Document::from_json_str(&data.to_string()).is_err());
    let mut data: serde_json::Value = serde_json::from_str(&source).unwrap();
    data["document"]["command_receipts"]["create"]["receipt"]["affected_objects"] =
        serde_json::json!([]);
    assert!(Document::from_json_str(&data.to_string()).is_err());
    let mut data: serde_json::Value = serde_json::from_str(&source).unwrap();
    data["document"]["command_receipts"]["create"]["recorded_commands"][0]["engineering"]["create_slope"]
        ["name"] = serde_json::json!("Forged request");
    assert!(Document::from_json_str(&data.to_string()).is_err());
}
#[test]
fn geotech_request_replay_and_unsupported_case_mutation_are_atomic() {
    let mut d = Document::new("Idempotency").unwrap();
    let request = d.begin_transaction("create", 0).with(create());
    let first = d.commit(request.clone()).unwrap();
    let after = d.to_json_string().unwrap();
    let second = d.commit(request).unwrap();
    assert!(second.replayed);
    assert_eq!(second.operation_ids, first.operation_ids);
    assert_eq!(d.to_json_string().unwrap(), after);
    let id = first.delta.engineering.created_cases[0];
    let mut bad = input();
    bad.conditions.water_table_elevation_m = Some(2.0);
    let t = d
        .begin_transaction("wet", d.revision())
        .with(Command::Engineering(EngineeringCommand::UpdateSlope {
            analysis_id: id,
            input: bad,
            name: None,
        }));
    assert!(matches!(d.commit(t), Err(CoreError::Geotech(_))));
    assert_eq!(d.to_json_string().unwrap(), after);
}

#[test]
fn geotech_checkpoint_compaction_and_history_diff_preserve_cases() {
    let mut d = Document::new("Engineering checkpoints").unwrap();
    d.set_history_policy(HistoryPolicy::compact(4)).unwrap();
    let r = apply(&mut d, "create", vec![create()]);
    let id = r.delta.engineering.created_cases[0];
    for i in 0..9 {
        let mut input = input();
        input.soil.effective_cohesion_kpa = i as f64;
        apply(
            &mut d,
            &format!("edit-{i}"),
            vec![Command::Engineering(EngineeringCommand::UpdateSlope {
                analysis_id: id,
                input,
                name: None,
            })],
        );
    }
    assert_eq!(
        d.history_diff(0, 1).unwrap().engineering.created_cases,
        vec![id]
    );
    assert_eq!(
        d.history_diff(1, 10).unwrap().engineering.updated_cases,
        vec![id]
    );
    assert_eq!(
        d.history_diff(1, 0).unwrap().engineering.deleted_cases,
        vec![id]
    );
    let before = d.slope_case(id).unwrap().clone();
    let mut opened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(opened.slope_case(id), Some(&before));
    assert_eq!(
        opened
            .replay_revision(4)
            .unwrap()
            .slope_case(id)
            .unwrap()
            .input
            .soil
            .effective_cohesion_kpa,
        2.0
    );
    opened.undo().unwrap();
    opened.redo().unwrap();
    assert_eq!(opened.slope_case(id), Some(&before));
    opened.verify_index_consistency().unwrap();
}

#[test]
fn geotech_noop_update_receipt_qualifies_touched_case() {
    let mut d = Document::new("Qualified no-op").unwrap();
    let r = apply(&mut d, "create", vec![create()]);
    let id = r.delta.engineering.created_cases[0];
    let r = apply(
        &mut d,
        "noop",
        vec![Command::Engineering(EngineeringCommand::UpdateSlope {
            analysis_id: id,
            input: input(),
            name: None,
        })],
    );
    assert!(r.delta.engineering.updated_cases.is_empty());
    assert_eq!(r.delta.engineering.affected_cases, vec![id]);
    assert_eq!(
        r.affected_objects,
        vec![ProjectObjectRef::AnalysisCase { analysis_id: id }]
    );
    assert_eq!(
        d.history_diff(1, 2).unwrap().engineering,
        EngineeringDelta::default()
    );
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}
