//! Deterministic display geometry for the AI Revit MVP.
//!
//! Model dimensions are metres in XY with Z as elevation. These triangle meshes
//! are derived, approximate display data. They are not exact solids or BRep,
//! and must not be used as the authority for document edits. Reported quantities
//! and sections are labelled approximate_mesh and belong to derived geometry.
//! Strip triangulation can create T junctions at complex contour transitions;
//! callers must not assume topological watertightness.

use ai_revit_core::geometry_validation::validate_element_geometry;
pub use ai_revit_core::geometry_validation::{
    DEFAULT_LINEAR_TOLERANCE_M, GeometryErrorCode, GeometryOptions,
};
use ai_revit_core::{Document, ElementId, ElementKind, ElementRecord, Point2};
use serde::Serialize;
use std::fmt;
mod backend;
mod cache;
mod hosted;
mod gpu;
pub use gpu::{configure_gpu_compute, update_gpu_compute_budget, refresh_gpu_compute_budget, shutdown_gpu_compute, gpu_compute_observe};
mod planar;
mod quantities;
mod section;
mod spatial;
pub use backend::*;
pub use cache::*;
pub use quantities::*;
pub use section::{GeometrySection, SectionPlane, SectionPolyline};
pub use spatial::*;
use viewer_core::{Aabb, Mesh, ObjectId, Scene, SceneObject, Triangle, Vec3};

const MAX_TRIANGLES_PER_ELEMENT: usize = 250_000;

/// Describes the precision and role of the produced geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryQuality {
    /// A deterministic planar triangle approximation; no exact BRep is attached.
    ApproximateTriangleMesh,
}

/// A failed element never contributes a partial object to the scene.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeometryError {
    pub element_id: ElementId,
    pub host_id: Option<ElementId>,
    pub opening_id: Option<ElementId>,
    pub error_code: String,
    /// Stable v0.4 category; error_code preserves legacy Wall diagnostics.
    pub canonical_error_code: String,
    pub backend: String,
    pub representation_quality: RepresentationQuality,
    #[serde(skip)]
    pub code: GeometryErrorCode,
    pub message: &'static str,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "element {}: {} ({:?})",
            self.element_id.get(),
            self.message,
            self.code
        )
    }
}

impl std::error::Error for GeometryError {}

pub struct ElementMesh {
    pub element_id: ElementId,
    pub mesh: Mesh,
    pub quality: GeometryQuality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedElement {
    pub element_id: ElementId,
    pub object_id: ObjectId,
    pub quality: GeometryQuality,
}

/// A scene plus complete accounting of accepted and rejected model elements.
pub struct SceneProjection {
    pub scene: Scene,
    pub projected: Vec<ProjectedElement>,
    pub failures: Vec<GeometryError>,
}
/// Per-element geometry outcome retained by GeometryCache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryStatusKind {
    Valid,
    Invalid,
    ResolutionFailed,
}

/// A lightweight observation of the latest cached mesh for a semantic element.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementGeometryStatus {
    pub element_id: ElementId,
    pub status: GeometryStatusKind,
    pub bounds: Option<Aabb>,
    pub mesh_revision: u64,
    pub failure: Option<GeometryError>,
    pub resolution_error: Option<String>,
}

/// Instrumentation and IDs affected by one incremental cache refresh.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RebuildReport {
    pub rebuilt_element_count: usize,
    pub rebuild_duration_ms: u64,
    pub rebuild_duration_us: u64,
    pub quantity_rebuilt_element_count: usize,
    pub spatial_updated_element_count: usize,
    pub rebuilt_element_ids: Vec<ElementId>,
    pub removed_element_count: usize,
    pub removed_element_ids: Vec<ElementId>,
    pub document_invalidated: bool,
    pub total_document_scan_count: usize,
    pub scanned_element_count: usize,
    pub fingerprint_comparison_count: usize,
    pub hosted_lookup_count: usize,
    pub display_changed_element_ids: Vec<ElementId>,
}

/// Updates a caller-owned geometry cache and returns rebuild instrumentation.
pub fn project_dirty_document(document: &Document, cache: &mut GeometryCache) -> RebuildReport {
    cache.rebuild(document)
}

/// Incrementally updates a caller-owned scene and returns rebuild instrumentation.
pub fn project_dirty_document_into_scene(
    document: &Document,
    cache: &mut GeometryCache,
    scene: &mut Scene,
) -> RebuildReport {
    cache.rebuild_into(document, scene)
}

/// The mapping is stable across rebuilds and preserves the document ID exactly.
pub const fn object_id_for(element_id: ElementId) -> ObjectId {
    element_id.get()
}

pub fn mesh_for_element(element: &ElementRecord) -> Result<ElementMesh, GeometryError> {
    mesh_for_element_with_options(element, GeometryOptions::default())
}

