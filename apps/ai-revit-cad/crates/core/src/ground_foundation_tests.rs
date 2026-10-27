//! Isolated Gate8 foundation regressions; these do not establish native acceptance.
use crate::*;
use geotech_core::circular as c;
use geotech_core::{AnalysisConditions, InputUnits, Point2 as GPoint, SlopeAnalysisInput, SoilMaterial};

fn ground() -> GroundDefinition {
    GroundDefinition {
        coordinate_system: c::SectionCoordinateSystem::CartesianXzMetres,
        terrain_polyline: vec![GPoint::new(-6.0, 3.0), GPoint::new(6.0, 0.0)],
        layer_interfaces: vec![],
        soil_regions: vec![c::SoilRegion { region_id: "region-a".into(), material_id: "soil-a".into() }],
        bottom_boundary: vec![GPoint::new(-6.0, -3.0), GPoint::new(6.0, -3.0)],
        materials: vec![c::GroundMaterial {
            material_id: "soil-a".into(),
            properties: SoilMaterial {
                name: "Explicit fixture soil".into(), unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 0.0, effective_friction_angle_deg: 28.0,
                provenance: "Foundation test data; not engineering soil acceptance".into(),
            },
        }],
    }
}
fn circle(ground_section_id: GroundSectionId) -> CircularCaseDefinition {
    CircularCaseDefinition {
        ground_section_id,
        slip_surface: c::CircularSlipSurface {
            slip_id: "explicit-slip".into(), center: GPoint::new(0.0, 5.0), radius_m: 4.875,
            arc_branch: c::ArcBranch::Lower, slide_direction: c::SlideDirection::PositiveX,
            selected_endpoints: None,
        },
        basis: c::AnalysisBasis::DryStatic, units: InputUnits::default(),
        extrusion_width_m: 1.0, config: c::CircularAnalysisConfig::default(), search_spec: None,
    }
}
fn create_ground(definition: GroundDefinition) -> Command {
    Command::Engineering(EngineeringCommand::CreateGround { definition, name: "Ground".into() })
}
fn create_circle(id: GroundSectionId) -> Command {
    Command::Engineering(EngineeringCommand::CreateCircular { definition: circle(id), name: "Circle".into() })
}
fn update_ground(id: GroundSectionId, definition: GroundDefinition, name: Option<&str>) -> Command {
    Command::Engineering(EngineeringCommand::UpdateGround {
        ground_section_id: id, definition, name: name.map(str::to_owned),
    })
}
fn apply(d: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut t = d.begin_transaction(id, d.revision());
    for command in commands { t.push(command); }
    d.commit(t).unwrap()
}
fn one() -> (Document, GroundSectionId, AnalysisCaseId) {
    let mut d = Document::new("Ground foundation fixture").unwrap();
    let r = apply(&mut d, "ground", vec![create_ground(ground())]);
    let g = r.delta.engineering.created_ground_sections[0];
    let r = apply(&mut d, "circle", vec![create_circle(g)]);
    (d, g, r.delta.engineering.created_cases[0])
}
fn wall() -> Command {
    Command::CreateElement {
        name: "Wall".into(), class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0,0.0), end: Point2::new(5.0,0.0),
            base_elevation_m: 0.0, height_m: 3.0, thickness_m: 0.2,
        }), properties: BTreeMap::new(),
    }
}
fn planar() -> Command {
    Command::Engineering(EngineeringCommand::CreateSlope {
        name: "Legacy planar".into(),
        input: SlopeAnalysisInput {
            section: geotech_core::triangular_section(10.0,45.0,30.0,1.0).unwrap(),
            soil: SoilMaterial { name: "Planar soil".into(), unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 0.0, effective_friction_angle_deg: 30.0,
                provenance: "Analytical fixture".into() },
            conditions: AnalysisConditions::default(), units: InputUnits::default(),
            required_factor_of_safety: None, standard: None,
        },
    })
}
fn drawing() -> Command {
    Command::Draft(DraftCommand::CreateDrawing { name: "Section".into(), plane: DrawingPlane::default() })
}

