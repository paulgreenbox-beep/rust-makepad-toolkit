use crate::{
    model::*,
    validation::{
        checked_add_record_count, geo_record_count, validate_geo_records, validate_geo_state,
    },
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

impl GeoState {
    /// Starts an editable geology document with no records or source revision.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Applies one command to a clone and publishes the clone only after it validates.
/// Every upserted record receives the revision supplied by the owning document.
pub fn apply_geo_command(
    state: &GeoState,
    command: &GeoCommand,
    source_revision: GeoSourceRevision,
) -> Result<GeoMutation, GeoError> {
    validate_source_revision(&source_revision)?;
    if let Some(head) = &state.head_revision {
        validate_source_revision(head)?;
        if head.document_uuid != source_revision.document_uuid {
            return Err(invalid(
                "source_revision.document_uuid",
                "A GeoState cannot change owning document UUID",
            ));
        }
        if source_revision.operation_id <= head.operation_id {
            return Err(invalid(
                "source_revision.operation_id",
                "Operation ID must increase beyond the current state head",
            ));
        }
    }
    if let GeoCommand::ImportRecords { records } = command {
        preflight_import_count(&state.records, records)?;
        reject_import_collisions(&state.records, records)?;
    }

    let mut next = state.clone();
    let mut delta = GeoDelta::default();
    match command {
        GeoCommand::ImportRecords { records } => {
            merge_imported_records(
                &mut next.records,
                records,
                source_revision.operation_id,
                &mut delta.changed_records,
            );
        }
        GeoCommand::UpsertSource { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.sources.insert(id.clone(), record);
            delta.changed_records.push(format!("sources:{id}"));
        }
        GeoCommand::UpsertSoil { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.soils.insert(id.clone(), record);
            delta.changed_records.push(format!("soils:{id}"));
        }
        GeoCommand::UpsertIge { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.iges.insert(id.clone(), record);
            delta.changed_records.push(format!("iges:{id}"));
        }
        GeoCommand::UpsertBorehole { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.boreholes.insert(id.clone(), record);
            delta.changed_records.push(format!("boreholes:{id}"));
        }
        GeoCommand::UpsertModel { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.models.insert(id.clone(), record);
            delta.changed_records.push(format!("models:{id}"));
        }
        GeoCommand::UpsertScenario { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.scenarios.insert(id.clone(), record);
            delta.changed_records.push(format!("scenarios:{id}"));
        }
        GeoCommand::UpsertWaterObservation { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.water_observations.insert(id.clone(), record);
            delta.changed_records.push(format!("water_observations:{id}"));
        }
        GeoCommand::UpsertSection { record } => {
            let mut record = record.clone();
            record.revision = source_revision.operation_id;
            let id = record.id.clone();
            next.records.sections.insert(id.clone(), record);
            delta.changed_records.push(format!("sections:{id}"));
        }
        GeoCommand::SaveSnapshot { id, selection } => {
            if next.snapshots.contains_key(id) {
                return Err(invalid(
                    "snapshot.id",
                    "Snapshots are immutable; this snapshot ID already exists",
                ));
            }
            let snapshot = make_geo_snapshot(state, id.clone(), selection, source_revision.clone())?;
            next.snapshots.insert(id.clone(), snapshot);
            delta.created_snapshot_ids.push(id.clone());
        }
    }
    next.head_revision = Some(source_revision);
    validate_geo_state(&next)?;
    Ok(GeoMutation { state: next, delta })
}

fn preflight_import_count(existing: &GeoRecords, incoming: &GeoRecords) -> Result<(), GeoError> {
    let existing_count = geo_record_count(existing)?;
    let incoming_count = geo_record_count(incoming)?;
    let combined = checked_add_record_count(existing_count, incoming_count)?;
    if combined > MAX_RECORDS {
        return Err(GeoError::ResourceLimit {
            reason: format!(
                "Import would raise GeoRecords count to {combined}, exceeding MAX_RECORDS ({MAX_RECORDS})"
            ),
        });
    }
    Ok(())
}

/// Captures only the selected model's referenced closure and the optional scenario.
/// The state is not changed and no solver or model generator is called.
pub fn make_geo_snapshot(
    state: &GeoState,
    id: GeoSnapshotId,
    selection: &GeoSnapshotSelection,
    source_revision: GeoSourceRevision,
) -> Result<GeoSnapshot, GeoError> {
    validate_source_revision(&source_revision)?;
    if let Some(head) = &state.head_revision {
        validate_source_revision(head)?;
        if head.document_uuid != source_revision.document_uuid
            || source_revision.operation_id < head.operation_id
        {
            return Err(invalid(
                "source_revision",
                "Snapshot revision must belong to this document and be at least the state head",
            ));
        }
    }
    let records = selected_records(&state.records, selection)?;
    validate_geo_records(&records)?;
    ensure_snapshot_record_revisions(&records, &source_revision)?;

    let selection_fingerprint = selection_fingerprint(selection, &records)?;
    let content_fingerprint = content_fingerprint(
        &id,
        &source_revision,
        selection,
        &records,
        &selection_fingerprint,
    )?;
    Ok(GeoSnapshot {
        id,
        source_revision,
        selection: selection.clone(),
        records,
        selection_fingerprint,
        content_fingerprint,
    })
}

/// Resolves only the scenario selected into this immutable snapshot.
pub fn resolve_geo_scenario(
    snapshot: &GeoSnapshot,
    scenario: &GeoScenario,
) -> Result<ResolvedGeoSnapshot, GeoError> {
    validate_snapshot_binding(snapshot)?;
    if snapshot.selection.scenario_id.as_ref() != Some(&scenario.id) {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Scenario ID is not the scenario bound to this snapshot".into(),
        });
    }
    if scenario.model_id != snapshot.selection.model_id {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Scenario references a different ground model".into(),
        });
    }
    if snapshot.records.scenarios.get(&scenario.id) != Some(scenario) {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Scenario content does not match the snapshot's immutable copy".into(),
        });
    }
    Ok(ResolvedGeoSnapshot {
        snapshot: snapshot.clone(),
        scenario: scenario.clone(),
    })
}

