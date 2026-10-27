//! Persistable geotechnical ground input and derived readiness checks.
//!
//! This module owns only ground structure. It does not accept a circle,
//! analysis configuration, Bishop settings, or a caller-supplied readiness
//! flag. Complete stacks use the default solver bounds (32 soil regions, 2048
//! terrain segments, 8192 total boundary points) and the default geometric
//! tolerance `max(1e-9 m, 4 world-coordinate ULPs, 64 * EPSILON * local scale)`.
//! Every complete boundary must span the terrain's exact X endpoints; each
//! boundary must increase in X by more than that tolerance. Every ordered pair
//! of complete supplied boundaries is checked on its common X interval, even
//! without complete terrain; no boundary is extrapolated. Unequal draft spans
//! are reported as incomplete coverage. Complete terrain retains the solver's
//! exact-span policy.

use super::{
    types::{
        CircularError, CircularErrorCode, GroundMaterial, LayerInterface, NumericalTolerances,
        ResourceLimits, SectionCoordinateSystem, SoilRegion,
    },
    validation::{finite, local, text, ulp, unique_id},
};
use crate::Point2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A local, serializable ground definition without server-owned identity or
/// revision fields. Incomplete drafts are retained as entered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundDefinition {
    pub coordinate_system: SectionCoordinateSystem,
    pub terrain_polyline: Vec<Point2>,
    pub layer_interfaces: Vec<LayerInterface>,
    pub soil_regions: Vec<SoilRegion>,
    pub bottom_boundary: Vec<Point2>,
    pub materials: Vec<GroundMaterial>,
}

