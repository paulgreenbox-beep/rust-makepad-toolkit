use crate::*;

fn geo_commit(document:&mut Document,id:&str,commands:Vec<Command>)->CommitReceipt{
    let mut transaction=document.begin_transaction(id,document.revision());
    for command in commands{transaction.push(command);}
    document.commit(transaction).unwrap()
}
fn geo_document()->Document{
    let mut document=Document::new("Single GEO authority").unwrap();
    document.set_history_policy(HistoryPolicy::compact(2)).unwrap();
    let commands=geo_foundation_demo_commands().unwrap().into_iter().filter(|c|matches!(c,Command::GeoFoundation(_))).collect();
    geo_commit(&mut document,"geo-bootstrap",commands);document
}
fn geo_identity()->EngineeringSourceIdentity{
    EngineeringSourceIdentity{source_manifest_sha256:"d".repeat(64),executable_sha256:None,
        solver_version:"geo-foundation-source-test".into(),source_label:"Synthetic manual source control; no engineering approval".into()}
}
fn geo_save_manual(document:&mut Document)->EngineeringSnapshotId{
    let mut numerical=foundation_core::RigidMatInput::default();numerical.mesh=foundation_core::MeshSpec{nx:4,ny:4};
    let input=document.prepare_geo_foundation_analysis_manual("1",numerical,"MANUAL_CONTROL synthetic test load").unwrap();
    let snapshot=analyze_geo_foundation(input,geo_identity()).unwrap();
    geo_commit(document,"manual-archive",vec![Command::GeoFoundation(GeoFoundationCommand::SaveAnalysisSnapshot{snapshot})])
        .delta.geo_foundation.created_snapshots[0]
}

#[test]
fn geo_foundation_bedding_requires_the_exact_authored_layout_source_and_stiffness(){
    let mut document=geo_document();
    let original=document.geo_foundation().layout().unwrap().input.clone();
    let declared=original.bedding_assumptions.get("1").unwrap();
    let numerical=foundation_core::RigidMatInput::default();
    let prepared=document.prepare_geo_foundation_analysis("1",numerical.clone()).unwrap();
    assert_eq!(&prepared.bedding_assumption,declared);
    let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{assumption_note,source_ids,..}=declared else{panic!("demo must declare uniform bedding");};
    assert!(!source_ids.is_empty());
    assert_eq!(&prepared.effective_input.soil.assumption_note,assumption_note);
    for id in source_ids{assert!(document.geo_state().records.sources.contains_key(id));assert!(prepared.ground.records.sources.contains_key(id));}
    let mut mismatched=numerical;
    if let foundation_core::SupportField::Uniform{k_kn_m3,..}=&mut mismatched.soil.support{*k_kn_m3+=1.0;}
    assert!(document.prepare_geo_foundation_analysis("1",mismatched).is_err());
    let before=document.to_json_string().unwrap();
    let mut foreign=original.clone();
    if let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{source_ids,..}=foreign.bedding_assumptions.get_mut("1").unwrap(){*source_ids=vec![geo_core::SourceId::new("MISSING-AUTHORED-BEDDING-SOURCE").unwrap()];}
    let transaction=document.begin_transaction("foreign-bedding-source",document.revision()).with(Command::GeoFoundation(GeoFoundationCommand::ReplaceFoundationLayout{input:foreign}));
    assert!(document.commit(transaction).is_err());assert_eq!(document.to_json_string().unwrap(),before);
    let mut geometry_only=original;geometry_only.bedding_assumptions.clear();
    geo_commit(&mut document,"geometry-only-layout",vec![Command::GeoFoundation(GeoFoundationCommand::ReplaceFoundationLayout{input:geometry_only})]);
    let error=document.prepare_geo_foundation_analysis("1",foundation_core::RigidMatInput::default()).unwrap_err().to_string();
    assert!(error.contains("geometry-only"));
    Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
}

