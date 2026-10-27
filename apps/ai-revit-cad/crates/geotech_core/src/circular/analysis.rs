use super::{
    bishop::{accept_physical_root, bishop_diagnostic_cancellable},
    circular_geometry::PreparedMass,
    fellenius::fellenius,
    slice_generator::{generate_prepared, slice_mechanics},
    types::*,
    validation::*,
};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    sync::{OnceLock, atomic::AtomicBool},
};

pub const CIRCULAR_SOLVER_VERSION: &str = "geotech-circular-0.2.0-gate8-geometry-precision01-v1";
pub fn source_fingerprint() -> String {
    static VALUE: OnceLock<String> = OnceLock::new();
    VALUE
        .get_or_init(|| {
            let mut h = Sha256::new();
            for (name, source) in [
                ("circular/mod.rs", include_str!("mod.rs")),
                ("circular/ground_definition.rs", include_str!("ground_definition.rs")),
                ("circular/canonical_digest.rs", include_str!("canonical_digest.rs")),
                ("circular/types.rs", include_str!("types.rs")),
                ("circular/validation.rs", include_str!("validation.rs")),
                ("circular/ground_model.rs", include_str!("ground_model.rs")),
                (
                    "circular/circular_geometry.rs",
                    include_str!("circular_geometry.rs"),
                ),
                (
                    "circular/slice_generator.rs",
                    include_str!("slice_generator.rs"),
                ),
                ("circular/fellenius.rs", include_str!("fellenius.rs")),
                ("circular/bishop.rs", include_str!("bishop.rs")),
                ("circular/physical.rs", include_str!("physical.rs")),
                ("circular/endpoint.rs", include_str!("endpoint.rs")),
                ("circular/contact_interval.rs", include_str!("contact_interval.rs")),
                ("circular/contact.rs", include_str!("contact.rs")),
                ("circular/arithmetic_environment.rs", include_str!("arithmetic_environment.rs")),
                ("circular/search_types.rs", include_str!("search_types.rs")),
                ("circular/search_grid.rs", include_str!("search_grid.rs")),
                ("circular/search_meter.rs", include_str!("search_meter.rs")),
                ("circular/search_evaluate.rs", include_str!("search_evaluate.rs")),
                ("circular/search_contact_filter.rs", include_str!("search_contact_filter.rs")),
                ("circular/search.rs", include_str!("search.rs")),
                ("circular/analysis.rs", include_str!("analysis.rs")),
                ("model.rs", include_str!("../model.rs")),
                ("Cargo.toml", include_str!("../../Cargo.toml")),
            ] {
                h.update(name.as_bytes());
                h.update([0]);
                h.update(source.as_bytes());
                h.update([0]);
            }
            format!("sha256:{:x}", h.finalize())
        })
        .clone()
}
fn fingerprint_validated(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<String> {
    let bytes = serde_json::to_vec(&(CIRCULAR_SOLVER_VERSION, input, config)).map_err(|e| {
        error(
            CircularErrorCode::InvalidParameter,
            format!("Circular input serialization: {e}"),
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
pub fn input_fingerprint(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<String> {
    PreparedMass::new(input, config)?;
    fingerprint_validated(input, config)
}
pub fn analyze_specified_circle(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<CircularSlopeResult> {
    analyze_specified_circle_cancellable(input, config, &AtomicBool::new(false))
}
pub fn analyze_specified_circle_cancellable(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
) -> CResult<CircularSlopeResult> {
    analyze_circle(input, config, cancel, false)
}

/// Explicit diagnostics preserve mathematical roots that are not accepted
/// physical solutions. Never use this output for critical-minimum selection.
pub fn diagnose_specified_circle(
    input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
) -> CResult<CircularSlopeResult> {
    analyze_circle(input, config, &AtomicBool::new(false), true)
}

fn analyze_circle(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
    cancel: &AtomicBool,
    diagnostic_only: bool,
) -> CResult<CircularSlopeResult> {
    cancelled(cancel)?;
    let prepared = PreparedMass::new_cancellable(input, config, cancel)?;
    cancelled(cancel)?;
    let mut target = config.slices.target_slices;
    let mut slices = generate_prepared(&prepared, config, target, cancel)?;
    let mechanics = slice_mechanics(&input.ground, &slices)?;
    let mut result = fellenius(&mechanics, config)?;
    let solve_bishop = |table: &[SliceMechanicsInput]| {
        let mut root = bishop_diagnostic_cancellable(table, config, cancel)?;
        let endpoints = super::endpoint::assess(&prepared, &root, config);
        if !endpoints.passed { root.accepted_factor_of_safety = None; }
        root.endpoint_assessment = Some(endpoints);
        let contact = super::contact::assess_prepared(input, config, &prepared,
            root.physical_assessment.root_bracket_lower,
            root.physical_assessment.root_bracket_upper,
            "COMPUTED_FINITE_TABLE_BISHOP_SIGN_BRACKET: conditional range; not an interval enclosure of the exact continuum Bishop root",
            cancel)?;
        if contact.status != WholeArcContactStatus::PassedDeclaredModel {
            root.accepted_factor_of_safety = None;
        }
        root.whole_arc_contact = Some(contact);
        if diagnostic_only { Ok(root) } else { accept_physical_root(root) }
    };
    let mut bishop_result = solve_bishop(&mechanics)?;
    let mut quality = DiscretizationQuality {
        refinement_checked: false,
        coarse_slice_count: slices.len(),
        fine_slice_count: None,
        coarse_factor_of_safety: result.factor_of_safety,
        fine_factor_of_safety: None,
        factor_delta: None,
        accepted_tolerance: config.tolerances.fs_abs
            + config.tolerances.fs_rel * result.factor_of_safety.abs(),
        coarse_bishop_factor_of_safety: bishop_result.factor_of_safety,
        fine_bishop_factor_of_safety: None,
        bishop_factor_delta: None,
        bishop_accepted_tolerance: config.tolerances.fs_abs
            + config.tolerances.fs_rel * bishop_result.factor_of_safety.abs(),
        area_closure_m2: 0.0,
        weight_closure_kn_per_m: 0.0,
    };
    if config.slices.verify_refinement {
        loop {
            cancelled(cancel)?;
            let next = target
                .checked_mul(2)
                .filter(|n| *n <= config.limits.max_slices)
                .ok_or_else(|| {
                    error(
                        CircularErrorCode::DiscretizationNotConverged,
                        "Slice budget exhausted before refinement converged",
                    )
                })?;
            let fine = generate_prepared(&prepared, config, next, cancel).map_err(|e| {
                if e.code == CircularErrorCode::ResourceLimit {
                    error(CircularErrorCode::DiscretizationNotConverged, e.message)
                } else {
                    e
                }
            })?;
            let fine_mechanics = slice_mechanics(&input.ground, &fine)?;
            let fine_result = fellenius(&fine_mechanics, config)?;
            let fine_bishop = solve_bishop(&fine_mechanics)?;
            let delta = (fine_result.factor_of_safety - result.factor_of_safety).abs();
            let tolerance = config.tolerances.fs_abs
                + config.tolerances.fs_rel * fine_result.factor_of_safety.abs();
            let bishop_delta =
                (fine_bishop.factor_of_safety - bishop_result.factor_of_safety).abs();
            let bishop_tolerance = config.tolerances.fs_abs
                + config.tolerances.fs_rel * fine_bishop.factor_of_safety.abs();
            quality.refinement_checked = true;
            quality.coarse_slice_count = slices.len();
            quality.fine_slice_count = Some(fine.len());
            quality.coarse_factor_of_safety = result.factor_of_safety;
            quality.fine_factor_of_safety = Some(fine_result.factor_of_safety);
            quality.factor_delta = Some(delta);
            quality.accepted_tolerance = tolerance;
            quality.coarse_bishop_factor_of_safety = bishop_result.factor_of_safety;
            quality.fine_bishop_factor_of_safety = Some(fine_bishop.factor_of_safety);
            quality.bishop_factor_delta = Some(bishop_delta);
            quality.bishop_accepted_tolerance = bishop_tolerance;
            slices = fine;
            result = fine_result;
            bishop_result = fine_bishop;
            target = next;
            if delta <= tolerance && bishop_delta <= bishop_tolerance {
                break;
            }
        }
    }
    let mut total_area = Sum::default();
    let mut total_weight = Sum::default();
    for slice in &slices {
        total_area.add(slice.total_area_m2);
        total_weight.add(slice.weight_kn_per_m);
    }
    let area = positive(total_area.total(), "total slice area")?;
    let weight = positive(total_weight.total(), "total slice weight")?;
    let mut expected_weight = Sum::default();
    for a in &prepared.geometry.material_areas {
        let m = input
            .ground
            .materials
            .iter()
            .find(|m| m.material_id == a.material_id)
            .unwrap();
        expected_weight.add(a.area_m2 * m.properties.unit_weight_kn_per_m3);
    }
    quality.area_closure_m2 = (area - prepared.geometry.area_m2).abs();
    quality.weight_closure_kn_per_m = (weight - expected_weight.total()).abs();
    if quality.area_closure_m2
        > config.tolerances.area_abs_m2 + config.tolerances.area_rel * prepared.geometry.area_m2
        || quality.weight_closure_kn_per_m
            > config.tolerances.force_abs_kn_per_m + config.tolerances.force_rel * weight
    {
        return Err(error(
            CircularErrorCode::DiscretizationNotConverged,
            "Slice area/weight closure exceeds its separate tolerance",
        ));
    }
    cancelled(cancel)?;
    let mut output = CircularSlopeResult {
        analysis_id: input.analysis_id.clone(),
        source_revision: input.source_revision.clone(),
        input_fingerprint: fingerprint_validated(input, config)?,
        source_fingerprint: source_fingerprint(),
        solver_version: CIRCULAR_SOLVER_VERSION.into(),
        soil_breakdown: prepared.geometry.material_areas.clone(),
        geometry: prepared.geometry,
        slices,
        total_area_m2: area,
        total_weight_kn_per_m: weight,
        fellenius_result: result,
        bishop_result,
        discretization: quality,
        warnings: vec![
            "DRY_STATIC_2D: self-weight only, without pore pressure or external loads".into(),
            "VISUALIZATION_3D_EXTRUSION_ONLY: extrusion width does not change the 2D factor".into(),
            "SPECIFIED_CIRCLE_ONLY: no critical search or global stability guarantee".into(),
            "CONTACT_SCOPES_SEPARATE: finite table, one-sided endpoints, and whole-arc numerical interval screen are reported independently".into(),
            "NOT_ELIGIBLE_FOR_CRITICAL_MINIMUM: whole-arc screen remains conditional on the declared geometry and numerical root range, pending independent acceptance".into(),
            "INDEPENDENT_NUMERICAL_REVIEW_REQUIRED: solver execution does not grant verification"
                .into(),
            if config.slices.verify_refinement {
                "SLICE_REFINEMENT_CHECKED: both method factors are from the converged fine discretization".into()
            } else {
                "DISCRETIZATION_NOT_CHECKED: fixed slice table; do not treat as converged".into()
            },
        ],
        numerically_verified: false,
        engineering_review_required: true,
        diagnostic_only,
        accepted: !diagnostic_only,
    };
    if diagnostic_only {
        output.warnings.push("DIAGNOSTIC_ONLY_NOT_FOR_CRITICAL_MINIMUM: mathematical root is not accepted analysis".into());
    }
    // Enforce the serialized limit without allocating another result-sized buffer.
    let mut counter = ByteBudget {
        used: 0,
        limit: config.limits.max_result_bytes,
    };
    serde_json::to_writer(&mut counter, &output).map_err(|_| {
        error(
            CircularErrorCode::ResourceLimit,
            "Serialized circular result exceeds max_result_bytes",
        )
    })?;
    cancelled(cancel)?;
    Ok(output)
}
struct ByteBudget {
    used: usize,
    limit: usize,
}
impl Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.used = self
            .used
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("result byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{InputUnits, Point2, SoilMaterial, circular::*};
    use sha2::{Digest, Sha256};

    #[test]
    fn public_input_fingerprint_separates_geometry_precision_version() {
        const OLD_VERSION: &str = "geotech-circular-0.2.0-gate8-foundation-draft02-v1";
        // Same valid one-material geometry as the existing cohesionless circular
        // fixture. This checks source-version identity, without solver outputs.
        let input = CircularAnalysisInput {
            analysis_id: "version-boundary-quarter-segment".into(),
            source_revision: "version-boundary-synthetic-source".into(),
            ground: GroundSection2D {
                section_id: "section-1".into(),
                coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
                terrain_polyline: vec![
                    Point2::new(-5.0, 15.0 / 7.0),
                    Point2::new(4.0, 6.0 / 7.0),
                ],
                layer_interfaces: vec![],
                soil_regions: vec![SoilRegion {
                    region_id: "region-0".into(),
                    material_id: "soil-0".into(),
                }],
                bottom_boundary: vec![Point2::new(-5.0, -2.0), Point2::new(4.0, -2.0)],
                materials: vec![GroundMaterial {
                    material_id: "soil-0".into(),
                    properties: SoilMaterial {
                        name: "Synthetic cohesionless material".into(),
                        unit_weight_kn_per_m3: 18.0,
                        effective_cohesion_kpa: 0.0,
                        effective_friction_angle_deg: 28.0,
                        provenance: "Version-boundary regression, not design data".into(),
                    },
                }],
                model_revision: 1,
            },
            slip_surface: CircularSlipSurface::from_center_radius(
                "circle-1".into(),
                Point2::new(0.0, 5.0),
                5.0,
                SlideDirection::PositiveX,
            )
            .unwrap(),
            basis: AnalysisBasis::DryStatic,
            units: InputUnits::default(),
            extrusion_width_m: 1.0,
        };
        let config = CircularAnalysisConfig {
            slices: SliceConfig {
                target_slices: 16,
                verify_refinement: false,
            },
            ..Default::default()
        };
        let current = input_fingerprint(&input, &config).unwrap();
        let old_version_bytes = serde_json::to_vec(&(OLD_VERSION, &input, &config)).unwrap();
        let previous = format!("sha256:{:x}", Sha256::digest(old_version_bytes));
        assert_ne!(current, previous);
    }
}
