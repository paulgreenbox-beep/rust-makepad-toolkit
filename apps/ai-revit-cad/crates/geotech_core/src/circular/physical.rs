//! Dry Bishop base forces. Compression is positive; no force is clipped.
use super::{fellenius::validate_mechanics_slice, types::*, validation::*};

pub(crate) const ROUNDING: f64 = 64.0 * f64::EPSILON;

fn representable(value: f64) -> CResult<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error(CircularErrorCode::NumericOverflow, "Bishop base-force arithmetic is not representable"))
    }
}

fn normal(s: &SliceMechanicsInput, factor: f64) -> CResult<(f64, f64, bool)> {
    let (sin, cos) = s.base_inclination_rad.sin_cos();
    let q = representable(sin * s.effective_friction_angle_rad.tan() / factor)?;
    let m = representable(cos + q)?;
    if m <= 0.0 {
        return Err(error(CircularErrorCode::BishopInvalidDenominator, "Bishop contact check requires positive m_alpha"));
    }
    let a = representable(s.effective_cohesion_kpa * s.base_length_m * sin / factor)?;
    let n = representable((s.weight_kn_per_m - a) / m)?;
    let bound = representable(ROUNDING *
        (s.weight_kn_per_m.abs() + a.abs() + n.abs() * (cos.abs() + q.abs())) / m)?;
    let resolved = m > ROUNDING * (cos.abs() + q.abs())
        && (bound > 0.0 || (n == 0.0 && s.weight_kn_per_m == 0.0));
    Ok((n, bound, resolved))
}

/// Check supplied forces, independently of deriving them. This does not solve
/// the global Bishop equation or grant acceptance to a circle or saved result.
pub fn check_bishop_base_forces(
    s: &SliceMechanicsInput,
    factor: f64,
    normal_kn_per_m: f64,
    mobilized_shear_kn_per_m: f64,
    config: &CircularAnalysisConfig,
) -> CResult<BishopBaseForces> {
    validate_config(config)?;
    validate_mechanics_slice(s, config)?;
    if !factor.is_finite() || factor < config.bishop.min_factor || factor > config.bishop.max_factor {
        return Err(error(CircularErrorCode::BishopFactorOutOfBounds, "Base-force check requires a factor inside the declared bounds"));
    }
    let n = representable(normal_kn_per_m)?;
    let t = representable(mobilized_shear_kn_per_m)?;
    let (sin, cos) = s.base_inclination_rad.sin_cos();
    let m = representable(cos + sin * s.effective_friction_angle_rad.tan() / factor)?;
    if m <= config.bishop.min_m_alpha {
        return Err(error(CircularErrorCode::BishopInvalidDenominator,
            "Base-force check requires m_alpha above the configured positive threshold"));
    }
    let (derived_normal, bound, resolved) = normal(s, factor)?;
    let cohesion = representable(s.effective_cohesion_kpa * s.base_length_m)?;
    let friction = representable(n * s.effective_friction_angle_rad.tan())?;
    let strength = representable(cohesion + friction)?;
    let n_vertical = representable(n * cos)?;
    let t_vertical = representable(t * sin)?;
    let vertical = representable((n_vertical + t_vertical) - s.weight_kn_per_m)?;
    let vertical_bound = representable(ROUNDING *
        (n_vertical.abs() + t_vertical.abs() + s.weight_kn_per_m.abs() + (cohesion / factor).abs()))?;
    let developed = representable(factor * t)?;
    let shear_residual = representable(developed - strength)?;
    let shear_bound = representable(ROUNDING *
        (developed.abs() + cohesion.abs() + friction.abs()))?;
    let status = if n < -bound {
        BishopPhysicalStatus::InadmissibleTension
    } else if vertical.abs() > vertical_bound {
        BishopPhysicalStatus::VerticalEquilibriumNotSatisfied
    } else if shear_residual.abs() > shear_bound {
        BishopPhysicalStatus::ShearLawNotSatisfied
    } else if n < 0.0 || (n - derived_normal).abs() > bound || !resolved
        || vertical_bound == 0.0 || shear_bound == 0.0 && strength != 0.0 {
        BishopPhysicalStatus::NumericallyIndeterminate
    } else {
        BishopPhysicalStatus::Admissible
    };
    Ok(BishopBaseForces {
        normal_kn_per_m: n, mobilized_shear_kn_per_m: t,
        available_shear_strength_kn_per_m: strength,
        normal_roundoff_bound_kn_per_m: bound,
        minimum_bracket_normal_kn_per_m: n,
        vertical_residual_kn_per_m: vertical,
        vertical_residual_bound_kn_per_m: vertical_bound,
        shear_law_residual_kn_per_m: shear_residual,
        shear_law_residual_bound_kn_per_m: shear_bound,
        physical_status: status,
    })
}