#[test]
fn geo_foundation_authored_bedding_change_stales_only_its_immutable_result(){
    let mut document=geo_document();
    let old_layout=document.geo_foundation().layout().unwrap().input.clone();
    let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{source_ids,..}=old_layout.bedding_assumptions["1"].clone()else{panic!("uniform fixture");};
    let mut alternative=document.geo_state().records.sources[&source_ids[0]].clone();
    alternative.id=geo_core::SourceId::new("SRC-SYNTHETIC-ALTERNATIVE-K").unwrap();
    alternative.title="Explicit synthetic alternative bedding assumption".into();
    alternative.reference="SYNTHETIC manual alternative k=20000 kN/m3; no E-to-k conversion or site verification".into();
    alternative.content_sha256=None;alternative.observed_at=None;alternative.source_date=None;
    alternative.source_page_or_record=Some("geo_foundation_tests: independently declared alternative k=20000 kN/m3".into());
    alternative.source_revision=Some("synthetic-alternative-bedding-v1".into());
    let alternative_id=alternative.id.clone();
    let selection=old_layout.ground_links["1"].clone();
    let scenario_id=selection.scenario_id.unwrap();
    let mut scenario=document.geo_state().records.scenarios[&scenario_id].clone();scenario.source_ids.push(alternative_id.clone());
    geo_commit(&mut document,"declare-alternative-k-source",vec![Command::GeoFoundation(GeoFoundationCommand::EditGeo{command:geo_core::GeoCommand::UpsertSource{record:alternative}}),
        Command::GeoFoundation(GeoFoundationCommand::EditGeo{command:geo_core::GeoCommand::UpsertScenario{record:scenario}})]);
    let snapshot_id=geo_save_manual(&mut document);
    let frozen=document.geo_foundation().snapshot(snapshot_id).unwrap().clone();
    let ground_before=document.geo_state().clone();
    let mut changed=old_layout;
    changed.bedding_assumptions.insert("1".into(),foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{
        k_kn_m3:20000.0,assumption_note:"SYNTHETIC explicit alternative uniform bedding k=20000; no engineering approval".into(),source_ids:vec![alternative_id.clone()]});
    let receipt=geo_commit(&mut document,"select-explicit-alternative-k",vec![Command::GeoFoundation(GeoFoundationCommand::ReplaceFoundationLayout{input:changed})]);
    assert_eq!(document.geo_state(),&ground_before);
    assert!(receipt.delta.geo_foundation.affected_snapshots.contains(&snapshot_id));
    assert_eq!(document.geo_foundation_snapshot_status(snapshot_id),Some(EngineeringSnapshotStatus::Stale));
    assert_eq!(document.geo_foundation().snapshot(snapshot_id),Some(&frozen));
    assert!(document.prepare_geo_foundation_analysis("1",foundation_core::RigidMatInput::default()).is_err());
    let mut matching=foundation_core::RigidMatInput::default();
    if let foundation_core::SupportField::Uniform{k_kn_m3,..}=&mut matching.soil.support{*k_kn_m3=20000.0;}
    let prepared=document.prepare_geo_foundation_analysis("1",matching).unwrap();
    let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{source_ids,..}=prepared.bedding_assumption else{panic!("uniform fixture");};
    assert_eq!(source_ids,vec![alternative_id]);
    document.undo().unwrap();assert_eq!(document.geo_foundation_snapshot_status(snapshot_id),Some(EngineeringSnapshotStatus::Current));
    Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
}

#[test]
fn geo_foundation_queries_are_readonly_and_history_checkpoint_replay_roundtrips(){
    let mut document=geo_document();
    assert_eq!(document.format_version,GEO_FOUNDATION_NATIVE_FORMAT_VERSION);
    let selection=document.geo_foundation().layout().unwrap().input.ground_links["1"].clone();
    let before=document.to_json_string().unwrap();
    let queried=document.query_geo(&selection,&geo_core::GeoQuery::ColumnAt{xy_m:[5.0,6.0]}).unwrap();
    assert_eq!(queried.status,geo_core::QueryStatus::Known);
    assert_eq!(document.to_json_string().unwrap(),before);
    let receipt=geo_commit(&mut document,"selected-geo-archive",vec![Command::GeoFoundation(GeoFoundationCommand::EditGeo{
        command:geo_core::GeoCommand::SaveSnapshot{id:geo_core::GeoSnapshotId::new("selected-source").unwrap(),selection},
    })]);
    let snapshot_id=geo_core::GeoSnapshotId::new("selected-source").unwrap();
    assert_eq!(receipt.delta.geo_foundation.created_geo_snapshots,vec![snapshot_id.clone()]);
    assert!(receipt.affected_objects.contains(&ProjectObjectRef::GeoSnapshot{snapshot_id}));
    let frozen=document.geo_state().clone();
    document.undo().unwrap();assert!(document.geo_state().snapshots.is_empty());
    document.redo().unwrap();assert_eq!(document.geo_state(),&frozen);
    let saved=document.to_json_string().unwrap();
    let reopened=Document::from_json_str(&saved).unwrap();
    assert_eq!(reopened.geo_state(),document.geo_state());
    assert_eq!(reopened.replay_revision(1).unwrap().geo_state().snapshots.len(),0);
    assert_eq!(reopened.replay_revision(2).unwrap().geo_state().snapshots.len(),1);
}

