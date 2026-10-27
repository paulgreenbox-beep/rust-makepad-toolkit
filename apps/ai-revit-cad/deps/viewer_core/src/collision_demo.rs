//! Deterministic scene used to exercise collision summaries and compact views.

use crate::{ExactGeometryRef, ExactGeometrySource, Mesh, Scene, SceneObject, Vec3};

/// Builds a scene with one overlap candidate, one near-clearance pair, a
/// depth-separated projected occlusion pair, and one remote context object.
pub fn collision_demo_scene() -> Scene {
    let mut scene = Scene::new();

    let mut candidate_a = SceneObject::new(
        3001,
        "Overlap candidate A",
        Mesh::cuboid(Vec3::new(1.6, 1.6, 1.6)),
    );
    candidate_a.transform.translation = Vec3::new(-4.0, 0.0, 0.0);
    candidate_a.color = [0.22, 0.58, 0.86, 1.0];
    scene.insert(candidate_a);

    let mut candidate_b = SceneObject::new(
        3002,
        "Overlap candidate B",
        Mesh::cuboid(Vec3::new(1.6, 1.6, 1.6)),
    );
    candidate_b.transform.translation = Vec3::new(-3.1, 0.0, 0.0);
    candidate_b.color = [0.93, 0.48, 0.18, 1.0];
    scene.insert(candidate_b);

    let mut clearance_a = SceneObject::new(
        3003,
        "Clearance object A",
        Mesh::cuboid(Vec3::new(1.0, 1.0, 1.0)),
    );
    clearance_a.transform.translation = Vec3::new(-0.8, 0.0, 0.0);
    clearance_a.color = [0.36, 0.68, 0.48, 1.0];
    scene.insert(clearance_a);

    let mut clearance_b = SceneObject::new(
        3004,
        "Clearance object B",
        Mesh::cuboid(Vec3::new(1.0, 1.0, 1.0)),
    );
    clearance_b.transform.translation = Vec3::new(0.23, 0.0, 0.0);
    clearance_b.color = [0.72, 0.42, 0.7, 1.0];
    scene.insert(clearance_b);

    // This pair overlaps in the default camera projection but has a positive
    // gap along world X, so it exercises hidden-line output without a collision.
    let mut rear = SceneObject::new(
        3005,
        "Partially occluded rear object",
        Mesh::cuboid(Vec3::new(1.2, 1.2, 1.2)),
    );
    rear.transform.translation = Vec3::new(2.8, 0.8, -3.0);
    rear.color = [0.68, 0.72, 0.78, 1.0];
    scene.insert(rear);

    let mut front = SceneObject::new(
        3006,
        "Partial occluder",
        Mesh::cuboid(Vec3::new(1.2, 1.2, 1.2)),
    );
    front.transform.translation = Vec3::new(4.157, 1.56, -1.959);
    front.color = [0.2, 0.66, 0.82, 1.0];
    scene.insert(front);

    let mut remote = SceneObject::new(
        3007,
        "Remote irrelevant context",
        Mesh::cuboid(Vec3::new(1.5, 1.5, 1.5)),
    );
    remote.transform.translation = Vec3::new(14.0, 0.0, 0.0);
    remote.color = [0.55, 0.57, 0.6, 1.0];
    scene.insert(remote);

    let box_sources = scene
        .objects()
        .filter_map(|object| {
            object
                .mesh
                .bounds()
                .map(|bounds| (object.object_id, bounds.extent()))
        })
        .collect::<Vec<_>>();
    for (id, size) in box_sources {
        scene.get_mut(id).expect("registered object").exact_geometry = Some(ExactGeometryRef {
            source: ExactGeometrySource::Box { size },
            revision: format!(
                "collision_demo_box_v1:{id}:{:.9}:{:.9}:{:.9}",
                size.x, size.y, size.z
            ),
        });
    }
    scene
}