/// Checks the selected closure only. Changes to other models do not stale a snapshot.
pub fn snapshot_currentness(
    state: &GeoState,
    snapshot: &GeoSnapshot,
) -> Result<SnapshotCurrentness, GeoError> {
    validate_snapshot_binding(snapshot)?;
    if state.head_revision.as_ref().is_some_and(|head| {
        head.document_uuid != snapshot.source_revision.document_uuid
    }) {
        return Ok(SnapshotCurrentness::Stale);
    }
    let current_records = selected_records(&state.records, &snapshot.selection)?;
    validate_geo_records(&current_records)?;
    let selection_fingerprint = selection_fingerprint(&snapshot.selection, &current_records)?;
    let content_fingerprint = content_fingerprint(
        &snapshot.id,
        &snapshot.source_revision,
        &snapshot.selection,
        &current_records,
        &selection_fingerprint,
    )?;
    Ok(if selection_fingerprint == snapshot.selection_fingerprint
        && content_fingerprint == snapshot.content_fingerprint
    {
        SnapshotCurrentness::Current
    } else {
        SnapshotCurrentness::Stale
    })
}

/// Verifies that a snapshot contains exactly its selected closure and both hashes bind it.
pub fn validate_snapshot_binding(snapshot: &GeoSnapshot) -> Result<(), GeoError> {
    validate_source_revision(&snapshot.source_revision).map_err(snapshot_binding_error)?;
    validate_geo_records(&snapshot.records).map_err(snapshot_binding_error)?;
    ensure_snapshot_record_revisions(&snapshot.records, &snapshot.source_revision).map_err(|error| {
        GeoError::SnapshotBindingMismatch {
            reason: format!("Snapshot contains a record newer than its source revision: {error}"),
        }
    })?;

    let reconstructed = selected_records(&snapshot.records, &snapshot.selection).map_err(|error| {
        GeoError::SnapshotBindingMismatch {
            reason: format!("Snapshot closure cannot be reconstructed: {error}"),
        }
    })?;
    if reconstructed != snapshot.records {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Snapshot contains records outside its selected reference closure".into(),
        });
    }
    let selection_fingerprint = selection_fingerprint(&snapshot.selection, &snapshot.records)?;
    if selection_fingerprint != snapshot.selection_fingerprint {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Selection fingerprint does not match selected IDs and record revisions".into(),
        });
    }
    let content_fingerprint = content_fingerprint(
        &snapshot.id,
        &snapshot.source_revision,
        &snapshot.selection,
        &snapshot.records,
        &snapshot.selection_fingerprint,
    )?;
    if content_fingerprint != snapshot.content_fingerprint {
        return Err(GeoError::SnapshotBindingMismatch {
            reason: "Content fingerprint does not match immutable snapshot content".into(),
        });
    }
    Ok(())
}

