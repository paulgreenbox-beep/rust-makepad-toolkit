use crate::ascii::{RawTag, preflight_ascii};
use crate::loss::{LossReport, LossSeverity};
use crate::units::DxfUnitChoice;
use drafting_core::{
    DraftStyle, DrawingId, EntityGeometry, ImportBatch, ImportEntity, ImportLayer,
    ImportSourceSpec, Point2, PolylineVertex, UnitResolution,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_IMPORTED_ENTITIES: usize = 20_000;
const MAX_IMPORTED_LAYERS: usize = 1_024;
const MAX_VERTICES_PER_POLYLINE: usize = 4_096;
const MAX_TOTAL_POLYLINE_VERTICES: usize = 65_536;
const PLANAR_TOLERANCE: f64 = 1e-12;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// Durable display name of the original source; no file is opened by this crate.
    pub source_name: String,
    /// Explicit user decision, required when `$INSUNITS` is absent, unitless, or unsupported.
    pub unit_override: Option<String>,
    /// Opt in to applying a preview that has critical reported losses.
    pub accept_critical_losses: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            source_name: "import.dxf".into(),
            unit_override: None,
            accept_critical_losses: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportPreview {
    pub source_hash: String,
    pub source_name: String,
    pub report: LossReport,
    pub source: Option<ImportSourceSpec>,
    pub layers: Vec<ImportLayer>,
    pub entities: Vec<ImportEntity>,
    pub can_apply: bool,
    pub decision_required: bool,
    #[serde(skip)]
    accept_critical_losses: bool,
}

impl ImportPreview {
    /// Build the atomic Core payload only after all required decisions are resolved.
    pub fn to_batch(&self, drawing_id: DrawingId) -> Result<ImportBatch, String> {
        if self.decision_required {
            return Err("import requires an explicit unit decision before apply".into());
        }
        if self.report.has_critical() && !self.accept_critical_losses {
            return Err(self.report.summary());
        }
        if !self.can_apply {
            return Err("import preview is not applicable".into());
        }
        let source = self
            .source
            .clone()
            .ok_or_else(|| "import preview has no resolved source metadata".to_string())?;
        source.validate().map_err(|error| error.to_string())?;
        Ok(ImportBatch {
            drawing_id,
            source,
            layers: self.layers.clone(),
            entities: self.entities.clone(),
        })
    }
}

/// Convert a bounded ASCII DXF into immutable command payloads. This function never
/// mutates `LabDocument`; the caller wraps the returned fields in one `ImportBatch`.
pub fn import_preview(bytes: &[u8], options: &ImportOptions) -> Result<ImportPreview, String> {
    validate_source_name(&options.source_name)?;
    let explicit_unit = options
        .unit_override
        .as_deref()
        .map(DxfUnitChoice::parse_name)
        .transpose()?;
    let (raw_tags, ascii_bytes) = preflight_ascii(bytes)?;
    let section_names = validate_document_structure(&raw_tags)?;

    // The preflight is intentionally ahead of CADCraft's parser: invalid numeric
    // strings are otherwise coerced to zero by that low-level reader.
    let parsed_tags = cadcraft_dxf::parse(ascii_bytes)
        .map_err(|error| format!("CADCraft DXF parse failed after preflight: {error}"))?;
    if parsed_tags.len() != raw_tags.len() {
        return Err("DXF tag count changed between bounded preflight and CADCraft parse".into());
    }
    let sections = cadcraft_dxf::sections(&parsed_tags);
    if sections.len() != section_names.len()
        || sections
            .iter()
            .zip(&section_names)
            .any(|(section, name)| !section.name.eq_ignore_ascii_case(name))
    {
        return Err("DXF section structure disagrees with CADCraft parse".into());
    }

    let mut report = LossReport::default();
    let source_hash = sha256_hex(bytes);
    report.source_sha256 = Some(source_hash.clone());
    validate_handles_global(&sections)?;
    let header = find_section(&sections, "HEADER");
    let declared_insunits = collect_header_units(header, &mut report)?;
    let unit_resolution = resolve_units(&declared_insunits, explicit_unit, &mut report)?;
    report.entity_count = find_section(&sections, "ENTITIES")
        .map_or(0, |section| cadcraft_dxf::records(&section.tags).len());
    report_unsupported_sections(&sections, &mut report);

    let Some(unit_resolution) = unit_resolution else {
        report.push(
            "unit_resolution_required",
            LossSeverity::Critical,
            "DXF coordinates were not converted; provide an explicit unit decision before apply",
            None,
            None,
        );
        return Ok(ImportPreview {
            source_hash,
            source_name: options.source_name.clone(),
            report,
            source: None,
            layers: Vec::new(),
            entities: Vec::new(),
            can_apply: false,
            decision_required: true,
            accept_critical_losses: options.accept_critical_losses,
        });
    };
    report.unit_resolution = Some(unit_resolution.clone());

    let mut layers = collect_layers(find_section(&sections, "TABLES"), &mut report)?;
    ensure_layer_zero(&mut layers);
    let mut layer_indices = layer_name_index(&layers)?;
    let mut entities = collect_entities(
        find_section(&sections, "ENTITIES"),
        &mut layers,
        &mut layer_indices,
        unit_resolution.scale_to_m,
        &mut report,
    )?;
    if layers.len() > MAX_IMPORTED_LAYERS {
        return Err(format!(
            "DXF exceeds the {MAX_IMPORTED_LAYERS} layer import budget"
        ));
    }
    if entities.len() > MAX_IMPORTED_ENTITIES {
        return Err(format!(
            "DXF exceeds the {MAX_IMPORTED_ENTITIES} entity import budget"
        ));
    }
    report.converted_count = entities.len();
    report.unsupported_count = report.entity_count.saturating_sub(report.converted_count);
    let source_name = options.source_name.clone();
    let source = ImportSourceSpec {
        name: source_name.clone(),
        sha256: source_hash.clone(),
        unit_resolution,
        report: serde_json::to_value(&report)
            .map_err(|error| format!("could not serialize DXF loss report: {error}"))?,
    };
    source.validate().map_err(|error| error.to_string())?;

    entities.shrink_to_fit();
    let can_apply = !report.has_critical() || options.accept_critical_losses;
    Ok(ImportPreview {
        source_hash,
        source_name,
        report,
        source: Some(source),
        layers,
        entities,
        can_apply,
        decision_required: false,
        accept_critical_losses: options.accept_critical_losses,
    })
}

fn validate_source_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 512 || name.contains('\0') {
        return Err("source name is empty, contains NUL or exceeds 512 bytes".into());
    }
    Ok(())
}

