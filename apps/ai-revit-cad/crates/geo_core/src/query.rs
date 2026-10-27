use crate::{geometry, model::*, validate_snapshot_binding};

/// Queries an immutable, content-bound selected model. Each geometry entry point
/// verifies the snapshot and scenario before returning derived values.
pub fn query_ground(
    resolved: &ResolvedGeoSnapshot,
    query: &GeoQuery,
) -> Result<GeoQueryResult, GeoError> {
    match query {
        GeoQuery::ColumnAt { xy_m } => geometry::query_column(resolved, *xy_m),
        GeoQuery::Section { polyline_xy_m, max_spacing_m } => {
            geometry::query_section(resolved, polyline_xy_m, *max_spacing_m)
        }
        GeoQuery::LayerVolumes => geometry::layer_volumes(resolved),
    }
}

impl ResolvedGeoSnapshot {
    pub fn query(&self, query: &GeoQuery) -> Result<GeoQueryResult, GeoError> {
        query_ground(self, query)
    }

    pub fn column_at(&self, xy_m: [f64; 2]) -> Result<GeoQueryResult, GeoError> {
        query_ground(self, &GeoQuery::ColumnAt { xy_m })
    }

    pub fn section_along(
        &self,
        polyline_xy_m: Vec<[f64; 2]>,
        max_spacing_m: f64,
    ) -> Result<GeoQueryResult, GeoError> {
        query_ground(self, &GeoQuery::Section { polyline_xy_m, max_spacing_m })
    }

    /// An explicit dry assumption is a prerequisite for dry-only consumers.
    /// This gate never substitutes unknown water data or executes a solver.
    pub fn require_explicit_dry(&self) -> Result<&[SourceId], GeoError> {
        validate_snapshot_binding(&self.snapshot)?;
        if self.snapshot.selection.scenario_id.as_ref() != Some(&self.scenario.id)
            || self.scenario.model_id != self.snapshot.selection.model_id
            || self.snapshot.records.scenarios.get(&self.scenario.id) != Some(&self.scenario)
        {
            return Err(GeoError::SnapshotBindingMismatch {
                reason: "Resolved scenario differs from the selected immutable scenario".into(),
            });
        }
        match &self.scenario.water {
            WaterScenario::ExplicitDry { source_ids } => Ok(source_ids),
            WaterScenario::Unknown => Err(GeoError::Unsupported {
                reason: "EXPLICIT_DRY_REQUIRED: water scenario is unknown".into(),
            }),
            WaterScenario::Observations { .. } => Err(GeoError::Unsupported {
                reason: "WATER_ANALYSIS_NOT_IMPLEMENTED: observed water is not an explicit dry assumption".into(),
            }),
        }
    }
}
