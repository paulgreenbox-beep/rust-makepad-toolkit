use super::{types::*, validation::*};
use crate::Point2;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use super::search_meter::{self, charge, WorkKind};

pub(crate) struct GroundModel<'a> {
    pub source: &'a GroundSection2D,
    pub boundaries: Vec<Vec<Point2>>,
    pub vertices: Vec<f64>,
    pub origin: Point2,
    pub tolerance: f64,
    pub resolution: f64,
}
pub(crate) fn eval(polyline: &[Point2], x: f64) -> f64 {
    let index = polyline
        .partition_point(|p| p.x <= x)
        .saturating_sub(1)
        .min(polyline.len() - 2);
    let a = polyline[index];
    let b = polyline[index + 1];
    ((x - a.x) / (b.x - a.x)).mul_add(b.z - a.z, a.z)
}
fn eval_source_boundary(source: &[Point2], local_line: &[Point2], world_x: f64) -> CResult<f64> {
    // Keep the source segment's X anchor: subtracting a distant shared origin
    // can round distinct world vertices from different boundaries together.
    if world_x < source[0].x || world_x > source.last().unwrap().x {
        return Err(error(
            CircularErrorCode::InvalidGround,
            "Cannot extrapolate a supplied ground boundary",
        ));
    }
    let index = source
        .partition_point(|p| p.x <= world_x)
        .saturating_sub(1)
        .min(source.len() - 2);
    let a = source[index];
    let b = source[index + 1];
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    let offset = world_x - a.x;
    if !dx.is_finite() || !dz.is_finite() || !offset.is_finite() || dx <= 0.0 {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Ground interpolation differences must be finite with positive X extent",
        ));
    }
    let z = (offset / dx).mul_add(dz, local_line[index].z);
    if !z.is_finite() {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Interpolated ground height must be finite",
        ));
    }
    Ok(z)
}
pub(crate) fn dedup_sorted(values: &mut Vec<f64>, tolerance: f64, cancel: &AtomicBool) -> CResult<()> {
    search_meter::sort_by(values, cancel, f64::total_cmp)?;
    search_meter::dedup_by(values, cancel, |a, b| (*a - *b).abs() <= tolerance)
}
impl<'a> GroundModel<'a> {
    pub fn new(ground: &'a GroundSection2D, config: &CircularAnalysisConfig) -> CResult<Self> {
        Self::new_cancellable(ground, config, &AtomicBool::new(false))
    }
    pub fn new_cancellable(ground: &'a GroundSection2D, config: &CircularAnalysisConfig,
        cancel: &AtomicBool) -> CResult<Self> {
        cancelled(cancel)?;
        validate_config(config)?;
        text(&ground.section_id.0, "section_id", 256)?;
        let l = config.limits;
        if ground.soil_regions.is_empty()
            || ground.soil_regions.len() > l.max_soil_regions
            || ground.layer_interfaces.len().checked_add(1) != Some(ground.soil_regions.len())
            || ground.materials.is_empty()
            || ground.materials.len() > l.max_soil_regions
        {
            return Err(error(
                CircularErrorCode::InvalidGround,
                "Require 1..max_soil_regions regions, N-1 interfaces, and explicit materials",
            ));
        }
        if ground.terrain_polyline.len() < 2
            || ground.terrain_polyline.len() - 1 > l.max_terrain_segments
        {
            return Err(error(
                CircularErrorCode::ResourceLimit,
                "Terrain point count is outside its bounded range",
            ));
        }
        let mut points = ground
            .terrain_polyline
            .len()
            .checked_add(ground.bottom_boundary.len())
            .ok_or_else(|| {
                error(
                    CircularErrorCode::ResourceLimit,
                    "Boundary point count overflow",
                )
            })?;
        for interface in &ground.layer_interfaces {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            points = points
                .checked_add(interface.polyline.len())
                .ok_or_else(|| {
                    error(
                        CircularErrorCode::ResourceLimit,
                        "Boundary point count overflow",
                    )
                })?;
        }
        if points > l.max_boundary_points {
            return Err(error(
                CircularErrorCode::ResourceLimit,
                "Ground boundary point budget exceeded",
            ));
        }
        let mut ids = BTreeSet::new();
        for material in &ground.materials {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            unique_id(&mut ids, &material.material_id.0, "material_id")?;
            material
                .properties
                .validate()
                .map_err(|e| error(CircularErrorCode::InvalidParameter, e.to_string()))?;
            text(&material.properties.name, "material name", 4096)?;
            text(&material.properties.provenance, "material provenance", 4096)?;
        }
        let mut regions = BTreeSet::new();
        for region in &ground.soil_regions {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            unique_id(&mut regions, &region.region_id.0, "region_id")?;
            if !ids.contains(&region.material_id.0) {
                return Err(error(
                    CircularErrorCode::MissingMaterial,
                    format!("Missing material {}", region.material_id.0),
                ));
            }
        }
        let mut interface_ids = BTreeSet::new();
        for interface in &ground.layer_interfaces {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            unique_id(
                &mut interface_ids,
                &interface.interface_id.0,
                "interface_id",
            )?;
        }
        let origin = ground.terrain_polyline[0];
        finite(origin.x, "ground origin x")?;
        finite(origin.z, "ground origin z")?;
        let first_x = origin.x;
        let last_x = ground.terrain_polyline.last().unwrap().x;
        finite(last_x, "ground point x")?;
        let mut boundaries = Vec::with_capacity(ground.soil_regions.len() + 1);
        let mut resolution: f64 = 0.0;
        let mut scale: f64 = 0.0;
        let source_boundaries: Vec<&[Point2]> = std::iter::once(ground.terrain_polyline.as_slice())
            .chain(ground.layer_interfaces.iter().map(|v| v.polyline.as_slice()))
            .chain(std::iter::once(ground.bottom_boundary.as_slice()))
            .collect();
        for &boundary in &source_boundaries {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            if boundary.len() < 2 {
                return Err(error(
                    CircularErrorCode::InvalidGround,
                    "Every boundary requires at least two points",
                ));
            }
            // Check exact coverage in source X before shared-origin translation:
            // distinct world endpoints can round to one local endpoint.
            finite(boundary[0].x, "ground point x")?;
            finite(boundary.last().unwrap().x, "ground point x")?;
            if boundary[0].x != first_x || boundary.last().unwrap().x != last_x {
                return Err(error(
                    CircularErrorCode::UnsupportedGroundTopology,
                    "All boundaries must have exactly the terrain X span; pinch-outs and outcrops are unsupported",
                ));
            }
            let mut local_boundary = Vec::with_capacity(boundary.len());
            for &p in boundary {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                finite(p.x, "ground point x")?;
                finite(p.z, "ground point z")?;
                resolution = resolution.max(ulp(p.x)).max(ulp(p.z));
                let q = local(p, origin)?;
                scale = scale.max(q.x.abs()).max(q.z.abs());
                local_boundary.push(q);
            }
            boundaries.push(local_boundary);
        }
        let tolerance = config
            .tolerances
            .intersection_abs_m
            .max(resolution * 4.0)
            .max(scale * 64.0 * f64::EPSILON);
        finite(tolerance, "geometric tolerance")?;
        if last_x - first_x <= tolerance {
            return Err(error(
                CircularErrorCode::UnsupportedGroundTopology,
                "Boundaries must run in strictly increasing X",
            ));
        }
        let mut vertices = Vec::with_capacity(points);
        let mut world_vertices = Vec::with_capacity(points);
        for (i, boundary) in boundaries.iter().enumerate() {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            let source = source_boundaries[i];
            for pair in boundary.windows(2) {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                if pair[1].x - pair[0].x <= tolerance {
                    return Err(error(
                        CircularErrorCode::UnsupportedGroundTopology,
                        format!("Boundary {i} is not resolvably increasing in X"),
                    ));
                }
            }
            for (p, source_point) in boundary.iter().zip(source) {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                vertices.push(p.x);
                world_vertices.push(source_point.x);
            }
        }
        // Validate every distinct world vertex before local X collapse can
        // conceal a crossing. The signed gap is piecewise linear in world X.
        // Sorting and every knot/pair evaluation retain metering/cancellation.
        cancelled(cancel)?;
        dedup_sorted(&mut world_vertices, 0.0, cancel)?;
        cancelled(cancel)?;
        for &x in &world_vertices {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            for i in 0..boundaries.len() - 1 {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                let upper = eval_source_boundary(source_boundaries[i], &boundaries[i], x)?;
                let lower = eval_source_boundary(source_boundaries[i + 1], &boundaries[i + 1], x)?;
                let gap = upper - lower;
                if !gap.is_finite() {
                    return Err(error(
                        CircularErrorCode::NumericOverflow,
                        "Interpolated ground signed gap must be finite",
                    ));
                }
                if gap <= tolerance {
                    return Err(error(
                        CircularErrorCode::InvalidGround,
                        "Crossing/touching interfaces, gaps, reversed layers or a nonclosed stack",
                    ));
                }
            }
        }
        cancelled(cancel)?;
        dedup_sorted(&mut vertices, 0.0, cancel)?;
        cancelled(cancel)?;
        // Consumers use these translated polylines, so retain the local check
        // as well: rounding different source knots together can invert a
        // source-valid stack in its actual computational representation.
        for &x in &vertices {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            for pair in boundaries.windows(2) {
                cancelled(cancel)?;
                charge(WorkKind::GeometryWork, 1)?;
                let gap = eval(&pair[0], x) - eval(&pair[1], x);
                if !gap.is_finite() {
                    return Err(error(
                        CircularErrorCode::NumericOverflow,
                        "Interpolated local ground signed gap must be finite",
                    ));
                }
                if gap <= tolerance {
                    return Err(error(
                        CircularErrorCode::InvalidGround,
                        "Crossing/touching interfaces, gaps, reversed layers or a nonclosed stack",
                    ));
                }
            }
        }
        Ok(Self {
            source: ground,
            boundaries,
            vertices,
            origin,
            tolerance,
            resolution,
        })
    }
    pub fn material(&self, region: usize) -> &GroundMaterial {
        let id = &self.source.soil_regions[region].material_id;
        self.source
            .materials
            .iter()
            .find(|m| &m.material_id == id)
            .expect("validated material reference")
    }
    pub fn quality(&self, area: f64) -> NumericalQuality {
        NumericalQuality {
            local_origin: self.origin,
            world_resolution_m: self.resolution,
            intersection_tolerance_m: self.tolerance,
            estimated_area_roundoff_m2: area.abs() * 128.0 * f64::EPSILON
                + self.resolution * (self.vertices.last().unwrap() - self.vertices[0]) * 8.0,
        }
    }
}