fn validate_document_structure(raw: &[RawTag]) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err("DXF contains no tags".into());
    }
    let mut names = Vec::new();
    let mut seen_sections = BTreeSet::new();
    let mut index = 0usize;
    let mut saw_eof = false;
    while index < raw.len() {
        let tag = raw
            .get(index)
            .ok_or_else(|| "DXF tag index overflow".to_string())?;
        if tag.code != 0 {
            return Err(format!(
                "line {}: expected SECTION or EOF outside a DXF section",
                tag.line
            ));
        }
        if tag.value.eq_ignore_ascii_case("EOF") {
            if index + 1 != raw.len() {
                return Err(format!("line {}: tags follow DXF EOF", tag.line));
            }
            saw_eof = true;
            break;
        }
        if !tag.value.eq_ignore_ascii_case("SECTION") {
            return Err(format!(
                "line {}: expected SECTION or EOF outside a DXF section",
                tag.line
            ));
        }
        let name_tag = raw
            .get(index + 1)
            .ok_or_else(|| "DXF SECTION has no name".to_string())?;
        if name_tag.code != 2 || name_tag.value.trim().is_empty() {
            return Err(format!(
                "line {}: DXF SECTION must be followed by group code 2 and a name",
                name_tag.line
            ));
        }
        let name = name_tag.value.trim().to_string();
        let key = name.to_ascii_uppercase();
        if !seen_sections.insert(key) {
            return Err(format!("DXF contains duplicate {name} sections"));
        }
        names.push(name.clone());
        if names.len() > 32 {
            return Err("DXF exceeds the 32 section import budget".into());
        }
        index += 2;
        let mut ended = false;
        while index < raw.len() {
            let inner = raw
                .get(index)
                .ok_or_else(|| "DXF tag index overflow".to_string())?;
            if inner.code == 0 && inner.value.eq_ignore_ascii_case("ENDSEC") {
                ended = true;
                index += 1;
                break;
            }
            if inner.code == 0
                && (inner.value.eq_ignore_ascii_case("SECTION")
                    || inner.value.eq_ignore_ascii_case("EOF"))
            {
                return Err(format!("line {}: section ended without ENDSEC", inner.line));
            }
            index += 1;
        }
        if !ended {
            return Err(format!("DXF {name} section is missing ENDSEC"));
        }
    }
    if !saw_eof {
        return Err("DXF is missing its final EOF tag".into());
    }
    Ok(names)
}

fn find_section<'a>(
    sections: &'a [cadcraft_dxf::Section],
    name: &str,
) -> Option<&'a cadcraft_dxf::Section> {
    sections
        .iter()
        .find(|section| section.name.eq_ignore_ascii_case(name))
}

fn collect_header_units(
    section: Option<&cadcraft_dxf::Section>,
    report: &mut LossReport,
) -> Result<Vec<i64>, String> {
    let Some(section) = section else {
        report.push(
            "missing_header",
            LossSeverity::Warning,
            "DXF has no HEADER section",
            None,
            None,
        );
        return Ok(Vec::new());
    };
    let mut declared = Vec::new();
    let mut unknown_seen = BTreeSet::new();
    let mut i = 0usize;
    while i < section.tags.len() {
        let tag = section
            .tags
            .get(i)
            .ok_or_else(|| "invalid HEADER tag index".to_string())?;
        if tag.code != 9 {
            report.push(
                "unmapped_header_tag",
                LossSeverity::Warning,
                format!("HEADER group {} is not imported", tag.code),
                None,
                Some(tag.code),
            );
            i += 1;
            continue;
        }
        let variable = tag.str();
        let value = section
            .tags
            .get(i + 1)
            .ok_or_else(|| format!("HEADER variable {variable} has no value"))?;
        if variable.eq_ignore_ascii_case("$INSUNITS") {
            if value.code != 70 {
                return Err("DXF $INSUNITS must use integer group code 70".into());
            }
            let code = value.i64();
            if !declared.contains(&code) {
                declared.push(code);
            }
        } else if !matches!(
            variable.to_ascii_uppercase().as_str(),
            "$ACADVER" | "$DWGCODEPAGE" | "$MEASUREMENT"
        ) {
            if unknown_seen.insert(variable.to_ascii_uppercase()) {
                report.push(
                    "unmapped_header_variable",
                    LossSeverity::Warning,
                    format!("HEADER variable {variable} is not imported"),
                    None,
                    Some(9),
                );
            }
        } else {
            report.push(
                "header_metadata_not_preserved",
                LossSeverity::Warning,
                format!("HEADER variable {variable} is not retained by the Lab model"),
                None,
                Some(9),
            );
        }
        i += 2;
    }
    if declared.len() > 1 {
        report.push(
            "unit_declaration_conflict",
            LossSeverity::Critical,
            "DXF has conflicting $INSUNITS declarations; an explicit override is required",
            None,
            Some(70),
        );
    }
    Ok(declared)
}

