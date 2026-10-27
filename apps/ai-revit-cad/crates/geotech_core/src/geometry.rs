use crate::model::*;

const MAX_TERRAIN_POINTS: usize = 2048;
#[derive(Clone, Copy)]
enum Intersection {
    None,
    Point(Point2),
    Overlap,
}
fn sub(a: Point2, b: Point2) -> Point2 {
    Point2::new(a.x - b.x, a.z - b.z)
}
fn cross(a: Point2, b: Point2) -> f64 {
    a.x.mul_add(b.z, -a.z * b.x)
}
fn dot(a: Point2, b: Point2) -> f64 {
    a.x.mul_add(b.x, a.z * b.z)
}
fn distance(a: Point2, b: Point2) -> f64 {
    (a.x - b.x).hypot(a.z - b.z)
}
fn lerp(a: Point2, b: Point2, t: f64) -> Point2 {
    Point2::new((b.x - a.x).mul_add(t, a.x), (b.z - a.z).mul_add(t, a.z))
}

/// Analytic fixture constructor, including a vertical slope (beta = 90 deg).
/// The terrain is finite and extends beyond the requested upper intersection.
pub fn triangular_section(
    height_m: f64,
    slope_angle_deg: f64,
    slip_angle_deg: f64,
    width_m: f64,
) -> Result<Section2D, GeotechError> {
    for (value, name) in [
        (height_m, "height_m"),
        (slope_angle_deg, "slope_angle_deg"),
        (slip_angle_deg, "slip_angle_deg"),
        (width_m, "width_m"),
    ] {
        finite(value, name)?;
    }
    if height_m <= 0.0
        || width_m <= 0.0
        || slope_angle_deg <= 0.0
        || slope_angle_deg > 90.0
        || slip_angle_deg <= 0.0
        || slip_angle_deg >= slope_angle_deg
    {
        return Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            "Triangle requires H > 0 m, B > 0 m and 0 < alpha < beta <= 90 deg",
        ));
    }
    let toe = Point2::new(0.0, 0.0);
    let crest = Point2::new(
        if slope_angle_deg == 90.0 {
            0.0
        } else {
            height_m / slope_angle_deg.to_radians().tan()
        },
        height_m,
    );
    let upper = Point2::new(height_m / slip_angle_deg.to_radians().tan(), height_m);
    let end = Point2::new(upper.x + height_m, height_m);
    let section = Section2D {
        terrain_polyline: vec![toe, crest, end],
        slope_toe: toe,
        crest,
        slip_surface: PlanarSlipSurface { lower: toe, upper },
        extrusion_width_m: width_m,
    };
    geometry(&section)?;
    Ok(section)
}

pub(crate) fn section_finite(section: &Section2D) -> Result<(), GeotechError> {
    for p in section.terrain_polyline.iter().chain([
        &section.slope_toe,
        &section.crest,
        &section.slip_surface.lower,
        &section.slip_surface.upper,
    ]) {
        finite(p.x, "section.point.x")?;
        finite(p.z, "section.point.z")?;
    }
    finite(section.extrusion_width_m, "section.extrusion_width_m")?;
    Ok(())
}

/// Tolerance reflects both local arithmetic and resolution of stored world coordinates.
/// It is a diagnostic rejection bound, never a coordinate-snapping operation.
pub(crate) fn tolerance(points: &[Point2]) -> Result<f64, GeotechError> {
    let Some(first) = points.first() else {
        return Err(GeotechError::new(
            ErrorCode::InvalidTerrain,
            "Empty point set",
        ));
    };
    let mut local_scale: f64 = 0.0;
    let mut absolute_scale: f64 = 0.0;
    for p in points {
        finite(p.x, "point.x")?;
        finite(p.z, "point.z")?;
        local_scale = local_scale
            .max((p.x - first.x).abs())
            .max((p.z - first.z).abs());
        absolute_scale = absolute_scale.max(p.x.abs()).max(p.z.abs());
    }
    let tol = local_scale * 64.0 * f64::EPSILON + absolute_scale * 4.0 * f64::EPSILON;
    if !local_scale.is_finite() || !tol.is_finite() {
        return Err(GeotechError::new(
            ErrorCode::NumericOverflow,
            "Coordinate differences exceed f64 range",
        ));
    }
    Ok(tol.max(f64::MIN_POSITIVE))
}

