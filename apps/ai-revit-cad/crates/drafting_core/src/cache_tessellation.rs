use crate::{
    AnalyticArc, Bounds2, BulgeArc, Camera2D, DraftError, DraftResult, EntityGeometry, GeometryLimits,
    Point2, ScreenSegment, bulge_curve,
};
use std::mem::size_of;
use std::f64::consts::TAU;

/// Camera independent tessellation data for one entity.
///
/// Straight segments keep their original world endpoints. Curves keep sampled
/// f64 offsets from a per-curve anchor, plus an analytic parameterization for
/// rare projection cases where rebasing would lose too much precision.
#[derive(Clone, Debug)]
pub struct TessellatedGeometry {
    chunks: Vec<TessellationChunk>,
    generated_segments: usize,
    quality_pixels_per_metre: f64,
    dpi_factor: f64,
    pixel_error: f64,
}

#[derive(Clone, Debug)]
enum TessellationChunk {
    Line {
        start: Point2,
        end: Point2,
    },
    Arc {
        arc: AnalyticArc,
        interior_offsets: Vec<Point2>,
        max_offset_component: f64,
    },
    Bulge {
        arc: BulgeArc,
        start: Point2,
        end: Point2,
        interior_offsets: Vec<Point2>,
        max_offset_component: f64,
    },
}

impl TessellatedGeometry {
    /// Number of generated source segments, including segments later clipped
    /// from a particular camera's viewport.
    pub fn generated_segments(&self) -> usize {
        self.generated_segments
    }

    /// Approximate owned memory, including the chunk vector's reserved slots.
    pub fn estimated_bytes(&self) -> usize {
        self.chunks.iter().fold(
            size_of::<Self>().saturating_add(
                self.chunks
                    .capacity()
                    .saturating_mul(size_of::<TessellationChunk>()),
            ),
            |bytes, chunk| {
                let nested = match chunk {
                    TessellationChunk::Arc {
                        interior_offsets, ..
                    }
                    | TessellationChunk::Bulge {
                        interior_offsets, ..
                    } => interior_offsets
                        .capacity()
                        .saturating_mul(size_of::<Point2>()),
                    TessellationChunk::Line { .. } => 0,
                };
                bytes.saturating_add(nested)
            },
        )
    }

    /// Number of segments that need analytic point evaluation because local
    /// sample offsets lose too much precision when rebased at this camera.
    pub fn fallback_segments(&self, camera: &Camera2D) -> DraftResult<usize> {
        self.validate_projection_camera(camera)?;
        let mut count = 0usize;
        for chunk in &self.chunks {
            let fallback = match chunk {
                TessellationChunk::Arc {
                    arc,
                    max_offset_component,
                    ..
                } => needs_analytic_fallback(
                    arc.center,
                    *max_offset_component,
                    camera,
                    self.quality_pixels_per_metre,
                    self.dpi_factor,
                    self.pixel_error,
                ),
                TessellationChunk::Bulge {
                    arc,
                    max_offset_component,
                    ..
                } => needs_analytic_fallback(
                    arc.start,
                    *max_offset_component,
                    camera,
                    self.quality_pixels_per_metre,
                    self.dpi_factor,
                    self.pixel_error,
                ),
                TessellationChunk::Line { .. } => false,
            };
            if fallback {
                count = count
                    .checked_add(chunk_segment_count(chunk))
                    .ok_or_else(|| {
                        DraftError::new("display_budget", "fallback segment count overflow")
                    })?;
            }
        }
        Ok(count)
    }

    fn validate_projection_camera(&self, camera: &Camera2D) -> DraftResult<()> {
        camera.validate()?;
        if camera.pixels_per_metre > self.quality_pixels_per_metre {
            return Err(DraftError::new(
                "tessellation_quality",
                "camera zoom exceeds the tessellation quality scale",
            ));
        }
        Ok(())
    }