pub fn mesh_for_element_with_options(
    element: &ElementRecord,
    options: GeometryOptions,
) -> Result<ElementMesh, GeometryError> {
    let geometry = validate_element_geometry(&element.kind, options)
        .map_err(|failure| error(element.id, failure.code, failure.message))?;
    let triangles = extrude_region(
        element.id,
        &geometry.boundary,
        &geometry.holes,
        |point| geometry.top_elevation_at(point),
        geometry.thickness_m,
    )?;
    if triangles.is_empty() {
        return Err(error(
            element.id,
            GeometryErrorCode::TessellationFailed,
            "no nondegenerate display triangles were produced",
        ));
    }
    Ok(ElementMesh {
        element_id: element.id,
        mesh: Mesh::new(triangles),
        quality: GeometryQuality::ApproximateTriangleMesh,
    })
}

pub fn project_document(document: &Document) -> SceneProjection {
    project_document_with_options(document, GeometryOptions::default())
}

pub fn project_document_with_options(
    document: &Document,
    options: GeometryOptions,
) -> SceneProjection {
    let mut cache = GeometryCache::with_options(options);
    cache.rebuild(document);
    cache.into_projection()
}

fn display_style(element: &ElementRecord) -> (String, [f32; 4]) {
    let (default_name, color) = match &element.kind {
        ElementKind::Wall(_) => ("Wall", [0.68, 0.73, 0.79, 1.0]),
        ElementKind::Floor(_) => ("Floor", [0.76, 0.72, 0.64, 1.0]),
        ElementKind::Roof(_) => ("Roof", [0.55, 0.64, 0.72, 1.0]),
        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_) => {
            ("Opening", [0.90, 0.68, 0.32, 1.0])
        }
        ElementKind::Space(_) => ("Space", [0.45, 0.75, 0.60, 0.25]),
        ElementKind::Door(_) => ("Door", [0.55, 0.36, 0.24, 1.0]),
        ElementKind::Window(_) => ("Window", [0.38, 0.68, 0.82, 1.0]),
    };
    let name = if element.name.trim().is_empty() {
        format!("{default_name} {}", element.id.get())
    } else {
        element.name.clone()
    };
    (name, color)
}

fn extrude_region<F: Fn(Point2) -> f64>(
    id: ElementId,
    outer: &[Point2],
    holes: &[Vec<Point2>],
    top_z: F,
    thickness: f64,
) -> Result<Vec<Triangle>, GeometryError> {
    let rings = std::iter::once(outer).chain(holes.iter().map(Vec::as_slice));
    let mut xs: Vec<f64> = rings
        .clone()
        .flat_map(|ring| ring.iter().map(|point| point.x))
        .collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let mut triangles = Vec::new();
    for pair in xs.windows(2) {
        let left_x = pair[0];
        let right_x = pair[1];
        let middle_x = left_x + (right_x - left_x) * 0.5;
        if middle_x <= left_x || middle_x >= right_x {
            return Err(numeric_error(id));
        }
        let mut crossings = Vec::new();
        for ring in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
            for index in 0..ring.len() {
                let a = ring[index];
                let b = ring[(index + 1) % ring.len()];
                if a.x.min(b.x) < middle_x && middle_x < a.x.max(b.x) {
                    let middle_y = y_at_x(a, b, middle_x);
                    let left = Point2 {
                        x: left_x,
                        y: y_at_x(a, b, left_x),
                    };
                    let right = Point2 {
                        x: right_x,
                        y: y_at_x(a, b, right_x),
                    };
                    if !middle_y.is_finite() || !left.y.is_finite() || !right.y.is_finite() {
                        return Err(numeric_error(id));
                    }
                    crossings.push(Crossing {
                        middle_y,
                        left,
                        right,
                    });
                }
            }
        }
        crossings.sort_by(|a, b| a.middle_y.total_cmp(&b.middle_y));
        if crossings.len() % 2 != 0 {
            return Err(error(
                id,
                GeometryErrorCode::TessellationFailed,
                "contour has an odd number of strip crossings",
            ));
        }
        for pair in crossings.chunks_exact(2) {
            let lower = pair[0];
            let upper = pair[1];
            if lower.middle_y >= upper.middle_y {
                return Err(error(
                    id,
                    GeometryErrorCode::TessellationFailed,
                    "strip crossings overlap",
                ));
            }
            let corners = [lower.left, lower.right, upper.right, upper.left];
            add_horizontal_faces(id, &mut triangles, corners, &top_z, thickness)?;
        }
    }
    for ring in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
        for index in 0..ring.len() {
            let a = ring[index];
            let b = ring[(index + 1) % ring.len()];
            let mut cuts = Vec::new();
            cuts.push(a);
            if a.x != b.x {
                let intermediate = xs
                    .iter()
                    .copied()
                    .filter(|&x| a.x.min(b.x) < x && x < a.x.max(b.x));
                let mut points: Vec<Point2> = intermediate
                    .map(|x| Point2 {
                        x,
                        y: y_at_x(a, b, x),
                    })
                    .collect();
                if a.x > b.x {
                    points.reverse();
                }
                cuts.extend(points);
            }
            cuts.push(b);
            for pair in cuts.windows(2) {
                let a = pair[0];
                let b = pair[1];
                let a_top = vertex(id, a, top_z(a))?;
                let b_top = vertex(id, b, top_z(b))?;
                let a_bottom = vertex(id, a, top_z(a) - thickness)?;
                let b_bottom = vertex(id, b, top_z(b) - thickness)?;
                add_triangle(id, &mut triangles, a_bottom, b_bottom, b_top)?;
                add_triangle(id, &mut triangles, a_bottom, b_top, a_top)?;
            }
        }
    }
    if triangles.is_empty() {
        return Err(error(
            id,
            GeometryErrorCode::TessellationFailed,
            "region produced no triangles",
        ));
    }
    // viewer_core::Mesh is intentionally a rendering/picking representation.
    // The source contours and dimensions remain the model authority.
    Ok(triangles)
}

