//! Shared, explicitly synthetic inputs for CLI and the native workspace.
//! No solver, archive, site approval or normative coefficient is created here.
use crate::*;
use std::collections::BTreeMap;

pub const GEO_FOUNDATION_DEMO_SOLE_Z_M: f64 = 99.5;
pub const GEO_FOUNDATION_DEMO_THICKNESS_M: f64 = 0.3;

/// All coordinates use the synthetic GEO datum. The terrain varies from 100
/// to 101.6 m; this is an embedded raft concept, with excavation design excluded.
/// The frame starts at the raft top, not at the unrelated legacy zero datum.
pub fn geo_foundation_demo_commands() -> Result<Vec<Command>, CoreError> {
    let mut state = geo_core::synthetic_demo_state()
        .map_err(|e| CoreError::InvalidDocument(format!("synthetic GEO inputs: {e}")))?;
    state.head_revision = None;
    if !state.snapshots.is_empty() {
        return Err(CoreError::InvalidDocument(
            "a demo input cannot contain analysis archives".into(),
        ));
    }
    let model_id = state
        .records
        .models
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| CoreError::InvalidDocument("synthetic GEO model is absent".into()))?;
    let scenario_id = state
        .records
        .scenarios
        .values()
        .find(|s| {
            s.model_id == model_id && matches!(s.water, geo_core::WaterScenario::ExplicitDry { .. })
        })
        .map(|s| s.id.clone())
        .ok_or_else(|| CoreError::InvalidDocument("an explicit GEO scenario is required".into()))?;
    let bedding_source = geo_core::SourceId::new("SRC-SYNTHETIC-BEDDING")
        .map_err(|e| CoreError::InvalidDocument(e.to_string()))?;
    let bedding_note = "SYNTHETIC_ASSUMPTION: independently chosen uniform k=10000 kN/m3 for a 10x12 m rigid-raft research model; no calibration, no universal E-to-k conversion, no site verification.";
    let record = geo_core::SourceRecord {
        id: bedding_source.clone(), title: "Independent synthetic Winkler bedding assumption".into(),
        origin: geo_core::SourceOrigin::Synthetic,
        reference: "crates/core/src/geo_foundation_demo.rs; explicit synthetic engineering-case input, not soil-test data".into(),
        content_sha256: None, observed_at: None, revision: 1,
        source_page_or_record: Some("uniform bedding: 10000 kN/m3; applicability limited to the declared prototype".into()),
        source_date: None, source_revision: Some("synthetic-bedding-v1".into()),
    };
    if state
        .records
        .sources
        .insert(bedding_source.clone(), record)
        .is_some()
    {
        return Err(CoreError::InvalidDocument(
            "synthetic bedding source identity already exists".into(),
        ));
    }
    state
        .records
        .scenarios
        .get_mut(&scenario_id)
        .ok_or_else(|| CoreError::InvalidDocument("synthetic scenario vanished".into()))?
        .source_ids
        .push(bedding_source.clone());
    let selection = geo_core::GeoSnapshotSelection {
        model_id,
        scenario_id: Some(scenario_id),
    };
    let layout: foundation_core::FoundationLayout = serde_json::from_value(serde_json::json!({
        "id": "synthetic-layout-1", "elements": [{
            "kind": "RAFT", "id": "1", "center_m": [5.0, 6.0, GEO_FOUNDATION_DEMO_SOLE_Z_M],
            "length_x_m": 10.0, "width_y_m": 12.0, "thickness_m": GEO_FOUNDATION_DEMO_THICKNESS_M
        }]
    }))?;
    let mut ground_links = BTreeMap::new();
    ground_links.insert("1".into(), selection);
    let mut bedding_assumptions = BTreeMap::new();
    bedding_assumptions.insert(
        "1".into(),
        foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform {
            k_kn_m3: 10000.0,
            assumption_note: bedding_note.into(),
            source_ids: vec![bedding_source],
        },
    );
    let (mut building, mut groups) =
        configured_fixture_input(structural_configurator::FixtureKind::SteppedConnected)?;
    let frame_base_z = GEO_FOUNDATION_DEMO_SOLE_Z_M + GEO_FOUNDATION_DEMO_THICKNESS_M;
    for section in &mut building.sections {
        section.roof.base_abs_m += frame_base_z;
        section.roof.eave_abs_m += frame_base_z;
        section.roof.ridge_abs_m += frame_base_z;
    }
    for group in &mut groups {
        group.reference_point_m[2] = GEO_FOUNDATION_DEMO_SOLE_Z_M;
    }
    Ok(vec![
        Command::GeoFoundation(GeoFoundationCommand::ReplaceGeoState { state }),
        Command::GeoFoundation(GeoFoundationCommand::ReplaceFoundationLayout {
            input: GeoFoundationLayoutInput {
                layout,
                ground_links,
                bedding_assumptions,
            },
        }),
        Command::StructuralConfigurator(
            StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
                building,
                foundation_groups: groups,
            },
        ),
    ])
}
