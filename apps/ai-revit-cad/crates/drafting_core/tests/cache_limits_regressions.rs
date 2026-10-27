use drafting_core::*;
use std::sync::Arc;

fn commit(doc: &mut LabDocument, name: &str, operations: Vec<DraftOperation>) -> CommandReceipt {
    let expected_revision = doc.revision();
    doc.apply(CommandEnvelope { command_id: name.into(), expected_revision, operations }).unwrap()
}

fn create_line(x: f64) -> DraftOperation {
    DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Line { start: Point2::new(x, 0.), end: Point2::new(x + 1., 0.) },
        style: DraftStyle::default(),
    }
}

fn cache(doc: &LabDocument, limits: DraftingCacheLimits) -> IncrementalDraftingCache {
    let mut result = IncrementalDraftingCache::new(limits).unwrap();
    result.reset(doc, doc.default_drawing_id(), 0).unwrap();
    result
}

#[test]
fn old_replayed_receipt_is_a_noop_after_newer_confirmed_commits() {
    let mut doc = LabDocument::new();
    let first = commit(&mut doc, "seed", vec![create_line(0.)]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let next = commit(&mut doc, "another", vec![create_line(2.)]);
    cache.sync_after_commit(&doc, &next, 0).unwrap();
    let frame = cache.prepare(&doc, 0, &Camera2D::default(), &DisplayOptions::default()).unwrap();
    let before = cache.performance();
    let replay = doc.apply(CommandEnvelope {
        command_id: "seed".into(), expected_revision: first.revision_before,
        operations: vec![create_line(0.)],
    }).unwrap();
    assert!(replay.replayed);
    assert!(cache.sync_after_commit(&doc, &replay, 0).unwrap().replayed);
    let current = cache.prepare(&doc, 0, &Camera2D::default(), &DisplayOptions::default()).unwrap();
    assert!(Arc::ptr_eq(&frame, &current));
    let after = cache.performance();
    assert_eq!(before.commits, after.commits);
    assert_eq!(before.geometry_clones, after.geometry_clones);
    assert_eq!(before.digest_leaf_updates, after.digest_leaf_updates);
    assert_eq!(before.digest_node_hashes, after.digest_node_hashes);
    assert_eq!(before.index_updates, after.index_updates);
    assert_eq!(before.replay_noops + 1, after.replay_noops);
}

#[test]
fn lru_limits_apply_even_when_a_frame_uses_more_meshes_than_the_lru_retains() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "seed", vec![create_line(0.), create_line(2.), create_line(4.)]);
    let limits = DraftingCacheLimits {
        max_tessellation_entries: 1, max_tessellation_bytes: 1024,
        ..DraftingCacheLimits::default()
    };
    let mut cache = cache(&doc, limits);
    let frame = cache.prepare(&doc, 0, &Camera2D::default(), &DisplayOptions::default()).unwrap();
    assert_eq!(frame.base.visible_entity_count, 3);
    assert_eq!(frame.base.generated_segments, 3);
    let metrics = cache.performance();
    assert!(metrics.tessellation_entries <= 1);
    assert!(metrics.tessellation_bytes <= 1024);
    assert!(metrics.tessellation_evictions >= 2);
}

#[test]
fn generated_work_budget_cannot_be_bypassed_by_clipping() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "long-polyline", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Polyline {
            vertices: [-10000., 0., 10000., 20000.].into_iter().map(|x| PolylineVertex {
                point: Point2::new(x, 0.), bulge: 0.,
            }).collect(), closed: false,
        }, style: DraftStyle::default(),
    }]);
    let limits = DraftingCacheLimits { max_generated_segments: 2, ..DraftingCacheLimits::default() };
    let mut cache = cache(&doc, limits);
    let before = cache.performance();
    assert!(cache.prepare(&doc, 0, &Camera2D::default(), &DisplayOptions::default()).is_err());
    let after = cache.performance();
    assert_eq!(after.frames_prepared, before.frames_prepared);
    assert_eq!(after.generations.resources, before.generations.resources);
    assert_eq!(cache.metadata(&doc, 0).unwrap().entity_count, 1);
}

