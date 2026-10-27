use ai_revit_core::{
    BUILTIN_FLOOR_CLASS_ID, BUILTIN_OPENING_CLASS_ID, BUILTIN_ROOF_CLASS_ID, ClassDef, ClassId,
    Command, Document, ElementId, ElementKind, FloorSpec, LevelId, OpeningSpec, Point2,
    PolygonalOpeningSpec, PropertyDef, PropertyType, PropertyValue, RoofSpec, WallSpec,
};
use ai_revit_geometry::{
    GeometryBackend, GeometryCache, GeometryInput, GeometryOptions, GeometryQuantities,
    MeshGeometryBackend, QuantityGrouping, SpatialQuery, object_id_for, project_dirty_document,
};
use std::collections::BTreeMap;
use viewer_core::{Aabb, Ray, Vec3};

const EPSILON: f64 = 1.0e-7;

fn execute(
    document: &mut Document,
    command_id: &str,
    commands: Vec<Command>,
) -> ai_revit_core::CommitReceipt {
    let mut transaction = document.begin_transaction(command_id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    document
        .commit(transaction)
        .expect("test transaction should commit")
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn assert_optional_near(actual: Option<f64>, expected: f64) {
    assert_near(actual.expect("quantity should be present"), expected);
}

fn floor_spec_at(x: f64, y: f64) -> FloorSpec {
    FloorSpec {
        boundary: vec![
            Point2::new(x, y),
            Point2::new(x + 6.0, y),
            Point2::new(x + 6.0, y + 4.0),
            Point2::new(x, y + 4.0),
        ],
        holes: Vec::new(),
        // Floor elevation is its top plane; the fixture spans z=0.2..0.4.
        elevation_m: 0.4,
        thickness_m: 0.2,
    }
}

fn create_element(
    document: &mut Document,
    command_id: &str,
    name: &str,
    class_id: impl Into<ClassId>,
    kind: ElementKind,
) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![Command::CreateElement {
            name: name.to_owned(),
            class_id: class_id.into(),
            kind,
            properties: BTreeMap::new(),
        }],
    )
    .created_elements;
    assert_eq!(created, vec![expected_id]);
    expected_id
}

fn create_floor(document: &mut Document, command_id: &str, x: f64, y: f64) -> ElementId {
    create_element(
        document,
        command_id,
        "Test floor",
        BUILTIN_FLOOR_CLASS_ID,
        ElementKind::Floor(floor_spec_at(x, y)),
    )
}

fn create_rectangular_opening(
    document: &mut Document,
    command_id: &str,
    host_id: ElementId,
    spec: OpeningSpec,
) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![Command::CreateOpening {
            name: "Rectangular through opening".into(),
            spec,
            properties: BTreeMap::new(),
            host_id,
            type_id: None,
        }],
    )
    .created_elements;
    assert_eq!(created, vec![expected_id]);
    expected_id
}

fn create_polygonal_opening(
    document: &mut Document,
    command_id: &str,
    host_id: ElementId,
    spec: PolygonalOpeningSpec,
) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![
            Command::CreateElement {
                name: "Polygonal through opening".into(),
                class_id: BUILTIN_OPENING_CLASS_ID.into(),
                kind: ElementKind::PolygonalOpening(spec),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id,
                element_id: expected_id,
            },
        ],
    )
    .created_elements;
    assert_eq!(created, vec![expected_id]);
    expected_id
}

fn triangle(center: Point2, elevation_m: f64, depth_m: f64) -> PolygonalOpeningSpec {
    PolygonalOpeningSpec {
        center,
        elevation_m,
        boundary: vec![
            Point2::new(-1.0, -1.0),
            Point2::new(1.0, -1.0),
            Point2::new(0.0, 1.0),
        ],
        depth_m,
        rotation_rad: 0.0,
    }
}

