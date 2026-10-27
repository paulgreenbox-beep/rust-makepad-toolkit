use crate::model::*;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

fn invalid(field: &str, reason: &str) -> FoundationError {
    FoundationError::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    }
}

fn finite_positive(field: &str, value: f64) -> Result<(), FoundationError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(invalid(field, "must be finite and positive"));
    }
    Ok(())
}

fn load_values(load: &PressureLoad) -> [f64; 6] {
    [
        load.n_kn,
        load.mx_pressure_knm,
        load.my_pressure_knm,
        load.fx_kn,
        load.fy_kn,
        load.mz_knm,
    ]
}
fn component_values(c: &LoadComponent) -> [f64; 6] {
    [
        c.n_kn,
        c.mx_pressure_knm,
        c.my_pressure_knm,
        c.fx_kn,
        c.fy_kn,
        c.mz_knm,
    ]
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Pure validation: no solving, I/O, host inspection, cache or process activity.
pub fn validate_input(input: &RigidMatInput) -> Result<(), FoundationError> {
    let g = &input.geometry;
    for (name, value) in [
        ("width_x_m", g.width_x_m),
        ("length_y_m", g.length_y_m),
        ("thickness_m", g.thickness_m),
        ("concrete_unit_weight_kn_m3", g.concrete_unit_weight_kn_m3),
    ] {
        finite_positive(name, value)?;
    }
    if !g.area_m2().is_finite()
        || !g.self_weight_kn().is_finite()
        || !g.ix_m4().is_finite()
        || !g.iy_m4().is_finite()
        || g.ix_m4() <= 0.0
        || g.iy_m4() <= 0.0
    {
        return Err(invalid(
            "geometry",
            "derived area/weight/inertia overflow or underflow",
        ));
    }
    let mesh = input.mesh;
    let count = mesh.nx.checked_mul(mesh.ny).unwrap_or(usize::MAX);
    if mesh.nx < 2 || mesh.ny < 2 {
        return Err(invalid(
            "mesh",
            "at least two cells per axis required for three independent rigid-plane DOFs",
        ));
    }
    if count > MAX_MESH_CELLS || mesh.nx > MAX_MESH_AXIS || mesh.ny > MAX_MESH_AXIS {
        return Err(FoundationError::MeshBudgetExceeded {
            requested: count,
            maximum: MAX_MESH_CELLS,
        });
    }
    if input.max_iterations == 0 || input.max_iterations > MAX_ACTIVE_SET_ITERATIONS {
        return Err(invalid(
            "max_iterations",
            "outside deterministic range 1..=128",
        ));
    }
    if !load_values(&input.load).iter().all(|v| v.is_finite()) {
        return Err(invalid("load", "all six components must be finite"));
    }
    if input.load.n_kn < 0.0 {
        return Err(FoundationError::NegativeVerticalLoad {
            n_kn: input.load.n_kn,
        });
    }
    if input.load.components.is_empty() {
        return Err(invalid(
            "load.components",
            "explicit source-ID ledger required",
        ));
    }
    let mut ids = HashSet::new();
    let mut sums = [0.0; 6];
    for c in &input.load.components {
        if c.source_id.trim().is_empty()
            || c.description.trim().is_empty()
            || !ids.insert(c.source_id.as_str())
        {
            return Err(invalid(
                "load.components",
                "nonempty unique source IDs and descriptions required",
            ));
        }
        for (sum, value) in sums.iter_mut().zip(component_values(c)) {
            if !value.is_finite() {
                return Err(invalid(
                    "load.components",
                    "all six components must be finite",
                ));
            }
            *sum += value;
        }
    }
    if sums
        .iter()
        .zip(load_values(&input.load))
        .any(|(a, b)| !a.is_finite() || !near(*a, b))
    {
        return Err(invalid(
            "load.components",
            "component sums do not match applied resultant",
        ));
    }
    let supplied: Vec<_> = input
        .load
        .components
        .iter()
        .filter(|c| c.kind == LoadComponentKind::ProvidedStructuralReaction)
        .collect();
    if !input.load.original_structural_reactions.is_empty() {
        let translated =
            load_from_structural_reactions(input.load.original_structural_reactions.clone())?;
        if supplied.len() != translated.components.len()
            || supplied.iter().zip(&translated.components).any(|(a, b)| {
                a.source_id != b.source_id
                    || component_values(a)
                        .iter()
                        .zip(component_values(b))
                        .any(|(x, y)| !near(*x, y))
            })
        {
            return Err(invalid(
                "load.original_structural_reactions",
                "original actions do not match translated structural components",
            ));
        }
    } else if !supplied.is_empty() {
        return Err(invalid(
            "load.original_structural_reactions",
            "supplied structural components require preserved original six-component actions and locations",
        ));
    }
    match &input.self_weight_policy {
        SelfWeightPolicy::IncludeOnce { source_id } => {
            if source_id.trim().is_empty()
                || ids.contains(source_id.as_str())
                || input
                    .load
                    .components
                    .iter()
                    .any(|c| c.kind == LoadComponentKind::FoundationSelfWeight)
            {
                return Err(invalid(
                    "self_weight_policy",
                    "new unique raft source ID required; raft component already present or ID reused",
                ));
            }
        }
        SelfWeightPolicy::IncludedInComponent { source_id } => {
            if !input
                .load
                .components
                .iter()
                .any(|c| &c.source_id == source_id)
            {
                return Err(invalid(
                    "self_weight_policy",
                    "already-included raft must identify an existing load component",
                ));
            }
            if input.load.components.iter().any(|c| {
                c.kind == LoadComponentKind::FoundationSelfWeight && &c.source_id != source_id
            }) {
                return Err(invalid(
                    "self_weight_policy",
                    "another explicit raft component would duplicate foundation self-weight",
                ));
            }
        }
        SelfWeightPolicy::ExcludedExplicitly => {
            if input
                .load
                .components
                .iter()
                .any(|c| c.kind == LoadComponentKind::FoundationSelfWeight)
            {
                return Err(invalid(
                    "self_weight_policy",
                    "raft exclusion contradicts explicit raft component",
                ));
            }
        }
    }
    if input.soil.name.trim().is_empty()
        || input.soil.assumption_note.trim().is_empty()
        || input.soil.layers.is_empty()
    {
        return Err(invalid(
            "soil",
            "named synthetic scenario, explicit assumptions and at least one layer required",
        ));
    }
    if input.soil.engineering_site_verified {
        return Err(invalid(
            "soil.engineering_site_verified",
            "Stage 1 synthetic scenarios cannot be site verified",
        ));
    }
    for layer in &input.soil.layers {
        if layer.name.trim().is_empty()
            || layer.parameter_assumption_note.trim().is_empty()
            || layer.engineering_site_verified
        {
            return Err(invalid(
                "soil.layers",
                "named unverified synthetic layers and explicit parameter assumptions required",
            ));
        }
        finite_positive("layer.thickness_m", layer.thickness_m)?;
        finite_positive("layer.young_modulus_kpa", layer.young_modulus_kpa)?;
        finite_positive("layer.unit_weight_kn_m3", layer.unit_weight_kn_m3)?;
        if !layer.depth_top_m.is_finite()
            || layer.depth_top_m < 0.0
            || !layer.visualization_bottom_depth_m.is_finite()
            || layer.visualization_bottom_depth_m <= layer.depth_top_m
            || !near(
                layer.visualization_bottom_depth_m - layer.depth_top_m,
                layer.thickness_m,
            )
            || layer.depth_bottom_m.is_some_and(|bottom| {
                !bottom.is_finite()
                    || bottom <= layer.depth_top_m
                    || !near(bottom, layer.visualization_bottom_depth_m)
            })
        {
            return Err(invalid(
                "layer.depth",
                "finite ordered display bounds and consistent thickness required; geological lower bound may be unspecified",
            ));
        }
        if !layer.effective_cohesion_kpa.is_finite()
            || layer.effective_cohesion_kpa < 0.0
            || !layer.effective_friction_angle_deg.is_finite()
            || layer.effective_friction_angle_deg < 0.0
            || layer.effective_friction_angle_deg >= 90.0
        {
            return Err(invalid(
                "layer.strength_metadata",
                "finite c >= 0 kPa and phi in [0,90) degrees required; metadata only, bearing remains unverified",
            ));
        }
        if !layer.poisson_ratio.is_finite()
            || layer.poisson_ratio <= -1.0
            || layer.poisson_ratio >= 0.5
        {
            return Err(invalid("layer.poisson_ratio", "must be finite in (-1,0.5)"));
        }
    }
    for pair in input.soil.layers.windows(2) {
        if pair[0].depth_bottom_m.is_none()
            || !near(pair[0].visualization_bottom_depth_m, pair[1].depth_top_m)
        {
            return Err(invalid(
                "soil.layers",
                "ordered contiguous synthetic layers required; only the final layer may be open-ended",
            ));
        }
    }
    match &input.soil.support {
        SupportField::Uniform { k_kn_m3, gap_m } => validate_spring(*k_kn_m3, *gap_m)?,
        SupportField::PerCell { k_kn_m3, gap_m } => {
            if k_kn_m3.len() != count || gap_m.len() != count {
                return Err(invalid(
                    "soil.support",
                    "row-major k and gap arrays must match mesh cell count",
                ));
            }
            for (k, gap) in k_kn_m3.iter().zip(gap_m) {
                validate_spring(*k, *gap)?;
            }
        }
    }
    match &input.groundwater {
        Groundwater::NotAssessed => {}
        Groundwater::DeclaredAbsent { source_id } => {
            if source_id.trim().is_empty() {
                return Err(invalid("groundwater.source_id", "source ID required"));
            }
        }
        Groundwater::WaterTable {
            depth_below_base_m,
            source_id,
        } => {
            if !depth_below_base_m.is_finite() || source_id.trim().is_empty() {
                return Err(invalid(
                    "groundwater",
                    "finite level and source ID required",
                ));
            }
            return Err(FoundationError::UnsupportedGroundwater);
        }
    }
    let total_n = input.load.n_kn
        + if matches!(
            &input.self_weight_policy,
            SelfWeightPolicy::IncludeOnce { .. }
        ) {
            g.self_weight_kn()
        } else {
            0.0
        };
    if !total_n.is_finite() {
        return Err(FoundationError::NonFinite {
            stage: "load ledger".into(),
        });
    }
    if total_n <= 0.0 {
        return Err(FoundationError::ZeroCompression);
    }
    Ok(())
}

fn validate_spring(k: f64, gap: f64) -> Result<(), FoundationError> {
    finite_positive("support.k_kn_m3", k)?;
    if !gap.is_finite() || gap < 0.0 {
        return Err(invalid("support.gap_m", "must be finite and nonnegative"));
    }
    Ok(())
}

/// Hash of the complete validated input for explicit cached-result binding.
pub fn input_fingerprint(input: &RigidMatInput) -> Result<String, FoundationError> {
    validate_input(input)?;
    let bytes = serde_json::to_vec(input).map_err(|_| invalid("input", "serialization failed"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Pure cache binding/shape validation; this is not an independent engineering
/// recalculation or numerical acceptance certificate.
pub fn validate_result_binding(
    input: &RigidMatInput,
    result: &RigidMatResult,
) -> Result<(), FoundationError> {
    if result.schema_version != 1
        || result.foundation_id != input.foundation_id
        || result.soil_scenario_id != input.soil.id
        || result.input_sha256 != input_fingerprint(input)?
        || result.mesh.nx != input.mesh.nx
        || result.mesh.ny != input.mesh.ny
        || result.mesh.cell_count != result.cells.len()
        || result.cells.len() != input.mesh.nx * input.mesh.ny
        || result.engineering_limits.engineering_site_verified
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    if ![
        result.plane.w0_m,
        result.plane.dw_dx,
        result.plane.dw_dy,
        result.equilibrium.normalized_residual,
        result.contact.active_sampled_area_m2,
        result.contact.sampled_qmin_kpa,
        result.contact.sampled_qmax_kpa,
    ]
    .iter()
    .all(|v| v.is_finite())
        || result.cells.iter().enumerate().any(|(index, c)| {
            c.index != index
                || ![
                    c.x_m,
                    c.y_m,
                    c.area_m2,
                    c.k_kn_m3,
                    c.spring_k_kn_m,
                    c.gap_m,
                    c.plane_settlement_m,
                    c.compression_m,
                    c.q_kpa,
                    c.vertical_force_kn,
                ]
                .iter()
                .all(|v| v.is_finite())
                || c.q_kpa < 0.0
                || c.area_m2 <= 0.0
                || c.k_kn_m3 <= 0.0
                || !near(
                    c.q_kpa,
                    c.k_kn_m3 * (c.plane_settlement_m - c.gap_m).max(0.0),
                )
                || !near(c.spring_k_kn_m, c.k_kn_m3 * c.area_m2)
                || !near(c.vertical_force_kn, c.q_kpa * c.area_m2)
        })
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let g = &input.geometry;
    let dx = g.width_x_m / input.mesh.nx as f64;
    let dy = g.length_y_m / input.mesh.ny as f64;
    if !near(result.mesh.dx_m, dx)
        || !near(result.mesh.dy_m, dy)
        || !near(result.mesh.cell_area_m2, dx * dy)
        || result.self_weight_ledger != self_weight_ledger(input)?
        || result.engineering_limits.ultimate_bearing
            != AssessmentStatus::UltimateBearingNotVerified
        || result.engineering_limits.settlement != AssessmentStatus::PreliminaryEstimate
        || result.engineering_limits.reinforcement != AssessmentStatus::NotImplemented
        || result.engineering_limits.groundwater != AssessmentStatus::NotImplemented
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let e = &result.equilibrium;
    if ![
        e.applied_n_kn,
        e.applied_mx_pressure_knm,
        e.applied_my_pressure_knm,
        e.original_external_mx_knm,
        e.original_external_my_knm,
        e.recovered_n_kn,
        e.recovered_mx_pressure_knm,
        e.recovered_my_pressure_knm,
        e.residual_n_kn,
        e.residual_mx_pressure_knm,
        e.residual_my_pressure_knm,
        e.normalized_residual,
        e.maximum_sign_violation_m,
        e.scaled_pivot_ratio,
        result.plane.rotation_about_x_rad,
        result.plane.rotation_about_y_rad,
        result.plane.tilt_magnitude_rad,
        result.contact.active_sampled_area_m2,
        result.contact.total_area_m2,
        result.contact.resultant_x_m,
        result.contact.resultant_y_m,
        result.continuum_full_contact.candidate_qmin_kpa,
        result.continuum_full_contact.candidate_qmax_kpa,
    ]
    .iter()
    .all(|v| v.is_finite())
        || e.normalized_residual > 1e-8
        || e.normalized_residual < 0.0
        || e.maximum_sign_violation_m < 0.0
        || e.scaled_pivot_ratio <= 0.0
        || e.scaled_pivot_ratio > 1.0
        || e.iterations == 0
        || e.iterations > input.max_iterations
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let ledger = &result.self_weight_ledger;
    if !near(e.applied_n_kn, ledger.total_n_kn)
        || !near(e.applied_mx_pressure_knm, ledger.total_mx_pressure_knm)
        || !near(e.applied_my_pressure_knm, ledger.total_my_pressure_knm)
        || !near(e.original_external_mx_knm, -e.applied_mx_pressure_knm)
        || !near(e.original_external_my_knm, e.applied_my_pressure_knm)
        || !near(result.plane.rotation_about_x_rad, -result.plane.dw_dy)
        || !near(result.plane.rotation_about_y_rad, result.plane.dw_dx)
        || !near(
            result.plane.tilt_magnitude_rad,
            result.plane.dw_dx.hypot(result.plane.dw_dy),
        )
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let h = &result.horizontal_actions_status;
    if h.status != AssessmentStatus::NotImplemented
        || h.fx_kn != ledger.total_fx_kn
        || h.fy_kn != ledger.total_fy_kn
        || h.mz_knm != ledger.total_mz_knm
        || h.nonzero_unconsumed_actions != (h.fx_kn != 0.0 || h.fy_kn != 0.0 || h.mz_knm != 0.0)
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let mut recovered = [0.0; 3];
    let mut active_count = 0;
    let mut qmin = f64::INFINITY;
    let mut qmax: f64 = 0.0;
    for (index, c) in result.cells.iter().enumerate() {
        let (k, gap) = match &input.soil.support {
            SupportField::Uniform { k_kn_m3, gap_m } => (*k_kn_m3, *gap_m),
            SupportField::PerCell { k_kn_m3, gap_m } => (k_kn_m3[index], gap_m[index]),
        };
        let x = (-1.0 + (2 * (index % input.mesh.nx) + 1) as f64 / input.mesh.nx as f64)
            * g.width_x_m
            / 2.0;
        let y = (-1.0 + (2 * (index / input.mesh.nx) + 1) as f64 / input.mesh.ny as f64)
            * g.length_y_m
            / 2.0;
        if !near(c.x_m, x)
            || !near(c.y_m, y)
            || !near(c.area_m2, dx * dy)
            || c.k_kn_m3 != k
            || c.gap_m != gap
            || !near(
                c.plane_settlement_m,
                result.plane.w0_m + result.plane.dw_dx * x + result.plane.dw_dy * y,
            )
            || !near(c.compression_m, (c.plane_settlement_m - c.gap_m).max(0.0))
        {
            return Err(FoundationError::ResultBindingMismatch);
        }
        recovered[0] += c.vertical_force_kn;
        recovered[1] += c.vertical_force_kn * c.y_m;
        recovered[2] += c.vertical_force_kn * c.x_m;
        active_count += usize::from(c.active);
        qmin = qmin.min(c.q_kpa);
        qmax = qmax.max(c.q_kpa);
    }
    if !near(e.recovered_n_kn, recovered[0])
        || !near(e.recovered_mx_pressure_knm, recovered[1])
        || !near(e.recovered_my_pressure_knm, recovered[2])
        || !near(e.residual_n_kn, recovered[0] - e.applied_n_kn)
        || !near(
            e.residual_mx_pressure_knm,
            recovered[1] - e.applied_mx_pressure_knm,
        )
        || !near(
            e.residual_my_pressure_knm,
            recovered[2] - e.applied_my_pressure_knm,
        )
        || result.contact.active_cells != active_count
        || !near(
            result.contact.active_sampled_area_m2,
            active_count as f64 * dx * dy,
        )
        || !near(result.contact.total_area_m2, g.area_m2())
        || !near(result.contact.sampled_qmin_kpa, qmin)
        || !near(result.contact.sampled_qmax_kpa, qmax)
        || !near(
            result.contact.resultant_x_m,
            ledger.total_my_pressure_knm / ledger.total_n_kn,
        )
        || !near(
            result.contact.resultant_y_m,
            ledger.total_mx_pressure_knm / ledger.total_n_kn,
        )
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    for p in &result.plane.control_points {
        if ![p.x_m, p.y_m, p.settlement_down_m, p.displacement_global_z_m]
            .iter()
            .all(|v| v.is_finite())
            || !near(
                p.settlement_down_m,
                result.plane.w0_m + result.plane.dw_dx * p.x_m + result.plane.dw_dy * p.y_m,
            )
            || !near(p.displacement_global_z_m, -p.settlement_down_m)
        {
            return Err(FoundationError::ResultBindingMismatch);
        }
    }
    if result
        .continuum_full_contact
        .candidate_corner_pressures
        .iter()
        .chain(
            result
                .continuum_full_contact
                .solved_plane_corner_pressures
                .iter()
                .flatten(),
        )
        .any(|p| ![p.x_m, p.y_m, p.q_kpa].iter().all(|v| v.is_finite()))
        || result
            .continuum_full_contact
            .solved_plane_corner_qmin_kpa
            .is_some_and(|v| !v.is_finite())
        || result
            .continuum_full_contact
            .solved_plane_corner_qmax_kpa
            .is_some_and(|v| !v.is_finite())
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    validate_continuum_binding(input, result, ledger)?;
    Ok(())
}

/// Validate the archived reference comparison from its authoritative support
/// field and already-bound load/settlement plane. These are four-point
/// evaluations only: no stiffness assembly, active-set iteration or solve.
fn validate_continuum_binding(
    input: &RigidMatInput,
    result: &RigidMatResult,
    ledger: &SelfWeightLedger,
) -> Result<(), FoundationError> {
    let comparison = &result.continuum_full_contact;
    let applicability = match &input.soil.support {
        SupportField::Uniform { gap_m, .. } if *gap_m == 0.0 => {
            ContinuumApplicability::HomogeneousFullContactCandidate
        }
        _ => ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps,
    };
    if comparison.applicability != applicability {
        return Err(FoundationError::ResultBindingMismatch);
    }
    let g = &input.geometry;
    let hx = g.width_x_m / 2.0;
    let hy = g.length_y_m / 2.0;
    let corners = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)];
    // This full-contact candidate remains a reference even when inapplicable
    // or tensile. It is not the unilateral physical pressure solution.
    let candidate = corners.map(|(x, y)| {
        ledger.total_n_kn / g.area_m2()
            + ledger.total_mx_pressure_knm * y / g.ix_m4()
            + ledger.total_my_pressure_knm * x / g.iy_m4()
    });
    let qmin = candidate.iter().copied().fold(f64::INFINITY, f64::min);
    let qmax = candidate.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !corner_values_match(&comparison.candidate_corner_pressures, &corners, &candidate)
        || !near(comparison.candidate_qmin_kpa, qmin)
        || !near(comparison.candidate_qmax_kpa, qmax)
        || comparison.candidate_has_tension != (qmin < 0.0)
    {
        return Err(FoundationError::ResultBindingMismatch);
    }
    match &input.soil.support {
        // A uniform initial gap invalidates the continuum applicability flag
        // above, but the solved-plane constitutive values are still available.
        SupportField::Uniform { k_kn_m3, gap_m } => {
            let plane = &result.plane;
            let expected = corners.map(|(x, y)| {
                k_kn_m3 * (plane.w0_m + plane.dw_dx * x + plane.dw_dy * y - gap_m).max(0.0)
            });
            let minimum = expected.iter().copied().fold(f64::INFINITY, f64::min);
            let maximum = expected.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if !comparison
                .solved_plane_corner_pressures
                .as_ref()
                .is_some_and(|stored| corner_values_match(stored, &corners, &expected))
                || !comparison
                    .solved_plane_corner_qmin_kpa
                    .is_some_and(|stored| near(stored, minimum))
                || !comparison
                    .solved_plane_corner_qmax_kpa
                    .is_some_and(|stored| near(stored, maximum))
            {
                return Err(FoundationError::ResultBindingMismatch);
            }
        }
        SupportField::PerCell { .. } => {
            if comparison.solved_plane_corner_pressures.is_some()
                || comparison.solved_plane_corner_qmin_kpa.is_some()
                || comparison.solved_plane_corner_qmax_kpa.is_some()
            {
                return Err(FoundationError::ResultBindingMismatch);
            }
        }
    }
    Ok(())
}

fn corner_values_match(
    stored: &[CornerPressure],
    corners: &[(f64, f64); 4],
    pressures: &[f64; 4],
) -> bool {
    if stored.len() != corners.len() {
        return false;
    }
    let mut seen = [false; 4];
    for point in stored {
        // Positions are exact physical geometry bounds, not measured samples.
        // Permit a different list order while rejecting repeated/missing corners.
        let Some(index) = corners
            .iter()
            .position(|(x, y)| point.x_m == *x && point.y_m == *y)
        else {
            return false;
        };
        if seen[index]
            || !point.q_kpa.is_finite()
            || !pressures[index].is_finite()
            || !near(point.q_kpa, pressures[index])
        {
            return false;
        }
        seen[index] = true;
    }
    true
}

/// Construct the named-component ledger. This only adds the raft, once; it
/// never guesses whether structural parts, snow or other actions were included.
pub fn self_weight_ledger(input: &RigidMatInput) -> Result<SelfWeightLedger, FoundationError> {
    validate_input(input)?;
    let mut components = input.load.components.clone();
    let added = match &input.self_weight_policy {
        SelfWeightPolicy::IncludeOnce { source_id } => {
            let weight = input.geometry.self_weight_kn();
            components.push(LoadComponent { source_id: source_id.clone(), description: "Centred raft self-weight: area times thickness times concrete unit weight; added exactly once".into(),
                kind: LoadComponentKind::FoundationSelfWeight, n_kn: weight, mx_pressure_knm: 0.0,
                my_pressure_knm: 0.0, fx_kn: 0.0, fy_kn: 0.0, mz_knm: 0.0 });
            weight
        }
        SelfWeightPolicy::IncludedInComponent { .. } | SelfWeightPolicy::ExcludedExplicitly => 0.0,
    };
    Ok(SelfWeightLedger {
        foundation_weight_kn: input.geometry.self_weight_kn(),
        foundation_weight_added_kn: added,
        policy: input.self_weight_policy.clone(),
        applied_components: components,
        total_n_kn: input.load.n_kn + added,
        total_mx_pressure_knm: input.load.mx_pressure_knm,
        total_my_pressure_knm: input.load.my_pressure_knm,
        total_fx_kn: input.load.fx_kn,
        total_fy_kn: input.load.fy_kn,
        total_mz_knm: input.load.mz_knm,
        original_structural_reactions: input.load.original_structural_reactions.clone(),
    })
}

#[derive(Clone)]
struct Spring {
    x: f64,
    y: f64,
    u: f64,
    v: f64,
    k: f64,
    gap: f64,
    area: f64,
}
impl Spring {
    fn phi(&self) -> [f64; 3] {
        [1.0, self.u, self.v]
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Diagonal scaling followed by partial pivoting, with a conservative scaled
/// pivot-ratio guard. The ratio is a diagnostic, not a full condition number.
fn solve_scaled(
    matrix: [[f64; 3]; 3],
    rhs: [f64; 3],
    iteration: usize,
) -> Result<([f64; 3], f64), FoundationError> {
    if matrix.iter().flatten().chain(&rhs).any(|v| !v.is_finite()) {
        return Err(FoundationError::NonFinite {
            stage: "stiffness assembly".into(),
        });
    }
    let mut d = [0.0; 3];
    for i in 0..3 {
        if matrix[i][i] <= 0.0 {
            return Err(FoundationError::SingularStiffness { iteration });
        }
        d[i] = matrix[i][i].sqrt();
    }
    let mut a = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..3 {
            a[i][j] = (matrix[i][j] / d[i]) / d[j];
        }
        a[i][3] = rhs[i] / d[i];
    }
    let mut min_pivot: f64 = f64::INFINITY;
    let mut max_pivot: f64 = 0.0;
    for col in 0..3 {
        let mut pivot = col;
        for row in (col + 1)..3 {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        let magnitude = a[pivot][col].abs();
        if !magnitude.is_finite() {
            return Err(FoundationError::NonFinite {
                stage: "scaled pivot".into(),
            });
        }
        if magnitude == 0.0 {
            return Err(FoundationError::SingularStiffness { iteration });
        }
        min_pivot = min_pivot.min(magnitude);
        max_pivot = max_pivot.max(magnitude);
        let ratio = min_pivot / max_pivot;
        if magnitude < 1e-12 || ratio < 1e-12 {
            return Err(FoundationError::IllConditionedStiffness {
                iteration,
                scaled_pivot_ratio: ratio,
            });
        }
        a.swap(col, pivot);
        for row in (col + 1)..3 {
            let multiplier = a[row][col] / a[col][col];
            for j in col..4 {
                a[row][j] -= multiplier * a[col][j];
            }
        }
    }
    let mut z = [0.0; 3];
    for i in (0..3).rev() {
        let mut value = a[i][3];
        for (j, zj) in z.iter().enumerate().skip(i + 1) {
            value -= a[i][j] * zj;
        }
        z[i] = value / a[i][i];
    }
    let p = [z[0] / d[0], z[1] / d[1], z[2] / d[2]];
    if p.iter().any(|v| !v.is_finite()) {
        return Err(FoundationError::NonFinite {
            stage: "settlement plane".into(),
        });
    }
    Ok((p, min_pivot / max_pivot))
}

/// Compression-only rigid mat. Each iteration redistributes reactions by
/// resolving equilibrium on the current active set; no clipped bilateral
/// solution is returned without redistribution and a final equilibrium check.
pub fn solve(input: &RigidMatInput) -> Result<RigidMatResult, FoundationError> {
    validate_input(input)?;
    let ledger = self_weight_ledger(input)?;
    let g = &input.geometry;
    let hx = g.width_x_m / 2.0;
    let hy = g.length_y_m / 2.0;
    let dx = g.width_x_m / input.mesh.nx as f64;
    let dy = g.length_y_m / input.mesh.ny as f64;
    let area = dx * dy;
    if !area.is_finite() || area <= 0.0 {
        return Err(invalid("mesh.cell_area", "nonfinite or degenerate"));
    }
    let mut springs = Vec::with_capacity(input.mesh.nx * input.mesh.ny);
    for iy in 0..input.mesh.ny {
        for ix in 0..input.mesh.nx {
            let index = iy * input.mesh.nx + ix;
            let (k, gap) = match &input.soil.support {
                SupportField::Uniform { k_kn_m3, gap_m } => (*k_kn_m3, *gap_m),
                SupportField::PerCell { k_kn_m3, gap_m } => (k_kn_m3[index], gap_m[index]),
            };
            let u = -1.0 + (2 * ix + 1) as f64 / input.mesh.nx as f64;
            let v = -1.0 + (2 * iy + 1) as f64 / input.mesh.ny as f64;
            springs.push(Spring {
                x: u * hx,
                y: v * hy,
                u,
                v,
                k,
                gap,
                area,
            });
        }
    }
    let n = ledger.total_n_kn;
    let mx = ledger.total_mx_pressure_knm;
    let my = ledger.total_my_pressure_knm;
    let ex = my / n;
    let ey = mx / n;
    let hull_x = [-hx + dx / 2.0, hx - dx / 2.0];
    let hull_y = [-hy + dy / 2.0, hy - dy / 2.0];
    if !ex.is_finite() || !ey.is_finite() {
        return Err(FoundationError::NonFinite {
            stage: "resultant eccentricity".into(),
        });
    }
    // Equality lies on a line of sampled supports and cannot support all three DOFs.
    if ex <= hull_x[0] || ex >= hull_x[1] || ey <= hull_y[0] || ey >= hull_y[1] {
        return Err(FoundationError::ResultantOutsideSupportHull { x_m: ex, y_m: ey });
    }
    let force_rhs = [n, my / hx, mx / hy];
    let mut active = vec![true; springs.len()];
    let mut seen = HashSet::new();
    for iteration in 1..=input.max_iterations {
        if !seen.insert(active.clone()) {
            return Err(FoundationError::ActiveSetCycle {
                iterations: iteration,
            });
        }
        let mut matrix = [[0.0; 3]; 3];
        let mut rhs = force_rhs;
        for (s, is_active) in springs.iter().zip(&active) {
            if !is_active {
                continue;
            }
            let phi = s.phi();
            let spring_k = s.k * s.area;
            for row in 0..3 {
                rhs[row] += spring_k * s.gap * phi[row];
                for col in 0..3 {
                    matrix[row][col] += spring_k * phi[row] * phi[col];
                }
            }
        }
        let (p, ratio) = solve_scaled(matrix, rhs, iteration)?;
        let magnitude = p.iter().map(|v| v.abs()).fold(0.0, f64::max);
        // Relative to the solved displacement scale, so multiplying every k
        // does not impose an artificial minimum load or settlement threshold.
        let sign_tolerance = (magnitude * 1e-12).max(f64::MIN_POSITIVE);
        let mut next_active = Vec::with_capacity(active.len());
        for s in &springs {
            next_active.push(dot(p, s.phi()) - s.gap > sign_tolerance);
        }
        if next_active != active {
            active = next_active;
            continue;
        }
        return assemble_result(
            input,
            ledger,
            &springs,
            p,
            ratio,
            iteration,
            sign_tolerance,
            hull_x,
            hull_y,
        );
    }
    Err(FoundationError::DidNotConverge {
        iterations: input.max_iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn assemble_result(
    input: &RigidMatInput,
    ledger: SelfWeightLedger,
    springs: &[Spring],
    p: [f64; 3],
    ratio: f64,
    iterations: usize,
    sign_tolerance: f64,
    hull_x: [f64; 2],
    hull_y: [f64; 2],
) -> Result<RigidMatResult, FoundationError> {
    let g = &input.geometry;
    let hx = g.width_x_m / 2.0;
    let hy = g.length_y_m / 2.0;
    let mut recovered = [0.0; 3];
    let mut active_cells = 0usize;
    let mut cells = Vec::with_capacity(springs.len());
    let mut qmin = f64::INFINITY;
    let mut qmax: f64 = 0.0;
    let mut sign_violation: f64 = 0.0;
    for (index, s) in springs.iter().enumerate() {
        let w = dot(p, s.phi());
        let penetration = w - s.gap;
        let compression = penetration.max(0.0);
        let q = s.k * compression;
        let force = q * s.area;
        if ![w, compression, q, force].iter().all(|v| v.is_finite()) {
            return Err(FoundationError::NonFinite {
                stage: "cell reaction".into(),
            });
        }
        let active = penetration > sign_tolerance;
        if active {
            active_cells += 1;
        } else {
            sign_violation = sign_violation.max(penetration.max(0.0));
        }
        recovered[0] += force;
        recovered[1] += force * s.y;
        recovered[2] += force * s.x;
        qmin = qmin.min(q);
        qmax = qmax.max(q);
        cells.push(CellResult {
            index,
            x_m: s.x,
            y_m: s.y,
            area_m2: s.area,
            k_kn_m3: s.k,
            spring_k_kn_m: s.k * s.area,
            gap_m: s.gap,
            plane_settlement_m: w,
            compression_m: compression,
            q_kpa: q,
            vertical_force_kn: force,
            active,
        });
    }
    let n = ledger.total_n_kn;
    let mx = ledger.total_mx_pressure_knm;
    let my = ledger.total_my_pressure_knm;
    let residual = [recovered[0] - n, recovered[1] - mx, recovered[2] - my];
    let normalized_residual = (residual[0] / n)
        .abs()
        .max((residual[1] / n / hy).abs())
        .max((residual[2] / n / hx).abs());
    if !normalized_residual.is_finite() || normalized_residual > 1e-8 {
        return Err(FoundationError::NumericalResidual {
            normalized_residual,
        });
    }
    let a = p[1] / hx;
    let b = p[2] / hy;
    let points = [
        ("centre", 0.0, 0.0),
        ("corner--", -hx, -hy),
        ("corner+-", hx, -hy),
        ("corner++", hx, hy),
        ("corner-+", -hx, hy),
    ];
    let control_points = points
        .into_iter()
        .map(|(label, x, y)| {
            let w = p[0] + a * x + b * y;
            ControlPointResult {
                label: label.into(),
                x_m: x,
                y_m: y,
                settlement_down_m: w,
                displacement_global_z_m: -w,
            }
        })
        .collect();
    let mut candidate = Vec::with_capacity(4);
    let mut plane_corners = match &input.soil.support {
        SupportField::Uniform { .. } => Some(Vec::with_capacity(4)),
        _ => None,
    };
    for (x, y) in [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)] {
        candidate.push(CornerPressure {
            x_m: x,
            y_m: y,
            q_kpa: n / g.area_m2() + mx * y / g.ix_m4() + my * x / g.iy_m4(),
        });
        if let (Some(corners), SupportField::Uniform { k_kn_m3, gap_m }) =
            (&mut plane_corners, &input.soil.support)
        {
            corners.push(CornerPressure {
                x_m: x,
                y_m: y,
                q_kpa: k_kn_m3 * (p[0] + a * x + b * y - gap_m).max(0.0),
            });
        }
    }
    let continuum_min = candidate
        .iter()
        .map(|c| c.q_kpa)
        .fold(f64::INFINITY, f64::min);
    let continuum_max = candidate
        .iter()
        .map(|c| c.q_kpa)
        .fold(f64::NEG_INFINITY, f64::max);
    let plane_min = plane_corners
        .as_ref()
        .map(|cs| cs.iter().map(|c| c.q_kpa).fold(f64::INFINITY, f64::min));
    let plane_max = plane_corners
        .as_ref()
        .map(|cs| cs.iter().map(|c| c.q_kpa).fold(f64::NEG_INFINITY, f64::max));
    let applicable = match &input.soil.support {
        SupportField::Uniform { gap_m, .. } if *gap_m == 0.0 => {
            ContinuumApplicability::HomogeneousFullContactCandidate
        }
        _ => ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps,
    };
    let horizontal = HorizontalActionsStatus { status:AssessmentStatus::NotImplemented,
        fx_kn:ledger.total_fx_kn,fy_kn:ledger.total_fy_kn,mz_knm:ledger.total_mz_knm,
        nonzero_unconsumed_actions:ledger.total_fx_kn != 0.0 || ledger.total_fy_kn != 0.0 || ledger.total_mz_knm != 0.0,
        note:"Fx, Fy and Mz are preserved and reported NOT_IMPLEMENTED: no sliding, lateral stiffness or torsion resistance is calculated. Only N and the two pressure moments enter contact equilibrium.".into() };
    let result = RigidMatResult { schema_version:1, foundation_id:input.foundation_id.clone(),soil_scenario_id:input.soil.id.clone(),
        input_sha256:input_fingerprint(input)?,
        mesh:MeshInfo { nx:input.mesh.nx,ny:input.mesh.ny,cell_count:springs.len(),
            dx_m:g.width_x_m/input.mesh.nx as f64,dy_m:g.length_y_m/input.mesh.ny as f64,
            cell_area_m2:springs[0].area,quadrature:"MIDPOINT_CELL_LUMPED_SPRINGS; sampled support hull and sampled contact area".into() },
        cells,plane:SettlementPlane { w0_m:p[0],dw_dx:a,dw_dy:b,rotation_about_x_rad:-b,rotation_about_y_rad:a,
            tilt_magnitude_rad:a.hypot(b),control_points },
        equilibrium:EquilibriumResult { applied_n_kn:n,applied_mx_pressure_knm:mx,applied_my_pressure_knm:my,
            original_external_mx_knm:-mx,original_external_my_knm:my,
            recovered_n_kn:recovered[0],recovered_mx_pressure_knm:recovered[1],recovered_my_pressure_knm:recovered[2],
            residual_n_kn:residual[0],residual_mx_pressure_knm:residual[1],residual_my_pressure_knm:residual[2],
            normalized_residual,maximum_sign_violation_m:sign_violation,iterations,scaled_pivot_ratio:ratio },
        contact:ContactResult { state:if active_cells == springs.len() { ContactState::FullSampledContact } else { ContactState::PartialSampledContact },
            active_cells,active_sampled_area_m2:active_cells as f64*springs[0].area,total_area_m2:g.area_m2(),
            sampled_qmin_kpa:qmin,sampled_qmax_kpa:qmax,resultant_x_m:my/n,resultant_y_m:mx/n,
            sampled_support_hull_x_m:hull_x,sampled_support_hull_y_m:hull_y },
        continuum_full_contact:FullContactContinuumComparison { applicability:applicable,candidate_has_tension:continuum_min < 0.0,
            candidate_qmin_kpa:continuum_min,candidate_qmax_kpa:continuum_max,candidate_corner_pressures:candidate,
            solved_plane_corner_pressures:plane_corners,solved_plane_corner_qmin_kpa:plane_min,solved_plane_corner_qmax_kpa:plane_max },
        self_weight_ledger:ledger,horizontal_actions_status:horizontal,
        engineering_limits:EngineeringLimits { source_type:SourceType::SyntheticAssumption,engineering_site_verified:false,
            ultimate_bearing:AssessmentStatus::UltimateBearingNotVerified,settlement:AssessmentStatus::PreliminaryEstimate,
            reinforcement:AssessmentStatus::NotImplemented,groundwater:AssessmentStatus::NotImplemented,
            notes:vec!["Synthetic unverified soil scenario; k is explicitly assumed and never universally derived from E.".into(),
                "Rigid mat, independent vertical compression-only springs, small rotations; no flexible raft or layer interaction model.".into(),
                "ULTIMATE_BEARING_NOT_VERIFIED; settlement PRELIMINARY_ESTIMATE; no normative ULS, bearing capacity, reinforcement or construction acceptance.".into(),
                "Groundwater, uplift and effective stress are NOT_IMPLEMENTED; a supplied water table is refused.".into(),
                "Full-contact continuum candidate, solved-plane corner values and midpoint samples are distinct outputs; mesh refinement remains a caller responsibility.".into()] } };
    validate_result_binding(input, &result)?;
    Ok(result)
}
