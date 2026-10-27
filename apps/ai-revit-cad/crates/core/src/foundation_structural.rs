//! Typed structural/foundation state belongs to the existing Document authority.
//! No solver is invoked by persistence, history, input edits or snapshot validation.
use crate::*;
use sha2::{Digest, Sha256};

pub const FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION: u16 = 5;
pub const FOUNDATION_STRUCTURAL_FEATURE: &str = "foundation_structural_v1";
pub const MAX_FOUNDATION_STRUCTURAL_SNAPSHOTS: usize = 16;
pub const MAX_FOUNDATION_STRUCTURAL_DOMAIN_BYTES: usize = 4 << 20;
pub const MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EngineeringSnapshotId(#[serde(with = "crate::drafting::decimal_id")] u64);
impl EngineeringSnapshotId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralSourceRevision {
    pub document_uuid: Uuid,
    #[serde(with = "crate::engineering::operation_source_id")]
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralInput {
    pub structural_model: structural_core::StructuralModel,
    pub analysis_target: structural_core::AnalysisTarget,
    pub foundation: Option<foundation_core::RigidMatInput>,
    #[serde(default)]
    pub foundation_load_source: FoundationLoadSource,
    /// Global right-handed z-up location of the raft base centre, in metres.
    #[serde(default)]
    pub foundation_reference_point_m: [f64; 3],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FoundationLoadSource {
    #[default]
    StructuralReactions,
    ManualSynthetic,
}
impl FoundationStructuralInput {
    /// Shared synthetic fixture for the native UI, typed API and ordinary CLI.
    /// The supplied G+Q factors are illustrative, never normative ULS/SLS factors.
    pub fn stage1_demo() -> Result<Self, CoreError> {
        let structural_model = structural_core::fixtures::braced_single_storey_10x12();
        let analysis_target = structural_model
            .combinations
            .first()
            .map(|combination| structural_core::AnalysisTarget::Combination(combination.id))
            .ok_or_else(|| fs_error("stage-one fixture has no supplied combination"))?;
        let mut foundation = foundation_core::RigidMatInput::default();
        foundation.soil = foundation_core::synthetic_scenarios()
            .into_iter()
            .find(|scenario| scenario.id.as_str() == "3")
            .ok_or_else(|| fs_error("stage-one synthetic three-layer scenario is missing"))?;
        foundation.self_weight_policy = foundation_core::SelfWeightPolicy::IncludeOnce {
            source_id: "stage1-raft-own-weight".into(),
        };
        let input = Self {
            structural_model,
            analysis_target,
            foundation: Some(foundation),
            foundation_load_source: FoundationLoadSource::StructuralReactions,
            foundation_reference_point_m: [5.0, 6.0, 0.0],
        };
        input.validate()?;
        Ok(input)
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        structural_core::validate(&self.structural_model).map_err(fs_error)?;
        structural_core::validate_target(&self.structural_model, self.analysis_target)
            .map_err(fs_error)?;
        if self
            .foundation_reference_point_m
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(fs_error("non-finite foundation reference point"));
        }
        if let Some(input) = &self.foundation {
            foundation_core::validate_input(input).map_err(fs_error)?;
        }
        bounded_value(self, MAX_FOUNDATION_STRUCTURAL_DOMAIN_BYTES)?;
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, CoreError> {
        fingerprint(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralInputState {
    pub definition: FoundationStructuralInput,
    pub source_revision: FoundationStructuralSourceRevision,
    pub input_fingerprint: String,
}
impl FoundationStructuralInputState {
    fn validate(&self) -> Result<(), CoreError> {
        self.definition.validate()?;
        if self.source_revision.document_uuid.is_nil()
            || self.source_revision.operation_id.get() == 0
            || self.input_fingerprint != self.definition.fingerprint()?
        {
            return Err(fs_error(
                "invalid structural/foundation source revision or input fingerprint",
            ));
        }
        Ok(())
    }
}

/// Recorded identity is not a certificate. Executable identity is absent for a
/// source-only producer; the UI must preserve that distinction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineeringSourceIdentity {
    pub source_manifest_sha256: String,
    pub executable_sha256: Option<String>,
    pub solver_version: String,
    pub source_label: String,
}
impl EngineeringSourceIdentity {
    fn validate(&self) -> Result<(), CoreError> {
        if !sha256_text(&self.source_manifest_sha256)
            || self
                .executable_sha256
                .as_ref()
                .is_some_and(|s| !sha256_text(s))
            || self.solver_version.trim().is_empty()
            || self.solver_version.len() > 128
            || self.source_label.trim().is_empty()
            || self.source_label.len() > 1024
        {
            return Err(fs_error(
                "invalid recorded structural/foundation producer identity",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EngineeringApprovalStatus {
    NotImplemented,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EngineeringCheckStatus {
    NotImplemented,
    UltimateBearingNotVerified,
    PreliminaryEstimate,
    SiteDataNotVerified,
}
/// Mandatory, separately machine-readable holds. A numerical residual pass
/// never changes one of these into engineering or normative approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralCheckRecords {
    pub steel_strength_uls: EngineeringCheckStatus,
    pub steel_stability_uls: EngineeringCheckStatus,
    pub structural_displacements_sls: EngineeringCheckStatus,
    pub ultimate_bearing: EngineeringCheckStatus,
    pub sliding: EngineeringCheckStatus,
    pub overturning: EngineeringCheckStatus,
    pub lateral_load_path: EngineeringCheckStatus,
    pub reinforcement: EngineeringCheckStatus,
    pub punching: EngineeringCheckStatus,
    pub slab_bending: EngineeringCheckStatus,
    pub cracks: EngineeringCheckStatus,
    pub second_order: EngineeringCheckStatus,
    pub settlement: EngineeringCheckStatus,
    pub groundwater: EngineeringCheckStatus,
    pub engineering_site_verification: EngineeringCheckStatus,
}
impl Default for FoundationStructuralCheckRecords {
    fn default() -> Self {
        use EngineeringCheckStatus::*;
        Self {
            steel_strength_uls: NotImplemented,
            steel_stability_uls: NotImplemented,
            structural_displacements_sls: NotImplemented,
            ultimate_bearing: UltimateBearingNotVerified,
            sliding: NotImplemented,
            overturning: NotImplemented,
            lateral_load_path: NotImplemented,
            reinforcement: NotImplemented,
            punching: NotImplemented,
            slab_bending: NotImplemented,
            cracks: NotImplemented,
            second_order: NotImplemented,
            settlement: PreliminaryEstimate,
            groundwater: NotImplemented,
            engineering_site_verification: SiteDataNotVerified,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringSnapshotStatus {
    Current,
    Stale,
}

/// Immutable input/result record. The content hash detects accidental changes;
/// it does not certify mathematics, site geology or regulatory compliance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralSnapshot {
    pub schema_version: u16,
    pub input: FoundationStructuralInputState,
    pub structural_result: structural_core::StructuralAnalysisResult,
    /// Retained effective load input after explicit reaction transfer. In
    /// manual mode it equals the declared manual input, without site approval.
    pub effective_foundation_input: Option<foundation_core::RigidMatInput>,
    pub foundation_result: Option<foundation_core::RigidMatResult>,
    pub producer: EngineeringSourceIdentity,
    pub engineering_approval: EngineeringApprovalStatus,
    pub check_records: FoundationStructuralCheckRecords,
    pub content_fingerprint: String,
}
impl FoundationStructuralSnapshot {
    pub fn new(
        input: &FoundationStructuralInputState,
        structural_result: structural_core::StructuralAnalysisResult,
        foundation_result: Option<foundation_core::RigidMatResult>,
        producer: EngineeringSourceIdentity,
    ) -> Result<Self, CoreError> {
        let effective_foundation_input =
            resolve_foundation_input(&input.definition, &structural_result)?;
        let mut snapshot = Self {
            schema_version: 1,
            input: input.clone(),
            structural_result,
            effective_foundation_input,
            foundation_result,
            producer,
            engineering_approval: EngineeringApprovalStatus::NotImplemented,
            check_records: FoundationStructuralCheckRecords::default(),
            content_fingerprint: String::new(),
        };
        snapshot.content_fingerprint = snapshot.expected_fingerprint()?;
        snapshot.validate()?;
        Ok(snapshot)
    }
    fn expected_fingerprint(&self) -> Result<String, CoreError> {
        fingerprint(&(
            self.schema_version,
            &self.input,
            &self.structural_result,
            &self.effective_foundation_input,
            &self.foundation_result,
            &self.producer,
            self.engineering_approval,
            &self.check_records,
        ))
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        self.input.validate()?;
        self.producer.validate()?;
        if self.schema_version != 1
            || self.structural_result.target != self.input.definition.analysis_target
            || self.check_records != FoundationStructuralCheckRecords::default()
            || self.foundation_result.is_some() != self.input.definition.foundation.is_some()
            || self.content_fingerprint != self.expected_fingerprint()?
        {
            return Err(fs_error(
                "invalid structural/foundation snapshot schema, target, result pair or content fingerprint",
            ));
        }
        validate_structural_result_binding(&self.input.definition, &self.structural_result)?;
        let expected_input =
            resolve_foundation_input(&self.input.definition, &self.structural_result)?;
        if self.effective_foundation_input != expected_input {
            return Err(fs_error(
                "effective foundation input disagrees with reaction transfer",
            ));
        }
        if let (Some(input), Some(result)) =
            (&self.effective_foundation_input, &self.foundation_result)
        {
            foundation_core::validate_result_binding(input, result).map_err(fs_error)?;
        }
        // This bounded serialization also rejects non-finite result values:
        // serde_json normally converts NaN/Inf to null, so inspect that token.
        let _roundtrip: (
            structural_core::StructuralAnalysisResult,
            Option<foundation_core::RigidMatResult>,
        ) = serde_json::from_slice(&serde_json::to_vec(&(
            &self.structural_result,
            &self.foundation_result,
        ))?)?;
        bounded_value(self, MAX_FOUNDATION_STRUCTURAL_DOMAIN_BYTES)?;
        Ok(())
    }
}

/// Explicit analysis entry point. Callers choose when to run this pure bounded
/// computation, then submit SaveAnalysisSnapshot through Document::commit.
/// Open, Undo, Redo and snapshot validation never call this function.
pub fn analyze_foundation_structural(
    input: &FoundationStructuralInputState,
    producer: EngineeringSourceIdentity,
) -> Result<FoundationStructuralSnapshot, CoreError> {
    input.validate()?;
    let structural_result = structural_core::analyze(
        &input.definition.structural_model,
        input.definition.analysis_target,
    )
    .map_err(fs_error)?;
    let effective = resolve_foundation_input(&input.definition, &structural_result)?;
    let foundation_result = effective
        .as_ref()
        .map(foundation_core::solve)
        .transpose()
        .map_err(fs_error)?;
    FoundationStructuralSnapshot::new(input, structural_result, foundation_result, producer)
}

/// Structural reactions are support-on-structure. Reverse all six components
/// to obtain structure-on-foundation, then translate about the declared base
/// reference with M+r×F. The kernel retains every original action and ledger ID.
pub fn resolve_foundation_input(
    input: &FoundationStructuralInput,
    result: &structural_core::StructuralAnalysisResult,
) -> Result<Option<foundation_core::RigidMatInput>, CoreError> {
    let Some(mut foundation) = input.foundation.clone() else {
        return Ok(None);
    };
    match input.foundation_load_source {
        FoundationLoadSource::StructuralReactions => {
            validate_structural_result_binding(input, result)?;
            let reactions = result
                .reactions
                .iter()
                .map(|reaction| {
                    let force = reaction.force_moment;
                    foundation_core::LocatedStructuralReaction {
                        source_id: format!("structural-support:{}", reaction.node.get()),
                        location_m: std::array::from_fn(|axis| {
                            reaction.position_m[axis] - input.foundation_reference_point_m[axis]
                        }),
                        external_force_kn: [-force[0], -force[1], -force[2]],
                        external_free_moment_knm: [-force[3], -force[4], -force[5]],
                    }
                })
                .collect();
            foundation.load =
                foundation_core::load_from_structural_reactions(reactions).map_err(fs_error)?;
        }
        FoundationLoadSource::ManualSynthetic => {}
    }
    foundation_core::validate_input(&foundation).map_err(fs_error)?;
    Ok(Some(foundation))
}

fn validate_structural_result_binding(
    input: &FoundationStructuralInput,
    result: &structural_core::StructuralAnalysisResult,
) -> Result<(), CoreError> {
    structural_core::validate_result_binding(
        &input.structural_model,
        input.analysis_target,
        result,
    )
    .map_err(fs_error)?;
    let _: structural_core::StructuralAnalysisResult =
        serde_json::from_slice(&serde_json::to_vec(result)?)?;
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralDomain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_input: Option<FoundationStructuralInputState>,
    #[serde(default, deserialize_with = "crate::drafting::unique_map")]
    snapshots: BTreeMap<EngineeringSnapshotId, FoundationStructuralSnapshot>,
}
impl FoundationStructuralDomain {
    pub fn is_empty(&self) -> bool {
        self.current_input.is_none() && self.snapshots.is_empty()
    }
    pub fn input(&self) -> Option<&FoundationStructuralInputState> {
        self.current_input.as_ref()
    }
    pub fn snapshots(
        &self,
    ) -> impl DoubleEndedIterator<Item = (EngineeringSnapshotId, &FoundationStructuralSnapshot)>
    + ExactSizeIterator {
        self.snapshots.iter().map(|(id, snapshot)| (*id, snapshot))
    }
    pub fn snapshot(&self, id: EngineeringSnapshotId) -> Option<&FoundationStructuralSnapshot> {
        self.snapshots.get(&id)
    }
    pub fn snapshot_status(&self, id: EngineeringSnapshotId) -> Option<EngineeringSnapshotStatus> {
        self.snapshots.get(&id).map(|snapshot| {
            if self.current_input.as_ref() == Some(&snapshot.input) {
                EngineeringSnapshotStatus::Current
            } else {
                EngineeringSnapshotStatus::Stale
            }
        })
    }
    pub(super) fn high_water(&self) -> u64 {
        self.current_input
            .iter()
            .map(|s| s.source_revision.operation_id.get())
            .chain(self.snapshots.iter().flat_map(|(id, snapshot)| {
                [id.get(), snapshot.input.source_revision.operation_id.get()]
            }))
            .max()
            .unwrap_or(0)
    }
    pub(super) fn validate(&self, document_uuid: Uuid) -> Result<(), CoreError> {
        if self.snapshots.len() > MAX_FOUNDATION_STRUCTURAL_SNAPSHOTS {
            return Err(fs_error("structural/foundation snapshot limit exceeded"));
        }
        for input in self
            .current_input
            .iter()
            .chain(self.snapshots.values().map(|s| &s.input))
        {
            input.validate()?;
            if input.source_revision.document_uuid != document_uuid {
                return Err(fs_error(
                    "structural/foundation source belongs to another document",
                ));
            }
        }
        for (id, snapshot) in &self.snapshots {
            if id.get() == 0 || id.get() <= snapshot.input.source_revision.operation_id.get() {
                return Err(fs_error(
                    "invalid structural/foundation snapshot operation identity",
                ));
            }
            snapshot.validate()?;
        }
        bounded_value(self, MAX_FOUNDATION_STRUCTURAL_DOMAIN_BYTES)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FoundationStructuralCommand {
    ReplaceInput {
        input: FoundationStructuralInput,
    },
    SaveAnalysisSnapshot {
        snapshot: FoundationStructuralSnapshot,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationStructuralDelta {
    pub input_changed: bool,
    pub created_snapshots: Vec<EngineeringSnapshotId>,
    pub removed_snapshots: Vec<EngineeringSnapshotId>,
    pub affected_snapshots: Vec<EngineeringSnapshotId>,
}
impl FoundationStructuralDelta {
    pub fn is_empty(&self) -> bool {
        !self.input_changed
            && self.created_snapshots.is_empty()
            && self.removed_snapshots.is_empty()
            && self.affected_snapshots.is_empty()
    }
}
pub(super) fn summarize_foundation_structural(changes: &[Change]) -> FoundationStructuralDelta {
    let mut transitions = changes.iter().filter_map(|c| {
        if let Change::FoundationStructural { before, after } = c {
            Some((before, after))
        } else {
            None
        }
    });
    let Some((first, mut last)) = transitions.next() else {
        return FoundationStructuralDelta::default();
    };
    for (_, after) in transitions {
        last = after;
    }
    domain_diff(first, last)
}
fn domain_diff(
    before: &FoundationStructuralDomain,
    after: &FoundationStructuralDomain,
) -> FoundationStructuralDelta {
    let input_changed = before.current_input != after.current_input;
    FoundationStructuralDelta {
        input_changed,
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
            .filter(|id| input_changed || before.snapshots.get(id) != after.snapshots.get(id))
            .collect(),
    }
}
pub(super) fn foundation_structural_projection(
    domain: &FoundationStructuralDomain,
    changes: Option<&[Change]>,
) -> Option<FoundationStructuralDomain> {
    if changes.is_some_and(|changes| {
        !changes
            .iter()
            .any(|c| matches!(c, Change::FoundationStructural { .. }))
    }) {
        return None;
    }
    // A touched empty domain is meaningful in compact history (Undo creation).
    if changes.is_none() && domain.is_empty() {
        None
    } else {
        Some(domain.clone())
    }
}

impl Document {
    pub fn foundation_structural(&self) -> &FoundationStructuralDomain {
        &self.foundation_structural
    }
    pub(super) fn apply_foundation_structural_command(
        &mut self,
        command: FoundationStructuralCommand,
        operation_id: OperationId,
    ) -> Result<bim::Applied, CoreError> {
        let before = self.foundation_structural.clone();
        let after =
            foundation_structural_transition(&before, &command, self.document_uuid, operation_id)?;
        self.foundation_structural = after.clone();
        Ok((
            OperationKind::FoundationStructuralChanged { command },
            None,
            vec![],
            vec![],
            "Structural/foundation state".into(),
            vec![Change::FoundationStructural { before, after }],
            vec![],
        ))
    }
    pub(super) fn has_foundation_structural_history(&self) -> bool {
        let change = |c: &Change| {
            matches!(c, Change::FoundationStructural { .. })
                || matches!(c, Change::Operation { before, after, .. } if before.iter().chain(after).any(|o| matches!(o.kind, OperationKind::FoundationStructuralChanged { .. })))
        };
        !self.foundation_structural.is_empty()
            || self
                .operations
                .values()
                .any(|o| matches!(o.kind, OperationKind::FoundationStructuralChanged { .. }))
            || self.revision_states.values().any(|s| {
                s.foundation_structural.is_some()
                    || s.delta
                        .as_ref()
                        .is_some_and(|d| d.changes.iter().any(&change))
            })
            || self
                .undo_stack
                .iter()
                .chain(&self.redo_stack)
                .any(|e| e.changes.iter().any(&change))
            || self.command_receipts.values().any(|r| {
                r.recorded_commands.as_ref().is_some_and(|cs| {
                    cs.iter()
                        .any(|c| matches!(c, Command::FoundationStructural(_)))
                })
            })
    }
    pub(super) fn validate_foundation_structural(&self) -> Result<(), CoreError> {
        self.foundation_structural.validate(self.document_uuid)?;
        if self.next_operation_id <= self.foundation_structural.high_water() {
            return Err(fs_error(
                "structural/foundation operation counter loses its high-water mark",
            ));
        }
        for input in self.foundation_structural.current_input.iter().chain(
            self.foundation_structural
                .snapshots
                .values()
                .map(|s| &s.input),
        ) {
            let source = self
                .operations
                .get(&input.source_revision.operation_id)
                .ok_or_else(|| {
                    fs_error("structural/foundation input source operation is missing")
                })?;
            if !matches!(&source.kind, OperationKind::FoundationStructuralChanged { command: FoundationStructuralCommand::ReplaceInput { input: recorded } } if recorded == &input.definition)
            {
                return Err(fs_error(
                    "structural/foundation input disagrees with its source operation",
                ));
            }
        }
        for (id, snapshot) in &self.foundation_structural.snapshots {
            let source = self.operations.get(&OperationId(id.get())).ok_or_else(|| {
                fs_error("structural/foundation snapshot save operation is missing")
            })?;
            if !matches!(&source.kind, OperationKind::FoundationStructuralChanged { command: FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot: recorded } } if recorded == snapshot)
            {
                return Err(fs_error(
                    "structural/foundation snapshot disagrees with its save operation",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn validate_local_foundation_structural(
        &self,
        changes: &[Change],
    ) -> Result<(), CoreError> {
        if changes
            .iter()
            .any(|c| matches!(c, Change::FoundationStructural { .. }))
        {
            for change in changes {
                if let Change::FoundationStructural { before, after } = change {
                    before.validate(self.document_uuid)?;
                    after.validate(self.document_uuid)?;
                }
            }
            self.validate_foundation_structural()?;
        }
        Ok(())
    }
    pub(super) fn validate_foundation_structural_retention(&self) -> Result<(), CoreError> {
        if self.has_foundation_structural_history() || self.has_configurator_history() {
            bounded_value(self, MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES - 4096)?;
        }
        Ok(())
    }
    pub(super) fn check_foundation_structural_pending(
        &self,
        changes: &[Change],
        commands: Option<&[Command]>,
        payload_bytes: usize,
    ) -> Result<(), CoreError> {
        if self.has_foundation_structural_history() || self.has_configurator_history()
            || changes
                .iter()
                .any(|c| matches!(c, Change::FoundationStructural { .. }))
        {
            // Existing current state/revision has already been applied. Reserve
            // the pending stack journal, receipt payload and original commands
            // before they enter retained history. No authority is dropped.
            let mut bytes = circular_archive::bounded_serialized_size(
                self,
                MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES,
            )?;
            bytes = bytes
                .checked_add(circular_archive::bounded_serialized_size(
                    changes,
                    MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES,
                )?)
                .and_then(|n| {
                    payload_bytes
                        .checked_mul(2)
                        .and_then(|payload| n.checked_add(payload))
                })
                .ok_or_else(|| fs_error("retention byte overflow"))?;
            if let Some(commands) = commands {
                bytes = bytes
                    .checked_add(
                        circular_archive::bounded_serialized_size(
                            commands,
                            MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES,
                        )?
                        .checked_mul(4)
                        .ok_or_else(|| fs_error("receipt reservation overflow"))?,
                    )
                    .ok_or_else(|| fs_error("retention byte overflow"))?;
            }
            if bytes > MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES - 65536 {
                return Err(fs_error(
                    "retained native5 document exceeds 64 MiB bounded schema",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn foundation_structural_diff(
        &self,
        from: u64,
        to: u64,
    ) -> Result<FoundationStructuralDelta, CoreError> {
        let before = self
            .reconstruct_revision_state(from)?
            .foundation_structural
            .unwrap_or_default();
        let after = self
            .reconstruct_revision_state(to)?
            .foundation_structural
            .unwrap_or_default();
        Ok(domain_diff(&before, &after))
    }
}
fn foundation_structural_transition(
    before: &FoundationStructuralDomain,
    command: &FoundationStructuralCommand,
    document_uuid: Uuid,
    operation_id: OperationId,
) -> Result<FoundationStructuralDomain, CoreError> {
    let mut after = before.clone();
    match command {
        FoundationStructuralCommand::ReplaceInput { input } => {
            input.validate()?;
            after.current_input = Some(FoundationStructuralInputState {
                definition: input.clone(),
                source_revision: FoundationStructuralSourceRevision {
                    document_uuid,
                    operation_id,
                },
                input_fingerprint: input.fingerprint()?,
            });
        }
        FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot } => {
            snapshot.validate()?;
            if before.current_input.as_ref() != Some(&snapshot.input) {
                return Err(fs_error(
                    "structural/foundation result is stale or has no current input",
                ));
            }
            if after
                .snapshots
                .insert(EngineeringSnapshotId(operation_id.get()), snapshot.clone())
                .is_some()
            {
                return Err(fs_error(
                    "immutable structural/foundation snapshot identity already exists",
                ));
            }
        }
    }
    after.validate(document_uuid)?;
    Ok(after)
}
pub(super) fn foundation_structural_effects_match(
    command: &FoundationStructuralCommand,
    changes: &[Change],
    operation: &OperationRecord,
) -> bool {
    let [Change::FoundationStructural { before, after }] = changes else {
        return false;
    };
    let uuid = after
        .current_input
        .as_ref()
        .map(|s| s.source_revision.document_uuid)
        .or_else(|| {
            before
                .current_input
                .as_ref()
                .map(|s| s.source_revision.document_uuid)
        });
    uuid.is_some_and(|uuid| {
        foundation_structural_transition(before, command, uuid, operation.id)
            .is_ok_and(|expected| &expected == after)
    })
}
fn fs_error(error: impl fmt::Display) -> CoreError {
    CoreError::InvalidDocument(format!("structural/foundation: {error}"))
}
fn fingerprint(value: &impl Serialize) -> Result<String, CoreError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn sha256_text(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn bounded_value(value: &(impl Serialize + ?Sized), limit: usize) -> Result<(), CoreError> {
    if circular_archive::bounded_serialized_size(value, limit).is_err() {
        return Err(fs_error(format!("bounded schema exceeds {limit} bytes")));
    }
    Ok(())
}