#[derive(Clone, Copy)]
struct Crossing {
    middle_y: f64,
    left: Point2,
    right: Point2,
}

fn add_horizontal_faces<F: Fn(Point2) -> f64>(
    id: ElementId,
    triangles: &mut Vec<Triangle>,
    corners: [Point2; 4],
    top_z: &F,
    thickness: f64,
) -> Result<(), GeometryError> {
    let mut top = [Vec3::ZERO; 4];
    let mut bottom = [Vec3::ZERO; 4];
    for index in 0..4 {
        let z = top_z(corners[index]);
        top[index] = vertex(id, corners[index], z)?;
        bottom[index] = vertex(id, corners[index], z - thickness)?;
    }
    add_triangle(id, triangles, top[0], top[1], top[2])?;
    add_triangle(id, triangles, top[0], top[2], top[3])?;
    add_triangle(id, triangles, bottom[2], bottom[1], bottom[0])?;
    add_triangle(id, triangles, bottom[3], bottom[2], bottom[0])?;
    Ok(())
}

fn add_triangle(
    id: ElementId,
    triangles: &mut Vec<Triangle>,
    a: Vec3,
    b: Vec3,
    c: Vec3,
) -> Result<(), GeometryError> {
    let area_vector = (b - a).cross(c - a);
    let squared = area_vector.length_squared();
    if !squared.is_finite() {
        return Err(numeric_error(id));
    }
    if squared == 0.0 {
        return Ok(());
    }
    if triangles.len() >= MAX_TRIANGLES_PER_ELEMENT {
        return Err(error(
            id,
            GeometryErrorCode::TriangleLimitExceeded,
            "triangle limit exceeded",
        ));
    }
    let triangle_id = u32::try_from(triangles.len() + 1).map_err(|_| {
        error(
            id,
            GeometryErrorCode::VertexLimitExceeded,
            "triangle ID limit exceeded",
        )
    })?;
    triangles.push(Triangle::new(triangle_id, a, b, c));
    Ok(())
}

fn vertex(id: ElementId, point: Point2, z: f64) -> Result<Vec3, GeometryError> {
    if !point.x.is_finite() || !point.y.is_finite() || !z.is_finite() {
        return Err(numeric_error(id));
    }
    Ok(Vec3::new(point.x, point.y, z))
}

fn y_at_x(a: Point2, b: Point2, x: f64) -> f64 {
    if x == a.x {
        a.y
    } else if x == b.x {
        b.y
    } else {
        a.y + (b.y - a.y) * ((x - a.x) / (b.x - a.x))
    }
}

fn numeric_error(id: ElementId) -> GeometryError {
    error(
        id,
        GeometryErrorCode::NumericRange,
        "geometry exceeds safe numeric range",
    )
}

fn error(id: ElementId, code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError {
        element_id: id,
        host_id: None,
        opening_id: None,
        error_code: geometry_error_code(code).to_owned(),
        canonical_error_code: geometry_error_code(code).to_owned(),
        backend: "mesh".to_owned(),
        representation_quality: RepresentationQuality::ApproximateMesh,
        code,
        message,
    }
}

fn geometry_error_code(code: GeometryErrorCode) -> &'static str {
    match code {
        GeometryErrorCode::InvalidOptions => "invalid_options",
        GeometryErrorCode::NonFinite => "non_finite",
        GeometryErrorCode::NonPositiveDimension => "non_positive_dimension",
        GeometryErrorCode::DegenerateLine => "degenerate_line",
        GeometryErrorCode::ContourTooSmall => "contour_too_small",
        GeometryErrorCode::DegenerateContour => "degenerate_contour",
        GeometryErrorCode::SelfIntersection => "self_intersection",
        GeometryErrorCode::HoleOutsideBoundary => "hole_outside_boundary",
        GeometryErrorCode::ContoursIntersect => "contours_intersect",
        GeometryErrorCode::NumericRange => "numeric_range",
        GeometryErrorCode::TessellationFailed => "tessellation_failed",
        GeometryErrorCode::VertexLimitExceeded => "vertex_limit_exceeded",
        GeometryErrorCode::TriangleLimitExceeded => "triangle_limit_exceeded",
    }
}