pub(crate) fn derive_forces(
    s: &SliceMechanicsInput, factor: f64, bracket: (f64, f64), config: &CircularAnalysisConfig,
) -> CResult<BishopBaseForces> {
    let (n, _, _) = normal(s, factor)?;
    let shear = representable((s.effective_cohesion_kpa * s.base_length_m
        + n * s.effective_friction_angle_rad.tan()) / factor)?;
    let mut forces = check_bishop_base_forces(s, factor, n, shear, config)?;
    // dN/dF has the sign of sin(alpha), so the endpoints suffice. A broad or
    // unresolved numerical root bracket can only prevent acceptance.
    let low = normal(s, bracket.0)?;
    let high = normal(s, bracket.1)?;
    forces.minimum_bracket_normal_kn_per_m = low.0.min(high.0);
    if forces.physical_status == BishopPhysicalStatus::Admissible
        && (!low.2 || !high.2 || low.0 - low.1 < 0.0 || high.0 - high.1 < 0.0)
    {
        forces.physical_status = BishopPhysicalStatus::NumericallyIndeterminate;
    }
    Ok(forces)
}

pub(crate) fn assessment(
    rows: &[BishopSliceContribution], bracket: (f64, f64), factor: f64,
    driving: f64, bishop_sum: f64, root_residual_tolerance: f64,
) -> CResult<BishopPhysicalAssessment> {
    let mut a = BishopPhysicalAssessment {
        status: BishopPhysicalStatus::Admissible,
        scope: BishopPhysicalScope::CurrentDiscreteSliceTableOnly,
        physical_contact_convergence_verified: false,
        eligible_for_critical_minimum: false,
        tensile_slice_ids: vec![], indeterminate_slice_ids: vec![],
        vertical_failure_slice_ids: vec![], shear_law_failure_slice_ids: vec![],
        minimum_normal_kn_per_m: f64::INFINITY,
        maximum_absolute_vertical_residual_kn_per_m: 0.0,
        maximum_absolute_shear_law_residual_kn_per_m: 0.0,
        root_bracket_lower: bracket.0, root_bracket_upper: bracket.1,
        roundoff_multiplier: 64.0,
        force_sum_identity_residual_kn_per_m: 0.0,
        force_sum_identity_bound_kn_per_m: 0.0,
        moment_equilibrium_residual_kn_per_m: 0.0,
        moment_equilibrium_bound_kn_per_m: 0.0,
    };
    let mut resistance = Sum::default();
    let mut absolute_resistance = Sum::default();
    let mut resistance_roundoff = Sum::default();
    for row in rows {
        let f = &row.base_forces;
        resistance.add(f.available_shear_strength_kn_per_m);
        absolute_resistance.add(f.available_shear_strength_kn_per_m.abs());
        resistance_roundoff.add(f.normal_roundoff_bound_kn_per_m * row.mechanics.effective_friction_angle_rad.tan());
        a.minimum_normal_kn_per_m = a.minimum_normal_kn_per_m.min(f.normal_kn_per_m);
        a.maximum_absolute_vertical_residual_kn_per_m = a.maximum_absolute_vertical_residual_kn_per_m.max(f.vertical_residual_kn_per_m.abs());
        a.maximum_absolute_shear_law_residual_kn_per_m = a.maximum_absolute_shear_law_residual_kn_per_m.max(f.shear_law_residual_kn_per_m.abs());
        if f.normal_kn_per_m < -f.normal_roundoff_bound_kn_per_m { a.tensile_slice_ids.push(row.slice_id); }
        if f.vertical_residual_kn_per_m.abs() > f.vertical_residual_bound_kn_per_m { a.vertical_failure_slice_ids.push(row.slice_id); }
        if f.shear_law_residual_kn_per_m.abs() > f.shear_law_residual_bound_kn_per_m { a.shear_law_failure_slice_ids.push(row.slice_id); }
        if f.physical_status == BishopPhysicalStatus::NumericallyIndeterminate { a.indeterminate_slice_ids.push(row.slice_id); }
    }
    let resistance = representable(resistance.total())?;
    a.force_sum_identity_residual_kn_per_m = representable(resistance - bishop_sum)?;
    a.force_sum_identity_bound_kn_per_m = representable(ROUNDING *
        (absolute_resistance.total() + bishop_sum.abs()) + resistance_roundoff.total())?;
    a.moment_equilibrium_residual_kn_per_m = representable(factor * driving - resistance)?;
    a.moment_equilibrium_bound_kn_per_m = representable(driving.abs() * root_residual_tolerance
        + a.force_sum_identity_bound_kn_per_m + ROUNDING * (factor * driving).abs())?;
    a.status = if !a.tensile_slice_ids.is_empty() { BishopPhysicalStatus::InadmissibleTension }
        else if !a.vertical_failure_slice_ids.is_empty() { BishopPhysicalStatus::VerticalEquilibriumNotSatisfied }
        else if !a.shear_law_failure_slice_ids.is_empty() { BishopPhysicalStatus::ShearLawNotSatisfied }
        else if a.force_sum_identity_residual_kn_per_m.abs() > a.force_sum_identity_bound_kn_per_m
            || a.moment_equilibrium_residual_kn_per_m.abs() > a.moment_equilibrium_bound_kn_per_m {
            BishopPhysicalStatus::MomentEquilibriumNotSatisfied
        }
        else if !a.indeterminate_slice_ids.is_empty() { BishopPhysicalStatus::NumericallyIndeterminate }
        else { BishopPhysicalStatus::Admissible };
    Ok(a)
}
