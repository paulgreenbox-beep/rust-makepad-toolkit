//! Compact, on-demand observations derived from the viewer's render meshes.
//!
//! This module never claims exact CAD geometry. Recognized primitives are fits
//! to the render mesh, mesh output is simplified, and a bounds-only result is
//! explicitly marked as such. Exact BRep operations belong to a separate
//! backend.

use crate::{Aabb, ObjectId, Scene, SceneObject, Transform3, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Requested detail level and its strict observation-wide mesh budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduced3DComplexity {
    /// Compact summary: at most 96 points, 64 triangles, and 8 context objects.
    Low,
    /// Balanced detail: at most 384 points, 256 triangles, and 32 context objects.
    Medium,
    /// Higher detail: at most 1536 points, 1024 triangles, and 96 context objects.
    High,
}

impl Reduced3DComplexity {
    /// Returns the hard limits applied to this observation.
    pub const fn budget(self) -> Reduced3DBudget {
        match self {
            Self::Low => Reduced3DBudget {
                max_points: 96,
                max_triangles: 64,
                max_context_objects: 8,
            },
            Self::Medium => Reduced3DBudget {
                max_points: 384,
                max_triangles: 256,
                max_context_objects: 32,
            },
            Self::High => Reduced3DBudget {
                max_points: 1536,
                max_triangles: 1024,
                max_context_objects: 96,
            },
        }
    }
}

/// Hard limits used while building one reduced-3D observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reduced3DBudget {
    /// Maximum number of discrete mesh/profile/polyline points in the result.
    pub max_points: usize,
    /// Maximum number of simplified mesh triangles in the result.
    pub max_triangles: usize,
    /// Maximum number of nearby context objects; requested objects are not capped.
    pub max_context_objects: usize,
}

/// Whether an output object was requested directly or added as nearby context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduced3DRole {
    Requested,
    Context,
}

/// Quality label for a reduced representation derived from render geometry.
///
/// Every value is explicitly approximate. The current scene has no BRep source
/// from which this API could claim exact geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduced3DQuality {
    /// A recognizable primitive fitted to the render mesh.
    RecognizedPrimitiveApproximation,
    /// A subset of the original render mesh selected within the budget.
    SimplifiedMeshApproximation,
    /// A line chain inferred from degenerate triangles in the render mesh.
    PolylineApproximation,
    /// Only the source mesh bounds are included because no mesh budget remained.
    BoundsOnlyApproximation,
}

/// Aggregate quality label. Neither variant represents exact BRep geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduced3DGeometryQuality {
    ApproximateRenderMesh,
    ApproximateRenderMeshWithBoundsOnlyObjects,
}

/// Why an object contains only its bounds instead of a primitive or mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduced3DBoundsReason {
    MeshBudgetExhausted,
    NoUsableTriangles,
}

/// Compact object-local geometry. Apply the owning object's transform to place
/// all local geometry in world coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "representation", rename_all = "snake_case")]
pub enum Reduced3DRepresentation {
    /// Axis-aligned cuboid recognized from the render mesh.
    Box { center: Vec3, size: Vec3 },
    /// Circular section and axis fitted to a tessellated render mesh.
    Cylinder {
        center: Vec3,
        axis: Vec3,
        radius: f64,
        height: f64,
    },
    /// A planar patch summarized by an oriented rectangular footprint.
    Plane {
        center: Vec3,
        normal: Vec3,
        axis_u: Vec3,
        axis_v: Vec3,
        size: Vec2,
    },
    /// A convex polygonal profile swept along a principal local axis.
    Extrusion {
        origin: Vec3,
        axis: Vec3,
        height: f64,
        profile: Vec<Vec2>,
    },
    /// An ordered chain inferred from degenerate render-mesh triangles.
    Polyline { points: Vec<Vec3>, closed: bool },
    /// A simplified indexed mesh in object-local coordinates.
    Mesh {
        points: Vec<Vec3>,
        triangles: Vec<[u32; 3]>,
    },
    /// Source bounds only. This is not a geometric solid or exact shape.
    BoundsOnly { reason: Reduced3DBoundsReason },
}