#[test]
fn ground_foundation_standalone_draft_ready_and_typed_resolution() {
    let mut d = Document::new("Sequential standalone ground").unwrap();
    let r = apply(&mut d, "draft-ground", vec![create_ground(GroundDefinition::default())]);
    let g = r.delta.engineering.created_ground_sections[0];
    assert_eq!(g.get(),r.operation_ids[0].get());
    assert!(d.slope_cases().is_empty() && d.circular_cases().is_empty());
    assert!(!d.ground_readiness(g).unwrap().is_ready());
    let body = d.to_json_string().unwrap();
    let mut d = Document::from_json_str(&body).unwrap();
    assert_eq!(d.ground_sections().len(),1);
    assert!(d.ground_section(g).unwrap().definition.materials.is_empty());
    let r = apply(&mut d, "case-on-draft", vec![create_circle(g)]);
    let a = r.delta.engineering.created_cases[0];
    assert!(d.resolve_circular_input(a).unwrap_err().to_string().contains("GROUND_NOT_READY"));
    apply(&mut d, "complete-ground", vec![update_ground(g,ground(),None)]);
    assert!(d.ground_readiness(g).unwrap().is_ready());
    let resolved = d.resolve_circular_input(a).unwrap();
    assert_eq!(resolved.input.analysis_id,a.to_string());
    assert_eq!(resolved.input.ground.section_id.0,g.to_string());
    assert_eq!(resolved.input.ground.model_revision,resolved.ground_source_operation_id.get());
    assert_eq!(resolved.input.source_revision,format!("ground-operation:{}/case-operation:{}",
        resolved.ground_source_operation_id,resolved.case_source_operation_id));
    let before = d.to_json_string().unwrap();
    let mut detached = resolved.input.clone();
    detached.ground.materials[0].properties.effective_cohesion_kpa = 123.0;
    assert_eq!(d.to_json_string().unwrap(),before);
    assert_eq!(d.resolve_circular_input(a).unwrap().input.ground.materials[0].properties.effective_cohesion_kpa,0.0);
}

