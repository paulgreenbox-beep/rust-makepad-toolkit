//! Broad-phase, world-space collision candidates based on object AABBs.
//!
//! An AABB overlap only identifies a possible interference. It does not prove
//! that the underlying render meshes intersect, so this module never reports a
//! collision as confirmed. A future narrow-phase backend can replace the AABB
//! method without changing object identity or the collision status contract.

use crate::{Aabb, ObjectId, Scene, Vec3};
use serde::{Deserialize, Serialize};

/// Broad-phase result status. AABB detection currently emits candidates and
/// clearance warnings only; exact confirmation belongs to a narrow phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionStatus {
    /// World-space object bounds overlap; mesh-level intersection is unverified.
    Candidate,
    /// A narrow-phase backend has verified an actual geometry intersection.
    Confirmed,
    /// Bounds do not overlap, but their minimum distance is within a tolerance.
    ClearanceWarning,
}

/// Method used to produce a collision result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionMethod {
    /// Conservative axis-aligned world bounds.
    Aabb,
    /// Reserved for a future exact boundary-representation narrow phase.
    ExactBrep,
}

/// High-level severity suitable for a compact collision summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionSeverity {
    /// The AABB candidate may be an interference; exact geometry is unverified.
    PotentialInterference,
    /// Object bounds are separated but closer than the requested clearance.
    ClearanceWarning,
}

/// Compact, text-friendly collision identity and approximate location.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollisionSummary {
    /// Stable identifier derived from the sorted object ID pair.
    pub collision_id: String,
    /// The stable object IDs in ascending order.
    pub objects: [ObjectId; 2],
    /// Approximate world-space location derived from bounds only.
    pub location: Vec3,
    /// Indicates possible interference rather than exact confirmation.
    pub severity: CollisionSeverity,
}

/// A broad-phase collision result for a stable pair of scene objects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collision {
    /// Stable for a pair of stable object IDs and independent of display order.
    pub collision_id: String,
    /// Lower of the two stable object IDs.
    pub object_a: ObjectId,
    /// Higher of the two stable object IDs.
    pub object_b: ObjectId,
    /// AABB results remain candidates and never imply exact mesh intersection.
    pub status: CollisionStatus,
    /// Detection method used for this result.
    pub method: CollisionMethod,
    /// Overlapping bounds for a candidate; None for a clearance warning.
    pub intersection_bounds: Option<Aabb>,
    /// AABB intersection volume in scene units cubed; zero for touching bounds.
    pub intersection_volume: Option<f64>,
    /// Approximate world-space location derived from bounds only.
    pub location: Vec3,
    /// Conservative severity that preserves the AABB-only limitation.
    pub severity: CollisionSeverity,
}

impl Collision {
    /// Returns a compact summary without exposing intersection geometry.
    pub fn summary(&self) -> CollisionSummary {
        CollisionSummary {
            collision_id: self.collision_id.clone(),
            objects: [self.object_a, self.object_b],
            location: self.location,
            severity: self.severity,
        }
    }
}

/// Returns object pairs whose world-space AABBs overlap.
///
/// The returned Candidate status is intentionally conservative: overlapping
/// AABBs do not prove that the source meshes intersect. Registered objects are
/// considered regardless of display visibility, since hiding an object does
/// not change its physical position in the scene. Results are ordered by the
/// stable object ID pair.
pub fn detect_aabb_collisions(scene: &Scene) -> Vec<Collision> {
    detect_aabb_collisions_with_clearance(scene, 0.0)
}

/// Returns AABB overlap candidates plus disjoint pairs within clearance.
///
/// Clearance is measured in scene units. A negative or non-finite tolerance is
/// treated as zero. A clearance warning has no intersection bounds or volume.
pub fn detect_aabb_collisions_with_clearance(scene: &Scene, clearance: f64) -> Vec<Collision> {
    let clearance = if clearance.is_finite() && clearance >= 0.0 {
        clearance
    } else {
        0.0
    };
    let objects: Vec<_> = scene
        .objects()
        .filter_map(|object| {
            object
                .world_bounds()
                .map(|bounds| (object.object_id, bounds))
        })
        .collect();

    let mut collisions = Vec::new();
    for left_index in 0..objects.len() {
        for right_index in (left_index + 1)..objects.len() {
            let (object_a, bounds_a) = objects[left_index];
            let (object_b, bounds_b) = objects[right_index];
            if let Some(intersection_bounds) = intersection(bounds_a, bounds_b) {
                let extent = intersection_bounds.extent();
                let volume = extent.x.max(0.0) * extent.y.max(0.0) * extent.z.max(0.0);
                collisions.push(Collision {
                    collision_id: collision_id(object_a, object_b),
                    object_a,
                    object_b,
                    status: CollisionStatus::Candidate,
                    method: CollisionMethod::Aabb,
                    intersection_bounds: Some(intersection_bounds),
                    intersection_volume: Some(volume),
                    location: intersection_bounds.center(),
                    severity: CollisionSeverity::PotentialInterference,
                });
                continue;
            }

            if clearance > 0.0 {
                let (distance, location) = bounds_distance_and_midpoint(bounds_a, bounds_b);
                if distance <= clearance {
                    collisions.push(Collision {
                        collision_id: collision_id(object_a, object_b),
                        object_a,
                        object_b,
                        status: CollisionStatus::ClearanceWarning,
                        method: CollisionMethod::Aabb,
                        intersection_bounds: None,
                        intersection_volume: None,
                        location,
                        severity: CollisionSeverity::ClearanceWarning,
                    });
                }
            }
        }
    }
    collisions
}

fn collision_id(object_a: ObjectId, object_b: ObjectId) -> String {
    let (first, second) = if object_a <= object_b {
        (object_a, object_b)
    } else {
        (object_b, object_a)
    };
    format!("COL-{}-{}", first, second)
}

fn intersection(a: Aabb, b: Aabb) -> Option<Aabb> {
    let min = a.min.component_max(b.min);
    let max = a.max.component_min(b.max);
    (min.x <= max.x && min.y <= max.y && min.z <= max.z).then_some(Aabb { min, max })
}

fn bounds_distance_and_midpoint(a: Aabb, b: Aabb) -> (f64, Vec3) {
    let (a_x, b_x) = closest_axis_points(a.min.x, a.max.x, b.min.x, b.max.x);
    let (a_y, b_y) = closest_axis_points(a.min.y, a.max.y, b.min.y, b.max.y);
    let (a_z, b_z) = closest_axis_points(a.min.z, a.max.z, b.min.z, b.max.z);
    let point_a = Vec3::new(a_x, a_y, a_z);
    let point_b = Vec3::new(b_x, b_y, b_z);
    ((point_b - point_a).length(), (point_a + point_b) * 0.5)
}

fn closest_axis_points(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> (f64, f64) {
    if a_max < b_min {
        (a_max, b_min)
    } else if b_max < a_min {
        (a_min, b_max)
    } else {
        let shared_midpoint = (a_min.max(b_min) + a_max.min(b_max)) * 0.5;
        (shared_midpoint, shared_midpoint)
    }
}
