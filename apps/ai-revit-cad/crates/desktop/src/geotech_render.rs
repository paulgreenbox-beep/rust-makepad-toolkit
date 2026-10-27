//! Derived display geometry only. Calculation authority remains the f64 section.
use crate::drafting_canvas::{AnalysisCanvasLayer, ScreenLabel, ScreenSegment, ScreenTriangle};
use drafting_core::{Bounds2, Camera2D, Point2 as DraftPoint};
use geotech_core::{ExtrudedMesh, Point2, SlopeAnalysisInput, SlopeAnalysisResult};
use viewer_core::{Mesh, Scene, SceneObject, Triangle, Vec3};

pub(super) struct GeotechRender {
    pub mesh: ExtrudedMesh,
    pub bounds: Bounds2,
}

impl GeotechRender {
    pub fn new(input: &SlopeAnalysisInput, result: &SlopeAnalysisResult) -> Result<Self, String> {
        let mesh = geotech_core::extrude(&result.geometry).map_err(|e| format!("{e:?}"))?;
        let mut bounds =
            Bounds2::from_points(point(input.section.slope_toe), point(input.section.crest));
        for p in input
            .section
            .terrain_polyline
            .iter()
            .chain(result.geometry.polygon.iter())
        {
            bounds = bounds.union(Bounds2::from_points(point(*p), point(*p)));
        }
        Ok(Self { mesh, bounds })
    }

