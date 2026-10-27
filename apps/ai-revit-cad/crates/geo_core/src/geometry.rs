//! Bounded queries over explicit borehole-connected TIN cells.
//!
//! The topology is supplied by the source model. Nothing in this module infers
//! triangles, extends a surface outside a triangle, or turns missing evidence
//! into a numerical zero.

use crate::{
    Borehole, BoreholeId, DerivedLayerVolume, GeoBoreholeProjection, GeoError, GeoQueryResult, GeoRecords,
    GeoSectionResult, GeoSectionSample, GeoTriangle, GroundColumn, GroundModel, LayerContact,
    MAX_ABS_COORDINATE_M, MAX_CONTACTS, MAX_MODEL_SPAN_M,
    MAX_MODEL_WELLS, MAX_RECORDS, MAX_SECTION_POINTS, MAX_TRIANGLES, MissingBoreholeCrsPolicy,
    Parameter, ParameterUnit, QueriedLayer, QueryStatus, ResolvedGeoSnapshot, SourceId,
    SourceOrigin, UnknownZone, ValueClass,
};
use std::collections::{BTreeMap, BTreeSet};

type Point = [f64; 2];

// Coordinates are bounded by the public model. All determinants use translated
// differences, so a large survey origin does not dominate a local triangle.
const POSITION_TOL_M: f64 = 1e-9;
const ELEVATION_TOL_M: f64 = 1e-8;
const MIN_TRIANGLE_ALTITUDE_M: f64 = 1e-6;

fn invalid(path: impl Into<String>, message: impl Into<String>) -> GeoError {
    GeoError::InvalidInput { path: path.into(), message: message.into() }
}

fn conflict(reason: impl Into<String>) -> GeoError {
    GeoError::GeometryConflict { reason: reason.into() }
}

fn limit(reason: impl Into<String>) -> GeoError {
    GeoError::ResourceLimit { reason: reason.into() }
}

fn finite_coordinate(value: f64, path: &str) -> Result<(), GeoError> {
    if !value.is_finite() || value.abs() > MAX_ABS_COORDINATE_M {
        return Err(invalid(path, format!("coordinate must be finite and within {MAX_ABS_COORDINATE_M} m")));
    }
    Ok(())
}

fn finite_point(point: Point, path: &str) -> Result<(), GeoError> {
    finite_coordinate(point[0], &format!("{path}[0]"))?;
    finite_coordinate(point[1], &format!("{path}[1]"))
}

fn sub(a: Point, b: Point) -> Point { [a[0] - b[0], a[1] - b[1]] }
fn cross(a: Point, b: Point) -> f64 { a[0] * b[1] - a[1] * b[0] }
fn orient(a: Point, b: Point, c: Point) -> f64 { cross(sub(b, a), sub(c, a)) }
fn distance(a: Point, b: Point) -> f64 { (a[0] - b[0]).hypot(a[1] - b[1]) }
fn elevation_tol(a: f64, b: f64) -> f64 {
    ELEVATION_TOL_M + 128.0 * f64::EPSILON * a.abs().max(b.abs())
}

fn roundoff_area_tolerance(points: &[Point; 3]) -> f64 {
    let edge = distance(points[0], points[1])
        .max(distance(points[1], points[2]))
        .max(distance(points[2], points[0]));
    (128.0 * f64::EPSILON * edge * edge).max(1e-12)
}

fn area_tolerance(points: &[Point; 3]) -> f64 {
    let edge = distance(points[0], points[1])
        .max(distance(points[1], points[2]))
        .max(distance(points[2], points[0]));
    roundoff_area_tolerance(points).max(MIN_TRIANGLE_ALTITUDE_M * edge)
}

fn triangle_points(records: &GeoRecords, triangle: &GeoTriangle) -> Result<[Point; 3], GeoError> {
    let point = |id: &BoreholeId| -> Result<Point, GeoError> {
        let well = records.boreholes.get(id).ok_or_else(|| GeoError::MissingReference {
            entity: "borehole".into(), id: id.to_string(),
        })?;
        Ok([well.collar_m[0], well.collar_m[1]])
    };
    Ok([
        point(&triangle.vertices[0])?, point(&triangle.vertices[1])?,
        point(&triangle.vertices[2])?,
    ])
}

fn triangle_wells<'a>(records: &'a GeoRecords, triangle: &GeoTriangle)
    -> Result<[&'a Borehole; 3], GeoError>
{
    let get = |id: &BoreholeId| records.boreholes.get(id).ok_or_else(||
        GeoError::MissingReference { entity: "borehole".into(), id: id.to_string() });
    Ok([get(&triangle.vertices[0])?, get(&triangle.vertices[1])?, get(&triangle.vertices[2])?])
}

fn polygon_area2(poly: &[Point]) -> f64 {
    if poly.len() < 3 { return 0.0; }
    let origin = poly[0];
    (1..poly.len() - 1).map(|i| orient(origin, poly[i], poly[i + 1])).sum()
}

fn bbox_overlaps(a: &[Point; 3], b: &[Point; 3]) -> bool {
    let bounds = |p: &[Point; 3], axis: usize| {
        let values = [p[0][axis], p[1][axis], p[2][axis]];
        (values.iter().copied().fold(f64::INFINITY, f64::min),
         values.iter().copied().fold(f64::NEG_INFINITY, f64::max))
    };
    for axis in 0..2 {
        let (alo, ahi) = bounds(a, axis);
        let (blo, bhi) = bounds(b, axis);
        if ahi < blo - POSITION_TOL_M || bhi < alo - POSITION_TOL_M { return false; }
    }
    true
}

// Sutherland-Hodgman clipping gives the positive-area intersection of two
// convex projected triangles. A shared edge or shared vertex has zero area.
fn overlap_area(a: &[Point; 3], b: &[Point; 3]) -> f64 {
    let sign = if orient(b[0], b[1], b[2]) >= 0.0 { 1.0 } else { -1.0 };
    let mut polygon = a.to_vec();
    for edge in 0..3 {
        let c = b[edge];
        let d = b[(edge + 1) % 3];
        let input = std::mem::take(&mut polygon);
        if input.is_empty() { break; }
        let mut prev = *input.last().expect("nonempty clip polygon");
        let mut prev_side = sign * orient(c, d, prev);
        for next in input {
            let next_side = sign * orient(c, d, next);
            let prev_inside = prev_side >= 0.0;
            let next_inside = next_side >= 0.0;
            if prev_inside != next_inside {
                let denominator = prev_side - next_side;
                if denominator != 0.0 {
                    let t = (prev_side / denominator).clamp(0.0, 1.0);
                    polygon.push([
                        prev[0] + t * (next[0] - prev[0]),
                        prev[1] + t * (next[1] - prev[1]),
                    ]);
                }
            }
            if next_inside { polygon.push(next); }
            prev = next;
            prev_side = next_side;
        }
    }
    polygon_area2(&polygon).abs() * 0.5
}

fn strictly_inside_triangle(cell: &[Point; 3], point: Point) -> bool {
    let area2 = orient(cell[0], cell[1], cell[2]);
    if area2 == 0.0 { return false; }
    let w1 = orient(cell[0], point, cell[2]) / area2;
    let w2 = orient(cell[0], cell[1], point) / area2;
    let w0 = 1.0 - w1 - w2;
    let edge = distance(cell[0], cell[1])
        .max(distance(cell[1], cell[2]))
        .max(distance(cell[2], cell[0]));
    let tolerance = (POSITION_TOL_M * edge / area2.abs()).max(128.0 * f64::EPSILON);
    [w0, w1, w2].iter().all(|w| *w > tolerance)
}

fn point_on_segment(point: Point, a: Point, b: Point) -> bool {
    let ab = sub(b, a);
    let ap = sub(point, a);
    let length = distance(a, b);
    if length <= POSITION_TOL_M { return distance(point, a) <= POSITION_TOL_M; }
    let signed_distance = cross(ab, ap).abs() / length;
    if signed_distance > POSITION_TOL_M { return false; }
    let projection = ap[0] * ab[0] + ap[1] * ab[1];
    projection >= -POSITION_TOL_M * length
        && projection <= length * length + POSITION_TOL_M * length
}

fn point_on_segment_interior(point: Point, a: Point, b: Point) -> bool {
    point_on_segment(point, a, b)
        && distance(point, a) > POSITION_TOL_M
        && distance(point, b) > POSITION_TOL_M
}

fn proper_segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let s1 = orient(a, b, c);
    let s2 = orient(a, b, d);
    let t1 = orient(c, d, a);
    let t2 = orient(c, d, b);
    let scale = distance(a, b).max(distance(c, d));
    let tol = POSITION_TOL_M * scale;
    ((s1 > tol && s2 < -tol) || (s1 < -tol && s2 > tol))
        && ((t1 > tol && t2 < -tol) || (t1 < -tol && t2 > tol))
}

fn edges_conform(a: &GeoTriangle, ap: &[Point; 3], b: &GeoTriangle, bp: &[Point; 3]) -> bool {
    for i in 0..3 {
        let ai = (i + 1) % 3;
        for j in 0..3 {
            let bj = (j + 1) % 3;
            let same_edge = (a.vertices[i] == b.vertices[j] && a.vertices[ai] == b.vertices[bj])
                || (a.vertices[i] == b.vertices[bj] && a.vertices[ai] == b.vertices[j]);
            if same_edge { continue; }
            let (p, q, r, s) = (ap[i], ap[ai], bp[j], bp[bj]);
            if proper_segments_cross(p, q, r, s)
                || point_on_segment_interior(p, r, s)
                || point_on_segment_interior(q, r, s)
                || point_on_segment_interior(r, p, q)
                || point_on_segment_interior(s, p, q)
            { return false; }
        }
    }
    true
}