/// One requested object or nearby context object in the observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reduced3DObject {
    /// Stable scene object ID, preserved unchanged from the source scene.
    pub object_id: ObjectId,
    /// Whether the ID was supplied by the caller or discovered as context.
    pub role: Reduced3DRole,
    /// World-space transform from object-local representation coordinates.
    pub transform: Transform3,
    /// Source render-mesh bounds in object-local coordinates.
    pub local_bounds: Aabb,
    /// Conservative transformed render-mesh bounds in world coordinates.
    pub world_bounds: Aabb,
    /// AABB distance to the nearest requested object for context entries.
    pub distance_to_requested: Option<f64>,
    /// Explicit statement that this object's representation is approximate.
    pub quality: Reduced3DQuality,
    /// Compact local representation.
    #[serde(flatten)]
    pub representation: Reduced3DRepresentation,
}

/// A bounded 3D observation of explicit objects and their nearby context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reduced3DObservation {
    /// Sorted, deduplicated IDs supplied by the caller.
    pub requested_ids: Vec<ObjectId>,
    /// Context IDs in ascending distance, then stable object-ID order.
    pub context_ids: Vec<ObjectId>,
    /// Requested objects first, followed by nearest context objects.
    pub objects: Vec<Reduced3DObject>,
    /// Maximum AABB separation for context discovery, in scene units.
    pub context_radius: f64,
    /// Complexity setting used to build this result.
    pub complexity: Reduced3DComplexity,
    /// Observation-level quality; no result is labeled exact.
    pub geometry_quality: Reduced3DGeometryQuality,
    /// Total discrete points actually included across the observation.
    pub point_count: usize,
    /// Total simplified triangles actually included across the observation.
    pub triangle_count: usize,
    /// Hard limits used for the actual point and triangle counts.
    pub budget: Reduced3DBudget,
}

/// Input errors for reduced-3D capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reduced3DError {
    /// No explicit object IDs were supplied.
    EmptyObjectIds,
    /// Context radius was negative or non-finite.
    InvalidContextRadius,
    /// An explicit object ID is not registered in the scene.
    UnknownObject(ObjectId),
    /// An explicit object has no finite, non-empty render-mesh bounds.
    ObjectHasNoBounds(ObjectId),
}

impl std::fmt::Display for Reduced3DError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyObjectIds => formatter.write_str("at least one object ID is required"),
            Self::InvalidContextRadius => {
                formatter.write_str("context radius must be finite and nonnegative")
            }
            Self::UnknownObject(object_id) => {
                write!(
                    formatter,
                    "object {object_id} is not registered in the scene"
                )
            }
            Self::ObjectHasNoBounds(object_id) => {
                write!(
                    formatter,
                    "object {object_id} has no finite render-mesh bounds"
                )
            }
        }
    }
}

impl std::error::Error for Reduced3DError {}