    pub fn canvas(
        &self,
        input: &SlopeAnalysisInput,
        result: &SlopeAnalysisResult,
        camera: &Camera2D,
    ) -> AnalysisCanvasLayer {
        let screen = |p: Point2| camera.world_to_screen(point(p));
        let mut layer = AnalysisCanvasLayer::default();
        for indices in &self.mesh.cross_section_triangles {
            let vertices = indices.map(|i| {
                let p = self.mesh.vertices[i as usize];
                screen(Point2 { x: p[0], z: p[2] })
            });
            layer.triangles.push(ScreenTriangle {
                vertices,
                color: [0.96, 0.60, 0.20, 0.27],
            });
        }
        for edge in input.section.terrain_polyline.windows(2) {
            layer.segments.push(segment(
                screen(edge[0]),
                screen(edge[1]),
                [0.57, 0.78, 0.53, 1.0],
                2.5,
            ));
        }
        let polygon = &result.geometry.polygon;
        for i in 0..polygon.len() {
            layer.segments.push(segment(
                screen(polygon[i]),
                screen(polygon[(i + 1) % polygon.len()]),
                [0.96, 0.66, 0.27, 1.0],
                1.8,
            ));
        }
        let slip = result.geometry.slip_surface;
        layer.segments.push(segment(
            screen(slip.lower),
            screen(slip.upper),
            [1.0, 0.36, 0.30, 1.0],
            3.2,
        ));
        for (name, p) in [
            ("Toe", input.section.slope_toe),
            ("Crest", input.section.crest),
            ("Slip exit", slip.upper),
        ] {
            let p_screen = screen(p);
            layer.labels.push(label(
                DraftPoint::new(p_screen.x + 7.0, p_screen.y - 19.0),
                format!("{name} ({:.3}, {:.3}) m", p.x, p.z),
                [0.85, 0.90, 0.96, 1.0],
            ));
            let r = 4.0;
            layer.segments.push(segment(
                DraftPoint::new(p_screen.x - r, p_screen.y),
                DraftPoint::new(p_screen.x + r, p_screen.y),
                [0.9, 0.94, 1.0, 1.0],
                2.0,
            ));
            layer.segments.push(segment(
                DraftPoint::new(p_screen.x, p_screen.y - r),
                DraftPoint::new(p_screen.x, p_screen.y + r),
                [0.9, 0.94, 1.0, 1.0],
                2.0,
            ));
        }
        let toe = screen(input.section.slope_toe);
        let crest = screen(input.section.crest);
        let h_x = toe.x.min(crest.x) - 29.0;
        layer.segments.push(segment(
            DraftPoint::new(h_x, toe.y),
            DraftPoint::new(h_x, crest.y),
            [0.60, 0.70, 0.82, 1.0],
            1.0,
        ));
        layer.labels.push(label(
            DraftPoint::new(h_x + 5.0, (toe.y + crest.y) * 0.5),
            format!(
                "H = {:.3} m",
                input.section.crest.z - input.section.slope_toe.z
            ),
            [0.76, 0.83, 0.92, 1.0],
        ));
        layer.labels.push(label(
            DraftPoint::new(toe.x + 28.0, toe.y - 42.0),
            format!("α = {:.3}°", result.geometry.slip_angle_deg),
            [1.0, 0.57, 0.42, 1.0],
        ));
        let beta = (input.section.crest.z - input.section.slope_toe.z)
            .atan2((input.section.crest.x - input.section.slope_toe.x).abs())
            .to_degrees();
        layer.labels.push(label(
            DraftPoint::new(toe.x + 28.0, toe.y - 61.0),
            format!("β = {beta:.3}°"),
            [0.60, 0.82, 0.56, 1.0],
        ));
        let count = polygon.len() as f64;
        // A display-only average, evaluated relative to the first f64 point.
        let anchor = polygon[0];
        let center = Point2 {
            x: anchor.x + polygon.iter().map(|p| p.x - anchor.x).sum::<f64>() / count,
            z: anchor.z + polygon.iter().map(|p| p.z - anchor.z).sum::<f64>() / count,
        };
        let c = screen(center);
        let alpha = result.geometry.slip_angle_deg.to_radians();
        let uphill_x = (slip.upper.x - slip.lower.x).signum();
        let f = &result.forces_per_m;
        // Arrows use equal display scale; their numerical magnitudes are labelled.
        for (offset, dx, dy, name, value, color) in [
            (
                (-12.0, -6.0),
                0.0,
                76.0,
                "W",
                f.weight_kn_per_m,
                [0.82, 0.87, 1.0, 1.0],
            ),
            (
                (8.0, 10.0),
                -uphill_x * alpha.sin() * 65.0,
                -alpha.cos() * 65.0,
                "N",
                f.normal_kn_per_m,
                [0.42, 0.78, 1.0, 1.0],
            ),
            (
                (6.0, 25.0),
                -uphill_x * alpha.cos() * 74.0,
                alpha.sin() * 74.0,
                "T",
                f.driving_kn_per_m,
                [1.0, 0.42, 0.35, 1.0],
            ),
            (
                (10.0, -20.0),
                uphill_x * alpha.cos() * 74.0,
                -alpha.sin() * 74.0,
                "R",
                f.resistance_kn_per_m,
                [0.40, 0.89, 0.61, 1.0],
            ),
        ] {
            let start = DraftPoint::new(c.x + offset.0, c.y + offset.1);
            let end = DraftPoint::new(start.x + dx, start.y + dy);
            arrow(&mut layer, start, end, color);
            layer.labels.push(label(
                DraftPoint::new(end.x + 5.0, end.y - 8.0),
                format!("{name} = {value:.3} kN/m"),
                color,
            ));
        }
        layer.labels.push(label(
            DraftPoint::new(16.0, 16.0),
            format!(
                "Fs = {:.6}   A = {:.6} m²   V = {:.6} m³",
                result.factor_of_safety, result.geometry.area_m2, result.geometry.volume_m3
            ),
            [1.0, 0.79, 0.44, 1.0],
        ));
        layer.labels.push(label(
            DraftPoint::new(16.0, 36.0),
            "Dry planar model · force arrows are schematic · engineering review required".into(),
            [0.69, 0.77, 0.86, 1.0],
        ));
        layer
    }

