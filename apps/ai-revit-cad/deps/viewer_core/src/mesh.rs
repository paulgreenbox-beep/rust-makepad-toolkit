use crate::{Aabb, Ray, Vec3};
use serde::{Deserialize, Serialize};

const LEAF_TRIANGLE_COUNT: usize = 4;
const HIT_EPSILON: f64 = 1.0e-10;

/// One indexed triangle with a stable ID within its owning object.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Triangle {
    /// Stable triangle identifier chosen by the mesh producer.
    pub triangle_id: u32,
    /// Triangle vertices in object-local coordinates, in winding order.
    pub vertices: [Vec3; 3],
}

impl Triangle {
    /// Creates a triangle from its ID and object-local vertices.
    pub const fn new(triangle_id: u32, a: Vec3, b: Vec3, c: Vec3) -> Self {
        Self {
            triangle_id,
            vertices: [a, b, c],
        }
    }

    /// Returns the geometric normal in object-local coordinates.
    pub fn normal(self) -> Vec3 {
        (self.vertices[1] - self.vertices[0])
            .cross(self.vertices[2] - self.vertices[0])
            .normalized()
    }

    /// Returns the triangle's axis-aligned bounds.
    pub fn bounds(self) -> Aabb {
        let mut bounds = Aabb::empty();
        for vertex in self.vertices {
            bounds.include_point(vertex);
        }
        bounds
    }

    fn centroid(self) -> Vec3 {
        (self.vertices[0] + self.vertices[1] + self.vertices[2]) / 3.0
    }
}

/// A triangle hit expressed in world coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickHit {
    /// Stable scene object identifier.
    pub object_id: u64,
    /// Stable triangle identifier within the object.
    pub triangle_id: u32,
    /// World-space intersection position.
    pub world_point: Vec3,
    /// Normalized world-space geometric normal following triangle winding.
    pub world_normal: Vec3,
    /// Distance from the ray origin in world units.
    pub distance: f64,
}

#[derive(Debug)]
enum BvhKind {
    Leaf(Box<[usize]>),
    Branch(Box<BvhNode>, Box<BvhNode>),
}

#[derive(Debug)]
struct BvhNode {
    bounds: Aabb,
    kind: BvhKind,
}

/// A triangle mesh with a bounding volume hierarchy for ray picking.
#[derive(Debug)]
pub struct Mesh {
    triangles: Vec<Triangle>,
    root: Option<BvhNode>,
    bounds: Option<Aabb>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LocalHit {
    pub triangle_id: u32,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance_parameter: f64,
}

impl Mesh {
    /// Builds a mesh and its BVH from triangles in object-local coordinates.
    pub fn new(triangles: Vec<Triangle>) -> Self {
        let bounds = triangles
            .iter()
            .map(|triangle| triangle.bounds())
            .reduce(Aabb::union);
        let root = if triangles.is_empty() {
            None
        } else {
            // Reuse triangle centers across BVH partitions so recursive sorts
            // do not recalculate the same values for every comparison.
            let centroids: Vec<Vec3> = triangles
                .iter()
                .map(|triangle| triangle.centroid())
                .collect();
            let mut indices: Vec<usize> = (0..triangles.len()).collect();
            Some(Self::build_node(&triangles, &centroids, &mut indices))
        };
        Self {
            triangles,
            root,
            bounds,
        }
    }

    /// Creates a centered, axis-aligned cuboid with twelve outward-wound triangles.
    pub fn cuboid(size: Vec3) -> Self {
        let half = Vec3::new(size.x.abs(), size.y.abs(), size.z.abs()) * 0.5;
        let faces = [
            [
                Vec3::new(half.x, -half.y, -half.z),
                Vec3::new(half.x, half.y, -half.z),
                Vec3::new(half.x, half.y, half.z),
                Vec3::new(half.x, -half.y, half.z),
            ],
            [
                Vec3::new(-half.x, -half.y, half.z),
                Vec3::new(-half.x, half.y, half.z),
                Vec3::new(-half.x, half.y, -half.z),
                Vec3::new(-half.x, -half.y, -half.z),
            ],
            [
                Vec3::new(-half.x, half.y, -half.z),
                Vec3::new(-half.x, half.y, half.z),
                Vec3::new(half.x, half.y, half.z),
                Vec3::new(half.x, half.y, -half.z),
            ],
            [
                Vec3::new(-half.x, -half.y, half.z),
                Vec3::new(-half.x, -half.y, -half.z),
                Vec3::new(half.x, -half.y, -half.z),
                Vec3::new(half.x, -half.y, half.z),
            ],
            [
                Vec3::new(-half.x, -half.y, half.z),
                Vec3::new(half.x, -half.y, half.z),
                Vec3::new(half.x, half.y, half.z),
                Vec3::new(-half.x, half.y, half.z),
            ],
            [
                Vec3::new(half.x, -half.y, -half.z),
                Vec3::new(-half.x, -half.y, -half.z),
                Vec3::new(-half.x, half.y, -half.z),
                Vec3::new(half.x, half.y, -half.z),
            ],
        ];
        let mut triangles = Vec::with_capacity(12);
        for (face_index, face) in faces.into_iter().enumerate() {
            let base_id = (face_index * 2 + 1) as u32;
            triangles.push(Triangle::new(base_id, face[0], face[1], face[2]));
            triangles.push(Triangle::new(base_id + 1, face[0], face[2], face[3]));
        }
        Self::new(triangles)
    }