fn validate_unknown_zone(zone: &UnknownZone, index: usize) -> Result<(), GeoError> {
    let path = format!("ground_model.unknown_zones[{index}]");
    let points = &zone.polygon_xy_m;
    if points.len() < 3 || points.len() > MAX_SECTION_POINTS {
        return Err(limit(format!("{path} polygon needs 3..={MAX_SECTION_POINTS} vertices")));
    }
    for (i, p) in points.iter().enumerate() {
        finite_point(*p, &format!("{path}.polygon_xy_m[{i}]"))?;
        if distance(*p, points[(i + 1) % points.len()]) <= POSITION_TOL_M {
            return Err(conflict(format!("{path} has a repeated or zero-length edge")));
        }
    }
    if polygon_area2(points).abs() <= 1e-12 {
        return Err(conflict(format!("{path} has zero projected area")));
    }
    for i in 0..points.len() {
        let next_i = (i + 1) % points.len();
        for j in i + 1..points.len() {
            let next_j = (j + 1) % points.len();
            if i == j || next_i == j || next_j == i { continue; }
            let (a, b, c, d) = (points[i], points[next_i], points[j], points[next_j]);
            if proper_segments_cross(a, b, c, d)
                || point_on_segment(c, a, b) || point_on_segment(d, a, b)
                || point_on_segment(a, c, d) || point_on_segment(b, c, d)
            { return Err(conflict(format!("{path} is self-intersecting"))); }
        }
    }
    Ok(())
}

fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        if point_on_segment(point, a, b) { return true; }
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let x = a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if x > point[0] { inside = !inside; }
        }
    }
    inside
}

fn unknown_zone_intersects_cell(zone: &UnknownZone, cell: &[Point; 3]) -> bool {
    if zone.polygon_xy_m.iter().any(|&p| barycentric(cell, p).is_some())
        || cell.iter().any(|&p| point_in_polygon(p, &zone.polygon_xy_m))
    { return true; }
    for i in 0..zone.polygon_xy_m.len() {
        let a = zone.polygon_xy_m[i];
        let b = zone.polygon_xy_m[(i + 1) % zone.polygon_xy_m.len()];
        for j in 0..3 {
            let c = cell[j];
            let d = cell[(j + 1) % 3];
            if proper_segments_cross(a, b, c, d)
                || point_on_segment(a, c, d) || point_on_segment(b, c, d)
                || point_on_segment(c, a, b) || point_on_segment(d, a, b)
            { return true; }
        }
    }
    false
}

fn checked_parameter(parameter: &Parameter<f64>, path: &str) -> Result<Option<f64>, GeoError> {
    let check = |v: f64| -> Result<(), GeoError> { finite_coordinate(v, path) };
    match parameter {
        Parameter::Missing { .. } => Ok(None),
        Parameter::Known { value, metadata, .. } => {
            if metadata.unit != ParameterUnit::Metre {
                return Err(invalid(path, "contact elevation requires explicit METRE unit metadata"));
            }
            check(*value)?; Ok(Some(*value))
        }
        Parameter::Conflicting { candidates } => {
            if candidates.is_empty() { return Err(invalid(path, "conflicting parameter has no candidates")); }
            for candidate in candidates {
                if candidate.metadata.unit != ParameterUnit::Metre {
                    return Err(invalid(path, "contact elevation candidate requires METRE unit metadata"));
                }
                check(candidate.value)?;
            }
            Ok(None)
        }
    }
}

/// Validate only model geometry and its explicit TIN references. Missing or
/// conflicting contact observations remain representable source facts; queries
/// report their coverage rather than inventing an interpolated elevation.
pub fn validate_model_geometry(records: &GeoRecords, model: &GroundModel) -> Result<(), GeoError> {
    let unknown_vertices = model.unknown_zones.iter().try_fold(0usize, |total, zone| {
        total.checked_add(zone.polygon_xy_m.len())
            .ok_or_else(|| limit("unknown-zone vertex count overflow"))
    })?;
    if model.borehole_ids.len() > MAX_MODEL_WELLS
        || model.triangles.len() > MAX_TRIANGLES
        || model.contacts.len() > MAX_CONTACTS
        || model.unknown_zones.len() > MAX_RECORDS
        || unknown_vertices > MAX_RECORDS
    { return Err(limit("ground model exceeds bounded geometry counts")); }
    let mut well_ids = BTreeSet::new();
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    let mut declared_well_crs: Option<&str> = None;
    for id in &model.borehole_ids {
        if !well_ids.insert(id.clone()) { return Err(conflict(format!("duplicate model borehole {id}"))); }
        let well = records.boreholes.get(id).ok_or_else(|| GeoError::MissingReference {
            entity: "borehole".into(), id: id.to_string(),
        })?;
        if let Some(crs) = well.crs.as_deref() {
            if model.frame.crs.as_deref().is_some_and(|frame| frame != crs)
                || declared_well_crs.is_some_and(|prior| prior != crs)
            {
                return Err(conflict(format!("borehole {id} CRS conflicts with the ground model frame")));
            }
            declared_well_crs = Some(crs);
        }
        for axis in 0..3 {
            finite_coordinate(well.collar_m[axis], &format!("borehole[{id}].collar_m[{axis}]"))?;
        }
        for axis in 0..2 {
            lo[axis] = lo[axis].min(well.collar_m[axis]);
            hi[axis] = hi[axis].max(well.collar_m[axis]);
        }
        z_min = z_min.min(well.collar_m[2]);
        z_max = z_max.max(well.collar_m[2]);
    }
    if !model.borehole_ids.is_empty()
        && (hi[0] - lo[0] > MAX_MODEL_SPAN_M || hi[1] - lo[1] > MAX_MODEL_SPAN_M)
    { return Err(limit("model collar span exceeds the coordinate budget")); }

    let mut triangle_ids = BTreeSet::new();
    let mut undirected_cells = BTreeSet::new();
    let mut edge_incidence: BTreeMap<(BoreholeId, BoreholeId), usize> = BTreeMap::new();
    let mut triangles = Vec::with_capacity(model.triangles.len());
    for triangle in &model.triangles {
        if !triangle_ids.insert(triangle.id.clone()) {
            return Err(conflict(format!("duplicate triangle {}", triangle.id)));
        }
        if triangle.vertices[0] == triangle.vertices[1]
            || triangle.vertices[1] == triangle.vertices[2]
            || triangle.vertices[2] == triangle.vertices[0]
        { return Err(conflict(format!("triangle {} repeats a borehole", triangle.id))); }
        for id in &triangle.vertices {
            if !well_ids.contains(id) {
                return Err(GeoError::MissingReference { entity: "model triangle borehole".into(), id: id.to_string() });
            }
        }
        let mut key = triangle.vertices.clone();
        key.sort();
        if !undirected_cells.insert(key) {
            return Err(conflict(format!("duplicate triangle cell {}", triangle.id)));
        }
        let points = triangle_points(records, triangle)?;
        let area2 = orient(points[0], points[1], points[2]);
        if !area2.is_finite() || area2.abs() <= area_tolerance(&points) {
            return Err(conflict(format!("triangle {} is degenerate in XY", triangle.id)));
        }
        for i in 0..3 {
            let mut edge = [triangle.vertices[i].clone(), triangle.vertices[(i + 1) % 3].clone()];
            edge.sort();
            let count = edge_incidence.entry((edge[0].clone(), edge[1].clone())).or_default();
            *count += 1;
            if *count > 2 { return Err(conflict(format!("non-manifold edge at triangle {}", triangle.id))); }
        }
        triangles.push(points);
    }
    let used_wells: Vec<_> = model.triangles.iter()
        .flat_map(|triangle| triangle.vertices.iter())
        .collect::<BTreeSet<_>>().into_iter().collect();
    for i in 0..used_wells.len() {
        for j in i + 1..used_wells.len() {
            let a = records.boreholes.get(used_wells[i]).expect("checked triangle borehole");
            let b = records.boreholes.get(used_wells[j]).expect("checked triangle borehole");
            if distance([a.collar_m[0], a.collar_m[1]], [b.collar_m[0], b.collar_m[1]])
                <= POSITION_TOL_M
            {
                return Err(conflict(format!("TIN vertices {} and {} have coincident XY collars",
                    a.id, b.id)));
            }
        }
    }
    for i in 0..triangles.len() {
        for j in i + 1..triangles.len() {
            let (a, b) = (&triangles[i], &triangles[j]);
            if !bbox_overlaps(a, b) { continue; }
            let tol = roundoff_area_tolerance(a).max(roundoff_area_tolerance(b)) * 16.0;
            if a.iter().any(|&point| strictly_inside_triangle(b, point))
                || b.iter().any(|&point| strictly_inside_triangle(a, point))
                || overlap_area(a, b) > tol
            {
                return Err(conflict(format!("triangles {} and {} overlap in XY",
                    model.triangles[i].id, model.triangles[j].id)));
            }
            if !edges_conform(&model.triangles[i], a, &model.triangles[j], b) {
                return Err(conflict(format!("triangles {} and {} have nonconforming edges",
                    model.triangles[i].id, model.triangles[j].id)));
            }
        }
    }

    for (index, zone) in model.unknown_zones.iter().enumerate() {
        validate_unknown_zone(zone, index)?;
    }
    let mut contact_ids = BTreeSet::new();
    for contact in &model.contacts {
        if contact.elevations_abs_m.len() > MAX_MODEL_WELLS
            || contact.elevations_abs_m.len() > model.borehole_ids.len()
        {
            return Err(limit(format!("horizon {} has more vertex elevations than model boreholes",
                contact.id)));
        }
        if !contact_ids.insert(contact.id.clone()) {
            return Err(conflict(format!("duplicate horizon {}", contact.id)));
        }
        if !records.iges.contains_key(&contact.ige_id) {
            return Err(GeoError::MissingReference { entity: "IGE".into(), id: contact.ige_id.to_string() });
        }
        for (id, parameter) in &contact.elevations_abs_m {
            if !well_ids.contains(id) {
                return Err(GeoError::MissingReference { entity: "contact borehole".into(), id: id.to_string() });
            }
            let path = format!("contact[{}].elevations_abs_m[{id}]", contact.id);
            if let Some(value) = checked_parameter(parameter, &path)? {
                z_min = z_min.min(value);
                z_max = z_max.max(value);
            }
            if let Parameter::Conflicting { candidates } = parameter {
                for candidate in candidates {
                    z_min = z_min.min(candidate.value);
                    z_max = z_max.max(candidate.value);
                }
            }
        }
    }
    if !model.borehole_ids.is_empty() && z_max - z_min > MAX_MODEL_SPAN_M {
        return Err(limit("model elevation span exceeds the coordinate budget"));
    }
    for id in &model.borehole_ids {
        let well = records.boreholes.get(id).expect("checked model borehole");
        let mut previous = well.collar_m[2];
        for contact in &model.contacts {
            if let Some(parameter) = contact.elevations_abs_m.get(id) {
                if let Some(bottom) = checked_parameter(parameter, &format!("contact[{}][{id}]", contact.id))? {
                    let tol = elevation_tol(previous, bottom);
                    if bottom > previous + tol {
                        return Err(conflict(format!("horizon {} rises above its previous known top at borehole {id}", contact.id)));
                    }
                    if !model.allow_pinch && (bottom - previous).abs() <= tol {
                        return Err(conflict(format!("horizon {} pinches at borehole {id} without allow_pinch", contact.id)));
                    }
                    previous = bottom;
                }
            }
        }
    }
    Ok(())
}

