//! Deterministic plane/triangle-mesh sectioning.
//!
//! The result is a polyline approximation of the supplied display mesh. It
//! does not recover exact BRep edges or replace the source model geometry.

use ai_revit_core::ElementId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use viewer_core::{Mesh, Vec3};

/// A plane described by one point and a normal direction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionPlane {
    pub origin: Vec3,
    pub normal: Vec3,
}

/// One open or closed section contour for a semantic model element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionPolyline {
    pub element_id: ElementId,
    pub points: Vec<Vec3>,
    pub closed: bool,
}

/// A section result for one or more semantic elements.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometrySection {
    pub plane: SectionPlane,
    pub element_ids: Vec<ElementId>,
    pub loops: Vec<SectionPolyline>,
    pub representation_quality: crate::RepresentationQuality,
}

/// Validates the plane and returns it with a finite unit normal.
pub(crate) fn validate_plane(
    mut plane: SectionPlane,
) -> Result<SectionPlane, crate::GeometryObservationError> {
    if !plane.origin.is_finite() || !plane.normal.is_finite() {
        return Err(crate::GeometryObservationError::new(
            "invalid_section_plane",
            None,
            "section plane origin and normal must contain finite coordinates",
        ));
    }

    // Scaling before taking the length avoids overflow for very large normals
    // and underflow for very small, but still nonzero, normals.
    let scale = plane
        .normal
        .x
        .abs()
        .max(plane.normal.y.abs())
        .max(plane.normal.z.abs());
    if scale == 0.0 {
        return Err(crate::GeometryObservationError::new(
            "invalid_section_plane",
            None,
            "section plane normal must be nonzero",
        ));
    }
    let scaled = plane.normal / scale;
    let length = (scaled.x * scaled.x + scaled.y * scaled.y + scaled.z * scaled.z).sqrt();
    if !length.is_finite() || length == 0.0 {
        return Err(crate::GeometryObservationError::new(
            "invalid_section_plane",
            None,
            "section plane normal could not be normalized",
        ));
    }
    plane.normal = scaled / length;
    Ok(plane)
}

/// Intersects a mesh with a plane and returns deterministic stitched contours.
///
/// `tolerance` is a linear distance in the mesh coordinate system. Invalid
/// plane or tolerance input produces no contours; callers should validate the
/// plane first when they need a structured error.
pub(crate) fn mesh_section(
    element_id: ElementId,
    mesh: &Mesh,
    plane: SectionPlane,
    tolerance: f64,
) -> Vec<SectionPolyline> {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Vec::new();
    }
    let Ok(plane) = validate_plane(plane) else {
        return Vec::new();
    };

    let mut segments = Vec::new();
    let mut coplanar_edges = Vec::new();
    for triangle in mesh.triangles() {
        let [a, b, c] = triangle.vertices;
        let vertices = [a, b, c];
        if vertices.iter().any(|point| !point.is_finite()) || is_degenerate(vertices, tolerance) {
            continue;
        }
        let Some(surface_normal) = triangle_normal(vertices) else {
            continue;
        };

        let mut distances = [0.0; 3];
        let mut sides = [0_i8; 3];
        let mut valid = true;
        for index in 0..3 {
            let relative = vertices[index] - plane.origin;
            let distance = relative.dot(plane.normal);
            if !relative.is_finite() || !distance.is_finite() {
                valid = false;
                break;
            }
            distances[index] = distance;
            sides[index] = if distance > tolerance {
                1
            } else if distance < -tolerance {
                -1
            } else {
                0
            };
        }
        if !valid {
            continue;
        }

        if sides.iter().all(|side| *side == 0) {
            // Coplanar triangles describe a filled 2D patch. Counting their
            // directed edges removes shared interior edges and triangulation
            // diagonals after the T-junctions are split below.
            let projected = vertices.map(|point| project_to_plane(point, plane));
            if let [Some(a), Some(b), Some(c)] = projected {
                if surface_normal.dot(plane.normal) >= 0.0 {
                    coplanar_edges.extend([(a, b), (b, c), (c, a)]);
                } else {
                    coplanar_edges.extend([(b, a), (c, b), (a, c)]);
                }
            }
            continue;
        }

        let mut intersections = Vec::with_capacity(3);
        for index in 0..3 {
            let next = (index + 1) % 3;
            if sides[index] == 0 {
                if let Some(point) = project_to_plane(vertices[index], plane) {
                    push_unique_point(&mut intersections, point, tolerance);
                }
            }
            if sides[index] * sides[next] == -1 {
                let denominator = distances[index] - distances[next];
                if denominator == 0.0 || !denominator.is_finite() {
                    continue;
                }
                let amount = distances[index] / denominator;
                if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                    continue;
                }
                let point = lerp(vertices[index], vertices[next], amount);
                if let Some(point) = project_to_plane(point, plane) {
                    push_unique_point(&mut intersections, point, tolerance);
                }
            }
        }

        if intersections.len() >= 2 {
            intersections.sort_by(point_cmp);
            let first = intersections[0];
            let last = *intersections.last().expect("at least two intersections");
            if !points_close(first, last, tolerance) {
                segments.push(orient_section_edge(
                    first,
                    last,
                    surface_normal,
                    plane.normal,
                ));
            }
        }
    }

    stitch_segments(element_id, segments, coplanar_edges, tolerance)
}

