use crate::{Aabb, Mesh, PickHit, Quat, Ray, SectionBounds, Vec3};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Stable identifier for a scene object.
pub type ObjectId = u64;

/// Translation, quaternion rotation, and nonuniform scale for a scene object.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform3 {
    /// World-space translation.
    pub translation: Vec3,
    /// Object-local to world-space rotation.
    pub rotation: Quat,
    /// Per-axis scale. Components must be finite and nonzero for picking.
    pub scale: Vec3,
}

impl Default for Transform3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform3 {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::new(1.0, 1.0, 1.0),
    };

    /// Creates a transform from translation, rotation, and scale.
    pub fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation: rotation.normalized(),
            scale,
        }
    }

    /// Transforms an object-local point to world space.
    pub fn transform_point(self, point: Vec3) -> Vec3 {
        self.translation + self.rotation.rotate_vec3(point.component_mul(self.scale))
    }

    /// Transforms an object-local direction to world space.
    pub fn transform_vector(self, vector: Vec3) -> Vec3 {
        self.rotation.rotate_vec3(vector.component_mul(self.scale))
    }

    /// Transforms a world-space point to object-local space.
    pub fn inverse_transform_point(self, point: Vec3) -> Option<Vec3> {
        self.inverse_transform_vector(point - self.translation)
    }

    /// Transforms a world-space direction to object-local space.
    pub fn inverse_transform_vector(self, vector: Vec3) -> Option<Vec3> {
        if !self.is_valid() {
            return None;
        }
        let local = self.rotation.inverse_rotate_vec3(vector);
        Some(local.component_div(self.scale))
    }

    /// Transforms a local normal by the inverse transpose of this transform.
    pub fn transform_normal(self, normal: Vec3) -> Option<Vec3> {
        if !self.is_valid() {
            return None;
        }
        let local = normal.component_div(self.scale);
        let orientation = if self.scale.x * self.scale.y * self.scale.z < 0.0 {
            -1.0
        } else {
            1.0
        };
        Some((self.rotation.rotate_vec3(local) * orientation).normalized())
    }

    /// Returns whether all components are finite and scale is invertible.
    pub fn is_valid(self) -> bool {
        self.translation.is_finite()
            && self.scale.is_finite()
            && self.scale.x.abs() > 1.0e-15
            && self.scale.y.abs() > 1.0e-15
            && self.scale.z.abs() > 1.0e-15
    }

    /// Transforms an axis-aligned local bound to a conservative world bound.
    pub fn transform_bounds(self, bounds: Aabb) -> Option<Aabb> {
        if bounds.is_empty() || !self.is_valid() {
            return None;
        }
        let mut result = Aabb::empty();
        for index in 0..8 {
            result.include_point(self.transform_point(bounds.corner(index)));
        }
        Some(result)
    }
}

/// One local axis-aligned box used to describe a unioned analytic fixture.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExactBoxPart {
    pub min: Vec3,
    pub size: Vec3,
}
/// Stable source reference for on-demand BRep queries. The render mesh remains
/// independent; this metadata never enters the frame render path.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExactGeometrySource {
    StepSolid {
        file: String,
        sha256: String,
        fingerprint: String,
        shape_index_hint: usize,
        frame_revision: String,
    },
    /// A named Part::Feature stored in an FCStd archive. The stable scene ID
    /// resolves through the reviewed index; ZIP member order is irrelevant.
    FcstdFeature {
        file: String,
        sha256: String,
        feature_name: String,
        archive_member: String,
        member_sha256: String,
        fingerprint: String,
        frame_revision: String,
    },
    Box {
        size: Vec3,
    },
    Sphere {
        radius: f64,
    },
    UnionBoxes {
        parts: Vec<ExactBoxPart>,
    },
}

/// A source revision changes whenever the underlying CAD shape changes.
/// An exact query cache also includes the current object transform.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExactGeometryRef {
    pub source: ExactGeometrySource,
    pub revision: String,
}
/// A renderable and pickable object registered in a [`Scene`].
#[derive(Debug)]
pub struct SceneObject {
    /// Stable identifier for semantic selection and picking.
    pub object_id: ObjectId,
    /// Display name supplied by the model or application.
    pub name: String,
    /// Object-local triangle mesh.
    pub mesh: Mesh,
    /// Optional OpenCascade source resolved lazily by the exact backend.
    pub exact_geometry: Option<ExactGeometryRef>,
    /// Object-to-world transform.
    pub transform: Transform3,
    /// Whether the object is included in rendering and picking.
    pub visible: bool,
    /// Whether the object belongs to the current selection.
    pub selected: bool,
    /// Whether the object has a transient hover or focus highlight.
    pub highlighted: bool,
    /// Linear RGBA display color, with each component normally in `0..=1`.
    pub color: [f32; 4],
    /// Optional renderer texture projected in object-local planar coordinates.
    pub render_texture: Option<RenderTexture>,
}

