//! Retained original requests and independent operation/effect checks. Old
//! formats cannot recover omitted request arguments or abandoned journals.
use crate::*;

/// Explicit limitation for an imported receipt without a typed original
/// request. The raw retry string is preserved; available journals are checked,
/// but unavailable arguments are never reconstructed from that same string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptPayloadDiagnostic {
    pub command_id: CommandId,
    pub operation_count: usize,
    pub verified_operation_count: usize,
    pub reason: String,
}

impl Document {
    /// Report imported receipts whose exact original request is unavailable.
    /// This explicit audit visits retained receipts/history, never mutations.
    pub fn receipt_payload_diagnostics(&self) -> Result<Vec<ReceiptPayloadDiagnostic>, CoreError> {
        self.verify_all_receipt_payloads()
    }

    pub(super) fn verify_all_receipt_payloads(
        &self,
    ) -> Result<Vec<ReceiptPayloadDiagnostic>, CoreError> {
        let _scan = self.scan_counters.track("receipt_payload_integrity_audit", 0, 0,
            "explicit open/save/replay audit of retained request payloads against original commands and available operation journals", true);
        let mut entries = BTreeMap::new();
        for entry in self.undo_stack.iter().chain(&self.redo_stack) {
            entries.insert(&entry.command_id, entry.changes.as_slice());
        }
        let mut diagnostics = Vec::new();
        for (id, stored) in &self.command_receipts {
            if let Some(diagnostic) =
                self.verify_receipt_payload(id, stored, entries.get(id).copied())?
            {
                diagnostics.push(diagnostic);
            }
        }
        Ok(diagnostics)
    }

    /// An idempotent retry checks only this receipt and its indexed originating
    /// revision/operations. It never runs a document/history collection pass.
    pub(super) fn verify_receipt_payload(
        &self,
        id: &CommandId,
        stored: &StoredCommand,
        stack_changes: Option<&[Change]>,
    ) -> Result<Option<ReceiptPayloadDiagnostic>, CoreError> {
        let fail = |reason: &str| {
            CoreError::InvalidDocument(format!("command receipt {id} payload {reason}"))
        };
        if id.is_empty()
            || stored.receipt.command_id != *id
            || (!stored.receipt.transaction_id.is_empty() && stored.receipt.transaction_id != *id)
            || (stored.receipt.revision_after != 0
                && (stored.receipt.revision_after != stored.receipt.revision
                    || stored.receipt.revision_before.checked_add(1)
                        != Some(stored.receipt.revision_after)))
        {
            return Err(fail(
                "receipt identity or originating revision is inconsistent",
            ));
        }
        let legacy_commands;
        let commands: &[Command] = if let Some(recorded) = &stored.recorded_commands {
            if !circular_archive_limits::serialized_payload_matches(recorded, &stored.payload_json)? {
                return Err(fail("disagrees with retained original commands"));
            }
            recorded
        } else {
            let probe = circular_archive_preflight::preflight_circular_command_payload(&stored.payload_json)?;
            if probe.snapshot_occurrences != 0 {
                return Err(fail("circular archive save has no typed original request"));
            }
            legacy_commands = serde_json::from_str::<Vec<Command>>(&stored.payload_json)
                .map_err(|_| fail("is not a valid command array"))?;
            if legacy_commands.iter().any(|c| matches!(c, Command::FoundationStructural(_) | Command::StructuralConfigurator(_) | Command::GeoFoundation(_))) { return Err(fail("structural/foundation/GEO request has no typed original command")); }
            &legacy_commands
        };
        if commands.is_empty() || commands.len() != stored.receipt.operation_ids.len() {
            return Err(fail("command count disagrees with recorded operation IDs"));
        }
        if stored.receipt.created_drawings != stored.receipt.delta.drafting.created_drawings
            || stored.receipt.created_draft_entities
                != stored.receipt.delta.drafting.created_entities
        {
            return Err(fail("drafting creation IDs disagree with retained delta"));
        }
        if stored.receipt.affected_objects
            != qualified_affected(
                &stored.receipt.affected_ids,
                &stored.receipt.delta.drafting,
                &stored.receipt.delta.engineering,
                    &stored.receipt.delta.foundation_structural,
                    &stored.receipt.delta.structural_configurator,
                    &stored.receipt.delta.geo_foundation,
            )
        {
            return Err(fail(
                "qualified affected objects disagree with retained effects",
            ));
        }
        let mut ids = BTreeSet::new();
        let originating_changes = self
            .revision_states
            .get(&stored.receipt.revision)
            .and_then(|state| state.delta.as_ref())
            .map(|delta| delta.changes.as_slice())
            .or(stack_changes);
        // One pass identifies the boundaries of every original command; a
        // large initial transaction must not search its entire journal per ID.
        let mut originating = BTreeMap::new();
        if let Some(changes) = originating_changes {
            let mut start = 0;
            for (end, change) in changes.iter().enumerate() {
                if let Change::Operation {
                    id: operation_id,
                    before: None,
                    after: Some(operation),
                } = change
                {
                    originating.insert(*operation_id, (operation, &changes[start..end]));
                    start = end + 1;
                }
            }
        }
        let mut verified = 0;
        let mut unavailable_circular_origin = false;
        for (index, (command, operation_id)) in commands
            .iter()
            .zip(&stored.receipt.operation_ids)
            .enumerate()
        {
            if operation_id.get() == 0 || !ids.insert(*operation_id) {
                return Err(fail("has zero or repeated operation IDs"));
            }
            let source = originating.get(operation_id).copied();
            if matches!(command, Command::Engineering(EngineeringCommand::SaveCircularSnapshot { .. }) | Command::FoundationStructural(_) | Command::StructuralConfigurator(_) | Command::GeoFoundation(_))
                && source.is_none() {
                if stored.receipt.revision <= self.revision {
                    return Err(fail("circular archive save has no originating operation/effect journal"));
                }
                // replay_revision deliberately preserves future receipts. They
                // remain historical bytes, not proof that a save occurred in
                // this selected revision and not an idempotent retry result.
                unavailable_circular_origin = true;
            }
            let operation = source
                .map(|(operation, _)| operation)
                .or_else(|| self.operations.get(operation_id));
            if let Some(operation) = operation {
                if operation.id != *operation_id
                    || &operation.command_id != id
                    || operation.command_index as usize != index
                    || operation.revision != stored.receipt.revision
                    || !command_matches_operation(
                        command,
                        operation,
                        source.map(|(_, changes)| changes),
                    )
                {
                    return Err(fail("disagrees with its recorded operation or effects"));
                }
                verified += 1;
            }
        }
        Ok((stored.recorded_commands.is_none() || unavailable_circular_origin).then(|| ReceiptPayloadDiagnostic {
            command_id: id.clone(), operation_count: commands.len(), verified_operation_count: verified,
            reason: if unavailable_circular_origin {
                "future circular archive receipt has no originating journal in this selected revision; it cannot authorize an idempotent retry".into()
            } else { "legacy receipt has no independently retained original request; only command shape, count and available operation/effect evidence can be verified".into() },
        }))
    }
}