    /// Returns the source triangles in their input order.
    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// Returns the object-local bounds, if this mesh has any triangles.
    pub fn bounds(&self) -> Option<Aabb> {
        self.bounds
    }

    /// Picks the closest triangle hit by a world-space ray for an identity-transformed mesh.
    pub fn pick(&self, object_id: u64, ray: Ray, max_distance: f64) -> Option<PickHit> {
        let ray = Ray::new(ray.origin, ray.direction);
        let local = self.pick_local(ray, max_distance)?;
        Some(PickHit {
            object_id,
            triangle_id: local.triangle_id,
            world_point: local.point,
            world_normal: local.normal,
            distance: local.distance_parameter,
        })
    }

    pub(crate) fn pick_local(&self, ray: Ray, max_distance: f64) -> Option<LocalHit> {
        self.pick_local_in_range(ray, 0.0, max_distance)
    }

    pub(crate) fn pick_local_in_range(
        &self,
        ray: Ray,
        min_distance: f64,
        max_distance: f64,
    ) -> Option<LocalHit> {
        let mut closest = max_distance;
        let mut best: Option<LocalHit> = None;
        let mut stack = Vec::new();
        if let Some(root) = &self.root {
            stack.push(root);
        }
        while let Some(node) = stack.pop() {
            if node.bounds.ray_entry(ray, closest).is_none() {
                continue;
            }
            match &node.kind {
                BvhKind::Leaf(indices) => {
                    for &index in indices.iter() {
                        let triangle = self.triangles[index];
                        if let Some(distance) = intersect_triangle(ray, triangle) {
                            if distance > max_distance || distance < min_distance {
                                continue;
                            }
                            let is_closer = distance < closest - HIT_EPSILON;
                            let is_stable_tie = (distance - closest).abs() <= HIT_EPSILON
                                && best.map_or(true, |hit| triangle.triangle_id < hit.triangle_id);
                            if is_closer || is_stable_tie {
                                closest = distance;
                                best = Some(LocalHit {
                                    triangle_id: triangle.triangle_id,
                                    point: ray.at(distance),
                                    normal: triangle.normal(),
                                    distance_parameter: distance,
                                });
                            }
                        }
                    }
                }
                BvhKind::Branch(left, right) => {
                    let left_entry = left.bounds.ray_entry(ray, closest);
                    let right_entry = right.bounds.ray_entry(ray, closest);
                    match (left_entry, right_entry) {
                        (Some(a), Some(b)) if a <= b => {
                            stack.push(right);
                            stack.push(left);
                        }
                        (Some(_), Some(_)) => {
                            stack.push(left);
                            stack.push(right);
                        }
                        (Some(_), None) => stack.push(left),
                        (None, Some(_)) => stack.push(right),
                        (None, None) => {}
                    }
                }
            }
        }
        best
    }

    fn build_node(triangles: &[Triangle], centroids: &[Vec3], indices: &mut [usize]) -> BvhNode {
        let bounds = indices
            .iter()
            .map(|&index| triangles[index].bounds())
            .reduce(Aabb::union)
            .expect("nonempty BVH partition");
        if indices.len() <= LEAF_TRIANGLE_COUNT {
            return BvhNode {
                bounds,
                kind: BvhKind::Leaf(indices.to_vec().into_boxed_slice()),
            };
        }
        let extent = bounds.extent();
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let middle = indices.len() / 2;
        indices.select_nth_unstable_by(middle, |&a, &b| {
            component(centroids[a], axis)
                .total_cmp(&component(centroids[b], axis))
                .then_with(|| triangles[a].triangle_id.cmp(&triangles[b].triangle_id))
                .then_with(|| a.cmp(&b))
        });
        let (left_indices, right_indices) = indices.split_at_mut(middle);
        let left = Box::new(Self::build_node(triangles, centroids, left_indices));
        let right = Box::new(Self::build_node(triangles, centroids, right_indices));
        BvhNode {
            bounds,
            kind: BvhKind::Branch(left, right),
        }
    }
}

fn component(vector: Vec3, axis: usize) -> f64 {
    match axis {
        0 => vector.x,
        1 => vector.y,
        _ => vector.z,
    }
}

fn intersect_triangle(ray: Ray, triangle: Triangle) -> Option<f64> {
    let [a, b, c] = triangle.vertices;
    let edge1 = b - a;
    let edge2 = c - a;
    let p = ray.direction.cross(edge2);
    let determinant = edge1.dot(p);
    if determinant.abs() < 1.0e-14 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let from_a = ray.origin - a;
    let u = from_a.dot(p) * inverse;
    if !(-HIT_EPSILON..=1.0 + HIT_EPSILON).contains(&u) {
        return None;
    }
    let q = from_a.cross(edge1);
    let v = ray.direction.dot(q) * inverse;
    if v < -HIT_EPSILON || u + v > 1.0 + HIT_EPSILON {
        return None;
    }
    let distance = edge2.dot(q) * inverse;
    if distance >= 0.0 {
        Some(distance)
    } else {
        None
    }
}
