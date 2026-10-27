use crate::model::{
    ConfiguratorError, Corner, GridSpacing, GridStation, IssueSeverity, LocalTransform, RoofKind,
    SectionConfig, SemanticId, ValidationIssue,
};
use std::collections::HashSet;

const GEOMETRY_EPSILON: f64 = 1.0e-9;
const MAX_GRID_STATIONS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// Unit building-local direction used for frames and roof slope.
    pub u: [f64; 2],
    /// Perpendicular building-local direction, `v = (-u_y, u_x)`.
    pub v: [f64; 2],
    pub u_min: f64,
    pub u_max: f64,
    pub v_min: f64,
    pub v_max: f64,
}

fn dot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn projected(bounds: &Bounds, xy: [f64; 2]) -> [f64; 2] {
    [dot2(xy, bounds.u), dot2(xy, bounds.v)]
}

fn extent_tolerance(bounds: &Bounds) -> f64 {
    let span = (bounds.u_max - bounds.u_min)
        .abs()
        .max((bounds.v_max - bounds.v_min).abs())
        .max(1.0e-6);
    let offset = bounds
        .u_min
        .abs()
        .max(bounds.u_max.abs())
        .max(bounds.v_min.abs())
        .max(bounds.v_max.abs());
    GEOMETRY_EPSILON * span + 64.0 * f64::EPSILON * offset
}

pub fn bounds(section: &SectionConfig) -> Bounds {
    let u = section.grid.span_direction;
    let v = [-u[1], u[0]];
    let mut result = Bounds {
        u,
        v,
        u_min: f64::INFINITY,
        u_max: f64::NEG_INFINITY,
        v_min: f64::INFINITY,
        v_max: f64::NEG_INFINITY,
    };

    for corner in &section.corners {
        let cu = dot2(corner.xy_m, u);
        let cv = dot2(corner.xy_m, v);
        if cu.is_finite() && cv.is_finite() {
            result.u_min = result.u_min.min(cu);
            result.u_max = result.u_max.max(cu);
            result.v_min = result.v_min.min(cv);
            result.v_max = result.v_max.max(cv);
        }
    }

    if !result.u_min.is_finite() {
        result.u_min = 0.0;
        result.u_max = 0.0;
        result.v_min = 0.0;
        result.v_max = 0.0;
    }
    result
}

/// Convert distances from the projected minima back into building-local XY.
pub fn point_at(bounds: &Bounds, u_distance: f64, v_distance: f64) -> [f64; 2] {
    let u = bounds.u_min + u_distance;
    let v = bounds.v_min + v_distance;
    [
        bounds.u[0] * u + bounds.v[0] * v,
        bounds.u[1] * u + bounds.v[1] * v,
    ]
}

pub fn world_point(transform: &LocalTransform, xy: [f64; 2], z: f64) -> [f64; 3] {
    let (sin, cos) = transform.rotation_rad.sin_cos();
    [
        transform.translation_m[0] + cos * xy[0] - sin * xy[1],
        transform.translation_m[1] + sin * xy[0] + cos * xy[1],
        z,
    ]
}

pub fn roof_height(section: &SectionConfig, bounds: &Bounds, xy: [f64; 2]) -> f64 {
    let roof = &section.roof;
    match roof.kind {
        RoofKind::Flat
        | RoofKind::Hip
        | RoofKind::Curved
        | RoofKind::MultiFacet
        | RoofKind::Stepped
        | RoofKind::Custom => roof.eave_abs_m,
        RoofKind::MonoPitch => {
            let span = bounds.u_max - bounds.u_min;
            if span <= extent_tolerance(bounds) {
                return roof.eave_abs_m;
            }
            let u = dot2(xy, bounds.u);
            let t = ((u - bounds.u_min) / span).clamp(0.0, 1.0);
            roof.eave_abs_m + t * (roof.ridge_abs_m - roof.eave_abs_m)
        }
        RoofKind::Gable => {
            let span = bounds.u_max - bounds.u_min;
            if span <= extent_tolerance(bounds) {
                return roof.eave_abs_m;
            }
            let u = dot2(xy, bounds.u);
            let t = ((u - bounds.u_min) / span).clamp(0.0, 1.0);
            let rise = roof.ridge_abs_m - roof.eave_abs_m;
            if t <= 0.5 {
                roof.eave_abs_m + 2.0 * t * rise
            } else {
                roof.ridge_abs_m - 2.0 * (t - 0.5) * rise
            }
        }
    }
}