/// Captures a compact render-mesh observation for explicit object IDs and
/// nearby visible context. `context_radius` uses scene world units.
///
/// Recognized primitives are approximations fitted to the render mesh. Mesh
/// fallbacks obey the observation-wide point and triangle budgets. Requested
/// IDs are always retained in the result; if their mesh budget is exhausted,
/// the object is returned as explicitly marked bounds-only approximation.
pub fn capture_reduced_3d(
    scene: &Scene,
    object_ids: &[ObjectId],
    context_radius: f64,
    complexity: Reduced3DComplexity,
) -> Result<Reduced3DObservation, Reduced3DError> {
    if object_ids.is_empty() {
        return Err(Reduced3DError::EmptyObjectIds);
    }
    if !context_radius.is_finite() || context_radius < 0.0 {
        return Err(Reduced3DError::InvalidContextRadius);
    }

    let requested_ids: Vec<_> = object_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut requested_bounds = BTreeMap::new();
    for object_id in &requested_ids {
        let object = scene
            .get(*object_id)
            .ok_or(Reduced3DError::UnknownObject(*object_id))?;
        let (local_bounds, world_bounds) =
            object_bounds(object).ok_or(Reduced3DError::ObjectHasNoBounds(*object_id))?;
        requested_bounds.insert(*object_id, world_bounds);
        // Keep the local bounds check in this validation path: malformed
        // source vertices must not leak non-finite coordinates downstream.
        let _ = local_bounds;
    }

    let budget = complexity.budget();
    let mut context_candidates = Vec::new();
    for object in scene.objects() {
        if requested_bounds.contains_key(&object.object_id) || !object.visible {
            continue;
        }
        let Some((_, bounds)) = object_bounds(object) else {
            continue;
        };
        let distance = requested_bounds
            .values()
            .map(|requested| aabb_distance(*requested, bounds))
            .fold(f64::INFINITY, f64::min);
        if distance <= context_radius {
            context_candidates.push((distance, object.object_id));
        }
    }
    context_candidates.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    context_candidates.truncate(budget.max_context_objects);

    let context_ids: Vec<_> = context_candidates.iter().map(|(_, id)| *id).collect();
    let mut remaining_points = budget.max_points;
    let mut remaining_triangles = budget.max_triangles;
    let mut point_count = 0;
    let mut triangle_count = 0;
    let mut contains_bounds_only = false;
    let mut objects = Vec::with_capacity(requested_ids.len() + context_ids.len());

    for object_id in &requested_ids {
        let object = scene
            .get(*object_id)
            .expect("explicit IDs were validated above");
        let (local_bounds, world_bounds) =
            object_bounds(object).expect("explicit bounds were validated above");
        let reduced = reduce_object(
            object,
            Reduced3DRole::Requested,
            local_bounds,
            world_bounds,
            None,
            &mut remaining_points,
            &mut remaining_triangles,
        );
        point_count += representation_point_count(&reduced.representation);
        triangle_count += representation_triangle_count(&reduced.representation);
        contains_bounds_only |= reduced.quality == Reduced3DQuality::BoundsOnlyApproximation;
        objects.push(reduced);
    }

    for (distance, object_id) in context_candidates {
        let object = scene
            .get(object_id)
            .expect("context IDs came from registered scene objects");
        let (local_bounds, world_bounds) =
            object_bounds(object).expect("context bounds were validated during discovery");
        let reduced = reduce_object(
            object,
            Reduced3DRole::Context,
            local_bounds,
            world_bounds,
            Some(distance),
            &mut remaining_points,
            &mut remaining_triangles,
        );
        point_count += representation_point_count(&reduced.representation);
        triangle_count += representation_triangle_count(&reduced.representation);
        contains_bounds_only |= reduced.quality == Reduced3DQuality::BoundsOnlyApproximation;
        objects.push(reduced);
    }

    debug_assert!(point_count <= budget.max_points);
    debug_assert!(triangle_count <= budget.max_triangles);
    Ok(Reduced3DObservation {
        requested_ids,
        context_ids,
        objects,
        context_radius,
        complexity,
        geometry_quality: if contains_bounds_only {
            Reduced3DGeometryQuality::ApproximateRenderMeshWithBoundsOnlyObjects
        } else {
            Reduced3DGeometryQuality::ApproximateRenderMesh
        },
        point_count,
        triangle_count,
        budget,
    })
}

fn reduce_object(
    object: &SceneObject,
    role: Reduced3DRole,
    local_bounds: Aabb,
    world_bounds: Aabb,
    distance_to_requested: Option<f64>,
    remaining_points: &mut usize,
    remaining_triangles: &mut usize,
) -> Reduced3DObject {
    let triangles: Vec<_> = object
        .mesh
        .triangles()
        .iter()
        .copied()
        .filter(|triangle| triangle.vertices.iter().all(|point| point.is_finite()))
        .collect();

    let recognized = recognize_box(&triangles, local_bounds)
        .map(|representation| {
            (
                representation,
                Reduced3DQuality::RecognizedPrimitiveApproximation,
                0,
            )
        })
        .or_else(|| {
            recognize_cylinder(&triangles, local_bounds).map(|representation| {
                (
                    representation,
                    Reduced3DQuality::RecognizedPrimitiveApproximation,
                    0,
                )
            })
        })
        .or_else(|| {
            recognize_extrusion(&triangles, local_bounds, *remaining_points).map(|representation| {
                let points = representation_point_count(&representation);
                (
                    representation,
                    Reduced3DQuality::RecognizedPrimitiveApproximation,
                    points,
                )
            })
        })
        .or_else(|| {
            recognize_plane(&triangles).map(|representation| {
                (
                    representation,
                    Reduced3DQuality::RecognizedPrimitiveApproximation,
                    0,
                )
            })
        })
        .or_else(|| {
            recognize_polyline(&triangles, *remaining_points).map(|representation| {
                let points = representation_point_count(&representation);
                (
                    representation,
                    Reduced3DQuality::PolylineApproximation,
                    points,
                )
            })
        });

    let (representation, quality) = if let Some((representation, quality, point_cost)) = recognized
    {
        *remaining_points -= point_cost;
        (representation, quality)
    } else if let Some(representation) =
        simplify_mesh(&triangles, *remaining_points, *remaining_triangles)
    {
        *remaining_points -= representation_point_count(&representation);
        *remaining_triangles -= representation_triangle_count(&representation);
        (
            representation,
            Reduced3DQuality::SimplifiedMeshApproximation,
        )
    } else {
        let reason = if triangles.is_empty() {
            Reduced3DBoundsReason::NoUsableTriangles
        } else {
            Reduced3DBoundsReason::MeshBudgetExhausted
        };
        (
            Reduced3DRepresentation::BoundsOnly { reason },
            Reduced3DQuality::BoundsOnlyApproximation,
        )
    };

    Reduced3DObject {
        object_id: object.object_id,
        role,
        transform: object.transform,
        local_bounds,
        world_bounds,
        distance_to_requested,
        quality,
        representation,
    }
}