fn quantities(cache: &GeometryCache, element_id: ElementId) -> GeometryQuantities {
    cache
        .quantities_get(element_id)
        .expect("valid derived element should have cached quantities")
        .quantities
}

fn assert_cut_quantities(
    cache: &GeometryCache,
    host_id: ElementId,
    gross_volume: f64,
    net_volume: f64,
    opening_area: f64,
    opening_volume: f64,
) {
    let q = quantities(cache, host_id);
    assert_optional_near(q.gross_volume, gross_volume);
    assert_optional_near(q.net_volume, net_volume);
    assert_optional_near(q.opening_area, opening_area);
    assert_optional_near(q.opening_volume, opening_volume);
}

#[test]
fn floor_rectangular_through_opening_cuts_mesh_and_reports_gross_and_net() {
    let mut document = Document::new("Floor rectangular opening").unwrap();
    let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
    create_rectangular_opening(
        &mut document,
        "rect-opening",
        floor_id,
        OpeningSpec {
            center: Point2::new(2.0, 2.0),
            elevation_m: 0.2,
            width_m: 1.2,
            height_m: 0.2,
            depth_m: 1.5,
            rotation_rad: 0.0,
        },
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);

    let floor = cache.inspect(floor_id).expect("floor record");
    assert!(floor.valid);
    assert_near(floor.volume.unwrap(), 4.44);
    assert_cut_quantities(&cache, floor_id, 4.8, 4.44, 1.8, 0.36);
    let mesh = &cache.scene().get(object_id_for(floor_id)).unwrap().mesh;
    assert!(
        mesh.pick(
            object_id_for(floor_id),
            Ray::new(Vec3::new(2.0, 2.0, 1.0), Vec3::new(0.0, 0.0, -1.0)),
            2.0,
        )
        .is_none(),
        "the host mesh must leave a real through-hole"
    );
    assert!(
        mesh.pick(
            object_id_for(floor_id),
            Ray::new(Vec3::new(0.5, 0.5, 1.0), Vec3::new(0.0, 0.0, -1.0)),
            2.0,
        )
        .is_some(),
        "solid floor beside the opening remains pickable"
    );
}

#[test]
fn floor_polygonal_through_opening_handles_both_ring_windings() {
    for (suffix, boundary) in [
        (
            "ccw",
            vec![
                Point2::new(-1.0, -1.0),
                Point2::new(1.0, -1.0),
                Point2::new(0.0, 1.0),
            ],
        ),
        (
            "cw",
            vec![
                Point2::new(0.0, 1.0),
                Point2::new(1.0, -1.0),
                Point2::new(-1.0, -1.0),
            ],
        ),
    ] {
        let mut document = Document::new(format!("Floor polygon {suffix}")).unwrap();
        let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
        let mut spec = triangle(Point2::new(3.0, 2.0), 0.2, 0.2);
        spec.boundary = boundary;
        create_polygonal_opening(&mut document, "poly-opening", floor_id, spec);
        let mut cache = GeometryCache::new();

        project_dirty_document(&document, &mut cache);

        assert!(cache.inspect(floor_id).unwrap().valid);
        assert_cut_quantities(&cache, floor_id, 4.8, 4.4, 2.0, 0.4);
    }
}

fn roof_spec(slope_x: f64) -> RoofSpec {
    RoofSpec {
        footprint: floor_spec_at(0.0, 0.0).boundary,
        elevation_m: 0.4,
        thickness_m: 0.2,
        slope_x,
        slope_y: 0.0,
    }
}

