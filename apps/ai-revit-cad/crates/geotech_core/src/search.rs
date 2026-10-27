use crate::{
    geometry::{ray_exit, validate_terrain},
    input_fingerprint,
    mechanics::{analyze_validated, cancelled, validate_mechanics},
    model::*,
};
use std::sync::atomic::AtomicBool;

const MAX_CANDIDATES: usize = 10_000;
// Upper bound on per-candidate polygon-predicate work and retained geometry.
const MAX_SEARCH_WORK: usize = 20_000_000;
pub fn search(
    input: &SlopeAnalysisInput,
    spec: &CandidateSearchSpec,
) -> Result<CandidateSearchResult, GeotechError> {
    search_cancellable(input, spec, &AtomicBool::new(false))
}
pub fn search_cancellable(
    input: &SlopeAnalysisInput,
    spec: &CandidateSearchSpec,
    cancel: &AtomicBool,
) -> Result<CandidateSearchResult, GeotechError> {
    cancelled(cancel)?;
    validate_mechanics(input)?;
    for (value, name) in [
        (spec.start_angle_deg, "search.start_angle_deg"),
        (spec.end_angle_deg, "search.end_angle_deg"),
        (spec.coarse_step_deg, "search.coarse_step_deg"),
    ] {
        finite(value, name)?;
    }
    if spec.start_angle_deg <= 0.0
        || spec.end_angle_deg >= 90.0
        || spec.start_angle_deg > spec.end_angle_deg
        || spec.coarse_step_deg <= 0.0
        || spec.refinement_rounds > 12
    {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Explicit search requires 0 < start <= end < 90 deg, positive step and at most 12 refinement rounds",
        ));
    }
    let count = ((spec.end_angle_deg - spec.start_angle_deg) / spec.coarse_step_deg).floor();
    if !count.is_finite()
        || count > (MAX_CANDIDATES - 2 - spec.refinement_rounds as usize * 2) as f64
    {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Search exceeds 10000 candidate budget",
        ));
    }
    let candidate_bound = count as usize + 2 + spec.refinement_rounds as usize * 2;
    let terrain_points = input.section.terrain_polyline.len();
    let estimated_work = terrain_points
        .checked_mul(terrain_points)
        .and_then(|v| v.checked_mul(candidate_bound));
    if estimated_work.is_none_or(|v| v > MAX_SEARCH_WORK) {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Search exceeds explicit 20000000 point-pair work budget; reduce terrain point count, refinement or candidate count",
        ));
    }
    let tol = validate_terrain(&input.section)?;
    if (input.section.slip_surface.upper.x - input.section.slip_surface.lower.x).abs() <= tol {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Search needs a resolved explicit horizontal uphill direction; zero/signed-zero direction is not inferred",
        ));
    }
    let mut angles: Vec<f64> = (0..=count as usize)
        .map(|i| spec.coarse_step_deg.mul_add(i as f64, spec.start_angle_deg))
        .collect();
    if angles.last().is_none_or(|last| *last < spec.end_angle_deg) {
        angles.push(spec.end_angle_deg);
    }
    let mut output=CandidateSearchResult {classification:"minimum_within_examined_planar_family".into(),spec:*spec,
        criterion:"Unique upward planar ray exit and ordered simple terrain interval; all selected terrain on material side; no unused-terrain intrusion; positive resolved CCW prism; dry self-weight; positive driving and normal force".into(),
        tested_surfaces:0,candidates:vec![],minimum:None,minimum_input:None,source_input_fingerprint:input_fingerprint(input)?};
    for angle in angles {
        test_candidate(input, angle, tol, cancel, &mut output)?;
    }
    let mut step = spec.coarse_step_deg * 0.5;
    for _ in 0..spec.refinement_rounds {
        cancelled(cancel)?;
        let Some(best) = &output.minimum else {
            break;
        };
        let center = best.geometry.slip_angle_deg;
        for angle in [center - step, center + step] {
            if angle >= spec.start_angle_deg && angle <= spec.end_angle_deg {
                test_candidate(input, angle, tol, cancel, &mut output)?;
            }
        }
        step *= 0.5;
    }
    cancelled(cancel)?;
    output.tested_surfaces = output.candidates.len();
    Ok(output)
}
fn test_candidate(
    input: &SlopeAnalysisInput,
    angle: f64,
    tol: f64,
    cancel: &AtomicBool,
    output: &mut CandidateSearchResult,
) -> Result<(), GeotechError> {
    cancelled(cancel)?;
    if output
        .candidates
        .iter()
        .any(|c| c.angle_deg.to_bits() == angle.to_bits())
    {
        return Ok(());
    }
    if output.candidates.len() >= MAX_CANDIDATES {
        return Err(GeotechError::new(
            ErrorCode::InvalidSearch,
            "Candidate budget exhausted",
        ));
    }
    let mut candidate = input.clone();
    let result = ray_exit(&candidate.section, angle, tol).and_then(|upper| {
        candidate.section.slip_surface.upper = upper;
        analyze_validated(&candidate, tol, cancel)
    });
    match result {
        Ok(result) => {
            if output
                .minimum
                .as_ref()
                .is_none_or(|current| result.factor_of_safety < current.factor_of_safety)
            {
                output.minimum = Some(result.clone());
                output.minimum_input = Some(candidate);
            }
            output.candidates.push(CandidateRecord {
                angle_deg: angle,
                result: Some(result),
                rejection: None,
            });
        }
        Err(error) if error.code == ErrorCode::Cancelled => return Err(error),
        Err(error) => output.candidates.push(CandidateRecord {
            angle_deg: angle,
            result: None,
            rejection: Some(error),
        }),
    }
    Ok(())
}
