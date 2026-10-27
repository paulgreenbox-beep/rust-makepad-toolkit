//! Shared source-geometry validation for model commits and display projection.
//!
//! Coordinates and dimensions are metres. Normalized contours are derived data;
//! callers retain the original specification in the document.

use crate::{ElementKind, Point2};

/// The default tolerance for collapsed source geometry in the MVP.
pub const DEFAULT_LINEAR_TOLERANCE_M: f64 = 1.0e-8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryOptions {
    pub linear_tolerance_m: f64,
    /// Bounds quadratic contour validation and strip tessellation cost.
    pub max_vertices_per_element: usize,
}

impl Default for GeometryOptions {
    fn default() -> Self {
        Self {
            linear_tolerance_m: DEFAULT_LINEAR_TOLERANCE_M,
            max_vertices_per_element: 2_048,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryErrorCode {
    InvalidOptions,
    NonFinite,
    NonPositiveDimension,
    DegenerateLine,
    ContourTooSmall,
    DegenerateContour,
    SelfIntersection,
    HoleOutsideBoundary,
    ContoursIntersect,
    NumericRange,
    TessellationFailed,
    VertexLimitExceeded,
    TriangleLimitExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryValidationError {
    pub code: GeometryErrorCode,
    pub message: &'static str,
}

/// A normalized planar region and its vertical extrusion parameters.
pub struct ValidatedGeometry {
    pub boundary: Vec<Point2>,
    pub holes: Vec<Vec<Point2>>,
    pub thickness_m: f64,
    elevation_m: f64,
    origin: Point2,
    slope_x: f64,
    slope_y: f64,
}

impl ValidatedGeometry {
    /// Strict planar containment for a hosted cut outline. Contact with an
    /// exterior edge is rejected because it no longer describes an interior cut.
    pub(crate) fn contains_outline(
        &self,
        outline: &[Point2],
        tol: f64,
    ) -> Result<bool, GeometryValidationError> {
        if rings_intersect(&self.boundary, outline, tol)? {
            return Ok(false);
        }
        for point in outline {
            if point_in_ring(*point, &self.boundary, tol)? != PointLocation::Inside {
                return Ok(false);
            }
        }
        for hole in &self.holes {
            if rings_intersect(hole, outline, tol)?
                || point_in_ring(outline[0], hole, tol)? != PointLocation::Outside
                || point_in_ring(hole[0], outline, tol)? != PointLocation::Outside
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn top_elevation_at(&self, point: Point2) -> f64 {
        self.elevation_m
            + self.slope_x * (point.x - self.origin.x)
            + self.slope_y * (point.y - self.origin.y)
    }
}

/// Validate every source shape with the same rules used by display projection.
/// Projection may still fail its triangle budget or numeric mesh calculations.
pub fn validate_element_geometry(
    kind: &ElementKind,
    options: GeometryOptions,
) -> Result<ValidatedGeometry, GeometryValidationError> {
    if let ElementKind::Opening(spec) | ElementKind::Door(spec) | ElementKind::Window(spec) = kind {
        let (sin, cos) = spec.rotation_rad.sin_cos();
        let half = spec.width_m / 2.0;
        let wall = crate::WallSpec {
            start: Point2::new(spec.center.x - half * cos, spec.center.y - half * sin),
            end: Point2::new(spec.center.x + half * cos, spec.center.y + half * sin),
            base_elevation_m: spec.elevation_m,
            height_m: spec.height_m,
            thickness_m: spec.depth_m,
        };
        return validate_element_geometry(&ElementKind::Wall(wall), options);
    }
    validate_options(options)?;
    let tol = options.linear_tolerance_m;
    let (boundary, holes, elevation_m, thickness_m, origin, slope_x, slope_y) = match kind {
        ElementKind::Space(_) => {
            return Err(error(
                GeometryErrorCode::DegenerateContour,
                "semantic Space has no extruded source geometry",
            ));
        }
        ElementKind::PolygonalOpening(spec) => {
            validate_scalar(spec.elevation_m)?;
            validate_scalar(spec.rotation_rad)?;
            validate_point(spec.center)?;
            positive_dimension(spec.depth_m, tol)?;
            let contour = validate_planar_contour(&spec.boundary, options)?;
            let (sin, cos) = spec.rotation_rad.sin_cos();
            let boundary = contour
                .into_iter()
                .map(|p| {
                    Point2::new(
                        spec.center.x + p.x * cos - p.y * sin,
                        spec.center.y + p.x * sin + p.y * cos,
                    )
                })
                .collect();
            (
                boundary,
                Vec::new(),
                spec.elevation_m,
                spec.depth_m,
                spec.center,
                0.0,
                0.0,
            )
        }
        ElementKind::Opening(_) | ElementKind::Door(_) | ElementKind::Window(_) => {
            unreachable!("rectangular element handled above")
        }
        ElementKind::Wall(spec) => {
            if options.max_vertices_per_element < 4 {
                return Err(error(
                    GeometryErrorCode::VertexLimitExceeded,
                    "wall requires four contour vertices",
                ));
            }
            validate_point(spec.start)?;
            validate_point(spec.end)?;
            validate_scalar(spec.base_elevation_m)?;
            positive_dimension(spec.height_m, tol)?;
            positive_dimension(spec.thickness_m, tol)?;
            let dx = spec.end.x - spec.start.x;
            let dy = spec.end.y - spec.start.y;
            let length = dx.hypot(dy);
            if !length.is_finite() {
                return Err(numeric_error());
            }
            if length <= tol {
                return Err(error(
                    GeometryErrorCode::DegenerateLine,
                    "wall baseline is too short",
                ));
            }
            let half = spec.thickness_m / 2.0;
            let ox = -dy / length * half;
            let oy = dx / length * half;
            let ring = [
                Point2::new(spec.start.x - ox, spec.start.y - oy),
                Point2::new(spec.end.x - ox, spec.end.y - oy),
                Point2::new(spec.end.x + ox, spec.end.y + oy),
                Point2::new(spec.start.x + ox, spec.start.y + oy),
            ];
            let top = spec.base_elevation_m + spec.height_m;
            if !top.is_finite() || top <= spec.base_elevation_m {
                return Err(numeric_error());
            }
            let (boundary, holes) = validate_region(&ring, &[], options)?;
            (
                boundary,
                holes,
                top,
                spec.height_m,
                Point2::default(),
                0.0,
                0.0,
            )
        }
        ElementKind::Floor(spec) => {
            validate_scalar(spec.elevation_m)?;
            positive_dimension(spec.thickness_m, tol)?;
            let (boundary, holes) = validate_region(&spec.boundary, &spec.holes, options)?;
            (
                boundary,
                holes,
                spec.elevation_m,
                spec.thickness_m,
                Point2::default(),
                0.0,
                0.0,
            )
        }
        ElementKind::Roof(spec) => {
            validate_scalar(spec.elevation_m)?;
            positive_dimension(spec.thickness_m, tol)?;
            validate_scalar(spec.slope_x)?;
            validate_scalar(spec.slope_y)?;
            let (boundary, holes) = validate_region(&spec.footprint, &[], options)?;
            let count = boundary.len() as f64;
            let origin = Point2::new(
                boundary.iter().map(|point| point.x / count).sum(),
                boundary.iter().map(|point| point.y / count).sum(),
            );
            if !origin.x.is_finite() || !origin.y.is_finite() {
                return Err(numeric_error());
            }
            (
                boundary,
                holes,
                spec.elevation_m,
                spec.thickness_m,
                origin,
                spec.slope_x,
                spec.slope_y,
            )
        }
    };
    let geometry = ValidatedGeometry {
        boundary,
        holes,
        elevation_m,
        thickness_m,
        origin,
        slope_x,
        slope_y,
    };
    for point in geometry
        .boundary
        .iter()
        .chain(geometry.holes.iter().flatten())
    {
        let top = geometry.top_elevation_at(*point);
        let bottom = top - geometry.thickness_m;
        if !top.is_finite() || !bottom.is_finite() || bottom >= top {
            return Err(numeric_error());
        }
    }
    Ok(geometry)
}

/// Shared contour validation for polygonal voids and optional semantic spaces.
/// The original stored ring is preserved; the returned ring is normalized CCW.
pub fn validate_planar_contour(
    contour: &[Point2],
    options: GeometryOptions,
) -> Result<Vec<Point2>, GeometryValidationError> {
    validate_options(options)?;
    validate_region(contour, &[], options).map(|(boundary, _)| boundary)
}

fn validate_options(options: GeometryOptions) -> Result<(), GeometryValidationError> {
    if !options.linear_tolerance_m.is_finite()
        || options.linear_tolerance_m <= 0.0
        || options.max_vertices_per_element < 3
    {
        return Err(error(
            GeometryErrorCode::InvalidOptions,
            "geometry tolerance must be finite and positive, and vertex limit at least three",
        ));
    }
    Ok(())
}

fn validate_region(
    outer_source: &[Point2],
    hole_sources: &[Vec<Point2>],
    options: GeometryOptions,
) -> Result<(Vec<Point2>, Vec<Vec<Point2>>), GeometryValidationError> {
    let total_vertices = hole_sources
        .iter()
        .try_fold(outer_source.len(), |total, hole| {
            total.checked_add(hole.len())
        })
        .ok_or_else(|| {
            error(
                GeometryErrorCode::VertexLimitExceeded,
                "contour vertex count overflowed",
            )
        })?;
    if total_vertices > options.max_vertices_per_element {
        return Err(error(
            GeometryErrorCode::VertexLimitExceeded,
            "contour vertex limit exceeded",
        ));
    }
    let tol = options.linear_tolerance_m;
    let mut outer = validate_ring(outer_source, tol)?;
    let mut holes = Vec::with_capacity(hole_sources.len());
    for source in hole_sources {
        holes.push(validate_ring(source, tol)?);
    }
    for hole in &holes {
        if point_in_ring(hole[0], &outer, tol)? != PointLocation::Inside {
            return Err(error(
                GeometryErrorCode::HoleOutsideBoundary,
                "hole is not strictly inside its floor boundary",
            ));
        }
        if rings_intersect(&outer, hole, tol)? {
            return Err(error(
                GeometryErrorCode::ContoursIntersect,
                "hole touches or intersects its floor boundary",
            ));
        }
    }
    for first in 0..holes.len() {
        for second in first + 1..holes.len() {
            if rings_intersect(&holes[first], &holes[second], tol)?
                || point_in_ring(holes[first][0], &holes[second], tol)? != PointLocation::Outside
                || point_in_ring(holes[second][0], &holes[first], tol)? != PointLocation::Outside
            {
                return Err(error(
                    GeometryErrorCode::ContoursIntersect,
                    "holes touch, overlap, or nest",
                ));
            }
        }
    }
    if signed_area2(&outer) < 0.0 {
        outer.reverse();
    }
    for hole in &mut holes {
        if signed_area2(hole) > 0.0 {
            hole.reverse();
        }
    }
    Ok((outer, holes))
}

fn validate_ring(source: &[Point2], tol: f64) -> Result<Vec<Point2>, GeometryValidationError> {
    if source.len() < 3 {
        return Err(error(
            GeometryErrorCode::ContourTooSmall,
            "contour needs at least three vertices",
        ));
    }
    for &point in source {
        validate_point(point)?;
    }
    let mut ring = source.to_vec();
    if ring.len() > 3 && ring.first() == ring.last() {
        ring.pop();
    }
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        let edge_length = (b.x - a.x).hypot(b.y - a.y);
        if !edge_length.is_finite() {
            return Err(numeric_error());
        }
        if edge_length <= tol {
            return Err(error(
                GeometryErrorCode::DegenerateContour,
                "contour has a collapsed edge",
            ));
        }
        let c = ring[(index + 2) % ring.len()];
        let turn = orientation(a, b, c)?;
        let turn_limit = tol * edge_length;
        let dot = (b.x - a.x) * (c.x - b.x) + (b.y - a.y) * (c.y - b.y);
        if !turn_limit.is_finite() || !dot.is_finite() {
            return Err(numeric_error());
        }
        if turn.abs() <= turn_limit && dot < 0.0 {
            return Err(error(
                GeometryErrorCode::SelfIntersection,
                "contour reverses along an adjacent edge",
            ));
        }
    }
    for first in 0..ring.len() {
        let a = ring[first];
        let b = ring[(first + 1) % ring.len()];
        for second in first + 1..ring.len() {
            if second == first + 1 || (first == 0 && second == ring.len() - 1) {
                continue;
            }
            if segments_intersect(a, b, ring[second], ring[(second + 1) % ring.len()], tol)? {
                return Err(error(
                    GeometryErrorCode::SelfIntersection,
                    "contour self-intersects or touches itself",
                ));
            }
        }
    }
    let area2 = signed_area2(&ring);
    let area_limit = tol * tol;
    if !area2.is_finite() || !area_limit.is_finite() {
        return Err(numeric_error());
    }
    if area2.abs() <= area_limit {
        return Err(error(
            GeometryErrorCode::DegenerateContour,
            "contour has negligible area",
        ));
    }
    Ok(ring)
}

fn signed_area2(ring: &[Point2]) -> f64 {
    let origin = ring[0];
    (1..ring.len() - 1)
        .map(|index| {
            let a = ring[index];
            let b = ring[index + 1];
            (a.x - origin.x) * (b.y - origin.y) - (a.y - origin.y) * (b.x - origin.x)
        })
        .sum()
}

fn rings_intersect(a: &[Point2], b: &[Point2], tol: f64) -> Result<bool, GeometryValidationError> {
    for first in 0..a.len() {
        for second in 0..b.len() {
            if segments_intersect(
                a[first],
                a[(first + 1) % a.len()],
                b[second],
                b[(second + 1) % b.len()],
                tol,
            )? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn segments_intersect(
    a: Point2,
    b: Point2,
    c: Point2,
    d: Point2,
    tol: f64,
) -> Result<bool, GeometryValidationError> {
    let ab_c = orientation(a, b, c)?;
    let ab_d = orientation(a, b, d)?;
    let cd_a = orientation(c, d, a)?;
    let cd_b = orientation(c, d, b)?;
    let ab_epsilon = tol * (b.x - a.x).hypot(b.y - a.y);
    let cd_epsilon = tol * (d.x - c.x).hypot(d.y - c.y);
    if !ab_epsilon.is_finite() || !cd_epsilon.is_finite() {
        return Err(numeric_error());
    }
    if ab_c.abs() <= ab_epsilon && point_on_segment(c, a, b, tol)
        || ab_d.abs() <= ab_epsilon && point_on_segment(d, a, b, tol)
        || cd_a.abs() <= cd_epsilon && point_on_segment(a, c, d, tol)
        || cd_b.abs() <= cd_epsilon && point_on_segment(b, c, d, tol)
    {
        return Ok(true);
    }
    Ok(
        (ab_c > ab_epsilon && ab_d < -ab_epsilon || ab_c < -ab_epsilon && ab_d > ab_epsilon)
            && (cd_a > cd_epsilon && cd_b < -cd_epsilon || cd_a < -cd_epsilon && cd_b > cd_epsilon),
    )
}

fn orientation(a: Point2, b: Point2, c: Point2) -> Result<f64, GeometryValidationError> {
    let value = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(numeric_error())
    }
}

fn point_on_segment(point: Point2, a: Point2, b: Point2, tol: f64) -> bool {
    point.x >= a.x.min(b.x) - tol
        && point.x <= a.x.max(b.x) + tol
        && point.y >= a.y.min(b.y) - tol
        && point.y <= a.y.max(b.y) + tol
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PointLocation {
    Inside,
    Outside,
    Boundary,
}

fn point_in_ring(
    point: Point2,
    ring: &[Point2],
    tol: f64,
) -> Result<PointLocation, GeometryValidationError> {
    let mut inside = false;
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        let epsilon = tol * (b.x - a.x).hypot(b.y - a.y);
        if !epsilon.is_finite() {
            return Err(numeric_error());
        }
        if orientation(a, b, point)?.abs() <= epsilon && point_on_segment(point, a, b, tol) {
            return Ok(PointLocation::Boundary);
        }
        if (a.y > point.y) != (b.y > point.y) {
            let crossing_x = a.x + (b.x - a.x) * ((point.y - a.y) / (b.y - a.y));
            if !crossing_x.is_finite() {
                return Err(numeric_error());
            }
            if crossing_x > point.x {
                inside = !inside;
            }
        }
    }
    Ok(if inside {
        PointLocation::Inside
    } else {
        PointLocation::Outside
    })
}

fn validate_point(point: Point2) -> Result<(), GeometryValidationError> {
    validate_scalar(point.x)?;
    validate_scalar(point.y)
}

fn validate_scalar(value: f64) -> Result<(), GeometryValidationError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(error(
            GeometryErrorCode::NonFinite,
            "geometry contains a nonfinite value",
        ))
    }
}

fn positive_dimension(value: f64, tol: f64) -> Result<(), GeometryValidationError> {
    validate_scalar(value)?;
    if value <= tol {
        Err(error(
            GeometryErrorCode::NonPositiveDimension,
            "dimension must exceed the linear tolerance",
        ))
    } else {
        Ok(())
    }
}

fn numeric_error() -> GeometryValidationError {
    error(
        GeometryErrorCode::NumericRange,
        "geometry exceeds safe numeric range",
    )
}

fn error(code: GeometryErrorCode, message: &'static str) -> GeometryValidationError {
    GeometryValidationError { code, message }
}
