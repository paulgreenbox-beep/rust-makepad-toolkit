use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use structural_configurator::{
    connected_analysis_scope, dense_id_map, diff_generated, fixture, generate_building,
    set_section_eave, to_structural_model, to_structural_model_for_scope,
    validate_generated_binding, AnalysisProjectionStatus, ConfiguratorError,
    ConnectedAnalysisScope, FixtureKind, GeneratedBuilding, GridSpacing, GridStation,
    InterfaceAnchor, InterfaceKind, InterfaceLoadPathStatus, InterfaceSpec, RoofDetails,
    RoofHeightUpdate, RoofKind, SemanticId, SurfaceRole,
};

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual:.12}, expected {expected:.12} ± {tolerance}"
    );
}

fn area_for(generated: &structural_configurator::GeneratedBuilding, role: SurfaceRole) -> f64 {
    generated
        .surfaces
        .values()
        .filter(|surface| surface.role == role)
        .map(|surface| surface.area_m2)
        .sum()
}

fn b_owned_only_fingerprints(
    generated: &GeneratedBuilding,
) -> (BTreeMap<SemanticId, String>, BTreeMap<SemanticId, String>) {
    let b = SemanticId::new("B").expect("valid fixture ID");
    let nodes = generated
        .nodes
        .iter()
        .filter(|(_, node)| node.owners.len() == 1 && node.owners.contains(&b))
        .map(|(id, node)| (id.clone(), node.fingerprint.clone()))
        .collect();
    let members = generated
        .members
        .iter()
        .filter(|(_, member)| member.owners.len() == 1 && member.owners.contains(&b))
        .map(|(id, member)| (id.clone(), member.fingerprint.clone()))
        .collect();
    (nodes, members)
}

fn b_non_interface_surface_fingerprints(
    generated: &GeneratedBuilding,
) -> BTreeMap<SemanticId, String> {
    let b = SemanticId::new("B").expect("valid fixture ID");
    generated
        .surfaces
        .iter()
        .filter(|(_, surface)| {
            surface.section_id == b
                && (surface.role == SurfaceRole::RoofFacet
                    || !surface
                        .vertices_m
                        .iter()
                        .all(|point| (point[0] - 6.0).abs() <= 1.0e-9))
        })
        .map(|(id, surface)| (id.clone(), surface.fingerprint.clone()))
        .collect()
}

fn json_sha256<T: serde::Serialize>(value: &T) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable fixture"))
    )
}

fn rehash_generated_graph(generated: &mut GeneratedBuilding) {
    generated.graph_fingerprint.clear();
    generated.graph_fingerprint = json_sha256(generated);
}

#[test]
fn manual_wall_and_roof_areas_match_both_interface_geometries() {
    // The connected exterior-wall total removes both coincident interface walls
    // and adds the 12 m × 2.8 m exposed step. The joint keeps both full envelopes.
    let connected = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("connected fixture should generate BIM geometry");
    close(area_for(&connected, SurfaceRole::WallFace), 304.2, 1.0e-8);
    close(
        area_for(&connected, SurfaceRole::RoofFacet),
        129.03544338876847,
        1.0e-8,
    );

    let independent = generate_building(&fixture(FixtureKind::IndependentJoint))
        .expect("joint fixture should generate BIM geometry");
    close(area_for(&independent, SurfaceRole::WallFace), 405.0, 1.0e-8);
    close(
        area_for(&independent, SurfaceRole::RoofFacet),
        129.03544338876847,
        1.0e-8,
    );
}

#[test]
fn same_profile_id_numeric_edit_is_reported_as_a_generated_delta() {
    let before_config = fixture(FixtureKind::SteppedConnected);
    let before = generate_building(&before_config).expect("baseline fixture should generate");
    let mut after_config = before_config;
    after_config.profiles[0].area_m2 *= 1.1;
    let after = generate_building(&after_config).expect("profile edit should generate");

    assert_ne!(
        before.bim_quantities.steel_weight_kn,
        after.bim_quantities.steel_weight_kn
    );
    let delta = diff_generated(&before, &after);
    assert!(
        !delta.changed.is_empty(),
        "same-ID profile property edits must be visible in the delta"
    );
}