fn on_segment(p: Point2, a: Point2, b: Point2, tol: f64) -> Option<f64> {
    let d = sub(b, a);
    let length = d.x.hypot(d.z);
    if length <= tol || !length.is_finite() {
        return None;
    }
    // Divide before multiplication to avoid squared-length overflow.
    let direction = Point2::new(d.x / length, d.z / length);
    let q = sub(p, a);
    let along = dot(q, direction);
    if cross(direction, q).abs() <= tol && along >= 0.0 && along <= length {
        Some(along)
    } else if distance(p, a) <= tol {
        Some(0.0)
    } else if distance(p, b) <= tol {
        Some(length)
    } else {
        None
    }
}

fn intersection(a: Point2, b: Point2, c: Point2, d: Point2, tol: f64) -> Intersection {
    let r = sub(b, a);
    let s = sub(d, c);
    let q = sub(c, a);
    let lr = r.x.hypot(r.z);
    let ls = s.x.hypot(s.z);
    if lr <= tol || ls <= tol {
        return Intersection::None;
    }
    // Unit directions keep predicates finite over a broad coordinate range.
    let ru = Point2::new(r.x / lr, r.z / lr);
    let su = Point2::new(s.x / ls, s.z / ls);
    let den = cross(ru, su);
    let angle_tol = tol / lr + tol / ls;
    if den.abs() <= angle_tol {
        if cross(ru, q).abs() > tol {
            return Intersection::None;
        }
        let t0 = dot(q, ru);
        let t1 = dot(sub(d, a), ru);
        let lo = t0.min(t1).max(0.0);
        let hi = t0.max(t1).min(lr);
        if hi < lo - tol {
            Intersection::None
        } else if hi - lo > tol {
            Intersection::Overlap
        } else {
            Intersection::Point(lerp(a, b, ((lo + hi) * 0.5 / lr).clamp(0.0, 1.0)))
        }
    } else {
        let along_r = cross(q, su) / den;
        let along_s = cross(q, ru) / den;
        if along_r >= -tol && along_r <= lr + tol && along_s >= -tol && along_s <= ls + tol {
            Intersection::Point(lerp(a, b, (along_r / lr).clamp(0.0, 1.0)))
        } else {
            Intersection::None
        }
    }
}

pub(crate) fn validate_terrain(section: &Section2D) -> Result<f64, GeotechError> {
    section_finite(section)?;
    let p = &section.terrain_polyline;
    if p.len() < 2 || p.len() > MAX_TERRAIN_POINTS {
        return Err(GeotechError::new(
            ErrorCode::InvalidTerrain,
            format!("Terrain requires 2..={MAX_TERRAIN_POINTS} points"),
        ));
    }
    let tol = tolerance(p)?;
    if section.extrusion_width_m <= 0.0 {
        return Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            "Extrusion width must be positive metres",
        ));
    }
    for pair in p.windows(2) {
        if distance(pair[0], pair[1]) <= tol {
            return Err(GeotechError::new(
                ErrorCode::InvalidTerrain,
                "Duplicate or numerically indistinguishable terrain vertices",
            )
            .at(pair.to_vec()));
        }
    }
    for i in 0..p.len() - 1 {
        for j in i + 1..p.len() - 1 {
            let hit = intersection(p[i], p[i + 1], p[j], p[j + 1], tol);
            if j == i + 1 {
                if matches!(hit, Intersection::Overlap) {
                    return Err(GeotechError::new(
                        ErrorCode::SelfIntersection,
                        "Adjacent terrain segments overlap",
                    )
                    .at(vec![p[i], p[i + 1], p[j + 1]]));
                }
            } else if !matches!(hit, Intersection::None) {
                return Err(GeotechError::new(
                    ErrorCode::SelfIntersection,
                    "Terrain has a crossing or repeated non-adjacent contact",
                )
                .at(vec![p[i], p[i + 1], p[j], p[j + 1]]));
            }
        }
    }
    if section.crest.z <= section.slope_toe.z {
        return Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            "Crest must be above the identified slope toe",
        )
        .at(vec![section.slope_toe, section.crest]));
    }
    locate(p, section.slope_toe, tol)?;
    locate(p, section.crest, tol)?;
    Ok(tol)
}

