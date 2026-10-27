//! Disposable circular-slope display geometry. No mesh enters the solver.
//!
//! The caller supplies one fresh, resolved Ground/Result snapshot. Camera and
//! OverlayFlags are display state and never participate in its fingerprint.
use crate::drafting_canvas::{
    AnalysisCanvasLayer, ScreenLabel, ScreenLabelPlacement, ScreenSegment, ScreenTriangle,
};
use drafting_core::{Bounds2, Camera2D, Point2 as DraftPoint};
use geotech_core::{
    Point2,
    circular::{
        self, ArcBranch, BoundCandidateTable, CircularAnalysisConfig, CircularSlipSurface,
        CircularSlopeResult, GroundSection2D, Slice,
    },
};
use std::collections::{BTreeMap, BTreeSet};
use viewer_core::{Mesh, Scene, SceneObject, Triangle, Vec3};

const ARC_SEGMENTS: usize = 192;
const MAX_RENDER_POINTS: usize = 32768;
const MAX_DISPLAY_TRIANGLES: usize = 131072;
const MAX_POSTCLIP_DISPLAY_TRIANGLES: usize = 131072;
const MAX_SCENE_TRIANGLES: usize = 524288;
const MAX_FORCE_SLICES: usize = 16;
const TERRAIN: [f32; 4] = [0.52, 0.83, 0.60, 1.0];
const SLIP: [f32; 4] = [1.0, 0.31, 0.25, 1.0];
const MASS: [f32; 4] = [0.98, 0.67, 0.26, 1.0];
const SLICES: [f32; 4] = [0.47, 0.85, 0.96, 1.0];
const TEXT: [f32; 4] = [0.87, 0.92, 0.98, 1.0];
const NEUTRAL: [f32; 4] = [0.35, 0.41, 0.47, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlayFlags {
    pub ground: bool,
    pub soil_layers: bool,
    pub failure_mass: bool,
    pub slices: bool,
    pub forces: bool,
    /// The selected result's lower arc; this does not certify a
    /// global critical surface or turn an ordinary analysis into a search.
    pub critical_circle: bool,
}
impl Default for OverlayFlags {
    fn default() -> Self {
        Self {
            ground: true,
            soil_layers: true,
            failure_mass: true,
            slices: true,
            forces: false,
            critical_circle: true,
        }
    }
}

struct RegionDisplay {
    material_id: String,
    name: String,
    color: [f32; 4],
    full: Vec<[Point2; 3]>,
    context: Vec<[Point2; 3]>,
    failure: Vec<[Point2; 3]>,
}
struct ForceGlyph {
    slice_id: usize,
    anchor: Point2,
    direction: Point2,
    value: f64,
    name: &'static str,
    color: [f32; 4],
}
struct ResultDisplay {
    arc: Vec<Point2>,
    mass: Vec<[Point2; 3]>,
    ground_context: Vec<[Point2; 3]>,
    slice_rings: Vec<Vec<Point2>>,
    forces: Vec<ForceGlyph>,
    force_stride: usize,
    center: Point2,
    entry: Point2,
    exit: Point2,
    radius_m: f64,
    summary: String,
    method_status: String,
    quality: String,
    physical_status: String,
}

/// All retained vertices use f64 section-local X/Z. `bounds` alone is in
/// authoritative section coordinates for the existing Camera2D::fit_bounds.
pub(super) struct GeotechCircularRender {
    pub bounds: Bounds2,
    pub origin: Point2,
    pub extrusion_width_m: f64,
    terrain: Vec<Point2>,
    bottom: Vec<Point2>,
    interfaces: Vec<Vec<Point2>>,
    ground_fill: Vec<[Point2; 3]>,
    regions: Vec<RegionDisplay>,
    result: Option<ResultDisplay>,
    section_name: String,
    local_scale: f64,
}

impl GeotechCircularRender {
    /// Validation is delegated to the core. This does not analyze a circle,
    /// recalculate a result, validate freshness, or infer material strengths.
    pub fn new(
        ground: &GroundSection2D,
        result: Option<&CircularSlopeResult>,
        extrusion_width_m: f64,
        config: &CircularAnalysisConfig,
    ) -> Result<Self, String> {
        // Establish the total bound before validation or copying vertices.
        ground
            .layer_interfaces
            .iter()
            .map(|boundary| boundary.polyline.len())
            .chain([ground.terrain_polyline.len(), ground.bottom_boundary.len()])
            .try_fold(0usize, |count, len| count.checked_add(len))
            .filter(|n| *n <= MAX_RENDER_POINTS)
            .ok_or("Ground exceeds the total display point budget")?;
        circular::validate_ground(ground, config).map_err(|e| e.to_string())?;
        if !extrusion_width_m.is_finite()
            || extrusion_width_m <= 0.0
            || extrusion_width_m > f32::MAX as f64 / 4.0
        {
            return Err("3D extrusion width is outside the finite display range".into());
        }
        let origin = ground.terrain_polyline[0];
        let local = |p: Point2| Point2::new(p.x - origin.x, p.z - origin.z);
        let terrain: Vec<_> = ground.terrain_polyline.iter().copied().map(local).collect();
        let bottom: Vec<_> = ground.bottom_boundary.iter().copied().map(local).collect();
        let interfaces: Vec<Vec<_>> = ground
            .layer_interfaces
            .iter()
            .map(|v| v.polyline.iter().copied().map(local).collect())
            .collect();
        let mut bounds = Bounds2::from_points(draft(origin), draft(origin));
        let mut local_scale: f64 = 0.0;
        for p in ground
            .terrain_polyline
            .iter()
            .chain(ground.bottom_boundary.iter())
            .chain(ground.layer_interfaces.iter().flat_map(|v| &v.polyline))
        {
            bounds = bounds.union(Bounds2::from_points(draft(*p), draft(*p)));
            let p = local(*p);
            if !display_point(p) {
                return Err("Ground exceeds the finite local display range".into());
            }
            local_scale = local_scale.max(p.x.abs()).max(p.z.abs());
        }
        let mut budget = MAX_DISPLAY_TRIANGLES;
        let ground_fill = strips(&terrain, &bottom, None, Clip::None, &mut budget)?;
        let mut resolved = if let Some(value) = result {
            if value.geometry.circle.surface.arc_branch != ArcBranch::Lower {
                return Err("Only the explicitly selected lower slip arc can be displayed".into());
            }
            if value.slices.len() > config.limits.max_slices
                || value.slices.len() > (MAX_RENDER_POINTS - ARC_SEGMENTS - 2) / 2
            {
                return Err("Result slices exceed the configured display input budget".into());
            }
            if value.fellenius_result.method != circular::CircularMethod::FelleniusOrdinary
                || value.bishop_result.method != circular::CircularMethod::BishopSimplified
            {
                return Err("Result method identity does not match the display contract".into());
            }
            if [
                value.fellenius_result.factor_of_safety,
                value.bishop_result.factor_of_safety,
                value.bishop_result.equation_residual,
                value
                    .bishop_result
                    .physical_assessment
                    .minimum_normal_kn_per_m,
                value.total_area_m2,
                value.total_weight_kn_per_m,
            ]
            .iter()
            .any(|v| !v.is_finite())
            {
                return Err("Nonfinite method summary in display snapshot".into());
            }
            let ring = circular::render_boundary(&value.geometry, ARC_SEGMENTS, MAX_RENDER_POINTS)
                .map_err(|e| e.to_string())?;
            // Remove the exact terrain vertices from the disposable ring,
            // leaving its lower-arc samples. This does not depend on which
            // vertex starts the CCW ring and never reconstructs an arc.
            let terrain_keys: BTreeSet<_> = value
                .geometry
                .terrain_boundary
                .iter()
                .copied()
                .map(point_key)
                .collect();
            let mut arc = Vec::with_capacity(ARC_SEGMENTS + value.slices.len() * 2 + 2);
            arc.push(local(value.geometry.circle.left_point));
            arc.extend(
                ring.iter()
                    .copied()
                    .filter(|p| !terrain_keys.contains(&point_key(*p)))
                    .map(local),
            );
            arc.push(local(value.geometry.circle.right_point));
            for slice in &value.slices {
                arc.push(local(slice.base_left));
                arc.push(local(slice.base_right));
            }
            arc.sort_by(|a, b| a.x.total_cmp(&b.x));
            arc.dedup_by(|a, b| a.x == b.x);
            if arc.len() > MAX_RENDER_POINTS
                || arc.iter().any(|p| !display_point(*p))
                || arc.windows(2).any(|v| v[1].x <= v[0].x)
            {
                return Err("Resolved arc is outside the bounded display contract".into());
            }
            let mass = strips(&terrain, &arc, None, Clip::None, &mut budget)?;
            let ground_context = strips(&terrain, &bottom, Some(&arc), Clip::Below, &mut budget)?;
            let center = local(value.geometry.circle.surface.center);
            let entry = local(value.geometry.circle.entry_point);
            let exit = local(value.geometry.circle.exit_point);
            if [center, entry, exit].iter().any(|p| !display_point(*p)) {
                return Err("Circle markers exceed the finite local display range".into());
            }
            for point in [
                value.geometry.circle.surface.center,
                value.geometry.circle.entry_point,
                value.geometry.circle.exit_point,
            ] {
                bounds = bounds.union(Bounds2::from_points(draft(point), draft(point)));
            }
            local_scale = local_scale.max(center.x.abs()).max(center.z.abs());
            let mut slice_rings = Vec::with_capacity(value.slices.len());
            let mut outline_points = 0usize;
            for slice in &value.slices {
                let a = slice.x_left - origin.x;
                let b = slice.x_right - origin.x;
                if !a.is_finite()
                    || !b.is_finite()
                    || a >= b
                    || a < arc[0].x
                    || b > arc.last().unwrap().x
                {
                    return Err("Slice does not lie in the displayed failure mass".into());
                }
                let mut perimeter = subline(&terrain, a, b);
                perimeter.extend(subline(&arc, a, b).into_iter().rev());
                outline_points = outline_points
                    .checked_add(perimeter.len())
                    .ok_or("Slice outline point count overflow")?;
                if outline_points > MAX_RENDER_POINTS {
                    return Err("Slice outline exceeds the display point budget".into());
                }
                slice_rings.push(perimeter);
            }
            let stride = value.slices.len().div_ceil(MAX_FORCE_SLICES).max(1);
            let mut forces = Vec::new();
            for slice in value.slices.iter().step_by(stride) {
                let contribution = value
                    .fellenius_result
                    .slice_contributions
                    .iter()
                    .find(|c| c.slice_id == slice.slice_id)
                    .ok_or("Missing Fellenius contribution for a displayed slice")?;
                let a = local(slice.base_left);
                let b = local(slice.base_right);
                let length = (b.x - a.x).hypot(b.z - a.z);
                if !length.is_finite() || length <= 0.0 {
                    return Err("Unresolved slice base in display snapshot".into());
                }
                let mid = Point2::new(a.x + (b.x - a.x) * 0.5, a.z + (b.z - a.z) * 0.5);
                let anchor = Point2::new(mid.x, mid.z + (eval(&terrain, mid.x) - mid.z) * 0.5);
                let sign = match value.geometry.circle.surface.slide_direction {
                    circular::SlideDirection::PositiveX => 1.0,
                    circular::SlideDirection::NegativeX => -1.0,
                };
                let tangent = Point2::new(sign * (b.x - a.x) / length, sign * (b.z - a.z) / length);
                for (name, direction, magnitude, color) in [
                    (
                        "W",
                        Point2::new(0.0, -1.0),
                        slice.weight_kn_per_m,
                        [0.90, 0.92, 1.0, 1.0],
                    ),
                    (
                        "N",
                        Point2::new(-(b.z - a.z) / length, (b.x - a.x) / length),
                        contribution.normal_kn_per_m,
                        [0.38, 0.76, 1.0, 1.0],
                    ),
                    ("D", tangent, contribution.driving_kn_per_m, SLIP),
                    (
                        "R",
                        Point2::new(-tangent.x, -tangent.z),
                        contribution.resistance_kn_per_m,
                        [0.43, 0.95, 0.63, 1.0],
                    ),
                ] {
                    if !magnitude.is_finite() {
                        return Err("Nonfinite force in display snapshot".into());
                    }
                    forces.push(ForceGlyph {
                        slice_id: slice.slice_id,
                        anchor,
                        direction,
                        value: magnitude,
                        name,
                        color,
                    });
                }
            }
            Some(ResultDisplay {
                arc,
                mass,
                ground_context,
                slice_rings,
                forces,
                force_stride: stride,
                center,
                entry,
                exit,
                radius_m: value.geometry.circle.surface.radius_m,
                summary: format!(
                    "{}Fellenius Fs = {:.6} · Bishop equation F = {:.6} · slices = {}",
                    if value.diagnostic_only
                        || !value.accepted
                        || value.bishop_result.accepted_factor_of_safety.is_none()
                    {
                        "DIAGNOSTIC_ONLY · "
                    } else {
                        ""
                    },
                    value.fellenius_result.factor_of_safety,
                    value.bishop_result.factor_of_safety,
                    value.slices.len()
                ),
                method_status: format!(
                    "Bishop: {:?} · iterations {} · residual {:.3e}",
                    value.bishop_result.convergence_status,
                    value.bishop_result.iterations.len(),
                    value.bishop_result.equation_residual,
                ),
                quality: format!(
                    "A {:.4} m² · W {:.4} kN/m\nСгущение: {} · численная проверка: {} · warnings {}",
                    value.total_area_m2,
                    value.total_weight_kn_per_m,
                    if value.discretization.refinement_checked {
                        "проверено"
                    } else {
                        "не проверено"
                    },
                    if value.numerically_verified {
                        "отмечена ядром"
                    } else {
                        "не выполнена"
                    },
                    value.warnings.len(),
                ),
                // Short canvas overview; full contact details remain in Results / Report.
                physical_status: format!(
                    "FINITE_SLICE_CONTACT {:?}\nmin N = {:+.6} kN/m\nКонцы: {} · whole arc: {}\nWhole arc independent: {} · critical contact: {}\nEngineering review required · Results / Report",
                    value.bishop_result.physical_assessment.status,
                    value.bishop_result.physical_assessment.minimum_normal_kn_per_m,
                    value.bishop_result.endpoint_assessment.as_ref().map_or(
                        "не проверены",
                        |assessment| if assessment.passed { "проверка пройдена" } else { "проверка не пройдена" },
                    ),
                    value.bishop_result.whole_arc_contact.as_ref().map_or(
                        "не проверена",
                        |report| match report.status {
                            circular::WholeArcContactStatus::PassedDeclaredModel
                                if report.coverage_complete => "проверка пройдена",
                            circular::WholeArcContactStatus::Violation => "проверка не пройдена",
                            _ => "не подтверждена",
                        },
                    ),
                    if value.bishop_result.whole_arc_contact.as_ref()
                        .is_some_and(|report| report.independent_whole_arc_acceptance) {
                        "отмечена"
                    } else {
                        "не выполнена"
                    },
                    if value.bishop_result.whole_arc_contact.as_ref().is_some_and(|report|
                        report.status == circular::WholeArcContactStatus::PassedDeclaredModel
                            && report.coverage_complete && report.eligible_for_critical_minimum
                    ) {
                        "допущен по контакту"
                    } else {
                        "не допущен"
                    },
                ),
            })
        } else {
            None
        };
        let boundaries: Vec<&[Point2]> = std::iter::once(terrain.as_slice())
            .chain(interfaces.iter().map(Vec::as_slice))
            .chain(std::iter::once(bottom.as_slice()))
            .collect();
        let mut regions = Vec::with_capacity(ground.soil_regions.len());
        for (i, region) in ground.soil_regions.iter().enumerate() {
            let material = ground
                .materials
                .iter()
                .find(|m| m.material_id == region.material_id)
                .ok_or("Missing validated Ground material")?;
            let full = strips(
                boundaries[i],
                boundaries[i + 1],
                None,
                Clip::None,
                &mut budget,
            )?;
            let (context, failure) = if let Some(result) = resolved.as_mut() {
                (
                    strips(
                        boundaries[i],
                        boundaries[i + 1],
                        Some(&result.arc),
                        Clip::Below,
                        &mut budget,
                    )?,
                    strips(
                        boundaries[i],
                        boundaries[i + 1],
                        Some(&result.arc),
                        Clip::Above,
                        &mut budget,
                    )?,
                )
            } else {
                (Vec::new(), Vec::new())
            };
            regions.push(RegionDisplay {
                material_id: material.material_id.0.clone(),
                name: format!(
                    "{} · {} · {}",
                    region.region_id.0, material.material_id.0, material.properties.name
                ),
                color: material_color(&material.material_id.0),
                full,
                context,
                failure,
            });
        }
        Ok(Self {
            bounds,
            origin,
            extrusion_width_m,
            terrain,
            bottom,
            interfaces,
            ground_fill,
            regions,
            result: resolved,
            section_name: ground.section_id.0.clone(),
            local_scale: local_scale.max(1e-6),
        })
    }

    /// A display-only colour change preserves the Ground/Result snapshot.
    pub fn set_material_color(&mut self, material_id: &str, color: [f32; 4]) -> Result<(), String> {
        if color
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Material display colour must contain finite channels in 0..=1".into());
        }
        let mut found = false;
        for region in &mut self.regions {
            if region.material_id == material_id {
                region.color = color;
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err("Unknown display material ID".into())
        }
    }

    pub fn canvas(&self, camera: &Camera2D, flags: OverlayFlags) -> AnalysisCanvasLayer {
        let mut output = AnalysisCanvasLayer::default();
        if camera.validate().is_err() {
            output.labels.push(label(
                DraftPoint::new(16.0, 16.0),
                "Circular display unavailable: invalid camera".into(),
                SLIP,
            ));
            return output;
        }
        // Subtract the world origin once, while still f64. Do not add local
        // vertices back to 1e9 before sending screen coordinates to the GPU.
        let mut local_camera = camera.clone();
        local_camera.center = DraftPoint::new(
            camera.center.x - self.origin.x,
            camera.center.y - self.origin.z,
        );
        let screen = |p: Point2| local_camera.world_to_screen(draft(p));
        if flags.soil_layers {
            for region in &self.regions {
                fill(&mut output, &region.full, region.color, &screen);
            }
        } else if flags.ground {
            fill(&mut output, &self.ground_fill, NEUTRAL, &screen);
        }
        if flags.ground {
            stroke(&mut output, &self.terrain, false, TERRAIN, 2.5, &screen);
            let mut perimeter = self.terrain.clone();
            perimeter.extend(self.bottom.iter().rev().copied());
            stroke(&mut output, &perimeter, true, NEUTRAL, 1.0, &screen);
        }
        if flags.soil_layers {
            for boundary in &self.interfaces {
                stroke(&mut output, boundary, false, TEXT, 1.2, &screen);
            }
            for (i, region) in self.regions.iter().enumerate() {
                output.labels.push(legend_label(
                    DraftPoint::new(16.0, 144.0 + i as f64 * 18.0),
                    region.name.clone(),
                    region.color,
                ));
            }
        }
        if let Some(result) = &self.result {
            if flags.failure_mass {
                fill(
                    &mut output,
                    &result.mass,
                    [MASS[0], MASS[1], MASS[2], 0.30],
                    &screen,
                );
            }
            if flags.slices {
                for ring in &result.slice_rings {
                    stroke(
                        &mut output,
                        ring,
                        true,
                        [SLICES[0], SLICES[1], SLICES[2], 0.7],
                        1.0,
                        &screen,
                    );
                }
            }
            if flags.critical_circle {
                stroke(&mut output, &result.arc, false, SLIP, 3.2, &screen);
                stroke(
                    &mut output,
                    &[result.center, result.entry],
                    false,
                    [0.85, 0.57, 0.43, 1.0],
                    1.0,
                    &screen,
                );
                for (name, point) in [
                    ("Center", result.center),
                    ("Entry", result.entry),
                    ("Exit", result.exit),
                ] {
                    let p = screen(point);
                    cross(&mut output, p, TEXT);
                    output.labels.push(callout_label(
                        DraftPoint::new(p.x + 7.0, p.y - 18.0),
                        format!(
                            "{name} ({:.3}, {:.3}) m",
                            point.x + self.origin.x,
                            point.z + self.origin.z
                        ),
                        TEXT,
                    ));
                }
                let p = screen(result.center);
                output.labels.push(callout_label(
                    DraftPoint::new(p.x + 7.0, p.y + 7.0),
                    format!("R = {:.3} m · lower arc", result.radius_m),
                    SLIP,
                ));
            }
            if flags.forces {
                for (i, glyph) in result.forces.iter().enumerate() {
                    if glyph.value == 0.0 {
                        continue;
                    }
                    let p = screen(glyph.anchor);
                    let offset = (i % 4) as f64 * 4.0 - 6.0;
                    let start = DraftPoint::new(p.x + offset, p.y - offset);
                    let sign = glyph.value.signum();
                    let end = DraftPoint::new(
                        start.x + glyph.direction.x * sign * 38.0,
                        start.y - glyph.direction.z * sign * 38.0,
                    );
                    arrow(&mut output, start, end, glyph.color);
                    // Full signed per-slice values remain in the existing table/report.
                    // Repeating all of them on this narrow canvas obscures the arrows.
                }
                output.labels.push(legend_label(
                    DraftPoint::new(16.0, 120.0),
                    format!(
                        "Fellenius W/N/D/R · signed arrows every {} slice(s) · kN/m values: table / Report",
                        result.force_stride
                    ),
                    TEXT,
                ));
            }
            output.labels.push(label(
                DraftPoint::new(16.0, 16.0),
                result.summary.clone(),
                MASS,
            ));
            output.labels.push(label(
                DraftPoint::new(16.0, 36.0),
                result.method_status.clone(),
                TEXT,
            ));
            output.labels.push(label(
                DraftPoint::new(16.0, 56.0),
                result.quality.clone(),
                TEXT,
            ));
            output.labels.push(label(
                DraftPoint::new(16.0, 76.0),
                result.physical_status.clone(),
                TEXT,
            ));
        } else {
            output.labels.push(label(
                DraftPoint::new(16.0, 16.0),
                format!(
                    "Ground {} · result layers not shown",
                    self.section_name
                ),
                TEXT,
            ));
        }
        output.labels.push(footer_label(
            DraftPoint::new(16.0, 96.0),
            "Dry static 2D · self-weight only\n3D EXTRUSION OF 2D MODEL · NOT SPATIAL EQUILIBRIUM".into(),
            TEXT,
        ));
        clip_canvas(output, camera.viewport_width, camera.viewport_height)
    }

    /// A cutaway context prevents opaque geological faces from hiding the
    /// failure mass. All objects are disposable, with section-local vertices.
    pub fn scene(&self, flags: OverlayFlags) -> Result<Scene, String> {
        let mut builder = SceneBuilder::default();
        let mass_visible = flags.failure_mass && self.result.is_some();
        if flags.soil_layers {
            for region in &self.regions {
                let context = if mass_visible {
                    &region.context
                } else {
                    &region.full
                };
                builder.extruded(
                    format!("Ground layer {}", region.name),
                    context,
                    self.extrusion_width_m,
                    region.color,
                )?;
                if mass_visible {
                    let color = [
                        0.65 * region.color[0] + 0.35 * MASS[0],
                        0.65 * region.color[1] + 0.35 * MASS[1],
                        0.65 * region.color[2] + 0.35 * MASS[2],
                        1.0,
                    ];
                    builder.extruded(
                        format!("Potential failure mass / {} · 2D extrusion", region.name),
                        &region.failure,
                        self.extrusion_width_m,
                        color,
                    )?;
                }
            }
        } else if flags.ground {
            let context = if mass_visible {
                &self.result.as_ref().unwrap().ground_context
            } else {
                &self.ground_fill
            };
            builder.extruded(
                "Ground context · 2D extrusion".into(),
                context,
                self.extrusion_width_m,
                NEUTRAL,
            )?;
        }
        if flags.ground {
            builder.sheet(
                "Terrain surface".into(),
                &self.terrain,
                self.extrusion_width_m,
                TERRAIN,
            )?;
        }
        if let Some(result) = &self.result {
            if flags.failure_mass && !flags.soil_layers {
                builder.extruded(
                    "Potential failure mass · extruded 2D section".into(),
                    &result.mass,
                    self.extrusion_width_m,
                    MASS,
                )?;
            }
            let thickness = self.local_scale * 0.0007;
            let front = -thickness * 3.0;
            if flags.slices {
                let mut lines = Vec::new();
                for ring in &result.slice_rings {
                    for i in 0..ring.len() {
                        lines.push((ring[i], ring[(i + 1) % ring.len()]));
                    }
                }
                builder.lines(
                    "Slice outlines · front".into(),
                    &lines,
                    front,
                    thickness,
                    SLICES,
                )?;
                builder.lines(
                    "Slice outlines · rear".into(),
                    &lines,
                    self.extrusion_width_m - front,
                    thickness,
                    SLICES,
                )?;
            }
            if flags.critical_circle {
                builder.sheet(
                    "Selected lower circular slip surface · 2D extrusion".into(),
                    &result.arc,
                    self.extrusion_width_m,
                    SLIP,
                )?;
                let mut markers = result
                    .arc
                    .windows(2)
                    .map(|p| (p[0], p[1]))
                    .collect::<Vec<_>>();
                markers.push((result.center, result.entry));
                let size = self.local_scale * 0.008;
                for p in [result.center, result.entry, result.exit] {
                    markers.push((Point2::new(p.x - size, p.z), Point2::new(p.x + size, p.z)));
                    markers.push((Point2::new(p.x, p.z - size), Point2::new(p.x, p.z + size)));
                }
                builder.lines(
                    format!(
                        "Circle center / entry / exit · R={:.6} m · lower arc",
                        result.radius_m
                    ),
                    &markers,
                    front * 2.0,
                    thickness * 1.5,
                    SLIP,
                )?;
            }
            if flags.forces {
                let length = self.local_scale * 0.045;
                for (index, color) in [
                    (0, [0.90, 0.92, 1.0, 1.0]),
                    (1, [0.38, 0.76, 1.0, 1.0]),
                    (2, SLIP),
                    (3, [0.43, 0.95, 0.63, 1.0]),
                ] {
                    let mut lines = Vec::new();
                    for glyph in result.forces.iter().skip(index).step_by(4) {
                        if glyph.value == 0.0 {
                            continue;
                        }
                        let dx = glyph.direction.x * glyph.value.signum() * length;
                        let dz = glyph.direction.z * glyph.value.signum() * length;
                        let end = Point2::new(glyph.anchor.x + dx, glyph.anchor.z + dz);
                        lines.push((glyph.anchor, end));
                        lines.push((
                            end,
                            Point2::new(
                                end.x - dx * 0.25 - dz * 0.12,
                                end.z - dz * 0.25 + dx * 0.12,
                            ),
                        ));
                        lines.push((
                            end,
                            Point2::new(
                                end.x - dx * 0.25 + dz * 0.12,
                                end.z - dz * 0.25 - dx * 0.12,
                            ),
                        ));
                    }
                    builder.lines(
                        format!(
                            "Fellenius {} arrows · schematic · signed forces retained",
                            ["W", "N", "D", "R"][index]
                        ),
                        &lines,
                        front * 3.0,
                        thickness,
                        color,
                    )?;
                }
            }
        }
        Ok(builder.scene)
    }
}

// Local selection paths are diagnostic wire overlays, independent of broad
// accepted-result overlays. They never produce fill, forces, Fs or contacts.
const MAX_LOCAL_SELECTION_POINTS: usize = MAX_RENDER_POINTS;
struct SelectionPath {
    points: Vec<Point2>,
    closed: bool,
    color: [f32; 4],
    label: String,
}
pub(super) struct LocalSelectionOverlay {
    origin: Point2,
    width: Option<f64>,
    pub bounds: Bounds2,
    paths: Vec<SelectionPath>,
}
impl LocalSelectionOverlay {
    pub fn new(
        origin: Point2,
        width: Option<f64>,
        circle: Option<&CircularSlipSurface>,
        result: Option<&CircularSlopeResult>,
        slice_id: Option<usize>,
        candidate: Option<&BoundCandidateTable>,
    ) -> Result<Option<Self>, String> {
        let mut paths = Vec::with_capacity(3);
        if let Some(circle) = circle {
            paths.push(SelectionPath {
                points: lower_selection_arc(circle, origin, None)?,
                closed: false,
                color: [0.65, 0.65, 1.0, 1.0],
                label: "Предпросмотр нижней полуокружности · контакты с грунтом не определены; без Fs и массы".into(),
            });
        }
        if let Some(id) = slice_id {
            let result = result.ok_or("SELECTED_SLICE_RESULT_NOT_CURRENT")?;
            paths.push(selected_slice_path(
                &result.geometry.terrain_boundary,
                &result.slices,
                id,
                origin,
                result.diagnostic_only || !result.accepted,
            )?);
        }
        if let Some(table) = candidate {
            let resolved = &table.evaluation.geometry.circle;
            paths.push(SelectionPath {
                points: lower_selection_arc(
                    &resolved.surface,
                    origin,
                    Some((resolved.left_point, resolved.right_point)),
                )?,
                closed: false,
                color: [1.0, 0.65, 0.95, 1.0],
                label: format!(
                    "Кандидат {} · дуга точной финальной таблицы; просмотр не разрешает применение",
                    table.candidate_id
                ),
            });
        }
        Self::finish(origin, width, paths)
    }
    fn finish(
        origin: Point2,
        width: Option<f64>,
        paths: Vec<SelectionPath>,
    ) -> Result<Option<Self>, String> {
        if paths.is_empty() {
            return Ok(None);
        }
        if !origin.x.is_finite()
            || !origin.z.is_finite()
            || width.is_some_and(|w| !w.is_finite() || w <= 0.0 || w > f32::MAX as f64 / 4.0)
        {
            return Err("LOCAL_SELECTION_DISPLAY_RANGE".into());
        }
        paths
            .iter()
            .try_fold(0usize, |n, p| n.checked_add(p.points.len()))
            .filter(|n| *n <= MAX_LOCAL_SELECTION_POINTS)
            .ok_or("LOCAL_SELECTION_POINT_BUDGET")?;
        let mut bounds: Option<Bounds2> = None;
        for point in paths.iter().flat_map(|path| &path.points) {
            if !display_point(*point) {
                return Err("LOCAL_SELECTION_DISPLAY_RANGE".into());
            }
            let world = Point2::new(point.x + origin.x, point.z + origin.z);
            if !world.x.is_finite() || !world.z.is_finite() {
                return Err("LOCAL_SELECTION_WORLD_RANGE".into());
            }
            let point_bounds = Bounds2::from_points(draft(world), draft(world));
            bounds = Some(bounds.map_or(point_bounds, |b| b.union(point_bounds)));
        }
        Ok(Some(Self {
            origin,
            width,
            bounds: bounds.ok_or("LOCAL_SELECTION_EMPTY_PATH")?,
            paths,
        }))
    }
    pub fn summary(&self) -> String {
        self.paths
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    }
    pub fn canvas(&self, camera: &Camera2D) -> AnalysisCanvasLayer {
        if camera.validate().is_err() {
            return AnalysisCanvasLayer::default();
        }
        let mut local_camera = camera.clone();
        local_camera.center = DraftPoint::new(
            camera.center.x - self.origin.x,
            camera.center.y - self.origin.z,
        );
        let screen = |p: Point2| local_camera.world_to_screen(draft(p));
        let mut layer = AnalysisCanvasLayer::default();
        for (i, path) in self.paths.iter().enumerate() {
            stroke(
                &mut layer,
                &path.points,
                path.closed,
                path.color,
                3.5,
                &screen,
            );
            layer.labels.push(footer_label(
                DraftPoint::new(
                    16.0,
                    (camera.viewport_height - 20.0 * (self.paths.len() - i) as f64).max(16.0),
                ),
                path.label.clone(),
                path.color,
            ));
        }
        clip_canvas(layer, camera.viewport_width, camera.viewport_height)
    }
    pub fn append_scene(&self, scene: Scene) -> Result<Scene, String> {
        let width = self.width.ok_or("LOCAL_SELECTION_3D_WIDTH_NOT_SET")?;
        let next_id = scene.object_ids().max().unwrap_or(0);
        next_id
            .checked_add((self.paths.len() * 2) as u64)
            .ok_or("LOCAL_SELECTION_OBJECT_ID_OVERFLOW")?;
        let triangles = scene
            .objects()
            .try_fold(0usize, |n, o| n.checked_add(o.mesh.triangles().len()))
            .filter(|n| *n <= MAX_SCENE_TRIANGLES)
            .ok_or("LOCAL_SELECTION_SCENE_BUDGET")?;
        let mut builder = SceneBuilder {
            scene,
            next_id,
            triangles,
        };
        let scale = self
            .paths
            .iter()
            .flat_map(|p| &p.points)
            .fold(1e-6_f64, |s, p| s.max(p.x.abs()).max(p.z.abs()));
        let thickness = scale * 0.0008;
        for path in &self.paths {
            let mut lines = path
                .points
                .windows(2)
                .map(|p| (p[0], p[1]))
                .collect::<Vec<_>>();
            if path.closed && path.points.len() > 2 {
                lines.push((*path.points.last().unwrap(), path.points[0]));
            }
            for (name, depth) in [
                ("front", -thickness * 6.0),
                ("rear", width + thickness * 6.0),
            ] {
                builder.lines(
                    format!("{} · {} · диагностическая 2D-экструзия", path.label, name),
                    &lines,
                    depth,
                    thickness,
                    path.color,
                )?;
            }
        }
        Ok(builder.scene)
    }
}
fn lower_selection_arc(
    surface: &CircularSlipSurface,
    origin: Point2,
    endpoints: Option<(Point2, Point2)>,
) -> Result<Vec<Point2>, String> {
    if surface.arc_branch != ArcBranch::Lower
        || !surface.radius_m.is_finite()
        || surface.radius_m <= 0.0
        || !surface.center.x.is_finite()
        || !surface.center.z.is_finite()
        || !origin.x.is_finite()
        || !origin.z.is_finite()
    {
        return Err("LOCAL_CIRCLE_INVALID_LOWER_BRANCH".into());
    }
    let center = Point2::new(surface.center.x - origin.x, surface.center.z - origin.z);
    let radius = surface.radius_m;
    if !display_point(center)
        || radius > f32::MAX as f64 / 8.0
        || !(surface.center.x - radius < surface.center.x + radius)
    {
        return Err("LOCAL_CIRCLE_DISPLAY_RANGE".into());
    }
    let local = |p: Point2| Point2::new(p.x - origin.x, p.z - origin.z);
    let (left_angle, right_angle) = if let Some((left, right)) = endpoints {
        if left.x >= right.x {
            return Err("LOCAL_CANDIDATE_ARC_ORDER".into());
        }
        let tolerance = 64.0
            * f64::EPSILON
            * surface
                .center
                .x
                .abs()
                .max(surface.center.z.abs())
                .max(radius)
                .max(1.0);
        let angle = |p: Point2| -> Result<f64, String> {
            let dx = p.x - surface.center.x;
            let dz = surface.center.z - p.z;
            if !display_point(local(p)) || dz < 0.0 || (dx.hypot(dz) - radius).abs() > tolerance {
                return Err("LOCAL_CANDIDATE_ARC_ENDPOINT_MISMATCH".into());
            }
            Ok(dz.atan2(dx))
        };
        let a = angle(left)?;
        let b = angle(right)?;
        if a <= b {
            return Err("LOCAL_CANDIDATE_ARC_ORDER".into());
        }
        (a, b)
    } else {
        (std::f64::consts::PI, 0.0)
    };
    let mut points = Vec::with_capacity(ARC_SEGMENTS + 1);
    for i in 0..=ARC_SEGMENTS {
        let angle = left_angle + (right_angle - left_angle) * i as f64 / ARC_SEGMENTS as f64;
        let p = Point2::new(
            center.x + radius * angle.cos(),
            center.z - radius * angle.sin(),
        );
        if !display_point(p) {
            return Err("LOCAL_CIRCLE_DISPLAY_RANGE".into());
        }
        points.push(p);
    }
    // Retained endpoints remain exact. The unbound preview endpoints are circle
    // extrema only, explicitly not terrain contacts or a resolved failure mass.
    let (left, right) = endpoints.map_or(
        (
            Point2::new(center.x - radius, center.z),
            Point2::new(center.x + radius, center.z),
        ),
        |(a, b)| (local(a), local(b)),
    );
    points[0] = left;
    points[ARC_SEGMENTS] = right;
    Ok(points)
}
fn selected_slice_path(
    terrain: &[Point2],
    slices: &[Slice],
    id: usize,
    origin: Point2,
    diagnostic: bool,
) -> Result<SelectionPath, String> {
    if !(2..=MAX_LOCAL_SELECTION_POINTS - 2).contains(&terrain.len())
        || slices.len() > MAX_RENDER_POINTS
        || terrain.iter().any(|p| !p.x.is_finite() || !p.z.is_finite())
        || terrain.windows(2).any(|p| p[0].x >= p[1].x)
    {
        return Err("LOCAL_SLICE_SOURCE_GEOMETRY_INVALID".into());
    }
    let mut found = slices.iter().filter(|s| s.slice_id == id);
    let slice = found.next().ok_or("LOCAL_SLICE_ID_NOT_FOUND")?;
    if found.next().is_some() {
        return Err("LOCAL_SLICE_ID_AMBIGUOUS".into());
    }
    let local = |p: Point2| Point2::new(p.x - origin.x, p.z - origin.z);
    let left = slice.x_left - origin.x;
    let right = slice.x_right - origin.x;
    let base_left = local(slice.base_left);
    let base_right = local(slice.base_right);
    let terrain = terrain.iter().copied().map(local).collect::<Vec<_>>();
    if terrain.iter().any(|p| !display_point(*p))
        || !display_point(base_left)
        || !display_point(base_right)
        || !left.is_finite()
        || !right.is_finite()
        || left >= right
        || base_left.x != left
        || base_right.x != right
        || left < terrain[0].x
        || right > terrain.last().unwrap().x
    {
        return Err("LOCAL_SLICE_SOURCE_RANGE_MISMATCH".into());
    }
    let mut points = subline(&terrain, left, right);
    // The stored Slice base is a chord. Use its actual endpoints and label it;
    // do not invent curved material areas or a new failure mass for selection.
    points.extend([base_right, base_left]);
    Ok(SelectionPath {
        points,
        closed: true,
        color: [1.0, 0.91, 0.30, 1.0],
        label: format!(
            "Выбранный отсек {id} · {}контур текущей таблицы; основание — хорда",
            if diagnostic {
                "диагностика · "
            } else {
                ""
            }
        ),
    })
}

/// Draft-only ground input preview. It retains supplied boundary polylines in
/// section-local f64 coordinates and never constructs soil fills or analyzes
/// a circular slip surface.
pub(super) struct GroundDraftRender {
    pub bounds: Bounds2,
    origin: Point2,
    extrusion_width_m: Option<f64>,
    terrain: Vec<Point2>,
    interfaces: Vec<(String, Vec<Point2>)>,
    bottom: Vec<Point2>,
    materials: Vec<(String, [f32; 4])>,
    readiness: String,
}

impl GroundDraftRender {
    pub fn view_context(&self) -> (Point2, Option<f64>) {
        (self.origin, self.extrusion_width_m)
    }
    /// Builds a diagnostic preview from the supplied definition only. The
    /// foundation module remains the sole owner of ground readiness.
    pub fn new(
        definition: &circular::GroundDefinition,
        extrusion_width_m: Option<f64>,
    ) -> Result<Self, String> {
        let readiness =
            circular::inspect_ground_readiness(definition).map_err(|error| error.to_string())?;
        let _point_count = definition
            .layer_interfaces
            .iter()
            .map(|boundary| boundary.polyline.len())
            .chain([
                definition.terrain_polyline.len(),
                definition.bottom_boundary.len(),
            ])
            .try_fold(0usize, |count, len| count.checked_add(len))
            .filter(|count| *count <= MAX_RENDER_POINTS)
            .ok_or("Ground draft exceeds the total display point budget")?;
        if extrusion_width_m.is_some_and(|width| {
            !width.is_finite() || width <= 0.0 || width > f32::MAX as f64 / 4.0
        }) {
            return Err("3D extrusion width is outside the finite display range".into());
        }

        let origin = std::iter::once(definition.terrain_polyline.as_slice())
            .chain(
                definition
                    .layer_interfaces
                    .iter()
                    .map(|interface| interface.polyline.as_slice()),
            )
            .chain(std::iter::once(definition.bottom_boundary.as_slice()))
            .flat_map(|line| line.iter().copied())
            .next()
            .unwrap_or_else(|| Point2::new(0.0, 0.0));
        let mut bounds = Bounds2::from_points(draft(origin), draft(origin));
        let terrain =
            localize_ground_draft_line(&definition.terrain_polyline, origin, &mut bounds)?;
        let interfaces = definition
            .layer_interfaces
            .iter()
            .map(|interface| {
                Ok((
                    interface.interface_id.0.clone(),
                    localize_ground_draft_line(&interface.polyline, origin, &mut bounds)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let bottom = localize_ground_draft_line(&definition.bottom_boundary, origin, &mut bounds)?;
        let materials = definition
            .materials
            .iter()
            .map(|material| {
                (
                    format!(
                        "Material {}: {}",
                        material.material_id.0, material.properties.name
                    ),
                    material_color(&material.material_id.0),
                )
            })
            .collect();
        let state = match readiness.state {
            circular::GroundReadinessState::Draft => "DRAFT",
            circular::GroundReadinessState::Ready => "READY",
        };
        let missing = readiness
            .missing
            .iter()
            .map(|part| match part {
                circular::GroundMissingPart::TerrainPolyline => "terrain polyline",
                circular::GroundMissingPart::LayerInterfaces => "layer interfaces",
                circular::GroundMissingPart::SoilRegions => "soil regions",
                circular::GroundMissingPart::Materials => "materials",
                circular::GroundMissingPart::BottomBoundary => "bottom boundary",
                circular::GroundMissingPart::BoundaryCoverage => "boundary coverage",
            })
            .collect::<Vec<_>>();
        let readiness = if missing.is_empty() {
            format!("Ground readiness (core): {state}")
        } else {
            format!(
                "Ground readiness (core): {state} · missing: {}",
                missing.join(", ")
            )
        };

        Ok(Self {
            bounds,
            origin,
            extrusion_width_m,
            terrain,
            interfaces,
            bottom,
            materials,
            readiness,
        })
    }

    /// Draws supplied vertices and line segments only. It adds no filled
    /// layers, failure mass, or circular geometry.
    pub fn canvas(&self, camera: &Camera2D) -> AnalysisCanvasLayer {
        let mut output = AnalysisCanvasLayer::default();
        if camera.validate().is_err() {
            output.labels.push(label(
                DraftPoint::new(16.0, 16.0),
                "Ground draft preview unavailable: invalid camera".into(),
                SLIP,
            ));
            return output;
        }
        let fit_center = self.bounds.center();
        if !fit_center.is_finite() || fit_center.x.abs() > 1e12 || fit_center.y.abs() > 1e12 {
            output.labels.push(label(
                DraftPoint::new(16.0, 16.0),
                "Ground draft preview unavailable: fit center exceeds Camera2D's ±1e12 coordinate limit".into(),
                SLIP,
            ));
            return output;
        }

        // Keep projection camera-relative in f64 so large authoritative world
        // origins do not pass through the GPU coordinate range.
        let mut local_camera = camera.clone();
        local_camera.center = DraftPoint::new(
            camera.center.x - self.origin.x,
            camera.center.y - self.origin.z,
        );
        let screen = |point: Point2| local_camera.world_to_screen(draft(point));
        draw_ground_draft_line(&mut output, &self.terrain, TERRAIN, &screen);
        for (_, line) in &self.interfaces {
            draw_ground_draft_line(&mut output, line, TEXT, &screen);
        }
        draw_ground_draft_line(&mut output, &self.bottom, NEUTRAL, &screen);

        output.labels.push(label(
            DraftPoint::new(16.0, 16.0),
            self.readiness.clone(),
            TEXT,
        ));
        output.labels.push(label(
            DraftPoint::new(16.0, 36.0),
            "DIAGNOSTIC ONLY · supplied ground preview · no calculation".into(),
            TEXT,
        ));
        for (index, (name, color)) in self.materials.iter().enumerate() {
            output.labels.push(label(
                DraftPoint::new(16.0, 60.0 + index as f64 * 18.0),
                name.clone(),
                *color,
            ));
        }
        for (index, (id, line)) in self.interfaces.iter().enumerate() {
            if !line.is_empty() {
                output.labels.push(label(
                    DraftPoint::new(16.0, 60.0 + (self.materials.len() + index) as f64 * 18.0),
                    format!("Supplied interface {id}"),
                    TEXT,
                ));
            }
        }
        if !self.terrain.is_empty() {
            output.labels.push(label(
                DraftPoint::new(
                    16.0,
                    60.0 + (self.materials.len() + self.interfaces.len()) as f64 * 18.0,
                ),
                "Supplied terrain polyline".into(),
                TERRAIN,
            ));
        }
        if !self.bottom.is_empty() {
            output.labels.push(label(
                DraftPoint::new(
                    16.0,
                    60.0 + (self.materials.len() + self.interfaces.len() + 1) as f64 * 18.0,
                ),
                "Supplied bottom boundary".into(),
                NEUTRAL,
            ));
        }
        clip_canvas(output, camera.viewport_width, camera.viewport_height)
    }

    /// Creates only line extrusions of supplied boundaries. Every object is
    /// marked diagnostic because this display performs no calculation.
    pub fn scene(&self) -> Result<Scene, String> {
        let mut builder = SceneBuilder::default();
        if self.terrain.len() < 2
            && self.bottom.len() < 2
            && self.interfaces.iter().all(|(_, line)| line.len() < 2)
        {
            return Ok(builder.scene);
        }
        let width = self.extrusion_width_m.ok_or(
            "Укажите ширину диагностической 3D-экструзии; 2D-предпросмотр доступен без неё",
        )?;
        if self.terrain.len() >= 2 {
            builder.sheet(
                "DIAGNOSTIC ONLY · terrain boundary line extrusion".into(),
                &self.terrain,
                width,
                TERRAIN,
            )?;
        }
        for (id, line) in &self.interfaces {
            if line.len() >= 2 {
                builder.sheet(
                    format!("DIAGNOSTIC ONLY · interface {id} line extrusion"),
                    line,
                    width,
                    TEXT,
                )?;
            }
        }
        if self.bottom.len() >= 2 {
            builder.sheet(
                "DIAGNOSTIC ONLY · bottom boundary line extrusion".into(),
                &self.bottom,
                width,
                NEUTRAL,
            )?;
        }
        Ok(builder.scene)
    }
}

fn localize_ground_draft_line(
    line: &[Point2],
    origin: Point2,
    bounds: &mut Bounds2,
) -> Result<Vec<Point2>, String> {
    line.iter()
        .map(|point| {
            *bounds = (*bounds).union(Bounds2::from_points(draft(*point), draft(*point)));
            let local = Point2::new(point.x - origin.x, point.z - origin.z);
            if !display_point(local) {
                return Err("Ground draft exceeds the finite local display range".into());
            }
            Ok(local)
        })
        .collect()
}

fn draw_ground_draft_line(
    output: &mut AnalysisCanvasLayer,
    line: &[Point2],
    color: [f32; 4],
    screen: &impl Fn(Point2) -> DraftPoint,
) {
    stroke(output, line, false, color, 2.0, screen);
    for point in line {
        cross(output, screen(*point), color);
    }
}

#[derive(Clone, Copy)]
enum Clip {
    None,
    Below,
    Above,
}

/// Piecewise-linear *display* strips, with optional cutaway along the already
/// tessellated core boundary. These areas are never used as numerical output.
fn strips(
    upper: &[Point2],
    lower: &[Point2],
    arc: Option<&[Point2]>,
    clip: Clip,
    budget: &mut usize,
) -> Result<Vec<[Point2; 3]>, String> {
    for line in [upper, lower].into_iter().chain(arc) {
        if line.len() < 2
            || line.iter().any(|p| !display_point(*p))
            || line.windows(2).any(|pair| pair[0].x >= pair[1].x)
        {
            return Err("Display strips require finite, strictly ordered boundary points".into());
        }
    }
    let left = upper[0].x.max(lower[0].x);
    let right = upper.last().unwrap().x.min(lower.last().unwrap().x);
    if left >= right {
        return Err("Display strips have no positive overlap".into());
    }
    let mut xs = vec![left, right];
    for p in upper.iter().chain(lower).chain(arc.unwrap_or(&[])) {
        if p.x > left && p.x < right {
            xs.push(p.x);
        }
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    if xs.len() > MAX_RENDER_POINTS {
        return Err("Display strip point budget exceeded".into());
    }
    let mut output = Vec::new();
    for pair in xs.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut polygon = vec![
            Point2::new(a, eval(upper, a)),
            Point2::new(a, eval(lower, a)),
            Point2::new(b, eval(lower, b)),
            Point2::new(b, eval(upper, b)),
        ];
        if let Some(arc) = arc {
            let inside = a >= arc[0].x && b <= arc.last().unwrap().x;
            if inside {
                let z0 = eval(arc, a);
                let z1 = eval(arc, b);
                polygon = clip_polygon(
                    polygon,
                    Point2::new(a, z0),
                    Point2::new(b, z1),
                    matches!(clip, Clip::Below),
                );
            } else if matches!(clip, Clip::Above) {
                continue;
            }
        }
        for i in 1..polygon.len().saturating_sub(1) {
            let tri = [polygon[0], polygon[i], polygon[i + 1]];
            let area = cross_value(tri[0], tri[1], tri[2]);
            if !area.is_finite() {
                return Err("Display triangulation overflow".into());
            }
            if area == 0.0 {
                continue;
            }
            *budget = budget
                .checked_sub(1)
                .ok_or("Display triangle budget exceeded")?;
            output.push(tri);
        }
    }
    Ok(output)
}
fn clip_polygon(input: Vec<Point2>, a: Point2, b: Point2, below: bool) -> Vec<Point2> {
    let mut output = Vec::with_capacity(6);
    for i in 0..input.len() {
        let p = input[i];
        let q = input[(i + 1) % input.len()];
        let dp = cross_value(a, b, p);
        let dq = cross_value(a, b, q);
        let inside_p = if below { dp <= 0.0 } else { dp >= 0.0 };
        let inside_q = if below { dq <= 0.0 } else { dq >= 0.0 };
        if inside_p {
            output.push(p);
        }
        if inside_p != inside_q {
            // Adjacent strips traverse their shared vertical edge in opposite
            // directions. Reuse the common clip endpoint so both caps retain
            // the same vertex, without welding unrelated nearby geometry.
            let intersection = if p.x == q.x && p.x == a.x {
                a
            } else if p.x == q.x && p.x == b.x {
                b
            } else {
                let t = dp / (dp - dq);
                Point2::new(
                    (q.x - p.x).mul_add(t, p.x),
                    (q.z - p.z).mul_add(t, p.z),
                )
            };
            output.push(intersection);
        }
    }
    output
}
fn cross_value(a: Point2, b: Point2, p: Point2) -> f64 {
    (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x)
}
fn eval(line: &[Point2], x: f64) -> f64 {
    let i = line
        .partition_point(|p| p.x <= x)
        .saturating_sub(1)
        .min(line.len() - 2);
    let (a, b) = (line[i], line[i + 1]);
    ((x - a.x) / (b.x - a.x)).mul_add(b.z - a.z, a.z)
}
fn subline(line: &[Point2], a: f64, b: f64) -> Vec<Point2> {
    let mut output = vec![Point2::new(a, eval(line, a))];
    output.extend(line.iter().copied().filter(|p| p.x > a && p.x < b));
    output.push(Point2::new(b, eval(line, b)));
    output
}
fn display_point(p: Point2) -> bool {
    p.x.is_finite()
        && p.z.is_finite()
        && p.x.abs() < f32::MAX as f64 / 4.0
        && p.z.abs() < f32::MAX as f64 / 4.0
}
fn draft(p: Point2) -> DraftPoint {
    DraftPoint::new(p.x, p.z)
}
fn material_color(id: &str) -> [f32; 4] {
    const COLORS: [[f32; 4]; 8] = [
        [0.70, 0.56, 0.36, 1.0],
        [0.44, 0.64, 0.77, 1.0],
        [0.63, 0.54, 0.77, 1.0],
        [0.57, 0.69, 0.42, 1.0],
        [0.75, 0.49, 0.43, 1.0],
        [0.44, 0.70, 0.66, 1.0],
        [0.69, 0.66, 0.44, 1.0],
        [0.58, 0.63, 0.71, 1.0],
    ];
    let hash = id.bytes().fold(14695981039346656037u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    });
    COLORS[hash as usize % COLORS.len()]
}

/// Clip in f64 before the canvas adapter converts coordinates to GPU floats.
/// Camera validation bounds the viewport and zoom, while constructor guards
/// bound local geometry. Neither clipping nor projection alters solver data.
fn clip_canvas(mut layer: AnalysisCanvasLayer, width: f64, height: f64) -> AnalysisCanvasLayer {
    let mut triangles =
        Vec::with_capacity(layer.triangles.len().min(MAX_POSTCLIP_DISPLAY_TRIANGLES));
    let mut postclip_budget_exceeded = false;
    for triangle in layer.triangles {
        if triangle.vertices.iter().any(|p| !p.is_finite()) {
            continue;
        }
        if triangle.vertices.iter().all(|p| p.x < 0.0)
            || triangle.vertices.iter().all(|p| p.x > width)
            || triangle.vertices.iter().all(|p| p.y < 0.0)
            || triangle.vertices.iter().all(|p| p.y > height)
        {
            continue;
        }
        if triangle
            .vertices
            .iter()
            .all(|p| (0.0..=width).contains(&p.x) && (0.0..=height).contains(&p.y))
        {
            if triangles.len() >= MAX_POSTCLIP_DISPLAY_TRIANGLES {
                postclip_budget_exceeded = true;
                break;
            }
            triangles.push(triangle);
            continue;
        }
        let mut polygon = triangle.vertices.to_vec();
        for (axis, boundary, keep_greater) in [
            (0, 0.0, true),
            (0, width, false),
            (1, 0.0, true),
            (1, height, false),
        ] {
            polygon = clip_screen_polygon(polygon, axis, boundary, keep_greater);
        }
        // A triangle clipped against a rectangle has at most seven vertices.
        // Thus each bounded input triangle produces at most five triangles.
        for i in 1..polygon.len().saturating_sub(1) {
            if triangles.len() >= MAX_POSTCLIP_DISPLAY_TRIANGLES {
                postclip_budget_exceeded = true;
                break;
            }
            triangles.push(ScreenTriangle {
                vertices: [polygon[0], polygon[i], polygon[i + 1]]
                    .map(|p| DraftPoint::new(p.x.clamp(0.0, width), p.y.clamp(0.0, height))),
                color: triangle.color,
            });
        }
        if postclip_budget_exceeded {
            break;
        }
    }
    if postclip_budget_exceeded {
        layer.triangles = Vec::new();
        layer.segments = Vec::new();
        layer.labels.clear();
        layer.labels.push(label(
            DraftPoint::new(width.min(16.0), height.min(16.0)),
            format!(
                "Display unavailable: post-clip triangle limit ({MAX_POSTCLIP_DISPLAY_TRIANGLES}) exceeded; no partial geometry shown"
            ),
            SLIP,
        ));
        return layer;
    }
    layer.triangles = triangles;
    let bounds = Bounds2::from_points(
        DraftPoint::new(-8.0, -8.0),
        DraftPoint::new(width + 8.0, height + 8.0),
    );
    layer.segments = layer
        .segments
        .into_iter()
        .filter_map(|mut segment| {
            if !segment.start.is_finite() || !segment.end.is_finite() {
                return None;
            }
            let clipped = drafting_core::clip_display_segment(
                drafting_core::ScreenSegment {
                    start: segment.start,
                    end: segment.end,
                },
                bounds,
            )?;
            segment.start = clipped.start;
            segment.end = clipped.end;
            Some(segment)
        })
        .collect();
    layer.labels.retain(|label| {
        label.position.is_finite()
            && (0.0..=width).contains(&label.position.x)
            && (0.0..=height).contains(&label.position.y)
    });
    // The draw adapter measures and bounds complete circular text rectangles.
    layer
}

fn clip_screen_polygon(
    input: Vec<DraftPoint>,
    axis: usize,
    boundary: f64,
    keep_greater: bool,
) -> Vec<DraftPoint> {
    let coordinate = |p: DraftPoint| if axis == 0 { p.x } else { p.y };
    let mut output = Vec::with_capacity(input.len() + 1);
    for i in 0..input.len() {
        let p = input[i];
        let q = input[(i + 1) % input.len()];
        let dp = coordinate(p) - boundary;
        let dq = coordinate(q) - boundary;
        let inside_p = if keep_greater { dp >= 0.0 } else { dp <= 0.0 };
        let inside_q = if keep_greater { dq >= 0.0 } else { dq <= 0.0 };
        if inside_p {
            output.push(p);
        }
        if inside_p != inside_q {
            // Interpolate from the closer endpoint and set the clipping
            // coordinate exactly, avoiding a large subtraction at the plane.
            let (a, b, t) = if dp.abs() <= dq.abs() {
                (p, q, dp / (dp - dq))
            } else {
                (q, p, dq / (dq - dp))
            };
            output.push(if axis == 0 {
                DraftPoint::new(boundary, (b.y - a.y).mul_add(t, a.y))
            } else {
                DraftPoint::new((b.x - a.x).mul_add(t, a.x), boundary)
            });
        }
    }
    output.dedup_by(|a, b| a.x == b.x && a.y == b.y);
    if output.len() > 1 && output.first() == output.last() {
        output.pop();
    }
    output
}

fn fill(
    out: &mut AnalysisCanvasLayer,
    triangles: &[[Point2; 3]],
    color: [f32; 4],
    screen: &impl Fn(Point2) -> DraftPoint,
) {
    out.triangles
        .extend(triangles.iter().map(|t| ScreenTriangle {
            vertices: t.map(screen),
            color,
        }));
}
fn segment(a: DraftPoint, b: DraftPoint, color: [f32; 4], width: f32) -> ScreenSegment {
    ScreenSegment {
        start: a,
        end: b,
        color,
        width,
    }
}
fn label(position: DraftPoint, text: String, color: [f32; 4]) -> ScreenLabel {
    ScreenLabel { position, text, color, placement: ScreenLabelPlacement::Hud }
}
fn legend_label(position: DraftPoint, text: String, color: [f32; 4]) -> ScreenLabel {
    ScreenLabel { placement: ScreenLabelPlacement::Legend, ..label(position, text, color) }
}
fn footer_label(position: DraftPoint, text: String, color: [f32; 4]) -> ScreenLabel {
    ScreenLabel { placement: ScreenLabelPlacement::Footer, ..label(position, text, color) }
}
fn callout_label(position: DraftPoint, text: String, color: [f32; 4]) -> ScreenLabel {
    ScreenLabel { placement: ScreenLabelPlacement::Callout, ..label(position, text, color) }
}
fn stroke(
    out: &mut AnalysisCanvasLayer,
    points: &[Point2],
    closed: bool,
    color: [f32; 4],
    width: f32,
    screen: &impl Fn(Point2) -> DraftPoint,
) {
    for p in points.windows(2) {
        out.segments
            .push(segment(screen(p[0]), screen(p[1]), color, width));
    }
    if closed && points.len() > 2 {
        out.segments.push(segment(
            screen(*points.last().unwrap()),
            screen(points[0]),
            color,
            width,
        ));
    }
}
fn cross(out: &mut AnalysisCanvasLayer, p: DraftPoint, color: [f32; 4]) {
    out.segments.push(segment(
        DraftPoint::new(p.x - 4.0, p.y),
        DraftPoint::new(p.x + 4.0, p.y),
        color,
        2.0,
    ));
    out.segments.push(segment(
        DraftPoint::new(p.x, p.y - 4.0),
        DraftPoint::new(p.x, p.y + 4.0),
        color,
        2.0,
    ));
}
fn arrow(out: &mut AnalysisCanvasLayer, a: DraftPoint, b: DraftPoint, color: [f32; 4]) {
    out.segments.push(segment(a, b, color, 1.6));
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    if len == 0.0 {
        return;
    }
    for sign in [-1.0, 1.0] {
        out.segments.push(segment(
            b,
            DraftPoint::new(
                b.x - dx / len * 7.0 + sign * dy / len * 3.5,
                b.y - dy / len * 7.0 - sign * dx / len * 3.5,
            ),
            color,
            1.6,
        ));
    }
}

#[derive(Default)]
struct SceneBuilder {
    scene: Scene,
    next_id: u64,
    triangles: usize,
}
impl SceneBuilder {
    fn reserve(&mut self, n: usize) -> Result<(), String> {
        self.triangles = self
            .triangles
            .checked_add(n)
            .filter(|n| *n <= MAX_SCENE_TRIANGLES)
            .ok_or("3D display triangle budget exceeded")?;
        Ok(())
    }
    fn insert(&mut self, name: String, vertices: Vec<[Vec3; 3]>, color: [f32; 4]) {
        if vertices.is_empty() {
            return;
        }
        self.next_id += 1;
        let mesh = Mesh::new(
            vertices
                .into_iter()
                .enumerate()
                .map(|(i, t)| Triangle::new(i as u32, t[0], t[1], t[2]))
                .collect(),
        );
        let mut object = SceneObject::new(self.next_id, name, mesh);
        object.color = color;
        self.scene.insert(object);
    }
    fn extruded(
        &mut self,
        name: String,
        cap: &[[Point2; 3]],
        width: f64,
        color: [f32; 4],
    ) -> Result<(), String> {
        if cap.is_empty() {
            return Ok(());
        }
        if !width.is_finite() || width <= 0.0 {
            return Err("3D extrusion needs a positive finite width".into());
        }
        let mut edges = BTreeMap::new();
        for tri in cap {
            if tri.iter().any(|p| !display_point(*p))
                || cross_value(tri[0], tri[1], tri[2]) <= 0.0
            {
                return Err("3D extrusion cap has a nonfinite, degenerate or reversed triangle".into());
            }
            for i in 0..3 {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                let (ka, kb) = (point_key(a), point_key(b));
                let key = if ka < kb { (ka, kb) } else { (kb, ka) };
                let entry = edges.entry(key).or_insert((0usize, a, b, 0i32));
                entry.0 += 1;
                entry.3 += if ka < kb { 1 } else { -1 };
            }
        }
        if edges.values().any(|edge| edge.0 > 2 || (edge.0 == 2 && edge.3 != 0)) {
            return Err("3D extrusion cap is nonmanifold or has inconsistent shared-edge winding".into());
        }
        let boundary = edges.values().filter(|v| v.0 == 1).collect::<Vec<_>>();
        let mut degree = BTreeMap::new();
        for &&(_, a, b, _) in &boundary {
            degree.entry(point_key(a)).or_insert((0usize, 0usize)).1 += 1;
            degree.entry(point_key(b)).or_insert((0usize, 0usize)).0 += 1;
        }
        if degree.values().any(|&(incoming, outgoing)| incoming != 1 || outgoing != 1) {
            return Err("3D extrusion cap boundary is open or contains a T junction".into());
        }
        let count = cap
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(boundary.len() * 2))
            .ok_or("3D mesh size overflow")?;
        self.reserve(count)?;
        let mut vertices = Vec::with_capacity(count);
        for tri in cap {
            vertices.push([
                vector(tri[0], 0.0),
                vector(tri[1], 0.0),
                vector(tri[2], 0.0),
            ]);
            vertices.push([
                vector(tri[2], width),
                vector(tri[1], width),
                vector(tri[0], width),
            ]);
        }
        for &&(_, a, b, _) in &boundary {
            let (a0, b0, a1, b1) = (
                vector(a, 0.0),
                vector(b, 0.0),
                vector(a, width),
                vector(b, width),
            );
            vertices.push([a0, b1, b0]);
            vertices.push([a0, a1, b1]);
        }
        self.insert(name, vertices, color);
        Ok(())
    }
    fn sheet(
        &mut self,
        name: String,
        line: &[Point2],
        width: f64,
        color: [f32; 4],
    ) -> Result<(), String> {
        self.reserve(line.len().saturating_sub(1) * 2)?;
        let mut vertices = Vec::with_capacity(line.len().saturating_sub(1) * 2);
        for pair in line.windows(2) {
            let (a, b, c, d) = (
                vector(pair[0], 0.0),
                vector(pair[1], 0.0),
                vector(pair[1], width),
                vector(pair[0], width),
            );
            vertices.push([a, b, c]);
            vertices.push([a, c, d]);
        }
        self.insert(name, vertices, color);
        Ok(())
    }
    fn lines(
        &mut self,
        name: String,
        lines: &[(Point2, Point2)],
        y: f64,
        width: f64,
        color: [f32; 4],
    ) -> Result<(), String> {
        self.reserve(lines.len() * 2)?;
        let mut vertices = Vec::with_capacity(lines.len() * 2);
        for &(a, b) in lines {
            let length = (b.x - a.x).hypot(b.z - a.z);
            if length == 0.0 {
                continue;
            }
            let (dx, dz) = (-(b.z - a.z) / length * width, (b.x - a.x) / length * width);
            let (p, q, r, s) = (
                Vec3::new(a.x + dx, y, a.z + dz),
                Vec3::new(b.x + dx, y, b.z + dz),
                Vec3::new(b.x - dx, y, b.z - dz),
                Vec3::new(a.x - dx, y, a.z - dz),
            );
            vertices.push([p, q, r]);
            vertices.push([p, r, s]);
        }
        self.insert(name, vertices, color);
        Ok(())
    }
}
fn point_key(p: Point2) -> (u64, u64) {
    // Canonicalise signed zero so coincident display edges cancel.
    (
        (if p.x == 0.0 { 0.0 } else { p.x }).to_bits(),
        (if p.z == 0.0 { 0.0 } else { p.z }).to_bits(),
    )
}
fn vector(p: Point2, y: f64) -> Vec3 {
    Vec3::new(p.x, y, p.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geotech_core::{InputUnits, SoilMaterial, circular::*};

    // A synthetic input shared in shape with the independent numerical case.
    // These tests examine rendering/identity only, never certify its solver.
    fn input(offset: f64) -> CircularAnalysisInput {
        let p = |x, z| Point2::new(x + offset, z + offset);
        CircularAnalysisInput {
            analysis_id: "display-test".into(),
            source_revision: "synthetic-display-only".into(),
            ground: GroundSection2D {
                section_id: "display-ground".into(),
                coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
                terrain_polyline: vec![p(-5.0, 15.0 / 7.0), p(4.0, 6.0 / 7.0)],
                layer_interfaces: vec![],
                soil_regions: vec![SoilRegion {
                    region_id: "region-A".into(),
                    material_id: "soil-A".into(),
                }],
                bottom_boundary: vec![p(-5.0, -2.0), p(4.0, -2.0)],
                materials: vec![GroundMaterial {
                    material_id: "soil-A".into(),
                    properties: SoilMaterial {
                        name: "Synthetic render material".into(),
                        unit_weight_kn_per_m3: 18.0,
                        effective_cohesion_kpa: 3.0,
                        effective_friction_angle_deg: 28.0,
                        provenance: "Synthetic display test; no engineering design provenance"
                            .into(),
                    },
                }],
                model_revision: 1,
            },
            slip_surface: CircularSlipSurface::from_center_radius(
                "display-circle".into(),
                p(0.0, 5.0),
                5.0,
                SlideDirection::PositiveX,
            )
            .unwrap(),
            basis: AnalysisBasis::DryStatic,
            units: InputUnits::default(),
            extrusion_width_m: 2.0,
        }
    }
    fn config() -> CircularAnalysisConfig {
        CircularAnalysisConfig {
            slices: SliceConfig {
                target_slices: 8,
                verify_refinement: false,
            },
            ..Default::default()
        }
    }
    fn area(triangles: &[[Point2; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| cross_value(t[0], t[1], t[2]).abs() * 0.5)
            .sum()
    }

    #[test]
    fn circular_render_cutaway_partitions_a_known_visual_rectangle() {
        let upper = [Point2::new(0.0, 2.0), Point2::new(4.0, 2.0)];
        let lower = [Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)];
        let arc = [Point2::new(0.0, 0.5), Point2::new(4.0, 1.5)];
        let mut budget = 100;
        let context = strips(&upper, &lower, Some(&arc), Clip::Below, &mut budget).unwrap();
        let mass = strips(&upper, &lower, Some(&arc), Clip::Above, &mut budget).unwrap();
        assert!((area(&context) - 4.0).abs() < 1e-12);
        assert!((area(&mass) - 4.0).abs() < 1e-12);
        assert!((area(&context) + area(&mass) - 8.0).abs() < 1e-12);
    }

    #[test]
    fn circular_render_ground_is_visible_without_a_solver_result() {
        let input = input(0.0);
        let render = GeotechCircularRender::new(&input.ground, None, 2.0, &config()).unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        let canvas = render.canvas(&camera, OverlayFlags::default());
        assert!(!canvas.triangles.is_empty());
        assert!(
            canvas
                .labels
                .iter()
                .any(|s| s.text.contains("result layers not shown"))
        );
        assert!(
            render
                .scene(OverlayFlags::default())
                .unwrap()
                .objects()
                .any(|o| o.name.starts_with("Ground layer"))
        );
        assert!(render.result.is_none());
    }

    #[test]
    fn circular_render_uses_local_vertices_for_a_billion_metre_origin() {
        let base = input(0.0);
        let moved = input(1_000_000_000.0);
        let a = GeotechCircularRender::new(&base.ground, None, 2.0, &config()).unwrap();
        let b = GeotechCircularRender::new(&moved.ground, None, 2.0, &config()).unwrap();
        assert_eq!(b.origin.x, 999_999_995.0);
        let scene_a = a.scene(OverlayFlags::default()).unwrap();
        let scene_b = b.scene(OverlayFlags::default()).unwrap();
        for (a, b) in scene_a.objects().zip(scene_b.objects()) {
            for (a, b) in a.mesh.triangles().iter().zip(b.mesh.triangles()) {
                for (a, b) in a.vertices.iter().zip(b.vertices.iter()) {
                    assert!(
                        (a.x - b.x).abs() < 1e-6
                            && (a.y - b.y).abs() < 1e-6
                            && (a.z - b.z).abs() < 1e-6
                    );
                    assert!(b.x.abs() < 20.0 && b.z.abs() < 20.0);
                }
            }
        }
    }

    #[test]
    fn circular_render_overlays_and_colours_do_not_mutate_the_input_or_result() {
        let input = input(0.0);
        let original_input = input.clone();
        let result = diagnose_specified_circle(&input, &config()).unwrap();
        let original_result = result.clone();
        let mut render =
            GeotechCircularRender::new(&input.ground, Some(&result), 2.0, &config()).unwrap();
        render
            .set_material_color("soil-A", [0.2, 0.3, 0.4, 1.0])
            .unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        for flags in [
            OverlayFlags::default(),
            OverlayFlags {
                ground: false,
                soil_layers: false,
                failure_mass: false,
                slices: false,
                forces: true,
                critical_circle: false,
            },
        ] {
            let _ = render.canvas(&camera, flags);
            let _ = render.scene(flags).unwrap();
        }
        assert_eq!(input, original_input);
        assert_eq!(result, original_result);
        assert!(
            render
                .result
                .as_ref()
                .unwrap()
                .forces
                .iter()
                .any(|g| g.name == "D" && g.value < 0.0)
        );
    }

    #[test]
    fn circular_render_extrusion_cancels_internal_strip_walls() {
        let upper = [
            Point2::new(0.0, 2.0),
            Point2::new(2.0, 2.0),
            Point2::new(4.0, 2.0),
        ];
        let lower = [Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)];
        let mut budget = 100;
        let cap = strips(&upper, &lower, None, Clip::None, &mut budget).unwrap();
        let mut builder = SceneBuilder::default();
        builder.extruded("rect".into(), &cap, 2.0, NEUTRAL).unwrap();
        // Four cap triangles per side, six outer edges (split top/bottom),
        // two triangles per edge. The shared vertical strip edge has no wall.
        assert_eq!(builder.scene.get(1).unwrap().mesh.triangles().len(), 20);
    }

    #[test]
    fn circular_render_extrusion_has_closed_oriented_faces_and_area_times_width_volume() {
        let cap = [
            [Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), Point2::new(4.0, 2.0)],
            [Point2::new(0.0, 0.0), Point2::new(4.0, 2.0), Point2::new(0.0, 2.0)],
        ];
        let mut builder = SceneBuilder::default();
        builder.extruded("volume".into(), &cap, 3.0, NEUTRAL).unwrap();
        let object = builder.scene.get(1).unwrap();
        let key = |p: Vec3| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits());
        let mut edges = BTreeMap::new();
        let mut volume = 0.0;
        for triangle in object.mesh.triangles() {
            let [a, b, c] = triangle.vertices;
            assert!((b - a).cross(c - a).length_squared() > 0.0);
            volume += a.dot(b.cross(c)) / 6.0;
            for (a, b) in [(a, b), (b, c), (c, a)] {
                let (a, b) = (key(a), key(b));
                let ordered = if a < b { (a, b) } else { (b, a) };
                let edge = edges.entry(ordered).or_insert((0, 0));
                edge.0 += 1;
                edge.1 += if a < b { 1 } else { -1 };
            }
        }
        assert!(edges.values().all(|&(count, winding)| count == 2 && winding == 0));
        assert!((volume - 8.0 * 3.0).abs() < 1e-12);
        let front = object.mesh.triangles().iter().filter(|t| t.vertices.iter().all(|p| p.y == 0.0));
        assert!(front.clone().all(|t| t.normal().y < 0.0));
        assert_eq!(front.count(), 2);
        assert!(object.mesh.triangles().iter().filter(|t| t.vertices.iter().all(|p| p.y == 3.0))
            .all(|t| t.normal().y > 0.0));
    }

    #[test]
    fn circular_render_clipped_strips_keep_closed_oriented_extrusions() {
        let upper = [
            Point2::new(0.0, 2.0),
            Point2::new(1.0, 2.0),
            Point2::new(2.0, 2.0),
        ];
        let lower = [Point2::new(0.0, 0.0), Point2::new(2.0, 0.0)];
        let arc = [Point2::new(0.0, 0.1), Point2::new(2.0, 0.1)];
        for (clip, expected_area) in [(Clip::Below, 0.2), (Clip::Above, 3.8)] {
            let mut budget = 100;
            let cap = strips(&upper, &lower, Some(&arc), clip, &mut budget).unwrap();
            assert!((area(&cap) - expected_area).abs() < 1e-12);
            let mut builder = SceneBuilder::default();
            builder.extruded("clipped".into(), &cap, 3.0, NEUTRAL).unwrap();
            let triangles = builder.scene.get(1).unwrap().mesh.triangles();
            // Both clipped halves retain six outer edges and no internal wall.
            assert_eq!(triangles.len(), 20);
            let key = |p: Vec3| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits());
            let mut edges = BTreeMap::new();
            let mut volume = 0.0;
            for triangle in triangles {
                let [a, b, c] = triangle.vertices;
                assert!((b - a).cross(c - a).length_squared() > 0.0);
                volume += a.dot(b.cross(c)) / 6.0;
                for (a, b) in [(a, b), (b, c), (c, a)] {
                    let (a, b) = (key(a), key(b));
                    let ordered = if a < b { (a, b) } else { (b, a) };
                    let edge = edges.entry(ordered).or_insert((0, 0));
                    edge.0 += 1;
                    edge.1 += if a < b { 1 } else { -1 };
                }
            }
            assert!(edges.values().all(|&(count, winding)| count == 2 && winding == 0));
            assert!((volume - expected_area * 3.0).abs() < 1e-12);
            assert!(triangles.iter()
                .filter(|t| t.vertices.iter().all(|p| p.y == 0.0))
                .all(|t| t.normal().y < 0.0));
            assert!(triangles.iter()
                .filter(|t| t.vertices.iter().all(|p| p.y == 3.0))
                .all(|t| t.normal().y > 0.0));
        }
    }

    #[test]
    fn circular_render_extrusion_refuses_reversed_duplicate_and_open_caps() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let c = Point2::new(2.0, 2.0);
        let d = Point2::new(0.0, 2.0);
        for cap in [vec![[a, c, b]], vec![[a, b, c], [a, b, c]],
            vec![[a, b, c], [a, Point2::new(1.0, 1.0), d]]] {
            let mut builder = SceneBuilder::default();
            assert!(builder.extruded("invalid".into(), &cap, 3.0, NEUTRAL).is_err());
            assert!(builder.scene.objects().next().is_none());
        }
    }

    #[test]
    fn circular_render_converged_bishop_tension_remains_a_diagnostic() {
        let input = input(0.0);
        let mut result = diagnose_specified_circle(&input, &config()).unwrap();
        result.bishop_result.convergence_status = BishopConvergenceStatus::Converged;
        result.bishop_result.accepted_factor_of_safety = None;
        result.bishop_result.physical_assessment.status = BishopPhysicalStatus::InadmissibleTension;
        result
            .bishop_result
            .physical_assessment
            .minimum_normal_kn_per_m = -12.5;
        let render =
            GeotechCircularRender::new(&input.ground, Some(&result), 2.0, &config()).unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        let labels = render
            .canvas(&camera, OverlayFlags::default())
            .labels
            .into_iter()
            .map(|label| label.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(labels.contains("Fellenius Fs"));
        assert!(labels.contains("DIAGNOSTIC_ONLY"));
        assert!(labels.contains("Bishop equation F"));
        assert!(labels.contains("Bishop: Converged"));
        assert!(labels.contains("InadmissibleTension"));
        assert!(labels.contains("min N = -12.500000"));
        assert!(!labels.contains("ACCEPTED"));
    }

    #[test]
    fn circular_render_force_provenance_and_negative_normal_are_explicit() {
        let input = input(0.0);
        let mut result = diagnose_specified_circle(&input, &config()).unwrap();
        let slice_id = result.slices[0].slice_id;
        result.fellenius_result.slice_contributions[0].normal_kn_per_m = -123.25;
        result.bishop_result.slice_contributions[0]
            .base_forces
            .normal_kn_per_m = -9999.0;
        let render =
            GeotechCircularRender::new(&input.ground, Some(&result), 2.0, &config()).unwrap();
        assert!(render.result.as_ref().unwrap().forces.iter().any(|glyph| {
            glyph.slice_id == slice_id && glyph.name == "N" && glyph.value == -123.25
        }));
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        let flags = OverlayFlags {
            forces: true,
            ..Default::default()
        };
        assert!(
            render
                .canvas(&camera, flags)
                .labels
                .iter()
                .any(|label| { label.text.contains("Fellenius W/N/D/R") })
        );
        assert!(
            render
                .scene(flags)
                .unwrap()
                .objects()
                .any(|object| { object.name.starts_with("Fellenius N arrows") })
        );
    }

    #[test]
    fn circular_render_all_hidden_flags_remove_geometry_in_both_views() {
        let input = input(0.0);
        let result = diagnose_specified_circle(&input, &config()).unwrap();
        let render =
            GeotechCircularRender::new(&input.ground, Some(&result), 2.0, &config()).unwrap();
        let flags = OverlayFlags {
            ground: false,
            soil_layers: false,
            failure_mass: false,
            slices: false,
            forces: false,
            critical_circle: false,
        };
        let canvas = render.canvas(&Camera2D::new(1000.0, 700.0), flags);
        assert!(canvas.triangles.is_empty());
        assert!(canvas.segments.is_empty());
        assert_eq!(render.scene(flags).unwrap().objects().count(), 0);
    }

    #[test]
    fn circular_render_clips_gpu_coordinates_without_clamping_world_geometry() {
        let layer = AnalysisCanvasLayer {
            triangles: vec![ScreenTriangle {
                vertices: [
                    DraftPoint::new(-1e21, -1e21),
                    DraftPoint::new(1e21, -1e21),
                    DraftPoint::new(0.0, 1e21),
                ],
                color: MASS,
            }],
            segments: vec![segment(
                DraftPoint::new(-1e21, -1e21),
                DraftPoint::new(1e21, 1e21),
                TEXT,
                1.0,
            )],
            labels: vec![label(DraftPoint::new(1e21, 0.0), "offscreen".into(), TEXT)],
        };
        let clipped = clip_canvas(layer, 1000.0, 700.0);
        let mut visible_area = 0.0;
        for triangle in &clipped.triangles {
            assert!(
                triangle
                    .vertices
                    .iter()
                    .all(|p| { (0.0..=1000.0).contains(&p.x) && (0.0..=700.0).contains(&p.y) })
            );
            visible_area += cross_value(
                Point2::new(triangle.vertices[0].x, triangle.vertices[0].y),
                Point2::new(triangle.vertices[1].x, triangle.vertices[1].y),
                Point2::new(triangle.vertices[2].x, triangle.vertices[2].y),
            )
            .abs()
                * 0.5;
        }
        assert!((visible_area - 700_000.0).abs() < 1e-6);
        assert_eq!(clipped.segments.len(), 1);
        assert!(
            clipped
                .segments
                .iter()
                .all(|s| [s.start, s.end].iter().all(|p| {
                    (-8.000001..=1008.000001).contains(&p.x)
                        && (-8.000001..=708.000001).contains(&p.y)
                }))
        );
        assert!(clipped.labels.is_empty());
    }

    #[test]
    fn circular_render_postclip_budget_discards_partial_canvas_with_error() {
        let triangle = ScreenTriangle {
            vertices: [
                DraftPoint::new(-1e21, -1e21),
                DraftPoint::new(1e21, -1e21),
                DraftPoint::new(0.0, 1e21),
            ],
            color: MASS,
        };
        let layer = AnalysisCanvasLayer {
            triangles: std::iter::repeat_with(|| ScreenTriangle {
                vertices: triangle.vertices,
                color: triangle.color,
            })
            .take(MAX_POSTCLIP_DISPLAY_TRIANGLES / 2 + 1)
            .collect(),
            segments: vec![segment(
                DraftPoint::new(1.0, 1.0),
                DraftPoint::new(2.0, 2.0),
                TEXT,
                1.0,
            )],
            labels: vec![label(DraftPoint::new(16.0, 16.0), "partial".into(), TEXT)],
        };

        let clipped = clip_canvas(layer, 1000.0, 700.0);

        assert!(clipped.triangles.is_empty());
        assert!(clipped.segments.is_empty());
        assert_eq!(clipped.labels.len(), 1);
        assert!(clipped.labels[0].text.contains("post-clip triangle limit"));
        assert!(clipped.labels[0].text.contains("no partial geometry shown"));
    }

    #[test]
    fn circular_render_invalid_camera_does_not_emit_gpu_geometry() {
        let input = input(0.0);
        let render = GeotechCircularRender::new(&input.ground, None, 2.0, &config()).unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.pixels_per_metre = f64::INFINITY;
        let layer = render.canvas(&camera, OverlayFlags::default());
        assert!(layer.triangles.is_empty());
        assert!(layer.segments.is_empty());
        assert!(
            layer
                .labels
                .iter()
                .any(|label| label.text.contains("invalid camera"))
        );
    }

    #[test]
    fn circular_render_strips_reject_disjoint_boundaries() {
        let upper = [Point2::new(0.0, 2.0), Point2::new(1.0, 2.0)];
        let lower = [Point2::new(2.0, 0.0), Point2::new(3.0, 0.0)];
        let mut budget = 100;
        assert!(strips(&upper, &lower, None, Clip::None, &mut budget).is_err());
        assert_eq!(budget, 100);
    }

    fn partial_ground_definition(offset: f64) -> GroundDefinition {
        GroundDefinition {
            terrain_polyline: vec![
                Point2::new(offset, offset + 4.0),
                Point2::new(offset + 8.0, offset + 2.0),
            ],
            materials: vec![GroundMaterial {
                material_id: "soil-A".into(),
                properties: SoilMaterial {
                    name: "Supplied clay name".into(),
                    unit_weight_kn_per_m3: 18.0,
                    effective_cohesion_kpa: 3.0,
                    effective_friction_angle_deg: 28.0,
                    provenance: "Preview fixture only".into(),
                },
            }],
            ..GroundDefinition::default()
        }
    }

    #[test]
    fn ground_draft_preview_empty_definition_has_no_world_site_shape() {
        let render = GroundDraftRender::new(&GroundDefinition::default(), Some(2.0)).unwrap();
        let canvas = render.canvas(&Camera2D::new(1000.0, 700.0));
        assert!(canvas.triangles.is_empty());
        assert!(canvas.segments.is_empty());
        assert!(
            canvas
                .labels
                .iter()
                .any(|label| label.text.contains("no calculation"))
        );
        assert_eq!(render.scene().unwrap().objects().count(), 0);
    }

    #[test]
    fn ground_draft_preview_partial_input_keeps_only_supplied_lines_and_names() {
        let definition = partial_ground_definition(0.0);
        let render = GroundDraftRender::new(&definition, Some(2.0)).unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        let canvas = render.canvas(&camera);
        assert!(canvas.triangles.is_empty());
        assert_eq!(canvas.segments.len(), 5);
        assert!(
            canvas
                .labels
                .iter()
                .any(|label| label.text.contains("Supplied clay name"))
        );
        assert!(
            canvas
                .labels
                .iter()
                .any(|label| label.text.contains("DIAGNOSTIC ONLY")
                    && label.text.contains("no calculation"))
        );
        let scene = render.scene().unwrap();
        let objects = scene.objects().collect::<Vec<_>>();
        assert_eq!(objects.len(), 1);
        assert!(objects[0].name.contains("DIAGNOSTIC ONLY"));
        assert_eq!(objects[0].mesh.triangles().len(), 2);
    }

    #[test]
    fn ground_draft_2d_does_not_require_an_extrusion_width() {
        let definition = partial_ground_definition(0.0);
        let render = GroundDraftRender::new(&definition, None).unwrap();
        let mut camera = Camera2D::new(1000.0, 700.0);
        camera.fit_bounds(render.bounds, 40.0).unwrap();
        let canvas = render.canvas(&camera);
        assert!(!canvas.segments.is_empty());
        assert!(matches!(render.scene(),Err(error) if error.contains("ширину")));
        let empty = GroundDraftRender::new(&GroundDefinition::default(), None).unwrap();
        assert_eq!(empty.scene().unwrap().objects().count(), 0);
    }
    #[test]
    fn ground_draft_preview_reports_camera_fit_center_limit() {
        let render =
            GroundDraftRender::new(&partial_ground_definition(1_000_000_000_100.0), Some(2.0))
                .unwrap();
        let canvas = render.canvas(&Camera2D::new(1000.0, 700.0));

        assert!(canvas.triangles.is_empty());
        assert!(canvas.segments.is_empty());
        assert!(canvas.labels.iter().any(|label| {
            label
                .text
                .contains("fit center exceeds Camera2D's ±1e12 coordinate limit")
        }));
        assert_eq!(render.scene().unwrap().objects().count(), 1);
    }

    #[test]
    fn ground_draft_preview_uses_local_f64_vertices_for_large_world_origins() {
        let base = GroundDraftRender::new(&partial_ground_definition(0.0), Some(2.0)).unwrap();
        let moved =
            GroundDraftRender::new(&partial_ground_definition(1_000_000_000.0), Some(2.0)).unwrap();
        let base_scene = base.scene().unwrap();
        let moved_scene = moved.scene().unwrap();
        for (base_object, moved_object) in base_scene.objects().zip(moved_scene.objects()) {
            for (base_triangle, moved_triangle) in base_object
                .mesh
                .triangles()
                .iter()
                .zip(moved_object.mesh.triangles())
            {
                for (base_vertex, moved_vertex) in base_triangle
                    .vertices
                    .iter()
                    .zip(moved_triangle.vertices.iter())
                {
                    assert!(
                        (base_vertex.x - moved_vertex.x).abs() < 1e-9
                            && (base_vertex.y - moved_vertex.y).abs() < 1e-9
                            && (base_vertex.z - moved_vertex.z).abs() < 1e-9
                    );
                    assert!(moved_vertex.x.abs() < 20.0 && moved_vertex.z.abs() < 20.0);
                }
            }
        }
    }
    fn selection_slice(id: usize, left: f64, right: f64, z_left: f64, z_right: f64) -> Slice {
        Slice {
            slice_id: id,
            x_left: left,
            x_right: right,
            horizontal_width_m: right - left,
            area_by_material: vec![],
            total_area_m2: 0.0,
            weight_kn_per_m: 0.0,
            base_left: Point2::new(left, z_left),
            base_right: Point2::new(right, z_right),
            base_inclination_rad: 0.0,
            base_length_m: right - left,
            arc_length_m: right - left,
            base_material_id: "selection-test".into(),
            numerical_quality: NumericalQuality {
                local_origin: Point2::new(0.0, 0.0),
                world_resolution_m: 1e-12,
                intersection_tolerance_m: 1e-9,
                estimated_area_roundoff_m2: 0.0,
            },
        }
    }
    #[test]
    fn local_selection_lower_arc_without_contacts_is_bounded_and_translation_stable() {
        let make = |offset| {
            CircularSlipSurface::from_center_radius(
                "preview".into(),
                Point2::new(offset + 10.0, offset + 20.0),
                5.0,
                SlideDirection::PositiveX,
            )
            .unwrap()
        };
        let a = lower_selection_arc(&make(0.0), Point2::new(8.0, 16.0), None).unwrap();
        let b = lower_selection_arc(&make(1e9), Point2::new(1e9 + 8.0, 1e9 + 16.0), None).unwrap();
        assert_eq!(a.len(), ARC_SEGMENTS + 1);
        assert_eq!(a.first(), Some(&Point2::new(-3.0, 4.0)));
        assert_eq!(a.last(), Some(&Point2::new(7.0, 4.0)));
        for (a, b) in a.iter().zip(&b) {
            assert!((a.x - b.x).abs() < 1e-12 && (a.z - b.z).abs() < 1e-12);
            assert!(a.z <= 4.0 + 1e-12);
            assert!(((a.x - 2.0).hypot(a.z - 4.0) - 5.0).abs() < 1e-12);
        }
    }
    #[test]
    fn local_selection_slice_resolves_sparse_id_and_retains_exact_table_chord() {
        let terrain = [
            Point2::new(0.0, 3.0),
            Point2::new(3.0, 4.0),
            Point2::new(6.0, 2.0),
        ];
        let slices = vec![
            selection_slice(7, 0.0, 1.0, -9.0, -8.0),
            selection_slice(42, 2.0, 4.0, 0.25, 0.75),
        ];
        let original = slices.clone();
        let path = selected_slice_path(&terrain, &slices, 42, Point2::new(0.0, 0.0), true).unwrap();
        assert!(path.closed && path.label.contains("42") && path.label.contains("диагностика"));
        assert!(path.points.contains(&Point2::new(3.0, 4.0)));
        assert_eq!(
            &path.points[path.points.len() - 2..],
            &[slices[1].base_right, slices[1].base_left]
        );
        assert!(!path.points.contains(&slices[0].base_left));
        assert_eq!(slices, original);
        assert!(selected_slice_path(&terrain, &slices, 1, Point2::new(0.0, 0.0), false).is_err());
        let duplicate = vec![slices[1].clone(), slices[1].clone()];
        assert!(
            selected_slice_path(&terrain, &duplicate, 42, Point2::new(0.0, 0.0), false).is_err()
        );
    }
    #[test]
    fn local_selection_rejects_invalid_source_and_oversize_without_partial_path() {
        let terrain = [Point2::new(0.0, 3.0), Point2::new(6.0, 2.0)];
        let mut slice = selection_slice(42, 2.0, 4.0, 0.25, 0.75);
        slice.base_left.x = 1.5;
        assert!(selected_slice_path(&terrain, &[slice], 42, Point2::new(0.0, 0.0), false).is_err());
        let oversize = vec![Point2::new(0.0, 0.0); MAX_LOCAL_SELECTION_POINTS];
        assert!(selected_slice_path(&oversize, &[], 42, Point2::new(0.0, 0.0), false).is_err());
        let mut circle = CircularSlipSurface::from_center_radius(
            "preview".into(),
            Point2::new(0.0, 5.0),
            5.0,
            SlideDirection::PositiveX,
        )
        .unwrap();
        circle.radius_m = f64::INFINITY;
        assert!(lower_selection_arc(&circle, Point2::new(0.0, 0.0), None).is_err());
    }
    #[test]
    fn local_selection_preserves_retained_arc_endpoints_and_draws_wire_only() {
        let circle = CircularSlipSurface::from_center_radius(
            "retained".into(),
            Point2::new(0.0, 5.0),
            5.0,
            SlideDirection::PositiveX,
        )
        .unwrap();
        let endpoints = (Point2::new(-3.0, 1.0), Point2::new(3.0, 1.0));
        let points = lower_selection_arc(&circle, Point2::new(0.0, 0.0), Some(endpoints)).unwrap();
        assert_eq!(points.first(), Some(&endpoints.0));
        assert_eq!(points.last(), Some(&endpoints.1));
        let overlay = LocalSelectionOverlay::finish(
            Point2::new(0.0, 0.0),
            Some(2.0),
            vec![SelectionPath {
                points,
                closed: false,
                color: [1.0; 4],
                label: "retained diagnostic arc".into(),
            }],
        )
        .unwrap()
        .unwrap();
        let mut camera = Camera2D::new(600.0, 400.0);
        camera.fit_bounds(overlay.bounds, 20.0).unwrap();
        let canvas = overlay.canvas(&camera);
        assert!(canvas.triangles.is_empty());
        assert!(!canvas.segments.is_empty());
        let mut scene = Scene::new();
        scene.insert(SceneObject::new(77, "existing", Mesh::new(vec![])));
        let scene = overlay.append_scene(scene).unwrap();
        assert!(scene.get(77).is_some());
        assert_eq!(scene.objects().count(), 3);
        assert_eq!(
            scene
                .objects()
                .map(|o| o.mesh.triangles().len())
                .sum::<usize>(),
            ARC_SEGMENTS * 4
        );
    }
}
