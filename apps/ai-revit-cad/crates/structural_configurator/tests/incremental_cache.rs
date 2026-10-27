use std::collections::BTreeSet;

use structural_configurator::{
    ConfiguratorError, FixtureKind, FoundationAssignment, GeneratedBuilding, GenerationDiagnostics,
    GeneratorCache, GeneratorCacheLimits, GeneratorCacheStats, InterfaceKind, MemberProfile,
    PanelLoadTransferStatus, PanelSpec, Provenance, RoofHeightUpdate, SemanticId, fixture,
    generate_building, generate_building_with_cache, set_section_eave, validate_generated_binding,
};

fn ids(values: &[&str]) -> BTreeSet<SemanticId> {
    values
        .iter()
        .map(|value| SemanticId::new(*value).expect("valid fixture ID"))
        .collect()
}

fn assert_json_equal(left: &GeneratedBuilding, right: &GeneratedBuilding) {
    assert_eq!(
        serde_json::to_vec(left).expect("left generated building serializes"),
        serde_json::to_vec(right).expect("right generated building serializes")
    );
}

fn cold_output(config: &structural_configurator::BuildingConfig) -> GeneratedBuilding {
    generate_building(config).expect("cold generator should accept the fixture input")
}

fn assert_matches_cold(
    config: &structural_configurator::BuildingConfig,
    cached: &GeneratedBuilding,
) {
    assert_json_equal(cached, &cold_output(config));
}

fn raw_call_count(cache: &GeneratorCache) -> u128 {
    cache.stats().total_raw_generation_calls as u128
}

fn cached_build(
    config: &structural_configurator::BuildingConfig,
    cache: &mut GeneratorCache,
) -> (GeneratedBuilding, GenerationDiagnostics) {
    let before = raw_call_count(cache);
    let generated =
        generate_building_with_cache(config, cache).expect("cached build should succeed");
    assert_raw_call_delta(cache, before, generated.1.raw_section_generation_calls);
    generated
}

fn assert_raw_call_delta(
    cache: &GeneratorCache,
    before: u128,
    diagnostic_calls: impl TryInto<u128>,
) {
    let expected = diagnostic_calls
        .try_into()
        .ok()
        .expect("diagnostic raw-call count fits u128");
    assert_eq!(raw_call_count(cache) - before, expected);
}

fn assert_section_sets(diagnostics: &GenerationDiagnostics, recomputed: &[&str], reused: &[&str]) {
    assert_eq!(diagnostics.raw_sections_recomputed, ids(recomputed));
    assert_eq!(diagnostics.raw_sections_reused, ids(reused));
}

fn assert_stats_equal(before: &GeneratorCacheStats, after: &GeneratorCacheStats) {
    assert_eq!(before.entries, after.entries);
    assert_eq!(before.entities, after.entities);
    assert_eq!(before.payload_bytes, after.payload_bytes);
    assert_eq!(before.total_hits, after.total_hits);
    assert_eq!(before.total_misses, after.total_misses);
    assert_eq!(before.total_evictions, after.total_evictions);
    assert_eq!(
        before.total_raw_generation_calls,
        after.total_raw_generation_calls
    );
}

fn interface_step_area(building: &GeneratedBuilding) -> f64 {
    building
        .surfaces
        .values()
        .filter(|surface| surface.id.as_str().ends_with("/step"))
        .map(|surface| surface.area_m2)
        .sum()
}

