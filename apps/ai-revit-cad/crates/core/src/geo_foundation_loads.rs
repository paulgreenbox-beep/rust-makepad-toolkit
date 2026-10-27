//! Explicit manual or saved-configuration load ownership for GEO foundations.
//! This adapter aggregates recorded reactions; it never invokes a solver.
use crate::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum GeoFoundationLoadBinding {
    Manual { source_reference: String },
    Configured {
        snapshot_id: EngineeringSnapshotId,
        content_fingerprint: String,
    },
}

impl Document {
    /// Use the complete archived demand of this exact foundation group. Mesh,
    /// explicit synthetic soil/stiffness, groundwater and numerical controls
    /// remain supplied by the caller; placement owns foundation geometry.
    pub fn prepare_geo_foundation_analysis_from_configured(
        &self,
        foundation_id: &str,
        snapshot_id: EngineeringSnapshotId,
        mut explicit_input: foundation_core::RigidMatInput,
    ) -> Result<GeoFoundationAnalysisInput, CoreError> {
        let snapshot = self.structural_configurator().snapshot(snapshot_id)
            .ok_or_else(|| load_error("configured load source is not saved in this document revision"))?;
        let binding = GeoFoundationLoadBinding::Configured {
            snapshot_id,
            content_fingerprint: snapshot.content_fingerprint.clone(),
        };
        if !self.geo_foundation_load_is_current(&binding)? {
            return Err(load_error("new GEO foundation analysis requires a current saved configurator result"));
        }
        let placement = self.geo_foundation().layout()
            .ok_or_else(|| load_error("no foundation layout"))?
            .input.layout.elements.iter()
            .find(|placement| placement_id(placement) == foundation_id)
            .ok_or_else(|| load_error("foundation placement is absent from the current layout"))?;
        // Translate the global ledger about the actual selected raft base,
        // rather than copying a resultant about another group's reference.
        explicit_input.load = configured_group_load(snapshot, foundation_id, placement)?;
        let mut input = self.prepare_geo_foundation_analysis(foundation_id, explicit_input)?;
        input.load_binding = binding;
        self.validate_geo_foundation_load_binding(&input)?;
        Ok(input)
    }

    /// Currentness concerns the live selected revision; immutable validation
    /// below also accepts a source retained after its save was undone.
    pub fn geo_foundation_load_is_current(
        &self,
        binding: &GeoFoundationLoadBinding,
    ) -> Result<bool, CoreError> {
        match binding {
            GeoFoundationLoadBinding::Manual { source_reference } => {
                validate_manual_reference(source_reference)?;
                Ok(true)
            }
            GeoFoundationLoadBinding::Configured { snapshot_id, content_fingerprint } => {
                self.retained_geo_configured_load_source(*snapshot_id, content_fingerprint)?;
                Ok(self.structural_configurator().snapshot_status(*snapshot_id)
                    == Some(EngineeringSnapshotStatus::Current))
            }
        }
    }

    /// A result is current only while both its selected GEO/placement sources
    /// and its explicit load source remain current. No archive is modified.
    pub fn geo_foundation_snapshot_status(
        &self,
        id: EngineeringSnapshotId,
    ) -> Option<EngineeringSnapshotStatus> {
        let snapshot = self.geo_foundation().snapshot(id)?;
        Some(if self.geo_foundation().ground_snapshot_status(id)
                == Some(EngineeringSnapshotStatus::Current)
            && self.geo_foundation_load_is_current(&snapshot.input.load_binding).unwrap_or(false)
        {
            EngineeringSnapshotStatus::Current
        } else {
            EngineeringSnapshotStatus::Stale
        })
    }

    /// Validate retained load provenance without requiring a historical source
    /// to remain the current building. This is used by Open/history ownership
    /// audits as well as new saves; it does not call structural/foundation solve.
    pub(super) fn validate_geo_foundation_load_binding(
        &self,
        input: &GeoFoundationAnalysisInput,
    ) -> Result<(), CoreError> {
        match &input.load_binding {
            GeoFoundationLoadBinding::Manual { source_reference } => {
                validate_manual_reference(source_reference)
            }
            GeoFoundationLoadBinding::Configured { snapshot_id, content_fingerprint } => {
                let source = self.retained_geo_configured_load_source(*snapshot_id, content_fingerprint)?;
                let expected = configured_group_load(source, &input.foundation_id, &input.placement)?;
                if input.effective_input.load != expected {
                    return Err(load_error("GEO foundation load differs from its complete archived configurator reaction group"));
                }
                Ok(())
            }
        }
    }

