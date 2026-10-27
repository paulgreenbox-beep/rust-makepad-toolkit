//! Original configurator parameters and generated records share Document's
//! existing transaction, operation IDs and history. No nested document exists.
use crate::*;
use sha2::{Digest, Sha256};
use structural_configurator::{BuildingConfig, GeneratedBuilding, SemanticId};

pub const STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION: u16 = 6;
pub const STRUCTURAL_CONFIGURATOR_FEATURE: &str = "structural_configurator_v1";
pub const MAX_CONFIGURATOR_DOMAIN_BYTES: usize = 16 << 20;
pub const MAX_CONFIGURATOR_SNAPSHOTS: usize = 16;

// Storage/generation resource admission, deliberately separate from the dense
// solver's unchanged 64-node/128-member admission. The semantic catalog owns
// its exact checked global budget; these cheap checks run before it can expand.
const MAX_CONFIGURATOR_STORED_ITEMS: usize = 100_000;

/// Observations of the last successful explicit generation in this in-memory
/// Document. Undo/Redo may restore another source without changing these facts.
/// This value is never persisted and is not authoritative document state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfiguratorGenerationDiagnostics {
    pub source_operation_id: OperationId,
    pub request_input_fingerprint: String,
    pub generation: structural_configurator::GenerationDiagnostics,
}

#[derive(Debug, Default)]
pub(super) struct ConfiguratorGeneratorRuntime {
    cache: structural_configurator::GeneratorCache,
    last_successful_explicit_generation: Option<ConfiguratorGenerationDiagnostics>,
}
impl Clone for ConfiguratorGeneratorRuntime {
    /// A cloned Document has a separate runtime identity and starts cold.
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// An explicit AllSupportsOfMemberSections assignment: every support owned by
/// the listed sections belongs to this raft. No duplicate per-support records
/// are required. Optional BuildingConfig overrides must agree with the group.
/// Multiple sections may share one foundation only through this named group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationAssignmentGroup {
    pub foundation_id: foundation_core::FoundationId,
    pub member_section_ids: BTreeSet<SemanticId>,
    pub input: foundation_core::RigidMatInput,
    pub reference_point_m: [f64; 3],
}
impl FoundationAssignmentGroup {
    fn validate(&self, building: &BuildingConfig) -> Result<(), CoreError> {
        if self.foundation_id != self.input.foundation_id
            || self.member_section_ids.is_empty()
            || self
                .member_section_ids
                .iter()
                .any(|id| !building.sections.iter().any(|s| &s.id == id))
            || self.reference_point_m.iter().any(|v| !v.is_finite())
        {
            return Err(configurator_error(
                "invalid explicit foundation group identity, ownership or reference point",
            ));
        }
        foundation_core::validate_input(&self.input).map_err(configurator_error)?;
        Ok(())
    }
}