fn result(status: QueryStatus, reason: impl Into<String>, source_ids: Vec<SourceId>) -> GeoQueryResult {
    let reason = reason.into();
    let source_ids = source_ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    GeoQueryResult {
        status, column: None, section: None, volumes: Vec::new(),
        reasons: if reason.is_empty() { Vec::new() } else { vec![reason] },
        source_ids,
    }
}

fn model_for_query(resolved: &ResolvedGeoSnapshot) -> Result<(&GeoRecords, &GroundModel), GeoError> {
    let snapshot = &resolved.snapshot;
    crate::validate_snapshot_binding(snapshot)?;
    let model = snapshot.records.models.get(&snapshot.selection.model_id).ok_or_else(||
        GeoError::MissingReference { entity: "snapshot model".into(), id: snapshot.selection.model_id.to_string() })?;
    if model.id != snapshot.selection.model_id || resolved.scenario.model_id != model.id {
        return Err(GeoError::SnapshotBindingMismatch { reason: "resolved model/scenario identity differs from snapshot selection".into() });
    }
    let Some(id) = &snapshot.selection.scenario_id else {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "resolved query requires a selected scenario".into(),
        });
    };
    if &resolved.scenario.id != id
        || snapshot.records.scenarios.get(id) != Some(&resolved.scenario)
    {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "resolved scenario differs from the immutable snapshot".into(),
        });
    }
    Ok((&snapshot.records, model))
}

// Missing reference declarations remain valid draft source records. They do
// not supply a coordinate transform or an elevation datum for a numeric query.
fn numeric_reference_issue(records: &GeoRecords, model: &GroundModel) -> Option<GeoQueryResult> {
    let model_sources = model.source_ids.clone();
    let Some(frame_crs) = model.frame.crs.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Some(result(QueryStatus::Unknown, "MODEL_CRS_NOT_DECLARED", model_sources));
    };
    let Some(frame_z) = model.frame.vertical_datum.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Some(result(QueryStatus::Unknown, "MODEL_VERTICAL_DATUM_NOT_DECLARED", model_sources));
    };
    for id in &model.borehole_ids {
        let well = records.boreholes.get(id)?;
        let mut sources = model.source_ids.clone();
        sources.extend(well.source_ids.iter().cloned());
        sources.extend(well.survey_source_ids.iter().cloned());
        if well.survey_source_ids.is_empty() && well.source_ids.is_empty() {
            return Some(result(QueryStatus::Unknown,
                format!("COLLAR_SURVEY_REFERENCE_MISSING: borehole {id}"), sources));
        }
        match well.crs.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(crs) if crs == frame_crs => {}
            Some(_) => return Some(result(QueryStatus::Unsupported,
                format!("BOREHOLE_CRS_DIFFERS_FROM_MODEL: borehole {id}"), sources)),
            None if model.frame.missing_borehole_crs == MissingBoreholeCrsPolicy::InheritModel => {}
            None => return Some(result(QueryStatus::Unknown,
                format!("BOREHOLE_CRS_NOT_DECLARED: borehole {id}"), sources)),
        }
        match well.collar_vertical_datum.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(datum) if datum == frame_z => {}
            Some(_) => return Some(result(QueryStatus::Unsupported,
                format!("COLLAR_VERTICAL_DATUM_DIFFERS_FROM_MODEL: borehole {id}"), sources)),
            None => return Some(result(QueryStatus::Unknown,
                format!("COLLAR_VERTICAL_DATUM_NOT_DECLARED: borehole {id}"), sources)),
        }
        if well.depth_datum.is_empty() {
            return Some(result(QueryStatus::Unknown,
                format!("BOREHOLE_DEPTH_DATUM_NOT_DECLARED: borehole {id}"), sources));
        }
        if well.depth_datum != "BELOW_COLLAR" {
            return Some(result(QueryStatus::Unsupported,
                format!("UNSUPPORTED_BOREHOLE_DEPTH_DATUM: borehole {id} requires BELOW_COLLAR"),
                sources));
        }
    }
    for contact in &model.contacts {
        let mut sources = model.source_ids.clone();
        sources.extend(contact.source_ids.iter().cloned());
        match contact.vertical_datum.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(datum) if datum == frame_z => {}
            Some(_) => return Some(result(QueryStatus::Unsupported,
                format!("CONTACT_VERTICAL_DATUM_DIFFERS_FROM_MODEL: horizon {}", contact.id),
                sources)),
            None => return Some(result(QueryStatus::Unknown,
                format!("CONTACT_VERTICAL_DATUM_NOT_DECLARED: horizon {}", contact.id), sources)),
        }
    }
    None
}

fn barycentric(points: &[Point; 3], point: Point) -> Option<[f64; 3]> {
    let area2 = orient(points[0], points[1], points[2]);
    if area2 == 0.0 { return None; }
    let w1 = orient(points[0], point, points[2]) / area2;
    let w2 = orient(points[0], points[1], point) / area2;
    let w0 = 1.0 - w1 - w2;
    let edge = distance(points[0], points[1])
        .max(distance(points[1], points[2]))
        .max(distance(points[2], points[0]));
    let tolerance = (POSITION_TOL_M * edge / area2.abs()).max(128.0 * f64::EPSILON);
    if [w0, w1, w2].iter().all(|w| *w >= -tolerance && *w <= 1.0 + tolerance) {
        let mut weights = [w0.max(0.0), w1.max(0.0), w2.max(0.0)];
        let sum = weights.iter().sum::<f64>();
        if sum <= 0.0 { return None; }
        for weight in &mut weights { *weight /= sum; }
        Some(weights)
    } else { None }
}

fn parameter_in_triangle(contact: &LayerContact, wells: [&Borehole; 3])
    -> Result<([f64; 3], Vec<SourceId>), GeoQueryResult>
{
    let mut values = [0.0; 3];
    let mut sources = BTreeSet::new();
    sources.extend(contact.source_ids.iter().cloned());
    for (i, well) in wells.iter().enumerate() {
        let Some(parameter) = contact.elevations_abs_m.get(&well.id) else {
            return Err(result(QueryStatus::Unknown,
                format!("horizon {} has no elevation at borehole {}", contact.id, well.id),
                sources.into_iter().collect()));
        };
        match parameter {
            Parameter::Known { value, source_ids, .. } => {
                values[i] = *value;
                sources.extend(source_ids.iter().cloned());
            }
            Parameter::Missing { reason } => {
                return Err(result(QueryStatus::Unknown,
                    format!("horizon {} missing at borehole {}: {reason}", contact.id, well.id),
                    sources.into_iter().collect()));
            }
            Parameter::Conflicting { candidates } => {
                for candidate in candidates { sources.extend(candidate.source_ids.iter().cloned()); }
                return Err(result(QueryStatus::Conflicting,
                    format!("horizon {} conflicts at borehole {}", contact.id, well.id),
                    sources.into_iter().collect()));
            }
        }
    }
    Ok((values, sources.into_iter().collect()))
}

