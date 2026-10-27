//! In-place transactions with bounded inverse journals. No document clone is
//! needed for commit, undo or redo; allocation and dirty state are restored on
//! every error and the history stacks move only after successful validation.
use crate::*;

impl Document {
    pub fn commit(&mut self, transaction: Transaction) -> Result<CommitReceipt, CoreError> {
        let start = Instant::now();
        if transaction.command_id.is_empty() {
            return Err(CoreError::EmptyCommandId);
        }
        if transaction.commands.is_empty() {
            return Err(CoreError::EmptyTransaction);
        }
        if transaction.commands.len() > u32::MAX as usize {
            return Err(CoreError::TransactionTooLarge);
        }
        self.reserve_circular_commit(&transaction.command_id, &transaction.commands)?;
        if transaction.commands.iter().any(|c| matches!(c, Command::GeoFoundation(_))) {
            circular_archive::bounded_serialized_size(&transaction.commands, MAX_GEO_FOUNDATION_DOMAIN_BYTES)?;
        }
        if transaction.commands.iter().any(|c| matches!(c, Command::FoundationStructural(_) | Command::StructuralConfigurator(_))) {
            circular_archive::bounded_serialized_size(&transaction.commands, MAX_FOUNDATION_STRUCTURAL_DOMAIN_BYTES)?;
        }
        let payload_json = serde_json::to_string(&transaction.commands)?;
        if let Some(previous) = self.command_receipts.get(&transaction.command_id) {
            self.verify_receipt_payload(&transaction.command_id, previous, None)?;
            if previous.receipt.revision > self.revision && previous.recorded_commands.as_ref().is_some_and(|commands|
                commands.iter().any(|command| matches!(command, Command::Engineering(EngineeringCommand::SaveCircularSnapshot { .. }) | Command::FoundationStructural(_) | Command::StructuralConfigurator(_) | Command::GeoFoundation(_)))) {
                return Err(CoreError::InvalidDocument("future circular archive receipt cannot satisfy an idempotent retry in this selected revision".into()));
            }
            if previous.payload_json != payload_json {
                return Err(CoreError::CommandIdReused(transaction.command_id));
            }
            let mut receipt = previous.receipt.clone();
            receipt.replayed = true;
            return Ok(receipt);
        }
        if self.revision != transaction.expected_revision {
            return Err(CoreError::RevisionConflict {
                expected: transaction.expected_revision,
                actual: self.revision,
            });
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        self.scan_counters.reset();
        self.indexes();
        let counters = self.allocation_counters();
        let old_dirty = std::mem::take(&mut self.dirty_elements);
        let old_revision = self.revision;
        let recorded_commands = transaction.commands.clone();
        let mut changes = Vec::new();
        let mut operation_ids = Vec::with_capacity(transaction.commands.len());
        let mut created_elements = Vec::new();
        let application = (|| -> Result<(), CoreError> {
            for (index, command) in transaction.commands.into_iter().enumerate() {
                let operation_id = OperationId(self.next_operation_id);
                self.next_operation_id = self
                    .next_operation_id
                    .checked_add(1)
                    .ok_or(CoreError::IdExhausted("operation"))?;
                let (kind, parent, inputs, outputs, label, command_changes, created) =
                    self.apply_command(command, operation_id)?;
                for c in &command_changes {
                    self.index_nonrelationship_change(c, true);
                }
                changes.extend(command_changes);
                created_elements.extend(created);
                let operation = OperationRecord {
                    id: operation_id,
                    command_id: transaction.command_id.clone(),
                    command_index: index as u32,
                    revision: next_revision,
                    parent,
                    inputs,
                    outputs,
                    label,
                    kind,
                };
                let c = Change::Operation {
                    id: operation_id,
                    before: None,
                    after: Some(operation.clone()),
                };
                self.index_nonrelationship_change(&c, true);
                self.operations.insert(operation_id, operation);
                changes.push(c);
                operation_ids.push(operation_id);
            }
            Ok(())
        })();
        if let Err(e) = application {
            self.rollback_journal(&changes, old_revision, &counters, old_dirty);
            return Err(e);
        }
        let result = self.finalize_journal(old_revision, next_revision, &changes);
        let (affected_ids, dirty_ids, dirty_us, delta) = match result {
            Ok(value) => value,
            Err(e) => {
                self.rollback_journal(&changes, old_revision, &counters, old_dirty);
                return Err(e);
            }
        };
        let archive_usage = match self.check_circular_pending(&changes, Some(&recorded_commands), payload_json.len()) {
            Ok(usage) => usage,
            Err(error) => {
                self.rollback_journal(&changes, old_revision, &counters, old_dirty);
                return Err(error);
            }
        };
        if let Err(error) = self.check_foundation_structural_pending(&changes, Some(&recorded_commands), payload_json.len()) {
            self.rollback_journal(&changes, old_revision, &counters, old_dirty);
            return Err(error);
        }
        if let Err(error) = self.check_geo_foundation_pending(&changes, Some(&recorded_commands), payload_json.len()) {
            self.rollback_journal(&changes, old_revision, &counters, old_dirty);
            return Err(error);
        }
        let mut receipt = CommitReceipt {
            affected_objects: qualified_affected(
                &affected_ids,
                &delta.drafting,
                &delta.engineering,
                &delta.foundation_structural,
                &delta.structural_configurator,
                &delta.geo_foundation,
            ),
            created_drawings: delta.drafting.created_drawings.clone(),
            created_draft_entities: delta.drafting.created_entities.clone(),
            transaction_id: transaction.command_id.clone(),
            revision_before: old_revision,
            revision_after: next_revision,
            affected_ids,
            dirty_ids,
            validation_results: Vec::new(),
            created_types: changes
                .iter()
                .filter_map(|c| {
                    if let Change::Type {
                        id,
                        before: None,
                        after: Some(_),
                    } = c
                    {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect(),
            created_levels: changes
                .iter()
                .filter_map(|c| {
                    if let Change::Level {
                        id,
                        before: None,
                        after: Some(_),
                    } = c
                    {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect(),
            created_relationships: changes
                .iter()
                .filter_map(|c| {
                    if let Change::Relationship {
                        id,
                        before: None,
                        after: Some(_),
                    } = c
                    {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect(),
            command_id: transaction.command_id.clone(),
            revision: next_revision,
            operation_ids: operation_ids.clone(),
            created_elements,
            replayed: false,
            transaction_duration_us: 0,
            dirty_propagation_duration_us: dirty_us,
            scanned_relationship_count: self.scan_counters.relationship_count(),
            document_scan_count: self.scan_counters.document_count(),
            snapshot_serialization_duration_us: self.scan_counters.snapshot_duration(),
            snapshot_serialized_bytes: self.scan_counters.snapshot_bytes(),
            delta,
            performance: self.core_performance(dirty_us, changes.len()),
        };
        self.undo_stack.push(HistoryEntry {
            command_id: transaction.command_id.clone(),
            operation_ids,
            changes,
        });
        self.redo_stack.clear();
        receipt.transaction_duration_us = elapsed_us(start);
        self.command_receipts.insert(
            transaction.command_id,
            StoredCommand {
                payload_json,
                recorded_commands: Some(recorded_commands),
                receipt: receipt.clone(),
            },
        );
        self.circular_archive_usage = OnceLock::from(archive_usage);
        Ok(receipt)
    }
    pub fn undo(&mut self) -> Result<Option<HistoryReceipt>, CoreError> {
        self.move_history(false)
    }
    pub fn redo(&mut self) -> Result<Option<HistoryReceipt>, CoreError> {
        self.move_history(true)
    }

    fn move_history(&mut self, redo: bool) -> Result<Option<HistoryReceipt>, CoreError> {
        let start = Instant::now();
        let entry = if redo {
            self.redo_stack.last()
        } else {
            self.undo_stack.last()
        };
        let Some(entry) = entry else {
            return Ok(None);
        };
        self.reserve_circular_history(entry)?;
        let command_id = entry.command_id.clone();
        let operation_ids = entry.operation_ids.clone();
        let changes = if redo { entry.changes.clone() }
            else { entry.changes.iter().rev().map(inverse_change).collect() };
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        self.scan_counters.reset();
        self.indexes();
        let counters = self.allocation_counters();
        let old_dirty = std::mem::take(&mut self.dirty_elements);
        let old_revision = self.revision;
        for c in &changes {
            self.apply_change(c, true);
        }
        let (affected_ids, dirty_ids, dirty_us, delta) =
            match self.finalize_journal(old_revision, next_revision, &changes) {
                Ok(value) => value,
                Err(e) => {
                    self.rollback_journal(&changes, old_revision, &counters, old_dirty);
                    return Err(e);
                }
            };
        let archive_usage = match self.check_circular_pending(&changes, None, 0) {
            Ok(usage) => usage,
            Err(error) => {
                self.rollback_journal(&changes, old_revision, &counters, old_dirty);
                return Err(error);
            }
        };
        if let Err(error) = self.check_foundation_structural_pending(&changes, None, 0) {
            self.rollback_journal(&changes, old_revision, &counters, old_dirty);
            return Err(error);
        }
        if let Err(error) = self.check_geo_foundation_pending(&changes, None, 0) {
            self.rollback_journal(&changes, old_revision, &counters, old_dirty);
            return Err(error);
        }
        let receipt = HistoryReceipt {
            affected_objects: qualified_affected(
                &affected_ids,
                &delta.drafting,
                &delta.engineering,
                &delta.foundation_structural,
                &delta.structural_configurator,
                &delta.geo_foundation,
            ),
            transaction_id: command_id.clone(),
            revision_before: old_revision,
            revision_after: next_revision,
            affected_ids,
            dirty_ids,
            validation_results: Vec::new(),
            command_id,
            revision: next_revision,
            operation_ids,
            transaction_duration_us: elapsed_us(start),
            dirty_propagation_duration_us: dirty_us,
            scanned_relationship_count: self.scan_counters.relationship_count(),
            document_scan_count: self.scan_counters.document_count(),
            snapshot_serialization_duration_us: self.scan_counters.snapshot_duration(),
            snapshot_serialized_bytes: self.scan_counters.snapshot_bytes(),
            delta,
            performance: self.core_performance(dirty_us, changes.len()),
        };
        if redo {
            let entry = self.redo_stack.pop().expect("checked history entry");
            self.undo_stack.push(entry);
        } else {
            let entry = self.undo_stack.pop().expect("checked history entry");
            self.redo_stack.push(entry);
        }
        self.circular_archive_usage = OnceLock::from(archive_usage);
        Ok(Some(receipt))
    }
    fn rollback_journal(
        &mut self,
        changes: &[Change],
        revision: u64,
        counters: &AllocationCounters,
        dirty: BTreeSet<ElementId>,
    ) {
        for c in changes.iter().rev() {
            self.apply_change(c, false);
        }
        if self.revision > revision {
            self.revision_states.remove(&self.revision);
        }
        self.revision = revision;
        self.restore_counters(counters);
        self.dirty_elements = dirty;
        self.refresh_format_version();
        self.circular_archive_usage = OnceLock::new();
        // Failed batches retain no derived generation or diagnostic observations.
        // The restored authoritative source will be generated cold on its next
        // explicit update; inverse journals never contain this transient work.
        self.configurator_generator_runtime =
            configured_building::ConfiguratorGeneratorRuntime::default();
    }
    fn finalize_journal(
        &mut self,
        old_revision: u64,
        next_revision: u64,
        changes: &[Change],
    ) -> Result<(Vec<ElementId>, Vec<ElementId>, u64, TransactionDelta), CoreError> {
        let mut affected: BTreeSet<_> = self.affected_from_changes(changes).into_iter().collect();
        // Revisit only journal entries to capture old resolved values and graph
        // influence. This handles reassigned types and detached hosts exactly.
        for c in changes.iter().rev() {
            self.apply_change(c, false);
        }
        affected.extend(self.affected_from_changes(changes));
        let affected: Vec<_> = affected.into_iter().collect();
        let before_result = self.bounded_projections(&affected);
        for c in changes {
            self.apply_change(c, true);
        }
        let before = before_result?;
        self.revision = next_revision;
        if changes.iter().any(|c| matches!(c,
            Change::EngineeringGroundSection { .. } | Change::CircularEngineeringCase { .. })) {
            self.format_version = self.format_version.max(4);
        } else if changes.iter().any(|c| matches!(c, Change::EngineeringCase { .. })) {
            self.format_version = self.format_version.max(3);
        }
        if changes.iter().any(|c| matches!(c, Change::FoundationStructural { .. })) {
            self.format_version = self.format_version.max(FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION);
        }
        if changes.iter().any(|c| matches!(c, Change::StructuralConfigurator { .. })) { self.format_version = self.format_version.max(STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION); }
        if changes.iter().any(|c| matches!(c, Change::GeoFoundation { .. })) { self.format_version = GEO_FOUNDATION_NATIVE_FORMAT_VERSION; }
        let dirty_start = Instant::now();
        let dirty = self.dirty_from_projections(&affected, &before);
        let dirty_us = elapsed_us(dirty_start);
        self.validate_local_changes(changes, &affected, &dirty)?;
        let delta = self.make_delta(old_revision, changes, &affected, &before)?;
        self.record_revision_delta(changes, &delta)?;
        self.dirty_elements = dirty.iter().copied().collect();
        Ok((affected, dirty, dirty_us, delta))
    }
}
pub(super) fn inverse_change(c: &Change) -> Change {
    match c {
        Change::GeoFoundation { before, after } => Change::GeoFoundation { before: after.clone(), after: before.clone() },
        Change::StructuralConfigurator { before, after } => Change::StructuralConfigurator { before: after.clone(), after: before.clone() },
        Change::FoundationStructural { before, after } => Change::FoundationStructural { before: after.clone(), after: before.clone() },
        Change::EngineeringGroundSection { id, before, after } => Change::EngineeringGroundSection {
            id: *id, before: after.clone(), after: before.clone(),
        },
        Change::CircularEngineeringCase { id, before, after } => Change::CircularEngineeringCase {
            id: *id, before: after.clone(), after: before.clone(),
        },
        Change::EngineeringCase { id, before, after } => Change::EngineeringCase {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Draft(change) => Change::Draft(inverse_draft_change(change)),
        Change::DraftDrawingMetadata { id, before, after } => Change::DraftDrawingMetadata {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::DraftView { id, before, after } => Change::DraftView {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Class { id, before, after } => Change::Class {
            id: id.clone(),
            before: after.clone(),
            after: before.clone(),
        },
        Change::Element { id, before, after } => Change::Element {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Operation { id, before, after } => Change::Operation {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Type { id, before, after } => Change::Type {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Level { id, before, after } => Change::Level {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        Change::Relationship { id, before, after } => Change::Relationship {
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
    }
}