impl Default for GroundDefinition {
    fn default() -> Self {
        Self {
            coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
            terrain_polyline: Vec::new(),
            layer_interfaces: Vec::new(),
            soil_regions: Vec::new(),
            bottom_boundary: Vec::new(),
            materials: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundReadinessState {
    Draft,
    Ready,
}

/// Geotechnical input that is still needed before a complete ground stack can
/// be handed to a circular analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundMissingPart {
    TerrainPolyline,
    LayerInterfaces,
    SoilRegions,
    Materials,
    BottomBoundary,
    BoundaryCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundReadinessReport {
    pub state: GroundReadinessState,
    pub missing: Vec<GroundMissingPart>,
}

impl GroundReadinessReport {
    /// Returns true only for a derived ready state with no outstanding parts.
    pub fn is_ready(&self) -> bool {
        self.state == GroundReadinessState::Ready && self.missing.is_empty()
    }
}

/// Validate every supplied value and any complete geometric relationships.
/// Missing parts remain a valid draft; malformed supplied values are errors.
pub fn validate_ground_structure(ground: &GroundDefinition) -> Result<(), CircularError> {
    inspect_inner(ground).map(|_| ())
}

/// Derive draft/readiness from the supplied ground definition.
pub fn inspect_ground_readiness(
    ground: &GroundDefinition,
) -> Result<GroundReadinessReport, CircularError> {
    inspect_inner(ground)
}

fn inspect_inner(ground: &GroundDefinition) -> Result<GroundReadinessReport, CircularError> {
    let limits = ResourceLimits::default();
    let max_regions = limits.max_soil_regions;
    let max_interfaces = max_regions.saturating_sub(1);
    if ground.soil_regions.len() > max_regions
        || ground.materials.len() > max_regions
        || ground.layer_interfaces.len() > max_interfaces
    {
        return Err(err(
            CircularErrorCode::ResourceLimit,
            "Ground region, material, or interface count exceeds the default bounded limits",
        ));
    }
    if ground.terrain_polyline.len() > limits.max_terrain_segments.saturating_add(1) {
        return Err(err(
            CircularErrorCode::ResourceLimit,
            "Terrain point count exceeds the default maximum terrain-segment limit",
        ));
    }

    let mut point_count = ground
        .terrain_polyline
        .len()
        .checked_add(ground.bottom_boundary.len())
        .ok_or_else(|| {
            err(
                CircularErrorCode::ResourceLimit,
                "Boundary point count overflow",
            )
        })?;
    for interface in &ground.layer_interfaces {
        point_count = point_count
            .checked_add(interface.polyline.len())
            .ok_or_else(|| {
                err(
                    CircularErrorCode::ResourceLimit,
                    "Boundary point count overflow",
                )
            })?;
    }
    if point_count > limits.max_boundary_points {
        return Err(err(
            CircularErrorCode::ResourceLimit,
            "Ground boundary point budget exceeds the default limit",
        ));
    }

    let mut material_ids = BTreeSet::new();
    for material in &ground.materials {
        unique_id(&mut material_ids, &material.material_id.0, "material_id")?;
        material
            .properties
            .validate()
            .map_err(|e| err(CircularErrorCode::InvalidParameter, e.to_string()))?;
        text(&material.properties.name, "material name", 4096)?;
        text(&material.properties.provenance, "material provenance", 4096)?;
    }

    let mut region_ids = BTreeSet::new();
    for region in &ground.soil_regions {
        unique_id(&mut region_ids, &region.region_id.0, "region_id")?;
        if !material_ids.contains(&region.material_id.0) {
            return Err(err(
                CircularErrorCode::MissingMaterial,
                format!("Missing material {}", region.material_id.0),
            ));
        }
    }

    let mut interface_ids = BTreeSet::new();
    for interface in &ground.layer_interfaces {
        unique_id(
            &mut interface_ids,
            &interface.interface_id.0,
            "interface_id",
        )?;
    }

    let mut missing = missing_parts(ground);
    validate_supplied_geometry(ground, &mut missing)?;
    let state = if missing.is_empty() {
        GroundReadinessState::Ready
    } else {
        GroundReadinessState::Draft
    };
    Ok(GroundReadinessReport { state, missing })
}

fn missing_parts(ground: &GroundDefinition) -> Vec<GroundMissingPart> {
    let mut missing = Vec::with_capacity(6);
    if ground.terrain_polyline.len() < 2 {
        missing.push(GroundMissingPart::TerrainPolyline);
    }
    if ground.soil_regions.is_empty() {
        missing.push(GroundMissingPart::SoilRegions);
    }
    if ground.materials.is_empty() {
        missing.push(GroundMissingPart::Materials);
    }
    if ground.bottom_boundary.len() < 2 {
        missing.push(GroundMissingPart::BottomBoundary);
    }
    if (!ground.soil_regions.is_empty()
        && ground.layer_interfaces.len().saturating_add(1) < ground.soil_regions.len())
        || ground
            .layer_interfaces
            .iter()
            .any(|interface| interface.polyline.len() < 2)
    {
        missing.push(GroundMissingPart::LayerInterfaces);
    }
    if ground.layer_interfaces.len().saturating_add(1) > ground.soil_regions.len() {
        // Interfaces entered before their corresponding regions are an
        // incomplete authoring state; references that do exist are validated.
        if !missing.contains(&GroundMissingPart::SoilRegions) {
            missing.push(GroundMissingPart::SoilRegions);
        }
    }
    missing
}

fn validate_supplied_geometry(
    ground: &GroundDefinition,
    missing: &mut Vec<GroundMissingPart>,
) -> Result<(), CircularError> {
    let limits = ResourceLimits::default();
    let terrain = &ground.terrain_polyline;
    let mut source_lines: Vec<&[Point2]> = Vec::with_capacity(ground.layer_interfaces.len() + 2);
    source_lines.push(terrain);
    source_lines.extend(
        ground
            .layer_interfaces
            .iter()
            .map(|interface| interface.polyline.as_slice()),
    );
    source_lines.push(&ground.bottom_boundary);

    // The terrain's first point is the solver's normal local origin. A draft
    // without terrain uses the first supplied point, keeping overflow checks
    // meaningful without inventing geometry.
    let origin = source_lines
        .iter()
        .flat_map(|line| line.iter())
        .next()
        .copied()
        .unwrap_or_else(|| Point2::new(0.0, 0.0));
    finite(origin.x, "ground origin x")?;
    finite(origin.z, "ground origin z")?;

    let mut local_lines: Vec<Vec<Point2>> = Vec::with_capacity(source_lines.len());
    let mut resolution = 0.0_f64;
    let mut scale = 0.0_f64;
    for line in &source_lines {
        let mut local_line = Vec::with_capacity(line.len());
        for &point in *line {
            finite(point.x, "ground point x")?;
            finite(point.z, "ground point z")?;
            resolution = resolution.max(ulp(point.x)).max(ulp(point.z));
            let local_point = local(point, origin)?;
            scale = scale.max(local_point.x.abs()).max(local_point.z.abs());
            local_line.push(local_point);
        }
        local_lines.push(local_line);
    }
    finite(resolution, "world coordinate resolution")?;
    finite(scale, "local ground scale")?;
    let tolerance = NumericalTolerances::default()
        .intersection_abs_m
        .max(resolution * 4.0)
        .max(scale * 64.0 * f64::EPSILON);
    finite(tolerance, "ground geometric tolerance")?;

    for (line_index, line) in local_lines.iter().enumerate() {
        if line.len() < 2 {
            continue;
        }
        if line_index == 0 && line.len() - 1 > limits.max_terrain_segments {
            return Err(err(
                CircularErrorCode::ResourceLimit,
                "Terrain point count exceeds the default maximum terrain-segment limit",
            ));
        }
        for pair in line.windows(2) {
            let dx = pair[1].x - pair[0].x;
            let dz = pair[1].z - pair[0].z;
            if !dx.is_finite() || !dz.is_finite() {
                return Err(err(
                    CircularErrorCode::NumericOverflow,
                    "Ground boundary segment differences must be finite",
                ));
            }
            if dx <= tolerance {
                return Err(err(
                    CircularErrorCode::UnsupportedGroundTopology,
                    format!("Boundary {line_index} is not resolvably increasing in X"),
                ));
            }
        }
    }

    // When terrain and another supplied polyline are complete, the supplied
    // line must span exactly the same world-X interval even if other draft
    // fields are still missing.
    if local_lines[0].len() >= 2 {
        let terrain_end_x = local_lines[0].last().unwrap().x;
        for (line_index, line) in local_lines.iter().enumerate().skip(1) {
            if line.len() >= 2 && (line[0].x != 0.0 || line.last().unwrap().x != terrain_end_x) {
                return Err(err(
                    CircularErrorCode::UnsupportedGroundTopology,
                    format!("Boundary {line_index} does not have the exact terrain X span"),
                ));
            }
        }
    }

    // Check every declared-order pair: a disjoint intermediate boundary must
    // not conceal a known crossing between an upper boundary and the bottom.
    // There are at most 33 complete lines and 8192 supplied points in total.
    let complete_lines: Vec<(usize, &[Point2])> = local_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.len() >= 2)
        .map(|(index, line)| (index, line.as_slice()))
        .collect();
    let mut incomplete_coverage = false;
    for (position, &(upper_index, upper)) in complete_lines.iter().enumerate() {
        for &(lower_index, lower) in complete_lines.iter().skip(position + 1) {
            let upper_source = source_lines[upper_index];
            let lower_source = source_lines[lower_index];
            // Decide actual coverage before local subtraction: distinct world
            // endpoints can round to the same translated floating-point X.
            let upper_start = upper_source[0].x;
            let upper_end = upper_source.last().unwrap().x;
            let lower_start = lower_source[0].x;
            let lower_end = lower_source.last().unwrap().x;
            incomplete_coverage |= upper_start != lower_start || upper_end != lower_end;
            let start = upper_start.max(lower_start);
            let end = upper_end.min(lower_end);
            if end < start {
                continue;
            }
            // Include a sole shared endpoint. The gap is piecewise linear,
            // so overlap endpoints plus every distinct source vertex suffice.
            let mut vertices = Vec::with_capacity(upper.len() + lower.len() + 2);
            vertices.extend([start, end]);
            vertices.extend(
                upper_source
                    .iter()
                    .chain(lower_source.iter())
                    .map(|point| point.x)
                    .filter(|&x| start <= x && x <= end),
            );
            vertices.sort_by(f64::total_cmp);
            // Tolerance merging could hide a known crossing at a close vertex.
            vertices.dedup_by(|a, b| *a == *b);
            for world_x in vertices {
                let upper_z = eval_supplied_boundary(upper_source, upper, world_x)?;
                let lower_z = eval_supplied_boundary(lower_source, lower, world_x)?;
                let gap = upper_z - lower_z;
                if !upper_z.is_finite() || !lower_z.is_finite() || !gap.is_finite() {
                    return Err(err(
                        CircularErrorCode::NumericOverflow,
                        "Interpolated ground heights and signed gaps must be finite",
                    ));
                }
                if gap <= tolerance {
                    return Err(err(
                        CircularErrorCode::InvalidGround,
                        "Crossing/touching interfaces, gaps, reversed layers, or a nonclosed stack",
                    ));
                }
            }
        }
    }
    if incomplete_coverage {
        missing.push(GroundMissingPart::BoundaryCoverage);
    }

    if missing.is_empty() {
        // These checks are redundant with the span checks above by design: the
        // ready-state invariant is explicit at the point it is enforced.
        if local_lines[0].len() < 2
            || local_lines.last().is_none_or(|line| line.len() < 2)
            || local_lines.iter().skip(1).any(|line| line.len() < 2)
        {
            return Err(err(
                CircularErrorCode::InvalidGround,
                "A ready ground requires a complete terrain, interface, and bottom boundary",
            ));
        }
    }
    Ok(())
}

fn eval_supplied_boundary(
    source: &[Point2],
    local_line: &[Point2],
    world_x: f64,
) -> Result<f64, CircularError> {
    // Select the segment and subtract its own source X anchor before dividing.
    // Translating every X by a distant shared origin can collapse two distinct
    // vertices from different boundaries and hide a known crossing. Heights
    // still use the shared local Z origin so their signed gap stays local.
    if world_x < source[0].x || world_x > source.last().unwrap().x {
        return Err(err(
            CircularErrorCode::InvalidGround,
            "Cannot extrapolate a supplied ground boundary",
        ));
    }
    let index = source
        .partition_point(|point| point.x <= world_x)
        .saturating_sub(1)
        .min(source.len() - 2);
    let a = source[index];
    let b = source[index + 1];
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    let offset = world_x - a.x;
    if !dx.is_finite() || !dz.is_finite() || !offset.is_finite() || dx <= 0.0 {
        return Err(err(
            CircularErrorCode::NumericOverflow,
            "Ground interpolation differences must be finite with positive X extent",
        ));
    }
    let z = (offset / dx).mul_add(dz, local_line[index].z);
    if !z.is_finite() {
        return Err(err(
            CircularErrorCode::NumericOverflow,
            "Interpolated ground height must be finite",
        ));
    }
    Ok(z)
}

fn err(code: CircularErrorCode, message: impl Into<String>) -> CircularError {
    CircularError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoilMaterial;

    fn material(id: &str) -> GroundMaterial {
        GroundMaterial {
            material_id: id.into(),
            properties: SoilMaterial {
                name: format!("material-{id}"),
                unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 3.0,
                effective_friction_angle_deg: 28.0,
                provenance: "unit-test input".into(),
            },
        }
    }

    fn one_layer_ground() -> GroundDefinition {
        GroundDefinition {
            coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
            terrain_polyline: vec![Point2::new(0.0, 5.0), Point2::new(10.0, 5.0)],
            layer_interfaces: Vec::new(),
            soil_regions: vec![SoilRegion {
                region_id: "region-0".into(),
                material_id: "material-0".into(),
            }],
            bottom_boundary: vec![Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)],
            materials: vec![material("material-0")],
        }
    }

    #[test]
    fn default_is_a_real_empty_cartesian_draft() {
        let definition = GroundDefinition::default();
        let report = inspect_ground_readiness(&definition).unwrap();
        assert_eq!(
            definition.coordinate_system,
            SectionCoordinateSystem::CartesianXzMetres
        );
        assert_eq!(report.state, GroundReadinessState::Draft);
        assert!(!report.is_ready());
        assert!(report.missing.contains(&GroundMissingPart::TerrainPolyline));
        assert!(report.missing.contains(&GroundMissingPart::SoilRegions));
        assert!(report.missing.contains(&GroundMissingPart::Materials));
        assert!(report.missing.contains(&GroundMissingPart::BottomBoundary));
    }

    #[test]
    fn complete_single_layer_stack_derives_ready_without_interfaces() {
        let report = inspect_ground_readiness(&one_layer_ground()).unwrap();
        assert_eq!(report.state, GroundReadinessState::Ready);
        assert!(report.missing.is_empty());
        assert!(report.is_ready());
    }

    #[test]
    fn partial_draft_is_preserved_and_missing_material_reference_is_rejected() {
        let mut draft = GroundDefinition {
            terrain_polyline: vec![Point2::new(0.0, 2.0)],
            ..GroundDefinition::default()
        };
        let before = draft.clone();
        let report = inspect_ground_readiness(&draft).unwrap();
        assert_eq!(draft, before);
        assert_eq!(report.state, GroundReadinessState::Draft);
        draft.soil_regions.push(SoilRegion {
            region_id: "region-0".into(),
            material_id: "not-present".into(),
        });
        assert_eq!(
            inspect_ground_readiness(&draft).unwrap_err().code,
            CircularErrorCode::MissingMaterial
        );
    }

    #[test]
    fn named_interface_with_incomplete_polyline_is_still_a_draft() {
        let mut draft = GroundDefinition {
            soil_regions: vec![
                SoilRegion {
                    region_id: "region-0".into(),
                    material_id: "material-0".into(),
                },
                SoilRegion {
                    region_id: "region-1".into(),
                    material_id: "material-0".into(),
                },
            ],
            materials: vec![material("material-0")],
            ..GroundDefinition::default()
        };
        draft.layer_interfaces.push(LayerInterface {
            interface_id: "interface-0".into(),
            polyline: vec![Point2::new(0.0, 2.0)],
        });
        let report = inspect_ground_readiness(&draft).unwrap();
        assert_eq!(report.state, GroundReadinessState::Draft);
        assert!(report.missing.contains(&GroundMissingPart::LayerInterfaces));
    }

    #[test]
    fn crossing_and_partial_span_corruption_are_rejected() {
        let mut ground = one_layer_ground();
        ground.layer_interfaces.push(LayerInterface {
            interface_id: "interface-0".into(),
            polyline: vec![Point2::new(0.0, 3.0), Point2::new(10.0, 6.0)],
        });
        ground.soil_regions.push(SoilRegion {
            region_id: "region-1".into(),
            material_id: "material-0".into(),
        });
        assert_eq!(
            inspect_ground_readiness(&ground).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );

        let mut bad_span = one_layer_ground();
        bad_span.bottom_boundary[1].x = 9.0;
        assert_eq!(
            validate_ground_structure(&bad_span).unwrap_err().code,
            CircularErrorCode::UnsupportedGroundTopology
        );
    }

    #[test]
    fn rejects_known_crossing_interfaces_without_complete_terrain() {
        for terrain_polyline in [Vec::new(), vec![Point2::new(0.0, 3.0)]] {
            let ground: GroundDefinition = GroundDefinition {
                coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
                terrain_polyline,
                layer_interfaces: vec![
                    LayerInterface {
                        interface_id: "interface-0".into(),
                        polyline: vec![Point2::new(0.0, 2.0), Point2::new(10.0, 0.0)],
                    },
                    LayerInterface {
                        interface_id: "interface-1".into(),
                        polyline: vec![Point2::new(0.0, 1.0), Point2::new(10.0, 1.0)],
                    },
                ],
                soil_regions: vec![
                    SoilRegion {
                        region_id: "region-0".into(),
                        material_id: "material-0".into(),
                    },
                    SoilRegion {
                        region_id: "region-1".into(),
                        material_id: "material-1".into(),
                    },
                    SoilRegion {
                        region_id: "region-2".into(),
                        material_id: "material-2".into(),
                    },
                ],
                bottom_boundary: vec![Point2::new(0.0, -1.0), Point2::new(10.0, -1.0)],
                materials: vec![
                    material("material-0"),
                    material("material-1"),
                    material("material-2"),
                ],
            };

            assert_eq!(
                validate_ground_structure(&ground).unwrap_err().code,
                CircularErrorCode::InvalidGround
            );
            assert_eq!(
                inspect_ground_readiness(&ground).unwrap_err().code,
                CircularErrorCode::InvalidGround
            );
        }
    }

    fn partial_stack(
        upper: Vec<Point2>,
        lower: Vec<Point2>,
        bottom: Vec<Point2>,
    ) -> GroundDefinition {
        let mut ground = one_layer_ground();
        ground.terrain_polyline.clear();
        ground.layer_interfaces = vec![
            LayerInterface {
                interface_id: "interface-0".into(),
                polyline: upper,
            },
            LayerInterface {
                interface_id: "interface-1".into(),
                polyline: lower,
            },
        ];
        ground
            .soil_regions
            .extend(["region-1", "region-2"].into_iter().map(|id| SoilRegion {
                region_id: id.into(),
                material_id: "material-0".into(),
            }));
        ground.bottom_boundary = bottom;
        ground
    }

    #[test]
    fn partial_overlap_is_not_extrapolated_and_coverage_stays_draft() {
        let mut ground = partial_stack(
            vec![Point2::new(0.0, 2.0), Point2::new(10.0, 2.0)],
            vec![Point2::new(5.0, 1.0), Point2::new(15.0, -10.0)],
            vec![Point2::new(5.0, -20.0), Point2::new(15.0, -20.0)],
        );
        // Extrapolating the second interface to X=0 would put it above the
        // first; only the actual common interval [5, 10] has known ordering.
        for terrain in [Vec::new(), vec![Point2::new(0.0, 3.0)]] {
            ground.terrain_polyline = terrain;
            let before = ground.clone();
            validate_ground_structure(&ground).unwrap();
            let report = inspect_ground_readiness(&ground).unwrap();
            assert_eq!(ground, before);
            assert_eq!(report.state, GroundReadinessState::Draft);
            assert!(report.missing.contains(&GroundMissingPart::TerrainPolyline));
            assert!(
                report
                    .missing
                    .contains(&GroundMissingPart::BoundaryCoverage)
            );
        }
    }

    #[test]
    fn disjoint_intermediate_boundary_cannot_hide_known_crossing() {
        let ground = partial_stack(
            vec![Point2::new(0.0, 2.0), Point2::new(10.0, 2.0)],
            vec![Point2::new(20.0, 0.0), Point2::new(30.0, 0.0)],
            vec![Point2::new(0.0, 1.0), Point2::new(10.0, 3.0)],
        );
        assert_eq!(
            validate_ground_structure(&ground).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );

        let mut disjoint = ground;
        disjoint.bottom_boundary = vec![Point2::new(40.0, -2.0), Point2::new(50.0, -2.0)];
        let report = inspect_ground_readiness(&disjoint).unwrap();
        assert_eq!(report.state, GroundReadinessState::Draft);
        assert!(
            report
                .missing
                .contains(&GroundMissingPart::BoundaryCoverage)
        );
    }

    #[test]
    fn sole_shared_endpoint_and_interior_vertices_are_checked() {
        let mut ground = partial_stack(
            vec![Point2::new(0.0, 3.0), Point2::new(10.0, 3.0)],
            vec![Point2::new(10.0, 2.0), Point2::new(20.0, 2.0)],
            vec![Point2::new(10.0, -2.0), Point2::new(20.0, -2.0)],
        );
        assert_eq!(
            inspect_ground_readiness(&ground).unwrap().state,
            GroundReadinessState::Draft
        );
        for lower_z in [3.0, 4.0] {
            ground.layer_interfaces[1].polyline[0].z = lower_z;
            assert_eq!(
                validate_ground_structure(&ground).unwrap_err().code,
                CircularErrorCode::InvalidGround
            );
        }
        ground.layer_interfaces[1].polyline = vec![
            Point2::new(0.0, 2.0),
            Point2::new(5.0, 4.0),
            Point2::new(10.0, 2.0),
        ];
        assert_eq!(
            inspect_ground_readiness(&ground).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );
    }

    #[test]
    fn partial_drafts_reject_nonfinite_derived_arithmetic() {
        let ground = GroundDefinition {
            terrain_polyline: vec![Point2::new(0.0, 0.0)],
            bottom_boundary: vec![Point2::new(-1e308, 1.0), Point2::new(1e308, 1.0)],
            ..GroundDefinition::default()
        };
        let code = validate_ground_structure(&ground).unwrap_err().code;
        assert!(matches!(
            code,
            CircularErrorCode::NonFiniteInput | CircularErrorCode::NumericOverflow
        ));

        let mut translated = partial_stack(
            vec![Point2::new(0.0, 2.0), Point2::new(10.0, 0.0)],
            vec![Point2::new(0.0, 1.0), Point2::new(10.0, 1.0)],
            vec![Point2::new(0.0, -1.0), Point2::new(10.0, -1.0)],
        );
        for point in translated
            .layer_interfaces
            .iter_mut()
            .flat_map(|line| line.polyline.iter_mut())
            .chain(translated.bottom_boundary.iter_mut())
        {
            point.x += 1e9;
            point.z += 1e9;
        }
        assert_eq!(
            validate_ground_structure(&translated).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );
    }

    #[test]
    fn world_domains_remain_distinct_when_translation_rounds_endpoints_together() {
        let endpoint = 1e9_f64;
        let next_endpoint = f64::from_bits(endpoint.to_bits() + 1);
        assert!(next_endpoint > endpoint);
        assert_eq!(endpoint - (-1e9), next_endpoint - (-1e9));

        let mut ground = partial_stack(
            vec![Point2::new(-1e9, 0.0), Point2::new(endpoint, 0.0)],
            vec![Point2::new(next_endpoint, 1.0), Point2::new(3e9, 1.0)],
            vec![Point2::new(4e9, -2.0), Point2::new(5e9, -2.0)],
        );
        let report = inspect_ground_readiness(&ground).unwrap();
        assert_eq!(report.state, GroundReadinessState::Draft);
        assert!(
            report
                .missing
                .contains(&GroundMissingPart::BoundaryCoverage)
        );

        ground.layer_interfaces[1].polyline =
            vec![Point2::new(-1e9, -1.0), Point2::new(next_endpoint, -1.0)];
        ground.bottom_boundary = vec![Point2::new(-1e9, -2.0), Point2::new(endpoint, -2.0)];
        let report = inspect_ground_readiness(&ground).unwrap();
        assert!(
            report
                .missing
                .contains(&GroundMissingPart::BoundaryCoverage)
        );
    }

    #[test]
    fn distinct_world_vertices_do_not_hide_a_known_crossing_after_translation() {
        let x1 = 1e9_f64;
        let x2 = f64::from_bits(x1.to_bits() + 1);
        let mut ground = one_layer_ground();
        ground.terrain_polyline.clear();
        ground.layer_interfaces = vec![LayerInterface {
            interface_id: "interface-0".into(),
            polyline: vec![
                Point2::new(-1e9, 0.5),
                Point2::new(x1 - 1e-4, 0.5),
                Point2::new(x1, 0.0),
                Point2::new(x1 + 1e-3, 0.0),
            ],
        }];
        ground.soil_regions.push(SoilRegion {
            region_id: "region-1".into(),
            material_id: "material-0".into(),
        });
        ground.bottom_boundary = vec![
            Point2::new(-1e9, -1.0),
            Point2::new(x2 - 1e-4, 0.2),
            Point2::new(x2, -1e-4),
            Point2::new(x1 + 1e-3, -1e-4),
        ];
        assert_eq!(
            validate_ground_structure(&ground).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );
        assert_eq!(
            inspect_ground_readiness(&ground).unwrap_err().code,
            CircularErrorCode::InvalidGround
        );
    }
}