#[test]
fn geo_foundation_selected_dependencies_stale_only_linked_results_and_undo_restores(){
    let mut document=geo_document();
    let id=geo_save_manual(&mut document);
    let frozen=document.geo_foundation().snapshot(id).unwrap().clone();
    assert_eq!(document.geo_foundation_snapshot_status(id),Some(EngineeringSnapshotStatus::Current));
    let mut unrelated=document.geo_state().records.soils.values().next().unwrap().clone();
    unrelated.id=geo_core::GeoSoilId::new("UNLINKED-SOIL").unwrap();unrelated.name="Unlinked explicit record".into();
    let receipt=geo_commit(&mut document,"unrelated-soil",vec![Command::GeoFoundation(GeoFoundationCommand::EditGeo{
        command:geo_core::GeoCommand::UpsertSoil{record:unrelated},
    })]);
    assert!(receipt.delta.geo_foundation.affected_models.is_empty());
    assert!(receipt.delta.geo_foundation.affected_snapshots.is_empty());
    assert_eq!(document.geo_foundation_snapshot_status(id),Some(EngineeringSnapshotStatus::Current));
    let mut linked=document.geo_state().records.soils.values().find(|s|s.id.as_str()!="UNLINKED-SOIL").unwrap().clone();
    if let geo_core::Parameter::Known{value,..}=&mut linked.parameters.cohesion_kpa{*value+=0.1;}else{panic!("fixture strength must be explicit");}
    let receipt=geo_commit(&mut document,"linked-soil",vec![Command::GeoFoundation(GeoFoundationCommand::EditGeo{
        command:geo_core::GeoCommand::UpsertSoil{record:linked},
    })]);
    assert!(receipt.delta.geo_foundation.affected_snapshots.contains(&id));
    assert_eq!(document.geo_foundation_snapshot_status(id),Some(EngineeringSnapshotStatus::Stale));
    assert_eq!(document.geo_foundation().snapshot(id),Some(&frozen));
    document.undo().unwrap();
    assert_eq!(document.geo_foundation_snapshot_status(id),Some(EngineeringSnapshotStatus::Current));
    let reopened=Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.geo_foundation_snapshot_status(id),Some(EngineeringSnapshotStatus::Current));
    assert_eq!(reopened.geo_foundation().snapshot(id),Some(&frozen));
}

#[test]
fn geo_foundation_mixed_failure_restores_state_history_format_and_allocation(){
    let mut document=Document::new("Atomic GEO source").unwrap();
    let before=document.to_json_string().unwrap();
    let mut transaction=document.begin_transaction("invalid-mixed-geo",0);
    for command in geo_foundation_demo_commands().unwrap().into_iter().filter(|c|matches!(c,Command::GeoFoundation(_))){transaction.push(command);}
    transaction.push(Command::CreateLevel{name:"Invalid".into(),elevation_m:f64::NAN,height_m:None});
    assert!(document.commit(transaction).is_err());
    assert_eq!(document.to_json_string().unwrap(),before);
    let commands=geo_foundation_demo_commands().unwrap().into_iter().filter(|c|matches!(c,Command::GeoFoundation(_))).collect();
    let receipt=geo_commit(&mut document,"valid-mixed-geo",commands);
    assert_eq!(receipt.operation_ids[0].get(),1);
    document.undo().unwrap();
    assert!(document.geo_foundation().is_empty());
    let saved=document.to_json_string().unwrap();
    let value:serde_json::Value=serde_json::from_str(&saved).unwrap();
    assert_eq!(value["format_version"],7);
    let mut reopened=Document::from_json_str(&saved).unwrap();
    reopened.redo().unwrap();assert!(!reopened.geo_foundation().is_empty());
    let receipt=geo_commit(&mut reopened,"new-id-after-redo",vec![Command::CreateLevel{name:"Valid".into(),elevation_m:0.0,height_m:None}]);
    assert!(receipt.operation_ids[0].get()>2);
}