    fn retained_geo_configured_load_source(
        &self,
        id: EngineeringSnapshotId,
        expected_fingerprint: &str,
    ) -> Result<&ConfiguratorAnalysisSnapshot, CoreError> {
        if id.get() == 0 || expected_fingerprint.len() != 64
            || !expected_fingerprint.bytes().all(|value| value.is_ascii_hexdigit())
        {
            return Err(load_error("invalid configured archive identity or fingerprint"));
        }
        let operation = self.retained_operation(OperationId(id.get()))
            .ok_or_else(|| load_error("configured load source save operation is not retained in this document revision"))?;
        let OperationKind::StructuralConfiguratorChanged {
            command: StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot },
        } = &operation.kind else {
            return Err(load_error("configured load source identity references another operation kind"));
        };
        snapshot.validate()?;
        if operation.id.get() != id.get()
            || snapshot.source.source_revision.document_uuid != self.document_uuid
            || snapshot.source.source_revision.operation_id.get() >= id.get()
            || snapshot.content_fingerprint != expected_fingerprint
        {
            return Err(load_error("configured load archive belongs to another document/source or differs from its pinned content"));
        }
        let source_operation = self.retained_operation(snapshot.source.source_revision.operation_id)
            .ok_or_else(|| load_error("configured load archive's authored building source operation is not retained"))?;
        let OperationKind::StructuralConfiguratorChanged {
            command: StructuralConfiguratorCommand::UpdateBuildingAndRegenerate { building, foundation_groups },
        } = &source_operation.kind else {
            return Err(load_error("configured load archive's source references another operation kind"));
        };
        if source_operation.revision > operation.revision
            || building != &snapshot.source.building
            || foundation_groups.len() != snapshot.source.foundation_groups.len()
            || foundation_groups.iter().map(|group| &group.foundation_id).collect::<BTreeSet<_>>().len()
                != foundation_groups.len()
            || foundation_groups.iter().any(|group|
                snapshot.source.foundation_groups.get(&group.foundation_id) != Some(group))
        {
            return Err(load_error("configured load archive disagrees with its exact authored building/foundation group source"));
        }
        if self.structural_configurator().snapshot(id).is_some_and(|current| current != snapshot) {
            return Err(load_error("live configured archive differs from its retained save operation"));
        }
        Ok(snapshot)
    }
}

fn configured_group_load(
    snapshot: &ConfiguratorAnalysisSnapshot,
    foundation_id: &str,
    placement: &foundation_core::FoundationPlacement,
) -> Result<foundation_core::PressureLoad, CoreError> {
    let id = foundation_core::FoundationId::new(foundation_id).map_err(load_error)?;
    if placement_id(placement) != foundation_id {
        return Err(load_error("configured load foundation identity differs from its GEO placement"));
    }
    let group = snapshot.source.foundation_groups.get(&id)
        .ok_or_else(|| load_error("configured source has no exact selected foundation assignment group"))?;
    let calculation = snapshot.foundations.iter().find(|calculation| calculation.foundation_id == id)
        .ok_or_else(|| load_error("configured archive has no outcome for the selected foundation group"))?;
    if !matches!(calculation.status, ConfiguredFoundationStatus::Computed | ConfiguredFoundationStatus::CalculationFailed)
        || calculation.effective_input.is_none() || calculation.support_node_ids.is_empty()
    {
        return Err(load_error("selected configured foundation demand is incomplete, shared/unallocated, or has no assigned supports"));
    }
    let reactions: Vec<_> = snapshot.reactions.reactions.iter()
        .filter(|reaction| !reaction.section_owners.is_disjoint(&group.member_section_ids))
        .collect();
    if reactions.is_empty()
        || reactions.iter().any(|reaction| reaction.foundation_group_id.as_ref() != Some(&id))
        || reactions.iter().map(|reaction| reaction.support_node_id.clone()).collect::<BTreeSet<_>>()
            != calculation.support_node_ids
    {
        return Err(load_error("configured reaction ledger is incomplete or not exclusively routed to the selected common foundation group"));
    }
    let foundation_core::FoundationPlacement::Raft { center_m, .. } = placement else {
        return Err(load_error("configured numerical load transfer currently supports an explicit rectangular raft only"));
    };
    translated_group_reactions(&reactions, *center_m)
}

