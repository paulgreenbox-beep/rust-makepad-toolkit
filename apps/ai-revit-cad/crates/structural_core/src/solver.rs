use crate::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

type M12 = [[f64; 12]; 12];
const RESIDUAL_TOLERANCE: f64 = 1.0e-8;
const MIN_EQUILIBRATED_PIVOT: f64 = 1.0e-12;
pub const SOLVER_VERSION: &str = "structural-core-stage1-linear-v1";

fn invalid(message: impl Into<String>) -> StructuralError {
    StructuralError::InvalidModel {
        message: message.into(),
    }
}
fn unsupported(message: impl Into<String>) -> StructuralError {
    StructuralError::Unsupported {
        message: message.into(),
    }
}
fn numeric(message: impl Into<String>) -> StructuralError {
    StructuralError::NumericalFailure {
        message: message.into(),
    }
}
fn finite(values: &[f64], context: &str) -> Result<(), StructuralError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(invalid(format!("{context}: non-finite value")))
    }
}
fn bounded(entity: &str, count: usize, limit: usize) -> Result<(), StructuralError> {
    if count > limit {
        Err(StructuralError::CapacityExceeded {
            entity: entity.into(),
            count,
            limit,
        })
    } else {
        Ok(())
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn norm(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn active_components(space: StructuralSpace) -> &'static [usize] {
    match space {
        StructuralSpace::Truss2dXz => &[0, 2],
        StructuralSpace::Frame2dXz => &[0, 2, 4],
        StructuralSpace::Frame3d => &[0, 1, 2, 3, 4, 5],
    }
}
fn require_text(value: &str, context: &str) -> Result<(), StructuralError> {
    if value.trim().is_empty() || value.len() > 4096 {
        Err(invalid(format!(
            "{context} must be nonempty and at most 4096 bytes"
        )))
    } else {
        Ok(())
    }
}
fn unique<I: Ord + std::fmt::Display>(
    set: &mut BTreeSet<I>,
    id: I,
    entity: &str,
) -> Result<(), StructuralError> {
    if set.contains(&id) {
        return Err(StructuralError::DuplicateId {
            entity: entity.into(),
            id: id.to_string(),
        });
    }
    set.insert(id);
    Ok(())
}
fn reference<I: Ord + std::fmt::Display>(
    set: &BTreeSet<I>,
    id: &I,
    entity: &str,
) -> Result<(), StructuralError> {
    if set.contains(id) {
        Ok(())
    } else {
        Err(StructuralError::MissingReference {
            entity: entity.into(),
            id: id.to_string(),
        })
    }
}

/// Pure validation only: no stiffness assembly, solve, mutation or run creation.
pub fn validate(model: &StructuralModel) -> Result<(), StructuralError> {
    bounded("nodes", model.nodes.len(), MAX_NODES)?;
    bounded("members", model.members.len(), MAX_MEMBERS)?;
    bounded("materials", model.materials.len(), MAX_MEMBERS)?;
    bounded("sections", model.sections.len(), MAX_MEMBERS)?;
    bounded("supports", model.supports.len(), MAX_NODES)?;
    bounded("load_cases", model.load_cases.len(), MAX_LOAD_CASES)?;
    bounded("combinations", model.combinations.len(), MAX_COMBINATIONS)?;
    if model.nodes.len() < 2 || model.members.is_empty() {
        return Err(invalid("At least two nodes and one member are required"));
    }
    let mut nodes = BTreeSet::new();
    let mut materials = BTreeSet::new();
    let mut sections = BTreeSet::new();
    let mut members = BTreeSet::new();
    let mut cases = BTreeSet::new();
    let active = active_components(model.space);
    let plane_y = model.nodes[0].position_m[1];
    for node in &model.nodes {
        unique(&mut nodes, node.id, "node")?;
        finite(&node.position_m, "node coordinates")?;
        if model.space != StructuralSpace::Frame3d && (node.position_m[1] - plane_y).abs() > 1.0e-9
        {
            return Err(invalid(
                "All plane nodes must lie in one XZ plane (same global Y)",
            ));
        }
    }
    for mat in &model.materials {
        unique(&mut materials, mat.id, "material")?;
        finite(
            &[
                mat.elastic_modulus_kn_m2,
                mat.shear_modulus_kn_m2,
                mat.weight_density_kn_m3,
            ],
            "material",
        )?;
        if mat.elastic_modulus_kn_m2 <= 0.0
            || mat.shear_modulus_kn_m2 <= 0.0
            || mat.weight_density_kn_m3 < 0.0
        {
            return Err(invalid(format!(
                "Material {} requires E>0, G>0 and weight density>=0",
                mat.id
            )));
        }
    }
    for sec in &model.sections {
        unique(&mut sections, sec.id, "section")?;
        finite(
            &[sec.area_m2, sec.iy_m4, sec.iz_m4, sec.torsional_constant_m4],
            "section",
        )?;
        if sec.area_m2 <= 0.0
            || sec.iy_m4 < 0.0
            || sec.iz_m4 < 0.0
            || sec.torsional_constant_m4 < 0.0
        {
            return Err(invalid(format!(
                "Section {} requires A>0 and nonnegative inertias",
                sec.id
            )));
        }
    }
    let node_map: BTreeMap<_, _> = model.nodes.iter().map(|n| (n.id, n)).collect();
    let section_map: BTreeMap<_, _> = model.sections.iter().map(|s| (s.id, s)).collect();
    for member in &model.members {
        unique(&mut members, member.id, "member")?;
        reference(&nodes, &member.start, "member start node")?;
        reference(&nodes, &member.end, "member end node")?;
        reference(&materials, &member.material, "member material")?;
        reference(&sections, &member.section, "member section")?;
        match (model.space, member.formulation) {
            (StructuralSpace::Truss2dXz, MemberFormulation::Truss2d)
            | (
                StructuralSpace::Frame2dXz,
                MemberFormulation::Frame2d | MemberFormulation::Truss2d,
            )
            | (StructuralSpace::Frame3d, MemberFormulation::Frame3d) => {}
            _ => {
                return Err(unsupported(
                    "Member formulation is incompatible with model space",
                ));
            }
        }
        if member.end_releases.is_some() || member.rigid_offset_m.is_some() {
            return Err(unsupported(
                "Joint releases and rigid offsets/links require an independently verified formulation",
            ));
        }
        let section = section_map[&member.section];
        if member.formulation != MemberFormulation::Truss2d && section.iy_m4 <= 0.0 {
            return Err(invalid(format!("Frame member {} requires Iy>0", member.id)));
        }
        if member.formulation == MemberFormulation::Frame3d
            && (section.iz_m4 <= 0.0 || section.torsional_constant_m4 <= 0.0)
        {
            return Err(invalid(format!(
                "3D frame member {} requires Iz>0 and J>0",
                member.id
            )));
        }
        axes(
            member,
            node_map[&member.start].position_m,
            node_map[&member.end].position_m,
        )?;
    }
    let mut support_nodes = BTreeSet::new();
    for support in &model.supports {
        reference(&nodes, &support.node, "support node")?;
        unique(&mut support_nodes, support.node, "support node")?;
        if !support.restrained.iter().any(|v| *v) {
            return Err(invalid("A support must restrain at least one component"));
        }
        if !active.iter().any(|c| support.restrained[*c]) {
            return Err(invalid("Support does not restrain any active component"));
        }
    }
    let mut self_weight_members = BTreeSet::new();
    let member_map: BTreeMap<_, _> = model.members.iter().map(|m| (m.id, m)).collect();
    for case in &model.load_cases {
        unique(&mut cases, case.id, "load case")?;
        require_text(&case.provenance, "load case provenance")?;
        if matches!(case.kind, LoadCaseKind::Temperature | LoadCaseKind::Special) {
            return Err(unsupported(
                "Temperature and Special load cases are reserved and not implemented",
            ));
        }
        bounded(
            "nodal loads per case",
            case.nodal_loads.len(),
            MAX_NODES * 6,
        )?;
        bounded(
            "uniform loads per case",
            case.uniform_member_loads.len(),
            MAX_MEMBERS * 4,
        )?;
        bounded(
            "self weight sources per case",
            case.self_weight.len(),
            MAX_MEMBERS,
        )?;
        let mut sources = BTreeSet::new();
        let mut source = |id: &str| -> Result<(), StructuralError> {
            require_text(id, "load source ID")?;
            if !sources.insert(id.to_string()) {
                return Err(StructuralError::DuplicateLoadSource {
                    case: case.id,
                    source_id: id.into(),
                });
            }
            Ok(())
        };
        for load in &case.nodal_loads {
            source(&load.source_id)?;
            reference(&nodes, &load.node, "load node")?;
            finite(&load.force_moment, "nodal load")?;
            if (0..6).any(|c| !active.contains(&c) && load.force_moment[c] != 0.0) {
                return Err(unsupported(
                    "Nodal load acts on a component excluded by model space",
                ));
            }
        }
        for load in &case.uniform_member_loads {
            source(&load.source_id)?;
            reference(&members, &load.member, "loaded member")?;
            finite(&load.local_force_kn_m, "uniform member load")?;
            check_uniform(member_map[&load.member], load.local_force_kn_m)?;
        }
        for weight in &case.self_weight {
            source(&weight.source_id)?;
            if case.kind != LoadCaseKind::Dead {
                return Err(invalid("Self weight is permitted only in a Dead load case"));
            }
            bounded(
                "members per self weight source",
                weight.members.len(),
                MAX_MEMBERS,
            )?;
            if weight.members.is_empty() {
                return Err(invalid("Self weight source must select members"));
            }
            for member_id in &weight.members {
                reference(&members, member_id, "self weight member")?;
                if !self_weight_members.insert(*member_id) {
                    return Err(StructuralError::DuplicateLoadSource {
                        case: case.id,
                        source_id: format!("self_weight/member/{member_id}"),
                    });
                }
                let member = member_map[member_id];
                if member.formulation == MemberFormulation::Truss2d {
                    return Err(unsupported(
                        "Truss self weight must be explicitly assigned as joint loads; transverse member loading is not a truss formulation",
                    ));
                }
            }
        }
    }
    let mut combinations = BTreeSet::new();
    for combination in &model.combinations {
        unique(&mut combinations, combination.id, "load combination")?;
        require_text(&combination.provenance, "combination provenance")?;
        bounded(
            "terms per combination",
            combination.terms.len(),
            MAX_LOAD_CASES,
        )?;
        if combination.terms.is_empty() {
            return Err(invalid("A combination requires at least one supplied term"));
        }
        let mut terms = BTreeSet::new();
        for term in &combination.terms {
            reference(&cases, &term.case, "combination load case")?;
            unique(&mut terms, term.case, "combination term")?;
            finite(&[term.coefficient], "combination coefficient")?;
            require_text(&term.provenance, "combination coefficient provenance")?;
        }
    }
    Ok(())
}

pub fn validate_target(
    model: &StructuralModel,
    target: impl Into<AnalysisTarget>,
) -> Result<(), StructuralError> {
    validate(model)?;
    selected_cases(model, target.into()).map(|_| ())
}

/// Stable across vector ordering; includes the entire authoritative model, exact
/// user coefficients, source ledger, target, schema/solver version and units.
/// A fingerprint is an input identity, never execution or engineering attestation.
pub fn input_fingerprint(
    model: &StructuralModel,
    target: impl Into<AnalysisTarget>,
) -> Result<String, StructuralError> {
    let target = target.into();
    validate_target(model, target)?;
    let mut canonical = model.clone();
    canonical.nodes.sort_by_key(|v| v.id);
    canonical.materials.sort_by_key(|v| v.id);
    canonical.sections.sort_by_key(|v| v.id);
    canonical.members.sort_by_key(|v| v.id);
    canonical.supports.sort_by_key(|v| v.node);
    canonical.load_cases.sort_by_key(|v| v.id);
    canonical.combinations.sort_by_key(|v| v.id);
    for case in &mut canonical.load_cases {
        case.nodal_loads
            .sort_by(|a, b| a.source_id.cmp(&b.source_id));
        case.uniform_member_loads
            .sort_by(|a, b| a.source_id.cmp(&b.source_id));
        case.self_weight
            .sort_by(|a, b| a.source_id.cmp(&b.source_id));
        for source in &mut case.self_weight {
            source.members.sort();
        }
    }
    for combination in &mut canonical.combinations {
        combination.terms.sort_by_key(|v| v.case);
    }
    let bytes = serde_json::to_vec(&(
        SOLVER_VERSION,
        "RH-global-z-up;XZ-plane-ry;m;kN;kNm;rad;kN/m;kN/m2;kN/m3",
        canonical,
        target,
    ))
    .map_err(|e| {
        invalid(format!(
            "Cannot serialize structural input fingerprint: {e}"
        ))
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn selected_cases(
    model: &StructuralModel,
    target: AnalysisTarget,
) -> Result<Vec<(&StructuralLoadCase, f64)>, StructuralError> {
    match target {
        AnalysisTarget::Case(id) => model
            .load_cases
            .iter()
            .find(|c| c.id == id)
            .map(|c| vec![(c, 1.0)])
            .ok_or_else(|| StructuralError::MissingReference {
                entity: "analysis load case".into(),
                id: id.to_string(),
            }),
        AnalysisTarget::Combination(id) => {
            let combination = model
                .combinations
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| StructuralError::MissingReference {
                    entity: "analysis load combination".into(),
                    id: id.to_string(),
                })?;
            combination
                .terms
                .iter()
                .map(|term| {
                    model
                        .load_cases
                        .iter()
                        .find(|c| c.id == term.case)
                        .map(|c| (c, term.coefficient))
                        .ok_or_else(|| StructuralError::MissingReference {
                            entity: "combination load case".into(),
                            id: term.case.to_string(),
                        })
                })
                .collect()
        }
    }
}

fn axes(
    member: &StructuralMember,
    a: [f64; 3],
    b: [f64; 3],
) -> Result<(f64, [[f64; 3]; 3]), StructuralError> {
    let delta = std::array::from_fn(|i| b[i] - a[i]);
    let length = norm(delta);
    let coordinate_scale = a
        .iter()
        .chain(b.iter())
        .fold(1.0_f64, |s, v| s.max(v.abs()));
    if !length.is_finite() || length <= (coordinate_scale * f64::EPSILON * 32.0).max(1.0e-9) {
        return Err(StructuralError::DegenerateMember {
            member: member.id,
            message: "Coincident, numerically indistinguishable or non-finite endpoints".into(),
        });
    }
    let x = delta.map(|v| v / length);
    let reference = if member.formulation == MemberFormulation::Frame3d {
        member.local_y_reference.ok_or_else(|| {
            invalid(format!(
                "Frame3d member {} requires local_y_reference",
                member.id
            ))
        })?
    } else {
        if member.local_y_reference.is_some() {
            return Err(unsupported(
                "Plane members have fixed local +Y; no roll reference",
            ));
        }
        [0.0, 1.0, 0.0]
    };
    finite(&reference, "member local y reference")?;
    let reference_norm = norm(reference);
    if reference_norm <= 0.0 || !reference_norm.is_finite() {
        return Err(StructuralError::DegenerateMember {
            member: member.id,
            message: "Zero or invalid orientation reference".into(),
        });
    }
    let normalized = reference.map(|v| v / reference_norm);
    let projection = dot(normalized, x);
    let y_unscaled = std::array::from_fn(|i| normalized[i] - projection * x[i]);
    let y_norm = norm(y_unscaled);
    if y_norm <= 1.0e-10 {
        return Err(StructuralError::DegenerateMember {
            member: member.id,
            message: "Local y reference is parallel or nearly parallel to member x".into(),
        });
    }
    let y = y_unscaled.map(|v| v / y_norm);
    let z = cross(x, y);
    Ok((length, [x, y, z]))
}

fn check_uniform(member: &StructuralMember, q: [f64; 3]) -> Result<(), StructuralError> {
    if member.formulation == MemberFormulation::Truss2d && (q[1] != 0.0 || q[2] != 0.0) {
        return Err(unsupported(
            "A truss member supports only axial distributed loading; apply transverse loading to joints explicitly",
        ));
    }
    if member.formulation == MemberFormulation::Frame2d && q[1] != 0.0 {
        return Err(unsupported(
            "Plane XZ frame cannot carry local Y distributed loading",
        ));
    }
    Ok(())
}

fn pair(k: &mut M12, a: usize, b: usize, value: f64) {
    k[a][a] += value;
    k[b][b] += value;
    k[a][b] -= value;
    k[b][a] -= value;
}
fn bending(k: &mut M12, indices: [usize; 4], ei: f64, length: f64, rotation_sign: f64) {
    let l = length;
    let base = [
        [12.0, 6.0 * l, -12.0, 6.0 * l],
        [6.0 * l, 4.0 * l * l, -6.0 * l, 2.0 * l * l],
        [-12.0, -6.0 * l, 12.0, -6.0 * l],
        [6.0 * l, 2.0 * l * l, -6.0 * l, 4.0 * l * l],
    ];
    let signs = [1.0, rotation_sign, 1.0, rotation_sign];
    for i in 0..4 {
        for j in 0..4 {
            k[indices[i]][indices[j]] += (ei / l / l / l) * base[i][j] * signs[i] * signs[j];
        }
    }
}

fn local_stiffness(
    member: &StructuralMember,
    material: &StructuralMaterial,
    section: &StructuralSection,
    l: f64,
) -> M12 {
    let mut k = [[0.0; 12]; 12];
    pair(
        &mut k,
        0,
        6,
        material.elastic_modulus_kn_m2 * section.area_m2 / l,
    );
    if member.formulation != MemberFormulation::Truss2d {
        bending(
            &mut k,
            [2, 4, 8, 10],
            material.elastic_modulus_kn_m2 * section.iy_m4,
            l,
            -1.0,
        );
    }
    if member.formulation == MemberFormulation::Frame3d {
        pair(
            &mut k,
            3,
            9,
            material.shear_modulus_kn_m2 * section.torsional_constant_m4 / l,
        );
        bending(
            &mut k,
            [1, 5, 7, 11],
            material.elastic_modulus_kn_m2 * section.iz_m4,
            l,
            1.0,
        );
    }
    k
}

fn equivalent_uniform(formulation: MemberFormulation, l: f64, q: [f64; 3]) -> [f64; 12] {
    let mut f = [0.0; 12];
    f[0] = q[0] * l / 2.0;
    f[6] = f[0];
    if formulation != MemberFormulation::Truss2d {
        f[2] = q[2] * l / 2.0;
        f[8] = f[2];
        f[4] = -q[2] * l * l / 12.0;
        f[10] = -f[4];
    }
    if formulation == MemberFormulation::Frame3d {
        f[1] = q[1] * l / 2.0;
        f[7] = f[1];
        f[5] = q[1] * l * l / 12.0;
        f[11] = -f[5];
    }
    f
}

fn transformation(axes: [[f64; 3]; 3]) -> M12 {
    let mut t = [[0.0; 12]; 12];
    for block in [0, 3, 6, 9] {
        for i in 0..3 {
            for j in 0..3 {
                t[block + i][block + j] = axes[i][j];
            }
        }
    }
    t
}
fn mat_vec(k: &M12, d: &[f64; 12]) -> [f64; 12] {
    std::array::from_fn(|i| (0..12).map(|j| k[i][j] * d[j]).sum())
}
fn transpose_vec(t: &M12, d: &[f64; 12]) -> [f64; 12] {
    std::array::from_fn(|i| (0..12).map(|j| t[j][i] * d[j]).sum())
}
fn rotate_stiffness(k: &M12, t: &M12) -> M12 {
    let mut kt = [[0.0; 12]; 12];
    let mut global = [[0.0; 12]; 12];
    for i in 0..12 {
        for j in 0..12 {
            kt[i][j] = (0..12).map(|c| k[i][c] * t[c][j]).sum();
        }
    }
    for i in 0..12 {
        for j in 0..12 {
            global[i][j] = (0..12).map(|c| t[c][i] * kt[c][j]).sum();
        }
    }
    global
}

struct Element {
    member: StructuralMemberId,
    formulation: MemberFormulation,
    nodes: [usize; 2],
    length: f64,
    axes: [[f64; 3]; 3],
    transform: M12,
    stiffness: M12,
    uniform: [f64; 3],
    equivalent: [f64; 12],
}

fn prepare(
    model: &StructuralModel,
    target: AnalysisTarget,
) -> Result<(Vec<Element>, Vec<f64>, Vec<LoadSourceReceipt>), StructuralError> {
    let cases = selected_cases(model, target)?;
    let active = active_components(model.space);
    let nd = active.len();
    let n = model.nodes.len() * nd;
    let node_index: BTreeMap<_, _> = model
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id, i))
        .collect();
    let material_map: BTreeMap<_, _> = model.materials.iter().map(|mat| (mat.id, mat)).collect();
    let section_map: BTreeMap<_, _> = model.sections.iter().map(|sec| (sec.id, sec)).collect();
    let mut elements = Vec::with_capacity(model.members.len());
    for member in &model.members {
        let nodes = [node_index[&member.start], node_index[&member.end]];
        let (length, axes) = axes(
            member,
            model.nodes[nodes[0]].position_m,
            model.nodes[nodes[1]].position_m,
        )?;
        elements.push(Element {
            member: member.id,
            formulation: member.formulation,
            nodes,
            length,
            axes,
            transform: transformation(axes),
            stiffness: local_stiffness(
                member,
                material_map[&member.material],
                section_map[&member.section],
                length,
            ),
            uniform: [0.0; 3],
            equivalent: [0.0; 12],
        });
    }
    let member_index: BTreeMap<_, _> = elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.member, i))
        .collect();
    let mut f = vec![0.0; n];
    let mut load_sources = Vec::new();
    for (case, coefficient) in cases {
        for load in &case.nodal_loads {
            let index = node_index[&load.node];
            for (d, c) in active.iter().enumerate() {
                f[index * nd + d] += coefficient * load.force_moment[*c];
            }
            load_sources.push(LoadSourceReceipt {
                case: case.id,
                source_id: load.source_id.clone(),
                coefficient,
                kind: "nodal".into(),
                member: None,
            });
        }
        for load in &case.uniform_member_loads {
            let e = &mut elements[member_index[&load.member]];
            for c in 0..3 {
                e.uniform[c] += coefficient * load.local_force_kn_m[c];
            }
            load_sources.push(LoadSourceReceipt {
                case: case.id,
                source_id: load.source_id.clone(),
                coefficient,
                kind: "uniform_local_force".into(),
                member: Some(load.member),
            });
        }
        for weight in &case.self_weight {
            for id in &weight.members {
                let index = member_index[id];
                let member = &model.members[index];
                let density = material_map[&member.material].weight_density_kn_m3;
                let area = section_map[&member.section].area_m2;
                let e = &mut elements[index];
                let global = [0.0, 0.0, -density * area];
                for c in 0..3 {
                    e.uniform[c] += coefficient * dot(e.axes[c], global);
                }
                load_sources.push(LoadSourceReceipt {
                    case: case.id,
                    source_id: weight.source_id.clone(),
                    coefficient,
                    kind: "body_self_weight_global_minus_z".into(),
                    member: Some(*id),
                });
            }
        }
    }
    for e in &mut elements {
        e.equivalent = equivalent_uniform(e.formulation, e.length, e.uniform);
    }
    load_sources.sort_by(|a, b| {
        (a.case, &a.source_id, a.member, &a.kind).cmp(&(b.case, &b.source_id, b.member, &b.kind))
    });
    Ok((elements, f, load_sources))
}

