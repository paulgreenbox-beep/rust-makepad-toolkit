use ai_revit_core::{Command, Document, ElementId, ElementKind, OpeningSpec, Point2, WallSpec};
use ai_revit_geometry::{GeometryCache, RebuildReport, SectionPlane};
use serde_json::{Value, json};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use viewer_core::Vec3;

const WALL_COUNT: usize = 1_000;
const OPENING_COUNT: usize = 100;
const GRID_COLUMNS: usize = 40;
const WALL_LENGTH_M: f64 = 6.0;
const WALL_HEIGHT_M: f64 = 3.0;
const WALL_THICKNESS_M: f64 = 0.2;

fn main() -> Result<(), Box<dyn Error>> {
    let output_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                "usage: benchmark_1000 <output.json>",
            )
        })?;

    let mut document = Document::new("Synthetic 1000 wall geometry benchmark")?;
    let mut create = document.begin_transaction("benchmark_create_1000", document.revision());

    for index in 0..WALL_COUNT {
        let (start, end) = wall_endpoints(index);
        create.push(Command::CreateElement {
            name: format!("Benchmark Wall {}", index + 1),
            class_id: ai_revit_core::BUILTIN_WALL_CLASS_ID.into(),
            kind: ElementKind::Wall(WallSpec {
                start,
                end,
                base_elevation_m: 0.0,
                height_m: WALL_HEIGHT_M,
                thickness_m: WALL_THICKNESS_M,
            }),
            properties: Default::default(),
        });
    }

    for index in 0..OPENING_COUNT {
        let host_wall_index = index * (WALL_COUNT / OPENING_COUNT);
        let (start, _) = wall_endpoints(host_wall_index);
        create.push(Command::CreateOpening {
            name: format!("Benchmark Opening {}", index + 1),
            spec: OpeningSpec {
                center: Point2::new(start.x + WALL_LENGTH_M / 2.0, start.y),
                elevation_m: 0.75,
                width_m: 1.2,
                height_m: 1.5,
                depth_m: WALL_THICKNESS_M,
                rotation_rad: 0.0,
            },
            properties: Default::default(),
            host_id: element_id_for_wall(host_wall_index),
            type_id: None,
        });
    }

    let create_started = Instant::now();
    let create_receipt = document.commit(create)?;
    let create_elapsed = create_started.elapsed();

    let mut cache = GeometryCache::new();
    let initial_rebuild_started = Instant::now();
    let initial_rebuild = cache.rebuild(&document);
    let initial_rebuild_elapsed = initial_rebuild_started.elapsed();
    let history_after_create = document.history_metrics()?;

    let host_wall_id = element_id_for_wall(0);
    let host_before = cache
        .record(host_wall_id)
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                "initial host geometry is unavailable",
            )
        })?
        .clone();
    let opening_id = ElementId::new((WALL_COUNT + 1) as u64);
    let existing_opening = document
        .element(opening_id)
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "benchmark opening was not created"))?;
    let ElementKind::Opening(mut updated_spec) = existing_opening.kind.clone() else {
        return Err(io::Error::new(ErrorKind::InvalidData, "expected an Opening element").into());
    };
    updated_spec.width_m = 1.5;
    updated_spec.center.x += 0.25;
    let updated_center_x = updated_spec.center.x;

    let mut update = document.begin_transaction("benchmark_update_opening", document.revision());
    update.push(Command::UpdateElement {
        element_id: opening_id,
        kind: ElementKind::Opening(updated_spec),
    });
    let update_started = Instant::now();
    let update_receipt = document.commit(update)?;
    let update_elapsed = update_started.elapsed();

    let update_rebuild_started = Instant::now();
    let update_rebuild = cache.rebuild(&document);
    let update_rebuild_elapsed = update_rebuild_started.elapsed();
    let history_after_update = document.history_metrics()?;
    if update_rebuild.rebuilt_element_ids != vec![host_wall_id, opening_id]
        || !cache
            .record(host_wall_id)
            .is_some_and(|record| record.valid)
        || !cache.record(opening_id).is_some_and(|record| record.valid)
    {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "one Opening update must rebuild only its valid host and Opening geometry",
        )
        .into());
    }

    let host_after = cache.record(host_wall_id).expect("validated host record");
    let comparison = cache.compare(
        host_wall_id,
        host_before.geometry_revision,
        host_after.geometry_revision,
    )?;
    let section = cache.section(
        &[host_wall_id],
        SectionPlane {
            origin: Vec3::ZERO,
            normal: Vec3::new(0.0, 1.0, 0.0),
        },
    )?;

    let output = json!({
        "benchmark": "synthetic_1000_walls_with_hosted_openings",
        "fixture": {
            "wall_count": WALL_COUNT,
            "opening_count": OPENING_COUNT,
            "opening_host_count": OPENING_COUNT,
            "opening_depth_m": WALL_THICKNESS_M,
            "wall_thickness_m": WALL_THICKNESS_M,
            "atomic_create_batch": true,
        },
        "create_transaction": {
            "elapsed_ms": elapsed_ms(create_elapsed),
            "transaction_duration_us": create_receipt.transaction_duration_us,
            "dirty_propagation_duration_us": create_receipt.dirty_propagation_duration_us,
            "scanned_relationship_count": create_receipt.scanned_relationship_count,
            "document_scan_count": create_receipt.document_scan_count,
            "snapshot_serialization_duration_us": create_receipt.snapshot_serialization_duration_us,
            "snapshot_serialized_bytes": create_receipt.snapshot_serialized_bytes,
            "created_element_count": create_receipt.created_elements.len(),
            "affected_id_count": create_receipt.affected_ids.len(),
            "dirty_id_count": create_receipt.dirty_ids.len(),
            "dirty_ids": element_ids(&create_receipt.dirty_ids),
        },
        "initial_geometry_rebuild": rebuild_json(&initial_rebuild, initial_rebuild_elapsed),
        "opening_update": {
            "element_id": opening_id.get(),
            "host_id": host_wall_id.get(),
            "new_width_m": 1.5,
            "new_center_x_m": updated_center_x,
            "elapsed_ms": elapsed_ms(update_elapsed),
            "transaction_duration_us": update_receipt.transaction_duration_us,
            "dirty_propagation_duration_us": update_receipt.dirty_propagation_duration_us,
            "scanned_relationship_count": update_receipt.scanned_relationship_count,
            "document_scan_count": update_receipt.document_scan_count,
            "snapshot_serialization_duration_us": update_receipt.snapshot_serialization_duration_us,
            "snapshot_serialized_bytes": update_receipt.snapshot_serialized_bytes,
            "affected_id_count": update_receipt.affected_ids.len(),
            "dirty_id_count": update_receipt.dirty_ids.len(),
            "dirty_ids": element_ids(&update_receipt.dirty_ids),
        },
        "updated_geometry_rebuild": rebuild_json(&update_rebuild, update_rebuild_elapsed),
        "geometry_history_record_count": cache.geometry_history_entry_count(),
        "geometry_records": {
            "host_wall": cache.record(host_wall_id),
            "updated_opening": cache.record(opening_id),
        },
        "geometry_observation_examples": {
            "inspect_before": host_before,
            "inspect_after": host_after,
            "dependencies": cache.dependencies(host_wall_id),
            "compare": comparison,
            "section": section,
        },
        "document_history_after_create": history_json(&history_after_create),
        "document_history_after_update": history_json(&history_after_update),
    });

    if let Some(parent) = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let output_file = File::create(&output_path)?;
    serde_json::to_writer_pretty(BufWriter::new(output_file), &output)?;
    println!("Wrote benchmark report to {}", output_path.display());
    Ok(())
}

