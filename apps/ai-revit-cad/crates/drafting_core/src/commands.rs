use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportLayer {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub style: DraftStyle,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportEntity {
    pub geometry: EntityGeometry,
    pub layer_name: String,
    pub style: DraftStyle,
    pub source_handle: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportBatch {
    pub drawing_id: DrawingId,
    pub source: ImportSourceSpec,
    pub layers: Vec<ImportLayer>,
    pub entities: Vec<ImportEntity>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftOperation {
    Create {
        drawing_id: DrawingId,
        layer_id: LayerId,
        geometry: EntityGeometry,
        #[serde(default)]
        style: DraftStyle,
    },
    Move {
        drawing_id: DrawingId,
        ids: Vec<DraftEntityId>,
        delta: Point2,
    },
    Edit {
        drawing_id: DrawingId,
        id: DraftEntityId,
        geometry: EntityGeometry,
    },
    Delete {
        drawing_id: DrawingId,
        ids: Vec<DraftEntityId>,
    },
    CreateLayer {
        drawing_id: DrawingId,
        name: String,
        visible: bool,
        locked: bool,
        #[serde(default)]
        style: DraftStyle,
    },
    SetLayer {
        drawing_id: DrawingId,
        layer_id: LayerId,
        visible: Option<bool>,
        locked: Option<bool>,
        style: Option<DraftStyle>,
    },
    CreateDrawing {
        name: String,
        #[serde(default)]
        plane: DrawingPlane,
    },
    Import {
        batch: ImportBatch,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    pub command_id: String,
    pub expected_revision: u64,
    pub operations: Vec<DraftOperation>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command_id: String,
    pub revision_before: u64,
    pub revision_after: u64,
    pub created_ids: Vec<DraftEntityId>,
    pub updated_ids: Vec<DraftEntityId>,
    pub deleted_ids: Vec<DraftEntityId>,
    pub created_drawing_ids: Vec<DrawingId>,
    pub changed_layer_ids: Vec<LayerId>,
    pub import_asset_ids: Vec<ImportAssetId>,
    pub replayed: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftChange {
    Entity {
        drawing_id: DrawingId,
        id: DraftEntityId,
        before: Option<DraftEntity>,
        after: Option<DraftEntity>,
    },
    Layer {
        drawing_id: DrawingId,
        id: LayerId,
        before: Option<DraftLayer>,
        after: Option<DraftLayer>,
    },
    ImportAsset {
        drawing_id: DrawingId,
        id: ImportAssetId,
        before: Option<ImportAsset>,
        after: Option<ImportAsset>,
    },
    Drawing {
        drawing_id: DrawingId,
        before: Option<DraftDrawing>,
        after: Option<DraftDrawing>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRecord {
    pub command_id: String,
    pub revision_before: u64,
    pub revision_after: u64,
    pub changes: Vec<DraftChange>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ReplayAction {
    Apply {
        expected_revision: u64,
        operations: Vec<DraftOperation>,
    },
    Undo {
        expected_revision: u64,
    },
    Redo {
        expected_revision: u64,
    },
    /// Redo-branch payloads are pruned; their SHA-256 identity and receipt remain.
    Pruned {
        payload_sha256: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayRecord {
    pub action: ReplayAction,
    pub receipt: CommandReceipt,
    #[serde(default)]
    pub allocators_before: Option<IdAllocators>,
    /// Issued IDs remain evidenced even when a redo branch is discarded and
    /// its net entity receipt is empty (for example Create followed by Delete).
    #[serde(default)]
    pub allocators_after: Option<IdAllocators>,
    #[serde(default)]
    pub target_command_id: Option<String>,
}

impl LabDocument {
    fn check_command(
        &self,
        command_id: &str,
        expected_revision: u64,
        action: &ReplayAction,
    ) -> DraftResult<Option<CommandReceipt>> {
        if command_id.trim().is_empty() || command_id.len() > 256 || command_id.contains('\0') {
            return Err(DraftError::new(
                "invalid_command_id",
                "command ID must contain 1..256 bytes without NUL",
            ));
        }
        if let Some(record) = self.receipts.get(command_id) {
            let matches = match &record.action {
                ReplayAction::Pruned { payload_sha256 } => {
                    *payload_sha256 == payload_digest(action)?
                }
                _ => record.action == *action,
            };
            if !matches {
                return Err(DraftError::new(
                    "command_id_conflict",
                    "command ID was already used with a different payload",
                ));
            }
            let mut receipt = record.receipt.clone();
            receipt.replayed = true;
            return Ok(Some(receipt));
        }
        if expected_revision != self.state.revision {
            return Err(DraftError::new(
                "stale_revision",
                format!(
                    "expected {expected_revision}, actual {}",
                    self.state.revision
                ),
            ));
        }
        if self.receipts.len() >= MAX_HISTORY_RECORDS * 4 {
            return Err(DraftError::new(
                "history_budget",
                "command receipt budget exhausted",
            ));
        }
        Ok(None)
    }
    pub fn apply(&mut self, command: CommandEnvelope) -> DraftResult<CommandReceipt> {
        preflight_operations(&command.operations)?;
        check_pretty_budget(&command)?;
        let action = ReplayAction::Apply {
            expected_revision: command.expected_revision,
            operations: command.operations.clone(),
        };
        if let Some(receipt) =
            self.check_command(&command.command_id, command.expected_revision, &action)?
        {
            return Ok(receipt);
        }
        // Idempotent replays are measured against their original payload, not
        // current geometry that a later accepted edit may have made larger.
        preflight_geometry_work(&self.state, &command.operations)?;
        if command.operations.is_empty() || command.operations.len() > MAX_DOCUMENT_ENTITIES {
            return Err(DraftError::new(
                "command_budget",
                "command must have 1..20000 operations",
            ));
        }
        if self.history_cursor >= MAX_HISTORY_RECORDS {
            return Err(DraftError::new(
                "history_budget",
                "history budget exhausted",
            ));
        }
        let before = self.state.revision;
        let after = before
            .checked_add(1)
            .ok_or_else(|| DraftError::new("revision_exhausted", "revision space exhausted"))?;
        // A failed operation drops this staging state, including allocator advances.
        let mut staged = self.state.clone();
        staged.revision = after;
        let mut changes = Vec::new();
        for operation in &command.operations {
            apply_operation(&mut staged, operation, &mut changes)?;
        }
        if changes.is_empty() {
            return Err(DraftError::new(
                "empty_change",
                "command did not change any persistent object",
            ));
        }
        validate_state(&staged)?;
        let receipt = summarize(&command.command_id, before, after, &changes, false);
        let mut candidate = Self {
            runtime_instance_id: self.runtime_instance_id.clone(),
            schema_version: self.schema_version,
            state: staged,
            history: self.history[..self.history_cursor].to_vec(),
            history_cursor: self.history_cursor,
            receipts: self.receipts.clone(),
        };
        let discarded: BTreeSet<_> = self.history[self.history_cursor..]
            .iter()
            .map(|r| r.command_id.clone())
            .collect();
        for (id, replay) in &mut candidate.receipts {
            if discarded.contains(id)
                || replay
                    .target_command_id
                    .as_ref()
                    .is_some_and(|target| discarded.contains(target))
            {
                replay.action = ReplayAction::Pruned {
                    payload_sha256: payload_digest(&replay.action)?,
                };
                replay.allocators_before = None;
                replay.target_command_id = None;
            }
        }
        candidate.history.push(HistoryRecord {
            command_id: command.command_id.clone(),
            revision_before: before,
            revision_after: after,
            changes,
        });
        candidate.history_cursor = candidate.history.len();
        candidate.receipts.insert(
            command.command_id,
            ReplayRecord {
                action,
                receipt: receipt.clone(),
                allocators_before: Some(self.state.next_ids.clone()),
                allocators_after: Some(candidate.state.next_ids.clone()),
                target_command_id: None,
            },
        );
        check_pretty_budget(&candidate)?;
        *self = candidate;
        Ok(receipt)
    }
    pub fn undo(
        &mut self,
        command_id: impl Into<String>,
        expected_revision: u64,
    ) -> DraftResult<CommandReceipt> {
        self.history_action(command_id.into(), expected_revision, false)
    }
    pub fn redo(
        &mut self,
        command_id: impl Into<String>,
        expected_revision: u64,
    ) -> DraftResult<CommandReceipt> {
        self.history_action(command_id.into(), expected_revision, true)
    }
    fn history_action(
        &mut self,
        command_id: String,
        expected_revision: u64,
        redo: bool,
    ) -> DraftResult<CommandReceipt> {
        let action = if redo {
            ReplayAction::Redo { expected_revision }
        } else {
            ReplayAction::Undo { expected_revision }
        };
        if let Some(receipt) = self.check_command(&command_id, expected_revision, &action)? {
            return Ok(receipt);
        }
        let index = if redo {
            if !self.can_redo() {
                return Err(DraftError::new("nothing_to_redo", "redo history is empty"));
            }
            self.history_cursor
        } else {
            if !self.can_undo() {
                return Err(DraftError::new("nothing_to_undo", "undo history is empty"));
            }
            self.history_cursor - 1
        };
        let before = self.state.revision;
        let after = before
            .checked_add(1)
            .ok_or_else(|| DraftError::new("revision_exhausted", "revision space exhausted"))?;
        let mut staged = self.state.clone();
        staged.revision = after;
        apply_changes(&mut staged, &self.history[index].changes, redo, after, true)?;
        validate_state(&staged)?;
        let receipt = summarize(
            &command_id,
            before,
            after,
            &self.history[index].changes,
            !redo,
        );
        let mut candidate = Self {
            runtime_instance_id: self.runtime_instance_id.clone(),
            schema_version: self.schema_version,
            state: staged,
            history: self.history.clone(),
            history_cursor: if redo { index + 1 } else { index },
            receipts: self.receipts.clone(),
        };
        candidate.receipts.insert(
            command_id,
            ReplayRecord {
                action,
                receipt: receipt.clone(),
                allocators_before: None,
                allocators_after: Some(candidate.state.next_ids.clone()),
                target_command_id: Some(self.history[index].command_id.clone()),
            },
        );
        check_pretty_budget(&candidate)?;
        *self = candidate;
        Ok(receipt)
    }
    pub(crate) fn validate_history(&self) -> DraftResult<()> {
        self.validate_event_chain()?;
        let mut command_ids = BTreeSet::new();
        let mut last_revision = 0;
        for record in &self.history {
            if record.command_id.trim().is_empty()
                || record.command_id.len() > 256
                || record.command_id.contains('\0')
                || !command_ids.insert(&record.command_id)
                || record.revision_after != record.revision_before.checked_add(1).unwrap_or(0)
                || record.revision_after > self.state.revision
                || record.revision_after <= last_revision
                || record.changes.is_empty()
                || record.changes.len() > MAX_DOCUMENT_ENTITIES * 3 + MAX_LAYERS
            {
                return Err(DraftError::new(
                    "invalid_history",
                    "history command identity/revision/change count invalid",
                ));
            }
            let replay = self.receipts.get(&record.command_id).ok_or_else(|| {
                DraftError::new("invalid_history", "history command lacks receipt")
            })?;
            if replay.receipt.revision_before != record.revision_before
                || replay.receipt.revision_after != record.revision_after
                || !matches!(replay.action, ReplayAction::Apply { .. })
            {
                return Err(DraftError::new(
                    "invalid_history",
                    "history and receipt disagree",
                ));
            }
            for change in &record.changes {
                validate_change_identity(change, &self.state.next_ids)?;
                validate_change_payload(change, self.state.revision)?;
            }
            last_revision = record.revision_after;
        }
        for (id, replay) in &self.receipts {
            if id != &replay.receipt.command_id
                || id.trim().is_empty()
                || id.len() > 256
                || id.contains('\0')
                || replay.receipt.replayed
                || replay.receipt.revision_after > self.state.revision
                || replay.receipt.revision_before.checked_add(1)
                    != Some(replay.receipt.revision_after)
            {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "receipt identity/revision invalid",
                ));
            }
            let expected = match &replay.action {
                ReplayAction::Apply {
                    expected_revision,
                    operations,
                } => {
                    preflight_operations(operations)?;
                    for operation in operations {
                        validate_operation_id_spaces(operation, &self.state.next_ids)?;
                    }
                    if replay.target_command_id.is_some() {
                        return Err(DraftError::new(
                            "invalid_receipt",
                            "Apply receipt has an undo target",
                        ));
                    }
                    *expected_revision
                }
                ReplayAction::Undo { expected_revision }
                | ReplayAction::Redo { expected_revision } => {
                    let target = replay
                        .target_command_id
                        .as_ref()
                        .and_then(|id| self.history.iter().find(|record| record.command_id == *id))
                        .ok_or_else(|| {
                            DraftError::new(
                                "invalid_receipt",
                                "undo/redo receipt lacks a retained delta target",
                            )
                        })?;
                    let expected = summarize(
                        id,
                        *expected_revision,
                        replay.receipt.revision_after,
                        &target.changes,
                        matches!(replay.action, ReplayAction::Undo { .. }),
                    );
                    if target.revision_after > *expected_revision || expected != replay.receipt {
                        return Err(DraftError::new(
                            "invalid_receipt",
                            "undo/redo receipt differs from its target delta",
                        ));
                    }
                    *expected_revision
                }
                ReplayAction::Pruned { payload_sha256 } => {
                    if payload_sha256.len() != 64
                        || !payload_sha256.bytes().all(|c| c.is_ascii_hexdigit())
                        || replay.target_command_id.is_some()
                        || replay.allocators_before.is_some()
                        || command_ids.contains(id)
                    {
                        return Err(DraftError::new(
                            "invalid_receipt",
                            "invalid redo-branch payload tombstone",
                        ));
                    }
                    replay.receipt.revision_before
                }
            };
            if expected != replay.receipt.revision_before {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "saved expected revision disagrees with receipt",
                ));
            }
            if replay
                .receipt
                .created_ids
                .iter()
                .chain(&replay.receipt.updated_ids)
                .chain(&replay.receipt.deleted_ids)
                .any(|id| id.get() == 0 || id.get() > self.state.next_ids.entity_high_water)
                || replay
                    .receipt
                    .created_drawing_ids
                    .iter()
                    .any(|id| id.get() == 0 || id.get() > self.state.next_ids.drawing_high_water)
                || replay
                    .receipt
                    .changed_layer_ids
                    .iter()
                    .any(|id| id.get() == 0 || id.get() > self.state.next_ids.layer_high_water)
                || replay
                    .receipt
                    .import_asset_ids
                    .iter()
                    .any(|id| id.get() == 0 || id.get() > self.state.next_ids.import_high_water)
            {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "receipt contains invalid allocated IDs",
                ));
            }
        }
        // Reconstruct the journal origin from the live cursor, then replay all
        // records with checked before/after values. Undo data cannot silently
        // replace unrelated live entities during a malformed document load.
        let mut state = self.state.clone();
        for record in self.history[..self.history_cursor].iter().rev() {
            apply_changes(
                &mut state,
                &record.changes,
                false,
                self.state.revision,
                true,
            )?;
            validate_state(&state)?;
        }
        if self.history_cursor == 0 && !semantic_state_eq(&state, &self.state) {
            return Err(DraftError::new(
                "invalid_history",
                "history origin differs from current state",
            ));
        }
        for (index, record) in self.history.iter().enumerate() {
            let replay = &self.receipts[&record.command_id];
            let ReplayAction::Apply { operations, .. } = &replay.action else {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "retained delta lacks Apply payload",
                ));
            };
            let mut trial = state.clone();
            trial.next_ids = replay
                .allocators_before
                .clone()
                .unwrap_or_else(|| infer_allocator_before(&state, &record.changes));
            if trial.next_ids.entity_high_water > self.state.next_ids.entity_high_water
                || trial.next_ids.drawing_high_water > self.state.next_ids.drawing_high_water
                || trial.next_ids.layer_high_water > self.state.next_ids.layer_high_water
                || trial.next_ids.import_high_water > self.state.next_ids.import_high_water
            {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "allocator-before exceeds high-water",
                ));
            }
            trial.revision = record.revision_after;
            for drawing in trial.drawings.values_mut() {
                drawing.revision = drawing.revision.min(record.revision_before);
                for entity in drawing.entities.values_mut() {
                    entity.revision = entity.revision.min(record.revision_before);
                }
            }
            preflight_geometry_work(&trial, operations)?;
            let mut generated = Vec::new();
            for operation in operations {
                apply_operation(&mut trial, operation, &mut generated)?;
            }
            validate_state(&trial)?;
            if !changes_semantic_eq(&generated, &record.changes)
                || replay.allocators_after.as_ref().is_some_and(|after| *after != trial.next_ids)
                || summarize(
                    &record.command_id,
                    record.revision_before,
                    record.revision_after,
                    &generated,
                    false,
                ) != replay.receipt
            {
                return Err(DraftError::new(
                    "invalid_receipt",
                    "saved typed payload does not reproduce retained delta and receipt",
                ));
            }
            apply_changes(&mut state, &record.changes, true, self.state.revision, true)?;
            validate_state(&state)?;
            if index + 1 == self.history_cursor && !semantic_state_eq(&state, &self.state) {
                return Err(DraftError::new(
                    "invalid_history",
                    "history cursor differs from current geometry",
                ));
            }
        }
        Ok(())
    }

    /// Each successful authority command advances revision once and keeps one
    /// receipt, including discarded redo branches. Validate the event order,
    /// not just each undo/redo summary in isolation: equal entity ID summaries
    /// do not identify which history record was actually undone.
    fn validate_event_chain(&self) -> DraftResult<()> {
        if self.state.revision != self.receipts.len() as u64 {
            return Err(DraftError::new(
                "invalid_history",
                "document revision differs from the retained command event count",
            ));
        }
        let mut events: Vec<_> = self.receipts.iter().collect();
        events.sort_by_key(|(_, event)| event.receipt.revision_after);
        let mut inserted = 0usize;
        let mut cursor = 0usize;
        let mut issued_floor = IdAllocators::default();
        for (position, (id, event)) in events.into_iter().enumerate() {
            if event.receipt.revision_before != position as u64
                || event.receipt.revision_after != position as u64 + 1
            {
                return Err(DraftError::new(
                    "invalid_history",
                    "command events must form one unique contiguous revision chain",
                ));
            }
            if let Some(before) = &event.allocators_before {
                if !allocator_at_least(before, &issued_floor)
                    || !allocator_at_least(&self.state.next_ids, before)
                {
                    return Err(DraftError::new(
                        "invalid_receipt",
                        "allocator-before regresses below issued IDs or exceeds live high-water",
                    ));
                }
                issued_floor = before.clone();
            }
            if let Some(after) = &event.allocators_after {
                if !allocator_at_least(after, &issued_floor)
                    || !allocator_at_least(&self.state.next_ids, after)
                    || (matches!(event.action, ReplayAction::Undo { .. } | ReplayAction::Redo { .. })
                        && *after != issued_floor)
                {
                    return Err(DraftError::new(
                        "invalid_receipt",
                        "allocator-after regresses below issued IDs or exceeds live high-water",
                    ));
                }
                issued_floor = after.clone();
            } else if matches!(event.action, ReplayAction::Pruned { .. }) {
                return Err(DraftError::new(
                    "allocator_evidence_missing",
                    "discarded history lacks a persisted issued-ID high-water proof",
                ));
            }
            match &event.action {
                ReplayAction::Apply { .. } => {
                    if cursor != inserted
                        || self.history.get(inserted).is_none_or(|record| record.command_id != *id)
                    {
                        return Err(DraftError::new(
                            "invalid_history",
                            "Apply event differs from the retained history order",
                        ));
                    }
                    // Legacy retained histories can reproduce consumed IDs
                    // from their full delta. Pruned histories cannot.
                    if event.allocators_after.is_none() {
                        include_delta_allocators(&mut issued_floor, &self.history[inserted].changes);
                    }
                    inserted += 1;
                    cursor += 1;
                }
                ReplayAction::Undo { .. } => {
                    if cursor == 0
                        || event.target_command_id.as_deref()
                            != Some(self.history[cursor - 1].command_id.as_str())
                    {
                        return Err(DraftError::new(
                            "invalid_history",
                            "Undo event does not target the current history cursor",
                        ));
                    }
                    cursor -= 1;
                }
                ReplayAction::Redo { .. } => {
                    if cursor >= inserted
                        || event.target_command_id.as_deref()
                            != Some(self.history[cursor].command_id.as_str())
                    {
                        return Err(DraftError::new(
                            "invalid_history",
                            "Redo event does not target the current history cursor",
                        ));
                    }
                    cursor += 1;
                }
                // These events affected a discarded branch. Their Apply
                // records and all undo/redo events targeting those records
                // were pruned together, so they have no retained cursor effect.
                ReplayAction::Pruned { .. } => {}
            }
        }
        if inserted != self.history.len() || cursor != self.history_cursor {
            return Err(DraftError::new(
                "invalid_history",
                "saved cursor differs from the command event chain",
            ));
        }
        if self.receipts.values().max_by_key(|event| event.receipt.revision_after)
            .and_then(|event| event.allocators_after.as_ref())
            .is_some_and(|after| *after != self.state.next_ids)
        {
            return Err(DraftError::new(
                "invalid_receipt",
                "last issued-ID high-water differs from the live allocator state",
            ));
        }
        Ok(())
    }
}
fn allocator_at_least(value: &IdAllocators, floor: &IdAllocators) -> bool {
    value.entity_high_water >= floor.entity_high_water
        && value.drawing_high_water >= floor.drawing_high_water
        && value.layer_high_water >= floor.layer_high_water
        && value.import_high_water >= floor.import_high_water
}
fn include_delta_allocators(counters: &mut IdAllocators, changes: &[DraftChange]) {
    for change in changes {
        match change {
            DraftChange::Entity { id, .. } => {
                counters.entity_high_water = counters.entity_high_water.max(id.get());
            }
            DraftChange::Layer { id, .. } => {
                counters.layer_high_water = counters.layer_high_water.max(id.get());
            }
            DraftChange::ImportAsset { id, .. } => {
                counters.import_high_water = counters.import_high_water.max(id.get());
            }
            DraftChange::Drawing { drawing_id, before, after } => {
                counters.drawing_high_water = counters.drawing_high_water.max(drawing_id.get());
                for drawing in before.iter().chain(after.iter()) {
                    counters.layer_high_water = counters.layer_high_water.max(
                        drawing.layers.keys().map(|id| id.get()).max().unwrap_or(0));
                    counters.entity_high_water = counters.entity_high_water.max(
                        drawing.entities.keys().map(|id| id.get()).max().unwrap_or(0));
                    counters.import_high_water = counters.import_high_water.max(
                        drawing.import_sources.keys().map(|id| id.get()).max().unwrap_or(0));
                }
            }
        }
    }
}
fn validate_operation_id_spaces(
    operation: &DraftOperation,
    counters: &IdAllocators,
) -> DraftResult<()> {
    let drawing = |id: DrawingId| id.get() > 0 && id.get() <= counters.drawing_high_water;
    let layer = |id: LayerId| id.get() > 0 && id.get() <= counters.layer_high_water;
    let entity = |id: DraftEntityId| id.get() > 0 && id.get() <= counters.entity_high_water;
    let valid = match operation {
        DraftOperation::Create {
            drawing_id,
            layer_id,
            ..
        } => drawing(*drawing_id) && layer(*layer_id),
        DraftOperation::Move {
            drawing_id, ids, ..
        }
        | DraftOperation::Delete { drawing_id, ids } => {
            drawing(*drawing_id) && ids.iter().all(|id| entity(*id))
        }
        DraftOperation::Edit { drawing_id, id, .. } => drawing(*drawing_id) && entity(*id),
        DraftOperation::CreateLayer { drawing_id, .. } => drawing(*drawing_id),
        DraftOperation::SetLayer {
            drawing_id,
            layer_id,
            ..
        } => drawing(*drawing_id) && layer(*layer_id),
        DraftOperation::Import { batch } => drawing(batch.drawing_id),
        DraftOperation::CreateDrawing { .. } => true,
    };
    if !valid {
        return Err(DraftError::new(
            "invalid_operation_id",
            "saved operation refers outside its typed allocated ID space",
        ));
    }
    Ok(())
}
fn changes_semantic_eq(a: &[DraftChange], b: &[DraftChange]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (
                DraftChange::Entity {
                    drawing_id: da,
                    id: ia,
                    before: ba,
                    after: aa,
                },
                DraftChange::Entity {
                    drawing_id: db,
                    id: ib,
                    before: bb,
                    after: ab,
                },
            ) => da == db && ia == ib && entity_eq(ba, bb) && entity_eq(aa, ab),
            (
                DraftChange::Drawing {
                    drawing_id: da,
                    before: ba,
                    after: aa,
                },
                DraftChange::Drawing {
                    drawing_id: db,
                    before: bb,
                    after: ab,
                },
            ) => da == db && drawing_eq(ba, bb) && drawing_eq(aa, ab),
            _ => a == b,
        })
}
fn infer_allocator_before(state: &DocumentState, changes: &[DraftChange]) -> IdAllocators {
    let mut counters = IdAllocators::default();
    let mut first_entity = None;
    let mut first_layer = None;
    let mut first_drawing = None;
    let mut first_asset = None;
    for drawing in state.drawings.values() {
        counters.drawing_high_water = counters.drawing_high_water.max(drawing.id.get());
        for entity in drawing.entities.values() {
            counters.entity_high_water = counters.entity_high_water.max(entity.id.get());
        }
        for layer in drawing.layers.values() {
            counters.layer_high_water = counters.layer_high_water.max(layer.id.get());
        }
        for asset in drawing.import_sources.values() {
            counters.import_high_water = counters.import_high_water.max(asset.id.get());
        }
    }
    let first = |target: &mut Option<u64>, id: u64| {
        *target = Some(target.map_or(id, |old| old.min(id)));
    };
    for change in changes {
        match change {
            DraftChange::Entity {
                id,
                before: None,
                after: Some(_),
                ..
            } => first(&mut first_entity, id.get()),
            DraftChange::Layer {
                id,
                before: None,
                after: Some(_),
                ..
            } => first(&mut first_layer, id.get()),
            DraftChange::ImportAsset {
                id,
                before: None,
                after: Some(_),
                ..
            } => first(&mut first_asset, id.get()),
            DraftChange::Drawing {
                drawing_id,
                before: None,
                after: Some(d),
                ..
            } => {
                first(&mut first_drawing, drawing_id.get());
                for layer in d.layers.values() {
                    first(&mut first_layer, layer.id.get());
                }
            }
            _ => {}
        }
    }
    if let Some(id) = first_entity {
        counters.entity_high_water = id - 1;
    }
    if let Some(id) = first_layer {
        counters.layer_high_water = id - 1;
    }
    if let Some(id) = first_drawing {
        counters.drawing_high_water = id - 1;
    }
    if let Some(id) = first_asset {
        counters.import_high_water = id - 1;
    }
    counters
}
fn validate_operation_payload(operation: &DraftOperation) -> DraftResult<()> {
    let invalid_id = match operation {
        DraftOperation::Create {
            drawing_id,
            layer_id,
            ..
        } => drawing_id.get() == 0 || layer_id.get() == 0,
        DraftOperation::Move { drawing_id, .. }
        | DraftOperation::Delete { drawing_id, .. }
        | DraftOperation::CreateLayer { drawing_id, .. } => drawing_id.get() == 0,
        DraftOperation::Edit { drawing_id, id, .. } => drawing_id.get() == 0 || id.get() == 0,
        DraftOperation::SetLayer {
            drawing_id,
            layer_id,
            ..
        } => drawing_id.get() == 0 || layer_id.get() == 0,
        DraftOperation::Import { batch } => batch.drawing_id.get() == 0,
        DraftOperation::CreateDrawing { .. } => false,
    };
    if invalid_id {
        return Err(DraftError::new(
            "invalid_operation_id",
            "operation contains a zero typed ID",
        ));
    }
    match operation {
        DraftOperation::Create {
            geometry, style, ..
        } => {
            geometry.validate(GeometryLimits::default())?;
            style.validate()?;
        }
        DraftOperation::Move { ids, delta, .. } => {
            validate_ids(ids)?;
            if !delta.is_finite() {
                return Err(DraftError::new(
                    "nonfinite_geometry",
                    "move delta is not finite",
                ));
            }
        }
        DraftOperation::Edit { geometry, .. } => geometry.validate(GeometryLimits::default())?,
        DraftOperation::Delete { ids, .. } => validate_ids(ids)?,
        DraftOperation::CreateLayer { name, style, .. } => {
            validate_name(name)?;
            style.validate()?;
        }
        DraftOperation::SetLayer { style, .. } => {
            if let Some(style) = style {
                style.validate()?;
            }
        }
        DraftOperation::CreateDrawing { name, plane } => {
            validate_name(name)?;
            plane.validate()?;
        }
        DraftOperation::Import { batch } => validate_import_batch(batch)?,
    }
    Ok(())
}
fn preflight_operations(operations: &[DraftOperation]) -> DraftResult<()> {
    if operations.is_empty() || operations.len() > MAX_DOCUMENT_ENTITIES {
        return Err(DraftError::new(
            "command_budget",
            "command must have 1..20000 operations",
        ));
    }
    let mut cost = 0usize;
    for op in operations {
        let next = match op {
            DraftOperation::Move { ids, .. } | DraftOperation::Delete { ids, .. } => ids.len(),
            DraftOperation::Import { batch } => batch
                .entities
                .len()
                .checked_add(batch.layers.len())
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| DraftError::new("command_budget", "import change count overflow"))?,
            DraftOperation::CreateDrawing { .. } => 2,
            _ => 1,
        };
        cost = cost
            .checked_add(next)
            .ok_or_else(|| DraftError::new("command_budget", "change count overflow"))?;
        if cost > MAX_COMMAND_CHANGES {
            return Err(DraftError::new(
                "command_budget",
                "prospective transaction exceeds 61024 changes",
            ));
        }
    }
    for op in operations {
        validate_operation_payload(op)?;
    }
    Ok(())
}
fn geometry_units(geometry: &EntityGeometry) -> usize {
    match geometry {
        EntityGeometry::Polyline { vertices, .. } => vertices.len(),
        EntityGeometry::Line { .. } => 2,
        _ => 1,
    }
}
fn preflight_geometry_work(
    state: &DocumentState,
    operations: &[DraftOperation],
) -> DraftResult<()> {
    let mut overlay = BTreeMap::new();
    let mut high_water = state.next_ids.entity_high_water;
    let mut total = 0usize;
    let add = |total: &mut usize, n: usize| -> DraftResult<()> {
        *total = total
            .checked_add(n)
            .ok_or_else(|| DraftError::new("command_budget", "journal geometry work overflow"))?;
        if *total > MAX_DOCUMENT_VERTICES {
            return Err(DraftError::new(
                "command_budget",
                "journal geometry exceeds one million vertex units",
            ));
        }
        Ok(())
    };
    for operation in operations {
        match operation {
            DraftOperation::Create { geometry, .. } => {
                let units = geometry_units(geometry);
                add(&mut total, units)?;
                high_water = high_water.checked_add(1).ok_or_else(|| {
                    DraftError::new("id_exhausted", "entity identifier space exhausted")
                })?;
                overlay.insert(DraftEntityId::new(high_water), units);
            }
            DraftOperation::Import { batch } => {
                for entity in &batch.entities {
                    let units = geometry_units(&entity.geometry);
                    add(&mut total, units)?;
                    high_water = high_water.checked_add(1).ok_or_else(|| {
                        DraftError::new("id_exhausted", "entity identifier space exhausted")
                    })?;
                    overlay.insert(DraftEntityId::new(high_water), units);
                }
            }
            DraftOperation::Move {
                drawing_id, ids, ..
            }
            | DraftOperation::Delete { drawing_id, ids } => {
                for id in ids {
                    let units = overlay
                        .get(id)
                        .copied()
                        .or_else(|| {
                            state
                                .drawings
                                .get(drawing_id)
                                .and_then(|d| d.entities.get(id))
                                .map(|e| geometry_units(&e.geometry))
                        })
                        .unwrap_or(1);
                    add(
                        &mut total,
                        units
                            * if matches!(operation, DraftOperation::Move { .. }) {
                                2
                            } else {
                                1
                            },
                    )?;
                    if matches!(operation, DraftOperation::Delete { .. }) {
                        overlay.insert(*id, 0);
                    }
                }
            }
            DraftOperation::Edit {
                drawing_id,
                id,
                geometry,
            } => {
                let old = overlay
                    .get(id)
                    .copied()
                    .or_else(|| {
                        state
                            .drawings
                            .get(drawing_id)
                            .and_then(|d| d.entities.get(id))
                            .map(|e| geometry_units(&e.geometry))
                    })
                    .unwrap_or(1);
                let units = geometry_units(geometry);
                add(&mut total, old + units)?;
                overlay.insert(*id, units);
            }
            _ => {}
        }
    }
    Ok(())
}
fn payload_digest(action: &ReplayAction) -> DraftResult<String> {
    use sha2::{Digest, Sha256};
    struct HashWriter<'a>(&'a mut Sha256);
    impl std::io::Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = Sha256::new();
    serde_json::to_writer(HashWriter(&mut hash), action)
        .map_err(|e| DraftError::new("serialization", e.to_string()))?;
    Ok(format!("{:x}", hash.finalize()))
}
fn validate_ids(ids: &[DraftEntityId]) -> DraftResult<()> {
    if ids.is_empty()
        || ids.len() > MAX_DOCUMENT_ENTITIES
        || ids.iter().any(|id| id.get() == 0)
        || ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(DraftError::new(
            "invalid_entity_ids",
            "entity selection is empty, duplicated or over budget",
        ));
    }
    Ok(())
}
fn validate_import_batch(batch: &ImportBatch) -> DraftResult<()> {
    batch.source.validate()?;
    if batch.entities.is_empty()
        || batch.entities.len() > MAX_DOCUMENT_ENTITIES
        || batch.layers.len() > MAX_LAYERS
    {
        return Err(DraftError::new(
            "import_budget",
            "import entity/layer count exceeds budget or is empty",
        ));
    }
    let mut names = BTreeSet::new();
    let mut handles = BTreeSet::new();
    for layer in &batch.layers {
        validate_name(&layer.name)?;
        layer.style.validate()?;
        if !names.insert(&layer.name) {
            return Err(DraftError::new(
                "duplicate_layer",
                "import contains duplicate layer names",
            ));
        }
    }
    for e in &batch.entities {
        validate_name(&e.layer_name)?;
        e.geometry.validate(GeometryLimits::default())?;
        e.style.validate()?;
        if let Some(h) = &e.source_handle {
            if h.is_empty() || h.len() > 64 || !handles.insert(h.to_ascii_uppercase()) {
                return Err(DraftError::new(
                    "duplicate_source_handle",
                    "import contains duplicate/invalid external handles",
                ));
            }
        }
    }
    Ok(())
}
fn drawing_mut(state: &mut DocumentState, id: DrawingId) -> DraftResult<&mut DraftDrawing> {
    state
        .drawings
        .get_mut(&id)
        .ok_or_else(|| DraftError::new("drawing_not_found", format!("drawing {id} is absent")))
}
fn editable(d: &DraftDrawing, id: DraftEntityId) -> DraftResult<DraftEntity> {
    let entity = d
        .entities
        .get(&id)
        .ok_or_else(|| DraftError::new("entity_not_found", format!("entity {id} is absent")))?;
    if d.layers
        .get(&entity.layer_id)
        .is_none_or(|layer| layer.locked)
    {
        return Err(DraftError::new(
            "locked_layer",
            format!("entity {id} belongs to a locked or absent layer"),
        ));
    }
    Ok(entity.clone())
}
fn import_layer_name(drawing: &DraftDrawing, requested: &str, asset: ImportAssetId) -> DraftResult<String> {
    let existing: BTreeSet<_> = drawing.layers.values().map(|layer| layer.name.as_str()).collect();
    for attempt in 1..=MAX_LAYERS + 1 {
        let suffix = if attempt == 1 {
            format!(" ({asset})")
        } else {
            format!(" ({asset}:{attempt})")
        };
        // Keep generated names within the same byte cap as user names, without
        // splitting a UTF-8 character. The full source name remains in the
        // retained typed import payload; this is an internal conflict name.
        let mut end = requested.len().min(512 - suffix.len());
        while !requested.is_char_boundary(end) {
            end -= 1;
        }
        let name = format!("{}{}", &requested[..end], suffix);
        if !existing.contains(name.as_str()) {
            return Ok(name);
        }
    }
    Err(DraftError::new("layer_budget", "no available import layer conflict name"))
}
fn apply_operation(
    state: &mut DocumentState,
    operation: &DraftOperation,
    changes: &mut Vec<DraftChange>,
) -> DraftResult<()> {
    validate_operation_payload(operation)?;
    let revision = state.revision;
    match operation {
        DraftOperation::Create {
            drawing_id,
            layer_id,
            geometry,
            style,
        } => {
            let drawing = state
                .drawings
                .get(drawing_id)
                .ok_or_else(|| DraftError::new("drawing_not_found", "drawing is absent"))?;
            if drawing.layers.get(layer_id).is_none_or(|l| l.locked) {
                return Err(DraftError::new(
                    "locked_layer",
                    "create target layer is absent or locked",
                ));
            }
            let id = state.next_ids.entity()?;
            let e = DraftEntity {
                id,
                layer_id: *layer_id,
                geometry: geometry.clone(),
                style: style.clone(),
                revision,
                source: None,
            };
            let drawing = drawing_mut(state, *drawing_id)?;
            drawing.entities.insert(id, e.clone());
            drawing.revision = revision;
            changes.push(DraftChange::Entity {
                drawing_id: *drawing_id,
                id,
                before: None,
                after: Some(e),
            });
        }
        DraftOperation::Move {
            drawing_id,
            ids,
            delta,
        } => {
            let drawing = drawing_mut(state, *drawing_id)?;
            for id in ids {
                let before = editable(drawing, *id)?;
                let mut after = before.clone();
                after.geometry = after.geometry.translated(*delta);
                after.geometry.validate(GeometryLimits::default())?;
                after.revision = revision;
                drawing.entities.insert(*id, after.clone());
                changes.push(DraftChange::Entity {
                    drawing_id: *drawing_id,
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                });
            }
            drawing.revision = revision;
        }
        DraftOperation::Edit {
            drawing_id,
            id,
            geometry,
        } => {
            let drawing = drawing_mut(state, *drawing_id)?;
            let before = editable(drawing, *id)?;
            let mut after = before.clone();
            after.geometry = geometry.clone();
            after.revision = revision;
            drawing.entities.insert(*id, after.clone());
            drawing.revision = revision;
            changes.push(DraftChange::Entity {
                drawing_id: *drawing_id,
                id: *id,
                before: Some(before),
                after: Some(after),
            });
        }
        DraftOperation::Delete { drawing_id, ids } => {
            let drawing = drawing_mut(state, *drawing_id)?;
            for id in ids {
                let before = editable(drawing, *id)?;
                drawing.entities.remove(id);
                changes.push(DraftChange::Entity {
                    drawing_id: *drawing_id,
                    id: *id,
                    before: Some(before),
                    after: None,
                });
            }
            drawing.revision = revision;
        }
        DraftOperation::CreateLayer {
            drawing_id,
            name,
            visible,
            locked,
            style,
        } => {
            if state
                .drawings
                .get(drawing_id)
                .ok_or_else(|| DraftError::new("drawing_not_found", "drawing is absent"))?
                .layers
                .values()
                .any(|l| l.name == *name)
            {
                return Err(DraftError::new(
                    "duplicate_layer",
                    "layer name already exists",
                ));
            }
            let id = state.next_ids.layer()?;
            let layer = DraftLayer {
                id,
                name: name.clone(),
                visible: *visible,
                locked: *locked,
                style: style.clone(),
            };
            let drawing = drawing_mut(state, *drawing_id)?;
            drawing.layers.insert(id, layer.clone());
            drawing.revision = revision;
            changes.push(DraftChange::Layer {
                drawing_id: *drawing_id,
                id,
                before: None,
                after: Some(layer),
            });
        }
        DraftOperation::SetLayer {
            drawing_id,
            layer_id,
            visible,
            locked,
            style,
        } => {
            let drawing = drawing_mut(state, *drawing_id)?;
            let before = drawing
                .layers
                .get(layer_id)
                .cloned()
                .ok_or_else(|| DraftError::new("layer_not_found", "layer is absent"))?;
            let mut after = before.clone();
            if let Some(v) = visible {
                after.visible = *v;
            }
            if let Some(l) = locked {
                after.locked = *l;
            }
            if let Some(s) = style {
                after.style = s.clone();
            }
            drawing.layers.insert(*layer_id, after.clone());
            drawing.revision = revision;
            changes.push(DraftChange::Layer {
                drawing_id: *drawing_id,
                id: *layer_id,
                before: Some(before),
                after: Some(after),
            });
        }
        DraftOperation::CreateDrawing { name, plane } => {
            let id = state.next_ids.drawing()?;
            let lid = state.next_ids.layer()?;
            let layer = DraftLayer {
                id: lid,
                name: "Layer0".into(),
                visible: true,
                locked: false,
                style: DraftStyle::default(),
            };
            let drawing = DraftDrawing {
                id,
                name: name.clone(),
                revision,
                plane: plane.clone(),
                entities: BTreeMap::new(),
                layers: BTreeMap::from([(lid, layer)]),
                import_sources: BTreeMap::new(),
            };
            state.drawings.insert(id, drawing.clone());
            changes.push(DraftChange::Drawing {
                drawing_id: id,
                before: None,
                after: Some(drawing),
            });
        }
        DraftOperation::Import { batch } => {
            if !state.drawings.contains_key(&batch.drawing_id) {
                return Err(DraftError::new(
                    "drawing_not_found",
                    "import drawing is absent",
                ));
            }
            let asset_id = state.next_ids.import()?;
            let asset = ImportAsset {
                id: asset_id,
                source: batch.source.clone(),
            };
            drawing_mut(state, batch.drawing_id)?
                .import_sources
                .insert(asset_id, asset.clone());
            changes.push(DraftChange::ImportAsset {
                drawing_id: batch.drawing_id,
                id: asset_id,
                before: None,
                after: Some(asset),
            });
            let mut layers: BTreeMap<String, LayerId> = BTreeMap::new();
            for layer in &batch.layers {
                let existing = state.drawings[&batch.drawing_id]
                    .layers
                    .values()
                    .find(|l| l.name == layer.name)
                    .cloned();
                if let Some(existing) = &existing {
                    // Import may initialize a new locked layer, but it cannot
                    // use an existing lock as a shortcut around Create's gate.
                    if !existing.locked
                        && existing.visible == layer.visible
                        && existing.locked == layer.locked
                        && existing.style == layer.style
                    {
                        layers.insert(layer.name.clone(), existing.id);
                        continue;
                    }
                }
                let id = state.next_ids.layer()?;
                let name = if existing.is_some() {
                    import_layer_name(&state.drawings[&batch.drawing_id], &layer.name, asset_id)?
                } else {
                    layer.name.clone()
                };
                let imported = DraftLayer {
                    id,
                    name,
                    visible: layer.visible,
                    locked: layer.locked,
                    style: layer.style.clone(),
                };
                drawing_mut(state, batch.drawing_id)?
                    .layers
                    .insert(id, imported.clone());
                layers.insert(layer.name.clone(), id);
                changes.push(DraftChange::Layer {
                    drawing_id: batch.drawing_id,
                    id,
                    before: None,
                    after: Some(imported),
                });
            }
            for incoming in &batch.entities {
                let layer_id = layers
                    .get(&incoming.layer_name)
                    .copied()
                    .or_else(|| {
                        state.drawings[&batch.drawing_id]
                            .layers
                            .values()
                            .find(|l| l.name == incoming.layer_name)
                            .map(|l| l.id)
                    })
                    .ok_or_else(|| {
                        DraftError::new(
                            "layer_not_found",
                            format!("import layer {} is absent", incoming.layer_name),
                        )
                    })?;
                // A layer listed by the import is either an unlocked reused
                // layer or a newly allocated import layer. An unlisted layer
                // is an ordinary existing target and must respect its lock.
                if !layers.contains_key(&incoming.layer_name)
                    && state.drawings[&batch.drawing_id].layers[&layer_id].locked
                {
                    return Err(DraftError::new(
                        "locked_layer",
                        "import target is an existing locked layer",
                    ));
                }
                let id = state.next_ids.entity()?;
                let entity = DraftEntity {
                    id,
                    layer_id,
                    geometry: incoming.geometry.clone(),
                    style: incoming.style.clone(),
                    revision,
                    source: Some(EntityProvenance {
                        import_asset_id: asset_id,
                        source_handle: incoming.source_handle.clone(),
                    }),
                };
                drawing_mut(state, batch.drawing_id)?
                    .entities
                    .insert(id, entity.clone());
                changes.push(DraftChange::Entity {
                    drawing_id: batch.drawing_id,
                    id,
                    before: None,
                    after: Some(entity),
                });
            }
            drawing_mut(state, batch.drawing_id)?.revision = revision;
        }
    }
    Ok(())
}
fn summarize(
    command_id: &str,
    before: u64,
    after: u64,
    changes: &[DraftChange],
    inverse: bool,
) -> CommandReceipt {
    let mut receipt = CommandReceipt {
        command_id: command_id.into(),
        revision_before: before,
        revision_after: after,
        ..Default::default()
    };
    let mut states: BTreeMap<DraftEntityId, (bool, bool)> = BTreeMap::new();
    let mut layers = BTreeSet::new();
    let mut assets = BTreeSet::new();
    let mut drawings = BTreeSet::new();
    let ordered: Vec<&DraftChange> = if inverse {
        changes.iter().rev().collect()
    } else {
        changes.iter().collect()
    };
    for change in ordered {
        match change {
            DraftChange::Entity {
                id, before, after, ..
            } => {
                let (a, b) = if inverse {
                    (after.is_some(), before.is_some())
                } else {
                    (before.is_some(), after.is_some())
                };
                states
                    .entry(*id)
                    .and_modify(|pair| pair.1 = b)
                    .or_insert((a, b));
            }
            DraftChange::Layer { id, .. } => {
                layers.insert(*id);
            }
            DraftChange::ImportAsset { id, .. } => {
                assets.insert(*id);
            }
            DraftChange::Drawing {
                drawing_id,
                before,
                after,
            } => {
                if (if inverse { before } else { after }).is_some() {
                    drawings.insert(*drawing_id);
                }
            }
        }
    }
    for (id, (a, b)) in states {
        match (a, b) {
            (false, true) => receipt.created_ids.push(id),
            (true, false) => receipt.deleted_ids.push(id),
            (true, true) => receipt.updated_ids.push(id),
            _ => {}
        }
    }
    receipt.changed_layer_ids = layers.into_iter().collect();
    receipt.import_asset_ids = assets.into_iter().collect();
    receipt.created_drawing_ids = drawings.into_iter().collect();
    receipt
}
fn entity_eq(a: &Option<DraftEntity>, b: &Option<DraftEntity>) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    if let Some(e) = &mut a {
        e.revision = 0;
    }
    if let Some(e) = &mut b {
        e.revision = 0;
    }
    a == b
}
fn drawing_eq(a: &Option<DraftDrawing>, b: &Option<DraftDrawing>) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    for d in [&mut a, &mut b].into_iter().flatten() {
        d.revision = 0;
        for e in d.entities.values_mut() {
            e.revision = 0;
        }
    }
    a == b
}
fn semantic_state_eq(a: &DocumentState, b: &DocumentState) -> bool {
    a.document_id == b.document_id
        && a.drawings.len() == b.drawings.len()
        && a.drawings
            .iter()
            .all(|(id, d)| drawing_eq(&Some(d.clone()), &b.drawings.get(id).cloned()))
        && a.next_ids == b.next_ids
}
fn validate_change_identity(change: &DraftChange, counters: &IdAllocators) -> DraftResult<()> {
    let valid = match change {
        DraftChange::Entity {
            drawing_id,
            id,
            before,
            after,
        } => {
            drawing_id.get() > 0
                && drawing_id.get() <= counters.drawing_high_water
                && id.get() > 0
                && id.get() <= counters.entity_high_water
                && before.iter().chain(after).all(|e| e.id == *id)
        }
        DraftChange::Layer {
            drawing_id,
            id,
            before,
            after,
        } => {
            drawing_id.get() > 0
                && drawing_id.get() <= counters.drawing_high_water
                && id.get() > 0
                && id.get() <= counters.layer_high_water
                && before.iter().chain(after).all(|l| l.id == *id)
        }
        DraftChange::ImportAsset {
            drawing_id,
            id,
            before,
            after,
        } => {
            drawing_id.get() > 0
                && drawing_id.get() <= counters.drawing_high_water
                && id.get() > 0
                && id.get() <= counters.import_high_water
                && before.iter().chain(after).all(|a| a.id == *id)
        }
        DraftChange::Drawing {
            drawing_id,
            before,
            after,
        } => {
            drawing_id.get() > 0
                && drawing_id.get() <= counters.drawing_high_water
                && before.iter().chain(after).all(|d| d.id == *drawing_id)
        }
    };
    if !valid {
        return Err(DraftError::new(
            "invalid_history",
            "change has conflicting IDs or exceeds high-water",
        ));
    }
    Ok(())
}
fn validate_change_payload(change: &DraftChange, revision: u64) -> DraftResult<()> {
    match change {
        DraftChange::Entity { before, after, .. } => {
            for e in before.iter().chain(after) {
                e.geometry.validate(GeometryLimits::default())?;
                e.style.validate()?;
                if e.revision > revision {
                    return Err(DraftError::new(
                        "invalid_history",
                        "historical entity revision exceeds document",
                    ));
                }
            }
        }
        DraftChange::Layer { before, after, .. } => {
            for layer in before.iter().chain(after) {
                validate_name(&layer.name)?;
                layer.style.validate()?;
            }
        }
        DraftChange::ImportAsset { before, after, .. } => {
            for asset in before.iter().chain(after) {
                asset.source.validate()?;
            }
        }
        DraftChange::Drawing { before, after, .. } => {
            for d in before.iter().chain(after) {
                validate_name(&d.name)?;
                d.plane.validate()?;
                for e in d.entities.values() {
                    e.geometry.validate(GeometryLimits::default())?;
                    e.style.validate()?;
                }
                for l in d.layers.values() {
                    validate_name(&l.name)?;
                    l.style.validate()?;
                }
            }
        }
    }
    Ok(())
}
fn apply_changes(
    state: &mut DocumentState,
    changes: &[DraftChange],
    forward: bool,
    revision: u64,
    strict: bool,
) -> DraftResult<()> {
    let ordered: Vec<&DraftChange> = if forward {
        changes.iter().collect()
    } else {
        changes.iter().rev().collect()
    };
    for change in ordered {
        match change {
            DraftChange::Entity {
                drawing_id,
                id,
                before,
                after,
            } => {
                let (expected, target) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                let drawing = drawing_mut(state, *drawing_id)?;
                if strict && !entity_eq(&drawing.entities.get(id).cloned(), expected) {
                    return Err(DraftError::new(
                        "history_state_mismatch",
                        format!("entity {id} differs from journal expectation"),
                    ));
                }
                if let Some(e) = target {
                    let mut entity = e.clone();
                    entity.revision = revision;
                    drawing.entities.insert(*id, entity);
                } else {
                    drawing.entities.remove(id);
                }
                drawing.revision = revision;
            }
            DraftChange::Layer {
                drawing_id,
                id,
                before,
                after,
            } => {
                let (expected, target) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                let drawing = drawing_mut(state, *drawing_id)?;
                if strict && &drawing.layers.get(id).cloned() != expected {
                    return Err(DraftError::new(
                        "history_state_mismatch",
                        "layer differs from journal expectation",
                    ));
                }
                if let Some(layer) = target {
                    drawing.layers.insert(*id, layer.clone());
                } else {
                    drawing.layers.remove(id);
                }
                drawing.revision = revision;
            }
            DraftChange::ImportAsset {
                drawing_id,
                id,
                before,
                after,
            } => {
                let (expected, target) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                let drawing = drawing_mut(state, *drawing_id)?;
                if strict && &drawing.import_sources.get(id).cloned() != expected {
                    return Err(DraftError::new(
                        "history_state_mismatch",
                        "import asset differs from journal expectation",
                    ));
                }
                if let Some(asset) = target {
                    drawing.import_sources.insert(*id, asset.clone());
                } else {
                    drawing.import_sources.remove(id);
                }
                drawing.revision = revision;
            }
            DraftChange::Drawing {
                drawing_id,
                before,
                after,
            } => {
                let (expected, target) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                if strict && !drawing_eq(&state.drawings.get(drawing_id).cloned(), expected) {
                    return Err(DraftError::new(
                        "history_state_mismatch",
                        "drawing differs from journal expectation",
                    ));
                }
                if let Some(d) = target {
                    let mut drawing = d.clone();
                    drawing.revision = revision;
                    for e in drawing.entities.values_mut() {
                        e.revision = revision;
                    }
                    state.drawings.insert(*drawing_id, drawing);
                } else {
                    state.drawings.remove(drawing_id);
                }
            }
        }
    }
    Ok(())
}
