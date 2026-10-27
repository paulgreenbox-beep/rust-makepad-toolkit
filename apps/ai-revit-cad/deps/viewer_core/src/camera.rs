use crate::{Aabb, Quat, Ray, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Pixel dimensions used by camera projection and navigation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Width in pixels.
    pub width: f64,
    /// Height in pixels.
    pub height: f64,
}

impl Viewport {
    /// Creates a viewport from pixel dimensions.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    /// Returns whether both dimensions are finite and positive.
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }

    /// Returns width divided by height, or zero for an invalid viewport.
    pub fn aspect_ratio(self) -> f64 {
        if self.is_valid() {
            self.width / self.height
        } else {
            0.0
        }
    }
}

/// A perspective camera pose and lens settings suitable for saving as JSON.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    /// Camera position in world coordinates.
    pub position: Vec3,
    /// Camera look-at target in world coordinates.
    pub target: Vec3,
    /// World-space orbit and navigation pivot.
    #[serde(default)]
    pub pivot: Vec3,
    /// Camera up hint in world coordinates.
    pub up: Vec3,
    /// Projection type used by this camera.
    #[serde(default)]
    pub projection: CameraProjection,
    /// Vertical field of view in degrees.
    pub vertical_fov_degrees: f64,
    /// Near clipping distance.
    pub near: f64,
    /// Far clipping distance.
    pub far: f64,
}

/// Projection types supported by the viewer core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraProjection {
    /// Perspective projection with a vertical field of view.
    #[default]
    Perspective,
}

/// A picked surface point used as the focus for cursor-centered dolly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceAnchor {
    /// World-space point where the pointer ray met a visible surface.
    pub point: Vec3,
}

/// A point projected to the viewport, including positive camera-space depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedPoint {
    /// Pixel coordinate with the origin at the viewport's top-left.
    pub screen: Vec2,
    /// Distance along the camera forward vector.
    pub depth: f64,
}

/// Errors returned for invalid camera parameters or projection inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraError {
    /// The viewport width or height is not finite and positive.
    InvalidViewport,
    /// The camera position and target do not define a direction.
    DegenerateView,
    /// Lens values do not describe a valid perspective projection.
    InvalidLens,
    /// The requested dolly scale is not finite and positive.
    InvalidDollyFactor,
    /// A camera state requests a projection not supported by this core.
    UnsupportedProjection,
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport => write!(f, "viewport dimensions must be finite and positive"),
            Self::DegenerateView => write!(f, "camera position and target must differ"),
            Self::InvalidLens => write!(f, "camera lens values are invalid"),
            Self::InvalidDollyFactor => write!(f, "dolly factor must be finite and positive"),
            Self::UnsupportedProjection => write!(f, "camera projection is not supported"),
        }
    }
}

impl std::error::Error for CameraError {}

/// A perspective camera with explicit world-space navigation operations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Camera position in world coordinates.
    pub position: Vec3,
    /// Camera look-at target in world coordinates.
    pub target: Vec3,
    /// World-space orbit and navigation pivot.
    pub pivot: Vec3,
    /// Camera up hint in world coordinates.
    pub up: Vec3,
    /// Projection type used by this camera.
    pub projection: CameraProjection,
    /// Vertical field of view in degrees.
    pub vertical_fov_degrees: f64,
    /// Near clipping distance.
    pub near: f64,
    /// Far clipping distance.
    pub far: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(5.0, 4.0, 7.0),
            target: Vec3::ZERO,
            pivot: Vec3::ZERO,
            up: Vec3::Y,
            projection: CameraProjection::Perspective,
            vertical_fov_degrees: 45.0,
            near: 0.01,
            far: 10_000.0,
        }
    }
}

impl Camera {
    /// Creates and validates a camera from an explicit state.
    pub fn new(state: CameraState) -> Result<Self, CameraError> {
        let camera = Self {
            position: state.position,
            target: state.target,
            pivot: state.pivot,
            up: state.up,
            projection: state.projection,
            vertical_fov_degrees: state.vertical_fov_degrees,
            near: state.near,
            far: state.far,
        };
        camera.validate()?;
        Ok(camera)
    }

