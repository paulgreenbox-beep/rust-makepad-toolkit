use crate::*;
use serde_json::json;
use std::f64::consts::{PI, TAU};

fn line() -> EntityGeometry {
    EntityGeometry::Line {
        start: Point2::new(-2., 0.),
        end: Point2::new(2., 0.),
    }
}
fn circle() -> EntityGeometry {
    EntityGeometry::Circle {
        center: Point2::default(),
        radius: 1.,
    }
}
fn create(doc: &mut LabDocument, geometry: EntityGeometry) -> DraftEntityId {
    let id = format!("create-{}", doc.revision());
    let r = doc
        .apply(CommandEnvelope {
            command_id: id,
            expected_revision: doc.revision(),
            operations: vec![DraftOperation::Create {
                drawing_id: DrawingId::new(1),
                layer_id: LayerId::new(1),
                geometry,
                style: DraftStyle::default(),
            }],
        })
        .unwrap();
    r.created_ids[0]
}
fn apply(
    doc: &mut LabDocument,
    id: &str,
    operations: Vec<DraftOperation>,
) -> DraftResult<CommandReceipt> {
    doc.apply(CommandEnvelope {
        command_id: id.into(),
        expected_revision: doc.revision(),
        operations,
    })
}
fn imported(handle: &str) -> ImportBatch {
    ImportBatch {
        drawing_id: DrawingId::new(1),
        source: ImportSourceSpec {
            name: "fixture.dxf".into(),
            sha256: "a".repeat(64),
            unit_resolution: UnitResolution {
                declared_unit: Some("millimetre".into()),
                resolved_unit: "millimetre".into(),
                scale_to_m: 0.001,
                decision_source: "header".into(),
            },
            report: json!({"imported_count":1}),
        },
        layers: vec![ImportLayer {
            name: "Layer0".into(),
            visible: true,
            locked: false,
            style: DraftStyle::default(),
        }],
        entities: vec![ImportEntity {
            geometry: line(),
            layer_name: "Layer0".into(),
            style: DraftStyle::default(),
            source_handle: Some(handle.into()),
        }],
    }
}
fn close(a: f64, b: f64, eps: f64) {
    assert!((a - b).abs() <= eps, "{a} != {b}");
}