fn translated_group_reactions(
    reactions: &[&ConfiguredSupportReaction],
    base_center_m: [f64; 3],
) -> Result<foundation_core::PressureLoad, CoreError> {
    let actions = reactions.iter().map(|reaction| {
        let force_moment = reaction.force_moment;
        foundation_core::LocatedStructuralReaction {
            source_id: format!("configured-support:{}", reaction.support_node_id),
            location_m: std::array::from_fn(|axis| reaction.position_m[axis] - base_center_m[axis]),
            external_force_kn: [-force_moment[0], -force_moment[1], -force_moment[2]],
            external_free_moment_knm: [-force_moment[3], -force_moment[4], -force_moment[5]],
        }
    }).collect();
    foundation_core::load_from_structural_reactions(actions).map_err(load_error)
}

fn validate_manual_reference(reference: &str) -> Result<(), CoreError> {
    if reference.trim().is_empty() || reference.len() > 4096 {
        return Err(load_error("manual foundation load requires an explicit source reference of at most 4096 bytes"));
    }
    Ok(())
}

fn load_error(error: impl fmt::Display) -> CoreError {
    CoreError::InvalidDocument(format!("GEO/foundation load: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_commands(document: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
        let mut transaction = document.begin_transaction(id, document.revision());
        for command in commands {
            transaction.push(command);
        }
        document.commit(transaction).unwrap()
    }

    fn identity() -> EngineeringSourceIdentity {
        EngineeringSourceIdentity {
            source_manifest_sha256: "e".repeat(64),
            executable_sha256: None,
            solver_version: "geo-configured-load-source-test".into(),
            source_label: "Synthetic source lifecycle fixture; no native GUI or engineering acceptance".into(),
        }
    }

    #[test]
    fn configured_geo_load_uses_actual_global_reaction_at_selected_raft_origin() {
        let reaction = ConfiguredSupportReaction {
            support_node_id: structural_configurator::SemanticId::new("support-1").unwrap(),
            section_owners: BTreeSet::new(),
            ownership: ConfiguredReactionOwnership::SharedUnallocated,
            foundation_group_id: None,
            position_m: [12.0, 25.0, 3.0],
            force_moment: [0.0, 0.0, 100.0, 0.0, 0.0, 0.0],
        };
        let load = translated_group_reactions(&[&reaction], [10.0, 20.0, 3.0]).unwrap();
        assert_eq!(load.n_kn, 100.0);
        assert_eq!(load.mx_pressure_knm, 500.0);
        assert_eq!(load.my_pressure_knm, 200.0);
        assert_eq!(load.original_structural_reactions[0].location_m, [2.0, 5.0, 0.0]);
        assert_eq!(load.original_structural_reactions[0].external_force_kn, [0.0, 0.0, -100.0]);
        assert_eq!(load.components.len(), 1);
    }

    #[test]
    fn configured_geo_foundation_archive_tracks_load_edits_undo_and_native_roundtrip() {
        let mut document = Document::new("Configured reactions and selected GEO share one history").unwrap();
        document.set_history_policy(HistoryPolicy::compact(2)).unwrap();
        let mut commands = geo_foundation_demo_commands().unwrap();
        for command in &mut commands {
            if let Command::StructuralConfigurator(
                StructuralConfiguratorCommand::UpdateBuildingAndRegenerate { foundation_groups, .. },
            ) = command {
                // Retain the shared placement/frame/GEO fixture; bound only
                // this test's numerical mesh, as in existing configurator tests.
                for group in foundation_groups {
                    group.input.mesh = foundation_core::MeshSpec { nx: 4, ny: 4 };
                }
            }
        }
        commit_commands(&mut document, "configured-geo-demo", commands);
        let configured = analyze_configured_building(
            document.structural_configurator().current().unwrap(),
            ConfiguratorAnalysisRequest::WholeBuilding,
            identity(),
        ).unwrap();
        let configured_id = commit_commands(&mut document, "configured-load-archive", vec![
            Command::StructuralConfigurator(StructuralConfiguratorCommand::SaveAnalysisSnapshot {
                snapshot: configured.clone(),
            }),
        ]).delta.structural_configurator.created_snapshots[0];
        let mut numerical = foundation_core::RigidMatInput::default();
        numerical.mesh = foundation_core::MeshSpec { nx: 4, ny: 4 };
        let input = document.prepare_geo_foundation_analysis_from_configured("1", configured_id, numerical).unwrap();
        assert_eq!(input.effective_input.mesh, foundation_core::MeshSpec { nx: 4, ny: 4 });
        assert_eq!(input.load_binding, GeoFoundationLoadBinding::Configured {
            snapshot_id: configured_id,
            content_fingerprint: configured.content_fingerprint.clone(),
        });
        assert!((input.effective_input.load.n_kn - foundation_core::reference_load_300_tonnes().n_kn).abs() > 1.0);
        let group_id = foundation_core::FoundationId::new("1").unwrap();
        let archived_group = configured.foundations.iter().find(|group| group.foundation_id == group_id).unwrap();
        let expected_supports: BTreeSet<_> = archived_group.support_node_ids.iter()
            .map(|id| format!("configured-support:{id}")).collect();
        let actual_supports: BTreeSet<_> = input.effective_input.load.original_structural_reactions.iter()
            .map(|reaction| reaction.source_id.clone()).collect();
        assert!(!expected_supports.is_empty());
        assert_eq!(actual_supports, expected_supports);
        assert_eq!(input.effective_input.load.original_structural_reactions.len(), archived_group.support_node_ids.len());
        let foundation_core::FoundationPlacement::Raft { center_m, .. } = &input.placement else {
            panic!("shared synthetic fixture requires a raft");
        };
        for original in &input.effective_input.load.original_structural_reactions {
            let reaction = configured.reactions.reactions.iter().find(|reaction|
                original.source_id == format!("configured-support:{}", reaction.support_node_id)).unwrap();
            assert_eq!(reaction.foundation_group_id.as_ref(), Some(&group_id));
            assert_eq!(original.location_m, std::array::from_fn(|axis| reaction.position_m[axis] - center_m[axis]));
            assert_eq!(original.external_force_kn, [-reaction.force_moment[0], -reaction.force_moment[1], -reaction.force_moment[2]]);
            assert_eq!(original.external_free_moment_knm, [-reaction.force_moment[3], -reaction.force_moment[4], -reaction.force_moment[5]]);
        }
        let snapshot = analyze_geo_foundation(input, identity()).unwrap();
        let id = commit_commands(&mut document, "geo-configured-archive", vec![
            Command::GeoFoundation(GeoFoundationCommand::SaveAnalysisSnapshot { snapshot: snapshot.clone() }),
        ]).delta.geo_foundation.created_snapshots[0];
        assert_eq!(document.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));
        assert_eq!(document.geo_foundation().ground_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));

        let source = document.structural_configurator().current().unwrap();
        let mut building = source.building.clone();
        let foundation_groups = source.foundation_groups.values().cloned().collect();
        let section_id = building.sections[0].id.clone();
        let raised_eave_m = building.sections[0].roof.eave_abs_m + 0.5;
        structural_configurator::set_section_eave(
            &mut building, section_id.as_str(), raised_eave_m,
            structural_configurator::RoofHeightUpdate::PreserveRise,
        ).unwrap();
        let changed = commit_commands(&mut document, "configured-height-edit", vec![
            Command::StructuralConfigurator(StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
                building, foundation_groups,
            }),
        ]);
        assert!(changed.delta.geo_foundation.affected_snapshots.contains(&id));
        assert!(!changed.delta.geo_foundation.state_changed);
        assert_eq!(document.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Stale));
        assert_eq!(document.geo_foundation().ground_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));
        assert_eq!(document.geo_foundation().snapshot(id), Some(&snapshot));
        document.undo().unwrap().unwrap();
        assert_eq!(document.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));
        // Native Save/Open serialization, independent of any GUI or disk-I/O
        // acceptance claim. Historical load sources and the undone branch stay.
        let mut reopened = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
        assert_eq!(reopened.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));
        assert_eq!(reopened.structural_configurator().snapshot_status(configured_id), Some(EngineeringSnapshotStatus::Current));
        assert_eq!(reopened.geo_foundation().snapshot(id), Some(&snapshot));

        let mut soil = reopened.geo_state().records.soils.values().next().unwrap().clone();
        if let geo_core::Parameter::Known { value, .. } = &mut soil.parameters.cohesion_kpa {
            *value += 0.1;
        } else {
            panic!("shared synthetic fixture requires an explicit soil cohesion");
        }
        let geo_edit = commit_commands(&mut reopened, "selected-soil-only-edit", vec![
            Command::GeoFoundation(GeoFoundationCommand::EditGeo {
                command: geo_core::GeoCommand::UpsertSoil { record: soil },
            }),
        ]);
        assert!(geo_edit.delta.geo_foundation.affected_snapshots.contains(&id));
        assert_eq!(reopened.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Stale));
        assert_eq!(reopened.structural_configurator().snapshot_status(configured_id), Some(EngineeringSnapshotStatus::Current));
        reopened.undo().unwrap().unwrap();
        assert_eq!(reopened.geo_foundation_snapshot_status(id), Some(EngineeringSnapshotStatus::Current));
        assert_eq!(reopened.structural_configurator().snapshot_status(configured_id), Some(EngineeringSnapshotStatus::Current));
    }
}
