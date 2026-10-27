//! Produce real persisted foundation documents for a separate old-reader check.
use ai_revit_core::*;
use geotech_core::circular as c;
use geotech_core::{AnalysisConditions, InputUnits, Point2 as GPoint, SlopeAnalysisInput, SoilMaterial};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, fs, path::{Path,PathBuf}};

fn ground() -> GroundDefinition {
    GroundDefinition {
        coordinate_system:c::SectionCoordinateSystem::CartesianXzMetres,
        terrain_polyline:vec![GPoint::new(-6.0,3.0),GPoint::new(6.0,0.0)],
        layer_interfaces:vec![],
        soil_regions:vec![c::SoilRegion{region_id:"region-a".into(),material_id:"soil-a".into()}],
        bottom_boundary:vec![GPoint::new(-6.0,-3.0),GPoint::new(6.0,-3.0)],
        materials:vec![c::GroundMaterial{material_id:"soil-a".into(),properties:SoilMaterial{
            name:"Explicit foundation fixture soil".into(),unit_weight_kn_per_m3:18.0,
            effective_cohesion_kpa:0.0,effective_friction_angle_deg:28.0,
            provenance:"Compatibility input only; no engineering acceptance".into(),
        }}],
    }
}
fn create_ground(definition:GroundDefinition)->Command {
    Command::Engineering(EngineeringCommand::CreateGround{definition,name:"Shared ground".into()})
}
fn create_circle(ground_section_id:GroundSectionId)->Command {
    Command::Engineering(EngineeringCommand::CreateCircular{
        name:"Explicit prescribed circle".into(),
        definition:CircularCaseDefinition{
            ground_section_id,
            slip_surface:c::CircularSlipSurface{slip_id:"explicit-slip".into(),center:GPoint::new(0.0,5.0),
                radius_m:4.875,arc_branch:c::ArcBranch::Lower,slide_direction:c::SlideDirection::PositiveX,
                selected_endpoints:None},
            basis:c::AnalysisBasis::DryStatic,units:InputUnits::default(),extrusion_width_m:1.0,
            config:c::CircularAnalysisConfig::default(),search_spec:None,
        },
    })
}
fn wall()->Command {
    Command::CreateElement{name:"Compatibility wall".into(),class_id:BUILTIN_WALL_CLASS_ID.into(),
        kind:ElementKind::Wall(WallSpec{start:Point2::new(0.0,0.0),end:Point2::new(5.0,0.0),
            base_elevation_m:0.0,height_m:3.0,thickness_m:0.2}),properties:BTreeMap::new()}
}
fn planar()->Command {
    Command::Engineering(EngineeringCommand::CreateSlope{name:"Preserved planar authority".into(),input:SlopeAnalysisInput{
        section:geotech_core::triangular_section(10.0,45.0,30.0,1.0).unwrap(),
        soil:SoilMaterial{name:"Explicit planar fixture".into(),unit_weight_kn_per_m3:18.0,
            effective_cohesion_kpa:0.0,effective_friction_angle_deg:30.0,provenance:"Compatibility fixture".into()},
        conditions:AnalysisConditions::default(),units:InputUnits::default(),required_factor_of_safety:None,standard:None,
    }})
}
fn apply(d:&mut Document,id:&str,commands:Vec<Command>)->Result<CommitReceipt,CoreError>{
    let mut transaction=d.begin_transaction(id,d.revision());
    for command in commands{transaction.push(command);}
    d.commit(transaction)
}
fn hash(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn persist(d:&Document,path:&Path,kind:&str)->Result<serde_json::Value,Box<dyn Error>>{
    if path.exists(){return Err(format!("Refusing to replace {}",path.display()).into());}
    d.save(path)?;
    let bytes=fs::read(path)?;
    let reopened=Document::open(path)?;
    reopened.verify_index_consistency()?;
    let value:serde_json::Value=serde_json::from_slice(&bytes)?;
    if value["format_version"]!=4{return Err("Fixture did not retain native4".into());}
    if value["required_features"]!=serde_json::json!(["typed_drafting_v1","geotech_slope_v1","geotech_circular_v1"]){
        return Err("Fixture feature set is not exact native4".into());
    }
    Ok(serde_json::json!({"kind":kind,"path":path,"sha256":hash(&bytes),"size_bytes":bytes.len(),
        "revision":d.revision(),"ground_count":d.ground_sections().len(),"circular_case_count":d.circular_cases().len(),
        "planar_case_count":d.slope_cases().len(),"real_transactions":true,"document_save":true,
        "reopened_through_current_draft_document":true,"old_reader_test":"NOT_RUN_BY_PRODUCER",
        "saved_circular_results":false}))
}
fn main()->Result<(),Box<dyn Error>>{
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if args.len()!=1{return Err("Usage: gate8_foundation_fixtures <new-output-directory>".into());}
    let output=PathBuf::from(&args[0]);
    if output.exists(){return Err("Output directory must be new".into());}
    let exe=std::env::current_exe()?;let exe_before=hash(&fs::read(&exe)?);
    fs::create_dir_all(&output)?;
    let mut rows=Vec::new();
    let mut draft=Document::new("Standalone genuine draft ground")?;
    apply(&mut draft,"create-draft-ground",vec![create_ground(GroundDefinition::default())])?;
    assert!(draft.circular_cases().is_empty());assert!(draft.ground_sections().values().all(|g|g.definition.materials.is_empty()));
    rows.push(persist(&draft,&output.join("ground-only-draft.native4.ai-revit.json"),"ground_only_draft")?);

    let mut mixed=Document::new("Mixed BIM planar ground circular drafting")?;
    apply(&mut mixed,"mixed-real-transaction",vec![wall(),planar(),create_ground(ground()),
        create_circle(GroundSectionId::new(3)),Command::Draft(DraftCommand::CreateDrawing{
            name:"Compatibility section".into(),plane:DrawingPlane::default()})])?;
    rows.push(persist(&mixed,&output.join("mixed.native4.ai-revit.json"),"mixed_shared_transaction")?);

    let mut retained=Document::new("New authority retained only in history")?;
    apply(&mut retained,"create-ground-and-case",vec![create_ground(ground()),create_circle(GroundSectionId::new(1))])?;
    retained.undo()?;
    apply(&mut retained,"abandon-redo-with-wall",vec![wall()])?;
    assert!(retained.ground_sections().is_empty()&&retained.circular_cases().is_empty());
    rows.push(persist(&retained,&output.join("retained-history-only.native4.ai-revit.json"),"retained_history_only_abandoned_branch")?);
    let exe_after=hash(&fs::read(&exe)?);if exe_before!=exe_after{return Err("Producer executable changed during run".into());}
    let manifest=serde_json::json!({"schema":"gate8-real-document-save-inputs-v1","files":rows,
        "producer_executable":exe,"producer_executable_sha256_before":exe_before,
        "producer_executable_sha256_after":exe_after,"compiled_example_source_sha256":hash(include_bytes!("gate8_foundation_fixtures.rs")),
        "draft_solver_version":c::CIRCULAR_SOLVER_VERSION,"draft_source_fingerprint":c::source_fingerprint(),
        "scope":"Real typed transactions and Document::save/open in an unapplied draft; no GUI, distinct-process acceptance, historical reader execution, snapshots, search runs or reports"});
    let bytes=serde_json::to_vec_pretty(&manifest)?;fs::write(output.join("producer-receipt.json"),&bytes)?;
    println!("{}",serde_json::json!({"status":"SAVED_REAL_FOUNDATION_INPUTS","file_count":3,
        "receipt":output.join("producer-receipt.json"),"receipt_sha256":hash(&bytes),
        "draft_source_fingerprint":c::source_fingerprint(),"producer_executable_sha256":exe_after}));
    Ok(())
}
