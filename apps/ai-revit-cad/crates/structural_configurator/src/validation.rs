use crate::generator::{compute_offsets, preflight_section_station_budget};
use crate::geometry;
use crate::model::*;
use std::collections::{BTreeMap, BTreeSet};

// These are input/resource bounds, not engineering acceptance limits.
const MAX_SECTIONS: usize = 64;
const MAX_CORNERS: usize = 256;
const MAX_STATIONS: usize = 512;
const MAX_INTERFACES: usize = 256;
const MAX_COORD_M: f64 = 1.0e6;

fn report(
    issues: &mut Vec<ValidationIssue>,
    severity: IssueSeverity,
    code: &str,
    path: String,
    message: &str,
) {
    issues.push(ValidationIssue {
        code: code.into(),
        path,
        message: message.into(),
        severity,
    });
}
fn error(issues: &mut Vec<ValidationIssue>, code: &str, path: String, message: &str) {
    report(issues, IssueSeverity::Error, code, path, message);
}
fn limit(issues: &mut Vec<ValidationIssue>, code: &str, path: String, message: &str) {
    report(issues, IssueSeverity::GenerationLimit, code, path, message);
}
fn check_id(issues: &mut Vec<ValidationIssue>, id: &str, path: String) {
    if id.is_empty()
        || id.len() > 512
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.@:/%".contains(&b))
    {
        error(
            issues,
            "INVALID_ID",
            path,
            "ID must match the public SemanticId alphabet and 1..512 byte limit",
        );
    }
}
fn check_finite(issues: &mut Vec<ValidationIssue>, value: f64, path: String) -> bool {
    if value.is_finite() {
        true
    } else {
        error(issues, "NONFINITE_VALUE", path, "Value must be finite");
        false
    }
}
fn check_positive(issues: &mut Vec<ValidationIssue>, value: f64, path: String) -> bool {
    if !check_finite(issues, value, path.clone()) {
        return false;
    }
    if value <= 0.0 || value > 1.0e12 {
        error(
            issues,
            "INVALID_SCALE",
            path,
            "Value must be positive and within the bounded input scale",
        );
        false
    } else {
        true
    }
}
fn check_nonnegative(issues: &mut Vec<ValidationIssue>, value: f64, path: String) -> bool {
    if !check_finite(issues, value, path.clone()) {
        return false;
    }
    if value < 0.0 || value > 1.0e12 {
        error(
            issues,
            "INVALID_SCALE",
            path,
            "Value must be nonnegative and within the bounded input scale",
        );
        false
    } else {
        true
    }
}
fn check_coord(issues: &mut Vec<ValidationIssue>, value: f64, path: String) -> bool {
    if !check_finite(issues, value, path.clone()) {
        return false;
    }
    if value.abs() > MAX_COORD_M {
        error(
            issues,
            "INVALID_SCALE",
            path,
            "Coordinate exceeds the bounded model scale",
        );
        false
    } else {
        true
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: [f64; 2]) -> f64 {
    dot(a, a).sqrt()
}
fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    norm(sub(a, b))
}
fn ring_points(s: &SectionConfig) -> Vec<[f64; 2]> {
    s.corners.iter().map(|c| c.xy_m).collect()
}
fn ring_scale(p: &[[f64; 2]]) -> f64 {
    if p.is_empty() {
        return 1.0;
    }
    let (mut minx, mut maxx, mut miny, mut maxy) = (p[0][0], p[0][0], p[0][1], p[0][1]);
    for q in p {
        minx = minx.min(q[0]);
        maxx = maxx.max(q[0]);
        miny = miny.min(q[1]);
        maxy = maxy.max(q[1]);
    }
    (maxx - minx).hypot(maxy - miny).max(1.0)
}
fn tolerance(p: &[[f64; 2]]) -> f64 {
    1.0e-8 + 1.0e-10 * ring_scale(p)
}
fn area2(p: &[[f64; 2]]) -> f64 {
    if p.len() < 3 {
        return 0.0;
    }
    (0..p.len())
        .map(|i| cross(p[i], p[(i + 1) % p.len()]))
        .sum()
}
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    cross(sub(b, a), sub(c, a))
}
fn on_segment(a: [f64; 2], b: [f64; 2], q: [f64; 2], tol: f64) -> bool {
    orient(a, b, q).abs() <= tol * dist(a, b).max(1.0)
        && q[0] >= a[0].min(b[0]) - tol
        && q[0] <= a[0].max(b[0]) + tol
        && q[1] >= a[1].min(b[1]) - tol
        && q[1] <= a[1].max(b[1]) + tol
}
fn sign(v: f64, tol: f64) -> i8 {
    if v > tol {
        1
    } else if v < -tol {
        -1
    } else {
        0
    }
}
fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], tol: f64) -> bool {
    let t = tol * dist(a, b).max(dist(c, d)).max(1.0);
    let (o1, o2, o3, o4) = (
        sign(orient(a, b, c), t),
        sign(orient(a, b, d), t),
        sign(orient(c, d, a), t),
        sign(orient(c, d, b), t),
    );
    (o1 * o2 < 0 && o3 * o4 < 0)
        || (o1 == 0 && on_segment(a, b, c, tol))
        || (o2 == 0 && on_segment(a, b, d, tol))
        || (o3 == 0 && on_segment(c, d, a, tol))
        || (o4 == 0 && on_segment(c, d, b, tol))
}
fn proper_crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], tol: f64) -> bool {
    let t = tol * dist(a, b).max(dist(c, d)).max(1.0);
    let (o1, o2, o3, o4) = (
        sign(orient(a, b, c), t),
        sign(orient(a, b, d), t),
        sign(orient(c, d, a), t),
        sign(orient(c, d, b), t),
    );
    o1 * o2 < 0 && o3 * o4 < 0
}
fn simple_polygon(p: &[[f64; 2]], tol: f64) -> bool {
    let n = p.len();
    for i in 0..n {
        let a = p[i];
        let b = p[(i + 1) % n];
        let c = p[(i + 2) % n];
        let e1 = sub(b, a);
        let e2 = sub(c, b);
        if cross(e1, e2).abs() <= tol * norm(e1).max(norm(e2)).max(1.0) && dot(e1, e2) < -tol * tol
        {
            return false;
        }
        for j in (i + 1)..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if segments_intersect(a, b, p[j], p[(j + 1) % n], tol) {
                return false;
            }
        }
    }
    true
}
fn point_inside_strict(p: &[[f64; 2]], q: [f64; 2], tol: f64) -> bool {
    let mut inside = false;
    for i in 0..p.len() {
        let a = p[i];
        let b = p[(i + 1) % p.len()];
        if on_segment(a, b, q, tol) {
            return false;
        }
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let x = a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if x > q[0] {
                inside = !inside;
            }
        }
    }
    inside
}
fn polygon_overlap_interior(a: &[[f64; 2]], b: &[[f64; 2]], tol: f64) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    for i in 0..a.len() {
        for j in 0..b.len() {
            if proper_crossing(a[i], a[(i + 1) % a.len()], b[j], b[(j + 1) % b.len()], tol) {
                return true;
            }
        }
    }
    if a.iter().any(|q| point_inside_strict(b, *q, tol))
        || b.iter().any(|q| point_inside_strict(a, *q, tol))
    {
        return true;
    }
    // Identical polygons have no strictly interior vertex or proper crossing.
    for (inside_ring, other) in [(a, b), (b, a)] {
        for i in 0..inside_ring.len() {
            let x = inside_ring[i];
            let y = inside_ring[(i + 1) % inside_ring.len()];
            let edge = sub(y, x);
            let length = norm(edge);
            if length <= tol {
                continue;
            }
            let offset = (1.0e-5 * length).max(10.0 * tol);
            let q = [
                (x[0] + y[0]) * 0.5 - edge[1] * offset / length,
                (x[1] + y[1]) * 0.5 + edge[0] * offset / length,
            ];
            if point_inside_strict(inside_ring, q, tol) && point_inside_strict(other, q, tol) {
                return true;
            }
        }
    }
    false
}
#[derive(Clone)]
struct Station {
    position_m: f64,
}
fn station_list(
    spacing: &GridSpacing,
    span: f64,
    tol: f64,
    path: String,
    issues: &mut Vec<ValidationIssue>,
) -> Option<Vec<Station>> {
    if !span.is_finite() || span <= tol {
        error(
            issues,
            "INVALID_GRID",
            path,
            "Projected section span must be positive",
        );
        return None;
    }
    match spacing {
        GridSpacing::Explicit { stations } if stations.len() > MAX_STATIONS => {
            error(
                issues,
                "RESOURCE_LIMIT",
                path,
                "Explicit grid exceeds the bounded 512-station validation preview",
            );
            return None;
        }
        GridSpacing::Uniform { pitch_m } => {
            if !check_positive(issues, *pitch_m, format!("{path}/pitch_m")) {
                return None;
            }
            if span / *pitch_m >= MAX_STATIONS as f64 {
                error(
                    issues,
                    "RESOURCE_LIMIT",
                    path,
                    "Uniform grid exceeds the bounded 512-station validation preview",
                );
                return None;
            }
        }
        _ => {}
    }
    match geometry::stations(spacing, span) {
        Ok(stations) => {
            if stations.len() > MAX_STATIONS {
                limit(
                    issues,
                    "RESOURCE_LIMIT",
                    path,
                    "Grid exceeds the bounded 512-station structural generation preview",
                );
                None
            } else {
                Some(
                    stations
                        .into_iter()
                        .map(|s| Station {
                            position_m: s.position_m,
                        })
                        .collect(),
                )
            }
        }
        Err(reason) => {
            error(issues, "INVALID_GRID", path, &reason.to_string());
            None
        }
    }
}