fn is_degenerate(vertices: [Vec3; 3], tolerance: f64) -> bool {
    let ab = vertices[1] - vertices[0];
    let ac = vertices[2] - vertices[0];
    if !ab.is_finite() || !ac.is_finite() {
        return true;
    }
    let scale = max_abs(ab).max(max_abs(ac));
    if scale == 0.0 {
        return true;
    }
    let ab_scaled = ab / scale;
    let ac_scaled = ac / scale;
    let cross = ab_scaled.cross(ac_scaled);
    let cross_length = (cross.x * cross.x + cross.y * cross.y + cross.z * cross.z).sqrt();
    let longest_edge_scaled = ab_scaled
        .length()
        .max(ac_scaled.length())
        .max((ac_scaled - ab_scaled).length());
    if !cross_length.is_finite() || !longest_edge_scaled.is_finite() || longest_edge_scaled == 0.0 {
        return true;
    }
    if tolerance == 0.0 {
        cross_length == 0.0
    } else {
        // The ratio is the triangle altitude divided by its longest edge;
        // multiplying by scale gives the altitude in model units.
        cross_length / longest_edge_scaled <= tolerance / scale
    }
}

fn triangle_normal(vertices: [Vec3; 3]) -> Option<Vec3> {
    let ab = vertices[1] - vertices[0];
    let ac = vertices[2] - vertices[0];
    if !ab.is_finite() || !ac.is_finite() {
        return None;
    }
    let scale = max_abs(ab).max(max_abs(ac));
    if scale == 0.0 {
        return None;
    }
    let normal = (ab / scale).cross(ac / scale);
    (normal.is_finite() && normal.length_squared() > 0.0).then_some(normal)
}

fn orient_section_edge(a: Vec3, b: Vec3, surface_normal: Vec3, plane_normal: Vec3) -> (Vec3, Vec3) {
    // For consistently wound boundary triangles, n_plane x n_surface gives
    // the section-edge direction compatible with the coplanar patch boundary.
    let section_direction = plane_normal.cross(surface_normal);
    let edge_direction = b - a;
    let section_scale = max_abs(section_direction);
    let edge_scale = max_abs(edge_direction);
    if section_scale == 0.0 || edge_scale == 0.0 {
        return if point_cmp(&a, &b).is_le() {
            (a, b)
        } else {
            (b, a)
        };
    }
    let alignment = (section_direction / section_scale).dot(edge_direction / edge_scale);
    if alignment > 0.0 || (alignment == 0.0 && point_cmp(&a, &b).is_le()) {
        (a, b)
    } else {
        (b, a)
    }
}

fn max_abs(vector: Vec3) -> f64 {
    vector.x.abs().max(vector.y.abs()).max(vector.z.abs())
}

fn project_to_plane(point: Vec3, plane: SectionPlane) -> Option<Vec3> {
    let distance = (point - plane.origin).dot(plane.normal);
    if !distance.is_finite() {
        return None;
    }
    let projected = point - plane.normal * distance;
    projected.is_finite().then_some(projected)
}

fn lerp(a: Vec3, b: Vec3, amount: f64) -> Vec3 {
    a * (1.0 - amount) + b * amount
}