    /// Project and clip the camera independent segments to the viewport
    /// expanded by `margin_px` on each side.
    pub fn project(&self, camera: &Camera2D, margin_px: f64) -> DraftResult<Vec<ScreenSegment>> {
        self.validate_projection_camera(camera)?;
        if !margin_px.is_finite() || !(0.0..=65_536.0).contains(&margin_px) {
            return Err(DraftError::new(
                "display_budget",
                "projection margin must be finite and bounded",
            ));
        }
        let clip_bounds = camera.viewport().expanded(margin_px);
        if !clip_bounds.min.is_finite() || !clip_bounds.max.is_finite() {
            return Err(DraftError::new(
                "render_overflow",
                "expanded viewport is not finite",
            ));
        }

        let mut projected = Vec::new();
        for chunk in &self.chunks {
            match chunk {
                TessellationChunk::Line { start, end } => {
                    append_projected_world_line(
                        &mut projected,
                        camera,
                        clip_bounds,
                        *start,
                        *end,
                    )?;
                }
                TessellationChunk::Arc {
                    arc,
                    interior_offsets,
                    max_offset_component,
                } => {
                    let fallback = needs_analytic_fallback(
                        arc.center,
                        *max_offset_component,
                        camera,
                        self.quality_pixels_per_metre,
                        self.dpi_factor,
                        self.pixel_error,
                    );
                    project_curve(
                        &mut projected,
                        camera,
                        clip_bounds,
                        arc.center,
                        CurveEndpoint::AnalyticArc { arc: *arc, t: 0.0 },
                        CurveEndpoint::AnalyticArc { arc: *arc, t: 1.0 },
                        interior_offsets,
                        fallback.then_some(CurveParameterization::Arc(*arc)),
                    )?;
                }
                TessellationChunk::Bulge {
                    arc,
                    start,
                    end,
                    interior_offsets,
                    max_offset_component,
                } => {
                    let fallback = needs_analytic_fallback(
                        arc.start,
                        *max_offset_component,
                        camera,
                        self.quality_pixels_per_metre,
                        self.dpi_factor,
                        self.pixel_error,
                    );
                    project_curve(
                        &mut projected,
                        camera,
                        clip_bounds,
                        arc.start,
                        CurveEndpoint::World(*start),
                        CurveEndpoint::World(*end),
                        interior_offsets,
                        fallback.then_some(CurveParameterization::Bulge(*arc)),
                    )?;
                }
            }
        }
        Ok(projected)
    }
}

pub(crate) fn build_tessellation(
    geometry: &EntityGeometry,
    pixels_per_metre: f64,
    dpi_factor: f64,
    pixel_error: f64,
    max_segments_per_curve: usize,
    max_segments: usize,
) -> DraftResult<TessellatedGeometry> {
    if !pixels_per_metre.is_finite()
        || !(1e-9..=1e12).contains(&pixels_per_metre)
        || !dpi_factor.is_finite()
        || !(0.25..=8.0).contains(&dpi_factor)
        || !pixel_error.is_finite()
        || !(0.01..=16.0).contains(&pixel_error)
        || max_segments_per_curve == 0
        || max_segments_per_curve > 65_536
        || max_segments == 0
        || max_segments > 1_000_000
    {
        return Err(DraftError::new(
            "display_budget",
            "invalid tessellation quality or bounded segment budgets",
        ));
    }
    geometry.validate(GeometryLimits::default())?;

    let mut chunks = Vec::new();
    let mut generated_segments = 0usize;
    match geometry {
        EntityGeometry::Line { start, end } => push_chunk(
            &mut chunks,
            &mut generated_segments,
            max_segments,
            1,
            TessellationChunk::Line {
                start: *start,
                end: *end,
            },
        )?,
        EntityGeometry::Circle { center, radius } => {
            let arc = AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: 0.0,
                sweep_angle: TAU,
            };
            let count = curve_segment_count(
                arc.radius,
                arc.sweep_angle,
                pixels_per_metre,
                dpi_factor,
                pixel_error,
                max_segments_per_curve,
            )?;
            push_arc_chunk(
                &mut chunks,
                &mut generated_segments,
                max_segments,
                count,
                arc,
            )?;
        }
        EntityGeometry::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let arc = AnalyticArc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            };
            let count = curve_segment_count(
                arc.radius,
                arc.sweep_angle,
                pixels_per_metre,
                dpi_factor,
                pixel_error,
                max_segments_per_curve,
            )?;
            push_arc_chunk(
                &mut chunks,
                &mut generated_segments,
                max_segments,
                count,
                arc,
            )?;
        }
        EntityGeometry::Polyline { vertices, closed } => {
            let segment_count = if *closed {
                vertices.len()
            } else {
                vertices.len() - 1
            };
            for index in 0..segment_count {
                let start = vertices[index];
                let end = vertices[(index + 1) % vertices.len()];
                match bulge_curve(start.point, end.point, start.bulge)? {
                    Some(arc) => {
                        let count = curve_segment_count(
                            arc.radius,
                            arc.sweep_angle,
                            pixels_per_metre,
                            dpi_factor,
                            pixel_error,
                            max_segments_per_curve,
                        )?;
                        push_bulge_chunk(
                            &mut chunks,
                            &mut generated_segments,
                            max_segments,
                            count,
                            arc,
                        )?;
                    }
                    None => push_chunk(
                        &mut chunks,
                        &mut generated_segments,
                        max_segments,
                        1,
                        TessellationChunk::Line {
                            start: start.point,
                            end: end.point,
                        },
                    )?,
                }
            }
        }
    }

    Ok(TessellatedGeometry {
        chunks,
        generated_segments,
        quality_pixels_per_metre: pixels_per_metre,
        dpi_factor,
        pixel_error,
    })
}

fn push_chunk(
    chunks: &mut Vec<TessellationChunk>,
    generated_segments: &mut usize,
    max_segments: usize,
    chunk_segments: usize,
    chunk: TessellationChunk,
) -> DraftResult<()> {
    let next_total = checked_total(*generated_segments, chunk_segments, max_segments)?;
    chunks.try_reserve(1).map_err(|_| {
        DraftError::new("display_budget", "unable to reserve tessellation storage")
    })?;
    chunks.push(chunk);
    *generated_segments = next_total;
    Ok(())
}

