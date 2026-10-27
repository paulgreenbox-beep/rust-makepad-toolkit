//! Bounded whole-lower-arc contact screen for a declared numerical envelope.
//!
//! The covered domain comes from raw source coordinates, not rounded terrain
//! intersection roots. PreparedMass roots and vertices are subdivision hints.
//! This screen is conditional on its supplied factor range; it does not prove
//! that a computed Bishop bracket encloses the exact continuum root.
use super::{
    analysis::source_fingerprint,
    circular_geometry::PreparedMass,
    contact_interval::{Interval as I, friction_tangent},
    types::*,
    validation::{CResult, cancelled, error, validate_config},
};
use crate::Point2;
use sha2::{Digest, Sha256};
use std::{io::Write, sync::atomic::{AtomicBool, Ordering}};
use super::search_meter::{self, charge, WorkKind};

const PUBLIC_RANGE_SCOPE: &str = "CALLER_SUPPLIED_FACTOR_RANGE: conditional on enclosure; no independent exact-root proof";

#[derive(Clone, Copy)]
struct Vertex { x: I, z: I }

#[derive(Clone, Copy)]
struct Cell { x: I, depth: usize }

struct Envelope<'a> {
    boundaries: Vec<Vec<Vertex>>,
    materials: Vec<&'a GroundMaterial>,
    tangents: Vec<Option<I>>,
    center_x: I,
    center_z: I,
    radius: I,
    hints: Vec<f64>,
    origin: Point2,
}

enum BoundFailure { Budget, Arithmetic }

struct Evaluation {
    detail: ContactIntervalReport,
    refinable: bool,
}

fn local_bound(value: f64, origin: f64, margin: f64) -> Option<I> {
    I::point(value)?.sub(I::point(origin)?)?.expand(margin)
}

fn fingerprint(
    input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    lower: f64, upper: f64, range_scope: &str, source: &str, cancel: &AtomicBool,
) -> CResult<String> {
    struct HashWriter<'a>(&'a mut Sha256, &'a AtomicBool);
    impl Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.1.load(Ordering::Relaxed) || search_meter::check().is_err() {
                // write_all retries Interrupted indefinitely; cancellation
                // must terminate serialization before mapping to Cancelled.
                return Err(std::io::Error::other("contact fingerprint cancelled"));
            }
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut hash = Sha256::new();
    // Stream the complete binding; do not allocate an input-sized JSON buffer.
    let serialized = serde_json::to_writer(HashWriter(&mut hash, cancel), &(
        "whole-arc-contact-envelope-v1", input, config, lower, upper, range_scope, source,
    ));
    cancelled(cancel)?;
    search_meter::check()?;
    serialized.map_err(|e| error(CircularErrorCode::InvalidParameter,
        format!("Whole-arc contact fingerprint serialization: {e}")))?;
    Ok(format!("sha256:{:x}", hash.finalize()))
}

fn add_reason(report: &mut WholeArcContactReport, reason: &str) {
    if report.reasons.len() < 16 && !report.reasons.iter().any(|r| r == reason) {
        report.reasons.push(reason.to_owned());
    }
}

fn bound_min(target: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *target = Some(target.map_or(value, |old| old.min(value)));
    }
}

fn record(report: &mut WholeArcContactReport, detail: ContactIntervalReport, detail_bytes: &mut usize,
          result_limit: usize) {
    report.leaf_intervals += 1;
    report.maximum_depth_reached = report.maximum_depth_reached.max(detail.depth);
    if detail.status != ContactIntervalStatus::OutsideFailureMass {
        bound_min(&mut report.minimum_b_lower_kpa, detail.b_lower_kpa);
        bound_min(&mut report.minimum_d_lower, detail.d_lower);
        bound_min(&mut report.minimum_cos_lower, detail.cos_lower);
    }
    // Keep both the explicit detail cap and a conservative memory allowance.
    // Truncating details never changes the coverage/status aggregation.
    let estimate = detail.material_ids.iter().try_fold(1024_usize + detail.reason.len(),
        |n, id| n.checked_add(id.0.len().saturating_mul(6).saturating_add(32)));
    if report.intervals.len() < report.config.max_report_intervals
        && estimate.and_then(|n| detail_bytes.checked_add(n)).is_some_and(|n| n <= result_limit / 2)
    {
        *detail_bytes += estimate.unwrap();
        report.intervals.push(detail);
    } else {
        report.details_truncated = true;
    }
}