fn push_unique_point(points: &mut Vec<Vec3>, point: Vec3, tolerance: f64) {
    if !points
        .iter()
        .any(|existing| points_close(*existing, point, tolerance))
    {
        points.push(point);
    }
}

fn points_close(a: Vec3, b: Vec3, tolerance: f64) -> bool {
    if tolerance == 0.0 {
        return a == b;
    }
    (a.x - b.x).hypot(a.y - b.y).hypot(a.z - b.z) <= tolerance
}

fn point_cmp(a: &Vec3, b: &Vec3) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x)
        .then_with(|| a.y.total_cmp(&b.y))
        .then_with(|| a.z.total_cmp(&b.z))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cell(i128, i128, i128);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PointBits(u64, u64, u64);

impl PointBits {
    fn new(point: Vec3) -> Self {
        fn bits(value: f64) -> u64 {
            if value == 0.0 { 0 } else { value.to_bits() }
        }
        Self(bits(point.x), bits(point.y), bits(point.z))
    }
}

fn cell(point: Vec3, tolerance: f64) -> Cell {
    if tolerance == 0.0 {
        let bits = PointBits::new(point);
        return Cell(bits.0 as i128, bits.1 as i128, bits.2 as i128);
    }
    Cell(
        (point.x / tolerance).floor() as i128,
        (point.y / tolerance).floor() as i128,
        (point.z / tolerance).floor() as i128,
    )
}

fn neighboring_cells(cell: Cell, exact: bool) -> Vec<Cell> {
    if exact {
        return vec![cell];
    }
    let mut cells = Vec::with_capacity(27);
    for dx in -1_i128..=1 {
        for dy in -1_i128..=1 {
            for dz in -1_i128..=1 {
                let (Some(x), Some(y), Some(z)) = (
                    cell.0.checked_add(dx),
                    cell.1.checked_add(dy),
                    cell.2.checked_add(dz),
                ) else {
                    continue;
                };
                cells.push(Cell(x, y, z));
            }
        }
    }
    cells
}

