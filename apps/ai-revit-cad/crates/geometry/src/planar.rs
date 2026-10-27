//! Bounded polygon difference in a host-local affine plane.
//!
//! All contour intersections become sweep events.  The cells are display-mesh
//! trapezoids; exposed boundary edges are derived independently from material
//! transitions, so cell T junctions cannot create spurious side faces.

use ai_revit_core::Point2;
use std::collections::HashMap;

const MAX_EDGES: usize = 2_048;
const MAX_X_EVENTS: usize = 8_192;
const MAX_CELLS: usize = 100_000;

#[derive(Clone, Debug)]
pub(crate) struct PlanarRegion {
    pub cells: Vec<[Point2; 4]>,
    pub boundary_edges: Vec<[Point2; 2]>,
    pub area: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlanarError {
    pub code: &'static str,
    pub message: &'static str,
    /// Index in the `voids` argument, when one input opening caused the error.
    pub void_index: Option<usize>,
}

impl PlanarError {
    fn new(code: &'static str, message: &'static str, void_index: Option<usize>) -> Self {
        Self {
            code,
            message,
            void_index,
        }
    }
}

#[derive(Clone, Copy)]
enum RingKind {
    Outer,
    Native,
    Void,
}

#[derive(Clone, Copy)]
struct Segment {
    a: Point2,
    b: Point2,
    ring: usize,
}

impl Segment {
    fn y_at(self, x: f64) -> f64 {
        self.a.y + (self.b.y - self.a.y) * ((x - self.a.x) / (self.b.x - self.a.x))
    }
}

#[derive(Clone, Copy)]
struct Crossing {
    y: f64,
    segment: usize,
}

#[derive(Default)]
struct StripSides {
    left: Vec<(f64, f64)>,
    right: Vec<(f64, f64)>,
}

#[derive(Clone, Copy)]
struct RawEdge {
    from_x: usize,
    to_x: usize,
    from_y: f64,
    to_y: f64,
}

#[derive(Clone, Copy)]
struct EndpointRef {
    y: f64,
    edge: usize,
    is_start: bool,
}

type Node = (usize, usize);

fn cross(a: Point2, b: Point2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn point_order(a: Point2, b: Point2) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
}

fn sub(a: Point2, b: Point2) -> Point2 {
    Point2::new(a.x - b.x, a.y - b.y)
}

fn lerp(a: Point2, b: Point2, t: f64) -> Point2 {
    Point2::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y))
}

fn length(a: Point2, b: Point2) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn signed_area(ring: &[Point2]) -> f64 {
    let mut sum = 0.0;
    for i in 0..ring.len() {
        sum += cross(ring[i], ring[(i + 1) % ring.len()]);
    }
    0.5 * sum
}

fn orient(a: Point2, b: Point2, c: Point2) -> f64 {
    cross(sub(b, a), sub(c, a))
}

fn on_segment(p: Point2, a: Point2, b: Point2, eps: f64) -> bool {
    let len = length(a, b);
    len > 0.0
        && orient(a, b, p).abs() <= eps * len
        && p.x >= a.x.min(b.x) - eps
        && p.x <= a.x.max(b.x) + eps
        && p.y >= a.y.min(b.y) - eps
        && p.y <= a.y.max(b.y) + eps
}

fn segments_meet(a: Point2, b: Point2, c: Point2, d: Point2, eps: f64) -> bool {
    if a.x.max(b.x) + eps < c.x.min(d.x)
        || c.x.max(d.x) + eps < a.x.min(b.x)
        || a.y.max(b.y) + eps < c.y.min(d.y)
        || c.y.max(d.y) + eps < a.y.min(b.y)
    {
        return false;
    }
    let ab = length(a, b);
    let cd = length(c, d);
    let o1 = orient(a, b, c);
    let o2 = orient(a, b, d);
    let o3 = orient(c, d, a);
    let o4 = orient(c, d, b);
    if o1.abs() <= eps * ab && on_segment(c, a, b, eps)
        || o2.abs() <= eps * ab && on_segment(d, a, b, eps)
        || o3.abs() <= eps * cd && on_segment(a, c, d, eps)
        || o4.abs() <= eps * cd && on_segment(b, c, d, eps)
    {
        return true;
    }
    (o1 > eps * ab && o2 < -eps * ab || o1 < -eps * ab && o2 > eps * ab)
        && (o3 > eps * cd && o4 < -eps * cd || o3 < -eps * cd && o4 > eps * cd)
}