fn world_interval(x: I, origin: f64) -> Option<I> { x.add(I::point(origin)?) }

fn empty_detail(cell: Cell, env: &Envelope<'_>) -> Option<ContactIntervalReport> {
    let world = world_interval(cell.x, env.origin.x)?;
    Some(ContactIntervalReport {
        x_left_m: world.lo, x_right_m: world.hi, depth: cell.depth,
        status: ContactIntervalStatus::Unresolved, material_ids: Vec::new(),
        material_union_exhaustive: false, structural_nonnegative_b: false,
        b_lower_kpa: None, b_upper_kpa: None, d_lower: None, d_upper: None,
        cos_lower: None, reason: "Unresolved numerical envelope".into(),
    })
}

impl<'a> Envelope<'a> {
    fn build(input: &'a CircularAnalysisInput, prepared: &PreparedMass<'_>,
             config: &CircularAnalysisConfig, report: &mut WholeArcContactReport,
             cancel: &AtomicBool) -> CResult<Option<Self>> {
        let margin = report.geometry_envelope_m;
        let origin = prepared.ground.origin;
        let center = input.slip_surface.center;
        let Some(center_x) = local_bound(center.x, origin.x, margin) else { return Ok(None); };
        let Some(center_z) = local_bound(center.z, origin.z, margin) else { return Ok(None); };
        let Some(radius) = I::point(input.slip_surface.radius_m).and_then(|v| v.expand(margin)) else { return Ok(None); };
        if radius.lo <= 0.0 { return Ok(None); }
        let mut hints = Vec::new();
        // Hints partition the complete domain; their coordinates grant no proof.
        for x in std::iter::once(prepared.circle.center.x).chain(prepared.breakpoints.iter().copied()) {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            if hints.len() < config.contact.max_intervals { hints.push(x); }
        }
        let mut boundaries = Vec::new();
        for source in std::iter::once(&input.ground.terrain_polyline)
            .chain(input.ground.layer_interfaces.iter().map(|i| &i.polyline))
            .chain(std::iter::once(&input.ground.bottom_boundary))
        {
            cancelled(cancel)?;
            let mut boundary = Vec::new();
            for p in source {
                cancelled(cancel)?;
                if report.boundary_evaluations >= config.contact.max_boundary_evaluations {
                    add_reason(report, "Boundary work budget exhausted while enclosing source vertices");
                    return Ok(None);
                }
                charge(WorkKind::ContactBoundaryEvaluations, 1)?;
                report.boundary_evaluations += 1;
                let Some(x) = local_bound(p.x, origin.x, margin) else { return Ok(None); };
                let Some(z) = local_bound(p.z, origin.z, margin) else { return Ok(None); };
                if hints.len() < config.contact.max_intervals {
                    hints.push(x.lo * 0.5 + x.hi * 0.5);
                }
                boundary.push(Vertex { x, z });
            }
            boundaries.push(boundary);
        }
        let mut materials = Vec::new();
        let mut tangents = Vec::new();
        for region in &input.ground.soil_regions {
            cancelled(cancel)?;
            if report.material_checks >= config.contact.max_material_checks {
                add_reason(report, "Material work budget exhausted while enclosing friction");
                return Ok(None);
            }
            charge(WorkKind::ContactMaterialChecks, 1)?;
            report.material_checks += 1;
            let material = input.ground.materials.iter().find(|m| m.material_id == region.material_id)
                .ok_or_else(|| error(CircularErrorCode::MissingMaterial, "Whole-arc material reference is missing"))?;
            materials.push(material);
            tangents.push(friction_tangent(material.properties.effective_friction_angle_deg,
                config.contact.friction_uncertainty_rad));
        }
        search_meter::sort_by(&mut hints, cancel, f64::total_cmp)?;
        search_meter::dedup_by(&mut hints, cancel, |a, b| *a == *b)?;
        Ok(Some(Self { boundaries, materials, tangents, center_x, center_z, radius, hints, origin }))
    }

