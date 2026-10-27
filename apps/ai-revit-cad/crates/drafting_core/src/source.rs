//! Read-only adapter from an authoritative document to derived drafting state.
//!
//! Implementations own transactions and history. They expose borrowed analytic
//! drawings; neither navigation nor cache reads require a materialized snapshot.
use crate::*;

/// Trusted, immutable authority for the drafting cache.
///
/// A project implements this on its existing authoritative document, not on a
/// second drafting session. `confirmed_drafting_changes` must read the actual
/// committed project journal, including undo/redo, and reject a missing or
/// nonadjacent revision. A BIM-only revision returns an empty change vector.
/// The cache requests this feed itself; callers cannot submit a receipt or a
/// fabricated vector to the generic synchronization API.
pub trait DrawingSource {
    fn drawing(&self, id: DrawingId) -> Option<&DraftDrawing>;

    /// Includes persistent document identity and a fresh, nonpersistent runtime
    /// identity for each load/clone, together with the caller's document epoch.
    fn cache_source_key(&self, id: DrawingId, epoch: u64) -> DraftResult<CacheSourceKey>;

    /// Explicit export boundary only. Interactive cache operations never call it.
    fn snapshot(&self, id: DrawingId) -> DraftResult<DraftSnapshot>;

    /// Return the actual applied drafting changes between this revision and the
    /// current adjacent project revision. Do not derive this from a second log.
    fn confirmed_drafting_changes(
        &self,
        from_project_revision: u64,
    ) -> DraftResult<Vec<DraftChange>>;
}

impl DrawingSource for LabDocument {
    fn drawing(&self, id: DrawingId) -> Option<&DraftDrawing> {
        self.drawings().get(&id)
    }

    fn cache_source_key(&self, id: DrawingId, epoch: u64) -> DraftResult<CacheSourceKey> {
        LabDocument::cache_source_key(self, id, epoch)
    }

    fn snapshot(&self, id: DrawingId) -> DraftResult<DraftSnapshot> {
        LabDocument::snapshot(self, id)
    }

    fn confirmed_drafting_changes(
        &self,
        from_project_revision: u64,
    ) -> DraftResult<Vec<DraftChange>> {
        if from_project_revision.checked_add(1) != Some(self.revision()) {
            return Err(DraftError::new(
                "cache_revision_gap",
                "confirmed journal must be adjacent to the current document",
            ));
        }
        let record = self.receipts.values().find(|record| {
            record.receipt.revision_before == from_project_revision
                && record.receipt.revision_after == self.revision()
        }).ok_or_else(|| DraftError::new(
            "cache_receipt_mismatch", "current revision has no authoritative receipt",
        ))?;
        let (index, inverse) = match &record.action {
            ReplayAction::Apply { expected_revision, .. }
                if *expected_revision == from_project_revision =>
            {
                (self.history_cursor.checked_sub(1), false)
            }
            ReplayAction::Undo { expected_revision }
                if *expected_revision == from_project_revision =>
            {
                (Some(self.history_cursor), true)
            }
            ReplayAction::Redo { expected_revision }
                if *expected_revision == from_project_revision =>
            {
                (self.history_cursor.checked_sub(1), false)
            }
            _ => return Err(DraftError::new(
                "cache_history_mismatch", "receipt cannot authorize the current journal",
            )),
        };
        let history = index.and_then(|index| self.history.get(index)).ok_or_else(|| {
            DraftError::new("cache_history_mismatch", "current history event is absent")
        })?;
        let matches = match &record.action {
            ReplayAction::Apply { .. } => {
                history.command_id == record.receipt.command_id
                    && history.revision_before == record.receipt.revision_before
                    && history.revision_after == record.receipt.revision_after
            }
            ReplayAction::Undo { .. } | ReplayAction::Redo { .. } => {
                record.target_command_id.as_deref() == Some(history.command_id.as_str())
            }
            ReplayAction::Pruned { .. } => false,
        };
        if !matches {
            return Err(DraftError::new(
                "cache_history_mismatch", "receipt and current history event disagree",
            ));
        }
        if inverse {
            Ok(history.changes.iter().rev().map(inverse_change).collect())
        } else {
            Ok(history.changes.clone())
        }
    }
}

fn inverse_change(change: &DraftChange) -> DraftChange {
    match change {
        DraftChange::Entity { drawing_id, id, before, after } => DraftChange::Entity {
            drawing_id: *drawing_id, id: *id, before: after.clone(), after: before.clone(),
        },
        DraftChange::Layer { drawing_id, id, before, after } => DraftChange::Layer {
            drawing_id: *drawing_id, id: *id, before: after.clone(), after: before.clone(),
        },
        DraftChange::ImportAsset { drawing_id, id, before, after } => DraftChange::ImportAsset {
            drawing_id: *drawing_id, id: *id, before: after.clone(), after: before.clone(),
        },
        DraftChange::Drawing { drawing_id, before, after } => DraftChange::Drawing {
            drawing_id: *drawing_id, before: after.clone(), after: before.clone(),
        },
    }
}