#[test]
fn unsupported_roof_detail_change_is_reported_even_without_generated_geometry_change() {
    // Keep the section's unsupported generation status fixed in both cases;
    // only the previously ignored roof detail input changes.
    let mut before_config = fixture(FixtureKind::IndependentJoint);
    let b = before_config
        .sections
        .iter_mut()
        .find(|section| section.id.as_str() == "B")
        .expect("section B exists");
    b.roof.kind = RoofKind::Stepped;
    let before = generate_building(&before_config)
        .expect("unsupported roof fixture should retain its bounded BIM output");

    let mut after_config = before_config;
    let b = after_config
        .sections
        .iter_mut()
        .find(|section| section.id.as_str() == "B")
        .expect("section B exists");
    let mut details = RoofDetails::default();
    details
        .corner_tops_abs_m
        .insert(SemanticId::new("b0").expect("valid fixture corner ID"), 5.0);
    b.roof.unsupported_details = Some(details);
    let after = generate_building(&after_config)
        .expect("unsupported roof detail metadata should remain a typed generation limit");

    assert_eq!(before.nodes, after.nodes);
    assert_eq!(before.members, after.members);
    assert_eq!(before.surfaces, after.surfaces);
    assert_eq!(before.outlines, after.outlines);
    let delta = diff_generated(&before, &after);
    assert!(
        !delta.changed.is_empty(),
        "parameter snapshot change must appear in the delta"
    );
}

#[test]
fn preserve_rise_changes_only_the_selected_section_height_pair() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let b_before = config.sections[1].clone();

    set_section_eave(&mut config, "A", 8.0, RoofHeightUpdate::PreserveRise)
        .expect("section A eave update should be supported");

    assert_eq!(config.sections[0].roof.eave_abs_m, 8.0);
    assert_eq!(config.sections[0].roof.ridge_abs_m, 9.5);
    assert_eq!(config.sections[1], b_before);
}

#[test]
fn changing_a_preserve_rise_keeps_b_semantics_and_dense_ids_stable() {
    let before_config = fixture(FixtureKind::SteppedConnected);
    let before = generate_building(&before_config).expect("baseline fixture should generate");
    let before_dense = dense_id_map(&before).expect("baseline dense IDs");

    let mut after_config = before_config;
    set_section_eave(&mut after_config, "A", 8.0, RoofHeightUpdate::PreserveRise)
        .expect("section A eave update should be supported");
    let after = generate_building(&after_config).expect("updated fixture should generate");
    let after_dense = dense_id_map(&after).expect("updated dense IDs");

    let (before_b_nodes, before_b_members) = b_owned_only_fingerprints(&before);
    let (after_b_nodes, after_b_members) = b_owned_only_fingerprints(&after);
    assert!(!before_b_nodes.is_empty());
    assert!(!before_b_members.is_empty());
    assert_eq!(
        before_b_nodes, after_b_nodes,
        "B-only node semantics should not change"
    );
    assert_eq!(
        before_b_members, after_b_members,
        "B-only member semantics should not change"
    );
    assert_eq!(
        b_non_interface_surface_fingerprints(&before),
        b_non_interface_surface_fingerprints(&after),
        "section B roof and non-interface surfaces should not change"
    );

    for id in before_b_nodes.keys() {
        assert_eq!(before_dense.nodes.get(id), after_dense.nodes.get(id));
    }
    for id in before_b_members.keys() {
        assert_eq!(before_dense.members.get(id), after_dense.members.get(id));
    }
}

#[test]
fn connected_fixture_stays_inside_the_bounded_analysis_projection() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("connected fixture should generate BIM geometry");

    assert!(
        generated.nodes.len() <= 64,
        "fixture exceeded the 64-node projection limit"
    );
    assert!(
        generated.members.len() <= 128,
        "fixture exceeded the 128-member projection limit"
    );
    assert!(matches!(
        generated.analysis_status,
        AnalysisProjectionStatus::ReadyForBoundedProjection
    ));
    let _model = to_structural_model(&generated)
        .expect("connected fixture should convert to the bounded structural model");
}