pub fn area2(polygon: &[[f64; 2]]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut twice_area = 0.0;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        twice_area += a[0] * b[1] - b[0] * a[1];
    }
    0.5 * twice_area
}

pub fn perimeter2(polygon: &[[f64; 2]]) -> f64 {
    if polygon.len() < 2 {
        return 0.0;
    }
    let mut perimeter = 0.0;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        perimeter += (b[0] - a[0]).hypot(b[1] - a[1]);
    }
    perimeter
}

/// Newell's polygon-area formula; works for concave and arbitrarily oriented faces.
pub fn area3(polygon: &[[f64; 3]]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut n = [0.0; 3];
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    0.5 * n[0].hypot(n[1]).hypot(n[2])
}

fn cross2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn point_on_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2], tolerance: f64) -> bool {
    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
    if length <= tolerance {
        return (point[0] - a[0]).hypot(point[1] - a[1]) <= tolerance;
    }
    if cross2(a, b, point).abs() > tolerance * length {
        return false;
    }
    point[0] >= a[0].min(b[0]) - tolerance
        && point[0] <= a[0].max(b[0]) + tolerance
        && point[1] >= a[1].min(b[1]) - tolerance
        && point[1] <= a[1].max(b[1]) + tolerance
}

pub fn inside_polygon(point: [f64; 2], corners: &[Corner]) -> bool {
    if corners.len() < 3 || !point[0].is_finite() || !point[1].is_finite() {
        return false;
    }
    let polygon: Vec<[f64; 2]> = corners.iter().map(|corner| corner.xy_m).collect();
    let coordinate_scale = polygon
        .iter()
        .flat_map(|p| [p[0].abs(), p[1].abs()])
        .fold(point[0].abs().max(point[1].abs()).max(1.0), f64::max);
    let tolerance = GEOMETRY_EPSILON + 64.0 * f64::EPSILON * coordinate_scale;
    let mut inside = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        if !a[0].is_finite() || !a[1].is_finite() || !b[0].is_finite() || !b[1].is_finite() {
            return false;
        }
        if point_on_segment(point, a, b, tolerance) {
            return true;
        }
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let crossing_x = a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if point[0] < crossing_x {
                inside = !inside;
            }
        }
    }
    inside
}

pub fn is_rectangle(section: &SectionConfig, bounds: &Bounds) -> bool {
    if section.corners.len() != 4 {
        return false;
    }
    let tolerance = extent_tolerance(bounds);
    let u_span = bounds.u_max - bounds.u_min;
    let v_span = bounds.v_max - bounds.v_min;
    if u_span <= tolerance || v_span <= tolerance {
        return false;
    }

    let mut combinations = [[false; 2]; 2];
    for corner in &section.corners {
        let [u, v] = projected(bounds, corner.xy_m);
        let u_side = if (u - bounds.u_min).abs() <= tolerance {
            0
        } else if (u - bounds.u_max).abs() <= tolerance {
            1
        } else {
            return false;
        };
        let v_side = if (v - bounds.v_min).abs() <= tolerance {
            0
        } else if (v - bounds.v_max).abs() <= tolerance {
            1
        } else {
            return false;
        };
        if combinations[u_side][v_side] {
            return false;
        }
        combinations[u_side][v_side] = true;
    }
    combinations
        .iter()
        .all(|row| row.iter().all(|value| *value))
        && area2(&section.corners.iter().map(|c| c.xy_m).collect::<Vec<_>>())
            > tolerance * tolerance
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], tol: f64) -> bool {
    let o1 = cross2(a, b, c);
    let o2 = cross2(a, b, d);
    let o3 = cross2(c, d, a);
    let o4 = cross2(c, d, b);
    let area_tol = tol
        * (b[0] - a[0])
            .hypot(b[1] - a[1])
            .max((d[0] - c[0]).hypot(d[1] - c[1]));
    if ((o1 > area_tol && o2 < -area_tol) || (o1 < -area_tol && o2 > area_tol))
        && ((o3 > area_tol && o4 < -area_tol) || (o3 < -area_tol && o4 > area_tol))
    {
        return true;
    }
    (o1.abs() <= area_tol && point_on_segment(c, a, b, tol))
        || (o2.abs() <= area_tol && point_on_segment(d, a, b, tol))
        || (o3.abs() <= area_tol && point_on_segment(a, c, d, tol))
        || (o4.abs() <= area_tol && point_on_segment(b, c, d, tol))
}

