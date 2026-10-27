//! Host-local planar voids. Roof thickness retains its semantic vertical axis.
use crate::backend::{hosted_error, mesh_quantities};
use crate::planar::{PlanarRegion, subtract_regions};
use crate::*;
use ai_revit_core::geometry_validation::validate_planar_contour;

struct HostFrame {
    origin: Vec3,
    u: Vec3,
    v: Vec3,
    extrusion: Vec3,
    lower: f64,
    upper: f64,
    outer: Vec<Point2>,
    holes: Vec<Vec<Point2>>,
    wall: bool,
    length: Option<f64>,
}

impl HostFrame {
    fn new(host: &ElementRecord, options: GeometryOptions) -> Result<Self, GeometryError> {
        let validated = validate_element_geometry(&host.kind, options)
            .map_err(|e| error(host.id, e.code, e.message))?;
        match &host.kind {
            ElementKind::Wall(w) => {
                let length = (w.end.x - w.start.x).hypot(w.end.y - w.start.y);
                let u = Vec3::new(
                    (w.end.x - w.start.x) / length,
                    (w.end.y - w.start.y) / length,
                    0.0,
                );
                Ok(Self {
                    origin: Vec3::new(w.start.x, w.start.y, w.base_elevation_m),
                    u,
                    v: Vec3::new(0.0, 0.0, 1.0),
                    extrusion: Vec3::new(u.y, -u.x, 0.0),
                    lower: -w.thickness_m * 0.5,
                    upper: w.thickness_m * 0.5,
                    outer: vec![
                        Point2::new(0.0, 0.0),
                        Point2::new(length, 0.0),
                        Point2::new(length, w.height_m),
                        Point2::new(0.0, w.height_m),
                    ],
                    holes: Vec::new(),
                    wall: true,
                    length: Some(length),
                })
            }
            ElementKind::Floor(_) | ElementKind::Roof(_) => {
                let n = validated.boundary.len() as f64;
                let center = Point2::new(
                    validated.boundary.iter().map(|p| p.x / n).sum(),
                    validated.boundary.iter().map(|p| p.y / n).sum(),
                );
                let (sx, sy) = match &host.kind {
                    ElementKind::Roof(r) => (r.slope_x, r.slope_y),
                    _ => (0.0, 0.0),
                };
                let local = |p: &Point2| Point2::new(p.x - center.x, p.y - center.y);
                Ok(Self {
                    origin: Vec3::new(center.x, center.y, validated.top_elevation_at(center)),
                    u: Vec3::new(1.0, 0.0, sx),
                    v: Vec3::new(0.0, 1.0, sy),
                    extrusion: Vec3::new(0.0, 0.0, 1.0),
                    lower: -validated.thickness_m,
                    upper: 0.0,
                    outer: validated.boundary.iter().map(local).collect(),
                    holes: validated
                        .holes
                        .iter()
                        .map(|h| h.iter().map(local).collect())
                        .collect(),
                    wall: false,
                    length: None,
                })
            }
            _ => Err(error(
                host.id,
                GeometryErrorCode::TessellationFailed,
                "host is not a planar Wall, Floor or Roof",
            )),
        }
    }
    fn world(&self, p: Point2, depth: f64) -> Vec3 {
        self.origin + self.u * p.x + self.v * p.y + self.extrusion * depth
    }
    fn placement(&self, center: Point2, elevation: f64) -> Point2 {
        if self.wall {
            Point2::new(
                (center.x - self.origin.x) * self.u.x + (center.y - self.origin.y) * self.u.y,
                elevation - self.origin.z,
            )
        } else {
            Point2::new(center.x - self.origin.x, center.y - self.origin.y)
        }
    }
    fn plane_area_factor(&self) -> f64 {
        self.u.cross(self.v).length()
    }
    fn volume_factor(&self) -> f64 {
        self.u.cross(self.v).dot(self.extrusion).abs()
    }
    fn boundary_metrics(&self, edges: impl Iterator<Item = [Point2; 2]>) -> (f64, f64) {
        let mut perimeter = 0.0;
        let mut side_factor = 0.0;
        for [a, b] in edges {
            let edge = self.u * (b.x - a.x) + self.v * (b.y - a.y);
            perimeter += edge.length();
            side_factor += edge.cross(self.extrusion).length();
        }
        (perimeter, side_factor)
    }
}