#[test]
fn externally_retained_frame_base_blocks_growth_until_it_is_released() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "seed", vec![create_line(0.)]);
    let limits = DraftingCacheLimits { max_retained_frame_bases: 1, ..DraftingCacheLimits::default() };
    let mut cache = cache(&doc, limits);
    let mut camera = Camera2D::default();
    let frame = cache.prepare(&doc, 0, &camera, &DisplayOptions::default()).unwrap();
    camera.pan_pixels(5., 0.).unwrap();
    let before = cache.performance();
    assert!(cache.prepare(&doc, 0, &camera, &DisplayOptions::default()).is_err());
    assert_eq!(cache.performance().generations.resources, before.generations.resources);
    drop(frame);
    let current = cache.prepare(&doc, 0, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(current.camera, camera);
    assert!(cache.performance().retained_frame_bases <= 1);
}

#[test]
fn stroke_padding_reaches_projection_without_changing_analytic_pick_distance() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "wide-offscreen-line", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Line { start: Point2::new(-1.1, -0.5), end: Point2::new(-1.1, 0.5) },
        style: DraftStyle { width_px: 32., ..DraftStyle::default() },
    }]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let mut camera = Camera2D::new(200., 200.);
    camera.pixels_per_metre = 100.;
    let frame = cache.prepare(&doc, 0, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(frame.base.visible_entity_count, 1);
    assert_eq!(frame.base.primitives.len(), 1);
    assert!((frame.base.primitives[0].start.x + 10.).abs() < 1e-8);
    assert!(cache.pick(&doc, 0, &camera, Point2::new(0., 100.), 4.).unwrap().is_empty());
    assert_eq!(cache.pick(&doc, 0, &camera, Point2::new(0., 100.), 11.).unwrap().len(), 1);
}

#[test]
fn performance_reads_do_not_generate_frames_or_add_timing_samples() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "seed", vec![create_line(0.)]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    cache.prepare(&doc, 0, &Camera2D::default(), &DisplayOptions::default()).unwrap();
    let before = cache.performance();
    for _ in 0..8 {
        let read = cache.performance();
        assert_eq!(read.frames_prepared, before.frames_prepared);
        assert_eq!(read.frame_hits, before.frame_hits);
        assert_eq!(read.source_entity_visits, before.source_entity_visits);
        assert_eq!(read.prepare_cpu.samples, before.prepare_cpu.samples);
        assert_eq!(read.prepare_cpu.last_ms, before.prepare_cpu.last_ms);
    }
}

#[test]
fn outward_circle_bounds_keep_a_visible_micro_radius_at_a_large_origin() {
    let mut doc = LabDocument::new();
    let ulp = 2f64.powi(-23);
    commit(&mut doc, "micro-circle", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Circle { center: Point2::new(1e9, 0.), radius: 1.25 * ulp },
        style: DraftStyle::default(),
    }]);
    let camera = Camera2D {
        center: Point2::new(1e9 + 2. * ulp, 0.), pixels_per_metre: 5e9,
        viewport_width: 1000., viewport_height: 700., revision: 0,
    };
    let options = DisplayOptions::default();
    let legacy = build_display(&doc.snapshot(DrawingId::new(1)).unwrap(), &camera, &options).unwrap();
    assert!(!legacy.primitives.is_empty());
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let frame = cache.prepare(&doc, 0, &camera, &options).unwrap();
    assert_eq!(frame.base.visible_entity_count, 1);
    assert!(frame.base.primitives.iter().any(|p| p.start.x > 50. || p.end.x > 50.));
}

#[test]
fn tiny_arc_sagitta_and_far_center_rebasing_survive_high_zoom() {
    let arc = AnalyticArc {
        center: Point2::default(), radius: 1e12, start_angle: 0., sweep_angle: 1.5e-12,
    };
    let endpoint = arc.point_relative(Point2::new(1e12, 1.5), 1.);
    assert!((endpoint.x + 1.125e-12).abs() < 1e-24);
    let far_arc = AnalyticArc { center: Point2::new(-1e12, 0.), ..arc };
    let start = far_arc.point_relative(Point2::new(1e-10, 0.), 0.);
    assert!((start.x + 1e-10).abs() < 1e-24);

    let mut doc = LabDocument::new();
    let supported_radius = arc.radius;
    commit(&mut doc, "tiny-arc", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Arc {
            center: arc.center, radius: supported_radius, start_angle: arc.start_angle,
            sweep_angle: arc.sweep_angle,
        }, style: DraftStyle::default(),
    }]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let camera = Camera2D {
        center: Point2::new(supported_radius, supported_radius * arc.sweep_angle), pixels_per_metre: 1e12,
        viewport_width: 1000., viewport_height: 700., revision: 0,
    };
    let options = DisplayOptions { pixel_error: 0.01, ..DisplayOptions::default() };
    let frame = cache.prepare(&doc, 0, &camera, &options).unwrap();
    assert!(frame.base.primitives.iter().any(|p| {
        (p.end.x - 498.875).abs() < options.pixel_error && (p.end.y - 350.).abs() < options.pixel_error
    }));
}