fn object_bounds(object: &SceneObject) -> Option<(Aabb, Aabb)> {
    if !object.transform.is_valid() {
        return None;
    }
    let mut local = Aabb::empty();
    let mut point_count = 0usize;
    for point in object
        .mesh
        .triangles()
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .filter(|point| point.is_finite())
    {
        local.include_point(point);
        point_count += 1;
    }
    if point_count == 0 || local.is_empty() || !aabb_is_finite(local) {
        return None;
    }
    let world = object.transform.transform_bounds(local)?;
    aabb_is_finite(world).then_some((local, world))
}

fn aabb_is_finite(bounds: Aabb) -> bool {
    bounds.min.is_finite() && bounds.max.is_finite() && !bounds.is_empty()
}

fn aabb_distance(left: Aabb, right: Aabb) -> f64 {
    let axis_gap = |left_min: f64, left_max: f64, right_min: f64, right_max: f64| {
        if left_max < right_min {
            right_min - left_max
        } else if right_max < left_min {
            left_min - right_max
        } else {
            0.0
        }
    };
    Vec3::new(
        axis_gap(left.min.x, left.max.x, right.min.x, right.max.x),
        axis_gap(left.min.y, left.max.y, right.min.y, right.max.y),
        axis_gap(left.min.z, left.max.z, right.min.z, right.max.z),
    )
    .length()
}

fn representation_point_count(representation: &Reduced3DRepresentation) -> usize {
    match representation {
        Reduced3DRepresentation::Extrusion { profile, .. } => profile.len(),
        Reduced3DRepresentation::Polyline { points, .. } => points.len(),
        Reduced3DRepresentation::Mesh { points, .. } => points.len(),
        Reduced3DRepresentation::Box { .. }
        | Reduced3DRepresentation::Cylinder { .. }
        | Reduced3DRepresentation::Plane { .. }
        | Reduced3DRepresentation::BoundsOnly { .. } => 0,
    }
}

fn representation_triangle_count(representation: &Reduced3DRepresentation) -> usize {
    match representation {
        Reduced3DRepresentation::Mesh { triangles, .. } => triangles.len(),
        Reduced3DRepresentation::Box { .. }
        | Reduced3DRepresentation::Cylinder { .. }
        | Reduced3DRepresentation::Plane { .. }
        | Reduced3DRepresentation::Extrusion { .. }
        | Reduced3DRepresentation::Polyline { .. }
        | Reduced3DRepresentation::BoundsOnly { .. } => 0,
    }
}

fn extent_tolerance(bounds: Aabb) -> f64 {
    (bounds.extent().length() * 1.0e-8).max(1.0e-12)
}

fn triangle_area(triangle: crate::Triangle) -> f64 {
    (triangle.vertices[1] - triangle.vertices[0])
        .cross(triangle.vertices[2] - triangle.vertices[0])
        .length()
        * 0.5
}