#[test]
fn analytic_geometry_is_preserved_in_json() {
    let mut doc = LabDocument::new();
    for geometry in [
        line(),
        circle(),
        EntityGeometry::Arc {
            center: Point2::new(3., 4.),
            radius: 2.,
            start_angle: 0.2,
            sweep_angle: -PI,
        },
        EntityGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(Point2::new(0., 0.), 0.5),
                PolylineVertex::new(Point2::new(2., 0.), -0.25),
                PolylineVertex::new(Point2::new(2., 2.), 1.),
            ],
            closed: true,
        },
    ] {
        create(&mut doc, geometry);
    }
    let opened = LabDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(doc, opened);
    assert_eq!(opened.history_len(), 4);
}
#[test]
fn positive_negative_and_major_bulges_have_expected_arcs() {
    for bulge in [1., -1., 2., -2., 0.01] {
        let arc = bulge_arc(Point2::new(-1., 0.), Point2::new(1., 0.), bulge)
            .unwrap()
            .unwrap();
        close(arc.sweep_angle, 4. * bulge.atan(), 1e-12);
        close(arc.point(0.).distance(Point2::new(-1., 0.)), 0., 1e-12);
        close(arc.point(1.).distance(Point2::new(1., 0.)), 0., 1e-12);
    }
    let positive = bulge_arc(Point2::new(-1., 0.), Point2::new(1., 0.), 1.)
        .unwrap()
        .unwrap();
    let negative = bulge_arc(Point2::new(-1., 0.), Point2::new(1., 0.), -1.)
        .unwrap()
        .unwrap();
    assert!(positive.point(0.5).y < 0.);
    assert!(negative.point(0.5).y > 0.);
}
#[test]
fn closed_polyline_uses_last_vertex_bulge() {
    let geometry = EntityGeometry::Polyline {
        vertices: vec![
            PolylineVertex::new(Point2::new(-1., 0.), 0.),
            PolylineVertex::new(Point2::new(1., 0.), 0.),
            PolylineVertex::new(Point2::new(0., 1.), 1.),
        ],
        closed: true,
    };
    geometry.validate(GeometryLimits::default()).unwrap();
    let last = bulge_arc(Point2::new(0., 1.), Point2::new(-1., 0.), 1.)
        .unwrap()
        .unwrap();
    close(geometry.distance(last.point(0.5)).unwrap(), 0., 1e-12);
    let b = geometry.bounds().unwrap();
    assert!(b.max.y >= 1.);
}
#[test]
fn invalid_geometry_is_rejected_without_mutation() {
    let invalid = [
        EntityGeometry::Line {
            start: Point2::default(),
            end: Point2::default(),
        },
        EntityGeometry::Circle {
            center: Point2::default(),
            radius: 0.,
        },
        EntityGeometry::Arc {
            center: Point2::default(),
            radius: 1.,
            start_angle: 0.,
            sweep_angle: 0.,
        },
        EntityGeometry::Arc {
            center: Point2::default(),
            radius: 1.,
            start_angle: 0.,
            sweep_angle: TAU + 1.,
        },
        EntityGeometry::Circle {
            center: Point2::new(f64::NAN, 0.),
            radius: 1.,
        },
        EntityGeometry::Circle {
            center: Point2::default(),
            radius: f64::INFINITY,
        },
        EntityGeometry::Polyline {
            vertices: vec![PolylineVertex::new(Point2::default(), 0.)],
            closed: false,
        },
    ];
    let mut doc = LabDocument::new();
    let before = doc.to_json().unwrap();
    for geometry in invalid {
        assert!(
            apply(
                &mut doc,
                "bad",
                vec![DraftOperation::Create {
                    drawing_id: DrawingId::new(1),
                    layer_id: LayerId::new(1),
                    geometry,
                    style: DraftStyle::default()
                }]
            )
            .is_err()
        );
        assert_eq!(doc.to_json().unwrap(), before);
    }
}
#[test]
fn failed_batch_rolls_back_allocators_and_history() {
    let mut doc = LabDocument::new();
    let before = doc.clone();
    let result = apply(
        &mut doc,
        "atomic",
        vec![
            DraftOperation::Create {
                drawing_id: DrawingId::new(1),
                layer_id: LayerId::new(1),
                geometry: line(),
                style: DraftStyle::default(),
            },
            DraftOperation::Delete {
                drawing_id: DrawingId::new(1),
                ids: vec![DraftEntityId::new(999)],
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(doc, before);
    assert_eq!(create(&mut doc, line()), DraftEntityId::new(1));
}
#[test]
fn command_replay_is_idempotent_before_revision_gate() {
    let mut doc = LabDocument::new();
    let command = CommandEnvelope {
        command_id: "same".into(),
        expected_revision: 0,
        operations: vec![DraftOperation::Create {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            geometry: line(),
            style: DraftStyle::default(),
        }],
    };
    let first = doc.apply(command.clone()).unwrap();
    let repeated = doc.apply(command.clone()).unwrap();
    assert!(repeated.replayed);
    assert_eq!(first.created_ids, repeated.created_ids);
    assert_eq!(doc.revision(), 1);
    assert_eq!(doc.history_len(), 1);
    let mut conflict = command;
    conflict.operations[0] = DraftOperation::Create {
        drawing_id: DrawingId::new(1),
        layer_id: LayerId::new(1),
        geometry: circle(),
        style: DraftStyle::default(),
    };
    assert_eq!(doc.apply(conflict).unwrap_err().code, "command_id_conflict");
}
#[test]
fn stale_revision_rejects_before_allocation() {
    let mut doc = LabDocument::new();
    create(&mut doc, line());
    let before = doc.clone();
    let err = doc
        .apply(CommandEnvelope {
            command_id: "stale".into(),
            expected_revision: 0,
            operations: vec![DraftOperation::Create {
                drawing_id: DrawingId::new(1),
                layer_id: LayerId::new(1),
                geometry: circle(),
                style: DraftStyle::default(),
            }],
        })
        .unwrap_err();
    assert_eq!(err.code, "stale_revision");
    assert_eq!(doc, before);
}
#[test]
fn undo_create_then_new_create_never_reuses_id() {
    let mut doc = LabDocument::new();
    let first = create(&mut doc, line());
    doc.undo("undo", 1).unwrap();
    let second = create(&mut doc, circle());
    assert_eq!(first.get(), 1);
    assert_eq!(second.get(), 2);
    assert!(!doc.can_redo());
    assert_eq!(doc.allocators().entity_high_water, 2);
    doc.validate().unwrap();
}
#[test]
fn move_edit_delete_undo_redo_preserve_entity_identity() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, line());
    apply(
        &mut doc,
        "move",
        vec![DraftOperation::Move {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
            delta: Point2::new(7., 3.),
        }],
    )
    .unwrap();
    let moved = doc
        .snapshot(DrawingId::new(1))
        .unwrap()
        .entity(id)
        .unwrap()
        .geometry
        .clone();
    doc.undo("undo-move", 2).unwrap();
    assert_eq!(
        doc.snapshot(DrawingId::new(1))
            .unwrap()
            .entity(id)
            .unwrap()
            .geometry,
        line()
    );
    doc.redo("redo-move", 3).unwrap();
    assert_eq!(
        doc.snapshot(DrawingId::new(1))
            .unwrap()
            .entity(id)
            .unwrap()
            .geometry,
        moved
    );
    apply(
        &mut doc,
        "edit",
        vec![DraftOperation::Edit {
            drawing_id: DrawingId::new(1),
            id,
            geometry: circle(),
        }],
    )
    .unwrap();
    apply(
        &mut doc,
        "delete",
        vec![DraftOperation::Delete {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
        }],
    )
    .unwrap();
    doc.undo("undo-delete", 6).unwrap();
    assert_eq!(
        doc.snapshot(DrawingId::new(1))
            .unwrap()
            .entity(id)
            .unwrap()
            .geometry,
        circle()
    );
    assert_eq!(doc.allocators().entity_high_water, 1);
    doc.validate().unwrap();
}
#[test]
fn reopening_retains_redo_and_idempotency_receipts() {
    let mut doc = LabDocument::new();
    create(&mut doc, line());
    doc.undo("undo", 1).unwrap();
    let mut opened = LabDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert!(opened.can_redo());
    assert!(opened.undo("undo", 1).unwrap().replayed);
    opened.redo("redo", 2).unwrap();
    assert_eq!(
        opened.snapshot(DrawingId::new(1)).unwrap().entities.len(),
        1
    );
    opened.validate().unwrap();
}
#[test]
fn ids_above_js_integer_limit_are_decimal_strings() {
    let mut doc = LabDocument::new();
    doc.state.next_ids.entity_high_water = 9_007_199_254_740_993;
    let id = create(&mut doc, line());
    let encoded = serde_json::to_string(&id).unwrap();
    assert_eq!(encoded, "\"9007199254740994\"");
    let reopened = LabDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(
        reopened
            .snapshot(DrawingId::new(1))
            .unwrap()
            .entity(id)
            .unwrap()
            .id,
        id
    );
    assert!(serde_json::from_str::<DraftEntityId>("9007199254740994").is_err());
    assert!(serde_json::from_str::<DraftEntityId>("\"01\"").is_err());
}
#[test]
fn allocator_exhaustion_is_structured_and_atomic() {
    let mut doc = LabDocument::new();
    doc.state.next_ids.entity_high_water = u64::MAX;
    let before = doc.clone();
    let err = apply(
        &mut doc,
        "overflow",
        vec![DraftOperation::Create {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            geometry: line(),
            style: DraftStyle::default(),
        }],
    )
    .unwrap_err();
    assert_eq!(err.code, "id_exhausted");
    assert_eq!(doc, before);
}
#[test]
fn last_u64_id_is_allocated_once_and_retained_after_undo() {
    let mut doc = LabDocument::new();
    doc.state.next_ids.entity_high_water = u64::MAX - 1;
    let id = create(&mut doc, line());
    assert_eq!(id.get(), u64::MAX);
    doc.undo("undo-max", 1).unwrap();
    let opened = LabDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(opened.allocators().entity_high_water, u64::MAX);
}
#[test]
fn duplicate_import_handles_abort_entire_import() {
    let mut doc = LabDocument::new();
    let mut batch = imported("a1");
    batch.entities.push(ImportEntity {
        source_handle: Some("A1".into()),
        ..batch.entities[0].clone()
    });
    let before = doc.clone();
    assert_eq!(
        apply(&mut doc, "dup", vec![DraftOperation::Import { batch }])
            .unwrap_err()
            .code,
        "duplicate_source_handle"
    );
    assert_eq!(doc, before);
}
#[test]
fn same_external_handle_in_two_import_assets_is_not_merged() {
    let mut doc = LabDocument::new();
    let a = apply(
        &mut doc,
        "import-a",
        vec![DraftOperation::Import {
            batch: imported("A1"),
        }],
    )
    .unwrap();
    let b = apply(
        &mut doc,
        "import-b",
        vec![DraftOperation::Import {
            batch: imported("A1"),
        }],
    )
    .unwrap();
    assert_ne!(a.created_ids, b.created_ids);
    assert_ne!(a.import_asset_ids, b.import_asset_ids);
    let opened = LabDocument::from_json(&doc.to_json().unwrap()).unwrap();
    assert_eq!(
        opened.snapshot(DrawingId::new(1)).unwrap().entities.len(),
        2
    );
}
#[test]
fn import_undo_redo_is_one_atomic_history_entry() {
    let mut doc = LabDocument::new();
    let mut batch = imported("A1");
    batch.entities.push(ImportEntity {
        source_handle: Some("A2".into()),
        geometry: circle(),
        ..batch.entities[0].clone()
    });
    let receipt = apply(&mut doc, "import", vec![DraftOperation::Import { batch }]).unwrap();
    assert_eq!(receipt.created_ids.len(), 2);
    assert_eq!(doc.history_len(), 1);
    doc.undo("undo-import", 1).unwrap();
    let snap = doc.snapshot(DrawingId::new(1)).unwrap();
    assert!(snap.entities.is_empty() && snap.import_sources.is_empty());
    doc.redo("redo-import", 2).unwrap();
    assert_eq!(
        doc.snapshot(DrawingId::new(1))
            .unwrap()
            .entities
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        receipt.created_ids
    );
    doc.validate().unwrap();
}
#[test]
fn explicit_unit_resolution_is_required() {
    let mut batch = imported("A1");
    batch.source.unit_resolution.resolved_unit.clear();
    let mut doc = LabDocument::new();
    assert!(
        apply(
            &mut doc,
            "unresolved",
            vec![DraftOperation::Import { batch }]
        )
        .is_err()
    );
    assert_eq!(doc.revision(), 0);
    for (unit, scale) in [("metre", 1.), ("millimetre", 0.001), ("inch", 0.0254)] {
        UnitResolution {
            declared_unit: Some(unit.into()),
            resolved_unit: unit.into(),
            scale_to_m: scale,
            decision_source: "header".into(),
        }
        .validate()
        .unwrap();
        close((1. / scale) * scale, 1., 1e-12);
    }
}
#[test]
fn new_drawing_and_layer_ids_survive_undo_without_reuse() {
    let mut doc = LabDocument::new();
    let r = apply(
        &mut doc,
        "drawing",
        vec![DraftOperation::CreateDrawing {
            name: "Second".into(),
            plane: DrawingPlane::default(),
        }],
    )
    .unwrap();
    assert_eq!(r.created_drawing_ids, [DrawingId::new(2)]);
    doc.undo("undo-drawing", 1).unwrap();
    let r = apply(
        &mut doc,
        "drawing-three",
        vec![DraftOperation::CreateDrawing {
            name: "Third".into(),
            plane: DrawingPlane::default(),
        }],
    )
    .unwrap();
    assert_eq!(r.created_drawing_ids, [DrawingId::new(3)]);
    assert_eq!(doc.allocators().layer_high_water, 3);
    doc.validate().unwrap();
}
#[test]
fn hidden_layers_are_not_picked_and_locked_layers_are_inspectable() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, line());
    let camera = Camera2D::new(800., 600.);
    apply(
        &mut doc,
        "hide",
        vec![DraftOperation::SetLayer {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            visible: Some(false),
            locked: None,
            style: None,
        }],
    )
    .unwrap();
    assert!(
        pick(
            &doc.snapshot(DrawingId::new(1)).unwrap(),
            &camera,
            Point2::new(400., 300.),
            5.
        )
        .unwrap()
        .is_empty()
    );
    apply(
        &mut doc,
        "lock",
        vec![DraftOperation::SetLayer {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            visible: Some(true),
            locked: Some(true),
            style: None,
        }],
    )
    .unwrap();
    let hits = pick(
        &doc.snapshot(DrawingId::new(1)).unwrap(),
        &camera,
        Point2::new(400., 300.),
        5.,
    )
    .unwrap();
    assert_eq!(hits[0].entity_id, id);
    assert!(hits[0].locked);
    let before = doc.clone();
    assert_eq!(
        apply(
            &mut doc,
            "locked-move",
            vec![DraftOperation::Move {
                drawing_id: DrawingId::new(1),
                ids: vec![id],
                delta: Point2::new(1., 0.)
            }]
        )
        .unwrap_err()
        .code,
        "locked_layer"
    );
    assert_eq!(doc, before);
}
#[test]
fn intersecting_pick_candidates_are_sorted_by_distance_and_id() {
    let mut doc = LabDocument::new();
    let a = create(&mut doc, line());
    let b = create(
        &mut doc,
        EntityGeometry::Line {
            start: Point2::new(0., -2.),
            end: Point2::new(0., 2.),
        },
    );
    let camera = Camera2D::new(800., 600.);
    let snap = doc.snapshot(DrawingId::new(1)).unwrap();
    let hits = pick(&snap, &camera, Point2::new(400., 300.), 7.).unwrap();
    assert_eq!(hits.iter().map(|h| h.entity_id).collect::<Vec<_>>(), [a, b]);
    let hits = pick(&snap, &camera, Point2::new(402., 300.), 7.).unwrap();
    assert_eq!(hits[0].entity_id, a);
    assert_eq!(hits[1].distance_px, 2.);
}
#[test]
fn analytic_circle_arc_and_bulge_picking_matches_curves() {
    let mut doc = LabDocument::new();
    let c = create(&mut doc, circle());
    let a = create(
        &mut doc,
        EntityGeometry::Arc {
            center: Point2::new(4., 0.),
            radius: 1.,
            start_angle: 0.,
            sweep_angle: PI,
        },
    );
    let p = create(
        &mut doc,
        EntityGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(Point2::new(-5., 0.), 1.),
                PolylineVertex::new(Point2::new(-3., 0.), 0.),
            ],
            closed: false,
        },
    );
    let camera = Camera2D::new(1000., 600.);
    let s = doc.snapshot(DrawingId::new(1)).unwrap();
    for (point, id) in [
        (Point2::new(1., 0.), c),
        (Point2::new(4., 1.), a),
        (Point2::new(-4., -1.), p),
    ] {
        let hits = pick(&s, &camera, camera.world_to_screen(point), 0.01).unwrap();
        assert_eq!(hits[0].entity_id, id);
    }
    assert!(
        pick(
            &s,
            &camera,
            camera.world_to_screen(Point2::new(4., -1.)),
            0.01
        )
        .unwrap()
        .is_empty()
    );
}
#[test]
fn pick_aperture_remains_in_pixels_at_tiny_and_large_zoom() {
    let mut doc = LabDocument::new();
    create(&mut doc, line());
    let snap = doc.snapshot(DrawingId::new(1)).unwrap();
    for ppm in [1e-6, 1., 1e9] {
        let mut camera = Camera2D::new(800., 600.);
        camera.pixels_per_metre = ppm;
        assert_eq!(
            pick(&snap, &camera, Point2::new(400., 304.), 5.)
                .unwrap()
                .len(),
            1
        );
        assert!(
            pick(&snap, &camera, Point2::new(400., 306.), 5.)
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn camera_pan_follows_drag_in_both_axes() {
    let mut camera = Camera2D::new(800., 600.);
    let p = Point2::new(2., 3.);
    let before = camera.world_to_screen(p);
    camera.pan_pixels(25., -18.).unwrap();
    let after = camera.world_to_screen(p);
    close(after.x - before.x, 25., 1e-12);
    close(after.y - before.y, -18., 1e-12);
}
#[test]
fn camera_zoom_keeps_world_point_under_cursor() {
    let mut camera = Camera2D::new(800., 600.);
    camera.center = Point2::new(23., -7.);
    let screen = Point2::new(125., 413.);
    let world = camera.screen_to_world(screen);
    camera.zoom_at(screen, 2.5).unwrap();
    close(camera.screen_to_world(screen).distance(world), 0., 1e-12);
}
#[test]
fn camera_fit_resize_and_plane_roundtrip_are_bounded() {
    let mut camera = Camera2D::new(800., 600.);
    let bounds = Bounds2::from_points(Point2::new(1., 2.), Point2::new(11., 7.));
    camera.fit_bounds(bounds, 20.).unwrap();
    let a = camera.world_to_screen(bounds.min);
    let b = camera.world_to_screen(bounds.max);
    assert!(a.x >= 19.999 && b.x <= 780.001 && b.y >= 19.999 && a.y <= 580.001);
    let center = camera.center;
    camera.resize(1200., 900.).unwrap();
    assert_eq!(camera.center, center);
    assert!(camera.resize(0., 10.).is_err());
    let plane = DrawingPlane {
        origin: Point3::new(1e9, 1e9, 12.),
        u_axis: Point3::new(0., 1., 0.),
        v_axis: Point3::new(0., 0., 1.),
    };
    plane.validate().unwrap();
    let point = Point2::new(0.25, 2.);
    assert_eq!(plane.world_to_local(plane.local_to_world(point)), point);
}
#[test]
fn large_coordinates_are_rebased_before_gpu_cast() {
    let geometry = EntityGeometry::Line {
        start: Point2::new(1e9, 1e9),
        end: Point2::new(1e9 + 0.25, 1e9),
    };
    let mut camera = Camera2D::new(800., 600.);
    camera.center = Point2::new(1e9, 1e9);
    camera.pixels_per_metre = 100.;
    let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(segments[0].start, Point2::new(400., 300.));
    assert_eq!(segments[0].end, Point2::new(425., 300.));
    assert_eq!(segments[0].end.x as f32 - segments[0].start.x as f32, 25.);
}
#[test]
fn tessellation_respects_physical_pixel_error_and_dpi() {
    for ppm in [5., 100., 1000.] {
        for dpi in [1., 2.] {
            let mut camera = Camera2D::new(10000., 10000.);
            camera.pixels_per_metre = ppm;
            let options = DisplayOptions {
                dpi_factor: dpi,
                ..Default::default()
            };
            let segments = tessellate_geometry(&circle(), &camera, &options).unwrap();
            let n = segments.len();
            assert!(n >= 4);
            let error = ppm * dpi * (1. - (PI / n as f64).cos());
            assert!(error <= options.pixel_error + 1e-10);
        }
    }
}
#[test]
fn tessellation_changes_with_zoom_and_rejects_budget_exhaustion() {
    let mut camera = Camera2D::new(3000., 3000.);
    camera.pixels_per_metre = 10.;
    let coarse = tessellate_geometry(&circle(), &camera, &DisplayOptions::default())
        .unwrap()
        .len();
    camera.pixels_per_metre = 1000.;
    let fine = tessellate_geometry(&circle(), &camera, &DisplayOptions::default())
        .unwrap()
        .len();
    assert!(fine > coarse);
    let options = DisplayOptions {
        max_segments_per_curve: 8,
        ..Default::default()
    };
    assert_eq!(
        tessellate_geometry(&circle(), &camera, &options)
            .unwrap_err()
            .code,
        "tessellation_budget"
    );
}
#[test]
fn screen_segments_are_clipped_to_canvas() {
    let camera = Camera2D::new(800., 600.);
    let geometry = EntityGeometry::Line {
        start: Point2::new(-100., -100.),
        end: Point2::new(100., 100.),
    };
    let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(segments.len(), 1);
    for p in [segments[0].start, segments[0].end] {
        assert!((0.0..=800.).contains(&p.x) && (0.0..=600.).contains(&p.y));
    }
}
#[test]
fn display_owner_mapping_and_tokens_follow_revision_camera_and_selection() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, circle());
    let camera = Camera2D::new(800., 600.);
    let snap = doc.snapshot(DrawingId::new(1)).unwrap();
    let options = DisplayOptions::default();
    let display = build_display(&snap, &camera, &options).unwrap();
    assert!(display.primitives.iter().all(|p| p.owner_entity_id == id));
    assert_eq!(display.analytic_entities[0].geometry, circle());
    display.require_current(&snap, &camera, &options).unwrap();
    let selected = DisplayOptions {
        selected_ids: vec![id],
        ..options.clone()
    };
    assert!(!display.is_current(&snap, &camera, &selected).unwrap());
    let mut camera2 = camera.clone();
    camera2.pan_pixels(1., 0.).unwrap();
    assert!(!display.is_current(&snap, &camera2, &options).unwrap());
    apply(
        &mut doc,
        "move-token",
        vec![DraftOperation::Move {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
            delta: Point2::new(1., 0.),
        }],
    )
    .unwrap();
    assert_eq!(
        display
            .require_current(&doc.snapshot(DrawingId::new(1)).unwrap(), &camera, &options)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
}
#[test]
fn transient_preview_and_camera_do_not_modify_authority() {
    let doc = LabDocument::new();
    let before = doc.to_json().unwrap();
    let mut camera = Camera2D::new(800., 600.);
    tessellate_geometry(&circle(), &camera, &DisplayOptions::default()).unwrap();
    camera.pan_pixels(15., 18.).unwrap();
    camera.zoom_at(Point2::new(200., 250.), 1.25).unwrap();
    tessellate_geometry(&line(), &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(doc.to_json().unwrap(), before);
    assert_eq!(doc.history_len(), 0);
    assert_eq!(doc.revision(), 0);
}
#[test]
fn malformed_load_rejects_schema_counters_and_history_geometry() {
    let mut doc = LabDocument::new();
    create(&mut doc, circle());
    let original = serde_json::to_value(&doc).unwrap();
    for (key, value) in [("schema_version", json!(2))] {
        let mut bad = original.clone();
        bad[key] = value;
        assert!(LabDocument::from_json(&bad.to_string()).is_err());
    }
    let mut bad = original.clone();
    bad["state"]["next_ids"]["entity_high_water"] = json!("0");
    assert!(LabDocument::from_json(&bad.to_string()).is_err());
    let mut bad = original.clone();
    bad["history"][0]["changes"][0]["after"]["geometry"]["radius"] = json!(-1.);
    assert!(LabDocument::from_json(&bad.to_string()).is_err());
    let mut bad = original.clone();
    bad["state"]["drawings"]["1"]["entities"]["1"]["geometry"]["radius"] = json!(2.);
    assert_eq!(
        LabDocument::from_json(&bad.to_string()).unwrap_err().code,
        "history_state_mismatch"
    );
}
#[test]
fn duplicate_json_entity_keys_are_rejected() {
    let mut doc = LabDocument::new();
    create(&mut doc, line());
    let value = serde_json::to_value(&doc).unwrap();
    let entity = value["state"]["drawings"]["1"]["entities"]["1"].to_string();
    let compact = value.to_string();
    let needle = format!("\"entities\":{{\"1\":{entity}}}");
    let replacement = format!("\"entities\":{{\"1\":{entity},\"1\":{entity}}}");
    assert!(compact.contains(&needle));
    let malformed = compact.replacen(&needle, &replacement, 1);
    assert!(
        LabDocument::from_json(&malformed)
            .unwrap_err()
            .message
            .contains("duplicate persistent map key")
    );
}
#[test]
fn load_validates_historical_geometry_even_when_deleted_in_same_batch() {
    let mut doc = LabDocument::new();
    apply(
        &mut doc,
        "create-delete",
        vec![
            DraftOperation::Create {
                drawing_id: DrawingId::new(1),
                layer_id: LayerId::new(1),
                geometry: circle(),
                style: DraftStyle::default(),
            },
            DraftOperation::Delete {
                drawing_id: DrawingId::new(1),
                ids: vec![DraftEntityId::new(1)],
            },
        ],
    )
    .unwrap();
    let mut bad = serde_json::to_value(&doc).unwrap();
    bad["history"][0]["changes"][0]["after"]["geometry"]["radius"] = json!(-1.);
    assert!(LabDocument::from_json(&bad.to_string()).is_err());
}
#[test]
fn filesystem_save_reopen_preserves_history_and_precise_f64() {
    let path = std::env::temp_dir().join(format!("drafting-core-{}.json", uuid::Uuid::new_v4()));
    let mut doc = LabDocument::new();
    create(
        &mut doc,
        EntityGeometry::Circle {
            center: Point2::new(0.12345678901234567, 1e9 + 0.125),
            radius: 0.012345678901234567,
        },
    );
    doc.save_json(&path).unwrap();
    let mut reopened = LabDocument::load_json(&path).unwrap();
    assert_eq!(doc, reopened);
    reopened.undo("new-process-undo", 1).unwrap();
    reopened.save_json(&path).unwrap();
    let reopened_again = LabDocument::load_json(&path).unwrap();
    assert!(reopened_again.can_redo());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn geometry_and_string_budgets_fail_closed() {
    let vertices = (0..4097)
        .map(|i| PolylineVertex::new(Point2::new(i as f64, 0.), 0.))
        .collect();
    assert_eq!(
        EntityGeometry::Polyline {
            vertices,
            closed: false
        }
        .validate(GeometryLimits::default())
        .unwrap_err()
        .code,
        "geometry_budget"
    );
    let mut batch = imported("A");
    batch.source.name = "x".repeat(513);
    assert!(batch.source.validate().is_err());
    let mut camera = Camera2D::new(800., 600.);
    let before = camera.clone();
    assert!(camera.zoom_at(Point2::default(), f64::NAN).is_err());
    assert_eq!(camera, before);
}
#[test]
fn clipping_does_not_bypass_tessellation_work_budget() {
    let geometry = EntityGeometry::Polyline {
        vertices: (0..50)
            .map(|i| PolylineVertex::new(Point2::new(100. + i as f64 * 3., 100.), 1.))
            .collect(),
        closed: false,
    };
    let camera = Camera2D::new(800., 600.);
    let options = DisplayOptions {
        max_primitives: 64,
        ..Default::default()
    };
    assert_eq!(
        tessellate_geometry(&geometry, &camera, &options)
            .unwrap_err()
            .code,
        "display_budget"
    );
}
#[test]
fn stale_spatial_index_is_rejected_after_entity_move() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, line());
    let snap = doc.snapshot(DrawingId::new(1)).unwrap();
    let index = SpatialIndex2D::build(&snap).unwrap();
    apply(
        &mut doc,
        "move-index",
        vec![DraftOperation::Move {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
            delta: Point2::new(10., 0.),
        }],
    )
    .unwrap();
    let err = pick_with_index(
        &doc.snapshot(DrawingId::new(1)).unwrap(),
        &index,
        &Camera2D::new(800., 600.),
        Point2::new(400., 300.),
        5.,
    )
    .unwrap_err();
    assert_eq!(err.code, "stale_pick_index");
}
#[test]
fn long_lines_keep_canvas_interval_at_extreme_zoom() {
    let geometry = EntityGeometry::Line {
        start: Point2::new(-1e9, -1e9),
        end: Point2::new(1e9, 1e9),
    };
    let mut camera = Camera2D::new(800., 600.);
    camera.pixels_per_metre = 1e12;
    let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(segments.len(), 1);
    close(segments[0].start.x, 100., 1e-7);
    close(segments[0].start.y, 600., 1e-7);
    close(segments[0].end.x, 700., 1e-7);
    close(segments[0].end.y, 0., 1e-7);
}

#[test]
fn discarded_redo_payloads_keep_idempotency_after_reopen() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, line());
    let old_move = CommandEnvelope {
        command_id: "discarded-move".into(),
        expected_revision: 1,
        operations: vec![DraftOperation::Move {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
            delta: Point2::new(2., 3.),
        }],
    };
    let move_receipt = doc.apply(old_move.clone()).unwrap();
    let undo_receipt = doc.undo("discarded-undo", 2).unwrap();
    create(&mut doc, circle());
    assert!(!doc.can_redo());
    let encoded = doc.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        value["receipts"]["discarded-move"]["action"]["action"],
        "pruned"
    );
    let mut reopened = LabDocument::from_json(&encoded).unwrap();
    let before = reopened.clone();
    let mut expected = move_receipt;
    expected.replayed = true;
    assert_eq!(reopened.apply(old_move.clone()).unwrap(), expected);
    let mut expected = undo_receipt;
    expected.replayed = true;
    assert_eq!(reopened.undo("discarded-undo", 2).unwrap(), expected);
    assert_eq!(reopened, before);
    let mut different = old_move;
    if let DraftOperation::Move { delta, .. } = &mut different.operations[0] {
        delta.x += 1.;
    }
    assert_eq!(
        reopened.apply(different).unwrap_err().code,
        "command_id_conflict"
    );
    assert_eq!(reopened, before);
}

