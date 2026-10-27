use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A two-dimensional floating-point vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f64,
    /// Vertical component.
    pub y: f64,
}

impl Vec2 {
    /// Creates a vector from its horizontal and vertical components.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Returns the vector length.
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// A three-dimensional floating-point vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    /// The positive X axis.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// The positive Y axis.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// The positive Z axis.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    /// Creates a vector from its X, Y, and Z components.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Returns the dot product with another vector.
    pub fn dot(self, rhs: Self) -> f64 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    /// Returns the cross product with another vector.
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    /// Returns the squared length.
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    /// Returns the length.
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    /// Returns a unit vector, or zero when this vector is too small to normalize.
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 1.0e-15 {
            self / length
        } else {
            Self::ZERO
        }
    }

    /// Returns this vector with each component clamped to the supplied range.
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self::new(
            self.x.clamp(min.x, max.x),
            self.y.clamp(min.y, max.y),
            self.z.clamp(min.z, max.z),
        )
    }

    /// Returns the component-wise minimum of two vectors.
    pub fn component_min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    /// Returns the component-wise maximum of two vectors.
    pub fn component_max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    /// Returns the component-wise product of two vectors.
    pub fn component_mul(self, rhs: Self) -> Self {
        Self::new(self.x * rhs.x, self.y * rhs.y, self.z * rhs.z)
    }

    /// Returns the component-wise quotient of two vectors.
    pub fn component_div(self, rhs: Self) -> Self {
        Self::new(self.x / rhs.x, self.y / rhs.y, self.z / rhs.z)
    }

    /// Returns true when all components are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Returns the largest component.
    pub fn max_component(self) -> f64 {
        self.x.max(self.y).max(self.z)
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}
impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}
impl Mul<Vec3> for f64 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        rhs * self
    }
}
impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}
impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// A normalized-or-not ray represented by an origin and direction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ray {
    /// Ray origin in the coordinate space where it is used.
    pub origin: Vec3,
    /// Ray direction. Camera rays are normalized; transformed rays may not be.
    pub direction: Vec3,
}

impl Ray {
    /// Creates a ray and normalizes its direction when possible.
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalized(),
        }
    }

    /// Evaluates the ray at parameter `t`.
    pub fn at(self, t: f64) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Creates a bounding box from minimum and maximum corners.
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min: min.component_min(max),
            max: min.component_max(max),
        }
    }

    /// Creates an empty accumulation box.
    pub fn empty() -> Self {
        Self {
            min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    /// Expands this box to contain a point.
    pub fn include_point(&mut self, point: Vec3) {
        self.min = self.min.component_min(point);
        self.max = self.max.component_max(point);
    }

    /// Returns a box containing both input boxes.
    pub fn union(self, rhs: Self) -> Self {
        Self {
            min: self.min.component_min(rhs.min),
            max: self.max.component_max(rhs.max),
        }
    }

    /// Returns the box center.
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Returns the box extent.
    pub fn extent(self) -> Vec3 {
        self.max - self.min
    }

    /// Returns the half-diagonal length.
    pub fn radius(self) -> f64 {
        self.extent().length() * 0.5
    }

    /// Returns whether the box contains no finite point.
    pub fn is_empty(self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// Returns the point at a corner selected by three zero-or-one bits.
    pub fn corner(self, index: usize) -> Vec3 {
        Vec3::new(
            if index & 1 == 0 {
                self.min.x
            } else {
                self.max.x
            },
            if index & 2 == 0 {
                self.min.y
            } else {
                self.max.y
            },
            if index & 4 == 0 {
                self.min.z
            } else {
                self.max.z
            },
        )
    }

    /// Returns the nonnegative entry distance, if a ray intersects this box.
    pub fn ray_entry(self, ray: Ray, max_distance: f64) -> Option<f64> {
        let mut near: f64 = 0.0;
        let mut far = max_distance;
        for (origin, direction, min, max) in [
            (ray.origin.x, ray.direction.x, self.min.x, self.max.x),
            (ray.origin.y, ray.direction.y, self.min.y, self.max.y),
            (ray.origin.z, ray.direction.z, self.min.z, self.max.z),
        ] {
            if direction.abs() < 1.0e-15 {
                if origin < min || origin > max {
                    return None;
                }
            } else {
                let inv = 1.0 / direction;
                let mut a = (min - origin) * inv;
                let mut b = (max - origin) * inv;
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                near = near.max(a);
                far = far.min(b);
                if near > far {
                    return None;
                }
            }
        }
        Some(near)
    }
}

/// A unit quaternion used for object rotation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    /// Scalar component.
    pub w: f64,
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    /// The identity rotation.
    pub const IDENTITY: Self = Self {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Creates a quaternion from components and normalizes it.
    pub fn new(w: f64, x: f64, y: f64, z: f64) -> Self {
        Self { w, x, y, z }.normalized()
    }

    /// Creates a rotation around a unit-or-normalizable axis by radians.
    pub fn from_axis_angle(axis: Vec3, angle_radians: f64) -> Self {
        let axis = axis.normalized();
        if axis == Vec3::ZERO {
            return Self::IDENTITY;
        }
        let half = angle_radians * 0.5;
        let sine = half.sin();
        Self::new(half.cos(), axis.x * sine, axis.y * sine, axis.z * sine)
    }

    /// Returns this quaternion normalized, or identity when it is degenerate.
    pub fn normalized(self) -> Self {
        let length = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if length > 1.0e-15 {
            Self {
                w: self.w / length,
                x: self.x / length,
                y: self.y / length,
                z: self.z / length,
            }
        } else {
            Self::IDENTITY
        }
    }

    /// Returns the conjugate; for a unit quaternion this is its inverse.
    pub fn conjugate(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    /// Rotates a vector by this quaternion.
    pub fn rotate_vec3(self, vector: Vec3) -> Vec3 {
        let q = self.normalized();
        let u = Vec3::new(q.x, q.y, q.z);
        vector + u.cross(vector) * (2.0 * q.w) + u.cross(u.cross(vector)) * 2.0
    }

    /// Rotates a vector by the inverse of this quaternion.
    pub fn inverse_rotate_vec3(self, vector: Vec3) -> Vec3 {
        self.normalized().conjugate().rotate_vec3(vector)
    }
}

impl Mul for Quat {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
            self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
        )
    }
}