#[test]
fn geo_foundation_rejects_downgrade_receipt_forgery_and_wrong_snapshot_source(){
    let mut document=geo_document();let id=geo_save_manual(&mut document);
    let source=document.to_json_string().unwrap();
    let value:serde_json::Value=serde_json::from_str(&source).unwrap();
    let mut downgraded=value.clone();downgraded["format_version"]=serde_json::json!(6);downgraded["document"]["format_version"]=serde_json::json!(6);
    downgraded["required_features"].as_array_mut().unwrap().pop();
    assert!(Document::from_json_str(&downgraded.to_string()).is_err());
    let mut forged=value.clone();forged["document"]["command_receipts"]["manual-archive"]["recorded_commands"]=serde_json::Value::Null;
    assert!(Document::from_json_str(&forged.to_string()).is_err());
    let mut snapshot=document.geo_foundation().snapshot(id).unwrap().clone();
    let mut changed=document.geo_state().records.soils.values().next().unwrap().clone();
    let changed_id=changed.id.clone();
    if let geo_core::Parameter::Known{value,..}=&mut changed.parameters.cohesion_kpa{*value+=0.25;}else{panic!("fixture cohesion must be explicit");}
    geo_commit(&mut document,"new-selected-source",vec![Command::GeoFoundation(GeoFoundationCommand::EditGeo{command:geo_core::GeoCommand::UpsertSoil{record:changed}})]);
    let mut current=document.prepare_geo_foundation_analysis("1",snapshot.input.effective_input.clone()).unwrap();
    let claimed_source=snapshot.input.ground.source_revision.clone();
    let rejected_old_source=geo_core::make_geo_snapshot(document.geo_state(),current.ground.id.clone(),&current.ground.selection,
        claimed_source.clone()).unwrap_err();
    assert!(matches!(rejected_old_source,geo_core::GeoError::InvalidInput{path,..}if path=="source_revision"));
    // An adversary controls this local fork, including its record revisions.
    // Keep the changed cohesion, falsely age its row/head and recompute every
    // hash. The actual Document and its original operation journal stay intact.
    let mut untrusted_fork=document.geo_state().clone();
    untrusted_fork.head_revision=Some(claimed_source.clone());
    untrusted_fork.records.soils.get_mut(&changed_id).unwrap().revision=claimed_source.operation_id;
    untrusted_fork.snapshots.retain(|_,snapshot|snapshot.source_revision.operation_id<=claimed_source.operation_id);
    assert_ne!(untrusted_fork.records.soils[&changed_id].parameters.cohesion_kpa,
        snapshot.input.ground.records.soils[&changed_id].parameters.cohesion_kpa);
    geo_core::validate_geo_state(&untrusted_fork).unwrap();
    current.ground=geo_core::make_geo_snapshot(&untrusted_fork,current.ground.id.clone(),&current.ground.selection,claimed_source).unwrap();
    geo_core::validate_snapshot_binding(&current.ground).unwrap();
    current.scenario=current.ground.records.scenarios.values().next().unwrap().clone();
    let resolved=geo_core::resolve_geo_scenario(&current.ground,&current.scenario).unwrap();
    let layout=foundation_core::FoundationLayout{id:"immutable-placement".into(),elements:vec![current.placement.clone()]};
    current.geo_context=foundation_core::geo_context::resolve_geo_context(&resolved,&layout).unwrap();
    snapshot=GeoFoundationAnalysisSnapshot::new(current,snapshot.foundation_result.clone(),geo_identity()).unwrap();
    let before=document.to_json_string().unwrap();
    let transaction=document.begin_transaction("forged-old-source",document.revision()).with(Command::GeoFoundation(GeoFoundationCommand::SaveAnalysisSnapshot{snapshot}));
    let error=document.commit(transaction).unwrap_err().to_string();
    assert!(error.contains("original selected source closure"),"expected exact historical-source refusal, got {error}");
    assert_eq!(document.to_json_string().unwrap(),before);
}