#[test]
fn retained_apply_receipt_and_allocator_metadata_must_match_its_delta() {
    let mut doc = LabDocument::new();
    create(&mut doc, circle());
    let original = serde_json::to_value(&doc).unwrap();
    let mut wrong_receipt = original.clone();
    wrong_receipt["receipts"]["create-0"]["receipt"]["created_ids"] = json!([]);
    assert!(LabDocument::from_json(&wrong_receipt.to_string()).is_err());
    let mut wrong_allocator = original.clone();
    wrong_allocator["receipts"]["create-0"]["allocators_before"]["entity_high_water"] = json!("1");
    assert!(LabDocument::from_json(&wrong_allocator.to_string()).is_err());
    let mut wrong_space = original;
    wrong_space["receipts"]["create-0"]["action"]["operations"][0]["layer_id"] = json!("2");
    assert!(LabDocument::from_json(&wrong_space.to_string()).is_err());
}

#[test]
fn tiny_bulge_bounds_and_distance_preserve_signed_sagitta() {
    for bulge in [1e-12, -1e-12] {
        let curve = bulge_curve(Point2::new(0., 0.), Point2::new(1., 0.), bulge)
            .unwrap()
            .unwrap();
        let mid = Point2::new(0.5, -bulge * 0.5);
        close(curve.point(0.5).x, mid.x, 1e-15);
        close(curve.point(0.5).y, mid.y, 1e-25);
        let bounds = curve.bounds();
        close(bounds.min.y, mid.y.min(0.), 1e-25);
        close(bounds.max.y, mid.y.max(0.), 1e-25);
        close(
            curve.distance(Point2::new(0.5, 0.)),
            bulge.abs() * 0.5,
            1e-25,
        );
        close(curve.distance(mid), 0., 1e-25);
        close(curve.distance(Point2::new(1.2, 0.)), 0.2, 1e-15);
        let geometry = EntityGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(curve.start, bulge),
                PolylineVertex::new(curve.end, 0.),
            ],
            closed: false,
        };
        close(geometry.distance(mid).unwrap(), 0., 1e-25);
        assert_eq!(geometry.bounds().unwrap(), bounds);
    }
}

