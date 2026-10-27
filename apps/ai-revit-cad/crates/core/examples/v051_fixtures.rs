//! Reproducible v0.5.1 workload recipes through unchanged authoritative Core.
//! Writes only create_new files beneath a caller-supplied absolute output root.
//! Generation and Core round trips are not native, GPU, QTO or release acceptance.
use ai_revit_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn micros(start: Instant) -> u64 { start.elapsed().as_micros().min(u64::MAX as u128) as u64 }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn file_hash(path: &Path) -> Result<String> {
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 { break; }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn apply(document: &mut Document, id: &str, commands: Vec<Command>) -> Result<CommitReceipt> {
    let mut transaction = document.begin_transaction(id, document.revision());
    for command in commands { transaction.push(command); }
    Ok(document.commit(transaction)?)
}

fn fresh(root: &Path, name: &str, bytes: &[u8]) -> Result<Value> {
    let path = root.join(name);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let expected = hash(bytes);
    let disk_hash = file_hash(&path)?;
    if disk_hash != expected { return Err("fresh fixture disk hash differs from serialized bytes".into()); }
    Ok(json!({"name":name,"path":path,"bytes":bytes.len(),"sha256":disk_hash,"create_new":true}))
}

fn fresh_json<T: Serialize>(root: &Path, name: &str, value: &T) -> Result<Value> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fresh(root, name, &bytes)
}

#[derive(Deserialize, Serialize)]
struct AllocatorProbe {
    next_element_id: u64,
    next_operation_id: u64,
    next_type_id: u64,
    next_level_id: u64,
    next_relationship_id: u64,
    draft_ids: Value,
}
#[derive(Deserialize)]
struct EnvelopeProbe { document: AllocatorProbe }