struct SectionFacts {
    polygon: Vec<[f64; 2]>,
    tol: f64,
    rectangle: bool,
    v: [f64; 2],
    frame_min: f64,
    frame_stations: Option<Vec<Station>>,
}

fn valid_provenance(issues: &mut Vec<ValidationIssue>, provenance: &Provenance, path: String) {
    match provenance {
        Provenance::Synthetic { note } if note.trim().is_empty() => error(
            issues,
            "INVALID_PROVENANCE",
            path,
            "Synthetic provenance requires a note",
        ),
        Provenance::Manual { reference } if reference.trim().is_empty() => error(
            issues,
            "INVALID_PROVENANCE",
            path,
            "Manual provenance requires a reference",
        ),
        _ => {}
    }
}
fn validate_materials(
    config: &BuildingConfig,
    issues: &mut Vec<ValidationIssue>,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (i, m) in config.materials.iter().enumerate() {
        let p = format!("/materials/{i}");
        check_id(issues, m.id.as_str(), format!("{p}/id"));
        if !ids.insert(m.id.to_string()) {
            error(
                issues,
                "DUPLICATE_ID",
                format!("{p}/id"),
                "Material ID repeated",
            );
        }
        if m.name.trim().is_empty() {
            error(
                issues,
                "INVALID_NAME",
                format!("{p}/name"),
                "Material name is required",
            );
        }
        check_positive(
            issues,
            m.elastic_modulus_kn_m2,
            format!("{p}/elastic_modulus_kn_m2"),
        );
        check_positive(
            issues,
            m.shear_modulus_kn_m2,
            format!("{p}/shear_modulus_kn_m2"),
        );
        check_nonnegative(
            issues,
            m.weight_density_kn_m3,
            format!("{p}/weight_density_kn_m3"),
        );
        valid_provenance(issues, &m.provenance, format!("{p}/provenance"));
    }
    ids
}
fn validate_profiles(
    config: &BuildingConfig,
    issues: &mut Vec<ValidationIssue>,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (i, m) in config.profiles.iter().enumerate() {
        let p = format!("/profiles/{i}");
        check_id(issues, m.id.as_str(), format!("{p}/id"));
        if !ids.insert(m.id.to_string()) {
            error(
                issues,
                "DUPLICATE_ID",
                format!("{p}/id"),
                "Profile ID repeated",
            );
        }
        if m.name.trim().is_empty() {
            error(
                issues,
                "INVALID_NAME",
                format!("{p}/name"),
                "Profile name is required",
            );
        }
        check_positive(issues, m.area_m2, format!("{p}/area_m2"));
        check_positive(issues, m.iy_m4, format!("{p}/iy_m4"));
        check_positive(issues, m.iz_m4, format!("{p}/iz_m4"));
        check_positive(
            issues,
            m.torsional_constant_m4,
            format!("{p}/torsional_constant_m4"),
        );
        valid_provenance(issues, &m.provenance, format!("{p}/provenance"));
        if let Some(v) = &m.visualization {
            let finite = check_positive(issues, v.width_m, format!("{p}/visualization/width_m"))
                & check_positive(issues, v.depth_m, format!("{p}/visualization/depth_m"))
                & check_positive(
                    issues,
                    v.web_thickness_m,
                    format!("{p}/visualization/web_thickness_m"),
                )
                & check_positive(
                    issues,
                    v.flange_thickness_m,
                    format!("{p}/visualization/flange_thickness_m"),
                );
            if finite {
                if v.web_thickness_m >= v.width_m || 2.0 * v.flange_thickness_m >= v.depth_m {
                    error(
                        issues,
                        "INVALID_PROFILE_VISUAL",
                        format!("{p}/visualization"),
                        "Visual web and flanges must fit inside width and depth",
                    );
                } else if matches!(&m.provenance, Provenance::Manual { .. })
                    && m.area_m2.is_finite()
                    && m.area_m2 > 0.0
                {
                    let visual_area = 2.0 * v.width_m * v.flange_thickness_m
                        + (v.depth_m - 2.0 * v.flange_thickness_m) * v.web_thickness_m;
                    if (visual_area - m.area_m2).abs() > (0.05 * m.area_m2).max(1.0e-8) {
                        report(issues, IssueSeverity::Warning, "PROFILE_VISUAL_PROPERTIES_INCONSISTENT", format!("{p}/visualization"), "I-profile visual dimensions differ from the manually supplied area; stiffness and area remain the supplied numeric properties");
                    }
                }
            }
        }
    }
    ids
}
fn validate_member_rules(
    s: &SectionConfig,
    section_index: usize,
    materials: &BTreeSet<String>,
    profiles: &BTreeSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    let rules = [
        ("column", &s.members.column),
        ("rafter", &s.members.rafter),
        ("beam", &s.members.beam),
        ("brace", &s.members.brace),
        ("purlin", &s.members.purlin),
        ("wall_girt", &s.members.wall_girt),
    ];
    for (role, rule) in rules {
        let p = format!("/sections/{section_index}/members/{role}");
        if !materials.contains(rule.material_id.as_str()) {
            error(
                issues,
                "MISSING_REFERENCE",
                format!("{p}/material_id"),
                "Member rule references unknown material",
            );
        }
        if !profiles.contains(rule.profile_id.as_str()) {
            error(
                issues,
                "MISSING_REFERENCE",
                format!("{p}/profile_id"),
                "Member rule references unknown profile",
            );
        }
        if rule.end_releases.is_some() {
            error(
                issues,
                "UNSUPPORTED_RELEASES",
                format!("{p}/end_releases"),
                "End releases are outside this configurator stage",
            );
        }
        if rule
            .local_y_reference
            .iter()
            .any(|x| !x.is_finite() || x.abs() > 1.0e6)
        {
            error(
                issues,
                "INVALID_MEMBER_ORIENTATION",
                format!("{p}/local_y_reference"),
                "Local-Y reference must have finite, bounded components",
            );
        } else {
            let norm2: f64 = rule.local_y_reference.iter().map(|x| x * x).sum();
            if norm2 <= 1.0e-20 {
                error(
                    issues,
                    "INVALID_MEMBER_ORIENTATION",
                    format!("{p}/local_y_reference"),
                    "Local-Y reference must be nonzero",
                );
            } else if role == "column"
                && rule.local_y_reference[0].hypot(rule.local_y_reference[1])
                    <= 1.0e-10 * norm2.sqrt()
            {
                error(
                    issues,
                    "INVALID_MEMBER_ORIENTATION",
                    format!("{p}/local_y_reference"),
                    "Column local-Y reference cannot be parallel to its vertical axis",
                );
            }
        }
    }
}
fn validate_roof(s: &SectionConfig, section_index: usize, issues: &mut Vec<ValidationIssue>) {
    let p = format!("/sections/{section_index}/roof");
    let r = &s.roof;
    let b = check_coord(issues, r.base_abs_m, format!("{p}/base_abs_m"));
    let e = check_coord(issues, r.eave_abs_m, format!("{p}/eave_abs_m"));
    let h = check_coord(issues, r.ridge_abs_m, format!("{p}/ridge_abs_m"));
    if b && e && h {
        if r.eave_abs_m - r.base_abs_m <= 1.0e-8 {
            error(
                issues,
                "INVALID_HEIGHT",
                p.clone(),
                "Eave absolute height must exceed base absolute height",
            );
        }
        match r.kind {
            RoofKind::Flat if (r.ridge_abs_m - r.eave_abs_m).abs() > 1.0e-8 => error(
                issues,
                "INVALID_HEIGHT",
                format!("{p}/ridge_abs_m"),
                "Flat roof ridge absolute height must equal eave absolute height",
            ),
            RoofKind::MonoPitch | RoofKind::Gable if r.ridge_abs_m - r.eave_abs_m <= 1.0e-8 => {
                error(
                    issues,
                    "INVALID_HEIGHT",
                    format!("{p}/ridge_abs_m"),
                    "Sloped roof ridge or high edge must exceed eave absolute height",
                )
            }
            RoofKind::Hip
            | RoofKind::Curved
            | RoofKind::MultiFacet
            | RoofKind::Stepped
            | RoofKind::Custom
                if r.ridge_abs_m < r.eave_abs_m =>
            {
                error(
                    issues,
                    "INVALID_HEIGHT",
                    format!("{p}/ridge_abs_m"),
                    "Roof high point cannot lie below eave absolute height",
                )
            }
            _ => {}
        }
    }
    if let Some(details) = &r.unsupported_details {
        if !details.corner_tops_abs_m.is_empty()
            || !details.break_lines_m.is_empty()
            || !details.ridge_lines_m.is_empty()
            || !details.facets_m.is_empty()
        {
            limit(issues, "UNSUPPORTED_ROOF_DETAILS", format!("{p}/unsupported_details"), "Explicit roof facets, break lines and corner tops are retained but have no stage-one structural generation");
        }
        let corners: BTreeSet<&str> = s.corners.iter().map(|c| c.id.as_str()).collect();
        for (id, z) in &details.corner_tops_abs_m {
            if !corners.contains(id.as_str()) {
                error(
                    issues,
                    "MISSING_REFERENCE",
                    format!("{p}/unsupported_details/corner_tops_abs_m/{id}"),
                    "Roof corner top references an unknown section corner",
                );
            }
            check_coord(
                issues,
                *z,
                format!("{p}/unsupported_details/corner_tops_abs_m/{id}"),
            );
        }
        for (name, lines) in [
            ("break_lines_m", &details.break_lines_m),
            ("ridge_lines_m", &details.ridge_lines_m),
            ("facets_m", &details.facets_m),
        ] {
            for (i, line) in lines.iter().enumerate() {
                let lp = format!("{p}/unsupported_details/{name}/{i}");
                let minimum = if name == "facets_m" { 3 } else { 2 };
                if line.len() < minimum || line.len() > MAX_CORNERS {
                    error(
                        issues,
                        "INVALID_ROOF_DETAILS",
                        lp.clone(),
                        "Roof line/facet has invalid vertex count",
                    );
                }
                for (j, point) in line.iter().enumerate() {
                    for (k, value) in point.iter().enumerate() {
                        check_coord(issues, *value, format!("{lp}/{j}/{k}"));
                    }
                }
            }
        }
    }
}