/// A terrain location keeps topology and physical along-edge distance separate.
/// Adding a fractional edge parameter to an integer index loses resolved bends
/// on very long edges; no dimensionless epsilon is used for membership/order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TerrainLocation {
    Vertex(usize),
    Edge { index: usize, along_m: f64 },
}
impl TerrainLocation {
    fn rank(self) -> usize {
        match self {
            Self::Vertex(i) => 2 * i,
            Self::Edge { index, .. } => 2 * index + 1,
        }
    }
    fn compare(self, other: Self) -> std::cmp::Ordering {
        self.rank()
            .cmp(&other.rank())
            .then_with(|| match (self, other) {
                (Self::Edge { along_m: a, .. }, Self::Edge { along_m: b, .. }) => a.total_cmp(&b),
                _ => std::cmp::Ordering::Equal,
            })
    }
    fn precedes_vertex(self, index: usize) -> bool {
        self.compare(Self::Vertex(index)).is_lt()
    }
    fn follows_vertex(self, index: usize) -> bool {
        self.compare(Self::Vertex(index)).is_gt()
    }
}

fn locate(terrain: &[Point2], point: Point2, tol: f64) -> Result<TerrainLocation, GeotechError> {
    let mut positions: Vec<TerrainLocation> = vec![];
    for (i, pair) in terrain.windows(2).enumerate() {
        if let Some(along_m) = on_segment(point, pair[0], pair[1], tol) {
            let position = if distance(point, pair[0]) <= tol {
                TerrainLocation::Vertex(i)
            } else if distance(point, pair[1]) <= tol {
                TerrainLocation::Vertex(i + 1)
            } else {
                TerrainLocation::Edge { index: i, along_m }
            };
            if !positions.contains(&position) {
                positions.push(position);
            }
        }
    }
    match positions.as_slice() {
        [position] => Ok(*position),
        [] => Err(GeotechError::new(
            ErrorCode::EndpointNotOnTerrain,
            "Specified characteristic point is not on terrain",
        )
        .at(vec![point])),
        _ => Err(GeotechError::new(
            ErrorCode::AmbiguousIntersection,
            "Point has multiple terrain positions at current precision",
        )
        .at(vec![point])),
    }
}

fn signed_area(points: &[Point2]) -> f64 {
    let origin = points[0];
    let mut sum = 0.0;
    let mut correction = 0.0;
    for i in 0..points.len() {
        let term = cross(
            sub(points[i], origin),
            sub(points[(i + 1) % points.len()], origin),
        );
        // Neumaier compensated addition, after translation to a local origin.
        let next = sum + term;
        correction += if sum.abs() >= term.abs() {
            (sum - next) + term
        } else {
            (term - next) + sum
        };
        sum = next;
    }
    (sum + correction) * 0.5
}