fn command_matches_operation(
    command: &Command,
    operation: &OperationRecord,
    changes: Option<&[Change]>,
) -> bool {
    let action = match &operation.kind {
        OperationKind::BimChanged { action } => Some(action.as_str()),
        _ => None,
    };
    let effects_match = |predicate: &dyn Fn(&[Change]) -> bool| changes.is_none_or(predicate);
    match command {
        Command::GeoFoundation(command) => matches!(&operation.kind, OperationKind::GeoFoundationChanged { command: recorded } if recorded == command)
            && changes.is_some_and(|changes| geo_foundation::geo_foundation_effects_match(command, changes, operation)),
        Command::StructuralConfigurator(command) => matches!(&operation.kind, OperationKind::StructuralConfiguratorChanged { command: recorded } if recorded == command) && changes.is_some_and(|changes| configured_building::configurator_effects_match(command, changes, operation)),
        Command::FoundationStructural(command) => matches!(&operation.kind, OperationKind::FoundationStructuralChanged { command: recorded } if recorded == command)
            && changes.is_some_and(|changes| foundation_structural::foundation_structural_effects_match(command, changes, operation)),
        Command::Engineering(command) => match &operation.kind {
            OperationKind::EngineeringGroundChanged { ground_section_id, command: recorded }
                if recorded == command => {
                let id_matches = match command {
                    EngineeringCommand::CreateGround { .. } =>
                        ground_section_id.get() == operation.id.get(),
                    EngineeringCommand::UpdateGround { ground_section_id: id, .. } =>
                        id == ground_section_id,
                    _ => false,
                };
                id_matches && changes.is_none_or(|changes| {
                    engineering::engineering_command_effects_match(command, changes, operation.id)
                        && changes.iter().any(|change| matches!(change,
                            Change::EngineeringGroundSection { id, .. } if id == ground_section_id))
                })
            }
            OperationKind::EngineeringChanged {
                analysis_id,
                command: recorded,
            } if recorded == command => {
                let id_matches = match command {
                    EngineeringCommand::CreateSlope { .. }
                    | EngineeringCommand::CreateCircular { .. } => {
                        analysis_id.get() == operation.id.get()
                    }
                    EngineeringCommand::UpdateSlope {
                        analysis_id: id, ..
                    }
                    | EngineeringCommand::SaveCalculationSnapshot { analysis_id: id }
                    | EngineeringCommand::UpdateCircular { analysis_id: id, .. } => {
                        id == analysis_id
                    }
                    EngineeringCommand::SaveCircularSnapshot { analysis_id: id, .. } => id == analysis_id,
                    _ => false,
                };
                id_matches
                    && changes.is_none_or(|changes| {
                        engineering::engineering_command_effects_match(command, changes, operation.id)
                            && changes.iter().any(
                                |c| matches!(c,Change::EngineeringCase{id,..}
                                    | Change::CircularEngineeringCase{id,..} if id==analysis_id),
                            )
                    })
            }
            _ => false,
        },
        Command::Draft(command) => {
            matches!(&operation.kind, OperationKind::DraftingChanged { command: recorded } if recorded==command)
                && changes.is_none_or(|changes| drafting_command_effects_match(command, changes))
        }
        Command::DefineClass(definition) => {
            matches!(&operation.kind, OperationKind::ClassDefined { class_id, name, parent_id }
            if class_id == &definition.id && name == &definition.name && parent_id == &definition.parent_id)
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Class { before: None, after: Some(value), .. } if value == definition))
                })
        }
        Command::DefineProperty {
            class_id,
            definition,
        } => {
            matches!(&operation.kind, OperationKind::PropertyDefined { class_id: recorded_class, property_id, name }
            if recorded_class == class_id && property_id == &definition.id && name == &definition.name)
                && effects_match(&|changes| {
                    changes.iter().any(|change| {
                        matches!(change, Change::Class { id, after: Some(value), .. }
                if id == class_id && value.properties.get(&definition.id) == Some(definition))
                    })
                })
        }
        Command::CreateElement {
            name,
            class_id,
            kind,
            properties,
        } => {
            matches!(&operation.kind, OperationKind::ElementCreated { class_id: recorded_class, element_type, name: recorded_name, .. }
            if recorded_class == class_id && *element_type == kind.element_type() && recorded_name == name)
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Element { before: None, after: Some(value), .. }
                if value.name == *name && value.class_id == *class_id && value.kind == *kind && &value.properties == properties))
                })
        }
        Command::SetProperty {
            element_id,
            property_id,
            value,
        } => {
            matches!(&operation.kind, OperationKind::PropertySet { element_id: recorded_element, property_id: recorded_property, .. }
            if recorded_element == element_id && recorded_property == property_id)
                && effects_match(&|changes| {
                    changes.iter().any(|change| {
                        matches!(change, Change::Element { id, after: Some(element), .. }
                if id == element_id && element.properties.get(property_id) == Some(value))
                    })
                })
        }
        Command::DeleteElement { element_id } => {
            action == Some(format!("Delete {element_id} using Reject").as_str())
                || matches!(&operation.kind, OperationKind::ElementDeleted { element_id: recorded, .. } if recorded == element_id)
        }
        Command::DeleteElementWithStrategy {
            element_id,
            strategy,
        } => action == Some(format!("Delete {element_id} using {strategy:?}").as_str()),
        Command::UpdateType {
            type_id,
            name,
            properties,
        } => {
            action == Some(format!("Update type {type_id}").as_str())
                && effects_match(&|changes| {
                    changes.iter().any(|change| {
                        if let Change::Type {
                            id,
                            before: Some(before),
                            after: Some(after),
                        } = change
                        {
                            let mut expected = before.clone();
                            if let Some(name) = name {
                                expected.name = name.clone();
                            }
                            expected.properties.extend(properties.clone());
                            id == type_id && &expected == after
                        } else {
                            false
                        }
                    })
                })
        }
        Command::AssignType {
            element_id,
            type_id,
        } => action == Some(format!("Assign type {type_id} to {element_id}").as_str()),
        Command::AssignLevel {
            element_id,
            level_id,
        } => action == Some(format!("Assign level {level_id} to {element_id}").as_str()),
        Command::RemoveRelationship { relationship_id } => {
            action == Some(format!("Remove relationship {relationship_id}").as_str())
        }
        Command::HostElement {
            host_id,
            element_id,
        } => action == Some(format!("Host {element_id} in {host_id}").as_str()),
        Command::ClearPropertyOverride {
            element_id,
            property_id,
        } => action == Some(format!("Clear override {element_id}.{property_id}").as_str()),
        Command::UpdateElement { element_id, kind } => {
            action == Some(format!("Update element {element_id}").as_str())
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Element { id, after: Some(value), .. } if id == element_id && &value.kind == kind))
                })
        }
        Command::CreateType {
            name,
            class_id,
            properties,
        } => {
            action.is_some_and(|action| action.starts_with("Create type "))
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Type { before: None, after: Some(value), .. }
                if &value.name == name && &value.class_id == class_id && &value.properties == properties))
                })
        }
        Command::CreateLevel {
            name,
            elevation_m,
            height_m,
        } => {
            action.is_some_and(|action| action.starts_with("Create level "))
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Level { before: None, after: Some(value), .. }
                if &value.name == name && &value.elevation_m == elevation_m && &value.height_m == height_m))
                })
        }
        Command::CreateRelationship {
            relationship_type,
            source_id,
            target_id,
            metadata,
        } => {
            action.is_some_and(|action| action.starts_with("Create relationship "))
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Relationship { before: None, after: Some(value), .. }
                if &value.relationship_type == relationship_type && &value.source_id == source_id && &value.target_id == target_id && &value.metadata == metadata))
                })
        }
        Command::CreateOpening {
            name,
            spec,
            properties,
            host_id,
            type_id,
        } => {
            action.is_some_and(|action| action.starts_with("Create Opening "))
                && effects_match(&|changes| {
                    opening_effect_matches(
                        changes,
                        name,
                        &ElementKind::Opening(spec.clone()),
                        properties,
                        *host_id,
                        *type_id,
                    )
                })
        }
        Command::CreatePolygonalOpening {
            name,
            spec,
            properties,
            host_id,
            type_id,
        } => {
            action.is_some_and(|action| action.starts_with("Create Opening "))
                && effects_match(&|changes| {
                    opening_effect_matches(
                        changes,
                        name,
                        &ElementKind::PolygonalOpening(spec.clone()),
                        properties,
                        *host_id,
                        *type_id,
                    )
                })
        }
        Command::CreateSpace {
            name,
            level_id,
            boundary,
            metadata,
        } => {
            action.is_some_and(|action| action.starts_with("Create Space "))
                && effects_match(&|changes| {
                    changes.iter().any(|change| matches!(change, Change::Element { before: None, after: Some(value), .. }
                if &value.name == name && value.kind == ElementKind::Space(SpaceSpec { boundary: boundary.clone(), metadata: metadata.clone() })
                && association_matches(changes, value.id, RelationshipType::BelongsToLevel, level_id.map(EntityRef::Level))))
                })
        }
    }
}

