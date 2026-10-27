use ai_revit_core::*;
use std::collections::BTreeMap;
use std::time::Instant;

fn wall(i: usize) -> Command {
    Command::CreateElement {
        name: format!("Wall {i}"),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(i as f64 * 10., 0.),
            end: Point2::new(i as f64 * 10. + 8., 0.),
            base_elevation_m: 0.,
            height_m: 3.,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    }
}
fn micros(t: Instant) -> u64 {
    t.elapsed().as_micros() as u64
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut document = Document::new("v0.4 synthetic 10k Core")?;
    let build = Instant::now();
    let mut transaction = document.begin_transaction("seed-10k", 0);
    for i in 0..10_000 {
        transaction.push(wall(i));
    }
    let seed = document.commit(transaction)?;
    let build_us = micros(build);
    document.verify_index_consistency()?;
    let serialization = Instant::now();
    let serialized = document.to_json_string()?;
    let serialize_us = micros(serialization);
    let open = Instant::now();
    let mut document = Document::from_json_str(&serialized)?;
    let open_us = micros(open);
    let observation = Instant::now();
    let index_metrics = document.index_metrics();
    let history_metrics = document.history_metrics()?;
    let observe_us = micros(observation);
    let id = seed.created_elements[5_000];
    let before = document.revision();
    let local = document.commit(document.begin_transaction("one-local-update", before).with(
        Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: PropertyValue::Number {
                value: 3.5,
                unit: Unit::Meter,
            },
        },
    ))?;
    assert_eq!(local.performance.core_scan_count, 0);
    assert!(local.performance.elements_scanned < 32);
    let diff = Instant::now();
    let history_diff = document.history_diff(before, document.revision())?;
    let history_diff_us = micros(diff);
    assert_eq!(history_diff.modified, vec![id]);
    let artifact = std::path::Path::new("artifacts/v04/core-10k-benchmark.json");
    let report = serde_json::json!({"benchmark":"10k_core","profile":"release","element_count":index_metrics.element_by_id,"fixture":"10,000 independent parametric walls, one atomic seed batch","build_us":build_us,"serialize_us":serialize_us,"open_us":open_us,"core_observe_us":observe_us,"index_metrics":index_metrics,"history_metrics_before_update":history_metrics,"serialized_pretty_bytes":serialized.len(),"local_receipt":local,"history_diff_us":history_diff_us,"history_diff":history_diff});
    std::fs::create_dir_all(artifact.parent().unwrap())?;
    std::fs::write(artifact, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