/// Shared demonstration inputs for CLI and native UI. This constructs explicit
/// synthetic assumptions only: no generated graph, solver run or certification.
pub fn configured_fixture_input(
    kind: structural_configurator::FixtureKind,
) -> Result<(BuildingConfig, Vec<FoundationAssignmentGroup>), CoreError> {
    let building = structural_configurator::fixture(kind);
    let fatal: Vec<_> = structural_configurator::validate_config(&building)
        .into_iter()
        .filter(|issue| issue.severity == structural_configurator::IssueSeverity::Error)
        .collect();
    if !fatal.is_empty() {
        return Err(configurator_error(
            structural_configurator::ConfiguratorError::InvalidConfiguration { issues: fatal },
        ));
    }
    let groups = match kind {
        structural_configurator::FixtureKind::SteppedConnected
        | structural_configurator::FixtureKind::EqualHeightConnected => {
            let mut input = foundation_core::RigidMatInput::default();
            input.geometry.width_x_m = 10.0;
            input.geometry.length_y_m = 12.0;
            input.soil = foundation_core::synthetic_scenarios()
                .into_iter()
                .find(|scenario| scenario.id.as_str() == "3")
                .ok_or_else(|| {
                    configurator_error("demonstration synthetic three-layer scenario is missing")
                })?;
            input.self_weight_policy = foundation_core::SelfWeightPolicy::IncludeOnce {
                source_id: "configurator-demo-raft-own-weight".into(),
            };
            vec![FoundationAssignmentGroup {
                foundation_id: input.foundation_id.clone(),
                member_section_ids: building
                    .sections
                    .iter()
                    .map(|section| section.id.clone())
                    .collect(),
                input,
                reference_point_m: [5.0, 6.0, 0.0],
            }]
        }
        structural_configurator::FixtureKind::IndependentJoint
        | structural_configurator::FixtureKind::GeometryOnly => vec![],
    };
    validate_foundation_groups(&building, &group_map(&groups)?)?;
    Ok((building, groups))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredBuildingState {
    pub building: BuildingConfig,
    pub generated: GeneratedBuilding,
    #[serde(deserialize_with = "crate::drafting::unique_map")]
    pub foundation_groups: BTreeMap<foundation_core::FoundationId, FoundationAssignmentGroup>,
    pub source_revision: FoundationStructuralSourceRevision,
    /// Parameters + explicit foundation groups, separate from graph/source identity.
    pub input_fingerprint: String,
}
impl ConfiguredBuildingState {
    pub fn validate(&self) -> Result<(), CoreError> {
        configurator_resource_preflight(self)?;
        if self.source_revision.document_uuid.is_nil()
            || self.source_revision.operation_id.get() == 0
            || self.input_fingerprint
                != configured_input_fingerprint(&self.building, &self.foundation_groups)?
        {
            return Err(configurator_error(
                "invalid configurator source revision/input fingerprint",
            ));
        }
        structural_configurator::validate_generated_binding(&self.building, &self.generated)
            .map_err(configurator_error)?;
        validate_foundation_groups(&self.building, &self.foundation_groups)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralConfiguratorDomain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<ConfiguredBuildingState>,
    #[serde(default, deserialize_with = "crate::drafting::unique_map")]
    snapshots: BTreeMap<EngineeringSnapshotId, ConfiguratorAnalysisSnapshot>,
}
impl StructuralConfiguratorDomain {
    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.snapshots.is_empty()
    }
    pub fn current(&self) -> Option<&ConfiguredBuildingState> {
        self.current.as_ref()
    }
    pub fn snapshots(
        &self,
    ) -> impl DoubleEndedIterator<Item = (EngineeringSnapshotId, &ConfiguratorAnalysisSnapshot)>
    + ExactSizeIterator {
        self.snapshots.iter().map(|(id, value)| (*id, value))
    }
    pub fn snapshot(&self, id: EngineeringSnapshotId) -> Option<&ConfiguratorAnalysisSnapshot> {
        self.snapshots.get(&id)
    }
    pub fn snapshot_status(&self, id: EngineeringSnapshotId) -> Option<EngineeringSnapshotStatus> {
        self.snapshots.get(&id).map(|snapshot| {
            if self.current.as_ref() == Some(&snapshot.source) {
                EngineeringSnapshotStatus::Current
            } else {
                EngineeringSnapshotStatus::Stale
            }
        })
    }
    pub(super) fn high_water(&self) -> u64 {
        self.current
            .iter()
            .map(|s| s.source_revision.operation_id.get())
            .chain(
                self.snapshots
                    .iter()
                    .flat_map(|(id, s)| [id.get(), s.source.source_revision.operation_id.get()]),
            )
            .max()
            .unwrap_or(0)
    }
    pub(super) fn validate(&self, document_uuid: Uuid) -> Result<(), CoreError> {
        if self.snapshots.len() > MAX_CONFIGURATOR_SNAPSHOTS {
            return Err(configurator_error(
                "configurator snapshot count limit exceeded",
            ));
        }
        configurator_bounded(self, MAX_CONFIGURATOR_DOMAIN_BYTES)?;
        for source in self
            .current
            .iter()
            .chain(self.snapshots.values().map(|s| &s.source))
        {
            source.validate()?;
            if source.source_revision.document_uuid != document_uuid {
                return Err(configurator_error(
                    "configurator source belongs to another document",
                ));
            }
        }
        for (id, snapshot) in &self.snapshots {
            if id.get() == 0 || id.get() <= snapshot.source.source_revision.operation_id.get() {
                return Err(configurator_error(
                    "invalid configurator snapshot operation identity",
                ));
            }
            snapshot.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuralConfiguratorCommand {
    UpdateBuildingAndRegenerate {
        building: BuildingConfig,
        foundation_groups: Vec<FoundationAssignmentGroup>,
    },
    SaveAnalysisSnapshot {
        snapshot: ConfiguratorAnalysisSnapshot,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralConfiguratorDelta {
    pub building_changed: bool,
    pub affected_building_ids: Vec<SemanticId>,
    pub affected_section_ids: Vec<SemanticId>,
    pub affected_sections: Vec<ConfiguredSectionRef>,
    /// Source revision invalidation is separate from physical geometry changes.
    pub analysis_invalidated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_delta: Option<structural_configurator::GenerationDelta>,
    pub created_snapshots: Vec<EngineeringSnapshotId>,
    pub removed_snapshots: Vec<EngineeringSnapshotId>,
    pub affected_snapshots: Vec<EngineeringSnapshotId>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredSectionRef {
    pub building_id: SemanticId,
    pub section_id: SemanticId,
}
impl StructuralConfiguratorDelta {
    pub fn is_empty(&self) -> bool {
        !self.building_changed
            && !self.analysis_invalidated
            && self.generation_delta.is_none()
            && self.affected_building_ids.is_empty()
            && self.affected_section_ids.is_empty()
            && self.affected_sections.is_empty()
            && self.affected_snapshots.is_empty()
            && self.created_snapshots.is_empty()
            && self.removed_snapshots.is_empty()
    }
}
fn configured_domain_diff(
    before: &StructuralConfiguratorDomain,
    after: &StructuralConfiguratorDomain,
) -> StructuralConfiguratorDelta {
    let analysis_invalidated = before.current != after.current;
    let building_changed = before
        .current
        .as_ref()
        .map(|s| (&s.building, &s.foundation_groups))
        != after
            .current
            .as_ref()
            .map(|s| (&s.building, &s.foundation_groups));
    let sources: Vec<_> = before.current.iter().chain(after.current.iter()).collect();
    let (generation_delta, changed_sections) =
        generated_geometry_diff(before.current.as_ref(), after.current.as_ref());
    StructuralConfiguratorDelta {
        building_changed,
        affected_building_ids: if analysis_invalidated {
            sources
                .iter()
                .map(|s| s.building.id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            vec![]
        },
        affected_section_ids: changed_sections.iter().cloned().collect(),
        affected_sections: sources
            .iter()
            .flat_map(|s| {
                s.building
                    .sections
                    .iter()
                    .filter(|section| changed_sections.contains(&section.id))
                    .map(|section| ConfiguredSectionRef {
                        building_id: s.building.id.clone(),
                        section_id: section.id.clone(),
                    })
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        analysis_invalidated,
        generation_delta,
        created_snapshots: after
            .snapshots
            .keys()
            .filter(|id| !before.snapshots.contains_key(id))
            .copied()
            .collect(),
        removed_snapshots: before
            .snapshots
            .keys()
            .filter(|id| !after.snapshots.contains_key(id))
            .copied()
            .collect(),
        affected_snapshots: before
            .snapshots
            .keys()
            .chain(after.snapshots.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|id| {
                analysis_invalidated || before.snapshots.get(id) != after.snapshots.get(id)
            })
            .collect(),
    }
}
fn generated_geometry_diff(
    before: Option<&ConfiguredBuildingState>,
    after: Option<&ConfiguredBuildingState>,
) -> (
    Option<structural_configurator::GenerationDelta>,
    BTreeSet<SemanticId>,
) {
    if before == after {
        return (None, BTreeSet::new());
    }
    fn entities(
        source: Option<&ConfiguredBuildingState>,
    ) -> BTreeMap<SemanticId, (String, BTreeSet<SemanticId>)> {
        let Some(source) = source else {
            return BTreeMap::new();
        };
        source
            .generated
            .nodes
            .values()
            .map(|node| {
                (
                    node.id.clone(),
                    (node.fingerprint.clone(), node.owners.clone()),
                )
            })
            .chain(source.generated.members.values().map(|member| {
                (
                    member.id.clone(),
                    (member.fingerprint.clone(), member.owners.clone()),
                )
            }))
            .chain(source.generated.surfaces.values().map(|surface| {
                (
                    surface.id.clone(),
                    (
                        surface.fingerprint.clone(),
                        BTreeSet::from([surface.section_id.clone()]),
                    ),
                )
            }))
            .collect()
    }
    let a = entities(before);
    let b = entities(after);
    let mut delta = structural_configurator::GenerationDelta {
        added: BTreeSet::new(),
        removed: BTreeSet::new(),
        changed: BTreeSet::new(),
        unchanged: BTreeSet::new(),
    };
    let mut sections = BTreeSet::new();
    for id in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        match (a.get(id), b.get(id)) {
            (None, Some(_)) => {
                delta.added.insert(id.clone());
            }
            (Some(_), None) => {
                delta.removed.insert(id.clone());
            }
            (Some(first), Some(last)) if first != last => {
                delta.changed.insert(id.clone());
            }
            _ => {
                delta.unchanged.insert(id.clone());
                continue;
            }
        }
        for (_, owners) in a.get(id).into_iter().chain(b.get(id)) {
            sections.extend(owners.iter().cloned());
        }
    }
    let old_sections: BTreeMap<_, _> = before
        .into_iter()
        .flat_map(|s| {
            s.building
                .sections
                .iter()
                .map(|section| (&section.id, section))
        })
        .collect();
    let new_sections: BTreeMap<_, _> = after
        .into_iter()
        .flat_map(|s| {
            s.building
                .sections
                .iter()
                .map(|section| (&section.id, section))
        })
        .collect();
    for id in old_sections
        .keys()
        .chain(new_sections.keys())
        .collect::<BTreeSet<_>>()
    {
        if old_sections.get(id) != new_sections.get(id) {
            sections.insert((**id).clone());
        }
    }
    if before.map(|s| &s.building.transform) != after.map(|s| &s.building.transform) {
        sections.extend(
            before
                .into_iter()
                .chain(after)
                .flat_map(|s| s.building.sections.iter().map(|section| section.id.clone())),
        );
    }
    (Some(delta), sections)
}
pub(super) fn summarize_configurator(changes: &[Change]) -> StructuralConfiguratorDelta {
    let mut entries = changes.iter().filter_map(|change| {
        if let Change::StructuralConfigurator { before, after } = change {
            Some((before, after))
        } else {
            None
        }
    });
    let Some((before, mut after)) = entries.next() else {
        return StructuralConfiguratorDelta::default();
    };
    for (_, last) in entries {
        after = last;
    }
    configured_domain_diff(before, after)
}
pub(super) fn configurator_projection(
    domain: &StructuralConfiguratorDomain,
    changes: Option<&[Change]>,
) -> Option<StructuralConfiguratorDomain> {
    if changes.is_some_and(|changes| {
        !changes
            .iter()
            .any(|change| matches!(change, Change::StructuralConfigurator { .. }))
    }) {
        return None;
    }
    if changes.is_none() && domain.is_empty() {
        None
    } else {
        Some(domain.clone())
    }
}

impl Document {
    pub fn structural_configurator(&self) -> &StructuralConfiguratorDomain {
        &self.structural_configurator
    }
    /// Last successful explicit generation in this runtime, not a claim about
    /// the current revision after Undo/Redo. Open, replay and Clone start cold.
    pub fn configurator_generation_diagnostics(
        &self,
    ) -> Option<&ConfiguratorGenerationDiagnostics> {
        self.configurator_generator_runtime
            .last_successful_explicit_generation
            .as_ref()
    }
    pub(super) fn apply_configurator_command(
        &mut self,
        command: StructuralConfiguratorCommand,
        operation_id: OperationId,
    ) -> Result<bim::Applied, CoreError> {
        let before = self.structural_configurator.clone();
        let mut after = before.clone();
        let mut generation_diagnostics = None;
        match &command {
            StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
                building,
                foundation_groups,
            } => {
                let groups = group_map(foundation_groups)?;
                validate_foundation_groups(building, &groups)?;
                // This is the sole generator call: an explicit ordinary command.
                let (generated, diagnostics) =
                    structural_configurator::generate_building_with_cache(
                        building,
                        &mut self.configurator_generator_runtime.cache,
                    )
                    .map_err(configurator_error)?;
                let current = ConfiguredBuildingState {
                    building: building.clone(),
                    generated,
                    foundation_groups: groups.clone(),
                    source_revision: FoundationStructuralSourceRevision {
                        document_uuid: self.document_uuid,
                        operation_id,
                    },
                    input_fingerprint: configured_input_fingerprint(building, &groups)?,
                };
                current.validate()?;
                generation_diagnostics = Some(ConfiguratorGenerationDiagnostics {
                    source_operation_id: operation_id,
                    request_input_fingerprint: current.input_fingerprint.clone(),
                    generation: diagnostics,
                });
                after.current = Some(current);
            }
            StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot } => {
                snapshot.validate()?;
                if before.current.as_ref() != Some(&snapshot.source) {
                    return Err(configurator_error(
                        "configurator result is stale or has no current generated source",
                    ));
                }
                if after
                    .snapshots
                    .insert(
                        EngineeringSnapshotId::new(operation_id.get()),
                        snapshot.clone(),
                    )
                    .is_some()
                {
                    return Err(configurator_error(
                        "immutable configurator snapshot identity already exists",
                    ));
                }
            }
        }
        after.validate(self.document_uuid)?;
        self.structural_configurator = after.clone();
        if let Some(diagnostics) = generation_diagnostics {
            self.configurator_generator_runtime
                .last_successful_explicit_generation = Some(diagnostics);
        }
        Ok((
            OperationKind::StructuralConfiguratorChanged { command },
            None,
            vec![],
            vec![],
            "Parametric configured building".into(),
            vec![Change::StructuralConfigurator { before, after }],
            vec![],
        ))
    }
    pub(super) fn has_configurator_history(&self) -> bool {
        let change = |c: &Change| {
            matches!(c, Change::StructuralConfigurator { .. })
                || matches!(c, Change::Operation { before, after, .. } if before.iter().chain(after).any(|o| matches!(o.kind, OperationKind::StructuralConfiguratorChanged { .. })))
        };
        !self.structural_configurator.is_empty()
            || self
                .operations
                .values()
                .any(|o| matches!(o.kind, OperationKind::StructuralConfiguratorChanged { .. }))
            || self.revision_states.values().any(|s| {
                s.structural_configurator.is_some()
                    || s.delta
                        .as_ref()
                        .is_some_and(|d| d.changes.iter().any(&change))
            })
            || self
                .undo_stack
                .iter()
                .chain(&self.redo_stack)
                .any(|s| s.changes.iter().any(&change))
            || self.command_receipts.values().any(|r| {
                r.recorded_commands.as_ref().is_some_and(|cs| {
                    cs.iter()
                        .any(|c| matches!(c, Command::StructuralConfigurator(_)))
                })
            })
    }
    pub(super) fn validate_configurator(&self) -> Result<(), CoreError> {
        self.structural_configurator.validate(self.document_uuid)?;
        if self.next_operation_id <= self.structural_configurator.high_water() {
            return Err(configurator_error(
                "configurator operation counter loses retained high-water mark",
            ));
        }
        for source in self.structural_configurator.current.iter().chain(
            self.structural_configurator
                .snapshots
                .values()
                .map(|s| &s.source),
        ) {
            let operation = self
                .operations
                .get(&source.source_revision.operation_id)
                .ok_or_else(|| configurator_error("missing generated building source operation"))?;
            if !matches!(&operation.kind, OperationKind::StructuralConfiguratorChanged { command: StructuralConfiguratorCommand::UpdateBuildingAndRegenerate { building, foundation_groups } } if building == &source.building && group_map(foundation_groups).is_ok_and(|groups| groups == source.foundation_groups))
            {
                return Err(configurator_error(
                    "generated building disagrees with its source operation",
                ));
            }
        }
        for (id, snapshot) in &self.structural_configurator.snapshots {
            let operation = self
                .operations
                .get(&OperationId(id.get()))
                .ok_or_else(|| configurator_error("missing configurator save operation"))?;
            if !matches!(&operation.kind, OperationKind::StructuralConfiguratorChanged { command: StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot: recorded } } if recorded == snapshot)
            {
                return Err(configurator_error(
                    "configurator archive disagrees with its save operation",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn validate_local_configurator(&self, changes: &[Change]) -> Result<(), CoreError> {
        if changes
            .iter()
            .any(|c| matches!(c, Change::StructuralConfigurator { .. }))
        {
            for c in changes {
                if let Change::StructuralConfigurator { before, after } = c {
                    before.validate(self.document_uuid)?;
                    after.validate(self.document_uuid)?;
                }
            }
            self.validate_configurator()?;
        }
        Ok(())
    }
    pub(super) fn configurator_diff(
        &self,
        from: u64,
        to: u64,
    ) -> Result<StructuralConfiguratorDelta, CoreError> {
        let a = self
            .reconstruct_revision_state(from)?
            .structural_configurator
            .unwrap_or_default();
        let b = self
            .reconstruct_revision_state(to)?
            .structural_configurator
            .unwrap_or_default();
        Ok(configured_domain_diff(&a, &b))
    }
}

pub(super) fn configurator_effects_match(
    command: &StructuralConfiguratorCommand,
    changes: &[Change],
    operation: &OperationRecord,
) -> bool {
    let [Change::StructuralConfigurator { before, after }] = changes else {
        return false;
    };
    match command {
        StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
            building,
            foundation_groups,
        } => {
            after.snapshots == before.snapshots
                && after.current.as_ref().is_some_and(|source| {
                    &source.building == building
                        && source.source_revision.operation_id == operation.id
                        && group_map(foundation_groups)
                            .is_ok_and(|groups| groups == source.foundation_groups)
                        && source.validate().is_ok()
                })
        }
        StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot } => {
            if before.current.as_ref() != Some(&snapshot.source)
                || before.current != after.current
                || snapshot.validate().is_err()
            {
                return false;
            }
            let mut expected = before.snapshots.clone();
            expected
                .insert(
                    EngineeringSnapshotId::new(operation.id.get()),
                    snapshot.clone(),
                )
                .is_none()
                && expected == after.snapshots
        }
    }
}
fn group_map(
    groups: &[FoundationAssignmentGroup],
) -> Result<BTreeMap<foundation_core::FoundationId, FoundationAssignmentGroup>, CoreError> {
    let mut result = BTreeMap::new();
    for group in groups {
        if result
            .insert(group.foundation_id.clone(), group.clone())
            .is_some()
        {
            return Err(configurator_error(
                "duplicate explicit foundation group identity",
            ));
        }
    }
    Ok(result)
}
fn validate_foundation_groups(
    building: &BuildingConfig,
    groups: &BTreeMap<foundation_core::FoundationId, FoundationAssignmentGroup>,
) -> Result<(), CoreError> {
    let mut section_owners = BTreeSet::new();
    for (id, group) in groups {
        if id != &group.foundation_id {
            return Err(configurator_error(
                "foundation group map key/identity mismatch",
            ));
        }
        group.validate(building)?;
        for section in &group.member_section_ids {
            if !section_owners.insert(section) {
                return Err(configurator_error(
                    "a section cannot be silently assigned to multiple independent foundation groups",
                ));
            }
        }
    }
    for assignment in &building.foundation_assignments {
        if let Some(group) = groups
            .values()
            .find(|group| group.member_section_ids.contains(&assignment.section_id))
        {
            if assignment.foundation_id.as_str() != group.foundation_id.as_str() {
                return Err(configurator_error(
                    "support assignment disagrees with the explicit section foundation group",
                ));
            }
        }
    }
    Ok(())
}
fn configured_input_fingerprint(
    building: &BuildingConfig,
    groups: &BTreeMap<foundation_core::FoundationId, FoundationAssignmentGroup>,
) -> Result<String, CoreError> {
    configurator_fingerprint(&(building, groups))
}
pub(super) fn configurator_fingerprint(
    value: &(impl Serialize + ?Sized),
) -> Result<String, CoreError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub(super) fn configurator_error(error: impl fmt::Display) -> CoreError {
    CoreError::InvalidDocument(format!("structural configurator: {error}"))
}
pub(super) fn configurator_bounded(
    value: &(impl Serialize + ?Sized),
    limit: usize,
) -> Result<(), CoreError> {
    circular_archive::bounded_serialized_size(value, limit)
        .map(|_| ())
        .map_err(configurator_error)
}

fn configurator_resource_limit(reason: impl Into<String>) -> CoreError {
    configurator_error(structural_configurator::ConfiguratorError::ResourceLimit {
        reason: reason.into(),
    })
}

/// Allocation-free collection admission and bounded streaming serialization
/// precede hashing and parameter-derived geometry catalogs on every entry path.
pub(super) fn configurator_resource_preflight(
    source: &ConfiguredBuildingState,
) -> Result<(), CoreError> {
    let b = &source.building;
    let g = &source.generated;
    if b.sections.len() > 64
        || g.outlines.len() > 64
        || g.generation_limits.len() > 64
        || source.foundation_groups.len() > 64
    {
        return Err(configurator_resource_limit(
            "stored section/foundation group count exceeds 64",
        ));
    }
    if b.interfaces.len() > 256 || g.interfaces.len() > 256 {
        return Err(configurator_resource_limit(
            "stored interface count exceeds 256",
        ));
    }
    if [
        g.nodes.len(),
        g.members.len(),
        g.surfaces.len(),
        g.reaction_routing.len(),
        g.bim_quantities.steel.len(),
        b.foundation_assignments.len(),
        b.materials.len(),
        b.profiles.len(),
    ]
    .into_iter()
    .any(|count| count > MAX_CONFIGURATOR_STORED_ITEMS)
    {
        return Err(configurator_resource_limit(
            "stored generated collection exceeds the 100000-item resource budget",
        ));
    }
    if b.sections.iter().any(|section| section.corners.len() > 256) {
        return Err(configurator_resource_limit(
            "stored section outline exceeds 256 corners",
        ));
    }
    configurator_bounded(source, MAX_CONFIGURATOR_DOMAIN_BYTES)?;
    for section in &b.sections {
        // Unsupported shapes/systems remain persistable with explicit holds.
        // This is only a lower-bound rejection for a supported rectangular
        // grid; exact merged/secondary-node catalog admission stays in Tech.
        if section.structural_system != structural_configurator::StructuralSystem::SteelFrame
            || !matches!(
                section.roof.kind,
                structural_configurator::RoofKind::Flat
                    | structural_configurator::RoofKind::MonoPitch
                    | structural_configurator::RoofKind::Gable
            )
        {
            continue;
        }
        let bounds = structural_configurator::geometry::bounds(section);
        if !structural_configurator::geometry::is_rectangle(section, &bounds) {
            continue;
        }
        let columns =
            grid_count_lower_bound(&section.grid.column_stations, bounds.u_max - bounds.u_min)?;
        let frames =
            grid_count_lower_bound(&section.grid.frame_stations, bounds.v_max - bounds.v_min)?;
        if columns
            .checked_mul(frames)
            .is_none_or(|product| product > MAX_CONFIGURATOR_STORED_ITEMS)
        {
            return Err(configurator_resource_limit(format!(
                "section {} station Cartesian product exceeds the 100000-item catalog resource budget",
                section.id
            )));
        }
    }
    Ok(())
}

fn grid_count_lower_bound(
    spacing: &structural_configurator::GridSpacing,
    extent: f64,
) -> Result<usize, CoreError> {
    let count = match spacing {
        structural_configurator::GridSpacing::Explicit { stations } => stations.len(),
        structural_configurator::GridSpacing::Uniform { pitch_m } => {
            // Invalid geometry is diagnosed by the ordinary semantic validator.
            // Valid tiny pitches must not allocate a station vector to be refused.
            if !extent.is_finite() || extent <= 0.0 || !pitch_m.is_finite() || *pitch_m <= 0.0 {
                return Ok(0);
            }
            let ratio = extent / pitch_m;
            if !ratio.is_finite() || ratio > MAX_CONFIGURATOR_STORED_ITEMS as f64 {
                return Err(configurator_resource_limit(
                    "uniform grid axis exceeds the 100000-item resource budget",
                ));
            }
            // Floor is a conservative lower bound even near an endpoint's
            // merge tolerance; this preflight cannot reject a smaller valid grid.
            (ratio.floor() as usize).max(2)
        }
    };
    if count > MAX_CONFIGURATOR_STORED_ITEMS {
        return Err(configurator_resource_limit(
            "explicit grid axis exceeds the 100000-item resource budget",
        ));
    }
    Ok(count)
}