#[test]
fn geo_foundation_slope_adapter_is_immutable_and_refuses_unknown_water(){
    let document=geo_document();let selection=document.geo_foundation().layout().unwrap().input.ground_links["1"].clone();
    let state=document.geo_state();let snapshot=geo_core::make_geo_snapshot(state,geo_core::GeoSnapshotId::new("slope-source").unwrap(),&selection,state.head_revision.clone().unwrap()).unwrap();
    let scenario=snapshot.records.scenarios.values().next().unwrap().clone();
    let resolved=geo_core::resolve_geo_scenario(&snapshot,&scenario).unwrap();
    let projection=geo_slope_adapter::project_geo_section_for_slope(&resolved,&[[0.0,6.0],[10.0,6.0]],2.0).unwrap();
    projection.validate().unwrap();assert_eq!(projection.ground.soil_regions.len(),3);
    let mut records=state.records.clone();records.scenarios.get_mut(&scenario.id).unwrap().water=geo_core::WaterScenario::Unknown;
    let mut changed=state.clone();changed.records=records;
    let unknown=geo_core::make_geo_snapshot(&changed,geo_core::GeoSnapshotId::new("unknown-water").unwrap(),&selection,state.head_revision.clone().unwrap()).unwrap();
    let scenario=unknown.records.scenarios.values().next().unwrap().clone();let resolved=geo_core::resolve_geo_scenario(&unknown,&scenario).unwrap();
    assert!(geo_slope_adapter::project_geo_section_for_slope(&resolved,&[[0.0,6.0],[10.0,6.0]],2.0).is_err());
}

