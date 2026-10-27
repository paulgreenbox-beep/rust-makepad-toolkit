use super::{
    ground_model::{GroundModel, dedup_sorted, eval},
    types::*,
    validation::*,
};
use crate::Point2;
use std::sync::atomic::AtomicBool;
use super::search_meter::{self, charge, WorkKind};

impl CircularSlipSurface {
    pub fn from_center_radius(
        slip_id: CircularSlipId,
        center: Point2,
        radius_m: f64,
        slide_direction: SlideDirection,
    ) -> CResult<Self> {
        text(&slip_id.0, "slip_id", 256)?;
        finite(center.x, "circle center x")?;
        finite(center.z, "circle center z")?;
        finite(radius_m, "circle radius")?;
        if radius_m <= 0.0 {
            return Err(error(
                CircularErrorCode::InvalidCircle,
                "Circle radius must be positive",
            ));
        }
        Ok(Self {
            slip_id,
            center,
            radius_m,
            arc_branch: ArcBranch::Lower,
            slide_direction,
            selected_endpoints: None,
        })
    }

    pub fn from_three_points(
        slip_id: CircularSlipId,
        points: [Point2; 3],
        slide_direction: SlideDirection,
        config: &CircularAnalysisConfig,
    ) -> CResult<Self> {
        validate_config(config)?;
        for p in points {
            finite(p.x, "circle point x")?;
            finite(p.z, "circle point z")?;
        }
        let a = local(points[1], points[0])?;
        let b = local(points[2], points[0])?;
        let scale = a.x.abs().max(a.z.abs()).max(b.x.abs()).max(b.z.abs());
        positive(scale, "three-point scale")?;
        let a = Point2::new(a.x / scale, a.z / scale);
        let b = Point2::new(b.x / scale, b.z / scale);
        let cross = a.x.mul_add(b.z, -a.z * b.x);
        let resolution = points
            .iter()
            .fold(0.0_f64, |m, p| m.max(ulp(p.x)).max(ulp(p.z)));
        let determinant_bound = (config.tolerances.angle_abs_rad.max(resolution / scale) * 8.0)
            .max(128.0 * f64::EPSILON);
        if cross.abs() <= determinant_bound {
            return Err(error(
                CircularErrorCode::InvalidCircle,
                "Three points are collinear or numerically unresolved",
            )
            .at(points.to_vec()));
        }
        let aa = a.x.mul_add(a.x, a.z * a.z);
        let bb = b.x.mul_add(b.x, b.z * b.z);
        let cx = (aa * b.z - bb * a.z) / (2.0 * cross) * scale;
        let cz = (a.x * bb - b.x * aa) / (2.0 * cross) * scale;
        let center = Point2::new(points[0].x + cx, points[0].z + cz);
        let circle = Self::from_center_radius(slip_id, center, cx.hypot(cz), slide_direction)?;
        if points.iter().any(|p| {
            p.z > circle.center.z + config.tolerances.intersection_abs_m.max(4.0 * resolution)
        }) {
            return Err(error(
                CircularErrorCode::InvalidCircle,
                "All three points must belong to the explicitly supported lower branch",
            ));
        }
        Ok(circle)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LocalCircle {
    pub center: Point2,
    pub radius: f64,
    pub tolerance: f64,
}
impl LocalCircle {
    fn vertical(&self, x: f64) -> CResult<f64> {
        let u = (x - self.center.x).abs();
        let slack = self.radius - u;
        if slack < -self.tolerance {
            return Err(error(
                CircularErrorCode::InvalidCircle,
                "X lies outside the circle domain",
            ));
        }
        let v = (slack.max(0.0)).sqrt() * (self.radius + u).sqrt();
        finite(v, "circle square root")?;
        Ok(v)
    }
    pub fn z(&self, x: f64) -> CResult<f64> {
        let u = x - self.center.x;
        let v = self.vertical(x)?;
        // Rationalized sagitta avoids subtracting nearly equal R and sqrt(R²-u²).
        let z = (self.center.z - self.radius) + u * (u / (self.radius + v));
        finite(z, "lower circular branch")?;
        Ok(z)
    }
    pub fn element(&self, left: f64, right: f64) -> CResult<(f64, f64, f64, f64)> {
        let b = positive(right - left, "circle interval width")?;
        let ul = left - self.center.x;
        let ur = right - self.center.x;
        let vl = self.vertical(left)?;
        let vr = self.vertical(right)?;
        let dz = if vl + vr > 0.0 {
            b * ((ur + ul) / (vl + vr))
        } else {
            0.0
        };
        finite(dz, "circular chord dz")?;
        let chord = positive(b.hypot(dz), "circular chord length")?;
        let ratio = (chord / self.radius) * 0.5;
        if ratio > 1.0 + 128.0 * f64::EPSILON {
            return Err(error(
                CircularErrorCode::NumericOverflow,
                "Chord exceeds circle diameter",
            ));
        }
        let theta = 2.0 * ratio.min(1.0).asin();
        let coefficient = if theta < 1e-3 {
            let q = theta * theta;
            theta / 12.0 * (1.0 + q / 30.0 + q * q / 840.0)
        } else {
            (theta - theta.sin()) / (8.0 * (theta * 0.5).sin().powi(2))
        };
        // Area between the lower arc and chord, evaluated without R² overflow.
        let segment = chord * (chord * coefficient);
        let arc = self.radius * theta;
        if !segment.is_finite() || segment < 0.0 || !arc.is_finite() || arc <= 0.0 {
            return Err(error(
                CircularErrorCode::NumericOverflow,
                "Circular segment area/arc length is not representable",
            ));
        }
        Ok((chord, dz, segment, arc))
    }
    pub fn intersections(&self, polyline: &[Point2], max_points: usize) -> CResult<Vec<Point2>> {
        self.intersections_cancellable(polyline, max_points, &AtomicBool::new(false))
    }
    pub fn intersections_cancellable(&self, polyline: &[Point2], max_points: usize,
        cancel: &AtomicBool) -> CResult<Vec<Point2>> {
        cancelled(cancel)?;
        let mut hits = vec![];
        for pair in polyline.windows(2) {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            let a = pair[0];
            let b = pair[1];
            let len = (b.x - a.x).hypot(b.z - a.z);
            let ux = (b.x - a.x) / len;
            let uz = (b.z - a.z) / len;
            let qx = a.x - self.center.x;
            let qz = a.z - self.center.z;
            let along = -qx.mul_add(ux, qz * uz);
            let distance = qx.mul_add(uz, -qz * ux).abs();
            if distance > self.radius + self.tolerance {
                continue;
            }
            let h = (self.radius - distance).max(0.0).sqrt() * (self.radius + distance).sqrt();
            for t in [along - h, along + h] {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                if t < -self.tolerance || t > len + self.tolerance {
                    continue;
                }
                let t = t.clamp(0.0, len);
                let p = if t <= self.tolerance {
                    a
                } else if len - t <= self.tolerance {
                    b
                } else {
                    Point2::new(ux.mul_add(t, a.x), uz.mul_add(t, a.z))
                };
                if p.z > self.center.z + self.tolerance {
                    continue;
                }
                let mut duplicate = false;
                for v in &hits {
                    cancelled(cancel)?;
                    charge(WorkKind::GeometryWork, 1)?;
                    let v: &Point2 = v;
                    if (v.x - p.x).hypot(v.z - p.z) <= self.tolerance {
                        duplicate = true;
                        break;
                    }
                }
                if duplicate {
                    continue;
                }
                if hits.len() >= max_points {
                    return Err(error(
                        CircularErrorCode::ResourceLimit,
                        "Circle intersection budget exceeded",
                    ));
                }
                hits.push(p);
            }
        }
        cancelled(cancel)?;
        search_meter::sort_by(&mut hits, cancel, |a, b| a.x.total_cmp(&b.x))?;
        cancelled(cancel)?;
        Ok(hits)
    }
}

pub(crate) struct PreparedMass<'a> {
    pub ground: GroundModel<'a>,
    pub circle: LocalCircle,
    pub left: Point2,
    pub right: Point2,
    pub breakpoints: Vec<f64>,
    pub geometry: FailureMassGeometry,
}
impl<'a> PreparedMass<'a> {
    pub fn new(input: &'a CircularAnalysisInput, config: &CircularAnalysisConfig) -> CResult<Self> {
        Self::new_cancellable(input, config, &AtomicBool::new(false))
    }
    pub fn new_cancellable(input: &'a CircularAnalysisInput, config: &CircularAnalysisConfig,
        cancel: &AtomicBool) -> CResult<Self> {
        cancelled(cancel)?;
        charge(WorkKind::GeometryPreparations, 1)?;
        charge(WorkKind::GeometryWork, 1)?;
        validate_input_header(input, config)?;
        let mut ground = GroundModel::new_cancellable(&input.ground, config, cancel)?;
        let s = &input.slip_surface;
        let center = local(s.center, ground.origin)?;
        let radius = positive(s.radius_m, "circle radius")?;
        let circle_resolution = ulp(s.center.x).max(ulp(s.center.z)).max(ulp(radius));
        ground.resolution = ground.resolution.max(circle_resolution);
        ground.tolerance = ground
            .tolerance
            .max(circle_resolution * 4.0)
            .max(center.x.abs().max(center.z.abs()).max(radius) * 64.0 * f64::EPSILON);
        finite(ground.tolerance, "circle intersection tolerance")?;
        let circle = LocalCircle {
            center,
            radius,
            tolerance: ground.tolerance,
        };
        let hits = circle.intersections_cancellable(&ground.boundaries[0], config.limits.max_slices + 1, cancel)?;
        if hits.len() < 2 {
            return Err(error(
                CircularErrorCode::NoTerrainIntersection,
                "Lower circular branch must have two distinct terrain contacts",
            ));
        }
        if hits.len() != 2 {
            let mut points = Vec::with_capacity(hits.len());
            for p in &hits {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                points.push(world(*p, ground.origin));
            }
            return Err(error(
                CircularErrorCode::AmbiguousIntersection,
                "Lower branch has additional terrain crossings or tangencies",
            )
            .at(points));
        }
        let left = hits[0];
        let right = hits[1];
        if right.x - left.x <= ground.tolerance {
            return Err(error(
                CircularErrorCode::DegenerateMass,
                "Terrain intersections are not resolvably separated",
            ));
        }
        if let Some(selected) = s.selected_endpoints {
            for p in [selected.entry_point, selected.exit_point] {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                finite(p.x, "selected contact x")?;
                finite(p.z, "selected contact z")?;
            }
            let (entry, exit) = if s.slide_direction == SlideDirection::PositiveX {
                (left, right)
            } else {
                (right, left)
            };
            for (supplied, expected) in [(selected.entry_point, entry), (selected.exit_point, exit)]
            {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                let supplied = local(supplied, ground.origin)?;
                if (supplied.x - expected.x).hypot(supplied.z - expected.z) > ground.tolerance * 4.0
                {
                    return Err(error(
                        CircularErrorCode::InvalidCircle,
                        "Selected entry/exit does not match the unique lower arc and direction",
                    ));
                }
            }
        }
        let mut breakpoints = vec![left.x, right.x];
        for &x in &ground.vertices {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            if x > left.x && x < right.x {
                push_break(&mut breakpoints, x, config)?;
            }
        }
        for boundary in &ground.boundaries {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            for p in circle.intersections_cancellable(boundary, config.limits.max_slices + 1, cancel)? {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                if p.x > left.x && p.x < right.x {
                    push_break(&mut breakpoints, p.x, config)?;
                }
            }
        }
        cancelled(cancel)?;
        // Mandatory source vertices and contact roots define separate linear pieces.
        // A horizontal tolerance merge can erase a steep terrain/material kink
        // and change its integral far beyond the declared area tolerance.
        // Keep distinct knots; slice generation refuses unresolved widths.
        dedup_sorted(&mut breakpoints, 0.0, cancel)?;
        cancelled(cancel)?;
        if breakpoints.len() < 2 {
            return Err(error(
                CircularErrorCode::DegenerateMass,
                "No resolved mass interval",
            ));
        }
        let mut total = Sum::default();
        for pair in breakpoints.windows(2) {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            let x = pair[0] + (pair[1] - pair[0]) * 0.5;
            let z = circle.z(x)?;
            if eval(&ground.boundaries[0], x) - z <= ground.tolerance {
                return Err(error(
                    CircularErrorCode::SlipOutsideGround,
                    "Selected arc leaves or touches the terrain inside the mass",
                ));
            }
            if z < eval(ground.boundaries.last().unwrap(), x) - ground.tolerance {
                return Err(error(
                    CircularErrorCode::SlipOutsideGround,
                    "Selected arc leaves the explicitly closed ground bottom",
                ));
            }
            total.add(area_above_arc(
                &circle,
                &ground.boundaries[0],
                pair[0],
                pair[1],
            )?);
        }
        let area = positive(total.total(), "failure mass area")?;
        let span = right.x - left.x;
        let area_resolution = ground.tolerance * span;
        if area <= area_resolution {
            return Err(error(
                CircularErrorCode::DegenerateMass,
                "Failure area is below world-coordinate resolution",
            ));
        }
        let material_areas = material_areas_cancellable(&ground, &circle, &breakpoints, config, cancel)?;
        let mut material_sum = Sum::default();
        for m in &material_areas {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            material_sum.add(m.area_m2);
        }
        if (material_sum.total() - area).abs()
            > config.tolerances.area_abs_m2 + config.tolerances.area_rel * area
        {
            return Err(error(
                CircularErrorCode::InvalidGround,
                "Material areas do not close to the independent total mass area",
            ));
        }
        let mut terrain = vec![world(left, ground.origin)];
        for &p in &ground.boundaries[0] {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            if p.x > left.x && p.x < right.x {
                terrain.push(world(p, ground.origin));
            }
        }
        terrain.push(world(right, ground.origin));
        let (entry, exit) = if s.slide_direction == SlideDirection::PositiveX {
            (left, right)
        } else {
            (right, left)
        };
        let (_, _, _, arc_length) = circle.element(left.x, right.x)?;
        let geometry = FailureMassGeometry {
            circle: ResolvedCircularSlip {
                surface: s.clone(),
                left_point: world(left, ground.origin),
                right_point: world(right, ground.origin),
                entry_point: world(entry, ground.origin),
                exit_point: world(exit, ground.origin),
                arc_length_m: arc_length,
            },
            terrain_boundary: terrain,
            area_m2: area,
            material_areas,
            numerical_quality: ground.quality(area),
        };
        cancelled(cancel)?;
        Ok(Self {
            ground,
            circle,
            left,
            right,
            breakpoints,
            geometry,
        })
    }
}

fn push_break(points: &mut Vec<f64>, x: f64, config: &CircularAnalysisConfig) -> CResult<()> {
    if points.len() > config.limits.max_slices {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Mandatory geometry intervals exceed max_slices",
        ));
    }
    points.push(x);
    Ok(())
}
pub(crate) fn world(p: Point2, origin: Point2) -> Point2 {
    Point2::new(p.x + origin.x, p.z + origin.z)
}
pub(crate) fn area_above_arc(
    circle: &LocalCircle,
    upper: &[Point2],
    left: f64,
    right: f64,
) -> CResult<f64> {
    let (_, _, segment, _) = circle.element(left, right)?;
    let h0 = eval(upper, left) - circle.z(left)?;
    let h1 = eval(upper, right) - circle.z(right)?;
    let area = (right - left) * ((h0 + h1) * 0.5) + segment;
    finite(area, "integrated circular area")?;
    Ok(area)
}
pub(crate) fn material_areas(
    ground: &GroundModel<'_>,
    circle: &LocalCircle,
    breaks: &[f64],
    config: &CircularAnalysisConfig,
) -> CResult<Vec<MaterialArea>> {
    material_areas_cancellable(ground, circle, breaks, config, &AtomicBool::new(false))
}
pub(crate) fn material_areas_cancellable(
    ground: &GroundModel<'_>,
    circle: &LocalCircle,
    breaks: &[f64],
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
) -> CResult<Vec<MaterialArea>> {
    cancelled(cancel)?;
    let mut sums: Vec<Sum> = (0..ground.source.materials.len())
        .map(|_| Sum::default())
        .collect();
    for pair in breaks.windows(2) {
        cancelled(cancel)?;
        charge(WorkKind::GeometryWork, 1)?;
        let (left, right) = (pair[0], pair[1]);
        let mid = left + (right - left) * 0.5;
        let base = circle.z(mid)?;
        for i in 0..ground.source.soil_regions.len() {
            cancelled(cancel)?;
            charge(WorkKind::MaterialIntegrations, 1)?;
            let upper = &ground.boundaries[i];
            let lower = &ground.boundaries[i + 1];
            let top = eval(upper, mid);
            let bottom = eval(lower, mid);
            let area = if base >= top {
                0.0
            } else if base <= bottom {
                (right - left)
                    * ((eval(upper, left) - eval(lower, left))
                        + (eval(upper, right) - eval(lower, right)))
                    * 0.5
            } else {
                area_above_arc(circle, upper, left, right)?
            };
            let allowed =
                config.tolerances.integral_abs_m2 + config.tolerances.integral_rel * area.abs();
            if !area.is_finite() || area < -allowed {
                return Err(error(
                    CircularErrorCode::NumericOverflow,
                    "Material integral is negative or nonfinite",
                ));
            }
            let id = &ground.source.soil_regions[i].material_id;
            let index = ground
                .source
                .materials
                .iter()
                .position(|m| &m.material_id == id)
                .unwrap();
            sums[index].add(area.max(0.0));
        }
    }
    let mut output = Vec::with_capacity(sums.len());
    for (material, sum) in ground.source.materials.iter().zip(sums) {
        cancelled(cancel)?;
        finite(sum.total(), "material area sum")?;
        output.push(MaterialArea {
            material_id: material.material_id.clone(),
            area_m2: sum.total(),
        });
    }
    cancelled(cancel)?;
    Ok(output)
}