/// Analyze one coherent case/combination. Never computes an envelope across runs.
pub fn analyze(
    model: &StructuralModel,
    target: impl Into<AnalysisTarget>,
) -> Result<StructuralAnalysisResult, StructuralError> {
    validate(model)?;
    let target = target.into();
    let input_fingerprint = input_fingerprint(model, target)?;
    let active = active_components(model.space);
    let nd = active.len();
    let n = model.nodes.len() * nd;
    let node_index: BTreeMap<_, _> = model
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id, i))
        .collect();
    let (elements, mut f, load_sources) = prepare(model, target)?;
    let mut k = vec![vec![0.0; n]; n];
    for e in &elements {
        let global_f = transpose_vec(&e.transform, &e.equivalent);
        let global_k = rotate_stiffness(&e.stiffness, &e.transform);
        for end_a in 0..2 {
            for (da, ca) in active.iter().enumerate() {
                let a = e.nodes[end_a] * nd + da;
                let local_a = end_a * 6 + ca;
                f[a] += global_f[local_a];
                for end_b in 0..2 {
                    for (db, cb) in active.iter().enumerate() {
                        let b = e.nodes[end_b] * nd + db;
                        k[a][b] += global_k[local_a][end_b * 6 + cb];
                    }
                }
            }
        }
    }
    if !f.iter().all(|v| v.is_finite()) || !k.iter().flatten().all(|v| v.is_finite()) {
        return Err(numeric("Overflow or non-finite value during assembly"));
    }
    let mut constrained = vec![false; n];
    for support in &model.supports {
        let index = node_index[&support.node];
        for (d, c) in active.iter().enumerate() {
            constrained[index * nd + d] = support.restrained[*c];
        }
    }
    let free: Vec<_> = (0..n).filter(|i| !constrained[*i]).collect();
    let characteristic_length = elements.iter().fold(0.0_f64, |m, e| m.max(e.length));
    let solution = solve_scaled(&k, &f, &free, active, model, characteristic_length)?;
    let d = solution.displacements;
    let kd: Vec<f64> = k
        .iter()
        .map(|row| row.iter().zip(&d).map(|(k, d)| k * d).sum())
        .collect();
    let residual: Vec<f64> = kd.iter().zip(&f).map(|(a, b)| a - b).collect();
    // Componentwise backward error prevents a large axial load from masking
    // an inaccurate small transverse force or moment on a different DOF.
    let relative_free_residual = free
        .iter()
        .map(|i| {
            let scale = f[*i].abs()
                + k[*i]
                    .iter()
                    .zip(&d)
                    .map(|(a, b)| (a * b).abs())
                    .sum::<f64>();
            safe_ratio(residual[*i].abs(), scale)
        })
        .fold(0.0_f64, f64::max);
    if !relative_free_residual.is_finite() || relative_free_residual > RESIDUAL_TOLERANCE {
        return Err(numeric(format!(
            "Free DOF equilibrium residual {relative_free_residual:e} exceeds tolerance"
        )));
    }
    let nodes: Vec<_> = model
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let mut displacement = [0.0; 6];
            for (a, c) in active.iter().enumerate() {
                displacement[*c] = d[i * nd + a];
            }
            StructuralNodeResult {
                node: node.id,
                position_m: node.position_m,
                displacement,
            }
        })
        .collect();
    let reactions: Vec<_> = model
        .supports
        .iter()
        .map(|support| {
            let index = node_index[&support.node];
            let mut force_moment = [0.0; 6];
            for (a, c) in active.iter().enumerate() {
                if constrained[index * nd + a] {
                    force_moment[*c] = residual[index * nd + a];
                }
            }
            StructuralReaction {
                node: support.node,
                position_m: model.nodes[index].position_m,
                restrained: support.restrained,
                force_moment,
            }
        })
        .collect();
    let members = elements
        .iter()
        .map(|e| {
            let mut global_d = [0.0; 12];
            global_d[..6].copy_from_slice(&nodes[e.nodes[0]].displacement);
            global_d[6..].copy_from_slice(&nodes[e.nodes[1]].displacement);
            let local_d = mat_vec(&e.transform, &global_d);
            let local_force = mat_vec(&e.stiffness, &local_d);
            StructuralMemberResult {
                member: e.member,
                length_m: e.length,
                local_axes: e.axes,
                local_end_displacements: std::array::from_fn(|end| {
                    std::array::from_fn(|c| local_d[end * 6 + c])
                }),
                local_end_forces: std::array::from_fn(|end| {
                    std::array::from_fn(|c| local_force[end * 6 + c] - e.equivalent[end * 6 + c])
                }),
                local_uniform_load_kn_m: e.uniform,
            }
        })
        .collect();
    let (force_balance, moment_balance, force_scale, moment_scale) =
        balance(model, &f, &reactions, active, characteristic_length);
    let relative_force_balance = safe_ratio(norm(force_balance), force_scale);
    let relative_moment_balance = safe_ratio(norm(moment_balance), moment_scale);
    if !relative_force_balance.is_finite()
        || !relative_moment_balance.is_finite()
        || relative_force_balance > RESIDUAL_TOLERANCE
        || relative_moment_balance > RESIDUAL_TOLERANCE
    {
        return Err(numeric(format!(
            "Global force/moment balance failed: {relative_force_balance:e}, {relative_moment_balance:e}"
        )));
    }
    Ok(StructuralAnalysisResult {
        input_fingerprint,target,normative_status:NormativeStatus::Unverified,nodes,reactions,members,load_sources,
        diagnostics:NumericalDiagnostics {
            active_dofs:n,free_dofs:free.len(),characteristic_length_m:characteristic_length,
            min_equilibrated_pivot:solution.min_pivot,max_equilibrated_pivot:solution.max_pivot,
            pivot_ratio_indicator:solution.pivot_ratio,
            relative_free_dof_residual:relative_free_residual,
            global_force_residual_kn:force_balance,global_moment_residual_kn_m:moment_balance,
            relative_force_balance_residual:relative_force_balance,
            relative_moment_balance_residual:relative_moment_balance,
            status:NumericalStatus::LinearSolutionResidualsVerified,
        },
        limitations:vec![
            "First-order small-displacement linear-elastic Euler-Bernoulli/truss analysis only; no shear deformation or second-order effects".into(),
            "Numerical residual acceptance is not engineering verification, lateral/bracing stability, buckling, steel strength, ULS/SLS or reinforced-concrete acceptance".into(),
            "User-supplied loads and combination coefficients; normative status UNVERIFIED; no load envelopes or automatic code factors".into(),
            "No joint releases, rigid links/offsets, support settlements, springs, surface pressure conversion, temperature or Special actions".into(),
        ],
    })
}