fn weighted(values: [f64; 3], weights: [f64; 3]) -> f64 {
    values[0] * weights[0] + values[1] * weights[1] + values[2] * weights[2]
}

fn column_on_triangle(records: &GeoRecords, model: &GroundModel,
    triangle: &GeoTriangle, weights: [f64; 3], xy: Point) -> Result<GeoQueryResult, GeoError>
{
    let wells = triangle_wells(records, triangle)?;
    let terrain_values = [wells[0].collar_m[2], wells[1].collar_m[2], wells[2].collar_m[2]];
    let terrain = weighted(terrain_values, weights);
    let mut top_values = terrain_values;
    let mut layers = Vec::with_capacity(model.contacts.len());
    let mut sources = BTreeSet::new();
    sources.extend(model.source_ids.iter().cloned());
    sources.extend(triangle.source_ids.iter().cloned());
    for well in wells {
        sources.extend(well.source_ids.iter().cloned());
        sources.extend(well.survey_source_ids.iter().cloned());
        sources.extend(well.evidence_source_ids.iter().cloned());
    }
    if model.contacts.is_empty() {
        return Ok(result(QueryStatus::Unknown, "ground model has no layer contacts",
            sources.into_iter().collect()));
    }
    for contact in &model.contacts {
        let (bottom_values, parameter_sources) = match parameter_in_triangle(contact, wells) {
            Ok(values) => values,
            Err(mut coverage) => {
                coverage.source_ids.extend(sources);
                coverage.source_ids.sort(); coverage.source_ids.dedup();
                return Ok(coverage);
            }
        };
        sources.extend(parameter_sources);
        if let Some(ige) = records.iges.get(&contact.ige_id) {
            sources.extend(ige.source_ids.iter().cloned());
        }
        let top = weighted(top_values, weights);
        let bottom = weighted(bottom_values, weights);
        if !top.is_finite() || !bottom.is_finite() {
            return Err(conflict(format!("nonfinite TIN interpolation in triangle {}", triangle.id)));
        }
        let tol = elevation_tol(top, bottom);
        if bottom > top + tol {
            return Err(conflict(format!("horizon {} crosses above its top inside triangle {}", contact.id, triangle.id)));
        }
        layers.push(QueriedLayer {
            ige_id: contact.ige_id.clone(), top_abs_m: top, bottom_abs_m: bottom,
            pinched: (top - bottom).abs() <= tol,
            source_ids: sources.iter().cloned().collect(),
        });
        top_values = bottom_values;
    }
    let source_ids: Vec<_> = sources.into_iter().collect();
    let column = GroundColumn {
        xy_m: xy, terrain_abs_m: terrain, layers,
        triangle_ids: vec![triangle.id.clone()], source_ids: source_ids.clone(),
        method: model.interpolation,
    };
    Ok(GeoQueryResult {
        status: QueryStatus::Known, column: Some(column), section: None,
        volumes: Vec::new(), reasons: Vec::new(), source_ids,
    })
}

fn columns_agree(a: &GroundColumn, b: &GroundColumn) -> bool {
    if a.method != b.method || a.layers.len() != b.layers.len()
        || (a.terrain_abs_m - b.terrain_abs_m).abs() > elevation_tol(a.terrain_abs_m, b.terrain_abs_m)
    { return false; }
    a.layers.iter().zip(&b.layers).all(|(a, b)| {
        a.ige_id == b.ige_id && a.pinched == b.pinched
            && (a.top_abs_m - b.top_abs_m).abs() <= elevation_tol(a.top_abs_m, b.top_abs_m)
            && (a.bottom_abs_m - b.bottom_abs_m).abs() <= elevation_tol(a.bottom_abs_m, b.bottom_abs_m)
    })
}

// A contact declared MEASURED is an assertion about the log at each source
// vertex used by a TIN cell. It needs the same IGE and matching top/bottom
// depths in that well's preserved interval log. An explicitly cited
// INTERPRETED source permits a modeled boundary to differ from observations.
fn measured_log_issue_at_well(records: &GeoRecords, model: &GroundModel, well: &Borehole)
    -> Option<GeoQueryResult>
{
    let mut top_abs = well.collar_m[2];
    for contact in &model.contacts {
        let Some(Parameter::Known { value, source_ids, metadata }) =
            contact.elevations_abs_m.get(&well.id) else {
                // Core contact evaluation returns Missing/Conflicting for this
                // cell; a deeper top cannot be checked against this log.
                return None;
            };
        if metadata.value_class == ValueClass::Measured {
            let interpreted = contact.source_ids.iter().chain(source_ids)
                .any(|source_id| records.sources.get(source_id)
                    .is_some_and(|source| source.origin == SourceOrigin::Interpreted));
            if !interpreted {
                let top_depth = well.collar_m[2] - top_abs;
                let bottom_depth = well.collar_m[2] - *value;
                let mut found_ige = false;
                let mut found_matching_interval = false;
                let mut sources = BTreeSet::new();
                sources.extend(well.source_ids.iter().cloned());
                sources.extend(well.survey_source_ids.iter().cloned());
                sources.extend(well.evidence_source_ids.iter().cloned());
                sources.extend(contact.source_ids.iter().cloned());
                sources.extend(source_ids.iter().cloned());
                for interval in &well.intervals {
                    if interval.ige_id != contact.ige_id { continue; }
                    found_ige = true;
                    sources.extend(interval.source_ids.iter().cloned());
                    if (interval.top_depth_m - top_depth).abs()
                        <= elevation_tol(interval.top_depth_m, top_depth)
                        && (interval.bottom_depth_m - bottom_depth).abs()
                            <= elevation_tol(interval.bottom_depth_m, bottom_depth)
                    { found_matching_interval = true; }
                }
                if !found_ige {
                    return Some(result(QueryStatus::Unknown,
                        format!("MEASURED_CONTACT_HAS_NO_MATCHING_IGE_LOG: horizon {} at borehole {}",
                            contact.id, well.id), sources.into_iter().collect()));
                }
                if !found_matching_interval {
                    return Some(result(QueryStatus::Conflicting,
                        format!("MEASURED_CONTACT_DISAGREES_WITH_BOREHOLE_LOG: horizon {} at borehole {}",
                            contact.id, well.id), sources.into_iter().collect()));
                }
            }
        }
        top_abs = *value;
    }
    None
}

fn measured_cell_issue(records: &GeoRecords, model: &GroundModel, triangle: &GeoTriangle,
    cache: &mut BTreeMap<BoreholeId, Option<GeoQueryResult>>)
    -> Option<GeoQueryResult>
{
    for id in &triangle.vertices {
        let issue = cache.entry(id.clone()).or_insert_with(|| {
            records.boreholes.get(id)
                .and_then(|well| measured_log_issue_at_well(records, model, well))
        });
        if let Some(issue) = issue {
            return Some(issue.clone());
        }
    }
    None
}

fn query_column_validated_with_cache(records: &GeoRecords, model: &GroundModel, xy: Point,
    measured_cache: &mut BTreeMap<BoreholeId, Option<GeoQueryResult>>)
    -> Result<GeoQueryResult, GeoError>
{
    for zone in &model.unknown_zones {
        if point_in_polygon(xy, &zone.polygon_xy_m) {
            return Ok(result(QueryStatus::Unknown,
                format!("explicit unknown zone: {}", zone.reason), zone.source_ids.clone()));
        }
    }
    let mut found: Option<GeoQueryResult> = None;
    for triangle in &model.triangles {
        let points = triangle_points(records, triangle)?;
        let Some(weights) = barycentric(&points, xy) else { continue; };
        let candidate = if let Some(issue) = measured_cell_issue(records, model, triangle,
            measured_cache) {
            issue
        } else {
            column_on_triangle(records, model, triangle, weights, xy)?
        };
        if let Some(ref mut existing) = found {
            let mut union: BTreeSet<_> = existing.source_ids.iter().cloned().collect();
            union.extend(candidate.source_ids.iter().cloned());
            existing.source_ids = union.into_iter().collect();
            let agreement = match (&existing.column, &candidate.column) {
                (Some(a), Some(b)) => Some(columns_agree(a, b)),
                _ => None,
            };
            match agreement {
                Some(true) => {
                    let other = candidate.column.as_ref().expect("checked column");
                    let old = existing.column.as_mut().expect("checked column");
                    old.triangle_ids.extend(other.triangle_ids.iter().cloned());
                    old.triangle_ids.sort(); old.triangle_ids.dedup();
                    old.source_ids = existing.source_ids.clone();
                }
                Some(false) => return Ok(result(QueryStatus::Conflicting,
                    "adjacent TIN cells disagree at the query point", existing.source_ids.clone())),
                None => {
                    let status = if existing.status == QueryStatus::Conflicting
                        || candidate.status == QueryStatus::Conflicting { QueryStatus::Conflicting }
                        else { QueryStatus::Unknown };
                    let mut reasons: BTreeSet<_> = existing.reasons.iter().cloned().collect();
                    reasons.extend(candidate.reasons.iter().cloned());
                    reasons.insert("TIN cells do not share complete contact evidence".into());
                    let mut merged = result(status, "", existing.source_ids.clone());
                    merged.reasons = reasons.into_iter().collect();
                    return Ok(merged);
                }
            }
        } else { found = Some(candidate); }
    }
    Ok(found.unwrap_or_else(|| result(QueryStatus::OutsideCoverage,
        "point is outside explicit TIN coverage", Vec::new())))
}