fn benchmark_metadata(document:&Document,record:&Value)->Result<Value> {
    let drawing=document.drawings().values().next();
    let layer=drawing.and_then(|drawing|drawing.layers.values().find(|layer|layer.visible&&!layer.locked));
    let selectable=drawing.and_then(|drawing|drawing.entities.values().find(|entity| {
        drawing.layers.get(&entity.layer_id).is_some_and(|layer|layer.visible&&!layer.locked)
    }));
    let mut selectable_record=Value::Null;
    let mut draft_edit=Value::Null;
    if let (Some(drawing),Some(entity))=(drawing,selectable) {
        let (kind,center,pick)=match &entity.geometry {
            DraftGeometry::Line {start,end}=>{
                let center=DraftPoint2::new((start.x+end.x)*0.5,(start.y+end.y)*0.5);
                ("line",center,center)
            },
            DraftGeometry::Circle {center,radius}=>("circle",*center,DraftPoint2::new(center.x+radius,center.y)),
            DraftGeometry::Arc {center,radius,start_angle,sweep_angle}=>{
                let angle=start_angle+sweep_angle*0.5;
                ("arc",*center,DraftPoint2::new(center.x+radius*angle.cos(),center.y+radius*angle.sin()))
            },
            DraftGeometry::Polyline {vertices,..}=>{
                let point=vertices.first().ok_or("selectable polyline has no vertices")?.point;
                ("polyline",point,point)
            },
        };
        selectable_record=json!({"entity_id":entity.id,"layer_id":entity.layer_id,"kind":kind,
            "analytic_center_world_m":center,"analytic_pick_world_m":pick,"original_geometry":entity.geometry,
            "pick_scope":"analytic world point on the entity; caller converts through current actual Camera2D to Canvas-local logical pixels"});
        if let DraftGeometry::Line {start,end}=&entity.geometry {
            let original=entity.geometry.clone();
            let edited=DraftGeometry::Line {start:*start,end:DraftPoint2::new(end.x+0.25,end.y)};
            draft_edit=json!({"drawing_id":drawing.id,"entity_id":entity.id,"layer_id":entity.layer_id,
                "original_geometry":original,"edited_geometry":edited,
                "apply_command":Command::Draft(DraftCommand::UpdateDraftEntity {drawing_id:drawing.id,entity_id:entity.id,geometry:edited}),
                "restore_command":Command::Draft(DraftCommand::UpdateDraftEntity {drawing_id:drawing.id,entity_id:entity.id,geometry:original}),
                "edit_scope":"extend the existing normal unlocked line endpoint by 0.25m; no entity allocation"});
        }
    }
    let mut bim_edit=Value::Null;
    if let Some(element)=document.elements().find(|element|matches!(element.kind,ElementKind::Wall(_))) {
        let original=element.kind.clone();
        let mut edited=original.clone();
        if let ElementKind::Wall(wall)=&mut edited { wall.end.x+=0.25; }
        bim_edit=json!({"element_id":element.id,"original_kind":original,"edited_kind":edited,
            "apply_command":Command::UpdateElement {element_id:element.id,kind:edited},
            "restore_command":Command::UpdateElement {element_id:element.id,kind:original},
            "edit_scope":"extend first wall end.x by 0.25m, keeping wall height, thickness and hosted opening placement unchanged"});
    }
    let manual_geometry=document.drawings().iter().flat_map(|(drawing_id,drawing)| {
        drawing.entities.iter().map(move |(entity_id,entity)|(*drawing_id,*entity_id,entity.layer_id,&entity.geometry))
    }).collect::<Vec<_>>();
    let manual_geometry_sha256=hash(&serde_json::to_vec(&manual_geometry)?);
    Ok(json!({"drawing_id":drawing.map(|drawing|drawing.id),"normal_unlocked_layer_id":layer.map(|layer|layer.id),
        "selectable_entity":selectable_record,"local_draft_edit":draft_edit,"local_bim_edit":bim_edit,
        "underlay_reference_id":document.view_definitions().keys().next(),
        "manual_geometry_sha256":manual_geometry_sha256,
        "manual_geometry_hash_scope":"compact JSON ordered tuples of (drawing_id,entity_id,layer_id,analytic_geometry), including locked and hidden entities; style and derived BIM underlay excluded",
        "safe_restore_state":{"method":"document.open","path":record["path"],"sha256":record["sha256"],
            "document_uuid":document.document_uuid,"revision":document.revision(),"next_allocators":record["next_allocators"],
            "reason":"reopen exact unchanged fixture to restore full authority, history and allocator state; undo or inverse commands alone do not restore original receipt/history bytes",
            "ui_precondition":"reopening replaces runtime identity/epoch; activate drawing/workspace and derive fresh camera/frame/context after open; never reuse prior native intent or snapshot token"},
        "within_sample_geometry_restore":{"method":"undo","scope":"restores the immediately preceding edit geometry; revision, retained receipts, history and allocator high-water state may differ from the initial fixture","strict_history_reset":"safe_restore_state document.open"}}))
}