fn snapshot_binding_error(error: GeoError) -> GeoError {
    GeoError::SnapshotBindingMismatch {
        reason: format!("Snapshot content is invalid: {error}"),
    }
}

fn selected_records(
    records: &GeoRecords,
    selection: &GeoSnapshotSelection,
) -> Result<GeoRecords, GeoError> {
    let model = records.models.get(&selection.model_id).ok_or_else(|| {
        missing_reference("ground_model", selection.model_id.as_str())
    })?;
    if model.borehole_ids.len() > MAX_MODEL_WELLS
        || model.triangles.len() > MAX_TRIANGLES
        || model.contacts.len() > MAX_CONTACTS
        || model.section_ids.len() > MAX_RECORDS
        || model.unknown_zones.len() > MAX_RECORDS
    {
        return Err(GeoError::ResourceLimit {
            reason: "Selected ground model exceeds bounded record counts".into(),
        });
    }
    let mut output = GeoRecords::default();
    let mut source_ids = BTreeSet::new();
    let mut ige_ids = BTreeSet::new();

    output.models.insert(model.id.clone(), model.clone());
    collect_source_ids(&mut source_ids, &model.source_ids, "model.source_ids")?;

    let model_boreholes: BTreeSet<_> = model.borehole_ids.iter().cloned().collect();
    for borehole_id in &model.borehole_ids {
        let borehole = records.boreholes.get(borehole_id).ok_or_else(|| {
            missing_reference("borehole", borehole_id.as_str())
        })?;
        if borehole.intervals.len() > MAX_RECORDS {
            return Err(GeoError::ResourceLimit {
                reason: "Selected borehole interval count exceeds MAX_RECORDS".into(),
            });
        }
        output.boreholes.insert(borehole_id.clone(), borehole.clone());
        collect_source_ids(&mut source_ids, &borehole.source_ids, "borehole.source_ids")?;
        collect_source_ids(&mut source_ids, &borehole.survey_source_ids, "borehole.survey_source_ids")?;
        collect_source_ids(&mut source_ids, &borehole.evidence_source_ids, "borehole.evidence_source_ids")?;
        collect_parameter_sources(&borehole.total_depth_m, &mut source_ids)?;
        for interval in &borehole.intervals {
            ige_ids.insert(interval.ige_id.clone());
            collect_source_ids(&mut source_ids, &interval.source_ids, "borehole.interval.source_ids")?;
        }
    }
    for triangle in &model.triangles {
        for vertex in &triangle.vertices {
            if !model_boreholes.contains(vertex) {
                return Err(invalid(
                    "model.triangles.vertices",
                    "Every triangle vertex must be listed in model.borehole_ids",
                ));
            }
        }
        collect_source_ids(&mut source_ids, &triangle.source_ids, "model.triangle.source_ids")?;
    }
    for contact in &model.contacts {
        if contact.elevations_abs_m.len() > MAX_RECORDS {
            return Err(GeoError::ResourceLimit {
                reason: "Selected contact observation count exceeds MAX_RECORDS".into(),
            });
        }
        ige_ids.insert(contact.ige_id.clone());
        collect_source_ids(&mut source_ids, &contact.source_ids, "model.contact.source_ids")?;
        for (borehole_id, elevation) in &contact.elevations_abs_m {
            if !model_boreholes.contains(borehole_id) {
                return Err(invalid(
                    "model.contacts.elevations_abs_m",
                    "Contact elevations may only refer to listed model boreholes",
                ));
            }
            collect_parameter_sources(elevation, &mut source_ids)?;
        }
    }
    for zone in &model.unknown_zones {
        collect_source_ids(&mut source_ids, &zone.source_ids, "model.unknown_zone.source_ids")?;
    }
    for section_id in &model.section_ids {
        let section = records.sections.get(section_id).ok_or_else(|| {
            missing_reference("section", section_id.as_str())
        })?;
        if section.model_id != model.id {
            return Err(invalid(
                "model.section_ids",
                "Referenced section must belong to the selected model",
            ));
        }
        collect_source_ids(&mut source_ids, &section.source_ids, "section.source_ids")?;
        output.sections.insert(section_id.clone(), section.clone());
    }

    for ige_id in ige_ids {
        let ige = records.iges.get(&ige_id).ok_or_else(|| {
            missing_reference("ige", ige_id.as_str())
        })?;
        collect_source_ids(&mut source_ids, &ige.source_ids, "ige.source_ids")?;
        let soil = records.soils.get(&ige.soil_id).ok_or_else(|| {
            missing_reference("soil", ige.soil_id.as_str())
        })?;
        source_ids.extend(soil_parameter_sources(&soil.parameters)?);
        output.soils.insert(soil.id.clone(), soil.clone());
        output.iges.insert(ige_id, ige.clone());
    }

    if let Some(scenario_id) = &selection.scenario_id {
        let scenario = records.scenarios.get(scenario_id).ok_or_else(|| {
            missing_reference("scenario", scenario_id.as_str())
        })?;
        if scenario.model_id != model.id {
            return Err(invalid(
                "scenario.model_id",
                "Selected scenario must reference the selected model",
            ));
        }
        collect_source_ids(&mut source_ids, &scenario.source_ids, "scenario.source_ids")?;
        match &scenario.water {
            WaterScenario::Unknown => {}
            WaterScenario::ExplicitDry { source_ids: water_sources } => {
                collect_source_ids(&mut source_ids, water_sources, "scenario.water.source_ids")?;
            }
            WaterScenario::Observations { observation_ids } => {
                if observation_ids.len() > MAX_RECORDS {
                    return Err(GeoError::ResourceLimit {
                        reason: "Selected water observation count exceeds MAX_RECORDS".into(),
                    });
                }
                for observation_id in observation_ids {
                    let observation = records.water_observations.get(observation_id).ok_or_else(|| {
                        missing_reference("water_observation", observation_id.as_str())
                    })?;
                    collect_parameter_sources(&observation.elevation_abs_m, &mut source_ids)?;
                    output.water_observations.insert(observation_id.clone(), observation.clone());
                }
            }
        }
        output.scenarios.insert(scenario_id.clone(), scenario.clone());
    }

    for source_id in source_ids {
        let source = records.sources.get(&source_id).ok_or_else(|| {
            missing_reference("source", source_id.as_str())
        })?;
        output.sources.insert(source_id, source.clone());
    }
    Ok(output)
}

