use ai_revit_core::{
    BUILTIN_FLOOR_CLASS_ID, BUILTIN_SPACE_CLASS_ID, Command, Document, ElementId, ElementKind,
    FloorSpec, Point2, SpaceSpec,
};
use ai_revit_geometry::GeometryCache;
use std::collections::BTreeMap;

fn boundary(width: f64, depth: f64) -> Vec<Point2> {
    vec![
        Point2::new(0.0, 0.0),
        Point2::new(width, 0.0),
        Point2::new(width, depth),
        Point2::new(0.0, depth),
    ]
}

fn fixture() -> (Document, ElementId, ElementId) {
    let mut document = Document::new("Space fingerprint regressions").unwrap();
    let initial_metadata = BTreeMap::from([
        ("phase".to_owned(), serde_json::Value::from(1)),
        ("zone".to_owned(), serde_json::Value::from("West")),
    ]);
    let mut transaction = document.begin_transaction("create fixture", document.revision());
    transaction.push(Command::CreateElement {
        name: "Space".into(),
        class_id: BUILTIN_SPACE_CLASS_ID.into(),
        kind: ElementKind::Space(SpaceSpec {
            boundary: Some(boundary(4.0, 3.0)),
            metadata: initial_metadata,
        }),
        properties: BTreeMap::new(),
    });
    transaction.push(Command::CreateElement {
        name: "Unrelated floor".into(),
        class_id: BUILTIN_FLOOR_CLASS_ID.into(),
        kind: ElementKind::Floor(FloorSpec {
            boundary: boundary(8.0, 6.0),
            holes: Vec::new(),
            elevation_m: 0.0,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    });
    let receipt = document.commit(transaction).unwrap();
    (
        document,
        receipt.created_elements[0],
        receipt.created_elements[1],
    )
}

fn update_space(document: &mut Document, id: ElementId, spec: SpaceSpec, command_id: &str) {
    let mut transaction = document.begin_transaction(command_id, document.revision());
    transaction.push(Command::UpdateElement {
        element_id: id,
        kind: ElementKind::Space(spec),
    });
    document.commit(transaction).unwrap();
}

fn assert_persisted_space(
    document: &Document,
    id: ElementId,
    expected_boundary: &[Point2],
    expected_metadata: &BTreeMap<String, serde_json::Value>,
) {
    let element = document.element(id).unwrap();
    let ElementKind::Space(spec) = &element.kind else {
        panic!("expected persisted Space, got {:?}", element.kind);
    };
    assert_eq!(spec.boundary.as_deref(), Some(expected_boundary));
    assert_eq!(&spec.metadata, expected_metadata);
}

#[test]
fn space_metadata_update_persists_without_changing_geometry_or_quantities() {
    let (mut document, space_id, unrelated_id) = fixture();
    let mut cache = GeometryCache::new();
    cache.rebuild(&document);

    let before = cache.inspect(space_id).unwrap().clone();
    let status_before = cache.element_status(space_id).unwrap().clone();
    let quantities_before = cache.quantities_get(space_id).unwrap();
    let unrelated_before = cache.inspect(unrelated_id).unwrap().clone();
    let unrelated_quantities_before = cache.quantities_get(unrelated_id).unwrap();
    let quantity_rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;
    let history_count_before = cache.geometry_history_entry_count();
    let source_revision_before = document.revision();

    let edited_metadata = BTreeMap::from([
        (
            "discipline".to_owned(),
            serde_json::Value::from("Architecture"),
        ),
        ("zone".to_owned(), serde_json::Value::from("East")),
    ]);
    let unchanged_boundary = boundary(4.0, 3.0);
    update_space(
        &mut document,
        space_id,
        SpaceSpec {
            boundary: Some(unchanged_boundary.clone()),
            metadata: edited_metadata.clone(),
        },
        "edit-space-metadata",
    );
    assert!(document.revision() > source_revision_before);
    assert_persisted_space(&document, space_id, &unchanged_boundary, &edited_metadata);

    let report = cache.rebuild(&document);

    assert_eq!(report.rebuilt_element_count, 0);
    assert!(report.rebuilt_element_ids.is_empty());
    assert_eq!(report.quantity_rebuilt_element_count, 0);
    let after = cache.inspect(space_id).unwrap();
    assert_eq!(after.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(after.fingerprint, before.fingerprint);
    assert_eq!(after.fingerprint, after.geometry_fingerprint);
    assert_eq!(after.geometry_revision, before.geometry_revision);
    assert_eq!(after.mesh_revision, before.mesh_revision);
    assert_eq!(after.source_document_revision, document.revision());
    assert!(after.source_document_revision > before.source_document_revision);
    assert_eq!(after.derived_from, before.derived_from);
    assert_eq!(after.openings, before.openings);

    assert_eq!(
        cache.element_status(space_id).unwrap().mesh_revision,
        status_before.mesh_revision
    );
    assert_eq!(cache.quantities_get(space_id).unwrap(), quantities_before);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        quantity_rebuilds_before
    );
    assert_eq!(cache.geometry_history_entry_count(), history_count_before);
    assert_eq!(
        cache.observation_metrics().history_record_count,
        history_count_before
    );
    assert_eq!(cache.inspect(unrelated_id), Some(&unrelated_before));
    assert_eq!(
        cache.quantities_get(unrelated_id),
        Some(unrelated_quantities_before)
    );
}

#[test]
fn space_boundary_update_changes_fingerprint_and_rebuilds_only_space() {
    let (mut document, space_id, unrelated_id) = fixture();
    let mut cache = GeometryCache::new();
    cache.rebuild(&document);

    let before = cache.inspect(space_id).unwrap().clone();
    let status_before = cache.element_status(space_id).unwrap().clone();
    let quantities_before = cache.quantities_get(space_id).unwrap();
    let unrelated_before = cache.inspect(unrelated_id).unwrap().clone();
    let unrelated_quantities_before = cache.quantities_get(unrelated_id).unwrap();
    let quantity_rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;
    let history_count_before = cache.geometry_history_entry_count();
    let persisted_spec = match document.element(space_id).unwrap().kind.clone() {
        ElementKind::Space(spec) => spec,
        other => panic!("expected persisted Space, got {other:?}"),
    };
    let revised_boundary = boundary(5.0, 3.0);

    update_space(
        &mut document,
        space_id,
        SpaceSpec {
            boundary: Some(revised_boundary.clone()),
            metadata: persisted_spec.metadata.clone(),
        },
        "edit-space-boundary",
    );
    assert!(document.revision() > before.source_document_revision);
    assert_persisted_space(
        &document,
        space_id,
        &revised_boundary,
        &persisted_spec.metadata,
    );

    let report = cache.rebuild(&document);

    assert_eq!(report.rebuilt_element_count, 1);
    assert_eq!(report.rebuilt_element_ids, vec![space_id]);
    assert_eq!(report.quantity_rebuilt_element_count, 1);
    let after = cache.inspect(space_id).unwrap();
    assert_ne!(after.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(after.fingerprint, after.geometry_fingerprint);
    assert!(after.geometry_revision > before.geometry_revision);
    assert!(after.mesh_revision > before.mesh_revision);
    assert_eq!(after.source_document_revision, document.revision());
    assert_eq!(after.derived_from, before.derived_from);
    assert_eq!(after.openings, before.openings);

    let status_after = cache.element_status(space_id).unwrap();
    assert!(status_after.mesh_revision > status_before.mesh_revision);
    assert_eq!(status_after.mesh_revision, after.mesh_revision);
    let quantities_after = cache.quantities_get(space_id).unwrap();
    assert_eq!(quantities_after.geometry_revision, after.geometry_revision);
    assert_eq!(
        quantities_after.geometry_fingerprint,
        after.geometry_fingerprint
    );
    assert_eq!(quantities_after.quantities, quantities_before.quantities);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        quantity_rebuilds_before + 1
    );
    assert_eq!(
        cache.geometry_history_entry_count(),
        history_count_before + 1
    );
    assert_eq!(
        cache.observation_metrics().history_record_count,
        history_count_before + 1
    );
    assert_eq!(cache.inspect(unrelated_id), Some(&unrelated_before));
    assert_eq!(
        cache.quantities_get(unrelated_id),
        Some(unrelated_quantities_before)
    );
}