fn stitch_segments(
    element_id: ElementId,
    segments: Vec<(Vec3, Vec3)>,
    coplanar_edges: Vec<(Vec3, Vec3)>,
    tolerance: f64,
) -> Vec<SectionPolyline> {
    if segments.is_empty() && coplanar_edges.is_empty() {
        return Vec::new();
    }

    let mut all_points: Vec<Vec3> = segments
        .iter()
        .chain(coplanar_edges.iter())
        .flat_map(|(a, b)| [*a, *b])
        .collect();
    all_points.sort_by(point_cmp);

    let mut nodes = Vec::<Vec3>::new();
    let mut buckets = BTreeMap::<Cell, Vec<usize>>::new();
    let mut point_nodes = BTreeMap::<PointBits, usize>::new();
    for point in all_points {
        let point_cell = cell(point, tolerance);
        let mut selected = None;
        for nearby in neighboring_cells(point_cell, tolerance == 0.0) {
            if let Some(candidates) = buckets.get(&nearby) {
                for &candidate in candidates {
                    if points_close(nodes[candidate], point, tolerance)
                        && selected.is_none_or(|current| candidate < current)
                    {
                        selected = Some(candidate);
                    }
                }
            }
        }
        let node = selected.unwrap_or_else(|| {
            let node = nodes.len();
            nodes.push(point);
            buckets.entry(point_cell).or_default().push(node);
            node
        });
        point_nodes.insert(PointBits::new(point), node);
    }

    let node_for = |point: Vec3| {
        *point_nodes
            .get(&PointBits::new(point))
            .expect("all segment endpoints were assigned a node")
    };
    let axis_order = sorted_nodes_by_axis(&nodes);
    let mut intersection_votes = BTreeMap::<(usize, usize), (usize, usize)>::new();
    for (a, b) in segments {
        let (a, b) = (node_for(a), node_for(b));
        for (start, end) in split_node_edge(a, b, &nodes, &axis_order, tolerance) {
            if start != end {
                let votes = intersection_votes
                    .entry(canonical_edge(start, end))
                    .or_default();
                if start < end {
                    votes.0 += 1;
                } else {
                    votes.1 += 1;
                }
            }
        }
    }

    let mut coplanar_balance = BTreeMap::<(usize, usize), i64>::new();
    for (a, b) in coplanar_edges {
        let (a, b) = (node_for(a), node_for(b));
        for (start, end) in split_node_edge(a, b, &nodes, &axis_order, tolerance) {
            if start != end {
                let balance = coplanar_balance
                    .entry(canonical_edge(start, end))
                    .or_default();
                *balance += if start < end { 1 } else { -1 };
            }
        }
    }

    // Noncoplanar triangle intersections can duplicate the same section edge
    // (for example, an edge shared by two faces). Reduce those to one stable
    // direction first, then combine them with oriented coplanar patch edges.
    // Opposing contributions cancel interior seams; nonzero outer boundaries
    // remain even when coincident triangles contribute more than once.
    let mut all_edges = BTreeSet::new();
    all_edges.extend(intersection_votes.keys().copied());
    all_edges.extend(coplanar_balance.keys().copied());
    let mut graph_edges = BTreeSet::<(usize, usize)>::new();
    for edge in all_edges {
        let intersection_sign =
            intersection_votes
                .get(&edge)
                .map_or(0_i64, |(forward, reverse)| {
                    match forward.cmp(reverse) {
                        std::cmp::Ordering::Greater => 1,
                        std::cmp::Ordering::Less => -1,
                        // Exact oppositely wound duplicates still represent one
                        // geometric section segment; choose a stable canonical sign.
                        std::cmp::Ordering::Equal => 1,
                    }
                });
        let coplanar_sign = coplanar_balance
            .get(&edge)
            .copied()
            .unwrap_or_default()
            .signum();
        let balance = intersection_sign + coplanar_sign;
        if balance != 0 {
            graph_edges.insert(edge);
        }
    }

    let mut adjacency = vec![BTreeSet::<usize>::new(); nodes.len()];
    for &(a, b) in &graph_edges {
        adjacency[a].insert(b);
        adjacency[b].insert(a);
    }

    let mut unused = graph_edges;
    let mut paths = Vec::<(Vec<usize>, bool)>::new();

    // Open chains and branches have an unambiguous set of starts: every
    // endpoint or junction. Walking each unused edge separately is stable.
    for start in 0..adjacency.len() {
        if adjacency[start].len() == 2 {
            continue;
        }
        for &next in &adjacency[start] {
            let edge = canonical_edge(start, next);
            if unused.remove(&edge) {
                paths.push(trace_path(start, next, &adjacency, &mut unused));
            }
        }
    }

    // Remaining components are closed cycles. Node IDs follow lexicographic
    // point order, so the first unused edge also fixes each cycle's direction.
    for start in 0..adjacency.len() {
        for &next in &adjacency[start] {
            let edge = canonical_edge(start, next);
            if unused.remove(&edge) {
                paths.push(trace_path(start, next, &adjacency, &mut unused));
            }
        }
    }

    let mut polylines = Vec::new();
    for (node_path, closed) in paths {
        let node_path = simplify_path(node_path, closed, &nodes, &adjacency, tolerance);
        if node_path.len() < 2 || (closed && node_path.len() < 3) {
            continue;
        }
        polylines.push(SectionPolyline {
            element_id,
            points: node_path.into_iter().map(|node| nodes[node]).collect(),
            closed,
        });
    }
    polylines.sort_by(|a, b| {
        a.closed
            .cmp(&b.closed)
            .then_with(|| compare_point_slices(&a.points, &b.points))
    });
    polylines
}

