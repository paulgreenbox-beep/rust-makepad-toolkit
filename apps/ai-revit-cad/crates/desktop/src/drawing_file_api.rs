//! Explicit snapshot exports. BIM references remain derived, domain-qualified data.
//! All writes use the canonical-root policy; no export commits a Core transaction.
use super::drafting_export;
use crate::{App, bim_underlay, file_guard};
use ai_revit_core::{Document, DrawingId, ElementId};
use drafting_core::{
    DisplaySnapshot, DraftSnapshot, DraftStyle, Point2, ScreenSegment, clip_display_segment,
};
use drafting_interop::{
    ExportOptions, ExportResult, LossItem, LossReport, LossSeverity, export_dxf_preview,
};
use makepad_widgets::Cx;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::Path;

const MAX_UNDERLAY_SEGMENTS: usize = 100_000;
const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
const MAX_SVG_BYTES: usize = 32 * 1024 * 1024;
const CLIP_PADDING: f64 = 2.0;
const SVG_CANVAS_GROUP: &str = "<g clip-path=\"url(#canvas)\" fill=\"none\">";

#[derive(Clone, Debug)]
struct ProjectedBimSegment {
    element_id: ElementId,
    source_index: usize,
    start: Point2,
    end: Point2,
    style: DraftStyle,
}

struct AnalyticDxfExport {
    revision: u64,
    epoch: u64,
    output: ExportResult,
    metadata: Value,
    options: ExportOptions,
}

impl App {
    pub(crate) fn drawing_export_file(
        &mut self,
        cx: &mut Cx,
        kind: &str,
        path: &Path,
    ) -> Result<Value, String> {
        let kind = match kind {
            "png" | "svg" | "dxf" => kind,
            "id_map" | "id-map" | "idmap" | "json" => "id_map",
            _ => return Err("drawing export kind must be png, svg, id_map or dxf".into()),
        };
        if kind == "dxf" {
            return self.drawing_export_dxf_options(cx, path, ExportOptions::default());
        }
        let policy = file_guard::FilePolicy::from_environment()?;
        let revision = self
            .document
            .as_ref()
            .ok_or("no document is open")?
            .revision();
        let epoch = self.api_document_generation;
        let drawing = self
            .drafting_current_drawing_id()
            .ok_or("no active drawing")?;
        let capture = self.drafting_capture_export()?;
        if capture.snapshot.drawing_id != drawing
            || capture.display.drawing_id != drawing
            || capture.snapshot.document_revision != revision
            || capture.display.document_revision != revision
        {
            return Err(
                "export snapshot does not match the current project revision and drawing".into(),
            );
        }
        let underlay = self.bim_underlay_frame(drawing)?;
        if underlay.source_revision != revision {
            return Err("BIM reference is not bound to the export project revision".into());
        }
        let projected = project_underlay(&capture.display, &underlay)?;
        let runtime = self
            .document
            .as_ref()
            .ok_or("no document")?
            .runtime_instance_id();
        let reference_count = self
            .document
            .as_ref()
            .ok_or("no document")?
            .view_definitions()
            .values()
            .filter(|view| view.drawing_id == drawing)
            .count();
        let state = self.drawing_ui_state(cx)?;
        let native_binding = native_frame_binding(&capture.display, &underlay, &state);
        let metadata = mixed_metadata(
            &capture.display,
            &capture.snapshot,
            &underlay,
            &projected,
            epoch,
            &runtime,
            reference_count,
            native_binding,
        )?;
        let mut display = capture.display.clone();
        // PNG's embedded text token and SVG/ID-map metadata identify the mixed export.
        // The original cache-issued manual token remains separately recorded.
        display.token = metadata["snapshot_token"]
            .as_str()
            .ok_or("missing export token")?
            .to_owned();
        let bytes = match kind {
            "png" => {
                let segments = projected
                    .iter()
                    .map(|segment| (segment.start, segment.end, segment.style.clone()))
                    .collect::<Vec<_>>();
                drafting_export::render_png_with_underlay(&display, &segments)?
            }
            "svg" => render_mixed_svg(&display, &metadata, &projected)?.into_bytes(),
            "id_map" => bounded_json(
                &mixed_id_map(&display, &projected, &metadata),
                MAX_JSON_BYTES,
            )?,
            _ => unreachable!(),
        };
        self.publish_drawing_export(
            cx, &policy, kind, path, revision, epoch, bytes, metadata, None,
        )
    }