#[test]
fn geo_foundation_slope_projection_matches_frozen_affine_geometry_and_existing_solver(){
    use geotech_core::circular::*;
    let state=geo_core::synthetic_demo_state().unwrap();
    let selection=geo_core::GeoSnapshotSelection{model_id:geo_core::GroundModelId::new("GEO-DEMO-1").unwrap(),scenario_id:Some(geo_core::GroundScenarioId::new("DRY-DEMO-1").unwrap())};
    let snapshot=geo_core::make_geo_snapshot(&state,geo_core::GeoSnapshotId::new("frozen-affine-projection").unwrap(),&selection,state.head_revision.clone().unwrap()).unwrap();
    let scenario=snapshot.records.scenarios[&geo_core::GroundScenarioId::new("DRY-DEMO-1").unwrap()].clone();
    let resolved=geo_core::resolve_geo_scenario(&snapshot,&scenario).unwrap();
    let projected=geo_slope_adapter::project_geo_section_for_slope(&resolved,&[[0.0,6.0],[5.0,6.0],[10.0,6.0]],5.0).unwrap();
    // Independently authored frozen control values. No production query,
    // interpolator, fixture result or solver produces these expected arrays.
    let line=|z:[f64;3]|vec![geotech_core::Point2::new(0.0,z[0]),geotech_core::Point2::new(5.0,z[1]),geotech_core::Point2::new(10.0,z[2])];
    let material=|id:&str,name:&str,gamma:f64,cohesion:f64,phi:f64|GroundMaterial{
        material_id:geotech_core::circular::SoilMaterialId(id.into()),properties:geotech_core::SoilMaterial{name:name.into(),
            unit_weight_kn_per_m3:gamma,effective_cohesion_kpa:cohesion,effective_friction_angle_deg:phi,
            provenance:"Frozen independently authored synthetic affine control; no site acceptance".into()}};
    let expected=GroundSection2D{
        section_id:"independent-affine-control".into(),coordinate_system:SectionCoordinateSystem::CartesianXzMetres,
        terrain_polyline:line([100.3,100.8,101.3]),
        layer_interfaces:vec![LayerInterface{interface_id:"fill-base-control".into(),polyline:line([98.8,99.2,99.6])},
            LayerInterface{interface_id:"loam-base-control".into(),polyline:line([95.24,95.74,96.24])}],
        soil_regions:vec![SoilRegion{region_id:"fill-control".into(),material_id:"SOIL-FILL".into()},
            SoilRegion{region_id:"loam-control".into(),material_id:"SOIL-LOAM".into()},
            SoilRegion{region_id:"sand-control".into(),material_id:"SOIL-SAND".into()}],
        bottom_boundary:line([88.3,88.8,89.3]),
        materials:vec![material("SOIL-FILL","Fill",18.0,5.0,22.0),material("SOIL-LOAM","Loam",19.0,18.0,24.0),material("SOIL-SAND","Sand",19.5,0.0,32.0)],model_revision:1,
    };
    let compare_line=|actual:&[geotech_core::Point2],expected:&[geotech_core::Point2]|{
        assert_eq!(actual.len(),expected.len());for(a,b)in actual.iter().zip(expected){assert!((a.x-b.x).abs()<1e-10);assert!((a.z-b.z).abs()<1e-10);}
    };
    compare_line(&projected.ground.terrain_polyline,&expected.terrain_polyline);
    assert_eq!(projected.ground.layer_interfaces.len(),2);
    for(a,b)in projected.ground.layer_interfaces.iter().zip(&expected.layer_interfaces){compare_line(&a.polyline,&b.polyline);}
    compare_line(&projected.ground.bottom_boundary,&expected.bottom_boundary);
    assert_eq!(projected.ground.soil_regions.iter().map(|r|&r.material_id).collect::<Vec<_>>(),expected.soil_regions.iter().map(|r|&r.material_id).collect::<Vec<_>>());
    for(actual,control)in projected.ground.materials.iter().zip(&expected.materials){
        assert_eq!(actual.material_id,control.material_id);
        assert_eq!(actual.properties.unit_weight_kn_per_m3,control.properties.unit_weight_kn_per_m3);
        assert_eq!(actual.properties.effective_cohesion_kpa,control.properties.effective_cohesion_kpa);
        assert_eq!(actual.properties.effective_friction_angle_deg,control.properties.effective_friction_angle_deg);
    }
    let input=|ground|CircularAnalysisInput{analysis_id:"adapter-equivalence-only".into(),source_revision:"frozen-affine-control-v1".into(),ground,
        slip_surface:CircularSlipSurface::from_center_radius("authored-circle".into(),geotech_core::Point2::new(5.0,102.0),4.5,SlideDirection::NegativeX).unwrap(),
        basis:AnalysisBasis::DryStatic,units:geotech_core::InputUnits::default(),extrusion_width_m:1.0};
    let config=CircularAnalysisConfig{slices:SliceConfig{target_slices:16,verify_refinement:false},..Default::default()};
    // Compare the complete typed outcome, including diagnostic mathematics and
    // all physical classifications. Source identity hashes intentionally differ:
    // the independent control has its own section IDs and material provenance.
    fn compare_outcome(actual:&serde_json::Value,control:&serde_json::Value,path:&str){
        match(actual,control){
            (serde_json::Value::Object(a),serde_json::Value::Object(b))=>{
                assert_eq!(a.keys().collect::<Vec<_>>(),b.keys().collect::<Vec<_>>(),"outcome shape at {path}");
                for(key,value)in a{
                    if key=="input_fingerprint"||key=="contact_input_fingerprint"{
                        for fingerprint in [value,&b[key]]{
                            let text=fingerprint.as_str().expect("typed source fingerprint");
                            let hash=text.strip_prefix("sha256:").unwrap_or(text);
                            assert!(hash.len()==64&&hash.bytes().all(|byte|byte.is_ascii_hexdigit()),"invalid source identity at {path}.{key}");
                        }
                    }else{compare_outcome(value,&b[key],&format!("{path}.{key}"));}
                }
            }
            (serde_json::Value::Array(a),serde_json::Value::Array(b))=>{
                assert_eq!(a.len(),b.len(),"outcome array length at {path}");
                for(index,(a,b))in a.iter().zip(b).enumerate(){compare_outcome(a,b,&format!("{path}[{index}]"));}
            }
            (serde_json::Value::Number(a),serde_json::Value::Number(b))=>{
                if a.is_i64()&&b.is_i64(){assert_eq!(a,b,"integer outcome at {path}");}
                else{
                    let(a,b)=(a.as_f64().unwrap(),b.as_f64().unwrap());
                    let tolerance=1e-9*(1.0+a.abs().max(b.abs()));
                    assert!(a.is_finite()&&b.is_finite()&&(a-b).abs()<=tolerance,
                        "numerical outcome at {path}: actual={a:.17e}, control={b:.17e}, tolerance={tolerance:.3e}");
                }
            }
            _=>assert_eq!(actual,control,"classified outcome at {path}"),
        }
    }
    let actual_input=input(projected.ground);
    let control_input=input(expected);
    let actual=analyze_specified_circle(&actual_input,&config);
    let control=analyze_specified_circle(&control_input,&config);
    let(actual_refusal,control_refusal)=match(actual,control){
        (Err(actual),Err(control))=>(actual,control),
        (actual,control)=>panic!("the authored contact-refusal control must remain rejected on both paths: actual={actual:?}, control={control:?}"),
    };
    assert_eq!(actual_refusal.code,CircularErrorCode::BishopPhysicallyInadmissible);
    assert_eq!(control_refusal.code,CircularErrorCode::BishopPhysicallyInadmissible);
    for refusal in [&actual_refusal,&control_refusal]{
        let diagnostic=refusal.bishop_diagnostic.as_ref().expect("typed Bishop refusal diagnostic");
        assert_eq!(diagnostic.status,BishopConvergenceStatus::Converged);
        let root=diagnostic.mathematical_root.as_ref().expect("retained refused mathematical root");
        assert!(root.accepted_factor_of_safety.is_none(),"a refused root is not an accepted factor");
    }
    compare_outcome(&serde_json::to_value(&actual_refusal).unwrap(),&serde_json::to_value(&control_refusal).unwrap(),"refusal");

    // Explicit diagnostic execution additionally exposes the Fellenius result
    // discarded by the ordinary refused analysis. It grants no acceptance.
    let actual_diagnostic=diagnose_specified_circle(&actual_input,&config).unwrap();
    let control_diagnostic=diagnose_specified_circle(&control_input,&config).unwrap();
    for diagnostic in [&actual_diagnostic,&control_diagnostic]{
        assert!(diagnostic.diagnostic_only&&!diagnostic.accepted);
        assert!(diagnostic.bishop_result.accepted_factor_of_safety.is_none());
        assert!(!diagnostic.numerically_verified&&diagnostic.engineering_review_required);
    }
    compare_outcome(&serde_json::to_value(&actual_diagnostic).unwrap(),&serde_json::to_value(&control_diagnostic).unwrap(),"diagnostic");
}

