use ai_revit_core::{
    Command, Document, ElementId, ElementKind, FloorSpec, OpeningSpec, Point2, SpaceSpec,
};
use ai_revit_geometry::{
    GeometryBackend, GeometryCache, GeometryInput, GeometryOptions, MeshGeometryBackend,
    RepresentationQuality, SectionPlane,
};
use viewer_core::Vec3;

fn square(x: f64, y: f64, size: f64) -> Vec<Point2> {
    vec![
        Point2::new(x, y),
        Point2::new(x + size, y),
        Point2::new(x + size, y + size),
        Point2::new(x, y + size),
    ]
}
fn floor_fixture(native_hole: bool) -> (Document, ElementId, ElementId) {
    let mut doc = Document::new("v04 geometry contract").unwrap();
    let host = doc.allocation_preview().element_id;
    let mut tx = doc.begin_transaction("create fixture", doc.revision());
    tx.push(Command::CreateElement {
        name: "Floor".into(),
        class_id: ai_revit_core::BUILTIN_FLOOR_CLASS_ID.into(),
        kind: ElementKind::Floor(FloorSpec {
            boundary: vec![
                Point2::new(0.0, 0.0),
                Point2::new(6.0, 0.0),
                Point2::new(6.0, 4.0),
                Point2::new(0.0, 4.0),
            ],
            holes: if native_hole {
                vec![square(0.5, 0.5, 1.0)]
            } else {
                Vec::new()
            },
            elevation_m: 0.4,
            thickness_m: 0.2,
        }),
        properties: Default::default(),
    });
    tx.push(Command::CreateOpening {
        name: "Opening".into(),
        spec: OpeningSpec {
            center: Point2::new(4.0, 2.0),
            elevation_m: 0.2,
            width_m: 1.0,
            height_m: 0.2,
            depth_m: 1.0,
            rotation_rad: 0.0,
        },
        properties: Default::default(),
        host_id: host,
        type_id: None,
    });
    let receipt = doc.commit(tx).unwrap();
    (doc, host, receipt.created_elements[1])
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-7, "{a} != {b}");
}

#[test]
fn native_holes_belong_to_gross_base_and_section_keeps_all_three_loops() {
    let (doc, host, opening) = floor_fixture(true);
    let mut cache = GeometryCache::new();
    cache.rebuild(&doc);
    let record = cache.inspect(host).unwrap();
    assert!(record.valid, "{:?}", record.failure);
    assert_eq!(record.fingerprint, record.geometry_fingerprint);
    let q = cache.quantities_get(host).unwrap().quantities;
    near(q.gross_volume.unwrap(), 4.6);
    near(q.net_volume.unwrap(), 4.4);
    near(q.opening_volume.unwrap(), 0.2);
    near(q.opening_area.unwrap(), 1.0);
    near(q.gross_surface_area.unwrap(), 50.8);
    near(q.net_surface_area.unwrap(), 49.6);
    near(q.perimeter.unwrap(), 28.0);
    let section = cache
        .section(
            &[host],
            SectionPlane {
                origin: Vec3::new(0.0, 0.0, 0.3),
                normal: Vec3::new(0.0, 0.0, 1.0),
            },
        )
        .unwrap();
    assert_eq!(section.loops.len(), 3);
    assert!(section.loops.iter().all(|l| l.closed));
    let batch = cache.quantities_batch(&[host, opening]);
    near(batch.aggregate.net_volume.unwrap(), 4.4);
    assert!(
        cache
            .quantities_get(opening)
            .unwrap()
            .quantities
            .net_volume
            .is_none()
    );
}

#[test]
fn outside_error_identifies_the_actual_second_contributor() {
    let (doc, host, opening) = floor_fixture(false);
    let first = doc.effective_element(opening).unwrap();
    let mut second = first.clone();
    second.id = ElementId::new(opening.get() + 1);
    if let ElementKind::Opening(ref mut spec) = second.kind {
        spec.center.x = 20.0;
    }
    let second_id = second.id;
    let input = GeometryInput {
        element: doc.effective_element(host).unwrap(),
        openings: vec![first, second],
        derived_from: vec![host, opening, second_id],
    };
    let error = MeshGeometryBackend
        .derive_host(&input, GeometryOptions::default())
        .err()
        .unwrap();
    assert_eq!(error.error_code, "void_outside_host");
    assert_eq!(error.canonical_error_code, "void_outside_host");
    assert_eq!(error.host_id, Some(host));
    assert_eq!(error.opening_id, Some(second_id));
    assert_eq!(error.backend, "mesh");
    assert_eq!(
        error.representation_quality,
        RepresentationQuality::ApproximateMesh
    );
}

#[test]
fn space_has_no_inferred_mass_and_capabilities_never_claim_exact_geometry() {
    let mut doc = Document::new("Semantic space").unwrap();
    let mut tx = doc.begin_transaction("create space", doc.revision());
    tx.push(Command::CreateElement {
        name: "Space".into(),
        class_id: ai_revit_core::BUILTIN_SPACE_CLASS_ID.into(),
        kind: ElementKind::Space(SpaceSpec {
            boundary: Some(square(0.0, 0.0, 4.0)),
            metadata: Default::default(),
        }),
        properties: Default::default(),
    });
    let id = doc.commit(tx).unwrap().created_elements[0];
    let mut cache = GeometryCache::new();
    cache.rebuild(&doc);
    let record = cache.inspect(id).unwrap();
    assert!(record.valid);
    assert_eq!(record.triangle_count, 0);
    assert!(record.volume.is_none());
    assert!(record.bounds.is_none());
    assert!(
        cache
            .quantities_get(id)
            .unwrap()
            .quantities
            .net_volume
            .is_none()
    );
    let caps = cache.backend_capabilities();
    assert_eq!(caps.backend, "mesh");
    assert!(caps.capabilities.polygonal_voids);
    assert!(
        !caps.capabilities.exact_brep
            && !caps.capabilities.exact_boolean
            && !caps.capabilities.exact_volume
            && !caps.capabilities.exact_section
    );
}
