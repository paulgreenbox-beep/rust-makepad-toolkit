//! Bishop simplified: vertical slice equilibrium and circular moment equilibrium.
//! No claim of full horizontal/vertical equilibrium for an arbitrary soil mass.
use super::{fellenius::validate_mechanics_slice, physical, types::*, validation::*};
use std::sync::atomic::AtomicBool;
use super::search_meter::{begin_bishop_pass, charge, WorkKind};

#[derive(Clone, Copy)]
struct Term {
    mechanics: SliceMechanicsInput,
    cos: f64,
    q: f64,
    numerator: f64,
}
#[derive(Clone, Copy)]
struct Evaluation {
    factor: f64,
    g: f64,
    residual: f64,
    relative: f64,
    min_m: f64,
    sum: f64,
}

fn diagnostic(
    status: BishopConvergenceStatus,
    lower: Option<f64>,
    last: Option<Evaluation>,
    iterations: Vec<BishopIteration>,
) -> BishopFailureDiagnostic {
    BishopFailureDiagnostic {
        status,
        admissible_lower_factor: lower,
        last_factor: last.map(|e| e.factor),
        equation_residual: last.map(|e| e.residual),
        relative_residual: last.map(|e| e.relative),
        faulty_slice_id: None,
        m_alpha: None,
        iterations,
        mathematical_root: None,
    }
}

fn evaluate(
    terms: &[Term],
    driving: f64,
    factor: f64,
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
) -> CResult<Evaluation> {
    cancelled(cancel)?;
    charge(WorkKind::BishopFunctionEvaluations, 1)?;
    if !factor.is_finite() || factor < config.bishop.min_factor || factor > config.bishop.max_factor
    {
        return Err(error(
            CircularErrorCode::BishopFactorOutOfBounds,
            "Bishop attempted a factor outside its declared positive bounds",
        ));
    }
    let mut sum = Sum::default();
    let mut minimum = f64::INFINITY;
    for (slice_id, t) in terms.iter().enumerate() {
        charge(WorkKind::BishopSliceTerms, 1)?;
        if slice_id % 64 == 0 {
            cancelled(cancel)?;
        }
        let m = t.cos + t.q / factor;
        if !m.is_finite() || m <= config.bishop.min_m_alpha {
            let mut e = error(
                CircularErrorCode::BishopInvalidDenominator,
                "Bishop m_alpha must be finite and above its positive safety threshold",
            );
            let mut d = diagnostic(
                BishopConvergenceStatus::InvalidDenominator,
                None,
                None,
                vec![],
            );
            d.last_factor = Some(factor);
            d.faulty_slice_id = Some(slice_id);
            d.m_alpha = m.is_finite().then_some(m);
            e.bishop_diagnostic = Some(d);
            return Err(e);
        }
        minimum = minimum.min(m);
        let value = t.numerator / m;
        if !value.is_finite() || value < 0.0 || (t.numerator > 0.0 && value == 0.0) {
            return Err(error(
                CircularErrorCode::NumericOverflow,
                "Bishop slice contribution overflowed or underflowed",
            ));
        }
        sum.add(value);
    }
    let sum = sum.total();
    let g = sum / driving;
    let residual = factor - g;
    if !sum.is_finite() || !g.is_finite() || g < 0.0 || !residual.is_finite() {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Bishop equation evaluation is not representable",
        ));
    }
    // Relative to F and G themselves, with no unit-sized floor. This rejects
    // the spurious near-zero limit H(F)->0 when the physical root is elsewhere.
    let relative = residual.abs() / factor.abs().max(g.abs());
    finite(relative, "Bishop relative equation residual")?;
    Ok(Evaluation {
        factor,
        g,
        residual,
        relative,
        min_m: minimum,
        sum,
    })
}

fn enrich(
    mut e: CircularError,
    lower: f64,
    last: Evaluation,
    iterations: Vec<BishopIteration>,
) -> CircularError {
    if let Some(d) = e.bishop_diagnostic.as_mut() {
        d.admissible_lower_factor = Some(lower);
        d.iterations = iterations;
    } else if e.code != CircularErrorCode::Cancelled {
        e.bishop_diagnostic = Some(diagnostic(
            BishopConvergenceStatus::InvalidDenominator,
            Some(lower),
            Some(last),
            iterations,
        ));
    }
    e
}

pub fn bishop(
    slices: &[SliceMechanicsInput],
    config: &CircularAnalysisConfig,
) -> CResult<BishopResult> {
    bishop_cancellable(slices, config, &AtomicBool::new(false))
}