    /// Analytic candidate only: no file policy access, display preparation or write.
    pub(crate) fn drawing_export_dxf_preview(
        &mut self,
        options: ExportOptions,
    ) -> Result<Value, String> {
        let export = prepare_analytic_dxf(
            self.document.as_ref().ok_or("no document is open")?,
            self.drafting_current_drawing_id()
                .ok_or("no active drawing")?,
            self.api_document_generation,
            options,
        )?;
        Ok(dxf_readiness(&export))
    }

    pub(crate) fn drawing_export_dxf_options(
        &mut self,
        cx: &mut Cx,
        path: &Path,
        options: ExportOptions,
    ) -> Result<Value, String> {
        let export = prepare_analytic_dxf(
            self.document.as_ref().ok_or("no document is open")?,
            self.drafting_current_drawing_id()
                .ok_or("no active drawing")?,
            self.api_document_generation,
            options,
        )?;
        if export.output.report.has_critical() && !export.options.accept_critical_losses {
            // Returning the full report is a successful refusal, without consulting
            // an output path or creating a temporary file.
            self.set_status(cx,"DXF не записан: обнаружены критические потери; требуется явное решение по LossReport");
            return Ok(dxf_readiness(&export));
        }
        let policy = file_guard::FilePolicy::from_environment()?;
        let options = export.options.clone();
        let mut response = self.publish_drawing_export(
            cx,
            &policy,
            "dxf",
            path,
            export.revision,
            export.epoch,
            export.output.bytes,
            export.metadata,
            Some(export.output.report),
        )?;
        response["options"] = serde_json::to_value(options).map_err(|e| e.to_string())?;
        response["can_export"] = json!(true);
        Ok(response)
    }

    fn publish_drawing_export(
        &mut self,
        cx: &mut Cx,
        policy: &file_guard::FilePolicy,
        kind: &str,
        path: &Path,
        revision: u64,
        epoch: u64,
        bytes: Vec<u8>,
        metadata: Value,
        loss_report: Option<LossReport>,
    ) -> Result<Value, String> {
        if self.api_document_generation != epoch
            || self.document.as_ref().map(|d| d.revision()) != Some(revision)
        {
            return Err("project changed while the snapshot export was being prepared".into());
        }
        let output = policy.output(path)?;
        // A visual export must not replace the real project currently open in the UI.
        if let Ok(open_project) = Path::new(&self.path_text(cx)).canonicalize() {
            if open_project == output {
                return Err("an export cannot overwrite the open project file".into());
            }
        }
        let hash = drafting_export::sha256(&bytes);
        let output = policy.write_atomic(&output, &bytes)?;
        self.set_status(cx, &format!("Экспорт {}: {}", kind, output.display()));
        Ok(
            json!({"written":true,"path":output,"kind":kind,"bytes":bytes.len(),"sha256":hash,"metadata":metadata,
            "loss_report":loss_report,"revision_before":revision,"revision_after":self.document.as_ref().map(|d|d.revision()),
            "document_mutated":false,"output_root_policy":"canonical_permitted_roots_atomic_replace"}),
        )
    }
}

