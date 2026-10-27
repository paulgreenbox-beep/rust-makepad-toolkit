use crate::generator::connected_analysis_scope;
use crate::model::{
    ConfiguratorError, ConnectedAnalysisScope, DenseIdMap, GeneratedBuilding, GeneratedMember,
    InterfaceLoadPathStatus, MemberProfile, SemanticId, SteelMaterial,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use structural_core::{
    LoadCaseId, LoadCaseKind, MaterialId, MemberFormulation, MemberId, NodeId, NormativeStatus,
    SectionId, SelfWeightLoad, StructuralLoadCase, StructuralMaterial, StructuralMember,
    StructuralModel, StructuralNode, StructuralSection, StructuralSpace, StructuralSupport,
    MAX_MEMBERS, MAX_NODES,
};

/// Return deterministic, namespace-separated dense IDs for every generated entity.
/// IDs are based on the full building graph so whole-building and scoped projections
/// retain identical IDs for the same semantic entity.
pub fn dense_id_map(building: &GeneratedBuilding) -> Result<DenseIdMap, ConfiguratorError> {
    let mut node_ids = BTreeSet::new();
    for (key, node) in &building.nodes {
        if key != &node.id {
            return Err(binding_mismatch(format!(
                "node map key {key} differs from generated node ID {}",
                node.id
            )));
        }
        node_ids.insert(node.id.clone());
    }

    let mut member_ids = BTreeSet::new();
    for (key, member) in &building.members {
        if key != &member.id {
            return Err(binding_mismatch(format!(
                "member map key {key} differs from generated member ID {}",
                member.id
            )));
        }
        member_ids.insert(member.id.clone());
    }

    let material_ids = unique_material_ids(&building.materials)?;
    let profile_ids = unique_profile_ids(&building.profiles)?;
    let mut used = BTreeMap::new();
    Ok(DenseIdMap {
        nodes: dense_ids_for(&node_ids, "node", &mut used)?,
        members: dense_ids_for(&member_ids, "member", &mut used)?,
        materials: dense_ids_for(&material_ids, "material", &mut used)?,
        profiles: dense_ids_for(&profile_ids, "profile", &mut used)?,
    })
}

/// Project the complete generated building, retaining disconnected components only
/// when the generated configuration explicitly allowed their joint analysis.
pub fn to_structural_model(
    building: &GeneratedBuilding,
) -> Result<StructuralModel, ConfiguratorError> {
    let selected = all_section_ids(building);
    if selected.is_empty() {
        return Err(invalid_scope("generated building has no section owners"));
    }
    check_generation_limits(building, &selected)?;
    check_unverified_interfaces(building, &selected)?;
    let component_count = connected_component_count(building, &selected)?;
    if component_count > 1 && !building.allow_disconnected_analysis {
        return Err(ConfiguratorError::DisconnectedLoadPathsNotAcknowledged);
    }
    project_selected(building, &selected)
}

/// Project one complete explicit-rigid component. The requested scope must match
/// the transitive closure exactly, so connected components cannot be partitioned to
/// evade the structural-core capacity limit.
pub fn to_structural_model_for_scope(
    building: &GeneratedBuilding,
    scope: &ConnectedAnalysisScope,
) -> Result<StructuralModel, ConfiguratorError> {
    if scope.section_ids.is_empty() {
        return Err(invalid_scope("connected analysis scope is empty"));
    }
    let owners = all_section_ids(building);
    if let Some(unknown) = scope.section_ids.difference(&owners).next() {
        return Err(invalid_scope(format!(
            "scope section {unknown} is not present in the generated building"
        )));
    }
    let Some(seed) = scope.section_ids.iter().next() else {
        return Err(invalid_scope("connected analysis scope is empty"));
    };
    let closure = connected_analysis_scope(building, seed.as_str())?;
    if closure.section_ids != scope.section_ids {
        return Err(invalid_scope(
            "scope must equal the full transitive closure of explicit rigid interfaces",
        ));
    }

    check_generation_limits(building, &scope.section_ids)?;
    check_unverified_interfaces(building, &scope.section_ids)?;
    project_selected(building, &scope.section_ids)
}

fn project_selected(
    building: &GeneratedBuilding,
    selected_owners: &BTreeSet<SemanticId>,
) -> Result<StructuralModel, ConfiguratorError> {
    if building.analysis_load_ledger.scope != building.analysis_load_scope {
        return Err(binding_mismatch(
            "analysis load ledger scope differs from generated load scope".into(),
        ));
    }

    let selected_members: Vec<&GeneratedMember> = building
        .members
        .values()
        .filter(|member| {
            member
                .owners
                .iter()
                .any(|owner| selected_owners.contains(owner))
        })
        .collect();
    if selected_members.is_empty() {
        let owner = selected_owners
            .iter()
            .next()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<empty>".into());
        return Err(ConfiguratorError::UnsupportedGeneration {
            section_id: owner,
            reason: "selected owners contain no generated physical frame members".into(),
        });
    }

    let mut selected_member_ids = BTreeSet::new();
    let mut material_ids = BTreeSet::new();
    let mut profile_ids = BTreeSet::new();
    let mut selected_node_ids = BTreeSet::new();
    for (key, node) in &building.nodes {
        if key != &node.id {
            return Err(binding_mismatch(format!(
                "node map key {key} differs from generated node ID {}",
                node.id
            )));
        }
        if node
            .owners
            .iter()
            .any(|owner| selected_owners.contains(owner))
        {
            if !node.owners.is_subset(selected_owners) {
                return Err(invalid_scope(format!(
                    "node {} is shared with an owner outside the selected component",
                    node.id
                )));
            }
            selected_node_ids.insert(node.id.clone());
        }
        if node.position_m.iter().any(|value| !value.is_finite()) {
            return Err(binding_mismatch(format!(
                "node {} has non-finite coordinates",
                node.id
            )));
        }
    }

    for member in &selected_members {
        if !member.owners.is_subset(selected_owners) {
            return Err(invalid_scope(format!(
                "member {} is shared with an owner outside the selected component",
                member.id
            )));
        }
        if !selected_member_ids.insert(member.id.clone()) {
            return Err(binding_mismatch(format!(
                "generated member {} appears more than once",
                member.id
            )));
        }
        material_ids.insert(member.material_id.clone());
        profile_ids.insert(member.profile_id.clone());
        for endpoint in [&member.start, &member.end] {
            let Some(node) = building.nodes.get(endpoint) else {
                return Err(ConfiguratorError::MissingReference {
                    entity: "member endpoint node".into(),
                    id: endpoint.to_string(),
                });
            };
            if !node.owners.is_subset(selected_owners) {
                return Err(invalid_scope(format!(
                    "member {} endpoint node {} is shared with an owner outside the selected component",
                    member.id, node.id
                )));
            }
            selected_node_ids.insert(endpoint.clone());
        }
    }

    let node_count = selected_node_ids.len();
    let member_count = selected_member_ids.len();
    if node_count > MAX_NODES || member_count > MAX_MEMBERS {
        return Err(ConfiguratorError::AnalysisModelCapacityExceeded {
            nodes: node_count,
            members: member_count,
            max_nodes: MAX_NODES,
            max_members: MAX_MEMBERS,
        });
    }

    let dense = dense_id_map(building)?;
    let mut nodes = Vec::with_capacity(node_count);
    let mut supports = Vec::new();
    for semantic_id in &selected_node_ids {
        let Some(node) = building.nodes.get(semantic_id) else {
            return Err(ConfiguratorError::MissingReference {
                entity: "generated node".into(),
                id: semantic_id.to_string(),
            });
        };
        let raw = dense.nodes.get(semantic_id).copied().ok_or_else(|| {
            ConfiguratorError::MissingReference {
                entity: "dense node ID".into(),
                id: semantic_id.to_string(),
            }
        })?;
        let node_id = NodeId::new(raw).map_err(|_| dense_id_failure("node", semantic_id))?;
        nodes.push(StructuralNode {
            id: node_id,
            position_m: node.position_m,
        });
        if let Some(restrained) = node.restrained {
            supports.push(StructuralSupport {
                node: node_id,
                restrained,
            });
        }
    }

    let structural_materials = material_ids
        .iter()
        .map(|semantic_id| {
            let source = unique_material(building, semantic_id)?;
            let raw = dense.materials.get(semantic_id).copied().ok_or_else(|| {
                ConfiguratorError::MissingReference {
                    entity: "dense material ID".into(),
                    id: semantic_id.to_string(),
                }
            })?;
            let id = MaterialId::new(raw).map_err(|_| dense_id_failure("material", semantic_id))?;
            Ok(StructuralMaterial {
                id,
                name: source.name.clone(),
                elastic_modulus_kn_m2: source.elastic_modulus_kn_m2,
                shear_modulus_kn_m2: source.shear_modulus_kn_m2,
                weight_density_kn_m3: source.weight_density_kn_m3,
            })
        })
        .collect::<Result<Vec<_>, ConfiguratorError>>()?;

    let structural_sections = profile_ids
        .iter()
        .map(|semantic_id| {
            let source = unique_profile(building, semantic_id)?;
            let raw = dense.profiles.get(semantic_id).copied().ok_or_else(|| {
                ConfiguratorError::MissingReference {
                    entity: "dense section ID".into(),
                    id: semantic_id.to_string(),
                }
            })?;
            let id = SectionId::new(raw).map_err(|_| dense_id_failure("profile", semantic_id))?;
            Ok(StructuralSection {
                id,
                name: source.name.clone(),
                area_m2: source.area_m2,
                iy_m4: source.iy_m4,
                iz_m4: source.iz_m4,
                torsional_constant_m4: source.torsional_constant_m4,
            })
        })
        .collect::<Result<Vec<_>, ConfiguratorError>>()?;

    let mut members = Vec::with_capacity(member_count);
    for source in &selected_members {
        if source.end_releases.is_some() {
            return Err(ConfiguratorError::UnsupportedReleases {
                member_id: source.id.to_string(),
            });
        }
        let raw = dense.members.get(&source.id).copied().ok_or_else(|| {
            ConfiguratorError::MissingReference {
                entity: "dense member ID".into(),
                id: source.id.to_string(),
            }
        })?;
        let id = MemberId::new(raw).map_err(|_| dense_id_failure("member", &source.id))?;
        let start = dense_node_id(&dense, &source.start)?;
        let end = dense_node_id(&dense, &source.end)?;
        let material = dense_material_id(&dense, &source.material_id)?;
        let section = dense_section_id(&dense, &source.profile_id)?;
        let start_node = building.nodes.get(&source.start).ok_or_else(|| {
            ConfiguratorError::MissingReference {
                entity: "member start node".into(),
                id: source.start.to_string(),
            }
        })?;
        let end_node =
            building
                .nodes
                .get(&source.end)
                .ok_or_else(|| ConfiguratorError::MissingReference {
                    entity: "member end node".into(),
                    id: source.end.to_string(),
                })?;
        if !valid_orientation(
            start_node.position_m,
            end_node.position_m,
            source.local_y_reference,
        ) {
            return Err(ConfiguratorError::InvalidMemberOrientation {
                member_id: source.id.to_string(),
            });
        }
        members.push(StructuralMember {
            id,
            start,
            end,
            material,
            section,
            formulation: MemberFormulation::Frame3d,
            local_y_reference: Some(source.local_y_reference),
            end_releases: None,
            rigid_offset_m: None,
        });
    }

    let load_case_id = LoadCaseId::new(1).map_err(|_| ConfiguratorError::ResourceLimit {
        reason: "reserved self-weight load case ID 1 is invalid".into(),
    })?;
    let self_weight_members = members.iter().map(|member| member.id).collect();
    let source_id = format!(
        "{}:FrameSelfWeightOnly:{}:{}",
        building.id, building.config_fingerprint, building.graph_fingerprint
    );
    let load_case = StructuralLoadCase {
        id: load_case_id,
        name: "Frame self-weight only".into(),
        kind: LoadCaseKind::Dead,
        normative_status: NormativeStatus::Unverified,
        provenance: format!(
            "FrameSelfWeightOnly; graph_fingerprint={}; panels, snow, and wind excluded; normative status unverified",
            building.graph_fingerprint
        ),
        nodal_loads: Vec::new(),
        uniform_member_loads: Vec::new(),
        self_weight: vec![SelfWeightLoad {
            source_id,
            members: self_weight_members,
        }],
    };

    Ok(StructuralModel {
        space: StructuralSpace::Frame3d,
        nodes,
        materials: structural_materials,
        sections: structural_sections,
        members,
        supports,
        load_cases: vec![load_case],
        combinations: Vec::new(),
    })
}

fn dense_ids_for(
    semantic_ids: &BTreeSet<SemanticId>,
    entity: &str,
    used: &mut BTreeMap<u64, (String, String)>,
) -> Result<BTreeMap<SemanticId, u64>, ConfiguratorError> {
    let mut result = BTreeMap::new();
    for semantic_id in semantic_ids {
        let mut hasher = Sha256::new();
        hasher.update(b"foundation-structural-configurator:dense-id:v1\0");
        hasher.update(entity.as_bytes());
        hasher.update([0]);
        hasher.update(semantic_id.as_str().as_bytes());
        let digest = hasher.finalize();
        let mut prefix = [0u8; 8];
        prefix.copy_from_slice(&digest[..8]);
        let raw = u64::from_be_bytes(prefix);
        if raw == 0 {
            return Err(ConfiguratorError::ResourceLimit {
                reason: format!("SHA-256 dense ID for {entity} {semantic_id} is zero"),
            });
        }
        let label = format!("{entity}:{semantic_id}");
        if let Some((first_entity, first_id)) = used.get(&raw) {
            return Err(ConfiguratorError::SemanticIdHashCollision {
                entity: format!("{first_entity}/{entity}"),
                first: first_id.clone(),
                second: label,
            });
        }
        used.insert(raw, (entity.to_owned(), label));
        result.insert(semantic_id.clone(), raw);
    }
    Ok(result)
}

fn unique_material_ids(
    materials: &[SteelMaterial],
) -> Result<BTreeSet<SemanticId>, ConfiguratorError> {
    let mut ids = BTreeSet::new();
    for material in materials {
        if !ids.insert(material.id.clone()) {
            return Err(binding_mismatch(format!(
                "duplicate generated material ID {}",
                material.id
            )));
        }
    }
    Ok(ids)
}

fn unique_profile_ids(
    profiles: &[MemberProfile],
) -> Result<BTreeSet<SemanticId>, ConfiguratorError> {
    let mut ids = BTreeSet::new();
    for profile in profiles {
        if !ids.insert(profile.id.clone()) {
            return Err(binding_mismatch(format!(
                "duplicate generated profile ID {}",
                profile.id
            )));
        }
    }
    Ok(ids)
}

fn unique_material<'a>(
    building: &'a GeneratedBuilding,
    id: &SemanticId,
) -> Result<&'a SteelMaterial, ConfiguratorError> {
    let mut matches = building
        .materials
        .iter()
        .filter(|material| &material.id == id);
    let Some(material) = matches.next() else {
        return Err(ConfiguratorError::MissingReference {
            entity: "material".into(),
            id: id.to_string(),
        });
    };
    if matches.next().is_some() {
        return Err(binding_mismatch(format!(
            "duplicate material definition {id}"
        )));
    }
    Ok(material)
}