    fn boundary(&self, index: usize, x: I, config: &CircularAnalysisConfig,
                report: &mut WholeArcContactReport, cancel: &AtomicBool) -> CResult<Result<I, BoundFailure>> {
        let mut bound: Option<I> = None;
        for pair in self.boundaries[index].windows(2) {
            cancelled(cancel)?;
            if report.boundary_evaluations >= config.contact.max_boundary_evaluations {
                return Ok(Err(BoundFailure::Budget));
            }
            charge(WorkKind::ContactBoundaryEvaluations, 1)?;
            report.boundary_evaluations += 1;
            let a = pair[0];
            let b = pair[1];
            if x.hi < a.x.lo || x.lo > b.x.hi { continue; }
            let value = (|| {
                let denominator = b.x.sub(a.x)?;
                if denominator.lo <= 0.0 { return None; }
                let t = x.sub(a.x)?.div(denominator)?.intersect(I::new(0.0, 1.0)?)?;
                a.z.add(t.mul(b.z.sub(a.z)?)?)
            })();
            let Some(value) = value else { return Ok(Err(BoundFailure::Arithmetic)); };
            bound = Some(bound.map_or(value, |v| v.hull(value)));
        }
        Ok(bound.ok_or(BoundFailure::Arithmetic))
    }

    fn split(&self, cell: Cell) -> Option<f64> {
        let midpoint = cell.x.lo + (cell.x.hi - cell.x.lo) * 0.5;
        if !midpoint.is_finite() || midpoint <= cell.x.lo || midpoint >= cell.x.hi { return None; }
        let i = self.hints.partition_point(|x| *x < midpoint);
        let mut split = midpoint;
        let mut distance = f64::INFINITY;
        for hint in [i.checked_sub(1).and_then(|j| self.hints.get(j)), self.hints.get(i)].into_iter().flatten() {
            // Avoid arbitrarily lopsided subdivisions around close hints.
            let width = cell.x.hi - cell.x.lo;
            if *hint > cell.x.lo + width * 0.125 && *hint < cell.x.hi - width * 0.125
                && (*hint - midpoint).abs() < distance {
                split = *hint;
                distance = (*hint - midpoint).abs();
            }
        }
        Some(split)
    }
}