fn prepare_analytic_dxf(
    document: &Document,
    drawing: DrawingId,
    epoch: u64,
    options: ExportOptions,
) -> Result<AnalyticDxfExport, String> {
    // This explicit export boundary reads the sole authoritative analytic model.
    // View tessellation, derived projection and native/GPU readiness are irrelevant.
    let revision = document.revision();
    let snapshot = document
        .drafting_snapshot(drawing)
        .map_err(|e| e.to_string())?;
    let references = document
        .view_definitions()
        .values()
        .filter(|view| view.drawing_id == drawing)
        .collect::<Vec<_>>();
    let source_count = references
        .iter()
        .flat_map(|view| view.source.element_ids.iter().copied())
        .collect::<BTreeSet<_>>()
        .len();
    let output = manual_dxf_candidate(&snapshot, references.len(), source_count, None, &options)?;
    let manual_hash = drafting_export::sha256(&bounded_json(
        &serde_json::to_value(&snapshot).map_err(|e| e.to_string())?,
        MAX_JSON_BYTES,
    )?);
    let reference_hash = drafting_export::sha256(&bounded_json(
        &serde_json::to_value(&references).map_err(|e| e.to_string())?,
        MAX_JSON_BYTES,
    )?);
    let binding = json!({"document_id":snapshot.document_id,"document_revision":revision,"drawing_id":drawing,
        "runtime_instance_id":document.runtime_instance_id(),"document_epoch":epoch,"manual_snapshot_sha256":manual_hash,
        "hash_scope":"active drawing analytic geometry, layers and import provenance; no display mesh",
        "excluded_reference_definitions_sha256":reference_hash,"export_options":options});
    let token = format!(
        "unified-dxf-{}",
        drafting_export::sha256(&bounded_json(&binding, MAX_JSON_BYTES)?)
    );
    let output_unit = output
        .report
        .unit_resolution
        .as_ref()
        .map(|unit| unit.resolved_unit.as_str());
    let metadata = json!({"snapshot_token":token,"source_binding":binding,"export_scope":"manual_analytic_2d_only",
        "renderer":"bounded_ascii_dxf_analytic_snapshot","representation_quality":"analytic_2d","units":output_unit,
        "source_units":"metre","document_revision":revision,"drawing_id":drawing,"manual_entity_count":snapshot.entities.len(),
        "bim_reference":{"included_in_output":false,"view_definition_count":references.len(),"source_element_count":source_count,"derived_geometry_evaluated":false},
        "native_frame_binding":{"applicable":false,"reason":"analytic DXF does not depend on a rendered frame"}});
    Ok(AnalyticDxfExport {
        revision,
        epoch,
        output,
        metadata,
        options,
    })
}

fn dxf_readiness(export: &AnalyticDxfExport) -> Value {
    let has_critical = export.output.report.has_critical();
    let allowed = !has_critical || export.options.accept_critical_losses;
    json!({"kind":"dxf","written":false,"can_export":allowed,"critical_losses_present":has_critical,
        "refusal_reason":if allowed {Value::Null} else {json!("critical_loss_refusal")},
        "loss_report":export.output.report,"options":export.options,"metadata":export.metadata,
        "candidate_bytes":export.output.bytes.len(),"candidate_sha256":drafting_export::sha256(&export.output.bytes),
        "revision_before":export.revision,"revision_after":export.revision,"document_mutated":false})
}

fn project_underlay(
    display: &DisplaySnapshot,
    underlay: &bim_underlay::UnderlayFrame,
) -> Result<Vec<ProjectedBimSegment>, String> {
    display.camera.validate().map_err(|e| e.to_string())?;
    if underlay.segments.len() > MAX_UNDERLAY_SEGMENTS {
        return Err("BIM reference export segment budget exceeded".into());
    }
    let bounds = display.viewport.expanded(CLIP_PADDING);
    let style = DraftStyle {
        color: [0.39, 0.48, 0.57, 1.0],
        width_px: 1.2,
    };
    let mut projected = Vec::with_capacity(underlay.segments.len());
    for (source_index, segment) in underlay.segments.iter().enumerate() {
        if !segment.start.is_finite() || !segment.end.is_finite() {
            return Err("nonfinite BIM reference export source".into());
        }
        let start = display.camera.world_to_screen(segment.start);
        let end = display.camera.world_to_screen(segment.end);
        if !start.is_finite() || !end.is_finite() {
            return Err("BIM reference projection exceeds finite display coordinates".into());
        }
        if let Some(clipped) = clip_display_segment(ScreenSegment { start, end }, bounds) {
            projected.push(ProjectedBimSegment {
                element_id: segment.element_id,
                source_index,
                start: clipped.start,
                end: clipped.end,
                style: style.clone(),
            });
        }
    }
    Ok(projected)
}

fn native_frame_binding(
    display: &DisplaySnapshot,
    underlay: &bim_underlay::UnderlayFrame,
    state: &Value,
) -> Value {
    let expected = format!("{}:bim:{}", display.token, underlay.generation);
    let draw = state.get("draw").cloned().unwrap_or(Value::Null);
    let source_matches = draw.get("snapshot_token").and_then(Value::as_str)
        == Some(expected.as_str())
        && draw.get("document_revision").and_then(Value::as_u64) == Some(display.document_revision)
        && draw.get("camera_revision").and_then(Value::as_u64) == Some(display.camera.revision);
    json!({"submitted_frame":draw,"document_epoch":state.get("document_epoch"),
        "expected_native_snapshot_token":expected,"native_submission_matches_export_source":source_matches,
        "export_render_origin":"cpu_snapshot_export","native_gpu_pass_or_present_proof":false,
        "scope":"source binding to an observed submitted Canvas frame; export is separately rendered from the snapshot"})
}