/// Positive CCW polygon area. Rejects clockwise, zero-area and self-crossing rings.
/// A ring is closed implicitly; an explicitly repeated last vertex is rejected.
pub fn polygon_area_m2(points: &[Point2]) -> Result<f64, GeotechError> {
    if points.len() < 3 || points.len() > MAX_TERRAIN_POINTS + 2 {
        return Err(GeotechError::new(
            ErrorCode::DegeneratePolygon,
            "Polygon requires at least three bounded vertices",
        ));
    }
    let tol = tolerance(points)?;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        if distance(a, b) <= tol {
            return Err(GeotechError::new(
                ErrorCode::DegeneratePolygon,
                "Polygon edge is below coordinate resolution",
            )
            .at(vec![a, b]));
        }
        for j in i + 1..points.len() {
            let c = points[j];
            let d = points[(j + 1) % points.len()];
            if j == i + 1 || (i == 0 && j == points.len() - 1) {
                if matches!(intersection(a, b, c, d, tol), Intersection::Overlap) {
                    return Err(GeotechError::new(
                        ErrorCode::SelfIntersection,
                        "Adjacent polygon edges overlap",
                    )
                    .at(vec![a, b, c, d]));
                }
                continue;
            }
            if !matches!(intersection(a, b, c, d, tol), Intersection::None) {
                return Err(GeotechError::new(
                    ErrorCode::SelfIntersection,
                    "Polygon edges cross or touch",
                )
                .at(vec![a, b, c, d]));
            }
        }
    }
    let area = signed_area(points);
    if !area.is_finite() {
        return Err(GeotechError::new(
            ErrorCode::NumericOverflow,
            "Polygon area overflowed",
        ));
    }
    let perimeter: f64 = (0..points.len())
        .map(|i| distance(points[i], points[(i + 1) % points.len()]))
        .sum();
    if area.abs() <= tol * perimeter {
        return Err(GeotechError::new(
            ErrorCode::DegeneratePolygon,
            "Polygon area is zero or unresolved at source-coordinate precision",
        )
        .at(points.to_vec()));
    }
    if area < 0.0 {
        return Err(GeotechError::new(
            ErrorCode::ClockwisePolygon,
            "Polygon area is negative; a CCW ring is required",
        )
        .at(points.to_vec()));
    }
    Ok(area)
}

