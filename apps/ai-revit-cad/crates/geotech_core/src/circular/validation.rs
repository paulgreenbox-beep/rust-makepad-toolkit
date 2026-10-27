use super::types::*;
use crate::Point2;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) type CResult<T> = Result<T, CircularError>;
pub(crate) fn error(code: CircularErrorCode, message: impl Into<String>) -> CircularError {
    CircularError::new(code, message)
}
pub(crate) fn finite(v: f64, name: &str) -> CResult<()> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(error(
            CircularErrorCode::NonFiniteInput,
            format!("{name} must be finite"),
        ))
    }
}
pub(crate) fn positive(v: f64, name: &str) -> CResult<f64> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(error(
            CircularErrorCode::NumericOverflow,
            format!("{name} is nonpositive or nonfinite"),
        ))
    }
}
pub(crate) fn text(value: &str, name: &str, max: usize) -> CResult<()> {
    if value.trim().is_empty() || value.len() > max {
        Err(error(
            CircularErrorCode::InvalidParameter,
            format!("{name} must contain 1..={max} bytes"),
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn cancelled(cancel: &AtomicBool) -> CResult<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            CircularErrorCode::Cancelled,
            "Circular calculation cancelled",
        ))
    } else {
        Ok(())
    }
}

pub fn validate_config(config: &CircularAnalysisConfig) -> CResult<()> {
    let t = config.tolerances;
    for (v, name) in [
        (t.intersection_abs_m, "intersection_abs_m"),
        (t.area_abs_m2, "area_abs_m2"),
        (t.area_rel, "area_rel"),
        (t.integral_abs_m2, "integral_abs_m2"),
        (t.integral_rel, "integral_rel"),
        (t.angle_abs_rad, "angle_abs_rad"),
        (t.min_base_cos, "min_base_cos"),
        (t.force_abs_kn_per_m, "force_abs_kn_per_m"),
        (t.force_rel, "force_rel"),
        (t.fs_abs, "fs_abs"),
        (t.fs_rel, "fs_rel"),
    ] {
        finite(v, name)?;
        if v <= 0.0 {
            return Err(error(
                CircularErrorCode::InvalidParameter,
                format!("{name} must be positive"),
            ));
        }
    }
    if t.min_base_cos >= 1.0
        || t.angle_abs_rad >= 0.1
        || t.area_rel >= 0.1
        || t.integral_rel >= 0.1
        || t.force_rel >= 0.1
        || t.fs_rel >= 0.1
    {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "Relative tolerances must be below 0.1 and min_base_cos below 1",
        ));
    }
    let l = config.limits;
    // Hard safety ceilings prevent a caller from disabling all allocation bounds.
    if !(1..=256).contains(&l.max_soil_regions)
        || !(1..=65536).contains(&l.max_terrain_segments)
        || !(4..=131072).contains(&l.max_boundary_points)
        || !(1..=65536).contains(&l.max_slices)
        || !(1..=4096).contains(&l.max_solver_iterations)
        || !(1024..=67108864).contains(&l.max_result_bytes)
    {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Resource limits exceed the bounded circular-core safety ceilings",
        ));
    }
    let required = config
        .slices
        .target_slices
        .checked_mul(if config.slices.verify_refinement {
            2
        } else {
            1
        });
    if required.is_none_or(|n| n == 0 || n > l.max_slices) {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Requested slice/refinement count exceeds max_slices",
        ));
    }
    validate_bishop_config(config)?;
    validate_contact_config(config)?;
    Ok(())
}

fn validate_contact_config(config: &CircularAnalysisConfig) -> CResult<()> {
    let c = config.contact;
    if !(1..=131072).contains(&c.max_intervals)
        || !(1..=64).contains(&c.max_depth)
        || !(1..=4194304).contains(&c.max_boundary_evaluations)
        || !(1..=4194304).contains(&c.max_material_checks)
        || !(1..=4096).contains(&c.max_report_intervals)
    {
        return Err(error(CircularErrorCode::ResourceLimit,
            "Contact screen work/detail limits exceed the bounded safety ceilings"));
    }
    for (value, name) in [
        (c.min_interval_width_m, "contact.min_interval_width_m"),
        (c.geometry_margin_multiplier, "contact.geometry_margin_multiplier"),
        (c.friction_uncertainty_rad, "contact.friction_uncertainty_rad"),
    ] { finite(value, name)?; }
    if c.min_interval_width_m <= 0.0
        || !(1.0..=64.0).contains(&c.geometry_margin_multiplier)
        || !(0.0..0.1).contains(&c.friction_uncertainty_rad)
    {
        return Err(error(CircularErrorCode::InvalidParameter,
            "Contact width must be positive, geometry multiplier in [1,64], and friction uncertainty in [0,0.1) radians"));
    }
    Ok(())
}

