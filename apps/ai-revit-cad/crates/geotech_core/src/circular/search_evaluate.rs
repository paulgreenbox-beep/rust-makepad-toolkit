//! Fixed-pass search adapter over the existing geometry and numerical owners.
//! A Bishop failure is one method observation and never erases Fellenius.
use super::{
    analysis::CIRCULAR_SOLVER_VERSION,
    bishop::bishop_diagnostic_cancellable,
    circular_geometry::PreparedMass,
    fellenius::fellenius,
    search_meter,
    slice_generator::{generate_prepared, slice_mechanics_cancellable},
    types::*, validation::*,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Write, sync::atomic::{AtomicBool, Ordering}};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchPassEvaluation {
    pub input_fingerprint: String,
    pub geometry: FailureMassGeometry,
    pub slices: Vec<Slice>,
    pub total_area_m2: f64,
    pub total_weight_kn_per_m: f64,
    pub area_closure_m2: f64,
    pub weight_closure_kn_per_m: f64,
    pub closure_passed: bool,
    pub fellenius: Result<FelleniusResult, CircularError>,
    pub bishop: Result<BishopResult, CircularError>,
}

pub fn prepare_search_geometry(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    cancel: &AtomicBool) -> CResult<FailureMassGeometry> {
    cancelled(cancel)?;
    search_meter::check()?;
    Ok(PreparedMass::new_cancellable(input, config, cancel)?.geometry)
}

pub fn evaluate_fixed(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    target: usize, cancel: &AtomicBool) -> CResult<SearchPassEvaluation> {
    cancelled(cancel)?;
    search_meter::check()?;
    let mut effective = *config;
    effective.slices.target_slices = target;
    effective.slices.verify_refinement = false;
    let config = &effective;
    let prepared = PreparedMass::new_cancellable(input, config, cancel)?;
    let slices = generate_prepared(&prepared, config, target, cancel)?;
    let mechanics = slice_mechanics_cancellable(&input.ground, &slices, cancel)?;
    cancelled(cancel)?;
    search_meter::check()?;
    let fellenius = fellenius(&mechanics, config);
    cancelled(cancel)?;
    search_meter::check()?;
    let bishop = bishop_diagnostic_cancellable(&mechanics, config, cancel).and_then(|mut root| {
        let endpoints = super::endpoint::assess(&prepared, &root, config);
        if !endpoints.passed { root.accepted_factor_of_safety = None; }
        root.endpoint_assessment = Some(endpoints);
        cancelled(cancel)?;
        let contact = super::contact::assess_prepared(input, config, &prepared,
            root.physical_assessment.root_bracket_lower,
            root.physical_assessment.root_bracket_upper,
            "COMPUTED_FINITE_TABLE_BISHOP_SIGN_BRACKET: conditional range; not an interval enclosure of the exact continuum Bishop root",
            cancel);
        let contact = match contact {
            Ok(report) => report,
            Err(mut failure) => {
                // Keep the independently computed mathematical table even if
                // the later contact report cannot complete. It is diagnostic
                // evidence and cannot supply an accepted Bishop factor.
                root.accepted_factor_of_safety = None;
                failure.bishop_diagnostic = Some(BishopFailureDiagnostic {
                    status: root.convergence_status,
                    admissible_lower_factor: Some(root.admissible_lower_factor),
                    last_factor: Some(root.factor_of_safety),
                    equation_residual: Some(root.equation_residual),
                    relative_residual: Some(root.relative_residual),
                    faulty_slice_id: None, m_alpha: None, iterations: Vec::new(),
                    mathematical_root: Some(Box::new(root)),
                });
                return Err(failure);
            }
        };
        if contact.status != WholeArcContactStatus::PassedDeclaredModel {
            root.accepted_factor_of_safety = None;
        }
        root.whole_arc_contact = Some(contact);
        // No existing physical/endpoint/contact global eligibility flag is
        // changed. The search owner separately records its reviewed policy.
        Ok(root)
    });
    cancelled(cancel)?;
    search_meter::check()?;
    // Preserve the specified-circle owner's summation order and independent
    // closure quantities. A failed closure remains an explicit pass gate.
    let mut total_area = Sum::default();
    let mut total_weight = Sum::default();
    for slice in &slices {
        cancelled(cancel)?;
        search_meter::check()?;
        total_area.add(slice.total_area_m2);
        total_weight.add(slice.weight_kn_per_m);
    }
    let area = positive(total_area.total(), "total slice area")?;
    let weight = positive(total_weight.total(), "total slice weight")?;
    let mut expected_weight = Sum::default();
    for a in &prepared.geometry.material_areas {
        cancelled(cancel)?;
        let m = input.ground.materials.iter().find(|m| m.material_id == a.material_id).unwrap();
        expected_weight.add(a.area_m2 * m.properties.unit_weight_kn_per_m3);
    }
    let area_closure = (area - prepared.geometry.area_m2).abs();
    let weight_closure = (weight - expected_weight.total()).abs();
    let closure_passed = area_closure <= config.tolerances.area_abs_m2
        + config.tolerances.area_rel * prepared.geometry.area_m2
        && weight_closure <= config.tolerances.force_abs_kn_per_m
            + config.tolerances.force_rel * weight;
    let input_fingerprint = fingerprint(input, config, cancel)?;
    let output = SearchPassEvaluation { input_fingerprint, geometry: prepared.geometry,
        slices, total_area_m2: area, total_weight_kn_per_m: weight,
        area_closure_m2: area_closure, weight_closure_kn_per_m: weight_closure,
        closure_passed, fellenius, bishop };
    let mut counter = PassBytes { used: 0, limit: config.limits.max_result_bytes, cancel };
    let serialized = serde_json::to_writer(&mut counter, &output);
    cancelled(cancel)?;
    search_meter::check()?;
    serialized.map_err(|_| error(CircularErrorCode::ResourceLimit,
        "Serialized fixed-pass evidence exceeds shared max_result_bytes"))?;
    Ok(output)
}

fn fingerprint(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    cancel: &AtomicBool) -> CResult<String> {
    struct HashWriter<'a>(&'a mut Sha256, &'a AtomicBool);
    impl Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.1.load(Ordering::Relaxed) || search_meter::check().is_err() {
                return Err(std::io::Error::other("search fingerprint cancelled/budget"));
            }
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut hash = Sha256::new();
    let encoded = serde_json::to_writer(HashWriter(&mut hash, cancel), &(CIRCULAR_SOLVER_VERSION, input, config));
    cancelled(cancel)?;
    search_meter::check()?;
    encoded.map_err(|e| error(CircularErrorCode::InvalidParameter,
        format!("Fixed-pass input serialization: {e}")))?;
    Ok(format!("sha256:{:x}", hash.finalize()))
}

struct PassBytes<'a> { used: usize, limit: usize, cancel: &'a AtomicBool }
impl Write for PassBytes<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) || search_meter::check().is_err() {
            return Err(std::io::Error::other("search pass serialization cancelled/budget"));
        }
        self.used = self.used.checked_add(bytes.len()).filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("search pass byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