fn resolve_units(
    declarations: &[i64],
    choice: Option<DxfUnitChoice>,
    report: &mut LossReport,
) -> Result<Option<UnitResolution>, String> {
    let declared_code = if declarations.len() == 1 {
        declarations.first().copied()
    } else {
        None
    };
    let declared_unit = match declared_code {
        Some(0) => Some("unitless".to_string()),
        Some(code) => DxfUnitChoice::from_insunits(code)
            .ok()
            .flatten()
            .map(|unit| unit.name().to_string())
            .or_else(|| Some(format!("insunits:{code}"))),
        None if declarations.len() > 1 => Some("conflicting declarations".into()),
        None => None,
    };

    let resolved = match (declared_code, choice) {
        (Some(code), Some(selected)) => {
            let declared_choice = DxfUnitChoice::from_insunits(code).ok().flatten();
            match declared_choice {
                Some(declared) if declared == selected => {
                    (selected, "dxf_header_confirmed_by_user")
                }
                Some(declared) => {
                    report.push(
                        "unit_choice_conflict",
                        LossSeverity::Critical,
                        format!(
                            "DXF declares {}, but the user selected {}",
                            declared.name(),
                            selected.name()
                        ),
                        None,
                        Some(70),
                    );
                    (selected, "user_choice_override")
                }
                None if code == 0 => {
                    report.push(
                        "unitless_user_resolution",
                        LossSeverity::Warning,
                        format!(
                            "unitless DXF interpreted as {} by explicit user choice",
                            selected.name()
                        ),
                        None,
                        Some(70),
                    );
                    (selected, "user_choice_for_unitless_dxf")
                }
                None => {
                    report.push(
                        "unsupported_unit_override",
                        LossSeverity::Critical,
                        format!(
                            "DXF INSUNITS code {code} is unsupported; explicit choice {} was used",
                            selected.name()
                        ),
                        None,
                        Some(70),
                    );
                    (selected, "user_choice_override_unsupported_code")
                }
            }
        }
        (Some(code), None) if declarations.len() == 1 => match DxfUnitChoice::from_insunits(code) {
            Ok(Some(unit)) => (unit, "dxf_header"),
            Ok(None) => return Ok(None),
            Err(_) => {
                report.push("unsupported_unit_requires_override", LossSeverity::Critical,
                    format!("DXF INSUNITS code {code} is unsupported; an explicit unit override is required"), None, Some(70));
                return Ok(None);
            }
        },
        (_, Some(selected)) => {
            report.push("missing_unit_user_resolution", LossSeverity::Warning,
                format!("DXF has no single supported $INSUNITS declaration; interpreted as {} by explicit user choice", selected.name()), None, None);
            (
                selected,
                "user_choice_for_missing_or_conflicting_declaration",
            )
        }
        (_, None) => return Ok(None),
    };
    Ok(Some(UnitResolution {
        declared_unit,
        resolved_unit: resolved.0.name().into(),
        scale_to_m: resolved.0.meters_per_unit(),
        decision_source: resolved.1.into(),
    }))
}

fn validate_handles_global(sections: &[cadcraft_dxf::Section]) -> Result<(), String> {
    let mut handles = BTreeSet::new();
    for section in sections {
        if section.name.eq_ignore_ascii_case("HEADER") {
            continue;
        }
        for (kind, tags) in cadcraft_dxf::records(&section.tags) {
            let Some(handle) = single_str(&tags, 5)? else {
                continue;
            };
            validate_handle(&handle)?;
            if !handles.insert(handle.to_ascii_uppercase()) {
                return Err(format!(
                    "duplicate DXF object handle {handle:?} in {} record {kind}",
                    section.name
                ));
            }
        }
    }
    Ok(())
}

fn collect_layers(
    section: Option<&cadcraft_dxf::Section>,
    report: &mut LossReport,
) -> Result<Vec<ImportLayer>, String> {
    let mut layers = Vec::new();
    let Some(section) = section else {
        return Ok(layers);
    };
    if section
        .tags
        .iter()
        .take_while(|tag| tag.code != 0)
        .next()
        .is_some()
    {
        report.push(
            "orphan_table_tags",
            LossSeverity::Critical,
            "TABLES contains tags outside a table record",
            None,
            None,
        );
    }
    let mut current_table: Option<String> = None;
    let mut unsupported_tables = BTreeSet::new();
    let mut layer_table_count = 0usize;
    for (kind, tags) in cadcraft_dxf::records(&section.tags) {
        match kind.as_str() {
            "TABLE" => {
                current_table = single_str(&tags, 2)?.map(|name| name.to_ascii_uppercase());
                if current_table.as_deref() == Some("LAYER") {
                    layer_table_count += 1;
                } else if let Some(name) = &current_table {
                    unsupported_tables.insert(name.clone());
                }
            }
            "ENDTAB" => current_table = None,
            "LAYER" if current_table.as_deref() == Some("LAYER") => {
                layers.push(parse_layer(&tags, report)?);
                if layers.len() > MAX_IMPORTED_LAYERS {
                    return Err(format!(
                        "DXF exceeds the {MAX_IMPORTED_LAYERS} layer import budget"
                    ));
                }
            }
            other if current_table.as_deref() == Some("LAYER") => {
                report.push(
                    "unsupported_layer_record",
                    LossSeverity::Critical,
                    format!("LAYER table record {other} is not imported"),
                    None,
                    None,
                );
            }
            other if current_table.is_some() => {
                if !matches!(other, "TABLE" | "ENDTAB") {
                    unsupported_tables.insert(current_table.clone().unwrap_or_default());
                }
            }
            other => report.push(
                "orphan_table_record",
                LossSeverity::Critical,
                format!("TABLES record {other} is not inside a table"),
                None,
                None,
            ),
        }
    }
    if layer_table_count > 1 {
        return Err("DXF contains duplicate LAYER tables".into());
    }
    for table in unsupported_tables {
        report.push(
            "unsupported_table",
            LossSeverity::Warning,
            format!("DXF {table} table is not imported"),
            None,
            None,
        );
    }
    validate_and_report_layer_tags(section, report)?;
    let mut seen = BTreeSet::new();
    for layer in &layers {
        if !seen.insert(layer.name.to_lowercase()) {
            return Err(format!(
                "DXF contains duplicate layer name {:?}",
                layer.name
            ));
        }
    }
    Ok(layers)
}