pub(crate) fn validate_input_header(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<()> {
    validate_config(config)?;
    text(&input.analysis_id, "analysis_id", 256)?;
    text(&input.source_revision, "source_revision", 4096)?;
    text(&input.slip_surface.slip_id.0, "slip_id", 256)?;
    finite(input.extrusion_width_m, "extrusion_width_m")?;
    if input.extrusion_width_m <= 0.0 {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "Extrusion width must be positive",
        ));
    }
    if input.units != crate::InputUnits::default() {
        return Err(error(
            CircularErrorCode::InvalidUnits,
            "Circular core requires m, deg, kN/m3 and kPa",
        ));
    }
    finite(input.slip_surface.center.x, "circle center x")?;
    finite(input.slip_surface.center.z, "circle center z")?;
    finite(input.slip_surface.radius_m, "circle radius")?;
    if input.slip_surface.radius_m <= 0.0 {
        return Err(error(
            CircularErrorCode::InvalidCircle,
            "Circle radius must be positive",
        ));
    }
    Ok(())
}

pub fn build_failure_mass(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<FailureMassGeometry> {
    Ok(PreparedMass::new(input, config)?.geometry)
}

/// Disposable CCW render ring. No calculation consumes this approximation.
pub fn render_boundary(
    geometry: &FailureMassGeometry,
    arc_segments: usize,
    max_points: usize,
) -> CResult<Vec<Point2>> {
    if arc_segments < 2
        || arc_segments > 65536
        || max_points > 131072
        || geometry
            .terrain_boundary
            .len()
            .checked_add(arc_segments)
            .is_none_or(|n| n > max_points)
    {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Render boundary budget exceeded",
        ));
    }
    let s = &geometry.circle.surface;
    let left = geometry.circle.left_point;
    let right = geometry.circle.right_point;
    let local = LocalCircle {
        center: Point2::new(0.0, 0.0),
        radius: s.radius_m,
        tolerance: geometry.numerical_quality.intersection_tolerance_m,
    };
    let a = ((left.x - s.center.x) / s.radius_m).clamp(-1.0, 1.0).asin();
    let b = ((right.x - s.center.x) / s.radius_m)
        .clamp(-1.0, 1.0)
        .asin();
    let mut ring = geometry.terrain_boundary.clone();
    // Terrain left->right, then lower arc right->left gives a clockwise ring;
    // reverse it below to use the existing CCW extrusion convention.
    for i in 1..arc_segments {
        let theta = b + (a - b) * (i as f64 / arc_segments as f64);
        let x = s.radius_m * theta.sin();
        let z = local.z(x)?;
        ring.push(Point2::new(x + s.center.x, z + s.center.z));
    }
    ring.reverse();
    Ok(ring)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mandatory_terrain_corner_near_contact_is_not_erased() {
        let mut config = CircularAnalysisConfig::default();
        config.tolerances.intersection_abs_m = 0.001;
        config.slices = SliceConfig {
            target_slices: 1,
            verify_refinement: false,
        };
        let input = CircularAnalysisInput {
            analysis_id: "close-terrain-kink".into(),
            source_revision: "independent-synthetic-geometry-regression".into(),
            ground: GroundSection2D {
                section_id: "close-terrain-kink".into(),
                coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
                terrain_polyline: vec![
                    Point2::new(0.999, -2.0),
                    Point2::new(1.0005, 4.0),
                    Point2::new(9.0, 2.0),
                    Point2::new(10.0, 0.0),
                ],
                layer_interfaces: vec![],
                soil_regions: vec![SoilRegion {
                    region_id: "region-0".into(),
                    material_id: "soil-0".into(),
                }],
                bottom_boundary: vec![Point2::new(0.999, -10.0), Point2::new(10.0, -10.0)],
                materials: vec![GroundMaterial {
                    material_id: "soil-0".into(),
                    properties: crate::SoilMaterial {
                        name: "Synthetic geometry material".into(),
                        unit_weight_kn_per_m3: 18.0,
                        effective_cohesion_kpa: 0.0,
                        effective_friction_angle_deg: 28.0,
                        provenance: "Independent synthetic witness; not design input".into(),
                    },
                }],
                model_revision: 1,
            },
            slip_surface: CircularSlipSurface::from_center_radius(
                "circle-0".into(),
                Point2::new(5.0, 5.0),
                5.0,
                SlideDirection::PositiveX,
            ).unwrap(),
            basis: AnalysisBasis::DryStatic,
            units: crate::InputUnits::default(),
            extrusion_width_m: 1.0,
        };
        // Independent analytic circle segment plus the actual terrain triangle.
        // The erased-corner chord would give an area smaller by 8 square metres.
        let expected_area = 25.0 * (4.0_f64 / 5.0).asin() - 4.0;
        let mass = build_failure_mass(&input, &config).unwrap();
        assert!((mass.area_m2 - expected_area).abs() <= 1e-9);
        assert!((mass.material_areas[0].area_m2 - expected_area).abs() <= 1e-9);
        assert!(mass.terrain_boundary.contains(&Point2::new(1.0005, 4.0)));
        // Retaining the real corner leaves a mandatory width below resolution.
        // Refuse that slice instead of manufacturing a wider, incorrect one.
        let failure = super::super::slice_generator::generate_slices(&input, &config).unwrap_err();
        assert_eq!(failure.code, CircularErrorCode::DegenerateMass);
        // Equal outer endpoint heights give zero one-slice signed driving;
        // this regression makes no factor-of-safety or search-minimum claim.
    }
}