fn canonical_edge(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

fn sorted_nodes_by_axis(nodes: &[Vec3]) -> [Vec<usize>; 3] {
    let mut axes = [Vec::new(), Vec::new(), Vec::new()];
    for axis in 0..3 {
        axes[axis] = (0..nodes.len()).collect();
        axes[axis].sort_by(|&a, &b| {
            coordinate(nodes[a], axis)
                .total_cmp(&coordinate(nodes[b], axis))
                .then_with(|| a.cmp(&b))
        });
    }
    axes
}

fn coordinate(point: Vec3, axis: usize) -> f64 {
    match axis {
        0 => point.x,
        1 => point.y,
        _ => point.z,
    }
}

fn split_node_edge(
    start: usize,
    end: usize,
    nodes: &[Vec3],
    axis_order: &[Vec<usize>; 3],
    tolerance: f64,
) -> Vec<(usize, usize)> {
    if start == end {
        return Vec::new();
    }
    let a = nodes[start];
    let b = nodes[end];
    let delta = b - a;
    if !delta.is_finite() {
        return vec![(start, end)];
    }
    let mut candidate_range = 0..nodes.len();
    let mut selected_axis = 0;
    for axis in 0..3 {
        let a_axis = coordinate(a, axis);
        let b_axis = coordinate(b, axis);
        let lower = a_axis.min(b_axis) - tolerance;
        let upper = a_axis.max(b_axis) + tolerance;
        let sorted = &axis_order[axis];
        let first =
            sorted.partition_point(|&node| coordinate(nodes[node], axis).total_cmp(&lower).is_lt());
        let last = sorted
            .partition_point(|&node| !coordinate(nodes[node], axis).total_cmp(&upper).is_gt());
        if last - first < candidate_range.len() {
            candidate_range = first..last;
            selected_axis = axis;
        }
    }
    let mut splits = vec![(0.0, start), (1.0, end)];
    for &node in &axis_order[selected_axis][candidate_range] {
        if node == start || node == end {
            continue;
        }
        if let Some(amount) = point_parameter_on_segment(nodes[node], a, b, tolerance) {
            splits.push((amount, node));
        }
    }
    splits.sort_by(|(left_t, left_node), (right_t, right_node)| {
        left_t
            .total_cmp(right_t)
            .then_with(|| left_node.cmp(right_node))
    });
    splits.dedup_by_key(|(_, node)| *node);
    splits
        .windows(2)
        .filter_map(|pair| (pair[0].1 != pair[1].1).then_some((pair[0].1, pair[1].1)))
        .collect()
}

fn point_parameter_on_segment(point: Vec3, a: Vec3, b: Vec3, tolerance: f64) -> Option<f64> {
    let delta = b - a;
    let offset = point - a;
    if !delta.is_finite() || !offset.is_finite() {
        return None;
    }
    let axis = if delta.x.abs() >= delta.y.abs() && delta.x.abs() >= delta.z.abs() {
        0
    } else if delta.y.abs() >= delta.z.abs() {
        1
    } else {
        2
    };
    let denominator = coordinate(delta, axis);
    if denominator == 0.0 {
        return None;
    }
    let amount = coordinate(offset, axis) / denominator;
    if !amount.is_finite() || amount <= 0.0 || amount >= 1.0 {
        return None;
    }
    points_close(lerp(a, b, amount), point, tolerance).then_some(amount)
}

fn trace_path(
    start: usize,
    next: usize,
    adjacency: &[BTreeSet<usize>],
    unused: &mut BTreeSet<(usize, usize)>,
) -> (Vec<usize>, bool) {
    let mut path = vec![start, next];
    let mut previous = start;
    let mut current = next;
    loop {
        if current == start {
            path.pop();
            return (path, true);
        }
        if adjacency[current].len() != 2 {
            return (path, false);
        }
        let following = adjacency[current]
            .iter()
            .copied()
            .filter(|candidate| *candidate != previous)
            .find(|candidate| unused.contains(&canonical_edge(current, *candidate)));
        let Some(following) = following else {
            return (path, false);
        };
        unused.remove(&canonical_edge(current, following));
        if following == start {
            return (path, true);
        }
        path.push(following);
        previous = current;
        current = following;
    }
}

fn simplify_path(
    mut path: Vec<usize>,
    closed: bool,
    nodes: &[Vec3],
    adjacency: &[BTreeSet<usize>],
    tolerance: f64,
) -> Vec<usize> {
    loop {
        let minimum = if closed { 3 } else { 2 };
        if path.len() <= minimum {
            return path;
        }
        let range = if closed {
            0..path.len()
        } else {
            1..path.len() - 1
        };
        let removable = range.into_iter().find(|&index| {
            if adjacency[path[index]].len() != 2 {
                return false;
            }
            let previous = path[(index + path.len() - 1) % path.len()];
            let next = path[(index + 1) % path.len()];
            collinear_between(nodes[previous], nodes[path[index]], nodes[next], tolerance)
        });
        let Some(index) = removable else {
            return path;
        };
        path.remove(index);
    }
}

fn collinear_between(a: Vec3, b: Vec3, c: Vec3, tolerance: f64) -> bool {
    let ab = b - a;
    let ac = c - a;
    if !ab.is_finite() || !ac.is_finite() {
        return false;
    }
    let scale = max_abs(ab).max(max_abs(ac));
    if scale == 0.0 {
        return true;
    }
    let ab = ab / scale;
    let ac = ac / scale;
    let length_squared = ac.length_squared();
    if !length_squared.is_finite() || length_squared == 0.0 {
        return false;
    }
    let amount = ab.dot(ac) / length_squared;
    if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
        return false;
    }
    let offset = ab - ac * amount;
    let distance_scaled = offset.length();
    if tolerance == 0.0 {
        distance_scaled == 0.0
    } else {
        distance_scaled <= tolerance / scale
    }
}