pub fn geometry(section: &Section2D) -> Result<FailureGeometry, GeotechError> {
    let tol = validate_terrain(section)?;
    geometry_validated(section, tol)
}
pub(crate) fn geometry_validated(
    section: &Section2D,
    tol: f64,
) -> Result<FailureGeometry, GeotechError> {
    let terrain = &section.terrain_polyline;
    let slip = section.slip_surface;
    let d = sub(slip.upper, slip.lower);
    let length = d.x.hypot(d.z);
    positive_output(length, "slip length")?;
    if d.z <= tol {
        return Err(GeotechError::new(ErrorCode::NoDrivingForce,"Slip upper point must be higher than lower point; horizontal/reversed surfaces have no admissible downhill driving force").at(vec![slip.lower,slip.upper]));
    }
    if d.x.abs() <= tol {
        return Err(GeotechError::new(
            ErrorCode::NonPositiveEffectiveNormal,
            "Vertical slip has zero compressive normal force and is outside this dry planar family",
        )
        .at(vec![slip.lower, slip.upper]));
    }
    let lower_s = locate(terrain, slip.lower, tol)?;
    let upper_s = locate(terrain, slip.upper, tol)?;
    let mut hits: Vec<Point2> = vec![];
    for pair in terrain.windows(2) {
        match intersection(slip.lower, slip.upper, pair[0], pair[1], tol) {
            Intersection::None => (),
            Intersection::Overlap => return Err(GeotechError::new(
                ErrorCode::AmbiguousIntersection,
                "Slip and terrain overlap or are indistinguishable at source-coordinate precision",
            )
            .at(pair.to_vec())),
            Intersection::Point(p) => {
                if !hits.iter().any(|q| distance(*q, p) <= tol * 2.0) {
                    hits.push(p);
                }
            }
        }
    }
    if hits.len() != 2
        || !hits.iter().any(|p| distance(*p, slip.lower) <= tol * 2.0)
        || !hits.iter().any(|p| distance(*p, slip.upper) <= tol * 2.0)
    {
        return Err(GeotechError::new(
            ErrorCode::AmbiguousIntersection,
            format!(
                "Expected only the two prescribed terrain intersections, found {}",
                hits.len()
            ),
        )
        .at(hits));
    }
    let lower_first = lower_s.compare(upper_s).is_lt();
    let (lo, hi, start, end) = if lower_first {
        (lower_s, upper_s, slip.lower, slip.upper)
    } else {
        (upper_s, lower_s, slip.upper, slip.lower)
    };
    let mut polygon = vec![start];
    for (i, p) in terrain.iter().enumerate() {
        if lo.precedes_vertex(i) && hi.follows_vertex(i) {
            polygon.push(*p);
        }
    }
    polygon.push(end);
    let unit = Point2::new(d.x / length, d.z / length);
    for p in &polygon {
        if cross(unit, sub(*p, slip.lower)) * d.x.signum() < -tol {
            return Err(GeotechError::new(
                ErrorCode::SlipOutsideGround,
                "Slip leaves the soil domain below the ordered terrain",
            )
            .at(polygon));
        }
    }
    if polygon.len() < 3 {
        return Err(GeotechError::new(
            ErrorCode::DegeneratePolygon,
            "Terrain interval and slip enclose no prism",
        )
        .at(polygon));
    }
    if signed_area(&polygon) < 0.0 {
        polygon.reverse();
    }
    let area = polygon_area_m2(&polygon)?;
    validate_unused_terrain(terrain, lo, hi, start, end, &polygon, tol)?;
    let width = section.extrusion_width_m;
    let volume = positive_output(area * width, "prism volume")?;
    let base_area = positive_output(length * width, "slip base area")?;
    Ok(FailureGeometry {
        polygon,
        slip_surface: slip,
        area_m2: area,
        slip_length_m: length,
        width_m: width,
        volume_m3: volume,
        slip_base_area_m2: base_area,
        slip_angle_deg: d.z.atan2(d.x.abs()).to_degrees(),
        numeric_tolerance_m: tol,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum RingLocation {
    Outside,
    Boundary,
    Inside,
}
fn point_in_ring(point: Point2, polygon: &[Point2], tol: f64) -> RingLocation {
    let mut winding = 0i32;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        if on_segment(point, a, b, tol).is_some() {
            return RingLocation::Boundary;
        }
        let edge = sub(b, a);
        let side = cross(edge, sub(point, a));
        if a.z <= point.z && b.z > point.z && side > 0.0 {
            winding += 1;
        } else if a.z > point.z && b.z <= point.z && side < 0.0 {
            winding -= 1;
        }
    }
    if winding == 0 {
        RingLocation::Outside
    } else {
        RingLocation::Inside
    }
}
/// An unused tail entering the selected prism makes the material side ambiguous.
/// Check complete clipped tail segments against the ring, not only their vertices.
fn validate_unused_terrain(
    terrain: &[Point2],
    lo: TerrainLocation,
    hi: TerrainLocation,
    start: Point2,
    end: Point2,
    polygon: &[Point2],
    tol: f64,
) -> Result<(), GeotechError> {
    let mut unused = vec![];
    for (i, pair) in terrain.windows(2).enumerate() {
        let a = TerrainLocation::Vertex(i);
        let b = TerrainLocation::Vertex(i + 1);
        if !b.compare(lo).is_gt() {
            unused.push((pair[0], pair[1]));
        } else if a.compare(lo).is_lt() {
            unused.push((pair[0], start));
        }
        if !a.compare(hi).is_lt() {
            unused.push((pair[0], pair[1]));
        } else if b.compare(hi).is_gt() {
            unused.push((end, pair[1]));
        }
    }
    for (a, b) in unused {
        if distance(a, b) <= tol {
            continue;
        }
        for point in [a, lerp(a, b, 0.5), b] {
            if distance(point, start) <= tol || distance(point, end) <= tol {
                continue;
            }
            if point_in_ring(point, polygon, tol) != RingLocation::Outside {
                return Err(GeotechError::new(ErrorCode::UnsupportedTerrain,
                    "Unused terrain enters or touches the failure prism; the material side is ambiguous").at(vec![a,b,point]));
            }
        }
        for i in 0..polygon.len() {
            match intersection(a,b,polygon[i],polygon[(i+1)%polygon.len()],tol){
                Intersection::None=>(),
                Intersection::Point(point) if distance(point,start)<=tol*2.0 || distance(point,end)<=tol*2.0=>(),
                _=>return Err(GeotechError::new(ErrorCode::UnsupportedTerrain,
                    "Unused terrain crosses or overlaps the failure-prism boundary; no unique material domain").at(vec![a,b,polygon[i],polygon[(i+1)%polygon.len()]])),
            }
        }
    }
    Ok(())
}

/// Finds the unique upper terrain intersection of an upward ray from the lower point.
/// Multiple contacts are rejected even if one could heuristically select the nearest.
pub(crate) fn ray_exit(
    section: &Section2D,
    angle_deg: f64,
    tol: f64,
) -> Result<Point2, GeotechError> {
    let lower = section.slip_surface.lower;
    locate(&section.terrain_polyline, lower, tol)?;
    let horizontal_delta = section.slip_surface.upper.x - lower.x;
    if horizontal_delta.abs() <= tol {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Search needs an explicit horizontal uphill direction",
        ));
    }
    let sign = horizontal_delta.signum();
    let angle = angle_deg.to_radians();
    let ray = Point2::new(angle.cos() * sign, angle.sin());
    let mut hits: Vec<Point2> = vec![];
    for pair in section.terrain_polyline.windows(2) {
        let edge = sub(pair[1], pair[0]);
        let len = edge.x.hypot(edge.z);
        let edge_u = Point2::new(edge.x / len, edge.z / len);
        let q = sub(pair[0], lower);
        let den = cross(ray, edge_u);
        if den.abs() <= tol / len + 64.0 * f64::EPSILON {
            if cross(ray, q).abs() <= tol
                && (dot(q, ray) > tol || dot(sub(pair[1], lower), ray) > tol)
            {
                return Err(GeotechError::new(
                    ErrorCode::AmbiguousIntersection,
                    "Search ray overlaps terrain",
                )
                .at(pair.to_vec()));
            }
            continue;
        }
        let t = cross(q, edge_u) / den;
        let u = cross(q, ray) / den;
        if t > tol && u >= -tol && u <= len + tol {
            // Preserve exact terrain vertex coordinates when the exit is at a vertex.
            let point = if u.abs() <= tol {
                pair[0]
            } else if (u - len).abs() <= tol {
                pair[1]
            } else {
                Point2::new(ray.x.mul_add(t, lower.x), ray.z.mul_add(t, lower.z))
            };
            if !hits.iter().any(|p| distance(*p, point) <= tol * 2.0) {
                hits.push(point);
            }
        }
    }
    match hits.as_slice() {
        [p] => Ok(*p),
        [] => Err(GeotechError::new(
            ErrorCode::EndpointNotOnTerrain,
            "Search ray has no upper exit within the explicitly supplied terrain",
        )
        .at(vec![lower])),
        _ => Err(GeotechError::new(
            ErrorCode::AmbiguousIntersection,
            "Search ray has multiple upper terrain contacts",
        )
        .at(hits)),
    }
}