#[test]
fn flat_and_single_slope_roofs_cut_rectangular_openings_with_vertical_depth() {
    for (suffix, slope_x) in [("flat", 0.0), ("single-slope", 0.2)] {
        let mut document = Document::new(format!("Roof rectangle {suffix}")).unwrap();
        let roof_id = create_element(
            &mut document,
            "roof",
            "Test roof",
            BUILTIN_ROOF_CLASS_ID,
            ElementKind::Roof(roof_spec(slope_x)),
        );
        // A rectangular cut is a world cuboid. On the slope its supplied
        // vertical span covers the lowest bottom and highest top at its edges.
        let (elevation_m, height_m) = if slope_x == 0.0 {
            (0.2, 0.2)
        } else {
            (0.08, 0.44)
        };
        create_rectangular_opening(
            &mut document,
            "rect-opening",
            roof_id,
            OpeningSpec {
                center: Point2::new(3.0, 2.0),
                elevation_m,
                width_m: 1.2,
                height_m,
                depth_m: 1.5,
                rotation_rad: 0.0,
            },
        );
        let mut cache = GeometryCache::new();

        project_dirty_document(&document, &mut cache);

        assert!(cache.inspect(roof_id).unwrap().valid, "{suffix} roof");
        assert_cut_quantities(
            &cache,
            roof_id,
            4.8,
            4.44,
            1.8 * (1.0 + slope_x * slope_x).sqrt(),
            0.36,
        );
    }
}

#[test]
fn flat_and_single_slope_roofs_cut_polygonal_openings_in_the_affine_host_plane() {
    for (suffix, slope_x) in [("flat", 0.0), ("single-slope", 0.2)] {
        let mut document = Document::new(format!("Roof polygon {suffix}")).unwrap();
        let roof_id = create_element(
            &mut document,
            "roof",
            "Test roof",
            BUILTIN_ROOF_CLASS_ID,
            ElementKind::Roof(roof_spec(slope_x)),
        );
        let mut spec = triangle(Point2::new(3.0, 2.0), 0.2, 0.2);
        spec.rotation_rad = 0.0;
        create_polygonal_opening(&mut document, "poly-opening", roof_id, spec);
        let mut cache = GeometryCache::new();

        project_dirty_document(&document, &mut cache);

        assert!(cache.inspect(roof_id).unwrap().valid, "{suffix} roof");
        assert_cut_quantities(
            &cache,
            roof_id,
            4.8,
            4.4,
            2.0 * (1.0 + slope_x * slope_x).sqrt(),
            0.4,
        );
    }
}

fn mixed_overlap_fixture(rectangle_first: bool) -> (Document, ElementId) {
    let mut document = Document::new("Mixed opening union").unwrap();
    let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
    let rectangle = || OpeningSpec {
        center: Point2::new(3.0, 2.0),
        elevation_m: 0.2,
        width_m: 2.0,
        height_m: 0.2,
        depth_m: 2.0,
        rotation_rad: 0.0,
    };
    let polygon = || triangle(Point2::new(4.0, 2.0), 0.2, 0.2);
    if rectangle_first {
        create_rectangular_opening(&mut document, "rectangle", floor_id, rectangle());
        create_polygonal_opening(&mut document, "polygon", floor_id, polygon());
    } else {
        create_polygonal_opening(&mut document, "polygon", floor_id, polygon());
        create_rectangular_opening(&mut document, "rectangle", floor_id, rectangle());
    }
    (document, floor_id)
}

#[test]
fn overlapping_rectangular_and_polygonal_voids_form_an_order_independent_union() {
    let (document_a, floor_a) = mixed_overlap_fixture(true);
    let (document_b, floor_b) = mixed_overlap_fixture(false);
    let mut cache_a = GeometryCache::new();
    let mut cache_b = GeometryCache::new();
    project_dirty_document(&document_a, &mut cache_a);
    project_dirty_document(&document_b, &mut cache_b);

    let q_a = quantities(&cache_a, floor_a);
    let q_b = quantities(&cache_b, floor_b);
    // Rectangle area 4 plus triangle area 2 minus overlap area 1.
    assert_optional_near(q_a.gross_volume, 4.8);
    assert_optional_near(q_a.net_volume, 3.8);
    assert_optional_near(q_a.opening_area, 5.0);
    assert_optional_near(q_a.opening_volume, 1.0);
    assert_eq!(q_a, q_b, "command order must not change union quantities");
    assert_eq!(
        cache_a
            .scene()
            .get(object_id_for(floor_a))
            .unwrap()
            .mesh
            .triangles(),
        cache_b
            .scene()
            .get(object_id_for(floor_b))
            .unwrap()
            .mesh
            .triangles(),
        "command order must not change the derived host mesh"
    );
}

