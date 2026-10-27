//! InclusiveCountFmaV1 and exact nested grids. No tolerance deduplication.
use super::{search_types::*, types::*, validation::{CResult, error}};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn count_overflow() -> CircularError {
    error(CircularErrorCode::InvalidParameter, "CountOverflow: checked search count arithmetic overflow")
}
pub(crate) fn checked_add(a: u64, b: u64) -> CResult<u64> { a.checked_add(b).ok_or_else(count_overflow) }
pub(crate) fn checked_mul(a: u64, b: u64) -> CResult<u64> { a.checked_mul(b).ok_or_else(count_overflow) }
pub(crate) fn canonical(value: f64) -> f64 { if value == 0.0 { 0.0 } else { value } }
pub(crate) fn axis_count(axis: InclusiveAxis) -> CResult<u64> {
    if !axis.min.is_finite() || !axis.max.is_finite() || axis.min > axis.max {
        return Err(error(CircularErrorCode::InvalidParameter, "Inclusive axis requires finite ordered endpoints"));
    }
    if axis.min == axis.max {
        if axis.intervals != 0 {
            return Err(error(CircularErrorCode::InvalidParameter, "Collapsed inclusive axis requires zero intervals"));
        }
        return Ok(1);
    }
    if axis.intervals == 0 {
        return Err(error(CircularErrorCode::InvalidParameter, "Noncollapsed inclusive axis requires positive intervals"));
    }
    checked_add(u64::from(axis.intervals), 1)
}
pub(crate) fn axis_step(axis: InclusiveAxis) -> CResult<f64> {
    axis_count(axis)?;
    if axis.min == axis.max { return Ok(0.0); }
    let span = axis.max - axis.min;
    let step = span / f64::from(axis.intervals);
    if !span.is_finite() || !step.is_finite() || step <= 0.0 {
        return Err(error(CircularErrorCode::InvalidParameter, "Nonrepresentable inclusive axis span/step"));
    }
    Ok(step)
}
pub(crate) fn axis_value(axis: InclusiveAxis, index: u32) -> CResult<f64> {
    let value = if index == 0 { axis.min }
        else if index == axis.intervals { axis.max }
        else { axis_step(axis)?.mul_add(f64::from(index), axis.min) };
    if !value.is_finite() || index > axis.intervals {
        return Err(error(CircularErrorCode::InvalidParameter, "Nonrepresentable inclusive coordinate"));
    }
    Ok(canonical(value))
}
pub(crate) fn validate_axis_stream(axis: InclusiveAxis, cancel: &AtomicBool) -> CResult<()> {
    axis_step(axis)?;
    let mut previous = axis_value(axis, 0)?;
    for index in 1..=axis.intervals {
        if index % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(error(CircularErrorCode::Cancelled, "Search axis validation cancelled"));
        }
        let value = axis_value(axis, index)?;
        if value <= previous || (index < axis.intervals && !(value > axis.min && value < axis.max)) {
            return Err(error(CircularErrorCode::InvalidParameter,
                "Inclusive coordinates are not strictly increasing after binary64 rounding"));
        }
        previous = value;
    }
    Ok(())
}
fn reserve<T>(values: &mut Vec<T>, count: u64) -> CResult<()> {
    let count = usize::try_from(count).map_err(|_| count_overflow())?;
    values.try_reserve_exact(count).map_err(|_| error(CircularErrorCode::ResourceLimit,
        "Search allocation reservation failed"))
}
pub(crate) fn axis_values(axis: InclusiveAxis, cancel: &AtomicBool) -> CResult<Vec<f64>> {
    validate_axis_stream(axis, cancel)?;
    let mut result = Vec::new();
    reserve(&mut result, axis_count(axis)?)?;
    for index in 0..=axis.intervals { result.push(axis_value(axis, index)?); }
    Ok(result)
}
#[derive(Clone)]
pub(crate) struct Grid { pub axes: [Vec<f64>; 3] }
impl Grid {
    pub(crate) fn new(domain: &SearchDomain, cancel: &AtomicBool) -> CResult<Self> {
        Ok(Self { axes: [axis_values(domain.center_x, cancel)?, axis_values(domain.center_z, cancel)?,
            axis_values(domain.radius, cancel)?] })
    }
    pub(crate) fn count(&self) -> CResult<u64> {
        checked_mul(checked_mul(self.axes[0].len() as u64, self.axes[1].len() as u64)?, self.axes[2].len() as u64)
    }
    pub(crate) fn bisect(&self, max_axis_points: u64, cancel: &AtomicBool) -> CResult<Self> {
        let mut axes = [Vec::new(), Vec::new(), Vec::new()];
        for (source, destination) in self.axes.iter().zip(axes.iter_mut()) {
            let count = if source.len() == 1 { 1 } else { checked_add(checked_mul(source.len() as u64 - 1, 2)?, 1)? };
            if count > max_axis_points { return Err(error(CircularErrorCode::ResourceLimit, "Sensitivity axis exceeds admitted ceiling")); }
            reserve(destination, count)?;
            destination.push(source[0]);
            for pair in source.windows(2) {
                if cancel.load(Ordering::Relaxed) { return Err(error(CircularErrorCode::Cancelled, "Grid bisection cancelled")); }
                let span = pair[1] - pair[0];
                let midpoint = canonical(0.5_f64.mul_add(span, pair[0]));
                if !span.is_finite() || !midpoint.is_finite() || midpoint <= pair[0] || midpoint >= pair[1] {
                    return Err(error(CircularErrorCode::DiscretizationNotConverged,
                        "Uniform grid bisection has no representable interior midpoint"));
                }
                destination.push(midpoint);
                destination.push(pair[1]);
            }
        }
        Ok(Self { axes })
    }
}
pub(crate) fn box_flags(point: [f64; 3], domain: &SearchDomain) -> [bool; 6] {
    [point[0] == domain.center_x.min, point[0] == domain.center_x.max,
     point[1] == domain.center_z.min, point[1] == domain.center_z.max,
     point[2] == domain.radius.min, point[2] == domain.radius.max]
}
pub(crate) fn stencil(seed: [f64; 3], domain: &SearchDomain, level: u32)
    -> CResult<Vec<([f64; 3], [i8; 3], [bool; 3])>> {
    let sources = [domain.center_x, domain.center_z, domain.radius];
    let mut axes: [Vec<(f64, i8, bool)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let exponent = i32::try_from(level).map_err(|_| count_overflow())?;
    for index in 0..3 {
        let source = sources[index];
        if source.min == source.max { axes[index].push((canonical(source.min), 0, false)); continue; }
        let delta = axis_step(source)? * 2.0_f64.powi(-exponent);
        if !delta.is_finite() || delta <= 0.0 {
            return Err(error(CircularErrorCode::DiscretizationNotConverged,
                "Neighborhood subdivision delta is nonfinite or rounded to zero"));
        }
        for offset in [-1_i8, 0, 1] {
            let raw = delta.mul_add(f64::from(offset), seed[index]);
            if !raw.is_finite() || (offset != 0 && raw == seed[index]) {
                return Err(error(CircularErrorCode::DiscretizationNotConverged,
                    "Neighborhood subdivision makes no representable progress"));
            }
            let value = canonical(raw.clamp(source.min, source.max));
            axes[index].push((value, offset, value != raw));
        }
    }
    let mut result = Vec::with_capacity(27);
    for x in &axes[0] { for z in &axes[1] { for r in &axes[2] {
        result.push(([x.0, z.0, r.0], [x.1, z.1, r.1], [x.2, z.2, r.2]));
    } } }
    Ok(result)
}