fn validate_bishop_config(config: &CircularAnalysisConfig) -> CResult<()> {
    let b = config.bishop;
    for (v, name) in [
        (b.initial_factor, "bishop.initial_factor"),
        (b.min_factor, "bishop.min_factor"),
        (b.max_factor, "bishop.max_factor"),
        (b.step_abs, "bishop.step_abs"),
        (b.step_rel, "bishop.step_rel"),
        (b.residual_abs, "bishop.residual_abs"),
        (b.residual_rel, "bishop.residual_rel"),
        (b.min_m_alpha, "bishop.min_m_alpha"),
    ] {
        finite(v, name)?;
        if v <= 0.0 {
            return Err(error(
                CircularErrorCode::InvalidParameter,
                format!("{name} must be positive"),
            ));
        }
    }
    if b.min_factor >= b.max_factor
        || b.initial_factor < b.min_factor
        || b.initial_factor > b.max_factor
    {
        let mut e = error(
            CircularErrorCode::BishopFactorOutOfBounds,
            "Require min_factor < max_factor and an initial factor inside the declared bounds",
        );
        e.bishop_diagnostic = Some(BishopFailureDiagnostic {
            status: BishopConvergenceStatus::FactorOutOfBounds,
            admissible_lower_factor: None,
            last_factor: Some(b.initial_factor),
            equation_residual: None,
            relative_residual: None,
            faulty_slice_id: None,
            m_alpha: None,
            iterations: vec![],
            mathematical_root: None,
        });
        return Err(e);
    }
    if b.step_rel >= 0.1 || b.residual_rel >= 0.1 || b.min_m_alpha >= config.tolerances.min_base_cos
    {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "Bishop relative tolerances must be below 0.1, and min_m_alpha below min_base_cos",
        ));
    }
    if b.max_iterations == 0 || b.max_iterations > config.limits.max_solver_iterations {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Bishop iteration budget exceeds max_solver_iterations",
        ));
    }
    Ok(())
}

pub(crate) fn ulp(value: f64) -> f64 {
    let v = value.abs();
    if v == 0.0 {
        f64::from_bits(1)
    } else {
        f64::from_bits(v.to_bits() + 1) - v
    }
}

#[derive(Default)]
pub(crate) struct Sum {
    sum: f64,
    correction: f64,
}
impl Sum {
    pub fn add(&mut self, x: f64) {
        let next = self.sum + x;
        self.correction += if self.sum.abs() >= x.abs() {
            (self.sum - next) + x
        } else {
            (x - next) + self.sum
        };
        self.sum = next;
    }
    pub fn total(&self) -> f64 {
        self.sum + self.correction
    }
}

pub(crate) fn unique_id(set: &mut BTreeSet<String>, value: &str, name: &str) -> CResult<()> {
    text(value, name, 256)?;
    if !set.insert(value.into()) {
        return Err(error(
            CircularErrorCode::InvalidGround,
            format!("Duplicate {name}: {value}"),
        ));
    }
    Ok(())
}

pub(crate) fn local(point: Point2, origin: Point2) -> CResult<Point2> {
    let p = Point2::new(point.x - origin.x, point.z - origin.z);
    finite(p.x, "local x")?;
    finite(p.z, "local z")?;
    Ok(p)
}

pub(crate) fn result_budget(
    slices: usize,
    materials: usize,
    config: &CircularAnalysisConfig,
) -> CResult<()> {
    // Conservative JSON-size allowance before any per-slice output allocation.
    let bytes = slices
        .checked_mul(
            materials
                .checked_mul(384)
                .and_then(|x| x.checked_add(3072))
                .ok_or_else(|| error(CircularErrorCode::ResourceLimit, "Result size overflow"))?,
        )
        .and_then(|x| x.checked_add(65536))
        .ok_or_else(|| error(CircularErrorCode::ResourceLimit, "Result size overflow"))?;
    if slices > config.limits.max_slices || bytes > config.limits.max_result_bytes {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Predicted slice result exceeds max_slices or max_result_bytes",
        ));
    }
    Ok(())
}