fn document_file(root: &Path, name: &str, document: &Document) -> Result<Value> {
    let started = Instant::now();
    let source = document.to_json_string()?;
    let serialize_us = micros(started);
    let allocators: EnvelopeProbe = serde_json::from_str(&source)?;
    let mut record = fresh(root, name, source.as_bytes())?;
    let started = Instant::now();
    let reopened = Document::open(root.join(name))?;
    let reopen_us = micros(started);
    if reopened.to_json_string()? != source
        || reopened.document_uuid != document.document_uuid
        || reopened.revision() != document.revision()
        || reopened.drafting() != document.drafting()
    { return Err(format!("fixture {name} failed canonical Core open round trip").into()); }
    document.verify_index_consistency()?;
    reopened.verify_index_consistency()?;
    let mut kinds = BTreeMap::<&str, usize>::new();
    for element in document.elements() {
        let kind = match &element.kind {
            ElementKind::Wall(_) => "walls", ElementKind::Floor(_) => "floors",
            ElementKind::Roof(_) => "roofs", ElementKind::Opening(_) => "hosted_openings",
            ElementKind::Door(_) => "doors", ElementKind::Window(_) => "windows",
            ElementKind::PolygonalOpening(_) => "polygonal_openings", ElementKind::Space(_) => "spaces",
        };
        *kinds.entry(kind).or_default() += 1;
    }
    let mut drawing_records = Vec::new();
    for drawing in document.drawings().values() {
        let mut type_counts = BTreeMap::<&str, usize>::new();
        let mut positive_bulges = 0usize;
        let mut negative_bulges = 0usize;
        for entity in drawing.entities.values() {
            let kind = match &entity.geometry {
                DraftGeometry::Line { .. } => "line", DraftGeometry::Circle { .. } => "circle",
                DraftGeometry::Arc { .. } => "arc",
                DraftGeometry::Polyline { vertices, .. } => {
                    positive_bulges += vertices.iter().filter(|v| v.bulge > 0.).count();
                    negative_bulges += vertices.iter().filter(|v| v.bulge < 0.).count();
                    "polyline"
                }
            };
            *type_counts.entry(kind).or_default() += 1;
        }
        let layers = drawing.layers.values().map(|layer| json!({
            "id":layer.id,"name":layer.name,"visible":layer.visible,"locked":layer.locked,
            "entity_count":drawing.entities.values().filter(|entity|entity.layer_id == layer.id).count(),
        })).collect::<Vec<_>>();
        drawing_records.push(json!({"id":drawing.id,"name":drawing.name,"revision":drawing.revision,
            "entity_count":drawing.entities.len(),"entity_ids":drawing.entities.keys().collect::<Vec<_>>(),
            "type_counts":type_counts,"positive_bulge_vertices":positive_bulges,"negative_bulge_vertices":negative_bulges,
            "layers":layers,"source_import_count":drawing.import_sources.len()}));
    }
    record["document_uuid"] = json!(document.document_uuid);
    record["revision"] = json!(document.revision());
    record["bim_element_ids"] = json!(document.elements().map(|e|e.id).collect::<Vec<_>>());
    record["bim_counts"] = json!(kinds);
    record["level_count"] = json!(document.levels().len());
    record["levels"] = json!(document.levels().collect::<Vec<_>>());
    record["relationship_count"] = json!(document.relationships().count());
    record["relationships"] = json!(document.relationships().collect::<Vec<_>>());
    record["draft_entity_count"] = json!(document.drafting().entity_count());
    record["drawings"] = json!(drawing_records);
    record["view_definitions"] = json!(document.view_definitions());
    record["next_allocators"] = serde_json::to_value(allocators.document)?;
    record["history_metrics"] = serde_json::to_value(document.history_metrics()?)?;
    record["operation_count"] = json!(document.operation_tree().len());
    record["canonical_core_round_trip"] = json!("PASS");
    record["round_trip_runtime_identity_changed"] = json!(document.runtime_instance_id() != reopened.runtime_instance_id());
    record["generation_timings_us"] = json!({"serialization_with_integrity_audit":serialize_us,"core_reopen_with_integrity_audit":reopen_us,
        "scope":"headless generator measurements; excluded from native performance baseline"});
    record["history_imported"] = json!(false);
    let benchmark=benchmark_metadata(document,&record)?;
    record["original_manual_geometry_sha256"] = benchmark["manual_geometry_sha256"].clone();
    record["benchmark"] = benchmark;
    Ok(record)
}

fn wall(index: usize, small: bool) -> WallSpec {
    let x = (index % if small {4} else {100}) as f64 * 8.;
    let y = (index / if small {4} else {100}) as f64 * 6.;
    WallSpec { start:Point2::new(x,y),end:Point2::new(x+6.,y),
        base_elevation_m:if small {(index/8) as f64*3.2}else{0.},height_m:3.,thickness_m:0.2 }
}
fn element(name: String, class: &str, kind: ElementKind) -> Command {
    Command::CreateElement {name,class_id:class.into(),kind,properties:BTreeMap::new()}
}
fn footprint() -> Vec<Point2> {
    vec![Point2::new(-1.,-1.),Point2::new(31.,-1.),Point2::new(31.,24.),Point2::new(-1.,24.)]
}