fn push_arc_chunk(
    chunks: &mut Vec<TessellationChunk>,
    generated_segments: &mut usize,
    max_segments: usize,
    count: usize,
    arc: AnalyticArc,
) -> DraftResult<()> {
    let next_total = checked_total(*generated_segments, count, max_segments)?;
    let mut interior_offsets = Vec::new();
    let mut max_offset_component = 0.0f64;
    interior_offsets
        .try_reserve_exact(count.saturating_sub(1))
        .map_err(|_| {
            DraftError::new("display_budget", "unable to reserve curve sample storage")
        })?;
    for index in 1..count {
        let direction = arc.direction(index as f64 / count as f64);
        let offset = Point2::new(arc.radius * direction.x, arc.radius * direction.y);
        if !offset.is_finite() {
            return Err(DraftError::new(
                "render_overflow",
                "curve sample offset is not finite",
            ));
        }
        max_offset_component = max_offset_component.max(offset.x.abs().max(offset.y.abs()));
        interior_offsets.push(offset);
    }
    chunks.try_reserve(1).map_err(|_| {
        DraftError::new("display_budget", "unable to reserve tessellation storage")
    })?;
    chunks.push(TessellationChunk::Arc {
        arc,
        interior_offsets,
        max_offset_component,
    });
    *generated_segments = next_total;
    Ok(())
}

fn push_bulge_chunk(
    chunks: &mut Vec<TessellationChunk>,
    generated_segments: &mut usize,
    max_segments: usize,
    count: usize,
    arc: BulgeArc,
) -> DraftResult<()> {
    let next_total = checked_total(*generated_segments, count, max_segments)?;
    let mut interior_offsets = Vec::new();
    let mut max_offset_component = 0.0f64;
    interior_offsets
        .try_reserve_exact(count.saturating_sub(1))
        .map_err(|_| {
            DraftError::new("display_budget", "unable to reserve curve sample storage")
        })?;
    for index in 1..count {
        let offset = arc.point_relative(arc.start, index as f64 / count as f64);
        if !offset.is_finite() {
            return Err(DraftError::new(
                "render_overflow",
                "bulge sample offset is not finite",
            ));
        }
        max_offset_component = max_offset_component.max(offset.x.abs().max(offset.y.abs()));
        interior_offsets.push(offset);
    }
    chunks.try_reserve(1).map_err(|_| {
        DraftError::new("display_budget", "unable to reserve tessellation storage")
    })?;
    chunks.push(TessellationChunk::Bulge {
        arc,
        start: arc.start,
        end: arc.end,
        interior_offsets,
        max_offset_component,
    });
    *generated_segments = next_total;
    Ok(())
}

fn checked_total(current: usize, addition: usize, max_segments: usize) -> DraftResult<usize> {
    let next_total = current
        .checked_add(addition)
        .ok_or_else(|| DraftError::new("display_budget", "generated segment count overflow"))?;
    if next_total > max_segments {
        return Err(DraftError::new(
            "display_budget",
            "geometry exceeds generated segment/work budget",
        ));
    }
    Ok(next_total)
}

fn curve_segment_count(
    radius: f64,
    sweep_angle: f64,
    pixels_per_metre: f64,
    dpi_factor: f64,
    pixel_error: f64,
    max_segments_per_curve: usize,
) -> DraftResult<usize> {
    let radius_px = radius * pixels_per_metre * dpi_factor;
    if !radius_px.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "projected radius overflow",
        ));
    }

    // This is the exact inverse of the circle sagitta formula. The same bound
    // applies to every equal-angle chord, including major signed bulge arcs.
    let max_angle = 4.0
        * (pixel_error / (2.0 * radius_px))
            .clamp(0.0, 1.0)
            .sqrt()
            .asin();
    let needed = (sweep_angle.abs() / max_angle.max(f64::MIN_POSITIVE)).ceil();
    let minimum = if sweep_angle.abs() >= TAU - 1e-12 {
        4
    } else {
        1
    };
    if !needed.is_finite() || needed > max_segments_per_curve as f64 {
        return Err(DraftError::new(
            "tessellation_budget",
            format!(
                "curve requires {needed} segments for requested pixel error"
            ),
        ));
    }
    let count = (needed as usize).max(minimum);
    if count > max_segments_per_curve {
        return Err(DraftError::new(
            "tessellation_budget",
            "curve minimum exceeds budget",
        ));
    }
    Ok(count)
}

#[derive(Clone, Copy)]
enum CurveParameterization {
    Arc(AnalyticArc),
    Bulge(BulgeArc),
}

#[derive(Clone, Copy)]
enum CurveEndpoint {
    World(Point2),
    AnalyticArc { arc: AnalyticArc, t: f64 },
}