pub fn bishop_cancellable(
    slices: &[SliceMechanicsInput],
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
) -> CResult<BishopResult> {
    accept_physical_root(bishop_diagnostic_cancellable(slices, config, cancel)?)
}

/// Explicit mathematical diagnostic: convergence does not imply admissibility.
pub fn bishop_diagnostic(
    slices: &[SliceMechanicsInput], config: &CircularAnalysisConfig,
) -> CResult<BishopResult> {
    bishop_diagnostic_cancellable(slices, config, &AtomicBool::new(false))
}

pub fn bishop_diagnostic_cancellable(
    slices: &[SliceMechanicsInput],
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
) -> CResult<BishopResult> {
    cancelled(cancel)?;
    begin_bishop_pass()?;
    validate_config(config)?;
    if slices.is_empty() {
        return Err(error(
            CircularErrorCode::DegenerateMass,
            "Bishop requires a nonempty slice table",
        ));
    }
    result_budget(slices.len(), 0, config)?;
    let estimated = slices
        .len()
        .checked_mul(3072)
        .and_then(|v| {
            config
                .bishop
                .max_iterations
                .checked_mul(768)
                .and_then(|i| v.checked_add(i))
        })
        .and_then(|v| v.checked_add(65536))
        .ok_or_else(|| {
            error(
                CircularErrorCode::ResourceLimit,
                "Bishop result size overflow",
            )
        })?;
    if estimated > config.limits.max_result_bytes {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Bishop table and iteration trace exceed max_result_bytes",
        ));
    }
    let mut terms = Vec::with_capacity(slices.len());
    let mut driving = Sum::default();
    let mut absolute_driving = Sum::default();
    let mut lower = config.bishop.min_factor;
    for (i, &s) in slices.iter().enumerate() {
        if i % 64 == 0 {
            cancelled(cancel)?;
        }
        validate_mechanics_slice(&s, config)?;
        let (sin, cos) = s.base_inclination_rad.sin_cos();
        let tan = s.effective_friction_angle_rad.tan();
        let q = sin * tan;
        let cohesion = s.effective_cohesion_kpa * s.horizontal_width_m;
        let friction = s.weight_kn_per_m * tan;
        let numerator = cohesion + friction;
        let d = s.weight_kn_per_m * sin;
        for (v, name) in [
            (q, "Bishop sin(alpha)*tan(phi)"),
            (numerator, "Bishop numerator"),
            (d, "Bishop driving force"),
        ] {
            finite(v, name)?;
        }
        if (s.effective_cohesion_kpa > 0.0 && cohesion == 0.0)
            || (s.effective_friction_angle_rad > 0.0 && friction == 0.0)
        {
            return Err(error(
                CircularErrorCode::NumericOverflow,
                "Positive Bishop numerator contribution underflowed",
            ));
        }
        if q < 0.0 {
            // Every negative-angle pole is excluded analytically. Because
            // min_m_alpha < min_base_cos, all cos-min_m_alpha are positive.
            let threshold = -q / (cos - config.bishop.min_m_alpha);
            let margin = (ulp(threshold) * 16.0).max(threshold * 64.0 * f64::EPSILON);
            let safe = threshold + margin;
            if !safe.is_finite() {
                return Err(error(
                    CircularErrorCode::BishopNoPhysicalRoot,
                    "No representable admissible factor above the negative-angle pole",
                ));
            }
            lower = lower.max(safe);
        }
        driving.add(d);
        absolute_driving.add(d.abs());
        terms.push(Term {
            mechanics: s,
            cos,
            q,
            numerator,
        });
    }
    let driving = driving.total();
    let absolute = absolute_driving.total();
    if !driving.is_finite() || !absolute.is_finite() {
        return Err(error(
            CircularErrorCode::NumericOverflow,
            "Bishop driving sums overflowed",
        ));
    }
    if driving <= config.tolerances.force_abs_kn_per_m + config.tolerances.force_rel * absolute {
        return Err(error(
            CircularErrorCode::NoDrivingForce,
            "Bishop requires a positive resolved signed driving sum",
        ));
    }
    let mut upper = config.bishop.max_factor;
    if lower >= upper {
        let mut e = error(
            CircularErrorCode::BishopNoPhysicalRoot,
            "Declared Bishop bounds do not include a denominator-admissible positive interval",
        );
        e.bishop_diagnostic = Some(diagnostic(
            BishopConvergenceStatus::NoPhysicalRoot,
            Some(lower),
            None,
            vec![],
        ));
        return Err(e);
    }
    let admissible_lower = lower;
    let low_eval = evaluate(&terms, driving, lower, config, cancel)?;
    let high_eval = evaluate(&terms, driving, upper, config, cancel)?;
    // For F>0 and all m>0, H(F)/F=1-sum[A/(cos*F+q)]/D is
    // strictly increasing when resistance is nonzero, hence at most one root.
    if low_eval.residual > 0.0 || high_eval.residual < 0.0 {
        let last = if low_eval.residual > 0.0 {
            low_eval
        } else {
            high_eval
        };
        let mut e = error(
            CircularErrorCode::BishopNoPhysicalRoot,
            "No positive Bishop root is bracketed within the declared admissible bounds",
        );
        e.bishop_diagnostic = Some(diagnostic(
            BishopConvergenceStatus::NoPhysicalRoot,
            Some(lower),
            Some(last),
            vec![],
        ));
        return Err(e);
    }
    let initial = if low_eval.residual == 0.0 {
        lower
    } else if high_eval.residual == 0.0 {
        upper
    } else if config.bishop.initial_factor >= lower {
        config.bishop.initial_factor
    } else {
        lower + (upper - lower) * 0.5
    };
    let mut current = evaluate(&terms, driving, initial, config, cancel)?;
    let mut iterations = Vec::with_capacity(config.bishop.max_iterations);
    for iteration in 1..=config.bishop.max_iterations {
        cancelled(cancel)?;
        if current.residual < 0.0 {
            lower = lower.max(current.factor);
        } else if current.residual > 0.0 {
            upper = upper.min(current.factor);
        }
        let fixed = current.g;
        let mut next = None;
        if fixed.is_finite() && fixed >= lower && fixed <= upper {
            let candidate = evaluate(&terms, driving, fixed, config, cancel)
                .map_err(|e| enrich(e, admissible_lower, current, iterations.clone()))?;
            if candidate.residual.abs() < current.residual.abs() * 0.9
                || current.relative <= config.bishop.residual_rel
                || candidate.factor == current.factor
            {
                next = Some((candidate, BishopStep::FixedPoint));
            }
        }
        let (next, step) = if let Some(value) = next {
            value
        } else {
            let midpoint = lower + (upper - lower) * 0.5;
            (
                evaluate(&terms, driving, midpoint, config, cancel)
                    .map_err(|e| enrich(e, admissible_lower, current, iterations.clone()))?,
                BishopStep::Bisection,
            )
        };
        let delta = (next.factor - current.factor).abs();
        let step_tolerance = config.bishop.step_abs
            + config.bishop.step_rel * next.factor.abs().max(current.factor.abs());
        let residual_tolerance = config.bishop.residual_abs
            + config.bishop.residual_rel * next.factor.abs().max(next.g.abs());
        if next.residual < 0.0 {
            lower = lower.max(next.factor);
        } else if next.residual > 0.0 {
            upper = upper.min(next.factor);
        }
        iterations.push(BishopIteration {
            iteration,
            step,
            factor_before: current.factor,
            factor_after: next.factor,
            step_delta: delta,
            equation_residual: next.residual,
            relative_residual: next.relative,
            minimum_m_alpha: next.min_m,
            bracket_lower: lower,
            bracket_upper: upper,
        });
        if delta <= step_tolerance
            && next.residual.abs() <= residual_tolerance
            && next.relative <= config.bishop.residual_rel
        {
            let contact_bracket = contact_bracket(
                &terms, driving, absolute, next, (lower, upper),
                step_tolerance, residual_tolerance, config, cancel,
            )?;
            let mut contributions = Vec::with_capacity(terms.len());
            for (slice_id, t) in terms.iter().enumerate() {
                if slice_id % 64 == 0 { cancelled(cancel)?; }
                let m = t.cos + t.q / next.factor;
                contributions.push(BishopSliceContribution {
                    slice_id,
                    mechanics: t.mechanics,
                    m_alpha: m,
                    numerator_kn_per_m: t.numerator,
                    contribution_kn_per_m: t.numerator / m,
                    base_forces: physical::derive_forces(&t.mechanics, next.factor, contact_bracket, config)?,
                });
            }
            let physical_assessment = physical::assessment(&contributions, contact_bracket,
                next.factor, driving, next.sum, residual_tolerance)?;
            let accepted_factor_of_safety = (physical_assessment.status == BishopPhysicalStatus::Admissible)
                .then_some(next.factor);
            cancelled(cancel)?;
            return Ok(BishopResult {method:CircularMethod::BishopSimplified,convergence_status:BishopConvergenceStatus::Converged,
                config:config.bishop,initial_factor_requested:config.bishop.initial_factor,initial_factor_used:initial,
                admissible_lower_factor:admissible_lower,factor_of_safety:next.factor,driving_sum_kn_per_m:driving,
                bishop_sum_kn_per_m:next.sum,iterations,final_step_delta:delta,equation_residual:next.residual,
                relative_residual:next.relative,step_tolerance,residual_tolerance,slice_contributions:contributions,
                accepted_factor_of_safety,physical_assessment,endpoint_assessment:None,whole_arc_contact:None,
                assumptions:vec![
                    "Bishop simplified: circular moment equilibrium and vertical slice equilibrium; zero interslice shear".into(),
                    "Dry static 2D self-weight only; no pore pressure or external loads; forces per metre".into(),
                    "Not a method of full horizontal and vertical equilibrium for an arbitrary soil mass".into(),
                    "Safeguarded fixed-point steps with bounded bisection; all m_alpha strictly positive".into(),
                    "Stopping requires step AND absolute/mixed equation residual AND relative equation residual".into(),
                    "Mathematical convergence is separate from non-tensile contact and base-force equilibrium".into(),
                    "Raw negative normals are never clipped; no implicit tension crack or fallback method".into(),
                ]});
        }
        current = next;
    }
    let mut e = error(
        CircularErrorCode::BishopNotConverged,
        "Bishop reached max_iterations without simultaneous step and equation-residual convergence",
    );
    e.bishop_diagnostic = Some(diagnostic(
        BishopConvergenceStatus::MaxIterations,
        Some(admissible_lower),
        Some(current),
        iterations,
    ));
    Err(e)
}