fn small_bim() -> Result<(Document, Vec<ElementId>)> {
    let mut document = Document::new("v0.5.1 Scene A small BIM")?;
    let mut commands = (0..16).map(|i|element(format!("Small wall {}",i+1),BUILTIN_WALL_CLASS_ID,
        ElementKind::Wall(wall(i,true)))).collect::<Vec<_>>();
    for i in 0..2 {
        commands.push(element(format!("Floor {}",i+1),BUILTIN_FLOOR_CLASS_ID,
            ElementKind::Floor(FloorSpec {boundary:footprint(),holes:Vec::new(),elevation_m:i as f64*3.2,thickness_m:0.25})));
    }
    commands.push(element("Roof".into(),BUILTIN_ROOF_CLASS_ID,
        ElementKind::Roof(RoofSpec {footprint:footprint(),elevation_m:6.4,thickness_m:0.2,slope_x:0.04,slope_y:0.02})));
    for i in 0..3 { commands.push(Command::CreateLevel{name:format!("Level {i}"),elevation_m:i as f64*3.2,height_m:Some(3.2)}); }
    let seed = apply(&mut document,"v051-small-bim-seed",commands)?;
    let wall_ids = seed.created_elements[..16].to_vec();
    let mut commands = Vec::new();
    for i in [0usize,4,8,12] {
        let host = wall(i,true);
        commands.push(Command::CreateOpening {
            name:format!("Hosted opening {}",i/4+1),
            spec:OpeningSpec {center:Point2::new(host.start.x+2.,host.start.y),elevation_m:host.base_elevation_m+0.5,
                width_m:1.,height_m:1.4,depth_m:0.2,rotation_rad:0.},
            properties:BTreeMap::new(),host_id:wall_ids[i],type_id:None,
        });
    }
    let openings = apply(&mut document,"v051-small-hosted-openings",commands)?.created_elements;
    let mut relationships = Vec::new();
    for (i,id) in wall_ids.iter().copied().enumerate() {
        relationships.push(Command::AssignLevel {element_id:id,level_id:seed.created_levels[i/8]});
        relationships.push(Command::CreateRelationship {relationship_type:RelationshipType::SupportedBy,
            source_id:EntityRef::Element(id),target_id:EntityRef::Element(seed.created_elements[16+i/8]),metadata:BTreeMap::new()});
    }
    for i in 0..2 { relationships.push(Command::AssignLevel {element_id:seed.created_elements[16+i],level_id:seed.created_levels[i]}); }
    relationships.push(Command::AssignLevel {element_id:seed.created_elements[18],level_id:seed.created_levels[2]});
    for (i,id) in openings.into_iter().enumerate() {
        relationships.push(Command::AssignLevel {element_id:id,level_id:seed.created_levels[i/2]});
    }
    for i in [0usize,4,8,12] {
        relationships.push(Command::CreateRelationship {relationship_type:RelationshipType::SupportedBy,
            source_id:EntityRef::Element(seed.created_elements[18]),target_id:EntityRef::Element(wall_ids[i]),metadata:BTreeMap::new()});
    }
    apply(&mut document,"v051-small-levels-and-dependencies",relationships)?;
    if document.elements().len()!=23 || document.levels().len()!=3 { return Err("small BIM count contract mismatch".into()); }
    Ok((document,wall_ids))
}

fn add_drawing(document: &mut Document, label: &str) -> Result<(DrawingId,DraftLayerId)> {
    let receipt = apply(document,&format!("{label}-drawing"),vec![Command::Draft(DraftCommand::CreateDrawing {
        name:format!("{label} XY"),plane:DrawingPlane::default(),
    })])?;
    let drawing = receipt.created_drawings[0];
    let layer = *document.drawing(drawing).ok_or("new drawing missing")?.layers.keys().next().ok_or("default layer missing")?;
    Ok((drawing,layer))
}
fn draft_entity(drawing:DrawingId,layer:DraftLayerId,geometry:DraftGeometry)->Command {
    Command::Draft(DraftCommand::CreateDraftEntity {drawing_id:drawing,layer_id:layer,geometry,style:DraftStyle::default()})
}

fn mixed_geometry(index:usize)->DraftGeometry {
    let x=(index%100) as f64*2.;
    let y=(index/100) as f64*2.;
    match index%4 {
        0=>DraftGeometry::Line {start:DraftPoint2::new(x,y),end:DraftPoint2::new(x+0.9,y+0.6)},
        1=>DraftGeometry::Circle {center:DraftPoint2::new(x+0.5,y+0.5),radius:0.4},
        2=>DraftGeometry::Arc {center:DraftPoint2::new(x+0.5,y+0.5),radius:0.4,start_angle:0.2,sweep_angle:std::f64::consts::PI*1.5},
        _=>DraftGeometry::Polyline {vertices:vec![PolylineVertex::new(DraftPoint2::new(x,y),0.5),
            PolylineVertex::new(DraftPoint2::new(x+0.5,y+0.7),-0.5),PolylineVertex::new(DraftPoint2::new(x+1.,y),0.)],closed:false},
    }
}