#[test]
fn a_connected_analysis_scope_cannot_partition_section_a_from_ab() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("connected fixture should generate");
    let full = connected_analysis_scope(&generated, "A").expect("A's full component exists");
    let expected = BTreeSet::from([
        SemanticId::new("A").expect("valid fixture ID"),
        SemanticId::new("B").expect("valid fixture ID"),
    ]);
    assert_eq!(full.section_ids, expected);

    let partial = ConnectedAnalysisScope {
        section_ids: BTreeSet::from([SemanticId::new("A").expect("valid fixture ID")]),
    };
    assert!(matches!(
        to_structural_model_for_scope(&generated, &partial),
        Err(ConfiguratorError::InvalidAnalysisScope { .. })
    ));
}

#[test]
fn independent_joint_translates_section_b_and_retains_no_shared_graph_ids() {
    let generated = generate_building(&fixture(FixtureKind::IndependentJoint))
        .expect("joint fixture should generate BIM geometry");
    let interface = generated
        .interfaces
        .first()
        .expect("fixture has one interface");

    assert_eq!(
        interface.status,
        InterfaceLoadPathStatus::IndependentWithJoint
    );
    assert!(interface.shared_node_ids.is_empty());
    assert!(interface.shared_member_ids.is_empty());

    let b_id = SemanticId::new("B").expect("valid fixture ID");
    let b = generated
        .outlines
        .get(&b_id)
        .expect("section B outline exists");
    let minimum_x = b
        .world_corners_m
        .iter()
        .map(|point| point[0])
        .fold(f64::INFINITY, f64::min);
    close(minimum_x, 6.1, 1.0e-9);
}

#[test]
fn explicit_column_and_uniform_purlin_station_labels_use_distinct_namespaces() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    config.sections[0].grid.column_stations = GridSpacing::Explicit {
        stations: vec![
            GridStation {
                id: SemanticId::new("start").expect("valid station ID"),
                position_m: 0.0,
            },
            GridStation {
                id: SemanticId::new("g001").expect("valid station ID"),
                position_m: 2.0,
            },
            GridStation {
                id: SemanticId::new("end").expect("valid station ID"),
                position_m: 6.0,
            },
        ],
    };
    config.sections[0].grid.purlin_pitch_m = 1.0;

    let generated = generate_building(&config)
        .expect("explicit column g001 and uniform purlin g001 must not collide");
    assert!(!generated.nodes.is_empty());
    assert!(!generated.members.is_empty());
}

#[test]
fn independent_joint_offsets_compose_across_a_three_section_chain() {
    let mut config = fixture(FixtureKind::IndependentJoint);
    let mut section_c = config.sections[1].clone();
    section_c.id = SemanticId::new("C").expect("valid fixture ID");
    for (corner, id) in section_c.corners.iter_mut().zip(["c0", "c1", "c2", "c3"]) {
        corner.id = SemanticId::new(id).expect("valid fixture corner ID");
        corner.xy_m[0] += 4.0;
    }
    config.sections.push(section_c);
    config.interfaces.push(InterfaceSpec {
        id: SemanticId::new("interface-b-c").expect("valid interface ID"),
        section_a: SemanticId::new("B").expect("valid fixture ID"),
        edge_a: [
            SemanticId::new("b1").expect("valid fixture corner ID"),
            SemanticId::new("b2").expect("valid fixture corner ID"),
        ],
        section_b: SemanticId::new("C").expect("valid fixture ID"),
        edge_b: [
            SemanticId::new("c3").expect("valid fixture corner ID"),
            SemanticId::new("c0").expect("valid fixture corner ID"),
        ],
        kind: InterfaceKind::IndependentWithJoint {
            width_m: 0.1,
            offset_b_direction: [1.0, 0.0],
        },
        anchors: vec![
            InterfaceAnchor {
                id: SemanticId::new("s0").expect("valid anchor ID"),
                fraction: 0.0,
            },
            InterfaceAnchor {
                id: SemanticId::new("s1").expect("valid anchor ID"),
                fraction: 0.5,
            },
            InterfaceAnchor {
                id: SemanticId::new("s2").expect("valid anchor ID"),
                fraction: 1.0,
            },
        ],
    });

    let generated = generate_building(&config)
        .expect("relative independent-joint offsets should compose along the chain");
    let b_id = SemanticId::new("B").expect("valid fixture ID");
    let c_id = SemanticId::new("C").expect("valid fixture ID");
    let min_x = |id: &SemanticId| {
        generated
            .outlines
            .get(id)
            .expect("section outline exists")
            .world_corners_m
            .iter()
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min)
    };
    close(min_x(&b_id), 6.1, 1.0e-9);
    close(min_x(&c_id), 10.2, 1.0e-9);
}

