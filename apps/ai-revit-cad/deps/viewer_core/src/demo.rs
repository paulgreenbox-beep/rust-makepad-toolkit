use crate::{Mesh, Quat, Scene, SceneObject, Transform3, Vec3};

/// Builds a deterministic multi-object scene for viewer integration and interaction tests.
pub fn procedural_test_scene() -> Scene {
    let mut scene = Scene::new();

    let mut base = SceneObject::new(1001, "Base block", Mesh::cuboid(Vec3::new(2.4, 0.8, 2.0)));
    base.transform.translation = Vec3::new(-0.8, -0.15, 0.0);
    base.color = [0.24, 0.48, 0.78, 1.0];
    scene.insert(base);

    let mut upright = SceneObject::new(1002, "Upright", Mesh::cuboid(Vec3::new(0.8, 2.6, 0.8)));
    upright.transform.translation = Vec3::new(-0.6, 1.25, 0.0);
    upright.color = [0.92, 0.48, 0.2, 1.0];
    scene.insert(upright);

    let mut rotated = SceneObject::new(
        1003,
        "Rotated block",
        Mesh::cuboid(Vec3::new(1.0, 1.0, 1.0)),
    );
    rotated.transform = Transform3::new(
        Vec3::new(1.35, 0.15, 0.55),
        Quat::from_axis_angle(Vec3::Y, 0.38),
        Vec3::new(1.0, 1.0, 1.0),
    );
    rotated.color = [0.25, 0.68, 0.48, 1.0];
    scene.insert(rotated);

    let mut small = SceneObject::new(
        1004,
        "Small block",
        Mesh::cuboid(Vec3::new(0.55, 0.55, 0.55)),
    );
    small.transform.translation = Vec3::new(1.8, 0.8, -0.35);
    small.color = [0.72, 0.38, 0.68, 1.0];
    scene.insert(small);

    scene
}
