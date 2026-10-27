//! Derived, source-bound geology samples under a raft. Never a soil registry,
//! continuous coverage proof, bedding calibration or settlement calculation.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_GEO_FOUNDATION_MAP_CELLS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoBoundQueryResult {
    pub selection: geo_core::GeoSnapshotSelection,
    pub source_revision: geo_core::GeoSourceRevision,
    pub selection_fingerprint: String,
    pub snapshot_fingerprint: String,
    pub coordinate_frame: geo_core::GeoCoordinateFrame,
    pub interpolation: geo_core::InterpolationMethod,
    pub applicability: String,
    pub result: geo_core::GeoQueryResult,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoPointSoilResult {
    pub point_m: [f64; 3],
    pub status: geo_core::QueryStatus,
    pub ige_ids: Vec<geo_core::IgeId>,
    pub reasons: Vec<String>,
    pub source: GeoBoundQueryResult,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationMapLayer {
    pub ige_id: geo_core::IgeId,
    pub top_abs_m: f64,
    pub bottom_abs_m: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationMapCell {
    pub index: usize,
    pub xy_m: [f64; 2],
    pub sole_z_m: f64,
    pub area_m2: f64,
    pub status: geo_core::QueryStatus,
    pub terrain_z_m: Option<f64>,
    pub embedment_depth_m: Option<f64>,
    /// Two IDs identify a contact interface; no tie is silently resolved.
    pub base_ige_ids: Vec<geo_core::IgeId>,
    pub layers_below_base: Vec<GeoFoundationMapLayer>,
    pub reasons: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationGroundMap {
    pub foundation_id: String,
    pub placement: foundation_core::FoundationPlacement,
    pub ground: geo_core::GeoSnapshot,
    pub nx: usize,
    pub ny: usize,
    pub cells: Vec<GeoFoundationMapCell>,
    pub midpoint_area_estimate_by_ige_m2: BTreeMap<geo_core::IgeId, f64>,
    pub unknown_or_interface_sample_area_m2: f64,
    pub water_scenario: geo_core::WaterScenario,
    pub sampling_policy: String,
    pub numerical_meaning: String,
}

impl Document {
    /// Include source identity in a standalone diagnostic response, rather
    /// than requiring a caller to retain the surrounding Document implicitly.
    pub fn query_geo_bound(
        &self,
        selection: &geo_core::GeoSnapshotSelection,
        query: &geo_core::GeoQuery,
    ) -> Result<GeoBoundQueryResult, CoreError> {
        let resolved = current_resolved_ground(self, selection, "source-bound-query")?;
        let model = resolved
            .snapshot
            .records
            .models
            .get(&selection.model_id)
            .ok_or_else(|| map_error("selected model missing from query closure"))?;
        let result = geo_core::query_ground(&resolved, query).map_err(map_error)?;
        Ok(GeoBoundQueryResult { selection: selection.clone(), source_revision: resolved.snapshot.source_revision.clone(),
            selection_fingerprint: resolved.snapshot.selection_fingerprint.clone(), snapshot_fingerprint: resolved.snapshot.content_fingerprint.clone(),
            coordinate_frame: model.frame.clone(), interpolation: model.interpolation,
            applicability: "INTERPRETED_GEOMETRY_ONLY; source and parameter provenance remain authoritative; no site/design approval, groundwater-pressure or normative calculation".into(), result })
    }

    /// A point on a geological interface retains both adjacent IGEs and an
    /// explicit unresolved side; it never invents a blended SoilMaterial.
    pub fn soil_at(
        &self,
        selection: &geo_core::GeoSnapshotSelection,
        point_m: [f64; 3],
    ) -> Result<GeoPointSoilResult, CoreError> {
        if point_m.iter().any(|v| !v.is_finite()) {
            return Err(map_error("soil_at coordinates must be finite metres"));
        }
        let source = self.query_geo_bound(
            selection,
            &geo_core::GeoQuery::ColumnAt {
                xy_m: [point_m[0], point_m[1]],
            },
        )?;
        let mut status = source.result.status;
        let mut reasons = source.result.reasons.clone();
        let ige_ids = source
            .result
            .column
            .as_ref()
            .map(|c| column_iges_at_z(c, point_m[2]))
            .unwrap_or_default();
        if status == geo_core::QueryStatus::Known {
            if ige_ids.is_empty() {
                status = geo_core::QueryStatus::OutsideCoverage;
                reasons.push("Z_OUTSIDE_MODELED_VERTICAL_COVERAGE".into());
            }
            if ige_ids.len() > 1 {
                status = geo_core::QueryStatus::Unknown;
                reasons.push(
                    "ON_GEOLOGICAL_INTERFACE: an explicit side-selection policy is required".into(),
                );
            }
        }
        Ok(GeoPointSoilResult {
            point_m,
            status,
            ige_ids,
            reasons,
            source,
        })
    }

    /// Read-only projection. It changes neither revisions nor retained results.
    /// Sampling describes the named immutable interpretation; it is not a
    /// proof of measured soil at every point between boreholes.
    pub fn foundation_ground_map(
        &self,
        foundation_id: &str,
        nx: usize,
        ny: usize,
    ) -> Result<GeoFoundationGroundMap, CoreError> {
        let count = nx
            .checked_mul(ny)
            .filter(|n| *n > 0 && *n <= MAX_GEO_FOUNDATION_MAP_CELLS)
            .ok_or_else(|| map_error("ground map requires 1..4096 bounded cells"))?;
        let layout = self
            .geo_foundation()
            .layout()
            .ok_or_else(|| map_error("foundation layout is absent"))?;
        foundation_core::validate_foundation_layout(&layout.input.layout).map_err(map_error)?;
        let placement = layout
            .input
            .layout
            .elements
            .iter()
            .find(|p| placement_id(p) == foundation_id)
            .ok_or_else(|| map_error("foundation placement is absent"))?;
        let foundation_core::FoundationPlacement::Raft {
            center_m,
            length_x_m,
            width_y_m,
            ..
        } = placement
        else {
            return Err(map_error(
                "UNSUPPORTED_FOUNDATION_MAP_SHAPE: current grid projection requires an axis-aligned rectangular raft",
            ));
        };
        let selection = layout
            .input
            .ground_links
            .get(foundation_id)
            .ok_or_else(|| map_error("foundation has no ground selection"))?;
        let resolved = current_resolved_ground(self, selection, "foundation-ground-map")?;
        let ground = resolved.snapshot.clone();
        let dx = length_x_m / nx as f64;
        let dy = width_y_m / ny as f64;
        let area_m2 = dx * dy;
        let points: Vec<[f64; 2]> = (0..count)
            .map(|index| {
                [
                    center_m[0] - length_x_m / 2.0 + (index % nx) as f64 * dx + dx / 2.0,
                    center_m[1] - width_y_m / 2.0 + (index / nx) as f64 * dy + dy / 2.0,
                ]
            })
            .collect();
        let results = geo_core::query_columns(&resolved, &points).map_err(map_error)?;
        if results.len() != count {
            return Err(map_error("ground query batch lost a requested point"));
        }
        let mut cells = Vec::with_capacity(count);
        let mut estimates = BTreeMap::new();
        let mut unknown_area = 0.0;
        for (index, query) in results.into_iter().enumerate() {
            let mut cell = GeoFoundationMapCell {
                index,
                xy_m: points[index],
                sole_z_m: center_m[2],
                area_m2,
                status: query.status,
                terrain_z_m: None,
                embedment_depth_m: None,
                base_ige_ids: Vec::new(),
                layers_below_base: Vec::new(),
                reasons: query.reasons,
            };
            if let Some(column) = query.column {
                cell.terrain_z_m = Some(column.terrain_abs_m);
                cell.embedment_depth_m = Some(column.terrain_abs_m - center_m[2]);
                cell.base_ige_ids = column_iges_at_z(&column, center_m[2]);
                for layer in column.layers {
                    if layer.bottom_abs_m < center_m[2] && !layer.pinched {
                        cell.layers_below_base.push(GeoFoundationMapLayer {
                            ige_id: layer.ige_id,
                            top_abs_m: layer.top_abs_m.min(center_m[2]),
                            bottom_abs_m: layer.bottom_abs_m,
                        });
                    }
                }
                if cell.base_ige_ids.is_empty() {
                    cell.status = geo_core::QueryStatus::Unknown;
                    cell.reasons
                        .push("FOUNDATION_BASE_OUTSIDE_MODELED_VERTICAL_COVERAGE".into());
                } else if cell.base_ige_ids.len() > 1 {
                    cell.status = geo_core::QueryStatus::Unknown;
                    cell.reasons.push(
                        "BASE_ON_GEOLOGICAL_INTERFACE: no material tie-break assumption".into(),
                    );
                }
            }
            if cell.status == geo_core::QueryStatus::Known && cell.base_ige_ids.len() == 1 {
                *estimates.entry(cell.base_ige_ids[0].clone()).or_insert(0.0) += area_m2;
            } else {
                unknown_area += area_m2;
            }
            cells.push(cell);
        }
        Ok(GeoFoundationGroundMap { foundation_id: foundation_id.into(), placement: placement.clone(), ground,
            nx, ny, cells, midpoint_area_estimate_by_ige_m2: estimates, unknown_or_interface_sample_area_m2: unknown_area,
            water_scenario: resolved.scenario.water,
            sampling_policy: "MIDPOINT_SAMPLE_MAP; area totals are quadrature estimates; finite samples do not prove continuous coverage or exclude unresolved interior gaps".into(),
            numerical_meaning: "GEO geometry/provenance only; no automatic E-to-k, bearing, layered settlement or groundwater-pressure calculation".into() })
    }
}
fn column_iges_at_z(column: &geo_core::GroundColumn, z: f64) -> Vec<geo_core::IgeId> {
    let mut ids: Vec<_> = column
        .layers
        .iter()
        .filter(|layer| {
            !layer.pinched && z <= layer.top_abs_m + 1e-9 && z >= layer.bottom_abs_m - 1e-9
        })
        .map(|layer| layer.ige_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}
fn current_resolved_ground(
    document: &Document,
    selection: &geo_core::GeoSnapshotSelection,
    snapshot_id: &str,
) -> Result<geo_core::ResolvedGeoSnapshot, CoreError> {
    let source = document
        .geo_state()
        .head_revision
        .clone()
        .ok_or_else(|| map_error("GEO source operation is absent"))?;
    let ground = geo_core::make_geo_snapshot(
        document.geo_state(),
        geo_core::GeoSnapshotId::new(snapshot_id).map_err(map_error)?,
        selection,
        source,
    )
    .map_err(map_error)?;
    let scenario_id = selection.scenario_id.as_ref().ok_or_else(|| {
        map_error("a named groundwater scenario is required even for a geometry query")
    })?;
    let scenario = ground
        .records
        .scenarios
        .get(scenario_id)
        .ok_or_else(|| map_error("selected scenario is absent"))?
        .clone();
    geo_core::resolve_geo_scenario(&ground, &scenario).map_err(map_error)
}
fn map_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidDocument(format!("foundation ground map: {error}"))
}