fn unique_profile<'a>(
    building: &'a GeneratedBuilding,
    id: &SemanticId,
) -> Result<&'a MemberProfile, ConfiguratorError> {
    let mut matches = building.profiles.iter().filter(|profile| &profile.id == id);
    let Some(profile) = matches.next() else {
        return Err(ConfiguratorError::MissingReference {
            entity: "profile".into(),
            id: id.to_string(),
        });
    };
    if matches.next().is_some() {
        return Err(binding_mismatch(format!(
            "duplicate profile definition {id}"
        )));
    }
    Ok(profile)
}

fn dense_node_id(dense: &DenseIdMap, id: &SemanticId) -> Result<NodeId, ConfiguratorError> {
    let raw = dense
        .nodes
        .get(id)
        .copied()
        .ok_or_else(|| ConfiguratorError::MissingReference {
            entity: "dense node ID".into(),
            id: id.to_string(),
        })?;
    NodeId::new(raw).map_err(|_| dense_id_failure("node", id))
}

fn dense_material_id(dense: &DenseIdMap, id: &SemanticId) -> Result<MaterialId, ConfiguratorError> {
    let raw =
        dense
            .materials
            .get(id)
            .copied()
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "dense material ID".into(),
                id: id.to_string(),
            })?;
    MaterialId::new(raw).map_err(|_| dense_id_failure("material", id))
}

