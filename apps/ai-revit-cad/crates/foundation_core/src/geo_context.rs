//! Source-bound Geo context for geometry-only foundation inputs.
//!
//! The Geo document remains the owner of soils, IGEs, boreholes, groundwater,
//! models, scenarios and source records. This adapter retains its immutable
//! snapshot identity and point-query results. It does not synthesize a second
//! soil registry, convert Geo elastic modulus into bedding k, or call a solver.

use crate::foundation_geometry::{
    derive_layout_geometry, FoundationGeometrySamplePoint, FoundationLayout,
    FoundationLayoutGeometry,
};
use crate::{FoundationError, FoundationId};
use geo_core::{
    query_ground, validate_snapshot_binding, GeoError, GeoQuery, GeoQueryResult,
    GeoSnapshotId, GeoSourceRevision, GroundModelId, GroundScenarioId, IgeId, QueryStatus,
    ResolvedGeoSnapshot, SourceId, WaterScenario,
};
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

const MAX_MANUAL_IGE_VALUES: usize = 4096;
const FOUNDATION_BASE_OUTSIDE_VERTICAL_COVERAGE: &str =
    "FOUNDATION_BASE_OUTSIDE_VERTICAL_COVERAGE";

/// Explicit, manually declared synthetic bedding stiffness assumptions.
/// These values are independent of Geo elastic modulus and are not derived
/// from Geo soil records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum FoundationBeddingAssumption {
    ManualSyntheticUniform {
        k_kn_m3: f64,
        assumption_note: String,
        #[serde(default)]
        source_ids: Vec<SourceId>,
    },
    ManualSyntheticPerIge {
        #[serde(deserialize_with = "deserialize_unique_ige_values")]
        by_ige: BTreeMap<IgeId, ManualIgeBeddingValue>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualIgeBeddingValue {
    pub k_kn_m3: f64,
    pub assumption_note: String,
    #[serde(default)]
    pub source_ids: Vec<SourceId>,
}

/// Hash-pinned relationship between one immutable Geo selection and one
/// foundation layout. This is an identity binding, not a freshness or
/// engineering-validity attestation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeoBinding {
    pub geo_snapshot_id: GeoSnapshotId,
    pub geo_source_revision: GeoSourceRevision,
    pub ground_model_id: GroundModelId,
    pub ground_scenario_id: GroundScenarioId,
    pub selection_fingerprint: String,
    pub content_fingerprint: String,
    pub foundation_layout_id: String,
    pub foundation_layout_fingerprint: String,
    /// Source IDs present in the validated snapshot closure.
    pub source_ids: Vec<SourceId>,
    /// IGE IDs present in the validated snapshot closure.
    pub snapshot_ige_ids: Vec<IgeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeoQuerySample {
    pub placement_id: FoundationId,
    pub tier_id: Option<String>,
    pub xy_m: [f64; 2],
    pub foundation_base_elevation_m: f64,
    pub result: GeoQueryResult,
}

/// A resolved, immutable Geo source and point-sample projection for a layout.
/// Point samples do not assert coverage beneath a full placement or establish
/// design values or approvals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeoContext {
    pub binding: FoundationGeoBinding,
    pub layout_geometry: FoundationLayoutGeometry,
    pub samples: Vec<FoundationGeoQuerySample>,
    pub water_scenario: WaterScenario,
    pub sampled_ige_ids: Vec<IgeId>,
    pub readiness: FoundationGeoReadiness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FoundationGeoReadinessState {
    BoundInputsOnly,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeoSampleIssue {
    pub placement_id: FoundationId,
    pub tier_id: Option<String>,
    /// An adapter-derived `OutsideCoverage` can flag vertical bounds while
    /// the preserved GeoQueryResult itself remains `Known`.
    pub status: QueryStatus,
    pub reasons: Vec<String>,
}

/// Gate statuses deliberately avoid engineering-pass wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FoundationMethodStatus {
    Unverified,
    NotImplemented,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateFoundationBReadiness {
    pub ultimate_bearing: FoundationMethodStatus,
    pub settlement: FoundationMethodStatus,
    pub sliding: FoundationMethodStatus,
    pub overturning: FoundationMethodStatus,
    pub groundwater: FoundationMethodStatus,
    pub layered_interaction: FoundationMethodStatus,
    pub normative_uls: FoundationMethodStatus,
    pub normative_sls: FoundationMethodStatus,
    pub reinforcement: FoundationMethodStatus,
}

impl Default for GateFoundationBReadiness {
    fn default() -> Self {
        Self {
            ultimate_bearing: FoundationMethodStatus::Unverified,
            settlement: FoundationMethodStatus::Unverified,
            sliding: FoundationMethodStatus::NotImplemented,
            overturning: FoundationMethodStatus::NotImplemented,
            groundwater: FoundationMethodStatus::NotImplemented,
            layered_interaction: FoundationMethodStatus::NotImplemented,
            normative_uls: FoundationMethodStatus::NotImplemented,
            normative_sls: FoundationMethodStatus::NotImplemented,
            reinforcement: FoundationMethodStatus::NotImplemented,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeoReadiness {
    pub state: FoundationGeoReadinessState,
    pub unresolved_samples: Vec<FoundationGeoSampleIssue>,
    pub gate_foundation_b: GateFoundationBReadiness,
}

/// Validation status for an explicit synthetic bedding assumption. A complete
/// record here only means the assumption is explicit and its references appear
/// in captured context metadata; it is not design acceptance or certification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationBeddingReadiness {
    pub state: FoundationGeoReadinessState,
    pub unresolved_samples: Vec<FoundationGeoSampleIssue>,
    pub missing_ige_values: Vec<IgeId>,
    pub source_ids: Vec<SourceId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum FoundationGeoError {
    InvalidSnapshot { reason: String },
    InvalidLayout { field: String, reason: String },
    InvalidBedding { field: String, reason: String },
    GeoQuery { placement_id: FoundationId, error: GeoError },
}

impl std::fmt::Display for FoundationGeoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for FoundationGeoError {}

/// Resolve geometry sample points against a validated immutable Geo snapshot.
/// Readiness covers source binding, point-query results, and the foundation
/// base elevation's inclusion in each returned finite vertical column. It does
/// not indicate bearing, settlement, or design adequacy.
pub fn resolve_geo_context(
    resolved: &ResolvedGeoSnapshot,
    layout: &FoundationLayout,
) -> Result<FoundationGeoContext, FoundationGeoError> {
    validate_snapshot(resolved)?;
    let layout_geometry = derive_layout_geometry(layout).map_err(|error| match error {
        FoundationError::InvalidInput { field, reason } => {
            FoundationGeoError::InvalidLayout { field, reason }
        }
        other => FoundationGeoError::InvalidLayout {
            field: "layout".into(),
            reason: other.to_string(),
        },
    })?;

    let snapshot_ige_ids: Vec<_> = resolved.snapshot.records.iges.keys().cloned().collect();
    let mut samples = Vec::new();
    let mut unresolved_samples = Vec::new();
    let mut observed_iges = BTreeSet::new();

    for geometry in &layout_geometry.elements {
        for point in &geometry.sample_points {
            let sample = resolve_sample(resolved, point, geometry.base_elevation_m)?;
            match &sample.result.column {
                Some(column) => {
                    let mut reasons = sample.result.reasons.clone();
                    if column.layers.is_empty() {
                        reasons.push("NO_IGE_LAYERS_AT_SAMPLE".into());
                    }
                    for layer in &column.layers {
                        if !resolved.snapshot.records.iges.contains_key(&layer.ige_id) {
                            return Err(FoundationGeoError::InvalidSnapshot {
                                reason: format!(
                                    "Geo query returned IGE {} absent from its bound snapshot",
                                    layer.ige_id
                                ),
                            });
                        }
                        observed_iges.insert(layer.ige_id.clone());
                    }
                    if sample.result.status != QueryStatus::Known {
                        if reasons.is_empty() {
                            reasons.push(format!("GEO_QUERY_{:?}", sample.result.status));
                        }
                        unresolved_samples.push(FoundationGeoSampleIssue {
                            placement_id: sample.placement_id.clone(),
                            tier_id: sample.tier_id.clone(),
                            status: sample.result.status,
                            reasons,
                        });
                    } else if column.layers.is_empty() {
                        unresolved_samples.push(FoundationGeoSampleIssue {
                            placement_id: sample.placement_id.clone(),
                            tier_id: sample.tier_id.clone(),
                            status: sample.result.status,
                            reasons,
                        });
                    }
                    if let Some(last_layer) = column.layers.last() {
                        let foundation_base_outside = sample.foundation_base_elevation_m
                            > column.terrain_abs_m
                            || sample.foundation_base_elevation_m <= last_layer.bottom_abs_m;
                        if foundation_base_outside {
                            unresolved_samples.push(FoundationGeoSampleIssue {
                                placement_id: sample.placement_id.clone(),
                                tier_id: sample.tier_id.clone(),
                                status: QueryStatus::OutsideCoverage,
                                reasons: vec![FOUNDATION_BASE_OUTSIDE_VERTICAL_COVERAGE.into()],
                            });
                        }
                    }
                }
                None => {
                    let reasons = if sample.result.reasons.is_empty() {
                        vec!["QUERY_DID_NOT_RETURN_A_GROUND_COLUMN".into()]
                    } else {
                        sample.result.reasons.clone()
                    };
                    unresolved_samples.push(FoundationGeoSampleIssue {
                        placement_id: sample.placement_id.clone(),
                        tier_id: sample.tier_id.clone(),
                        status: sample.result.status,
                        reasons,
                    });
                }
            }
            samples.push(sample);
        }
    }

    let source_ids: Vec<_> = resolved
        .snapshot
        .records
        .sources
        .keys()
        .cloned()
        .collect();
    let binding = FoundationGeoBinding {
        geo_snapshot_id: resolved.snapshot.id.clone(),
        geo_source_revision: resolved.snapshot.source_revision.clone(),
        ground_model_id: resolved.scenario.model_id.clone(),
        ground_scenario_id: resolved.scenario.id.clone(),
        selection_fingerprint: resolved.snapshot.selection_fingerprint.clone(),
        content_fingerprint: resolved.snapshot.content_fingerprint.clone(),
        foundation_layout_id: layout.id.clone(),
        foundation_layout_fingerprint: fingerprint_layout(layout)?,
        source_ids,
        snapshot_ige_ids,
    };
    let sampled_ige_ids = observed_iges.into_iter().collect();
    let state = if unresolved_samples.is_empty() {
        FoundationGeoReadinessState::BoundInputsOnly
    } else {
        FoundationGeoReadinessState::Incomplete
    };

    Ok(FoundationGeoContext {
        binding,
        layout_geometry,
        samples,
        water_scenario: resolved.scenario.water.clone(),
        sampled_ige_ids,
        readiness: FoundationGeoReadiness {
            state,
            unresolved_samples,
            gate_foundation_b: GateFoundationBReadiness::default(),
        },
    })
}

/// Check manually supplied synthetic k values against IDs captured in the Geo
/// context. This does not re-open or revalidate the snapshot. No soil modulus
/// or other Geo property is converted to k.
pub fn validate_bedding_assumption(
    context: &FoundationGeoContext,
    assumption: &FoundationBeddingAssumption,
) -> Result<FoundationBeddingReadiness, FoundationGeoError> {
    let captured_sources: BTreeSet<_> = context.binding.source_ids.iter().cloned().collect();
    let captured_iges: BTreeSet<_> = context.binding.snapshot_ige_ids.iter().cloned().collect();
    let sampled_iges: BTreeSet<_> = context.sampled_ige_ids.iter().cloned().collect();
    let mut assumption_sources = BTreeSet::new();
    let mut configured_iges = BTreeSet::new();

    match assumption {
        FoundationBeddingAssumption::ManualSyntheticUniform {
            k_kn_m3,
            assumption_note,
            source_ids,
        } => {
            validate_k(*k_kn_m3, "bedding/k_kn_m3")?;
            validate_note(assumption_note, "bedding/assumption_note")?;
            validate_source_ids(
                source_ids,
                &captured_sources,
                &mut assumption_sources,
                "bedding/source_ids",
            )?;
            configured_iges.extend(sampled_iges.iter().cloned());
        }
        FoundationBeddingAssumption::ManualSyntheticPerIge { by_ige } => {
            for (ige_id, value) in by_ige {
                if !captured_iges.contains(ige_id) {
                    return Err(FoundationGeoError::InvalidBedding {
                        field: format!("bedding/by_ige/{ige_id}"),
                        reason: "IGE ID is not present in the bound Geo snapshot".into(),
                    });
                }
                validate_k(value.k_kn_m3, &format!("bedding/by_ige/{ige_id}/k_kn_m3"))?;
                validate_note(
                    &value.assumption_note,
                    &format!("bedding/by_ige/{ige_id}/assumption_note"),
                )?;
                validate_source_ids(
                    &value.source_ids,
                    &captured_sources,
                    &mut assumption_sources,
                    &format!("bedding/by_ige/{ige_id}/source_ids"),
                )?;
                configured_iges.insert(ige_id.clone());
            }
        }
    }

    let missing_ige_values: Vec<_> = sampled_iges.difference(&configured_iges).cloned().collect();
    let state = if missing_ige_values.is_empty() && context.readiness.unresolved_samples.is_empty() {
        FoundationGeoReadinessState::BoundInputsOnly
    } else {
        FoundationGeoReadinessState::Incomplete
    };

    Ok(FoundationBeddingReadiness {
        state,
        unresolved_samples: context.readiness.unresolved_samples.clone(),
        missing_ige_values,
        source_ids: assumption_sources.into_iter().collect(),
    })
}

fn resolve_sample(
    resolved: &ResolvedGeoSnapshot,
    point: &FoundationGeometrySamplePoint,
    foundation_base_elevation_m: f64,
) -> Result<FoundationGeoQuerySample, FoundationGeoError> {
    let result = query_ground(resolved, &GeoQuery::ColumnAt { xy_m: point.xy_m }).map_err(
        |error| FoundationGeoError::GeoQuery {
            placement_id: point.placement_id.clone(),
            error,
        },
    )?;
    Ok(FoundationGeoQuerySample {
        placement_id: point.placement_id.clone(),
        tier_id: point.tier_id.clone(),
        xy_m: point.xy_m,
        foundation_base_elevation_m,
        result,
    })
}

fn validate_snapshot(resolved: &ResolvedGeoSnapshot) -> Result<(), FoundationGeoError> {
    validate_snapshot_binding(&resolved.snapshot).map_err(|error| {
        FoundationGeoError::InvalidSnapshot {
            reason: error.to_string(),
        }
    })?;
    if resolved.snapshot.selection.model_id != resolved.scenario.model_id
        || resolved.snapshot.selection.scenario_id.as_ref() != Some(&resolved.scenario.id)
        || resolved
            .snapshot
            .records
            .scenarios
            .get(&resolved.scenario.id)
            != Some(&resolved.scenario)
        || !resolved
            .snapshot
            .records
            .models
            .contains_key(&resolved.scenario.model_id)
    {
        return Err(FoundationGeoError::InvalidSnapshot {
            reason: "resolved scenario is not the selection bound into this snapshot".into(),
        });
    }
    Ok(())
}

fn validate_k(k_kn_m3: f64, field: &str) -> Result<(), FoundationGeoError> {
    if !k_kn_m3.is_finite() || k_kn_m3 <= 0.0 {
        return Err(FoundationGeoError::InvalidBedding {
            field: field.into(),
            reason: "manual synthetic k must be finite and positive".into(),
        });
    }
    Ok(())
}

fn validate_note(note: &str, field: &str) -> Result<(), FoundationGeoError> {
    if note.trim().is_empty() || note.len() > 4096 {
        return Err(FoundationGeoError::InvalidBedding {
            field: field.into(),
            reason: "a non-empty bounded manual-assumption note is required".into(),
        });
    }
    Ok(())
}

fn validate_source_ids(
    ids: &[SourceId],
    valid_sources: &BTreeSet<SourceId>,
    accumulated: &mut BTreeSet<SourceId>,
    field: &str,
) -> Result<(), FoundationGeoError> {
    if ids.is_empty() {
        return Err(FoundationGeoError::InvalidBedding {
            field: field.into(),
            reason: "at least one explicit source ID is required".into(),
        });
    }
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(FoundationGeoError::InvalidBedding {
                field: field.into(),
                reason: format!("duplicate source ID {id}"),
            });
        }
        if !valid_sources.contains(id) {
            return Err(FoundationGeoError::InvalidBedding {
                field: field.into(),
                reason: format!("source ID {id} is not present in the bound Geo snapshot"),
            });
        }
        accumulated.insert(id.clone());
    }
    Ok(())
}

fn deserialize_unique_ige_values<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<IgeId, ManualIgeBeddingValue>, D::Error>
where
    D: Deserializer<'de>,
{
    struct UniqueIgeValues;

    impl<'de> Visitor<'de> for UniqueIgeValues {
        type Value = BTreeMap<IgeId, ManualIgeBeddingValue>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "a map with distinct IGE IDs and at most {MAX_MANUAL_IGE_VALUES} entries"
            )
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some(ige_id) = access.next_key::<IgeId>()? {
                if values.contains_key(&ige_id) {
                    return Err(A::Error::custom(format!(
                        "duplicate IGE bedding key: {ige_id}"
                    )));
                }
                if values.len() >= MAX_MANUAL_IGE_VALUES {
                    return Err(A::Error::custom(format!(
                        "IGE bedding map exceeds {MAX_MANUAL_IGE_VALUES} entries"
                    )));
                }
                let value = access.next_value::<ManualIgeBeddingValue>()?;
                values.insert(ige_id, value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueIgeValues)
}

fn fingerprint_layout(layout: &FoundationLayout) -> Result<String, FoundationGeoError> {
    let bytes = serde_json::to_vec(layout).map_err(|error| FoundationGeoError::InvalidLayout {
        field: "layout".into(),
        reason: format!("layout serialization failed: {error}"),
    })?;
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(&mut output, "{byte:02x}").map_err(|error| FoundationGeoError::InvalidLayout {
            field: "layout".into(),
            reason: format!("layout fingerprint formatting failed: {error}"),
        })?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FoundationPlacement, FoundationTier};
    use geo_core::{
        make_geo_snapshot, resolve_geo_scenario, synthetic_demo_state, GeoSnapshotSelection,
        GeoSoilId, GeoState, MissingBoreholeCrsPolicy, Parameter, SYNTHETIC_DEMO_MODEL_ID,
        SYNTHETIC_DEMO_SCENARIO_ID, UnknownZone,
    };

    fn demo_resolved(
        water: Option<WaterScenario>,
        fill_modulus_kpa: Option<f64>,
        snapshot_name: &str,
    ) -> ResolvedGeoSnapshot {
        let mut state = synthetic_demo_state().expect("synthetic Geo fixture should be valid");
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");
        let scenario_id =
            GroundScenarioId::new(SYNTHETIC_DEMO_SCENARIO_ID).expect("scenario ID");

        if let Some(water) = water {
            state
                .records
                .scenarios
                .get_mut(&scenario_id)
                .expect("demo scenario exists")
                .water = water;
        }
        if let Some(modulus) = fill_modulus_kpa {
            let fill_id = GeoSoilId::new("SOIL-FILL").expect("fill soil ID");
            let fill = state.records.soils.get_mut(&fill_id).expect("fill soil exists");
            match &mut fill.parameters.elastic_modulus_kpa {
                Parameter::Known { value, .. } => *value = modulus,
                _ => panic!("synthetic fill modulus should be known"),
            }
        }

        let selection = GeoSnapshotSelection {
            model_id,
            scenario_id: Some(scenario_id.clone()),
        };
        let source_revision = state
            .head_revision
            .clone()
            .expect("synthetic fixture has a source revision");
        let snapshot = make_geo_snapshot(
            &state,
            GeoSnapshotId::new(snapshot_name).expect("snapshot ID"),
            &selection,
            source_revision,
        )
        .expect("selected synthetic snapshot should validate");
        let scenario = snapshot
            .records
            .scenarios
            .get(&scenario_id)
            .expect("scenario is included in the snapshot")
            .clone();
        resolve_geo_scenario(&snapshot, &scenario).expect("scenario is bound to snapshot")
    }

    fn context_from_state(
        state: &GeoState,
        layout: &FoundationLayout,
        snapshot_name: &str,
    ) -> Result<FoundationGeoContext, String> {
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).map_err(|e| e.to_string())?;
        let scenario_id =
            GroundScenarioId::new(SYNTHETIC_DEMO_SCENARIO_ID).map_err(|e| e.to_string())?;
        let selection = GeoSnapshotSelection {
            model_id,
            scenario_id: Some(scenario_id.clone()),
        };
        let source_revision = state
            .head_revision
            .clone()
            .ok_or_else(|| "synthetic Geo state has no head revision".to_string())?;
        let snapshot = make_geo_snapshot(
            state,
            GeoSnapshotId::new(snapshot_name).map_err(|e| e.to_string())?,
            &selection,
            source_revision,
        )
        .map_err(|error| error.to_string())?;
        let scenario = snapshot
            .records
            .scenarios
            .get(&scenario_id)
            .ok_or_else(|| "selected scenario is missing from snapshot".to_string())?;
        let resolved = resolve_geo_scenario(&snapshot, scenario)
            .map_err(|error| error.to_string())?;
        resolve_geo_context(&resolved, layout).map_err(|error| error.to_string())
    }

    fn assert_no_numeric_admission(result: Result<FoundationGeoContext, String>) {
        if let Ok(context) = result {
            assert_ne!(context.readiness.state, FoundationGeoReadinessState::BoundInputsOnly);
            assert!(!context.readiness.unresolved_samples.is_empty());
            assert!(context
                .samples
                .iter()
                .any(|sample| sample.result.status != QueryStatus::Known));
        }
    }

    fn layout_at(id: &str, xy: [f64; 2]) -> FoundationLayout {
        layout_at_base(id, xy, 90.0)
    }

    fn layout_at_base(id: &str, xy: [f64; 2], base_z_m: f64) -> FoundationLayout {
        FoundationLayout {
            id: id.into(),
            elements: vec![FoundationPlacement::Raft {
                id: FoundationId::new("1").expect("foundation ID"),
                center_m: [xy[0], xy[1], base_z_m],
                length_x_m: 2.0,
                width_y_m: 2.0,
                thickness_m: 0.3,
            }],
        }
    }

    fn source_id(context: &FoundationGeoContext) -> SourceId {
        context
            .binding
            .source_ids
            .first()
            .expect("synthetic snapshot has a source record")
            .clone()
    }

    fn manual_uniform(k_kn_m3: f64, source_ids: Vec<SourceId>) -> FoundationBeddingAssumption {
        FoundationBeddingAssumption::ManualSyntheticUniform {
            k_kn_m3,
            assumption_note: "Manually stated synthetic test input".into(),
            source_ids,
        }
    }

    #[test]
    fn binds_five_footprint_queries_to_the_selected_geo_snapshot() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-CONTEXT");
        let layout = layout_at("layout-geo-binding", [5.0, 6.0]);
        let context = resolve_geo_context(&resolved, &layout).expect("valid source binding");

        assert_eq!(context.binding.geo_snapshot_id, resolved.snapshot.id);
        assert_eq!(
            context.binding.content_fingerprint,
            resolved.snapshot.content_fingerprint
        );
        assert_eq!(context.binding.foundation_layout_id, layout.id);
        assert_eq!(context.samples.len(), 5);
        assert!(context
            .samples
            .iter()
            .all(|sample| sample.result.status == QueryStatus::Known));
        assert_eq!(context.readiness.state, FoundationGeoReadinessState::BoundInputsOnly);
        assert_eq!(
            context.readiness.gate_foundation_b.ultimate_bearing,
            FoundationMethodStatus::Unverified
        );
        assert_eq!(
            context.readiness.gate_foundation_b.groundwater,
            FoundationMethodStatus::NotImplemented
        );
    }

    #[test]
    fn stepped_geo_samples_use_only_the_lowest_tier_and_keep_its_base_elevation() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-STEPPED");
        let layout = FoundationLayout {
            id: "layout-stepped-geo".into(),
            elements: vec![FoundationPlacement::Stepped {
                id: FoundationId::new("8").expect("foundation ID"),
                tiers: vec![
                    FoundationTier {
                        id: "bottom-tier".into(),
                        center_m: [5.0, 6.0, 90.0],
                        length_x_m: 4.0,
                        width_y_m: 4.0,
                        thickness_m: 0.5,
                    },
                    FoundationTier {
                        id: "upper-tier".into(),
                        center_m: [5.0, 6.0, 90.5],
                        length_x_m: 2.0,
                        width_y_m: 2.0,
                        thickness_m: 0.4,
                    },
                ],
            }],
        };

        let context = resolve_geo_context(&resolved, &layout).expect("valid stepped Geo samples");
        assert_eq!(context.samples.len(), 5);
        assert!(context.samples.iter().all(|sample| {
            sample.tier_id.as_deref() == Some("bottom-tier")
                && sample.foundation_base_elevation_m == 90.0
        }));
        assert_eq!(context.samples[0].xy_m, [3.0, 4.0]);
        assert_eq!(context.samples[4].xy_m, [5.0, 6.0]);
    }

    #[test]
    fn rejects_tampered_snapshot_binding_and_unselected_scenario() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-BINDING-ERRORS");
        let layout = layout_at("layout-binding-errors", [5.0, 6.0]);

        let mut tampered_hash = resolved.clone();
        tampered_hash.snapshot.content_fingerprint = "0".repeat(64);
        assert!(matches!(
            resolve_geo_context(&tampered_hash, &layout),
            Err(FoundationGeoError::InvalidSnapshot { .. })
        ));

        let mut wrong_scenario = resolved;
        wrong_scenario.scenario.id =
            GroundScenarioId::new("OTHER-SCENARIO").expect("valid alternate ID");
        assert!(matches!(
            resolve_geo_context(&wrong_scenario, &layout),
            Err(FoundationGeoError::InvalidSnapshot { .. })
        ));
    }

    #[test]
    fn missing_model_crs_and_collar_datum_gaps_never_yield_bound_context() {
        let layout = layout_at("layout-frame-reference-gates", [5.0, 6.0]);
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");

        let mut missing_model_crs = synthetic_demo_state().expect("synthetic Geo state");
        let model = missing_model_crs
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists");
        assert_eq!(
            model.frame.missing_borehole_crs,
            MissingBoreholeCrsPolicy::RequireExplicit
        );
        model.frame.crs = None;
        assert_no_numeric_admission(context_from_state(
            &missing_model_crs,
            &layout,
            "SNAP-MISSING-MODEL-CRS",
        ));

        let mut missing_inherited_model_crs =
            synthetic_demo_state().expect("synthetic Geo state");
        missing_inherited_model_crs
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists")
            .frame
            .missing_borehole_crs = MissingBoreholeCrsPolicy::InheritModel;
        missing_inherited_model_crs
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists")
            .frame
            .crs = None;
        missing_inherited_model_crs
            .records
            .boreholes
            .values_mut()
            .next()
            .expect("demo has boreholes")
            .crs = None;
        assert_no_numeric_admission(context_from_state(
            &missing_inherited_model_crs,
            &layout,
            "SNAP-MISSING-INHERITED-MODEL-CRS",
        ));

        let mut missing_collar_datum = synthetic_demo_state().expect("synthetic Geo state");
        missing_collar_datum
            .records
            .boreholes
            .values_mut()
            .next()
            .expect("demo has boreholes")
            .collar_vertical_datum = None;
        assert_no_numeric_admission(context_from_state(
            &missing_collar_datum,
            &layout,
            "SNAP-MISSING-COLLAR-DATUM",
        ));

        let mut mismatched_collar_datum = synthetic_demo_state().expect("synthetic Geo state");
        mismatched_collar_datum
            .records
            .boreholes
            .values_mut()
            .next()
            .expect("demo has boreholes")
            .collar_vertical_datum = Some("UNRELATED-DATUM".into());
        assert_no_numeric_admission(context_from_state(
            &mismatched_collar_datum,
            &layout,
            "SNAP-MISMATCHED-COLLAR-DATUM",
        ));
    }

    #[test]
    fn mismatched_layer_contact_datum_never_yields_bound_context() {
        let mut state = synthetic_demo_state().expect("synthetic Geo state");
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");
        state
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists")
            .contacts
            .first_mut()
            .expect("demo model has layer contacts")
            .vertical_datum = Some("UNRELATED-DATUM".into());

        let result = context_from_state(
            &state,
            &layout_at("layout-contact-datum-gate", [5.0, 6.0]),
            "SNAP-MISMATCHED-CONTACT-DATUM",
        );
        assert_no_numeric_admission(result);
    }

    #[test]
    fn unknown_geo_column_status_propagates_to_incomplete_context() {
        let mut state = synthetic_demo_state().expect("synthetic Geo state");
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");
        let source_id = state
            .records
            .sources
            .keys()
            .next()
            .expect("demo has a source")
            .clone();
        state
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists")
            .unknown_zones
            .push(UnknownZone {
                polygon_xy_m: vec![
                    [4.8, 5.8],
                    [5.2, 5.8],
                    [5.2, 6.2],
                    [4.8, 6.2],
                ],
                reason: "test-only unknown patch".into(),
                source_ids: vec![source_id],
            });

        let context = context_from_state(
            &state,
            &layout_at("layout-unknown-column", [5.0, 6.0]),
            "SNAP-UNKNOWN-COLUMN",
        )
        .expect("unknown columns are query results, not snapshot errors");
        assert_eq!(context.readiness.state, FoundationGeoReadinessState::Incomplete);
        let center_sample = context
            .samples
            .iter()
            .find(|sample| sample.xy_m == [5.0, 6.0])
            .expect("center sample is included");
        assert_eq!(center_sample.result.status, QueryStatus::Unknown);
        assert!(center_sample.result.column.is_none());
        assert!(context.readiness.unresolved_samples.iter().any(|issue| {
            issue.status == QueryStatus::Unknown
                && issue.reasons.iter().any(|reason| reason.contains("unknown patch"))
        }));
    }

    #[test]
    fn require_explicit_crs_policy_holds_when_a_borehole_crs_is_missing() {
        let mut state = synthetic_demo_state().expect("synthetic Geo state");
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");
        assert_eq!(
            state
                .records
                .models
                .get(&model_id)
                .expect("demo model exists")
                .frame
                .missing_borehole_crs,
            MissingBoreholeCrsPolicy::RequireExplicit
        );
        state
            .records
            .boreholes
            .values_mut()
            .next()
            .expect("demo has boreholes")
            .crs = None;

        assert_no_numeric_admission(context_from_state(
            &state,
            &layout_at("layout-require-explicit-crs", [5.0, 6.0]),
            "SNAP-REQUIRE-EXPLICIT-CRS",
        ));
    }

    #[test]
    fn explicit_model_crs_inheritance_allows_a_missing_well_crs() {
        let mut state = synthetic_demo_state().expect("synthetic Geo state");
        let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID).expect("model ID");
        let model = state
            .records
            .models
            .get_mut(&model_id)
            .expect("demo model exists");
        assert!(model.frame.crs.is_some());
        model.frame.missing_borehole_crs = MissingBoreholeCrsPolicy::InheritModel;
        state
            .records
            .boreholes
            .values_mut()
            .next()
            .expect("demo has boreholes")
            .crs = None;

        let context = context_from_state(
            &state,
            &layout_at("layout-inherit-model-crs", [5.0, 6.0]),
            "SNAP-INHERIT-MODEL-CRS",
        )
        .expect("explicit inheritance uses the available model CRS");
        assert_eq!(context.readiness.state, FoundationGeoReadinessState::BoundInputsOnly);
        assert!(context
            .samples
            .iter()
            .all(|sample| sample.result.status == QueryStatus::Known));
    }

    #[test]
    fn outside_coverage_stays_incomplete_and_unknown_water_stays_unverified() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-OUTSIDE");
        let outside = resolve_geo_context(
            &resolved,
            &layout_at("layout-outside", [50.0, 50.0]),
        )
        .expect("outside-coverage is a typed query result");
        assert_eq!(outside.samples.len(), 5);
        assert_eq!(outside.readiness.state, FoundationGeoReadinessState::Incomplete);
        assert!(outside
            .samples
            .iter()
            .all(|sample| sample.result.status == QueryStatus::OutsideCoverage));

        let unknown_water = demo_resolved(
            Some(WaterScenario::Unknown),
            None,
            "SNAP-FOUNDATION-UNKNOWN-WATER",
        );
        let unknown_context = resolve_geo_context(
            &unknown_water,
            &layout_at("layout-unknown-water", [5.0, 6.0]),
        )
        .expect("unknown water is retained as an input state");
        assert_eq!(unknown_context.water_scenario, WaterScenario::Unknown);
        assert_eq!(
            unknown_context.readiness.gate_foundation_b.groundwater,
            FoundationMethodStatus::NotImplemented
        );
    }

    #[test]
    fn foundation_base_outside_vertical_coverage_is_held_without_altering_geo_columns() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-VERTICAL-COVERAGE");
        for (layout_id, base_z_m) in [("layout-above-terrain", 110.0), ("layout-below-bottom", 80.0)] {
            let context = resolve_geo_context(
                &resolved,
                &layout_at_base(layout_id, [5.0, 6.0], base_z_m),
            )
            .expect("Geo columns remain available for vertical coverage review");

            assert_eq!(context.readiness.state, FoundationGeoReadinessState::Incomplete);
            assert!(context.samples.iter().all(|sample| {
                sample.result.status == QueryStatus::Known && sample.result.column.is_some()
            }));
            assert_eq!(context.readiness.unresolved_samples.len(), 5);
            assert!(context.readiness.unresolved_samples.iter().all(|issue| {
                issue.status == QueryStatus::OutsideCoverage
                    && issue.reasons.iter().any(|reason| {
                        reason == FOUNDATION_BASE_OUTSIDE_VERTICAL_COVERAGE
                    })
            }));
        }

        let center_column = query_ground(
            &resolved,
            &GeoQuery::ColumnAt { xy_m: [5.0, 6.0] },
        )
        .expect("center Geo query should succeed")
        .column
        .expect("center Geo query should return a column");
        let survey_bottom = center_column
            .layers
            .last()
            .expect("synthetic model has a finite last layer")
            .bottom_abs_m;
        let at_survey_bottom = resolve_geo_context(
            &resolved,
            &layout_at_base("layout-at-survey-bottom", [5.0, 6.0], survey_bottom),
        )
        .expect("the boundary elevation preserves its Geo columns");
        assert!(at_survey_bottom
            .samples
            .iter()
            .all(|sample| sample.result.status == QueryStatus::Known));
        assert!(at_survey_bottom
            .readiness
            .unresolved_samples
            .iter()
            .any(|issue| issue.reasons.iter().any(|reason| {
                reason == FOUNDATION_BASE_OUTSIDE_VERTICAL_COVERAGE
            })));
    }

    #[test]
    fn explicit_dry_does_not_promote_foundation_groundwater_status() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-EXPLICIT-DRY");
        let context = resolve_geo_context(
            &resolved,
            &layout_at("layout-explicit-dry", [5.0, 6.0]),
        )
        .expect("synthetic demo has a valid explicit-dry scenario");

        assert!(matches!(context.water_scenario, WaterScenario::ExplicitDry { .. }));
        assert_eq!(
            context.readiness.gate_foundation_b.groundwater,
            FoundationMethodStatus::NotImplemented
        );
        assert_eq!(
            context.readiness.gate_foundation_b.normative_uls,
            FoundationMethodStatus::NotImplemented
        );
    }

    #[test]
    fn manual_k_is_separate_from_geo_modulus_and_per_ige_coverage_is_explicit() {
        let layout = layout_at("layout-manual-k", [5.0, 6.0]);
        let low_modulus = demo_resolved(None, Some(1000.0), "SNAP-FOUNDATION-LOW-E");
        let high_modulus = demo_resolved(None, Some(1_000_000.0), "SNAP-FOUNDATION-HIGH-E");
        let low_context = resolve_geo_context(&low_modulus, &layout).expect("low-E context");
        let high_context = resolve_geo_context(&high_modulus, &layout).expect("high-E context");

        let manual = manual_uniform(321.5, vec![source_id(&low_context)]);
        let low_readiness = validate_bedding_assumption(&low_context, &manual)
            .expect("explicit synthetic uniform k should be accepted as input");
        let high_readiness = validate_bedding_assumption(&high_context, &manual)
            .expect("changing Geo E does not change manual-k completeness");
        assert_eq!(low_readiness, high_readiness);
        assert_eq!(low_readiness.state, FoundationGeoReadinessState::BoundInputsOnly);
        assert!(low_readiness.missing_ige_values.is_empty());

        let source_id = source_id(&low_context);
        let by_ige = low_context
            .sampled_ige_ids
            .iter()
            .cloned()
            .map(|ige_id| {
                (
                    ige_id,
                    ManualIgeBeddingValue {
                        k_kn_m3: 321.5,
                        assumption_note: "Manually stated per-IGE synthetic k".into(),
                        source_ids: vec![source_id.clone()],
                    },
                )
            })
            .collect();
        let complete_by_ige = FoundationBeddingAssumption::ManualSyntheticPerIge { by_ige };
        let complete = validate_bedding_assumption(&low_context, &complete_by_ige)
            .expect("every sampled IGE has an explicit manual value");
        assert_eq!(complete.state, FoundationGeoReadinessState::BoundInputsOnly);
        assert!(complete.missing_ige_values.is_empty());

        let partial_by_ige = FoundationBeddingAssumption::ManualSyntheticPerIge {
            by_ige: low_context
                .sampled_ige_ids
                .first()
                .cloned()
                .map(|ige_id| {
                    (
                        ige_id,
                        ManualIgeBeddingValue {
                            k_kn_m3: 321.5,
                            assumption_note: "Manually stated partial synthetic k".into(),
                            source_ids: vec![source_id],
                        },
                    )
                })
                .into_iter()
                .collect(),
        };
        let partial = validate_bedding_assumption(&low_context, &partial_by_ige)
            .expect("missing IGE values are a readiness state");
        assert_eq!(partial.state, FoundationGeoReadinessState::Incomplete);
        assert!(!partial.missing_ige_values.is_empty());
    }

    #[test]
    fn requires_manual_source_references_and_rejects_duplicate_ige_json_keys() {
        let resolved = demo_resolved(None, None, "SNAP-FOUNDATION-MANUAL-REFERENCES");
        let context = resolve_geo_context(
            &resolved,
            &layout_at("layout-manual-references", [5.0, 6.0]),
        )
        .expect("valid context");

        assert!(matches!(
            validate_bedding_assumption(&context, &manual_uniform(100.0, vec![])),
            Err(FoundationGeoError::InvalidBedding { .. })
        ));

        let missing_per_ige_source = FoundationBeddingAssumption::ManualSyntheticPerIge {
            by_ige: context
                .sampled_ige_ids
                .iter()
                .cloned()
                .map(|ige_id| {
                    (
                        ige_id,
                        ManualIgeBeddingValue {
                            k_kn_m3: 100.0,
                            assumption_note: "Manual test assumption".into(),
                            source_ids: vec![],
                        },
                    )
                })
                .collect(),
        };
        assert!(matches!(
            validate_bedding_assumption(&context, &missing_per_ige_source),
            Err(FoundationGeoError::InvalidBedding { .. })
        ));

        let duplicate_keys = r#"{"kind":"MANUAL_SYNTHETIC_PER_IGE","by_ige":{"IGE-FILL":{"k_kn_m3":100.0,"assumption_note":"manual"},"IGE-FILL":{"k_kn_m3":200.0,"assumption_note":"replacement"}}}"#;
        assert!(serde_json::from_str::<FoundationBeddingAssumption>(duplicate_keys).is_err());
    }

    #[test]
    fn rejects_unknown_fields_on_tagged_geo_errors() {
        let extra_field = r#"{"code":"INVALID_SNAPSHOT","reason":"bad binding","unexpected":true}"#;
        assert!(serde_json::from_str::<FoundationGeoError>(extra_field).is_err());
    }

    #[test]
    fn bounds_per_ige_bedding_json_map_size() {
        use std::fmt::Write as _;

        let mut json = String::from(r#"{"kind":"MANUAL_SYNTHETIC_PER_IGE","by_ige":{"#);
        for index in 0..=MAX_MANUAL_IGE_VALUES {
            if index != 0 {
                json.push(',');
            }
            write!(
                &mut json,
                "\"IGE-{index}\":{{\"k_kn_m3\":1.0,\"assumption_note\":\"manual\"}}"
            )
            .expect("string formatting should succeed");
        }
        json.push_str("}}");
        assert!(serde_json::from_str::<FoundationBeddingAssumption>(&json).is_err());
    }
}

