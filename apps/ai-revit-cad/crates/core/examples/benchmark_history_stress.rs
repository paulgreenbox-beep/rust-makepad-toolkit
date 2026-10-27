use ai_revit_core::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

fn micros(t: Instant) -> u64 {
    t.elapsed().as_micros() as u64
}
fn fixture(policy: HistoryPolicy) -> Result<(Document, ElementId), CoreError> {
    let mut document = Document::new("1000 edit history stress")?;
    document.set_history_policy(policy)?;
    let mut t = document.begin_transaction("seed", 0);
    for i in 0..128 {
        t.push(Command::CreateElement {
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
        });
    }
    let id = document.commit(t)?.created_elements[64];
    Ok((document, id))
}
fn run(name: &str, policy: HistoryPolicy) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let (mut d, id) = fixture(policy)?;
    let edits = Instant::now();
    let mut total_full_scans = 0u64;
    let mut total_elements_scanned = 0u64;
    let mut total_snapshot_us = 0u64;
    for i in 0..1000 {
        let r = d.commit(d.begin_transaction(format!("edit-{i}"), d.revision()).with(
            Command::SetProperty {
                element_id: id,
                property_id: "height".into(),
                value: PropertyValue::Number {
                    value: 3. + i as f64 / 1000.,
                    unit: Unit::Meter,
                },
            },
        ))?;
        total_full_scans += r.performance.core_scan_count;
        total_elements_scanned += r.performance.elements_scanned;
        total_snapshot_us += r.performance.snapshot_us;
    }
    let edits_us = micros(edits);
    let metrics = d.history_metrics()?;
    assert_eq!(metrics.history_entry_count, 1001);
    let diff = Instant::now();
    let changes = d.history_diff(1000, 1001)?;
    let diff_us = micros(diff);
    assert_eq!(changes.modified, vec![id]);
    let undo = Instant::now();
    let undo_receipt = d.undo()?.unwrap();
    let undo_us = micros(undo);
    d.redo()?.unwrap();
    let path = std::path::PathBuf::from(format!("artifacts/v04/core-history-{name}.json"));
    let save = Instant::now();
    d.save(&path)?;
    let save_us = micros(save);
    let saved_bytes = std::fs::metadata(&path)?.len();
    let reopen = Instant::now();
    let reopened = Document::open(&path)?;
    let reopen_us = micros(reopen);
    reopened.verify_index_consistency()?;
    assert_eq!(reopened.history_diff(1000, 1001)?, changes);
    Ok(
        serde_json::json!({"policy":name,"checkpoint_interval":policy.checkpoint_interval,"fixture_element_count":128,"sequential_local_edits":1000,"edits_us":edits_us,"total_full_scans":total_full_scans,"total_elements_scanned":total_elements_scanned,"total_snapshot_us":total_snapshot_us,"history_metrics":metrics,"saved_pretty_bytes":saved_bytes,"save_us":save_us,"reopen_us":reopen_us,"undo_us":undo_us,"diff_us":diff_us,"undo_performance":undo_receipt.performance,"document_path":path}),
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(Path::new("artifacts/v04"))?;
    let full = run("full_snapshot", HistoryPolicy::full_snapshot())?;
    let compact = run("compact", HistoryPolicy::compact(32))?;
    let report = serde_json::json!({"benchmark":"1000_sequential_local_edits","profile":"release","full_snapshot":full,"compact":compact});
    std::fs::write(
        "artifacts/v04/core-history-stress-benchmark.json",
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