#[test]
fn captured_export_uses_the_exact_cached_vertices_and_overlay_appearance() {
    let mut doc = LabDocument::new();
    let receipt = commit(&mut doc, "circle", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Circle { center: Point2::default(), radius: 2. },
        style: DraftStyle::default(),
    }]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let options = DisplayOptions { selected_ids: receipt.created_ids.clone(), ..DisplayOptions::default() };
    let frame = cache.prepare(&doc, 0, &Camera2D::default(), &options).unwrap();
    let export = cache.materialize_export(&doc, 0, &frame).unwrap();
    assert_eq!(export.display.token, frame.token);
    assert_eq!(export.display.primitives.len(), frame.base.primitives.len());
    for (owned, cached) in export.display.primitives.iter().zip(frame.base.primitives.iter()) {
        assert_eq!(owned.primitive_id, cached.primitive_id);
        assert_eq!(owned.owner_entity_id, cached.owner_entity_id);
        assert_eq!(owned.start, cached.start);
        assert_eq!(owned.end, cached.end);
        assert_eq!(owned.source_revision, frame.source.drawing_revision);
        assert!(owned.selected);
        assert_eq!(owned.appearance.width_px, cached.appearance.width_px + 1.);
        assert_eq!(owned.appearance.color, [1., 0.74, 0.22, 1.]);
    }
}

#[test]
fn json_reloads_keep_persistent_equality_but_replace_runtime_identity() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "seed", vec![create_line(0.)]);
    let text = doc.to_json().unwrap();
    assert!(!text.contains("runtime_instance_id"));
    let reload = LabDocument::from_json(&text).unwrap();
    assert_eq!(doc, reload);
    assert_ne!(doc.runtime_instance_id(), reload.runtime_instance_id());
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let old_hash = cache.metadata(&doc, 0).unwrap().source_sha256.clone();
    assert!(cache.metadata(&reload, 0).is_err());
    cache.reset(&reload, reload.default_drawing_id(), 0).unwrap();
    assert_eq!(old_hash, cache.metadata(&reload, 0).unwrap().source_sha256);
}

#[test]
fn rejected_overlapping_curves_stop_building_when_the_running_budget_is_used() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "overlapping-circles", (0..256).map(|_| DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Circle { center: Point2::default(), radius: 1. },
        style: DraftStyle::default(),
    }).collect());
    let limits = DraftingCacheLimits { max_generated_segments: 4096, ..DraftingCacheLimits::default() };
    let mut cache = cache(&doc, limits);
    let mut camera = Camera2D::default();
    camera.pixels_per_metre = 1e6;
    assert!(cache.prepare(&doc, 0, &camera, &DisplayOptions::default()).is_err());
    let metrics = cache.performance();
    // A bucket pass and an exact-scale retry may each build one mesh and fail
    // before allocating the second; later candidates must never be sampled.
    assert!(metrics.tessellation_misses <= 4);
    assert!(metrics.tessellation_entries <= 2);
    assert!(metrics.tessellation_bytes < 512 * 1024);
    assert_eq!(metrics.frames_prepared, 0);
    assert_eq!(metrics.retained_frame_bases, 0);
}