fn one_polygon_floor(depth_m: f64) -> (Document, ElementId, ElementId) {
    let mut document = Document::new("Polygon backend error fixture").unwrap();
    let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
    let opening_id = create_polygonal_opening(
        &mut document,
        "polygon",
        floor_id,
        triangle(Point2::new(3.0, 2.0), 0.2, depth_m),
    );
    (document, floor_id, opening_id)
}

fn direct_host_result(
    document: &Document,
    host_id: ElementId,
    opening: ai_revit_core::ElementRecord,
) -> Result<ai_revit_geometry::DerivedGeometry, ai_revit_geometry::GeometryError> {
    let host = document.effective_element(host_id).unwrap();
    let mut derived_from = vec![host_id, opening.id];
    derived_from.sort_unstable();
    MeshGeometryBackend.derive_element(
        &GeometryInput {
            element: host,
            openings: vec![opening],
            derived_from,
        },
        GeometryOptions::default(),
    )
}

#[test]
fn backend_reports_a_structured_error_for_a_self_intersecting_polygon() {
    let (document, floor_id, opening_id) = one_polygon_floor(0.2);
    let mut invalid = document.effective_element(opening_id).unwrap();
    let ElementKind::PolygonalOpening(mut spec) = invalid.kind else {
        panic!("fixture should be polygonal")
    };
    spec.boundary = vec![
        Point2::new(-1.0, -1.0),
        Point2::new(1.0, 1.0),
        Point2::new(-1.0, 1.0),
        Point2::new(1.0, -1.0),
    ];
    invalid.kind = ElementKind::PolygonalOpening(spec);

    let error = direct_host_result(&document, floor_id, invalid)
        .err()
        .expect("backend must reject a self-intersecting opening ring");
    assert_eq!(error.error_code, "void_self_intersection");
    assert_eq!(error.host_id, Some(floor_id));
    assert_eq!(error.opening_id, Some(opening_id));
    assert!(!error.message.is_empty());
}

#[test]
fn backend_rejects_polygonal_through_depth_short_of_the_floor() {
    let (document, floor_id, opening_id) = one_polygon_floor(0.2);
    let mut opening = document.effective_element(opening_id).unwrap();
    // Core rejects a shallow hosted polygon at commit. Inject the adversarial
    // effective input directly to verify the backend's independent validation.
    let ElementKind::PolygonalOpening(ref mut spec) = opening.kind else {
        panic!("fixture should be polygonal")
    };
    spec.depth_m = 0.1;

    let error = direct_host_result(&document, floor_id, opening)
        .err()
        .expect("a 0.1 m void must not pass through a 0.2 m slab");
    assert_eq!(error.error_code, "void_depth_insufficient");
    assert_eq!(error.host_id, Some(floor_id));
    assert_eq!(error.opening_id, Some(opening_id));
}

