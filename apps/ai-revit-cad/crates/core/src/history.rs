//! Checkpoints plus bounded resolved projections and authoritative inverse
//! journals. Mesh data is owned by Geometry and never enters this history.
use crate::*;

impl Document {
    /// Owner audits may need an operation whose branch was undone or abandoned.
    /// Retry verification continues using its indexed originating revision; this
    /// retained scan is for explicit archive/history ownership audits only.
    pub(crate) fn retained_operation(&self, id: OperationId) -> Option<&OperationRecord> {
        fn from_changes(changes: &[Change], id: OperationId) -> Option<&OperationRecord> {
            changes.iter().find_map(|change| match change {
                Change::Operation { id: key, before, after } if *key == id =>
                    after.as_ref().or(before.as_ref()),
                _ => None,
            })
        }
        self.operations.get(&id).or_else(|| {
            self.revision_states.values().rev().find_map(|state| {
                state.operations.as_ref().and_then(|operations| operations.get(&id))
                    .or_else(|| state.delta.as_ref().and_then(|delta| from_changes(&delta.changes, id)))
            })
        }).or_else(|| {
            self.undo_stack.iter().chain(&self.redo_stack)
                .find_map(|entry| from_changes(&entry.changes, id))
        }).filter(|operation| operation.revision <= self.revision)
    }
    /// Replay raw journals once in revision order during explicit open/save
    /// audits. Local mutations never call this document/history-wide audit.
    /// Canonical summaries and resolved sources are recomputed from semantics,
    /// so omitted summary members cannot silently corrupt history.diff.
    pub(super) fn verify_history_replay_integrity(&self) -> Result<(), CoreError> {
        let _scan=self.scan_counters.track("historical_journal_replay_audit",self.revision_states.values().map(|s|s.elements.len()).sum(),self.revision_states.values().map(|s|s.relationships.len()).sum(),"explicit open/save reconstruction audit of retained journals, complete delta summaries and property sources",true);
        let mut model: Option<Document> = None;
        let mut prior_revision = None;
        let mut prior_counters = AllocationCounters {
            drafting: DraftAllocators::default(),
            element: 1,
            operation: 1,
            element_type: 1,
            level: 1,
            relationship: 1,
        };
        for (revision, state) in &self.revision_states {
            if let Some(anchor) = &state.history_stack_anchor {
                if !anchor.counters.retains_high_water(&prior_counters) {
                    return Err(CoreError::InvalidDocument(
                        "legacy history anchor loses historical high-water marks".into(),
                    ));
                }
                prior_counters = anchor.counters.clone();
            }
            if let Some(delta) = &state.delta {
                if !delta.counters.retains_high_water(&prior_counters) {
                    return Err(CoreError::InvalidDocument(
                        "historical allocation high-water marks move backward".into(),
                    ));
                }
                // A new commit may allocate fresh IDs; undo and redo restore
                // original records instead. Counter monotonicity alone cannot
                // detect a forged commit that reuses a deleted historical ID.
                let new_commit = delta.changes.iter().any(|change| {
                    matches!(change, Change::Operation { before: None, after: Some(operation), .. }
                        if operation.revision == delta.summary.revision_after)
                });
                if new_commit {
                    for change in &delta.changes {
                        check_fresh_draft_allocation(change, &prior_counters.drafting)?;
                        let allocated = match change {
                            Change::EngineeringCase {
                                id,
                                before: None,
                                after: Some(_),
                            } => Some((id.get(), prior_counters.operation)),
                            Change::EngineeringGroundSection {
                                id, before: None, after: Some(_),
                            } => Some((id.get(), prior_counters.operation)),
                            Change::CircularEngineeringCase {
                                id, before: None, after: Some(_),
                            } => Some((id.get(), prior_counters.operation)),
                            Change::Element {
                                id,
                                before: None,
                                after: Some(_),
                                ..
                            } => Some((id.get(), prior_counters.element)),
                            Change::Operation {
                                id,
                                before: None,
                                after: Some(_),
                                ..
                            } => Some((id.get(), prior_counters.operation)),
                            Change::Type {
                                id,
                                before: None,
                                after: Some(_),
                                ..
                            } => Some((id.get(), prior_counters.element_type)),
                            Change::Level {
                                id,
                                before: None,
                                after: Some(_),
                                ..
                            } => Some((id.get(), prior_counters.level)),
                            Change::Relationship {
                                id,
                                before: None,
                                after: Some(_),
                                ..
                            } => Some((id.get(), prior_counters.relationship)),
                            _ => None,
                        };
                        if allocated.is_some_and(|(id, minimum)| id < minimum) {
                            return Err(CoreError::InvalidDocument(
                                "new historical commit reuses an allocated ID below its high-water mark".into(),
                            ));
                        }
                    }
                }
                prior_counters = delta.counters.clone();
            }
            if let (Some(current), Some(delta)) = (&mut model, &state.delta) {
                if prior_revision != Some(delta.summary.revision_before) {
                    return Err(CoreError::InvalidDocument(
                        "compact history has a missing predecessor".into(),
                    ));
                }
                current.scan_counters.reset();
                let mut affected: BTreeSet<_> = current
                    .affected_from_changes(&delta.changes)
                    .into_iter()
                    .collect();
                // Compute the after dependency footprint, then restore it to
                // obtain bounded before projections without cloning a model.
                for change in &delta.changes {
                    current.validate_change_before(change)?;
                    if let Change::Operation { after: Some(OperationRecord {
                        kind: OperationKind::EngineeringChanged { analysis_id,
                            command: EngineeringCommand::SaveCircularSnapshot { snapshot, .. } }, ..
                    }), .. } = change {
                        snapshot.validate_for_current_document(current, *analysis_id)?;
                    }
                    current.apply_change(change, true);
                }
                affected.extend(current.affected_from_changes(&delta.changes));
                for change in delta.changes.iter().rev() {
                    current.apply_change(change, false);
                }
                let affected: Vec<_> = affected.into_iter().collect();
                let before = current.bounded_projections(&affected)?;
                for change in &delta.changes {
                    current.apply_change(change, true);
                }
                current.revision = *revision;
                current.restore_counters(&delta.counters);
                let dirty = current.dirty_from_projections(&affected, &before);
                current.validate_local_changes(&delta.changes, &affected, &dirty)?;
                // References belong to this reconstructed successor, after the
                // entire batch. A touched partial projection is not a domain.
                current.engineering.validate()?;
                current.verify_engineering_owners()?;
                current.validate_foundation_structural()?;
                current.validate_configurator()?;
                current.validate_geo_foundation()?;
                let canonical = current.make_delta(
                    delta.summary.revision_before,
                    &delta.changes,
                    &affected,
                    &before,
                )?;
                if canonical != delta.summary {
                    return Err(CoreError::InvalidDocument(format!(
                        "revision {revision} delta summary is incomplete or disagrees with semantic changes"
                    )));
                }
                current.verify_record_projection(state, &affected, &delta.changes)?;
            } else {
                // A legacy checkpoint lacks historical class/operation maps.
                // Continue preserving its snapshots; exact replay validation
                // resumes when a complete v0.4 checkpoint becomes available.
                model = if let (Some(classes), Some(operations), Some(instances)) =
                    (&state.classes, &state.operations, &state.instances)
                {
                    let mut current = Document::new(self.name.clone())?;
                    current.document_uuid = self.document_uuid;
                    current.revision = *revision;
                    current.classes = classes.clone();
                    current.operations = operations.clone();
                    current.elements = instances.clone();
                    current.types = state.types.clone();
                    current.levels = state.levels.clone();
                    current.drafting = state.drafting.clone().unwrap_or_default();
                    current.engineering = state.engineering.clone().unwrap_or_default();
                    current.foundation_structural = state.foundation_structural.clone().unwrap_or_default();
                    current.structural_configurator = state.structural_configurator.clone().unwrap_or_default();
                    current.geo_foundation = state.geo_foundation.clone().unwrap_or_default();
                    current.relationships = state
                        .relationship_records
                        .clone()
                        .unwrap_or_else(|| state.relationships.clone());
                    current.relationship_indexes = OnceLock::new();
                    current.indexes();
                    current.engineering.validate()?;
                    current.verify_engineering_owners()?;
                    if let Some(delta) = &state.delta {
                        current.restore_counters(&delta.counters);
                    } else {
                        current.restore_counters(&self.allocation_counters());
                    }
                    current.validate_foundation_structural()?;
                current.validate_configurator()?;
                    current.validate_geo_foundation()?;
                    Some(current)
                } else {
                    None
                };
            }
            prior_revision = Some(*revision);
        }
        if !self
            .allocation_counters()
            .retains_high_water(&prior_counters)
        {
            return Err(CoreError::InvalidDocument(
                "current allocation counters lose historical high-water marks".into(),
            ));
        }
        let mut commands = BTreeSet::new();
        self.verify_all_receipt_payloads()?;
        for (id, stored) in &self.command_receipts {
            if let Some(original) = self
                .revision_states
                .get(&stored.receipt.revision)
                .and_then(|s| s.delta.as_ref())
            {
                if stored.receipt.delta != original.summary {
                    return Err(CoreError::InvalidDocument(
                        "retained command receipt delta disagrees with its originating revision"
                            .into(),
                    ));
                }
                let operations: Vec<_> = original
                    .changes
                    .iter()
                    .filter_map(|c| {
                        if let Change::Operation {
                            id: operation_id,
                            before: None,
                            after: Some(operation),
                        } = c
                        {
                            if &operation.command_id == id {
                                Some(*operation_id)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                if operations != stored.receipt.operation_ids {
                    return Err(CoreError::InvalidDocument("retained command receipt operation IDs disagree with its originating revision".into()));
                }
            }
        }
        for entry in self.undo_stack.iter().chain(&self.redo_stack) {
            if !commands.insert(&entry.command_id) {
                return Err(CoreError::InvalidDocument(
                    "history stack repeats a command id".into(),
                ));
            }
            let receipt = self
                .command_receipts
                .get(&entry.command_id)
                .ok_or_else(|| {
                    CoreError::InvalidDocument(
                        "history entry has no retained command receipt".into(),
                    )
                })?;
            if entry.operation_ids != receipt.receipt.operation_ids {
                return Err(CoreError::InvalidDocument(
                    "history entry operation IDs disagree with its receipt".into(),
                ));
            }
            if let Some(original) = self
                .revision_states
                .get(&receipt.receipt.revision)
                .and_then(|s| s.delta.as_ref())
            {
                if entry.changes != original.changes {
                    return Err(CoreError::InvalidDocument(
                        "undo/redo entry disagrees with its originating revision journal".into(),
                    ));
                }
                if receipt.receipt.delta != original.summary {
                    return Err(CoreError::InvalidDocument(
                        "retained command receipt delta disagrees with its originating revision"
                            .into(),
                    ));
                }
            }
        }
        let expected_stacks = self.reconstruct_history_stacks(self.revision)?;
        if self.undo_stack != expected_stacks.undo || self.redo_stack != expected_stacks.redo {
            return Err(CoreError::InvalidDocument(
                "undo/redo stack order or membership disagrees with revision history".into(),
            ));
        }
        Ok(())
    }
    fn validate_change_before(&self, change: &Change) -> Result<(), CoreError> {
        let matches = match change {
            Change::GeoFoundation { before, .. } => &self.geo_foundation == before,
            Change::StructuralConfigurator { before, .. } => &self.structural_configurator == before,
            Change::FoundationStructural { before, .. } => &self.foundation_structural == before,
            Change::EngineeringCase { id, before, .. } => self.slope_case(*id) == before.as_ref(),
            Change::EngineeringGroundSection { id, before, .. } =>
                self.engineering.ground_sections.get(id) == before.as_ref(),
            Change::CircularEngineeringCase { id, before, .. } =>
                self.engineering.circular_cases.get(id) == before.as_ref(),
            Change::Draft(change) => self.drafting.matches_before(change),
            Change::DraftDrawingMetadata { id, before, .. } => self
                .drawing(*id)
                .is_some_and(|d| DrawingMetadata::of(d) == *before),
            Change::DraftView { id, before, .. } => self.drafting.views.get(id) == before.as_ref(),
            Change::Class { id, before, .. } => self.classes.get(id) == before.as_ref(),
            Change::Element { id, before, .. } => self.elements.get(id) == before.as_ref(),
            Change::Type { id, before, .. } => self.types.get(id) == before.as_ref(),
            Change::Level { id, before, .. } => self.levels.get(id) == before.as_ref(),
            Change::Relationship { id, before, .. } => {
                self.relationships.get(id) == before.as_ref()
            }
            Change::Operation { id, before, .. } => self.operations.get(id) == before.as_ref(),
        };
        if !matches {
            return Err(CoreError::InvalidDocument(
                "historical journal before record disagrees with reconstructed predecessor".into(),
            ));
        }
        Ok(())
    }
    fn verify_record_projection(
        &self,
        state: &RevisionState,
        affected: &[ElementId],
        changes: &[Change],
    ) -> Result<(), CoreError> {
        let expected = if state.checkpoint {
            self.revision_state()?
        } else {
            let mut expected = RevisionState {
                geo_foundation: geo_foundation::geo_foundation_projection(&self.geo_foundation, Some(changes)),
                structural_configurator: configured_building::configurator_projection(&self.structural_configurator, Some(changes)),
                foundation_structural: foundation_structural::foundation_structural_projection(&self.foundation_structural, Some(changes)),
                engineering: engineering::engineering_projection(&self.engineering, Some(changes)),
                checkpoint: false,
                instances: Some(BTreeMap::new()),
                property_sources: Some(BTreeMap::new()),
                ..RevisionState::default()
            };
            for id in affected {
                if self.elements.contains_key(id) {
                    let p = self.projection(*id)?;
                    let mut e = p.effective.expect("existing element");
                    e.properties = p
                        .properties
                        .iter()
                        .map(|(id, p)| (id.clone(), p.value.clone()))
                        .collect();
                    expected.elements.insert(*id, e);
                    expected
                        .instances
                        .as_mut()
                        .unwrap()
                        .insert(*id, p.instance.unwrap());
                    expected.property_sources.as_mut().unwrap().insert(
                        *id,
                        p.properties
                            .into_iter()
                            .map(|(id, p)| (id, p.source))
                            .collect(),
                    );
                }
            }
            for change in changes {
                match change {
                    Change::Type { id, .. } => {
                        if let Some(v) = self.types.get(id) {
                            expected.types.insert(*id, v.clone());
                        }
                    }
                    Change::Level { id, .. } => {
                        if let Some(v) = self.levels.get(id) {
                            expected.levels.insert(*id, v.clone());
                        }
                    }
                    Change::Relationship { id, .. } => {
                        if let Some(v) = self.relationships.get(id).filter(|r| r.is_active()) {
                            expected.relationships.insert(*id, v.clone());
                        }
                    }
                    _ => {}
                }
            }
            expected
        };
        if state.elements != expected.elements
            || state.geo_foundation != expected.geo_foundation
            || state.structural_configurator != expected.structural_configurator
            || state.foundation_structural != expected.foundation_structural
            || state.engineering != expected.engineering
            || state.drafting != expected.drafting
            || state.instances != expected.instances
            || state.property_sources != expected.property_sources
            || state.types != expected.types
            || state.levels != expected.levels
            || state.relationships != expected.relationships
            || state.classes != expected.classes
            || state.operations != expected.operations
            || state.relationship_records != expected.relationship_records
        {
            return Err(CoreError::InvalidDocument(
                "historical projection disagrees with authoritative journal replay".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn verify_delta_projection(
        &self,
        revision: u64,
        state: &RevisionState,
    ) -> Result<(), CoreError> {
        let Some(delta) = &state.delta else {
            return Ok(());
        };
        let fail = || {
            CoreError::InvalidDocument(format!(
                "revision {revision} delta disagrees with its persisted projection"
            ))
        };
        let mut elements = BTreeMap::new();
        let mut types = BTreeMap::new();
        let mut levels = BTreeMap::new();
        let mut relationships = BTreeMap::new();
        for c in &delta.changes {
            match c {
                Change::Element { id, before, after } => {
                    let entry = elements
                        .entry(*id)
                        .or_insert((before.clone(), after.clone()));
                    entry.1 = after.clone();
                }
                Change::Type { id, before, after } => {
                    let entry = types.entry(*id).or_insert((before.clone(), after.clone()));
                    entry.1 = after.clone();
                }
                Change::Level { id, before, after } => {
                    let entry = levels.entry(*id).or_insert((before.clone(), after.clone()));
                    entry.1 = after.clone();
                }
                Change::Relationship { id, before, after } => {
                    let entry = relationships
                        .entry(*id)
                        .or_insert((before.clone(), after.clone()));
                    entry.1 = after.clone();
                }
                _ => {}
            }
        }
        let affected: BTreeSet<_> = delta.summary.affected_elements.iter().copied().collect();
        if !state.checkpoint && state.elements.keys().any(|id| !affected.contains(id)) {
            return Err(fail());
        }
        for (id, (before, after)) in elements {
            if !affected.contains(&id) {
                return Err(fail());
            }
            if let Some(instances) = &state.instances {
                if instances.get(&id) != after.as_ref() {
                    return Err(fail());
                }
            }
            if let Some((_, Some(old), _)) =
                self.historical_element(delta.summary.revision_before, id)
            {
                if before.as_ref() != Some(&old) {
                    return Err(fail());
                }
            } else if before.is_none()
                && self
                    .historical_element(delta.summary.revision_before, id)
                    .is_some()
            {
                return Err(fail());
            }
            match (&before, &after) {
                (None, Some(_)) if !delta.summary.created_elements.contains(&id) => {
                    return Err(fail());
                }
                (Some(_), None) if !delta.summary.deleted_elements.contains(&id) => {
                    return Err(fail());
                }
                (Some(a), Some(b)) if a != b && !delta.summary.updated_elements.contains(&id) => {
                    return Err(fail());
                }
                _ => {}
            }
        }
        for (id, (before, after)) in types {
            if state.types.get(&id) != after.as_ref()
                || self.historical_type(delta.summary.revision_before, id) != before
            {
                return Err(fail());
            }
        }
        for (id, (before, after)) in levels {
            if state.levels.get(&id) != after.as_ref()
                || self.historical_level(delta.summary.revision_before, id) != before
            {
                return Err(fail());
            }
        }
        for (id, (before, after)) in relationships {
            if state.relationships.get(&id) != after.as_ref().filter(|r| r.is_active())
                || self
                    .historical_relationship(delta.summary.revision_before, id)
                    .as_ref()
                    != before.as_ref().filter(|r| r.is_active())
            {
                return Err(fail());
            }
        }
        for p in &delta.summary.property_changes {
            if let EntityRef::Element(id) = p.entity {
                let old = self.historical_element(delta.summary.revision_before, id);
                let before = old
                    .as_ref()
                    .and_then(|(e, _, _)| e.properties.get(&p.property));
                let source = old
                    .as_ref()
                    .and_then(|(_, _, s)| s.as_ref())
                    .and_then(|s| s.get(&p.property));
                let after = state
                    .elements
                    .get(&id)
                    .and_then(|e| e.properties.get(&p.property));
                let new_source = state
                    .property_sources
                    .as_ref()
                    .and_then(|m| m.get(&id))
                    .and_then(|s| s.get(&p.property));
                if before != p.before.as_ref()
                    || after != p.after.as_ref()
                    || (old
                        .as_ref()
                        .is_some_and(|(_, _, sources)| sources.is_some())
                        && source.copied() != p.source_before)
                    || new_source.copied() != p.source_after
                {
                    return Err(fail());
                }
            }
        }
        Ok(())
    }
    pub(super) fn record_revision_delta(
        &mut self,
        changes: &[Change],
        summary: &TransactionDelta,
    ) -> Result<(), CoreError> {
        let total_start = Instant::now();
        let checkpoint = self.revision_states.is_empty()
            || self.revision % self.history_policy.checkpoint_interval == 0;
        let mut state = if checkpoint {
            self.revision_state()?
        } else {
            self.scan_counters.document_scan();
            let ids = &summary.affected_elements;
            let _scan = self.scan_counters.track(
                "compact_revision_projection",
                ids.len(),
                changes
                    .iter()
                    .filter(|c| matches!(c, Change::Relationship { .. }))
                    .count(),
                "compact delta records only affected resolved elements and changed entities",
                false,
            );
            let mut state = RevisionState {
                geo_foundation: geo_foundation::geo_foundation_projection(&self.geo_foundation, Some(changes)),
                structural_configurator: configured_building::configurator_projection(&self.structural_configurator, Some(changes)),
                foundation_structural: foundation_structural::foundation_structural_projection(&self.foundation_structural, Some(changes)),
                engineering: engineering::engineering_projection(&self.engineering, Some(changes)),
                checkpoint: false,
                instances: Some(BTreeMap::new()),
                property_sources: Some(BTreeMap::new()),
                ..RevisionState::default()
            };
            for id in ids {
                if self.elements.contains_key(id) {
                    let p = self.projection(*id)?;
                    let mut effective = p.effective.expect("existing projection");
                    effective.properties = p
                        .properties
                        .iter()
                        .map(|(id, p)| (id.clone(), p.value.clone()))
                        .collect();
                    state.elements.insert(*id, effective);
                    state
                        .instances
                        .as_mut()
                        .expect("instances")
                        .insert(*id, p.instance.expect("instance"));
                    state.property_sources.as_mut().expect("sources").insert(
                        *id,
                        p.properties
                            .into_iter()
                            .map(|(id, p)| (id, p.source))
                            .collect(),
                    );
                }
            }
            for c in changes {
                match c {
                    Change::Type { id, .. } => {
                        if let Some(value) = self.types.get(id) {
                            state.types.insert(*id, value.clone());
                        }
                    }
                    Change::Level { id, .. } => {
                        if let Some(value) = self.levels.get(id) {
                            state.levels.insert(*id, value.clone());
                        }
                    }
                    Change::Relationship { id, .. } => {
                        if let Some(value) = self.relationships.get(id).filter(|r| r.is_active()) {
                            state.relationships.insert(*id, value.clone());
                        }
                    }
                    _ => {}
                }
            }
            state
        };
        state.delta = Some(RevisionDelta {
            summary: summary.clone(),
            changes: changes.to_vec(),
            counters: self.allocation_counters(),
        });
        let start = Instant::now();
        let bytes = circular_archive::bounded_serialized_size(&state, usize::MAX)?;
        self.scan_counters.snapshot(elapsed_us(start), bytes);
        self.scan_counters.snapshot_total(elapsed_us(total_start));
        self.revision_states.insert(self.revision, state);
        Ok(())
    }
    pub(super) fn reconstruct_revision_state(
        &self,
        revision: u64,
    ) -> Result<RevisionState, CoreError> {
        self.reserve_circular_archive_audit()?;
        if !self.revision_states.contains_key(&revision) {
            return Err(CoreError::UnknownRevision(revision));
        }
        let (base_id, base) = self
            .revision_states
            .range(..=revision)
            .rev()
            .find(|(_, s)| s.checkpoint)
            .ok_or_else(|| {
                CoreError::InvalidDocument("compact history has no preceding checkpoint".into())
            })?;
        let _scan = self.scan_counters.track(
            "history_checkpoint_reconstruction",
            base.elements.len(),
            base.relationships.len(),
            "requested historical document reconstruction from nearest checkpoint",
            true,
        );
        let mut state = base.clone();
        if *base_id == revision {
            state.checkpoint = true;
            return Ok(state);
        }
        for (_, entry) in self.revision_states.range((*base_id + 1)..=revision) {
            if entry.checkpoint {
                state = entry.clone();
                continue;
            }
            let d = entry
                .delta
                .as_ref()
                .ok_or_else(|| CoreError::InvalidDocument("compact revision lacks delta".into()))?;
            for id in &d.summary.deleted_elements {
                state.elements.remove(id);
                if let Some(m) = state.instances.as_mut() {
                    m.remove(id);
                }
                if let Some(m) = state.property_sources.as_mut() {
                    m.remove(id);
                }
            }
            for id in &d.summary.deleted_types {
                state.types.remove(id);
            }
            for id in &d.summary.deleted_levels {
                state.levels.remove(id);
            }
            for id in &d.summary.deleted_relationships {
                state.relationships.remove(id);
            }
            state.elements.extend(entry.elements.clone());
            state.types.extend(entry.types.clone());
            state.levels.extend(entry.levels.clone());
            state.relationships.extend(entry.relationships.clone());
            if let Some(m) = state.instances.as_mut() {
                if let Some(new) = &entry.instances {
                    m.extend(new.clone());
                }
            }
            if let Some(m) = state.property_sources.as_mut() {
                if let Some(new) = &entry.property_sources {
                    m.extend(new.clone());
                }
            }
            for c in &d.changes {
                match c {
                    Change::GeoFoundation { after, .. } => state.geo_foundation = if after.is_empty() { None } else { Some(after.clone()) },
                    Change::StructuralConfigurator { after, .. } => state.structural_configurator = if after.is_empty() { None } else { Some(after.clone()) },
                    Change::FoundationStructural { after, .. } => state.foundation_structural = if after.is_empty() { None } else { Some(after.clone()) },
                    Change::EngineeringCase { id, after, .. } => {
                        let domain = state
                            .engineering
                            .get_or_insert_with(EngineeringDomain::default);
                        set_optional(&mut domain.slope_cases, *id, after);
                    }
                    Change::EngineeringGroundSection { id, after, .. } => {
                        let domain = state.engineering.get_or_insert_with(EngineeringDomain::default);
                        set_optional(&mut domain.ground_sections, *id, after);
                    }
                    Change::CircularEngineeringCase { id, after, .. } => {
                        let domain = state.engineering.get_or_insert_with(EngineeringDomain::default);
                        set_optional(&mut domain.circular_cases, *id, after);
                    }
                    Change::Draft(change) => {
                        if let Some(domain) = state.drafting.as_mut() {
                            domain.apply_change(change, true);
                        }
                    }
                    Change::DraftDrawingMetadata { id, after, .. } => {
                        if let Some(drawing) =
                            state.drafting.as_mut().and_then(|d| d.drawings.get_mut(id))
                        {
                            after.install(drawing);
                        }
                    }
                    Change::DraftView { id, after, .. } => {
                        if let Some(domain) = state.drafting.as_mut() {
                            set_optional(&mut domain.views, *id, after);
                        }
                    }
                    Change::Class { id, after, .. } => {
                        if let Some(m) = state.classes.as_mut() {
                            set_optional(m, id.clone(), after);
                        }
                    }
                    Change::Operation { id, after, .. } => {
                        if let Some(m) = state.operations.as_mut() {
                            set_optional(m, *id, after);
                        }
                    }
                    Change::Relationship { id, after, .. } => {
                        if let Some(m) = state.relationship_records.as_mut() {
                            set_optional(m, *id, after);
                        }
                    }
                    _ => {}
                }
            }
            state.delta = entry.delta.clone();
            if state
                .engineering
                .as_ref()
                .is_some_and(EngineeringDomain::is_empty)
            {
                state.engineering = None;
            }
        }
        state.checkpoint = true;
        Ok(state)
    }
    fn reconstruct_history_stacks(&self, revision: u64) -> Result<HistoryStackAnchor, CoreError> {
        self.reserve_circular_archive_audit()?;
        let _scan = self.scan_counters.track(
            "history_stack_reconstruction",
            0,
            0,
            "explicit replay/open/save derivation of undo/redo order from operation journals",
            true,
        );
        let mut stacks = HistoryStackAnchor::default();
        let mut anchor_seen = false;
        let mut delta_seen = false;
        for (_, entry) in self.revision_states.range(..=revision) {
            if let Some(anchor) = &entry.history_stack_anchor {
                if anchor_seen || delta_seen || entry.delta.is_some() || !entry.checkpoint {
                    return Err(CoreError::InvalidDocument(
                        "history stack anchor must precede the v0.4 journal timeline".into(),
                    ));
                }
                stacks = anchor.clone();
                anchor_seen = true;
            }
            let Some(delta) = &entry.delta else {
                continue;
            };
            delta_seen = true;
            let operations: Vec<_> = delta
                .changes
                .iter()
                .filter_map(|change| {
                    if let Change::Operation { before, after, .. } = change {
                        Some((before, after))
                    } else {
                        None
                    }
                })
                .collect();
            let Some((before, after)) = operations.first() else {
                continue;
            };
            let operation = after.as_ref().or(before.as_ref()).ok_or_else(|| {
                CoreError::InvalidDocument("historical operation change has no record".into())
            })?;
            let command = &operation.command_id;
            if operations.iter().all(|(_, after)| {
                after
                    .as_ref()
                    .is_some_and(|o| o.revision == delta.summary.revision_after)
            }) {
                stacks.undo.push(HistoryEntry {
                    command_id: command.clone(),
                    operation_ids: operations
                        .iter()
                        .filter_map(|(_, after)| after.as_ref().map(|o| o.id))
                        .collect(),
                    changes: delta.changes.clone(),
                });
                stacks.redo.clear();
            } else if after.is_none() {
                let entry = stacks.undo.pop().ok_or_else(|| {
                    CoreError::InvalidDocument("historical undo has no matching commit".into())
                })?;
                if &entry.command_id != command {
                    return Err(CoreError::InvalidDocument(
                        "historical undo command disagrees with stack".into(),
                    ));
                }
                if delta.changes.len() != entry.changes.len()
                    || !delta
                        .changes
                        .iter()
                        .zip(entry.changes.iter().rev())
                        .all(|(actual, original)| actual == &transactions::inverse_change(original))
                {
                    return Err(CoreError::InvalidDocument(
                        "historical undo journal is not the complete inverse of its commit".into(),
                    ));
                }
                stacks.redo.push(entry);
            } else {
                let entry = stacks.redo.pop().ok_or_else(|| {
                    CoreError::InvalidDocument("historical redo has no matching undo".into())
                })?;
                if &entry.command_id != command {
                    return Err(CoreError::InvalidDocument(
                        "historical redo command disagrees with stack".into(),
                    ));
                }
                if delta.changes != entry.changes {
                    return Err(CoreError::InvalidDocument(
                        "historical redo journal disagrees with its complete originating commit"
                            .into(),
                    ));
                }
                stacks.undo.push(entry);
            }
        }
        Ok(stacks)
    }
    /// Reconstruct a standalone semantic document at an available revision.
    /// Allocation high-water marks and all idempotent receipts remain retained.
    pub fn replay_revision(&self, revision: u64) -> Result<Document, CoreError> {
        self.reserve_circular_archive_audit()?;
        self.verify_all_receipt_payloads()?;
        let state = self.reconstruct_revision_state(revision)?;
        if revision != self.revision && (state.classes.is_none() || state.operations.is_none()) {
            // Legacy formats did not retain these maps at historical
            // revisions. Never manufacture a past state with future records.
            return Err(CoreError::UnknownRevision(revision));
        }
        let mut d = Document::new(self.name.clone())?;
        d.document_uuid = self.document_uuid;
        d.revision = revision;
        d.classes = state
            .classes
            .clone()
            .unwrap_or_else(|| self.classes.clone());
        d.elements = state
            .instances
            .clone()
            .unwrap_or_else(|| state.elements.clone());
        d.types = state.types.clone();
        d.levels = state.levels.clone();
        d.drafting = state.drafting.clone().unwrap_or_default();
        d.engineering = state.engineering.clone().unwrap_or_default();
        d.foundation_structural = state.foundation_structural.clone().unwrap_or_default();
        d.structural_configurator = state.structural_configurator.clone().unwrap_or_default();
        d.geo_foundation = state.geo_foundation.clone().unwrap_or_default();
        d.relationships = state
            .relationship_records
            .clone()
            .unwrap_or_else(|| state.relationships.clone());
        d.operations = state
            .operations
            .clone()
            .unwrap_or_else(|| self.operations.clone());
        d.command_receipts = self.command_receipts.clone();
        d.restore_counters(&self.allocation_counters());
        d.relationship_indexes = OnceLock::new();
        d.indexes();
        d.history_policy = self.history_policy;
        d.revision_states = self
            .revision_states
            .range(..=revision)
            .map(|(r, s)| (*r, s.clone()))
            .collect();
        let stacks = self.reconstruct_history_stacks(revision)?;
        d.undo_stack = stacks.undo;
        d.redo_stack = stacks.redo;
        d.refresh_format_version();
        d.circular_archive_usage = OnceLock::new();
        d.verify_invariants()?;
        Ok(d)
    }
    pub fn history_diff(&self, from: u64, to: u64) -> Result<RevisionDiff, CoreError> {
        if !self.revision_states.contains_key(&from) {
            return Err(CoreError::UnknownRevision(from));
        }
        if !self.revision_states.contains_key(&to) {
            return Err(CoreError::UnknownRevision(to));
        }
        if from == to {
            return Ok(RevisionDiff {
                revision_from: from,
                revision_to: to,
                ..RevisionDiff::default()
            });
        }
        let low = from.min(to);
        let high = from.max(to);
        let mut elements = BTreeSet::new();
        let mut types = BTreeSet::new();
        let mut levels = BTreeSet::new();
        let mut relations = BTreeSet::new();
        let legacy = self
            .revision_states
            .range((low + 1)..=high)
            .any(|(_, s)| s.delta.is_none());
        if legacy {
            let before = self.reconstruct_revision_state(from)?;
            let after = self.reconstruct_revision_state(to)?;
            elements.extend(before.elements.keys().chain(after.elements.keys()).copied());
            types.extend(before.types.keys().chain(after.types.keys()).copied());
            levels.extend(before.levels.keys().chain(after.levels.keys()).copied());
            relations.extend(
                before
                    .relationships
                    .keys()
                    .chain(after.relationships.keys())
                    .copied(),
            );
        } else {
            for (_, s) in self.revision_states.range((low + 1)..=high) {
                let d = &s.delta.as_ref().expect("checked delta").summary;
                elements.extend(&d.affected_elements);
                types.extend(
                    d.created_types
                        .iter()
                        .chain(&d.updated_types)
                        .chain(&d.deleted_types),
                );
                levels.extend(
                    d.created_levels
                        .iter()
                        .chain(&d.updated_levels)
                        .chain(&d.deleted_levels),
                );
                relations.extend(
                    d.created_relationships
                        .iter()
                        .chain(&d.updated_relationships)
                        .chain(&d.deleted_relationships),
                );
            }
        }
        let _scan = self.scan_counters.track(
            "history_diff_candidates",
            elements.len(),
            relations.len(),
            if legacy {
                "legacy snapshots require an explicit full-history comparison"
            } else {
                "compare only entity IDs touched by intervening deltas"
            },
            legacy,
        );
        let mut result = RevisionDiff {
            revision_from: from,
            revision_to: to,
            ..RevisionDiff::default()
        };
        for id in elements {
            let a = self.historical_element(from, id);
            let b = self.historical_element(to, id);
            match (&a, &b) {
                (None, Some(_)) => result.created.push(id),
                (Some(_), None) => result.deleted.push(id),
                (Some(_), Some(_)) if a != b => result.modified.push(id),
                _ => {}
            }
        }
        for id in types {
            let a = self.historical_type(from, id);
            let b = self.historical_type(to, id);
            match (&a, &b) {
                (None, Some(_)) => result.types_created.push(id),
                (Some(_), None) => result.types_deleted.push(id),
                (Some(_), Some(_)) if a != b => result.types_modified.push(id),
                _ => {}
            }
        }
        for id in levels {
            let a = self.historical_level(from, id);
            let b = self.historical_level(to, id);
            match (&a, &b) {
                (None, Some(_)) => result.levels_created.push(id),
                (Some(_), None) => result.levels_deleted.push(id),
                (Some(_), Some(_)) if a != b => result.levels_modified.push(id),
                _ => {}
            }
        }
        for id in relations {
            match (
                self.historical_relationship(from, id),
                self.historical_relationship(to, id),
            ) {
                (None, Some(_)) => result.relationships_created.push(id),
                (Some(_), None) => result.relationships_deleted.push(id),
                _ => {}
            }
        }
        result.drafting = self.drafting_diff(from, to)?;
        result.engineering = self.engineering_diff(from, to)?;
        result.foundation_structural = self.foundation_structural_diff(from, to)?;
        result.structural_configurator = self.configurator_diff(from, to)?;
        result.geo_foundation = self.geo_foundation_diff(from, to)?;
        Ok(result)
    }
    fn historical_element(
        &self,
        revision: u64,
        id: ElementId,
    ) -> Option<(
        ElementRecord,
        Option<ElementRecord>,
        Option<BTreeMap<PropertyId, PropertySource>>,
    )> {
        for (_, s) in self.revision_states.range(..=revision).rev() {
            if let Some(e) = s.elements.get(&id) {
                return Some((
                    e.clone(),
                    s.instances.as_ref().and_then(|m| m.get(&id)).cloned(),
                    s.property_sources
                        .as_ref()
                        .and_then(|m| m.get(&id))
                        .cloned(),
                ));
            }
            if s.checkpoint
                || s.delta
                    .as_ref()
                    .is_some_and(|d| d.summary.deleted_elements.contains(&id))
            {
                return None;
            }
        }
        None
    }
    fn historical_type(&self, revision: u64, id: TypeId) -> Option<ElementType> {
        for (_, s) in self.revision_states.range(..=revision).rev() {
            if let Some(v) = s.types.get(&id) {
                return Some(v.clone());
            }
            if s.checkpoint
                || s.delta
                    .as_ref()
                    .is_some_and(|d| d.summary.deleted_types.contains(&id))
            {
                return None;
            }
        }
        None
    }
    fn historical_level(&self, revision: u64, id: LevelId) -> Option<Level> {
        for (_, s) in self.revision_states.range(..=revision).rev() {
            if let Some(v) = s.levels.get(&id) {
                return Some(v.clone());
            }
            if s.checkpoint
                || s.delta
                    .as_ref()
                    .is_some_and(|d| d.summary.deleted_levels.contains(&id))
            {
                return None;
            }
        }
        None
    }
    fn historical_relationship(&self, revision: u64, id: RelationshipId) -> Option<Relationship> {
        for (_, s) in self.revision_states.range(..=revision).rev() {
            if let Some(v) = s.relationships.get(&id) {
                return Some(v.clone());
            }
            if s.checkpoint
                || s.delta
                    .as_ref()
                    .is_some_and(|d| d.summary.deleted_relationships.contains(&id))
            {
                return None;
            }
        }
        None
    }
}
