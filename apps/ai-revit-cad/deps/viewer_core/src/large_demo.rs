use crate::{Mesh, Quat, Scene, SceneObject, Transform3, Vec3};

/// Builds a deterministic, meter-scale engineering rack assembly with 132 mesh objects.
///
/// The fixture is intentionally made from simple render meshes. It provides enough
/// independent stable objects and overlapping structure to exercise viewport
/// clipping, picking, and semantic snapshots without requiring a CAD importer.
pub fn large_demo_scene() -> Scene {
    let mut scene = Scene::new();
    let length = 8.0;
    let half_width = 1.75;
    let stations = 9;
    let z_at = |station: usize| -length * 0.5 + length * station as f64 / (stations - 1) as f64;
    let steel = [0.47, 0.53, 0.61, 1.0];

    // Two rows of uprights, with IDs partitioned by the 210_000 range.
    for station in 0..stations {
        let z = z_at(station);
        for (side, x) in [-half_width, half_width].into_iter().enumerate() {
            let id = 210_000 + station as u64 * 2 + side as u64;
            let side_name = if side == 0 { "west" } else { "east" };
            insert_box(
                &mut scene,
                id,
                format!("RACK-A / column / station-{:02} / {side_name}", station + 1),
                Vec3::new(0.10, 4.2, 0.10),
                Vec3::new(x, 2.1, z),
                steel,
            );
        }
    }

    // Three longitudinal base members.
    for (index, x) in [-half_width, 0.0, half_width].into_iter().enumerate() {
        insert_box(
            &mut scene,
            211_001 + index as u64,
            format!("RACK-A / base rail / line-{:02}", index + 1),
            Vec3::new(0.14, 0.14, length),
            Vec3::new(x, 0.10, 0.0),
            steel,
        );
    }

    // Transverse beams at each station and three working elevations.
    let elevations = [0.16, 2.10, 4.10];
    for station in 0..stations {
        for (level, y) in elevations.into_iter().enumerate() {
            let id = 212_000 + station as u64 * 3 + level as u64;
            insert_box(
                &mut scene,
                id,
                format!(
                    "RACK-A / cross beam / station-{:02} / level-{:02}",
                    station + 1,
                    level + 1
                ),
                Vec3::new(half_width * 2.0, 0.10, 0.10),
                Vec3::new(0.0, y, z_at(station)),
                steel,
            );
        }
    }

    // X bracing on both sides. Each bay gets two diagonal members per side.
    let brace_thickness = 0.045;
    let brace_length = (1.0_f64 + 4.0_f64 * 4.0).sqrt();
    let up_angle = 4.0_f64.atan2(1.0);
    for bay in 0..stations - 1 {
        let z0 = z_at(bay);
        let z1 = z_at(bay + 1);
        for (side, x) in [-half_width, half_width].into_iter().enumerate() {
            for (diagonal, angle) in [-up_angle, up_angle].into_iter().enumerate() {
                let id = 213_000 + bay as u64 * 4 + side as u64 * 2 + diagonal as u64;
                insert_rotated_box(
                    &mut scene,
                    id,
                    format!(
                        "RACK-A / diagonal brace / bay-{:02} / {} / {}",
                        bay + 1,
                        if side == 0 { "west" } else { "east" },
                        if diagonal == 0 { "rising" } else { "falling" }
                    ),
                    Vec3::new(brace_thickness, brace_thickness, brace_length),
                    Vec3::new(x, 2.18, (z0 + z1) * 0.5),
                    Quat::from_axis_angle(Vec3::X, angle),
                    [0.40, 0.46, 0.54, 1.0],
                );
            }
        }
    }

    // Four color-coded longitudinal service trays, in the 214_000 ID range.
    let tray_x = [-1.20, -0.40, 0.40, 1.20];
    let tray_colors = [
        [0.86, 0.40, 0.24, 1.0],
        [0.25, 0.62, 0.84, 1.0],
        [0.30, 0.70, 0.43, 1.0],
        [0.78, 0.66, 0.25, 1.0],
    ];
    for lane in 0..tray_x.len() {
        insert_box(
            &mut scene,
            214_001 + lane as u64,
            format!("SERVICES / cable tray / lane-{:02}", lane + 1),
            Vec3::new(0.34, 0.07, length),
            Vec3::new(tray_x[lane], 2.72, 0.0),
            tray_colors[lane],
        );
    }

    // Tray cross supports: one per lane and station, creating a dense, useful
    // test for cropped snapshots and Section Box clipping.
    for station in 0..stations {
        for lane in 0..tray_x.len() {
            let id = 215_000 + station as u64 * tray_x.len() as u64 + lane as u64;
            insert_box(
                &mut scene,
                id,
                format!(
                    "SERVICES / tray support / station-{:02} / lane-{:02}",
                    station + 1,
                    lane + 1
                ),
                Vec3::new(0.44, 0.035, 0.045),
                Vec3::new(tray_x[lane], 2.63, z_at(station)),
                [0.60, 0.64, 0.68, 1.0],
            );
        }
    }

    // Four small parallel rectangular service runs with distinct identities.
    for line in 0..4 {
        let x = -1.05 + line as f64 * 0.70;
        insert_box(
            &mut scene,
            216_001 + line as u64,
            format!("SERVICES / duct run / line-{:02}", line + 1),
            Vec3::new(0.12, 0.12, length),
            Vec3::new(x, 3.20, 0.0),
            [0.60, 0.68, 0.74, 1.0],
        );
    }

    // Four equipment skids, each split into a base and body so the object tree
    // and semantic ID buffer have meaningful part-level records.
    let equipment = [
        (
            "MCC-01",
            Vec3::new(-0.85, 0.0, -4.75),
            [0.31, 0.48, 0.70, 1.0],
        ),
        (
            "VFD-01",
            Vec3::new(0.45, 0.0, -4.75),
            [0.36, 0.57, 0.70, 1.0],
        ),
        (
            "PUMP-01",
            Vec3::new(-0.80, 0.0, 4.75),
            [0.38, 0.58, 0.48, 1.0],
        ),
        (
            "FILTER-01",
            Vec3::new(0.55, 0.0, 4.75),
            [0.63, 0.54, 0.34, 1.0],
        ),
    ];
    for (unit, (name, base, color)) in equipment.into_iter().enumerate() {
        insert_box(
            &mut scene,
            217_000 + unit as u64 * 2,
            format!("EQUIPMENT / {name} / skid"),
            Vec3::new(0.90, 0.12, 0.72),
            Vec3::new(base.x, base.y + 0.06, base.z),
            [0.34, 0.38, 0.42, 1.0],
        );
        insert_box(
            &mut scene,
            217_001 + unit as u64 * 2,
            format!("EQUIPMENT / {name} / body"),
            Vec3::new(0.82, 1.10, 0.64),
            Vec3::new(base.x, base.y + 0.67, base.z),
            color,
        );
    }

    scene
}