fn add_mixed_drafting(document:&mut Document,count:usize,label:&str,walls:&[ElementId])->Result<Value> {
    if count%16!=8 && count%16!=0 { return Err("fixture requires a balanced four-kind/four-layer count".into()); }
    let (drawing,normal)=add_drawing(document,label)?;
    let mut layers=vec![normal];
    let mut commands=vec![Command::Draft(DraftCommand::UpdateDraftLayer {drawing_id:drawing,layer_id:normal,
        name:Some("normal".into()),visible:None,locked:None,style:None})];
    for name in ["auxiliary","locked","hidden"] {
        commands.push(Command::Draft(DraftCommand::CreateDraftLayer {drawing_id:drawing,name:name.into(),
            visible:true,locked:false,style:DraftStyle::default()}));
    }
    apply(document,&format!("{label}-layers-unlocked-during-authoring"),commands)?;
    for name in ["auxiliary","locked","hidden"] {
        layers.push(document.drawing(drawing).ok_or("drawing missing")?.layers.values()
            .find(|layer|layer.name==name).ok_or("authored layer missing")?.id);
    }
    let mut commands=(0..count).map(|i|draft_entity(drawing,layers[(i/4+i%4)%4],mixed_geometry(i))).collect::<Vec<_>>();
    if !walls.is_empty() {
        // Z=1 intersects the lower eight walls. Upper walls remain explicitly
        // referenced and correctly produce no intersection at this elevation.
        commands.push(Command::Draft(DraftCommand::CreateViewDefinition {drawing_id:drawing,
            name:"Explicit BIM ElementId XY mesh section Z=1m".into(),source:DraftSourceReference {
                element_ids:walls.to_vec(),elevation_m:Some(1.),coordinate_system:DraftCoordinateSystem::Xy,
            }}));
    }
    let created=apply(document,&format!("{label}-analytic-entities-and-reference"),commands)?;
    apply(document,&format!("{label}-final-layer-state"),vec![
        Command::Draft(DraftCommand::UpdateDraftLayer {drawing_id:drawing,layer_id:layers[2],name:None,visible:None,locked:Some(true),style:None}),
        Command::Draft(DraftCommand::UpdateDraftLayer {drawing_id:drawing,layer_id:layers[3],name:None,visible:Some(false),locked:None,style:None}),
    ])?;
    let d=document.drawing(drawing).ok_or("drawing missing")?;
    if d.entities.len()!=count || d.layers.len()!=4 { return Err("mixed drafting count contract mismatch".into()); }
    Ok(json!({"drawing_id":drawing,"normal_layer_id":normal,"layer_ids":layers,
        "entity_count":count,"each_analytic_kind_count":count/4,"each_layer_entity_count":count/4,
        "example_entity_ids":created.created_draft_entities.iter().take(16).collect::<Vec<_>>(),
        "view_definition_ids":created.delta.drafting.created_views,"source_wall_ids":walls,
        "authoring":"Core CreateDraftEntity commands; layer lock/hide applied only after authoring; no imported history"}))
}

fn circle_fixture(count:usize)->Result<Document> {
    let mut document=Document::new(format!("v0.5.1 display boundary {count} circles R10000m"))?;
    let (drawing,layer)=add_drawing(&mut document,&format!("display-{count}"))?;
    apply(&mut document,"v051-display-circles",(0..count).map(|i|draft_entity(drawing,layer,DraftGeometry::Circle {
        center:DraftPoint2::new((i%8) as f64*0.01,(i/8) as f64*0.01),radius:10_000.,
    })).collect())?;
    Ok(document)
}