fn validate_panel(panel: &PanelSpec, path: String, issues: &mut Vec<ValidationIssue>) {
    check_positive(issues, panel.thickness_m, format!("{path}/thickness_m"));
    check_positive(
        issues,
        panel.layout_width_m,
        format!("{path}/layout_width_m"),
    );
    check_positive(
        issues,
        panel.layout_length_m,
        format!("{path}/layout_length_m"),
    );
    if panel.material_label.trim().is_empty() {
        error(
            issues,
            "INVALID_NAME",
            format!("{path}/material_label"),
            "Panel material label is required",
        );
    }
    valid_provenance(issues, &panel.provenance, format!("{path}/provenance"));
    if let Some(mass) = panel.mass_kg_m2 {
        check_nonnegative(issues, mass, format!("{path}/mass_kg_m2"));
    }
    if panel.stiffness_enabled {
        error(
            issues,
            "UNSUPPORTED_PANEL_STIFFNESS",
            format!("{path}/stiffness_enabled"),
            "Stage-one panels do not contribute structural stiffness",
        );
    }
    for (i, feature) in panel.requested_features.iter().enumerate() {
        report(issues, IssueSeverity::Warning, "PANEL_FEATURE_NOT_VERIFIED", format!("{path}/requested_features/{i}"), &format!("Requested panel feature {feature:?} is metadata only; no geometry, support or load transfer is generated"));
    }
}

