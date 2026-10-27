//! Unified drawing API. All persistent edits enter the existing Core dispatcher.
use super::*;
use ai_revit_core::{DraftCommand, DraftCoordinateSystem, DraftSourceReference, DrawingId};
use serde_json::{Value, json};
use std::path::Path;

#[path = "drafting_export.rs"]
mod drafting_export;
#[path = "drawing_file_api.rs"]
mod drawing_file_api;
#[path = "drawing_import_api.rs"]
mod drawing_import_api;
#[path = "drawing_observation.rs"]
mod drawing_observation;
#[path = "drawing_ui_api.rs"]
mod drawing_ui_api;

impl App {
    pub(super) fn dispatch_drawing_request(
        &mut self,
        cx: &mut Cx,
        request: &agent_api::ApiCommand,
    ) -> Result<Value, agent_handler::RequestFailure> {
        let method = request.method.as_str();
        if method == "drawing.performance" {
            let mut result = self.drawing_performance();
            result["bim_reference_cache"] = self.bim_underlay_cache.metrics();
            return Ok(result);
        }
        if let Some(result) = drawing_observation::query(self, method, &request.params) {
            return result.map_err(Into::into);
        }
        if drawing_ui_api::handles(method) {
            if !drawing_ui_api::is_read_only(method) {
                self.require_api_token(request)?;
            }
            return drawing_ui_api::dispatch(self, cx, method, &request.params)
                .ok_or_else(|| agent_handler::RequestFailure::from("unknown drawing UI method"))?
                .map_err(Into::into);
        }
        // File reads also require authorization; they may reveal local source data.
        self.require_api_token(request)?;
        if drawing_import_api::handles(method) {
            return self
                .drawing_import_request(cx, method, &request.params)
                .map_err(Into::into);
        }
        match method {
            "drawing.export.dxf.preview" => self
                .drawing_export_dxf_preview(dxf_export_options(&request.params)?)
                .map_err(Into::into),
            "drawing.export"
            | "drawing.export.png"
            | "drawing.export.svg"
            | "drawing.export.id_map"
            | "drawing.export.dxf" => {
                let kind = method
                    .strip_prefix("drawing.export.")
                    .or_else(|| request.params.get("format").and_then(Value::as_str))
                    .ok_or("drawing.export requires format")?;
                let path = request
                    .params
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("drawing export requires path")?;
                if request.params.get("drawing_id").is_some()
                    && self.drafting_current_drawing_id()
                        != Some(required_drawing_id(&request.params)?)
                {
                    return Err(
                        "export drawing_id differs from the active drawing; activate it first"
                            .into(),
                    );
                }
                if kind == "dxf" {
                    self.drawing_export_dxf_options(
                        cx,
                        Path::new(path),
                        dxf_export_options(&request.params)?,
                    )
                    .map_err(Into::into)
                } else {
                    self.drawing_export_file(cx, kind, Path::new(path))
                        .map_err(Into::into)
                }
            }
            "drawing.reference.create" => {
                let id = required_drawing_id(&request.params)?;
                let command_id = request
                    .params
                    .get("command_id")
                    .and_then(Value::as_str)
                    .ok_or("reference creation requires command_id")?;
                let expected = request
                    .params
                    .get("expected_revision")
                    .and_then(Value::as_u64)
                    .ok_or("reference creation requires expected_revision")?;
                let source: DraftSourceReference = serde_json::from_value(
                    request
                        .params
                        .get("source")
                        .cloned()
                        .ok_or("reference creation requires explicit source")?,
                )
                .map_err(|e| e.to_string())?;
                let name = request
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("BIM XY section")
                    .to_owned();
                self.apply_unified_commands(
                    cx,
                    command_id,
                    expected,
                    vec![Command::Draft(DraftCommand::CreateViewDefinition {
                        drawing_id: id,
                        name,
                        source,
                    })],
                )
                .map_err(Into::into)
            }
            _ => Err(format!("unknown unified drawing method: {method}").into()),
        }
    }

    pub(super) fn apply_unified_commands(
        &mut self,
        cx: &mut Cx,
        command_id: &str,
        expected_revision: u64,
        commands: Vec<Command>,
    ) -> Result<Value, String> {
        let params = json!({"command_id":command_id,"expected_revision":expected_revision,
            "transaction_mode":"atomic","commands":commands});
        self.api_command_apply(cx, &params).map_err(|e| e.message)
    }

    /// A bounded, explicit XY section from current BIM selection or existing elements.
    pub(super) fn create_default_bim_reference(
        &mut self,
        cx: &mut Cx,
        drawing_id: DrawingId,
    ) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        if document.drawing(drawing_id).is_none() {
            return Err("drawing is absent".into());
        }
        let elements = if let Some(id) = self.selected_element {
            vec![id]
        } else {
            document
                .elements()
                .take(20_001)
                .map(|e| e.id)
                .collect::<Vec<_>>()
        };
        if elements.is_empty() {
            return Err("there are no BIM elements for this reference".into());
        }
        if elements.len() > 20_000 {
            return Err("select a BIM element; reference limit is 20000".into());
        }
        let revision = document.revision();
        let command = Command::Draft(DraftCommand::CreateViewDefinition {
            drawing_id,
            name: "BIM XY section · Z=0 m".into(),
            source: DraftSourceReference {
                element_ids: elements,
                elevation_m: Some(0.0),
                coordinate_system: DraftCoordinateSystem::Xy,
            },
        });
        let id = self.next_command_id("bim-xy-reference");
        let mut result = self.apply_unified_commands(cx, &id, revision, vec![command])?;
        result["representation_quality"] = json!("approximate_mesh_projection");
        result["projection"] = json!({"kind":"mesh_section_xy","elevation_m":0.0});
        Ok(result)
    }
}

fn required_drawing_id(params: &Value) -> Result<DrawingId, String> {
    let value = params
        .get("drawing_id")
        .and_then(Value::as_str)
        .ok_or("drawing_id must be a decimal string")?;
    let id = value.parse::<u64>().map_err(|_| "invalid drawing_id")?;
    if id == 0 || value != id.to_string() {
        return Err("drawing_id must be a canonical nonzero decimal string".into());
    }
    Ok(DrawingId::new(id))
}

fn dxf_export_options(params: &Value) -> Result<drafting_interop::ExportOptions, String> {
    let params = params.get("options").unwrap_or(params);
    let unit = match params.get("unit") {
        None => "metre",
        Some(value) => value.as_str().ok_or("DXF unit must be a string")?,
    };
    drafting_interop::DxfUnitChoice::parse_name(unit)?;
    let accept_critical_losses = match params.get("accept_critical_losses") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or("accept_critical_losses must be an explicit boolean")?,
    };
    Ok(drafting_interop::ExportOptions {
        unit: unit.into(),
        accept_critical_losses,
    })
}

#[cfg(test)]
mod export_option_tests {
    use super::*;
    #[test]
    fn dxf_loss_acceptance_and_units_require_explicit_supported_values() {
        assert!(
            !dxf_export_options(&json!({}))
                .unwrap()
                .accept_critical_losses
        );
        assert!(
            dxf_export_options(&json!({"unit":"mm","accept_critical_losses":true}))
                .unwrap()
                .accept_critical_losses
        );
        for invalid in [
            json!({"unit":"unknown"}),
            json!({"unit":1}),
            json!({"accept_critical_losses":"true"}),
        ] {
            assert!(dxf_export_options(&invalid).is_err());
        }
    }
}