fn evaluate(cell: Cell, env: &Envelope<'_>, input: &CircularAnalysisInput,
            config: &CircularAnalysisConfig, factors: I, report: &mut WholeArcContactReport,
            cancel: &AtomicBool) -> CResult<Evaluation> {
    let Some(mut detail) = empty_detail(cell, env) else {
        return Ok(Evaluation { detail: ContactIntervalReport {
            x_left_m: report.domain_x_left_m, x_right_m: report.domain_x_right_m, depth: cell.depth,
            status: ContactIntervalStatus::Unresolved, material_ids: Vec::new(),
            material_union_exhaustive: false, structural_nonnegative_b: false,
            b_lower_kpa: None, b_upper_kpa: None, d_lower: None, d_upper: None,
            cos_lower: None, reason: "World coordinate bounds are not representable".into(),
        }, refinable: false });
    };
    macro_rules! unresolved {
        ($reason:expr, $refinable:expr) => {{
            detail.reason = $reason.into();
            return Ok(Evaluation { detail, refinable: $refinable });
        }};
    }
    let Some(raw_u) = cell.x.sub(env.center_x).and_then(|v| v.div(env.radius)) else {
        unresolved!("Circle abscissa arithmetic is not representable", false);
    };
    let Some(u) = raw_u.intersect(I::new(-1.0, 1.0).unwrap()) else {
        detail.status = ContactIntervalStatus::OutsideFailureMass;
        detail.reason = "Cell is definitely outside every supported circle abscissa".into();
        return Ok(Evaluation { detail, refinable: false });
    };
    let Some((sin, cos, arc)) = (|| {
        let sin = u.scale(-input.slip_surface.slide_direction.sign())?;
        // Intersect with the exact lower-circle radicand domain. This is not
        // clipping a computed normal force or choosing a material.
        let radicand = I::point(1.0)?.sub(u.mul(u)?.positive_part())?
            .intersect(I::new(0.0, 1.0)?)?;
        let cos = radicand.sqrt()?;
        let arc = env.center_z.sub(env.radius.mul(cos)?)?;
        Some((sin, cos, arc))
    })() else { unresolved!("Circle ordinate/cosine bounds are unresolved", true); };
    detail.cos_lower = Some(cos.lo);
    let mut bounds = Vec::new();
    for index in 0..env.boundaries.len() {
        cancelled(cancel)?;
        let bound = match env.boundary(index, cell.x, config, report, cancel)? {
            Ok(value) => value,
            Err(BoundFailure::Budget) => unresolved!("Boundary work budget exhausted", false),
            Err(BoundFailure::Arithmetic) => unresolved!("Source boundary interpolation is unresolved", true),
        };
        if index == 0 && arc.lo > bound.hi {
            detail.status = ContactIntervalStatus::OutsideFailureMass;
            detail.reason = "Lower circle is definitely above the complete terrain envelope".into();
            return Ok(Evaluation { detail, refinable: false });
        }
        bounds.push(bound);
    }
    let terrain = bounds[0];
    let bottom = *bounds.last().unwrap();
    if arc.lo < bottom.hi { unresolved!("Arc is not resolved above the closed bottom envelope", true); }
    if cos.lo <= config.tolerances.min_base_cos {
        unresolved!("Continuous tangent is vertical or unresolved against min_base_cos", true);
    }
    // q is the sum of physical column/layer intersections. Positive-part here
    // defines layer thickness, and never alters N or the Bishop equation.
    let mut q = I::point(0.0).unwrap();
    for (index, material) in env.materials.iter().enumerate() {
        cancelled(cancel)?;
        if report.material_checks >= config.contact.max_material_checks {
            unresolved!("Material work budget exhausted while bounding column weight", false);
        }
        charge(WorkKind::ContactMaterialChecks, 1)?;
        report.material_checks += 1;
        let Some(thickness) = bounds[index].sub(bounds[index + 1].max(arc)) else {
            unresolved!("Column thickness arithmetic is not representable", false);
        };
        let Some(next) = thickness.positive_part().scale(material.properties.unit_weight_kn_per_m3)
            .and_then(|weight| q.add(weight)) else {
            unresolved!("Column weight bounds are not representable", false);
        };
        // Every exact summand is nonnegative, including zero-weight contacts.
        q = next.positive_part();
    }
    let Some(m_guard) = I::point(factors.hi).and_then(|f| f.scale(config.bishop.min_m_alpha)) else {
        unresolved!("Denominator safety threshold is not representable", false);
    };
    let mut all_pass = true;
    let mut all_violate = true;
    let mut all_structural = true;
    let mut b_union: Option<I> = None;
    let mut d_union: Option<I> = None;
    for (index, material) in env.materials.iter().enumerate() {
        cancelled(cancel)?;
        if report.material_checks >= config.contact.max_material_checks {
            unresolved!("Material work budget exhausted while checking base possibilities", false);
        }
        charge(WorkKind::ContactMaterialChecks, 1)?;
        report.material_checks += 1;
        if arc.lo > bounds[index].hi || arc.hi < bounds[index + 1].lo { continue; }
        // Check every possible one-sided material at a crossing, even when
        // another candidate has a better sign or the midpoint is unambiguous.
        detail.material_ids.push(material.material_id.clone());
        let Some(tan) = env.tangents[index] else {
            unresolved!("Declared friction interval cannot be enclosed below its pole", false);
        };
        let cohesion = material.properties.effective_cohesion_kpa;
        let structural = cohesion == 0.0 || sin.hi <= 0.0;
        all_structural &= structural;
        let Some((mut b, d)) = (|| {
            let b = factors.mul(q)?.mul(cos)?.sub(sin.scale(cohesion)?)?;
            let d = factors.mul(cos)?.add(sin.mul(tan)?)?;
            Some((b, d))
        })() else { unresolved!("Contact sign arithmetic is not representable", false); };
        if structural {
            // Algebra proves B>=0 for q,c,F>=0 and positive cosine. Retain
            // this fact at q=0 instead of endlessly subdividing roundoff.
            b = b.positive_part();
        }
        all_pass &= b.lo >= 0.0 && d.lo > 0.0 && d.lo > m_guard.hi;
        all_violate &= b.hi < 0.0 || d.hi <= 0.0;
        b_union = Some(b_union.map_or(b, |old| old.hull(b)));
        d_union = Some(d_union.map_or(d, |old| old.hull(d)));
    }
    if detail.material_ids.is_empty() {
        unresolved!("No exhaustive base-material possibility covers the arc envelope", true);
    }
    detail.material_union_exhaustive = true;
    detail.structural_nonnegative_b = all_structural;
    detail.b_lower_kpa = b_union.map(|v| v.lo);
    detail.b_upper_kpa = b_union.map(|v| v.hi);
    detail.d_lower = d_union.map(|v| v.lo);
    detail.d_upper = d_union.map(|v| v.hi);
    if all_pass {
        detail.status = ContactIntervalStatus::Passed;
        detail.reason = "Every possible base material has B>=0 and positive resolved D throughout this cell".into();
        Ok(Evaluation { detail, refinable: false })
    } else if all_violate && arc.hi < terrain.lo && arc.lo > bottom.hi
        && env.boundaries.iter().all(|boundary|
            cell.x.lo >= boundary[0].x.hi && cell.x.hi <= boundary.last().unwrap().x.lo)
    {
        // An envelope containing outside-ground points cannot establish a
        // physical counterexample. Definite violation needs an interior mass
        // cell, guaranteed common X support of every source boundary, and a
        // violation for every still-possible base material. Outward endpoint
        // fringes alone cannot witness a violation of the raw-source model.
        detail.status = ContactIntervalStatus::Violation;
        detail.reason = "Interior mass cell violates B>=0 or D>0 for every possible base material and supplied factor".into();
        Ok(Evaluation { detail, refinable: false })
    } else {
        detail.reason = "A contact lower bound is unresolved; sampling or midpoint material cannot grant PASS".into();
        Ok(Evaluation { detail, refinable: true })
    }
}

