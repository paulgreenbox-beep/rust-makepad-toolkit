use crate::*;
use structural_configurator::{BuildingConfig, FixtureKind, GridSpacing, SemanticId};

fn building(kind: FixtureKind) -> BuildingConfig {
    let mut result = structural_configurator::fixture(kind);
    for section in &mut result.sections {
        section.grid.purlin_pitch_m = 100.0;
        section.grid.wall_girt_pitch_m = 100.0;
    }
    result
}
fn update(building: BuildingConfig, foundation_groups: Vec<FoundationAssignmentGroup>) -> Command {
    Command::StructuralConfigurator(StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
        building,
        foundation_groups,
    })
}
fn commit(d: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut tx = d.begin_transaction(id, d.revision());
    for command in commands {
        tx.push(command);
    }
    d.commit(tx).unwrap()
}
fn producer() -> EngineeringSourceIdentity {
    EngineeringSourceIdentity {
        source_manifest_sha256: "a".repeat(64),
        executable_sha256: None,
        solver_version: "configurator-test-only".into(),
        source_label: "Synthetic source fixture; no execution or engineering certificate".into(),
    }
}
fn group(
    building: &BuildingConfig,
    id: &str,
    sections: impl IntoIterator<Item = SemanticId>,
) -> FoundationAssignmentGroup {
    let mut input = foundation_core::RigidMatInput::default();
    input.foundation_id = foundation_core::FoundationId::new(id).unwrap();
    input.geometry.width_x_m = 100.0;
    input.geometry.length_y_m = 100.0;
    input.mesh = foundation_core::MeshSpec { nx: 4, ny: 4 };
    input.self_weight_policy = foundation_core::SelfWeightPolicy::ExcludedExplicitly;
    let _ = building; // Explicit synthetic test raft; no inference of design dimensions.
    FoundationAssignmentGroup {
        foundation_id: input.foundation_id.clone(),
        member_section_ids: sections.into_iter().collect(),
        input,
        reference_point_m: [5.0, 6.0, 0.0],
    }
}
fn rehash(snapshot: &mut ConfiguratorAnalysisSnapshot) {
    snapshot.content_fingerprint = configured_building::configurator_fingerprint(&(
        snapshot.schema_version,
        &snapshot.source,
        &snapshot.request,
        &snapshot.scope,
        &snapshot.dense_ids,
        &snapshot.structural,
        &snapshot.reactions,
        &snapshot.foundations,
    ))
    .unwrap();
}

#[test]
fn configurator_atomic_rollback_native6_and_original_identity_roundtrip() {
    let mut d = Document::new("One configured Document").unwrap();
    let uuid = d.document_uuid;
    let original = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("rollback", 0)
        .with(update(building(FixtureKind::IndependentJoint), vec![]))
        .with(Command::CreateLevel {
            name: "Invalid BIM".into(),
            elevation_m: f64::NAN,
            height_m: None,
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(original, d.to_json_string().unwrap());
    let request = update(building(FixtureKind::IndependentJoint), vec![]);
    let receipt = commit(
        &mut d,
        "generate",
        vec![
            request.clone(),
            Command::CreateLevel {
                name: "BIM shares revision".into(),
                elevation_m: 0.0,
                height_m: None,
            },
        ],
    );
    assert_eq!(receipt.operation_ids[0], OperationId(1));
    assert_eq!(
        d.structural_configurator()
            .current()
            .unwrap()
            .source_revision
            .operation_id,
        OperationId(1)
    );
    assert!(
        !receipt
            .delta
            .structural_configurator
            .generation_delta
            .as_ref()
            .unwrap()
            .added
            .is_empty()
    );
    let value: serde_json::Value = serde_json::from_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(value["format_version"], 6);
    assert_eq!(
        value["required_features"],
        serde_json::json!([
            "typed_drafting_v1",
            "geotech_slope_v1",
            "geotech_circular_v1",
            "foundation_structural_v1",
            "structural_configurator_v1"
        ])
    );
    let before = d.structural_configurator().clone();
    d.undo().unwrap();
    assert!(d.structural_configurator().is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&d.to_json_string().unwrap()).unwrap()["format_version"],
        6
    );
    d.redo().unwrap();
    assert_eq!(d.structural_configurator(), &before);
    let reopened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(
        reopened.structural_configurator(),
        d.structural_configurator()
    );
    assert_eq!(reopened.document_uuid, uuid);
    assert!(
        reopened
            .replay_revision(0)
            .unwrap()
            .structural_configurator()
            .is_empty()
    );
    let mut tx = d.begin_transaction("generate", 0);
    tx.push(request);
    tx.push(Command::CreateLevel {
        name: "BIM shares revision".into(),
        elevation_m: 0.0,
        height_m: None,
    });
    let replay = d.commit(tx).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.operation_ids, receipt.operation_ids);
}

