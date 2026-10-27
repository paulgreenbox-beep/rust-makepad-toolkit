use crate::{METHOD, SOLVER_VERSION, input_fingerprint, model::*, source_fingerprint};
use std::fmt::Write;

/// A traceable human-readable report. Rejects stale or differently sourced results.
/// An independent audit record is never manufactured by this renderer.
pub fn report(
    input: &SlopeAnalysisInput,
    result: &SlopeAnalysisResult,
    search: Option<&CandidateSearchResult>,
) -> Result<String, GeotechError> {
    if input_fingerprint(input)? != result.input_fingerprint
        || result.source_fingerprint != source_fingerprint()
        || result.solver_version != SOLVER_VERSION
        || result.method != METHOD
    {
        return Err(GeotechError::new(
            ErrorCode::StaleResult,
            "Result fingerprint/version does not match current input and solver source",
        ));
    }
    let g = &result.geometry;
    let p = &result.forces_per_m;
    let total = &result.total_forces;
    for value in [
        g.area_m2,
        g.slip_length_m,
        g.width_m,
        g.volume_m3,
        g.slip_base_area_m2,
        g.slip_angle_deg,
        p.weight_kn_per_m,
        p.normal_kn_per_m,
        p.driving_kn_per_m,
        p.cohesion_kn_per_m,
        p.friction_kn_per_m,
        p.resistance_kn_per_m,
        total.weight_kn,
        total.normal_kn,
        total.driving_kn,
        total.cohesion_kn,
        total.friction_kn,
        total.resistance_kn,
        result.factor_of_safety,
    ] {
        if !value.is_finite() {
            return Err(GeotechError::new(
                ErrorCode::NonFiniteInput,
                "Report result contains a non-finite number",
            ));
        }
    }
    let mut text = String::new();
    writeln!(text, "GEO-SLOPE CALCULATION REPORT\n").unwrap();
    writeln!(
        text,
        "1. Calculation identification\nMethod: {}\nInput snapshot: {}\n",
        result.method, result.input_fingerprint
    )
    .unwrap();
    writeln!(text,"2. Geometry and coordinate system\nx horizontal; z upward; length m; authoritative f64 geometry.\nTerrain order preserved; polygon closure is final-to-first.\nToe: ({:.12}, {:.12}) m; crest: ({:.12}, {:.12}) m.",input.section.slope_toe.x,input.section.slope_toe.z,input.section.crest.x,input.section.crest.z).unwrap();
    for (i, point) in input.section.terrain_polyline.iter().enumerate() {
        writeln!(text, "Terrain[{i}]: ({:.12}, {:.12}) m", point.x, point.z).unwrap();
    }
    writeln!(text,"\n3. Soil material parameters\nName: {}\nSource: {}\ngamma = {:.12} kN/m3\nc' = {:.12} kPa (= kN/m2)\nphi' = {:.12} deg\n",input.soil.name,input.soil.provenance,input.soil.unit_weight_kn_per_m3,input.soil.effective_cohesion_kpa,input.soil.effective_friction_angle_deg).unwrap();
    writeln!(text,"4. Assumed slip surface\nLower: ({:.12}, {:.12}) m\nUpper: ({:.12}, {:.12}) m\nalpha = {:.12} deg; L = hypot(dx,dz) = {:.12} m\n",g.slip_surface.lower.x,g.slip_surface.lower.z,g.slip_surface.upper.x,g.slip_surface.upper.z,g.slip_angle_deg,g.slip_length_m).unwrap();
    writeln!(text,"5. Applied method and assumptions\nDry homogeneous drained effective Mohr-Coulomb. One rigid block in planar translation; static self-weight only. No water, loads, anchors, seismicity, layers or lateral 3D friction.\nStandard: {}\n",input.standard.as_deref().unwrap_or("not specified; no regulatory factors applied")).unwrap();
    writeln!(text,"6. Failure mass geometry\nUnique terrain interval closed by slip segment; canonical CCW polygon:").unwrap();
    for (i, point) in g.polygon.iter().enumerate() {
        writeln!(text, "Polygon[{i}]: ({:.12}, {:.12}) m", point.x, point.z).unwrap();
    }
    writeln!(text,"\n7. Area and volume\nA = 1/2 sum cross(p_i-origin,p_(i+1)-origin) = {:.12} m2\nB = {:.12} m\nV = A B = {:.12} * {:.12} = {:.12} m3\nS = L B = {:.12} m2\n",g.area_m2,g.width_m,g.area_m2,g.width_m,g.volume_m3,g.slip_base_area_m2).unwrap();
    writeln!(
        text,
        "8. Weight\nW = gamma A = {:.12} * {:.12} = {:.12} kN/m\nW_total = W B = {:.12} kN\n",
        input.soil.unit_weight_kn_per_m3, g.area_m2, p.weight_kn_per_m, total.weight_kn
    )
    .unwrap();
    writeln!(text,"9. Normal and driving forces\nN = W cos(alpha) = {:.12} kN/m; total N = {:.12} kN\nT = W sin(alpha) = {:.12} kN/m; total T = {:.12} kN\nU = 0 kN/m (dry contract; pore-pressure extension disabled)\n",p.normal_kn_per_m,total.normal_kn,p.driving_kn_per_m,total.driving_kn).unwrap();
    writeln!(text,"10. Resisting shear force\nC = c' L = {:.12} * {:.12} = {:.12} kN/m\nFriction = N tan(phi') = {:.12} * tan({:.12} deg) = {:.12} kN/m\nR = C + friction = {:.12} kN/m; total R = {:.12} kN\n",input.soil.effective_cohesion_kpa,g.slip_length_m,p.cohesion_kn_per_m,p.normal_kn_per_m,input.soil.effective_friction_angle_deg,p.friction_kn_per_m,p.resistance_kn_per_m,total.resistance_kn).unwrap();
    writeln!(text,"11. Factor of safety\nFs = R / T = {:.12} / {:.12} = {:.12}\nRequired value: {}\nComparison: {:?}\n",p.resistance_kn_per_m,p.driving_kn_per_m,result.factor_of_safety,input.required_factor_of_safety.map(|v|format!("{v:.12}")).unwrap_or_else(||"not specified".into()),result.requirement_comparison).unwrap();
    writeln!(text, "12. Search limits, if applicable").unwrap();
    if let Some(s) = search {
        if s.minimum
            .as_ref()
            .is_none_or(|r| r.input_fingerprint != result.input_fingerprint)
        {
            return Err(GeotechError::new(
                ErrorCode::StaleResult,
                "Search metadata does not bind the reported minimum",
            ));
        }
        writeln!(text,"Classification: {}\nSource family input: {}\nStart = {:.12} deg; end = {:.12} deg; coarse step = {:.12} deg; refinement rounds = {}\nAdmissibility: {}\nTested surfaces: {}",s.classification,s.source_input_fingerprint,s.spec.start_angle_deg,s.spec.end_angle_deg,s.spec.coarse_step_deg,s.spec.refinement_rounds,s.criterion,s.tested_surfaces).unwrap();
        for c in &s.candidates {
            if let Some(r) = &c.result {
                writeln!(
                    text,
                    "alpha={:.12} deg: Fs={:.12}, A={:.12} m2, V={:.12} m3",
                    c.angle_deg, r.factor_of_safety, r.geometry.area_m2, r.geometry.volume_m3
                )
                .unwrap();
            }
            if let Some(e) = &c.rejection {
                writeln!(
                    text,
                    "alpha={:.12} deg: REJECTED {:?}: {}",
                    c.angle_deg, e.code, e.message
                )
                .unwrap();
            }
        }
    } else {
        writeln!(text, "Single specified surface; no search performed.").unwrap();
    }
    writeln!(text,"\n13. Numerical precision/verification\nTranslated compensated polygon area; coordinate-resolution tolerance = {:.12e} m\nGEOMETRY_VALID: {}\nCALCULATION_COMPLETED: {}\nNUMERICALLY_VERIFIED: {} (independent evidence required; this report does not attest it)\n",g.numeric_tolerance_m,result.status.geometry_valid,result.status.calculation_completed,result.status.numerically_verified).unwrap();
    writeln!(text, "14. Warnings and unsupported conditions").unwrap();
    for warning in &result.warnings {
        writeln!(text, "- {warning}").unwrap();
    }
    writeln!(text,"\n15. Source fingerprints\nInput: {}\nSolver Rust implementation: {}\nThis source hash does not attest executable identity, compilation or native runtime.\n\n16. Solver version\n{}\n\n17. Engineering review status\nENGINEERING_REVIEW_REQUIRED: {}\nNo construction permission, regulatory approval or global stability proof is issued.",result.input_fingerprint,result.source_fingerprint,result.solver_version,result.status.engineering_review_required).unwrap();
    Ok(text)
}