pub fn validate_ground(
    ground: &GroundSection2D,
    config: &CircularAnalysisConfig,
) -> CResult<NumericalQuality> {
    let model = GroundModel::new(ground, config)?;
    Ok(model.quality(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_and_local_crossings_are_rejected_when_x_knots_collapse() {
        let x1 = 1e9_f64;
        let x2 = f64::from_bits(x1.to_bits() + 1);
        let d = 2.0_f64.powi(-13);
        let e = 2.0_f64.powi(-10);
        assert_eq!(x1 - (-1e9), x2 - (-1e9));
        assert_eq!((x1 - d) - (-1e9), (x2 - d) - (-1e9));
        let ground = GroundSection2D {
            section_id: "world-x-collapse".into(),
            coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
            terrain_polyline: vec![Point2::new(-1e9, 1.0), Point2::new(x1 + e, 1.0)],
            layer_interfaces: vec![LayerInterface {
                interface_id: "interface-0".into(),
                polyline: vec![
                    Point2::new(-1e9, 0.5),
                    Point2::new(x1 - d, 0.5),
                    Point2::new(x1, 0.0),
                    Point2::new(x1 + e, 0.0),
                ],
            }],
            soil_regions: vec![
                SoilRegion { region_id: "region-0".into(), material_id: "soil-0".into() },
                SoilRegion { region_id: "region-1".into(), material_id: "soil-1".into() },
            ],
            bottom_boundary: vec![
                Point2::new(-1e9, -1.0),
                Point2::new(x2 - d, 0.2),
                Point2::new(x2, -0.0001),
                Point2::new(x1 + e, -0.0001),
            ],
            materials: [("soil-0", 18.0, 28.0), ("soil-1", 20.0, 22.0)]
                .into_iter()
                .map(|(id, gamma, phi)| GroundMaterial {
                    material_id: id.into(),
                    properties: crate::SoilMaterial {
                        name: format!("Synthetic {id}"),
                        unit_weight_kn_per_m3: gamma,
                        effective_cohesion_kpa: 0.0,
                        effective_friction_angle_deg: phi,
                        provenance: "Independent synthetic topology witness; not design input".into(),
                    },
                })
                .collect(),
            model_revision: 1,
        };
        // At world x1 the interface is zero while the supplied bottom is
        // -0.0001 + 0.2001/1024 > 0. Translation hides that known crossing.
        assert_eq!(
            validate_ground(&ground, &CircularAnalysisConfig::default()).unwrap_err().code,
            CircularErrorCode::InvalidGround,
        );

        // The converse also needs the pre-existing local guard. These source
        // lines have the same 4096 slope after x1-d and a positive 0.0001 gap,
        // but distinct x1/x2 knots coincide locally and invert the stored stack.
        let mut source_valid = ground;
        source_valid.terrain_polyline = vec![
            Point2::new(-1e9, 10.0), Point2::new(x1 + e, 10.0),
        ];
        source_valid.layer_interfaces[0].polyline = vec![
            Point2::new(-1e9, -0.5),
            Point2::new(x1 - d, -0.5),
            Point2::new(x1, 0.0),
            Point2::new(x1 + e, 4.0),
        ];
        source_valid.bottom_boundary = vec![
            Point2::new(-1e9, -0.5001),
            Point2::new(x1 - d, -0.5001),
            Point2::new(x2, 0.00038828125),
            Point2::new(x1 + e, 3.9999),
        ];
        assert_eq!(
            validate_ground(&source_valid, &CircularAnalysisConfig::default()).unwrap_err().code,
            CircularErrorCode::InvalidGround,
        );
    }
}