#[test]
fn ground_foundation_shared_edit_is_one_change_and_dependency_union_survives_rebind() {
    let mut d = Document::new("Shared ground").unwrap();
    let r = apply(&mut d,"grounds",vec![create_ground(ground()),create_ground(ground())]);
    let (g,h)=(r.delta.engineering.created_ground_sections[0],r.delta.engineering.created_ground_sections[1]);
    let r=apply(&mut d,"cases",vec![create_circle(g),create_circle(g),create_circle(h)]);
    let (a,b,z)=(r.delta.engineering.created_cases[0],r.delta.engineering.created_cases[1],r.delta.engineering.created_cases[2]);
    let old_a=d.circular_case(a).unwrap().clone();
    let old_z=d.resolve_circular_input(z).unwrap().input;
    let mut changed=ground(); changed.materials[0].properties.effective_cohesion_kpa=2.0;
    let r=apply(&mut d,"shared-edit",vec![update_ground(g,changed.clone(),None)]);
    assert_eq!(r.delta.engineering.affected_cases,vec![a,b]);
    assert!(r.delta.engineering.updated_cases.is_empty());
    let journal=&d.revision_states[&r.revision].delta.as_ref().unwrap().changes;
    assert_eq!(journal.iter().filter(|v| matches!(v,Change::EngineeringGroundSection{..})).count(),1);
    assert_eq!(d.circular_case(a),Some(&old_a));
    assert_eq!(d.resolve_circular_input(z).unwrap().input,old_z);
    let before=d.revision();
    changed.materials[0].properties.effective_cohesion_kpa=3.0;
    let r=apply(&mut d,"edit-and-rebind",vec![update_ground(g,changed,None),
        Command::Engineering(EngineeringCommand::UpdateCircular {analysis_id:a,definition:circle(h),name:None})]);
    assert_eq!(r.delta.engineering.affected_cases,vec![a,b]);
    assert_eq!(r.delta.engineering.updated_cases,vec![a]);
    assert_eq!(d.indexes().circular_cases_by_ground_section[&g],BTreeSet::from([b]));
    assert_eq!(d.indexes().circular_cases_by_ground_section[&h],BTreeSet::from([a,z]));
    assert_eq!(d.history_diff(before,d.revision()).unwrap().engineering.affected_cases,vec![a,b]);
    d.verify_index_consistency().unwrap();
    d.undo().unwrap();
    assert_eq!(d.indexes().circular_cases_by_ground_section[&g],BTreeSet::from([a,b]));
    d.redo().unwrap(); d.verify_index_consistency().unwrap();
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

#[test]
fn ground_foundation_noop_rename_preserves_sources_and_unrelated_transactions() {
    let (mut d,g,a)=one();
    let bound=d.resolve_circular_input(a).unwrap();
    let gs=d.ground_section(g).unwrap().source_operation_id;
    let cs=d.circular_case(a).unwrap().source_operation_id;
    let r=apply(&mut d,"rename-ground",vec![update_ground(g,ground(),Some("Renamed ground"))]);
    assert_eq!(d.ground_section(g).unwrap().source_operation_id,gs);
    assert!(r.delta.engineering.affected_cases.is_empty());
    assert_eq!(r.delta.engineering.affected_ground_sections,vec![g]);
    apply(&mut d,"rename-case",vec![Command::Engineering(EngineeringCommand::UpdateCircular {
        analysis_id:a,definition:circle(g),name:Some("Renamed case".into())})]);
    assert_eq!(d.circular_case(a).unwrap().source_operation_id,cs);
    apply(&mut d,"noop",vec![update_ground(g,ground(),None)]);
    apply(&mut d,"unrelated",vec![wall(),drawing()]);
    assert_eq!(d.resolve_circular_input(a).unwrap().input,bound.input);
    assert_eq!(d.resolve_circular_input(a).unwrap().config,bound.config);
    d.undo().unwrap();d.redo().unwrap();
    assert_eq!(d.resolve_circular_input(a).unwrap().input,bound.input);
}

#[test]
fn ground_foundation_mixed_transaction_shared_undo_replay_and_exact_rollback() {
    let mut d=Document::new("Mixed foundation").unwrap();
    // Ground is the third allocated operation in this single atomic batch.
    let g=GroundSectionId::new(3);
    let commands=vec![wall(),planar(),create_ground(ground()),create_circle(g),drawing()];
    let r=apply(&mut d,"mixed",commands.clone());
    assert_eq!(r.revision,1); assert_eq!(r.operation_ids.len(),5);
    assert_eq!(r.created_elements.len(),1);assert_eq!(r.created_drawings.len(),1);
    assert_eq!(d.slope_cases().len(),1);assert_eq!(d.circular_cases().len(),1);
    assert!(r.affected_objects.contains(&ProjectObjectRef::GroundSection{ground_section_id:g}));
    assert!(r.affected_objects.contains(&ProjectObjectRef::AnalysisCase{analysis_id:AnalysisCaseId::new(4)}));
    let state=d.to_json_string().unwrap();
    d.undo().unwrap();assert!(d.engineering().is_empty());
    d.redo().unwrap();
    let replay=d.replay_revision(1).unwrap();assert_eq!(replay.ground_sections(),d.ground_sections());
    assert_eq!(replay.circular_cases(),d.circular_cases());
    Document::from_json_str(&state).unwrap();
    for bad in [
        Command::Engineering(EngineeringCommand::CreateCircular {definition:circle(GroundSectionId::new(999)),name:"bad".into()}),
        Command::Engineering(EngineeringCommand::UpdateCircular {analysis_id:AnalysisCaseId::new(999),definition:circle(g),name:None}),
    ] {
        let before=d.to_json_string().unwrap();
        let counters=d.allocation_counters();let index=d.indexes().clone();
        let mut t=d.begin_transaction("failed-mixed",d.revision());
        for command in [wall(),planar(),drawing(),bad] {t.push(command);}
        assert!(d.commit(t).is_err());
        assert_eq!(d.to_json_string().unwrap(),before);assert_eq!(d.allocation_counters(),counters);
        assert_eq!(d.indexes(),&index);d.verify_index_consistency().unwrap();
    }
    let before=d.to_json_string().unwrap();
    let t=d.begin_transaction("first-command-failure",d.revision()).with(
        Command::Engineering(EngineeringCommand::CreateCircular{definition:circle(GroundSectionId::new(999)),name:"bad".into()}));
    assert!(d.commit(t).is_err());assert_eq!(d.to_json_string().unwrap(),before);
    let r=apply(&mut d,"after-failure",vec![create_ground(ground())]);
    assert_eq!(r.operation_ids[0].get(),6);
}

#[test]
fn ground_foundation_retry_is_immutable_after_undo_abandon_and_reopen() {
    let mut d=Document::new("Retry identity").unwrap();
    let command=create_ground(ground());
    let r=apply(&mut d,"stable",vec![command.clone()]);let g=r.delta.engineering.created_ground_sections[0];
    let retry=d.commit(Transaction::new("stable",999).with(command.clone())).unwrap();
    assert!(retry.replayed);assert_eq!(retry.operation_ids,r.operation_ids);
    for changed in [create_ground(GroundDefinition::default()),Command::Engineering(EngineeringCommand::CreateGround{definition:ground(),name:"Different name".into()})] {
        assert!(matches!(d.commit(Transaction::new("stable",d.revision()).with(changed)),Err(CoreError::CommandIdReused(_))));
    }
    assert!(matches!(d.commit(Transaction::new("fresh-stale",0).with(wall())),Err(CoreError::RevisionConflict{..})));
    d.undo().unwrap();assert!(d.ground_section(g).is_none());
    apply(&mut d,"abandon",vec![wall()]);
    let before=d.to_json_string().unwrap();
    let mut d=Document::from_json_str(&before).unwrap();
    let retry=d.commit(Transaction::new("stable",0).with(command)).unwrap();
    assert!(retry.replayed);assert!(d.ground_section(g).is_none());
    assert_eq!(d.to_json_string().unwrap(),before);
    assert_eq!(d.replay_revision(1).unwrap().ground_sections().len(),1);
    let r=apply(&mut d,"new-ground",vec![create_ground(ground())]);
    assert!(r.operation_ids[0].get()>g.get());
    d.verify_index_consistency().unwrap();
}

#[test]
fn ground_foundation_partial_projection_uses_historical_ground_not_today() {
    for interval in [2,4,8] {
        let (mut d,g,a)=one();d.set_history_policy(HistoryPolicy::compact(interval)).unwrap();
        let mut def=circle(g);def.extrusion_width_m=2.0;
        let r=apply(&mut d,"case-only",vec![Command::Engineering(EngineeringCommand::UpdateCircular{analysis_id:a,definition:def.clone(),name:None})]);
        let historical=d.resolve_circular_input(a).unwrap().input;
        if !d.revision_states[&r.revision].checkpoint {
            let projected=d.revision_states[&r.revision].engineering.as_ref().unwrap();
            assert!(projected.ground_sections.is_empty());assert!(projected.circular_cases.contains_key(&a));
        }
        apply(&mut d,"ground-to-draft",vec![update_ground(g,GroundDefinition::default(),None)]);
        assert!(d.resolve_circular_input(a).is_err());
        let opened=Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
        let replay=opened.replay_revision(r.revision).unwrap();
        assert_eq!(replay.resolve_circular_input(a).unwrap().input,historical);
        assert_eq!(opened.history_diff(r.revision,r.revision).unwrap().engineering,EngineeringDelta::default());
        d.undo().unwrap();assert_eq!(d.resolve_circular_input(a).unwrap().input,historical);
    }
}

#[test]
fn ground_foundation_native4_features_survive_ground_only_and_retained_history() {
    let mut d=Document::new("Ground-only native4").unwrap();
    apply(&mut d,"draft",vec![create_ground(GroundDefinition::default())]);
    for retained_only in [false,true] {
        if retained_only {d.undo().unwrap();apply(&mut d,"abandoned-branch",vec![wall()]);assert!(d.engineering().is_empty());}
        let source=d.to_json_string().unwrap();let value:serde_json::Value=serde_json::from_str(&source).unwrap();
        assert_eq!(value["format_version"],4);
        assert_eq!(value["required_features"],serde_json::json!(["typed_drafting_v1","geotech_slope_v1","geotech_circular_v1"]));
        Document::from_json_str(&source).unwrap();
        assert!(Document::from_json_str(&value["document"].to_string()).is_err());
        let mut bare=value["document"].clone();
        bare["required_features"]=serde_json::json!(["future_unknown"]);
        assert!(Document::from_json_str(&bare.to_string()).is_err());
        for version in [2,3] {
            let mut bad=value.clone();bad["format_version"]=version.into();bad["document"]["format_version"]=version.into();
            bad["required_features"]=if version==2 {serde_json::json!(["typed_drafting_v1"])} else {serde_json::json!(["typed_drafting_v1","geotech_slope_v1"])};
            assert!(Document::from_json_str(&bad.to_string()).is_err());
            assert!(Document::from_json_str(&bad["document"].to_string()).is_err());
        }
        let mut bad=value.clone();bad["required_features"]=serde_json::json!(["typed_drafting_v1","geotech_slope_v1"]);
        assert!(Document::from_json_str(&bad.to_string()).is_err());
        let mut bad=value;bad["document"]["format_version"]=3.into();assert!(Document::from_json_str(&bad.to_string()).is_err());
    }
}

#[test]
fn ground_foundation_typed_local_ids_duplicates_and_corruption_are_rejected() {
    for raw in ["\"01\"","\"+1\"","\" 1\"","1","\"18446744073709551616\""] {
        assert!(serde_json::from_str::<GroundSectionId>(raw).is_err(),"{raw}");
    }
    let (mut d,g,a)=one();
    let before=d.to_json_string().unwrap();
    for definition in [
        {let mut v=ground();v.materials.push(v.materials[0].clone());v},
        {let mut v=ground();v.soil_regions[0].material_id="missing".into();v},
        {let mut v=ground();v.terrain_polyline[0].z=f64::NAN;v},
    ] {
        let t=d.begin_transaction("invalid-ground",d.revision()).with(update_ground(g,definition,None));
        assert!(d.commit(t).is_err());assert_eq!(d.to_json_string().unwrap(),before);
    }
    let mut raw:serde_json::Value=serde_json::from_str(&before).unwrap();
    raw["document"]["engineering"]["circular_cases"][a.to_string()]["definition"]["ground_section_id"]="999".into();
    assert!(Document::from_json_str(&raw.to_string()).is_err());
    let mut raw:serde_json::Value=serde_json::from_str(&before).unwrap();
    raw["document"]["engineering"]["ground_sections"][g.to_string()]["source_operation_id"]="999".into();
    assert!(Document::from_json_str(&raw.to_string()).is_err());
    let raw=serde_json::to_string(&serde_json::from_str::<serde_json::Value>(&before).unwrap()).unwrap();
    let record=serde_json::to_string(&serde_json::to_value(d.ground_section(g).unwrap()).unwrap()).unwrap();
    let original=format!("\"ground_sections\":{{\"{g}\":{record}}}");
    assert!(raw.contains(&original));
    let duplicate=format!("\"ground_sections\":{{\"{g}\":{record},\"{g}\":{record}}}");
    assert!(Document::from_json_str(&raw.replacen(&original,&duplicate,1)).is_err());
}

#[test]
fn ground_foundation_new_source_stamps_use_lossless_canonical_decimal_strings() {
    let (d,g,a)=one();
    let mut record=d.ground_section(g).unwrap().clone();
    record.source_operation_id=OperationId(u64::MAX-1);
    let raw=serde_json::to_string(&record).unwrap();
    assert!(raw.contains("\"source_operation_id\":\"18446744073709551614\""));
    assert_eq!(serde_json::from_str::<GroundSection>(&raw).unwrap(),record);
    for invalid in ["\"01\"","1","\"+1\""] {
        let forged=raw.replace("\"18446744073709551614\"",invalid);
        assert!(serde_json::from_str::<GroundSection>(&forged).is_err());
    }
    let resolved=d.resolve_circular_input(a).unwrap();
    let raw=serde_json::to_value(&resolved).unwrap();
    assert!(raw["ground_source_operation_id"].is_string());
    assert!(raw["case_source_operation_id"].is_string());
}

#[test]
fn ground_foundation_legacy_delta_less_checkpoint_preserves_planar_summary() {
    let mut d=Document::new("Real supported planar migration fallback").unwrap();
    d.set_history_policy(HistoryPolicy::full_snapshot()).unwrap();
    let r=apply(&mut d,"planar",vec![planar()]);
    assert!(!r.delta.engineering.created_cases.is_empty());
    d.semantic_schema=2;
    for state in d.revision_states.values_mut() {
        state.delta=None;state.classes=None;state.operations=None;state.relationship_records=None;
    }
    let imported=Document::from_json_str(&serde_json::to_string(&d).unwrap()).unwrap();
    let current=&imported.revision_states[&imported.revision()];
    assert!(current.delta.is_none());assert!(current.history_stack_anchor.is_some());
    assert_eq!(imported.history_delta(1).unwrap().engineering,r.delta.engineering);
    assert_eq!(imported.history_diff(0,1).unwrap().engineering,r.delta.engineering);
}