/// Validate the editable input without generating, merging or analyzing structural members.
/// GenerationLimit issues preserve an outline while preventing unsupported structural output.
pub fn validate_config(config: &BuildingConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    check_id(&mut issues, config.id.as_str(), "/id".into());
    for (i, value) in config.transform.translation_m.iter().enumerate() {
        check_coord(&mut issues, *value, format!("/transform/translation_m/{i}"));
    }
    check_finite(
        &mut issues,
        config.transform.rotation_rad,
        "/transform/rotation_rad".into(),
    );
    if config.sections.is_empty() {
        error(
            &mut issues,
            "INVALID_CONFIGURATION",
            "/sections".into(),
            "At least one section is required",
        );
    }
    if config.sections.len() > MAX_SECTIONS {
        error(
            &mut issues,
            "RESOURCE_LIMIT",
            "/sections".into(),
            "Section count exceeds 64",
        );
    }
    if config.interfaces.len() > MAX_INTERFACES {
        error(
            &mut issues,
            "RESOURCE_LIMIT",
            "/interfaces".into(),
            "Interface count exceeds 256",
        );
    }
    let material_ids = validate_materials(config, &mut issues);
    let profile_ids = validate_profiles(config, &mut issues);
    let mut section_ids = BTreeSet::new();
    let mut facts: BTreeMap<String, SectionFacts> = BTreeMap::new();
    for (i, s) in config.sections.iter().enumerate() {
        let p = format!("/sections/{i}");
        check_id(&mut issues, s.id.as_str(), format!("{p}/id"));
        if !section_ids.insert(s.id.to_string()) {
            error(
                &mut issues,
                "DUPLICATE_ID",
                format!("{p}/id"),
                "Section ID repeated",
            );
        }
        if s.structural_system != StructuralSystem::SteelFrame {
            limit(&mut issues, "UNSUPPORTED_STRUCTURAL_SYSTEM", format!("{p}/structural_system"), "Only STEEL_FRAME has stage-one structural generation; section outline remains available");
        }
        validate_roof(s, i, &mut issues);
        if let Some(panel) = &s.roof_panels {
            validate_panel(panel, format!("{p}/roof_panels"), &mut issues);
        }
        if let Some(panel) = &s.wall_panels {
            validate_panel(panel, format!("{p}/wall_panels"), &mut issues);
        }
        validate_member_rules(s, i, &material_ids, &profile_ids, &mut issues);
        if s.corners.len() < 3 {
            error(
                &mut issues,
                "INVALID_POLYGON",
                format!("{p}/corners"),
                "Section needs at least three unclosed corners",
            );
            continue;
        }
        if s.corners.len() > MAX_CORNERS {
            error(
                &mut issues,
                "RESOURCE_LIMIT",
                format!("{p}/corners"),
                "Section outline exceeds 256 corners",
            );
            continue;
        }
        let mut corner_ids = BTreeSet::new();
        let mut all_finite = true;
        for (j, corner) in s.corners.iter().enumerate() {
            let cp = format!("{p}/corners/{j}");
            check_id(&mut issues, corner.id.as_str(), format!("{cp}/id"));
            if !corner_ids.insert(corner.id.to_string()) {
                error(
                    &mut issues,
                    "DUPLICATE_ID",
                    format!("{cp}/id"),
                    "Corner ID repeated within section",
                );
            }
            for k in 0..2 {
                all_finite &= check_coord(&mut issues, corner.xy_m[k], format!("{cp}/xy_m/{k}"));
            }
        }
        if !all_finite {
            continue;
        }
        let polygon = ring_points(s);
        let tol = tolerance(&polygon);
        for j in 0..polygon.len() {
            if dist(polygon[j], polygon[(j + 1) % polygon.len()]) <= tol {
                error(
                    &mut issues,
                    "ZERO_EDGE",
                    format!("{p}/corners/{j}"),
                    "Consecutive corners form a zero-length edge",
                );
            }
            for k in (j + 1)..polygon.len() {
                if dist(polygon[j], polygon[k]) <= tol {
                    error(
                        &mut issues,
                        "DUPLICATE_CORNER",
                        format!("{p}/corners/{k}"),
                        "Corner coordinates repeat within geometric tolerance",
                    );
                }
            }
        }
        let twice_area = area2(&polygon);
        if twice_area <= tol * ring_scale(&polygon) {
            error(
                &mut issues,
                "INVALID_WINDING",
                format!("{p}/corners"),
                "Section ring must have positive CCW area above geometric tolerance",
            );
        }
        if !simple_polygon(&polygon, tol) {
            error(
                &mut issues,
                "SELF_INTERSECTION",
                format!("{p}/corners"),
                "Section ring intersects itself or backtracks",
            );
        }
        let u = s.grid.span_direction;
        let mut valid_direction = true;
        for k in 0..2 {
            valid_direction &=
                check_finite(&mut issues, u[k], format!("{p}/grid/span_direction/{k}"));
        }
        if valid_direction && (norm(u) - 1.0).abs() > 1.0e-8 {
            error(
                &mut issues,
                "INVALID_GRID",
                format!("{p}/grid/span_direction"),
                "Span direction must be a unit vector",
            );
            valid_direction = false;
        }
        check_positive(
            &mut issues,
            s.grid.purlin_pitch_m,
            format!("{p}/grid/purlin_pitch_m"),
        );
        check_positive(
            &mut issues,
            s.grid.wall_girt_pitch_m,
            format!("{p}/grid/wall_girt_pitch_m"),
        );
        if !valid_direction {
            continue;
        }
        let bounds = geometry::bounds(s);
        let rectangle = geometry::is_rectangle(s, &bounds);
        let l_shape = geometry::is_orthogonal_l(s, &bounds);
        let details = s.roof.unsupported_details.as_ref().is_some_and(|d| {
            !d.corner_tops_abs_m.is_empty()
                || !d.break_lines_m.is_empty()
                || !d.ridge_lines_m.is_empty()
                || !d.facets_m.is_empty()
        });
        let supported = s.structural_system == StructuralSystem::SteelFrame
            && !details
            && matches!(
                s.roof.kind,
                RoofKind::Flat | RoofKind::MonoPitch | RoofKind::Gable
            )
            && (rectangle || (l_shape && s.roof.kind == RoofKind::Flat));
        if supported {
            if let Err(reason) = preflight_section_station_budget(s) {
                error(
                    &mut issues,
                    "RESOURCE_LIMIT",
                    format!("{p}/grid"),
                    &reason.to_string(),
                );
                continue;
            }
        }
        let v = [-u[1], u[0]];
        let (u_min, u_max) = polygon
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                (lo.min(dot(*q, u)), hi.max(dot(*q, u)))
            });
        let (v_min, v_max) = polygon
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                (lo.min(dot(*q, v)), hi.max(dot(*q, v)))
            });
        let columns = station_list(
            &s.grid.column_stations,
            u_max - u_min,
            tol,
            format!("{p}/grid/column_stations"),
            &mut issues,
        );
        let frames = station_list(
            &s.grid.frame_stations,
            v_max - v_min,
            tol,
            format!("{p}/grid/frame_stations"),
            &mut issues,
        );
        if let (Some(a), Some(b)) = (&columns, &frames) {
            if a.len().saturating_mul(b.len()) > 20_000 {
                limit(&mut issues, "RESOURCE_LIMIT", format!("{p}/grid"), "Grid expansion exceeds the bounded structural-generation preview; outline remains available");
            }
        }
        if matches!(
            s.roof.kind,
            RoofKind::Hip
                | RoofKind::Curved
                | RoofKind::MultiFacet
                | RoofKind::Stepped
                | RoofKind::Custom
        ) {
            limit(&mut issues, "UNSUPPORTED_ROOF", format!("{p}/roof/kind"), "Roof kind is retained for outline display but has no stage-one structural generation");
        } else if !(rectangle || (l_shape && s.roof.kind == RoofKind::Flat)) {
            limit(&mut issues, "UNSUPPORTED_GENERATION", format!("{p}/corners"), "Only grid-aligned rectangles and flat orthogonal L sections have structural generation; arbitrary polygon outline remains available");
        }
        facts.insert(
            s.id.to_string(),
            SectionFacts {
                polygon,
                tol,
                rectangle,
                v,
                frame_min: v_min,
                frame_stations: frames,
            },
        );
    }
    validate_interfaces(config, &facts, &mut issues);
    if let Ok(pairs) = undeclared_contact_pairs(config) {
        for (a, b) in pairs {
            report(&mut issues, IssueSeverity::Warning, "INTERFACE_LOAD_PATH_NOT_VERIFIED", "/interfaces".into(),
                &format!("Sections {a} and {b} touch without an explicit interface; coincidence establishes no structural load path"));
        }
    }
    validate_assignments(config, &section_ids, &mut issues);
    issues
}