pub fn is_orthogonal_l(section: &SectionConfig, bounds: &Bounds) -> bool {
    if section.corners.len() != 6
        || area2(&section.corners.iter().map(|c| c.xy_m).collect::<Vec<_>>()) <= 0.0
    {
        return false;
    }
    let points: Vec<[f64; 2]> = section
        .corners
        .iter()
        .map(|corner| projected(bounds, corner.xy_m))
        .collect();
    if points.iter().flatten().any(|value| !value.is_finite()) {
        return false;
    }
    let tolerance = extent_tolerance(bounds);

    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let du = (b[0] - a[0]).abs();
        let dv = (b[1] - a[1]).abs();
        if (du <= tolerance) == (dv <= tolerance) {
            return false;
        }
    }

    for i in 0..points.len() {
        let i_next = (i + 1) % points.len();
        for j in (i + 1)..points.len() {
            let j_next = (j + 1) % points.len();
            if i == j || i_next == j || j_next == i {
                continue;
            }
            if segments_intersect(
                points[i],
                points[i_next],
                points[j],
                points[j_next],
                tolerance,
            ) {
                return false;
            }
        }
    }

    let area_tolerance = tolerance
        * (bounds.u_max - bounds.u_min)
            .abs()
            .max((bounds.v_max - bounds.v_min).abs());
    let mut reflex = 0;
    for i in 0..points.len() {
        let prev = points[(i + points.len() - 1) % points.len()];
        let curr = points[i];
        let next = points[(i + 1) % points.len()];
        let turn =
            (curr[0] - prev[0]) * (next[1] - curr[1]) - (curr[1] - prev[1]) * (next[0] - curr[0]);
        if turn < -area_tolerance {
            reflex += 1;
        } else if turn <= area_tolerance {
            return false;
        }
    }
    reflex == 1
}

fn invalid_grid(message: impl Into<String>) -> ConfiguratorError {
    ConfiguratorError::InvalidConfiguration {
        issues: vec![ValidationIssue {
            code: "INVALID_GRID_STATIONS".to_string(),
            path: "/grid/stations".to_string(),
            message: message.into(),
            severity: IssueSeverity::Error,
        }],
    }
}

fn station_resource_limit(reason: impl Into<String>) -> ConfiguratorError {
    ConfiguratorError::ResourceLimit {
        reason: reason.into(),
    }
}

fn generated_station_id(value: String) -> Result<SemanticId, ConfiguratorError> {
    SemanticId::new(value).map_err(|message| ConfiguratorError::Serialization { message })
}

fn station_tolerance(extent: f64) -> f64 {
    1.0e-10 * extent.abs().max(1.0e-6)
}

pub fn stations(spacing: &GridSpacing, extent: f64) -> Result<Vec<GridStation>, ConfiguratorError> {
    if !extent.is_finite() || extent <= 0.0 {
        return Err(invalid_grid(
            "Grid extent must be finite and greater than zero.",
        ));
    }
    let tolerance = station_tolerance(extent);
    if extent <= tolerance {
        return Err(invalid_grid(
            "Grid extent is too small to form nonzero members.",
        ));
    }

    match spacing {
        GridSpacing::Uniform { pitch_m } => {
            if !pitch_m.is_finite() || *pitch_m <= 0.0 {
                return Err(invalid_grid(
                    "Uniform grid pitch must be finite and greater than zero.",
                ));
            }
            let ratio = extent / *pitch_m;
            if !ratio.is_finite() || ratio >= (MAX_GRID_STATIONS - 1) as f64 {
                return Err(station_resource_limit(format!(
                    "Uniform spacing would exceed the {MAX_GRID_STATIONS} station geometry limit."
                )));
            }

            let mut result = Vec::new();
            result.push(GridStation {
                id: generated_station_id("start".to_string())?,
                position_m: 0.0,
            });
            let mut ordinal = 1usize;
            loop {
                let position = *pitch_m * ordinal as f64;
                if !position.is_finite() {
                    return Err(station_resource_limit(
                        "Uniform grid station position overflowed.",
                    ));
                }
                if position >= extent - tolerance {
                    break;
                }
                if result.len() >= MAX_GRID_STATIONS - 1 {
                    return Err(station_resource_limit(format!(
                        "Uniform spacing would exceed the {MAX_GRID_STATIONS} station geometry limit."
                    )));
                }
                result.push(GridStation {
                    id: generated_station_id(format!("g{ordinal:03}"))?,
                    position_m: position,
                });
                ordinal += 1;
            }
            result.push(GridStation {
                id: generated_station_id("end".to_string())?,
                position_m: extent,
            });
            Ok(result)
        }
        GridSpacing::Explicit { stations } => {
            if stations.len() < 2 {
                return Err(invalid_grid(
                    "Explicit spacing requires start and end stations.",
                ));
            }
            if stations.len() > MAX_GRID_STATIONS {
                return Err(station_resource_limit(format!(
                    "Explicit spacing exceeds the {MAX_GRID_STATIONS} station geometry limit."
                )));
            }
            let mut ids = HashSet::with_capacity(stations.len());
            for station in stations {
                if station.id.as_str().trim().is_empty() {
                    return Err(invalid_grid("Explicit grid station IDs must not be empty."));
                }
                if !ids.insert(station.id.as_str()) {
                    return Err(invalid_grid("Explicit grid station IDs must be unique."));
                }
                if !station.position_m.is_finite() {
                    return Err(invalid_grid(
                        "Explicit grid station positions must be finite.",
                    ));
                }
            }
            if stations[0].position_m.abs() > tolerance
                || (stations[stations.len() - 1].position_m - extent).abs() > tolerance
            {
                return Err(invalid_grid(
                    "Explicit spacing must include positions 0 and extent as its endpoints.",
                ));
            }
            for pair in stations.windows(2) {
                if pair[1].position_m - pair[0].position_m <= tolerance {
                    return Err(invalid_grid(
                        "Explicit grid stations must be strictly ordered with nonzero spacing.",
                    ));
                }
            }
            if stations.iter().any(|station| {
                station.position_m < -tolerance || station.position_m > extent + tolerance
            }) {
                return Err(invalid_grid(
                    "Explicit grid station positions must lie within the grid extent.",
                ));
            }

            let mut result = stations.clone();
            result[0].position_m = 0.0;
            let last_index = result.len() - 1;
            result[last_index].position_m = extent;
            Ok(result)
        }
    }
}