    pub fn scene(&self, input: &SlopeAnalysisInput) -> Scene {
        let mut scene = Scene::new();
        let origin = input.section.slope_toe;
        let to_vec = |p: [f64; 3]| Vec3::new(p[0] - origin.x, p[1], p[2] - origin.z);
        let mesh_for = |indices: &[[u32; 3]]| {
            Mesh::new(
                indices
                    .iter()
                    .enumerate()
                    .map(|(id, tri)| {
                        Triangle::new(
                            id as u32,
                            to_vec(self.mesh.vertices[tri[0] as usize]),
                            to_vec(self.mesh.vertices[tri[1] as usize]),
                            to_vec(self.mesh.vertices[tri[2] as usize]),
                        )
                    })
                    .collect(),
            )
        };
        let mass_indices = self
            .mesh
            .triangles
            .iter()
            .copied()
            .filter(|tri| !self.mesh.slip_triangles.contains(tri))
            .collect::<Vec<_>>();
        let mut mass = SceneObject::new(
            1,
            "Potential failure mass · extruded 2D section",
            mesh_for(&mass_indices),
        );
        mass.color = [0.98, 0.72, 0.30, 0.58];
        scene.insert(mass);
        let mut slip = SceneObject::new(2, "Planar slip base", mesh_for(&self.mesh.slip_triangles));
        slip.color = [1.0, 0.22, 0.18, 1.0];
        scene.insert(slip);
        let width = input.section.extrusion_width_m;
        let mut terrain_triangles = Vec::new();
        // This open ribbon represents only the terrain surface. It adds no
        // synthetic soil depth or vertical cap to the calculated failure mass.
        let reverse_winding = matches!(
            (
                input.section.terrain_polyline.first(),
                input.section.terrain_polyline.last()
            ),
            (Some(first), Some(last)) if last.x < first.x
        );
        for pair in input.section.terrain_polyline.windows(2) {
            let a = to_vec([pair[0].x, 0.0, pair[0].z]);
            let b = to_vec([pair[1].x, 0.0, pair[1].z]);
            let c = Vec3::new(b.x, width, b.z);
            let d = Vec3::new(a.x, width, a.z);
            for mut vertices in [[a, b, c], [a, c, d]] {
                if reverse_winding {
                    vertices.swap(1, 2);
                }
                terrain_triangles.push(Triangle::new(
                    terrain_triangles.len() as u32,
                    vertices[0],
                    vertices[1],
                    vertices[2],
                ));
            }
        }
        let mut terrain = SceneObject::new(
            3,
            "Original terrain · open surface context",
            Mesh::new(terrain_triangles),
        );
        terrain.color = [0.50, 0.70, 0.50, 0.32];
        scene.insert(terrain);
        scene
    }
}