pub(crate) fn accept_physical_root(root: BishopResult) -> CResult<BishopResult> {
    if root.accepted_factor_of_safety.is_some() {
        return Ok(root);
    }
    let mut e = error(CircularErrorCode::BishopPhysicallyInadmissible,
        format!("Converged Bishop mathematical root is not physically accepted: table={:?}, endpoint_screen_passed={:?}, whole_arc_contact={:?}",
            root.physical_assessment.status, root.endpoint_assessment.as_ref().map(|e| e.passed),
            root.whole_arc_contact.as_ref().map(|c| c.status)));
    let mut d = diagnostic(BishopConvergenceStatus::Converged,
        Some(root.admissible_lower_factor), None, vec![]);
    d.last_factor = Some(root.factor_of_safety);
    d.equation_residual = Some(root.equation_residual);
    d.relative_residual = Some(root.relative_residual);
    d.faulty_slice_id = root.slice_contributions.iter()
        .find(|r| r.base_forces.physical_status != BishopPhysicalStatus::Admissible)
        .map(|r| r.slice_id);
    d.m_alpha = d.faulty_slice_id.map(|id| root.slice_contributions[id].m_alpha);
    d.mathematical_root = Some(Box::new(root));
    e.bishop_diagnostic = Some(d);
    Err(e)
}

