use crate::loss::{LossReport, LossSeverity};
use crate::units::DxfUnitChoice;
use cadcraft_dxf::{Tag, Value};
use drafting_core::{
    DraftLayer, DraftSnapshot, DraftStyle, EntityGeometry, LayerId, UnitResolution,
};

const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXPORT_TAGS: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportOptions {
    /// Target DXF units. Accepted aliases include metre/meter, mm, and inch/inches.
    pub unit: String,
    /// Opt in to writing output when the loss report contains critical losses.
    pub accept_critical_losses: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            unit: "metre".into(),
            accept_critical_losses: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportResult {
    pub bytes: Vec<u8>,
    pub report: LossReport,
}

pub fn export_dxf(
    snapshot: &DraftSnapshot,
    options: &ExportOptions,
) -> Result<ExportResult, String> {
    let unit = DxfUnitChoice::parse_name(&options.unit)?;
    let scale = unit.meters_per_unit();
    let mut report = LossReport::default();
    report.entity_count = snapshot.entities.len();
    report.unit_resolution = Some(UnitResolution {
        declared_unit: None,
        resolved_unit: unit.name().into(),
        scale_to_m: scale,
        decision_source: "export_option".into(),
    });
    if !snapshot.import_sources.is_empty()
        || snapshot
            .entities
            .values()
            .any(|entity| entity.source.is_some())
    {
        report.push(
            "source_provenance_not_exported",
            LossSeverity::Warning,
            "Lab import asset hashes and source handles are not embedded in the DXF output",
            None,
            None,
        );
    }

    let plane = &snapshot.plane;
    plane
        .validate()
        .map_err(|error| format!("invalid drawing plane: {error}"))?;
    if plane != &drafting_core::DrawingPlane::default() {
        report.push(
            "drawing_plane_not_preserved",
            LossSeverity::Critical,
            "DXF output stores local XY geometry and does not encode the Lab drawing plane",
            None,
            None,
        );
    }

    let mut tags = Vec::new();
    let mut next_handle = 1_u64;
    push(&mut tags, 0, "SECTION", &mut report)?;
    push(&mut tags, 2, "HEADER", &mut report)?;
    push(&mut tags, 9, "$ACADVER", &mut report)?;
    push(&mut tags, 1, "AC1015", &mut report)?;
    push(&mut tags, 9, "$INSUNITS", &mut report)?;
    push(&mut tags, 70, unit.insunits_code(), &mut report)?;
    push(&mut tags, 0, "ENDSEC", &mut report)?;

    let needs_default_layer = !snapshot
        .layers
        .values()
        .any(|layer| layer.name.eq_ignore_ascii_case("0"));
    let synthetic_default = needs_default_layer.then(|| DraftLayer {
        id: LayerId::new(0),
        name: "0".into(),
        visible: true,
        locked: false,
        style: DraftStyle::default(),
    });
    if needs_default_layer {
        report.push(
            "default_layer_added",
            LossSeverity::Info,
            "DXF reserved layer 0 was added to the exported layer table",
            None,
            None,
        );
    }
    let mut ordered_layers: Vec<_> = snapshot.layers.values().collect();
    if let Some(layer) = synthetic_default.as_ref() {
        ordered_layers.push(layer);
    }
    ordered_layers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    if ordered_layers.len() > drafting_core::MAX_LAYERS {
        return Err("snapshot exceeds the DXF layer export budget".into());
    }

    push(&mut tags, 0, "SECTION", &mut report)?;
    push(&mut tags, 2, "TABLES", &mut report)?;
    push(&mut tags, 0, "TABLE", &mut report)?;
    push(&mut tags, 2, "LAYER", &mut report)?;
    push_handle(&mut tags, &mut next_handle, &mut report)?;
    push(&mut tags, 100, "AcDbSymbolTable", &mut report)?;
    push(&mut tags, 70, ordered_layers.len() as i64, &mut report)?;
    let mut saw_width_loss = false;
    for layer in &ordered_layers {
        layer
            .style
            .validate()
            .map_err(|error| format!("invalid style for layer {:?}: {error}", layer.name))?;
        let name = encode_dxf_name(&layer.name)?;
        let color = rgb24(layer.style.color);
        let aci = if layer.visible { 7 } else { -7 };
        let flags = if layer.locked { 4 } else { 0 };
        push(&mut tags, 0, "LAYER", &mut report)?;
        push_handle(&mut tags, &mut next_handle, &mut report)?;
        push(&mut tags, 100, "AcDbSymbolTableRecord", &mut report)?;
        push(&mut tags, 100, "AcDbLayerTableRecord", &mut report)?;
        push(&mut tags, 2, name, &mut report)?;
        push(&mut tags, 70, flags, &mut report)?;
        push(&mut tags, 62, aci, &mut report)?;
        push(&mut tags, 6, "CONTINUOUS", &mut report)?;
        push(&mut tags, 420, i64::from(color), &mut report)?;
        saw_width_loss |= (layer.style.width_px - drafting_core::DraftStyle::default().width_px)
            .abs()
            > f64::EPSILON;
        if layer.style.color[3] < 1.0 {
            report.push(
                "layer_alpha_not_preserved",
                LossSeverity::Critical,
                format!(
                    "layer {:?} transparency is not exported by this adapter",
                    layer.name
                ),
                None,
                Some(420),
            );
        }
    }
    push(&mut tags, 0, "ENDTAB", &mut report)?;
    push(&mut tags, 0, "ENDSEC", &mut report)?;

    push(&mut tags, 0, "SECTION", &mut report)?;
    push(&mut tags, 2, "ENTITIES", &mut report)?;
    let mut ordered_entities: Vec<_> = snapshot.entities.values().collect();
    ordered_entities.sort_by_key(|entity| entity.id);
    for entity in ordered_entities {
        entity
            .geometry
            .validate(drafting_core::GeometryLimits::default())
            .map_err(|error| format!("invalid entity {}: {error}", entity.id))?;
        entity
            .style
            .validate()
            .map_err(|error| format!("invalid style on entity {}: {error}", entity.id))?;
        let layer = snapshot
            .layers
            .get(&entity.layer_id)
            .ok_or_else(|| format!("entity {} references a missing layer", entity.id))?;
        let layer_name = encode_dxf_name(&layer.name)?;
        let color = rgb24(entity.style.color);
        if entity.style.color[3] < 1.0 {
            report.push(
                "entity_alpha_not_preserved",
                LossSeverity::Critical,
                format!(
                    "entity {} transparency is not exported by this adapter",
                    entity.id
                ),
                None,
                Some(420),
            );
        }
        saw_width_loss |= (entity.style.width_px - drafting_core::DraftStyle::default().width_px)
            .abs()
            > f64::EPSILON;
        match &entity.geometry {
            EntityGeometry::Line { start, end } => {
                write_entity_header(
                    &mut tags,
                    "LINE",
                    "AcDbLine",
                    &mut next_handle,
                    &layer_name,
                    color,
                    &mut report,
                )?;
                point_tag(&mut tags, 10, 20, *start, scale, &mut report)?;
                point_tag(&mut tags, 11, 21, *end, scale, &mut report)?;
            }
            EntityGeometry::Circle { center, radius } => {
                write_entity_header(
                    &mut tags,
                    "CIRCLE",
                    "AcDbCircle",
                    &mut next_handle,
                    &layer_name,
                    color,
                    &mut report,
                )?;
                point_tag(&mut tags, 10, 20, *center, scale, &mut report)?;
                push(&mut tags, 40, finite_scaled(*radius, scale)?, &mut report)?;
            }
            EntityGeometry::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                let is_full_circle = (sweep_angle.abs() - std::f64::consts::TAU).abs() <= 1e-12;
                if is_full_circle {
                    report.push(
                        "full_arc_encoded_as_circle",
                        LossSeverity::Warning,
                        format!(
                            "entity {} is a full-turn arc and was written as CIRCLE",
                            entity.id
                        ),
                        None,
                        None,
                    );
                    write_entity_header(
                        &mut tags,
                        "CIRCLE",
                        "AcDbCircle",
                        &mut next_handle,
                        &layer_name,
                        color,
                        &mut report,
                    )?;
                    point_tag(&mut tags, 10, 20, *center, scale, &mut report)?;
                    push(&mut tags, 40, finite_scaled(*radius, scale)?, &mut report)?;
                } else if *sweep_angle < 0.0 {
                    let arc = drafting_core::AnalyticArc { center:*center,radius:*radius,start_angle:*start_angle,sweep_angle:*sweep_angle };
                    let start = arc.point(0.);
                    let end = arc.point(1.);
                    if start.distance(end)<=drafting_core::GeometryLimits::default().linear_tolerance_m {
                        // Endpoint coordinates cannot carry this chord at the
                        // supplied origin. ARC preserves its locus, but DXF's
                        // counter-clockwise convention reverses its direction.
                        report.push("negative_arc_endpoint_precision_loss",LossSeverity::Critical,
                            format!("entity {} cannot preserve clockwise direction as LWPOLYLINE at this coordinate precision; candidate ARC reverses direction",entity.id),None,None);
                        write_entity_header(&mut tags,"ARC","AcDbCircle",&mut next_handle,&layer_name,color,&mut report)?;
                        push(&mut tags,100,"AcDbArc",&mut report)?;
                        point_tag(&mut tags,10,20,*center,scale,&mut report)?;
                        push(&mut tags,40,finite_scaled(*radius,scale)?,&mut report)?;
                        let (sin,cos)=start_angle.sin_cos();let phase=sin.atan2(cos);
                        push(&mut tags,50,(phase+sweep_angle).rem_euclid(std::f64::consts::TAU).to_degrees(),&mut report)?;
                        push(&mut tags,51,phase.rem_euclid(std::f64::consts::TAU).to_degrees(),&mut report)?;
                        continue;
                    }
                    report.push(
                        "negative_arc_encoded_as_polyline",
                        LossSeverity::Warning,
                        format!(
                            "entity {} orientation is preserved with a signed LWPOLYLINE bulge",
                            entity.id
                        ),
                        None,
                        None,
                    );
                    write_entity_header(
                        &mut tags,
                        "LWPOLYLINE",
                        "AcDbPolyline",
                        &mut next_handle,
                        &layer_name,
                        color,
                        &mut report,
                    )?;
                    push(&mut tags, 90, 2_i64, &mut report)?;
                    push(&mut tags, 70, 0_i64, &mut report)?;
                    point_tag(&mut tags, 10, 20, start, scale, &mut report)?;
                    push(&mut tags, 42, (sweep_angle * 0.25).tan(), &mut report)?;
                    point_tag(&mut tags, 10, 20, end, scale, &mut report)?;
                    push(&mut tags, 42, 0.0_f64, &mut report)?;
                } else {
                    write_entity_header(
                        &mut tags,
                        "ARC",
                        "AcDbCircle",
                        &mut next_handle,
                        &layer_name,
                        color,
                        &mut report,
                    )?;
                    push(&mut tags, 100, "AcDbArc", &mut report)?;
                    point_tag(&mut tags, 10, 20, *center, scale, &mut report)?;
                    push(&mut tags, 40, finite_scaled(*radius, scale)?, &mut report)?;
                    let (sin,cos) = start_angle.sin_cos();
                    let start_phase = sin.atan2(cos);
                    let start_degrees = start_phase.rem_euclid(std::f64::consts::TAU).to_degrees();
                    let end_degrees = (start_phase + sweep_angle)
                        .rem_euclid(std::f64::consts::TAU)
                        .to_degrees();
                    push(&mut tags, 50, start_degrees, &mut report)?;
                    push(&mut tags, 51, end_degrees, &mut report)?;
                }
            }
            EntityGeometry::Polyline { vertices, closed } => {
                write_entity_header(
                    &mut tags,
                    "LWPOLYLINE",
                    "AcDbPolyline",
                    &mut next_handle,
                    &layer_name,
                    color,
                    &mut report,
                )?;
                push(&mut tags, 90, vertices.len() as i64, &mut report)?;
                push(
                    &mut tags,
                    70,
                    if *closed { 1_i64 } else { 0_i64 },
                    &mut report,
                )?;
                for vertex in vertices {
                    point_tag(&mut tags, 10, 20, vertex.point, scale, &mut report)?;
                    push(&mut tags, 42, vertex.bulge, &mut report)?;
                }
            }
        }
    }
    if saw_width_loss {
        report.push(
            "screen_width_not_preserved",
            LossSeverity::Warning,
            "DraftStyle width is in logical pixels and has no stable DXF lineweight equivalent",
            None,
            None,
        );
    }
    report.converted_count = snapshot.entities.len();
    push(&mut tags, 0, "ENDSEC", &mut report)?;
    push(&mut tags, 0, "EOF", &mut report)?;

    let text = cadcraft_dxf::write_ascii(&tags);
    if !text.is_ascii() {
        return Err("internal DXF export produced non-ASCII output".into());
    }
    if text.len() > MAX_EXPORT_BYTES {
        return Err(format!(
            "DXF export exceeds the {MAX_EXPORT_BYTES} byte budget"
        ));
    }
    let bytes = text.into_bytes();
    report.source_sha256 = Some(sha256_hex(&bytes));
    if report.has_critical() && !options.accept_critical_losses {
        return Err(report.summary());
    }
    Ok(ExportResult { bytes, report })
}

