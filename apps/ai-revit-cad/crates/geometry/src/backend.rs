//! Derived geometry only: the backend never mutates the BIM document.
use crate::*;
use ai_revit_core::WallSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationQuality {
    ApproximateMesh,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeometryCapabilities {
    pub exact_brep: bool,
    pub exact_boolean: bool,
    pub exact_volume: bool,
    pub exact_section: bool,
    pub polygonal_voids: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackendCapabilities {
    pub backend: String,
    pub capabilities: GeometryCapabilities,
}

/// Effective semantic parameters collected by the cache using core indexes.
#[derive(Clone, Debug)]
pub struct GeometryInput {
    pub element: ElementRecord,
    pub openings: Vec<ElementRecord>,
    pub derived_from: Vec<ElementId>,
}

pub struct DerivedGeometry {
    pub element_id: ElementId,
    pub mesh: Mesh,
    pub representation_quality: RepresentationQuality,
    pub bounds: Option<Aabb>,
    pub volume: Option<f64>,
    pub surface_area: Option<f64>,
    pub quantities: GeometryQuantities,
}

/// Replaceable boundary between effective BIM parameters and display geometry.
/// Quantities and sections inherit the backend's declared representation quality.
pub trait GeometryBackend {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            backend: self.name().to_owned(),
            capabilities: GeometryCapabilities {
                exact_brep: false,
                exact_boolean: false,
                exact_volume: false,
                exact_section: false,
                polygonal_voids: false,
            },
        }
    }
    fn derive_contextual(
        &self,
        input: &GeometryInput,
        _host: Option<&ElementRecord>,
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError> {
        self.derive_host(input, options)
    }
    fn derive_element(
        &self,
        input: &GeometryInput,
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError>;
    fn derive_host(
        &self,
        input: &GeometryInput,
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError> {
        self.derive_element(input, options)
    }
    fn apply_voids(
        &self,
        host: &ElementRecord,
        openings: &[ElementRecord],
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError> {
        self.derive_host(
            &GeometryInput {
                element: host.clone(),
                openings: openings.to_vec(),
                derived_from: std::iter::once(host.id)
                    .chain(openings.iter().map(|o| o.id))
                    .collect(),
            },
            options,
        )
    }
    fn validate_geometry(&self, geometry: &DerivedGeometry) -> Result<(), GeometryError> {
        if geometry.mesh.triangles().iter().any(|t| {
            t.vertices
                .iter()
                .any(|v| !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite())
        }) || geometry.volume.is_some_and(|v| !v.is_finite() || v < 0.0)
            || geometry
                .surface_area
                .is_some_and(|v| !v.is_finite() || v < 0.0)
            || [
                geometry.quantities.gross_volume,
                geometry.quantities.net_volume,
                geometry.quantities.gross_surface_area,
                geometry.quantities.net_surface_area,
                geometry.quantities.opening_area,
                geometry.quantities.opening_volume,
                geometry.quantities.perimeter,
                geometry.quantities.length,
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || v < 0.0)
        {
            return Err(numeric_error(geometry.element_id));
        }
        Ok(())
    }
    fn bounds(&self, geometry: &DerivedGeometry) -> Option<Aabb> {
        geometry.bounds
    }
    fn volume(&self, geometry: &DerivedGeometry) -> Option<f64> {
        geometry.volume
    }
    fn surface_area(&self, geometry: &DerivedGeometry) -> Option<f64> {
        geometry.surface_area
    }
    fn section(
        &self,
        geometry: &DerivedGeometry,
        plane: SectionPlane,
        tolerance: f64,
    ) -> Result<Vec<SectionPolyline>, GeometryObservationError> {
        let plane = crate::section::validate_plane(plane)?;
        Ok(crate::section::mesh_section(
            geometry.element_id,
            &geometry.mesh,
            plane,
            tolerance,
        ))
    }
}

#[derive(Default)]
pub struct MeshGeometryBackend;

impl GeometryBackend for MeshGeometryBackend {
    fn name(&self) -> &'static str {
        "mesh_v04_planar_union"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            backend: "mesh".to_owned(),
            capabilities: GeometryCapabilities {
                exact_brep: false,
                exact_boolean: false,
                exact_volume: false,
                exact_section: false,
                polygonal_voids: true,
            },
        }
    }

    fn derive_contextual(
        &self,
        input: &GeometryInput,
        host: Option<&ElementRecord>,
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError> {
        if matches!(input.element.kind, ElementKind::PolygonalOpening(_)) {
            if let Some(host) = host {
                let geometry = crate::hosted::opening_proxy(host, &input.element, options)?;
                self.validate_geometry(&geometry)?;
                return Ok(geometry);
            }
        }
        self.derive_element(input, options)
    }

    fn derive_element(
        &self,
        input: &GeometryInput,
        options: GeometryOptions,
    ) -> Result<DerivedGeometry, GeometryError> {
        if matches!(input.element.kind, ElementKind::Space(_)) {
            if let Some(opening) = input.openings.iter().find(|o| {
                matches!(
                    o.kind,
                    ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                )
            }) {
                return Err(hosted_error(
                    input.element.id,
                    opening.id,
                    "unsupported_host_shape",
                    "semantic Space has no solid host geometry",
                ));
            }
            return Ok(DerivedGeometry {
                element_id: input.element.id,
                mesh: Mesh::new(Vec::new()),
                representation_quality: RepresentationQuality::ApproximateMesh,
                bounds: None,
                volume: None,
                surface_area: None,
                quantities: crate::hosted::unknown_quantities(),
            });
        }
        validate_element_geometry(&input.element.kind, options).map_err(|e| {
            if let Some(opening) = input.openings.iter().find(|o| {
                matches!(
                    o.kind,
                    ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                )
            }) {
                let mut failure = hosted_error(
                    input.element.id,
                    opening.id,
                    "unsupported_host_shape",
                    e.message,
                );
                failure.code = e.code;
                failure
            } else {
                error(input.element.id, e.code, e.message)
            }
        })?;
        let openings: Vec<_> = input
            .openings
            .iter()
            .filter(|o| {
                matches!(
                    o.kind,
                    ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                )
            })
            .collect();
        let geometry = if !openings.is_empty() {
            match &input.element.kind {
                ElementKind::Wall(wall)
                    if openings
                        .iter()
                        .all(|o| matches!(o.kind, ElementKind::Opening(_))) =>
                {
                    cut_wall(input.element.id, wall, &openings, options)?
                }
                _ => crate::hosted::derive_host(&input.element, &openings, options)?,
            }
        } else {
            let result = mesh_for_element_with_options(&input.element, options)?;
            let bounds = result.mesh.bounds();
            let (volume, area) = mesh_quantities(&result.mesh);
            DerivedGeometry {
                element_id: input.element.id,
                mesh: result.mesh,
                representation_quality: RepresentationQuality::ApproximateMesh,
                bounds,
                volume: Some(volume),
                surface_area: Some(area),
                quantities: crate::hosted::uncut_quantities(&input.element, volume, area, options)?,
            }
        };
        self.validate_geometry(&geometry).map_err(|mut failure| {
            if let Some(opening) = openings.first() {
                failure.host_id = Some(input.element.id);
                failure.opening_id = Some(opening.id);
            }
            failure
        })?;
        Ok(geometry)
    }
}

pub(crate) fn hosted_error(
    host: ElementId,
    opening: ElementId,
    code: &'static str,
    message: &'static str,
) -> GeometryError {
    GeometryError {
        element_id: host,
        host_id: Some(host),
        opening_id: Some(opening),
        code: GeometryErrorCode::TessellationFailed,
        error_code: code.to_owned(),
        canonical_error_code: match code {
            "opening_depth_insufficient" => "void_depth_insufficient",
            "opening_exceeds_host" => "void_outside_host",
            "invalid_opening_orientation" => "void_non_planar",
            "nonmanifold_void_union" => "nonmanifold_result",
            "invalid_opening_geometry" => "void_degenerate",
            _ => code,
        }
        .to_owned(),
        backend: "mesh".to_owned(),
        representation_quality: RepresentationQuality::ApproximateMesh,
        message,
    }
}

#[derive(Clone, Copy)]
struct VoidRect {
    opening_id: ElementId,
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
}

/// Rectangles are unioned in the wall's local (along, elevation) plane. Every
/// solid grid cell contributes front/back faces, and only exposed grid edges
/// contribute reveals. No internal faces or overlap-dependent subtraction order.
fn cut_wall(
    id: ElementId,
    wall: &WallSpec,
    openings: &[&ElementRecord],
    options: GeometryOptions,
) -> Result<DerivedGeometry, GeometryError> {
    let dx = wall.end.x - wall.start.x;
    let dy = wall.end.y - wall.start.y;
    let length = dx.hypot(dy);
    let ux = dx / length;
    let uy = dy / length;
    let tol = options.linear_tolerance_m;
    let half = wall.thickness_m * 0.5;
    let mut rects = Vec::new();
    for opening in openings {
        let ElementKind::Opening(spec) = &opening.kind else {
            continue;
        };
        validate_element_geometry(&opening.kind, options).map_err(|e| GeometryError {
            element_id: id,
            host_id: Some(id),
            opening_id: Some(opening.id),
            code: e.code,
            error_code: "invalid_opening_geometry".to_owned(),
            canonical_error_code: "void_degenerate".to_owned(),
            backend: "mesh".to_owned(),
            representation_quality: RepresentationQuality::ApproximateMesh,
            message: e.message,
        })?;
        // Use the same angular alignment tolerance as semantic hosting. The
        // accepted numerical angular residual is projected into the host frame.
        let (sin, cos) = spec.rotation_rad.sin_cos();
        if (cos * uy - sin * ux).abs() > 1.0e-6 {
            return Err(hosted_error(
                id,
                opening.id,
                "invalid_opening_orientation",
                "opening must align with the host wall",
            ));
        }
        let cx = spec.center.x - wall.start.x;
        let cy = spec.center.y - wall.start.y;
        let along = cx * ux + cy * uy;
        let across = -cx * uy + cy * ux;
        let mut rect = VoidRect {
            opening_id: opening.id,
            x0: along - spec.width_m * 0.5,
            x1: along + spec.width_m * 0.5,
            z0: spec.elevation_m - wall.base_elevation_m,
            z1: spec.elevation_m - wall.base_elevation_m + spec.height_m,
        };
        if rect.x0 < -tol
            || rect.x1 > length + tol
            || rect.z0 < -tol
            || rect.z1 > wall.height_m + tol
        {
            return Err(hosted_error(
                id,
                opening.id,
                "opening_exceeds_host",
                "opening lies outside the wall boundary",
            ));
        }
        if across - spec.depth_m * 0.5 > -half + tol || across + spec.depth_m * 0.5 < half - tol {
            return Err(hosted_error(
                id,
                opening.id,
                "opening_depth_insufficient",
                "opening volume must span the full host thickness",
            ));
        }
        // Clamp only the accepted numerical tolerance at the host boundary.
        rect.x0 = rect.x0.max(0.0);
        rect.x1 = rect.x1.min(length);
        rect.z0 = rect.z0.max(0.0);
        rect.z1 = rect.z1.min(wall.height_m);
        rects.push(rect);
    }
    rects.sort_by(|a, b| {
        a.x0.total_cmp(&b.x0)
            .then(a.x1.total_cmp(&b.x1))
            .then(a.z0.total_cmp(&b.z0))
            .then(a.z1.total_cmp(&b.z1))
            .then(a.opening_id.cmp(&b.opening_id))
    });
    rects.dedup_by(|a, b| a.x0 == b.x0 && a.x1 == b.x1 && a.z0 == b.z0 && a.z1 == b.z1);
    let mut xs = vec![0.0, length];
    let mut zs = vec![0.0, wall.height_m];
    for r in &rects {
        xs.extend([r.x0, r.x1]);
        zs.extend([r.z0, r.z1]);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    zs.sort_by(f64::total_cmp);
    zs.dedup();
    let nx = xs.len() - 1;
    let nz = zs.len() - 1;
    let cells = nx
        .checked_mul(nz)
        .filter(|n| *n <= MAX_TRIANGLES_PER_ELEMENT)
        .ok_or_else(|| {
            error(
                id,
                GeometryErrorCode::TriangleLimitExceeded,
                "opening grid limit exceeded",
            )
        })?;
    let mut solid = vec![false; cells];
    for x in 0..nx {
        for z in 0..nz {
            let mx = xs[x] + (xs[x + 1] - xs[x]) * 0.5;
            let mz = zs[z] + (zs[z + 1] - zs[z]) * 0.5;
            if !(xs[x] < mx && mx < xs[x + 1] && zs[z] < mz && mz < zs[z + 1]) {
                return Err(numeric_error(id));
            }
            solid[x * nz + z] = !rects
                .iter()
                .any(|r| r.x0 < mx && mx < r.x1 && r.z0 < mz && mz < r.z1);
        }
    }
    // Diagonally alternating solid/void cells meet along a pinched extrusion
    // edge with four incident faces. Return a structured failure rather than
    // claiming that this nonmanifold union is a validated wall surface.
    for x in 1..nx {
        for z in 1..nz {
            let a = solid[(x - 1) * nz + z - 1];
            let b = solid[x * nz + z - 1];
            let c = solid[(x - 1) * nz + z];
            let d = solid[x * nz + z];
            if a == d && b == c && a != b {
                let opening_id = rects
                    .iter()
                    .filter(|r| {
                        (r.x0 == xs[x] || r.x1 == xs[x]) && (r.z0 == zs[z] || r.z1 == zs[z])
                    })
                    .map(|r| r.opening_id)
                    .min()
                    .unwrap_or(openings[0].id);
                return Err(hosted_error(
                    id,
                    opening_id,
                    "nonmanifold_void_union",
                    "opening union touches at a corner and creates a pinched wall boundary",
                ));
            }
        }
    }
    let world = |x: f64, y: f64, z: f64| {
        Vec3::new(
            wall.start.x + ux * x - uy * y,
            wall.start.y + uy * x + ux * y,
            wall.base_elevation_m + z,
        )
    };
    let mut triangles = Vec::new();
    let mut solid_area = 0.0;
    let mut perimeter = 0.0;
    for x in 0..nx {
        for z in 0..nz {
            if !solid[x * nz + z] {
                continue;
            }
            let (a, b, c, d) = (xs[x], xs[x + 1], zs[z], zs[z + 1]);
            solid_area += (b - a) * (d - c);
            let mut quad = |points: [(f64, f64, f64); 4]| -> Result<(), GeometryError> {
                let p = points.map(|(x, y, z)| world(x, y, z));
                add_triangle(id, &mut triangles, p[0], p[1], p[2])?;
                add_triangle(id, &mut triangles, p[0], p[2], p[3])
            };
            quad([(a, -half, c), (b, -half, c), (b, -half, d), (a, -half, d)])?;
            quad([(a, half, c), (a, half, d), (b, half, d), (b, half, c)])?;
            if x == 0 || !solid[(x - 1) * nz + z] {
                perimeter += d - c;
                quad([(a, -half, c), (a, -half, d), (a, half, d), (a, half, c)])?;
            }
            if x + 1 == nx || !solid[(x + 1) * nz + z] {
                perimeter += d - c;
                quad([(b, -half, c), (b, half, c), (b, half, d), (b, -half, d)])?;
            }
            if z == 0 || !solid[x * nz + z - 1] {
                perimeter += b - a;
                quad([(a, -half, c), (a, half, c), (b, half, c), (b, -half, c)])?;
            }
            if z + 1 == nz || !solid[x * nz + z + 1] {
                perimeter += b - a;
                quad([(a, -half, d), (b, -half, d), (b, half, d), (a, half, d)])?;
            }
        }
    }
    let mesh = Mesh::new(triangles);
    let bounds = mesh.bounds();
    Ok(DerivedGeometry {
        element_id: id,
        mesh,
        representation_quality: RepresentationQuality::ApproximateMesh,
        bounds,
        volume: Some(solid_area * wall.thickness_m),
        surface_area: Some(2.0 * solid_area + perimeter * wall.thickness_m),
        quantities: GeometryQuantities {
            quality: RepresentationQuality::ApproximateMesh,
            gross_volume: Some(length * wall.height_m * wall.thickness_m),
            net_volume: Some(solid_area * wall.thickness_m),
            gross_surface_area: Some(
                2.0 * length * wall.height_m + 2.0 * (length + wall.height_m) * wall.thickness_m,
            ),
            net_surface_area: Some(2.0 * solid_area + perimeter * wall.thickness_m),
            opening_area: Some((length * wall.height_m - solid_area).max(0.0)),
            opening_volume: Some((length * wall.height_m - solid_area).max(0.0) * wall.thickness_m),
            perimeter: Some(perimeter),
            length: Some(length),
        },
    })
}

pub(crate) fn mesh_quantities(mesh: &Mesh) -> (f64, f64) {
    if let Some(quantities) = crate::gpu::mesh_quantities(mesh) {
        return quantities;
    }
    let origin = mesh
        .bounds()
        .map(|b| (b.min + b.max) * 0.5)
        .unwrap_or(Vec3::ZERO);
    let mut volume = 0.0;
    let mut area = 0.0;
    for triangle in mesh.triangles() {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() * 0.5;
        volume += (a - origin).dot((b - origin).cross(c - origin)) / 6.0;
    }
    (volume.abs(), area)
}
