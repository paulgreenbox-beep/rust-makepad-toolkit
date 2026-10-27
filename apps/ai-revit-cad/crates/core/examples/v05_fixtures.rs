//! Fresh v0.5 acceptance/performance inputs. This executable never opens or
//! overwrites historical fixtures. The caller supplies an absolute output root.
use ai_revit_core::*;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const GOOD_DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n5\nA\n8\n0\n10\n0\n20\n0\n11\n1000\n21\n1000\n0\nCIRCLE\n5\nB\n8\n0\n10\n2000\n20\n2000\n40\n500\n0\nARC\n5\nC\n8\n0\n10\n4000\n20\n1000\n40\n500\n50\n0\n51\n180\n0\nLWPOLYLINE\n5\nD\n8\n0\n90\n3\n70\n0\n10\n0\n20\n2000\n42\n0.5\n10\n1000\n20\n2500\n42\n-0.5\n10\n2000\n20\n2000\n0\nENDSEC\n0\nEOF\n";
const CRITICAL_DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nELLIPSE\n5\nE1\n8\n0\n10\n0\n20\n0\n11\n1000\n21\n0\n40\n0.5\n41\n0\n42\n6.283185307179586\n0\nENDSEC\n0\nEOF\n";
const MISSING_UNITS_DXF: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n5\nA\n8\n0\n10\n0\n20\n0\n11\n1000\n21\n1000\n0\nENDSEC\n0\nEOF\n";

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros().min(u64::MAX as u128) as u64
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn apply(document: &mut Document, id: &str, commands: Vec<Command>) -> Result<CommitReceipt> {
    let mut transaction = document.begin_transaction(id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    Ok(document.commit(transaction)?)
}
fn wall(length: f64) -> ElementKind {
    ElementKind::Wall(WallSpec {
        start: Point2::new(0., 0.),
        end: Point2::new(length, 0.),
        base_elevation_m: 0.,
        height_m: 3.,
        thickness_m: 0.2,
    })
}
fn create_wall() -> Command {
    Command::CreateElement {
        name: "Fixture wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: wall(6.),
        properties: BTreeMap::new(),
    }
}
fn create_drawing() -> Command {
    Command::Draft(DraftCommand::CreateDrawing {
        name: "Mixed XY · Z=1 m".into(),
        plane: DrawingPlane::default(),
    })
}
fn geometries() -> Vec<DraftGeometry> {
    vec![
        DraftGeometry::Line {
            start: DraftPoint2::new(0., 1.),
            end: DraftPoint2::new(6., 1.),
        },
        DraftGeometry::Circle {
            center: DraftPoint2::new(3., 2.5),
            radius: 0.65,
        },
        DraftGeometry::Arc {
            center: DraftPoint2::new(7.5, 2.),
            radius: 1.,
            start_angle: 0.,
            sweep_angle: std::f64::consts::PI * 1.5,
        },
        DraftGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(DraftPoint2::new(0., 4.), 0.5),
                PolylineVertex::new(DraftPoint2::new(2., 5.), -0.75),
                PolylineVertex::new(DraftPoint2::new(4., 4.), 0.),
            ],
            closed: false,
        },
    ]
}
fn entity(drawing_id: DrawingId, layer_id: DraftLayerId, geometry: DraftGeometry) -> Command {
    Command::Draft(DraftCommand::CreateDraftEntity {
        drawing_id,
        layer_id,
        geometry,
        style: DraftStyle::default(),
    })
}
fn fresh(root: &Path, name: &str, bytes: &[u8]) -> Result<Value> {
    let path = root.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(json!({"name":name,"path":path,"bytes":bytes.len(),"sha256":hash(bytes)}))
}
fn json_file<T: Serialize>(root: &Path, name: &str, value: &T) -> Result<Value> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fresh(root, name, &bytes)
}
fn document_file(root: &Path, name: &str, document: &Document) -> Result<Value> {
    let started = Instant::now();
    let source = document.to_json_string()?;
    let serialization_us = micros(started);
    let started = Instant::now();
    let mut record = fresh(root, name, source.as_bytes())?;
    let fresh_write_us = micros(started);
    let started = Instant::now();
    let reopened = Document::open(root.join(name))?;
    let reopen_us = micros(started);
    if reopened.document_uuid != document.document_uuid
        || reopened.revision() != document.revision()
        || reopened.drafting() != document.drafting()
        || reopened.to_json_string()? != source
    {
        return Err("generated native fixture did not round-trip canonically".into());
    }
    record["document_id"] = json!(document.document_uuid);
    record["revision"] = json!(document.revision());
    record["bim_ids"] = json!(document.elements().map(|e| e.id).collect::<Vec<_>>());
    record["drawing_ids"] = json!(document.drawings().keys().copied().collect::<Vec<_>>());
    record["draft_entity_count"] = json!(document.drafting().entity_count());
    record["operation_count"] = json!(document.operation_tree().len());
    record["timing_us"] = json!({"native_serialization_including_invariant_audit":serialization_us,"fresh_create_new_write_and_sync":fresh_write_us,"core_open_including_migration_and_integrity_audit":reopen_us});
    record["writer"] = json!("Document::to_json_string + create_new; no existing file overwritten");
    record["headless_round_trip"] = json!("PASS");
    Ok(record)
}