fn insert_box(
    scene: &mut Scene,
    id: u64,
    name: String,
    size: Vec3,
    translation: Vec3,
    color: [f32; 4],
) {
    insert_rotated_box(scene, id, name, size, translation, Quat::IDENTITY, color);
}

fn insert_rotated_box(
    scene: &mut Scene,
    id: u64,
    name: String,
    size: Vec3,
    translation: Vec3,
    rotation: Quat,
    color: [f32; 4],
) {
    let mut object = SceneObject::new(id, name, Mesh::cuboid(size));
    object.transform = Transform3::new(translation, rotation, Vec3::new(1.0, 1.0, 1.0));
    object.color = color;
    scene.insert(object);
}

#[cfg(test)]
mod tests {
    use super::large_demo_scene;
    use crate::Viewer;

    #[test]
    fn large_fixture_has_deterministic_stable_engineering_objects() {
        let first = large_demo_scene();
        let second = large_demo_scene();
        assert_eq!(first.object_ids().count(), 132);
        assert_eq!(
            first.object_ids().collect::<Vec<_>>(),
            second.object_ids().collect::<Vec<_>>()
        );
        assert_eq!(
            first.get(210_000).map(|object| object.name.as_str()),
            Some("RACK-A / column / station-01 / west")
        );
        assert_eq!(
            first.get(214_001).map(|object| object.name.as_str()),
            Some("SERVICES / cable tray / lane-01")
        );
        assert_eq!(
            first.get(217_007).map(|object| object.name.as_str()),
            Some("EQUIPMENT / FILTER-01 / body")
        );
        let bounds = first.visible_bounds().expect("fixture has geometry");
        assert!(bounds.min.x < -1.7 && bounds.max.x > 1.7);
        assert!(bounds.min.y <= 0.0 && bounds.max.y > 4.1);
        assert!(bounds.min.z < -5.0 && bounds.max.z > 5.0);
    }

    #[test]
    fn large_demo_viewer_starts_fitted_to_the_complete_scene() {
        let viewer = Viewer::large_demo();
        let bounds = viewer
            .scene()
            .visible_bounds()
            .expect("fixture has geometry");
        let target = viewer.camera().target;
        assert!((target - bounds.center()).length() < 1.0e-9);
        let section = viewer.section_box().bounds;
        assert!(section.contains(bounds.min));
        assert!(section.contains(bounds.max));
        assert_eq!(viewer.scene().object_ids().count(), 132);
    }
}
