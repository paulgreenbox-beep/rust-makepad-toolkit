use ai_revit_core::{
    BUILTIN_FLOOR_CLASS_ID, Command, CoreError, Document, ElementId, ElementKind, FloorSpec, Point2,
};
use ai_revit_geometry::{
    DEFAULT_LINEAR_TOLERANCE_M, GeometryErrorCode, mesh_for_element, object_id_for,
};
use std::collections::BTreeMap;
use viewer_core::{Ray, Vec3};

fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Vec<Point2> {
    vec![
        Point2::new(x, y),
        Point2::new(x + width, y),
        Point2::new(x + width, y + height),
        Point2::new(x, y + height),
    ]
}

fn floor(boundary: Vec<Point2>) -> FloorSpec {
    FloorSpec {
        boundary,
        holes: Vec::new(),
        elevation_m: 0.0,
        thickness_m: 0.2,
    }
}

fn create_floor(document: &mut Document, spec: FloorSpec) -> Result<ElementId, CoreError> {
    let transaction = document
        .begin_transaction("create-floor", document.revision())
        .with(Command::CreateElement {
            name: "Floor".to_owned(),
            class_id: BUILTIN_FLOOR_CLASS_ID.into(),
            kind: ElementKind::Floor(spec),
            properties: BTreeMap::new(),
        });
    Ok(document.commit(transaction)?.created_elements[0])
}

fn assert_model_and_projection_reject(spec: FloorSpec, expected_code: GeometryErrorCode) {
    let mut document = Document::new("Invalid contour").unwrap();
    let before = document.to_json_string().unwrap();
    assert!(matches!(
        create_floor(&mut document, spec.clone()),
        Err(CoreError::InvalidGeometry(_))
    ));
    assert_eq!(
        document.to_json_string().unwrap(),
        before,
        "failed creation must be atomic"
    );

    // A detached record exercises projection's validation of direct caller input.
    let mut seed = Document::new("Projection input").unwrap();
    let id = create_floor(&mut seed, floor(rectangle(0.0, 0.0, 4.0, 3.0))).unwrap();
    let mut element = seed.element(id).unwrap().clone();
    element.kind = ElementKind::Floor(spec);
    let error = mesh_for_element(&element)
        .err()
        .expect("projection must reject the same shape");
    assert_eq!(error.code, expected_code);
}

#[test]
fn sub_tolerance_thickness_is_rejected_before_commit_and_projection() {
    let mut spec = floor(rectangle(0.0, 0.0, 4.0, 3.0));
    spec.thickness_m = DEFAULT_LINEAR_TOLERANCE_M / 2.0;
    assert_model_and_projection_reject(spec, GeometryErrorCode::NonPositiveDimension);
}

#[test]
fn adjacent_edge_reversal_is_rejected_before_commit_and_projection() {
    let spec = floor(vec![
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 0.0),
        Point2::new(1.0, 0.0),
        Point2::new(1.0, 2.0),
        Point2::new(0.0, 2.0),
    ]);
    assert_model_and_projection_reject(spec, GeometryErrorCode::SelfIntersection);
}

#[test]
fn translated_floor_round_trips_and_projects_an_actual_opening() {
    let offset = 1.0e9;
    let mut spec = floor(rectangle(offset, offset, 6.0, 4.0));
    spec.boundary.push(spec.boundary[0]);
    spec.holes
        .push(rectangle(offset + 2.0, offset + 1.0, 2.0, 2.0));
    let mut document = Document::new("Floor opening").unwrap();
    let id = create_floor(&mut document, spec.clone()).unwrap();
    let reopened = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
    let element = reopened.element(id).unwrap();
    assert_eq!(
        element.kind,
        ElementKind::Floor(spec),
        "source rings must round trip exactly"
    );
    let projected = mesh_for_element(element).unwrap();

    let top_area: f64 = projected
        .mesh
        .triangles()
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = triangle.vertices;
            (a.z == 0.0 && b.z == 0.0 && c.z == 0.0).then(|| (b - a).cross(c - a).z.abs() / 2.0)
        })
        .sum();
    assert!(
        (top_area - 20.0).abs() < 1.0e-10,
        "top area must exclude the 4 m² opening"
    );
    let down = Vec3::new(0.0, 0.0, -1.0);
    let opening_ray = Ray::new(Vec3::new(offset + 3.0, offset + 2.0, 2.0), down);
    let solid_ray = Ray::new(Vec3::new(offset + 1.0, offset + 2.0, 2.0), down);
    assert!(
        projected
            .mesh
            .pick(object_id_for(id), opening_ray, 10.0)
            .is_none()
    );
    assert!(
        projected
            .mesh
            .pick(object_id_for(id), solid_ray, 10.0)
            .is_some()
    );
}