fn mixed_metadata(
    display: &DisplaySnapshot,
    snapshot: &DraftSnapshot,
    underlay: &bim_underlay::UnderlayFrame,
    projected: &[ProjectedBimSegment],
    epoch: u64,
    runtime: &str,
    reference_count: usize,
    native_binding: Value,
) -> Result<Value, String> {
    let manual = drafting_export::snapshot_metadata(display, snapshot);
    let world_payload = json!({"source_revision":underlay.source_revision,"generation":underlay.generation,
        "projection_status":underlay.status,"segments":underlay.segments.iter().map(|segment|json!({
            "owner":{"domain":"bim","element_id":segment.element_id.get()},"start":segment.start,"end":segment.end})).collect::<Vec<_>>()});
    let bim_hash = drafting_export::sha256(&bounded_json(&world_payload, MAX_JSON_BYTES)?);
    let binding = json!({"document_id":display.document_id,"document_revision":display.document_revision,
        "drawing_id":display.drawing_id,"drawing_revision":display.source_revision,
        "runtime_instance_id":runtime,"document_epoch":epoch,
        "manual_snapshot_token":display.token,"manual_snapshot_sha256":manual["source_sha256"],
        "manual_hash_scope":"active drawing analytic snapshot, layers and import provenance",
        "bim_reference_sha256":bim_hash,"bim_hash_scope":"derived world XY segments, qualified ElementId owners and projection status",
        "bim_source_revision":underlay.source_revision,"bim_projection_generation":underlay.generation,
        "camera":display.camera,"viewport":display.viewport,"dpi_factor":display.dpi_factor,
        "manual_source_fingerprint":display.source_fingerprint});
    let token = format!(
        "unified-export-{}",
        drafting_export::sha256(&bounded_json(&binding, MAX_JSON_BYTES)?)
    );
    Ok(
        json!({"snapshot_token":token,"source_binding":binding,"manual_metadata":manual,
        "export_scope":"active_drawing_manual_analytic_2d_plus_derived_bim_reference",
        "renderer":"bounded_cpu_display_snapshot","units":"metre","document_id":display.document_id,
        "document_revision":display.document_revision,"drawing_id":display.drawing_id,"camera":display.camera,
        "viewport":display.viewport,"dpi_factor":display.dpi_factor,"clip_padding_logical_px":CLIP_PADDING,
        "manual":{"source_scope":"manual_analytic_2d","representation_quality":"analytic_2d",
            "entity_count":snapshot.entities.len(),"visible_analytic_entity_count":display.analytic_entities.len(),"display_primitive_count":display.primitives.len()},
        "bim_reference":{"included_in_output":true,"representation_quality":"approximate_mesh_projection",
            "view_definition_count":reference_count,"world_segment_count":underlay.segments.len(),"visible_segment_count":projected.len(),
            "source_revision":underlay.source_revision,"projection_generation":underlay.generation,"status":underlay.status},
        "native_frame_binding":native_binding}),
    )
}

fn mixed_id_map(
    display: &DisplaySnapshot,
    underlay: &[ProjectedBimSegment],
    metadata: &Value,
) -> Value {
    json!({"metadata":metadata,
        "manual_primitives":display.primitives.iter().map(|primitive|json!({
            "render_primitive_id":primitive.primitive_id,
            "owner":{"domain":"drafting","drawing_id":primitive.drawing_id,"entity_id":primitive.owner_entity_id},
            "source_scope":"manual_analytic_2d","source_quality":"analytic_2d","source_revision":primitive.source_revision,
            "screen_start":primitive.start,"screen_end":primitive.end})).collect::<Vec<_>>(),
        "bim_reference_primitives":underlay.iter().map(|segment|json!({
            "render_local_source_index":segment.source_index,"owner":{"domain":"bim","element_id":segment.element_id.get()},
            "representation_quality":"approximate_mesh_projection","screen_start":segment.start,"screen_end":segment.end})).collect::<Vec<_>>(),
        "identity_contract":"render primitive IDs and local source indices are transient; only qualified owners identify project objects"})
}