fn replacement_pair()->Result<(Document,Document)> {
    let mut a=Document::new("v0.5.1 same visible identity")?;
    let mut b=Document::new("v0.5.1 same visible identity")?;
    b.document_uuid=a.document_uuid;
    for (document,end_x) in [(&mut a,3.),(&mut b,7.)] {
        let (drawing,layer)=add_drawing(document,"replacement")?;
        apply(document,"replacement-analytic-line",vec![draft_entity(drawing,layer,DraftGeometry::Line {
            start:DraftPoint2::new(0.,0.),end:DraftPoint2::new(end_x,1.),
        })])?;
    }
    if a.document_uuid!=b.document_uuid || a.revision()!=b.revision() || a.drawings().keys().ne(b.drawings().keys())
        || a.runtime_instance_id()==b.runtime_instance_id() || a.to_json_string()?==b.to_json_string()?
    { return Err("same-identity replacement pair contract mismatch".into()); }
    Ok((a,b))
}

fn read_only_reference(path:&Path,purpose:&str)->Result<Value> {
    if !path.is_file() { return Ok(json!({"path":path,"status":"NOT_AVAILABLE","purpose":purpose})); }
    Ok(json!({"path":path.canonicalize()?,"bytes":fs::metadata(path)?.len(),"sha256":file_hash(path)?,
        "access":"read-only; not copied, generated or overwritten","purpose":purpose}))
}