#[test]
fn precise_line_polyline_and_arc_pick_do_not_hit_a_distant_curve() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "far-lines", vec![
        DraftOperation::Create {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            geometry: EntityGeometry::Line { start: Point2::new(-1e12, -1e12), end: Point2::new(1e12, 1e12) },
            style: DraftStyle::default(),
        },
        DraftOperation::Create {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            geometry: EntityGeometry::Polyline {
                vertices: vec![PolylineVertex::new(Point2::new(-1e12, -1e12), 0.), PolylineVertex::new(Point2::new(1e12, 1e12), 0.)],
                closed: false,
            }, style: DraftStyle::default(),
        },
    ]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = Point2::new(1e-10, 0.);
    camera.pixels_per_metre = 1e12;
    assert!(cache.pick(&doc, 0, &camera, Point2::new(500., 350.), 1.).unwrap().is_empty());
    assert_eq!(cache.pick(&doc, 0, &camera, Point2::new(500., 350.), 71.).unwrap().len(), 2);

    let mut arc_doc = LabDocument::new();
    commit(&mut arc_doc, "far-arc", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Arc { center: Point2::new(-1e12, 0.), radius: 1e12, start_angle: 0., sweep_angle: 1e-6 },
        style: DraftStyle::default(),
    }]);
    let mut arc_cache = self::cache(&arc_doc, DraftingCacheLimits::default());
    camera.center = Point2::new(-1e-10, 0.);
    assert!(arc_cache.pick(&arc_doc, 0, &camera, Point2::new(500., 350.), 1.).unwrap().is_empty());
    assert_eq!(arc_cache.pick(&arc_doc, 0, &camera, Point2::new(500., 350.), 101.).unwrap().len(), 1);
}

#[test]
fn compensated_arc_bounds_keep_a_tangent_endpoint_in_the_visible_set() {
    let mut doc = LabDocument::new();
    commit(&mut doc, "tangent-arc", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Arc { center: Point2::new(-1e12, 0.), radius: 1e12, start_angle: 0., sweep_angle: 1e-10 },
        style: DraftStyle::default(),
    }]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let camera = Camera2D {
        center: Point2::new(-5e-9, 100.), pixels_per_metre: 1e12,
        viewport_width: 1000., viewport_height: 700., revision: 0,
    };
    let options = DisplayOptions { pixel_error: 0.01, ..DisplayOptions::default() };
    let frame = cache.prepare(&doc, 0, &camera, &options).unwrap();
    assert_eq!(frame.base.visible_entity_count, 1);
    assert!(frame.base.primitives.iter().any(|p| {
        (p.end.x - 500.).abs() < 0.01 && (p.end.y - 350.).abs() < 0.01
    }));
}