/// Build candidate ASCII bytes and the complete loss report without applying the
/// critical-loss gate. Callers must inspect `report.has_critical()` before writing.
pub fn export_dxf_preview(
    snapshot: &DraftSnapshot,
    options: &ExportOptions,
) -> Result<ExportResult, String> {
    let preview_options = ExportOptions {
        accept_critical_losses: true,
        ..options.clone()
    };
    export_dxf(snapshot, &preview_options)
}

fn write_entity_header(
    tags: &mut Vec<Tag>,
    kind: &str,
    subclass: &str,
    next_handle: &mut u64,
    layer: &str,
    color: u32,
    report: &mut LossReport,
) -> Result<(), String> {
    push(tags, 0, kind, report)?;
    push_handle(tags, next_handle, report)?;
    push(tags, 100, "AcDbEntity", report)?;
    push(tags, 8, layer, report)?;
    push(tags, 62, 7_i64, report)?;
    push(tags, 420, i64::from(color), report)?;
    push(tags, 100, subclass, report)
}

fn point_tag(
    tags: &mut Vec<Tag>,
    x_code: i32,
    y_code: i32,
    point: drafting_core::Point2,
    scale: f64,
    report: &mut LossReport,
) -> Result<(), String> {
    push(tags, x_code, finite_scaled(point.x, scale)?, report)?;
    push(tags, y_code, finite_scaled(point.y, scale)?, report)
}