#[test]
fn independent_joint_root_is_stable_when_section_ids_and_order_change() {
    let original_config = fixture(FixtureKind::IndependentJoint);
    let original = generate_building(&original_config)
        .expect("baseline independent-joint fixture should generate");
    let old_a = SemanticId::new("A").expect("valid fixture ID");
    let old_b = SemanticId::new("B").expect("valid fixture ID");
    let new_z = SemanticId::new("Z").expect("valid fixture ID");
    let new_a = SemanticId::new("A").expect("valid fixture ID");

    let mut renamed_config = original_config;
    for section in &mut renamed_config.sections {
        if section.id == old_a {
            section.id = new_z.clone();
        } else if section.id == old_b {
            section.id = new_a.clone();
        }
    }
    for interface in &mut renamed_config.interfaces {
        if interface.section_a == old_a {
            interface.section_a = new_z.clone();
        } else if interface.section_a == old_b {
            interface.section_a = new_a.clone();
        }
        if interface.section_b == old_a {
            interface.section_b = new_z.clone();
        } else if interface.section_b == old_b {
            interface.section_b = new_a.clone();
        }
    }
    for assignment in &mut renamed_config.foundation_assignments {
        if assignment.section_id == old_a {
            assignment.section_id = new_z.clone();
        } else if assignment.section_id == old_b {
            assignment.section_id = new_a.clone();
        }
    }
    // Make the lexicographically first section the old B, so it cannot be
    // mistaken for the directed joint root. There is one interface in this
    // fixture; reverse its collection too to keep the operation explicit.
    renamed_config.sections.reverse();
    renamed_config.interfaces.reverse();

    let renamed = generate_building(&renamed_config)
        .expect("directed joint offsets should not depend on IDs or collection order");
    let original_a = original.outlines.get(&old_a).expect("original A outline");
    let original_b = original.outlines.get(&old_b).expect("original B outline");
    let renamed_z = renamed.outlines.get(&new_z).expect("renamed Z outline");
    let renamed_a = renamed.outlines.get(&new_a).expect("renamed A outline");

    assert_eq!(original_a.world_corners_m, renamed_z.world_corners_m);
    assert_eq!(original_b.world_corners_m, renamed_a.world_corners_m);
    close(
        renamed_z
            .world_corners_m
            .iter()
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min),
        0.0,
        0.0,
    );
    close(
        renamed_a
            .world_corners_m
            .iter()
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min),
        6.1,
        1.0e-9,
    );
}

#[test]
fn geometry_only_contact_retains_geometry_without_claiming_a_load_path() {
    let generated = generate_building(&fixture(FixtureKind::GeometryOnly))
        .expect("geometry-only fixture should retain BIM geometry");
    let interface = generated
        .interfaces
        .first()
        .expect("fixture has one interface");

    assert_eq!(
        interface.status,
        InterfaceLoadPathStatus::InterfaceLoadPathNotVerified
    );
    assert!(matches!(
        generated.analysis_status,
        AnalysisProjectionStatus::Blocked { .. }
    ));
}

