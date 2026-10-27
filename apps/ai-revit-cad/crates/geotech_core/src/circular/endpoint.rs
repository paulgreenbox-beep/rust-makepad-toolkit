//! One-sided contact limits for the supported continuous full-span layer stack.
use super::{circular_geometry::{PreparedMass, world}, ground_model::eval, physical::ROUNDING, types::*};

pub(crate) fn assess(prepared: &PreparedMass<'_>, root: &BishopResult, config: &CircularAnalysisConfig) -> BishopEndpointAssessment {
    let endpoints = [(prepared.left, 1_i8), (prepared.right, -1_i8)].into_iter().map(|(p, inward)| {
        let entry = (inward == 1) == (prepared.geometry.circle.surface.slide_direction == SlideDirection::PositiveX);
        let mut out = BishopEndpointContact {
            contact_id: if inward == 1 { "left_contact" } else { "right_contact" }.into(),
            endpoint_type: if entry { BishopEndpointType::Entry } else { BishopEndpointType::Exit },
            point: world(p, prepared.ground.origin), inward_x_direction: inward,
            material_id: None, material_separation_m: None, cohesion_kpa: None, friction_angle_rad: None,
            sin_alpha: None, cos_alpha: None, angle_uncertainty_rad: None,
            factor: root.factor_of_safety,
            factor_lower: root.physical_assessment.root_bracket_lower,
            factor_upper: root.physical_assessment.root_bracket_upper,
            m_alpha: None, denominator_f_cos_plus_sin_tan: None, denominator_lower_bound: None,
            normal_stress_limit_kpa: None, status: BishopEndpointStatus::Unresolved,
            reason: "Unresolved one-sided endpoint contact".into(),
        };
        // GroundModel::new forbids touching/crossing boundaries and outcrops.
        // At every terrain contact the strictly separated, continuous top
        // region therefore persists in an interior neighborhood of the arc.
        // No coarse-slice midpoint or arbitrary epsilon probe chooses material.
        let terrain = eval(&prepared.ground.boundaries[0], p.x);
        let below = eval(&prepared.ground.boundaries[1], p.x);
        let gap = terrain - below;
        out.material_separation_m = gap.is_finite().then_some(gap);
        if !gap.is_finite() || gap <= 4.0 * prepared.ground.tolerance {
            out.reason = "One-sided top material is not resolved above the endpoint geometric uncertainty".into();
            return out;
        }
        let material = prepared.ground.material(0);
        out.material_id = Some(material.material_id.clone());
        let c = material.properties.effective_cohesion_kpa;
        let phi = material.properties.effective_friction_angle_deg.to_radians();
        out.cohesion_kpa = Some(c);
        out.friction_angle_rad = Some(phi);
        let sin = -prepared.geometry.circle.surface.slide_direction.sign()
            * ((p.x - prepared.circle.center.x) / prepared.circle.radius);
        if !sin.is_finite() || sin.abs() >= 1.0 {
            out.reason = "Endpoint tangent is vertical or unresolved".into();
            return out;
        }
        let cos = (1.0 - sin.abs()).sqrt() * (1.0 + sin.abs()).sqrt();
        let mut sin_error = 128.0 * f64::EPSILON * (1.0 + sin.abs())
            + 4.0 * prepared.ground.tolerance / prepared.circle.radius;
        sin_error += ROUNDING * (1.0 + sin.abs() + sin_error);
        let upper_sin = sin.abs() + sin_error;
        out.sin_alpha = Some(sin);
        out.cos_alpha = Some(cos);
        if !upper_sin.is_finite() || upper_sin >= 1.0 {
            out.reason = "Endpoint inclination is unresolved or too close to vertical".into();
            return out;
        }
        // Bound the nonlinear cosine over the whole sine interval. A tangent
        // (first-derivative) estimate would understate the loss near vertical.
        let cos_lower_raw = (1.0 - upper_sin).sqrt() * (1.0 + upper_sin).sqrt();
        let cos_lower = cos_lower_raw - ROUNDING * (1.0 + cos_lower_raw);
        if !cos_lower.is_finite() || cos_lower <= config.tolerances.min_base_cos {
            out.reason = "Endpoint inclination is unresolved or too close to vertical".into();
            return out;
        }
        let angle_error = sin_error / cos_lower;
        out.angle_uncertainty_rad = angle_error.is_finite().then_some(angle_error);
        let tan = phi.tan();
        let denominator = root.factor_of_safety * cos + sin * tan;
        let lower = out.factor_lower * cos_lower + sin * tan - tan.abs() * sin_error
            - ROUNDING * ((out.factor_lower * cos).abs() + (sin * tan).abs());
        let m = denominator / root.factor_of_safety;
        out.denominator_f_cos_plus_sin_tan = denominator.is_finite().then_some(denominator);
        out.denominator_lower_bound = lower.is_finite().then_some(lower);
        out.m_alpha = m.is_finite().then_some(m);
        if !lower.is_finite() || lower <= out.factor_upper * config.bishop.min_m_alpha
            || !m.is_finite() || m <= config.bishop.min_m_alpha {
            out.reason = "Endpoint denominator is nonpositive or unresolved over the numerical factor bracket".into();
            return out;
        }
        let limit = -c * sin / denominator;
        if !limit.is_finite() {
            out.reason = "Endpoint normal-stress limit is not representable".into();
            return out;
        }
        out.normal_stress_limit_kpa = Some(limit);
        if c > 0.0 && sin - sin_error > 0.0 {
            out.status = BishopEndpointStatus::TensileInterval;
            out.reason = "Positive cohesion and positive limiting inclination imply a tensile interior neighborhood: W/l -> 0, N/l -> -c*sin(alpha)/(F*cos(alpha)+sin(alpha)*tan(phi)) < 0".into();
        } else if c == 0.0 {
            out.status = BishopEndpointStatus::CohesionlessLocalScreenPassed;
            out.reason = "Cohesion is identically zero in the resolved one-sided material neighborhood; W>=0 and m_alpha>0 give locally nonnegative N, not whole-arc acceptance".into();
        } else if sin + sin_error < 0.0 {
            out.status = BishopEndpointStatus::PositiveLimitLocalScreenPassed;
            out.reason = "Positive endpoint normal-stress limit screens only the contact neighborhood; the rest of the arc remains unverified".into();
        } else {
            out.reason = "Zero or unresolved limiting inclination requires higher-order contact analysis".into();
        }
        out
    }).collect::<Vec<_>>();
    let passed = endpoints.iter().all(|e| matches!(e.status,
        BishopEndpointStatus::CohesionlessLocalScreenPassed | BishopEndpointStatus::PositiveLimitLocalScreenPassed));
    BishopEndpointAssessment { passed, endpoints, whole_arc_contact_verified: false }
}