fn chunk_segment_count(chunk: &TessellationChunk) -> usize {
    match chunk {
        TessellationChunk::Line { .. } => 1,
        TessellationChunk::Arc {
            interior_offsets, ..
        }
        | TessellationChunk::Bulge {
            interior_offsets, ..
        } => interior_offsets.len().saturating_add(1),
    }
}

fn needs_analytic_fallback(
    anchor: Point2,
    max_offset_component: f64,
    camera: &Camera2D,
    quality_pixels_per_metre: f64,
    dpi_factor: f64,
    pixel_error: f64,
) -> bool {
    if max_offset_component == 0.0 {
        return false;
    }
    let relative_anchor = Point2::new(
        anchor.x - camera.center.x,
        anchor.y - camera.center.y,
    );
    let physical_scale = quality_pixels_per_metre * dpi_factor;
    let allowed_world_loss = (pixel_error * 0.125) / physical_scale;
    if !relative_anchor.is_finite()
        || !max_offset_component.is_finite()
        || !allowed_world_loss.is_finite()
    {
        return true;
    }
    let coordinate_scale = relative_anchor.x.abs().max(relative_anchor.y.abs())
        + max_offset_component;
    let loss_bound = f64::EPSILON * coordinate_scale;
    !loss_bound.is_finite() || loss_bound > allowed_world_loss
}

fn project_curve(
    output: &mut Vec<ScreenSegment>,
    camera: &Camera2D,
    clip_bounds: Bounds2,
    anchor: Point2,
    start: CurveEndpoint,
    end: CurveEndpoint,
    interior_offsets: &[Point2],
    fallback: Option<CurveParameterization>,
) -> DraftResult<()> {
    let count = interior_offsets.len().saturating_add(1);
    let mut previous = project_curve_endpoint(camera, start)?;
    let anchor_relative = Point2::new(
        anchor.x - camera.center.x,
        anchor.y - camera.center.y,
    );

    for (index, offset) in interior_offsets.iter().enumerate() {
        let t = (index + 1) as f64 / count as f64;
        let next_relative = match fallback {
            Some(CurveParameterization::Arc(arc)) => arc.point_relative(camera.center, t),
            Some(CurveParameterization::Bulge(arc)) => arc.point_relative(camera.center, t),
            None => Point2::new(anchor_relative.x + offset.x, anchor_relative.y + offset.y),
        };
        let next = project_relative_point(camera, next_relative)?;
        append_projected_segment(output, clip_bounds, previous, next)?;
        previous = next;
    }
    let last = project_curve_endpoint(camera, end)?;
    append_projected_segment(output, clip_bounds, previous, last)
}

fn project_curve_endpoint(camera: &Camera2D, endpoint: CurveEndpoint) -> DraftResult<Point2> {
    match endpoint {
        CurveEndpoint::World(point) => project_world_point(camera, point),
        CurveEndpoint::AnalyticArc { arc, t } => {
            project_relative_point(camera, arc.point_relative(camera.center, t))
        }
    }
}

fn project_world_point(camera: &Camera2D, point: Point2) -> DraftResult<Point2> {
    project_relative_point(
        camera,
        Point2::new(point.x - camera.center.x, point.y - camera.center.y),
    )
}

fn project_relative_point(camera: &Camera2D, point: Point2) -> DraftResult<Point2> {
    let projected = Point2::new(
        camera.viewport_width * 0.5 + point.x * camera.pixels_per_metre,
        camera.viewport_height * 0.5 - point.y * camera.pixels_per_metre,
    );
    if !projected.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "screen coordinate overflow",
        ));
    }
    Ok(projected)
}

fn append_projected_segment(
    output: &mut Vec<ScreenSegment>,
    clip_bounds: Bounds2,
    start: Point2,
    end: Point2,
) -> DraftResult<()> {
    if let Some(segment) = clip_segment(ScreenSegment { start, end }, clip_bounds) {
        output.try_reserve(1).map_err(|_| {
            DraftError::new("display_budget", "unable to reserve projected segments")
        })?;
        output.push(segment);
    }
    Ok(())
}