fn safe_ratio(value: f64, scale: f64) -> f64 {
    if scale > 0.0 {
        value / scale
    } else if value == 0.0 {
        0.0
    } else {
        f64::INFINITY
    }
}

/// Check an archived result against its authoritative model and target without
/// solving or factorizing. Reconstructs loads, element constitutive forces and
/// nodal/global equilibrium. This does not create a new run or independently
/// re-prove matrix rank; the archived producer/run hash remains required.
pub fn validate_result_binding(
    model: &StructuralModel,
    target: impl Into<AnalysisTarget>,
    result: &StructuralAnalysisResult,
) -> Result<(), StructuralError> {
    validate(model)?;
    let target = target.into();
    if result.input_fingerprint != input_fingerprint(model, target)? {
        return Err(invalid(
            "Archived result input fingerprint differs from authoritative model/target",
        ));
    }
    if result.target != target || result.normative_status != NormativeStatus::Unverified {
        return Err(invalid(
            "Archived result target/normative status differs from the requested model",
        ));
    }
    if result.nodes.len() != model.nodes.len()
        || result.members.len() != model.members.len()
        || result.reactions.len() != model.supports.len()
    {
        return Err(invalid(
            "Archived result node/member/support counts do not match model",
        ));
    }
    let (elements, point_loads, load_sources) = prepare(model, target)?;
    if load_sources.len() != result.load_sources.len() {
        return Err(invalid(
            "Archived result load-source count differs from selected target",
        ));
    }
    let mut archived_sources = result.load_sources.clone();
    archived_sources.sort_by(|a, b| {
        (a.case, &a.source_id, a.member, &a.kind).cmp(&(b.case, &b.source_id, b.member, &b.kind))
    });
    if load_sources != archived_sources {
        return Err(invalid(
            "Archived result load-source ledger differs from selected target",
        ));
    }
    let active = active_components(model.space);
    let nd = active.len();
    let n = model.nodes.len() * nd;
    let node_index: BTreeMap<_, _> = model
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id, i))
        .collect();
    let mut nodes = BTreeMap::new();
    for node in &result.nodes {
        if nodes.insert(node.node, node).is_some() {
            return Err(invalid("Duplicate archived result node"));
        }
        finite(&node.displacement, "archived displacement")?;
    }
    for node in &model.nodes {
        let value = nodes
            .get(&node.id)
            .ok_or_else(|| invalid("Archived result missing model node"))?;
        if value.position_m != node.position_m {
            return Err(invalid("Archived node position differs from model"));
        }
        if (0..6).any(|c| !active.contains(&c) && value.displacement[c] != 0.0) {
            return Err(invalid(
                "Archived displacement is nonzero on an excluded plane DOF",
            ));
        }
    }
    let mut members = BTreeMap::new();
    for member in &result.members {
        if members.insert(member.member, member).is_some() {
            return Err(invalid("Duplicate archived result member"));
        }
    }
    let length = elements.iter().fold(0.0_f64, |s, e| s.max(e.length));
    let mut nodal_ends = vec![0.0; n];
    let mut nodal_scales: Vec<f64> = point_loads.iter().map(|f| f.abs()).collect();
    let mut total_loads = point_loads.clone();
    for e in &elements {
        let value = members
            .get(&e.member)
            .ok_or_else(|| invalid("Archived result missing model member"))?;
        verify_vectors(&[value.length_m], &[e.length], "archived member length")?;
        verify_vectors(
            &value.local_axes.concat(),
            &e.axes.concat(),
            "archived member axes",
        )?;
        verify_vectors(
            &value.local_uniform_load_kn_m,
            &e.uniform,
            "archived member uniform load",
        )?;
        let mut global_d = [0.0; 12];
        for end in 0..2 {
            global_d[end * 6..end * 6 + 6]
                .copy_from_slice(&nodes[&model.nodes[e.nodes[end]].id].displacement);
        }
        let local_d = mat_vec(&e.transform, &global_d);
        let kd = mat_vec(&e.stiffness, &local_d);
        let end_forces: [f64; 12] = std::array::from_fn(|c| kd[c] - e.equivalent[c]);
        let archived_d: [f64; 12] =
            std::array::from_fn(|c| value.local_end_displacements[c / 6][c % 6]);
        let archived_forces: [f64; 12] =
            std::array::from_fn(|c| value.local_end_forces[c / 6][c % 6]);
        let normalized_d: Vec<f64> = local_d
            .iter()
            .enumerate()
            .map(|(c, v)| if c % 6 < 3 { v / e.length } else { *v })
            .collect();
        let normalized_archived_d: Vec<f64> = archived_d
            .iter()
            .enumerate()
            .map(|(c, v)| if c % 6 < 3 { v / e.length } else { *v })
            .collect();
        verify_vectors(
            &normalized_archived_d,
            &normalized_d,
            "archived member local displacements",
        )?;
        let normalized_forces: Vec<f64> = end_forces
            .iter()
            .enumerate()
            .map(|(c, v)| if c % 6 >= 3 { v / e.length } else { *v })
            .collect();
        let normalized_archived_forces: Vec<f64> = archived_forces
            .iter()
            .enumerate()
            .map(|(c, v)| if c % 6 >= 3 { v / e.length } else { *v })
            .collect();
        verify_vectors(
            &normalized_archived_forces,
            &normalized_forces,
            "archived member constitutive end forces",
        )?;
        let global_ends = transpose_vec(&e.transform, &end_forces);
        let global_equivalent = transpose_vec(&e.transform, &e.equivalent);
        for end in 0..2 {
            for (d, c) in active.iter().enumerate() {
                let g = e.nodes[end] * nd + d;
                nodal_ends[g] += global_ends[end * 6 + c];
                nodal_scales[g] += (0..12)
                    .map(|local| {
                        e.transform[local][end * 6 + c].abs()
                            * (kd[local].abs() + e.equivalent[local].abs())
                    })
                    .sum::<f64>();
                total_loads[g] += global_equivalent[end * 6 + c];
            }
        }
    }
    let mut reactions = BTreeMap::new();
    for reaction in &result.reactions {
        if reactions.insert(reaction.node, reaction).is_some() {
            return Err(invalid("Duplicate archived support reaction"));
        }
        finite(&reaction.force_moment, "archived support reaction")?;
    }
    let reaction_comparison_scale = nodal_scales.iter().enumerate().fold(0.0_f64, |s, (g, v)| {
        s.max(if active[g % nd] >= 3 { v / length } else { *v })
    });
    let mut constrained = vec![false; n];
    for support in &model.supports {
        let index = node_index[&support.node];
        let value = reactions
            .get(&support.node)
            .ok_or_else(|| invalid("Archived result missing model support"))?;
        if value.position_m != model.nodes[index].position_m
            || value.restrained != support.restrained
        {
            return Err(invalid(
                "Archived support position/restraint differs from model",
            ));
        }
        let mut expected = [0.0; 6];
        for (d, c) in active.iter().enumerate() {
            let g = index * nd + d;
            constrained[g] = support.restrained[*c];
            if constrained[g] {
                if nodes[&support.node].displacement[*c] != 0.0 {
                    return Err(invalid("Archived constrained displacement is nonzero"));
                }
                expected[*c] = nodal_ends[g] - point_loads[g];
            }
        }
        let actual: Vec<f64> = value
            .force_moment
            .iter()
            .enumerate()
            .map(|(c, v)| if c >= 3 { v / length } else { *v })
            .collect();
        let expected: Vec<f64> = expected
            .iter()
            .enumerate()
            .map(|(c, v)| if c >= 3 { v / length } else { *v })
            .collect();
        verify_vectors_with_scale(
            &actual,
            &expected,
            reaction_comparison_scale,
            "archived global support reaction",
        )?;
    }
    let mut relative_free_equilibrium = 0.0_f64;
    for g in 0..n {
        if !constrained[g] {
            relative_free_equilibrium = relative_free_equilibrium.max(safe_ratio(
                (nodal_ends[g] - point_loads[g]).abs(),
                nodal_scales[g],
            ));
        }
    }
    if !relative_free_equilibrium.is_finite() || relative_free_equilibrium > RESIDUAL_TOLERANCE {
        return Err(invalid(
            "Archived free DOFs do not satisfy nodal equilibrium",
        ));
    }
    let (force, moment, force_scale, moment_scale) =
        balance(model, &total_loads, &result.reactions, active, length);
    let force_relative = safe_ratio(norm(force), force_scale);
    let moment_relative = safe_ratio(norm(moment), moment_scale);
    if !force_relative.is_finite()
        || !moment_relative.is_finite()
        || force_relative > RESIDUAL_TOLERANCE
        || moment_relative > RESIDUAL_TOLERANCE
    {
        return Err(invalid(
            "Archived result does not satisfy global force/moment balance",
        ));
    }
    let diag = &result.diagnostics;
    finite(
        &[
            diag.characteristic_length_m,
            diag.min_equilibrated_pivot,
            diag.max_equilibrated_pivot,
            diag.pivot_ratio_indicator,
            diag.relative_free_dof_residual,
            diag.relative_force_balance_residual,
            diag.relative_moment_balance_residual,
        ],
        "archived numerical diagnostics",
    )?;
    finite(&diag.global_force_residual_kn, "archived force residual")?;
    finite(
        &diag.global_moment_residual_kn_m,
        "archived moment residual",
    )?;
    if diag.active_dofs != n || diag.free_dofs != constrained.iter().filter(|c| !**c).count() {
        return Err(invalid("Archived numerical DOF counts differ from model"));
    }
    verify_vectors(
        &[diag.characteristic_length_m],
        &[length],
        "archived characteristic length",
    )?;
    if diag.min_equilibrated_pivot <= MIN_EQUILIBRATED_PIVOT
        || diag.max_equilibrated_pivot < diag.min_equilibrated_pivot
    {
        return Err(invalid(
            "Archived factorization receipt has invalid pivot bounds",
        ));
    }
    verify_vectors(
        &[diag.pivot_ratio_indicator],
        &[diag.max_equilibrated_pivot / diag.min_equilibrated_pivot],
        "archived pivot ratio",
    )?;
    for residual in [
        diag.relative_free_dof_residual,
        diag.relative_force_balance_residual,
        diag.relative_moment_balance_residual,
    ] {
        if !(0.0..=RESIDUAL_TOLERANCE).contains(&residual) {
            return Err(invalid("Archived claimed residual exceeds tolerance"));
        }
    }
    let force_difference = std::array::from_fn(|c| diag.global_force_residual_kn[c] - force[c]);
    let moment_difference =
        std::array::from_fn(|c| diag.global_moment_residual_kn_m[c] - moment[c]);
    if safe_ratio(norm(force_difference), force_scale) > RESIDUAL_TOLERANCE
        || safe_ratio(norm(moment_difference), moment_scale) > RESIDUAL_TOLERANCE
    {
        return Err(invalid(
            "Archived residual components disagree with reconstructed balance",
        ));
    }
    if result.limitations.is_empty() {
        return Err(invalid("Archived result omitted its scope limitations"));
    }
    Ok(())
}