/// Screen the supported whole lower arc for a caller-declared positive factor
/// range. A passed report remains conditional and is never search eligibility.
pub fn screen_whole_arc_contact(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    factor_lower: f64, factor_upper: f64) -> CResult<WholeArcContactReport> {
    screen_whole_arc_contact_cancellable(input, config, factor_lower, factor_upper, &AtomicBool::new(false))
}

pub fn screen_whole_arc_contact_cancellable(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    factor_lower: f64, factor_upper: f64, cancel: &AtomicBool) -> CResult<WholeArcContactReport> {
    cancelled(cancel)?;
    validate_config(config)?;
    let prepared = PreparedMass::new_cancellable(input, config, cancel)?;
    let report = assess_prepared(input, config, &prepared, factor_lower, factor_upper, PUBLIC_RANGE_SCOPE, cancel)?;
    // The standalone API must honor the same strict serialized output bound
    // as geometric analysis, including fixed metadata and escaped strings.
    struct ReportBudget<'a> { used: usize, limit: usize, cancel: &'a AtomicBool }
    impl Write for ReportBudget<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::other("contact report cancelled"));
            }
            self.used = self.used.checked_add(bytes.len()).filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("contact report byte budget"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let serialized = serde_json::to_writer(ReportBudget { used: 0,
        limit: config.limits.max_result_bytes, cancel }, &report);
    cancelled(cancel)?;
    serialized.map_err(|_| error(CircularErrorCode::ResourceLimit,
        "Serialized whole-arc contact report exceeds max_result_bytes"))?;
    Ok(report)
}