struct LocalVoid {
    ring: Vec<Point2>,
    lower: f64,
    upper: f64,
}

/// Any semantic void producer can implement the same local-plane contract.
trait VoidProvider {
    fn local_void(
        &self,
        host: &ElementRecord,
        frame: &HostFrame,
        options: GeometryOptions,
    ) -> Result<LocalVoid, GeometryError>;
}
impl VoidProvider for ElementRecord {
    fn local_void(
        &self,
        host: &ElementRecord,
        frame: &HostFrame,
        options: GeometryOptions,
    ) -> Result<LocalVoid, GeometryError> {
        validate_element_geometry(&self.kind, options).map_err(|e| {
            let code = match e.code {
                GeometryErrorCode::SelfIntersection => "void_self_intersection",
                GeometryErrorCode::NonFinite => "void_non_planar",
                GeometryErrorCode::VertexLimitExceeded => "unsupported_host_shape",
                _ => "void_degenerate",
            };
            let mut failure = hosted_error(host.id, self.id, code, e.message);
            failure.code = e.code;
            failure
        })?;
        let (center, elevation, rotation) = match &self.kind {
            ElementKind::Opening(s) => (s.center, s.elevation_m, s.rotation_rad),
            ElementKind::PolygonalOpening(s) => (s.center, s.elevation_m, s.rotation_rad),
            _ => {
                return Err(hosted_error(
                    host.id,
                    self.id,
                    "unsupported_host_shape",
                    "entity does not provide a planar void",
                ));
            }
        };
        let anchor = frame.placement(center, elevation);
        let (sin, cos) = rotation.sin_cos();
        if frame.wall && (cos * frame.u.y - sin * frame.u.x).abs() > 1.0e-6 {
            return Err(hosted_error(
                host.id,
                self.id,
                "void_non_planar",
                "void placement plane must align with the wall",
            ));
        }
        let source_ring = match &self.kind {
            ElementKind::Opening(s) if frame.wall => vec![
                Point2::new(-s.width_m * 0.5, 0.0),
                Point2::new(s.width_m * 0.5, 0.0),
                Point2::new(s.width_m * 0.5, s.height_m),
                Point2::new(-s.width_m * 0.5, s.height_m),
            ],
            ElementKind::Opening(s) => vec![
                Point2::new(-s.width_m * 0.5, -s.depth_m * 0.5),
                Point2::new(s.width_m * 0.5, -s.depth_m * 0.5),
                Point2::new(s.width_m * 0.5, s.depth_m * 0.5),
                Point2::new(-s.width_m * 0.5, s.depth_m * 0.5),
            ],
            ElementKind::PolygonalOpening(s) => validate_planar_contour(&s.boundary, options)
                .map_err(|e| hosted_error(host.id, self.id, "void_degenerate", e.message))?,
            _ => unreachable!(),
        };
        let alignment = cos * frame.u.x + sin * frame.u.y;
        let ring: Vec<_> = source_ring
            .into_iter()
            .map(|p| {
                if frame.wall {
                    Point2::new(anchor.x + p.x * alignment, anchor.y + p.y)
                } else {
                    Point2::new(
                        anchor.x + p.x * cos - p.y * sin,
                        anchor.y + p.x * sin + p.y * cos,
                    )
                }
            })
            .collect();
        let (lower, upper) = if frame.wall {
            let depth = match &self.kind {
                ElementKind::Opening(s) => s.depth_m,
                ElementKind::PolygonalOpening(s) => s.depth_m,
                _ => unreachable!(),
            };
            let across = (center.x - frame.origin.x) * frame.extrusion.x
                + (center.y - frame.origin.y) * frame.extrusion.y;
            (across - depth * 0.5, across + depth * 0.5)
        } else {
            match &self.kind {
                ElementKind::Opening(s) => {
                    // Intersection of all vertical intervals must cover the host.
                    let lower = ring
                        .iter()
                        .map(|p| s.elevation_m - frame.world(*p, 0.0).z)
                        .fold(f64::NEG_INFINITY, f64::max);
                    let upper = ring
                        .iter()
                        .map(|p| s.elevation_m + s.height_m - frame.world(*p, 0.0).z)
                        .fold(f64::INFINITY, f64::min);
                    (lower, upper)
                }
                ElementKind::PolygonalOpening(s) => {
                    let lower = s.elevation_m - frame.world(anchor, 0.0).z;
                    (lower, lower + s.depth_m)
                }
                _ => unreachable!(),
            }
        };
        if ring.iter().any(|p| !p.x.is_finite() || !p.y.is_finite())
            || !lower.is_finite()
            || !upper.is_finite()
        {
            return Err(hosted_error(
                host.id,
                self.id,
                "void_degenerate",
                "void transform exceeds finite numeric range",
            ));
        }
        Ok(LocalVoid { ring, lower, upper })
    }
}