fn append_projected_world_line(
    output: &mut Vec<ScreenSegment>,
    camera: &Camera2D,
    clip_bounds: Bounds2,
    start: Point2,
    end: Point2,
) -> DraftResult<()> {
    let dx_exact = DoubleDouble::difference(end.x, start.x);
    let dy_exact = DoubleDouble::difference(end.y, start.y);
    let dx = dx_exact.value();
    let dy = dy_exact.value();
    let length = dx.hypot(dy);
    if !length.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "line direction is not finite",
        ));
    }
    if length == 0.0 {
        return append_projected_segment(
            output,
            clip_bounds,
            project_world_point(camera, start)?,
            project_world_point(camera, end)?,
        );
    }

    // The source endpoints can be far outside the viewport. Keep the camera
    // center as separate compensated terms when finding the line's normal
    // distance, so a small pan is not rounded away by endpoint subtraction.
    let start_x = DoubleDouble::difference(start.x, camera.center.x);
    let start_y = DoubleDouble::difference(start.y, camera.center.y);
    let end_x = DoubleDouble::difference(end.x, camera.center.x);
    let end_y = DoubleDouble::difference(end.y, camera.center.y);
    let normal_numerator = DoubleDouble::multiply(start_x, dy_exact)
        .sub(DoubleDouble::multiply(start_y, dx_exact));
    let normal_distance = (normal_numerator.value() / length) * camera.pixels_per_metre;
    if !normal_distance.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "line normal distance overflow",
        ));
    }

    let ux = dx / length;
    let uy = -dy / length;
    let start_along = (DoubleDouble::multiply(start_x, dx_exact)
        .add(DoubleDouble::multiply(start_y, dy_exact))
        .value()
        / length)
        * camera.pixels_per_metre;
    let end_along = (DoubleDouble::multiply(end_x, dx_exact)
        .add(DoubleDouble::multiply(end_y, dy_exact))
        .value()
        / length)
        * camera.pixels_per_metre;
    if !start_along.is_finite() || !end_along.is_finite() {
        return Err(DraftError::new(
            "render_overflow",
            "line extent overflow",
        ));
    }
    if let Some(segment) = clip_segment_with_along_range(
        clip_bounds,
        ux,
        uy,
        normal_distance,
        start_along,
        end_along,
    ) {
        output.try_reserve(1).map_err(|_| {
            DraftError::new("display_budget", "unable to reserve projected segments")
        })?;
        output.push(segment);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct DoubleDouble {
    high: f64,
    low: f64,
}

impl DoubleDouble {
    fn difference(a: f64, b: f64) -> Self {
        let high = a - b;
        let b_virtual = a - high;
        let a_virtual = high + b_virtual;
        let b_roundoff = b_virtual - b;
        let a_roundoff = a - a_virtual;
        Self {
            high,
            low: a_roundoff + b_roundoff,
        }
    }

    fn product(a: Self, b: f64) -> Self {
        let high = a.high * b;
        let high_error = a.high.mul_add(b, -high);
        let low_product = a.low * b;
        let low_error = a.low.mul_add(b, -low_product);
        let (sum, sum_error) = two_sum(high, low_product);
        let (result, residual) = two_sum(sum, sum_error + high_error + low_error);
        Self {
            high: result,
            low: residual,
        }
    }

    fn multiply(a: Self, b: Self) -> Self {
        Self::product(a, b.high).add(Self::product(a, b.low))
    }

    fn add(self, other: Self) -> Self {
        let (high_sum, high_error) = two_sum(self.high, other.high);
        let (low_sum, low_error) = two_sum(self.low, other.low);
        let (middle_sum, middle_error) = two_sum(high_error, low_sum);
        let (sum, sum_error) = two_sum(high_sum, middle_sum);
        let tail = sum_error + middle_error + low_error;
        let (high, low) = two_sum(sum, tail);
        Self { high, low }
    }

    fn sub(self, other: Self) -> Self {
        self.add(Self {
            high: -other.high,
            low: -other.low,
        })
    }

    fn value(self) -> f64 {
        self.high + self.low
    }
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let b_virtual = sum - a;
    let error = (a - (sum - b_virtual)) + (b - b_virtual);
    (sum, error)
}

/// Liang-Barsky style clipping with an anchored line representation. Anchoring
/// near the viewport keeps the canvas interval when both source endpoints are
/// many orders of magnitude farther away than the viewport.
fn clip_segment(segment: ScreenSegment, bounds: Bounds2) -> Option<ScreenSegment> {
    let dx = segment.end.x - segment.start.x;
    let dy = segment.end.y - segment.start.y;
    let length = dx.hypot(dy);
    if length == 0.0 {
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

    // Compensated products retain the small normal distance after cancellation.
    let p = nx * near.x;
    let q = ny * near.y;
    let sum = p + q;
    let z = sum - p;
    let error = nx.mul_add(near.x, -p)
        + ny.mul_add(near.y, -q)
        + (p - (sum - z))
        + (q - z);
    let distance = sum + error;
    clip_segment_with_frame(segment, bounds, ux, uy, distance)
}

fn clip_segment_with_frame(
    segment: ScreenSegment,
    bounds: Bounds2,
    ux: f64,
    uy: f64,
    distance: f64,
) -> Option<ScreenSegment> {
    let center = bounds.center();
    let start_along = (segment.start.x - center.x) * ux + (segment.start.y - center.y) * uy;
    let end_along = (segment.end.x - center.x) * ux + (segment.end.y - center.y) * uy;
    clip_segment_with_along_range(bounds, ux, uy, distance, start_along, end_along)
}

fn clip_segment_with_along_range(
    bounds: Bounds2,
    ux: f64,
    uy: f64,
    distance: f64,
    start_along: f64,
    end_along: f64,
) -> Option<ScreenSegment> {
    let nx = -uy;
    let ny = ux;
    let center = bounds.center();
    let normal_limit = nx.abs() * (bounds.max.x - bounds.min.x) * 0.5
        + ny.abs() * (bounds.max.y - bounds.min.y) * 0.5;
    if distance.abs() > normal_limit + 1e-7 {
        return None;
    }

    let anchor = Point2::new(center.x + nx * distance, center.y + ny * distance);
    let sa = start_along;
    let sb = end_along;
    let mut lower = sa.min(sb);
    let mut upper = sa.max(sb);
    for (origin, direction, min, max) in [
        (anchor.x, ux, bounds.min.x, bounds.max.x),
        (anchor.y, uy, bounds.min.y, bounds.max.y),
    ] {
        if direction == 0.0 {
            if origin < min || origin > max {
                return None;
            }
        } else {
            let first = (min - origin) / direction;
            let second = (max - origin) / direction;
            lower = lower.max(first.min(second));
            upper = upper.min(first.max(second));
            if lower > upper {
                return None;
            }
        }
    }

    let endpoint = |distance_along: f64| {
        Point2::new(
            (anchor.x + ux * distance_along).clamp(bounds.min.x, bounds.max.x),
            (anchor.y + uy * distance_along).clamp(bounds.min.y, bounds.max.y),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(zoom: f64) -> Camera2D {
        Camera2D {
            center: Point2::default(),
            pixels_per_metre: zoom,
            viewport_width: 16_000.0,
            viewport_height: 16_000.0,
            revision: 0,
        }
    }

    fn build(geometry: &EntityGeometry, zoom: f64, dpi: f64, error: f64) -> TessellatedGeometry {
        build_tessellation(geometry, zoom, dpi, error, 4096, 100_000).unwrap()
    }

    fn screen(camera: &Camera2D, relative: Point2) -> Point2 {
        Point2::new(
            camera.viewport_width * 0.5 + relative.x * camera.pixels_per_metre,
            camera.viewport_height * 0.5 - relative.y * camera.pixels_per_metre,
        )
    }

    fn point_segment_distance(point: Point2, segment: ScreenSegment) -> f64 {
        let dx = segment.end.x - segment.start.x;
        let dy = segment.end.y - segment.start.y;
        let length_squared = dx * dx + dy * dy;
        if length_squared == 0.0 {
            return point.distance(segment.start);
        }
        let t = (((point.x - segment.start.x) * dx + (point.y - segment.start.y) * dy)
            / length_squared)
            .clamp(0.0, 1.0);
        point.distance(Point2::new(
            segment.start.x + t * dx,
            segment.start.y + t * dy,
        ))
    }

    fn assert_arc_error(arc: AnalyticArc, dpi: f64, pixel_error: f64) {
        let camera = camera(800.0);
        let geometry = EntityGeometry::Arc {
            center: arc.center,
            radius: arc.radius,
            start_angle: arc.start_angle,
            sweep_angle: arc.sweep_angle,
        };
        let tessellation = build(&geometry, camera.pixels_per_metre, dpi, pixel_error);
        let segments = tessellation.project(&camera, 0.0).unwrap();
        assert_eq!(segments.len(), tessellation.generated_segments());
        let count = segments.len();
        for (index, segment) in segments.into_iter().enumerate() {
            let t = (index as f64 + 0.5) / count as f64;
            let analytic = screen(&camera, arc.point_relative(camera.center, t));
            let error = point_segment_distance(analytic, segment);
            assert!(
                error * dpi <= pixel_error + 1e-8,
                "chord deviation {error} px exceeded physical tolerance {} px",
                pixel_error / dpi
            );
        }
    }

    fn assert_bulge_error(start: Point2, end: Point2, bulge: f64, dpi: f64, pixel_error: f64) {
        let camera = camera(800.0);
        let geometry = EntityGeometry::Polyline {
            vertices: vec![
                crate::PolylineVertex::new(start, bulge),
                crate::PolylineVertex::new(end, 0.0),
            ],
            closed: false,
        };
        let arc = bulge_curve(start, end, bulge).unwrap().unwrap();
        let tessellation = build(&geometry, camera.pixels_per_metre, dpi, pixel_error);
        let segments = tessellation.project(&camera, 0.0).unwrap();
        assert_eq!(segments.len(), tessellation.generated_segments());
        let count = segments.len();
        for (index, segment) in segments.into_iter().enumerate() {
            let t = (index as f64 + 0.5) / count as f64;
            let analytic = screen(&camera, arc.point_relative(camera.center, t));
            let error = point_segment_distance(analytic, segment);
            assert!(
                error * dpi <= pixel_error + 1e-8,
                "bulge chord deviation {error} px exceeded physical tolerance {} px",
                pixel_error / dpi
            );
        }
    }

    #[test]
    fn positive_and_negative_arcs_meet_actual_pixel_error() {
        for sweep in [4.2, -4.2] {
            assert_arc_error(
                AnalyticArc {
                    center: Point2::new(0.25, -0.5),
                    radius: 4.0,
                    start_angle: 0.37,
                    sweep_angle: sweep,
                },
                2.0,
                0.4,
            );
        }
    }

    #[test]
    fn positive_and_negative_bulges_meet_actual_pixel_error() {
        for bulge in [0.5, -0.5] {
            assert_bulge_error(
                Point2::new(-2.0, 0.0),
                Point2::new(2.0, 0.0),
                bulge,
                2.0,
                0.4,
            );
        }
    }

    #[test]
    fn tiny_and_large_bulges_use_stable_chord_frames() {
        assert_bulge_error(
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            1e-9,
            1.0,
            0.25,
        );
        assert_bulge_error(
            Point2::new(0.0, 0.0),
            Point2::new(1e-8, 0.0),
            1e9,
            1.0,
            0.5,
        );
    }

    #[test]
    fn original_line_endpoint_survives_large_origin_projection() {
        let end_x = 1e12 - 2f64.powi(-13);
        let geometry = EntityGeometry::Line {
            start: Point2::new(-1e12, 0.0),
            end: Point2::new(end_x, 0.0),
        };
        let camera = Camera2D {
            center: Point2::new(1e12, 0.0),
            pixels_per_metre: 1e4,
            viewport_width: 800.0,
            viewport_height: 700.0,
            revision: 0,
        };
        let tessellation = build(&geometry, camera.pixels_per_metre, 1.0, 0.5);
        let segments = tessellation.project(&camera, 64.0).unwrap();
        assert_eq!(segments.len(), 1);
        let expected_x = camera.viewport_width * 0.5
            + (end_x - camera.center.x) * camera.pixels_per_metre;
        assert!((segments[0].end.x - expected_x).abs() <= 1e-9);
        assert!((expected_x - 398.779_296_875).abs() <= 1e-9);
    }

    #[test]
    fn far_diagonal_line_keeps_tiny_camera_shift_in_compensated_frame() {
        let geometry = EntityGeometry::Line {
            start: Point2::new(-1e12, -1e12),
            end: Point2::new(1e12, 1e12),
        };
        let camera = Camera2D {
            center: Point2::new(1e-10, 0.0),
            pixels_per_metre: 1e12,
            viewport_width: 1_000.0,
            viewport_height: 700.0,
            revision: 0,
        };
        let tessellation = build(&geometry, camera.pixels_per_metre, 1.0, 0.5);
        let projected = tessellation.project(&camera, 0.0).unwrap();
        assert_eq!(projected.len(), 1);
        let segment = projected[0];
        assert!((segment.start.x - 50.0).abs() <= 1e-6);
        assert!((segment.start.y - 700.0).abs() <= 1e-6);
        assert!((segment.end.x - 750.0).abs() <= 1e-6);
        assert!(segment.end.y.abs() <= 1e-6);
    }

    #[test]
    fn far_non_diagonal_line_uses_compensated_world_determinant() {
        let geometry = EntityGeometry::Line {
            start: Point2::new(-1e12, -0.5e12),
            end: Point2::new(1e12, 0.5e12),
        };
        let camera = Camera2D {
            center: Point2::new(1e-10, 0.0),
            pixels_per_metre: 1e12,
            viewport_width: 1_000.0,
            viewport_height: 700.0,
            revision: 0,
        };
        let tessellation = build(&geometry, camera.pixels_per_metre, 1.0, 0.5);
        let projected = tessellation.project(&camera, 0.0).unwrap();
        assert_eq!(projected.len(), 1);
        let segment = projected[0];
        assert!((segment.start.x).abs() <= 1e-6);
        assert!((segment.start.y - 550.0).abs() <= 1e-6);
        assert!((segment.end.x - 1_000.0).abs() <= 1e-6);
        assert!((segment.end.y - 50.0).abs() <= 1e-6);
    }

    #[test]
    fn arc_endpoint_uses_camera_relative_analytic_evaluation() {
        let geometry = EntityGeometry::Arc {
            center: Point2::new(1e9, 0.0),
            radius: 1e-4,
            start_angle: 0.0,
            sweep_angle: 0.001,
        };
        let analytic = AnalyticArc {
            center: Point2::new(1e9, 0.0),
            radius: 1e-4,
            start_angle: 0.0,
            sweep_angle: 0.001,
        };
        let camera = Camera2D {
            center: Point2::new(1e9 + 1e-4, 0.0),
            pixels_per_metre: 1e10,
            viewport_width: 2_000.0,
            viewport_height: 3_200.0,
            revision: 0,
        };
        let tessellation = build_tessellation(&geometry, 1e10, 1.0, 16.0, 4096, 100).unwrap();
        assert_eq!(tessellation.generated_segments(), 1);
        let projected = tessellation.project(&camera, 0.0).unwrap();
        assert_eq!(projected.len(), 1);
        let expected = screen(&camera, analytic.point_relative(camera.center, 0.0));
        assert!((projected[0].start.x - expected.x).abs() <= 1e-8);
        assert!((projected[0].start.y - expected.y).abs() <= 1e-8);
        assert!((expected.x - 834.060_668_945_312_5).abs() <= 1e-6);
    }

    #[test]
    fn cached_curve_offsets_project_after_camera_pan() {
        let geometry = EntityGeometry::Arc {
            center: Point2::default(),
            radius: 4.0,
            start_angle: 0.1,
            sweep_angle: 2.0,
        };
        let tessellation = build(&geometry, 800.0, 1.0, 0.5);
        let cached_count = match &tessellation.chunks[0] {
            TessellationChunk::Arc {
                interior_offsets, ..
            } => interior_offsets.len(),
            _ => panic!("arc should retain its local f64 samples"),
        };
        assert_eq!(cached_count + 1, tessellation.generated_segments());

        let first_camera = camera(800.0);
        let mut second_camera = camera(800.0);
        second_camera.center = Point2::new(0.25, -0.5);
        assert_eq!(tessellation.fallback_segments(&first_camera).unwrap(), 0);
        assert_eq!(tessellation.fallback_segments(&second_camera).unwrap(), 0);
        let first = tessellation.project(&first_camera, 0.0).unwrap();
        let second = tessellation.project(&second_camera, 0.0).unwrap();
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert!((b.start.x - a.start.x + 200.0).abs() <= 1e-7);
            assert!((b.end.x - a.end.x + 200.0).abs() <= 1e-7);
            assert!((b.start.y - a.start.y + 400.0).abs() <= 1e-7);
            assert!((b.end.y - a.end.y + 400.0).abs() <= 1e-7);
        }
    }

    #[test]
    fn stroke_margin_expands_the_projection_clip() {
        let camera = Camera2D {
            center: Point2::default(),
            pixels_per_metre: 10.0,
            viewport_width: 800.0,
            viewport_height: 700.0,
            revision: 0,
        };
        let geometry = EntityGeometry::Line {
            start: Point2::new(-42.0, -1.0),
            end: Point2::new(-42.0, 1.0),
        };
        let tessellation = build(&geometry, camera.pixels_per_metre, 1.0, 0.5);
        assert!(tessellation.project(&camera, 0.0).unwrap().is_empty());
        let expanded = tessellation.project(&camera, 64.0).unwrap();
        assert_eq!(expanded.len(), 1);
        assert!((expanded[0].start.x + 20.0).abs() <= 1e-10);
        assert!((expanded[0].end.x + 20.0).abs() <= 1e-10);
    }

    #[test]
    fn large_origin_curve_reports_analytic_projection_fallback_work() {
        let geometry = EntityGeometry::Arc {
            center: Point2::new(1e12 - 10.0, 0.0),
            radius: 4.0,
            start_angle: 0.2,
            sweep_angle: 1.0,
        };
        let camera = camera(1e7);
        let tessellation = build(&geometry, camera.pixels_per_metre, 1.0, 0.5);
        assert!(tessellation.generated_segments() <= 4096);
        assert_eq!(tessellation.fallback_segments(&camera).unwrap(), tessellation.generated_segments());
        assert!(tessellation.fallback_segments(&camera).unwrap() > 0);
    }

    #[test]
    fn curve_and_total_budgets_fail_before_partial_success() {
        let circle = EntityGeometry::Circle {
            center: Point2::default(),
            radius: 100.0,
        };
        let curve_error = build_tessellation(&circle, 1e6, 1.0, 0.01, 4, 100_000)
            .unwrap_err();
        assert_eq!(curve_error.code, "tessellation_budget");

        let polyline = EntityGeometry::Polyline {
            vertices: vec![
                crate::PolylineVertex::new(Point2::new(0.0, 0.0), 0.0),
                crate::PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
                crate::PolylineVertex::new(Point2::new(2.0, 0.0), 0.0),
            ],
            closed: false,
        };
        let total_error = build_tessellation(&polyline, 10.0, 1.0, 0.5, 4096, 1).unwrap_err();
        assert_eq!(total_error.code, "display_budget");
    }

    #[test]
    fn tessellation_does_not_mutate_source_geometry() {
        let geometry = EntityGeometry::Polyline {
            vertices: vec![
                crate::PolylineVertex::new(Point2::new(-1.0, 0.0), 0.25),
                crate::PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            closed: false,
        };
        let before = geometry.clone();
        let _ = build(&geometry, 200.0, 1.0, 0.5);
        assert_eq!(geometry, before);
    }

    #[test]
    fn projection_rejects_scale_above_build_quality() {
        let geometry = EntityGeometry::Circle {
            center: Point2::default(),
            radius: 1.0,
        };
        let tessellation = build(&geometry, 100.0, 1.0, 0.5);
        let error = tessellation.project(&camera(101.0), 0.0).unwrap_err();
        assert_eq!(error.code, "tessellation_quality");
    }
}