    /// Creates a camera from a serialized state.
    pub fn from_state(state: CameraState) -> Result<Self, CameraError> {
        Self::new(state)
    }

    /// Returns the camera pose and lens settings in serializable form.
    pub fn state(self) -> CameraState {
        CameraState {
            position: self.position,
            target: self.target,
            pivot: self.pivot,
            up: self.up,
            projection: self.projection,
            vertical_fov_degrees: self.vertical_fov_degrees,
            near: self.near,
            far: self.far,
        }
    }

    /// Checks that this camera can define a perspective view.
    pub fn validate(self) -> Result<(), CameraError> {
        if self.projection != CameraProjection::Perspective {
            return Err(CameraError::UnsupportedProjection);
        }
        if !self.position.is_finite()
            || !self.target.is_finite()
            || !self.pivot.is_finite()
            || !self.up.is_finite()
            || (self.target - self.position).length() < 1.0e-12
        {
            return Err(CameraError::DegenerateView);
        }
        if !self.vertical_fov_degrees.is_finite()
            || !(1.0..179.0).contains(&self.vertical_fov_degrees)
            || !self.near.is_finite()
            || !self.far.is_finite()
            || self.near <= 0.0
            || self.far <= self.near
        {
            return Err(CameraError::InvalidLens);
        }
        if self.forward().cross(self.up.normalized()).length_squared() < 1.0e-18 {
            return Err(CameraError::DegenerateView);
        }
        Ok(())
    }

    /// Returns the normalized direction from the camera toward its target.
    pub fn forward(self) -> Vec3 {
        (self.target - self.position).normalized()
    }

    /// Returns the normalized screen-right direction.
    pub fn right(self) -> Vec3 {
        self.forward().cross(self.up.normalized()).normalized()
    }

    /// Returns the normalized camera-up direction after resolving the view basis.
    pub fn camera_up(self) -> Vec3 {
        self.right().cross(self.forward()).normalized()
    }

    /// Returns a world-space ray through a top-left-origin pixel coordinate.
    pub fn ray_from_screen(self, screen: Vec2, viewport: Viewport) -> Result<Ray, CameraError> {
        self.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        let ndc_x = 2.0 * screen.x / viewport.width - 1.0;
        let ndc_y = 1.0 - 2.0 * screen.y / viewport.height;
        let half_height = (self.vertical_fov_degrees.to_radians() * 0.5).tan();
        let direction = (self.forward()
            + self.right() * (ndc_x * viewport.aspect_ratio() * half_height)
            + self.camera_up() * (ndc_y * half_height))
            .normalized();
        Ok(Ray::new(self.position, direction))
    }

    /// Projects a world point to pixels. Points behind the camera return `None`.
    pub fn project_point(
        self,
        point: Vec3,
        viewport: Viewport,
    ) -> Result<Option<ProjectedPoint>, CameraError> {
        self.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        let relative = point - self.position;
        let forward = self.forward();
        let depth = relative.dot(forward);
        if depth <= 0.0 {
            return Ok(None);
        }
        let half_height = (self.vertical_fov_degrees.to_radians() * 0.5).tan();
        let ndc_x = relative.dot(self.right()) / (depth * half_height * viewport.aspect_ratio());
        let ndc_y = relative.dot(self.camera_up()) / (depth * half_height);
        Ok(Some(ProjectedPoint {
            screen: Vec2::new(
                (ndc_x + 1.0) * 0.5 * viewport.width,
                (1.0 - ndc_y) * 0.5 * viewport.height,
            ),
            depth,
        }))
    }

    /// Orbits around the current target using yaw and pitch deltas in radians.
    /// Pitch is clamped so the camera does not flip over its up direction.
    pub fn orbit(&mut self, yaw_radians: f64, pitch_radians: f64) -> Result<(), CameraError> {
        self.orbit_about(self.pivot, yaw_radians, pitch_radians)
    }

