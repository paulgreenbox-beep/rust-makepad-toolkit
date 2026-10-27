use crate::{
    METHOD, SOLVER_VERSION,
    geometry::{geometry, geometry_validated, section_finite},
    model::*,
};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Hashes the exact owned Rust solver implementation and crate manifest, not an EXE.
pub fn source_fingerprint() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(compute_source_fingerprint).clone()
}
fn compute_source_fingerprint() -> String {
    let mut hash = Sha256::new();
    for (name, source) in [
        ("lib.rs", include_str!("lib.rs")),
        ("model.rs", include_str!("model.rs")),
        ("geometry.rs", include_str!("geometry.rs")),
        ("mechanics.rs", include_str!("mechanics.rs")),
        ("search.rs", include_str!("search.rs")),
        ("report.rs", include_str!("report.rs")),
        ("Cargo.toml", include_str!("../Cargo.toml")),
    ] {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(source.as_bytes());
        hash.update([0]);
    }
    format!("sha256:{:x}", hash.finalize())
}

/// Snapshot identity preserves every supplied coordinate and parameter.
/// Non-finite numbers are rejected before serde_json could encode them as null.
pub fn input_fingerprint(input: &SlopeAnalysisInput) -> Result<String, GeotechError> {
    section_finite(&input.section)?;
    finite(
        input.soil.unit_weight_kn_per_m3,
        "soil.unit_weight_kn_per_m3",
    )?;
    finite(
        input.soil.effective_cohesion_kpa,
        "soil.effective_cohesion_kpa",
    )?;
    finite(
        input.soil.effective_friction_angle_deg,
        "soil.effective_friction_angle_deg",
    )?;
    if let Some(v) = input.required_factor_of_safety {
        finite(v, "required_factor_of_safety")?;
    }
    // Validating conditions here also prevents hashing an unsupported contract as calculated.
    input.conditions.validate()?;
    let bytes = serde_json::to_vec(input).map_err(|e| {
        GeotechError::new(
            ErrorCode::InvalidParameter,
            format!("Input serialization: {e}"),
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

pub(crate) fn validate_mechanics(input: &SlopeAnalysisInput) -> Result<(), GeotechError> {
    input.units.validate()?;
    input.soil.validate()?;
    input.conditions.validate()?;
    if let Some(v) = input.required_factor_of_safety {
        finite(v, "required_factor_of_safety")?;
        if v <= 0.0 {
            return Err(GeotechError::new(
                ErrorCode::InvalidParameter,
                "Specified required factor of safety must be positive",
            ));
        }
    }
    if input.standard.as_ref().is_some_and(|s| s.trim().is_empty()) {
        return Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            "If a standard is supplied its identifier cannot be blank",
        ));
    }
    Ok(())
}
/// Validate the supported input and terrain/prism admissibility without solving forces.
pub fn validate_input(input: &SlopeAnalysisInput) -> Result<(), GeotechError> {
    validate_mechanics(input)?;
    geometry(&input.section)?;
    Ok(())
}
pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<(), GeotechError> {
    if cancel.load(Ordering::Relaxed) {
        Err(GeotechError::new(
            ErrorCode::Cancelled,
            "Analysis cancelled; no calculation result was published",
        ))
    } else {
        Ok(())
    }
}

pub fn analyze(input: &SlopeAnalysisInput) -> Result<SlopeAnalysisResult, GeotechError> {
    analyze_cancellable(input, &AtomicBool::new(false))
}
pub fn analyze_cancellable(
    input: &SlopeAnalysisInput,
    cancel: &AtomicBool,
) -> Result<SlopeAnalysisResult, GeotechError> {
    cancelled(cancel)?;
    validate_mechanics(input)?;
    let shape = geometry(&input.section)?;
    cancelled(cancel)?;
    let result = calculate(input, shape)?;
    cancelled(cancel)?;
    Ok(result)
}
pub(crate) fn analyze_validated(
    input: &SlopeAnalysisInput,
    tol: f64,
    cancel: &AtomicBool,
) -> Result<SlopeAnalysisResult, GeotechError> {
    cancelled(cancel)?;
    let result = calculate(input, geometry_validated(&input.section, tol)?)?;
    cancelled(cancel)?;
    Ok(result)
}
fn calculate(
    input: &SlopeAnalysisInput,
    shape: FailureGeometry,
) -> Result<SlopeAnalysisResult, GeotechError> {
    let soil = &input.soil;
    let d = shape.slip_surface;
    let dx = (d.upper.x - d.lower.x).abs();
    let dz = d.upper.z - d.lower.z;
    // Use directional ratios for the force decomposition instead of an angle round-trip.
    let sin = dz / shape.slip_length_m;
    let cos = dx / shape.slip_length_m;
    let weight = positive_output(
        soil.unit_weight_kn_per_m3 * shape.area_m2,
        "weight per metre",
    )?;
    let driving = weight * sin;
    let normal = weight * cos;
    if driving <= 0.0 || !driving.is_finite() {
        return Err(GeotechError::new(
            ErrorCode::NoDrivingForce,
            "Driving force is non-positive or not representable; no finite Fs is defined",
        ));
    }
    if normal <= 0.0 || !normal.is_finite() {
        return Err(GeotechError::new(
            ErrorCode::NonPositiveEffectiveNormal,
            "Dry effective normal force must be positive and finite",
        ));
    }
    let cohesion = soil.effective_cohesion_kpa * shape.slip_length_m;
    let friction = normal * soil.effective_friction_angle_deg.to_radians().tan();
    let resistance = cohesion + friction;
    let factor = resistance / driving;
    if (soil.effective_cohesion_kpa > 0.0 && cohesion == 0.0)
        || (soil.effective_friction_angle_deg > 0.0 && friction == 0.0)
        || (resistance > 0.0 && factor == 0.0)
    {
        return Err(GeotechError::new(
            ErrorCode::NumericOverflow,
            "Positive shear resistance underflowed; no fabricated zero factor is returned",
        ));
    }
    for (value, name) in [
        (cohesion, "cohesion force"),
        (friction, "friction force"),
        (resistance, "resistance force"),
        (factor, "factor of safety"),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(GeotechError::new(
                ErrorCode::NumericOverflow,
                format!("{name} is negative or non-finite"),
            ));
        }
    }
    let per = ForcesPerMetre {
        weight_kn_per_m: weight,
        normal_kn_per_m: normal,
        driving_kn_per_m: driving,
        cohesion_kn_per_m: cohesion,
        friction_kn_per_m: friction,
        resistance_kn_per_m: resistance,
    };
    let b = shape.width_m;
    let total = TotalForces {
        weight_kn: positive_output(weight * b, "total weight")?,
        normal_kn: positive_output(normal * b, "total normal force")?,
        driving_kn: positive_output(driving * b, "total driving force")?,
        cohesion_kn: cohesion * b,
        friction_kn: friction * b,
        resistance_kn: resistance * b,
    };
    for value in [total.cohesion_kn, total.friction_kn, total.resistance_kn] {
        if !value.is_finite() || value < 0.0 {
            return Err(GeotechError::new(
                ErrorCode::NumericOverflow,
                "Total resistance force overflowed",
            ));
        }
    }
    if (cohesion > 0.0 && total.cohesion_kn == 0.0)
        || (friction > 0.0 && total.friction_kn == 0.0)
        || (resistance > 0.0 && total.resistance_kn == 0.0)
    {
        return Err(GeotechError::new(
            ErrorCode::NumericOverflow,
            "Positive total shear force underflowed",
        ));
    }
    let requirement_comparison = match input.required_factor_of_safety {
        None => RequirementComparison::NotSpecified,
        Some(value) if factor >= value => RequirementComparison::MeetsSpecifiedValue,
        Some(_) => RequirementComparison::BelowSpecifiedValue,
    };
    Ok(SlopeAnalysisResult {method:METHOD.into(),status:CalculationStatus {geometry_valid:true,calculation_completed:true,
        numerically_verified:false,engineering_review_required:true},geometry:shape,forces_per_m:per,total_forces:total,
        factor_of_safety:factor,requirement_comparison,input_fingerprint:input_fingerprint(input)?,
        source_fingerprint:source_fingerprint(),solver_version:SOLVER_VERSION.into(),
        warnings:vec![
            "ENGINEERING_REVIEW_REQUIRED: Fs concerns only the specified planar surface and accepted assumptions".into(),
            "Dry homogeneous drained Mohr-Coulomb rigid translation; self-weight only; no side friction or 3D equilibrium".into(),
            "Width extrusion is a visual 2D prism, not a spatial geotechnical stability method".into(),
            "NUMERICALLY_VERIFIED is not granted by solver execution; independent verification evidence is separate".into(),
            "No regulatory factors, load combinations, engineering approval or global slope minimum are inferred".into(),
        ]})
}