fn mixed(root: &Path, files: &mut Vec<Value>) -> Result<Value> {
    let generation_started = Instant::now();
    let mut document = Document::new("v0.5 mixed acceptance input")?;
    let seed = apply(
        &mut document,
        "fixtures-mixed-seed",
        vec![create_wall(), create_drawing()],
    )?;
    let wall_id = seed.created_elements[0];
    let drawing_id = seed.created_drawings[0];
    let layer = apply(
        &mut document,
        "fixtures-annotation-layer",
        vec![Command::Draft(DraftCommand::CreateDraftLayer {
            drawing_id,
            name: "Manual annotations".into(),
            visible: true,
            locked: false,
            style: DraftStyle {
                color: [0.35, 0.9, 1., 1.],
                width_px: 1.8,
            },
        })],
    )?
    .delta
    .drafting
    .changed_layers[0];
    let mut commands = geometries()
        .into_iter()
        .map(|g| entity(drawing_id, layer, g))
        .collect::<Vec<_>>();
    commands.push(Command::Draft(DraftCommand::CreateViewDefinition {
        drawing_id,
        name: "BIM mesh section XY at Z=1 m".into(),
        source: DraftSourceReference {
            element_ids: vec![wall_id],
            elevation_m: Some(1.),
            coordinate_system: DraftCoordinateSystem::Xy,
        },
    }));
    let analytic = apply(&mut document, "fixtures-analytic-and-reference", commands)?;
    let view_id = analytic.delta.drafting.created_views[0];
    let manual_ids = analytic.created_draft_entities.clone();
    let provenance = apply(
        &mut document,
        "fixtures-authored-import-provenance",
        vec![Command::Draft(DraftCommand::ImportDraftingEntities {
            batch: ImportBatch {
                drawing_id,
                source: ImportSourceSpec {
                    name: "good-mm.dxf".into(),
                    sha256: hash(GOOD_DXF.as_bytes()),
                    unit_resolution: UnitResolution {
                        declared_unit: Some("millimetre".into()),
                        resolved_unit: "millimetre".into(),
                        scale_to_m: 0.001,
                        decision_source: "fixture_authored_known_SI_correspondence".into(),
                    },
                    report: json!({"scope":"authored Core provenance input for one known line; not an executed DXF conversion","real_dxf_preview":"NOT_RUN","source_fixture":"good-mm.dxf"}),
                },
                layers: vec![ImportLayer {
                    name: "Authored provenance line".into(),
                    visible: true,
                    locked: false,
                    style: DraftStyle {
                        color: [1., 0.65, 0.25, 1.],
                        width_px: 1.5,
                    },
                }],
                entities: vec![ImportEntity {
                    geometry: DraftGeometry::Line {
                        start: DraftPoint2::new(0., 0.),
                        end: DraftPoint2::new(1., 1.),
                    },
                    layer_name: "Authored provenance line".into(),
                    style: DraftStyle {
                        color: [1., 0.65, 0.25, 1.],
                        width_px: 1.5,
                    },
                    source_handle: Some("A".into()),
                }],
            },
        })],
    )?;
    let baseline_revision = document.revision();
    files.push(document_file(
        root,
        "mixed-before.ai-revit.json",
        &document,
    )?);
    let before_geometry = manual_ids
        .iter()
        .map(|id| {
            document
                .draft_entity(drawing_id, *id)
                .unwrap()
                .geometry
                .clone()
        })
        .collect::<Vec<_>>();
    let mut changed = document.clone();
    let receipt = apply(
        &mut changed,
        "fixtures-one-cross-domain-change",
        vec![
            Command::UpdateElement {
                element_id: wall_id,
                kind: wall(9.),
            },
            entity(
                drawing_id,
                layer,
                DraftGeometry::Line {
                    start: DraftPoint2::new(0., 6.),
                    end: DraftPoint2::new(9., 6.),
                },
            ),
        ],
    )?;
    if receipt.revision_after != baseline_revision + 1 {
        return Err("mixed transaction did not create exactly one revision".into());
    }
    files.push(json_file(
        root,
        "mixed-cross-domain-receipt.json",
        &receipt,
    )?);
    files.push(document_file(root, "mixed-after.ai-revit.json", &changed)?);
    let extra_id = receipt.created_draft_entities[0];
    let started = Instant::now();
    let undo = changed.undo()?.ok_or("mixed undo missing")?;
    let undo_us = micros(started);
    if changed.element(wall_id) != document.element(wall_id)
        || changed.draft_entity(drawing_id, extra_id).is_some()
    {
        return Err("mixed undo failed".into());
    }
    let started = Instant::now();
    let redo = changed.redo()?.ok_or("mixed redo missing")?;
    let redo_us = micros(started);
    if changed.draft_entity(drawing_id, extra_id).is_none() {
        return Err("mixed redo failed".into());
    }
    for (id, geometry) in manual_ids.iter().zip(before_geometry) {
        if changed.draft_entity(drawing_id, *id).map(|e| &e.geometry) != Some(&geometry) {
            return Err("manual geometry changed during BIM edit/undo/redo".into());
        }
    }
    files.push(json_file(root, "mixed-undo-receipt.json", &undo)?);
    files.push(json_file(root, "mixed-redo-receipt.json", &redo)?);
    files.push(document_file(
        root,
        "mixed-after-undo-redo.ai-revit.json",
        &changed,
    )?);
    Ok(
        json!({"document_id":document.document_uuid,"wall_id":wall_id,"drawing_id":drawing_id,"view_definition_id":view_id,"manual_entity_ids":manual_ids,"authored_provenance_entity_ids":provenance.created_draft_entities,"mixed_created_entity_id":extra_id,
        "before_revision":baseline_revision,"after_revision":receipt.revision_after,"after_undo_redo_revision":changed.revision(),"section_elevation_m":1.,"wall_length_before_m":6.,"wall_length_after_m":9.,
        "timing_us":{"generation_wall_including_fixture_serialization_and_reopen":micros(generation_started),"cross_domain_core_transaction":receipt.transaction_duration_us,"undo":undo_us,"redo":redo_us},"native_GUI_acceptance":"NOT_RUN"}),
    )
}