fn finite_scaled(value: f64, scale: f64) -> Result<f64, String> {
    let result = value / scale;
    if !value.is_finite() || !result.is_finite() {
        return Err("DXF unit conversion overflowed".into());
    }
    Ok(result)
}

fn push_handle(
    tags: &mut Vec<Tag>,
    next_handle: &mut u64,
    report: &mut LossReport,
) -> Result<(), String> {
    let value = format!("{:X}", *next_handle);
    *next_handle = (*next_handle)
        .checked_add(1)
        .ok_or_else(|| "DXF handle space exhausted".to_string())?;
    push(tags, 5, value, report)
}

enum DxfValue {
    String(String),
    Real(f64),
    Integer(i64),
}
impl From<&str> for DxfValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<String> for DxfValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<i64> for DxfValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}
impl From<i32> for DxfValue {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}
impl From<u32> for DxfValue {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}
impl From<f64> for DxfValue {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

fn push(
    tags: &mut Vec<Tag>,
    code: i32,
    value: impl Into<DxfValue>,
    _report: &mut LossReport,
) -> Result<(), String> {
    if tags.len() >= MAX_EXPORT_TAGS {
        return Err(format!(
            "DXF export exceeds the {MAX_EXPORT_TAGS} tag budget"
        ));
    }
    let value = match value.into() {
        DxfValue::String(text) => Value::Str(text),
        DxfValue::Real(number) => Value::Real(number),
        DxfValue::Integer(number) => Value::Int(number),
    };
    tags.push(Tag::new(code, value));
    Ok(())
}

fn encode_dxf_name(name: &str) -> Result<String, String> {
    const RESERVED: &str = "<>/\\\";:?*|=,";
    if name.trim().is_empty()
        || name.len() > 512
        || name.contains(['\r', '\n', '\0'])
        || name.chars().any(|ch| ch.is_ascii_control())
        || name.chars().any(|ch| RESERVED.contains(ch))
    {
        return Err(
            "DXF layer name is empty, exceeds 512 bytes, or contains a reserved character".into(),
        );
    }
    let mut out = String::new();
    for ch in name.chars() {
        match ch {
            ch if ch.is_ascii() && !ch.is_ascii_control() => out.push(ch),
            ch if u32::from(ch) <= 0xFFFF => out.push_str(&format!("\\U+{:04X}", u32::from(ch))),
            _ => {
                return Err(
                    "non-BMP Unicode layer names are unsupported by this ASCII adapter".into(),
                );
            }
        }
    }
    if out.len() > 1024 {
        return Err("escaped DXF layer name exceeds the 1024-byte export limit".into());
    }
    Ok(out)
}

fn rgb24(color: [f32; 4]) -> u32 {
    let r = (f64::from(color[0]) * 255.0).round() as u32;
    let g = (f64::from(color[1]) * 255.0).round() as u32;
    let b = (f64::from(color[2]) * 255.0).round() as u32;
    (r << 16) | (g << 8) | b
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafting_core::{
        DraftEntityId, DraftLayer, DraftStyle, DrawingId, DrawingPlane, LayerId, Point2,
        PolylineVertex,
    };
    use std::collections::BTreeMap;

    fn snapshot(entities: Vec<(u64, EntityGeometry)>, layer_name: &str) -> DraftSnapshot {
        let layer_id = LayerId::new(1);
        let layer = DraftLayer {
            id: layer_id,
            name: layer_name.into(),
            visible: true,
            locked: false,
            style: DraftStyle::default(),
        };
        let entity_map = entities
            .into_iter()
            .map(|(id, geometry)| {
                (
                    DraftEntityId::new(id),
                    drafting_core::DraftEntity {
                        id: DraftEntityId::new(id),
                        layer_id,
                        geometry,
                        style: DraftStyle::default(),
                        revision: 0,
                        source: None,
                    },
                )
            })
            .collect();
        DraftSnapshot {
            document_id: "test".into(),
            document_revision: 0,
            drawing_id: DrawingId::new(1),
            name: "Test".into(),
            revision: 0,
            plane: DrawingPlane::default(),
            entities: entity_map,
            layers: BTreeMap::from([(layer_id, layer)]),
            import_sources: BTreeMap::new(),
            source_fingerprint: "test".into(),
        }
    }

    #[test]
    fn exports_and_reimports_supported_geometry_in_requested_units() {
        let snapshot = snapshot(
            vec![
                (
                    1,
                    EntityGeometry::Line {
                        start: Point2::new(1.0, 2.0),
                        end: Point2::new(3.0, 4.0),
                    },
                ),
                (
                    2,
                    EntityGeometry::Circle {
                        center: Point2::new(1.0, 1.0),
                        radius: 0.5,
                    },
                ),
                (
                    3,
                    EntityGeometry::Arc {
                        center: Point2::new(0.0, 0.0),
                        radius: 1.0,
                        start_angle: 0.0,
                        sweep_angle: 1.0,
                    },
                ),
                (
                    4,
                    EntityGeometry::Polyline {
                        vertices: vec![
                            PolylineVertex::new(Point2::new(0.0, 0.0), -0.5),
                            PolylineVertex::new(Point2::new(1.0, 0.0), 0.25),
                        ],
                        closed: true,
                    },
                ),
            ],
            "Layer A",
        );
        let output = export_dxf(
            &snapshot,
            &ExportOptions {
                unit: "mm".into(),
                accept_critical_losses: false,
            },
        )
        .unwrap();
        assert!(output.bytes.is_ascii());
        assert_eq!(output.report.converted_count, 4);
        let preview =
            crate::import_preview(&output.bytes, &crate::ImportOptions::default()).unwrap();
        assert!(preview.can_apply);
        assert_eq!(preview.entities.len(), 4);
        assert_eq!(
            preview.entities[0].geometry,
            EntityGeometry::Line {
                start: Point2::new(1.0, 2.0),
                end: Point2::new(3.0, 4.0)
            }
        );
        let EntityGeometry::Polyline { vertices, closed } = &preview.entities[3].geometry else {
            panic!("expected polyline")
        };
        assert!(*closed);
        assert_eq!(vertices[0].bulge, -0.5);
        assert_eq!(vertices[1].bulge, 0.25);
    }

    #[test]
    fn negative_arc_direction_is_preserved_with_signed_bulge() {
        let snapshot = snapshot(
            vec![(
                1,
                EntityGeometry::Arc {
                    center: Point2::new(0.0, 0.0),
                    radius: 2.0,
                    start_angle: 0.4,
                    sweep_angle: -1.25,
                },
            )],
            "0",
        );
        let output = export_dxf(&snapshot, &ExportOptions::default()).unwrap();
        assert!(
            output
                .report
                .items
                .iter()
                .any(|item| item.code == "negative_arc_encoded_as_polyline")
        );
        let preview =
            crate::import_preview(&output.bytes, &crate::ImportOptions::default()).unwrap();
        let EntityGeometry::Polyline { vertices, closed } = &preview.entities[0].geometry else {
            panic!("expected signed polyline")
        };
        assert!(!closed);
        assert!(vertices[0].bulge < 0.0);
        let arc = drafting_core::bulge_arc(vertices[0].point, vertices[1].point, vertices[0].bulge)
            .unwrap()
            .unwrap();
        assert!((arc.sweep_angle + 1.25).abs() < 1e-12);
    }

    #[test]
    fn huge_start_angle_small_sweep_round_trips_without_collapse() {
        for sweep_angle in [1e-6,-1e-6] {
            let snapshot = snapshot(vec![(1,EntityGeometry::Arc{center:Point2::new(0.,0.),radius:2.,start_angle:1e12,sweep_angle})],"0");
            let output=export_dxf(&snapshot,&ExportOptions::default()).unwrap();
            let preview=crate::import_preview(&output.bytes,&crate::ImportOptions::default()).unwrap();
            assert!(preview.can_apply,"{:?}",preview.report);assert_eq!(preview.entities.len(),1);
            let arc=match &preview.entities[0].geometry {
                EntityGeometry::Arc{center,radius,start_angle,sweep_angle}=>drafting_core::AnalyticArc{center:*center,radius:*radius,start_angle:*start_angle,sweep_angle:*sweep_angle},
                EntityGeometry::Polyline{vertices,..}=>drafting_core::bulge_arc(vertices[0].point,vertices[1].point,vertices[0].bulge).unwrap().unwrap(),
                _=>panic!("arc or signed polyline expected"),
            };
            assert!((arc.sweep_angle-sweep_angle).abs()<1e-12);
            let original=drafting_core::AnalyticArc{center:Point2::new(0.,0.),radius:2.,start_angle:1e12,sweep_angle};
            assert!(arc.point(0.).distance(original.point(0.))<1e-8);
            assert!(arc.point(1.).distance(original.point(1.))<1e-8);
        }
    }

    #[test]
    fn negative_arc_unrepresentable_chord_requires_explicit_loss_acceptance() {
        let snapshot=snapshot(vec![(1,EntityGeometry::Arc{center:Point2::new(1e9,1e9),radius:1.,start_angle:0.,sweep_angle:-1e-8})],"0");
        let preview=export_dxf_preview(&snapshot,&ExportOptions::default()).unwrap();
        assert!(preview.report.has_critical());assert!(preview.report.items.iter().any(|i|i.code=="negative_arc_endpoint_precision_loss"));
        assert!(export_dxf(&snapshot,&ExportOptions::default()).is_err());
        let accepted=export_dxf(&snapshot,&ExportOptions{accept_critical_losses:true,..Default::default()}).unwrap();
        let imported=crate::import_preview(&accepted.bytes,&crate::ImportOptions::default()).unwrap();
        assert!(imported.can_apply);assert_eq!(imported.entities.len(),1);
        assert!(matches!(imported.entities[0].geometry,EntityGeometry::Arc{sweep_angle,..} if sweep_angle>0.));
    }

    #[test]
    fn unicode_layer_names_are_ascii_escaped_and_round_trip_for_bmp() {
        let snapshot = snapshot(
            vec![(
                1,
                EntityGeometry::Circle {
                    center: Point2::new(0.0, 0.0),
                    radius: 1.0,
                },
            )],
            "Этаж 1",
        );
        let output = export_dxf(&snapshot, &ExportOptions::default()).unwrap();
        assert!(output.bytes.is_ascii());
        let preview =
            crate::import_preview(&output.bytes, &crate::ImportOptions::default()).unwrap();
        assert!(preview.layers.iter().any(|layer| layer.name == "Этаж 1"));
    }

    #[test]
    fn reports_plane_and_transparency_loss_and_gates_write_by_default() {
        let mut snapshot = snapshot(
            vec![(
                1,
                EntityGeometry::Circle {
                    center: Point2::new(0.0, 0.0),
                    radius: 1.0,
                },
            )],
            "0",
        );
        snapshot.plane.origin.x = 10.0;
        let layer = snapshot.layers.get_mut(&LayerId::new(1)).unwrap();
        layer.style.color[3] = 0.5;
        let preview = export_dxf_preview(&snapshot, &ExportOptions::default()).unwrap();
        assert!(preview.report.has_critical());
        assert!(!preview.bytes.is_empty());
        let error = export_dxf(&snapshot, &ExportOptions::default()).unwrap_err();
        assert!(error.contains("critical losses"));
        let output = export_dxf(
            &snapshot,
            &ExportOptions {
                accept_critical_losses: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            output
                .report
                .items
                .iter()
                .any(|item| item.code == "drawing_plane_not_preserved")
        );
        assert!(
            output
                .report
                .items
                .iter()
                .any(|item| item.code == "layer_alpha_not_preserved")
        );
    }
}
