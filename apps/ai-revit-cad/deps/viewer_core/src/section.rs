use crate::{Aabb, Ray, Vec3};
use serde::{de, Deserialize, Deserializer, Serialize};

/// World-space limits for the six planes of an axis-aligned section box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SectionBounds {
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: f64,
    pub ymax: f64,
    pub zmin: f64,
    pub zmax: f64,
}

impl SectionBounds {
    /// Creates validated bounds. Every coordinate must be finite and each minimum
    /// must be strictly smaller than its matching maximum.
    pub fn new(
        xmin: f64,
        xmax: f64,
        ymin: f64,
        ymax: f64,
        zmin: f64,
        zmax: f64,
    ) -> Result<Self, SectionBoxError> {
        let bounds = Self {
            xmin,
            xmax,
            ymin,
            ymax,
            zmin,
            zmax,
        };
        if bounds.is_valid() {
            Ok(bounds)
        } else {
            Err(SectionBoxError::InvalidBounds)
        }
    }

    /// Creates bounds from a world-space AABB, adding a 0.5 mm extent on
    /// either side of a degenerate axis so flat geometry remains sectionable.
    pub fn from_aabb(aabb: Aabb) -> Result<Self, SectionBoxError> {
        let mut min = aabb.min;
        let mut max = aabb.max;
        if !min.is_finite() || !max.is_finite() {
            return Err(SectionBoxError::InvalidBounds);
        }
        for (low, high) in [
            (&mut min.x, &mut max.x),
            (&mut min.y, &mut max.y),
            (&mut min.z, &mut max.z),
        ] {
            if *low == *high {
                *low -= 0.0005;
                *high += 0.0005;
            }
        }
        Self::new(min.x, max.x, min.y, max.y, min.z, max.z)
    }

    /// Returns whether these bounds have six finite values and positive extent.
    pub fn is_valid(self) -> bool {
        self.xmin.is_finite()
            && self.xmax.is_finite()
            && self.ymin.is_finite()
            && self.ymax.is_finite()
            && self.zmin.is_finite()
            && self.zmax.is_finite()
            && self.xmin < self.xmax
            && self.ymin < self.ymax
            && self.zmin < self.zmax
    }

    /// Returns the minimum corner as a world-space vector.
    pub const fn min(self) -> Vec3 {
        Vec3::new(self.xmin, self.ymin, self.zmin)
    }

    /// Returns the maximum corner as a world-space vector.
    pub const fn max(self) -> Vec3 {
        Vec3::new(self.xmax, self.ymax, self.zmax)
    }

    /// Returns whether a world-space point is inside or on all six planes.
    pub fn contains(self, point: Vec3) -> bool {
        point.is_finite()
            && point.x >= self.xmin
            && point.x <= self.xmax
            && point.y >= self.ymin
            && point.y <= self.ymax
            && point.z >= self.zmin
            && point.z <= self.zmax
    }

    /// Expands every face outward by a nonnegative distance in scene units.
    pub fn expanded(self, distance: f64) -> Result<Self, SectionBoxError> {
        if !distance.is_finite() || distance < 0.0 {
            return Err(SectionBoxError::InvalidMargin);
        }
        Self::new(
            self.xmin - distance,
            self.xmax + distance,
            self.ymin - distance,
            self.ymax + distance,
            self.zmin - distance,
            self.zmax + distance,
        )
    }

    /// Intersects a normalized world-space ray with the box and returns the
    /// forward parameter interval, clipped to `max_distance`.
    pub(crate) fn ray_interval(self, ray: Ray, max_distance: f64) -> Option<(f64, f64)> {
        if !self.is_valid() || !max_distance.is_finite() || max_distance < 0.0 {
            return None;
        }
        let mut near: f64 = 0.0;
        let mut far = max_distance;
        for (origin, direction, min, max) in [
            (ray.origin.x, ray.direction.x, self.xmin, self.xmax),
            (ray.origin.y, ray.direction.y, self.ymin, self.ymax),
            (ray.origin.z, ray.direction.z, self.zmin, self.zmax),
        ] {
            if direction.abs() < 1.0e-15 {
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }
            let inverse = 1.0 / direction;
            let mut axis_near = (min - origin) * inverse;
            let mut axis_far = (max - origin) * inverse;
            if axis_near > axis_far {
                std::mem::swap(&mut axis_near, &mut axis_far);
            }
            near = near.max(axis_near);
            far = far.min(axis_far);
            if near > far {
                return None;
            }
        }
        (far >= 0.0).then_some((near.max(0.0), far))
    }
}

impl<'de> Deserialize<'de> for SectionBounds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawBounds {
            xmin: f64,
            xmax: f64,
            ymin: f64,
            ymax: f64,
            zmin: f64,
            zmax: f64,
        }

        let raw = RawBounds::deserialize(deserializer)?;
        Self::new(raw.xmin, raw.xmax, raw.ymin, raw.ymax, raw.zmin, raw.zmax)
            .map_err(de::Error::custom)
    }
}

/// The supported Section Box visualization mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionMode {
    /// Keep geometry inside the six planes.
    #[default]
    Inside,
}

/// Serializable visualization-only clipping state for a CAD scene.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionBox {
    /// Whether the bounds currently clip visible geometry.
    pub enabled: bool,
    /// The six world-space clipping planes.
    pub bounds: SectionBounds,
    /// Section operation. The v0.2 core supports `Inside`.
    #[serde(default)]
    pub mode: SectionMode,
}

impl Default for SectionBox {
    fn default() -> Self {
        Self {
            enabled: false,
            bounds: SectionBounds {
                xmin: -1.0,
                xmax: 1.0,
                ymin: -1.0,
                ymax: 1.0,
                zmin: -1.0,
                zmax: 1.0,
            },
            mode: SectionMode::Inside,
        }
    }
}

impl SectionBox {
    /// Returns whether this box is enabled in the supported inside mode.
    pub fn is_active(self) -> bool {
        self.enabled && self.mode == SectionMode::Inside
    }

    /// Enables clipping without changing the current bounds.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disables clipping without changing the current bounds.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Replaces all six bounds after validation.
    pub fn set_bounds(
        &mut self,
        xmin: f64,
        xmax: f64,
        ymin: f64,
        ymax: f64,
        zmin: f64,
        zmax: f64,
    ) -> Result<(), SectionBoxError> {
        self.bounds = SectionBounds::new(xmin, xmax, ymin, ymax, zmin, zmax)?;
        Ok(())
    }

    /// Returns all six current bounds.
    pub fn get_bounds(self) -> SectionBounds {
        self.bounds
    }

    /// Expands the six planes outward by `distance` in scene units.
    pub fn expand(&mut self, distance: f64) -> Result<(), SectionBoxError> {
        self.bounds = self.bounds.expanded(distance)?;
        Ok(())
    }

    /// Resets to a disabled unit box. A Viewer reset uses current model bounds.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Returns whether the box can be used for clipping.
    pub fn is_valid(self) -> bool {
        self.bounds.is_valid() && self.mode == SectionMode::Inside
    }
}

/// Errors from invalid Section Box bounds or expansion distances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionBoxError {
    /// Bounds contain a non-finite coordinate or a minimum is not less than its maximum.
    InvalidBounds,
    /// An expansion distance is negative or non-finite.
    InvalidMargin,
}

impl std::fmt::Display for SectionBoxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBounds => write!(f, "section box bounds must be finite and ordered"),
            Self::InvalidMargin => write!(f, "section box margin must be finite and nonnegative"),
        }
    }
}

impl std::error::Error for SectionBoxError {}