fn reject_import_collisions(existing: &GeoRecords, incoming: &GeoRecords) -> Result<(), GeoError> {
    macro_rules! reject_collisions {
        ($field:ident) => {
            if let Some(id) = incoming.$field.keys().find(|id| existing.$field.contains_key(*id)) {
                return Err(GeoError::InvalidInput {
                    path: concat!("records.", stringify!($field)).into(),
                    message: format!("Import collides with existing ID {id}"),
                });
            }
        };
    }
    reject_collisions!(sources);
    reject_collisions!(soils);
    reject_collisions!(iges);
    reject_collisions!(boreholes);
    reject_collisions!(models);
    reject_collisions!(scenarios);
    reject_collisions!(water_observations);
    reject_collisions!(sections);
    Ok(())
}

fn merge_imported_records(
    target: &mut GeoRecords,
    incoming: &GeoRecords,
    operation_id: u64,
    changed: &mut Vec<String>,
) {
    macro_rules! merge_records {
        ($field:ident, $label:literal) => {
            for (id, record) in &incoming.$field {
                let mut record = record.clone();
                record.revision = operation_id;
                target.$field.insert(id.clone(), record);
                changed.push(format!(concat!($label, ":{}"), id));
            }
        };
    }
    merge_records!(sources, "sources");
    merge_records!(soils, "soils");
    merge_records!(iges, "iges");
    merge_records!(boreholes, "boreholes");
    merge_records!(models, "models");
    merge_records!(scenarios, "scenarios");
    merge_records!(water_observations, "water_observations");
    merge_records!(sections, "sections");
}