#[test]
fn undeclared_adjacent_contact_blocks_analysis_even_when_disconnection_is_acknowledged() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    config.interfaces.clear();
    config.allow_disconnected_analysis = true;

    let generated =
        generate_building(&config).expect("undeclared contact should retain both BIM sections");
    let a = SemanticId::new("A").expect("valid fixture ID");
    let b = SemanticId::new("B").expect("valid fixture ID");
    let contact = generated
        .interfaces
        .iter()
        .find(|interface| {
            (interface.section_a == a && interface.section_b == b)
                || (interface.section_a == b && interface.section_b == a)
        })
        .expect("coincident undeclared sections should be reported as an implicit contact");
    assert_eq!(
        contact.status,
        InterfaceLoadPathStatus::InterfaceLoadPathNotVerified
    );
    assert!(contact.shared_node_ids.is_empty());
    assert!(contact.shared_member_ids.is_empty());

    let a_base_nodes: Vec<_> = generated
        .nodes
        .values()
        .filter(|node| {
            node.owners.contains(&a)
                && !node.owners.contains(&b)
                && node.restrained.is_some()
                && (node.position_m[0] - 6.0).abs() <= 1.0e-9
        })
        .collect();
    let b_base_nodes: Vec<_> = generated
        .nodes
        .values()
        .filter(|node| {
            node.owners.contains(&b)
                && !node.owners.contains(&a)
                && node.restrained.is_some()
                && (node.position_m[0] - 6.0).abs() <= 1.0e-9
        })
        .collect();
    let coincident_but_distinct = a_base_nodes.iter().any(|a_node| {
        b_base_nodes.iter().any(|b_node| {
            a_node.id != b_node.id
                && (0..3)
                    .all(|axis| (a_node.position_m[axis] - b_node.position_m[axis]).abs() <= 1.0e-9)
        })
    });
    assert!(
        coincident_but_distinct,
        "undeclared contact must retain separate coincident support nodes"
    );
    assert!(generated
        .nodes
        .values()
        .all(|node| { !(node.owners.contains(&a) && node.owners.contains(&b)) }));

    assert!(matches!(
        to_structural_model(&generated),
        Err(ConfiguratorError::InterfaceLoadPathNotVerified { .. })
    ));
}

#[test]
fn shared_column_weight_sources_are_unique_and_shared_supports_need_allocation() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("connected fixture should generate");
    let shared_column_ids: BTreeSet<SemanticId> = generated
        .members
        .values()
        .filter(|member| {
            member.role == structural_configurator::MemberRole::Column && member.owners.len() > 1
        })
        .map(|member| member.id.clone())
        .collect();
    assert!(
        !shared_column_ids.is_empty(),
        "fixture should have shared interface columns"
    );

    let shared_quantities: Vec<_> = generated
        .bim_quantities
        .steel
        .iter()
        .filter(|quantity| shared_column_ids.contains(&quantity.member_id))
        .collect();
    assert_eq!(shared_quantities.len(), shared_column_ids.len());
    let sources: BTreeSet<SemanticId> = shared_quantities
        .iter()
        .map(|quantity| quantity.source_id.clone())
        .collect();
    assert_eq!(
        sources.len(),
        shared_quantities.len(),
        "shared column source IDs must be unique"
    );

    let shared_supports: Vec<_> = generated
        .reaction_routing
        .iter()
        .filter(|route| route.section_owners.len() > 1)
        .collect();
    assert!(
        !shared_supports.is_empty(),
        "fixture should route shared interface supports"
    );
    assert!(shared_supports
        .iter()
        .all(|route| route.requires_explicit_allocation));
    assert!(shared_supports
        .iter()
        .all(|route| route.foundation_by_section.is_empty()));
}

#[test]
fn stale_generated_graph_binding_is_rejected() {
    let config = fixture(FixtureKind::SteppedConnected);
    let mut generated = generate_building(&config).expect("fixture should generate");
    let node = generated
        .nodes
        .values_mut()
        .next()
        .expect("fixture has generated nodes");
    node.position_m[0] += 0.125;

    assert!(matches!(
        validate_generated_binding(&config, &generated),
        Err(ConfiguratorError::GeneratedBindingMismatch { .. })
    ));
}

