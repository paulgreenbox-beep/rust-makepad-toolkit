#![recursion_limit = "256"]
//! Ordinary mixed drawings through the same public document/display paths as UI.
//! Run once from the workspace root to write fresh evidence and fixture documents:
//! cargo run -p drafting_core --offline -j2 --example core_baseline -- evidence/runtime/core-baseline.json evidence/runtime/fixtures
use drafting_core::*;
use serde_json::{Value, json};
use std::{
    f64::consts::PI,
    path::{Path, PathBuf},
    time::Instant,
};

fn mixed_geometry(i: usize, columns: usize) -> EntityGeometry {
    let x = (i % columns) as f64;
    let y = (i / columns) as f64;
    match i % 4 {
        0 => EntityGeometry::Line {
            start: Point2::new(x + 0.1, y + 0.1),
            end: Point2::new(x + 0.9, y + 0.8),
        },
        1 => EntityGeometry::Circle {
            center: Point2::new(x + 0.5, y + 0.5),
            radius: 0.3,
        },
        2 => EntityGeometry::Arc {
            center: Point2::new(x + 0.5, y + 0.5),
            radius: 0.35,
            start_angle: 0.2,
            sweep_angle: PI * 1.6,
        },
        _ => EntityGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(Point2::new(x + 0.1, y + 0.1), 0.4),
                PolylineVertex::new(Point2::new(x + 0.8, y + 0.1), -0.3),
                PolylineVertex::new(Point2::new(x + 0.8, y + 0.8), 0.),
                PolylineVertex::new(Point2::new(x + 0.1, y + 0.8), 0.5),
            ],
            closed: true,
        },
    }
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    json!({"samples":n,"mean_ms":samples.iter().sum::<f64>()/n as f64,"p50_ms":samples[n/2],"p95_ms":samples[((n as f64*0.95).ceil() as usize).saturating_sub(1)],"max_ms":samples[n-1]})
}
fn measure(count: usize, fixture_dir: Option<&Path>) -> DraftResult<Value> {
    let columns = (count as f64).sqrt().ceil() as usize;
    let polyline_count = count.saturating_sub(3).div_ceil(4);
    let counts_by_type = json!({
        "line": count.div_ceil(4),
        "circle": count.saturating_sub(1).div_ceil(4),
        "arc": count.saturating_sub(2).div_ceil(4),
        "polyline": polyline_count,
    });
    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    let operations = (0..count)
        .map(|i| DraftOperation::Create {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            geometry: mixed_geometry(i, columns),
            style: DraftStyle::default(),
        })
        .collect();
    let mut doc = LabDocument::new();
    let start = Instant::now();
    doc.apply(CommandEnvelope {
        command_id: format!("baseline-{count}"),
        expected_revision: 0,
        operations,
    })?;
    let create_ms = ms(start);
    let start = Instant::now();
    let serialized = doc.to_json()?;
    let json_validate_serialize_ms = ms(start);
    let start = Instant::now();
    let opened = LabDocument::from_json(&serialized)?;
    let json_parse_validate_ms = ms(start);
    if opened != doc {
        return Err(DraftError::new(
            "baseline_mismatch",
            "JSON reopen differs from source document",
        ));
    }
    let path = std::env::temp_dir().join(format!(
        "drafting-baseline-{count}-{}.json",
        uuid::Uuid::new_v4()
    ));
    let start = Instant::now();
    doc.save_json(&path)?;
    let save_json_atomic_sync_ms = ms(start);
    let start = Instant::now();
    let file_opened = LabDocument::load_json(&path)?;
    let reopen_file_read_validate_ms = ms(start);
    std::fs::remove_file(&path).map_err(|e| DraftError::new("baseline_file", e.to_string()))?;
    if file_opened != doc {
        return Err(DraftError::new(
            "baseline_mismatch",
            "file reopen differs from source document",
        ));
    }
    let start = Instant::now();
    let snapshot = doc.snapshot(DrawingId::new(1))?;
    let snapshot_ms = ms(start);
    let mut camera = Camera2D::new(1200., 800.);
    camera.fit_bounds(snapshot.bounds(true)?.unwrap(), 24.)?;
    let options = DisplayOptions::default();
    let start = Instant::now();
    let display = build_display(&snapshot, &camera, &options)?;
    let display_ms = ms(start);
    let start = Instant::now();
    let mut tessellated_segments = 0usize;
    for entity in snapshot.entities.values() {
        tessellated_segments += tessellate_geometry(&entity.geometry, &camera, &options)?.len();
    }
    let tessellation_ms = ms(start);
    let start = Instant::now();
    let index = SpatialIndex2D::build(&snapshot)?;
    let index_build_ms = ms(start);
    let mut indexed_pick_query = Vec::new();
    let mut public_pick_with_index_build = Vec::new();
    let mut indexed_pick_hit_count = 0usize;
    let mut public_pick_hit_count = 0usize;
    let mut pick_results_match = true;
    for i in 0..100 {
        let entity = snapshot.entities.values().nth((i * 97) % count).unwrap();
        let point = match &entity.geometry {
            EntityGeometry::Line { start, .. } => *start,
            EntityGeometry::Circle { center, radius } => Point2::new(center.x + radius, center.y),
            EntityGeometry::Arc {
                center,
                radius,
                start_angle,
                ..
            } => Point2::new(
                center.x + radius * start_angle.cos(),
                center.y + radius * start_angle.sin(),
            ),
            EntityGeometry::Polyline { vertices, .. } => vertices[0].point,
        };
        let screen = camera.world_to_screen(point);
        let start = Instant::now();
        let indexed_hits = pick_with_index(&snapshot, &index, &camera, screen, 7.)?;
        indexed_pick_query.push(ms(start));
        let start = Instant::now();
        let public_hits = pick(&snapshot, &camera, screen, 7.)?;
        public_pick_with_index_build.push(ms(start));
        indexed_pick_hit_count += indexed_hits.len();
        public_pick_hit_count += public_hits.len();
        pick_results_match &= indexed_hits.as_slice() == public_hits.as_slice();
    }
    if !pick_results_match {
        return Err(DraftError::new(
            "baseline_pick_mismatch",
            "public pick results differ from the prebuilt-index path",
        ));
    }
    let rev = doc.revision();
    let history = doc.history_len();
    let mut pan_zoom = Vec::new();
    for i in 0..20 {
        let start = Instant::now();
        camera.pan_pixels(if i % 2 == 0 { 2. } else { -2. }, 1.)?;
        camera.zoom_at(
            Point2::new(500., 350.),
            if i % 2 == 0 { 1.01 } else { 1. / 1.01 },
        )?;
        let _ = build_display(&snapshot, &camera, &options)?;
        pan_zoom.push(ms(start));
    }
    let revision_unchanged = rev == doc.revision();
    let history_unchanged = history == doc.history_len();
    let snapshot_fingerprint_unchanged =
        snapshot.source_fingerprint == doc.snapshot(DrawingId::new(1))?.source_fingerprint;
    let document_json_unchanged = doc.to_json()? == serialized;
    let authority_unchanged = revision_unchanged
        && history_unchanged
        && snapshot_fingerprint_unchanged
        && document_json_unchanged;
    if !authority_unchanged {
        return Err(DraftError::new(
            "baseline_mutation",
            "camera modified authoritative geometry or history",
        ));
    }
    let fixture_path = fixture_dir.map(|dir| dir.join(format!("mixed-{count}.lab.json")));
    if let Some(path) = &fixture_path {
        // Preserve the same normal authoritative document outside timed sections.
        doc.save_json(path)?;
    }
    Ok(
        json!({
            "entities": count,
            "counts_by_type": counts_by_type,
            "profile": profile,
            "polyline_bulges": {
                "closed_polylines": polyline_count,
                "positive_bulge_segments": polyline_count * 2,
                "negative_bulge_segments": polyline_count,
                "straight_segments": polyline_count
            },
            "create_atomic_batch_ms": create_ms,
            "core_json_persistence_export_ms": json_validate_serialize_ms,
            "core_json_persistence_export_scope": "LabDocument::to_json validation and whole-document JSON serialization, including analytic geometry and document IDs/history/allocators",
            "document_json_parse_validate_ms": json_parse_validate_ms,
            "document_json_round_trip_matches": opened == doc,
            "save_json_atomic_sync_ms": save_json_atomic_sync_ms,
            "save_json_timing_scope": "document validation and serialization, temporary-file write, sync_all, and rename",
            "reopen_file_read_parse_validate_ms": reopen_file_read_validate_ms,
            "file_document_round_trip_matches": file_opened == doc,
            "document_json_bytes": serialized.len(),
            "snapshot_clone_fingerprint_ms": snapshot_ms,
            "build_display_with_tessellation_ms": display_ms,
            "display_primitive_count": display.primitives.len(),
            "standalone_tessellation_pass_ms": tessellation_ms,
            "standalone_tessellated_segment_count": tessellated_segments,
            "spatial_index_build_ms": index_build_ms,
            "pick_query_count": 100,
            "prebuilt_index_pick_timing_scope": "pick_with_index analytic query; index build is measured separately",
            "prebuilt_index_analytic_pick_query_ms": distribution(indexed_pick_query),
            "public_pick_timing_scope": "pick rebuilds SpatialIndex2D per call, then runs the indexed analytic query",
            "public_pick_with_index_build_per_call_ms": distribution(public_pick_with_index_build),
            "prebuilt_and_public_pick_results_match": pick_results_match,
            "prebuilt_index_pick_hit_count_sum": indexed_pick_hit_count,
            "public_pick_hit_count_sum": public_pick_hit_count,
            "cpu_camera_pan_zoom_and_display_build_ms": distribution(pan_zoom),
            "camera_operation_pairs": 20,
            "authority_revision_unchanged": revision_unchanged,
            "authority_history_unchanged": history_unchanged,
            "authority_snapshot_fingerprint_unchanged": snapshot_fingerprint_unchanged,
            "authority_document_json_unchanged": document_json_unchanged,
            "authority_unchanged": authority_unchanged,
            "fixture_document_path": fixture_path,
            "external_export_ms": null,
            "external_export_status": "SVG/DXF/PNG are outside drafting_core and measured separately by the owner",
            "gpu_upload_ms": null,
            "native_frame_time_ms": null,
            "process_memory_bytes": null,
            "process_memory_status": "unavailable_without_platform_specific_measurement",
            "limitations": "CPU core timings only; document JSON persistence is not an external export; GPU upload, native frame time, process memory, and exporter timing are not measured by this example"
        }),
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_dir = std::env::args_os().nth(2).map(PathBuf::from);
    if let Some(dir) = &fixture_dir {
        std::fs::create_dir_all(dir)?;
    }
    let result = json!({"benchmark":"drafting_core_baseline_v01","process_id":std::process::id(),"fixtures":[measure(1000, fixture_dir.as_deref())?,measure(10000, fixture_dir.as_deref())?]});
    let text = serde_json::to_string_pretty(&result)?;
    if let Some(path) = std::env::args_os().nth(1) {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &text)?;
    }
    println!("{text}");
    Ok(())
}