fn recognize_box(triangles: &[crate::Triangle], bounds: Aabb) -> Option<Reduced3DRepresentation> {
    if triangles.len() < 12 {
        return None;
    }
    let size = bounds.extent();
    let tolerance = extent_tolerance(bounds);
    if size.x <= tolerance || size.y <= tolerance || size.z <= tolerance {
        return None;
    }

    let mut faces = [false; 6];
    let mut total_area = 0.0;
    for triangle in triangles {
        let normal = triangle.normal();
        if normal == Vec3::ZERO || !normal.is_finite() {
            return None;
        }
        let components = [normal.x.abs(), normal.y.abs(), normal.z.abs()];
        let axis = if components[0] >= components[1] && components[0] >= components[2] {
            0
        } else if components[1] >= components[2] {
            1
        } else {
            2
        };
        if components[axis] < 1.0 - 1.0e-7 {
            return None;
        }
        let coordinate = |point: Vec3| match axis {
            0 => point.x,
            1 => point.y,
            _ => point.z,
        };
        let value = coordinate(triangle.vertices[0]);
        if triangle
            .vertices
            .iter()
            .any(|point| (coordinate(*point) - value).abs() > tolerance)
        {
            return None;
        }
        let (minimum, maximum) = match axis {
            0 => (bounds.min.x, bounds.max.x),
            1 => (bounds.min.y, bounds.max.y),
            _ => (bounds.min.z, bounds.max.z),
        };
        let side = if (value - minimum).abs() <= tolerance {
            0
        } else if (value - maximum).abs() <= tolerance {
            1
        } else {
            return None;
        };
        faces[axis * 2 + side] = true;
        total_area += triangle_area(*triangle);
        for point in triangle.vertices {
            for (coordinate, minimum, maximum) in [
                (point.x, bounds.min.x, bounds.max.x),
                (point.y, bounds.min.y, bounds.max.y),
                (point.z, bounds.min.z, bounds.max.z),
            ] {
                if (coordinate - minimum).abs() > tolerance
                    && (coordinate - maximum).abs() > tolerance
                {
                    return None;
                }
            }
        }
    }
    let expected_area = 2.0 * (size.x * size.y + size.y * size.z + size.z * size.x);
    if !faces.into_iter().all(|present| present)
        || !total_area.is_finite()
        || (total_area - expected_area).abs() > expected_area * 0.025
    {
        return None;
    }
    Some(Reduced3DRepresentation::Box {
        center: bounds.center(),
        size,
    })
}

fn recognize_cylinder(
    triangles: &[crate::Triangle],
    bounds: Aabb,
) -> Option<Reduced3DRepresentation> {
    if triangles.len() < 8 {
        return None;
    }
    let tolerance = extent_tolerance(bounds);
    let extents = bounds.extent();
    let axes = [
        (Vec3::X, extents.x, bounds.min.x, bounds.max.x, 1, 2),
        (Vec3::Y, extents.y, bounds.min.y, bounds.max.y, 0, 2),
        (Vec3::Z, extents.z, bounds.min.z, bounds.max.z, 0, 1),
    ];
    for (axis, height, axis_min, axis_max, radial_a, radial_b) in axes {
        if height <= tolerance {
            continue;
        }
        let center = bounds.center();
        let coord = |point: Vec3, index: usize| match index {
            0 => point.x,
            1 => point.y,
            _ => point.z,
        };
        let mut radii = Vec::new();
        let mut endpoint_ring_counts = [0usize; 2];
        let mut endpoint_cap_triangles = [0usize; 2];
        let mut side_triangle_count = 0usize;
        let mut valid = true;
        for triangle in triangles {
            let mut in_plane: Option<usize> = None;
            let mut triangle_radii = Vec::with_capacity(3);
            for point in triangle.vertices {
                let along = coord(point, axis_index(axis));
                let at_min = (along - axis_min).abs() <= tolerance;
                let at_max = (along - axis_max).abs() <= tolerance;
                if !at_min && !at_max {
                    valid = false;
                    break;
                }
                if at_min || at_max {
                    in_plane = Some(if at_min { 0 } else { 1 });
                }
                let delta_a = coord(point, radial_a) - coord(center, radial_a);
                let delta_b = coord(point, radial_b) - coord(center, radial_b);
                let radius = (delta_a * delta_a + delta_b * delta_b).sqrt();
                if radius > tolerance {
                    triangle_radii.push(radius);
                }
            }
            if !valid {
                break;
            }
            if triangle
                .vertices
                .iter()
                .all(|point| (coord(*point, axis_index(axis)) - axis_min).abs() <= tolerance)
            {
                endpoint_cap_triangles[0] += 1;
            } else if triangle
                .vertices
                .iter()
                .all(|point| (coord(*point, axis_index(axis)) - axis_max).abs() <= tolerance)
            {
                endpoint_cap_triangles[1] += 1;
            } else {
                side_triangle_count += 1;
            }
            if let Some(endpoint) = in_plane {
                if triangle_radii.len() >= 2 {
                    endpoint_ring_counts[endpoint] += triangle_radii.len();
                }
            }
            radii.extend(triangle_radii);
        }
        if !valid || side_triangle_count < 6 || radii.len() < 12 {
            continue;
        }
        let radius = radii.iter().sum::<f64>() / radii.len() as f64;
        if radius <= tolerance || radius.is_nan() {
            continue;
        }
        let radial_tolerance = (radius * 0.025).max(tolerance * 4.0);
        if radii
            .iter()
            .any(|measured| (*measured - radius).abs() > radial_tolerance)
            || endpoint_cap_triangles.iter().any(|count| *count == 0)
            || endpoint_ring_counts.iter().any(|count| *count < 6)
        {
            continue;
        }
        return Some(Reduced3DRepresentation::Cylinder {
            center,
            axis,
            radius,
            height,
        });
    }
    None
}

