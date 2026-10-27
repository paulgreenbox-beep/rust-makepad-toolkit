use crate::{DraftError, DraftResult};
use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, TAU};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}
impl Point2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
    pub fn offset(self, delta: Self) -> Self {
        Self::new(self.x + delta.x, self.y + delta.y)
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Point3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    fn cross(self, b: Self) -> Self {
        Self::new(
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        )
    }
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingPlane {
    pub origin: Point3,
    pub u_axis: Point3,
    pub v_axis: Point3,
}
impl Default for DrawingPlane {
    fn default() -> Self {
        Self {
            origin: Point3::default(),
            u_axis: Point3::new(1., 0., 0.),
            v_axis: Point3::new(0., 1., 0.),
        }
    }
}
impl DrawingPlane {
    pub fn validate(&self) -> DraftResult<()> {
        if !self.origin.finite()
            || self.origin.x.abs() > 1e12
            || self.origin.y.abs() > 1e12
            || self.origin.z.abs() > 1e12
            || !self.u_axis.finite()
            || !self.v_axis.finite()
            || (self.u_axis.dot(self.u_axis) - 1.).abs() > 1e-9
            || (self.v_axis.dot(self.v_axis) - 1.).abs() > 1e-9
            || self.u_axis.dot(self.v_axis).abs() > 1e-9
        {
            return Err(DraftError::new(
                "invalid_plane",
                "plane must have finite orthonormal axes",
            ));
        }
        Ok(())
    }
    pub fn local_to_world(&self, p: Point2) -> Point3 {
        Point3::new(
            self.origin.x + self.u_axis.x * p.x + self.v_axis.x * p.y,
            self.origin.y + self.u_axis.y * p.x + self.v_axis.y * p.y,
            self.origin.z + self.u_axis.z * p.x + self.v_axis.z * p.y,
        )
    }
    pub fn world_to_local(&self, p: Point3) -> Point2 {
        let d = Point3::new(
            p.x - self.origin.x,
            p.y - self.origin.y,
            p.z - self.origin.z,
        );
        Point2::new(d.dot(self.u_axis), d.dot(self.v_axis))
    }
    pub fn normal(&self) -> Point3 {
        self.u_axis.cross(self.v_axis)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds2 {
    pub min: Point2,
    pub max: Point2,
}
impl Bounds2 {
    pub fn from_points(a: Point2, b: Point2) -> Self {
        Self {
            min: Point2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }
    pub fn union(self, b: Self) -> Self {
        Self {
            min: Point2::new(self.min.x.min(b.min.x), self.min.y.min(b.min.y)),
            max: Point2::new(self.max.x.max(b.max.x), self.max.y.max(b.max.y)),
        }
    }
    pub fn expanded(self, d: f64) -> Self {
        Self {
            min: Point2::new(self.min.x - d, self.min.y - d),
            max: Point2::new(self.max.x + d, self.max.y + d),
        }
    }
    fn rounded_outward(self) -> Self {
        Self {
            min: Point2::new(next_down(self.min.x), next_down(self.min.y)),
            max: Point2::new(next_up(self.max.x), next_up(self.max.y)),
        }
    }
    pub fn intersects(self, b: Self) -> bool {
        self.min.x <= b.max.x
            && self.max.x >= b.min.x
            && self.min.y <= b.max.y
            && self.max.y >= b.min.y
    }
    pub fn center(self) -> Point2 {
        Point2::new(
            self.min.x + (self.max.x - self.min.x) * 0.5,
            self.min.y + (self.max.y - self.min.y) * 0.5,
        )
    }
}
fn next_up(value: f64) -> f64 {
    if value.is_nan() || value == f64::INFINITY {
        return value;
    }
    if value == 0. {
        return f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0. { bits + 1 } else { bits - 1 })
}
fn next_down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0. {
        return f64::from_bits((1_u64 << 63) | 1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0. { bits - 1 } else { bits + 1 })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Line,
    Circle,
    Arc,
    Polyline,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolylineVertex {
    pub point: Point2,
    pub bulge: f64,
}
impl PolylineVertex {
    pub const fn new(point: Point2, bulge: f64) -> Self {
        Self { point, bulge }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntityGeometry {
    Line {
        start: Point2,
        end: Point2,
    },
    Circle {
        center: Point2,
        radius: f64,
    },
    Arc {
        center: Point2,
        radius: f64,
        start_angle: f64,
        sweep_angle: f64,
    },
    Polyline {
        vertices: Vec<PolylineVertex>,
        closed: bool,
    },
}
pub type Geometry = EntityGeometry;
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeometryLimits {
    pub max_vertices: usize,
    pub max_abs_coordinate_m: f64,
    pub linear_tolerance_m: f64,
}
impl Default for GeometryLimits {
    fn default() -> Self {
        Self {
            max_vertices: 4096,
            max_abs_coordinate_m: 1e12,
            linear_tolerance_m: 1e-12,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalyticArc {
    pub center: Point2,
    pub radius: f64,
    pub start_angle: f64,
    pub sweep_angle: f64,
}
impl AnalyticArc {
    /// Unit direction at normalized arc parameter `t`, evaluated without adding
    /// a small sweep to a potentially large start angle.
    pub fn direction(self, t: f64) -> Point2 {
        // Rotate the start direction by the local sweep. Adding a tiny sweep
        // to a very large start angle can round it away before trigonometry.
        let (start_sin, start_cos) = self.start_angle.sin_cos();
        let (delta_sin, delta_cos) = (self.sweep_angle * t).sin_cos();
        Point2::new(
            start_cos * delta_cos - start_sin * delta_sin,
            start_sin * delta_cos + start_cos * delta_sin,
        )
    }
    pub fn point(self, t: f64) -> Point2 {
        let direction = self.direction(t);
        Point2::new(
            self.center.x + self.radius * direction.x,
            self.center.y + self.radius * direction.y,
        )
    }
    /// Evaluate the point after rebasing the center by `origin`.
    pub fn point_relative(self, origin: Point2, t: f64) -> Point2 {
        let x = exact_difference(self.center.x, origin.x);
        let y = exact_difference(self.center.y, origin.y);
        let delta = self.sweep_angle * t;
        if delta.abs() <= 0.125 {
            // Keep cos(delta)-1 separately: cos(delta) itself rounds to one
            // for tiny sweeps, losing the sagitta at a highly zoomed tangent.
            let (start_sin, start_cos) = self.start_angle.sin_cos();
            let half_sin = (delta * 0.5).sin();
            let cos_minus_one = -2. * half_sin * half_sin;
            let delta_sin = delta.sin();
            let change_x = start_cos.mul_add(cos_minus_one, -start_sin * delta_sin);
            let change_y = start_sin.mul_add(cos_minus_one, start_cos * delta_sin);
            Point2::new(
                self.radius.mul_add(change_x, self.radius.mul_add(start_cos, x.0)) + x.1,
                self.radius.mul_add(change_y, self.radius.mul_add(start_sin, y.0)) + y.1,
            )
        } else {
            let direction = self.direction(t);
            Point2::new(
                self.radius.mul_add(direction.x, x.0) + x.1,
                self.radius.mul_add(direction.y, y.0) + y.1,
            )
        }
    }
    fn angular_progress(self, angle: f64) -> f64 {
        let (start_sin, start_cos) = self.start_angle.sin_cos();
        let start_phase = start_sin.atan2(start_cos);
        if self.sweep_angle > 0. {
            (angle - start_phase).rem_euclid(TAU)
        } else {
            (start_phase - angle).rem_euclid(TAU)
        }
    }
    pub fn contains_angle(self, angle: f64) -> bool {
        if self.sweep_angle.abs() >= TAU {
            return true;
        }
        let delta = self.angular_progress(angle);
        let roundoff = 8. * f64::EPSILON * delta.abs().max(self.sweep_angle.abs()).max(1.);
        delta <= self.sweep_angle.abs() + roundoff
    }
    pub fn bounds(self) -> Bounds2 {
        let origin = Point2::default();
        let mut bounds = Bounds2::from_points(
            self.point_relative(origin, 0.0),
            self.point_relative(origin, 1.0),
        );
        for (i, (x, y)) in [(1., 0.), (0., 1.), (-1., 0.), (0., -1.)]
            .into_iter()
            .enumerate()
        {
            let angle = i as f64 * FRAC_PI_2;
            if self.contains_angle(angle) {
                let p = Point2::new(
                    self.center.x + self.radius * x,
                    self.center.y + self.radius * y,
                );
                bounds = bounds.union(Bounds2::from_points(p, p));
            }
        }
        bounds.rounded_outward()
    }
    pub fn distance(self, p: Point2) -> f64 {
        self.distance_relative(Point2::default(), p)
    }
    pub(crate) fn distance_relative(self, origin: Point2, p: Point2) -> f64 {
        let (dx, dy) = query_minus_world_point(origin, p, self.center);
        let (start_sin, start_cos) = self.start_angle.sin_cos();
        let angular_cross = DoubleDouble::product(dy, start_cos)
            .sub(DoubleDouble::product(dx, start_sin));
        let angular_dot = DoubleDouble::product(dx, start_cos)
            .add(DoubleDouble::product(dy, start_sin));
        let signed_angle = angular_cross.value().atan2(angular_dot.value());
        let progress = if self.sweep_angle > 0.0 {
            signed_angle.rem_euclid(TAU)
        } else {
            (-signed_angle).rem_euclid(TAU)
        };
        let sweep = self.sweep_angle.abs();
        if sweep >= TAU {
            radial_distance(dx, dy, self.radius)
        } else {
            let boundary_band = 16.0 * f64::EPSILON * progress.abs().max(sweep).max(1.0);
            let near_start = progress <= boundary_band || TAU - progress <= boundary_band;
            let near_end = (progress - sweep).abs() <= boundary_band;
            let endpoint_distance = |start: Point2, end: Point2| {
                double_double_hypot(
                    DoubleDouble::difference(p.x, start.x),
                    DoubleDouble::difference(p.y, start.y),
                )
                .min(double_double_hypot(
                    DoubleDouble::difference(p.x, end.x),
                    DoubleDouble::difference(p.y, end.y),
                ))
            };

            // atan2 cannot distinguish a sub-ULP angular step at a large
            // radius. Rebase each cross product at its analytic endpoint so
            // the endpoint-side test retains that small local displacement.
            if near_start || near_end {
                let start = self.point_relative(origin, 0.0);
                let end = self.point_relative(origin, 1.0);
                let oriented_cross = |endpoint: Point2, direction: Point2| {
                    let delta_x = DoubleDouble::difference(p.x, endpoint.x);
                    let delta_y = DoubleDouble::difference(p.y, endpoint.y);
                    let cross = DoubleDouble::product(delta_y, direction.x)
                        .sub(DoubleDouble::product(delta_x, direction.y));
                    DoubleDouble::product(cross, self.sweep_angle.signum())
                };
                let start_inside = !near_start
                    || oriented_cross(start, self.direction(0.0))
                        .compare(DoubleDouble::from(0.0))
                        != std::cmp::Ordering::Less;
                let end_inside = !near_end
                    || oriented_cross(end, self.direction(1.0))
                        .compare(DoubleDouble::from(0.0))
                        != std::cmp::Ordering::Greater;
                let inside = match (near_start, near_end) {
                    (true, true) if sweep <= std::f64::consts::PI => {
                        start_inside && end_inside
                    }
                    (true, true) => start_inside || end_inside,
                    (true, false) => start_inside,
                    (false, true) => end_inside,
                    (false, false) => unreachable!(),
                };
                if inside {
                    radial_distance(dx, dy, self.radius)
                } else {
                    endpoint_distance(start, end)
                }
            } else if progress <= sweep {
                radial_distance(dx, dy, self.radius)
            } else {
                endpoint_distance(
                    self.point_relative(origin, 0.0),
                    self.point_relative(origin, 1.0),
                )
            }
        }
    }
}

/// Two-part subtraction retains an origin smaller than the ULP of a far
/// center. The low part is added after cancellation with the curve radius.
fn exact_difference(a: f64, b: f64) -> (f64, f64) {
    let high = a - b;
    let b_virtual = a - high;
    let a_virtual = high + b_virtual;
    (high, (a - a_virtual) + (b_virtual - b))
}

#[derive(Clone, Copy)]
struct DoubleDouble {
    high: f64,
    low: f64,
}

impl DoubleDouble {
    fn from(value: f64) -> Self {
        Self {
            high: value,
            low: 0.0,
        }
    }

    fn difference(a: f64, b: f64) -> Self {
        let (high, low) = exact_difference(a, b);
        Self { high, low }
    }

    fn product(a: Self, b: f64) -> Self {
        let high = a.high * b;
        let high_error = a.high.mul_add(b, -high);
        let low_product = a.low * b;
        let low_error = a.low.mul_add(b, -low_product);
        let (sum, sum_error) = two_sum(high, low_product);
        let (high, low) = two_sum(sum, sum_error + high_error + low_error);
        Self { high, low }
    }

    fn multiply(self, other: Self) -> Self {
        Self::product(self, other.high).add(Self::product(self, other.low))
    }

    fn add(self, other: Self) -> Self {
        let (high_sum, high_error) = two_sum(self.high, other.high);
        let (low_sum, low_error) = two_sum(self.low, other.low);
        let (middle_sum, middle_error) = two_sum(high_error, low_sum);
        let (sum, sum_error) = two_sum(high_sum, middle_sum);
        let tail = sum_error + middle_error + low_error;
        let (high, low) = two_sum(sum, tail);
        Self { high, low }
    }

    fn sub(self, other: Self) -> Self {
        self.add(Self {
            high: -other.high,
            low: -other.low,
        })
    }

    fn value(self) -> f64 {
        self.high + self.low
    }

    fn compare(self, other: Self) -> std::cmp::Ordering {
        let difference = self.sub(other);
        if difference.high < 0.0 || (difference.high == 0.0 && difference.low < 0.0) {
            std::cmp::Ordering::Less
        } else if difference.high > 0.0
            || (difference.high == 0.0 && difference.low > 0.0)
        {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let b_virtual = sum - a;
    let error = (a - (sum - b_virtual)) + (b - b_virtual);
    (sum, error)
}

fn query_minus_world_point(origin: Point2, query: Point2, world: Point2) -> (DoubleDouble, DoubleDouble) {
    (
        DoubleDouble::difference(origin.x, world.x).add(DoubleDouble::from(query.x)),
        DoubleDouble::difference(origin.y, world.y).add(DoubleDouble::from(query.y)),
    )
}

fn double_double_hypot(x: DoubleDouble, y: DoubleDouble) -> f64 {
    x.value().hypot(y.value())
}

fn radial_distance(x: DoubleDouble, y: DoubleDouble, radius: f64) -> f64 {
    let length = double_double_hypot(x, y);
    if radius < 0.0 {
        return (length - radius).abs();
    }
    let squared_difference = DoubleDouble::multiply(x, x)
        .add(DoubleDouble::multiply(y, y))
        .sub(DoubleDouble::product(DoubleDouble::from(radius), radius));
    let denominator = length + radius;
    if denominator == 0.0 {
        (length - radius).abs()
    } else {
        (squared_difference.value() / denominator).abs()
    }
}

fn line_distance_relative(origin: Point2, query: Point2, start: Point2, end: Point2) -> f64 {
    let (query_x, query_y) = query_minus_world_point(origin, query, start);
    let direction_x = DoubleDouble::difference(end.x, start.x);
    let direction_y = DoubleDouble::difference(end.y, start.y);
    let direction_squared = DoubleDouble::multiply(direction_x, direction_x)
        .add(DoubleDouble::multiply(direction_y, direction_y));
    if direction_squared.compare(DoubleDouble::from(0.0)) == std::cmp::Ordering::Equal {
        return double_double_hypot(query_x, query_y);
    }

    let projection = DoubleDouble::multiply(query_x, direction_x)
        .add(DoubleDouble::multiply(query_y, direction_y));
    let from_start = projection.compare(DoubleDouble::from(0.0));
    if from_start != std::cmp::Ordering::Greater {
        return double_double_hypot(query_x, query_y);
    }
    if projection.compare(direction_squared) != std::cmp::Ordering::Less {
        return double_double_hypot(
            query_x.sub(direction_x),
            query_y.sub(direction_y),
        );
    }

    let determinant = DoubleDouble::multiply(direction_x, query_y)
        .sub(DoubleDouble::multiply(direction_y, query_x));
    determinant.value().abs() / direction_squared.value().sqrt()
}

pub fn bulge_arc(start: Point2, end: Point2, bulge: f64) -> DraftResult<Option<AnalyticArc>> {
    if !start.is_finite() || !end.is_finite() || !bulge.is_finite() {
        return Err(DraftError::new(
            "nonfinite_geometry",
            "bulge endpoints and bulge must be finite",
        ));
    }
    if bulge == 0. {
        return Ok(None);
    }
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let chord = dx.hypot(dy);
    if chord <= 1e-12 {
        return Err(DraftError::new(
            "degenerate_geometry",
            "bulge segment has coincident endpoints",
        ));
    }
    // Algebraically stable for small bulges; avoid squaring huge values.
    let factor = (1. / bulge - bulge) * 0.25;
    let center = Point2::new(
        start.x + dx * 0.5 - dy * factor,
        start.y + dy * 0.5 + dx * factor,
    );
    let radius = chord * (bulge.abs() + 1. / bulge.abs()) * 0.25;
    let arc = AnalyticArc {
        center,
        radius,
        start_angle: (start.y - center.y).atan2(start.x - center.x),
        sweep_angle: 4. * bulge.atan(),
    };
    if !center.is_finite() || !radius.is_finite() {
        return Err(DraftError::new(
            "geometry_overflow",
            "bulge conversion overflow",
        ));
    }
    Ok(Some(arc))
}
/// Analytic bulge in its chord frame. Evaluate locally instead of subtracting
/// two radii/center values of order 1/b for an almost flat arc.
#[derive(Clone, Copy, Debug)]
pub struct BulgeArc {
    pub start: Point2,
    pub end: Point2,
    pub bulge: f64,
    pub chord: f64,
    pub unit: Point2,
    pub normal: Point2,
    pub center_height: f64,
    pub radius: f64,
    pub sweep_angle: f64,
}
pub fn bulge_curve(start: Point2, end: Point2, bulge: f64) -> DraftResult<Option<BulgeArc>> {
    let Some(arc) = bulge_arc(start, end, bulge)? else {
        return Ok(None);
    };
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let chord = dx.hypot(dy);
    let unit = Point2::new(dx / chord, dy / chord);
    Ok(Some(BulgeArc {
        start,
        end,
        bulge,
        chord,
        unit,
        normal: Point2::new(-unit.y, unit.x),
        center_height: chord * (1. / bulge - bulge) * 0.25,
        radius: arc.radius,
        sweep_angle: arc.sweep_angle,
    }))
}
impl BulgeArc {
    fn chord_point(self, t: f64) -> Point2 {
        if t == 0. {
            return Point2::default();
        }
        if t == 1. {
            return Point2::new(self.chord, 0.);
        }
        let phi = (t - 0.5) * self.sweep_angle;
        let signed_radius = self.bulge.signum() * self.radius;
        Point2::new(
            self.chord * 0.5 + signed_radius * phi.sin(),
            -self.chord * self.bulge * 0.5 + signed_radius * 2. * (phi * 0.5).sin().powi(2),
        )
    }
    pub fn point_relative(self, origin: Point2, t: f64) -> Point2 {
        if t == 0. {
            return Point2::new(self.start.x - origin.x, self.start.y - origin.y);
        }
        if t == 1. {
            return Point2::new(self.end.x - origin.x, self.end.y - origin.y);
        }
        let p = self.chord_point(t);
        Point2::new(
            (self.start.x - origin.x) + self.unit.x * p.x + self.normal.x * p.y,
            (self.start.y - origin.y) + self.unit.y * p.x + self.normal.y * p.y,
        )
    }
    pub fn point(self, t: f64) -> Point2 {
        self.point_relative(Point2::default(), t)
    }
    pub fn bounds(self) -> Bounds2 {
        let mut bounds = Bounds2::from_points(self.start, self.end);
        for (a, b) in [(self.unit.x, self.normal.x), (self.unit.y, self.normal.y)] {
            let base = (-a).atan2(b);
            for k in -3..=3 {
                let phi = base + k as f64 * std::f64::consts::PI;
                let t = phi / self.sweep_angle + 0.5;
                if (0.0..=1.0).contains(&t) {
                    let p = self.point(t);
                    bounds = bounds.union(Bounds2::from_points(p, p));
                }
            }
        }
        bounds.rounded_outward()
    }
    pub fn distance(self, p: Point2) -> f64 {
        self.distance_relative(Point2::default(), p)
    }
    pub(crate) fn distance_relative(self, origin: Point2, p: Point2) -> f64 {
        // Keep the circle equation in the endpoint frame: for d=end-start,
        // w=query-start-d/2, and f=(1/b-b)/4, its radial numerator is
        // |w|^2 - 2*f*cross(d,w) - |d|^2/4. This avoids rounded chord/unit
        // intermediates when source endpoints are far from the query origin.
        let (query_x, query_y) = query_minus_world_point(origin, p, self.start);
        let chord_x = DoubleDouble::difference(self.end.x, self.start.x);
        let chord_y = DoubleDouble::difference(self.end.y, self.start.y);
        let midpoint_x = DoubleDouble::product(chord_x, 0.5);
        let midpoint_y = DoubleDouble::product(chord_y, 0.5);
        let centered_x = query_x.sub(midpoint_x);
        let centered_y = query_y.sub(midpoint_y);
        let length_squared = DoubleDouble::multiply(chord_x, chord_x)
            .add(DoubleDouble::multiply(chord_y, chord_y));
        let along = DoubleDouble::multiply(centered_x, chord_x)
            .add(DoubleDouble::multiply(centered_y, chord_y));
        let across = DoubleDouble::multiply(chord_x, centered_y)
            .sub(DoubleDouble::multiply(chord_y, centered_x));
        let height_factor = (1.0 / self.bulge - self.bulge) * 0.25;
        let sign = self.bulge.signum();
        let factor_length_squared = DoubleDouble::product(length_squared, height_factor);
        let phi = (sign * along.value())
            .atan2(sign * factor_length_squared.sub(across).value());
        let half_sweep = self.sweep_angle.abs() * 0.5;
        let phi_gap = phi.abs() - half_sweep;
        let boundary_band = 16.0 * f64::EPSILON * phi.abs().max(half_sweep).max(1.0);
        let inside_sweep = if phi_gap.abs() <= boundary_band {
            // atan2 can round a sub-ULP tangent offset back onto an endpoint.
            // The compensated endpoint cross signs distinguish the two sides.
            let half_across = DoubleDouble::product(across, 0.5);
            let half_factor_length_squared =
                DoubleDouble::product(factor_length_squared, 0.5);
            let factor_along = DoubleDouble::product(along, height_factor);
            let start_cross = factor_along
                .add(half_factor_length_squared)
                .sub(half_across);
            let end_cross = factor_along
                .sub(half_factor_length_squared)
                .add(half_across);
            let near_start = phi * sign < 0.0;
            let endpoint_cross = if near_start { start_cross } else { end_cross };
            let oriented_cross = if sign < 0.0 {
                DoubleDouble::from(0.0).sub(endpoint_cross)
            } else {
                endpoint_cross
            };
            let side = oriented_cross.compare(DoubleDouble::from(0.0));
            if near_start {
                side != std::cmp::Ordering::Less
            } else {
                side != std::cmp::Ordering::Greater
            }
        } else {
            phi_gap < 0.0
        };
        if inside_sweep {
            let numerator = DoubleDouble::multiply(centered_x, centered_x)
                .add(DoubleDouble::multiply(centered_y, centered_y))
                .sub(DoubleDouble::product(across, 2.0 * height_factor))
                .sub(DoubleDouble::product(length_squared, 0.25));
            let center_relative_x = centered_x.add(DoubleDouble::product(chord_y, height_factor));
            let center_relative_y = centered_y.sub(DoubleDouble::product(chord_x, height_factor));
            let denominator =
                double_double_hypot(center_relative_x, center_relative_y) + self.radius;
            (numerator.value() / denominator).abs()
        } else {
            let (end_x, end_y) = query_minus_world_point(origin, p, self.end);
            double_double_hypot(query_x, query_y).min(double_double_hypot(end_x, end_y))
        }
    }
}
pub fn line_distance(p: Point2, a: Point2, b: Point2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length2 = dx * dx + dy * dy;
    if length2 == 0. {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length2).clamp(0., 1.);
    p.distance(Point2::new(a.x + t * dx, a.y + t * dy))
}
impl EntityGeometry {
    pub fn kind(&self) -> EntityKind {
        match self {
            Self::Line { .. } => EntityKind::Line,
            Self::Circle { .. } => EntityKind::Circle,
            Self::Arc { .. } => EntityKind::Arc,
            Self::Polyline { .. } => EntityKind::Polyline,
        }
    }
    pub fn validate(&self, limits: GeometryLimits) -> DraftResult<()> {
        let point = |p: Point2| -> DraftResult<()> {
            if !p.is_finite() {
                return Err(DraftError::new(
                    "nonfinite_geometry",
                    "coordinates must be finite",
                ));
            }
            if p.x.abs() > limits.max_abs_coordinate_m || p.y.abs() > limits.max_abs_coordinate_m {
                return Err(DraftError::new(
                    "geometry_budget",
                    "coordinate exceeds bounded domain",
                ));
            }
            Ok(())
        };
        let bound_point = |p: Point2| -> DraftResult<()> {
            if !p.is_finite() {
                return Err(DraftError::new(
                    "nonfinite_geometry",
                    "geometry bounds must be finite",
                ));
            }
            // Bounds round outward by one representable step. Permit that
            // conservative margin at the configured domain edge.
            let conservative_limit = next_up(limits.max_abs_coordinate_m);
            if p.x.abs() > conservative_limit || p.y.abs() > conservative_limit {
                return Err(DraftError::new(
                    "geometry_budget",
                    "geometry bounds exceed bounded domain",
                ));
            }
            Ok(())
        };
        let circle = |center: Point2, radius: f64| -> DraftResult<()> {
            point(center)?;
            if !radius.is_finite() {
                return Err(DraftError::new(
                    "nonfinite_geometry",
                    "radius must be finite",
                ));
            }
            if radius <= limits.linear_tolerance_m {
                return Err(DraftError::new(
                    "degenerate_geometry",
                    "radius must be positive",
                ));
            }
            if radius > limits.max_abs_coordinate_m {
                return Err(DraftError::new(
                    "geometry_budget",
                    "radius exceeds bounded domain",
                ));
            }
            Ok(())
        };
        match self {
            Self::Line { start, end } => {
                point(*start)?;
                point(*end)?;
                if start.distance(*end) <= limits.linear_tolerance_m {
                    return Err(DraftError::new(
                        "degenerate_geometry",
                        "line endpoints coincide",
                    ));
                }
            }
            Self::Circle { center, radius } => circle(*center, *radius)?,
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                circle(*center, *radius)?;
                if !start_angle.is_finite() || !sweep_angle.is_finite() {
                    return Err(DraftError::new(
                        "nonfinite_geometry",
                        "angles must be finite",
                    ));
                }
                if sweep_angle.abs() <= 1e-12
                    || sweep_angle.abs() > TAU + 1e-12
                    || start_angle.abs() > 1e12
                {
                    return Err(DraftError::new(
                        "invalid_arc",
                        "sweep must be nonzero and at most one turn",
                    ));
                }
            }
            Self::Polyline { vertices, closed } => {
                if vertices.len() < 2 {
                    return Err(DraftError::new(
                        "degenerate_geometry",
                        "polyline requires two vertices",
                    ));
                }
                if vertices.len() > limits.max_vertices {
                    return Err(DraftError::new(
                        "geometry_budget",
                        "too many polyline vertices",
                    ));
                }
                for v in vertices {
                    point(v.point)?;
                    if !v.bulge.is_finite() {
                        return Err(DraftError::new(
                            "nonfinite_geometry",
                            "bulge must be finite",
                        ));
                    }
                }
                let count = if *closed {
                    vertices.len()
                } else {
                    vertices.len() - 1
                };
                for i in 0..count {
                    let a = vertices[i];
                    let b = vertices[(i + 1) % vertices.len()];
                    if a.point.distance(b.point) <= limits.linear_tolerance_m {
                        return Err(DraftError::new(
                            "degenerate_geometry",
                            "polyline adjacent vertices coincide",
                        ));
                    }
                    if let Some(arc) = bulge_arc(a.point, b.point, a.bulge)? {
                        // The circle center is a derived construction value and
                        // can lie far outside the drawing domain for a shallow
                        // bulge. Bound the radius here; the actual curve bounds
                        // below enforce the coordinate domain.
                        if arc.radius > limits.max_abs_coordinate_m {
                            return Err(DraftError::new(
                                "geometry_budget",
                                "bulge arc exceeds bounded domain",
                            ));
                        }
                    }
                }
            }
        }
        let bounds = self.bounds()?;
        bound_point(bounds.min)?;
        bound_point(bounds.max)?;
        Ok(())
    }
    pub fn bounds(&self) -> DraftResult<Bounds2> {
        Ok(match self {
            Self::Line { start, end } => Bounds2::from_points(*start, *end),
            Self::Circle { center, radius } => Bounds2::from_points(
                Point2::new(center.x - radius, center.y - radius),
                Point2::new(center.x + radius, center.y + radius),
            )
            .rounded_outward(),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            }
            .bounds(),
            Self::Polyline { vertices, closed } => {
                if vertices.len() < 2 {
                    return Err(DraftError::new(
                        "degenerate_geometry",
                        "polyline requires two vertices",
                    ));
                }
                let mut bounds = Bounds2::from_points(vertices[0].point, vertices[0].point);
                let count = if *closed {
                    vertices.len()
                } else {
                    vertices.len() - 1
                };
                for i in 0..count {
                    let a = vertices[i];
                    let b = vertices[(i + 1) % vertices.len()];
                    bounds = bounds.union(match bulge_curve(a.point, b.point, a.bulge)? {
                        Some(arc) => arc.bounds(),
                        None => Bounds2::from_points(a.point, b.point),
                    });
                }
                bounds
            }
        })
    }
    pub fn translated(&self, delta: Point2) -> Self {
        match self {
            Self::Line { start, end } => Self::Line {
                start: start.offset(delta),
                end: end.offset(delta),
            },
            Self::Circle { center, radius } => Self::Circle {
                center: center.offset(delta),
                radius: *radius,
            },
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Self::Arc {
                center: center.offset(delta),
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            },
            Self::Polyline { vertices, closed } => Self::Polyline {
                vertices: vertices
                    .iter()
                    .map(|v| PolylineVertex {
                        point: v.point.offset(delta),
                        bulge: v.bulge,
                    })
                    .collect(),
                closed: *closed,
            },
        }
    }
    pub fn distance(&self, p: Point2) -> DraftResult<f64> {
        self.distance_relative(Point2::default(), p)
    }
    pub(crate) fn distance_relative(&self, origin: Point2, p: Point2) -> DraftResult<f64> {
        Ok(match self {
            Self::Line { start, end } => line_distance_relative(origin, p, *start, *end),
            Self::Circle { center, radius } => {
                let (dx, dy) = query_minus_world_point(origin, p, *center);
                radial_distance(dx, dy, *radius)
            }
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            }
            .distance_relative(origin, p),
            Self::Polyline { vertices, closed } => {
                if vertices.len() < 2 {
                    return Err(DraftError::new(
                        "degenerate_geometry",
                        "polyline requires two vertices",
                    ));
                }
                let count = if *closed {
                    vertices.len()
                } else {
                    vertices.len() - 1
                };
                let mut distance = f64::INFINITY;
                for i in 0..count {
                    let a = vertices[i];
                    let b = vertices[(i + 1) % vertices.len()];
                    distance = distance.min(match bulge_curve(a.point, b.point, a.bulge)? {
                        Some(arc) => arc.distance_relative(origin, p),
                        None => line_distance_relative(origin, p, a.point, b.point),
                    });
                }
                distance
            }
        })
    }
}