pub(crate) fn assess_prepared(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    prepared: &PreparedMass<'_>, factor_lower: f64, factor_upper: f64,
    root_range_scope: &str, cancel: &AtomicBool) -> CResult<WholeArcContactReport> {
    cancelled(cancel)?;
    validate_config(config)?;
    let factors = I::new(factor_lower, factor_upper).filter(|f| f.lo > 0.0
        && f.lo >= config.bishop.min_factor && f.hi <= config.bishop.max_factor)
        .ok_or_else(|| error(CircularErrorCode::InvalidParameter,
            "Whole-arc factor range must be finite, positive, ordered and within declared Bishop bounds"))?;
    if root_range_scope.is_empty() || root_range_scope.len() > 4096 {
        return Err(error(CircularErrorCode::InvalidParameter, "Whole-arc root range scope is missing or too long"));
    }
    let source = source_fingerprint();
    let binding = fingerprint(input, config, factors.lo, factors.hi, root_range_scope, &source, cancel)?;
    let margin = I::point(prepared.ground.tolerance)
        .and_then(|t| t.scale(config.contact.geometry_margin_multiplier)).map(|m| m.hi);
    let mut report = WholeArcContactReport {
        status: WholeArcContactStatus::Unresolved,
        scope: "BOUNDED_OUTWARD_ROUNDED_WHOLE_LOWER_ARC_NUMERICAL_ENVELOPE; raw binary64 source coordinates, declared geometry/friction uncertainty; conditional on supplied factor range; no continuum-root/discretization/engineering proof".into(),
        root_range_scope: root_range_scope.into(), contact_input_fingerprint: binding,
        source_fingerprint: source, config: config.contact, factor_lower, factor_upper,
        geometry_envelope_m: margin.unwrap_or(0.0),
        domain_x_left_m: input.ground.terrain_polyline[0].x,
        domain_x_right_m: input.ground.terrain_polyline.last().unwrap().x,
        coverage_complete: false, minimum_b_lower_kpa: None, minimum_d_lower: None,
        minimum_cos_lower: None, intervals_evaluated: 0, leaf_intervals: 0,
        subdivisions: 0, boundary_evaluations: 0, material_checks: 0,
        maximum_depth_reached: 0, details_truncated: false, intervals: Vec::new(), reasons: Vec::new(),
        independent_whole_arc_acceptance: false, eligible_for_critical_minimum: false,
    };
    if margin.is_none() {
        add_reason(&mut report, "Declared geometry envelope is not representable");
        return Ok(report);
    }
    let Some(env) = Envelope::build(input, prepared, config, &mut report, cancel)? else {
        add_reason(&mut report, "Source geometry/friction envelope could not be prepared within work bounds");
        return Ok(report);
    };
    let domain = (|| {
        let circle_left = env.center_x.sub(env.radius)?;
        let circle_right = env.center_x.add(env.radius)?;
        let terrain = &env.boundaries[0];
        I::new(terrain[0].x.lo.max(circle_left.lo), terrain.last()?.x.hi.min(circle_right.hi))
    })();
    let Some(domain) = domain.filter(|d| d.hi > d.lo) else {
        add_reason(&mut report, "Whole-circle/source-span domain is empty or unrepresentable");
        return Ok(report);
    };
    let Some(world_domain) = world_interval(domain, env.origin.x) else {
        add_reason(&mut report, "Whole-arc world domain is not representable");
        return Ok(report);
    };
    report.domain_x_left_m = world_domain.lo;
    report.domain_x_right_m = world_domain.hi;
    let mut stack = vec![Cell { x: domain, depth: 0 }];
    let mut unresolved = false;
    let mut passed = 0_usize;
    let mut detail_bytes = 0_usize;
    while let Some(cell) = stack.pop() {
        cancelled(cancel)?;
        if report.intervals_evaluated >= config.contact.max_intervals {
            add_reason(&mut report, "Interval evaluation budget exhausted with uncovered cells");
            unresolved = true;
            if let Some(mut detail) = empty_detail(cell, &env) {
                detail.reason = "Interval work budget exhausted before evaluating this cell".into();
                record(&mut report, detail, &mut detail_bytes, config.limits.max_result_bytes);
            }
            // Pending cells remain explicitly uncovered; no allocation or
            // further numeric evaluation is performed after budget exhaustion.
            break;
        }
        charge(WorkKind::ContactIntervals, 1)?;
        report.intervals_evaluated += 1;
        report.maximum_depth_reached = report.maximum_depth_reached.max(cell.depth);
        let mut evaluation = evaluate(cell, &env, input, config, factors, &mut report, cancel)?;
        if evaluation.detail.status == ContactIntervalStatus::Unresolved && evaluation.refinable {
            let width = cell.x.hi - cell.x.lo;
            if cell.depth < config.contact.max_depth && width > config.contact.min_interval_width_m {
                // Account for every already-pending cell and both children
                // before allocating children. Hitting this budget leaves the
                // current cell unresolved without growing the stack.
                let can_allocate = report.intervals_evaluated.checked_add(stack.len())
                    .and_then(|n| n.checked_add(2))
                    .is_some_and(|n| n <= config.contact.max_intervals);
                if !can_allocate {
                    evaluation.detail.reason.push_str("; subdivision interval work budget cannot cover both children");
                }
                if let Some(split) = env.split(cell).filter(|_| can_allocate) {
                    // A depth-first stack has at most max_depth+1 entries.
                    // The two closed children exactly cover the parent cell.
                    stack.push(Cell { x: I::new(split, cell.x.hi).unwrap(), depth: cell.depth + 1 });
                    stack.push(Cell { x: I::new(cell.x.lo, split).unwrap(), depth: cell.depth + 1 });
                    report.subdivisions += 1;
                    continue;
                }
            }
            evaluation.detail.reason.push_str("; subdivision depth/width/resolution bound reached");
        }
        match evaluation.detail.status {
            ContactIntervalStatus::Passed => passed += 1,
            ContactIntervalStatus::Unresolved => {
                unresolved = true;
                add_reason(&mut report, &evaluation.detail.reason);
            }
            ContactIntervalStatus::Violation => {
                report.status = WholeArcContactStatus::Violation;
                add_reason(&mut report, &evaluation.detail.reason);
                record(&mut report, evaluation.detail, &mut detail_bytes, config.limits.max_result_bytes);
                // A definite interior counterexample is sufficient to reject;
                // coverage_complete remains false when other cells are pending.
                report.coverage_complete = false;
                return Ok(report);
            }
            ContactIntervalStatus::OutsideFailureMass => {}
        }
        let budget_exhausted = report.boundary_evaluations >= config.contact.max_boundary_evaluations
            || report.material_checks >= config.contact.max_material_checks;
        record(&mut report, evaluation.detail, &mut detail_bytes, config.limits.max_result_bytes);
        if budget_exhausted && !stack.is_empty() {
            unresolved = true;
            add_reason(&mut report, "Boundary/material work budget exhausted with uncovered cells");
            break;
        }
    }
    cancelled(cancel)?;
    report.coverage_complete = !unresolved && stack.is_empty() && passed > 0;
    if report.coverage_complete {
        report.status = WholeArcContactStatus::PassedDeclaredModel;
        add_reason(&mut report, "Every possibly occupied lower-arc cell passed for every possible material in the declared envelope");
    } else {
        add_reason(&mut report, "Whole-arc numerical coverage/signs remain unresolved; result is ineligible");
    }
    Ok(report)
}
