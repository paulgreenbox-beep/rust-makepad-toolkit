use super::{types::*, validation::*};

pub fn fellenius(
    slices: &[SliceMechanicsInput],
    config: &CircularAnalysisConfig,
) -> CResult<FelleniusResult> {
    validate_config(config)?;
    if slices.is_empty() {
        return Err(error(
            CircularErrorCode::DegenerateMass,
            "Fellenius requires a nonempty positive-mass slice table",
        ));
    }
    result_budget(slices.len(), 0, config)?;
    let mut driving = Sum::default();
    let mut resisting = Sum::default();
    let mut absolute_driving = Sum::default();
    let mut terms = Vec::with_capacity(slices.len());
    for (slice_id, &s) in slices.iter().enumerate() {
        validate_mechanics_slice(&s, config)?;
        let (sin, cos) = s.base_inclination_rad.sin_cos();
        let normal = s.weight_kn_per_m * cos;
        let d = s.weight_kn_per_m * sin;
        let cohesion = s.effective_cohesion_kpa * s.base_length_m;
        let friction = normal * s.effective_friction_angle_rad.tan();
        let resistance = cohesion + friction;
        for (v, name) in [
            (normal, "normal force"),
            (d, "driving contribution"),
            (cohesion, "cohesion contribution"),
            (friction, "friction contribution"),
            (resistance, "resistance contribution"),
        ] {
            finite(v, name)?;
        }
        if normal <= 0.0
            || cohesion < 0.0
            || friction < 0.0
            || resistance < 0.0
            || (s.effective_cohesion_kpa > 0.0 && cohesion == 0.0)
            || (s.effective_friction_angle_rad > 0.0 && friction == 0.0)
        {
            return Err(error(
                CircularErrorCode::NumericOverflow,
                "Positive normal/resistance force underflowed or is invalid",
            ));
        }
        driving.add(d);
        absolute_driving.add(d.abs());
        resisting.add(resistance);
        terms.push(FelleniusSliceContribution {
            slice_id,
            mechanics: s,
            normal_kn_per_m: normal,
            driving_kn_per_m: d,
            cohesion_kn_per_m: cohesion,
            friction_kn_per_m: friction,
            resistance_kn_per_m: resistance,
        });
    }
    let d = driving.total();
    let r = resisting.total();
    if !d.is_finite() || !r.is_finite() || !absolute_driving.total().is_finite() {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Fellenius force sums overflowed",
        ));
    }
    let threshold = config.tolerances.force_abs_kn_per_m
        + config.tolerances.force_rel * absolute_driving.total();
    if d <= threshold {
        return Err(error(
            CircularErrorCode::NoDrivingForce,
            "Signed total driving force is nonpositive or unresolved; individual terms are not absolutized",
        ));
    }
    let factor = r / d;
    if !factor.is_finite() || factor < 0.0 || (r > 0.0 && factor == 0.0) {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Fellenius factor is not representable",
        ));
    }
    Ok(FelleniusResult {method:CircularMethod::FelleniusOrdinary,factor_of_safety:factor,
        driving_sum_kn_per_m:d,resisting_sum_kn_per_m:r,slice_contributions:terms,
        assumptions:vec![
            "Dry static 2D self-weight only; drained effective Mohr-Coulomb parameters; forces per metre".into(),
            "Chord base: l=hypot(b,dz)=b/cos(alpha); true arc length is reported separately".into(),
            "X right, Z up; sin(alpha)=-slide_direction*dz/l; negative individual driving terms retained".into(),
            "Ordinary method of slices neglects interslice forces; no pore pressure or external loads".into(),
        ]})
}

pub(crate) fn validate_mechanics_slice(
    s: &SliceMechanicsInput,
    config: &CircularAnalysisConfig,
) -> CResult<()> {
    for (v, name) in [
        (s.horizontal_width_m, "slice b"),
        (s.base_length_m, "slice l"),
        (s.base_inclination_rad, "slice alpha"),
        (s.weight_kn_per_m, "slice W"),
        (s.effective_cohesion_kpa, "slice c"),
        (s.effective_friction_angle_rad, "slice phi"),
    ] {
        finite(v, name)?;
    }
    if s.horizontal_width_m <= 0.0
        || s.base_length_m <= 0.0
        || s.weight_kn_per_m <= 0.0
        || s.effective_cohesion_kpa < 0.0
        || !(0.0..std::f64::consts::FRAC_PI_2).contains(&s.effective_friction_angle_rad)
        || s.base_inclination_rad.abs() >= std::f64::consts::FRAC_PI_2
    {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "Require b,l,W>0, c>=0, 0<=phi<pi/2 and -pi/2<alpha<pi/2",
        ));
    }
    let cos = s.base_inclination_rad.cos();
    if cos <= config.tolerances.min_base_cos {
        return Err(error(
            CircularErrorCode::NearVerticalBase,
            "Base inclination is too close to vertical",
        ));
    }
    let expected = s.base_length_m * cos;
    let tolerance = s.base_length_m * (config.tolerances.angle_abs_rad + 128.0 * f64::EPSILON);
    if !expected.is_finite() || (expected - s.horizontal_width_m).abs() > tolerance {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "Inconsistent b/l/alpha: the mechanics table must use chord l=b/cos(alpha)",
        ));
    }
    Ok(())
}