/// Boundary contacts without an explicit interface, after actual joint offsets.
/// This uses inclusive segment contact, so a shared edge or a touching corner
/// is reported. Interior overlap is a separate fatal polygon validation issue.
pub(crate) fn undeclared_contact_pairs(
    config: &BuildingConfig,
) -> Result<Vec<(SemanticId, SemanticId)>, ConfiguratorError> {
    let offsets = compute_offsets(config)?;
    let mut declared = BTreeSet::new();
    for i in &config.interfaces {
        let pair = if i.section_a < i.section_b {
            (i.section_a.clone(), i.section_b.clone())
        } else {
            (i.section_b.clone(), i.section_a.clone())
        };
        declared.insert(pair);
    }
    let mut found = BTreeSet::new();
    for (i, a) in config.sections.iter().enumerate() {
        let ap = ring_points(a);
        if ap.len() < 3 || ap.iter().flatten().any(|x| !x.is_finite()) {
            continue;
        }
        let at = tolerance(&ap);
        if area2(&ap) <= at * ring_scale(&ap) || !simple_polygon(&ap, at) {
            continue;
        }
        let da = offsets.get(&a.id).copied().unwrap_or([0.0; 2]);
        let ap: Vec<_> = ap.iter().map(|p| [p[0] + da[0], p[1] + da[1]]).collect();
        for b in config.sections.iter().skip(i + 1) {
            let pair = if a.id < b.id {
                (a.id.clone(), b.id.clone())
            } else {
                (b.id.clone(), a.id.clone())
            };
            if declared.contains(&pair) {
                continue;
            }
            let bp = ring_points(b);
            if bp.len() < 3 || bp.iter().flatten().any(|x| !x.is_finite()) {
                continue;
            }
            let bt = tolerance(&bp);
            if area2(&bp) <= bt * ring_scale(&bp) || !simple_polygon(&bp, bt) {
                continue;
            }
            let db = offsets.get(&b.id).copied().unwrap_or([0.0; 2]);
            let bp: Vec<_> = bp.iter().map(|p| [p[0] + db[0], p[1] + db[1]]).collect();
            let tol = at.max(bt);
            if polygon_overlap_interior(&ap, &bp, tol) {
                continue;
            }
            let contact = (0..ap.len()).any(|ai| {
                (0..bp.len()).any(|bi| {
                    segments_intersect(
                        ap[ai],
                        ap[(ai + 1) % ap.len()],
                        bp[bi],
                        bp[(bi + 1) % bp.len()],
                        tol,
                    )
                })
            });
            if contact {
                found.insert(pair);
            }
        }
    }
    Ok(found.into_iter().collect())
}