fn dense_section_id(dense: &DenseIdMap, id: &SemanticId) -> Result<SectionId, ConfiguratorError> {
    let raw =
        dense
            .profiles
            .get(id)
            .copied()
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "dense section ID".into(),
                id: id.to_string(),
            })?;
    SectionId::new(raw).map_err(|_| dense_id_failure("profile", id))
}

fn dense_id_failure(entity: &str, id: &SemanticId) -> ConfiguratorError {
    ConfiguratorError::ResourceLimit {
        reason: format!("dense {entity} ID for {id} is not a positive structural ID"),
    }
}

fn all_section_ids(building: &GeneratedBuilding) -> BTreeSet<SemanticId> {
    let mut ids: BTreeSet<SemanticId> = building.outlines.keys().cloned().collect();
    for node in building.nodes.values() {
        ids.extend(node.owners.iter().cloned());
    }
    for member in building.members.values() {
        ids.extend(member.owners.iter().cloned());
    }
    ids
}

fn check_generation_limits(
    building: &GeneratedBuilding,
    owners: &BTreeSet<SemanticId>,
) -> Result<(), ConfiguratorError> {
    for owner in owners {
        if let Some(reason) = building.generation_limits.get(owner) {
            return Err(ConfiguratorError::UnsupportedGeneration {
                section_id: owner.to_string(),
                reason: reason.clone(),
            });
        }
    }
    Ok(())
}

