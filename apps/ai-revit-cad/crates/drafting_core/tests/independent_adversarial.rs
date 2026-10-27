//! Independent acceptance regressions. No production or native app execution.
//! CHEAPEST_VERIFICATION_PATH: cargo test -p drafting_core --test independent_adversarial --offline -j2
use drafting_core::*;

fn create_lines(document: &mut LabDocument, count: usize) -> Vec<DraftEntityId> {
    document.apply(CommandEnvelope {
        command_id: "independent-seed".into(),
        expected_revision: document.revision(),
        operations: (0..count).map(|i| DraftOperation::Create {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            geometry: EntityGeometry::Line {
                start: Point2::new(i as f64, 0.), end: Point2::new(i as f64, 1.),
            }, style: DraftStyle::default(),
        }).collect(),
    }).unwrap().created_ids
}

#[test]
fn oversized_change_batch_is_rejected_atomically_or_remains_saveable() {
    let mut document = LabDocument::new();
    let ids = create_lines(&mut document, 100);
    let before = document.to_json().unwrap();
    let result = document.apply(CommandEnvelope {
        command_id: "independent-many-changes".into(),
        expected_revision: document.revision(),
        operations: (0..620).map(|_| DraftOperation::Move {
            drawing_id: DrawingId::new(1), ids: ids.clone(), delta: Point2::new(0.001, 0.),
        }).collect(),
    });
    match result {
        Ok(_) => { document.to_json().expect("successful persistent command must remain saveable"); }
        Err(_) => { assert_eq!(document.to_json().unwrap(), before, "budget refusal must not publish partial state"); }
    }
}

#[test]
fn almost_flat_bulge_stays_at_the_correct_canvas_location_at_high_zoom() {
    let geometry = EntityGeometry::Polyline {
        vertices: vec![PolylineVertex::new(Point2::new(0., 0.), 1e-12),
                       PolylineVertex::new(Point2::new(1., 0.), 0.)],
        closed: false,
    };
    geometry.validate(GeometryLimits::default()).unwrap();
    for ppm in [1e6, 1e8] {
        let mut camera = Camera2D::new(1000., 700.);
        camera.center = Point2::new(0.5, 0.);
        camera.pixels_per_metre = ppm;
        let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
        assert!(!segments.is_empty(), "visible valid nearly-flat bulge must produce canvas geometry");
        // The exact arc sagitta is chord*bulge/2 = 5e-13 m. Even at ppm=1e8,
        // its deviation from y=350 is only 5e-5 px, far below the 0.5 px budget.
        for segment in segments {
            for point in [segment.start, segment.end] {
                assert!((point.y - 350.).abs() <= 0.501,
                    "ppm={ppm}: valid bulge projected to y={} instead of ≈350", point.y);
            }
        }
    }
}

#[test]
fn malformed_duplicate_entity_map_key_is_rejected_independently() {
    let mut document = LabDocument::new();
    create_lines(&mut document, 1);
    let value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    let entity = serde_json::to_string(&value["state"]["drawings"]["1"]["entities"]["1"]).unwrap();
    let serialized = serde_json::to_string(&value).unwrap();
    let singleton = format!("\"entities\":{{\"1\":{entity}}}");
    let duplicate = format!("\"entities\":{{\"1\":{entity},\"1\":{entity}}}");
    assert!(serialized.contains(&singleton));
    assert!(LabDocument::from_json(&serialized.replacen(&singleton, &duplicate, 1)).is_err());
}

#[test]
fn history_after_payload_cannot_silently_replace_live_geometry_on_load() {
    let mut document = LabDocument::new();
    create_lines(&mut document, 1);
    let mut value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    value["history"][0]["changes"][0]["after"]["geometry"]["end"]["x"] = serde_json::json!(100.);
    assert!(LabDocument::from_json(&value.to_string()).is_err());
}

#[test]
fn saved_replay_cannot_claim_a_zero_drawing_id_command_succeeded() {
    let mut document = LabDocument::new();
    create_lines(&mut document, 1);
    let mut value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    value["receipts"]["independent-seed"]["action"]["operations"][0]["drawing_id"] = serde_json::json!("0");
    assert!(LabDocument::from_json(&value.to_string()).is_err(),
        "saved successful receipt must reject an impossible zero-ID operation");
}

#[test]
fn saved_replay_payload_must_agree_with_its_retained_history_delta() {
    let mut document = LabDocument::new();
    create_lines(&mut document, 1);
    let mut value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    value["receipts"]["independent-seed"]["action"]["operations"][0]["geometry"]["end"]["x"] = serde_json::json!(100.);
    assert!(LabDocument::from_json(&value.to_string()).is_err(),
        "saved receipt must not report a successful create for geometry absent from its delta");
}

#[test]
fn camera_overflow_rejects_without_mutating_authority_or_view() {
    let document = LabDocument::new();
    let before_document = document.to_json().unwrap();
    let mut camera = Camera2D::new(1000., 700.);
    camera.revision = u64::MAX;
    let before_camera = camera.clone();
    assert!(camera.pan_pixels(3., 4.).is_err());
    assert_eq!(camera, before_camera);
    assert_eq!(document.to_json().unwrap(), before_document);
}
