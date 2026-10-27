use crate::{
    geometry::validate_model_geometry,
    model::*,
    state::validate_snapshot_binding,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_UNIT_WEIGHT_KN_M3: f64 = 150.0;
const MAX_PARAMETER_KPA: f64 = 1.0e12;
const MAX_DEPTH_DATUM_BYTES: usize = 256;

pub fn validate_geo_state(state: &GeoState) -> Result<(), GeoError> {
    if let Some(head) = &state.head_revision {
        validate_source_revision(head)?;
    }
    validate_geo_records(&state.records)?;

    if state.snapshots.len() > MAX_RECORDS {
        return Err(resource_limit("snapshot count exceeds MAX_RECORDS"));
    }
    for (key, snapshot) in &state.snapshots {
        if key != &snapshot.id {
            return Err(invalid("snapshots", "Map key does not match snapshot ID"));
        }
        validate_snapshot_binding(snapshot)?;
        if let Some(head) = &state.head_revision {
            if snapshot.source_revision.document_uuid != head.document_uuid
                || snapshot.source_revision.operation_id > head.operation_id
            {
                return Err(invalid(
                    "snapshots.source_revision",
                    "Snapshot revision must belong to this document and not exceed the state head",
                ));
            }
        }
    }

    if let Some(head) = &state.head_revision {
        for revision in record_revisions(&state.records) {
            if revision > head.operation_id {
                return Err(invalid(
                    "records.revision",
                    "Record revision cannot be newer than the state head",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_geo_records(records: &GeoRecords) -> Result<(), GeoError> {
    let total_count = geo_record_count(records)?;
    if total_count > MAX_RECORDS {
        return Err(resource_limit("GeoRecords count exceeds MAX_RECORDS"));
    }

    for (key, record) in &records.sources {
        if key != &record.id {
            return Err(invalid("sources", "Map key does not match source ID"));
        }
        validate_source_record(record)?;
    }
    for (key, record) in &records.soils {
        if key != &record.id {
            return Err(invalid("soils", "Map key does not match soil ID"));
        }
        required_text(&record.name, "soils.name")?;
        validate_parameter(
            &record.parameters.unit_weight_kn_m3,
            ParameterUnit::KnPerM3,
            "soils.parameters.unit_weight_kn_m3",
            &records.sources,
            |value| {
                bounded_positive(
                    *value,
                    MAX_UNIT_WEIGHT_KN_M3,
                    "soils.parameters.unit_weight_kn_m3",
                )
            },
        )?;
        validate_parameter(
            &record.parameters.cohesion_kpa,
            ParameterUnit::Kpa,
            "soils.parameters.cohesion_kpa",
            &records.sources,
            |value| bounded_nonnegative(*value, MAX_PARAMETER_KPA, "soils.parameters.cohesion_kpa"),
        )?;
        validate_parameter(
            &record.parameters.friction_angle_deg,
            ParameterUnit::Degree,
            "soils.parameters.friction_angle_deg",
            &records.sources,
            |value| {
                if !value.is_finite() || !(0.0..90.0).contains(value) {
                    Err(invalid(
                        "soils.parameters.friction_angle_deg",
                        "Friction angle must be finite and in [0, 90) degrees",
                    ))
                } else {
                    Ok(())
                }
            },
        )?;
        validate_parameter(
            &record.parameters.elastic_modulus_kpa,
            ParameterUnit::Kpa,
            "soils.parameters.elastic_modulus_kpa",
            &records.sources,
            |value| bounded_positive(*value, MAX_PARAMETER_KPA, "soils.parameters.elastic_modulus_kpa"),
        )?;
        validate_parameter(
            &record.parameters.poisson_ratio,
            ParameterUnit::Dimensionless,
            "soils.parameters.poisson_ratio",
            &records.sources,
            |value| {
                if !value.is_finite() || !(-1.0..0.5).contains(value) {
                    Err(invalid(
                        "soils.parameters.poisson_ratio",
                        "Poisson ratio must be finite and in (-1, 0.5)",
                    ))
                } else {
                    Ok(())
                }
            },
        )?;
    }

    for (key, record) in &records.iges {
        if key != &record.id {
            return Err(invalid("iges", "Map key does not match IGE ID"));
        }
        required_text(&record.name, "iges.name")?;
        validate_source_ids(&record.source_ids, &records.sources, "iges.source_ids")?;
        if !records.soils.contains_key(&record.soil_id) {
            return Err(missing_reference("soil", record.soil_id.as_str()));
        }
    }

    for (key, record) in &records.boreholes {
        if key != &record.id {
            return Err(invalid("boreholes", "Map key does not match borehole ID"));
        }
        validate_coordinate3(record.collar_m, "boreholes.collar_m")?;
        optional_datum_label(&record.depth_datum, "boreholes.depth_datum")?;
        optional_text(record.drilling_date.as_deref(), "boreholes.drilling_date")?;
        optional_text(record.crs.as_deref(), "boreholes.crs")?;
        optional_datum_text(
            record.collar_vertical_datum.as_deref(),
            "boreholes.collar_vertical_datum",
        )?;
        validate_source_ids(&record.source_ids, &records.sources, "boreholes.source_ids")?;
        validate_source_ids(
            &record.survey_source_ids,
            &records.sources,
            "boreholes.survey_source_ids",
        )?;
        validate_source_ids(
            &record.evidence_source_ids,
            &records.sources,
            "boreholes.evidence_source_ids",
        )?;
        validate_parameter(
            &record.total_depth_m,
            ParameterUnit::Metre,
            "boreholes.total_depth_m",
            &records.sources,
            |value| bounded_positive(*value, MAX_ABS_COORDINATE_M, "boreholes.total_depth_m"),
        )?;
        if record.intervals.len() > MAX_RECORDS {
            return Err(resource_limit("Borehole interval count exceeds MAX_RECORDS"));
        }
        ensure_unique(
            &record.intervals.iter().map(|interval| &interval.id).collect::<Vec<_>>(),
            "boreholes.intervals.id",
        )?;
        let mut prior_bottom = 0.0;
        for interval in &record.intervals {
            bounded_nonnegative(
                interval.top_depth_m,
                MAX_ABS_COORDINATE_M,
                "boreholes.intervals.top_depth_m",
            )?;
            bounded_positive(
                interval.bottom_depth_m,
                MAX_ABS_COORDINATE_M,
                "boreholes.intervals.bottom_depth_m",
            )?;
            if interval.top_depth_m < prior_bottom || interval.bottom_depth_m <= interval.top_depth_m {
                return Err(invalid(
                    "boreholes.intervals",
                    "Intervals must be ordered, non-overlapping, and have positive depth",
                ));
            }
            prior_bottom = interval.bottom_depth_m;
            validate_source_ids(&interval.source_ids, &records.sources, "boreholes.intervals.source_ids")?;
            if !records.iges.contains_key(&interval.ige_id) {
                return Err(missing_reference("ige", interval.ige_id.as_str()));
            }
        }
        if let Some(total_depth) = record.total_depth_m.known() {
            if prior_bottom > *total_depth {
                return Err(invalid(
                    "boreholes.total_depth_m",
                    "Known total depth is shallower than a logged interval",
                ));
            }
        }
    }

    for (key, record) in &records.water_observations {
        if key != &record.id {
            return Err(invalid(
                "water_observations",
                "Map key does not match water observation ID",
            ));
        }
        validate_coordinate2(record.xy_m, "water_observations.xy_m")?;
        optional_text(record.observed_at.as_deref(), "water_observations.observed_at")?;
        validate_parameter(
            &record.elevation_abs_m,
            ParameterUnit::Metre,
            "water_observations.elevation_abs_m",
            &records.sources,
            |value| bounded_abs(*value, MAX_ABS_COORDINATE_M, "water_observations.elevation_abs_m"),
        )?;
    }

    for (key, record) in &records.models {
        if key != &record.id {
            return Err(invalid("models", "Map key does not match ground model ID"));
        }
        required_text(&record.name, "models.name")?;
        required_text(&record.frame.name, "models.frame.name")?;
        optional_text(record.frame.crs.as_deref(), "models.frame.crs")?;
        optional_text(
            record.frame.vertical_datum.as_deref(),
            "models.frame.vertical_datum",
        )?;
        if record.borehole_ids.len() > MAX_MODEL_WELLS {
            return Err(resource_limit("GroundModel borehole count exceeds MAX_MODEL_WELLS"));
        }
        if record.triangles.len() > MAX_TRIANGLES {
            return Err(resource_limit("GroundModel triangle count exceeds MAX_TRIANGLES"));
        }
        if record.contacts.len() > MAX_CONTACTS {
            return Err(resource_limit("GroundModel contact count exceeds MAX_CONTACTS"));
        }
        ensure_unique(&record.borehole_ids, "models.borehole_ids")?;
        ensure_unique(&record.section_ids, "models.section_ids")?;
        ensure_unique(
            &record.triangles.iter().map(|triangle| &triangle.id).collect::<Vec<_>>(),
            "models.triangles.id",
        )?;
        ensure_unique(
            &record.contacts.iter().map(|contact| &contact.id).collect::<Vec<_>>(),
            "models.contacts.id",
        )?;
        validate_source_ids(&record.source_ids, &records.sources, "models.source_ids")?;

        for borehole_id in &record.borehole_ids {
            if !records.boreholes.contains_key(borehole_id) {
                return Err(missing_reference("borehole", borehole_id.as_str()));
            }
        }
        let listed_boreholes: BTreeSet<_> = record.borehole_ids.iter().cloned().collect();
        for triangle in &record.triangles {
            validate_source_ids(&triangle.source_ids, &records.sources, "models.triangles.source_ids")?;
            for vertex in &triangle.vertices {
                if !listed_boreholes.contains(vertex) {
                    return Err(invalid(
                        "models.triangles.vertices",
                        "Triangle vertices must be listed in model.borehole_ids",
                    ));
                }
            }
        }
        for contact in &record.contacts {
            validate_source_ids(&contact.source_ids, &records.sources, "models.contacts.source_ids")?;
            optional_datum_text(
                contact.vertical_datum.as_deref(),
                "models.contacts.vertical_datum",
            )?;
            if !records.iges.contains_key(&contact.ige_id) {
                return Err(missing_reference("ige", contact.ige_id.as_str()));
            }
            for (borehole_id, elevation) in &contact.elevations_abs_m {
                if !listed_boreholes.contains(borehole_id) {
                    return Err(invalid(
                        "models.contacts.elevations_abs_m",
                        "Contact elevations must refer to listed model boreholes",
                    ));
                }
                validate_parameter(
                    elevation,
                    ParameterUnit::Metre,
                    "models.contacts.elevations_abs_m",
                    &records.sources,
                    |value| bounded_abs(*value, MAX_ABS_COORDINATE_M, "models.contacts.elevations_abs_m"),
                )?;
            }
        }
        for zone in &record.unknown_zones {
            required_text(&zone.reason, "models.unknown_zones.reason")?;
            if !(3..=MAX_SECTION_POINTS).contains(&zone.polygon_xy_m.len()) {
                return Err(invalid(
                    "models.unknown_zones.polygon_xy_m",
                    "Unknown zone polygon must contain 3..=MAX_SECTION_POINTS vertices",
                ));
            }
            for point in &zone.polygon_xy_m {
                validate_coordinate2(*point, "models.unknown_zones.polygon_xy_m")?;
            }
            validate_source_ids(&zone.source_ids, &records.sources, "models.unknown_zones.source_ids")?;
        }
        for section_id in &record.section_ids {
            let section = records.sections.get(section_id).ok_or_else(|| {
                missing_reference("section", section_id.as_str())
            })?;
            if section.model_id != record.id {
                return Err(invalid(
                    "models.section_ids",
                    "Referenced section belongs to a different model",
                ));
            }
        }
        validate_model_geometry(records, record)?;
    }

    for (key, record) in &records.sections {
        if key != &record.id {
            return Err(invalid("sections", "Map key does not match section ID"));
        }
        if !(2..=MAX_SECTION_POINTS).contains(&record.polyline_xy_m.len()) {
            return Err(invalid(
                "sections.polyline_xy_m",
                "Section polyline must contain 2..=MAX_SECTION_POINTS vertices",
            ));
        }
        for point in &record.polyline_xy_m {
            validate_coordinate2(*point, "sections.polyline_xy_m")?;
        }
        validate_source_ids(&record.source_ids, &records.sources, "sections.source_ids")?;
        if !records.models.contains_key(&record.model_id) {
            return Err(missing_reference("ground_model", record.model_id.as_str()));
        }
    }

    for (key, record) in &records.scenarios {
        if key != &record.id {
            return Err(invalid("scenarios", "Map key does not match scenario ID"));
        }
        required_text(&record.name, "scenarios.name")?;
        validate_source_ids(&record.source_ids, &records.sources, "scenarios.source_ids")?;
        if !records.models.contains_key(&record.model_id) {
            return Err(missing_reference("ground_model", record.model_id.as_str()));
        }
        match &record.water {
            WaterScenario::Unknown => {}
            WaterScenario::ExplicitDry { source_ids } => {
                if source_ids.is_empty() {
                    return Err(invalid(
                        "scenarios.water.source_ids",
                        "ExplicitDry requires at least one cited source",
                    ));
                }
                validate_source_ids(source_ids, &records.sources, "scenarios.water.source_ids")?;
            }
            WaterScenario::Observations { observation_ids } => {
                if observation_ids.is_empty() {
                    return Err(invalid(
                        "scenarios.water.observation_ids",
                        "Use Unknown when no water observations are selected",
                    ));
                }
                ensure_unique(observation_ids, "scenarios.water.observation_ids")?;
                for observation_id in observation_ids {
                    if !records.water_observations.contains_key(observation_id) {
                        return Err(missing_reference(
                            "water_observation",
                            observation_id.as_str(),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn geo_record_count(records: &GeoRecords) -> Result<usize, GeoError> {
    [
        records.sources.len(),
        records.soils.len(),
        records.iges.len(),
        records.boreholes.len(),
        records.models.len(),
        records.scenarios.len(),
        records.water_observations.len(),
        records.sections.len(),
    ]
    .into_iter()
    .try_fold(0usize, checked_add_record_count)
}

pub(crate) fn checked_add_record_count(total: usize, next: usize) -> Result<usize, GeoError> {
    total
        .checked_add(next)
        .ok_or_else(|| resource_limit("GeoRecords aggregate count overflow"))
}

fn validate_source_record(record: &SourceRecord) -> Result<(), GeoError> {
    required_text(&record.title, "sources.title")?;
    required_text(&record.reference, "sources.reference")?;
    optional_text(record.observed_at.as_deref(), "sources.observed_at")?;
    optional_text(record.source_page_or_record.as_deref(), "sources.source_page_or_record")?;
    optional_text(record.source_date.as_deref(), "sources.source_date")?;
    optional_text(record.source_revision.as_deref(), "sources.source_revision")?;
    if let Some(hash) = &record.content_sha256 {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(
                "sources.content_sha256",
                "Content hash must contain exactly 64 hexadecimal characters",
            ));
        }
    }
    Ok(())
}

fn validate_parameter<T>(
    parameter: &Parameter<T>,
    expected_unit: ParameterUnit,
    path: &str,
    sources: &BTreeMap<SourceId, SourceRecord>,
    validate_value: impl Fn(&T) -> Result<(), GeoError>,
) -> Result<(), GeoError> {
    match parameter {
        Parameter::Missing { reason } => required_text(reason, &format!("{path}.reason")),
        Parameter::Known { value, source_ids, metadata } => {
            if metadata.unit != expected_unit {
                return Err(invalid(
                    &format!("{path}.metadata.unit"),
                    &format!("Known value must declare unit {expected_unit:?}"),
                ));
            }
            validate_metadata(metadata, path)?;
            if source_ids.is_empty() {
                return Err(invalid(
                    &format!("{path}.source_ids"),
                    "Known values require at least one source ID",
                ));
            }
            validate_source_ids(source_ids, sources, &format!("{path}.source_ids"))?;
            validate_value(value)
        }
        Parameter::Conflicting { candidates } => {
            if candidates.is_empty() {
                return Err(invalid(path, "Conflicting values require at least one candidate"));
            }
            if candidates.len() > MAX_RECORDS {
                return Err(resource_limit("Parameter candidate count exceeds MAX_RECORDS"));
            }
            for candidate in candidates {
                if candidate.metadata.unit != expected_unit {
                    return Err(invalid(
                        &format!("{path}.candidates.metadata.unit"),
                        &format!("Candidate must declare unit {expected_unit:?}"),
                    ));
                }
                validate_metadata(&candidate.metadata, path)?;
                if candidate.source_ids.is_empty() {
                    return Err(invalid(
                        &format!("{path}.candidates.source_ids"),
                        "Every conflicting candidate requires at least one source ID",
                    ));
                }
                validate_source_ids(
                    &candidate.source_ids,
                    sources,
                    &format!("{path}.candidates.source_ids"),
                )?;
                validate_value(&candidate.value)?;
            }
            Ok(())
        }
    }
}

fn validate_metadata(metadata: &ParameterMetadata, path: &str) -> Result<(), GeoError> {
    optional_text(
        metadata.source_page_or_record.as_deref(),
        &format!("{path}.metadata.source_page_or_record"),
    )?;
    optional_text(
        metadata.source_date.as_deref(),
        &format!("{path}.metadata.source_date"),
    )?;
    optional_text(
        metadata.source_revision.as_deref(),
        &format!("{path}.metadata.source_revision"),
    )?;
    // engineering_verified is preserved as provenance metadata only. It does not
    // change validation status or imply independent numerical or normative approval.
    Ok(())
}

fn validate_source_ids(
    ids: &[SourceId],
    sources: &BTreeMap<SourceId, SourceRecord>,
    path: &str,
) -> Result<(), GeoError> {
    ensure_unique(ids, path)?;
    for id in ids {
        if !sources.contains_key(id) {
            return Err(missing_reference("source", id.as_str()));
        }
    }
    Ok(())
}

fn ensure_unique<T: Ord>(values: &[T], path: &str) -> Result<(), GeoError> {
    if values.len() > MAX_RECORDS {
        return Err(resource_limit("Reference list exceeds MAX_RECORDS"));
    }
    let set: BTreeSet<_> = values.iter().collect();
    if set.len() != values.len() {
        return Err(invalid(path, "Duplicate IDs are not allowed"));
    }
    Ok(())
}

fn validate_coordinate2(value: [f64; 2], path: &str) -> Result<(), GeoError> {
    for coordinate in value {
        bounded_abs(coordinate, MAX_ABS_COORDINATE_M, path)?;
    }
    Ok(())
}

fn validate_coordinate3(value: [f64; 3], path: &str) -> Result<(), GeoError> {
    for coordinate in value {
        bounded_abs(coordinate, MAX_ABS_COORDINATE_M, path)?;
    }
    Ok(())
}

fn bounded_positive(value: f64, maximum: f64, path: &str) -> Result<(), GeoError> {
    if !value.is_finite() || value <= 0.0 || value > maximum {
        Err(invalid(path, "Value must be finite, positive, and within its bound"))
    } else {
        Ok(())
    }
}

fn bounded_nonnegative(value: f64, maximum: f64, path: &str) -> Result<(), GeoError> {
    if !value.is_finite() || value < 0.0 || value > maximum {
        Err(invalid(path, "Value must be finite, nonnegative, and within its bound"))
    } else {
        Ok(())
    }
}

fn bounded_abs(value: f64, maximum: f64, path: &str) -> Result<(), GeoError> {
    if !value.is_finite() || value.abs() > maximum {
        Err(invalid(path, "Coordinate must be finite and within its absolute bound"))
    } else {
        Ok(())
    }
}

fn required_text(value: &str, path: &str) -> Result<(), GeoError> {
    if value.trim().is_empty() {
        Err(invalid(path, "Text value cannot be blank"))
    } else {
        Ok(())
    }
}

fn optional_text(value: Option<&str>, path: &str) -> Result<(), GeoError> {
    if value.is_some_and(|text| text.trim().is_empty()) {
        Err(invalid(path, "Optional text cannot be blank when present"))
    } else {
        Ok(())
    }
}

fn optional_datum_text(value: Option<&str>, path: &str) -> Result<(), GeoError> {
    if let Some(value) = value {
        validate_datum_text(value, path)?;
    }
    Ok(())
}

fn optional_datum_label(value: &str, path: &str) -> Result<(), GeoError> {
    // Empty is the persisted unknown/default state. Query logic decides whether
    // a non-BELOW_COLLAR convention can be used for measured log comparisons.
    if value.is_empty() {
        return Ok(());
    }
    validate_datum_text(value, path)
}

fn validate_datum_text(value: &str, path: &str) -> Result<(), GeoError> {
    if value.len() > MAX_DEPTH_DATUM_BYTES {
        return Err(invalid(
            path,
            "Datum label exceeds the configured byte limit",
        ));
    }
    required_text(value, path)
}

fn validate_source_revision(revision: &GeoSourceRevision) -> Result<(), GeoError> {
    if revision.document_uuid.trim().is_empty() || revision.document_uuid.len() > 128 {
        return Err(invalid(
            "head_revision.document_uuid",
            "Document UUID must be non-empty and at most 128 bytes",
        ));
    }
    if revision.operation_id == 0 {
        return Err(invalid(
            "head_revision.operation_id",
            "Operation ID must be positive",
        ));
    }
    Ok(())
}

fn record_revisions(records: &GeoRecords) -> Vec<u64> {
    records.sources.values().map(|record| record.revision)
        .chain(records.soils.values().map(|record| record.revision))
        .chain(records.iges.values().map(|record| record.revision))
        .chain(records.boreholes.values().map(|record| record.revision))
        .chain(records.models.values().map(|record| record.revision))
        .chain(records.scenarios.values().map(|record| record.revision))
        .chain(records.water_observations.values().map(|record| record.revision))
        .chain(records.sections.values().map(|record| record.revision))
        .collect()
}

fn missing_reference(entity: &str, id: &str) -> GeoError {
    GeoError::MissingReference { entity: entity.into(), id: id.into() }
}

fn resource_limit(reason: &str) -> GeoError {
    GeoError::ResourceLimit { reason: reason.into() }
}

fn invalid(path: &str, message: &str) -> GeoError {
    GeoError::InvalidInput { path: path.into(), message: message.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing_soil() -> GeoSoilRecord {
        GeoSoilRecord {
            id: GeoSoilId::new("soil-1").unwrap(),
            name: "Draft soil".into(),
            parameters: SoilParameters::default(),
            revision: 0,
        }
    }

    #[test]
    fn missing_parameters_are_valid_draft_values_and_remain_missing() {
        let mut state = GeoState::empty();
        let soil = missing_soil();
        assert!(soil.parameters.unit_weight_kn_m3.known().is_none());
        state.records.soils.insert(soil.id.clone(), soil.clone());
        validate_geo_state(&state).unwrap();
        assert!(matches!(
            &state.records.soils[&soil.id].parameters.unit_weight_kn_m3,
            Parameter::Missing { .. }
        ));
    }

    #[test]
    fn known_parameter_without_source_is_rejected() {
        let mut state = GeoState::empty();
        let mut soil = missing_soil();
        soil.parameters.unit_weight_kn_m3 = Parameter::Known {
            value: 18.0,
            source_ids: vec![],
            metadata: ParameterMetadata {
                unit: ParameterUnit::KnPerM3,
                ..ParameterMetadata::default()
            },
        };
        state.records.soils.insert(soil.id.clone(), soil);
        assert!(validate_geo_state(&state).is_err());
    }

    #[test]
    fn known_parameter_with_wrong_unit_is_rejected_even_when_finite() {
        let source = SourceRecord {
            id: SourceId::new("src-1").unwrap(),
            title: "Lab sheet".into(),
            origin: SourceOrigin::Measured,
            reference: "sample 1".into(),
            content_sha256: None,
            observed_at: None,
            revision: 0,
            source_page_or_record: None,
            source_date: None,
            source_revision: None,
        };
        let mut state = GeoState::empty();
        state.records.sources.insert(source.id.clone(), source);
        let mut soil = missing_soil();
        soil.parameters.unit_weight_kn_m3 = Parameter::Known {
            value: 18.0,
            source_ids: vec![SourceId::new("src-1").unwrap()],
            metadata: ParameterMetadata {
                unit: ParameterUnit::Kpa,
                ..ParameterMetadata::default()
            },
        };
        state.records.soils.insert(soil.id.clone(), soil);
        assert!(validate_geo_state(&state).is_err());
    }

    #[test]
    fn malformed_tagged_parameter_is_rejected() {
        let malformed = r#"{"status":"MISSING","reason":"not supplied","extra":1}"#;
        assert!(serde_json::from_str::<Parameter<f64>>(malformed).is_err());
    }

    #[test]
    fn duplicate_typed_record_map_keys_are_rejected() {
        let source = r#"{"id":"src-1","title":"Source","origin":"SYNTHETIC","reference":"fixture","content_sha256":null,"observed_at":null,"revision":0,"source_page_or_record":null,"source_date":null,"source_revision":null}"#;
        let json = format!(
            r#"{{"sources":{{"src-1":{source},"src-1":{source}}},"soils":{{}},"iges":{{}},"boreholes":{{}},"models":{{}},"scenarios":{{}},"water_observations":{{}},"sections":{{}}}}"#
        );
        assert!(serde_json::from_str::<GeoRecords>(&json).is_err());
    }

    #[test]
    fn aggregate_record_count_addition_rejects_overflow() {
        assert!(matches!(
            checked_add_record_count(usize::MAX, 1),
            Err(GeoError::ResourceLimit { .. })
        ));
    }

    #[test]
    fn empty_depth_datum_is_preserved_as_unknown_draft_metadata() {
        let mut state = GeoState::empty();
        let id = BoreholeId::new("BH-DRAFT").unwrap();
        state.records.boreholes.insert(
            id.clone(),
            Borehole {
                id: id.clone(),
                collar_m: [0.0, 0.0, 0.0],
                source_ids: vec![],
                intervals: vec![],
                revision: 0,
                total_depth_m: Parameter::Missing {
                    reason: "NOT_PROVIDED".into(),
                },
                drilling_date: None,
                crs: None,
                collar_vertical_datum: None,
                depth_datum: String::new(),
                survey_source_ids: vec![],
                evidence_source_ids: vec![],
            },
        );

        validate_geo_state(&state).unwrap();
        assert_eq!(state.records.boreholes[&id].depth_datum, "");
        state.records.boreholes.get_mut(&id).unwrap().depth_datum = "   ".into();
        assert!(validate_geo_state(&state).is_err());
    }

    #[test]
    fn blank_present_vertical_datum_labels_are_rejected() {
        let mut collar_datum = crate::synthetic_demo_state().unwrap();
        collar_datum
            .records
            .boreholes
            .values_mut()
            .next()
            .unwrap()
            .collar_vertical_datum = Some(" ".into());
        assert!(validate_geo_state(&collar_datum).is_err());

        let mut contact_datum = crate::synthetic_demo_state().unwrap();
        contact_datum
            .records
            .models
            .values_mut()
            .next()
            .unwrap()
            .contacts[0]
            .vertical_datum = Some(" ".into());
        assert!(validate_geo_state(&contact_datum).is_err());
    }
}