fn ring_error(kind: RingKind, void_index: Option<usize>, self_crossing: bool) -> PlanarError {
    match kind {
        RingKind::Void if self_crossing => PlanarError::new(
            "void_self_intersection",
            "opening contour self-intersects or touches itself",
            void_index,
        ),
        RingKind::Void => PlanarError::new(
            "void_degenerate",
            "opening contour is nonfinite, too small, or degenerate",
            void_index,
        ),
        _ => PlanarError::new(
            "unsupported_host_shape",
            "host contour is invalid or exceeds planar limits",
            None,
        ),
    }
}

fn validate_ring(
    input: &[Point2],
    kind: RingKind,
    void_index: Option<usize>,
    tolerance: f64,
    eps: f64,
) -> Result<Vec<Point2>, PlanarError> {
    let mut ring = input.to_vec();
    if ring.len() >= 2 && length(ring[0], ring[ring.len() - 1]) <= eps {
        ring.pop();
    }
    if ring.len() < 3
        || ring.len() > MAX_EDGES
        || ring.iter().any(|p| !p.x.is_finite() || !p.y.is_finite())
    {
        return Err(ring_error(kind, void_index, false));
    }
    let n = ring.len();
    let mut perimeter = 0.0;
    for i in 0..n {
        let edge_length = length(ring[i], ring[(i + 1) % n]);
        if !edge_length.is_finite() || edge_length <= tolerance {
            return Err(ring_error(kind, void_index, false));
        }
        perimeter += edge_length;
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let c = ring[(i + 2) % n];
        if orient(a, b, c).abs() <= eps * edge_length
            && (b.x - a.x) * (c.x - b.x) + (b.y - a.y) * (c.y - b.y) < 0.0
        {
            return Err(ring_error(kind, void_index, true));
        }
    }
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || i == 0 && j == n - 1 {
                continue;
            }
            if segments_meet(ring[i], ring[(i + 1) % n], ring[j], ring[(j + 1) % n], eps) {
                return Err(ring_error(kind, void_index, true));
            }
        }
    }
    let area = signed_area(&ring);
    if !area.is_finite() || area.abs() <= tolerance * perimeter * 0.25 {
        return Err(ring_error(kind, void_index, false));
    }
    if area < 0.0 {
        ring.reverse();
    }
    Ok(ring)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Location {
    Outside,
    Boundary,
    Inside,
}

fn point_location(p: Point2, ring: &[Point2], eps: f64) -> Location {
    let mut inside = false;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        if on_segment(p, a, b, eps) {
            return Location::Boundary;
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x_cross = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if x_cross > p.x {
                inside = !inside;
            }
        }
    }
    if inside {
        Location::Inside
    } else {
        Location::Outside
    }
}