fn performance(root: &Path, count: usize, files: &mut Vec<Value>) -> Result<Value> {
    let started = Instant::now();
    let mut document = Document::new(format!("v0.5 {count} manual LINE performance input"))?;
    let drawing = apply(
        &mut document,
        "fixtures-performance-drawing",
        vec![create_drawing()],
    )?
    .created_drawings[0];
    let layer = *document
        .drawing(drawing)
        .ok_or("performance drawing missing")?
        .layers
        .keys()
        .next()
        .ok_or("default layer missing")?;
    let columns = (count as f64).sqrt().ceil() as usize;
    let commands = (0..count)
        .map(|i| {
            let x = (i % columns) as f64 * 2.;
            let y = (i / columns) as f64 * 2.;
            entity(
                drawing,
                layer,
                DraftGeometry::Line {
                    start: DraftPoint2::new(x, y),
                    end: DraftPoint2::new(x + 0.9, y + 0.35),
                },
            )
        })
        .collect();
    let receipt = apply(
        &mut document,
        &format!("fixtures-{count}-line-batch"),
        commands,
    )?;
    if receipt.created_draft_entities.len() != count || document.drafting().entity_count() != count
    {
        return Err("performance fixture entity count mismatch".into());
    }
    let generation_wall_us = micros(started);
    let record = document_file(root, &format!("manual-{count}.ai-revit.json"), &document)?;
    files.push(record);
    Ok(
        json!({"entity_count":count,"geometry_mix":"LINE only; mixed fixture covers curved primitives","drawing_id":drawing,"layer_id":layer,"first_entity_id":receipt.created_draft_entities.first(),"last_entity_id":receipt.created_draft_entities.last(),"document_id":document.document_uuid,"revision":document.revision(),
        "timing_us":{"generation_wall_before_serialization":generation_wall_us,"core_transaction":receipt.transaction_duration_us},"native_pan_zoom_pick_GPU_measurements":"NOT_RUN"}),
    )
}