fn edge_points(
    section: &SectionConfig,
    edge: &[SemanticId; 2],
    path: String,
    issues: &mut Vec<ValidationIssue>,
) -> Option<([f64; 2], [f64; 2])> {
    if edge[0].as_str() == edge[1].as_str() {
        error(
            issues,
            "INTERFACE_CONFLICT",
            path,
            "Interface edge requires distinct corner IDs",
        );
        return None;
    }
    if section.corners.is_empty() {
        return None;
    }
    let Some(i) = section
        .corners
        .iter()
        .position(|c| c.id.as_str() == edge[0].as_str())
    else {
        error(
            issues,
            "MISSING_REFERENCE",
            path.clone(),
            "Interface start corner is missing from section",
        );
        return None;
    };
    let next = (i + 1) % section.corners.len();
    if section.corners[next].id.as_str() != edge[1].as_str() {
        error(
            issues,
            "INTERFACE_CONFLICT",
            path,
            "Interface edge IDs must be adjacent in the section's CCW declared order",
        );
        return None;
    }
    Some((section.corners[i].xy_m, section.corners[next].xy_m))
}
fn same_reference(a: [f64; 3], b: [f64; 3]) -> bool {
    if a.iter().chain(b.iter()).any(|x| !x.is_finite()) {
        return false;
    }
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    na > 1.0e-10 && nb > 1.0e-10 && (0..3).all(|i| (a[i] / na - b[i] / nb).abs() <= 1.0e-8)
}
fn connected_stations_match(
    a: &SectionFacts,
    b: &SectionFacts,
    edge_a: ([f64; 2], [f64; 2]),
    anchors: &[InterfaceAnchor],
) -> bool {
    let (Some(a_stations), Some(b_stations)) = (&a.frame_stations, &b.frame_stations) else {
        return false;
    };
    if a_stations.len() != b_stations.len() || a_stations.len() != anchors.len() {
        return false;
    }
    let alignment = dot(a.v, b.v);
    if (alignment.abs() - 1.0).abs() > 1.0e-8 {
        return false;
    }
    let mut a_positions: Vec<f64> = a_stations
        .iter()
        .map(|s| a.frame_min + s.position_m)
        .collect();
    let mut b_positions: Vec<f64> = b_stations
        .iter()
        .map(|s| alignment * (b.frame_min + s.position_m))
        .collect();
    a_positions.sort_by(f64::total_cmp);
    b_positions.sort_by(f64::total_cmp);
    let tol = a.tol.max(b.tol);
    if a_positions
        .iter()
        .zip(&b_positions)
        .any(|(x, y)| (x - y).abs() > tol)
    {
        return false;
    }
    let begin = dot(edge_a.0, a.v);
    let end = dot(edge_a.1, a.v);
    if (end - begin).abs() <= tol {
        return false;
    }
    let mut anchor_positions: Vec<f64> = anchors
        .iter()
        .map(|anchor| begin + anchor.fraction * (end - begin))
        .collect();
    anchor_positions.sort_by(f64::total_cmp);
    a_positions
        .iter()
        .zip(&anchor_positions)
        .all(|(x, y)| (x - y).abs() <= tol)
}
fn validate_interfaces(
    config: &BuildingConfig,
    facts: &BTreeMap<String, SectionFacts>,
    issues: &mut Vec<ValidationIssue>,
) {
    let sections: BTreeMap<&str, &SectionConfig> =
        config.sections.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut ids = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    let mut joint_offsets: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    for (i, interface) in config.interfaces.iter().enumerate() {
        let p = format!("/interfaces/{i}");
        check_id(issues, interface.id.as_str(), format!("{p}/id"));
        if !ids.insert(interface.id.to_string()) {
            error(
                issues,
                "DUPLICATE_ID",
                format!("{p}/id"),
                "Interface ID repeated",
            );
        }
        if interface.section_a == interface.section_b {
            error(
                issues,
                "INTERFACE_CONFLICT",
                p.clone(),
                "An interface cannot join a section to itself",
            );
            continue;
        }
        let a_name = interface.section_a.to_string();
        let b_name = interface.section_b.to_string();
        let pair = if a_name < b_name {
            (a_name.clone(), b_name.clone())
        } else {
            (b_name.clone(), a_name.clone())
        };
        if !pairs.insert(pair) {
            error(
                issues,
                "INTERFACE_CONFLICT",
                p.clone(),
                "Section pair has duplicate or conflicting interfaces",
            );
        }
        let Some(a_section) = sections.get(interface.section_a.as_str()) else {
            error(
                issues,
                "MISSING_REFERENCE",
                format!("{p}/section_a"),
                "Interface section A is missing",
            );
            continue;
        };
        let Some(b_section) = sections.get(interface.section_b.as_str()) else {
            error(
                issues,
                "MISSING_REFERENCE",
                format!("{p}/section_b"),
                "Interface section B is missing",
            );
            continue;
        };
        let ea = edge_points(a_section, &interface.edge_a, format!("{p}/edge_a"), issues);
        let eb = edge_points(b_section, &interface.edge_b, format!("{p}/edge_b"), issues);
        let (Some(ea), Some(eb)) = (ea, eb) else {
            continue;
        };
        let (Some(fa), Some(fb)) = (facts.get(&a_name), facts.get(&b_name)) else {
            continue;
        };
        let tol = fa.tol.max(fb.tol);
        if dist(ea.0, eb.1) > tol || dist(ea.1, eb.0) > tol {
            error(issues, "INTERFACE_CONFLICT", p.clone(), "Declared interface edges must describe the same full boundary in opposite CCW directions before any joint offset");
            continue;
        }
        let mut anchor_ids = BTreeSet::new();
        let mut fractions = Vec::new();
        for (j, anchor) in interface.anchors.iter().enumerate() {
            let ap = format!("{p}/anchors/{j}");
            check_id(issues, anchor.id.as_str(), format!("{ap}/id"));
            if !anchor_ids.insert(anchor.id.to_string()) {
                error(
                    issues,
                    "DUPLICATE_ID",
                    format!("{ap}/id"),
                    "Anchor ID repeated within interface",
                );
            }
            if check_finite(issues, anchor.fraction, format!("{ap}/fraction")) {
                if anchor.fraction < -1.0e-10 || anchor.fraction > 1.0 + 1.0e-10 {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        format!("{ap}/fraction"),
                        "Anchor fraction must lie from 0 to 1 along edge A",
                    );
                }
                fractions.push(anchor.fraction);
            }
        }
        fractions.sort_by(f64::total_cmp);
        if fractions
            .windows(2)
            .any(|pair| pair[1] - pair[0] <= 1.0e-10)
        {
            error(
                issues,
                "INTERFACE_CONFLICT",
                format!("{p}/anchors"),
                "Anchor fractions must identify distinct physical stations",
            );
        }
        match &interface.kind {
            InterfaceKind::StructurallyConnected { .. } => {
                let edge = sub(ea.1, ea.0);
                let length = norm(edge);
                if !fa.rectangle
                    || !fb.rectangle
                    || length <= tol
                    || cross(edge, fa.v).abs() > tol * length
                    || (dot(fa.v, fb.v).abs() - 1.0).abs() > 1.0e-8
                {
                    error(issues, "INTERFACE_CONFLICT", p.clone(), "Connected generation supports full shared edges parallel to both rectangular sections' frame direction only");
                }
                if (a_section.roof.base_abs_m - b_section.roof.base_abs_m).abs() > tol {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        p.clone(),
                        "Connected sections require equal absolute base heights",
                    );
                }
                let ac = &a_section.members.column;
                let bc = &b_section.members.column;
                if ac.material_id != bc.material_id
                    || ac.profile_id != bc.profile_id
                    || !same_reference(ac.local_y_reference, bc.local_y_reference)
                    || a_section.base_restraints != b_section.base_restraints
                {
                    error(issues, "INTERFACE_CONFLICT", p.clone(), "Shared columns require compatible material, profile, local orientation and base restraints");
                }
                if !connected_stations_match(fa, fb, ea, &interface.anchors) {
                    error(issues, "INTERFACE_CONFLICT", format!("{p}/anchors"), "Every shared frame/column position must match in both grids and have one explicit anchor; anchor IDs are independent join identities");
                }
            }
            InterfaceKind::IndependentWithJoint {
                width_m,
                offset_b_direction,
            } => {
                if !check_positive(issues, *width_m, format!("{p}/kind/width_m")) || *width_m <= tol
                {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        format!("{p}/kind/width_m"),
                        "Joint width must exceed geometric tolerance",
                    );
                    continue;
                }
                if offset_b_direction.iter().any(|x| !x.is_finite()) {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        format!("{p}/kind/offset_b_direction"),
                        "Joint direction must be finite",
                    );
                    continue;
                }
                let edge = sub(ea.1, ea.0);
                let len = norm(edge);
                let dir = *offset_b_direction;
                if len <= tol
                    || (norm(dir) - 1.0).abs() > 1.0e-8
                    || dot(dir, edge).abs() > 1.0e-8 * len
                    || cross(edge, dir) >= -1.0e-8 * len
                {
                    error(issues, "INTERFACE_CONFLICT", format!("{p}/kind/offset_b_direction"), "Joint translation must be unit, normal to edge A and point outside section A");
                    continue;
                }
                let translation = [dir[0] * *width_m, dir[1] * *width_m];
                if joint_offsets.insert(b_name, translation).is_some() {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        p.clone(),
                        "Section B has duplicate or conflicting joint translations",
                    );
                }
            }
            InterfaceKind::GeometryOnlyContact | InterfaceKind::NotDefined => {
                if !interface.anchors.is_empty() {
                    report(issues, IssueSeverity::Warning, "UNUSED_INTERFACE_ANCHORS", format!("{p}/anchors"), "Unconnected contact retains anchors as metadata; they do not establish a load path");
                }
            }
        }
    }
    // The generator resolves relative joint constraints across the interface
    // graph. Reuse that pure resolution so chains are admitted and cycles fail.
    let resolved_offsets = match compute_offsets(config) {
        Ok(offsets) => offsets,
        Err(reason) => {
            error(
                issues,
                "INTERFACE_CONFLICT",
                "/interfaces".into(),
                &reason.to_string(),
            );
            BTreeMap::new()
        }
    };
    for (i, interface) in config.interfaces.iter().enumerate() {
        let a = resolved_offsets
            .get(interface.section_a.as_str())
            .copied()
            .unwrap_or([0.0; 2]);
        let b = resolved_offsets
            .get(interface.section_b.as_str())
            .copied()
            .unwrap_or([0.0; 2]);
        let relative = sub(b, a);
        let tol = facts
            .get(interface.section_a.as_str())
            .map(|f| f.tol)
            .unwrap_or(1.0e-8)
            .max(
                facts
                    .get(interface.section_b.as_str())
                    .map(|f| f.tol)
                    .unwrap_or(1.0e-8),
            );
        match &interface.kind {
            InterfaceKind::IndependentWithJoint {
                width_m,
                offset_b_direction,
            } if width_m.is_finite() && offset_b_direction.iter().all(|x| x.is_finite()) => {
                let intended = [
                    *width_m * offset_b_direction[0],
                    *width_m * offset_b_direction[1],
                ];
                if dist(relative, intended) > tol {
                    error(issues, "INTERFACE_CONFLICT", format!("/interfaces/{i}/kind"), "Accumulated section offsets do not produce the declared physical joint width and direction");
                }
            }
            InterfaceKind::StructurallyConnected { .. } if norm(relative) > tol => {
                error(
                    issues,
                    "INTERFACE_CONFLICT",
                    format!("/interfaces/{i}/kind"),
                    "Structurally connected sections have different joint translations",
                );
            }
            _ => {}
        }
    }
    let entries: Vec<(&String, &SectionFacts)> = facts.iter().collect();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let (a_id, a) = entries[i];
            let (b_id, b) = entries[j];
            if area2(&a.polygon) <= a.tol * ring_scale(&a.polygon)
                || area2(&b.polygon) <= b.tol * ring_scale(&b.polygon)
                || !simple_polygon(&a.polygon, a.tol)
                || !simple_polygon(&b.polygon, b.tol)
            {
                continue;
            }
            let tol = a.tol.max(b.tol);
            if polygon_overlap_interior(&a.polygon, &b.polygon, tol) {
                error(
                    issues,
                    "OVERLAPPING_SECTIONS",
                    "/sections".into(),
                    &format!("Sections {a_id} and {b_id} overlap in plan before joint translation"),
                );
            }
            let ao = resolved_offsets
                .get(a_id.as_str())
                .copied()
                .unwrap_or([0.0; 2]);
            let bo = resolved_offsets
                .get(b_id.as_str())
                .copied()
                .unwrap_or([0.0; 2]);
            if ao != [0.0; 2] || bo != [0.0; 2] {
                let ap: Vec<_> = a
                    .polygon
                    .iter()
                    .map(|q| [q[0] + ao[0], q[1] + ao[1]])
                    .collect();
                let bp: Vec<_> = b
                    .polygon
                    .iter()
                    .map(|q| [q[0] + bo[0], q[1] + bo[1]])
                    .collect();
                if polygon_overlap_interior(&ap, &bp, tol) {
                    error(
                        issues,
                        "INTERFACE_CONFLICT",
                        "/interfaces".into(),
                        &format!(
                            "Sections {a_id} and {b_id} overlap after declared joint translations"
                        ),
                    );
                }
            }
        }
    }
}
fn validate_assignments(
    config: &BuildingConfig,
    section_ids: &BTreeSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut assignments: BTreeMap<(String, String), String> = BTreeMap::new();
    for (i, a) in config.foundation_assignments.iter().enumerate() {
        let p = format!("/foundation_assignments/{i}");
        check_id(issues, a.section_id.as_str(), format!("{p}/section_id"));
        check_id(
            issues,
            a.support_node_id.as_str(),
            format!("{p}/support_node_id"),
        );
        check_id(
            issues,
            a.foundation_id.as_str(),
            format!("{p}/foundation_id"),
        );
        if !section_ids.contains(a.section_id.as_str()) {
            error(
                issues,
                "MISSING_REFERENCE",
                format!("{p}/section_id"),
                "Foundation assignment references unknown section",
            );
        }
        let key = (a.section_id.to_string(), a.support_node_id.to_string());
        if assignments
            .insert(key, a.foundation_id.to_string())
            .is_some()
        {
            error(
                issues,
                "DUPLICATE_ASSIGNMENT",
                p,
                "Support node has duplicate or conflicting foundation assignments within section",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(value: &str) -> SemanticId {
        SemanticId::new(value).expect("valid test ID")
    }
    fn rule(reference: [f64; 3]) -> MemberRule {
        MemberRule {
            material_id: sid("steel"),
            profile_id: sid("profile"),
            local_y_reference: reference,
            end_releases: None,
        }
    }
    fn rectangle(id: &str, x0: f64, x1: f64) -> SectionConfig {
        SectionConfig {
            id: sid(id),
            structural_system: StructuralSystem::SteelFrame,
            corners: vec![
                Corner {
                    id: sid("a"),
                    xy_m: [x0, 0.0],
                },
                Corner {
                    id: sid("b"),
                    xy_m: [x1, 0.0],
                },
                Corner {
                    id: sid("c"),
                    xy_m: [x1, 3.0],
                },
                Corner {
                    id: sid("d"),
                    xy_m: [x0, 3.0],
                },
            ],
            roof: RoofSpec {
                kind: RoofKind::Flat,
                base_abs_m: 0.0,
                eave_abs_m: 3.0,
                ridge_abs_m: 3.0,
                unsupported_details: None,
            },
            grid: SectionGrid {
                span_direction: [1.0, 0.0],
                frame_stations: GridSpacing::Uniform { pitch_m: 1.0 },
                column_stations: GridSpacing::Uniform { pitch_m: 2.0 },
                purlin_pitch_m: 1.0,
                wall_girt_pitch_m: 1.0,
                braces: false,
            },
            members: MemberRules {
                column: rule([1.0, 0.0, 0.0]),
                rafter: rule([0.0, 1.0, 0.0]),
                beam: rule([0.0, 1.0, 0.0]),
                brace: rule([0.0, 1.0, 0.0]),
                purlin: rule([0.0, 0.0, 1.0]),
                wall_girt: rule([0.0, 0.0, 1.0]),
            },
            base_restraints: [true; 6],
            roof_panels: None,
            wall_panels: None,
        }
    }
    fn config() -> BuildingConfig {
        BuildingConfig {
            id: sid("building"),
            transform: LocalTransform::default(),
            sections: vec![rectangle("one", 0.0, 4.0)],
            interfaces: vec![],
            materials: vec![SteelMaterial {
                id: sid("steel"),
                name: "Synthetic steel".into(),
                elastic_modulus_kn_m2: 210_000_000.0,
                shear_modulus_kn_m2: 80_000_000.0,
                weight_density_kn_m3: 77.0,
                provenance: Provenance::Synthetic {
                    note: "test".into(),
                },
            }],
            profiles: vec![MemberProfile {
                id: sid("profile"),
                name: "Synthetic section".into(),
                area_m2: 0.01,
                iy_m4: 4.0e-6,
                iz_m4: 8.0e-6,
                torsional_constant_m4: 6.0e-6,
                provenance: Provenance::Synthetic {
                    note: "test".into(),
                },
                visualization: None,
            }],
            foundation_assignments: vec![],
            allow_disconnected_analysis: false,
            analysis_load_scope: AnalysisLoadScope::FrameSelfWeightOnly,
        }
    }
    fn has(issues: &[ValidationIssue], code: &str) -> bool {
        issues.iter().any(|issue| issue.code == code)
    }
    fn shared_interface() -> InterfaceSpec {
        InterfaceSpec {
            id: sid("shared"),
            section_a: sid("one"),
            edge_a: [sid("b"), sid("c")],
            section_b: sid("two"),
            edge_b: [sid("d"), sid("a")],
            kind: InterfaceKind::StructurallyConnected {
                idealization: ConnectionIdealization::Rigid,
            },
            anchors: (0..=3)
                .map(|i| InterfaceAnchor {
                    id: sid(&format!("join{i}")),
                    fraction: i as f64 / 3.0,
                })
                .collect(),
        }
    }

    #[test]
    fn valid_rectangle_has_no_issues() {
        assert!(validate_config(&config()).is_empty());
    }
    #[test]
    fn refuses_duplicate_corner_and_self_intersection() {
        let mut duplicate = config();
        duplicate.sections[0].corners[2].xy_m = duplicate.sections[0].corners[1].xy_m;
        assert!(has(&validate_config(&duplicate), "ZERO_EDGE"));
        let mut bow = config();
        bow.sections[0].corners[1].xy_m = [4.0, 3.0];
        bow.sections[0].corners[2].xy_m = [4.0, 0.0];
        assert!(has(&validate_config(&bow), "SELF_INTERSECTION"));
    }
    #[test]
    fn requires_ccw_and_valid_height() {
        let mut reversed = config();
        reversed.sections[0].corners.reverse();
        assert!(has(&validate_config(&reversed), "INVALID_WINDING"));
        let mut low = config();
        low.sections[0].roof.eave_abs_m = 0.0;
        assert!(has(&validate_config(&low), "INVALID_HEIGHT"));
    }
    #[test]
    fn detects_plan_overlap_without_joining_coincident_edges() {
        let mut c = config();
        c.sections.push(rectangle("two", 2.0, 6.0));
        assert!(has(&validate_config(&c), "OVERLAPPING_SECTIONS"));
        c.sections[1] = rectangle("two", 4.0, 8.0);
        assert!(!has(&validate_config(&c), "OVERLAPPING_SECTIONS"));
        assert!(has(
            &validate_config(&c),
            "INTERFACE_LOAD_PATH_NOT_VERIFIED"
        ));
        assert_eq!(
            undeclared_contact_pairs(&c).unwrap(),
            vec![(sid("one"), sid("two"))]
        );
    }
    #[test]
    fn connected_interface_requires_complete_physical_station_anchors() {
        let mut c = config();
        c.sections.push(rectangle("two", 4.0, 8.0));
        c.interfaces.push(shared_interface());
        assert!(!has(&validate_config(&c), "INTERFACE_CONFLICT"));
        c.interfaces[0].anchors.reverse();
        assert!(!has(&validate_config(&c), "INTERFACE_CONFLICT"));
        c.interfaces[0].anchors.remove(1);
        assert!(has(&validate_config(&c), "INTERFACE_CONFLICT"));
    }
    #[test]
    fn joint_direction_must_point_outside_first_section() {
        let mut c = config();
        c.sections.push(rectangle("two", 4.0, 8.0));
        let mut interface = shared_interface();
        interface.anchors.clear();
        interface.kind = InterfaceKind::IndependentWithJoint {
            width_m: 0.05,
            offset_b_direction: [-1.0, 0.0],
        };
        c.interfaces.push(interface);
        assert!(has(&validate_config(&c), "INTERFACE_CONFLICT"));
    }
    #[test]
    fn chained_joints_resolve_each_declared_physical_gap() {
        let mut c = config();
        c.sections.push(rectangle("two", 4.0, 8.0));
        c.sections.push(rectangle("three", 8.0, 12.0));
        let mut first = shared_interface();
        first.anchors.clear();
        first.kind = InterfaceKind::IndependentWithJoint {
            width_m: 0.05,
            offset_b_direction: [1.0, 0.0],
        };
        let mut second = first.clone();
        second.id = sid("joint23");
        second.section_a = sid("two");
        second.section_b = sid("three");
        c.interfaces = vec![first, second];
        assert!(!has(&validate_config(&c), "INTERFACE_CONFLICT"));
    }
    #[test]
    fn arbitrary_outline_and_unsupported_roof_keep_typed_generation_limit() {
        let mut c = config();
        c.sections[0].roof.kind = RoofKind::Custom;
        assert!(validate_config(&c)
            .iter()
            .any(|issue| issue.code == "UNSUPPORTED_ROOF"
                && issue.severity == IssueSeverity::GenerationLimit));
        c.sections[0].roof.kind = RoofKind::Flat;
        c.sections[0].corners.insert(
            2,
            Corner {
                id: sid("extra"),
                xy_m: [4.0, 1.0],
            },
        );
        assert!(validate_config(&c)
            .iter()
            .any(|issue| issue.code == "UNSUPPORTED_GENERATION"
                && issue.severity == IssueSeverity::GenerationLimit));
    }
    #[test]
    fn panel_feature_is_diagnostic_and_unknown_mass_stays_optional() {
        let mut c = config();
        c.sections[0].roof_panels = Some(PanelSpec {
            thickness_m: 0.08,
            layout_width_m: 1.0,
            layout_length_m: 3.0,
            material_label: "synthetic panel".into(),
            provenance: Provenance::Synthetic {
                note: "test".into(),
            },
            mass_kg_m2: None,
            stiffness_enabled: false,
            load_transfer_status: PanelLoadTransferStatus::NotVerified,
            requested_features: vec![PanelFeature::Openings],
        });
        let issues = validate_config(&c);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "PANEL_FEATURE_NOT_VERIFIED"
                && issue.severity == IssueSeverity::Warning));
        assert!(!issues
            .iter()
            .any(|issue| issue.severity == IssueSeverity::Error));
    }
}
