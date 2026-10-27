//! Four separated analytic pairs with known exact outcomes.
//! IDs 4101/4102: AABB false positive. 4201/4202: volume overlap.
//! IDs 4301/4302: face touch. 4401/4402: 30 mm clearance.

use crate::{
    ExactBoxPart, ExactGeometryRef, ExactGeometrySource, Mesh, Scene, SceneObject, Triangle, Vec3,
};

fn box_object(id: u64, name: &str, min: Vec3, size: Vec3, color: [f32; 4]) -> SceneObject {
    let mut object = SceneObject::new(id, name, Mesh::cuboid(size));
    object.transform.translation = min + size * 0.5;
    object.color = color;
    object.exact_geometry = Some(ExactGeometryRef {
        source: ExactGeometrySource::Box { size },
        revision: format!("exact_acceptance_v1:{id}:box"),
    });
    object
}

fn union_box_object(id: u64, name: &str, parts: Vec<ExactBoxPart>, color: [f32; 4]) -> SceneObject {
    let mut triangles = Vec::new();
    for part in &parts {
        let center = part.min + part.size * 0.5;
        for triangle in Mesh::cuboid(part.size).triangles() {
            let vertices = triangle.vertices.map(|vertex| vertex + center);
            triangles.push(Triangle::new(
                triangles.len() as u32,
                vertices[0],
                vertices[1],
                vertices[2],
            ));
        }
    }
    let mut object = SceneObject::new(id, name, Mesh::new(triangles));
    object.color = color;
    object.exact_geometry = Some(ExactGeometryRef {
        source: ExactGeometrySource::UnionBoxes { parts },
        revision: format!("exact_acceptance_v1:{id}:union_boxes"),
    });
    object
}

/// Returns all four exact acceptance cases, spatially separated by group.
pub fn exact_acceptance_scene() -> Scene {
    let mut scene = Scene::new();
    scene.insert(union_box_object(
        4101,
        "AABB false positive L profile",
        vec![
            ExactBoxPart {
                min: Vec3::new(-6.0, 0.0, 0.0),
                size: Vec3::new(1.0, 0.1, 0.1),
            },
            ExactBoxPart {
                min: Vec3::new(-6.0, 0.0, 0.0),
                size: Vec3::new(0.1, 1.0, 0.1),
            },
        ],
        [0.20, 0.55, 0.85, 1.0],
    ));
    scene.insert(box_object(
        4102,
        "Cube inside L cutout",
        Vec3::new(-5.5, 0.5, 0.0),
        Vec3::new(0.1, 0.1, 0.1),
        [0.9, 0.5, 0.2, 1.0],
    ));
    scene.insert(box_object(
        4201,
        "Real intersection A",
        Vec3::new(-2.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.25, 0.68, 0.48, 1.0],
    ));
    scene.insert(box_object(
        4202,
        "Real intersection B",
        Vec3::new(-1.1, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.77, 0.43, 0.68, 1.0],
    ));
    scene.insert(box_object(
        4301,
        "Touching face A",
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.24, 0.60, 0.86, 1.0],
    ));
    scene.insert(box_object(
        4302,
        "Touching face B",
        Vec3::new(3.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.88, 0.56, 0.23, 1.0],
    ));
    scene.insert(box_object(
        4401,
        "30 mm gap A",
        Vec3::new(6.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.28, 0.65, 0.56, 1.0],
    ));
    scene.insert(box_object(
        4402,
        "30 mm gap B",
        Vec3::new(7.03, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 1.0),
        [0.73, 0.53, 0.73, 1.0],
    ));
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect_aabb_collisions;

    #[test]
    fn first_three_pairs_are_broad_phase_candidates() {
        let scene = exact_acceptance_scene();
        let collisions = detect_aabb_collisions(&scene);
        assert!(collisions
            .iter()
            .any(|value| value.collision_id == "COL-4101-4102"));
        assert!(collisions
            .iter()
            .any(|value| value.collision_id == "COL-4201-4202"));
        assert!(collisions
            .iter()
            .any(|value| value.collision_id == "COL-4301-4302"));
        assert!(!collisions
            .iter()
            .any(|value| value.collision_id == "COL-4401-4402"));
        assert!(scene.objects().all(SceneObject::has_exact_geometry));
    }
}
