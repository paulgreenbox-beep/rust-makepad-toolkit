//! Explicit component analysis and source-bound archives. Validation performs
//! graph projection and algebra only; it never invokes a generator or solver.
use crate::*;
use configured_building::{configurator_error, configurator_fingerprint};
use structural_configurator::{ConnectedAnalysisScope, DenseIdMap, InterfaceKind, SemanticId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "scope",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum ConfiguratorAnalysisRequest {
    WholeBuilding,
    ConnectedComponent { seed_section_id: SemanticId },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum ConfiguratorAnalysisReadiness {
    Ready {
        scope: ConnectedAnalysisScope,
        nodes: usize,
        members: usize,
    },
    AnalysisModelCapacityExceeded {
        scope: ConnectedAnalysisScope,
        nodes: usize,
        members: usize,
        max_nodes: usize,
        max_members: usize,
    },
    InterfaceLoadPathNotVerified {
        interface_ids: Vec<SemanticId>,
    },
    GenerationBlocked {
        section_ids: Vec<SemanticId>,
    },
    DisconnectedLoadPathsNotAcknowledged,
}

/// Derive closure from authoritative parameters, never from a supplied set of
/// IDs or a potentially forged generated interface status.
pub fn configured_analysis_readiness(
    source: &ConfiguredBuildingState,
    request: &ConfiguratorAnalysisRequest,
) -> Result<ConfiguratorAnalysisReadiness, CoreError> {
    source.validate()?;
    let all: BTreeSet<_> = source
        .building
        .sections
        .iter()
        .map(|s| s.id.clone())
        .collect();
    let selected = match request {
        ConfiguratorAnalysisRequest::WholeBuilding => all.clone(),
        ConfiguratorAnalysisRequest::ConnectedComponent { seed_section_id } => {
            if !all.contains(seed_section_id) {
                return Err(configurator_error(
                    "unknown connected-component seed section",
                ));
            }
            authoritative_component(source, seed_section_id)
        }
    };
    let unknown: Vec<_> = source
        .building
        .interfaces
        .iter()
        .filter(|interface| {
            (selected.contains(&interface.section_a) || selected.contains(&interface.section_b))
                && matches!(
                    interface.kind,
                    InterfaceKind::GeometryOnlyContact | InterfaceKind::NotDefined
                )
        })
        .map(|interface| interface.id.clone())
        .collect();
    if !unknown.is_empty() {
        return Ok(
            ConfiguratorAnalysisReadiness::InterfaceLoadPathNotVerified {
                interface_ids: unknown,
            },
        );
    }
    if matches!(request, ConfiguratorAnalysisRequest::WholeBuilding)
        && !source.building.allow_disconnected_analysis
    {
        if let Some(seed) = all.iter().next() {
            if authoritative_component(source, seed) != all {
                return Ok(ConfiguratorAnalysisReadiness::DisconnectedLoadPathsNotAcknowledged);
            }
        }
    }
    let blocked: Vec<_> = selected
        .iter()
        .filter(|id| {
            source.generated.generation_limits.contains_key(*id)
                || source
                    .generated
                    .outlines
                    .get(*id)
                    .is_none_or(|outline| outline.generation_limit.is_some())
        })
        .cloned()
        .collect();
    if !blocked.is_empty() {
        return Ok(ConfiguratorAnalysisReadiness::GenerationBlocked {
            section_ids: blocked,
        });
    }
    let nodes = source
        .generated
        .nodes
        .values()
        .filter(|node| !node.owners.is_disjoint(&selected))
        .count();
    let members = source
        .generated
        .members
        .values()
        .filter(|member| !member.owners.is_disjoint(&selected))
        .count();
    let scope = ConnectedAnalysisScope {
        section_ids: selected,
    };
    if nodes > structural_core::MAX_NODES || members > structural_core::MAX_MEMBERS {
        return Ok(
            ConfiguratorAnalysisReadiness::AnalysisModelCapacityExceeded {
                scope,
                nodes,
                members,
                max_nodes: structural_core::MAX_NODES,
                max_members: structural_core::MAX_MEMBERS,
            },
        );
    }
    Ok(ConfiguratorAnalysisReadiness::Ready {
        scope,
        nodes,
        members,
    })
}
fn authoritative_component(
    source: &ConfiguredBuildingState,
    seed: &SemanticId,
) -> BTreeSet<SemanticId> {
    let mut selected = BTreeSet::from([seed.clone()]);
    loop {
        let before = selected.len();
        for interface in &source.building.interfaces {
            if matches!(interface.kind, InterfaceKind::StructurallyConnected { .. })
                && (selected.contains(&interface.section_a)
                    || selected.contains(&interface.section_b))
            {
                selected.insert(interface.section_a.clone());
                selected.insert(interface.section_b.clone());
            }
        }
        if selected.len() == before {
            break;
        }
    }
    selected
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredAnalysisInput {
    pub scope: ConnectedAnalysisScope,
    pub dense_ids: DenseIdMap,
    pub input: FoundationStructuralInputState,
}
pub fn configured_analysis_input(
    source: &ConfiguredBuildingState,
    request: &ConfiguratorAnalysisRequest,
) -> Result<ConfiguredAnalysisInput, CoreError> {
    let readiness = configured_analysis_readiness(source, request)?;
    let ConfiguratorAnalysisReadiness::Ready {
        scope,
        nodes,
        members,
    } = readiness
    else {
        return Err(configurator_error(format!(
            "analysis is blocked: {readiness:?}"
        )));
    };
    let model = match request {
        ConfiguratorAnalysisRequest::WholeBuilding => {
            structural_configurator::to_structural_model(&source.generated)
        }
        ConfiguratorAnalysisRequest::ConnectedComponent { .. } => {
            structural_configurator::to_structural_model_for_scope(&source.generated, &scope)
        }
    }
    .map_err(configurator_error)?;
    if model.nodes.len() != nodes || model.members.len() != members {
        return Err(configurator_error(
            "dense projection omits physical component nodes or members",
        ));
    }
    let dense_ids =
        structural_configurator::dense_id_map(&source.generated).map_err(configurator_error)?;
    let target = if let Some(combination) = model.combinations.first() {
        structural_core::AnalysisTarget::Combination(combination.id)
    } else {
        structural_core::AnalysisTarget::Case(
            model
                .load_cases
                .first()
                .ok_or_else(|| configurator_error("projected component has no explicit load case"))?
                .id,
        )
    };
    let definition = FoundationStructuralInput {
        structural_model: model,
        analysis_target: target,
        foundation: None,
        foundation_load_source: FoundationLoadSource::StructuralReactions,
        foundation_reference_point_m: [0.0; 3],
    };
    definition.validate()?;
    validate_dense_mapping(source, &scope, &dense_ids, &definition.structural_model)?;
    let input = FoundationStructuralInputState {
        input_fingerprint: definition.fingerprint()?,
        definition,
        source_revision: source.source_revision.clone(),
    };
    Ok(ConfiguredAnalysisInput {
        scope,
        dense_ids,
        input,
    })
}
fn validate_dense_mapping(
    source: &ConfiguredBuildingState,
    scope: &ConnectedAnalysisScope,
    ids: &DenseIdMap,
    model: &structural_core::StructuralModel,
) -> Result<(), CoreError> {
    let node_numbers: BTreeSet<_> = ids.nodes.values().copied().collect();
    let member_numbers: BTreeSet<_> = ids.members.values().copied().collect();
    if node_numbers.len() != ids.nodes.len()
        || node_numbers.contains(&0)
        || member_numbers.len() != ids.members.len()
        || member_numbers.contains(&0)
    {
        return Err(configurator_error(
            "dense map is not a positive identity bijection",
        ));
    }
    for node in source
        .generated
        .nodes
        .values()
        .filter(|node| !node.owners.is_disjoint(&scope.section_ids))
    {
        let number = ids
            .nodes
            .get(&node.id)
            .ok_or_else(|| configurator_error("dense map omits selected physical node"))?;
        if !model
            .nodes
            .iter()
            .any(|n| n.id.get() == *number && n.position_m == node.position_m)
        {
            return Err(configurator_error(
                "dense node identity/position differs from frozen generated graph",
            ));
        }
    }
    for member in source
        .generated
        .members
        .values()
        .filter(|member| !member.owners.is_disjoint(&scope.section_ids))
    {
        let number = ids
            .members
            .get(&member.id)
            .ok_or_else(|| configurator_error("dense map omits selected physical member"))?;
        if !model.members.iter().any(|m| {
            m.id.get() == *number
                && ids.nodes.get(&member.start) == Some(&m.start.get())
                && ids.nodes.get(&member.end) == Some(&m.end.get())
        }) {
            return Err(configurator_error(
                "dense member identity/endpoints differ from frozen generated graph",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "ownership",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum ConfiguredReactionOwnership {
    Exclusive {
        section_id: SemanticId,
    },
    /// Unallocated between sections. An explicit common foundation group may
    /// still receive this one physical action, once, without allocating it twice.
    SharedUnallocated,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredSupportReaction {
    pub support_node_id: SemanticId,
    pub section_owners: BTreeSet<SemanticId>,
    pub ownership: ConfiguredReactionOwnership,
    pub foundation_group_id: Option<foundation_core::FoundationId>,
    pub position_m: [f64; 3],
    /// Support-on-structure, global RH z-up [Fx,Fy,Fz,Mx,My,Mz].
    pub force_moment: [f64; 6],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguratorReactionLedger {
    pub reactions: Vec<ConfiguredSupportReaction>,
    /// Exclusive reactions only. Moments include r×F about the global origin.
    pub exclusive_section_totals: BTreeMap<SemanticId, [f64; 6]>,
    pub shared_unallocated_total: [f64; 6],
    pub global_total: [f64; 6],
}
fn reaction_ledger(
    source: &ConfiguredBuildingState,
    plan: &ConfiguredAnalysisInput,
    result: &structural_core::StructuralAnalysisResult,
) -> Result<ConfiguratorReactionLedger, CoreError> {
    structural_core::validate_result_binding(
        &plan.input.definition.structural_model,
        plan.input.definition.analysis_target,
        result,
    )
    .map_err(configurator_error)?;
    let reverse: BTreeMap<_, _> = plan
        .dense_ids
        .nodes
        .iter()
        .map(|(id, number)| (*number, id))
        .collect();
    let mut ledger = ConfiguratorReactionLedger {
        reactions: vec![],
        exclusive_section_totals: BTreeMap::new(),
        shared_unallocated_total: [0.0; 6],
        global_total: [0.0; 6],
    };
    let mut seen = BTreeSet::new();
    for reaction in &result.reactions {
        let id = reverse
            .get(&reaction.node.get())
            .ok_or_else(|| configurator_error("reaction has no physical semantic node identity"))?;
        if !seen.insert((*id).clone()) {
            return Err(configurator_error("physical support reaction is repeated"));
        }
        let node = source
            .generated
            .nodes
            .get(*id)
            .ok_or_else(|| configurator_error("reaction source node is absent"))?;
        if node.owners.is_empty() || !node.owners.is_subset(&plan.scope.section_ids) {
            return Err(configurator_error(
                "reaction owners extend outside complete connected scope",
            ));
        }
        let group = source.foundation_groups.values().find(|group| {
            node.owners.is_subset(&group.member_section_ids)
                && node.owners.iter().all(|owner| {
                    source
                        .building
                        .foundation_assignments
                        .iter()
                        .filter(|a| &a.section_id == owner && &a.support_node_id == *id)
                        .all(|a| a.foundation_id.as_str() == group.foundation_id.as_str())
                })
        });
        let ownership = if node.owners.len() == 1 {
            ConfiguredReactionOwnership::Exclusive {
                section_id: node.owners.iter().next().unwrap().clone(),
            }
        } else {
            ConfiguredReactionOwnership::SharedUnallocated
        };
        let total = reaction_about_origin(reaction.position_m, reaction.force_moment);
        add_six(&mut ledger.global_total, total);
        match &ownership {
            ConfiguredReactionOwnership::Exclusive { section_id } => add_six(
                ledger
                    .exclusive_section_totals
                    .entry(section_id.clone())
                    .or_insert([0.0; 6]),
                total,
            ),
            ConfiguredReactionOwnership::SharedUnallocated => {
                add_six(&mut ledger.shared_unallocated_total, total)
            }
        }
        ledger.reactions.push(ConfiguredSupportReaction {
            support_node_id: (*id).clone(),
            section_owners: node.owners.clone(),
            ownership,
            foundation_group_id: group.map(|g| g.foundation_id.clone()),
            position_m: reaction.position_m,
            force_moment: reaction.force_moment,
        });
    }
    ledger
        .reactions
        .sort_by(|a, b| a.support_node_id.cmp(&b.support_node_id));
    Ok(ledger)
}
fn add_six(total: &mut [f64; 6], value: [f64; 6]) {
    for (sum, value) in total.iter_mut().zip(value) {
        *sum += value;
    }
}
fn reaction_about_origin([x, y, z]: [f64; 3], f: [f64; 6]) -> [f64; 6] {
    [
        f[0],
        f[1],
        f[2],
        f[3] + y * f[2] - z * f[1],
        f[4] + z * f[0] - x * f[2],
        f[5] + x * f[1] - y * f[0],
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfiguredFoundationStatus {
    Computed,
    IncompleteGroupScope,
    SharedReactionsUnallocated,
    NoAssignedSupports,
    CalculationFailed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredFoundationCalculation {
    pub foundation_id: foundation_core::FoundationId,
    pub support_node_ids: BTreeSet<SemanticId>,
    pub status: ConfiguredFoundationStatus,
    pub effective_input: Option<foundation_core::RigidMatInput>,
    pub result: Option<foundation_core::RigidMatResult>,
    pub failure: Option<foundation_core::FoundationError>,
}
fn foundation_plan(
    source: &ConfiguredBuildingState,
    scope: &ConnectedAnalysisScope,
    ledger: &ConfiguratorReactionLedger,
    group: &FoundationAssignmentGroup,
) -> Result<
    (
        ConfiguredFoundationStatus,
        BTreeSet<SemanticId>,
        Option<foundation_core::RigidMatInput>,
    ),
    CoreError,
> {
    if !group.member_section_ids.is_subset(&scope.section_ids) {
        return Ok((
            ConfiguredFoundationStatus::IncompleteGroupScope,
            BTreeSet::new(),
            None,
        ));
    }
    let relevant: Vec<_> = ledger
        .reactions
        .iter()
        .filter(|r| !r.section_owners.is_disjoint(&group.member_section_ids))
        .collect();
    if relevant
        .iter()
        .any(|r| r.foundation_group_id.as_ref() != Some(&group.foundation_id))
    {
        return Ok((
            ConfiguredFoundationStatus::SharedReactionsUnallocated,
            BTreeSet::new(),
            None,
        ));
    }
    if relevant.is_empty() {
        return Ok((
            ConfiguredFoundationStatus::NoAssignedSupports,
            BTreeSet::new(),
            None,
        ));
    }
    let support_node_ids = relevant.iter().map(|r| r.support_node_id.clone()).collect();
    let actions = relevant
        .into_iter()
        .map(|reaction| {
            let f = reaction.force_moment;
            foundation_core::LocatedStructuralReaction {
                source_id: format!("configured-support:{}", reaction.support_node_id),
                location_m: std::array::from_fn(|axis| {
                    reaction.position_m[axis] - group.reference_point_m[axis]
                }),
                external_force_kn: [-f[0], -f[1], -f[2]],
                external_free_moment_knm: [-f[3], -f[4], -f[5]],
            }
        })
        .collect();
    let mut input = group.input.clone();
    input.load =
        foundation_core::load_from_structural_reactions(actions).map_err(configurator_error)?;
    foundation_core::validate_input(&input).map_err(configurator_error)?;
    let _ = source; // Source ownership is explicit through this frozen group's membership.
    Ok((
        ConfiguredFoundationStatus::Computed,
        support_node_ids,
        Some(input),
    ))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguratorAnalysisSnapshot {
    pub schema_version: u16,
    pub source: ConfiguredBuildingState,
    pub request: ConfiguratorAnalysisRequest,
    pub scope: ConnectedAnalysisScope,
    pub dense_ids: DenseIdMap,
    pub structural: FoundationStructuralSnapshot,
    pub reactions: ConfiguratorReactionLedger,
    pub foundations: Vec<ConfiguredFoundationCalculation>,
    pub content_fingerprint: String,
}
impl ConfiguratorAnalysisSnapshot {
    fn expected_fingerprint(&self) -> Result<String, CoreError> {
        configurator_fingerprint(&(
            self.schema_version,
            &self.source,
            &self.request,
            &self.scope,
            &self.dense_ids,
            &self.structural,
            &self.reactions,
            &self.foundations,
        ))
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        configured_building::configurator_bounded(self, MAX_CONFIGURATOR_DOMAIN_BYTES)?;
        configured_building::configurator_resource_preflight(&self.source)?;
        if self.schema_version != 1 || self.content_fingerprint != self.expected_fingerprint()? {
            return Err(configurator_error(
                "invalid configurator archive schema/content fingerprint",
            ));
        }
        self.source.validate()?;
        let plan = configured_analysis_input(&self.source, &self.request)?;
        if self.scope != plan.scope
            || self.dense_ids != plan.dense_ids
            || self.structural.input != plan.input
        {
            return Err(configurator_error(
                "archive scope/model/IDs differ from complete frozen component",
            ));
        }
        self.structural.validate()?;
        let reactions = reaction_ledger(&self.source, &plan, &self.structural.structural_result)?;
        if self.reactions != reactions {
            return Err(configurator_error(
                "archive reaction ownership, group routing or exclusive/shared totals disagree",
            ));
        }
        let groups: Vec<_> = self
            .source
            .foundation_groups
            .values()
            .filter(|g| !g.member_section_ids.is_disjoint(&plan.scope.section_ids))
            .collect();
        if groups.len() != self.foundations.len() {
            return Err(configurator_error(
                "archive omits/repeats a foundation group outcome",
            ));
        }
        for (group, stored) in groups.into_iter().zip(&self.foundations) {
            let (status, ids, input) =
                foundation_plan(&self.source, &plan.scope, &reactions, group)?;
            if group.foundation_id != stored.foundation_id
                || ids != stored.support_node_ids
                || input != stored.effective_input
            {
                return Err(configurator_error(
                    "archive foundation demand/identity does not match coherent explicit group",
                ));
            }
            if status == ConfiguredFoundationStatus::Computed {
                match stored.status {
                    ConfiguredFoundationStatus::Computed => {
                        if stored.failure.is_some() {
                            return Err(configurator_error(
                                "computed foundation cannot also claim a failure",
                            ));
                        }
                        foundation_core::validate_result_binding(
                            input.as_ref().unwrap(),
                            stored.result.as_ref().ok_or_else(|| {
                                configurator_error("computed foundation has no result")
                            })?,
                        )
                        .map_err(configurator_error)?;
                    }
                    ConfiguredFoundationStatus::CalculationFailed
                        if stored.result.is_none() && stored.failure.is_some() => {}
                    _ => return Err(configurator_error("invalid foundation calculation outcome")),
                }
            } else if status != stored.status || stored.result.is_some() || stored.failure.is_some()
            {
                return Err(configurator_error(
                    "held/unallocated foundation cannot claim a computed result",
                ));
            }
        }
        Ok(())
    }
}

/// Explicit Analyze only. Neither source edits, Open nor Undo/Redo call this.
pub fn analyze_configured_building(
    source: &ConfiguredBuildingState,
    request: ConfiguratorAnalysisRequest,
    producer: EngineeringSourceIdentity,
) -> Result<ConfiguratorAnalysisSnapshot, CoreError> {
    let plan = configured_analysis_input(source, &request)?;
    let structural = analyze_foundation_structural(&plan.input, producer)?;
    let reactions = reaction_ledger(source, &plan, &structural.structural_result)?;
    let mut foundations = Vec::new();
    for group in source.foundation_groups.values().filter(|group| {
        !group
            .member_section_ids
            .is_disjoint(&plan.scope.section_ids)
    }) {
        let (mut status, support_node_ids, effective_input) =
            foundation_plan(source, &plan.scope, &reactions, group)?;
        let mut result = None;
        let mut failure = None;
        if let Some(input) = &effective_input {
            match foundation_core::solve(input) {
                Ok(value) => result = Some(value),
                Err(error) => {
                    status = ConfiguredFoundationStatus::CalculationFailed;
                    failure = Some(error);
                }
            }
        }
        foundations.push(ConfiguredFoundationCalculation {
            foundation_id: group.foundation_id.clone(),
            support_node_ids,
            status,
            effective_input,
            result,
            failure,
        });
    }
    let mut snapshot = ConfiguratorAnalysisSnapshot {
        schema_version: 1,
        source: source.clone(),
        request,
        scope: plan.scope,
        dense_ids: plan.dense_ids,
        structural,
        reactions,
        foundations,
        content_fingerprint: String::new(),
    };
    snapshot.content_fingerprint = snapshot.expected_fingerprint()?;
    snapshot.validate()?;
    Ok(snapshot)
}