fn validate_and_report_layer_tags(
    section: &cadcraft_dxf::Section,
    report: &mut LossReport,
) -> Result<(), String> {
    let mut current_table: Option<String> = None;
    for (kind, tags) in cadcraft_dxf::records(&section.tags) {
        match kind.as_str() {
            "TABLE" => current_table = single_str(&tags, 2)?.map(|name| name.to_ascii_uppercase()),
            "ENDTAB" => current_table = None,
            "LAYER" if current_table.as_deref() == Some("LAYER") => {
                check_unmapped_tags(&tags, &[2, 5, 6, 62, 70, 100, 330, 370, 420], None, report);
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_layer(tags: &[cadcraft_dxf::Tag], report: &mut LossReport) -> Result<ImportLayer, String> {
    let name = single_str(tags, 2)?
        .ok_or_else(|| "DXF LAYER record is missing group 2 name".to_string())?;
    if name.trim().is_empty() || name.len() > 512 || name.contains('\0') {
        return Err("DXF layer name is empty or exceeds the 512-byte core limit".into());
    }
    let flags = single_i64(tags, 70)?.unwrap_or(0);
    let aci = single_i64(tags, 62)?.unwrap_or(7);
    let true_color = single_i64(tags, 420)?;
    if true_color.is_none() && !matches!(aci.unsigned_abs(), 1..=7) {
        report.push(
            "aci_color_approximated",
            LossSeverity::Warning,
            format!("layer {name:?} ACI color {aci} is approximated by the limited v0.1 palette"),
            None,
            Some(62),
        );
    }
    let color = decode_color(aci, true_color)?;
    let layerweight = single_i64(tags, 370)?;
    if layerweight.is_some_and(|width| width >= 0) {
        report.push(
            "layer_lineweight_not_preserved",
            LossSeverity::Warning,
            format!("lineweight on layer {name:?} is not represented by DraftStyle"),
            None,
            Some(370),
        );
    }
    if single_str(tags, 6)?.is_some() {
        report.push(
            "layer_linetype_not_preserved",
            LossSeverity::Warning,
            format!("linetype on layer {name:?} is not represented by DraftStyle"),
            None,
            Some(6),
        );
    }
    if tags.iter().any(|tag| tag.code == 330) {
        report.push(
            "layer_owner_reference_not_preserved",
            LossSeverity::Warning,
            format!("owner reference on layer {name:?} is not imported"),
            None,
            Some(330),
        );
    }
    Ok(ImportLayer {
        name,
        visible: flags & 1 == 0 && aci >= 0,
        locked: flags & 4 != 0,
        style: DraftStyle {
            color,
            ..DraftStyle::default()
        },
    })
}

fn ensure_layer_zero(layers: &mut Vec<ImportLayer>) {
    if !layers
        .iter()
        .any(|layer| layer.name.eq_ignore_ascii_case("0"))
    {
        layers.push(ImportLayer {
            name: "0".into(),
            visible: true,
            locked: false,
            style: DraftStyle::default(),
        });
    }
}

fn layer_name_index(layers: &[ImportLayer]) -> Result<BTreeMap<String, usize>, String> {
    let mut index = BTreeMap::new();
    for (position, layer) in layers.iter().enumerate() {
        let key = layer.name.to_lowercase();
        if index.insert(key, position).is_some() {
            return Err(format!("duplicate DXF layer name {:?}", layer.name));
        }
    }
    Ok(index)
}

fn collect_entities(
    section: Option<&cadcraft_dxf::Section>,
    layers: &mut Vec<ImportLayer>,
    layer_indices: &mut BTreeMap<String, usize>,
    scale_to_m: f64,
    report: &mut LossReport,
) -> Result<Vec<ImportEntity>, String> {
    let mut output = Vec::new();
    let Some(section) = section else {
        return Ok(output);
    };
    if section
        .tags
        .iter()
        .take_while(|tag| tag.code != 0)
        .next()
        .is_some()
    {
        report.push(
            "orphan_entity_tags",
            LossSeverity::Critical,
            "ENTITIES contains tags before its first record",
            None,
            None,
        );
    }
    let records = cadcraft_dxf::records(&section.tags);
    if records.len() > MAX_IMPORTED_ENTITIES {
        return Err(format!(
            "DXF exceeds the {MAX_IMPORTED_ENTITIES} entity import budget"
        ));
    }
    let mut handles = BTreeSet::new();
    let mut total_vertices = 0usize;
    for (kind, tags) in records {
        let handle = single_str(&tags, 5)?;
        if let Some(value) = &handle {
            validate_handle(value)?;
            if !handles.insert(value.to_ascii_uppercase()) {
                return Err(format!("duplicate DXF entity handle {value:?}"));
            }
        }
        if is_paper_space(&tags)? {
            report.push(
                "paper_space_entity_not_imported",
                LossSeverity::Critical,
                format!("{kind} entity in paper space is not imported"),
                handle.clone(),
                Some(67),
            );
            check_unmapped_tags(
                &tags,
                &[5, 8, 60, 62, 67, 100, 330, 370, 410, 420],
                handle.clone(),
                report,
            );
            continue;
        }
        let Some(allowed) = entity_allowed_codes(&kind) else {
            report.push(
                "unsupported_entity",
                LossSeverity::Critical,
                format!("DXF entity {kind} is outside the v0.1 import set"),
                handle.clone(),
                Some(0),
            );
            check_unmapped_tags(
                &tags,
                &[5, 8, 60, 62, 67, 100, 330, 370, 410, 420],
                handle.clone(),
                report,
            );
            continue;
        };
        check_unmapped_tags(&tags, allowed, handle.clone(), report);
        if has_non_planar_coordinates(&kind, &tags)? {
            report.push(
                "non_planar_entity_not_imported",
                LossSeverity::Critical,
                format!("{kind} entity is not in the XY plane"),
                handle,
                None,
            );
            continue;
        }
        if has_entity_visibility_flag(&tags)? {
            report.push(
                "entity_visibility_not_preserved",
                LossSeverity::Critical,
                format!("invisible {kind} entity has no v0.1 entity visibility field"),
                handle.clone(),
                Some(60),
            );
        }
        report_entity_style_losses(&kind, &tags, handle.clone(), report)?;
        if kind == "LWPOLYLINE"
            && tags
                .iter()
                .any(|tag| matches!(tag.code, 40 | 41 | 43) && tag.f64() != 0.0)
        {
            report.push(
                "polyline_width_not_preserved",
                LossSeverity::Critical,
                "LWPOLYLINE width is not represented by analytic centerline geometry",
                handle.clone(),
                Some(40),
            );
        }
        let layer_name = single_str(&tags, 8)?.unwrap_or_else(|| "0".into());
        let layer_key = layer_name.to_lowercase();
        let layer_index = if let Some(index) = layer_indices.get(&layer_key) {
            *index
        } else {
            if layers.len() >= MAX_IMPORTED_LAYERS {
                return Err(format!(
                    "DXF exceeds the {MAX_IMPORTED_LAYERS} layer import budget"
                ));
            }
            report.push("missing_layer_record", LossSeverity::Warning, format!("entity refers to absent layer {layer_name:?}; a default layer record was synthesized"), handle.clone(), Some(8));
            let index = layers.len();
            layers.push(ImportLayer {
                name: layer_name.clone(),
                visible: true,
                locked: false,
                style: DraftStyle::default(),
            });
            layer_indices.insert(layer_key, index);
            index
        };
        let geometry = match parse_entity_geometry(&kind, &tags, scale_to_m) {
            Ok(geometry) => geometry,
            Err(message) if message.starts_with("drop:") => {
                report.push(
                    "unsupported_entity_geometry",
                    LossSeverity::Critical,
                    message.trim_start_matches("drop:"),
                    handle,
                    None,
                );
                continue;
            }
            Err(message) => return Err(message),
        };
        if let EntityGeometry::Polyline { vertices, .. } = &geometry {
            total_vertices = total_vertices
                .checked_add(vertices.len())
                .ok_or_else(|| "DXF vertex count overflow".to_string())?;
            if vertices.len() > MAX_VERTICES_PER_POLYLINE
                || total_vertices > MAX_TOTAL_POLYLINE_VERTICES
            {
                return Err("DXF exceeds the bounded polyline vertex budget".into());
            }
        }
        geometry
            .validate(drafting_core::GeometryLimits::default())
            .map_err(|error| format!("invalid {kind} geometry: {error}"))?;
        // Core resolves import entity layer names exactly, so retain the table's spelling.
        let layer = layers
            .get(layer_index)
            .ok_or_else(|| "internal layer index is invalid".to_string())?;
        let style = entity_style(&tags, layer.style.clone())?;
        style
            .validate()
            .map_err(|error| format!("invalid {kind} style: {error}"))?;
        output.push(ImportEntity {
            geometry,
            layer_name: layer.name.clone(),
            style,
            source_handle: handle,
        });
    }
    Ok(output)
}

fn entity_allowed_codes(kind: &str) -> Option<&'static [i32]> {
    const COMMON: &[i32] = &[5, 8, 60, 62, 67, 100, 330, 370, 410, 420];
    const LINE: &[i32] = &[
        5, 8, 10, 11, 20, 21, 30, 31, 60, 62, 67, 100, 210, 220, 230, 330, 370, 410, 420,
    ];
    const CIRCLE: &[i32] = &[
        5, 8, 10, 20, 30, 39, 40, 60, 62, 67, 100, 210, 220, 230, 330, 370, 410, 420,
    ];
    const ARC: &[i32] = &[
        5, 8, 10, 20, 30, 39, 40, 50, 51, 60, 62, 67, 100, 210, 220, 230, 330, 370, 410, 420,
    ];
    const LWPOLYLINE: &[i32] = &[
        5, 8, 10, 20, 38, 40, 41, 42, 43, 60, 62, 67, 70, 90, 100, 210, 220, 230, 330, 370, 410,
        420,
    ];
    let _ = COMMON;
    match kind {
        "LINE" => Some(LINE),
        "CIRCLE" => Some(CIRCLE),
        "ARC" => Some(ARC),
        "LWPOLYLINE" => Some(LWPOLYLINE),
        _ => None,
    }
}

fn parse_entity_geometry(
    kind: &str,
    tags: &[cadcraft_dxf::Tag],
    scale: f64,
) -> Result<EntityGeometry, String> {
    let number = |code| required_f64(tags, code, kind);
    let point = |x_code, y_code| -> Result<Point2, String> {
        let x = number(x_code)? * scale;
        let y = number(y_code)? * scale;
        if !x.is_finite() || !y.is_finite() {
            return Err(format!("{kind} coordinate conversion overflow"));
        }
        Ok(Point2::new(x, y))
    };
    match kind {
        "LINE" => Ok(EntityGeometry::Line {
            start: point(10, 20)?,
            end: point(11, 21)?,
        }),
        "CIRCLE" => {
            let radius = number(40)? * scale;
            if !radius.is_finite() {
                return Err("CIRCLE radius conversion overflow".into());
            }
            Ok(EntityGeometry::Circle {
                center: point(10, 20)?,
                radius,
            })
        }
        "ARC" => {
            let radius = number(40)? * scale;
            if !radius.is_finite() {
                return Err("ARC radius conversion overflow".into());
            }
            let start_degrees = number(50)?;
            let end_degrees = number(51)?;
            let sweep_degrees = (end_degrees - start_degrees).rem_euclid(360.0);
            if sweep_degrees <= f64::EPSILON {
                return Err(
                    "drop:full-circle ARC is ambiguous; use CIRCLE for a complete revolution"
                        .into(),
                );
            }
            let start_angle = start_degrees.to_radians().rem_euclid(std::f64::consts::TAU);
            let sweep_angle = sweep_degrees.to_radians();
            Ok(EntityGeometry::Arc {
                center: point(10, 20)?,
                radius,
                start_angle,
                sweep_angle,
            })
        }
        "LWPOLYLINE" => parse_polyline(tags, scale),
        other => Err(format!("unsupported DXF entity {other}")),
    }
}

fn parse_polyline(tags: &[cadcraft_dxf::Tag], scale: f64) -> Result<EntityGeometry, String> {
    let declared_count = single_i64(tags, 90)?
        .ok_or_else(|| "LWPOLYLINE is missing group 90 vertex count".to_string())?;
    if declared_count < 2 || declared_count as usize > MAX_VERTICES_PER_POLYLINE {
        return Err("LWPOLYLINE vertex count is outside the supported range 2..=4096".into());
    }
    let flags = single_i64(tags, 70)?.unwrap_or(0);
    if flags & !1 != 0 {
        return Err("drop:LWPOLYLINE fit, mesh, 3D, and polyface flags are unsupported".into());
    }
    let mut vertices = Vec::with_capacity(declared_count as usize);
    let mut current_x: Option<f64> = None;
    let mut current_y: Option<f64> = None;
    let mut current_bulge = 0.0;
    let mut bulge_seen = false;
    for tag in tags {
        match tag.code {
            10 => {
                if let Some(x) = current_x.take() {
                    let y = current_y
                        .take()
                        .ok_or_else(|| "LWPOLYLINE vertex is missing group 20".to_string())?;
                    vertices.push(PolylineVertex::new(
                        scale_point(x, y, scale)?,
                        current_bulge,
                    ));
                    current_bulge = 0.0;
                    bulge_seen = false;
                }
                current_x = Some(tag.f64());
            }
            20 => {
                if current_x.is_none() || current_y.is_some() {
                    return Err("LWPOLYLINE group 20 is misplaced or duplicated".into());
                }
                current_y = Some(tag.f64());
            }
            42 => {
                if current_x.is_none() || current_y.is_none() || bulge_seen {
                    return Err(
                        "LWPOLYLINE group 42 must occur once after a complete vertex".into(),
                    );
                }
                current_bulge = tag.f64();
                bulge_seen = true;
            }
            _ => {}
        }
    }
    if let Some(x) = current_x {
        let y =
            current_y.ok_or_else(|| "LWPOLYLINE final vertex is missing group 20".to_string())?;
        vertices.push(PolylineVertex::new(
            scale_point(x, y, scale)?,
            current_bulge,
        ));
    }
    if vertices.len() != declared_count as usize {
        return Err("LWPOLYLINE group 90 count does not match its vertices".into());
    }
    Ok(EntityGeometry::Polyline {
        vertices,
        closed: flags & 1 != 0,
    })
}

fn scale_point(x: f64, y: f64, scale: f64) -> Result<Point2, String> {
    let point = Point2::new(x * scale, y * scale);
    if !point.is_finite() {
        return Err("DXF coordinate conversion overflow".into());
    }
    Ok(point)
}

fn required_f64(tags: &[cadcraft_dxf::Tag], code: i32, kind: &str) -> Result<f64, String> {
    let value = single_f64(tags, code)?
        .ok_or_else(|| format!("{kind} is missing required group {code}"))?;
    if !value.is_finite() {
        return Err(format!("{kind} group {code} is non-finite"));
    }
    Ok(value)
}

fn has_non_planar_coordinates(kind: &str, tags: &[cadcraft_dxf::Tag]) -> Result<bool, String> {
    let z_codes: &[i32] = match kind {
        "LINE" => &[30, 31],
        "CIRCLE" | "ARC" => &[30],
        "LWPOLYLINE" => &[38],
        _ => &[],
    };
    for code in z_codes {
        if single_f64(tags, *code)?.is_some_and(|z| z.abs() > PLANAR_TOLERANCE) {
            return Ok(true);
        }
    }
    let normal = [210, 220, 230]
        .map(|code| single_f64(tags, code))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let actual = [
        normal[0].unwrap_or(0.0),
        normal[1].unwrap_or(0.0),
        normal[2].unwrap_or(1.0),
    ];
    Ok((actual[0]).abs() > PLANAR_TOLERANCE
        || actual[1].abs() > PLANAR_TOLERANCE
        || (actual[2] - 1.0).abs() > PLANAR_TOLERANCE)
}

fn is_paper_space(tags: &[cadcraft_dxf::Tag]) -> Result<bool, String> {
    let paper_flag = single_i64(tags, 67)?.unwrap_or(0);
    let layout = single_str(tags, 410)?;
    Ok(paper_flag != 0 || layout.is_some_and(|name| !name.eq_ignore_ascii_case("Model")))
}

fn has_entity_visibility_flag(tags: &[cadcraft_dxf::Tag]) -> Result<bool, String> {
    Ok(single_i64(tags, 60)?.unwrap_or(0) != 0)
}

fn report_entity_style_losses(
    kind: &str,
    tags: &[cadcraft_dxf::Tag],
    handle: Option<String>,
    report: &mut LossReport,
) -> Result<(), String> {
    let aci = single_i64(tags, 62)?;
    let true_color = single_i64(tags, 420)?;
    if true_color.is_none() && aci.is_some_and(|value| !matches!(value.unsigned_abs(), 1..=7 | 256))
    {
        report.push(
            "aci_color_approximated",
            LossSeverity::Warning,
            format!(
                "{kind} ACI color uses by-block or an unmapped palette entry; layer color was used"
            ),
            handle.clone(),
            Some(62),
        );
    }
    if single_i64(tags, 370)?.is_some_and(|width| width >= 0) {
        report.push(
            "entity_lineweight_not_preserved",
            LossSeverity::Warning,
            format!("lineweight on {kind} is not represented by DraftStyle"),
            handle.clone(),
            Some(370),
        );
    }
    if tags.iter().any(|tag| tag.code == 330) {
        report.push(
            "entity_owner_reference_not_preserved",
            LossSeverity::Warning,
            format!("owner reference on {kind} is not imported"),
            handle,
            Some(330),
        );
    }
    Ok(())
}

fn entity_style(tags: &[cadcraft_dxf::Tag], layer_style: DraftStyle) -> Result<DraftStyle, String> {
    let aci = single_i64(tags, 62)?;
    let true_color = single_i64(tags, 420)?;
    if aci.is_none() && true_color.is_none() {
        return Ok(layer_style);
    }
    if true_color.is_none() && matches!(aci, Some(0 | 256)) {
        return Ok(layer_style);
    }
    let color = decode_color(aci.unwrap_or(256), true_color)?;
    Ok(DraftStyle {
        color,
        ..DraftStyle::default()
    })
}

fn decode_color(aci: i64, true_color: Option<i64>) -> Result<[f32; 4], String> {
    if let Some(value) = true_color {
        if !(0..=0xFF_FFFF).contains(&value) {
            return Err("DXF true color must be a 24-bit RGB value".into());
        }
        return Ok([
            ((value >> 16) & 0xFF) as f32 / 255.0,
            ((value >> 8) & 0xFF) as f32 / 255.0,
            (value & 0xFF) as f32 / 255.0,
            1.0,
        ]);
    }
    let index = aci.unsigned_abs();
    let rgb = match index {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 | 0 | 256 => [255, 255, 255],
        _ => [255, 255, 255],
    };
    Ok([
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    ])
}

fn validate_handle(handle: &str) -> Result<(), String> {
    if handle.is_empty()
        || handle.len() > 64
        || !handle.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("DXF entity handle must be 1..=64 hexadecimal ASCII characters".into());
    }
    Ok(())
}

fn check_unmapped_tags(
    tags: &[cadcraft_dxf::Tag],
    allowed: &[i32],
    handle: Option<String>,
    report: &mut LossReport,
) {
    let has_xdata = tags.iter().any(|tag| (1000..=1071).contains(&tag.code));
    if has_xdata {
        report.push(
            "xdata_not_preserved",
            LossSeverity::Critical,
            "DXF XDATA is not imported",
            handle.clone(),
            Some(1001),
        );
    }
    for tag in tags {
        if (1000..=1071).contains(&tag.code) || allowed.contains(&tag.code) {
            continue;
        }
        report.push(
            "unsupported_tag",
            LossSeverity::Critical,
            format!("group code {} is not mapped", tag.code),
            handle.clone(),
            Some(tag.code),
        );
    }
}

fn report_unsupported_sections(sections: &[cadcraft_dxf::Section], report: &mut LossReport) {
    for section in sections {
        if matches!(
            section.name.to_ascii_uppercase().as_str(),
            "HEADER" | "TABLES" | "ENTITIES"
        ) {
            continue;
        }
        if section.name.eq_ignore_ascii_case("THUMBNAILIMAGE") {
            report.push(
                "thumbnail_not_preserved",
                LossSeverity::Warning,
                "DXF thumbnail is not imported",
                None,
                None,
            );
        } else {
            report.push(
                "unsupported_section",
                LossSeverity::Critical,
                format!("DXF {} section is not imported", section.name),
                None,
                None,
            );
        }
    }
}

fn single_tag<'a>(
    tags: &'a [cadcraft_dxf::Tag],
    code: i32,
) -> Result<Option<&'a cadcraft_dxf::Tag>, String> {
    let mut matches = tags.iter().filter(|tag| tag.code == code);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(format!("DXF record contains duplicate group {code}"));
    }
    Ok(first)
}