fn check_unverified_interfaces(
    building: &GeneratedBuilding,
    owners: &BTreeSet<SemanticId>,
) -> Result<(), ConfiguratorError> {
    let interface_ids: BTreeSet<String> = building
        .interfaces
        .iter()
        .filter(|interface| {
            interface.status == InterfaceLoadPathStatus::InterfaceLoadPathNotVerified
                && (owners.contains(&interface.section_a) || owners.contains(&interface.section_b))
        })
        .map(|interface| interface.id.to_string())
        .collect();
    if interface_ids.is_empty() {
        Ok(())
    } else {
        Err(ConfiguratorError::InterfaceLoadPathNotVerified {
            interface_ids: interface_ids.into_iter().collect(),
        })
    }
}

fn connected_component_count(
    building: &GeneratedBuilding,
    owners: &BTreeSet<SemanticId>,
) -> Result<usize, ConfiguratorError> {
    let mut remaining = owners.clone();
    let mut count = 0;
    while let Some(seed) = remaining.iter().next().cloned() {
        let scope = connected_analysis_scope(building, seed.as_str())?;
        let found: BTreeSet<SemanticId> = scope.section_ids.intersection(owners).cloned().collect();
        if found.is_empty() {
            return Err(invalid_scope(format!(
                "no explicit-rigid component contains section {seed}"
            )));
        }
        remaining = remaining.difference(&found).cloned().collect();
        count += 1;
    }
    Ok(count)
}

fn valid_orientation(start: [f64; 3], end: [f64; 3], y_reference: [f64; 3]) -> bool {
    if start
        .iter()
        .chain(end.iter())
        .chain(y_reference.iter())
        .any(|v| !v.is_finite())
    {
        return false;
    }
    let x = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let x_norm = norm(x);
    let y_norm = norm(y_reference);
    if x_norm == 0.0 || y_norm == 0.0 {
        return false;
    }
    let cross = [
        x[1] * y_reference[2] - x[2] * y_reference[1],
        x[2] * y_reference[0] - x[0] * y_reference[2],
        x[0] * y_reference[1] - x[1] * y_reference[0],
    ];
    norm(cross) > 1.0e-12 * x_norm * y_norm
}

fn norm(vector: [f64; 3]) -> f64 {
    vector[0].hypot(vector[1]).hypot(vector[2])
}

fn invalid_scope(reason: impl Into<String>) -> ConfiguratorError {
    ConfiguratorError::InvalidAnalysisScope {
        reason: reason.into(),
    }
}

fn binding_mismatch(message: String) -> ConfiguratorError {
    ConfiguratorError::GeneratedBindingMismatch { message }
}