/// A PNG base-color image for a renderer-backed object.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderTexture {
    /// Asset path supplied by the scene loader.
    pub path: PathBuf,
    /// World-space metres covered by one complete texture repeat.
    pub repeat_m: f64,
}

impl SceneObject {
    /// Creates a visible object with identity transform and a neutral gray color.
    pub fn new(object_id: ObjectId, name: impl Into<String>, mesh: Mesh) -> Self {
        Self {
            object_id,
            name: name.into(),
            mesh,
            exact_geometry: None,
            transform: Transform3::IDENTITY,
            visible: true,
            selected: false,
            highlighted: false,
            color: [0.72, 0.74, 0.78, 1.0],
            render_texture: None,
        }
    }

    /// Reports whether this object can be resolved to an exact BRep shape.
    pub fn has_exact_geometry(&self) -> bool {
        self.exact_geometry.is_some()
    }

    /// Returns the object's world-space bound, if the mesh has triangles.
    pub fn world_bounds(&self) -> Option<Aabb> {
        self.transform.transform_bounds(self.mesh.bounds()?)
    }
}

/// An ID-ordered scene object registry with selection and visibility helpers.
#[derive(Debug, Default)]
pub struct Scene {
    objects: BTreeMap<ObjectId, SceneObject>,
    highlighted_id: Option<ObjectId>,
}

impl Scene {
    /// Creates an empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts an object. An existing object with the same ID is replaced and returned.
    pub fn insert(&mut self, object: SceneObject) -> Option<SceneObject> {
        self.objects.insert(object.object_id, object)
    }

    /// Removes an object by ID and clears its selection/highlight state.
    pub fn remove(&mut self, object_id: ObjectId) -> Option<SceneObject> {
        let object = self.objects.remove(&object_id);
        if self.highlighted_id == Some(object_id) {
            self.highlighted_id = None;
        }
        object
    }

    /// Gets an immutable object reference by ID.
    pub fn get(&self, object_id: ObjectId) -> Option<&SceneObject> {
        self.objects.get(&object_id)
    }

    /// Gets a mutable object reference by ID.
    pub fn get_mut(&mut self, object_id: ObjectId) -> Option<&mut SceneObject> {
        self.objects.get_mut(&object_id)
    }

    /// Iterates scene objects in stable, ascending object ID order.
    pub fn objects(&self) -> impl Iterator<Item = &SceneObject> {
        self.objects.values()
    }