fn axis_index(axis: Vec3) -> usize {
    if axis.x != 0.0 {
        0
    } else if axis.y != 0.0 {
        1
    } else {
        2
    }
}

fn recognize_extrusion(
    triangles: &[crate::Triangle],
    bounds: Aabb,
    point_budget: usize,
) -> Option<Reduced3DRepresentation> {
    if triangles.len() < 8 {
        return None;
    }
    let tolerance = extent_tolerance(bounds);
    let extents = bounds.extent();
    for (axis_index, axis, height, minimum, maximum) in [
        (0, Vec3::X, extents.x, bounds.min.x, bounds.max.x),
        (1, Vec3::Y, extents.y, bounds.min.y, bounds.max.y),
        (2, Vec3::Z, extents.z, bounds.min.z, bounds.max.z),
    ] {
        if height <= tolerance {
            continue;
        }
        let (radial_a, radial_b) = match axis_index {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let mut end_points: [BTreeMap<(u64, u64), Vec2>; 2] = [BTreeMap::new(), BTreeMap::new()];
        let mut cap_area = [0.0, 0.0];
        let mut side_edges = Vec::new();
        let mut valid = true;
        for triangle in triangles {
            let axis_coords: Vec<_> = triangle
                .vertices
                .iter()
                .map(|point| coordinate(*point, axis_index))
                .collect();
            let at_min = axis_coords
                .iter()
                .all(|coordinate| (*coordinate - minimum).abs() <= tolerance);
            let at_max = axis_coords
                .iter()
                .all(|coordinate| (*coordinate - maximum).abs() <= tolerance);
            if !at_min && !at_max {
                if axis_coords.iter().any(|coordinate| {
                    (*coordinate - minimum).abs() > tolerance
                        && (*coordinate - maximum).abs() > tolerance
                }) {
                    valid = false;
                    break;
                }
                let mut projected: Vec<_> = triangle
                    .vertices
                    .iter()
                    .map(|point| project_axes(*point, radial_a, radial_b))
                    .collect();
                projected.sort_by_key(|point| vec2_key(*point));
                projected.dedup_by_key(|point| vec2_key(*point));
                if projected.len() != 2 {
                    valid = false;
                    break;
                }
                side_edges.push((vec2_key(projected[0]), vec2_key(projected[1])));
            } else {
                let end = if at_min { 0 } else { 1 };
                let projected: Vec<_> = triangle
                    .vertices
                    .iter()
                    .map(|point| project_axes(*point, radial_a, radial_b))
                    .collect();
                for point in projected {
                    end_points[end].insert(vec2_key(point), point);
                }
                cap_area[end] += triangle_area(*triangle);
            }
        }
        if !valid || side_edges.len() < 3 || end_points[0].len() < 3 {
            continue;
        }
        if end_points[0].keys().ne(end_points[1].keys()) {
            continue;
        }
        let profile = convex_hull(end_points[0].values().copied().collect());
        if profile.len() < 3 || profile.len() > point_budget {
            continue;
        }
        let profile_area = polygon_area(&profile);
        if profile_area <= tolerance * tolerance
            || cap_area
                .iter()
                .any(|area| (*area - profile_area).abs() > profile_area * 0.03)
        {
            continue;
        }
        let profile_keys: Vec<_> = profile.iter().map(|point| vec2_key(*point)).collect();
        if side_edges.iter().any(|(left, right)| {
            !profile_keys.iter().enumerate().any(|(index, key)| {
                (key == left && profile_keys[(index + 1) % profile_keys.len()] == *right)
                    || (key == right && profile_keys[(index + 1) % profile_keys.len()] == *left)
            })
        }) {
            continue;
        }
        let origin = match axis_index {
            0 => Vec3::new(minimum, 0.0, 0.0),
            1 => Vec3::new(0.0, minimum, 0.0),
            _ => Vec3::new(0.0, 0.0, minimum),
        };
        return Some(Reduced3DRepresentation::Extrusion {
            origin,
            axis,
            height,
            profile,
        });
    }
    None
}

fn coordinate(point: Vec3, axis: usize) -> f64 {
    match axis {
        0 => point.x,
        1 => point.y,
        _ => point.z,
    }
}

fn project_axes(point: Vec3, axis_a: usize, axis_b: usize) -> Vec2 {
    Vec2::new(coordinate(point, axis_a), coordinate(point, axis_b))
}

fn vec2_key(point: Vec2) -> (u64, u64) {
    let x = if point.x == 0.0 { 0.0 } else { point.x };
    let y = if point.y == 0.0 { 0.0 } else { point.y };
    (x.to_bits(), y.to_bits())
}

fn convex_hull(mut points: Vec<Vec2>) -> Vec<Vec2> {
    points.sort_by(|left, right| left.x.total_cmp(&right.x).then(left.y.total_cmp(&right.y)));
    points.dedup_by(|left, right| vec2_key(*left) == vec2_key(*right));
    if points.len() <= 2 {
        return points;
    }
    let mut lower = Vec::new();
    for point in &points {
        while lower.len() >= 2
            && cross2(lower[lower.len() - 2], lower[lower.len() - 1], *point) <= 0.0
        {
            lower.pop();
        }
        lower.push(*point);
    }
    let mut upper = Vec::new();
    for point in points.iter().rev() {
        while upper.len() >= 2
            && cross2(upper[upper.len() - 2], upper[upper.len() - 1], *point) <= 0.0
        {
            upper.pop();
        }
        upper.push(*point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross2(origin: Vec2, left: Vec2, right: Vec2) -> f64 {
    (left.x - origin.x) * (right.y - origin.y) - (left.y - origin.y) * (right.x - origin.x)
}

fn polygon_area(points: &[Vec2]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut doubled_area = 0.0;
    for index in 0..points.len() {
        let current = points[index];
        let next = points[(index + 1) % points.len()];
        doubled_area += current.x * next.y - next.x * current.y;
    }
    doubled_area.abs() * 0.5
}

fn recognize_plane(triangles: &[crate::Triangle]) -> Option<Reduced3DRepresentation> {
    let first = triangles
        .iter()
        .copied()
        .find(|triangle| triangle.normal() != Vec3::ZERO)?;
    let normal = first.normal();
    let point = first.vertices[0];
    let diagonal = triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .fold(Aabb::empty(), |mut bounds, vertex| {
            bounds.include_point(vertex);
            bounds
        })
        .extent()
        .length();
    let tolerance = (diagonal * 1.0e-8).max(1.0e-12);
    if triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .any(|vertex| ((vertex - point).dot(normal)).abs() > tolerance)
    {
        return None;
    }

    let raw_u = first.vertices[1] - first.vertices[0];
    let axis_u = (raw_u - normal * raw_u.dot(normal)).normalized();
    if axis_u == Vec3::ZERO {
        return None;
    }
    let axis_v = normal.cross(axis_u).normalized();
    let mut min_u = f64::INFINITY;
    let mut max_u = f64::NEG_INFINITY;
    let mut min_v = f64::INFINITY;
    let mut max_v = f64::NEG_INFINITY;
    for vertex in triangles.iter().flat_map(|triangle| triangle.vertices) {
        let delta = vertex - point;
        let u = delta.dot(axis_u);
        let v = delta.dot(axis_v);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    Some(Reduced3DRepresentation::Plane {
        center: point + axis_u * ((min_u + max_u) * 0.5) + axis_v * ((min_v + max_v) * 0.5),
        normal,
        axis_u,
        axis_v,
        size: Vec2::new(max_u - min_u, max_v - min_v),
    })
}

fn recognize_polyline(
    triangles: &[crate::Triangle],
    point_budget: usize,
) -> Option<Reduced3DRepresentation> {
    if triangles.is_empty() {
        return None;
    }
    let bounds = triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .fold(Aabb::empty(), |mut bounds, point| {
            bounds.include_point(point);
            bounds
        });
    let tolerance = extent_tolerance(bounds);
    let area_tolerance = tolerance * tolerance;
    let mut points_by_key = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for triangle in triangles {
        if triangle_area(*triangle) > area_tolerance {
            return None;
        }
        let vertices = triangle.vertices;
        let mut longest = (0usize, 1usize, (vertices[1] - vertices[0]).length_squared());
        for (left, right) in [(0, 2), (1, 2)] {
            let length = (vertices[right] - vertices[left]).length_squared();
            if length > longest.2 {
                longest = (left, right, length);
            }
        }
        if longest.2 <= tolerance * tolerance {
            continue;
        }
        let left = vertices[longest.0];
        let right = vertices[longest.1];
        let left_key = vec3_key(left);
        let right_key = vec3_key(right);
        if left_key == right_key {
            continue;
        }
        points_by_key.insert(left_key, left);
        points_by_key.insert(right_key, right);
        edges.insert(ordered_edge(left_key, right_key));
    }
    if edges.is_empty() || points_by_key.len() < 2 {
        return None;
    }

    let mut adjacency: BTreeMap<(u64, u64, u64), BTreeSet<(u64, u64, u64)>> = BTreeMap::new();
    for (left, right) in &edges {
        adjacency.entry(*left).or_default().insert(*right);
        adjacency.entry(*right).or_default().insert(*left);
    }
    if adjacency.values().any(|neighbors| neighbors.len() > 2) {
        return None;
    }
    let endpoints: Vec<_> = adjacency
        .iter()
        .filter_map(|(point, neighbors)| (neighbors.len() == 1).then_some(*point))
        .collect();
    let closed = endpoints.is_empty();
    if (!closed && endpoints.len() != 2) || (closed && adjacency.len() < 3) {
        return None;
    }

    let start = endpoints
        .first()
        .copied()
        .unwrap_or_else(|| *adjacency.keys().next().unwrap());
    let mut ordered = vec![start];
    let mut previous = None;
    let mut current = start;
    let mut traversed = BTreeSet::new();
    loop {
        let Some(next) = adjacency[&current].iter().copied().find(|candidate| {
            Some(*candidate) != previous && !traversed.contains(&ordered_edge(current, *candidate))
        }) else {
            break;
        };
        traversed.insert(ordered_edge(current, next));
        if next == start {
            break;
        }
        ordered.push(next);
        previous = Some(current);
        current = next;
    }
    if traversed.len() != edges.len() {
        return None;
    }
    let points: Vec<_> = ordered.iter().map(|key| points_by_key[key]).collect();
    let points = simplify_polyline_points(&points, closed, point_budget)?;
    Some(Reduced3DRepresentation::Polyline { points, closed })
}

fn simplify_polyline_points(points: &[Vec3], closed: bool, budget: usize) -> Option<Vec<Vec3>> {
    let minimum = if closed { 3 } else { 2 };
    if points.len() < minimum || budget < minimum {
        return None;
    }
    if points.len() <= budget {
        return Some(points.to_vec());
    }
    let mut simplified = Vec::with_capacity(budget);
    for slot in 0..budget {
        let index = if closed {
            slot * points.len() / budget
        } else {
            slot * (points.len() - 1) / (budget - 1)
        };
        simplified.push(points[index]);
    }
    Some(simplified)
}

fn vec3_key(point: Vec3) -> (u64, u64, u64) {
    let x = if point.x == 0.0 { 0.0 } else { point.x };
    let y = if point.y == 0.0 { 0.0 } else { point.y };
    let z = if point.z == 0.0 { 0.0 } else { point.z };
    (x.to_bits(), y.to_bits(), z.to_bits())
}

fn ordered_edge(
    left: (u64, u64, u64),
    right: (u64, u64, u64),
) -> ((u64, u64, u64), (u64, u64, u64)) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn simplify_mesh(
    triangles: &[crate::Triangle],
    point_budget: usize,
    triangle_budget: usize,
) -> Option<Reduced3DRepresentation> {
    let target = triangles.len().min(triangle_budget).min(point_budget / 3);
    if target == 0 {
        return None;
    }
    let mut ordered: Vec<_> = triangles.iter().collect();
    ordered.sort_by_key(|triangle| triangle.triangle_id);
    let mut points = Vec::new();
    let mut indices = BTreeMap::new();
    let mut reduced_triangles = Vec::with_capacity(target);
    for sample in 0..target {
        let triangle = ordered[sample * ordered.len() / target];
        let mut triangle_indices = [0u32; 3];
        for (slot, point) in triangle.vertices.iter().enumerate() {
            let key = vec3_key(*point);
            let index = if let Some(index) = indices.get(&key) {
                *index
            } else {
                if points.len() >= point_budget || points.len() >= u32::MAX as usize {
                    return None;
                }
                let index = points.len() as u32;
                points.push(*point);
                indices.insert(key, index);
                index
            };
            triangle_indices[slot] = index;
        }
        reduced_triangles.push(triangle_indices);
    }
    (!reduced_triangles.is_empty()).then_some(Reduced3DRepresentation::Mesh {
        points,
        triangles: reduced_triangles,
    })
}