fn collect_source_ids(
    sources: &mut BTreeSet<SourceId>,
    ids: &[SourceId],
    path: &str,
) -> Result<(), GeoError> {
    if ids.len() > MAX_RECORDS {
        return Err(GeoError::ResourceLimit {
            reason: format!("{path} exceeds MAX_RECORDS"),
        });
    }
    sources.extend(ids.iter().cloned());
    Ok(())
}

fn collect_parameter_sources<T>(
    parameter: &Parameter<T>,
    sources: &mut BTreeSet<SourceId>,
) -> Result<(), GeoError> {
    match parameter {
        Parameter::Missing { .. } => {}
        Parameter::Known { source_ids, .. } => collect_source_ids(sources, source_ids, "parameter.source_ids")?,
        Parameter::Conflicting { candidates } => {
            if candidates.len() > MAX_RECORDS {
                return Err(GeoError::ResourceLimit {
                    reason: "Parameter candidate count exceeds MAX_RECORDS".into(),
                });
            }
            for candidate in candidates {
                collect_source_ids(sources, &candidate.source_ids, "parameter.candidate.source_ids")?;
            }
        }
    }
    Ok(())
}

fn soil_parameter_sources(parameters: &SoilParameters) -> Result<BTreeSet<SourceId>, GeoError> {
    let mut sources = BTreeSet::new();
    collect_parameter_sources(&parameters.unit_weight_kn_m3, &mut sources)?;
    collect_parameter_sources(&parameters.cohesion_kpa, &mut sources)?;
    collect_parameter_sources(&parameters.friction_angle_deg, &mut sources)?;
    collect_parameter_sources(&parameters.elastic_modulus_kpa, &mut sources)?;
    collect_parameter_sources(&parameters.poisson_ratio, &mut sources)?;
    Ok(sources)
}

fn selection_fingerprint(
    selection: &GeoSnapshotSelection,
    records: &GeoRecords,
) -> Result<String, GeoError> {
    #[derive(Serialize)]
    struct SelectionBinding<'a> {
        selection: &'a GeoSnapshotSelection,
        records: Vec<(String, String, u64)>,
    }
    let mut revisions = Vec::new();
    revisions.extend(records.sources.iter().map(|(id, record)| ("source".into(), id.to_string(), record.revision)));
    revisions.extend(records.soils.iter().map(|(id, record)| ("soil".into(), id.to_string(), record.revision)));
    revisions.extend(records.iges.iter().map(|(id, record)| ("ige".into(), id.to_string(), record.revision)));
    revisions.extend(records.boreholes.iter().map(|(id, record)| ("borehole".into(), id.to_string(), record.revision)));
    revisions.extend(records.models.iter().map(|(id, record)| ("model".into(), id.to_string(), record.revision)));
    revisions.extend(records.scenarios.iter().map(|(id, record)| ("scenario".into(), id.to_string(), record.revision)));
    revisions.extend(records.water_observations.iter().map(|(id, record)| ("water_observation".into(), id.to_string(), record.revision)));
    revisions.extend(records.sections.iter().map(|(id, record)| ("section".into(), id.to_string(), record.revision)));
    hash_serialized(&SelectionBinding { selection, records: revisions })
}

