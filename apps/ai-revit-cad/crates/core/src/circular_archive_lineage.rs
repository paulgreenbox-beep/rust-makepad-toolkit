//! Prospective archive copies created by circular case commands in a transaction.
//!
//! This ledger follows only snapshot byte sizes for touched analyses. It does
//! not clone cases or archives and does not inspect unrelated document history.

use crate::circular_archive_budget::{
    audit_circular_archive_retention as audit, MAX_CIRCULAR_RETAINED_BYTES,
};
use crate::{AnalysisCaseId, Command, CoreError, Document, EngineeringCommand};
use std::collections::BTreeMap;

const TRANSIENT_SCOPE: &str = "transaction transient reservation";

fn reservation_limit() -> CoreError {
    CoreError::CircularArchiveLimit {
        scope: TRANSIENT_SCOPE,
        limit: MAX_CIRCULAR_RETAINED_BYTES,
    }
}

fn add_bytes(left: usize, right: usize) -> Result<usize, CoreError> {
    let total = left.checked_add(right).ok_or_else(reservation_limit)?;
    if total > MAX_CIRCULAR_RETAINED_BYTES {
        return Err(reservation_limit());
    }
    Ok(total)
}

fn copied_bytes(bytes: usize, copies: usize) -> Result<usize, CoreError> {
    bytes.checked_mul(copies).ok_or_else(reservation_limit)
}

fn staged_snapshot_bytes(
    document: &Document,
    ledger: &mut BTreeMap<AnalysisCaseId, usize>,
    analysis_id: AnalysisCaseId,
) -> Result<usize, CoreError> {
    if let Some(bytes) = ledger.get(&analysis_id) {
        return Ok(*bytes);
    }

    let bytes = document
        .circular_case(analysis_id)
        .map(audit)
        .transpose()?
        .map_or(0, |usage| usage.snapshot_bytes);
    ledger.insert(analysis_id, bytes);
    Ok(bytes)
}

/// Estimate additional copies of existing or staged snapshots made by the
/// circular case journal when applying commands in order.
///
/// The ordinary per-command allocation reservation is accounted separately by
/// `reserve_circular_commit`; this helper covers six copies of each prior
/// staged snapshot for Update and Save journal lineage.
pub(crate) fn prospective_circular_clone_bytes(
    document: &Document,
    commands: &[Command],
) -> Result<usize, CoreError> {
    let mut ledger = BTreeMap::new();
    let mut clone_bytes = 0usize;
    let mut next_operation_id = document.next_operation_id;

    for command in commands {
        // Every command consumes one operation ID before application. A
        // CreateCircular case is allocated from that ID.
        let operation_id = next_operation_id;
        next_operation_id = next_operation_id
            .checked_add(1)
            .ok_or(CoreError::IdExhausted("operation"))?;

        let Command::Engineering(engineering_command) = command else {
            continue;
        };

        match engineering_command {
            EngineeringCommand::CreateCircular { .. } => {
                ledger.insert(AnalysisCaseId::new(operation_id), 0);
            }
            EngineeringCommand::UpdateCircular { analysis_id, .. } => {
                let prior = staged_snapshot_bytes(document, &mut ledger, *analysis_id)?;
                clone_bytes = add_bytes(clone_bytes, copied_bytes(prior, 6)?)?;
                // Update preserves the existing snapshot, including one staged
                // by an earlier SaveCircularSnapshot in this transaction.
            }
            EngineeringCommand::SaveCircularSnapshot {
                analysis_id,
                snapshot,
            } => {
                let prior = staged_snapshot_bytes(document, &mut ledger, *analysis_id)?;
                clone_bytes = add_bytes(clone_bytes, copied_bytes(prior, 6)?)?;
                let replacement = audit(snapshot)?.snapshot_bytes;
                ledger.insert(*analysis_id, replacement);
            }
            EngineeringCommand::CreateGround { .. }
            | EngineeringCommand::UpdateGround { .. }
            | EngineeringCommand::CreateSlope { .. }
            | EngineeringCommand::UpdateSlope { .. }
            | EngineeringCommand::SaveCalculationSnapshot { .. } => {}
        }
    }

    Ok(clone_bytes)
}