#[test]
fn repeated_polyline_edits_reject_before_large_journal_staging() {
    let mut doc = LabDocument::new();
    let id = create(
        &mut doc,
        EntityGeometry::Polyline {
            vertices: (0..4096)
                .map(|i| PolylineVertex::new(Point2::new(i as f64, 0.), 0.))
                .collect(),
            closed: false,
        },
    );
    let before = doc.clone();
    let operations = (0..500)
        .map(|_| DraftOperation::Move {
            drawing_id: DrawingId::new(1),
            ids: vec![id],
            delta: Point2::new(0., 1.),
        })
        .collect();
    assert_eq!(
        apply(&mut doc, "oversized-polyline-journal", operations)
            .unwrap_err()
            .code,
        "command_budget"
    );
    assert_eq!(doc, before);
}

#[test]
fn prospective_pretty_document_cap_rejects_before_publishing() {
    let mut doc = LabDocument::new();
    let before = doc.clone();
    let operations = (0..45)
        .map(|i| {
            let mut batch = imported(&format!("{i:X}"));
            batch.source.report = json!({"padding": "x".repeat(512 * 1024)});
            DraftOperation::Import { batch }
        })
        .collect();
    assert_eq!(
        apply(&mut doc, "oversized-persistent-imports", operations)
            .unwrap_err()
            .code,
        "document_byte_budget"
    );
    assert_eq!(doc, before);
    assert_eq!(
        LabDocument::from_json(&doc.to_json().unwrap()).unwrap(),
        before
    );
}

#[test]
fn replay_geometry_work_is_not_recomputed_from_a_later_larger_edit() {
    let mut doc = LabDocument::new();
    let id = create(&mut doc, line());
    let command = CommandEnvelope {
        command_id: "line-moves-before-edit".into(),
        expected_revision: 1,
        operations: (0..500)
            .map(|_| DraftOperation::Move {
                drawing_id: DrawingId::new(1),
                ids: vec![id],
                delta: Point2::new(0., 1.),
            })
            .collect(),
    };
    let mut expected = doc.apply(command.clone()).unwrap();
    apply(
        &mut doc,
        "edit-to-large-polyline",
        vec![DraftOperation::Edit {
            drawing_id: DrawingId::new(1),
            id,
            geometry: EntityGeometry::Polyline {
                vertices: (0..4096)
                    .map(|i| PolylineVertex::new(Point2::new(i as f64, 0.), 0.))
                    .collect(),
                closed: false,
            },
        }],
    )
    .unwrap();
    expected.replayed = true;
    let before = doc.clone();
    assert_eq!(doc.apply(command).unwrap(), expected);
    assert_eq!(doc, before);
}
