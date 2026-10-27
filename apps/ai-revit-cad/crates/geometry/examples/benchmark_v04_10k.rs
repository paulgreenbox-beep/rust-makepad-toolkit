use ai_revit_core::{
    Command, Document, ElementId, ElementKind, HistoryMetrics, OpeningSpec, Point2, WallSpec,
    BUILTIN_WALL_CLASS_ID,
};
use ai_revit_geometry::{GeometryCache, QuantityGrouping, RebuildReport, SpatialQuery};
use serde_json::{json, Value};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use viewer_core::{Aabb, Vec3};

const WALL_COUNT: usize = 9_000;
const OPENING_COUNT: usize = 1_000;
const ELEMENT_COUNT: usize = WALL_COUNT + OPENING_COUNT;
const GRID_COLUMNS: usize = 100;
const WALL_LENGTH_M: f64 = 6.0;
const WALL_HEIGHT_M: f64 = 3.0;
const WALL_THICKNESS_M: f64 = 0.2;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let native_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/v04/geometry-10k-document.json"));
    let report_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/v04/geometry-10k-benchmark.json"));
    if args.next().is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "usage: benchmark_v04_10k [native-document.json] [benchmark-report.json]",
        )
        .into());
    }
    create_parent(&native_path)?;
    create_parent(&report_path)?;

    let mut document = Document::new("Synthetic v0.4 10,000 element geometry benchmark")?;
    let mut create = document.begin_transaction("benchmark_create_10000", document.revision());
    for index in 0..WALL_COUNT {
        let (start, end) = wall_endpoints(index);
        create.push(Command::CreateElement {
            name: format!("Benchmark Wall {}", index + 1),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
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
    if create_receipt.created_elements.len() != ELEMENT_COUNT {
        return Err(invalid_data(format!(
            "creation returned {} semantic elements, expected {ELEMENT_COUNT}",
            create_receipt.created_elements.len()
        ))
        .into());
    }

    let count_started = Instant::now();
    let mut observed_wall_count = 0;
    let mut observed_opening_count = 0;
    for element in document.elements() {
        match &element.kind {
            ElementKind::Wall(_) => observed_wall_count += 1,
            ElementKind::Opening(_) => observed_opening_count += 1,
            _ => {
                return Err(invalid_data(format!(
                    "unexpected element kind in this fixture at element {}",
                    element.id.get()
                ))
                .into());
            }
        }
    }
    let semantic_count_verification_elapsed = count_started.elapsed();
    if observed_wall_count != WALL_COUNT || observed_opening_count != OPENING_COUNT {
        return Err(invalid_data(format!(
            "fixture kind counts were {observed_wall_count} Walls and {observed_opening_count} Openings"
        ))
        .into());
    }

    let save_started = Instant::now();
    document.save(&native_path)?;
    let save_elapsed = save_started.elapsed();
    let native_file_bytes = fs::metadata(&native_path)?.len();

    let open_started = Instant::now();
    let mut opened_document = Document::open(&native_path)?;
    let open_elapsed = open_started.elapsed();
    if opened_document.revision() != document.revision()
        || opened_document.document_uuid != document.document_uuid
        || opened_document.elements().len() != ELEMENT_COUNT
    {
        return Err(invalid_data(
            "native Core document open changed identity, revision, or element count",
        )
        .into());
    }

    let index_verify_started = Instant::now();
    opened_document.verify_index_consistency()?;
    let index_verify_elapsed = index_verify_started.elapsed();
    let index_metrics = opened_document.index_metrics();
    if index_metrics.element_by_id != ELEMENT_COUNT
        || index_metrics.host_of_element != OPENING_COUNT
    {
        return Err(invalid_data(format!(
            "opened runtime indexes report {} elements and {} hosted children",
            index_metrics.element_by_id, index_metrics.host_of_element
        ))
        .into());
    }

    let equivalence_started = Instant::now();
    let source_native_json = document.to_json_string()?;
    let opened_native_json = opened_document.to_json_string()?;
    let canonical_native_json_equal = source_native_json == opened_native_json;
    let canonical_native_json_bytes = source_native_json.len();
    drop(source_native_json);
    drop(opened_native_json);
    let equivalence_elapsed = equivalence_started.elapsed();
    if !canonical_native_json_equal {
        return Err(invalid_data(
            "native save/open round-trip changed canonical Core document serialization",
        )
        .into());
    }

    let initial_history_started = Instant::now();
    let initial_history_metrics = opened_document.history_metrics()?;
    let initial_history_metrics_elapsed = initial_history_started.elapsed();

    let mut cache = GeometryCache::new();
    let initial_rebuild_started = Instant::now();
    let initial_rebuild = cache.rebuild(&opened_document);
    let initial_rebuild_elapsed = initial_rebuild_started.elapsed();
    if initial_rebuild.rebuilt_element_count != ELEMENT_COUNT
        || initial_rebuild.scanned_element_count != ELEMENT_COUNT
        || initial_rebuild.total_document_scan_count != 1
    {
        return Err(invalid_data(format!(
            "initial GeometryCache derivation rebuilt {} of {ELEMENT_COUNT} elements ({} scanned)",
            initial_rebuild.rebuilt_element_count, initial_rebuild.scanned_element_count
        ))
        .into());
    }

    let geometry_validity_started = Instant::now();
    let mut valid_geometry_count = 0;
    for raw_id in 1..=ELEMENT_COUNT as u64 {
        if cache
            .record(ElementId::new(raw_id))
            .is_some_and(|record| record.valid)
        {
            valid_geometry_count += 1;
        }
    }
    let geometry_validity_elapsed = geometry_validity_started.elapsed();
    if valid_geometry_count != ELEMENT_COUNT {
        return Err(invalid_data(format!(
            "GeometryCache produced valid records for {valid_geometry_count} of {ELEMENT_COUNT} semantic elements"
        ))
        .into());
    }

    let first_host_id = element_id_for_wall(0);
    let first_opening_id = element_id_for_opening(0);
    let host_before = cache
        .record(first_host_id)
        .ok_or_else(|| invalid_data("first host geometry record is missing"))?
        .clone();
    let opening_before = cache
        .record(first_opening_id)
        .ok_or_else(|| invalid_data("first opening geometry record is missing"))?
        .clone();

    let quantity_ids = (1..=ELEMENT_COUNT as u64)
        .map(ElementId::new)
        .collect::<Vec<_>>();
    let quantity_batch_started = Instant::now();
    let quantity_batch = cache.quantities_batch(&quantity_ids);
    let quantity_batch_elapsed = quantity_batch_started.elapsed();
    if quantity_batch.elements.len() != ELEMENT_COUNT
        || !quantity_batch.missing_element_ids.is_empty()
        || quantity_batch.element_ids.len() != ELEMENT_COUNT
    {
        return Err(invalid_data(format!(
            "indexed quantity batch returned {} of {ELEMENT_COUNT} elements and {} missing IDs",
            quantity_batch.elements.len(),
            quantity_batch.missing_element_ids.len()
        ))
        .into());
    }
    let quantity_cache_metrics_before_update = cache.quantity_cache_metrics();
    if quantity_cache_metrics_before_update.entry_count != ELEMENT_COUNT {
        return Err(invalid_data(format!(
            "quantity cache contains {} entries, expected {ELEMENT_COUNT}",
            quantity_cache_metrics_before_update.entry_count
        ))
        .into());
    }

    let grouped_started = Instant::now();
    let class_groups = cache.quantities_grouped(QuantityGrouping::Class);
    let grouped_elapsed = grouped_started.elapsed();
    if class_groups.iter().map(|group| group.count).sum::<usize>() != ELEMENT_COUNT {
        return Err(
            invalid_data("class quantity groups do not cover all semantic elements").into(),
        );
    }

    let spatial_query = SpatialQuery {
        bounds: Aabb {
            min: Vec3::new(-0.5, -0.5, -0.5),
            max: Vec3::new(WALL_LENGTH_M + 0.5, 0.5, WALL_HEIGHT_M + 0.5),
        },
        class_id: None,
        level_id: None,
    };
    let spatial_query_started = Instant::now();
    let spatial_result = cache.spatial_bounds_query(&spatial_query)?;
    let spatial_query_elapsed = spatial_query_started.elapsed();
    let spatial_metrics_after_query = cache.spatial_index_metrics();
    if !spatial_result.element_ids.contains(&first_host_id)
        || !spatial_result.element_ids.contains(&first_opening_id)
        || spatial_metrics_after_query.element_count != ELEMENT_COUNT
        || spatial_metrics_after_query.node_count == 0
        || spatial_result.visited_nodes.saturating_mul(100)
            >= spatial_metrics_after_query.node_count
    {
        return Err(invalid_data(format!(
            "local bounds query did not find the first host/opening pair with bounded traversal ({} visited / {} nodes)",
            spatial_result.visited_nodes, spatial_metrics_after_query.node_count
        ))
        .into());
    }

    let host_opening_index_verification_started = Instant::now();
    let all_openings_have_expected_hosts = (0..OPENING_COUNT).all(|index| {
        let host_index = index * (WALL_COUNT / OPENING_COUNT);
        opened_document.host_of(element_id_for_opening(index))
            == Some(element_id_for_wall(host_index))
    });
    let host_opening_index_verification_elapsed = host_opening_index_verification_started.elapsed();
    if !all_openings_have_expected_hosts {
        return Err(invalid_data(
            "one or more opening-to-host relationships did not survive native open",
        )
        .into());
    }

    let existing_opening = opened_document
        .element(first_opening_id)
        .ok_or_else(|| invalid_data("first Opening semantic element is missing"))?;
    let ElementKind::Opening(mut updated_spec) = existing_opening.kind.clone() else {
        return Err(invalid_data("first opening ID does not refer to an Opening").into());
    };
    updated_spec.width_m = 1.5;
    updated_spec.center.x += 0.25;
    let updated_center_x = updated_spec.center.x;
    let update_revision_before = opened_document.revision();
    let mut update =
        opened_document.begin_transaction("benchmark_update_one_opening", update_revision_before);
    update.push(Command::UpdateElement {
        element_id: first_opening_id,
        kind: ElementKind::Opening(updated_spec),
    });
    let update_started = Instant::now();
    let update_receipt = opened_document.commit(update)?;
    let update_elapsed = update_started.elapsed();

    let update_rebuild_started = Instant::now();
    let update_rebuild = cache.rebuild(&opened_document);
    let update_rebuild_elapsed = update_rebuild_started.elapsed();
    let expected_rebuilt_ids = vec![first_host_id, first_opening_id];
    if update_rebuild.rebuilt_element_ids != expected_rebuilt_ids
        || update_rebuild.total_document_scan_count != 0
        || update_rebuild.scanned_element_count != 0
        || update_receipt.performance.core_scan_count != 0
        || update_rebuild.rebuilt_element_count != 2
    {
        return Err(invalid_data(format!(
            "one Opening edit was not bounded: Core scans {}, GeometryCache rebuilt {} elements / scanned {}",
            update_receipt.performance.core_scan_count,
            update_rebuild.rebuilt_element_count,
            update_rebuild.scanned_element_count
        ))
        .into());
    }
    let host_after = cache
        .record(first_host_id)
        .ok_or_else(|| invalid_data("updated host geometry record is missing"))?
        .clone();
    let opening_after = cache
        .record(first_opening_id)
        .ok_or_else(|| invalid_data("updated opening geometry record is missing"))?
        .clone();
    let geometry_comparison = cache.compare(
        first_host_id,
        host_before.geometry_revision,
        host_after.geometry_revision,
    )?;
    let quantity_cache_metrics_after_update = cache.quantity_cache_metrics();
    let spatial_metrics_after_update = cache.spatial_index_metrics();

    let history_diff_started = Instant::now();
    let history_diff =
        opened_document.history_diff(update_revision_before, opened_document.revision())?;
    let history_diff_elapsed = history_diff_started.elapsed();
    let history_diff_bytes = serde_json::to_vec(&history_diff)?.len();
    let history_delta_started = Instant::now();
    let history_delta = opened_document.history_delta(opened_document.revision())?;
    let history_delta_elapsed = history_delta_started.elapsed();
    let history_delta_bytes = serde_json::to_vec(&history_delta)?.len();
    if history_diff.modified != vec![first_opening_id]
        || history_delta.updated_elements != vec![first_opening_id]
    {
        return Err(
            invalid_data("local history diff/delta did not identify the edited Opening").into(),
        );
    }

    let post_update_history_started = Instant::now();
    let post_update_history_metrics = opened_document.history_metrics()?;
    let post_update_history_metrics_elapsed = post_update_history_started.elapsed();

    let core_scans_after_open_and_observation = opened_document.scan_metrics();
    let backend_capabilities = cache.backend_capabilities();
    let quantity_group_summaries = class_groups
        .iter()
        .map(|group| {
            json!({
                "class_id": group.class_id,
                "count": group.count,
                "aggregate": group.quantities,
            })
        })
        .collect::<Vec<_>>();

    let output = json!({
        "benchmark": "synthetic_v04_10000_semantic_elements",
        "scope": {
            "geometry_execution": "pure Core GeometryCache and GeometryBackend over a native Core Document",
            "representation_quality": "approximate_mesh",
            "native_document_equivalent": true,
            "native_document_equivalence_method": "canonical Core Document JSON serialization matched byte for byte after Document::save and Document::open",
            "native_application_api_10k_acceptance": "separate milestone; this benchmark does not claim Desktop/API acceptance",
            "fixture_specific_production_fast_path": false,
        },
        "fixture": {
            "semantic_element_count": ELEMENT_COUNT,
            "wall_count": observed_wall_count,
            "opening_count": observed_opening_count,
            "hosted_opening_count": OPENING_COUNT,
            "types_counted_as_elements": false,
            "atomic_create_transaction": true,
            "wall_length_m": WALL_LENGTH_M,
            "wall_height_m": WALL_HEIGHT_M,
            "wall_thickness_m": WALL_THICKNESS_M,
            "grid_columns": GRID_COLUMNS,
            "grid_rows": WALL_COUNT / GRID_COLUMNS,
            "wall_spacing_x_m": 8.0,
            "wall_spacing_y_m": 6.0,
        },
        "creation": {
            "elapsed_ms": elapsed_ms(create_elapsed),
            "elapsed_us": elapsed_us(create_elapsed),
            "transaction_duration_us": create_receipt.transaction_duration_us,
            "dirty_propagation_duration_us": create_receipt.dirty_propagation_duration_us,
            "scanned_relationship_count": create_receipt.scanned_relationship_count,
            "document_scan_count": create_receipt.document_scan_count,
            "snapshot_serialization_duration_us": create_receipt.snapshot_serialization_duration_us,
            "snapshot_serialized_bytes": create_receipt.snapshot_serialized_bytes,
            "created_element_count": create_receipt.created_elements.len(),
            "created_relationship_count": create_receipt.created_relationships.len(),
            "affected_id_count": create_receipt.affected_ids.len(),
            "dirty_id_count": create_receipt.dirty_ids.len(),
            "core_performance": create_receipt.performance,
            "semantic_count_verification_ms": elapsed_ms(semantic_count_verification_elapsed),
        },
        "native_document_round_trip": {
            "path": native_path,
            "file_bytes": native_file_bytes,
            "save_ms": elapsed_ms(save_elapsed),
            "save_us": elapsed_us(save_elapsed),
            "open_ms": elapsed_ms(open_elapsed),
            "open_us": elapsed_us(open_elapsed),
            "canonical_json_equal": canonical_native_json_equal,
            "canonical_json_bytes": canonical_native_json_bytes,
            "canonical_comparison_ms": elapsed_ms(equivalence_elapsed),
            "index_consistency_verified": true,
            "index_consistency_ms": elapsed_ms(index_verify_elapsed),
            "opening_host_index_verified_count": OPENING_COUNT,
            "opening_host_index_verification_ms": elapsed_ms(host_opening_index_verification_elapsed),
            "index_metrics": index_metrics,
        },
        "core_document_observation": {
            "revision_after_open_before_update": update_revision_before,
            "semantic_element_count": index_metrics.element_by_id,
            "index_metrics": index_metrics,
            "scan_metrics_after_observation": core_scans_after_open_and_observation,
            "history_metrics_before_local_update": history_metrics_json(&initial_history_metrics),
            "history_metrics_elapsed_ms": elapsed_ms(initial_history_metrics_elapsed),
            "history_metrics_after_local_update": history_metrics_json(&post_update_history_metrics),
            "history_metrics_after_update_elapsed_ms": elapsed_ms(post_update_history_metrics_elapsed),
        },
        "initial_geometry_rederive": {
            "rebuild": rebuild_json(&initial_rebuild, initial_rebuild_elapsed),
            "valid_geometry_record_count": valid_geometry_count,
            "validity_verification_ms": elapsed_ms(geometry_validity_elapsed),
            "backend_capabilities": backend_capabilities,
            "sample_host_record": host_before,
            "sample_opening_record": opening_before,
        },
        "quantity_observations": {
            "all_element_batch": {
                "elapsed_ms": elapsed_ms(quantity_batch_elapsed),
                "elapsed_us": elapsed_us(quantity_batch_elapsed),
                "requested_id_count": quantity_ids.len(),
                "returned_element_count": quantity_batch.elements.len(),
                "missing_element_ids": quantity_batch.missing_element_ids,
                "aggregate": quantity_batch.aggregate,
                "sample_first_wall": quantity_batch.elements.first(),
                "sample_last_opening": quantity_batch.elements.last(),
                "quantity_cache_metrics_before_update": quantity_cache_metrics_before_update,
            },
            "class_grouped_aggregates": {
                "elapsed_ms": elapsed_ms(grouped_elapsed),
                "group_count": quantity_group_summaries.len(),
                "groups": quantity_group_summaries,
            },
        },
        "spatial_bounds_query": {
            "elapsed_ms": elapsed_ms(spatial_query_elapsed),
            "elapsed_us": elapsed_us(spatial_query_elapsed),
            "indexed_element_count": spatial_metrics_after_query.element_count,
            "node_count": spatial_metrics_after_query.node_count,
            "returned_element_ids": element_ids(&spatial_result.element_ids),
            "returned_element_count": spatial_result.element_ids.len(),
            "visited_nodes": spatial_result.visited_nodes,
            "tested_bounds": spatial_result.tested_bounds,
            "visited_node_ratio": spatial_result.visited_nodes as f64 / spatial_metrics_after_query.node_count as f64,
            "visited_nodes_under_one_percent": spatial_result.visited_nodes.saturating_mul(100) < spatial_metrics_after_query.node_count,
            "index_metrics_after_query": spatial_metrics_after_query,
            "index_metrics_after_local_update": spatial_metrics_after_update,
        },
        "one_opening_local_update": {
            "opening_id": first_opening_id.get(),
            "host_id": first_host_id.get(),
            "new_width_m": 1.5,
            "new_center_x_m": updated_center_x,
            "elapsed_ms": elapsed_ms(update_elapsed),
            "elapsed_us": elapsed_us(update_elapsed),
            "transaction_duration_us": update_receipt.transaction_duration_us,
            "dirty_propagation_duration_us": update_receipt.dirty_propagation_duration_us,
            "scanned_relationship_count": update_receipt.scanned_relationship_count,
            "document_scan_count": update_receipt.document_scan_count,
            "snapshot_serialization_duration_us": update_receipt.snapshot_serialization_duration_us,
            "snapshot_serialized_bytes": update_receipt.snapshot_serialized_bytes,
            "affected_ids": element_ids(&update_receipt.affected_ids),
            "dirty_ids": element_ids(&update_receipt.dirty_ids),
            "core_performance": update_receipt.performance,
            "geometry_rebuild": rebuild_json(&update_rebuild, update_rebuild_elapsed),
            "bounded_to_host_and_opening": update_rebuild.rebuilt_element_ids == expected_rebuilt_ids,
            "core_full_scan_count": update_receipt.performance.core_scan_count,
            "host_geometry_before": host_before,
            "host_geometry_after": host_after,
            "opening_geometry_before": opening_before,
            "opening_geometry_after": opening_after,
            "host_geometry_comparison": geometry_comparison,
            "quantity_cache_metrics_after_update": quantity_cache_metrics_after_update,
            "quantity_rebuilt_count_from_opening_update": update_rebuild.quantity_rebuilt_element_count,
            "spatial_updated_count_from_opening_update": update_rebuild.spatial_updated_element_count,
        },
        "history_change_observation": {
            "revision_before": history_diff.revision_from,
            "revision_after": history_diff.revision_to,
            "diff_elapsed_ms": elapsed_ms(history_diff_elapsed),
            "diff_elapsed_us": elapsed_us(history_diff_elapsed),
            "diff_serialized_bytes": history_diff_bytes,
            "diff": history_diff,
            "delta_elapsed_ms": elapsed_ms(history_delta_elapsed),
            "delta_elapsed_us": elapsed_us(history_delta_elapsed),
            "delta_serialized_bytes": history_delta_bytes,
            "delta": history_delta,
        },
    });

    let report_file = File::create(&report_path)?;
    serde_json::to_writer_pretty(BufWriter::new(report_file), &output)?;
    println!("Wrote native 10k fixture to {}", native_path.display());
    println!("Wrote 10k benchmark report to {}", report_path.display());
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

fn element_id_for_opening(index: usize) -> ElementId {
    ElementId::new((WALL_COUNT + index + 1) as u64)
}

fn element_ids(ids: &[ElementId]) -> Vec<u64> {
    ids.iter().map(|id| id.get()).collect()
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn elapsed_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn rebuild_json(report: &RebuildReport, elapsed: Duration) -> Value {
    json!({
        "elapsed_ms": elapsed_ms(elapsed),
        "elapsed_us": elapsed_us(elapsed),
        "reported_rebuild_duration_ms": report.rebuild_duration_ms,
        "reported_rebuild_duration_us": report.rebuild_duration_us,
        "rebuilt_element_count": report.rebuilt_element_count,
        "quantity_rebuilt_element_count": report.quantity_rebuilt_element_count,
        "spatial_updated_element_count": report.spatial_updated_element_count,
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

fn history_metrics_json(metrics: &HistoryMetrics) -> Value {
    serde_json::to_value(metrics).expect("HistoryMetrics is serializable")
}

fn create_parent(path: &PathBuf) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.into())
}