fn verify_vectors(actual: &[f64], expected: &[f64], context: &str) -> Result<(), StructuralError> {
    verify_vectors_with_scale(actual, expected, 0.0, context)
}
fn verify_vectors_with_scale(
    actual: &[f64],
    expected: &[f64],
    comparison_scale: f64,
    context: &str,
) -> Result<(), StructuralError> {
    finite(actual, context)?;
    finite(expected, context)?;
    if actual.len() != expected.len() {
        return Err(invalid(format!("{context}: size mismatch")));
    }
    let scale = actual
        .iter()
        .chain(expected)
        .fold(comparison_scale, |s, v| s.max(v.abs()));
    let difference = actual
        .iter()
        .zip(expected)
        .fold(0.0_f64, |s, (a, b)| s.max((a - b).abs()));
    if safe_ratio(difference, scale) > RESIDUAL_TOLERANCE {
        return Err(invalid(format!("{context}: mismatch")));
    }
    Ok(())
}

struct Solution {
    displacements: Vec<f64>,
    min_pivot: f64,
    max_pivot: f64,
    pivot_ratio: f64,
}

fn solve_scaled(
    k: &[Vec<f64>],
    f: &[f64],
    free: &[usize],
    active: &[usize],
    model: &StructuralModel,
    length: f64,
) -> Result<Solution, StructuralError> {
    let count = free.len();
    let mut d = vec![0.0; f.len()];
    if count == 0 {
        return Ok(Solution {
            displacements: d,
            min_pivot: 1.0,
            max_pivot: 1.0,
            pivot_ratio: 1.0,
        });
    }
    // d = S z: translations use characteristic L, rotations unit radians.
    // Then symmetric Jacobi equilibration makes the diagonal dimensionless.
    let physical_scale: Vec<_> = free
        .iter()
        .map(|i| {
            if active[*i % active.len()] < 3 {
                length
            } else {
                1.0
            }
        })
        .collect();
    let singular = |index: usize, pivot: f64| {
        let original = free[index];
        StructuralError::SingularMechanism {
            free_dof_index: index,
            node: model.nodes[original / active.len()].id,
            component: active[original % active.len()],
            pivot,
        }
    };
    let mut scale = Vec::with_capacity(count);
    for (i, original) in free.iter().enumerate() {
        let diagonal = k[*original][*original] * physical_scale[i] * physical_scale[i];
        if !diagonal.is_finite() {
            return Err(numeric("Non-finite diagonal during DOF scaling"));
        }
        if diagonal <= 0.0 {
            return Err(singular(i, diagonal));
        }
        let factor = physical_scale[i] / diagonal.sqrt();
        if !factor.is_finite() || factor <= 0.0 {
            return Err(numeric("Invalid DOF scaling"));
        }
        scale.push(factor);
    }
    let a: Vec<Vec<f64>> = free
        .iter()
        .enumerate()
        .map(|(i, gi)| {
            free.iter()
                .enumerate()
                .map(|(j, gj)| k[*gi][*gj] * scale[i] * scale[j])
                .collect()
        })
        .collect();
    let rhs: Vec<f64> = free
        .iter()
        .enumerate()
        .map(|(i, g)| f[*g] * scale[i])
        .collect();
    if !rhs.iter().all(|v| v.is_finite()) || !a.iter().flatten().all(|v| v.is_finite()) {
        return Err(numeric("Non-finite equilibrated system"));
    }
    // Cholesky is appropriate only for a supported elastic, positive-definite model.
    // Never regularize a mechanism or substitute zero displacements.
    let mut l = vec![vec![0.0; count]; count];
    let mut min_pivot = f64::INFINITY;
    let mut max_pivot = 0.0_f64;
    for i in 0..count {
        for j in 0..=i {
            let value = a[i][j] - (0..j).map(|c| l[i][c] * l[j][c]).sum::<f64>();
            if i == j {
                if !value.is_finite() {
                    return Err(numeric("Non-finite Cholesky pivot"));
                }
                if value <= MIN_EQUILIBRATED_PIVOT {
                    return Err(singular(i, value));
                }
                min_pivot = min_pivot.min(value);
                max_pivot = max_pivot.max(value);
                l[i][j] = value.sqrt();
            } else {
                l[i][j] = value / l[j][j];
            }
        }
    }
    let mut z = cholesky_substitute(&l, &rhs);
    // A bounded iterative-refinement pass reduces roundoff without modifying K.
    for _ in 0..2 {
        let correction_rhs: Vec<f64> = a
            .iter()
            .zip(&rhs)
            .map(|(row, b)| b - row.iter().zip(&z).map(|(a, z)| a * z).sum::<f64>())
            .collect();
        let correction = cholesky_substitute(&l, &correction_rhs);
        for i in 0..count {
            z[i] += correction[i];
        }
    }
    for (i, g) in free.iter().enumerate() {
        d[*g] = scale[i] * z[i];
    }
    if !d.iter().all(|v| v.is_finite()) {
        return Err(numeric("Non-finite solved displacement"));
    }
    Ok(Solution {
        displacements: d,
        min_pivot,
        max_pivot,
        pivot_ratio: max_pivot / min_pivot,
    })
}