fn assert_arc_endpoint_apertures(sweep_angle: f64, endpoint_t: f64, outward: Point2) {
    let arc = AnalyticArc {
        center: Point2::default(), radius: 1e12, start_angle: 0., sweep_angle,
    };
    let mut doc = LabDocument::new();
    let receipt = commit(&mut doc, "arc-endpoint-apertures", vec![DraftOperation::Create {
        drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
        geometry: EntityGeometry::Arc {
            center: arc.center, radius: arc.radius, start_angle: arc.start_angle,
            sweep_angle: arc.sweep_angle,
        }, style: DraftStyle::default(),
    }]);
    let mut cache = cache(&doc, DraftingCacheLimits::default());
    let snapshot = doc.snapshot(DrawingId::new(1)).unwrap();
    let camera = Camera2D {
        center: arc.point_relative(Point2::default(), endpoint_t),
        pixels_per_metre: 1e12, viewport_width: 2000., viewport_height: 2000., revision: 0,
    };
    // Anchor at the stably rebased analytic endpoint, rather than reconstructing
    // a world query whose 100-pixel displacement could round away.
    let endpoint = arc.point_relative(camera.center, endpoint_t);
    let endpoint_screen = Point2::new(
        1000. + endpoint.x * camera.pixels_per_metre,
        1000. - endpoint.y * camera.pixels_per_metre,
    );
    let outside = Point2::new(endpoint_screen.x + outward.x, endpoint_screen.y + outward.y);
    let inside = Point2::new(endpoint_screen.x - outward.x, endpoint_screen.y - outward.y);
    let before = cache.performance().distance_checks;
    assert!(cache.pick(&doc, 0, &camera, outside, 1.).unwrap().is_empty(),
        "outward tangent must miss: sweep={sweep_angle}, endpoint={endpoint_t}");
    // Major arcs and these minor-end fixtures remain AABB candidates. At the
    // minor start the outward query is correctly culled, so also check the
    // analytic public distance directly below.
    let expected_checks = if endpoint_t == 0. && sweep_angle.abs() <= std::f64::consts::PI {
        before
    } else {
        before + 1
    };
    assert_eq!(cache.performance().distance_checks, expected_checks);
    let wide = cache.pick(&doc, 0, &camera, outside, 101.).unwrap();
    assert_eq!(wide.len(), 1);
    assert_eq!(wide[0].entity_id, receipt.created_ids[0]);
    assert!((wide[0].distance_px - 100.).abs() < 1e-6,
        "endpoint distance must be 100 pixels, got {}", wide[0].distance_px);
    let inward = cache.pick(&doc, 0, &camera, inside, 1.).unwrap();
    assert_eq!(inward.len(), 1,
        "inward tangent must hit: sweep={sweep_angle}, endpoint={endpoint_t}");
    assert!(inward[0].distance_px < 1e-6);

    assert!(pick(&snapshot, &camera, outside, 1.).unwrap().is_empty());
    let owned_wide = pick(&snapshot, &camera, outside, 101.).unwrap();
    assert_eq!(owned_wide.len(), 1);
    assert!((owned_wide[0].distance_px - 100.).abs() < 1e-6);
    let owned_inward = pick(&snapshot, &camera, inside, 1.).unwrap();
    assert_eq!(owned_inward.len(), 1);
    assert!(owned_inward[0].distance_px < 1e-6);

    if endpoint_t == 0. {
        // The start is exactly (R,0), so these world-space tangent offsets are
        // representable without losing the displacement to a large origin.
        let outside_world = Point2::new(arc.radius, -sweep_angle.signum() * 1e-10);
        let inside_world = Point2::new(arc.radius, sweep_angle.signum() * 1e-10);
        assert!((arc.distance(outside_world) * camera.pixels_per_metre - 100.).abs() < 1e-6);
        assert!(arc.distance(inside_world) * camera.pixels_per_metre < 1e-6);
    }
}

#[test]
fn major_arc_end_tangent_picking_respects_both_signed_sweeps() {
    for sweep in [3. * std::f64::consts::FRAC_PI_2, -3. * std::f64::consts::FRAC_PI_2] {
        assert_arc_endpoint_apertures(sweep, 1., Point2::new(100., 0.));
    }
}

#[test]
fn major_arc_start_tangent_picking_respects_both_signed_sweeps() {
    for sweep in [3. * std::f64::consts::FRAC_PI_2, -3. * std::f64::consts::FRAC_PI_2] {
        assert_arc_endpoint_apertures(sweep, 0., Point2::new(0., 100. * sweep.signum()));
    }
}

#[test]
fn minor_arc_end_tangent_picking_respects_both_signed_sweeps() {
    for sweep in [std::f64::consts::FRAC_PI_2, -std::f64::consts::FRAC_PI_2] {
        assert_arc_endpoint_apertures(sweep, 1., Point2::new(-100., 0.));
    }
}

#[test]
fn minor_arc_start_tangent_picking_respects_both_signed_sweeps() {
    for sweep in [std::f64::consts::FRAC_PI_2, -std::f64::consts::FRAC_PI_2] {
        assert_arc_endpoint_apertures(sweep, 0., Point2::new(0., 100. * sweep.signum()));
    }
}