#[test]
fn configurator_independent_b_geometry_and_ids_survive_a_edit_and_noop_regenerate() {
    let mut d = Document::new("Independent geometry deltas").unwrap();
    let mut config = building(FixtureKind::IndependentJoint);
    let a = config.sections[0].id.clone();
    let b = config.sections[1].id.clone();
    commit(&mut d, "v1", vec![update(config.clone(), vec![])]);
    let old = d
        .structural_configurator()
        .current()
        .unwrap()
        .generated
        .clone();
    structural_configurator::set_section_eave(
        &mut config,
        a.as_str(),
        7.5,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    let receipt = commit(&mut d, "a-edit", vec![update(config.clone(), vec![])]);
    let delta = &receipt.delta.structural_configurator;
    assert!(delta.affected_section_ids.contains(&a));
    assert!(!delta.affected_section_ids.contains(&b));
    let new = &d.structural_configurator().current().unwrap().generated;
    for node in old.nodes.values().filter(|node| node.owners.contains(&b)) {
        assert_eq!(new.nodes.get(&node.id), Some(node));
        assert!(
            delta
                .generation_delta
                .as_ref()
                .unwrap()
                .unchanged
                .contains(&node.id)
        );
    }
    for member in old
        .members
        .values()
        .filter(|member| member.owners.contains(&b))
    {
        assert_eq!(new.members.get(&member.id), Some(member));
    }
    for surface in old
        .surfaces
        .values()
        .filter(|surface| surface.section_id == b)
    {
        assert_eq!(new.surfaces.get(&surface.id), Some(surface));
    }
    let noop = commit(&mut d, "noop", vec![update(config, vec![])]);
    let delta = noop.delta.structural_configurator;
    assert!(!delta.building_changed);
    assert!(delta.analysis_invalidated);
    assert!(delta.affected_section_ids.is_empty());
    let generated = delta.generation_delta.unwrap();
    assert!(
        generated.added.is_empty() && generated.removed.is_empty() && generated.changed.is_empty()
    );
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

// CHEAPEST_VERIFICATION_PATH: Root runs the configurator_runtime_cache tests,
// then the Core configurator consumer suite. No GUI or solver acceptance.
#[test]
fn configurator_runtime_cache_reuses_b_in_ordinary_update() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Document>();

    let mut d = Document::new("Ordinary command raw section reuse").unwrap();
    let mut config = building(FixtureKind::IndependentJoint);
    let a = config.sections[0].id.clone();
    let b = config.sections[1].id.clone();
    assert_eq!(config.sections[0].roof.eave_abs_m, 7.0);
    let created = commit(&mut d, "cache-create", vec![update(config.clone(), vec![])]);
    assert_eq!(created.operation_ids, vec![OperationId(1)]);
    let cold = &d.configurator_generation_diagnostics().unwrap().generation;
    assert_eq!(
        cold.raw_sections_recomputed,
        BTreeSet::from([a.clone(), b.clone()])
    );
    assert!(cold.raw_sections_reused.is_empty());
    assert_eq!(cold.raw_section_generation_calls, 2);
    let before = d
        .structural_configurator()
        .current()
        .unwrap()
        .generated
        .clone();

    structural_configurator::set_section_eave(
        &mut config,
        a.as_str(),
        8.0,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    let edited = commit(&mut d, "cache-a-8", vec![update(config.clone(), vec![])]);
    assert_eq!(edited.operation_ids, vec![OperationId(2)]);
    assert!(edited.created_elements.is_empty());
    let observed = d.configurator_generation_diagnostics().unwrap();
    let source = d.structural_configurator().current().unwrap();
    assert_eq!(
        observed.source_operation_id,
        source.source_revision.operation_id
    );
    assert_eq!(observed.request_input_fingerprint, source.input_fingerprint);
    assert_eq!(
        observed.generation.raw_sections_recomputed,
        BTreeSet::from([a])
    );
    assert_eq!(
        observed.generation.raw_sections_reused,
        BTreeSet::from([b.clone()])
    );
    assert_eq!(observed.generation.raw_section_generation_calls, 1);
    assert!(observed.generation.full_graph_postprocess);
    assert_eq!(
        observed.generation.interfaces_regenerated,
        config.interfaces.len()
    );
    assert_eq!(
        source.generated,
        structural_configurator::generate_building(&config).unwrap()
    );
    for node in before
        .nodes
        .values()
        .filter(|node| node.owners.contains(&b))
    {
        assert_eq!(source.generated.nodes.get(&node.id), Some(node));
    }
    for member in before
        .members
        .values()
        .filter(|member| member.owners.contains(&b))
    {
        assert_eq!(source.generated.members.get(&member.id), Some(member));
    }
    for surface in before
        .surfaces
        .values()
        .filter(|surface| surface.section_id == b)
    {
        assert_eq!(source.generated.surfaces.get(&surface.id), Some(surface));
    }
    assert_eq!(source.generated.outlines.get(&b), before.outlines.get(&b));
}

#[test]
fn configurator_runtime_cache_is_not_native_history_or_clone_authority() {
    let mut d = Document::new("Cache lifetime is process local").unwrap();
    let mut config = building(FixtureKind::IndependentJoint);
    let a = config.sections[0].id.clone();
    let section_ids: BTreeSet<_> = config
        .sections
        .iter()
        .map(|section| section.id.clone())
        .collect();
    commit(
        &mut d,
        "cache-initial",
        vec![update(config.clone(), vec![])],
    );
    structural_configurator::set_section_eave(
        &mut config,
        a.as_str(),
        8.0,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    commit(&mut d, "cache-edited", vec![update(config.clone(), vec![])]);
    let observed = d.configurator_generation_diagnostics().unwrap().clone();
    let native = d.to_json_string().unwrap();
    assert_eq!(d.configurator_generation_diagnostics(), Some(&observed));
    let native_value: serde_json::Value = serde_json::from_str(&native).unwrap();
    assert_eq!(native_value["format_version"], 6);
    assert!(
        native_value["document"]
            .get("configurator_generator_runtime")
            .is_none()
    );
    assert!(
        native_value["document"]
            .get("runtime_generation_diagnostics")
            .is_none()
    );

    d.undo().unwrap().unwrap();
    assert_eq!(d.configurator_generation_diagnostics(), Some(&observed));
    assert_ne!(
        d.structural_configurator()
            .current()
            .unwrap()
            .input_fingerprint,
        observed.request_input_fingerprint
    );
    d.redo().unwrap().unwrap();
    assert_eq!(d.configurator_generation_diagnostics(), Some(&observed));
    assert!(
        d.replay_revision(1)
            .unwrap()
            .configurator_generation_diagnostics()
            .is_none()
    );

    let mut cloned = d.clone();
    assert!(cloned.configurator_generation_diagnostics().is_none());
    assert_eq!(
        cloned.to_json_string().unwrap(),
        d.to_json_string().unwrap()
    );
    commit(
        &mut cloned,
        "cache-clone-cold",
        vec![update(config.clone(), vec![])],
    );
    assert_eq!(
        cloned
            .configurator_generation_diagnostics()
            .unwrap()
            .generation
            .raw_sections_recomputed,
        section_ids
    );
    assert!(
        cloned
            .configurator_generation_diagnostics()
            .unwrap()
            .generation
            .raw_sections_reused
            .is_empty()
    );
    assert_eq!(d.configurator_generation_diagnostics(), Some(&observed));

    let mut reopened = Document::from_json_str(&native).unwrap();
    assert_eq!(
        reopened.structural_configurator(),
        d.structural_configurator()
    );
    assert!(reopened.configurator_generation_diagnostics().is_none());
    assert_eq!(reopened.to_json_string().unwrap(), native);
    structural_configurator::set_section_eave(
        &mut config,
        a.as_str(),
        9.0,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    commit(
        &mut reopened,
        "cache-reopen-cold",
        vec![update(config.clone(), vec![])],
    );
    let first_after_open = &reopened
        .configurator_generation_diagnostics()
        .unwrap()
        .generation;
    assert_eq!(first_after_open.raw_sections_recomputed, section_ids);
    assert!(first_after_open.raw_sections_reused.is_empty());
    assert_eq!(first_after_open.raw_section_generation_calls, 2);
    assert_eq!(
        reopened
            .structural_configurator()
            .current()
            .unwrap()
            .generated,
        structural_configurator::generate_building(&config).unwrap()
    );
}

#[test]
fn configurator_runtime_cache_failed_batch_preserves_atomic_document() {
    let mut d = Document::new("Failed cache update is not authority").unwrap();
    let mut config = building(FixtureKind::IndependentJoint);
    let a = config.sections[0].id.clone();
    let section_ids: BTreeSet<_> = config
        .sections
        .iter()
        .map(|section| section.id.clone())
        .collect();
    commit(
        &mut d,
        "cache-before-failure",
        vec![update(config.clone(), vec![])],
    );
    let before = d.to_json_string().unwrap();
    structural_configurator::set_section_eave(
        &mut config,
        a.as_str(),
        8.0,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    let tx = d
        .begin_transaction("cache-failed-batch", d.revision())
        .with(update(config.clone(), vec![]))
        .with(Command::CreateLevel {
            name: "Invalid second command".into(),
            elevation_m: f64::NAN,
            height_m: None,
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    assert!(d.configurator_generation_diagnostics().is_none());
    let retried = commit(
        &mut d,
        "cache-after-failure",
        vec![update(config.clone(), vec![])],
    );
    assert_eq!(retried.operation_ids, vec![OperationId(2)]);
    let diagnostics = &d.configurator_generation_diagnostics().unwrap().generation;
    assert_eq!(diagnostics.raw_sections_recomputed, section_ids);
    assert!(diagnostics.raw_sections_reused.is_empty());
    assert_eq!(
        d.structural_configurator().current().unwrap().generated,
        structural_configurator::generate_building(&config).unwrap()
    );
}

#[test]
fn configurator_connected_capacity_is_one_gate_and_does_not_block_document_save() {
    let mut config = building(FixtureKind::EqualHeightConnected);
    for section in &mut config.sections {
        section.grid.column_stations = GridSpacing::Uniform { pitch_m: 0.5 };
    }
    let seed = config.sections[0].id.clone();
    let mut d = Document::new("Large full BIM remains saveable").unwrap();
    commit(&mut d, "large", vec![update(config, vec![])]);
    let source = d.structural_configurator().current().unwrap();
    let request = ConfiguratorAnalysisRequest::ConnectedComponent {
        seed_section_id: seed,
    };
    match configured_analysis_readiness(source, &request).unwrap() {
        ConfiguratorAnalysisReadiness::AnalysisModelCapacityExceeded {
            scope,
            nodes,
            members,
            max_nodes,
            max_members,
        } => {
            assert_eq!(scope.section_ids.len(), 2);
            assert!(nodes > 64 || members > 128);
            assert_eq!((max_nodes, max_members), (64, 128));
        }
        other => panic!("expected common connected capacity gate, got {other:?}"),
    }
    assert!(configured_analysis_input(source, &request).is_err());
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

#[test]
fn configurator_whole_building_refuses_unknown_interfaces() {
    let mut d = Document::new("Unverified interface retained").unwrap();
    commit(
        &mut d,
        "geometry",
        vec![update(building(FixtureKind::GeometryOnly), vec![])],
    );
    assert!(matches!(
        configured_analysis_readiness(
            d.structural_configurator().current().unwrap(),
            &ConfiguratorAnalysisRequest::WholeBuilding
        )
        .unwrap(),
        ConfiguratorAnalysisReadiness::InterfaceLoadPathNotVerified { .. }
    ));
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

#[test]
fn configurator_common_group_gets_shared_reactions_once_and_archive_becomes_stale() {
    let config = building(FixtureKind::EqualHeightConnected);
    let seed = config.sections[0].id.clone();
    let common = group(&config, "1", config.sections.iter().map(|s| s.id.clone()));
    let mut d = Document::new("Common raft explicit group").unwrap();
    commit(
        &mut d,
        "v1",
        vec![update(config.clone(), vec![common.clone()])],
    );
    let snapshot = analyze_configured_building(
        d.structural_configurator().current().unwrap(),
        ConfiguratorAnalysisRequest::ConnectedComponent {
            seed_section_id: seed.clone(),
        },
        producer(),
    )
    .unwrap();
    assert_eq!(snapshot.scope.section_ids.len(), 2);
    assert_eq!(snapshot.foundations.len(), 1);
    assert_eq!(
        snapshot.foundations[0].status,
        ConfiguredFoundationStatus::Computed
    );
    assert!(
        snapshot
            .reactions
            .reactions
            .iter()
            .any(|r| matches!(r.ownership, ConfiguredReactionOwnership::SharedUnallocated))
    );
    let physical: BTreeSet<_> = snapshot
        .reactions
        .reactions
        .iter()
        .map(|r| r.support_node_id.clone())
        .collect();
    assert_eq!(physical.len(), snapshot.reactions.reactions.len());
    assert_eq!(snapshot.foundations[0].support_node_ids, physical);
    for axis in 0..6 {
        let exclusive: f64 = snapshot
            .reactions
            .exclusive_section_totals
            .values()
            .map(|sum| sum[axis])
            .sum();
        assert!(
            (exclusive + snapshot.reactions.shared_unallocated_total[axis]
                - snapshot.reactions.global_total[axis])
                .abs()
                < 1e-8
        );
    }
    let archive = commit(
        &mut d,
        "run",
        vec![Command::StructuralConfigurator(
            StructuralConfiguratorCommand::SaveAnalysisSnapshot {
                snapshot: snapshot.clone(),
            },
        )],
    )
    .delta
    .structural_configurator
    .created_snapshots[0];
    let mut forged = snapshot.clone();
    forged.scope.section_ids = BTreeSet::from([seed.clone()]);
    rehash(&mut forged);
    assert!(forged.validate().is_err());
    let mut edited = config;
    structural_configurator::set_section_eave(
        &mut edited,
        seed.as_str(),
        7.5,
        structural_configurator::RoofHeightUpdate::PreserveRise,
    )
    .unwrap();
    commit(&mut d, "v2", vec![update(edited, vec![common])]);
    assert_eq!(
        d.structural_configurator().snapshot_status(archive),
        Some(EngineeringSnapshotStatus::Stale)
    );
    assert_eq!(
        d.structural_configurator().snapshot(archive),
        Some(&snapshot)
    );
    d.undo().unwrap();
    assert_eq!(
        d.structural_configurator().snapshot_status(archive),
        Some(EngineeringSnapshotStatus::Current)
    );
    d.redo().unwrap();
    assert_eq!(
        d.structural_configurator().snapshot_status(archive),
        Some(EngineeringSnapshotStatus::Stale)
    );
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

#[test]
fn configurator_independent_whole_acknowledgement_preserves_two_foundation_groups() {
    let config = building(FixtureKind::IndependentJoint);
    assert!(config.allow_disconnected_analysis);
    let groups: Vec<_> = config
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| group(&config, &(i + 1).to_string(), [section.id.clone()]))
        .collect();
    let mut d = Document::new("Independent groups never merged").unwrap();
    commit(&mut d, "generate", vec![update(config, groups)]);
    let source = d.structural_configurator().current().unwrap();
    assert!(matches!(
        configured_analysis_readiness(source, &ConfiguratorAnalysisRequest::WholeBuilding).unwrap(),
        ConfiguratorAnalysisReadiness::Ready { .. }
    ));
    let snapshot = analyze_configured_building(
        source,
        ConfiguratorAnalysisRequest::WholeBuilding,
        producer(),
    )
    .unwrap();
    assert_eq!(snapshot.scope.section_ids.len(), 2);
    assert_eq!(snapshot.foundations.len(), 2);
    assert!(
        snapshot
            .foundations
            .iter()
            .all(|result| result.status == ConfiguredFoundationStatus::Computed)
    );
    assert!(
        snapshot.foundations[0]
            .support_node_ids
            .is_disjoint(&snapshot.foundations[1].support_node_ids)
    );
    assert!(
        snapshot
            .reactions
            .reactions
            .iter()
            .all(|r| matches!(r.ownership, ConfiguredReactionOwnership::Exclusive { .. }))
    );
    snapshot.validate().unwrap();
}

#[test]
fn configurator_separate_groups_cannot_each_claim_a_shared_support() {
    let config = building(FixtureKind::EqualHeightConnected);
    let groups: Vec<_> = config
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| group(&config, &(i + 1).to_string(), [section.id.clone()]))
        .collect();
    let seed = config.sections[0].id.clone();
    let mut d = Document::new("Shared reactions stay unallocated").unwrap();
    commit(&mut d, "generate", vec![update(config, groups)]);
    let snapshot = analyze_configured_building(
        d.structural_configurator().current().unwrap(),
        ConfiguratorAnalysisRequest::ConnectedComponent {
            seed_section_id: seed,
        },
        producer(),
    )
    .unwrap();
    assert_eq!(snapshot.foundations.len(), 2);
    assert!(snapshot.foundations.iter().all(|r| r.status
        == ConfiguredFoundationStatus::SharedReactionsUnallocated
        && r.result.is_none()));
    assert!(
        snapshot
            .reactions
            .reactions
            .iter()
            .filter(|r| r.section_owners.len() > 1)
            .all(|r| r.foundation_group_id.is_none())
    );
    snapshot.validate().unwrap();
}

#[test]
fn configurator_native6_rejects_downgrade_forged_features_and_missing_original_request() {
    let mut d = Document::new("Native6 typed authority").unwrap();
    commit(
        &mut d,
        "generate",
        vec![update(building(FixtureKind::IndependentJoint), vec![])],
    );
    let original: serde_json::Value = serde_json::from_str(&d.to_json_string().unwrap()).unwrap();
    let mut downgraded = original.clone();
    downgraded["format_version"] = 5.into();
    downgraded["document"]["format_version"] = 5.into();
    downgraded["required_features"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(Document::from_json_str(&downgraded.to_string()).is_err());
    let mut features = original.clone();
    features["required_features"].as_array_mut().unwrap().pop();
    assert!(Document::from_json_str(&features.to_string()).is_err());
    assert!(Document::from_json_str(&original["document"].to_string()).is_err());
    let mut missing = original;
    missing["document"]["command_receipts"]["generate"]
        .as_object_mut()
        .unwrap()
        .remove("recorded_commands");
    assert!(Document::from_json_str(&missing.to_string()).is_err());
    let empty = Document::new("Old documents remain native2").unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&empty.to_json_string().unwrap()).unwrap()["format_version"],
        2
    );
}

/// A tiny stored graph with adversarial, rehashed parameters. Only two pitch
/// numbers change; this helper never generates the requested million-node grid.
fn rehash_million_grid_request(source: &mut ConfiguredBuildingState) {
    let section = &mut source.building.sections[0];
    let bounds = structural_configurator::geometry::bounds(section);
    section.grid.column_stations = GridSpacing::Uniform {
        pitch_m: (bounds.u_max - bounds.u_min) / 1000.0,
    };
    section.grid.frame_stations = GridSpacing::Uniform {
        pitch_m: (bounds.v_max - bounds.v_min) / 1000.0,
    };
    source.input_fingerprint = configured_building::configurator_fingerprint(&(
        &source.building,
        &source.foundation_groups,
    ))
    .unwrap();
    let mut canonical = source.building.clone();
    canonical.sections.sort_by(|a, b| a.id.cmp(&b.id));
    canonical.interfaces.sort_by(|a, b| a.id.cmp(&b.id));
    for interface in &mut canonical.interfaces {
        interface.anchors.sort_by(|a, b| a.id.cmp(&b.id));
    }
    canonical.materials.sort_by(|a, b| a.id.cmp(&b.id));
    canonical.profiles.sort_by(|a, b| a.id.cmp(&b.id));
    canonical.foundation_assignments.sort_by(|a, b| {
        (&a.section_id, &a.support_node_id, &a.foundation_id).cmp(&(
            &b.section_id,
            &b.support_node_id,
            &b.foundation_id,
        ))
    });
    source.generated.config_fingerprint =
        configured_building::configurator_fingerprint(&canonical).unwrap();
    source.generated.graph_fingerprint.clear();
    source.generated.graph_fingerprint =
        configured_building::configurator_fingerprint(&source.generated).unwrap();
    assert!(serde_json::to_vec(source).unwrap().len() < 128 * 1024);
}
fn assert_early_catalog_resource_refusal(error: CoreError) {
    assert!(
        matches!(error,CoreError::InvalidDocument(ref message)
        if message.contains("ResourceLimit") && message.contains("station Cartesian product")),
        "unexpected refusal: {error}"
    );
}

#[test]
fn configurator_early_resource_gate_refuses_tiny_rehashed_million_grid_before_semantics() {
    let mut d = Document::new("Early source resource gate").unwrap();
    commit(
        &mut d,
        "small",
        vec![update(building(FixtureKind::IndependentJoint), vec![])],
    );
    let mut source = d.structural_configurator().current().unwrap().clone();
    let stored_nodes = source.generated.nodes.len();
    rehash_million_grid_request(&mut source);
    assert!(stored_nodes < 64);
    assert_eq!(source.generated.nodes.len(), stored_nodes);
    assert_early_catalog_resource_refusal(source.validate().unwrap_err());
}

#[test]
fn configurator_early_resource_gate_covers_native6_open_before_history_semantics() {
    let mut d = Document::new("Early Open resource gate").unwrap();
    commit(
        &mut d,
        "small",
        vec![update(building(FixtureKind::IndependentJoint), vec![])],
    );
    let mut native: serde_json::Value = serde_json::from_str(&d.to_json_string().unwrap()).unwrap();
    let mut source = d.structural_configurator().current().unwrap().clone();
    rehash_million_grid_request(&mut source);
    native["document"]["structural_configurator"]["current"] =
        serde_json::to_value(source).unwrap();
    let encoded = native.to_string();
    assert!(encoded.len() < 1024 * 1024);
    assert_early_catalog_resource_refusal(Document::from_json_str(&encoded).unwrap_err());
}

#[test]
fn configurator_early_resource_gate_covers_archived_source_before_hash_and_projection() {
    let mut d = Document::new("Early archive resource gate").unwrap();
    commit(
        &mut d,
        "small",
        vec![update(building(FixtureKind::IndependentJoint), vec![])],
    );
    let mut snapshot = analyze_configured_building(
        d.structural_configurator().current().unwrap(),
        ConfiguratorAnalysisRequest::WholeBuilding,
        producer(),
    )
    .unwrap();
    rehash_million_grid_request(&mut snapshot.source);
    rehash(&mut snapshot);
    assert_early_catalog_resource_refusal(snapshot.validate().unwrap_err());
}

#[test]
fn configurator_shared_fixture_inputs_keep_cli_and_ui_assumptions_identical() {
    for kind in [
        FixtureKind::SteppedConnected,
        FixtureKind::EqualHeightConnected,
    ] {
        let (config, groups) = configured_fixture_input(kind).unwrap();
        assert_eq!(config, structural_configurator::fixture(kind));
        assert_eq!(groups.len(), 1);
        let common = &groups[0];
        assert_eq!(
            common.member_section_ids,
            config
                .sections
                .iter()
                .map(|section| section.id.clone())
                .collect()
        );
        assert_eq!(common.reference_point_m, [5.0, 6.0, 0.0]);
        assert_eq!(
            (
                common.input.geometry.width_x_m,
                common.input.geometry.length_y_m
            ),
            (10.0, 12.0)
        );
        assert_eq!(common.input.soil.id.as_str(), "3");
        assert!(!common.input.soil.engineering_site_verified);
        assert_eq!(
            common.input.self_weight_policy,
            foundation_core::SelfWeightPolicy::IncludeOnce {
                source_id: "configurator-demo-raft-own-weight".into()
            }
        );
        assert_eq!(common.foundation_id, common.input.foundation_id);
        foundation_core::validate_input(&common.input).unwrap();
    }
    for kind in [FixtureKind::IndependentJoint, FixtureKind::GeometryOnly] {
        let (config, groups) = configured_fixture_input(kind).unwrap();
        assert_eq!(config, structural_configurator::fixture(kind));
        assert!(groups.is_empty());
    }
}