#[test]
fn rotated_wall_polygon_uses_the_wall_baseline_as_its_local_x_axis() {
    let mut document = Document::new("Rotated wall polygon").unwrap();
    let wall_id = create_element(
        &mut document,
        "wall",
        "North-south wall",
        ai_revit_core::BUILTIN_WALL_CLASS_ID,
        ElementKind::Wall(WallSpec {
            start: Point2::new(1.0, 2.0),
            end: Point2::new(1.0, 8.0),
            base_elevation_m: 0.0,
            height_m: 3.0,
            thickness_m: 0.2,
        }),
    );
    create_polygonal_opening(
        &mut document,
        "wall-opening",
        wall_id,
        PolygonalOpeningSpec {
            center: Point2::new(1.0, 5.0),
            elevation_m: 0.5,
            boundary: vec![
                Point2::new(-0.5, 0.5),
                Point2::new(0.5, 0.5),
                Point2::new(0.5, 2.0),
                Point2::new(-0.5, 2.0),
            ],
            depth_m: 0.2,
            rotation_rad: std::f64::consts::FRAC_PI_2,
        },
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);

    let wall = cache.inspect(wall_id).unwrap();
    assert!(wall.valid);
    assert_near(wall.volume.unwrap(), 3.3);
    assert_cut_quantities(&cache, wall_id, 3.6, 3.3, 1.5, 0.3);
    let mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    assert!(
        mesh.pick(
            object_id_for(wall_id),
            Ray::new(Vec3::new(0.0, 5.0, 1.5), Vec3::new(1.0, 0.0, 0.0)),
            2.0,
        )
        .is_none(),
        "the opening follows the rotated wall baseline"
    );
    assert!(
        mesh.pick(
            object_id_for(wall_id),
            Ray::new(Vec3::new(0.0, 4.0, 1.5), Vec3::new(1.0, 0.0, 0.0)),
            2.0,
        )
        .is_some(),
        "the adjacent wall remains solid"
    );
}