    /// Iterates object IDs in stable, ascending order.
    pub fn object_ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.objects.keys().copied()
    }

    /// Returns the currently highlighted object ID, if it is still registered.
    pub fn highlighted_id(&self) -> Option<ObjectId> {
        self.highlighted_id
    }

    /// Returns IDs of all objects carrying a highlight flag, in stable order.
    pub fn highlighted_ids(&self) -> Vec<ObjectId> {
        self.objects
            .values()
            .filter(|object| object.highlighted)
            .map(|object| object.object_id)
            .collect()
    }

    /// Returns the number of registered visible objects.
    pub fn visible_count(&self) -> usize {
        self.objects
            .values()
            .filter(|object| object.visible)
            .count()
    }

    /// Returns the number of registered hidden objects.
    pub fn hidden_count(&self) -> usize {
        self.objects
            .values()
            .filter(|object| !object.visible)
            .count()
    }

    /// Returns IDs of all selected objects in stable order.
    pub fn selected_ids(&self) -> Vec<ObjectId> {
        self.objects
            .values()
            .filter(|object| object.selected)
            .map(|object| object.object_id)
            .collect()
    }

    /// Marks one object selected or clears the selection when `None` is supplied.
    /// Hidden and unknown objects cannot be newly selected.
    pub fn select(&mut self, object_id: Option<ObjectId>, additive: bool) -> bool {
        if !additive {
            for object in self.objects.values_mut() {
                object.selected = false;
            }
        }
        if let Some(id) = object_id {
            if let Some(object) = self.objects.get_mut(&id) {
                if object.visible {
                    object.selected = true;
                    return true;
                }
            }
            return false;
        }
        true
    }

    /// Toggles one registered visible object's selection flag.
    pub fn toggle_selection(&mut self, object_id: ObjectId) -> bool {
        let Some(object) = self.objects.get_mut(&object_id) else {
            return false;
        };
        if !object.visible {
            return false;
        }
        object.selected = !object.selected;
        true
    }

    /// Sets an object's visibility and deselects it when it becomes hidden.
    pub fn set_visible(&mut self, object_id: ObjectId, visible: bool) -> bool {
        let Some(object) = self.objects.get_mut(&object_id) else {
            return false;
        };
        object.visible = visible;
        if !visible {
            object.selected = false;
            object.highlighted = false;
        }
        if self.highlighted_id == Some(object_id) && !visible {
            self.highlighted_id = None;
        }
        true
    }

    /// Sets the single transient highlighted object; hidden or unknown IDs clear it.
    pub fn set_highlighted(&mut self, object_id: Option<ObjectId>) -> Option<ObjectId> {
        let accepted =
            object_id.filter(|id| self.objects.get(id).map_or(false, |object| object.visible));
        for object in self.objects.values_mut() {
            object.highlighted = Some(object.object_id) == accepted;
        }
        self.highlighted_id = accepted;
        accepted
    }

    /// Returns the closest visible object's triangle hit for a world-space ray.
    pub fn pick(&self, ray: Ray, max_distance: f64) -> Option<PickHit> {
        let ray = Ray::new(ray.origin, ray.direction);
        self.pick_in_interval(ray, 0.0, max_distance)
    }

    /// Returns the closest visible hit along the ray portion inside the section
    /// bounds. Intersections before entering the box are skipped, so a clipped
    /// front surface does not hide a deeper surface that remains inside.
    pub(crate) fn pick_with_section_bounds(
        &self,
        ray: Ray,
        max_distance: f64,
        bounds: SectionBounds,
    ) -> Option<PickHit> {
        let ray = Ray::new(ray.origin, ray.direction);
        let (min_distance, max_distance) = bounds.ray_interval(ray, max_distance)?;
        self.pick_in_interval(ray, min_distance, max_distance)
    }

    fn pick_in_interval(&self, ray: Ray, min_distance: f64, max_distance: f64) -> Option<PickHit> {
        let mut best: Option<PickHit> = None;
        for object in self
            .objects
            .values()
            .filter(|object| object.visible && object.transform.is_valid())
        {
            let local_ray = Ray {
                origin: object.transform.inverse_transform_point(ray.origin)?,
                direction: object.transform.inverse_transform_vector(ray.direction)?,
            };
            if let Some(local) = object.mesh.pick_local_in_range(
                local_ray,
                min_distance,
                best.map_or(max_distance, |hit| hit.distance),
            ) {
                let point = object.transform.transform_point(local.point);
                let normal = object.transform.transform_normal(local.normal)?;
                let distance = (point - ray.origin).length();
                if distance < 0.0 || distance > max_distance {
                    continue;
                }
                let hit = PickHit {
                    object_id: object.object_id,
                    triangle_id: local.triangle_id,
                    world_point: point,
                    world_normal: normal,
                    distance,
                };
                let replace = best.map_or(true, |current| {
                    distance < current.distance - 1.0e-10
                        || ((distance - current.distance).abs() <= 1.0e-10
                            && (hit.object_id, hit.triangle_id)
                                < (current.object_id, current.triangle_id))
                });
                if replace {
                    best = Some(hit);
                }
            }
        }
        best
    }

    /// Returns the combined world bounds of all visible objects with geometry.
    pub fn visible_bounds(&self) -> Option<Aabb> {
        self.objects
            .values()
            .filter(|object| object.visible)
            .filter_map(SceneObject::world_bounds)
            .reduce(Aabb::union)
    }

    /// Clears all selection flags before applying an iterator of selected IDs.
    pub fn restore_selection(&mut self, selected_ids: &[ObjectId]) {
        for object in self.objects.values_mut() {
            object.selected = false;
        }
        for id in selected_ids {
            if let Some(object) = self.objects.get_mut(id) {
                if object.visible {
                    object.selected = true;
                }
            }
        }
    }

    /// Restores hidden state by making the listed registered IDs invisible.
    pub fn restore_visibility(&mut self, hidden_ids: &[ObjectId]) {
        for object in self.objects.values_mut() {
            object.visible = !hidden_ids.contains(&object.object_id);
            if !object.visible {
                object.selected = false;
                object.highlighted = false;
            }
        }
        if self.highlighted_id.map_or(false, |id| {
            self.objects.get(&id).map_or(true, |object| !object.visible)
        }) {
            self.highlighted_id = None;
        }
    }

    /// Restores highlight flags for registered and visible object IDs.
    pub fn restore_highlights(&mut self, highlighted_ids: &[ObjectId]) {
        for object in self.objects.values_mut() {
            object.highlighted = object.visible && highlighted_ids.contains(&object.object_id);
        }
        self.highlighted_id = highlighted_ids.iter().copied().find(|id| {
            self.objects
                .get(id)
                .map_or(false, |object| object.highlighted)
        });
    }
}