fn compare_point_slices(a: &[Vec3], b: &[Vec3]) -> std::cmp::Ordering {
    for (left, right) in a.iter().zip(b) {
        let ordering = point_cmp(left, right);
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GeometryBackend;
    use ai_revit_core::{
        ClassId, ElementKind, ElementRecord, FloorSpec, OperationId, Point2, RectangularSpec,
        WallSpec,
    };
    use viewer_core::Triangle;

    fn plane(origin: Vec3, normal: Vec3) -> SectionPlane {
        SectionPlane { origin, normal }
    }

    fn element_record(id: u64, class_id: &str, kind: ElementKind) -> ElementRecord {
        ElementRecord {
            id: ElementId::new(id),
            name: class_id.to_owned(),
            class_id: ClassId::from(class_id),
            kind,
            properties: Default::default(),
            created_by: OperationId::default(),
            type_id: None,
        }
    }

    #[test]
    fn validation_normalizes_and_rejects_invalid_planes() {
        let normalized = validate_plane(plane(Vec3::ZERO, Vec3::new(0.0, 0.0, 8.0))).unwrap();
        assert_eq!(normalized.normal, Vec3::Z);
        assert!(validate_plane(plane(Vec3::ZERO, Vec3::ZERO)).is_err());
        assert!(validate_plane(plane(Vec3::new(f64::NAN, 0.0, 0.0), Vec3::Z)).is_err());
    }

    #[test]
    fn cube_cut_stitches_a_closed_four_corner_contour() {
        let mesh = Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0));
        let loops = mesh_section(
            ElementId::new(42),
            &mesh,
            plane(Vec3::ZERO, Vec3::X),
            1.0e-8,
        );

        assert_eq!(loops.len(), 1);
        assert!(loops[0].closed);
        assert_eq!(loops[0].points.len(), 4);
        assert!(loops[0].points.iter().all(|point| point.x.abs() <= 1.0e-8));
    }

    #[test]
    fn coplanar_quad_omits_its_internal_diagonal() {
        let a = Vec3::new(-1.0, -1.0, 0.0);
        let b = Vec3::new(1.0, -1.0, 0.0);
        let c = Vec3::new(1.0, 1.0, 0.0);
        let d = Vec3::new(-1.0, 1.0, 0.0);
        let mesh = Mesh::new(vec![Triangle::new(1, a, b, c), Triangle::new(2, a, c, d)]);

        let loops = mesh_section(ElementId::new(7), &mesh, plane(Vec3::ZERO, Vec3::Z), 1.0e-8);
        assert_eq!(loops.len(), 1);
        assert!(loops[0].closed);
        assert_eq!(loops[0].points.len(), 4);
        assert!(
            loops[0]
                .points
                .iter()
                .all(|point| point.x.abs() == 1.0 && point.y.abs() == 1.0)
        );
    }

    #[test]
    fn duplicate_crossings_are_deduplicated_and_degenerate_triangles_are_ignored() {
        let a = Vec3::new(0.0, 0.0, -1.0);
        let b = Vec3::new(1.0, 0.0, 1.0);
        let c = Vec3::new(0.0, 1.0, 1.0);
        let degenerate = Vec3::new(4.0, 4.0, 0.0);
        let mesh = Mesh::new(vec![
            Triangle::new(1, a, b, c),
            Triangle::new(2, a, c, b),
            Triangle::new(3, degenerate, degenerate, degenerate),
        ]);

        let loops = mesh_section(ElementId::new(8), &mesh, plane(Vec3::ZERO, Vec3::Z), 1.0e-8);
        assert_eq!(loops.len(), 1);
        assert!(!loops[0].closed);
        assert_eq!(loops[0].points.len(), 2);
        assert!(loops[0].points.iter().all(|point| point.z.abs() <= 1.0e-8));
    }

    #[test]
    fn floor_top_with_hole_returns_only_outer_and_inner_boundaries() {
        let floor = element_record(
            101,
            "core.floor",
            ElementKind::Floor(FloorSpec {
                boundary: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(6.0, 0.0),
                    Point2::new(6.0, 4.0),
                    Point2::new(0.0, 4.0),
                ],
                holes: vec![vec![
                    Point2::new(2.0, 1.0),
                    Point2::new(4.0, 1.0),
                    Point2::new(4.0, 3.0),
                    Point2::new(2.0, 3.0),
                ]],
                elevation_m: 0.0,
                thickness_m: 0.2,
            }),
        );
        let mesh = crate::mesh_for_element(&floor).unwrap().mesh;
        let loops = mesh_section(floor.id, &mesh, plane(Vec3::ZERO, Vec3::Z), 1.0e-8);

        assert_eq!(
            loops.len(),
            2,
            "top surface should have one outer and one hole boundary"
        );
        assert!(loops.iter().all(|section| section.closed));
        assert!(loops.iter().all(|section| section.points.len() == 4));
        let mut bounds: Vec<_> = loops
            .iter()
            .map(|section| {
                let min_x = section
                    .points
                    .iter()
                    .map(|point| point.x)
                    .fold(f64::INFINITY, f64::min);
                let min_y = section
                    .points
                    .iter()
                    .map(|point| point.y)
                    .fold(f64::INFINITY, f64::min);
                let max_x = section
                    .points
                    .iter()
                    .map(|point| point.x)
                    .fold(f64::NEG_INFINITY, f64::max);
                let max_y = section
                    .points
                    .iter()
                    .map(|point| point.y)
                    .fold(f64::NEG_INFINITY, f64::max);
                (min_x, min_y, max_x, max_y)
            })
            .collect();
        bounds.sort_by(|left, right| left.0.total_cmp(&right.0));
        assert_eq!(bounds, vec![(0.0, 0.0, 6.0, 4.0), (2.0, 1.0, 4.0, 3.0)]);
        let reversed = mesh_section(floor.id, &mesh, plane(Vec3::ZERO, -Vec3::Z), 1.0e-8);
        assert_eq!(loops, reversed);
    }

    #[test]
    fn section_through_wall_opening_reveal_has_no_internal_head_or_sill_seams() {
        let wall = element_record(
            201,
            "core.wall",
            ElementKind::Wall(WallSpec {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(6.0, 0.0),
                base_elevation_m: 0.0,
                height_m: 3.0,
                thickness_m: 0.2,
            }),
        );
        let opening = element_record(
            202,
            "core.opening",
            ElementKind::Opening(RectangularSpec {
                center: Point2::new(2.0, 0.0),
                elevation_m: 0.8,
                width_m: 1.2,
                height_m: 1.5,
                depth_m: 0.2,
                rotation_rad: 0.0,
            }),
        );
        let geometry = crate::MeshGeometryBackend
            .derive_element(
                &crate::GeometryInput {
                    element: wall.clone(),
                    openings: vec![opening.clone()],
                    derived_from: vec![wall.id, opening.id],
                },
                crate::GeometryOptions::default(),
            )
            .unwrap();
        let loops = mesh_section(
            wall.id,
            &geometry.mesh,
            plane(Vec3::new(1.4, 0.0, 0.0), Vec3::X),
            1.0e-8,
        );

        assert_eq!(
            loops.len(),
            1,
            "the reveal and neighboring sections form one boundary"
        );
        assert!(loops[0].closed);
        assert_eq!(loops[0].points.len(), 4);
        assert!(
            loops[0]
                .points
                .iter()
                .all(|point| (point.x - 1.4).abs() <= 1.0e-8
                    && (point.y.abs() - 0.1).abs() <= 1.0e-8
                    && (point.z.abs() <= 1.0e-8 || (point.z - 3.0).abs() <= 1.0e-8))
        );
        let reversed = mesh_section(
            wall.id,
            &geometry.mesh,
            plane(Vec3::new(1.4, 0.0, 0.0), -Vec3::X),
            1.0e-8,
        );
        assert_eq!(loops, reversed);
    }
}