fn render_mixed_svg(
    display: &DisplaySnapshot,
    metadata: &Value,
    underlay: &[ProjectedBimSegment],
) -> Result<String, String> {
    let svg = drafting_export::render_svg_with_metadata(display, metadata)?;
    if svg.matches(SVG_CANVAS_GROUP).count() != 1 {
        return Err(
            "accepted SVG canvas-group contract changed; refusing an unverified mixed export"
                .into(),
        );
    }
    // Preserve accepted analytic ARC/bulge paths. Add explicit owner domains to those
    // existing paths, then insert the BIM group at the checked back-to-front seam.
    let manual_prefix = format!(
        "<path data-domain=\"drafting\" data-drawing-id=\"{}\" data-source-quality=\"analytic_2d\" data-entity-id=",
        display.drawing_id
    );
    let svg = svg.replace("<path data-entity-id=", &manual_prefix);
    let insertion = svg
        .find(SVG_CANVAS_GROUP)
        .ok_or("SVG canvas group missing")?
        + SVG_CANVAS_GROUP.len();
    let mut bim = String::from(
        "<g data-domain=\"bim\" data-representation-quality=\"approximate_mesh_projection\">",
    );
    for segment in underlay {
        let color = format!(
            "#{:02x}{:02x}{:02x}",
            (segment.style.color[0] * 255.0).round() as u8,
            (segment.style.color[1] * 255.0).round() as u8,
            (segment.style.color[2] * 255.0).round() as u8
        );
        let path = format!(
            "<path data-domain=\"bim\" data-element-id=\"{}\" data-render-local-source-index=\"{}\" d=\"M {} {} L {} {}\" stroke=\"{}\" stroke-width=\"{}\" stroke-opacity=\"{}\" stroke-linecap=\"round\"/>",
            segment.element_id.get(),
            segment.source_index,
            segment.start.x,
            segment.start.y,
            segment.end.x,
            segment.end.y,
            color,
            segment.style.width_px,
            segment.style.color[3]
        );
        if svg
            .len()
            .checked_add(bim.len())
            .and_then(|n| n.checked_add(path.len() + 4))
            .is_none_or(|n| n > MAX_SVG_BYTES)
        {
            return Err("mixed SVG byte budget exceeded".into());
        }
        bim.push_str(&path);
    }
    bim.push_str("</g>");
    if svg
        .len()
        .checked_add(bim.len())
        .is_none_or(|n| n > MAX_SVG_BYTES)
    {
        return Err("mixed SVG byte budget exceeded".into());
    }
    let mut result = String::with_capacity(svg.len() + bim.len());
    result.push_str(&svg[..insertion]);
    result.push_str(&bim);
    result.push_str(&svg[insertion..]);
    Ok(result)
}

fn manual_dxf_candidate(
    snapshot: &DraftSnapshot,
    reference_count: usize,
    source_count: usize,
    derived_segment_count: Option<usize>,
    options: &ExportOptions,
) -> Result<ExportResult, String> {
    let mut result = export_dxf_preview(snapshot, options)?;
    if reference_count > 0 || source_count > 0 {
        append_underlay_exclusion(
            &mut result.report,
            reference_count,
            source_count,
            derived_segment_count,
        );
    }
    Ok(result)
}

fn append_underlay_exclusion(
    report: &mut LossReport,
    views: usize,
    sources: usize,
    segments: Option<usize>,
) {
    let segment_scope = segments
        .map(|count| format!("{count} available derived world segments"))
        .unwrap_or_else(|| "derived geometry was not evaluated for this analytic export".into());
    let item = LossItem {
        code: "bim_reference_excluded_from_dxf".into(),
        severity: LossSeverity::Warning,
        message: format!(
            "DXF contains only manual analytic 2D entities. {views} BIM reference view definitions and {sources} BIM source elements are excluded; {segment_scope}. No BIM lines were converted to DraftEntity IDs."
        ),
        entity_handle: None,
        group_code: None,
    };
    // Match the accepted adapter's bounded report contract without changing its
    // manual entity_count/converted_count into unrelated BIM source counts.
    if report.items.len() < 512 {
        report.items.push(item.clone());
        report.warnings.push(item.clone());
        report.discarded_fields.push(item);
    } else {
        report.omitted_items = report.omitted_items.saturating_add(1);
    }
}