fn native7_history_fixture() -> serde_json::Value {
    let mut document = Document::new("Native7 strict retained authority").unwrap();
    document.set_history_policy(HistoryPolicy::compact(2)).unwrap();
    for index in 0..3 {
        let transaction = document.begin_transaction(format!("level-{index}"), document.revision())
            .with(Command::CreateLevel {
                name: format!("Level {index}"),
                elevation_m: f64::from(index),
                height_m: Some(3.0),
            });
        document.commit(transaction).unwrap();
    }
    document.undo().unwrap();
    document.format_version = GEO_FOUNDATION_NATIVE_FORMAT_VERSION;
    serde_json::from_str(&document.to_json_string().unwrap()).unwrap()
}

#[test]
fn native7_rejects_unknown_fields_in_each_retained_authority_container() {
    let original = native7_history_fixture();
    assert!(Document::from_json_str(&original.to_string()).is_ok());
    let mut pointers = vec![
        "/document".to_owned(),
        "/document/revision_states/0".to_owned(),
        "/document/revision_states/1/delta".to_owned(),
        "/document/revision_states/1/delta/summary".to_owned(),
        "/document/command_receipts/level-0".to_owned(),
        "/document/command_receipts/level-0/receipt".to_owned(),
        "/document/command_receipts/level-0/recorded_commands/0/create_level".to_owned(),
        "/document/undo_stack/0".to_owned(),
        "/document/redo_stack/0".to_owned(),
    ];
    let first_change = original.pointer("/document/revision_states/1/delta/changes/0")
        .unwrap().as_object().unwrap().keys().next().unwrap();
    pointers.push(format!("/document/revision_states/1/delta/changes/0/{first_change}"));
    for pointer in pointers {
        let mut altered = original.clone();
        let object = altered.pointer_mut(&pointer).unwrap_or_else(|| panic!("missing fixture {pointer}"))
            .as_object_mut().unwrap();
        object.insert("unknown_native7_authority".into(), serde_json::json!({"kept": false}));
        assert!(Document::from_json_str(&altered.to_string()).is_err(), "accepted unknown field at {pointer}");
    }
}