fn point(p: Point2) -> DraftPoint {
    DraftPoint::new(p.x, p.z)
}
fn segment(start: DraftPoint, end: DraftPoint, color: [f32; 4], width: f32) -> ScreenSegment {
    ScreenSegment {
        start,
        end,
        color,
        width,
    }
}
fn label(position: DraftPoint, text: String, color: [f32; 4]) -> ScreenLabel {
    ScreenLabel {
        position,
        text,
        color,
        placement: crate::drafting_canvas::ScreenLabelPlacement::Absolute,
    }
}
fn arrow(layer: &mut AnalysisCanvasLayer, start: DraftPoint, end: DraftPoint, color: [f32; 4]) {
    layer.segments.push(segment(start, end, color, 2.0));
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = dx.hypot(dy).max(1.0);
    for side in [-1.0, 1.0] {
        layer.segments.push(segment(
            end,
            DraftPoint::new(
                end.x - dx / len * 9.0 + side * dy / len * 4.5,
                end.y - dy / len * 9.0 - side * dx / len * 4.5,
            ),
            color,
            2.0,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn golden_input() -> SlopeAnalysisInput {
        SlopeAnalysisInput {
            section: geotech_core::triangular_section(10.0, 45.0, 30.0, 7.0).unwrap(),
            soil: geotech_core::SoilMaterial {
                name: "Golden fixture, demonstration".into(),
                unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 0.0,
                effective_friction_angle_deg: 30.0,
                provenance: "GEO-SLOPE-001 analytical fixture".into(),
            },
            conditions: geotech_core::AnalysisConditions::default(),
            units: geotech_core::InputUnits::default(),
            required_factor_of_safety: None,
            standard: None,
        }
    }
    #[test]
    fn geotech_render_preserves_section_and_separates_slip_from_mass() {
        let input = golden_input();
        let original = input.clone();
        let result = geotech_core::analyze(&input).unwrap();
        let render = GeotechRender::new(&input, &result).unwrap();
        let scene = render.scene(&input);
        let mass = scene.get(1).unwrap();
        let slip = scene.get(2).unwrap();
        assert_eq!(
            mass.mesh.triangles().len() + slip.mesh.triangles().len(),
            render.mesh.triangles.len()
        );
        assert_eq!(slip.mesh.triangles().len(), 2);
        assert!(mass.color[3] > 0.0 && mass.color[3] < 1.0);
        assert_eq!(slip.color[3], 1.0);
        assert_eq!(input, original);
        assert!((mass.mesh.bounds().unwrap().max.y - 7.0).abs() < 1e-12);
    }

    #[test]
    fn geotech_render_projected_fill_matches_f64_area_after_large_origin_and_camera_changes() {
        let mut input = golden_input();
        let offset = 1_000_000_000.0;
        for point in &mut input.section.terrain_polyline {
            point.x += offset;
            point.z += offset;
        }
        for point in [
            &mut input.section.slope_toe,
            &mut input.section.crest,
            &mut input.section.slip_surface.lower,
            &mut input.section.slip_surface.upper,
        ] {
            point.x += offset;
            point.z += offset;
        }
        let result = geotech_core::analyze(&input).unwrap();
        let original = result.clone();
        let render = GeotechRender::new(&input, &result).unwrap();
        let mut camera = Camera2D::new(1200.0, 900.0);
        camera.fit_bounds(render.bounds, 100.0).unwrap();
        for _ in 0..5 {
            camera.pan_pixels(2.5, -1.0).unwrap();
            camera.zoom_at(DraftPoint::new(600.0, 450.0), 1.01).unwrap();
            let layer = render.canvas(&input, &result, &camera);
            let area_pixels = layer
                .triangles
                .iter()
                .map(|tri| {
                    let [a, b, c] = tri.vertices;
                    ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5
                })
                .sum::<f64>();
            let area = area_pixels / camera.pixels_per_metre.powi(2);
            assert!((area - result.geometry.area_m2).abs() < 1e-8);
        }
        assert_eq!(result, original);
    }

    #[test]
    fn geotech_render_mirrored_slope_keeps_inclination_and_driving_direction() {
        let mut input = golden_input();
        for p in &mut input.section.terrain_polyline {
            p.x = -p.x;
        }
        input.section.slope_toe.x = -input.section.slope_toe.x;
        input.section.crest.x = -input.section.crest.x;
        input.section.slip_surface.lower.x = -input.section.slip_surface.lower.x;
        input.section.slip_surface.upper.x = -input.section.slip_surface.upper.x;
        let result = geotech_core::analyze(&input).unwrap();
        let render = GeotechRender::new(&input, &result).unwrap();
        let mut camera = Camera2D::new(1200.0, 900.0);
        camera.fit_bounds(render.bounds, 100.0).unwrap();
        let layer = render.canvas(&input, &result, &camera);
        assert!(layer.labels.iter().any(|label| label.text == "β = 45.000°"));
        let t = layer
            .labels
            .iter()
            .find(|label| label.text.starts_with("T ="))
            .unwrap();
        let polygon = &result.geometry.polygon;
        let center_x = polygon.iter().map(|p| p.x).sum::<f64>() / polygon.len() as f64;
        let center_z = polygon.iter().map(|p| p.z).sum::<f64>() / polygon.len() as f64;
        let center = camera.world_to_screen(DraftPoint::new(center_x, center_z));
        assert!(
            t.position.x > center.x,
            "downslope force must point right on the mirrored slope"
        );
    }
}