fn single_str(tags: &[cadcraft_dxf::Tag], code: i32) -> Result<Option<String>, String> {
    Ok(single_tag(tags, code)?.map(cadcraft_dxf::Tag::str))
}

fn single_f64(tags: &[cadcraft_dxf::Tag], code: i32) -> Result<Option<f64>, String> {
    Ok(single_tag(tags, code)?.map(cadcraft_dxf::Tag::f64))
}

fn single_i64(tags: &[cadcraft_dxf::Tag], code: i32) -> Result<Option<i64>, String> {
    Ok(single_tag(tags, code)?.map(cadcraft_dxf::Tag::i64))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_dxf(units: Option<i64>, entity: &str) -> Vec<u8> {
        let mut text = String::from("0\nSECTION\n2\nHEADER\n");
        if let Some(code) = units {
            text.push_str(&format!("9\n$INSUNITS\n70\n{code}\n"));
        }
        text.push_str("0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
        text.push_str(entity);
        text.push_str("0\nENDSEC\n0\nEOF\n");
        text.into_bytes()
    }

    fn dxf_with_layer_table(units: Option<i64>, layer_names: &[&str], entity: &str) -> Vec<u8> {
        let mut text = String::from("0\nSECTION\n2\nHEADER\n");
        if let Some(code) = units {
            text.push_str(&format!("9\n$INSUNITS\n70\n{code}\n"));
        }
        text.push_str("0\nENDSEC\n0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n");
        text.push_str(&format!("70\n{}\n", layer_names.len()));
        for (index, name) in layer_names.iter().enumerate() {
            text.push_str(&format!(
                "0\nLAYER\n5\n{:X}\n2\n{name}\n70\n0\n62\n7\n6\nCONTINUOUS\n",
                index + 1
            ));
        }
        text.push_str("0\nENDTAB\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
        text.push_str(entity);
        text.push_str("0\nENDSEC\n0\nEOF\n");
        text.into_bytes()
    }

    #[test]
    fn preview_uses_canonical_table_layer_for_core_apply() {
        let bytes = dxf_with_layer_table(
            Some(6),
            &["Walls"],
            "0\nLINE\n5\nA1\n8\nwalls\n10\n1\n20\n2\n11\n3\n21\n4\n",
        );
        let preview = import_preview(&bytes, &ImportOptions::default())
            .expect("case-insensitive DXF layer reference should resolve");
        assert!(preview.can_apply);
        assert_eq!(preview.entities[0].layer_name, "Walls");

        let mut document = drafting_core::LabDocument::new();
        let receipt = document
            .apply(drafting_core::CommandEnvelope {
                command_id: "case-insensitive-layer-import".into(),
                expected_revision: document.revision(),
                operations: vec![drafting_core::DraftOperation::Import {
                    batch: preview
                        .to_batch(DrawingId::new(1))
                        .expect("applicable preview should form a batch"),
                }],
            })
            .expect("Core should apply the previewed batch");
        let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
        let entity = snapshot
            .entities
            .get(&receipt.created_ids[0])
            .expect("Core should retain the imported entity");
        let layer = snapshot
            .layers
            .get(&entity.layer_id)
            .expect("imported entity should reference its layer");
        assert_eq!(layer.name, "Walls");
        assert_eq!(
            entity.geometry,
            EntityGeometry::Line {
                start: Point2::new(1.0, 2.0),
                end: Point2::new(3.0, 4.0),
            }
        );
    }

    #[test]
    fn rejects_layer_table_names_that_only_differ_by_case() {
        let bytes = dxf_with_layer_table(
            Some(6),
            &["Walls", "walls"],
            "0\nLINE\n5\nA1\n8\nwalls\n10\n1\n20\n2\n11\n3\n21\n4\n",
        );
        let error = import_preview(&bytes, &ImportOptions::default())
            .expect_err("ambiguous layer names must not be silently merged");
        assert!(error.contains("duplicate layer name"), "{error}");
    }

    #[test]
    fn imports_line_with_declared_millimetres_to_metres_and_durable_source() {
        let bytes = minimal_dxf(
            Some(4),
            "0\nLINE\n5\nA1\n8\n0\n10\n1000\n20\n2000\n11\n3000\n21\n4000\n",
        );
        let preview =
            import_preview(&bytes, &ImportOptions::default()).expect("valid line should preview");
        assert_eq!(preview.entities.len(), 1);
        assert_eq!(preview.layers.len(), 1);
        let source = preview
            .source
            .as_ref()
            .expect("declared units resolve source metadata");
        assert_eq!(source.sha256, preview.source_hash);
        assert_eq!(source.name, "import.dxf");
        assert_eq!(source.unit_resolution.resolved_unit, "millimetre");
        assert_eq!(source.unit_resolution.scale_to_m, 0.001);
        assert_eq!(preview.entities[0].source_handle.as_deref(), Some("A1"));
        assert!(preview.can_apply);
        assert_eq!(
            preview.entities[0].geometry,
            EntityGeometry::Line {
                start: Point2::new(1.0, 2.0),
                end: Point2::new(3.0, 4.0)
            }
        );
        assert_eq!(
            preview.to_batch(DrawingId::new(1)).unwrap().entities.len(),
            1
        );
    }

    #[test]
    fn previews_missing_and_unitless_units_without_applying_or_inventing_a_scale() {
        let bytes = minimal_dxf(None, "");
        let preview = import_preview(&bytes, &ImportOptions::default()).unwrap();
        assert!(preview.decision_required);
        assert!(!preview.can_apply);
        assert!(preview.source.is_none());
        assert!(preview.to_batch(DrawingId::new(1)).is_err());
        let bytes = minimal_dxf(Some(0), "");
        let preview = import_preview(&bytes, &ImportOptions::default()).unwrap();
        assert!(preview.decision_required);
        let options = ImportOptions {
            unit_override: Some("mm".into()),
            ..Default::default()
        };
        let resolved = import_preview(&bytes, &options).unwrap();
        assert!(!resolved.decision_required);
        assert!(resolved.can_apply);
        assert_eq!(resolved.source.unwrap().unit_resolution.scale_to_m, 0.001);
    }

    #[test]
    fn reports_and_gates_conflicting_explicit_unit_choice() {
        let bytes = minimal_dxf(Some(4), "");
        let options = ImportOptions {
            source_name: "unit-conflict.dxf".into(),
            unit_override: Some("inch".into()),
            ..Default::default()
        };
        let preview =
            import_preview(&bytes, &options).expect("preview should expose unit conflict");
        assert!(preview.report.has_critical());
        assert!(!preview.can_apply);
        assert!(preview.to_batch(DrawingId::new(1)).is_err());
        assert_eq!(
            preview.source.as_ref().unwrap().unit_resolution.scale_to_m,
            0.0254
        );
        assert!(
            preview
                .report
                .items
                .iter()
                .any(|item| item.code == "unit_choice_conflict")
        );
        let accepted = import_preview(
            &bytes,
            &ImportOptions {
                accept_critical_losses: true,
                ..options
            },
        )
        .unwrap();
        assert!(accepted.can_apply);
        assert!(accepted.to_batch(DrawingId::new(1)).is_ok());
    }

    #[test]
    fn duplicate_handles_are_rejected_case_insensitively() {
        let entities = "0\nLINE\n5\nA1\n8\n0\n10\n0\n20\n0\n11\n1\n21\n1\n0\nCIRCLE\n5\na1\n8\n0\n10\n0\n20\n0\n40\n1\n";
        let bytes = minimal_dxf(Some(6), entities);
        assert!(import_preview(&bytes, &ImportOptions::default()).is_err());
    }

    #[test]
    fn imports_closed_polyline_with_signed_bulge_and_preserves_closing_segment_value() {
        let body = "0\nLWPOLYLINE\n5\nCA\n8\n0\n90\n3\n70\n1\n10\n0\n20\n0\n42\n0.5\n10\n2\n20\n0\n42\n-0.25\n10\n2\n20\n2\n42\n0.75\n";
        let bytes = minimal_dxf(Some(6), body);
        let preview = import_preview(&bytes, &ImportOptions::default())
            .expect("valid closed polyline should preview");
        let EntityGeometry::Polyline { vertices, closed } = &preview.entities[0].geometry else {
            panic!("expected polyline")
        };
        assert!(*closed);
        assert_eq!(vertices.len(), 3);
        assert_eq!(vertices[0].bulge, 0.5);
        assert_eq!(vertices[1].bulge, -0.25);
        assert_eq!(vertices[2].bulge, 0.75);
    }

    #[test]
    fn unsupported_entity_and_xdata_are_visible_and_block_default_policy() {
        let body = "0\nSPLINE\n5\n11\n8\n0\n210\n0.5\n1001\nAPP\n1000\nvalue\n";
        let bytes = minimal_dxf(Some(6), body);
        let preview = import_preview(&bytes, &ImportOptions::default())
            .expect("losses should remain visible in the preview");
        assert!(preview.entities.is_empty());
        assert!(!preview.can_apply);
        assert_eq!(preview.report.unsupported_count, 1);
        assert_eq!(
            preview.report.source_sha256.as_deref(),
            Some(preview.source_hash.as_str())
        );
        assert!(
            preview
                .report
                .items
                .iter()
                .any(|item| item.code == "unsupported_entity")
        );
        assert!(
            preview
                .report
                .items
                .iter()
                .any(|item| item.code == "unsupported_tag")
        );
        assert!(
            preview
                .report
                .items
                .iter()
                .any(|item| item.code == "xdata_not_preserved")
        );
        let options = ImportOptions {
            accept_critical_losses: true,
            ..ImportOptions::default()
        };
        let preview = import_preview(&bytes, &options)
            .expect("accepted critical losses should remain reportable");
        assert!(preview.can_apply);
        assert!(preview.to_batch(DrawingId::new(1)).is_ok());
    }

    #[test]
    fn conflicting_header_declarations_require_explicit_override_and_loss_acceptance() {
        let bytes = b"0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n9\n$INSUNITS\n70\n6\n9\n$CUSTOM\n1\nvalue\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let unresolved = import_preview(bytes, &ImportOptions::default()).unwrap();
        assert!(unresolved.decision_required);
        assert!(!unresolved.can_apply);
        assert!(unresolved.report.has_critical());
        assert!(
            unresolved
                .report
                .items
                .iter()
                .any(|item| item.code == "unmapped_header_variable")
        );

        let selected = ImportOptions {
            unit_override: Some("metre".into()),
            ..Default::default()
        };
        let gated = import_preview(bytes, &selected).unwrap();
        assert!(!gated.decision_required);
        assert!(!gated.can_apply);
        assert!(
            gated
                .report
                .items
                .iter()
                .any(|item| item.code == "unit_declaration_conflict")
        );
        let accepted = import_preview(
            bytes,
            &ImportOptions {
                accept_critical_losses: true,
                ..selected
            },
        )
        .unwrap();
        assert!(accepted.can_apply);
        assert_eq!(
            accepted.source.unwrap().unit_resolution.resolved_unit,
            "metre"
        );
    }
}