fn triangulate(points: &[Point2], tol: f64) -> Result<Vec<[u32; 3]>, GeotechError> {
    let mut ring: Vec<usize> = (0..points.len()).collect();
    let mut triangles = vec![];
    while ring.len() > 3 {
        let mut ear = None;
        for k in 0..ring.len() {
            let ia = ring[(k + ring.len() - 1) % ring.len()];
            let ib = ring[k];
            let ic = ring[(k + 1) % ring.len()];
            let a = points[ia];
            let b = points[ib];
            let c = points[ic];
            let area = cross(sub(b, a), sub(c, a));
            let bound = tol * (distance(a, b) + distance(b, c) + distance(c, a));
            if area.abs() <= bound {
                // Keep boundary vertices in the cap so side faces cannot form T-junctions.
                continue;
            }
            if area < 0.0 {
                continue;
            }
            let contains = ring.iter().any(|&ip| {
                if ip == ia || ip == ib || ip == ic {
                    return false;
                }
                let p = points[ip];
                cross(sub(b, a), sub(p, a)) >= -bound
                    && cross(sub(c, b), sub(p, b)) >= -bound
                    && cross(sub(a, c), sub(p, c)) >= -bound
            });
            if !contains {
                ear = Some((k, Some([ia as u32, ib as u32, ic as u32])));
                break;
            }
        }
        let Some((k, triangle)) = ear else {
            return Err(GeotechError::new(
                ErrorCode::InvalidMesh,
                "Cross-section triangulation is numerically ambiguous",
            )
            .at(points.to_vec()));
        };
        if let Some(t) = triangle {
            triangles.push(t);
        }
        ring.remove(k);
    }
    if ring.len() == 3 {
        triangles.push([ring[0] as u32, ring[1] as u32, ring[2] as u32]);
    }
    Ok(triangles)
}