fn query_column_validated(records: &GeoRecords, model: &GroundModel, xy: Point)
    -> Result<GeoQueryResult, GeoError>
{
    let mut measured_cache = BTreeMap::new();
    query_column_validated_with_cache(records, model, xy, &mut measured_cache)
}

/// Query a complete vertical column within an explicit projected TIN cell.
pub fn query_column(resolved: &ResolvedGeoSnapshot, xy_m: [f64; 2]) -> Result<GeoQueryResult, GeoError> {
    finite_point(xy_m, "column.xy_m")?;
    let (records, model) = model_for_query(resolved)?;
    if let Some(issue) = numeric_reference_issue(records, model) { return Ok(issue); }
    query_column_validated(records, model, xy_m)
}

/// Query up to MAX_RECORDS point columns against one immutable selected
/// snapshot. Each result is the same typed coverage as `query_column`; the
/// batch is a collection of local samples, never a whole-area assessment.
pub fn query_columns(resolved: &ResolvedGeoSnapshot, xy_m: &[[f64; 2]])
    -> Result<Vec<GeoQueryResult>, GeoError>
{
    if xy_m.len() > MAX_RECORDS {
        return Err(limit(format!("column batch exceeds {MAX_RECORDS} points")));
    }
    for (index, &xy) in xy_m.iter().enumerate() {
        finite_point(xy, &format!("columns.xy_m[{index}]"))?;
    }
    let (records, model) = model_for_query(resolved)?;
    let basis_issue = numeric_reference_issue(records, model);
    let mut answers = Vec::with_capacity(xy_m.len());
    if let Some(issue) = basis_issue {
        answers.resize(xy_m.len(), issue);
        return Ok(answers);
    }
    let mut measured_cache = BTreeMap::new();
    for &xy in xy_m {
        answers.push(query_column_validated_with_cache(records, model, xy,
            &mut measured_cache)?);
    }
    Ok(answers)
}

/// Sample the supplied XY path at bounded station spacing. The output carries
/// each point's own coverage status; an incomplete section is never labelled
/// wholly known.
#[derive(Clone, Copy)]
struct PathSegment {
    start: Point,
    end: Point,
    length: f64,
    base_station: f64,
}

#[derive(Clone, Copy)]
struct StationPoint { station: f64, xy: Point }

fn station_tolerance(a: f64, b: f64) -> f64 {
    POSITION_TOL_M + 128.0 * f64::EPSILON * a.abs().max(b.abs())
}

fn insert_station(stations: &mut Vec<StationPoint>, point: StationPoint) -> Result<(), GeoError> {
    if !point.station.is_finite() || !point.xy.iter().all(|value| value.is_finite()) {
        return Err(conflict("nonfinite section station"));
    }
    let at = stations.partition_point(|prior| prior.station < point.station);
    if (at > 0 && (stations[at - 1].station - point.station).abs()
        <= station_tolerance(stations[at - 1].station, point.station))
        || (at < stations.len() && (stations[at].station - point.station).abs()
            <= station_tolerance(stations[at].station, point.station))
    { return Ok(()); }
    if stations.len() >= MAX_SECTION_POINTS {
        return Err(limit("section TIN boundary and spacing stations exceed the point budget"));
    }
    stations.insert(at, point);
    Ok(())
}

fn append_edge_crossings(stations: &mut Vec<StationPoint>, path: PathSegment, c: Point, d: Point)
    -> Result<(), GeoError>
{
    let r = sub(path.end, path.start);
    let s = sub(d, c);
    let q = sub(c, path.start);
    let denominator = cross(r, s);
    let edge_length = distance(c, d);
    let area_tol = POSITION_TOL_M * path.length.max(edge_length);
    let mut append = |t: f64| -> Result<(), GeoError> {
        let t = t.clamp(0.0, 1.0);
        insert_station(stations, StationPoint {
            station: path.base_station + t * path.length,
            xy: [path.start[0] + t * r[0], path.start[1] + t * r[1]],
        })
    };
    if denominator.abs() > area_tol {
        let t = cross(q, s) / denominator;
        let u = cross(q, r) / denominator;
        let t_tol = POSITION_TOL_M / path.length;
        let u_tol = if edge_length > 0.0 { POSITION_TOL_M / edge_length } else { 0.0 };
        if t >= -t_tol && t <= 1.0 + t_tol && u >= -u_tol && u <= 1.0 + u_tol {
            append(t)?;
        }
    } else if cross(q, r).abs() <= area_tol {
        let length2 = r[0] * r[0] + r[1] * r[1];
        if point_on_segment(c, path.start, path.end) {
            append((q[0] * r[0] + q[1] * r[1]) / length2)?;
        }
        if point_on_segment(d, path.start, path.end) {
            let v = sub(d, path.start);
            append((v[0] * r[0] + v[1] * r[1]) / length2)?;
        }
    }
    Ok(())
}

fn project_boreholes(records: &GeoRecords, model: &GroundModel, segments: &[(PathSegment, usize)])
    -> Result<Vec<GeoBoreholeProjection>, GeoError>
{
    let mut projections = Vec::with_capacity(model.borehole_ids.len());
    for id in &model.borehole_ids {
        let well = records.boreholes.get(id).ok_or_else(|| GeoError::MissingReference {
            entity: "borehole".into(), id: id.to_string(),
        })?;
        let xy = [well.collar_m[0], well.collar_m[1]];
        let mut best = (f64::INFINITY, f64::INFINITY); // offset, chainage
        for &(path, _) in segments {
            let direction = sub(path.end, path.start);
            let from_start = sub(xy, path.start);
            let length2 = path.length * path.length;
            let t = ((from_start[0] * direction[0] + from_start[1] * direction[1]) / length2)
                .clamp(0.0, 1.0);
            let foot = [path.start[0] + t * direction[0], path.start[1] + t * direction[1]];
            let candidate = (distance(xy, foot), path.base_station + t * path.length);
            if candidate.0 < best.0 - POSITION_TOL_M
                || ((candidate.0 - best.0).abs() <= POSITION_TOL_M && candidate.1 < best.1)
            { best = candidate; }
        }
        let mut sources = BTreeSet::new();
        sources.extend(well.source_ids.iter().cloned());
        sources.extend(well.survey_source_ids.iter().cloned());
        sources.extend(well.evidence_source_ids.iter().cloned());
        projections.push(GeoBoreholeProjection {
            borehole_id: id.clone(), chainage_m: best.1, offset_m: best.0,
            source_ids: sources.into_iter().collect(),
        });
    }
    projections.sort_by(|a, b| a.chainage_m.total_cmp(&b.chainage_m)
        .then_with(|| a.borehole_id.cmp(&b.borehole_id)));
    Ok(projections)
}