/// A concave host can contain every void vertex but exclude the middle of a
/// void edge. Split at all host crossings, then inspect every open subsegment.
fn ring_within_outer(inner: &[Point2], outer: &[Point2], eps: f64) -> bool {
    for i in 0..inner.len() {
        let a = inner[i];
        let b = inner[(i + 1) % inner.len()];
        if point_location(a, outer, eps) == Location::Outside {
            return false;
        }
        let ab = sub(b, a);
        let mut parameters = vec![0.0, 1.0];
        for j in 0..outer.len() {
            let c = outer[j];
            let d = outer[(j + 1) % outer.len()];
            let cd = sub(d, c);
            let denominator = cross(ab, cd);
            let scale = (ab.x * cd.y).abs() + (ab.y * cd.x).abs();
            if denominator.abs() > 64.0 * f64::EPSILON * scale.max(1.0) {
                let ac = sub(c, a);
                let t = cross(ac, cd) / denominator;
                let u = cross(ac, ab) / denominator;
                if (-eps..=1.0 + eps).contains(&t) && (-eps..=1.0 + eps).contains(&u) {
                    parameters.push(t.clamp(0.0, 1.0));
                }
            } else if orient(a, b, c).abs() <= eps * length(a, b) {
                let len2 = ab.x * ab.x + ab.y * ab.y;
                for p in [c, d] {
                    let t = ((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2;
                    if (0.0..=1.0).contains(&t) {
                        parameters.push(t);
                    }
                }
            }
        }
        parameters.sort_by(f64::total_cmp);
        parameters.dedup_by(|a, b| (*a - *b).abs() <= 64.0 * f64::EPSILON);
        for pair in parameters.windows(2) {
            let mid = 0.5 * (pair[0] + pair[1]);
            if point_location(lerp(a, b, mid), outer, eps) == Location::Outside {
                return false;
            }
        }
    }
    true
}

fn intersection_x(a: Segment, b: Segment) -> Option<f64> {
    let r = sub(a.b, a.a);
    let s = sub(b.b, b.a);
    let denominator = cross(r, s);
    let scale = (r.x * s.y).abs() + (r.y * s.x).abs();
    if denominator.abs() <= 64.0 * f64::EPSILON * scale.max(1.0) {
        return None;
    }
    let delta = sub(b.a, a.a);
    let t = cross(delta, s) / denominator;
    let u = cross(delta, r) / denominator;
    if (-64.0 * f64::EPSILON..=1.0 + 64.0 * f64::EPSILON).contains(&t)
        && (-64.0 * f64::EPSILON..=1.0 + 64.0 * f64::EPSILON).contains(&u)
    {
        Some(a.a.x + t.clamp(0.0, 1.0) * r.x)
    } else {
        None
    }
}

fn topology_error() -> PlanarError {
    PlanarError::new(
        "nonmanifold_result",
        "planar subtraction produces a pinched or inconsistent boundary",
        None,
    )
}

fn complexity_error() -> PlanarError {
    PlanarError::new(
        "unsupported_host_shape",
        "planar subtraction exceeds bounded edge, event, or cell limits",
        None,
    )
}

fn add_edge(
    edges: &mut Vec<RawEdge>,
    from_x: usize,
    from_y: f64,
    to_x: usize,
    to_y: f64,
) -> Result<(), PlanarError> {
    if edges.len() >= 4 * MAX_CELLS {
        return Err(complexity_error());
    }
    edges.push(RawEdge {
        from_x,
        to_x,
        from_y,
        to_y,
    });
    Ok(())
}

fn vertical_boundary(
    event: usize,
    left: &[(f64, f64)],
    right: &[(f64, f64)],
    eps: f64,
    edges: &mut Vec<RawEdge>,
) -> Result<(), PlanarError> {
    let mut changes = Vec::<(f64, usize, i32)>::with_capacity(2 * (left.len() + right.len()));
    for (side, intervals) in [(0, left), (1, right)] {
        for &(bottom, top) in intervals {
            changes.push((bottom, side, 1));
            changes.push((top, side, -1));
        }
    }
    changes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut active = [0i32; 2];
    let mut pos = 0;
    while pos < changes.len() {
        let y = changes[pos].0;
        while pos < changes.len() && (changes[pos].0 - y).abs() <= eps {
            active[changes[pos].1] += changes[pos].2;
            pos += 1;
        }
        if pos == changes.len() {
            break;
        }
        let next = changes[pos].0;
        if next - y <= eps || (active[0] > 0) == (active[1] > 0) {
            continue;
        }
        if active[0] > 0 {
            add_edge(edges, event, y, event, next)?;
        } else {
            add_edge(edges, event, next, event, y)?;
        }
    }
    if active != [0, 0] {
        return Err(topology_error());
    }
    Ok(())
}

fn material(outer: bool, native_count: usize, void_count: usize) -> bool {
    outer && native_count == 0 && void_count == 0
}

fn toggle(
    ring: usize,
    kinds: &[RingKind],
    active: &mut [bool],
    outer: &mut bool,
    native_count: &mut usize,
    void_count: &mut usize,
) {
    let was = active[ring];
    active[ring] = !was;
    match kinds[ring] {
        RingKind::Outer => *outer = !was,
        RingKind::Native => {
            if was {
                *native_count -= 1;
            } else {
                *native_count += 1;
            }
        }
        RingKind::Void => {
            if was {
                *void_count -= 1;
            } else {
                *void_count += 1;
            }
        }
    }
}

fn assemble_boundary(
    raw: Vec<RawEdge>,
    x_events: &[f64],
    eps: f64,
    expected_area: f64,
    tolerance: f64,
) -> Result<Vec<[Point2; 2]>, PlanarError> {
    if raw.is_empty() {
        return if expected_area == 0.0 {
            Ok(Vec::new())
        } else {
            Err(topology_error())
        };
    }
    let mut endpoints = vec![Vec::<EndpointRef>::new(); x_events.len()];
    for (edge, item) in raw.iter().enumerate() {
        endpoints[item.from_x].push(EndpointRef {
            y: item.from_y,
            edge,
            is_start: true,
        });
        endpoints[item.to_x].push(EndpointRef {
            y: item.to_y,
            edge,
            is_start: false,
        });
    }
    let mut starts = vec![(0usize, 0usize); raw.len()];
    let mut ends = vec![(0usize, 0usize); raw.len()];
    let mut node_y = Vec::<Vec<f64>>::with_capacity(x_events.len());
    for (x, list) in endpoints.iter_mut().enumerate() {
        list.sort_by(|a, b| a.y.total_cmp(&b.y));
        let mut values = Vec::<f64>::new();
        for endpoint in list.iter() {
            if values.last().is_none_or(|last| endpoint.y - *last > eps) {
                values.push(endpoint.y);
            }
            let node = (x, values.len() - 1);
            if endpoint.is_start {
                starts[endpoint.edge] = node;
            } else {
                ends[endpoint.edge] = node;
            }
        }
        node_y.push(values);
    }
    let point = |node: Node| Point2::new(x_events[node.0], node_y[node.0][node.1]);
    let mut outgoing = HashMap::<Node, usize>::with_capacity(raw.len());
    let mut incoming = HashMap::<Node, usize>::with_capacity(raw.len());
    for edge in 0..raw.len() {
        if starts[edge] == ends[edge]
            || outgoing.insert(starts[edge], edge).is_some()
            || incoming.insert(ends[edge], edge).is_some()
        {
            return Err(topology_error());
        }
    }
    if outgoing.keys().any(|node| !incoming.contains_key(node))
        || incoming.keys().any(|node| !outgoing.contains_key(node))
    {
        return Err(topology_error());
    }
    let mut visited = vec![false; raw.len()];
    let mut cycles = Vec::<(f64, Vec<usize>)>::new();
    for first in 0..raw.len() {
        if visited[first] {
            continue;
        }
        let mut cycle = Vec::new();
        let mut edge = first;
        loop {
            if visited[edge] {
                if edge == first {
                    break;
                }
                return Err(topology_error());
            }
            visited[edge] = true;
            cycle.push(edge);
            edge = *outgoing.get(&ends[edge]).ok_or_else(topology_error)?;
        }
        let area = 0.5
            * cycle
                .iter()
                .map(|&i| cross(point(starts[i]), point(ends[i])))
                .sum::<f64>();
        if area.abs() <= tolerance * tolerance * 0.25 {
            return Err(topology_error());
        }
        let first_at = cycle
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let pa = point(starts[**a]);
                let pb = point(starts[**b]);
                pa.x.total_cmp(&pb.x).then(pa.y.total_cmp(&pb.y))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        cycle.rotate_left(first_at);
        cycles.push((area, cycle));
    }
    cycles.sort_by(|a, b| {
        (b.0 > 0.0)
            .cmp(&(a.0 > 0.0))
            .then(b.0.abs().total_cmp(&a.0.abs()))
            .then_with(|| {
                let pa = point(starts[a.1[0]]);
                let pb = point(starts[b.1[0]]);
                pa.x.total_cmp(&pb.x).then(pa.y.total_cmp(&pb.y))
            })
    });
    let perimeter = raw
        .iter()
        .enumerate()
        .map(|(i, _)| length(point(starts[i]), point(ends[i])))
        .sum::<f64>();
    let boundary_area: f64 = cycles.iter().map(|(area, _)| *area).sum();
    if (boundary_area - expected_area).abs()
        > (tolerance * perimeter * 2.0).max(expected_area.abs() * 1e-9)
    {
        return Err(topology_error());
    }
    Ok(cycles
        .into_iter()
        .flat_map(|(_, cycle)| {
            cycle
                .into_iter()
                .map(|i| [point(starts[i]), point(ends[i])])
        })
        .collect())
}

/// Subtract the union of hosted voids from a host polygon with native holes.
/// Inputs are simple implicitly closed rings in one host-local plane.
pub(crate) fn subtract_regions(
    outer: &[Point2],
    native_holes: &[Vec<Point2>],
    voids: &[Vec<Point2>],
    tolerance: f64,
) -> Result<PlanarRegion, PlanarError> {
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(PlanarError::new(
            "unsupported_host_shape",
            "planar tolerance must be finite and positive",
            None,
        ));
    }
    let scale = outer
        .iter()
        .chain(native_holes.iter().flatten())
        .chain(voids.iter().flatten())
        .fold(1.0_f64, |max, p| max.max(p.x.abs()).max(p.y.abs()));
    let eps = (tolerance * 1e-3)
        .max(64.0 * f64::EPSILON * scale)
        .max(1e-12);
    let mut rings = Vec::<Vec<Point2>>::new();
    let mut kinds = Vec::<RingKind>::new();
    let host = validate_ring(outer, RingKind::Outer, None, tolerance, eps)?;
    rings.push(host);
    kinds.push(RingKind::Outer);
    for hole in native_holes {
        let ring = validate_ring(hole, RingKind::Native, None, tolerance, eps)?;
        if !ring_within_outer(&ring, &rings[0], eps) {
            return Err(ring_error(RingKind::Native, None, false));
        }
        rings.push(ring);
        kinds.push(RingKind::Native);
    }
    for (index, void) in voids.iter().enumerate() {
        let ring = validate_ring(void, RingKind::Void, Some(index), tolerance, eps)?;
        if !ring_within_outer(&ring, &rings[0], eps) {
            return Err(PlanarError::new(
                "void_outside_host",
                "opening contour extends outside its host",
                Some(index),
            ));
        }
        rings.push(ring);
        kinds.push(RingKind::Void);
    }
    let edge_count: usize = rings.iter().map(Vec::len).sum();
    if edge_count > MAX_EDGES {
        return Err(complexity_error());
    }
    let mut segments = Vec::with_capacity(edge_count);
    let mut x_events = Vec::with_capacity(edge_count);
    for (ring_id, ring) in rings.iter().enumerate() {
        for i in 0..ring.len() {
            let mut a = ring[i];
            let mut b = ring[(i + 1) % ring.len()];
            if point_order(a, b).is_gt() {
                std::mem::swap(&mut a, &mut b);
            }
            segments.push(Segment {
                a,
                b,
                ring: ring_id,
            });
            x_events.push(ring[i].x);
        }
    }
    // The sweep and its floating-point interpolation order must be independent
    // of opening creation order, ring winding, and the implicit closing vertex.
    segments.sort_by(|a, b| point_order(a.a, b.a).then(point_order(a.b, b.b)));
    for i in 0..segments.len() {
        for j in i + 1..segments.len() {
            if let Some(x) = intersection_x(segments[i], segments[j]) {
                x_events.push(x);
                if x_events.len() > MAX_X_EVENTS * 4 {
                    return Err(complexity_error());
                }
            }
        }
    }
    x_events.sort_by(f64::total_cmp);
    x_events.dedup_by(|a, b| (*a - *b).abs() <= eps);
    if x_events.len() > MAX_X_EVENTS || x_events.len() < 2 {
        return Err(complexity_error());
    }
    let mut cells = Vec::<[Point2; 4]>::new();
    let mut raw_edges = Vec::<RawEdge>::new();
    let mut sides = Vec::<StripSides>::with_capacity(x_events.len() - 1);
    let mut area = 0.0;
    for strip in 0..x_events.len() - 1 {
        let x0 = x_events[strip];
        let x1 = x_events[strip + 1];
        let mid = x0 + 0.5 * (x1 - x0);
        let mut crossings = Vec::<Crossing>::new();
        for (index, segment) in segments.iter().enumerate() {
            if mid > segment.a.x.min(segment.b.x) && mid < segment.a.x.max(segment.b.x) {
                crossings.push(Crossing {
                    y: segment.y_at(mid),
                    segment: index,
                });
            }
        }
        crossings.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.segment.cmp(&b.segment)));
        let mut groups = Vec::<(usize, usize)>::new();
        let mut index = 0;
        while index < crossings.len() {
            let start = index;
            let y = crossings[start].y;
            index += 1;
            while index < crossings.len() && (crossings[index].y - y).abs() <= eps {
                index += 1;
            }
            groups.push((start, index));
        }
        let mut active = vec![false; rings.len()];
        let mut outer_active = false;
        let mut native_count = 0;
        let mut void_count = 0;
        let mut side = StripSides::default();
        for (group_id, &(start, end)) in groups.iter().enumerate() {
            let before = material(outer_active, native_count, void_count);
            for crossing in &crossings[start..end] {
                toggle(
                    segments[crossing.segment].ring,
                    &kinds,
                    &mut active,
                    &mut outer_active,
                    &mut native_count,
                    &mut void_count,
                );
            }
            let after = material(outer_active, native_count, void_count);
            let edge_segment = segments[crossings[start].segment];
            if before != after {
                if after {
                    add_edge(
                        &mut raw_edges,
                        strip,
                        edge_segment.y_at(x0),
                        strip + 1,
                        edge_segment.y_at(x1),
                    )?;
                } else {
                    add_edge(
                        &mut raw_edges,
                        strip + 1,
                        edge_segment.y_at(x1),
                        strip,
                        edge_segment.y_at(x0),
                    )?;
                }
            }
            if after && group_id + 1 < groups.len() {
                let top_segment = segments[crossings[groups[group_id + 1].0].segment];
                let bottom_left = edge_segment.y_at(x0);
                let bottom_right = edge_segment.y_at(x1);
                let mut top_left = top_segment.y_at(x0);
                let mut top_right = top_segment.y_at(x1);
                if top_left < bottom_left - eps || top_right < bottom_right - eps {
                    return Err(topology_error());
                }
                top_left = top_left.max(bottom_left);
                top_right = top_right.max(bottom_right);
                let cell_area =
                    0.5 * (x1 - x0) * ((top_left - bottom_left) + (top_right - bottom_right));
                if cell_area > 0.0 {
                    if cells.len() >= MAX_CELLS {
                        return Err(complexity_error());
                    }
                    cells.push([
                        Point2::new(x0, bottom_left),
                        Point2::new(x1, bottom_right),
                        Point2::new(x1, top_right),
                        Point2::new(x0, top_left),
                    ]);
                    area += cell_area;
                    if top_left - bottom_left > eps {
                        side.left.push((bottom_left, top_left));
                    }
                    if top_right - bottom_right > eps {
                        side.right.push((bottom_right, top_right));
                    }
                }
            }
        }
        if active.iter().any(|value| *value) || outer_active || native_count != 0 || void_count != 0
        {
            return Err(topology_error());
        }
        sides.push(side);
    }
    for event in 0..x_events.len() {
        let left = if event > 0 {
            &sides[event - 1].right[..]
        } else {
            &[][..]
        };
        let right = if event < sides.len() {
            &sides[event].left[..]
        } else {
            &[][..]
        };
        vertical_boundary(event, left, right, eps, &mut raw_edges)?;
    }
    let boundary_edges = assemble_boundary(raw_edges, &x_events, eps, area, tolerance)?;
    Ok(PlanarRegion {
        cells,
        boundary_edges,
        area,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(points: &[(f64, f64)]) -> Vec<Point2> {
        points.iter().map(|&(x, y)| Point2::new(x, y)).collect()
    }

    fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point2> {
        polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
    }

    fn region(
        outer: &[(f64, f64)],
        holes: &[Vec<Point2>],
        voids: &[Vec<Point2>],
    ) -> Result<PlanarRegion, PlanarError> {
        subtract_regions(&polygon(outer), holes, voids, 1e-7)
    }

    fn boundary_area(edges: &[[Point2; 2]]) -> f64 {
        edges
            .iter()
            .map(|edge| cross(edge[0], edge[1]))
            .sum::<f64>()
            * 0.5
    }

    fn perimeter(edges: &[[Point2; 2]]) -> f64 {
        edges.iter().map(|edge| length(edge[0], edge[1])).sum()
    }

    fn assert_ccw_cells(cells: &[[Point2; 4]]) {
        assert!(!cells.is_empty());
        for cell in cells {
            let twice_area = (0..4)
                .map(|i| cross(cell[i], cell[(i + 1) % 4]))
                .sum::<f64>();
            assert!(twice_area > 0.0, "cell has reversed winding: {cell:?}");
        }
    }

    #[test]
    fn overlapping_rectangles_union_once() {
        let outer = rectangle(0.0, 0.0, 10.0, 10.0);
        let a = rectangle(2.0, 2.0, 6.0, 6.0);
        let b = rectangle(4.0, 4.0, 8.0, 8.0);
        let result = subtract_regions(&outer, &[], &[a.clone(), b.clone()], 1e-7).unwrap();
        let reversed = subtract_regions(&outer, &[], &[b, a], 1e-7).unwrap();
        assert!((result.area - 72.0).abs() < 1e-8);
        assert!((reversed.area - result.area).abs() < 1e-8);
        assert!((boundary_area(&result.boundary_edges) - result.area).abs() < 1e-8);
        assert!((perimeter(&result.boundary_edges) - 64.0).abs() < 1e-8);
        assert_ccw_cells(&result.cells);
    }

    #[test]
    fn crossing_triangles_and_rotated_host() {
        let outer = polygon(&[(-5.0, 0.0), (0.0, -5.0), (5.0, 0.0), (0.0, 5.0)]);
        let a = polygon(&[(-2.0, -1.0), (2.0, -1.0), (0.0, 2.0)]);
        let b = polygon(&[(-2.0, 1.0), (2.0, 1.0), (0.0, -2.0)]);
        let result = subtract_regions(&outer, &[], &[a, b], 1e-7).unwrap();
        assert!((result.area - 42.0).abs() < 1e-8);
        assert!((boundary_area(&result.boundary_edges) - result.area).abs() < 1e-8);
        assert_ccw_cells(&result.cells);
    }

    #[test]
    fn native_hole_and_edge_touch() {
        let outer = rectangle(0.0, 0.0, 10.0, 10.0);
        let hole = rectangle(3.0, 3.0, 5.0, 5.0);
        let void = rectangle(8.0, 2.0, 10.0, 4.0);
        let result = subtract_regions(&outer, &[hole], &[void], 1e-7).unwrap();
        assert!((result.area - 92.0).abs() < 1e-8);
        assert!((boundary_area(&result.boundary_edges) - 92.0).abs() < 1e-8);
        assert!((perimeter(&result.boundary_edges) - 52.0).abs() < 1e-8);
        assert_ccw_cells(&result.cells);
    }

    #[test]
    fn fully_removed_host_is_empty() {
        let outer = rectangle(0.0, 0.0, 2.0, 2.0);
        let result = subtract_regions(&outer, &[], &[outer.clone()], 1e-7).unwrap();
        assert_eq!(result.area, 0.0);
        assert!(result.cells.is_empty());
        assert!(result.boundary_edges.is_empty());
    }

    #[test]
    fn spanning_void_can_leave_two_valid_components() {
        let outer = rectangle(0.0, 0.0, 4.0, 4.0);
        let void = rectangle(1.0, 0.0, 3.0, 4.0);
        let result = subtract_regions(&outer, &[], &[void], 1e-7).unwrap();
        assert!((result.area - 8.0).abs() < 1e-8);
        assert!((boundary_area(&result.boundary_edges) - 8.0).abs() < 1e-8);
        assert!((perimeter(&result.boundary_edges) - 20.0).abs() < 1e-8);
    }

    #[test]
    fn oversized_void_is_rejected_instead_of_clipped() {
        let outer = rectangle(0.0, 0.0, 4.0, 4.0);
        let void = rectangle(1.0, 1.0, 4.001, 2.0);
        let error = subtract_regions(&outer, &[], &[void], 1e-6).unwrap_err();
        assert_eq!(error.code, "void_outside_host");
        assert_eq!(error.void_index, Some(0));
    }

    #[test]
    fn point_pinch_is_nonmanifold() {
        let outer = rectangle(0.0, 0.0, 4.0, 4.0);
        let void = polygon(&[(2.0, 0.0), (3.0, 1.0), (2.0, 2.0), (1.0, 1.0)]);
        let error = subtract_regions(&outer, &[], &[void], 1e-7).unwrap_err();
        assert_eq!(error.code, "nonmanifold_result");
    }

    #[test]
    fn two_voids_touching_at_one_point_are_nonmanifold() {
        let outer = rectangle(0.0, 0.0, 5.0, 5.0);
        let first = rectangle(1.0, 1.0, 2.0, 2.0);
        let second = rectangle(2.0, 2.0, 3.0, 3.0);
        let error = subtract_regions(&outer, &[], &[first, second], 1e-7).unwrap_err();
        assert_eq!(error.code, "nonmanifold_result");
    }

    #[test]
    fn concave_host_with_valid_void() {
        let outer = polygon(&[
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ]);
        let void = rectangle(0.2, 1.5, 0.8, 2.5);
        let result = subtract_regions(&outer, &[], &[void], 1e-7).unwrap();
        assert!((result.area - 6.4).abs() < 1e-8);
        assert!((boundary_area(&result.boundary_edges) - result.area).abs() < 1e-8);
        assert_ccw_cells(&result.cells);
    }

    #[test]
    fn concave_host_checks_edges_not_only_vertices() {
        let outer = &[
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ];
        let void = polygon(&[(0.5, 3.0), (3.0, 0.5), (0.5, 0.5)]);
        let error = region(outer, &[], &[void]).unwrap_err();
        assert_eq!(error.code, "void_outside_host");
        assert_eq!(error.void_index, Some(0));
    }

    #[test]
    fn invalid_void_reports_its_index() {
        let outer = rectangle(0.0, 0.0, 5.0, 5.0);
        let valid = rectangle(0.5, 0.5, 1.0, 1.0);
        let bowtie = polygon(&[(2.0, 2.0), (4.0, 4.0), (4.0, 2.0), (2.0, 4.0)]);
        let error = subtract_regions(&outer, &[], &[valid, bowtie], 1e-7).unwrap_err();
        assert_eq!(error.code, "void_self_intersection");
        assert_eq!(error.void_index, Some(1));
    }

    #[test]
    fn contour_order_winding_and_start_do_not_change_geometry() {
        let outer = rectangle(0.0, 0.0, 10.0, 10.0);
        let a = polygon(&[(2.0, 2.0), (8.0, 2.0), (5.0, 8.0)]);
        let b = polygon(&[(3.0, 4.0), (5.0, 8.0), (1.0, 4.0)]);
        let first = subtract_regions(&outer, &[], &[a.clone(), b.clone()], 1e-7).unwrap();
        let mut outer_reversed = outer.clone();
        outer_reversed.reverse();
        outer_reversed.rotate_left(1);
        let mut a_reversed = a;
        a_reversed.reverse();
        a_reversed.rotate_left(2);
        let mut b_reversed = b;
        b_reversed.reverse();
        b_reversed.rotate_left(1);
        let second =
            subtract_regions(&outer_reversed, &[], &[b_reversed, a_reversed], 1e-7).unwrap();
        assert_eq!(first.area, second.area);
        assert_eq!(first.cells, second.cells);
        assert_eq!(first.boundary_edges, second.boundary_edges);
    }
}