/// Extrusion in global x/z and y in [0,B]. Meshes are disposable f64 render data.
pub fn extrude(geometry: &FailureGeometry) -> Result<ExtrudedMesh, GeotechError> {
    let area = polygon_area_m2(&geometry.polygon)?;
    let tol = tolerance(&geometry.polygon)?;
    if !geometry.width_m.is_finite()
        || geometry.width_m <= 0.0
        || !geometry.area_m2.is_finite()
        || (area - geometry.area_m2).abs() > tol * geometry.slip_length_m.max(1.0) * 8.0
        || !geometry.volume_m3.is_finite()
        || (area * geometry.width_m - geometry.volume_m3).abs() > geometry.volume_m3.abs() * 1e-10
    {
        return Err(GeotechError::new(
            ErrorCode::InvalidMesh,
            "Extrusion geometry is inconsistent or non-finite",
        ));
    }
    let caps = triangulate(&geometry.polygon, tol)?;
    let n = geometry.polygon.len() as u32;
    let mut vertices = vec![];
    for y in [0.0, geometry.width_m] {
        for p in &geometry.polygon {
            vertices.push([p.x, y, p.z]);
        }
    }
    let mut triangles = caps.clone();
    triangles.extend(caps.iter().map(|t| [t[2] + n, t[1] + n, t[0] + n]));
    let mut slip_triangles = vec![];
    for i in 0..n {
        let j = (i + 1) % n;
        let a = geometry.polygon[i as usize];
        let b = geometry.polygon[j as usize];
        let faces = [[i, n + i, n + j], [i, n + j, j]];
        triangles.extend(faces);
        let slip = geometry.slip_surface;
        if (distance(a, slip.lower) <= tol * 2.0 && distance(b, slip.upper) <= tol * 2.0)
            || (distance(a, slip.upper) <= tol * 2.0 && distance(b, slip.lower) <= tol * 2.0)
        {
            slip_triangles.extend(faces);
        }
    }
    if slip_triangles.len() != 2 {
        return Err(GeotechError::new(
            ErrorCode::InvalidMesh,
            "Exactly one extruded slip-base edge is required",
        ));
    }
    Ok(ExtrudedMesh {
        vertices,
        triangles,
        slip_triangles,
        cross_section_triangles: caps,
    })
}