pub fn query_section(resolved: &ResolvedGeoSnapshot, polyline: &[[f64; 2]], max_spacing_m: f64)
    -> Result<GeoQueryResult, GeoError>
{
    if polyline.len() < 2 || polyline.len() > MAX_SECTION_POINTS {
        return Err(limit(format!("section path needs 2..={MAX_SECTION_POINTS} points")));
    }
    if !max_spacing_m.is_finite() || max_spacing_m <= 0.0 {
        return Err(invalid("section.max_spacing_m", "spacing must be finite and positive"));
    }
    let (records, model) = model_for_query(resolved)?;
    if let Some(issue) = numeric_reference_issue(records, model) { return Ok(issue); }
    let mut segments = Vec::with_capacity(polyline.len() - 1);
    let mut count = 1usize;
    let mut base_station = 0.0;
    for (i, pair) in polyline.windows(2).enumerate() {
        finite_point(pair[0], &format!("section.polyline_xy_m[{i}]"))?;
        finite_point(pair[1], &format!("section.polyline_xy_m[{}]", i + 1))?;
        let length = distance(pair[0], pair[1]);
        if !length.is_finite() || length <= POSITION_TOL_M {
            return Err(invalid(format!("section.segment[{i}]"), "segment has zero length"));
        }
        let n_float = (length / max_spacing_m).ceil().max(1.0);
        if !n_float.is_finite() || n_float > MAX_SECTION_POINTS as f64 {
            return Err(limit("section sampling exceeds the point budget"));
        }
        let n = n_float as usize;
        count = count.checked_add(n).ok_or_else(|| limit("section point count overflow"))?;
        if count > MAX_SECTION_POINTS {
            return Err(limit("section sampling exceeds the point budget"));
        }
        segments.push((PathSegment { start: pair[0], end: pair[1], length, base_station }, n));
        base_station += length;
    }
    let mut stations = Vec::with_capacity(MAX_SECTION_POINTS);
    insert_station(&mut stations, StationPoint { station: 0.0, xy: polyline[0] })?;
    for &(path, n) in &segments {
        for j in 1..=n {
            let t = j as f64 / n as f64;
            insert_station(&mut stations, StationPoint {
                station: path.base_station + t * path.length,
                xy: [path.start[0] + t * (path.end[0] - path.start[0]),
                     path.start[1] + t * (path.end[1] - path.start[1])],
            })?;
        }
    }
    // Facet changes must be stations even when a coarse requested spacing
    // would otherwise jump over them. Unknown-zone boundaries likewise split
    // the path; no interpolation crosses an unsampled source boundary.
    for &(path, _) in &segments {
        for triangle in &model.triangles {
            let points = triangle_points(records, triangle)?;
            for i in 0..3 {
                append_edge_crossings(&mut stations, path, points[i], points[(i + 1) % 3])?;
            }
        }
        for zone in &model.unknown_zones {
            for i in 0..zone.polygon_xy_m.len() {
                append_edge_crossings(&mut stations, path, zone.polygon_xy_m[i],
                    zone.polygon_xy_m[(i + 1) % zone.polygon_xy_m.len()])?;
            }
        }
    }
    let mut samples = Vec::with_capacity(stations.len());
    for point in stations {
        let query = query_column_validated(records, model, point.xy)?;
        samples.push(GeoSectionSample { station_m: point.station, result: query });
    }
    let borehole_projections = project_boreholes(records, model, &segments)?;
    let mut sources = BTreeSet::new();
    let mut reasons = BTreeSet::new();
    let mut all_outside = true;
    let mut any_unknown = false;
    let mut any_conflict = false;
    let mut any_unsupported = false;
    for sample in &samples {
        sources.extend(sample.result.source_ids.iter().cloned());
        reasons.extend(sample.result.reasons.iter().cloned());
        match sample.result.status {
            QueryStatus::Known => { all_outside = false; }
            QueryStatus::Unknown => { all_outside = false; any_unknown = true; }
            QueryStatus::Conflicting => { all_outside = false; any_conflict = true; }
            QueryStatus::OutsideCoverage => { any_unknown = true; }
            QueryStatus::Unsupported => { all_outside = false; any_unsupported = true; }
        }
    }
    for projection in &borehole_projections {
        sources.extend(projection.source_ids.iter().cloned());
    }
    let status = if any_conflict { QueryStatus::Conflicting }
        else if any_unsupported { QueryStatus::Unsupported }
        else if all_outside { QueryStatus::OutsideCoverage }
        else if any_unknown { QueryStatus::Unknown }
        else { QueryStatus::Known };
    Ok(GeoQueryResult {
        status, column: None,
        section: Some(Box::new(GeoSectionResult {
            samples, borehole_projections, method: model.interpolation,
        })),
        volumes: Vec::new(), reasons: reasons.into_iter().collect(),
        source_ids: sources.into_iter().collect(),
    })
}

fn volume_contact_values(contact: &LayerContact, wells: [&Borehole; 3])
    -> Result<([f64; 3], Vec<SourceId>), GeoQueryResult>
{
    parameter_in_triangle(contact, wells)
}