#[test]
fn native_authority_preserves_explicit_metadata_and_rejects_unknown_legacy_before_migration() {
    let mut document = Document::new("Metadata").unwrap();
    let metadata = BTreeMap::from([("future_vendor_data".into(), serde_json::json!({"nested": [1, 2], "opaque": true}))]);
    let transaction = document.begin_transaction("metadata", 0).with(Command::CreateSpace {
        name: "Metadata room".into(), level_id: None, boundary: None, metadata,
    });
    document.commit(transaction).unwrap();
    document.format_version = GEO_FOUNDATION_NATIVE_FORMAT_VERSION;
    let source = document.to_json_string().unwrap();
    let reopened = Document::from_json_str(&source).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&reopened.to_json_string().unwrap()).unwrap();
    assert_eq!(saved.pointer("/document/elements/1/kind/space/metadata/future_vendor_data/opaque"), Some(&serde_json::json!(true)));

    document.format_version = STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION;
    let mut legacy: serde_json::Value = serde_json::from_str(&document.to_json_string().unwrap()).unwrap();
    let reopened_legacy=Document::from_json_str(&legacy.to_string()).unwrap();
    let legacy_saved:serde_json::Value=serde_json::from_str(&reopened_legacy.to_json_string().unwrap()).unwrap();
    assert_eq!(legacy_saved.pointer("/document/elements/1/kind/space/metadata/future_vendor_data/opaque"),Some(&serde_json::json!(true)));
    legacy["document"]["legacy_unknown_extra"] = serde_json::json!({"unsupported": true});
    assert!(Document::from_json_str(&legacy.to_string()).is_err());
}

#[test]
fn valid_native_public_writer_versions_2_through_6_open_without_geo_migration(){
    for version in 2..=6{
        let mut document=Document::new(format!("Valid Native{version} writer fixture")).unwrap();
        geo_commit(&mut document,"legacy-level",vec![Command::CreateLevel{name:"Original level".into(),elevation_m:3.0,height_m:Some(3.0)}]);
        document.format_version=version;
        let source=document.to_json_string().unwrap();
        let reopened=Document::from_json_str(&source).unwrap();
        assert_eq!(reopened.format_version,version);
        assert_eq!(reopened.revision(),1);
        assert!(reopened.geo_foundation().is_empty());
        assert_eq!(serde_json::from_str::<serde_json::Value>(&reopened.to_json_string().unwrap()).unwrap(),serde_json::from_str::<serde_json::Value>(&source).unwrap());
        let mut unknown:serde_json::Value=serde_json::from_str(&source).unwrap();
        unknown["document"]["unknown_legacy_authority"]=serde_json::json!({"must_not_be_lost":true});
        assert!(Document::from_json_str(&unknown.to_string()).is_err(),"Native{version} ignored unsupported authority before migration");
    }
}

#[test]
fn native7_exact_feature_envelope_rejects_missing_extra_and_reordered_flags() {
    let source = native7_history_fixture();
    for mode in 0..3 {
        let mut changed = source.clone();
        let features = changed["required_features"].as_array_mut().unwrap();
        match mode {
            0 => { features.pop(); }
            1 => features.push(serde_json::json!("future_geo_v2")),
            _ => features.swap(0, 1),
        }
        assert!(Document::from_json_str(&changed.to_string()).is_err());
    }
}