#[test]
fn nearly_full_turn_arc_excludes_its_gap_when_boundary_bands_overlap() {
    let magnitude = f64::from_bits(std::f64::consts::TAU.to_bits() - 1);
    for sweep_angle in [magnitude, -magnitude] {
        let arc = AnalyticArc {
            center: Point2::default(), radius: 1e12, start_angle: 0., sweep_angle,
        };
        let mut doc = LabDocument::new();
        commit(&mut doc, "nearly-full-turn-gap", vec![DraftOperation::Create {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            geometry: EntityGeometry::Arc {
                center: arc.center, radius: arc.radius, start_angle: arc.start_angle,
                sweep_angle: arc.sweep_angle,
            }, style: DraftStyle::default(),
        }]);
        let mut cache = cache(&doc, DraftingCacheLimits::default());
        let snapshot = doc.snapshot(DrawingId::new(1)).unwrap();
        let end = arc.point_relative(Point2::default(), 1.);
        let mut camera = Camera2D {
            center: Point2::new(arc.radius, end.y * 0.5), pixels_per_metre: 1e5,
            viewport_width: 2000., viewport_height: 2000., revision: 0,
        };
        let query = Point2::new(1000., 1000.);
        // The gap midpoint is about 56.65 pixels from either endpoint. Both
        // angular boundary bands match, but both oriented sides exclude it.
        let expected_distance_px = end.y.abs() * 0.5 * camera.pixels_per_metre;
        assert!(expected_distance_px > 50. && expected_distance_px < 60.);
        let before = cache.performance().distance_checks;
        assert!(cache.pick(&doc, 0, &camera, query, 1.).unwrap().is_empty());
        assert_eq!(cache.performance().distance_checks, before + 1);
        let wide = cache.pick(&doc, 0, &camera, query, 101.).unwrap();
        assert_eq!(wide.len(), 1);
        assert!((wide[0].distance_px - expected_distance_px).abs() < 1e-6);
        assert!(pick(&snapshot, &camera, query, 1.).unwrap().is_empty());
        let owned_wide = pick(&snapshot, &camera, query, 101.).unwrap();
        assert_eq!(owned_wide.len(), 1);
        assert!((owned_wide[0].distance_px - expected_distance_px).abs() < 1e-6);

        // Beyond the endpoint toward the arc interior, the start-side sign is
        // outside but the end-side sign is inside. Major-arc OR must admit it.
        camera.center.y = 2. * end.y;
        let before = cache.performance().distance_checks;
        let inward = cache.pick(&doc, 0, &camera, query, 1.).unwrap();
        assert_eq!(cache.performance().distance_checks, before + 1);
        assert_eq!(inward.len(), 1);
        assert!(inward[0].distance_px < 1e-6);
        let owned_inward = pick(&snapshot, &camera, query, 1.).unwrap();
        assert_eq!(owned_inward.len(), 1);
        assert!(owned_inward[0].distance_px < 1e-6);
    }
}

#[test]
fn full_turn_arc_picking_keeps_both_sides_of_nominal_endpoints() {
    for sweep_angle in [std::f64::consts::TAU, -std::f64::consts::TAU] {
        let arc = AnalyticArc {
            center: Point2::default(), radius: 1e12, start_angle: 0., sweep_angle,
        };
        let mut doc = LabDocument::new();
        commit(&mut doc, "full-turn-apertures", vec![DraftOperation::Create {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            geometry: EntityGeometry::Arc {
                center: arc.center, radius: arc.radius, start_angle: arc.start_angle,
                sweep_angle: arc.sweep_angle,
            }, style: DraftStyle::default(),
        }]);
        let mut cache = cache(&doc, DraftingCacheLimits::default());
        let snapshot = doc.snapshot(DrawingId::new(1)).unwrap();
        for endpoint_t in [0., 1.] {
            let camera = Camera2D {
                center: arc.point_relative(Point2::default(), endpoint_t),
                pixels_per_metre: 1e12, viewport_width: 2000., viewport_height: 2000., revision: 0,
            };
            let endpoint = arc.point_relative(camera.center, endpoint_t);
            let direction = arc.direction(endpoint_t);
            for side in [-1., 1.] {
                let query = Point2::new(
                    1000. + endpoint.x * camera.pixels_per_metre - side * 100. * direction.y,
                    1000. - endpoint.y * camera.pixels_per_metre - side * 100. * direction.x,
                );
                let before = cache.performance().distance_checks;
                let hits = cache.pick(&doc, 0, &camera, query, 1.).unwrap();
                assert_eq!(cache.performance().distance_checks, before + 1);
                assert_eq!(hits.len(), 1,
                    "full turn has no excluded side: sweep={sweep_angle}, endpoint={endpoint_t}, side={side}");
                assert!(hits[0].distance_px < 1e-6);
                let owned_hits = pick(&snapshot, &camera, query, 1.).unwrap();
                assert_eq!(owned_hits.len(), 1);
                assert!(owned_hits[0].distance_px < 1e-6);
            }
        }
    }
}