fn main()->Result<()> {
    let mut args=std::env::args_os().skip(1);
    if args.next().as_deref()!=Some(std::ffi::OsStr::new("--output-dir")) {
        return Err("usage: v051_fixtures --output-dir <absolute fresh output directory>".into());
    }
    let output=PathBuf::from(args.next().ok_or("missing output directory")?);
    if args.next().is_some() || !output.is_absolute() { return Err("one absolute output directory is required".into()); }
    fs::create_dir_all(&output)?;
    let output=output.canonicalize()?;
    let names=["small-bim.ai-revit.json","drafting-mixed-1000.ai-revit.json","drafting-mixed-10000.ai-revit.json",
        "mixed-bim-drafting.ai-revit.json","gpu-bim-10000.ai-revit.json","display-frame-budget-64.ai-revit.json",
        "display-circle-one.ai-revit.json","replacement-a.ai-revit.json","replacement-b.ai-revit.json","generation-manifest.json"];
    if names.iter().any(|name|output.join(name).exists()) { return Err("fixture output exists; use a fresh directory, nothing overwritten".into()); }
    let started=Instant::now();
    let mut files=Vec::new();
    let mut scenes=BTreeMap::<String,Value>::new();
    {
        let (small,walls)=small_bim()?;
        let mut record=document_file(&output,names[0],&small)?;
        record["benchmark_scene"]=json!("A");
        record["qto_native_execution"]=json!("NOT_RUN; authored BIM geometry is the native QTO workload");
        record["analytic_wall_volume_reference_m3"]=json!({"gross":16.*6.*3.*0.2,"net_after_four_hosted_openings":16.*6.*3.*0.2-4.*1.*1.4*0.2,
            "scope":"scalar authored wall-volume reference only; no native geometry/QTO execution or roof quantity inference"});
        scenes.insert("A".into(),record.clone());files.push(record);
        let mut mixed=small.clone();
        let mapping=add_mixed_drafting(&mut mixed,1000,"mixed-bim-drafting",&walls)?;
        let mut record=document_file(&output,names[3],&mixed)?;
        record["benchmark_scene"]=json!("D");record["drafting_mapping"]=mapping;
        record["bim_origin"]=json!({"source":names[0],"same_document_uuid":mixed.document_uuid==small.document_uuid,
            "preserved_wall_ids":walls,"preserved_all_bim_element_ids":small.elements().map(|element|element.id).collect::<Vec<_>>(),
            "history":"cloned same authoritative history plus newly authored drafting commands; no external history imported"});
        scenes.insert("D".into(),record.clone());files.push(record);
    }
    for (scene,count,name) in [("B",1000,names[1]),("C2",10000,names[2])] {
        let mut document=Document::new(format!("v0.5.1 {scene} representative mixed drafting {count}"))?;
        let mapping=add_mixed_drafting(&mut document,count,&format!("mixed-{count}"),&[])?;
        let mut record=document_file(&output,name,&document)?;
        record["benchmark_scene"]=json!(scene);record["drafting_mapping"]=mapping;
        scenes.insert(scene.into(),record.clone());files.push(record);
    }
    {
        let mut document=Document::new("v0.5.1 Scene E 10000 independent BIM walls")?;
        apply(&mut document,"v051-gpu-10000-walls",(0..10000).map(|i|element(format!("GPU wall {}",i+1),
            BUILTIN_WALL_CLASS_ID,ElementKind::Wall(wall(i,false)))).collect())?;
        let mut record=document_file(&output,names[4],&document)?;
        record["benchmark_scene"]=json!("E");record["qto_gpu_execution"]=json!("NOT_RUN");
        record["authored_wall_dimensions_m"]=json!({"length":6.,"height":3.,"thickness":0.2,"grid_columns":100,"grid_spacing_xy":[8.,6.]});
        record["existing_gpu_workload_difference"]=json!("This contracted scene is 10000 walls. Historical v04 geometry-10k is 9000 walls plus 1000 hosted openings and remains a separate unchanged GPU regression fixture.");
        scenes.insert("E".into(),record.clone());files.push(record);
    }
    for (count,name) in [(64,names[5]),(1,names[6])] {
        let document=circle_fixture(count)?;
        let mut record=document_file(&output,name,&document)?;
        record["boundary_recipe"]=json!({"count":count,"radius_m":10000.,"camera_center_m":[0.,0.],
            "proposed_pixels_per_metre":if count==64{64.}else{1000.},"viewport":"use actual native canvas dimensions/DPI",
            "intent":if count==64{"frame 100000 generated segment budget failure after successful geometry commit"}else{"camera curve 4096 segment rejection without camera publication"},
            "native_display_result":"NOT_RUN; proposed camera values must be verified in native runtime"});
        files.push(record);
    }
    {
        let (a,b)=replacement_pair()?;
        let mut left=document_file(&output,names[7],&a)?;
        let mut right=document_file(&output,names[8],&b)?;
        let common=json!({"same_uuid":a.document_uuid==b.document_uuid,"same_revision":a.revision()==b.revision(),
            "same_drawing_id":"1","same_entity_id":"1","different_analytic_end_x_m":[3.,7.],
            "different_generator_runtime_identity":a.runtime_instance_id()!=b.runtime_instance_id(),
            "native_document_replacement":"NOT_RUN; open B after capturing a native intent from A"});
        left["replacement_pair"]=common.clone();right["replacement_pair"]=common;
        files.push(left);files.push(right);
    }
    let app_root=Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("core parent missing")?.parent().ok_or("app root missing")?;
    let c1=read_only_reference(&app_root.join("artifacts/v05/fixtures/manual-10000.ai-revit.json"),"C1 unchanged historical v05 line-only 10000 scene")?;
    let historical_gpu=read_only_reference(&app_root.join("artifacts/v04/geometry-10k-document.json"),"unchanged historical GPU regression: 9000 walls plus 1000 hosted openings")?;
    scenes.insert("C1".into(),c1);
    let executable=std::env::current_exe()?;
    let manifest=json!({"schema":"ai-revit-v051-generated-fixtures-v1","task_id":"AI-REVIT-HARDENING-001",
        "created_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),"output_root":output,
        "generator":{"package_version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"executable_path":executable,
            "executable_disk_sha256":file_hash(&executable)?,"compiled_example_source_sha256":hash(include_bytes!("v051_fixtures.rs")),
            "source_scope":"example source only; root source freeze binds unchanged Core dependencies","history_imported":false},
        "scope":{"canonical_core_round_trip":"verified per file","native_ui":"NOT_RUN","GPU":"NOT_RUN","QTO":"NOT_RUN",
            "performance_acceptance":"NOT_RUN","source_fixtures_overwritten":false,"timings":"generator only, not native benchmark samples"},
        "scenes":scenes,"historical_gpu_reference":historical_gpu,"files":files,
        "total_generation_wall_us_before_manifest_write":micros(started),"manifest_hash_scope":"files excludes this manifest itself"});
    let record=fresh_json(&output,"generation-manifest.json",&manifest)?;
    println!("{}",serde_json::to_string(&json!({"manifest":record,"native_acceptance":"NOT_RUN"}))?);
    Ok(())
}