    /// Orbits camera position and look target around an explicit world-space pivot.
    pub fn orbit_about(
        &mut self,
        pivot: Vec3,
        yaw_radians: f64,
        pitch_radians: f64,
    ) -> Result<(), CameraError> {
        self.validate()?;
        if !yaw_radians.is_finite() || !pitch_radians.is_finite() {
            return Err(CameraError::DegenerateView);
        }
        let world_up = self.up.normalized();
        let yaw = Quat::from_axis_angle(world_up, yaw_radians);
        let mut position_offset = yaw.rotate_vec3(self.position - pivot);
        let mut target_offset = yaw.rotate_vec3(self.target - pivot);
        let current_forward = (target_offset - position_offset).normalized();
        let elevation = current_forward.dot(world_up).clamp(-1.0, 1.0).asin();
        let max_elevation = std::f64::consts::FRAC_PI_2 - 0.01;
        let pitch = (elevation + pitch_radians).clamp(-max_elevation, max_elevation) - elevation;
        let pitch_rotation =
            Quat::from_axis_angle(current_forward.cross(world_up).normalized(), pitch);
        position_offset = pitch_rotation.rotate_vec3(position_offset);
        target_offset = pitch_rotation.rotate_vec3(target_offset);
        self.position = pivot + position_offset;
        self.target = pivot + target_offset;
        self.pivot = pivot;
        self.validate()
    }

    /// Pans the camera and target by a screen-space drag in pixels.
    /// Positive deltas move the viewed content in the same screen direction.
    pub fn pan_pixels(&mut self, delta: Vec2, viewport: Viewport) -> Result<(), CameraError> {
        self.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        let depth = (self.target - self.position).length();
        self.pan_pixels_at_depth(delta, depth, viewport)
    }

    /// Pans at an explicit depth, useful for keeping a picked surface point under the pointer.
    pub fn pan_pixels_at_depth(
        &mut self,
        delta: Vec2,
        depth: f64,
        viewport: Viewport,
    ) -> Result<(), CameraError> {
        self.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        let world_per_pixel =
            2.0 * depth.max(self.near) * (self.vertical_fov_degrees.to_radians() * 0.5).tan()
                / viewport.height;
        let shift = self.right() * (-delta.x * world_per_pixel)
            + self.camera_up() * (delta.y * world_per_pixel);
        self.position += shift;
        self.target += shift;
        self.pivot += shift;
        Ok(())
    }

    /// Dollies by a positive scale. A scale above one moves closer.
    /// When `anchor` is supplied, both camera and target move around that picked
    /// surface point so it stays under the same pointer coordinate.
    pub fn dolly_at(
        &mut self,
        factor: f64,
        anchor: Option<SurfaceAnchor>,
    ) -> Result<(), CameraError> {
        self.validate()?;
        if !factor.is_finite() || factor <= 0.0 {
            return Err(CameraError::InvalidDollyFactor);
        }
        let min_distance = self.near * 2.0;
        let max_distance = (self.far * 0.8).max(min_distance);
        let pivot = anchor.map_or(self.pivot, |anchor| anchor.point);
        let offset = self.position - pivot;
        let distance = offset.length();
        if distance <= 1.0e-12 {
            return Ok(());
        }
        let next_distance = (distance / factor).clamp(min_distance, max_distance);
        let effective_factor = distance / next_distance;
        self.position = pivot + offset / effective_factor;
        self.target = pivot + (self.target - pivot) / effective_factor;
        self.pivot = pivot;
        self.validate()
    }

    /// Fits an axis-aligned scene bound, with an optional clearance multiplier.
    pub fn fit_bounds(
        &mut self,
        bounds: Aabb,
        viewport: Viewport,
        margin: f64,
    ) -> Result<(), CameraError> {
        self.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        if bounds.is_empty() {
            return Ok(());
        }
        let margin = if margin.is_finite() {
            margin.max(1.0)
        } else {
            1.15
        };
        let half_vertical = self.vertical_fov_degrees.to_radians() * 0.5;
        let half_horizontal = (2.0 * (half_vertical.tan() * viewport.aspect_ratio()).atan()) * 0.5;
        let limiting_half_angle = half_vertical.min(half_horizontal).max(0.01);
        let radius = bounds.radius().max(self.near * 2.0);
        let distance = radius / limiting_half_angle.sin() * margin;
        let forward = self.forward();
        self.target = bounds.center();
        self.position = self.target - forward * distance;
        self.pivot = self.target;
        self.validate()
    }
}
