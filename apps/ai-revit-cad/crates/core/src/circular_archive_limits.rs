//! Payload-equivalent resource admission. These are encoded byte reservations,
//! not allocator/RSS measurements. Equal hashes never merge owning copies.
use crate::circular_archive::bounded_serialized_size;
use crate::circular_archive_budget::audit_circular_archive_retention as audit;
use crate::*;

fn limit(scope: &'static str) -> CoreError {
    CoreError::CircularArchiveLimit {
        scope,
        limit: MAX_CIRCULAR_RETAINED_BYTES,
    }
}
fn add(a: usize, b: usize) -> Result<usize, CoreError> {
    a.checked_add(b)
        .ok_or_else(|| limit("reservation overflow"))
}
fn mul(a: usize, b: usize) -> Result<usize, CoreError> {
    a.checked_mul(b)
        .ok_or_else(|| limit("reservation overflow"))
}
fn admit(bytes: usize, scope: &'static str) -> Result<(), CoreError> {
    if bytes > MAX_CIRCULAR_RETAINED_BYTES {
        Err(limit(scope))
    } else {
        Ok(())
    }
}

impl Document {
    /// All owning typed snapshots and exact retained request-string bytes.
    /// This is a payload-equivalent quota, not a promise about process memory.
    pub fn circular_archive_retention_usage(&self) -> Result<ArchiveRetentionUsage, CoreError> {
        if let Some(usage) = self.circular_archive_usage.get() {
            return Ok(*usage);
        }
        self.audit_circular_archive_budget()
    }

    pub(crate) fn audit_circular_archive_budget(&self) -> Result<ArchiveRetentionUsage, CoreError> {
        let _scan = self.scan_counters.track(
            "circular_archive_retention_audit",
            self.elements.len(),
            self.relationships.len(),
            "explicit accounting of every retained circular archive owner and request string",
            true,
        );
        let usage = audit(self)?;
        admit(usage.total_bytes()?, "retained payload")?;
        let _ = self.circular_archive_usage.set(usage);
        Ok(usage)
    }

    /// Explicit conservative reservation for reconstruction, stacks and
    /// projections while the original document remains owned by the caller.
    /// A document close to the retained quota may fail a copying operation.
    pub(crate) fn reserve_circular_archive_audit(&self) -> Result<(), CoreError> {
        let usage = self.circular_archive_retention_usage()?;
        if usage.snapshot_occurrences == 0 {
            return Ok(());
        }
        let scratch = add(mul(usage.snapshot_bytes, 4)?, usage.payload_json_bytes)?;
        admit(
            add(usage.total_bytes()?, scratch)?,
            "history/audit transient reservation",
        )
    }

    /// Called before request serialization and before the first archive clone.
    pub(crate) fn reserve_circular_commit(
        &self,
        id: &CommandId,
        commands: &[Command],
    ) -> Result<(), CoreError> {
        let current = self.circular_archive_retention_usage()?;
        let incoming = audit(commands)?;
        if current.snapshot_occurrences == 0 && incoming.snapshot_occurrences == 0 {
            return Ok(());
        }
        let request = bounded_serialized_size(commands, MAX_CIRCULAR_RETAINED_BYTES)?;
        let base = add(current.snapshot_bytes, current.payload_json_bytes)?;
        if self.command_receipts.contains_key(id) {
            return admit(
                add(base, add(incoming.snapshot_bytes, request)?)?,
                "retry transient reservation",
            );
        }
        // Request, recorded command, live case/operation, inverse journal,
        // projection, revision delta, and temporary replacement copies.
        let mut extra = add(request, mul(incoming.snapshot_bytes, 9)?)?;
        // Later commands observe earlier staged saves in this transaction.
        // Reuse only the encoded size; every prospective owner is charged.
        extra = add(
            extra,
            circular_archive_lineage::prospective_circular_clone_bytes(self, commands)?,
        )?;
        let next = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        if self.revision_states.is_empty() || next % self.history_policy.checkpoint_interval == 0 {
            extra = add(
                extra,
                audit(&(&self.engineering, &self.operations))?.snapshot_bytes,
            )?;
        }
        extra = add(
            extra,
            current.max_snapshot_bytes.max(incoming.max_snapshot_bytes),
        )?;
        admit(add(base, extra)?, "transaction transient reservation")
    }

    pub(crate) fn reserve_circular_history(&self, entry: &HistoryEntry) -> Result<(), CoreError> {
        let current = self.circular_archive_retention_usage()?;
        if current.snapshot_occurrences == 0 {
            return Ok(());
        }
        let journal = audit(entry)?;
        let mut extra = add(mul(journal.snapshot_bytes, 4)?, current.max_snapshot_bytes)?;
        let next = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        if next % self.history_policy.checkpoint_interval == 0 {
            extra = add(
                extra,
                audit(&(&self.engineering, &self.operations))?.snapshot_bytes,
            )?;
        }
        admit(
            add(current.total_bytes()?, extra)?,
            "undo/redo transient reservation",
        )
    }

    /// Journal and original commands have not moved into their retained fields
    /// yet. Charge them now; failure is handled by the existing atomic rollback.
    pub(crate) fn check_circular_pending(
        &self,
        changes: &[Change],
        recorded: Option<&[Command]>,
        payload_bytes: usize,
    ) -> Result<ArchiveRetentionUsage, CoreError> {
        let pending = audit(&(changes, recorded))?;
        if let Some(known) = self.circular_archive_usage.get() {
            if known.snapshot_occurrences == 0 && pending.snapshot_occurrences == 0 {
                // Public mutations cannot introduce an archive outside the
                // supplied journal/original commands. Preserve the old bounded
                // local-mutation path without walking the entire legacy model.
                let mut usage = *known;
                usage.payload_json_bytes = add(usage.payload_json_bytes, payload_bytes)?;
                return Ok(usage);
            }
        }
        let current = self.audit_circular_archive_budget()?;
        let mut combined = current.combine(pending)?;
        combined.payload_json_bytes = add(combined.payload_json_bytes, payload_bytes)?;
        admit(combined.total_bytes()?, "pending retained payload")?;
        if recorded.is_some() {
            // A successful commit clears redo only after admission. Its copies
            // were charged above while still owned during the transaction.
            let cleared = audit(&self.redo_stack)?;
            combined.snapshot_occurrences -= cleared.snapshot_occurrences;
            combined.snapshot_bytes -= cleared.snapshot_bytes;
            combined.payload_json_bytes -= cleared.payload_json_bytes;
            // max_snapshot_bytes is a conservative cache; public audit reads
            // authoritative values after export/open. It cannot underreserve.
            Ok(combined)
        } else {
            Ok(current)
        }
    }
}

/// Exact byte comparison avoids reparsing a second owning archive from every
/// stored request during open/save/retry verification.
pub(crate) fn serialized_payload_matches<T: serde::Serialize + ?Sized>(
    value: &T,
    expected: &str,
) -> Result<bool, CoreError> {
    struct Compare<'a> {
        expected: &'a [u8],
        offset: usize,
        equal: bool,
    }
    impl std::io::Write for Compare<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let end = self
                .offset
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("payload byte overflow"))?;
            self.equal &= self.expected.get(self.offset..end) == Some(bytes);
            self.offset = end;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Compare {
        expected: expected.as_bytes(),
        offset: 0,
        equal: true,
    };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.equal && writer.offset == writer.expected.len())
}