/// Integrate linear thickness over each explicit triangle. Any unresolved
/// contact or unknown zone prevents a complete model total; partial sums are
/// deliberately withheld.
pub fn layer_volumes(resolved: &ResolvedGeoSnapshot) -> Result<GeoQueryResult, GeoError> {
    let (records, model) = model_for_query(resolved)?;
    if let Some(issue) = numeric_reference_issue(records, model) { return Ok(issue); }
    if model.triangles.is_empty() {
        return Ok(result(QueryStatus::OutsideCoverage, "model has no TIN cells", Vec::new()));
    }
    if model.contacts.is_empty() {
        return Ok(result(QueryStatus::Unknown, "model has no layer contacts", Vec::new()));
    }
    if !model.unknown_zones.is_empty() {
        for zone in &model.unknown_zones {
            for triangle in &model.triangles {
                let points = triangle_points(records, triangle)?;
                if unknown_zone_intersects_cell(zone, &points) {
                    return Ok(result(QueryStatus::Unknown,
                        "volume total withheld where an explicit unknown zone crosses TIN coverage",
                        zone.source_ids.clone()));
                }
            }
        }
    }
    let mut sums: Vec<(f64, f64, BTreeSet<SourceId>)> = model.contacts.iter()
        .map(|c| (0.0, 0.0, c.source_ids.iter().cloned().collect())).collect();
    let mut measured_cache = BTreeMap::new();
    for triangle in &model.triangles {
        if let Some(issue) = measured_cell_issue(records, model, triangle,
            &mut measured_cache) {
            return Ok(issue);
        }
        let points = triangle_points(records, triangle)?;
        let area = orient(points[0], points[1], points[2]).abs() * 0.5;
        let wells = triangle_wells(records, triangle)?;
        let mut top = [wells[0].collar_m[2], wells[1].collar_m[2], wells[2].collar_m[2]];
        for (index, contact) in model.contacts.iter().enumerate() {
            let (bottom, sources) = match volume_contact_values(contact, wells) {
                Ok(values) => values,
                Err(coverage) => return Ok(coverage),
            };
            let thickness = [top[0] - bottom[0], top[1] - bottom[1], top[2] - bottom[2]];
            let value = area * (thickness[0] + thickness[1] + thickness[2]) / 3.0;
            if !value.is_finite() || value < -ELEVATION_TOL_M * area {
                return Err(conflict(format!("invalid layer volume in triangle {}", triangle.id)));
            }
            let entry = &mut sums[index];
            let y = value.max(0.0) - entry.1;
            let t = entry.0 + y;
            entry.1 = (t - entry.0) - y;
            entry.0 = t;
            entry.2.extend(sources);
            entry.2.extend(triangle.source_ids.iter().cloned());
            for well in wells {
                entry.2.extend(well.source_ids.iter().cloned());
                entry.2.extend(well.survey_source_ids.iter().cloned());
                entry.2.extend(well.evidence_source_ids.iter().cloned());
            }
            if let Some(ige) = records.iges.get(&contact.ige_id) {
                entry.2.extend(ige.source_ids.iter().cloned());
            }
            top = bottom;
        }
    }
    let mut volumes = Vec::with_capacity(model.contacts.len());
    let mut all_sources = BTreeSet::new();
    all_sources.extend(model.source_ids.iter().cloned());
    for (contact, (volume, _, sources)) in model.contacts.iter().zip(sums) {
        all_sources.extend(sources.iter().cloned());
        volumes.push(DerivedLayerVolume {
            ige_id: contact.ige_id.clone(), volume_m3: volume,
            source_ids: sources.into_iter().collect(), method: model.interpolation,
        });
    }
    Ok(GeoQueryResult {
        status: QueryStatus::Known, column: None, section: None, volumes,
        reasons: vec!["VOLUME_SCOPE_EXPLICIT_TIN_CELLS_ONLY".into()],
        source_ids: all_sources.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        make_geo_snapshot, resolve_geo_scenario, synthetic_demo_state, GeoSnapshotId,
        GeoSnapshotSelection, GeoState, GroundModelId, GroundScenarioId, IgeId,
        InterpolationMethod, ParameterCandidate, ParameterMetadata, ParameterUnit, SourceOrigin,
        ValueClass,
    };
    use crate::fixtures::{SYNTHETIC_DEMO_MODEL_ID, SYNTHETIC_DEMO_SCENARIO_ID};

    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() <= 1e-8,
            "actual {actual} differs from analytical {expected}");
    }

    fn selected_model(state: &GeoState) -> &GroundModel {
        let id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).unwrap();
        state.records.models.get(&id).unwrap()
    }

    fn selected_model_mut(state: &mut GeoState) -> &mut GroundModel {
        let id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).unwrap();
        state.records.models.get_mut(&id).unwrap()
    }

    fn resolved(state: &GeoState) -> ResolvedGeoSnapshot {
        let scenario_id = GroundScenarioId::new(SYNTHETIC_DEMO_SCENARIO_ID).unwrap();
        let selection = GeoSnapshotSelection {
            model_id: GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).unwrap(),
            scenario_id: Some(scenario_id.clone()),
        };
        let snapshot = make_geo_snapshot(
            state, GeoSnapshotId::new("GEOMETRY-TEST-SNAPSHOT").unwrap(), &selection,
            state.head_revision.clone().unwrap(),
        ).unwrap();
        let scenario = state.records.scenarios.get(&scenario_id).unwrap();
        resolve_geo_scenario(&snapshot, scenario).unwrap()
    }

    fn assert_all_numeric_queries_hold(state: &GeoState, status: QueryStatus) {
        let snapshot = resolved(state);
        let answers = [
            query_column(&snapshot, [5.0, 6.0]).unwrap(),
            query_section(&snapshot, &[[0.0, 6.0], [10.0, 6.0]], 4.0).unwrap(),
            layer_volumes(&snapshot).unwrap(),
        ];
        for answer in answers {
            assert_eq!(answer.status, status);
            assert!(answer.column.is_none());
            assert!(answer.section.is_none());
            assert!(answer.volumes.is_empty());
            assert!(!answer.reasons.is_empty());
        }
    }

    #[test]
    fn affine_barycentric_column_agrees_on_shared_tin_edge() {
        let state = synthetic_demo_state().unwrap();
        let resolved = resolved(&state);
        // The source surfaces are affine: terrain=100+.1x+.05y,
        // successive bottoms=98.5+.08x+.05y, 95+.1x+.04y,
        // and 88+.1x+.05y. At (5,6), both explicit triangles meet.
        let answer = query_column(&resolved, [5.0, 6.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
        let column = answer.column.unwrap();
        near(column.terrain_abs_m, 100.8);
        assert_eq!(column.triangle_ids.len(), 2);
        assert_eq!(column.layers.len(), 3);
        for (layer, (top, bottom)) in column.layers.iter()
            .zip([(100.8, 99.2), (99.2, 95.74), (95.74, 88.8)])
        {
            near(layer.top_abs_m, top);
            near(layer.bottom_abs_m, bottom);
            assert!(!layer.pinched);
        }
    }

    #[test]
    fn section_samples_each_explicit_facet_crossing_and_projects_wells() {
        let state = synthetic_demo_state().unwrap();
        let answer = query_section(&resolved(&state), &[[0.0, 6.0], [10.0, 6.0]], 4.0).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
        let section = answer.section.unwrap();
        // Spacing alone gives stations 0, 10/3, 20/3, 10. The
        // independent TIN diagonal y=1.2x crosses y=6 at station 5.
        assert!(section.samples.iter().any(|sample| (sample.station_m - 5.0).abs() < 1e-8));
        assert!(section.samples.len() >= 5);
        assert_eq!(section.borehole_projections.len(), 4);
        for projection in &section.borehole_projections {
            near(projection.offset_m, 6.0);
            assert!(projection.chainage_m == 0.0 || projection.chainage_m == 10.0);
            assert!(!projection.source_ids.is_empty());
        }
    }

    #[test]
    fn affine_layer_volumes_are_integrals_over_explicit_cells() {
        let state = synthetic_demo_state().unwrap();
        let answer = layer_volumes(&resolved(&state)).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
        assert_eq!(answer.volumes.len(), 3);
        // The explicit TIN covers a 10x12 rectangle (area 120). Linear
        // thickness means its mean is the value at centroid (5,6):
        // 1.6, 3.46, 6.94 m. No volumetric solver or golden file is used.
        for (volume, expected) in answer.volumes.iter().zip([192.0, 415.2, 832.8]) {
            near(volume.volume_m3, expected);
            assert_eq!(volume.method, InterpolationMethod::ExplicitTinPiecewisePlanar);
        }
    }

    #[test]
    fn outside_coverage_and_explicit_unknown_zone_do_not_extrapolate() {
        let state = synthetic_demo_state().unwrap();
        assert_eq!(query_column(&resolved(&state), [11.0, 6.0]).unwrap().status,
            QueryStatus::OutsideCoverage);
        let mut state = state;
        let source = SourceId::new("SRC-SYNTHETIC-PLANES").unwrap();
        selected_model_mut(&mut state).unknown_zones.push(UnknownZone {
            polygon_xy_m: vec![[4.5, 5.5], [5.5, 5.5], [5.5, 6.5], [4.5, 6.5]],
            reason: "SOURCE_GAP".into(), source_ids: vec![source],
        });
        let resolved = resolved(&state);
        assert_eq!(query_column(&resolved, [5.0, 6.0]).unwrap().status, QueryStatus::Unknown);
        assert_eq!(layer_volumes(&resolved).unwrap().status, QueryStatus::Unknown);
    }

    #[test]
    fn missing_and_conflicting_contact_vertices_keep_typed_coverage() {
        let mut state = synthetic_demo_state().unwrap();
        let well = BoreholeId::new("BH-0-0").unwrap();
        selected_model_mut(&mut state).contacts[0].elevations_abs_m.insert(
            well.clone(), Parameter::Missing { reason: "UNSAMPLED_HORIZON".into() });
        let snapshot = resolved(&state);
        assert_eq!(query_column(&snapshot, [2.0, 2.0]).unwrap().status, QueryStatus::Unknown);
        assert_eq!(layer_volumes(&snapshot).unwrap().status, QueryStatus::Unknown);

        let source = SourceId::new("SRC-SYNTHETIC-PLANES").unwrap();
        let metadata = ParameterMetadata {
            unit: ParameterUnit::Metre,
            value_class: ValueClass::Synthetic,
            ..ParameterMetadata::default()
        };
        selected_model_mut(&mut state).contacts[0].elevations_abs_m.insert(
            well,
            Parameter::Conflicting { candidates: vec![
                ParameterCandidate { value: 98.5, source_ids: vec![source.clone()],
                    metadata: metadata.clone() },
                ParameterCandidate { value: 98.4, source_ids: vec![source], metadata },
            ] },
        );
        let snapshot = resolved(&state);
        assert_eq!(query_column(&snapshot, [2.0, 2.0]).unwrap().status,
            QueryStatus::Conflicting);
        assert_eq!(layer_volumes(&snapshot).unwrap().status, QueryStatus::Conflicting);
    }

    #[test]
    fn crossing_horizon_is_refused_and_explicit_pinch_is_preserved() {
        let mut state = synthetic_demo_state().unwrap();
        let well = BoreholeId::new("BH-0-0").unwrap();
        let first = &mut selected_model_mut(&mut state).contacts[0];
        let Parameter::Known { value, .. } = first.elevations_abs_m.get_mut(&well).unwrap()
            else { panic!("synthetic source must contain known contact"); };
        *value = 101.0; // above its 100 m collar
        assert!(matches!(validate_model_geometry(&state.records, selected_model(&state)),
            Err(GeoError::GeometryConflict { .. })));

        let first = &mut selected_model_mut(&mut state).contacts[0];
        let Parameter::Known { value, .. } = first.elevations_abs_m.get_mut(&well).unwrap()
            else { panic!("synthetic source must contain known contact"); };
        *value = 100.0; // zero thickness exactly at this source vertex
        assert!(matches!(validate_model_geometry(&state.records, selected_model(&state)),
            Err(GeoError::GeometryConflict { .. })));
        selected_model_mut(&mut state).allow_pinch = true;
        let answer = query_column(&resolved(&state), [0.0, 0.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
        let column = answer.column.unwrap();
        assert!(column.layers[0].pinched);
        near(column.layers[0].top_abs_m, column.layers[0].bottom_abs_m);
        assert_eq!(column.layers[0].ige_id, IgeId::new("IGE-FILL").unwrap());
    }

    #[test]
    fn declared_measured_contact_disagreement_has_typed_query_reason() {
        let mut state = synthetic_demo_state().unwrap();
        let well = BoreholeId::new("BH-0-0").unwrap();
        // Test-only conflicting declaration: the logged first-layer bottom is
        // 1.5 m below the 100 m collar, but this declared-measured contact is
        // 1.6 m below it. The original synthetic source is not site evidence.
        let Parameter::Known { value, metadata, .. } = selected_model_mut(&mut state)
            .contacts[0].elevations_abs_m.get_mut(&well).unwrap()
            else { panic!("synthetic source must contain known contact"); };
        *value = 98.4;
        metadata.value_class = ValueClass::Measured;
        let answer = query_column(&resolved(&state), [0.0, 0.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Conflicting);
        assert!(answer.reasons.iter().any(|reason|
            reason.contains("MEASURED_CONTACT_DISAGREES_WITH_BOREHOLE_LOG")));

        state.records.boreholes.get_mut(&well).unwrap().depth_datum =
            "UNSPECIFIED_DATUM".into();
        let answer = query_column(&resolved(&state), [0.0, 0.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Unsupported);
        assert!(answer.reasons.iter().any(|reason|
            reason.contains("UNSUPPORTED_BOREHOLE_DEPTH_DATUM")));

        state.records.boreholes.get_mut(&well).unwrap().depth_datum =
            "BELOW_COLLAR".into();

        // A separately cited interpretation allows a model contact to differ
        // from a logged bottom without recoding the logged interval.
        let source = SourceId::new("SRC-TEST-INTERPRETATION").unwrap();
        let mut interpreted = state.records.sources
            .get(&SourceId::new("SRC-SYNTHETIC-PLANES").unwrap()).unwrap().clone();
        interpreted.id = source.clone();
        interpreted.title = "Test-only interpreted contact".into();
        interpreted.origin = SourceOrigin::Interpreted;
        state.records.sources.insert(source.clone(), interpreted);
        selected_model_mut(&mut state).contacts[0].source_ids.push(source);
        let answer = query_column(&resolved(&state), [0.0, 0.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
    }

    #[test]
    fn absent_same_ige_log_blocks_interpolation_and_global_volume() {
        let mut state = synthetic_demo_state().unwrap();
        let well = BoreholeId::new("BH-0-0").unwrap();
        let Parameter::Known { metadata, .. } = selected_model_mut(&mut state)
            .contacts[0].elevations_abs_m.get_mut(&well).unwrap()
            else { panic!("synthetic source must contain known contact"); };
        metadata.value_class = ValueClass::Measured;
        // This test deliberately removes the only logged FILL interval at this
        // well. It does not manufacture an adjacent replacement observation.
        state.records.boreholes.get_mut(&well).unwrap().intervals[0].ige_id =
            IgeId::new("IGE-LOAM").unwrap();
        let resolved = resolved(&state);
        let column = query_column(&resolved, [2.0, 2.0]).unwrap();
        assert_eq!(column.status, QueryStatus::Unknown);
        assert!(column.column.is_none());
        assert!(column.reasons.iter().any(|reason|
            reason.contains("MEASURED_CONTACT_HAS_NO_MATCHING_IGE_LOG")));
        let volumes = layer_volumes(&resolved).unwrap();
        assert_eq!(volumes.status, QueryStatus::Unknown);
        assert!(volumes.volumes.is_empty());
    }

    #[test]
    fn unresolved_reference_frames_hold_every_numeric_query() {
        let mut state = synthetic_demo_state().unwrap();
        selected_model_mut(&mut state).frame.crs = None;
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);

        let mut state = synthetic_demo_state().unwrap();
        selected_model_mut(&mut state).frame.vertical_datum = None;
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);

        let mut state = synthetic_demo_state().unwrap();
        state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap().crs = None;
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);
        selected_model_mut(&mut state).frame.missing_borehole_crs =
            MissingBoreholeCrsPolicy::InheritModel;
        assert_eq!(query_column(&resolved(&state), [5.0, 6.0]).unwrap().status,
            QueryStatus::Known);

        let mut state = synthetic_demo_state().unwrap();
        state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap().collar_vertical_datum = None;
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);

        let mut state = synthetic_demo_state().unwrap();
        selected_model_mut(&mut state).contacts[0].vertical_datum = None;
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);

        let mut state = synthetic_demo_state().unwrap();
        state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap().depth_datum.clear();
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);

        let mut state = synthetic_demo_state().unwrap();
        let well = state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap();
        well.survey_source_ids.clear();
        well.source_ids.clear();
        assert_all_numeric_queries_hold(&state, QueryStatus::Unknown);
    }

    #[test]
    fn conflicting_explicit_vertical_references_are_unsupported() {
        let mut state = synthetic_demo_state().unwrap();
        state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap().collar_vertical_datum = Some("OTHER_Z_DATUM".into());
        assert_all_numeric_queries_hold(&state, QueryStatus::Unsupported);

        let mut state = synthetic_demo_state().unwrap();
        selected_model_mut(&mut state).contacts[0].vertical_datum = Some("OTHER_Z_DATUM".into());
        assert_all_numeric_queries_hold(&state, QueryStatus::Unsupported);

        let mut state = synthetic_demo_state().unwrap();
        state.records.boreholes.get_mut(&BoreholeId::new("BH-0-0").unwrap())
            .unwrap().crs = Some("OTHER_XY_FRAME".into());
        assert!(matches!(validate_model_geometry(&state.records, selected_model(&state)),
            Err(GeoError::GeometryConflict { .. })));
    }

    #[test]
    fn section_station_budget_applies_before_each_insertion() {
        let mut points = Vec::with_capacity(MAX_SECTION_POINTS);
        for i in 0..MAX_SECTION_POINTS {
            insert_station(&mut points, StationPoint {
                station: i as f64, xy: [i as f64, 0.0],
            }).unwrap();
        }
        assert_eq!(points.len(), MAX_SECTION_POINTS);
        insert_station(&mut points, StationPoint { station: 42.0, xy: [42.0, 0.0] }).unwrap();
        assert_eq!(points.len(), MAX_SECTION_POINTS);
        assert!(matches!(insert_station(&mut points, StationPoint {
            station: MAX_SECTION_POINTS as f64, xy: [MAX_SECTION_POINTS as f64, 0.0],
        }), Err(GeoError::ResourceLimit { .. })));
        assert_eq!(points.len(), MAX_SECTION_POINTS);
    }

    #[test]
    fn large_survey_origin_preserves_local_interpolation() {
        let mut state = synthetic_demo_state().unwrap();
        for borehole in state.records.boreholes.values_mut() {
            borehole.collar_m[0] += 999_000.0;
            borehole.collar_m[1] += 999_000.0;
        }
        let answer = query_column(&resolved(&state), [999_005.0, 999_006.0]).unwrap();
        assert_eq!(answer.status, QueryStatus::Known);
        near(answer.column.unwrap().terrain_abs_m, 100.8);
    }

    #[test]
    fn overlapping_cells_and_self_intersecting_unknown_zone_are_refused() {
        let mut state = synthetic_demo_state().unwrap();
        let ids = selected_model(&state).borehole_ids.clone();
        selected_model_mut(&mut state).triangles[1].vertices =
            [ids[0].clone(), ids[1].clone(), ids[3].clone()];
        assert!(matches!(validate_model_geometry(&state.records, selected_model(&state)),
            Err(GeoError::GeometryConflict { .. })));

        let mut state = synthetic_demo_state().unwrap();
        selected_model_mut(&mut state).unknown_zones.push(UnknownZone {
            polygon_xy_m: vec![[0.0, 0.0], [4.0, 4.0], [0.0, 4.0],
                               [4.0, 0.0], [5.0, -1.0]],
            reason: "CROSSING_SOURCE_BOUNDARY".into(), source_ids: vec![],
        });
        let error = validate_model_geometry(&state.records, selected_model(&state)).unwrap_err();
        assert!(matches!(&error, GeoError::GeometryConflict { .. }));
        assert!(error.to_string().contains("self-intersecting"));
    }

    #[test]
    fn query_budgets_and_forged_snapshot_binding_fail_closed() {
        let state = synthetic_demo_state().unwrap();
        let resolved = resolved(&state);
        assert!(matches!(query_section(&resolved, &[[0.0, 6.0], [10.0, 6.0]], 1e-6),
            Err(GeoError::ResourceLimit { .. })));
        assert!(matches!(query_column(&resolved, [MAX_ABS_COORDINATE_M + 1.0, 6.0]),
            Err(GeoError::InvalidInput { .. })));

        let mut forged = resolved;
        let well = BoreholeId::new("BH-0-0").unwrap();
        forged.snapshot.records.boreholes.get_mut(&well).unwrap().collar_m[2] += 0.25;
        assert!(matches!(query_column(&forged, [5.0, 6.0]),
            Err(GeoError::SnapshotBindingMismatch { .. })));

        let mut oversized = synthetic_demo_state().unwrap();
        let triangle = selected_model(&oversized).triangles[0].clone();
        selected_model_mut(&mut oversized).triangles.resize(MAX_TRIANGLES + 1, triangle);
        assert!(matches!(validate_model_geometry(&oversized.records, selected_model(&oversized)),
            Err(GeoError::ResourceLimit { .. })));
    }

    #[test]
    fn bounded_column_batch_exactly_matches_individual_queries() {
        let state = synthetic_demo_state().unwrap();
        let snapshot = resolved(&state);
        let points = [[2.0, 2.0], [5.0, 6.0], [8.0, 4.0], [11.0, 6.0]];
        let batch = query_columns(&snapshot, &points).unwrap();
        assert_eq!(batch.len(), points.len());
        assert_eq!(batch[0].status, QueryStatus::Known);
        assert_eq!(batch[1].column.as_ref().unwrap().triangle_ids.len(), 2);
        assert_eq!(batch[3].status, QueryStatus::OutsideCoverage);
        for (point, answer) in points.into_iter().zip(batch) {
            assert_eq!(answer, query_column(&snapshot, point).unwrap());
        }

        let mut unknown = state;
        selected_model_mut(&mut unknown).frame.crs = None;
        let unknown = resolved(&unknown);
        let batch = query_columns(&unknown, &[[5.0, 6.0], [11.0, 6.0]]).unwrap();
        for (point, answer) in [[5.0, 6.0], [11.0, 6.0]].into_iter().zip(batch) {
            assert_eq!(answer, query_column(&unknown, point).unwrap());
        }

        let mut disputed = synthetic_demo_state().unwrap();
        let well = BoreholeId::new("BH-0-0").unwrap();
        let Parameter::Known { value, metadata, .. } = selected_model_mut(&mut disputed)
            .contacts[0].elevations_abs_m.get_mut(&well).unwrap()
            else { panic!("synthetic source must contain known contact"); };
        *value = 98.4;
        metadata.value_class = ValueClass::Measured;
        let disputed = resolved(&disputed);
        let points = [[0.0, 0.0], [2.0, 2.0]];
        let batch = query_columns(&disputed, &points).unwrap();
        for (point, answer) in points.into_iter().zip(batch) {
            assert_eq!(answer, query_column(&disputed, point).unwrap());
            assert!(answer.reasons.iter().any(|reason|
                reason.contains("MEASURED_CONTACT_DISAGREES_WITH_BOREHOLE_LOG")));
        }
    }

    #[test]
    fn column_batch_checks_count_coordinates_and_binding_before_output() {
        let state = synthetic_demo_state().unwrap();
        let snapshot = resolved(&state);
        let oversized = vec![[5.0, 6.0]; MAX_RECORDS + 1];
        assert!(matches!(query_columns(&snapshot, &oversized),
            Err(GeoError::ResourceLimit { .. })));
        assert!(matches!(query_columns(&snapshot, &[[f64::NAN, 6.0]]),
            Err(GeoError::InvalidInput { .. })));

        let mut forged = snapshot;
        let well = BoreholeId::new("BH-0-0").unwrap();
        forged.snapshot.records.boreholes.get_mut(&well).unwrap().collar_m[2] += 0.25;
        assert!(matches!(query_columns(&forged, &[[5.0, 6.0]]),
            Err(GeoError::SnapshotBindingMismatch { .. })));
        assert!(matches!(query_columns(&forged, &[]),
            Err(GeoError::SnapshotBindingMismatch { .. })));
    }
}

