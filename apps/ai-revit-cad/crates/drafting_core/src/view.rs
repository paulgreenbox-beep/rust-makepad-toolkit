use crate::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, f64::consts::TAU};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera2D {
    pub center: Point2,
    pub pixels_per_metre: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub revision: u64,
}
impl Default for Camera2D {
    fn default() -> Self {
        Self::new(1000., 700.)
    }
}
impl Camera2D {
    pub fn new(viewport_width: f64, viewport_height: f64) -> Self {
        Self {
            center: Point2::default(),
            pixels_per_metre: 48.,
            viewport_width,
            viewport_height,
            revision: 0,
        }
    }
    pub fn validate(&self) -> DraftResult<()> {
        if !self.center.is_finite()
            || self.center.x.abs() > 1e12
            || self.center.y.abs() > 1e12
            || !self.pixels_per_metre.is_finite()
            || !(1e-9..=1e12).contains(&self.pixels_per_metre)
            || !self.viewport_width.is_finite()
            || !self.viewport_height.is_finite()
            || self.viewport_width <= 0.
            || self.viewport_height <= 0.
            || self.viewport_width > 32768.
            || self.viewport_height > 32768.
        {
            return Err(DraftError::new(
                "invalid_camera",
                "camera has invalid center, zoom or viewport dimensions",
            ));
        }
        Ok(())
    }
    pub fn world_to_screen(&self, p: Point2) -> Point2 {
        Point2::new(
            self.viewport_width * 0.5 + (p.x - self.center.x) * self.pixels_per_metre,
            self.viewport_height * 0.5 - (p.y - self.center.y) * self.pixels_per_metre,
        )
    }
    pub fn screen_to_world(&self, p: Point2) -> Point2 {
        Point2::new(
            self.center.x + (p.x - self.viewport_width * 0.5) / self.pixels_per_metre,
            self.center.y - (p.y - self.viewport_height * 0.5) / self.pixels_per_metre,
        )
    }
    fn mutate(&mut self, mut next: Self) -> DraftResult<()> {
        next.validate()?;
        next.revision = self.revision.checked_add(1).ok_or_else(|| {
            DraftError::new("camera_revision_exhausted", "camera revision exhausted")
        })?;
        *self = next;
        Ok(())
    }
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) -> DraftResult<()> {
        self.pan_delta(Point2::new(dx, dy))
    }
    pub fn pan_delta(&mut self, delta: Point2) -> DraftResult<()> {
        self.validate()?;
        if !delta.is_finite() {
            return Err(DraftError::new("invalid_camera", "pan delta is not finite"));
        }
        let mut next = self.clone();
        next.center.x -= delta.x / self.pixels_per_metre;
        next.center.y += delta.y / self.pixels_per_metre;
        self.mutate(next)
    }
    pub fn zoom_at(&mut self, screen: Point2, factor: f64) -> DraftResult<()> {
        self.validate()?;
        if !screen.is_finite() || !factor.is_finite() || factor <= 0. {
            return Err(DraftError::new(
                "invalid_camera",
                "zoom anchor/factor invalid",
            ));
        }
        let world = self.screen_to_world(screen);
        let mut next = self.clone();
        next.pixels_per_metre = (self.pixels_per_metre * factor).clamp(1e-9, 1e12);
        next.center.x = world.x - (screen.x - self.viewport_width * 0.5) / next.pixels_per_metre;
        next.center.y = world.y + (screen.y - self.viewport_height * 0.5) / next.pixels_per_metre;
        self.mutate(next)
    }
    pub fn resize(&mut self, width: f64, height: f64) -> DraftResult<()> {
        let mut next = self.clone();
        next.viewport_width = width;
        next.viewport_height = height;
        self.mutate(next)
    }
    pub fn fit_bounds(&mut self, bounds: Bounds2, padding_px: f64) -> DraftResult<()> {
        self.validate()?;
        if !bounds.min.is_finite()
            || !bounds.max.is_finite()
            || bounds.max.x < bounds.min.x
            || bounds.max.y < bounds.min.y
            || !padding_px.is_finite()
            || padding_px < 0.
            || padding_px * 2. >= self.viewport_width.min(self.viewport_height)
        {
            return Err(DraftError::new(
                "invalid_camera",
                "fit bounds or padding invalid",
            ));
        }
        let width = (bounds.max.x - bounds.min.x).max(1e-9);
        let height = (bounds.max.y - bounds.min.y).max(1e-9);
        let mut next = self.clone();
        next.center = bounds.center();
        next.pixels_per_metre = ((self.viewport_width - 2. * padding_px) / width)
            .min((self.viewport_height - 2. * padding_px) / height)
            .clamp(1e-9, 1e12);
        self.mutate(next)
    }
    pub fn viewport(&self) -> Bounds2 {
        Bounds2::from_points(
            Point2::default(),
            Point2::new(self.viewport_width, self.viewport_height),
        )
    }
    pub fn world_bounds(&self) -> Bounds2 {
        Bounds2::from_points(
            self.screen_to_world(Point2::default()),
            self.screen_to_world(Point2::new(self.viewport_width, self.viewport_height)),
        )
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayOptions {
    pub generation: u64,
    pub selected_ids: Vec<DraftEntityId>,
    pub hover_id: Option<DraftEntityId>,
    /// Maximum chord deviation in physical pixels.
    pub pixel_error: f64,
    pub dpi_factor: f64,
    pub max_segments_per_curve: usize,
    pub max_primitives: usize,
}
impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            generation: 0,
            selected_ids: Vec::new(),
            hover_id: None,
            pixel_error: 0.5,
            dpi_factor: 1.,
            max_segments_per_curve: 4096,
            max_primitives: 100_000,
        }
    }
}
impl DisplayOptions {
    pub fn validate(&self) -> DraftResult<()> {
        if !self.pixel_error.is_finite()
            || !(0.01..=16.).contains(&self.pixel_error)
            || !self.dpi_factor.is_finite()
            || !(0.25..=8.).contains(&self.dpi_factor)
            || self.max_segments_per_curve == 0
            || self.max_segments_per_curve > 65536
            || self.max_primitives == 0
            || self.max_primitives > 1_000_000
            || self.selected_ids.len() > MAX_DOCUMENT_ENTITIES
        {
            return Err(DraftError::new(
                "display_budget",
                "invalid pixel quality or bounded display budgets",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenSegment {
    pub start: Point2,
    pub end: Point2,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderPrimitive {
    pub primitive_id: RenderPrimitiveId,
    pub owner_entity_id: DraftEntityId,
    pub drawing_id: DrawingId,
    pub source_revision: u64,
    pub start: Point2,
    pub end: Point2,
    pub appearance: DraftStyle,
    pub selected: bool,
    pub hovered: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplaySnapshot {
    pub token: String,
    pub document_id: String,
    pub document_revision: u64,
    pub drawing_id: DrawingId,
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub camera: Camera2D,
    pub viewport: Bounds2,
    pub selected_ids: Vec<DraftEntityId>,
    pub hover_id: Option<DraftEntityId>,
    pub visible_layer_ids: Vec<LayerId>,
    pub analytic_entities: Vec<DraftEntity>,
    pub primitives: Vec<RenderPrimitive>,
    pub pixel_error: f64,
    pub dpi_factor: f64,
    pub representation_quality: String,
}
pub type DraftingDisplayList = DisplaySnapshot;
impl DisplaySnapshot {
    pub fn is_current(
        &self,
        snapshot: &DraftSnapshot,
        camera: &Camera2D,
        options: &DisplayOptions,
    ) -> DraftResult<bool> {
        Ok(self.token == snapshot_token(snapshot, camera, options)?)
    }
    pub fn require_current(
        &self,
        snapshot: &DraftSnapshot,
        camera: &Camera2D,
        options: &DisplayOptions,
    ) -> DraftResult<()> {
        if !self.is_current(snapshot, camera, options)? {
            return Err(DraftError::new(
                "stale_snapshot",
                "display token does not match current document/view state",
            ));
        }
        Ok(())
    }
}
fn snapshot_token(
    snapshot: &DraftSnapshot,
    camera: &Camera2D,
    options: &DisplayOptions,
) -> DraftResult<String> {
    let bytes = serde_json::to_vec(&(
        snapshot.document_id.as_str(),
        snapshot.document_revision,
        snapshot.drawing_id,
        snapshot.revision,
        snapshot.source_fingerprint.as_str(),
        camera,
        options,
        &snapshot.layers,
    ))
    .map_err(|e| DraftError::new("serialization", e.to_string()))?;
    Ok(format!(
        "{}:d{}:r{}:c{}:g{}:{}",
        snapshot.document_id,
        snapshot.drawing_id,
        snapshot.document_revision,
        camera.revision,
        options.generation,
        fingerprint(&bytes)
    ))
}
pub fn tessellate_geometry(
    geometry: &EntityGeometry,
    camera: &Camera2D,
    options: &DisplayOptions,
) -> DraftResult<Vec<ScreenSegment>> {
    tessellate_bounded(geometry, camera, options).map(|(segments, _)| segments)
}
fn tessellate_bounded(
    geometry: &EntityGeometry,
    camera: &Camera2D,
    options: &DisplayOptions,
) -> DraftResult<(Vec<ScreenSegment>, usize)> {
    camera.validate()?;
    options.validate()?;
    geometry.validate(GeometryLimits::default())?;
    let mut segments = Vec::new();
    let mut work = 0usize;
    let mut append = |a: Point2, b: Point2| -> DraftResult<()> {
        if work >= options.max_primitives {
            return Err(DraftError::new(
                "display_budget",
                "geometry exceeds generated segment/work budget",
            ));
        }
        work += 1;
        if !a.is_finite() || !b.is_finite() {
            return Err(DraftError::new(
                "render_overflow",
                "screen coordinate overflow",
            ));
        }
        if let Some(segment) = clip_segment(ScreenSegment { start: a, end: b }, camera.viewport()) {
            segments.push(segment);
        }
        Ok(())
    };
    match geometry {
        EntityGeometry::Line { start, end } => {
            append(camera.world_to_screen(*start), camera.world_to_screen(*end))?
        }
        EntityGeometry::Circle { center, radius } => tessellate_arc(
            AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: 0.,
                sweep_angle: TAU,
            },
            camera,
            options,
            &mut append,
        )?,
        EntityGeometry::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => tessellate_arc(
            AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            },
            camera,
            options,
            &mut append,
        )?,
        EntityGeometry::Polyline { vertices, closed } => {
            let count = if *closed {
                vertices.len()
            } else {
                vertices.len() - 1
            };
            for i in 0..count {
                let a = vertices[i];
                let b = vertices[(i + 1) % vertices.len()];
                match bulge_curve(a.point, b.point, a.bulge)? {
                    Some(arc) => tessellate_bulge(arc, camera, options, &mut append)?,
                    None => append(
                        camera.world_to_screen(a.point),
                        camera.world_to_screen(b.point),
                    )?,
                }
            }
        }
    }
    Ok((segments, work))
}
fn tessellate_arc(
    arc: AnalyticArc,
    camera: &Camera2D,
    options: &DisplayOptions,
    append: &mut impl FnMut(Point2, Point2) -> DraftResult<()>,
) -> DraftResult<()> {
    let count = curve_segment_count(arc.radius, arc.sweep_angle, camera, options)?;
    // Rebase the analytic point before scaling. Projecting center and radius
    // separately can discard the viewport offset when both screen values are
    // much larger than the viewport (large drawing origin at high zoom).
    let project = |t: f64| {
        let p = arc.point_relative(camera.center, t);
        Point2::new(
            camera.viewport_width * 0.5 + p.x * camera.pixels_per_metre,
            camera.viewport_height * 0.5 - p.y * camera.pixels_per_metre,
        )
    };
    let mut previous = project(0.);
    for i in 1..=count {
        let next = project(i as f64 / count as f64);
        append(previous, next)?;
        previous = next;
    }
    Ok(())
}
fn curve_segment_count(
    radius: f64,
    sweep_angle: f64,
    camera: &Camera2D,
    options: &DisplayOptions,
) -> DraftResult<usize> {
    let radius_px = radius * camera.pixels_per_metre * options.dpi_factor;
    if !radius_px.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "projected radius overflow",
        ));
    }
    // 4*asin(sqrt(e/(2r))) avoids cancellation in acos(1-e/r) at high zoom.
    let max_angle = 4.
        * (options.pixel_error / (2. * radius_px))
            .clamp(0., 1.)
            .sqrt()
            .asin();
    let needed = (sweep_angle.abs() / max_angle.max(f64::MIN_POSITIVE)).ceil();
    let minimum = if sweep_angle.abs() >= TAU - 1e-12 {
        4
    } else {
        1
    };
    if needed > options.max_segments_per_curve as f64 {
        return Err(DraftError::new(
            "tessellation_budget",
            format!("curve requires {needed} segments for requested pixel error"),
        ));
    }
    let count = (needed as usize).max(minimum);
    if count > options.max_segments_per_curve {
        return Err(DraftError::new(
            "tessellation_budget",
            "curve minimum exceeds budget",
        ));
    }
    Ok(count)
}
fn tessellate_bulge(
    arc: BulgeArc,
    camera: &Camera2D,
    options: &DisplayOptions,
    append: &mut impl FnMut(Point2, Point2) -> DraftResult<()>,
) -> DraftResult<()> {
    let count = curve_segment_count(arc.radius, arc.sweep_angle, camera, options)?;
    let project = |t: f64| {
        let p = arc.point_relative(camera.center, t);
        Point2::new(
            camera.viewport_width * 0.5 + p.x * camera.pixels_per_metre,
            camera.viewport_height * 0.5 - p.y * camera.pixels_per_metre,
        )
    };
    let mut previous = project(0.);
    for i in 1..=count {
        let next = project(i as f64 / count as f64);
        append(previous, next)?;
        previous = next;
    }
    Ok(())
}
/// Shared bounded display clipping for derived BIM underlay adapters.
/// This does not create a drafting entity or mutate analytic authority.
pub fn clip_display_segment(segment: ScreenSegment, bounds: Bounds2) -> Option<ScreenSegment> {
    clip_segment(segment, bounds)
}
fn clip_segment(segment: ScreenSegment, bounds: Bounds2) -> Option<ScreenSegment> {
    let dx = segment.end.x - segment.start.x;
    let dy = segment.end.y - segment.start.y;
    let length = dx.hypot(dy);
    if length == 0. {
        return bounds
            .intersects(Bounds2::from_points(segment.start, segment.start))
            .then_some(segment);
    }
    let ux = dx / length;
    let uy = dy / length;
    let nx = -uy;
    let ny = ux;
    let center = bounds.center();
    let a = Point2::new(segment.start.x - center.x, segment.start.y - center.y);
    let b = Point2::new(segment.end.x - center.x, segment.end.y - center.y);
    let near = if a.x.hypot(a.y) <= b.x.hypot(b.y) {
        a
    } else {
        b
    };
    // Re-anchor the infinite line close to the viewport. Evaluating a+t*(b-a)
    // directly loses the whole canvas interval when endpoints project to 1e21.
    // Compensated products retain the small normal distance after cancellation.
    let p = nx * near.x;
    let q = ny * near.y;
    let sum = p + q;
    let z = sum - p;
    let error = nx.mul_add(near.x, -p) + ny.mul_add(near.y, -q) + (p - (sum - z)) + (q - z);
    let distance = sum + error;
    let normal_limit = nx.abs() * (bounds.max.x - bounds.min.x) * 0.5
        + ny.abs() * (bounds.max.y - bounds.min.y) * 0.5;
    if distance.abs() > normal_limit + 1e-7 {
        return None;
    }
    let anchor = Point2::new(center.x + nx * distance, center.y + ny * distance);
    let sa = (segment.start.x - anchor.x) * ux + (segment.start.y - anchor.y) * uy;
    let sb = (segment.end.x - anchor.x) * ux + (segment.end.y - anchor.y) * uy;
    let mut lower = sa.min(sb);
    let mut upper = sa.max(sb);
    for (origin, direction, min, max) in [
        (anchor.x, ux, bounds.min.x, bounds.max.x),
        (anchor.y, uy, bounds.min.y, bounds.max.y),
    ] {
        if direction == 0. {
            if origin < min || origin > max {
                return None;
            }
        } else {
            let a = (min - origin) / direction;
            let b = (max - origin) / direction;
            lower = lower.max(a.min(b));
            upper = upper.min(a.max(b));
            if lower > upper {
                return None;
            }
        }
    }
    let endpoint = |s: f64| {
        Point2::new(
            (anchor.x + ux * s).clamp(bounds.min.x, bounds.max.x),
            (anchor.y + uy * s).clamp(bounds.min.y, bounds.max.y),
        )
    };
    let (first, last) = if sa <= sb {
        (lower, upper)
    } else {
        (upper, lower)
    };
    Some(ScreenSegment {
        start: endpoint(first),
        end: endpoint(last),
    })
}
pub fn build_display(
    snapshot: &DraftSnapshot,
    camera: &Camera2D,
    options: &DisplayOptions,
) -> DraftResult<DisplaySnapshot> {
    camera.validate()?;
    options.validate()?;
    let selected: BTreeSet<_> = options.selected_ids.iter().copied().collect();
    if selected
        .iter()
        .any(|id| !snapshot.entities.contains_key(id))
        || options
            .hover_id
            .is_some_and(|id| !snapshot.entities.contains_key(&id))
    {
        return Err(DraftError::new(
            "entity_not_found",
            "selection or hover refers to absent entity",
        ));
    }
    let visible_layer_ids: Vec<_> = snapshot
        .layers
        .values()
        .filter(|l| l.visible)
        .map(|l| l.id)
        .collect();
    let mut analytic_entities = Vec::new();
    let mut primitives = Vec::new();
    let mut total_work = 0usize;
    let world_bounds = camera.world_bounds();
    for entity in snapshot.entities.values() {
        if !snapshot
            .layers
            .get(&entity.layer_id)
            .is_some_and(|l| l.visible)
        {
            continue;
        }
        // Include visible analytic source data even if the current viewport culls it.
        analytic_entities.push(entity.clone());
        if !entity.geometry.bounds()?.intersects(world_bounds) {
            continue;
        }
        let mut entity_options = options.clone();
        if total_work >= options.max_primitives {
            return Err(DraftError::new(
                "display_budget",
                "snapshot generated segment/work budget exceeded",
            ));
        }
        entity_options.max_primitives = options.max_primitives - total_work;
        let (segments, work) = tessellate_bounded(&entity.geometry, camera, &entity_options)?;
        total_work += work;
        if primitives
            .len()
            .checked_add(segments.len())
            .is_none_or(|n| n > options.max_primitives)
        {
            return Err(DraftError::new(
                "display_budget",
                "snapshot primitive budget exceeded",
            ));
        }
        let is_selected = selected.contains(&entity.id);
        let hovered = options.hover_id == Some(entity.id);
        let mut appearance = entity.style.clone();
        if is_selected {
            appearance.color = [1., 0.74, 0.22, 1.];
            appearance.width_px += 1.;
        } else if hovered {
            appearance.color = [0.35, 1., 0.9, 1.];
            appearance.width_px += 0.5;
        }
        for segment in segments {
            let id = RenderPrimitiveId::new(primitives.len() as u64 + 1);
            primitives.push(RenderPrimitive {
                primitive_id: id,
                owner_entity_id: entity.id,
                drawing_id: snapshot.drawing_id,
                source_revision: snapshot.revision,
                start: segment.start,
                end: segment.end,
                appearance: appearance.clone(),
                selected: is_selected,
                hovered,
            });
        }
    }
    Ok(DisplaySnapshot {
        token: snapshot_token(snapshot, camera, options)?,
        document_id: snapshot.document_id.clone(),
        document_revision: snapshot.document_revision,
        drawing_id: snapshot.drawing_id,
        source_revision: snapshot.revision,
        source_fingerprint: snapshot.source_fingerprint.clone(),
        camera: camera.clone(),
        viewport: camera.viewport(),
        selected_ids: selected.into_iter().collect(),
        hover_id: options.hover_id,
        visible_layer_ids,
        analytic_entities,
        primitives,
        pixel_error: options.pixel_error,
        dpi_factor: options.dpi_factor,
        representation_quality: "analytic_2d;display_tessellated".into(),
    })
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickHit {
    pub entity_id: DraftEntityId,
    pub distance_px: f64,
    pub locked: bool,
}
/// Sorted AABB broad-phase index. A query uses a binary prefix and exact bounds
/// filtering; analytic narrow-phase never relies on render buffer order.
#[derive(Clone, Debug)]
pub struct SpatialIndex2D {
    items: Vec<(Bounds2, DraftEntityId)>,
    document_id: String,
    drawing_id: DrawingId,
    source_fingerprint: String,
}
impl SpatialIndex2D {
    pub fn build(snapshot: &DraftSnapshot) -> DraftResult<Self> {
        let mut items = Vec::new();
        for entity in snapshot.entities.values() {
            if snapshot
                .layers
                .get(&entity.layer_id)
                .is_some_and(|l| l.visible)
            {
                items.push((entity.geometry.bounds()?, entity.id));
            }
        }
        items.sort_by(|a, b| a.0.min.x.total_cmp(&b.0.min.x).then(a.1.cmp(&b.1)));
        Ok(Self {
            items,
            document_id: snapshot.document_id.clone(),
            drawing_id: snapshot.drawing_id,
            source_fingerprint: snapshot.source_fingerprint.clone(),
        })
    }
    pub fn candidates(&self, bounds: Bounds2) -> Vec<DraftEntityId> {
        let end = self.items.partition_point(|(b, _)| b.min.x <= bounds.max.x);
        self.items[..end]
            .iter()
            .filter(|(b, _)| b.intersects(bounds))
            .map(|(_, id)| *id)
            .collect()
    }
    fn candidates_relative(&self, bounds: Bounds2, origin: Point2) -> Vec<DraftEntityId> {
        // The items remain ordered after subtracting the same origin. Translate
        // their bounds before comparing so a sub-ULP screen aperture at a large
        // world coordinate does not collapse onto the camera center.
        let end = self
            .items
            .partition_point(|(b, _)| b.min.x - origin.x <= bounds.max.x);
        self.items[..end]
            .iter()
            .filter(|(b, _)| {
                Bounds2 {
                    min: Point2::new(b.min.x - origin.x, b.min.y - origin.y),
                    max: Point2::new(b.max.x - origin.x, b.max.y - origin.y),
                }
                .intersects(bounds)
            })
            .map(|(_, id)| *id)
            .collect()
    }
}
pub fn pick(
    snapshot: &DraftSnapshot,
    camera: &Camera2D,
    screen: Point2,
    tolerance_px: f64,
) -> DraftResult<Vec<PickHit>> {
    let index = SpatialIndex2D::build(snapshot)?;
    pick_with_index(snapshot, &index, camera, screen, tolerance_px)
}
pub fn pick_with_index(
    snapshot: &DraftSnapshot,
    index: &SpatialIndex2D,
    camera: &Camera2D,
    screen: Point2,
    tolerance_px: f64,
) -> DraftResult<Vec<PickHit>> {
    if snapshot.document_id != index.document_id
        || snapshot.drawing_id != index.drawing_id
        || snapshot.source_fingerprint != index.source_fingerprint
    {
        return Err(DraftError::new(
            "stale_pick_index",
            "spatial index does not match source snapshot",
        ));
    }
    camera.validate()?;
    if !screen.is_finite() || !tolerance_px.is_finite() || !(0.0..=128.).contains(&tolerance_px) {
        return Err(DraftError::new(
            "invalid_pick",
            "pick point or aperture invalid",
        ));
    }
    if screen.x < 0.
        || screen.y < 0.
        || screen.x > camera.viewport_width
        || screen.y > camera.viewport_height
    {
        return Ok(Vec::new());
    }
    let camera_relative = Point2::new(
        (screen.x - camera.viewport_width * 0.5) / camera.pixels_per_metre,
        (camera.viewport_height * 0.5 - screen.y) / camera.pixels_per_metre,
    );
    let aperture = tolerance_px / camera.pixels_per_metre;
    let bounds = Bounds2::from_points(camera_relative, camera_relative).expanded(aperture);
    let mut hits = Vec::new();
    for id in index.candidates_relative(bounds, camera.center) {
        let entity = snapshot.entity(id)?;
        let Some(layer) = snapshot.layers.get(&entity.layer_id) else {
            continue;
        };
        if !layer.visible {
            continue;
        }
        let distance_px = entity
            .geometry
            .distance_relative(camera.center, camera_relative)?
            * camera.pixels_per_metre;
        if distance_px.is_finite() && distance_px <= tolerance_px + 1e-8 {
            hits.push(PickHit {
                entity_id: id,
                distance_px,
                locked: layer.locked,
            });
        }
    }
    hits.sort_by(|a, b| {
        a.distance_px
            .total_cmp(&b.distance_px)
            .then(a.entity_id.cmp(&b.entity_id))
    });
    Ok(hits)
}