fn ring_edges(ring: &[Point2]) -> impl Iterator<Item = [Point2; 2]> + '_ {
    ring.iter()
        .copied()
        .zip(ring.iter().copied().cycle().skip(1))
        .take(ring.len())
        .map(|(a, b)| [a, b])
}
fn area(ring: &[Point2]) -> f64 {
    ring_edges(ring)
        .map(|[a, b]| a.x * b.y - a.y * b.x)
        .sum::<f64>()
        .abs()
        * 0.5
}
fn base_metrics(frame: &HostFrame) -> (f64, f64, f64, f64) {
    let base_area = area(&frame.outer) - frame.holes.iter().map(|h| area(h)).sum::<f64>();
    let (perimeter, side) = frame.boundary_metrics(
        ring_edges(&frame.outer).chain(frame.holes.iter().flat_map(|h| ring_edges(h))),
    );
    let thickness = frame.upper - frame.lower;
    (
        base_area,
        base_area * thickness * frame.volume_factor(),
        2.0 * base_area * frame.plane_area_factor() + side * thickness,
        perimeter,
    )
}

pub(crate) fn unknown_quantities() -> GeometryQuantities {
    GeometryQuantities {
        quality: RepresentationQuality::ApproximateMesh,
        gross_volume: None,
        net_volume: None,
        gross_surface_area: None,
        net_surface_area: None,
        opening_area: None,
        opening_volume: None,
        perimeter: None,
        length: None,
    }
}

pub(crate) fn uncut_quantities(
    element: &ElementRecord,
    volume: f64,
    surface: f64,
    options: GeometryOptions,
) -> Result<GeometryQuantities, GeometryError> {
    if matches!(
        element.kind,
        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_) | ElementKind::Space(_)
    ) {
        // A semantic void is accounted for by its host, not as material mass.
        return Ok(unknown_quantities());
    }
    let mut q = unknown_quantities();
    q.gross_volume = Some(volume);
    q.net_volume = Some(volume);
    q.gross_surface_area = Some(surface);
    q.net_surface_area = Some(surface);
    if matches!(
        element.kind,
        ElementKind::Wall(_) | ElementKind::Floor(_) | ElementKind::Roof(_)
    ) {
        let frame = HostFrame::new(element, options)?;
        q.perimeter = Some(base_metrics(&frame).3);
        q.length = frame.length;
        q.opening_area = Some(0.0);
        q.opening_volume = Some(0.0);
    }
    Ok(q)
}

fn extrude(
    id: ElementId,
    frame: &HostFrame,
    region: &PlanarRegion,
    lower: f64,
    upper: f64,
) -> Result<Mesh, GeometryError> {
    let mut triangles = Vec::new();
    for cell in &region.cells {
        let top = cell.map(|p| frame.world(p, upper));
        let bottom = cell.map(|p| frame.world(p, lower));
        add_triangle(id, &mut triangles, top[0], top[1], top[2])?;
        add_triangle(id, &mut triangles, top[0], top[2], top[3])?;
        add_triangle(id, &mut triangles, bottom[0], bottom[2], bottom[1])?;
        add_triangle(id, &mut triangles, bottom[0], bottom[3], bottom[2])?;
    }
    for [a, b] in &region.boundary_edges {
        let (a0, b0, a1, b1) = (
            frame.world(*a, lower),
            frame.world(*b, lower),
            frame.world(*a, upper),
            frame.world(*b, upper),
        );
        add_triangle(id, &mut triangles, a0, b0, b1)?;
        add_triangle(id, &mut triangles, a0, b1, a1)?;
    }
    Ok(Mesh::new(triangles))
}