fn content_fingerprint(
    id: &GeoSnapshotId,
    source_revision: &GeoSourceRevision,
    selection: &GeoSnapshotSelection,
    records: &GeoRecords,
    selection_fingerprint: &str,
) -> Result<String, GeoError> {
    #[derive(Serialize)]
    struct SnapshotContent<'a> {
        id: &'a GeoSnapshotId,
        source_revision: &'a GeoSourceRevision,
        selection: &'a GeoSnapshotSelection,
        records: &'a GeoRecords,
        selection_fingerprint: &'a str,
    }
    hash_serialized(&SnapshotContent {
        id,
        source_revision,
        selection,
        records,
        selection_fingerprint,
    })
}

fn ensure_snapshot_record_revisions(
    records: &GeoRecords,
    source_revision: &GeoSourceRevision,
) -> Result<(), GeoError> {
    macro_rules! check_revisions {
        ($field:ident) => {
            if records.$field.values().any(|record| record.revision > source_revision.operation_id) {
                return Err(invalid(
                    "snapshot.records.revision",
                    "Snapshot record revision exceeds the snapshot source revision",
                ));
            }
        };
    }
    check_revisions!(sources);
    check_revisions!(soils);
    check_revisions!(iges);
    check_revisions!(boreholes);
    check_revisions!(models);
    check_revisions!(scenarios);
    check_revisions!(water_observations);
    check_revisions!(sections);
    Ok(())
}