#[test]
fn recomputed_hashes_do_not_attest_forged_parameters_or_interface_semantics() {
    let mut altered_config = fixture(FixtureKind::SteppedConnected);
    let a = altered_config
        .sections
        .iter_mut()
        .find(|section| section.id.as_str() == "A")
        .expect("section A exists");
    a.roof.eave_abs_m += 1.0;
    a.roof.ridge_abs_m += 1.0;

    let mut forged = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("baseline fixture should generate");
    // Fixture arrays are already in canonical ID order. This simulates an
    // archive editor that changed parameters and then recomputed both hashes.
    forged.config_fingerprint = json_sha256(&altered_config);
    rehash_generated_graph(&mut forged);
    assert!(matches!(
        validate_generated_binding(&altered_config, &forged),
        Err(ConfiguratorError::GeneratedBindingMismatch { .. })
    ));

    let geometry_only = fixture(FixtureKind::GeometryOnly);
    let mut forged_interface =
        generate_building(&geometry_only).expect("geometry-only fixture should generate");
    forged_interface.interfaces[0].status = InterfaceLoadPathStatus::ExplicitRigidIdealization;
    rehash_generated_graph(&mut forged_interface);
    assert!(matches!(
        validate_generated_binding(&geometry_only, &forged_interface),
        Err(ConfiguratorError::GeneratedBindingMismatch { .. })
    ));
}

#[test]
fn rehashed_entity_omission_is_rejected_even_when_remaining_member_hashes_are_valid() {
    let config = fixture(FixtureKind::SteppedConnected);
    let mut forged = generate_building(&config).expect("fixture should generate");
    let removed_id = forged
        .members
        .keys()
        .next()
        .expect("fixture has members")
        .clone();
    forged.members.remove(&removed_id);
    rehash_generated_graph(&mut forged);

    assert!(matches!(
        validate_generated_binding(&config, &forged),
        Err(ConfiguratorError::GeneratedBindingMismatch { .. })
    ));
}

#[test]
fn duplicate_raw_generated_map_keys_are_rejected_during_deserialization() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("fixture should generate");
    let mut object = serde_json::to_value(&generated)
        .expect("generated fixture serializes")
        .as_object()
        .expect("generated fixture serializes as an object")
        .clone();
    let nodes = object.remove("nodes").expect("generated map is serialized");
    let entries = nodes.as_object().expect("nodes serialize as an object");
    let (key, node) = entries
        .iter()
        .next()
        .expect("fixture has at least one node");
    let encoded_nodes = serde_json::to_string(&nodes).expect("nodes serialize");
    let body = encoded_nodes
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .expect("nodes serialize as an object");
    let duplicate_nodes = format!(
        "{{{body},{}:{}}}",
        serde_json::to_string(key).expect("node ID serializes"),
        serde_json::to_string(node).expect("node serializes")
    );

    let mut fields: Vec<String> = object
        .iter()
        .map(|(field, value)| {
            format!(
                "{}:{}",
                serde_json::to_string(field).expect("field name serializes"),
                serde_json::to_string(value).expect("field serializes")
            )
        })
        .collect();
    fields.push(format!("\"nodes\":{duplicate_nodes}"));
    let raw = format!("{{{}}}", fields.join(","));

    let error = serde_json::from_str::<GeneratedBuilding>(&raw)
        .expect_err("a duplicate semantic node key must not be silently replaced");
    assert!(error.to_string().contains("duplicate map key"));
}

#[test]
fn high_density_bim_is_retained_but_analysis_capacity_failure_is_typed() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    config.sections[0].grid.purlin_pitch_m = 0.025;

    let generated = generate_building(&config)
        .expect("high-density but bounded BIM generation should retain geometry");
    assert!(matches!(
        generated.analysis_status,
        AnalysisProjectionStatus::AnalysisModelCapacityExceeded { .. }
    ));
    assert!(matches!(
        to_structural_model(&generated),
        Err(ConfiguratorError::AnalysisModelCapacityExceeded { .. })
    ));
}

#[test]
fn dense_id_maps_are_nonzero_collision_free_and_repeatable() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected))
        .expect("fixture should generate");
    let first = dense_id_map(&generated).expect("dense IDs should be available");
    let second = dense_id_map(&generated).expect("dense IDs should be repeatable");
    assert_eq!(first.nodes, second.nodes);
    assert_eq!(first.members, second.members);
    assert_eq!(first.materials, second.materials);
    assert_eq!(first.profiles, second.profiles);

    for values in [
        &first.nodes,
        &first.members,
        &first.materials,
        &first.profiles,
    ] {
        assert!(values.values().all(|value| *value != 0));
        let unique: BTreeSet<u64> = values.values().copied().collect();
        assert_eq!(
            unique.len(),
            values.len(),
            "dense ID namespace contains a collision"
        );
    }
}