fn opening_effect_matches(
    changes: &[Change],
    name: &str,
    kind: &ElementKind,
    properties: &BTreeMap<PropertyId, PropertyValue>,
    host: ElementId,
    type_id: Option<TypeId>,
) -> bool {
    changes.iter().any(|change| {
        if let Change::Element {
            before: None,
            after: Some(element),
            ..
        } = change
        {
            element.name == name
                && &element.kind == kind
                && &element.properties == properties
                && association_matches(
                    changes,
                    element.id,
                    RelationshipType::InstanceOf,
                    type_id.map(EntityRef::Type),
                )
                && changes.iter().rev().find_map(|change| {
                    if let Change::Element {
                        id,
                        after: Some(value),
                        ..
                    } = change
                    {
                        (*id == element.id).then_some(value.type_id)
                    } else {
                        None
                    }
                }) == Some(type_id)
                && changes.iter().any(|change| {
                    matches!(change, Change::Relationship { after: Some(r), .. }
                    if hosting_pair_for_receipt(r) == Some((host, element.id)))
                })
        } else {
            false
        }
    })
}

fn hosting_pair_for_receipt(r: &Relationship) -> Option<(ElementId, ElementId)> {
    indexes::hosting_pair(r)
}

fn association_matches(
    changes: &[Change],
    element: ElementId,
    kind: RelationshipType,
    target: Option<EntityRef>,
) -> bool {
    let recorded: Vec<_> = changes
        .iter()
        .filter_map(|change| {
            if let Change::Relationship {
                before: None,
                after: Some(r),
                ..
            } = change
            {
                (r.relationship_type == kind && r.source_id == EntityRef::Element(element))
                    .then_some(r.target_id)
            } else {
                None
            }
        })
        .collect();
    match target {
        Some(target) => recorded == [target],
        None => recorded.is_empty(),
    }
}