fn hash_serialized<T: Serialize>(value: &T) -> Result<String, GeoError> {
    let bytes = serde_json::to_vec(value).map_err(|error| GeoError::Serialization {
        message: error.to_string(),
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn validate_source_revision(revision: &GeoSourceRevision) -> Result<(), GeoError> {
    if revision.document_uuid.trim().is_empty() || revision.document_uuid.len() > 128 {
        return Err(invalid(
            "source_revision.document_uuid",
            "Document UUID must be non-empty and at most 128 bytes",
        ));
    }
    if revision.operation_id == 0 {
        return Err(invalid(
            "source_revision.operation_id",
            "Operation ID must be positive",
        ));
    }
    Ok(())
}

fn missing_reference(entity: &str, id: &str) -> GeoError {
    GeoError::MissingReference {
        entity: entity.into(),
        id: id.into(),
    }
}

fn invalid(path: &str, message: &str) -> GeoError {
    GeoError::InvalidInput {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(operation_id: u64) -> GeoSourceRevision {
        GeoSourceRevision {
            document_uuid: "test-document".into(),
            operation_id,
        }
    }

    #[test]
    fn empty_state_has_no_editable_records_or_head() {
        let state = GeoState::empty();
        assert_eq!(state, GeoState::default());
        assert!(state.head_revision.is_none());
    }

    #[test]
    fn command_clones_state_and_overwrites_record_revision() {
        let state = GeoState::empty();
        let source = SourceRecord {
            id: SourceId::new("src-1").unwrap(),
            title: "Field log".into(),
            origin: SourceOrigin::Measured,
            reference: "log-1".into(),
            content_sha256: None,
            observed_at: None,
            revision: 999,
            source_page_or_record: None,
            source_date: None,
            source_revision: None,
        };
        let mutation = apply_geo_command(
            &state,
            &GeoCommand::UpsertSource { record: source },
            revision(4),
        )
        .unwrap();

        assert!(state.records.sources.is_empty());
        let stored = mutation.state.records.sources.values().next().unwrap();
        assert_eq!(stored.revision, 4);
        assert_eq!(mutation.state.head_revision, Some(revision(4)));
        assert_eq!(
            mutation.delta.changed_records,
            vec!["sources:src-1".to_owned()]
        );
    }

    #[test]
    fn import_is_atomic_rejects_existing_ids_and_overwrites_revisions() {
        let original = GeoState::empty();
        let source = SourceRecord {
            id: SourceId::new("src-import").unwrap(),
            title: "Imported source".into(),
            origin: SourceOrigin::ImportedLegacy,
            reference: "archive-1".into(),
            content_sha256: None,
            observed_at: None,
            revision: 999,
            source_page_or_record: None,
            source_date: None,
            source_revision: None,
        };
        let mut records = GeoRecords::default();
        records.sources.insert(source.id.clone(), source);
        let model_id = GroundModelId::new("model-cycle").unwrap();
        let section_id = GeoSectionId::new("section-cycle").unwrap();
        records.models.insert(
            model_id.clone(),
            GroundModel {
                id: model_id.clone(),
                name: "Draft model".into(),
                frame: GeoCoordinateFrame {
                    name: "local".into(),
                    crs: None,
                    vertical_datum: None,
                    missing_borehole_crs: MissingBoreholeCrsPolicy::default(),
                },
                borehole_ids: vec![],
                triangles: vec![],
                contacts: vec![],
                allow_pinch: false,
                unknown_zones: vec![],
                interpolation: InterpolationMethod::ExplicitTinPiecewisePlanar,
                source_ids: vec![],
                section_ids: vec![section_id.clone()],
                revision: 999,
            },
        );
        records.sections.insert(
            section_id.clone(),
            InterpretedSection {
                id: section_id,
                model_id,
                polyline_xy_m: vec![[0.0, 0.0], [1.0, 1.0]],
                source_ids: vec![],
                revision: 999,
            },
        );
        let mutation = apply_geo_command(
            &original,
            &GeoCommand::ImportRecords { records: records.clone() },
            revision(7),
        )
        .unwrap();
        assert_eq!(
            mutation.state.records.sources[&SourceId::new("src-import").unwrap()].revision,
            7
        );
        assert_eq!(mutation.state.records.models.values().next().unwrap().revision, 7);
        assert_eq!(mutation.state.records.sections.values().next().unwrap().revision, 7);
        assert!(original.records.sources.is_empty());

        let collision = apply_geo_command(
            &mutation.state,
            &GeoCommand::ImportRecords { records },
            revision(8),
        );
        assert!(collision.is_err());
        assert_eq!(mutation.state.head_revision, Some(revision(7)));
        assert_eq!(mutation.state.records.sources.len(), 1);
    }

    #[test]
    fn import_aggregate_preflight_rejects_count_over_limit_without_publishing() {
        let mut state = GeoState::empty();
        for index in 0..MAX_RECORDS {
            let id = GeoSoilId::new(format!("soil-{index}")).unwrap();
            state.records.soils.insert(
                id.clone(),
                GeoSoilRecord {
                    id,
                    name: "Draft".into(),
                    parameters: SoilParameters::default(),
                    revision: 0,
                },
            );
        }
        let extra_id = GeoSoilId::new("soil-extra").unwrap();
        let mut incoming = GeoRecords::default();
        incoming.soils.insert(
            extra_id.clone(),
            GeoSoilRecord {
                id: extra_id.clone(),
                name: "Extra draft".into(),
                parameters: SoilParameters::default(),
                revision: 999,
            },
        );

        let result = apply_geo_command(
            &state,
            &GeoCommand::ImportRecords { records: incoming },
            revision(1),
        );
        assert!(matches!(result, Err(GeoError::ResourceLimit { .. })));
        assert_eq!(state.records.soils.len(), MAX_RECORDS);
        assert!(!state.records.soils.contains_key(&extra_id));
    }

    fn empty_model(id: &str, name: &str, revision: u64) -> GroundModel {
        GroundModel {
            id: GroundModelId::new(id).unwrap(),
            name: name.into(),
            frame: GeoCoordinateFrame {
                name: "local".into(),
                crs: None,
                vertical_datum: None,
                missing_borehole_crs: MissingBoreholeCrsPolicy::default(),
            },
            borehole_ids: vec![],
            triangles: vec![],
            contacts: vec![],
            allow_pinch: false,
            unknown_zones: vec![],
            interpolation: InterpolationMethod::ExplicitTinPiecewisePlanar,
            source_ids: vec![],
            section_ids: vec![],
            revision,
        }
    }

    #[test]
    fn unrelated_model_edit_does_not_stale_snapshot_but_selected_edit_does() {
        let mut state = GeoState::empty();
        let model = empty_model("model-a", "Model A", 1);
        let model_id = model.id.clone();
        state.records.models.insert(model_id.clone(), model);
        let selection = GeoSnapshotSelection {
            model_id: model_id.clone(),
            scenario_id: None,
        };
        let snapshot = make_geo_snapshot(
            &state,
            GeoSnapshotId::new("snap-a").unwrap(),
            &selection,
            revision(1),
        )
        .unwrap();

        let unrelated = empty_model("model-b", "Model B", 99);
        state.records.models.insert(unrelated.id.clone(), unrelated);
        assert_eq!(
            snapshot_currentness(&state, &snapshot).unwrap(),
            SnapshotCurrentness::Current
        );

        let selected = state.records.models.get_mut(&model_id).unwrap();
        selected.name = "Edited model A".into();
        selected.revision = 2;
        assert_eq!(
            snapshot_currentness(&state, &snapshot).unwrap(),
            SnapshotCurrentness::Stale
        );
    }

    #[test]
    fn scenario_with_forged_content_cannot_bind_to_snapshot() {
        let state = crate::synthetic_demo_state().unwrap();
        let model_id = state.records.models.keys().next().unwrap().clone();
        let scenario = state.records.scenarios.values().next().unwrap().clone();
        let selection = GeoSnapshotSelection {
            model_id,
            scenario_id: Some(scenario.id.clone()),
        };
        let snapshot = make_geo_snapshot(
            &state,
            GeoSnapshotId::new("snapshot-demo-binding").unwrap(),
            &selection,
            state.head_revision.clone().unwrap(),
        )
        .unwrap();

        assert!(resolve_geo_scenario(&snapshot, &scenario).is_ok());
        let mut forged = scenario;
        forged.name.push_str(" forged");
        assert!(matches!(
            resolve_geo_scenario(&snapshot, &forged),
            Err(GeoError::SnapshotBindingMismatch { .. })
        ));
    }

    #[test]
    fn selected_source_content_stales_synthetic_snapshot_but_unrelated_source_does_not() {
        let state = crate::synthetic_demo_state().unwrap();
        let selection = GeoSnapshotSelection {
            model_id: state.records.models.keys().next().unwrap().clone(),
            scenario_id: state.records.scenarios.keys().next().cloned(),
        };
        let snapshot = make_geo_snapshot(
            &state,
            GeoSnapshotId::new("snapshot-demo-source").unwrap(),
            &selection,
            state.head_revision.clone().unwrap(),
        )
        .unwrap();

        let mut with_unrelated_source = state.clone();
        let unrelated_id = SourceId::new("SRC-UNRELATED").unwrap();
        with_unrelated_source.records.sources.insert(
            unrelated_id.clone(),
            SourceRecord {
                id: unrelated_id,
                title: "Unrelated source".into(),
                origin: SourceOrigin::Synthetic,
                reference: "not referenced by selected model".into(),
                content_sha256: None,
                observed_at: None,
                revision: 1,
                source_page_or_record: None,
                source_date: None,
                source_revision: None,
            },
        );
        assert_eq!(
            snapshot_currentness(&with_unrelated_source, &snapshot).unwrap(),
            SnapshotCurrentness::Current
        );

        let mut with_changed_source = state;
        let selected_source_id = with_changed_source.records.sources.keys().next().unwrap().clone();
        with_changed_source.records.sources.get_mut(&selected_source_id).unwrap().title
            .push_str(" edited without a revision change");
        assert_eq!(
            snapshot_currentness(&with_changed_source, &snapshot).unwrap(),
            SnapshotCurrentness::Stale
        );
    }
}