#[test]
fn metadata_only_change_does_not_rebuild_cached_quantities() {
    let mut document = Document::new("Quantity fingerprint cache").unwrap();
    execute(
        &mut document,
        "define-floor-description",
        vec![Command::DefineProperty {
            class_id: BUILTIN_FLOOR_CLASS_ID.into(),
            definition: PropertyDef::new("description", "Description", PropertyType::Text),
        }],
    );
    let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let quantity = cache.quantities_get(floor_id).unwrap().quantities.clone();
    let rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;

    execute(
        &mut document,
        "set-floor-description",
        vec![Command::SetProperty {
            element_id: floor_id,
            property_id: "description".into(),
            value: PropertyValue::Text("Metadata does not change shape".into()),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);

    assert_eq!(report.rebuilt_element_count, 0);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        rebuilds_before
    );
    assert_eq!(cache.quantities_get(floor_id).unwrap().quantities, quantity);
}

#[test]
fn cached_quantity_groups_aggregate_by_type_level_and_class() {
    let mut document = Document::new("Quantity group indexes").unwrap();
    let special_class = ClassId::from("test.floor.special");
    execute(
        &mut document,
        "define-special-floor-class",
        vec![Command::DefineClass(ClassDef::child(
            special_class.clone(),
            "Special floor",
            BUILTIN_FLOOR_CLASS_ID,
        ))],
    );
    let (floor_a, floor_b, floor_c) = (
        create_floor(&mut document, "floor-a", 0.0, 0.0),
        create_floor(&mut document, "floor-b", 10.0, 0.0),
        create_element(
            &mut document,
            "floor-c",
            "Special floor",
            special_class.clone(),
            ElementKind::Floor(floor_spec_at(20.0, 0.0)),
        ),
    );
    let types = execute(
        &mut document,
        "create-types",
        vec![
            Command::CreateType {
                name: "Floor type A".into(),
                class_id: BUILTIN_FLOOR_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
            Command::CreateType {
                name: "Floor type B".into(),
                class_id: BUILTIN_FLOOR_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
        ],
    )
    .created_types;
    let levels = execute(
        &mut document,
        "create-levels",
        vec![
            Command::CreateLevel {
                name: "Level 1".into(),
                elevation_m: 0.0,
                height_m: Some(3.0),
            },
            Command::CreateLevel {
                name: "Level 2".into(),
                elevation_m: 3.0,
                height_m: Some(3.0),
            },
        ],
    )
    .created_levels;
    execute(
        &mut document,
        "assign-type-and-levels",
        vec![
            Command::AssignType {
                element_id: floor_a,
                type_id: types[0],
            },
            Command::AssignType {
                element_id: floor_b,
                type_id: types[0],
            },
            Command::AssignType {
                element_id: floor_c,
                type_id: types[1],
            },
            Command::AssignLevel {
                element_id: floor_a,
                level_id: levels[0],
            },
            Command::AssignLevel {
                element_id: floor_b,
                level_id: levels[1],
            },
            Command::AssignLevel {
                element_id: floor_c,
                level_id: levels[1],
            },
        ],
    );
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let by_type = cache.quantities_grouped(QuantityGrouping::Type);
    let type_a = by_type
        .iter()
        .find(|group| group.type_id == Some(types[0]))
        .unwrap();
    assert_eq!(type_a.element_ids, vec![floor_a, floor_b]);
    assert_optional_near(type_a.quantities.net_volume, 9.6);
    let type_b = by_type
        .iter()
        .find(|group| group.type_id == Some(types[1]))
        .unwrap();
    assert_eq!(type_b.element_ids, vec![floor_c]);
    assert_optional_near(type_b.quantities.net_volume, 4.8);

    let by_level = cache.quantities_grouped(QuantityGrouping::Level);
    let level_two = by_level
        .iter()
        .find(|group| group.level_id == Some(levels[1]))
        .unwrap();
    assert_eq!(level_two.element_ids, vec![floor_b, floor_c]);
    assert_optional_near(level_two.quantities.gross_volume, 9.6);

    let by_class = cache.quantities_grouped(QuantityGrouping::Class);
    let special = by_class
        .iter()
        .find(|group| group.class_id.as_ref() == Some(&special_class))
        .unwrap();
    assert_eq!(special.element_ids, vec![floor_c]);
    assert_optional_near(special.quantities.net_volume, 4.8);
}

#[test]
fn quantity_batch_deduplicates_ids_and_returns_aggregate_and_missing_ids() {
    let mut document = Document::new("Quantity batch").unwrap();
    let floor_a = create_floor(&mut document, "floor-a", 0.0, 0.0);
    let floor_b = create_floor(&mut document, "floor-b", 10.0, 0.0);
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let missing = ElementId::new(9999);

    let batch = cache.quantities_batch(&[floor_b, floor_a, floor_b, missing]);

    assert_eq!(batch.element_ids, vec![floor_a, floor_b, missing]);
    assert_eq!(
        batch
            .elements
            .iter()
            .map(|element| element.element_id)
            .collect::<Vec<_>>(),
        vec![floor_a, floor_b]
    );
    assert_eq!(batch.missing_element_ids, vec![missing]);
    assert_optional_near(batch.aggregate.gross_volume, 9.6);
    assert_optional_near(batch.aggregate.net_volume, 9.6);
}

fn bounds(min: [f64; 3], max: [f64; 3]) -> Aabb {
    Aabb {
        min: Vec3::new(min[0], min[1], min[2]),
        max: Vec3::new(max[0], max[1], max[2]),
    }
}

fn spatial_query(class_id: Option<ClassId>, level_id: Option<LevelId>) -> SpatialQuery {
    SpatialQuery {
        bounds: bounds([-1.0, -1.0, -1.0], [30.0, 10.0, 2.0]),
        class_id,
        level_id,
    }
}

#[test]
fn spatial_bounds_query_filters_by_class_and_level() {
    let mut document = Document::new("Spatial query filters").unwrap();
    let special_class = ClassId::from("test.floor.special");
    execute(
        &mut document,
        "define-special-class",
        vec![Command::DefineClass(ClassDef::child(
            special_class.clone(),
            "Special floor",
            BUILTIN_FLOOR_CLASS_ID,
        ))],
    );
    let ordinary = create_floor(&mut document, "ordinary-floor", 0.0, 0.0);
    let special = create_element(
        &mut document,
        "special-floor",
        "Special floor",
        special_class.clone(),
        ElementKind::Floor(floor_spec_at(10.0, 0.0)),
    );
    let levels = execute(
        &mut document,
        "levels",
        vec![
            Command::CreateLevel {
                name: "Level 1".into(),
                elevation_m: 0.0,
                height_m: None,
            },
            Command::CreateLevel {
                name: "Level 2".into(),
                elevation_m: 3.0,
                height_m: None,
            },
        ],
    )
    .created_levels;
    execute(
        &mut document,
        "assign-levels",
        vec![
            Command::AssignLevel {
                element_id: ordinary,
                level_id: levels[0],
            },
            Command::AssignLevel {
                element_id: special,
                level_id: levels[1],
            },
        ],
    );
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    assert_eq!(
        cache
            .spatial_bounds_query(&spatial_query(None, None))
            .unwrap()
            .element_ids,
        vec![ordinary, special]
    );
    assert_eq!(
        cache
            .spatial_bounds_query(&spatial_query(Some(BUILTIN_FLOOR_CLASS_ID.into()), None,))
            .unwrap()
            .element_ids,
        vec![ordinary]
    );
    assert_eq!(
        cache
            .spatial_bounds_query(&spatial_query(None, Some(levels[1])))
            .unwrap()
            .element_ids,
        vec![special]
    );
    assert!(
        cache
            .spatial_bounds_query(&spatial_query(
                Some(BUILTIN_FLOOR_CLASS_ID.into()),
                Some(levels[1]),
            ))
            .unwrap()
            .element_ids
            .is_empty()
    );
}

fn local_query(min_x: f64, max_x: f64) -> SpatialQuery {
    SpatialQuery {
        bounds: bounds([min_x, -1.0, -1.0], [max_x, 5.0, 1.0]),
        class_id: None,
        level_id: None,
    }
}

#[test]
fn spatial_index_tracks_move_delete_undo_and_document_load() {
    let mut document = Document::new("Spatial incremental lifecycle").unwrap();
    let floor_id = create_floor(&mut document, "floor", 0.0, 0.0);
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let old_query = local_query(-1.0, 7.0);
    let new_query = local_query(9.0, 17.0);
    assert_eq!(
        cache.spatial_bounds_query(&old_query).unwrap().element_ids,
        vec![floor_id]
    );

    let mut moved = match document.element(floor_id).unwrap().kind.clone() {
        ElementKind::Floor(spec) => spec,
        other => panic!("expected floor, got {other:?}"),
    };
    for point in &mut moved.boundary {
        point.x += 10.0;
    }
    execute(
        &mut document,
        "move-floor",
        vec![Command::UpdateElement {
            element_id: floor_id,
            kind: ElementKind::Floor(moved),
        }],
    );
    project_dirty_document(&document, &mut cache);
    assert!(
        cache
            .spatial_bounds_query(&old_query)
            .unwrap()
            .element_ids
            .is_empty()
    );
    assert_eq!(
        cache.spatial_bounds_query(&new_query).unwrap().element_ids,
        vec![floor_id]
    );

    execute(
        &mut document,
        "delete-floor",
        vec![Command::DeleteElement {
            element_id: floor_id,
        }],
    );
    project_dirty_document(&document, &mut cache);
    assert!(
        cache
            .spatial_bounds_query(&new_query)
            .unwrap()
            .element_ids
            .is_empty()
    );

    document.undo().unwrap().expect("delete should be undoable");
    project_dirty_document(&document, &mut cache);
    assert_eq!(
        cache.spatial_bounds_query(&new_query).unwrap().element_ids,
        vec![floor_id]
    );

    let loaded = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
    let mut loaded_cache = GeometryCache::new();
    project_dirty_document(&loaded, &mut loaded_cache);
    assert_eq!(
        loaded_cache
            .spatial_bounds_query(&new_query)
            .unwrap()
            .element_ids,
        vec![floor_id]
    );
}