#[test]
fn cold_then_identical_warm_build_is_all_hit_and_json_equivalent() {
    let config = fixture(FixtureKind::SteppedConnected);
    let cold = cold_output(&config);
    let config_json = serde_json::to_vec(&config).expect("config serializes");
    let mut cache = GeneratorCache::default();

    let raw_before = raw_call_count(&cache);
    let (first, first_diag) = generate_building_with_cache(&config, &mut cache)
        .expect("first cached build should succeed");
    assert_json_equal(&first, &cold);
    assert_section_sets(&first_diag, &["A", "B"], &[]);
    assert_eq!(first_diag.raw_section_generation_calls, 2);
    assert_raw_call_delta(&cache, raw_before, first_diag.raw_section_generation_calls);
    validate_generated_binding(&config, &first)
        .expect("cold cached output must retain the independent parameter and graph binding");

    let before_second_hits = cache.stats().total_hits;
    let raw_before_second = raw_call_count(&cache);
    let (second, second_diag) = generate_building_with_cache(&config, &mut cache)
        .expect("identical warm build should succeed");
    assert_json_equal(&second, &cold);
    assert_section_sets(&second_diag, &[], &["A", "B"]);
    assert_eq!(second_diag.raw_section_generation_calls, 0);
    assert_raw_call_delta(
        &cache,
        raw_before_second,
        second_diag.raw_section_generation_calls,
    );
    assert!(cache.stats().total_hits >= before_second_hits + 2);
    assert_eq!(
        serde_json::to_vec(&config).expect("config serializes"),
        config_json
    );
}