fn cholesky_substitute(l: &[Vec<f64>], rhs: &[f64]) -> Vec<f64> {
    let count = rhs.len();
    let mut x = vec![0.0; count];
    for i in 0..count {
        x[i] = (rhs[i] - (0..i).map(|j| l[i][j] * x[j]).sum::<f64>()) / l[i][i];
    }
    for i in (0..count).rev() {
        x[i] = (x[i] - (i + 1..count).map(|j| l[j][i] * x[j]).sum::<f64>()) / l[i][i];
    }
    x
}

fn balance(
    model: &StructuralModel,
    f: &[f64],
    reactions: &[StructuralReaction],
    active: &[usize],
    length: f64,
) -> ([f64; 3], [f64; 3], f64, f64) {
    let mut force = [0.0; 3];
    let mut moment = [0.0; 3];
    let mut force_scale = 0.0;
    let mut moment_scale = 0.0;
    for (i, node) in model.nodes.iter().enumerate() {
        let mut load = [0.0; 6];
        for (d, c) in active.iter().enumerate() {
            load[*c] = f[i * active.len() + d];
        }
        accumulate_balance(
            node.position_m,
            load,
            &mut force,
            &mut moment,
            &mut force_scale,
            &mut moment_scale,
        );
    }
    for reaction in reactions {
        accumulate_balance(
            reaction.position_m,
            reaction.force_moment,
            &mut force,
            &mut moment,
            &mut force_scale,
            &mut moment_scale,
        );
    }
    // Characteristic force × L also makes a zero-applied-moment case well scaled.
    moment_scale = moment_scale.max(force_scale * length);
    (force, moment, force_scale, moment_scale)
}

fn accumulate_balance(
    position: [f64; 3],
    load: Six,
    force: &mut [f64; 3],
    moment: &mut [f64; 3],
    force_scale: &mut f64,
    moment_scale: &mut f64,
) {
    let f = [load[0], load[1], load[2]];
    let m = [load[3], load[4], load[5]];
    let arm = cross(position, f);
    for c in 0..3 {
        force[c] += f[c];
        moment[c] += m[c] + arm[c];
    }
    *force_scale += norm(f);
    *moment_scale += norm(m) + norm(arm);
}