fn roof_point(section: &SectionConfig, bounds: &Bounds, u: f64, v: f64) -> [f64; 3] {
    let xy = point_at(bounds, u, v);
    [xy[0], xy[1], roof_height(section, bounds, xy)]
}

pub fn roof_surfaces(section: &SectionConfig) -> Vec<(String, Vec<[f64; 3]>)> {
    if section.corners.len() < 3 {
        return Vec::new();
    }
    let bounds = bounds(section);
    match section.roof.kind {
        RoofKind::Flat => {
            if !is_rectangle(section, &bounds) && !is_orthogonal_l(section, &bounds) {
                return Vec::new();
            }
            let vertices = section
                .corners
                .iter()
                .map(|corner| [corner.xy_m[0], corner.xy_m[1], section.roof.eave_abs_m])
                .collect();
            vec![(format!("{}:roof:flat", section.id), vertices)]
        }
        RoofKind::MonoPitch if is_rectangle(section, &bounds) => {
            let u_span = bounds.u_max - bounds.u_min;
            let v_span = bounds.v_max - bounds.v_min;
            let vertices = vec![
                roof_point(section, &bounds, 0.0, 0.0),
                roof_point(section, &bounds, u_span, 0.0),
                roof_point(section, &bounds, u_span, v_span),
                roof_point(section, &bounds, 0.0, v_span),
            ];
            vec![(format!("{}:roof:mono", section.id), vertices)]
        }
        RoofKind::Gable if is_rectangle(section, &bounds) => {
            let u_span = bounds.u_max - bounds.u_min;
            let v_span = bounds.v_max - bounds.v_min;
            let middle = 0.5 * u_span;
            let left = vec![
                roof_point(section, &bounds, 0.0, 0.0),
                roof_point(section, &bounds, middle, 0.0),
                roof_point(section, &bounds, middle, v_span),
                roof_point(section, &bounds, 0.0, v_span),
            ];
            let right = vec![
                roof_point(section, &bounds, middle, 0.0),
                roof_point(section, &bounds, u_span, 0.0),
                roof_point(section, &bounds, u_span, v_span),
                roof_point(section, &bounds, middle, v_span),
            ];
            vec![
                (format!("{}:roof:gable:left", section.id), left),
                (format!("{}:roof:gable:right", section.id), right),
            ]
        }
        // Hip, curved, faceted, stepped, custom, and nonrectangular sloped roofs are left to the caller's
        // typed generation-limit path instead of returning misleading panels.
        RoofKind::MonoPitch
        | RoofKind::Gable
        | RoofKind::Hip
        | RoofKind::Curved
        | RoofKind::MultiFacet
        | RoofKind::Stepped
        | RoofKind::Custom => Vec::new(),
    }
}