fn lab_migration(root: &Path, files: &mut Vec<Value>) -> Result<Value> {
    let mut lab = drafting_core::LabDocument::new();
    let drawing = lab.default_drawing_id();
    let layer = *lab.drawings()[&drawing]
        .layers
        .keys()
        .next()
        .ok_or("Lab default layer missing")?;
    lab.apply(drafting_core::CommandEnvelope {
        command_id: "fixtures-lab-analytic".into(),
        expected_revision: 0,
        operations: geometries()
            .into_iter()
            .map(|geometry| drafting_core::DraftOperation::Create {
                drawing_id: drawing,
                layer_id: layer,
                geometry,
                style: DraftStyle::default(),
            })
            .collect(),
    })?;
    let source = lab.to_json()?;
    files.push(fresh(root, "lab-source-v011.json", source.as_bytes())?);
    let mut document = Document::new("v0.5 Lab migration input")?;
    let seed = apply(
        &mut document,
        "fixtures-lab-target-seed",
        vec![create_wall(), create_drawing()],
    )?;
    let target = seed.created_drawings[0];
    let target_layer = *document
        .drawing(target)
        .ok_or("target drawing missing")?
        .layers
        .keys()
        .next()
        .ok_or("target layer missing")?;
    apply(
        &mut document,
        "fixtures-existing-manual-id",
        vec![entity(
            target,
            target_layer,
            DraftGeometry::Line {
                start: DraftPoint2::new(-2., -2.),
                end: DraftPoint2::new(-1., -1.),
            },
        )],
    )?;
    let preview = document.prepare_lab_import(&source, "lab-source-v011.json")?;
    let before = document.revision();
    let receipt = apply(&mut document, "fixtures-one-lab-import", preview.commands)?;
    if document.revision() != before + 1 || preview.history_imported {
        return Err("Lab migration was not one history-independent transaction".into());
    }
    for (old, new) in &preview.mapping.entities {
        let output_drawing = preview.mapping.drawings[&drawing];
        if document
            .draft_entity(output_drawing, *new)
            .map(|e| &e.geometry)
            != lab.drawings()[&drawing]
                .entities
                .get(old)
                .map(|e| &e.geometry)
        {
            return Err("Lab analytic mapping mismatch".into());
        }
    }
    files.push(json_file(
        root,
        "lab-import-mapping.json",
        &preview.mapping,
    )?);
    files.push(json_file(root, "lab-import-receipt.json", &receipt)?);
    files.push(document_file(
        root,
        "lab-imported.ai-revit.json",
        &document,
    )?);
    Ok(
        json!({"source_document_id":lab.document_id(),"source_revision":lab.revision(),"target_document_id":document.document_uuid,"target_revision_before":before,"target_revision_after":document.revision(),"mapping":preview.mapping,"history_imported":false,"headless_analytic_mapping":"PASS","native_reopen_acceptance":"NOT_RUN"}),
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--output-dir")) {
        return Err("usage: v05_fixtures --output-dir <absolute output directory>".into());
    }
    let output = PathBuf::from(args.next().ok_or("missing --output-dir value")?);
    if args.next().is_some() || !output.is_absolute() {
        return Err("exactly one absolute output directory is required".into());
    }
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let names = [
        "generation-manifest.json",
        "good-mm.dxf",
        "critical-unsupported-mm.dxf",
        "missing-units.dxf",
        "mixed-before.ai-revit.json",
        "mixed-after.ai-revit.json",
        "manual-1000.ai-revit.json",
        "manual-10000.ai-revit.json",
        "lab-source-v011.json",
        "lab-imported.ai-revit.json",
    ];
    if names.iter().any(|name| output.join(name).exists()) {
        return Err(
            "output contains a fixture name; use a fresh directory (nothing overwritten)".into(),
        );
    }
    let started = Instant::now();
    let mut files = Vec::new();
    files.push(fresh(&output, "good-mm.dxf", GOOD_DXF.as_bytes())?);
    files.push(fresh(
        &output,
        "critical-unsupported-mm.dxf",
        CRITICAL_DXF.as_bytes(),
    )?);
    files.push(fresh(
        &output,
        "missing-units.dxf",
        MISSING_UNITS_DXF.as_bytes(),
    )?);
    let mixed = mixed(&output, &mut files)?;
    let one_thousand = performance(&output, 1000, &mut files)?;
    let ten_thousand = performance(&output, 10000, &mut files)?;
    let lab = lab_migration(&output, &mut files)?;
    let executable = std::env::current_exe()?;
    let manifest = json!({"schema":"ai-revit-v05-generated-fixtures-v1","created_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),"output_root":output,
        "generator":{"package_version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"executable_path":executable,"executable_disk_sha256":hash(&fs::read(&executable)?),"compiled_example_source_sha256":hash(include_bytes!("v05_fixtures.rs")),"source_scope":"example source only; root integration source manifest binds the Core crates","loaded_image_identity":"NOT_MEASURED"},
        "scope":{"native_gui":"NOT_RUN","actual_old_reader":"NOT_RUN","DXF_adapter_for_authored_ASCII_inputs":"NOT_RUN","gpu":"NOT_RUN","timings":"actual headless Core/serialization/filesystem durations; not native action-to-frame, FPS or GPU time"},
        "mixed":mixed,"performance":{"1000":one_thousand,"10000":ten_thousand},"lab_migration":lab,"files":files,"total_generation_wall_us_before_manifest_write":micros(started),"manifest_hash_scope":"files array excludes this manifest itself"});
    let record = json_file(&output, "generation-manifest.json", &manifest)?;
    println!(
        "{}",
        serde_json::to_string(&json!({"manifest":record,"native_acceptance":"NOT_RUN"}))?
    );
    Ok(())
}