fn wall_endpoints(index: usize) -> (Point2, Point2) {
    let column = (index % GRID_COLUMNS) as f64;
    let row = (index / GRID_COLUMNS) as f64;
    let start = Point2::new(column * 8.0, row * 6.0);
    let end = Point2::new(start.x + WALL_LENGTH_M, start.y);
    (start, end)
}

fn element_id_for_wall(index: usize) -> ElementId {
    ElementId::new((index + 1) as u64)
}

fn element_ids(ids: &[ElementId]) -> Vec<u64> {
    ids.iter().map(|id| id.get()).collect()
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn rebuild_json(report: &RebuildReport, elapsed: Duration) -> Value {
    json!({
        "elapsed_ms": elapsed_ms(elapsed),
        "reported_rebuild_duration_ms": report.rebuild_duration_ms,
        "rebuilt_element_count": report.rebuilt_element_count,
        "rebuilt_element_ids": element_ids(&report.rebuilt_element_ids),
        "removed_element_count": report.removed_element_count,
        "removed_element_ids": element_ids(&report.removed_element_ids),
        "document_invalidated": report.document_invalidated,
        "total_document_scan_count": report.total_document_scan_count,
        "scanned_element_count": report.scanned_element_count,
        "fingerprint_comparison_count": report.fingerprint_comparison_count,
        "hosted_lookup_count": report.hosted_lookup_count,
    })
}

fn history_json(metrics: &ai_revit_core::HistoryMetrics) -> Value {
    json!({
        "history_entry_count": metrics.history_entry_count,
        "history_serialized_bytes": metrics.history_serialized_bytes,
        "document_serialized_bytes": metrics.document_serialized_bytes,
    })
}