/// Tighten only with numerically resolved equation signs. This is a computed
/// sign bracket, not a formal interval enclosure of transcendental functions.
fn contact_bracket(
    terms: &[Term], driving: f64, absolute_driving: f64, root: Evaluation,
    bracket: (f64, f64), step_tolerance: f64, residual_tolerance: f64,
    config: &CircularAnalysisConfig, cancel: &AtomicBool,
) -> CResult<(f64, f64)> {
    let mut bracket = bracket;
    let margin = (4.0 * (step_tolerance + residual_tolerance)).max(64.0 * ulp(root.factor));
    if !margin.is_finite() { return Ok(bracket); }
    for factor in [(root.factor - margin).max(bracket.0), (root.factor + margin).min(bracket.1)] {
        let e = evaluate(terms, driving, factor, config, cancel)?;
        let mut denominator_condition: f64 = 1.0;
        for (slice_id, t) in terms.iter().enumerate() {
            if slice_id % 64 == 0 { cancelled(cancel)?; super::search_meter::check()?; }
            let q = t.q / factor;
            let m = t.cos + q;
            denominator_condition = denominator_condition.max((t.cos.abs() + q.abs()) / m);
        }
        let sign_bound = physical::ROUNDING * (e.factor.abs() + e.g.abs())
            * (1.0 + absolute_driving / driving + denominator_condition);
        if !sign_bound.is_finite() { continue; }
        if e.residual < -sign_bound { bracket.0 = bracket.0.max(factor); }
        if e.residual > sign_bound { bracket.1 = bracket.1.min(factor); }
    }
    Ok(bracket)
}