pub(crate) fn derive_host(
    host: &ElementRecord,
    openings: &[&ElementRecord],
    options: GeometryOptions,
) -> Result<DerivedGeometry, GeometryError> {
    let frame = HostFrame::new(host, options).map_err(|_| {
        hosted_error(
            host.id,
            openings[0].id,
            "unsupported_host_shape",
            "host must be a valid planar Wall, Floor or Roof",
        )
    })?;
    let mut voids = Vec::new();
    for opening in openings {
        let local = opening.local_void(host, &frame, options)?;
        if local.lower > frame.lower + options.linear_tolerance_m
            || local.upper < frame.upper - options.linear_tolerance_m
        {
            return Err(hosted_error(
                host.id,
                opening.id,
                "void_depth_insufficient",
                "void must span the full host extrusion interval",
            ));
        }
        voids.push(local.ring);
    }
    let region = subtract_regions(
        &frame.outer,
        &frame.holes,
        &voids,
        options.linear_tolerance_m,
    )
    .map_err(|e| {
        hosted_error(
            host.id,
            openings[e.void_index.unwrap_or(0)].id,
            e.code,
            e.message,
        )
    })?;
    let mesh = extrude(host.id, &frame, &region, frame.lower, frame.upper).map_err(|mut e| {
        e.host_id = Some(host.id);
        e.opening_id = Some(openings[0].id);
        e
    })?;
    let (base_area, gross_volume, gross_surface, _) = base_metrics(&frame);
    let (perimeter, side) = frame.boundary_metrics(region.boundary_edges.iter().copied());
    let depth = frame.upper - frame.lower;
    let volume = region.area * depth * frame.volume_factor();
    let surface = 2.0 * region.area * frame.plane_area_factor() + side * depth;
    let opening_area = (base_area - region.area).max(0.0) * frame.plane_area_factor();
    let quantities = GeometryQuantities {
        quality: RepresentationQuality::ApproximateMesh,
        gross_volume: Some(gross_volume),
        net_volume: Some(volume),
        gross_surface_area: Some(gross_surface),
        net_surface_area: Some(surface),
        opening_area: Some(opening_area),
        opening_volume: Some((gross_volume - volume).max(0.0)),
        perimeter: Some(perimeter),
        length: frame.length,
    };
    Ok(DerivedGeometry {
        element_id: host.id,
        bounds: mesh.bounds(),
        mesh,
        representation_quality: RepresentationQuality::ApproximateMesh,
        volume: Some(volume),
        surface_area: Some(surface),
        quantities,
    })
}

pub(crate) fn opening_proxy(
    host: &ElementRecord,
    opening: &ElementRecord,
    options: GeometryOptions,
) -> Result<DerivedGeometry, GeometryError> {
    let frame = HostFrame::new(host, options).map_err(|e| {
        let mut failure = hosted_error(host.id, opening.id, "unsupported_host_shape", e.message);
        failure.code = e.code;
        failure
    })?;
    let local = opening.local_void(host, &frame, options)?;
    let region = subtract_regions(&local.ring, &[], &[], options.linear_tolerance_m)
        .map_err(|e| hosted_error(host.id, opening.id, e.code, e.message))?;
    let mesh = extrude(opening.id, &frame, &region, local.lower, local.upper)?;
    let (volume, surface) = mesh_quantities(&mesh);
    Ok(DerivedGeometry {
        element_id: opening.id,
        bounds: mesh.bounds(),
        mesh,
        representation_quality: RepresentationQuality::ApproximateMesh,
        volume: Some(volume),
        surface_area: Some(surface),
        quantities: unknown_quantities(),
    })
}