struct BudgetWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BudgetWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(buffer.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::Error::other("snapshot export byte budget exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded_json(value: &Value, limit: usize) -> Result<Vec<u8>, String> {
    let mut writer = BudgetWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafting_core::*;
    use std::sync::Arc;

    fn fixture() -> (DraftSnapshot, DisplaySnapshot, bim_underlay::UnderlayFrame) {
        let mut document = LabDocument::new();
        let drawing = document.default_drawing_id();
        document
            .apply(CommandEnvelope {
                command_id: "export-fixture".into(),
                expected_revision: 0,
                operations: vec![DraftOperation::Create {
                    drawing_id: drawing,
                    layer_id: LayerId::new(1),
                    geometry: Geometry::Line {
                        start: Point2::new(-1.0, 0.0),
                        end: Point2::new(1.0, 0.0),
                    },
                    style: DraftStyle::default(),
                }],
            })
            .unwrap();
        let snapshot = document.snapshot(drawing).unwrap();
        let display = build_display(
            &snapshot,
            &Camera2D::new(200.0, 100.0),
            &DisplayOptions::default(),
        )
        .unwrap();
        let underlay = bim_underlay::UnderlayFrame {
            segments: Arc::new(vec![bim_underlay::UnderlaySegment {
                element_id: ElementId::new(1),
                start: Point2::new(-2.0, 0.0),
                end: Point2::new(2.0, 0.0),
            }]),
            source_revision: snapshot.document_revision,
            generation: 3,
            status: json!({"quality":"approximate_mesh_projection","source_count":1}),
        };
        (snapshot, display, underlay)
    }
    #[test]
    fn mixed_id_map_keeps_colliding_bim_and_draft_numbers_qualified() {
        let (_, display, underlay) = fixture();
        let projected = project_underlay(&display, &underlay).unwrap();
        let map = mixed_id_map(&display, &projected, &json!({}));
        assert_eq!(
            map["manual_primitives"][0]["owner"],
            json!({"domain":"drafting","drawing_id":display.drawing_id,"entity_id":"1"})
        );
        assert_eq!(
            map["bim_reference_primitives"][0]["owner"],
            json!({"domain":"bim","element_id":1})
        );
        assert!(
            map["bim_reference_primitives"][0]
                .get("owner_entity_id")
                .is_none()
        );
    }
    #[test]
    fn projected_underlay_clips_huge_offscreen_segments_before_raster_work() {
        let (_, display, mut underlay) = fixture();
        underlay.segments = Arc::new(vec![
            bim_underlay::UnderlaySegment {
                element_id: ElementId::new(8),
                start: Point2::new(-1e10, 0.0),
                end: Point2::new(1e10, 0.0),
            },
            bim_underlay::UnderlaySegment {
                element_id: ElementId::new(9),
                start: Point2::new(-1e10, 1e10),
                end: Point2::new(1e10, 1e10),
            },
        ]);
        let projected = project_underlay(&display, &underlay).unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].element_id, ElementId::new(8));
        assert!(projected[0].start.x >= -CLIP_PADDING - 1e-3);
        assert!(projected[0].end.x <= display.camera.viewport_width + CLIP_PADDING + 1e-3);
    }
    #[test]
    fn svg_puts_qualified_bim_behind_accepted_analytic_manual_paths() {
        let (_, display, underlay) = fixture();
        let projected = project_underlay(&display, &underlay).unwrap();
        let svg = render_mixed_svg(
            &display,
            &json!({"snapshot_token":"mixed-export-test"}),
            &projected,
        )
        .unwrap();
        let bim = svg.find("<path data-domain=\"bim\"").unwrap();
        let manual = svg.find("<path data-domain=\"drafting\"").unwrap();
        assert!(bim < manual);
        assert!(svg.contains("data-element-id=\"1\""));
        assert!(svg.contains("data-entity-id=\"1\""));
        assert!(svg.contains("mixed-export-test"));
        assert!(svg.contains("approximate_mesh_projection"));
    }
    #[test]
    fn mixed_token_changes_when_bim_source_changes_and_keeps_manual_hash_scope() {
        let (snapshot, display, mut underlay) = fixture();
        let projected = project_underlay(&display, &underlay).unwrap();
        let first = mixed_metadata(
            &display,
            &snapshot,
            &underlay,
            &projected,
            1,
            "instance",
            1,
            Value::Null,
        )
        .unwrap();
        underlay.segments = Arc::new(vec![bim_underlay::UnderlaySegment {
            element_id: ElementId::new(1),
            start: Point2::new(-3.0, 0.0),
            end: Point2::new(2.0, 0.0),
        }]);
        let second = mixed_metadata(
            &display,
            &snapshot,
            &underlay,
            &project_underlay(&display, &underlay).unwrap(),
            1,
            "instance",
            1,
            Value::Null,
        )
        .unwrap();
        assert_ne!(first["snapshot_token"], second["snapshot_token"]);
        assert_eq!(
            first["source_binding"]["manual_snapshot_sha256"],
            second["source_binding"]["manual_snapshot_sha256"]
        );
    }
    #[test]
    fn dxf_candidate_preserves_reference_exclusion_and_full_critical_loss_report() {
        let (mut snapshot, _, underlay) = fixture();
        let output = manual_dxf_candidate(
            &snapshot,
            1,
            1,
            Some(underlay.segments.len()),
            &ExportOptions::default(),
        )
        .unwrap();
        assert_eq!(output.report.converted_count, 1);
        assert!(
            output
                .report
                .warnings
                .iter()
                .any(|item| item.code == "bim_reference_excluded_from_dxf")
        );
        let preview = drafting_interop::import_preview(
            &output.bytes,
            &drafting_interop::ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(preview.entities.len(), 1);
        snapshot.entities.values_mut().next().unwrap().style.color[3] = 0.5;
        let critical = manual_dxf_candidate(
            &snapshot,
            1,
            1,
            Some(underlay.segments.len()),
            &ExportOptions::default(),
        )
        .unwrap();
        assert!(critical.report.has_critical());
        assert!(
            critical
                .report
                .items
                .iter()
                .any(|item| item.code == "bim_reference_excluded_from_dxf")
        );
        assert!(
            critical
                .report
                .items
                .iter()
                .any(|item| item.severity == LossSeverity::Critical)
        );
    }
    #[test]
    fn native_frame_binding_reports_stale_submission_without_claiming_present() {
        let (_, display, underlay) = fixture();
        let wrong = json!({"draw":{"snapshot_token":"old","document_revision":display.document_revision,"camera_revision":display.camera.revision}});
        let binding = native_frame_binding(&display, &underlay, &wrong);
        assert_eq!(binding["native_submission_matches_export_source"], false);
        assert_eq!(binding["native_gpu_pass_or_present_proof"], false);
        let correct = json!({"draw":{"snapshot_token":format!("{}:bim:{}",display.token,underlay.generation),"document_revision":display.document_revision,"camera_revision":display.camera.revision}});
        assert_eq!(
            native_frame_binding(&display, &underlay, &correct)["native_submission_matches_export_source"],
            true
        );
    }
    #[test]
    fn png_embeds_mixed_token_without_changing_issued_manual_snapshot() {
        let (snapshot, display, underlay) = fixture();
        let original = display.token.clone();
        let projected = project_underlay(&display, &underlay).unwrap();
        let metadata = mixed_metadata(
            &display,
            &snapshot,
            &underlay,
            &projected,
            1,
            "instance",
            1,
            Value::Null,
        )
        .unwrap();
        let token = metadata["snapshot_token"].as_str().unwrap();
        let mut export = display.clone();
        export.token = token.to_owned();
        let segments = projected
            .iter()
            .map(|s| (s.start, s.end, s.style.clone()))
            .collect::<Vec<_>>();
        let bytes = drafting_export::render_png_with_underlay(&export, &segments).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"));
        assert!(
            bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes())
        );
        assert_eq!(display.token, original);
    }
    #[test]
    fn analytic_dxf_survives_unified_display_tessellation_budget_failure() {
        use ai_revit_core::{Command, Document, DraftCommand};
        let mut document = Document::new("DXF semantic export").unwrap();
        let mut transaction = document.begin_transaction("drawing", document.revision());
        transaction.push(Command::Draft(DraftCommand::CreateDrawing {
            name: "Plan".into(),
            plane: DrawingPlane::default(),
        }));
        let drawing = document.commit(transaction).unwrap().created_drawings[0];
        let layer = *document
            .drawing(drawing)
            .unwrap()
            .layers
            .keys()
            .next()
            .unwrap();
        let mut transaction = document.begin_transaction("circle", document.revision());
        transaction.push(Command::Draft(DraftCommand::CreateDraftEntity {
            drawing_id: drawing,
            layer_id: layer,
            geometry: Geometry::Circle {
                center: Point2::new(0.0, 0.0),
                radius: 1.0,
            },
            style: DraftStyle::default(),
        }));
        document.commit(transaction).unwrap();
        let revision = document.revision();
        let mut cache = DraftingCache::new(DraftingCacheLimits::default()).unwrap();
        cache.reset(&document, drawing, 1).unwrap();
        let mut camera = Camera2D::new(200.0, 100.0);
        camera.pixels_per_metre = 1e12;
        assert!(
            cache
                .prepare(&document, 1, &camera, &DisplayOptions::default())
                .is_err()
        );
        let snapshot = document.drafting_snapshot(drawing).unwrap();
        let exported =
            manual_dxf_candidate(&snapshot, 1, 1, None, &ExportOptions::default()).unwrap();
        assert_eq!(exported.report.converted_count, 1);
        assert_eq!(document.revision(), revision);
        assert!(
            exported
                .report
                .items
                .iter()
                .any(|item| item.code == "bim_reference_excluded_from_dxf")
        );
    }

    #[test]
    fn dxf_preview_and_refusal_bind_explicit_options_and_keep_authoritative_state() {
        use ai_revit_core::{Command, DraftCommand};
        let mut document = Document::new("DXF preview").unwrap();
        let mut transaction = document.begin_transaction("drawing", document.revision());
        transaction.push(Command::Draft(DraftCommand::CreateDrawing {
            name: "Plan".into(),
            plane: DrawingPlane::default(),
        }));
        let drawing = document.commit(transaction).unwrap().created_drawings[0];
        let layer = *document
            .drawing(drawing)
            .unwrap()
            .layers
            .keys()
            .next()
            .unwrap();
        let mut transaction = document.begin_transaction("critical-layer", document.revision());
        transaction.push(Command::Draft(DraftCommand::UpdateDraftLayer {
            drawing_id: drawing,
            layer_id: layer,
            name: None,
            visible: None,
            locked: None,
            style: Some(DraftStyle {
                color: [0.65, 0.85, 0.96, 0.5],
                width_px: 1.5,
            }),
        }));
        document.commit(transaction).unwrap();
        let before = document.to_json_string().unwrap();
        let candidate =
            prepare_analytic_dxf(&document, drawing, 7, ExportOptions::default()).unwrap();
        let refused = dxf_readiness(&candidate);
        assert_eq!(refused["written"], false);
        assert_eq!(refused["can_export"], false);
        assert_eq!(refused["refusal_reason"], "critical_loss_refusal");
        assert!(
            refused["loss_report"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["code"] == "layer_alpha_not_preserved")
        );
        let accepted = prepare_analytic_dxf(
            &document,
            drawing,
            7,
            ExportOptions {
                accept_critical_losses: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(dxf_readiness(&accepted)["can_export"], true);
        assert_eq!(dxf_readiness(&accepted)["written"], false);
        assert_eq!(accepted.output.report, candidate.output.report);
        assert_eq!(accepted.output.bytes, candidate.output.bytes);
        assert_ne!(
            accepted.metadata["snapshot_token"],
            candidate.metadata["snapshot_token"]
        );
        let millimetres = prepare_analytic_dxf(
            &document,
            drawing,
            7,
            ExportOptions {
                unit: "mm".into(),
                accept_critical_losses: true,
            },
        )
        .unwrap();
        assert_eq!(millimetres.metadata["units"], "millimetre");
        assert_ne!(millimetres.output.bytes, candidate.output.bytes);
        assert!(
            prepare_analytic_dxf(
                &document,
                drawing,
                7,
                ExportOptions {
                    unit: "unitless".into(),
                    accept_critical_losses: false
                }
            )
            .is_err()
        );
        assert_eq!(document.to_json_string().unwrap(), before);
    }
    #[test]
    fn output_byte_budget_rejects_before_growing_beyond_limit() {
        let mut writer = BudgetWriter {
            bytes: Vec::new(),
            limit: 4,
        };
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        assert_eq!(writer.bytes, b"1234");
        assert!(bounded_json(&json!({"large":"1234567890"}), 4).is_err());
    }
}