#[test]
fn preserve_rise_reuses_b_recomputes_a_and_rebuilds_the_interface_step() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let (before, _) = cached_build(&config, &mut cache);

    // Independent rectangle arithmetic: 12 × (7.0 - 4.2) = 33.6 m².
    close(interface_step_area(&before), 33.6, 1.0e-9);
    set_section_eave(&mut config, "A", 8.0, RoofHeightUpdate::PreserveRise)
        .expect("PreserveRise edit should be valid");

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("changed cached build should succeed");
    assert_section_sets(&diagnostics, &["A"], &["B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 1);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert!(diagnostics.interfaces_regenerated > 0);
    assert!(diagnostics.full_graph_postprocess);
    assert_matches_cold(&config, &warm);

    // Independent rectangle arithmetic: 12 × (8.0 - 4.2) = 45.6 m².
    close(interface_step_area(&warm), 45.6, 1.0e-9);
    validate_generated_binding(&config, &warm)
        .expect("warm output must bind to the changed input without regenerating it");
}

#[test]
fn used_material_property_change_invalidates_both_sections() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    config.materials[0].elastic_modulus_kn_m2 += 1.0e6;

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("used material edit should rebuild");
    assert_section_sets(&diagnostics, &["A", "B"], &[]);
    assert_eq!(diagnostics.raw_section_generation_calls, 2);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

#[test]
fn used_profile_property_change_invalidates_both_sections() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    config.profiles[0].area_m2 *= 1.1;

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("used profile edit should rebuild");
    assert_section_sets(&diagnostics, &["A", "B"], &[]);
    assert_eq!(diagnostics.raw_section_generation_calls, 2);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

#[test]
fn unused_library_additions_do_not_miss_raw_sections() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);

    let mut unused_profile: MemberProfile = config.profiles[0].clone();
    unused_profile.id = SemanticId::new("profile-unused").expect("valid profile ID");
    unused_profile.name = "Unused synthetic profile".into();
    config.profiles.push(unused_profile);
    let mut unused_material = config.materials[0].clone();
    unused_material.id = SemanticId::new("material-unused").expect("valid material ID");
    unused_material.name = "Unused synthetic material".into();
    config.materials.push(unused_material);

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("unused library entries should not invalidate raw section geometry");
    assert_section_sets(&diagnostics, &[], &["A", "B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 0);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

#[test]
fn global_transform_change_invalidates_both_sections() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    config.transform.translation_m = [2.0, -3.0];

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("global transform edit should rebuild transformed sections");
    assert_section_sets(&diagnostics, &["A", "B"], &[]);
    assert_eq!(diagnostics.raw_section_generation_calls, 2);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

#[test]
fn panel_change_invalidates_only_its_host_section() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    config.sections[0].roof_panels = Some(PanelSpec {
        thickness_m: 0.08,
        layout_width_m: 1.0,
        layout_length_m: 6.0,
        material_label: "Synthetic fixture panel".into(),
        provenance: Provenance::Synthetic {
            note: "cache invalidation fixture".into(),
        },
        mass_kg_m2: Some(12.0),
        stiffness_enabled: false,
        load_transfer_status: PanelLoadTransferStatus::NotVerified,
        requested_features: Vec::new(),
    });

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("panel metadata should rebuild only its host section");
    assert_section_sets(&diagnostics, &["A"], &["B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 1);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

#[test]
fn joint_offset_invalidates_but_does_not_recompute_the_fixed_section() {
    let mut config = fixture(FixtureKind::IndependentJoint);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    match &mut config.interfaces[0].kind {
        InterfaceKind::IndependentWithJoint { width_m, .. } => *width_m = 0.2,
        _ => panic!("fixture interface should be an independent joint"),
    }

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("joint width edit should rebuild section B and interfaces");
    assert_section_sets(&diagnostics, &["B"], &["A"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 1);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert!(diagnostics.interfaces_regenerated > 0);
    assert_matches_cold(&config, &warm);
}

#[test]
fn connection_mode_change_reuses_raw_sections_but_rebuilds_the_merged_graph() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let (before, _) = cached_build(&config, &mut cache);
    config.interfaces[0].kind = InterfaceKind::GeometryOnlyContact;

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("geometry-only interface edit should rebuild assembly");
    assert_section_sets(&diagnostics, &[], &["A", "B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 0);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert!(diagnostics.interfaces_regenerated > 0);
    assert!(diagnostics.full_graph_postprocess);
    assert_matches_cold(&config, &warm);
    assert_ne!(
        serde_json::to_vec(&before).expect("before serializes"),
        serde_json::to_vec(&warm).expect("after serializes"),
        "interface-mode changes must affect final output even when raw section parts hit"
    );
}

#[test]
fn fresh_cache_after_config_reopen_is_cold_and_binding_valid() {
    let config = fixture(FixtureKind::SteppedConnected);
    let encoded_config = serde_json::to_vec(&config).expect("config serializes");
    let reopened: structural_configurator::BuildingConfig =
        serde_json::from_slice(&encoded_config).expect("config reopens");
    let cold = cold_output(&reopened);
    let mut fresh_cache = GeneratorCache::default();
    assert_eq!(fresh_cache.stats().total_hits, 0);
    assert_eq!(fresh_cache.stats().entries, 0);

    let raw_before = raw_call_count(&fresh_cache);
    let (generated, diagnostics) = generate_building_with_cache(&reopened, &mut fresh_cache)
        .expect("fresh cache build should succeed");
    assert_json_equal(&generated, &cold);
    assert_section_sets(&diagnostics, &["A", "B"], &[]);
    assert_eq!(diagnostics.raw_section_generation_calls, 2);
    assert_raw_call_delta(
        &fresh_cache,
        raw_before,
        diagnostics.raw_section_generation_calls,
    );
    validate_generated_binding(&reopened, &generated)
        .expect("fresh cached output binds to reopened input independently");
}

#[test]
fn cache_failure_does_not_publish_partial_raw_updates_or_stats() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);
    let before_failure = cache.stats();

    let mut invalid = config.clone();
    set_section_eave(&mut invalid, "A", 8.0, RoofHeightUpdate::PreserveRise)
        .expect("height change itself is valid");
    invalid.foundation_assignments.push(FoundationAssignment {
        section_id: SemanticId::new("A").expect("valid section ID"),
        support_node_id: SemanticId::new("not-a-generated-support").expect("valid node ID"),
        foundation_id: SemanticId::new("foundation-fixture").expect("valid foundation ID"),
    });
    assert!(matches!(
        generate_building_with_cache(&invalid, &mut cache),
        Err(ConfiguratorError::MissingReference { .. })
    ));
    let after_failure = cache.stats();
    assert_stats_equal(&before_failure, &after_failure);

    // If the failed call had published its recomputed A fragment, this valid
    // retry would be a hit instead of one raw A generation.
    set_section_eave(&mut config, "A", 8.0, RoofHeightUpdate::PreserveRise)
        .expect("height change itself is valid");
    let raw_before_retry = raw_call_count(&cache);
    let (warm_retry, diagnostics) =
        generate_building_with_cache(&config, &mut cache).expect("valid retry should succeed");
    assert_section_sets(&diagnostics, &["A"], &["B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 1);
    assert_raw_call_delta(
        &cache,
        raw_before_retry,
        diagnostics.raw_section_generation_calls,
    );
    assert_matches_cold(&config, &warm_retry);
}

#[test]
fn bounded_cache_evicts_deterministically_and_keeps_cold_equivalence() {
    let config = fixture(FixtureKind::SteppedConnected);
    let cold = cold_output(&config);
    let limits = GeneratorCacheLimits {
        max_sections: 1,
        max_entities: 100_000,
        max_payload_bytes: 16_000_000,
    };
    let mut first_cache = GeneratorCache::new(GeneratorCacheLimits {
        max_sections: limits.max_sections,
        max_entities: limits.max_entities,
        max_payload_bytes: limits.max_payload_bytes,
    })
    .expect("valid bounded cache limits");
    let mut second_cache = GeneratorCache::new(limits).expect("valid bounded cache limits");

    let first_raw_before = raw_call_count(&first_cache);
    let (first, first_diag) = generate_building_with_cache(&config, &mut first_cache)
        .expect("first bounded build should succeed");
    assert_json_equal(&first, &cold);
    assert_section_sets(&first_diag, &["A", "B"], &[]);
    assert_raw_call_delta(
        &first_cache,
        first_raw_before,
        first_diag.raw_section_generation_calls,
    );
    assert!(first_diag.cache_evictions > 0);
    assert!(first_cache.stats().entries <= 1);
    assert!(first_cache.stats().entities <= 100_000);
    assert!(first_cache.stats().payload_bytes <= 16_000_000);
    assert_eq!(
        first_cache.stats().total_evictions,
        first_diag.cache_evictions as u64
    );

    let second_raw_before = raw_call_count(&second_cache);
    let (second, second_diag) = generate_building_with_cache(&config, &mut second_cache)
        .expect("second bounded build should succeed");
    assert_json_equal(&second, &cold);
    assert_section_sets(&second_diag, &["A", "B"], &[]);
    assert_raw_call_delta(
        &second_cache,
        second_raw_before,
        second_diag.raw_section_generation_calls,
    );
    assert_eq!(first_diag.cache_evictions, second_diag.cache_evictions);
    assert_eq!(first_cache.stats().entries, second_cache.stats().entries);
    assert_eq!(first_cache.stats().entities, second_cache.stats().entities);
    assert_eq!(
        first_cache.stats().payload_bytes,
        second_cache.stats().payload_bytes
    );
    assert_eq!(
        first_cache.stats().total_evictions,
        second_cache.stats().total_evictions
    );
}

#[test]
fn orientation_change_invalidates_only_its_host_section() {
    let mut config = fixture(FixtureKind::SteppedConnected);
    let mut cache = GeneratorCache::default();
    let _ = cached_build(&config, &mut cache);

    config.sections[0].members.rafter.local_y_reference = [0.0, 2.0, 0.0];

    let raw_before = raw_call_count(&cache);
    let (warm, diagnostics) = generate_building_with_cache(&config, &mut cache)
        .expect("orientation edit should rebuild only its host section");
    assert_section_sets(&diagnostics, &["A"], &["B"]);
    assert_eq!(diagnostics.raw_section_generation_calls, 1);
    assert_raw_call_delta(&cache, raw_before, diagnostics.raw_section_generation_calls);
    assert_matches_cold(&config, &warm);
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual:.12}, expected {expected:.12} ± {tolerance}"
    );
}
