//! Application-level JSON API dispatch for the AI Revit desktop app.
//!
//! Requests are executed on the UI thread so document edits, scene rebuilds,
//! selection, and viewport export use the same app state as the native UI.

use super::*;
use ai_revit_core::CommitReceipt;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::TryRecvError;

pub(super) fn api_session_id() -> &'static str {
    static SESSION: OnceLock<String> = OnceLock::new();
    SESSION.get_or_init(|| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{stamp}", std::process::id())
    })
}

const MAX_API_REQUESTS_PER_TICK: usize = 8;

pub(super) struct RequestFailure {
    pub(super) message: String,
    validation_results: Vec<Value>,
    geometry_error: Option<Value>,
}
impl RequestFailure {
    pub(super) fn geometry_error(message: String, details: Value) -> Self {
        Self {
            message,
            validation_results: Vec::new(),
            geometry_error: Some(details),
        }
    }
}
impl From<String> for RequestFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            validation_results: Vec::new(),
            geometry_error: None,
        }
    }
}
impl From<&str> for RequestFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<ai_revit_core::CoreError> for RequestFailure {
    fn from(error: ai_revit_core::CoreError) -> Self {
        use ai_revit_core::{CoreError, EntityRef};
        let message = error.to_string();
        if let CoreError::Geotech(details) = &error {
            return Self::geometry_error(message, json!({"module":"geotech_core","error":details}));
        }
        let validation = match &error {
            CoreError::BimValidation { entity, .. } => {
                json!({"entity":entity,"code":"bim_validation","message":message})
            }
            CoreError::UnknownElement(id) => {
                json!({"entity":EntityRef::Element(*id),"code":"unknown_element","message":message})
            }
            CoreError::UnknownType(id) => {
                json!({"entity":EntityRef::Type(*id),"code":"unknown_type","message":message})
            }
            CoreError::UnknownLevel(id) => {
                json!({"entity":EntityRef::Level(*id),"code":"unknown_level","message":message})
            }
            CoreError::UnknownRelationship(id) => {
                json!({"relationship_id":id,"code":"unknown_relationship","message":message})
            }
            _ => json!({"code":"transaction_rejected","message":message}),
        };
        Self {
            message,
            validation_results: vec![validation],
            geometry_error: None,
        }
    }
}

fn owned_circular_snapshot_required() -> RequestFailure {
    RequestFailure::geometry_error(
        "OwnedCircularSnapshotRequired: external prepared circular snapshots are not accepted"
            .to_owned(),
        json!({"module":"geotech_circular","code":"OwnedCircularSnapshotRequired"}),
    )
}

/// Inspect decoded semantic enum tags before cloning or decoding archive data.
/// Only the existing Command::Engineering/SaveCircularSnapshot shape is denied.
fn reject_external_circular_snapshot_value(commands: &Value) -> Result<(), RequestFailure> {
    if commands.as_array().is_some_and(|commands| {
        commands.iter().any(|command| {
            command
                .get("engineering")
                .and_then(Value::as_object)
                .is_some_and(|engineering| engineering.contains_key("save_circular_snapshot"))
        })
    }) {
        return Err(owned_circular_snapshot_required());
    }
    Ok(())
}

fn reject_external_circular_snapshot_commands(commands: &[Command]) -> Result<(), RequestFailure> {
    if commands.iter().any(|command| {
        matches!(
            command,
            Command::Engineering(
                ai_revit_core::engineering::EngineeringCommand::SaveCircularSnapshot { .. }
            )
        )
    }) {
        return Err(owned_circular_snapshot_required());
    }
    Ok(())
}

/// Ordinary operations keep their original wire shape. The archive-save variant
/// has an explicit, non-command summary; serializing it never visits the payload.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum ApiOperationView<'a> {
    Ordinary(&'a ai_revit_core::OperationRecord),
    CircularArchiveSave(CircularArchiveSaveOperationSummary<'a>),
}

#[derive(serde::Serialize)]
struct CircularArchiveSaveOperationSummary<'a> {
    projection_format: &'static str,
    id: OperationId,
    command_id: &'a ai_revit_core::CommandId,
    command_index: u32,
    revision: u64,
    parent: Option<OperationId>,
    inputs: &'a [ElementId],
    outputs: &'a [ElementId],
    label: &'a str,
    operation_kind: &'static str,
    command_kind: &'static str,
    analysis_id: ai_revit_core::engineering::AnalysisCaseId,
    archive_digest: &'a str,
    bundle_digest: &'a str,
    report_digest: &'a str,
    captured_global_revision: u64,
    ground_id: u64,
    ground_source_operation: u64,
    case_source_operation: u64,
}

fn api_operation_view(operation: &ai_revit_core::OperationRecord) -> ApiOperationView<'_> {
    if let OperationKind::EngineeringChanged {
        analysis_id,
        command:
            ai_revit_core::engineering::EngineeringCommand::SaveCircularSnapshot { snapshot, .. },
    } = &operation.kind
    {
        let archive = snapshot.archive();
        let context = &archive.bundle.context;
        ApiOperationView::CircularArchiveSave(CircularArchiveSaveOperationSummary {
            projection_format: "circular-archive-save-operation-summary-v1",
            id: operation.id,
            command_id: &operation.command_id,
            command_index: operation.command_index,
            revision: operation.revision,
            parent: operation.parent,
            inputs: &operation.inputs,
            outputs: &operation.outputs,
            label: &operation.label,
            operation_kind: "engineering_changed",
            command_kind: "save_circular_snapshot",
            analysis_id: *analysis_id,
            archive_digest: &archive.archive_digest,
            bundle_digest: &archive.bundle_digest,
            report_digest: &archive.report_digest,
            captured_global_revision: context.captured_global_revision,
            ground_id: context.ground_id.0,
            ground_source_operation: context.ground_source_operation.0,
            case_source_operation: context.case_source_operation.0,
        })
    } else {
        ApiOperationView::Ordinary(operation)
    }
}

impl App {
    /// Drain a bounded number of requests so API traffic cannot starve the UI.
    pub(super) fn poll_agent_api(&mut self, cx: &mut Cx) {
        #[cfg(feature = "native-boundary-harness")]
        if crate::queue_harness::dispatch_held() { return; }
        for _request_index in 0..MAX_API_REQUESTS_PER_TICK {
            let next = match self.api_receiver.as_ref() {
                Some(receiver) => receiver.try_recv(),
                None => return,
            };
            match next {
                Ok(request) => {
                    if !request.try_start() {
                        let _ = request.reply.send(agent_api::failure(
                            request.id.clone(),
                            "request expired before execution",
                        ));
                        continue;
                    }
                    #[cfg(feature = "native-boundary-harness")]
                    crate::queue_harness::after_start(&request);
                    #[cfg(feature = "native-boundary-harness")]
                    let revision_before = self.document.as_ref().map_or(0, Document::revision);
                    #[cfg(feature = "native-boundary-harness")]
                    let owner_order_before = crate::queue_harness::owner_order_capture(
                        self.document.as_ref(), self.api_document_generation);
                    let response = self.handle_agent_request(cx, &request);
                    #[cfg(feature = "native-boundary-harness")]
                    crate::queue_harness::handler_finished(&request, &response, revision_before,
                        self.document.as_ref().map_or(0, Document::revision), owner_order_before,
                        crate::queue_harness::owner_order_capture(
                            self.document.as_ref(), self.api_document_generation));
                    request.complete();
                    let _ = request.reply.send(response);
                    #[cfg(feature = "native-boundary-harness")]
                    let _ = crate::queue_harness::after_reply(&request, || {
                        let ui = self.drawing_ui_state(cx).unwrap_or_else(|error| json!({"observation_error":error}));
                        let authority = self.document.as_ref().map(|document| json!({
                            "document_uuid":document.document_uuid,"revision":document.revision(),
                            "runtime_instance_id":document.runtime_instance_id(),"document_epoch":self.api_document_generation,
                            "canonical_sha256":document.to_json_string().ok().map(|text|crate::queue_harness::canonical_document_hash(text.as_bytes()))}));
                        json!({"authority":authority,"prepared_source":self.drafting_ui.frame.as_ref().map(|frame|&frame.source),
                            "prepared_snapshot_token":self.drafting_ui.frame.as_ref().map(|frame|&frame.token),
                            "last_drawn_telemetry":ui.get("draw"),"camera":self.drafting_ui.camera,
                            "dpi_factor":ui.get("dpi_factor"),"remaining_requests_in_tick":MAX_API_REQUESTS_PER_TICK-_request_index-1})
                    });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.api_receiver = None;
                    break;
                }
            }
        }
    }

    /// Dispatch one API command and return its JSON-RPC-style response envelope.
    pub(super) fn handle_agent_request(
        &mut self,
        cx: &mut Cx,
        request: &agent_api::ApiCommand,
    ) -> Value {
        ai_revit_geometry::refresh_gpu_compute_budget(gpu::compute_device_budget(cx));
        let revision_before = self.document.as_ref().map_or(0, Document::revision);
        let selection_before = self.selected_element;
        let elements_before = if matches!(request.method.as_str(), "document.open" | "open") {
            self.document
                .as_ref()
                .map(|d| d.elements().map(|e| e.id).collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        match self.dispatch_agent_request(cx, request) {
            Ok(mut result) => {
                // Existing envelope and fields remain intact. Non-document writes
                // expose their scope and do not pretend to create BIM history.
                let scope = match request.method.as_str() {
                    "element.select" | "viewport.pick_element" | "viewport.pick_world" => {
                        Some("selection")
                    }
                    "view.fit_all"
                    | "view.wall_front"
                    | "view.host_plane"
                    | "geometry.set_display" => Some("view"),
                    "document.save" | "save" | "view.export_png" => Some("file_write"),
                    "document.open" | "open" => Some("document_replace"),
                    "ui.invoke" => Some("ui"),
                    "undo" | "document.undo" | "redo" | "document.redo" => Some("history"),
                    _ => None,
                };
                if let Some(scope) = scope {
                    if let Some(receipt) = result.get("receipt").cloned() {
                        for key in [
                            "transaction_id",
                            "revision_before",
                            "revision_after",
                            "affected_ids",
                            "dirty_ids",
                            "validation_results",
                            "delta",
                            "performance",
                        ] {
                            if let Some(value) = receipt.get(key) {
                                result[key] = value.clone();
                            }
                        }
                    }
                    if result.get("transaction_id").is_none() {
                        let transaction_id = self.next_command_id(&format!("api-{scope}"));
                        let revision_after = self.document.as_ref().map_or(0, Document::revision);
                        let mut affected = match scope {
                            "selection" | "ui" if selection_before != self.selected_element => {
                                selection_before
                                    .into_iter()
                                    .chain(self.selected_element)
                                    .collect::<Vec<_>>()
                            }
                            "document_replace" => elements_before
                                .into_iter()
                                .chain(
                                    self.document
                                        .as_ref()
                                        .into_iter()
                                        .flat_map(|d| d.elements().map(|e| e.id)),
                                )
                                .collect(),
                            _ => Vec::new(),
                        };
                        affected.sort();
                        affected.dedup();
                        result["transaction_id"] = json!(transaction_id);
                        result["revision_before"] = json!(revision_before);
                        result["revision_after"] = json!(revision_after);
                        result["affected_ids"] = json!(affected);
                        result["dirty_ids"] = json!([]);
                        result["validation_results"] = json!([]);
                    }
                    result["transaction_scope"] = json!(scope);
                }
                let replayed = result
                    .get("replayed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let geometry_us = if replayed {
                    json!(0)
                } else {
                    self.rebuild_observation()["rebuild_duration_us"].clone()
                };
                if let Some(performance) =
                    result.get_mut("performance").and_then(Value::as_object_mut)
                {
                    performance.insert("geometry_rebuild_us".into(), geometry_us);
                }
                agent_api::success(request.id.clone(), result)
            }
            Err(error) => {
                let mut response = agent_api::failure(request.id.clone(), error.message.clone());
                if let Some(details) = &error.geometry_error {
                    response["geometry_error"] = details.clone();
                }
                if (request.method.starts_with("drawing.import.")
                    && request.method.ends_with(".commit"))
                    || request.method == "drawing.reference.create"
                    || matches!(
                        request.method.as_str(),
                        "command.apply"
                            | "element.select"
                            | "viewport.pick_element"
                            | "viewport.pick_world"
                            | "view.wall_front"
                            | "view.host_plane"
                            | "geometry.set_display"
                            | "view.fit_all"
                            | "document.open"
                            | "open"
                            | "document.save"
                            | "save"
                            | "undo"
                            | "document.undo"
                            | "redo"
                            | "document.redo"
                            | "view.export_png"
                            | "ui.invoke"
                    )
                {
                    let transaction_id = request
                        .params
                        .get("command_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| self.next_command_id("api-rejected"));
                    response["transaction_id"] = json!(transaction_id);
                    response["revision_before"] = json!(revision_before);
                    response["revision_after"] =
                        json!(self.document.as_ref().map_or(0, Document::revision));
                    response["affected_ids"] = json!([]);
                    response["dirty_ids"] = json!([]);
                    response["validation_results"] = if error.validation_results.is_empty() {
                        json!([{"code":"request_rejected","message":error.message}])
                    } else {
                        json!(error.validation_results)
                    };
                }
                response
            }
        }
    }

    fn dispatch_agent_request(
        &mut self,
        cx: &mut Cx,
        request: &agent_api::ApiCommand,
    ) -> Result<Value, RequestFailure> {
        #[cfg(target_os = "linux")]
        {
            if request.method.starts_with("diagnostic.lifecycle.") {
                return self.dispatch_lifecycle_request(cx, request).map_err(Into::into);
            }
            if self.lifecycle_blocks_business() {
                return Err("LIFECYCLE_BUSINESS_ADMISSION_CLOSED".into());
            }
            if request.method.starts_with("diagnostic.gui.") {
                return self.dispatch_geotech_gui_request(cx, request).map_err(Into::into);
            }
            if self.gui_persistence_enabled() && request.method != "document.info" {
                return Err("GUI_V2_BUSINESS_API_NOT_ADMITTED".into());
            }
        }
        if request.method.starts_with("geotech.circular.")
            || request.method.starts_with("geotech.ground.")
            || request.method.starts_with("geotech.soil_material.")
        {
            return self.dispatch_circular_request(cx, request);
        }
        if request.method.starts_with("geotech.") {
            return self.dispatch_geotech_request(cx, request);
        }
        if request.method.starts_with("drawing.")
            || matches!(
                request.method.as_str(),
                "workspace.set" | "project.workspace.set"
            )
        {
            return self.dispatch_drawing_request(cx, request);
        }
        match request.method.as_str() {
            "capabilities" => Ok(self.api_capabilities()),
            "gpu.observe" => {
                let mut result = gpu::observe(cx);
                result["process_id"] = json!(std::process::id());
                result["session_id"] = json!(api_session_id());
                result["requested_policy"] = json!(
                    std::env::var("AI_REVIT_GPU_MODE")
                        .unwrap_or_else(|_| "nvidia-preferred".into())
                );
                let requested_nvidia = result["requested_policy"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("nvidia-");
                let nvidia = result["actual_adapter"]["vendor_id"]
                    .as_u64()
                    .map(|vendor| vendor == 0x10de);
                result["nvidia_match"] = json!(nvidia);
                result["fallback_used"] = json!(nvidia.map(|matched| requested_nvidia && !matched));
                result["compute"] = ai_revit_geometry::gpu_compute_observe();
                Ok(result)
            }
            "document.info" => self.api_document_info().map_err(Into::into),
            "elements.list" => self.api_elements_list().map_err(Into::into),
            "elements.get" | "element.get" => {
                self.api_element_get(&request.params).map_err(Into::into)
            }
            "operations.list" => self.api_operations_list().map_err(Into::into),
            "classes.list" => self.api_classes_list().map_err(Into::into),
            "geometry.failures" => self.api_geometry_failures().map_err(Into::into),
            "geometry.inspect"
            | "geometry.dependencies"
            | "geometry.compare"
            | "geometry.section"
            | "geometry.capabilities"
            | "quantities.get"
            | "quantities.batch"
            | "quantities.by_class"
            | "quantities.by_type"
            | "quantities.by_level"
            | "spatial.bounds_query"
            | "spatial.intersects_query" => self.geometry_query(&request.method, &request.params),
            "document.history_metrics" => {
                let document = self.document.as_ref().ok_or("no document is open")?;
                let metrics = document.history_metrics().map_err(|e| e.to_string())?;
                serde_json::to_value(metrics).map_err(|e| e.to_string().into())
            }
            "history.delta" => {
                let d = self.document.as_ref().ok_or("no document is open")?;
                let revision = request
                    .params
                    .get("revision")
                    .and_then(Value::as_u64)
                    .ok_or("history.delta requires unsigned revision")?;
                serde_json::to_value(d.history_delta(revision)?).map_err(|e| e.to_string().into())
            }
            "history.replay_info" => {
                let d = self.document.as_ref().ok_or("no document is open")?;
                let revision = request
                    .params
                    .get("revision")
                    .and_then(Value::as_u64)
                    .ok_or("history.replay_info requires unsigned revision")?;
                let started = std::time::Instant::now();
                let replay = d.replay_revision(revision)?;
                Ok(
                    json!({"revision":replay.revision(),"requested_revision":revision,"document_id":replay.document_uuid,
                    "element_count":replay.elements().len(),"index_metrics":replay.index_metrics(),
                    "document_mutated":false,"reconstruction_time_us":started.elapsed().as_micros().min(u64::MAX as u128) as u64}),
                )
            }
            "viewport.raycast" => {
                let point = super::geometry_api::vector3(
                    request
                        .params
                        .get("world_point")
                        .ok_or("viewport.raycast requires world_point")?,
                )?;
                let viewport = self.viewport_widget(cx);
                let viewport = viewport
                    .borrow::<CadViewport>()
                    .ok_or("viewport is not ready")?;
                let projected = viewport
                    .project_world(point)?
                    .ok_or("world point is behind the camera")?;
                let hit = viewport.raycast_screen(projected.screen.x, projected.screen.y)?;
                Ok(
                    json!({"representation_quality":"approximate_mesh","hit":hit.map(|h|json!({"element_id":h.object_id,"world_point":h.world_point,"world_normal":h.world_normal})),"world_point":point,"screen_point":[projected.screen.x,projected.screen.y]}),
                )
            }
            "document.observe"
            | "element.inspect"
            | "element.relationships"
            | "element.dependencies"
            | "element.neighbors"
            | "element.delete_impact"
            | "type.get"
            | "type.instances"
            | "types.list"
            | "level.get"
            | "level.elements"
            | "levels.list"
            | "relationship.get"
            | "relationship.list"
            | "history.diff" => self
                .semantic_query(&request.method, &request.params)
                .map_err(Into::into),
            "ui.state" => {
                let mut state = self.semantic_ui_state(cx);
                state["layout"] = self.layout_observation(cx);
                state["structural_configurator"] = self.structural_configurator_ui_state(cx);
                Ok(state)
            }
            "ui.invoke" => {
                self.require_api_token(request)?;
                let control = request
                    .params
                    .get("control_id")
                    .and_then(Value::as_str)
                    .ok_or("ui.invoke requires control_id")?;
                if control == "openings.inputs" {
                    self.configure_opening_inputs(cx, &request.params)
                        .map_err(Into::into)
                } else if control == "properties.inputs" {
                    self.configure_property_editor(cx, &request.params)
                        .map_err(Into::into)
                } else {
                    self.invoke_semantic_control(cx, control)
                        .map_err(Into::into)
                }
            }
            "element.select" => {
                self.require_api_token(request)?;
                self.api_element_select(cx, &request.params)
                    .map_err(Into::into)
            }
            "viewport.pick_element" => {
                self.require_api_token(request)?;
                let id = numeric_element_id(&request.params)?;
                let viewport = self.viewport_widget(cx);
                let mut picked_result = Err("viewport is not ready".to_owned());
                let actions = cx.capture_actions(|cx| {
                    if let Some(mut viewport) = viewport.borrow_mut::<CadViewport>() {
                        picked_result = viewport.invoke_pick_element(cx, id);
                    }
                });
                let picked = picked_result?;
                // Dispatch the emitted picking actions through the native UI
                // handler. No direct core selection bypass is allowed here.
                self.handle_actions(cx, &actions);
                if self.selected_element.map(ElementId::get) != Some(picked) {
                    return Err("viewport action did not synchronize the core selection".into());
                }
                Ok(
                    json!({"picked_element_id":picked,"selection_path":"viewport_actions","selection":self.semantic_ui_state(cx)}),
                )
            }
            "viewport.pick_world" => {
                self.require_api_token(request)?;
                let point = super::geometry_api::vector3(
                    request
                        .params
                        .get("world_point")
                        .ok_or("viewport.pick_world requires world_point")?,
                )?;
                let viewport = self.viewport_widget(cx);
                let mut result = Err("viewport is not ready".to_owned());
                let actions = cx.capture_actions(|cx| {
                    if let Some(mut viewport) = viewport.borrow_mut::<CadViewport>() {
                        result = viewport.invoke_pick_world(cx, point);
                    }
                });
                let picked = result?;
                self.handle_actions(cx, &actions);
                if self.selected_element.map(ElementId::get) != picked {
                    return Err("viewport actions did not synchronize ray selection".into());
                }
                Ok(
                    json!({"picked_element_id":picked,"selection_path":"viewport_actions","selection":self.semantic_ui_state(cx)}),
                )
            }
            "view.wall_front" => {
                self.require_api_token(request)?;
                let id = ElementId::new(numeric_element_id(&request.params)?);
                self.set_wall_front_view(cx, id).map_err(Into::into)
            }
            "view.host_plane" => {
                self.require_api_token(request)?;
                let id = ElementId::new(numeric_element_id(&request.params)?);
                self.set_host_view(cx, id).map_err(Into::into)
            }
            "geometry.set_display" => {
                self.require_api_token(request)?;
                let show = request
                    .params
                    .get("show_opening_volumes")
                    .and_then(Value::as_bool)
                    .ok_or("geometry.set_display requires show_opening_volumes as boolean")?;
                self.set_show_opening_volumes(cx, show).map_err(Into::into)
            }
            "view.fit_all" => {
                self.require_api_token(request)?;
                let widget = self.viewport_widget(cx);
                let mut viewport = widget
                    .borrow_mut::<CadViewport>()
                    .ok_or("viewport is not ready")?;
                viewport
                    .viewer_mut()
                    .fit_scene(1.15)
                    .map_err(|e| e.to_string())?;
                viewport.redraw(cx);
                Ok(json!({"fitted":true}))
            }
            "document.save" | "save" => {
                self.require_api_token(request)?;
                self.api_document_save(cx, &request.params)
                    .map_err(Into::into)
            }
            "document.open" | "open" => {
                self.require_api_token(request)?;
                self.api_document_open(cx, &request.params)
                    .map_err(Into::into)
            }
            "command.apply" => {
                self.require_api_token(request)?;
                self.api_command_apply(cx, &request.params)
            }
            "undo" | "document.undo" => {
                self.require_api_token(request)?;
                self.api_undo(cx).map_err(Into::into)
            }
            "redo" | "document.redo" => {
                self.require_api_token(request)?;
                self.api_redo(cx).map_err(Into::into)
            }
            "view.export_png" => {
                self.require_api_token(request)?;
                self.api_export_png(cx, &request.params).map_err(Into::into)
            }
            method => Err(format!("unknown AI Revit API method: {method}").into()),
        }
    }

    fn api_capabilities(&self) -> Value {
        json!({
            "api": "ai-revit-desktop",
            "version": 1,
            "document_open": self.document.is_some(),
            "experimental_linux_lifecycle": {
                "registered": cfg!(target_os = "linux"),
                "methods": ["diagnostic.lifecycle.status", "diagnostic.lifecycle.close"],
                "session_identity": "lifecycle bootstrap session; distinct from legacy api_session_id",
                "native_xid_verified": false
            },
            "circular_methods": geotech_circular_api::CIRCULAR_API_METHODS,
            "read_methods": [
                "capabilities",
                "gpu.observe",
                "document.info",
                "elements.list",
                "elements.get",
                "operations.list",
                "classes.list",
                "geometry.failures", "document.observe", "element.inspect", "element.relationships",
                "element.dependencies", "element.neighbors", "element.delete_impact",
                "type.get", "type.instances", "types.list", "level.get", "level.elements", "levels.list",
                "relationship.get", "relationship.list", "history.diff", "ui.state",
                "geometry.inspect", "geometry.dependencies", "geometry.compare", "geometry.section", "geometry.capabilities",
                "document.history_metrics", "history.delta", "history.replay_info", "viewport.raycast",
                "quantities.get", "quantities.batch", "quantities.by_class", "quantities.by_type", "quantities.by_level",
                "spatial.bounds_query", "spatial.intersects_query",
                "drawing.list", "drawing.observe", "drawing.entities", "drawing.entity.inspect", "drawing.layers",
                "drawing.view.state", "drawing.ui.state", "drawing.performance",
                "geotech.slope.inspect", "geotech.slope.geometry", "geotech.slope.report"
            ],
            "write_methods": [
                "element.select",
                "document.save",
                "document.open",
                "command.apply",
                "undo",
                "redo",
                "view.export_png", "view.fit_all", "ui.invoke", "viewport.pick_element", "viewport.pick_world", "view.wall_front", "view.host_plane", "geometry.set_display",
                "workspace.set", "drawing.activate", "drawing.camera.set", "drawing.camera.pan", "drawing.camera.zoom", "drawing.camera.fit",
                "drawing.selection.set", "drawing.ui.tool", "drawing.ui.numeric", "drawing.ui.native_input",
                "drawing.reference.create", "drawing.import.dxf.preview", "drawing.import.dxf.commit",
                "drawing.import.lab.preview", "drawing.import.lab.commit", "drawing.export",
                "geotech.slope.create", "geotech.slope.update", "geotech.slope.analyze", "geotech.slope.candidates"
            ],
            "authentication": {
                "environment_variable": if cfg!(target_os = "linux") { Value::Null } else { json!("AI_REVIT_API_TOKEN") },
                "source": if cfg!(target_os = "linux") { "private_fixed_0600_startup_token" } else { "environment" },
                "required_for_write_methods": true,
                "configured": api_token_is_configured()
            }
        })
    }

    fn api_document_info(&self) -> Result<Value, String> {
        let Some(document) = self.document.as_ref() else {
            return Ok(json!({ "open": false }));
        };
        Ok(json!({
            "open": true,
            "name": document.name(),
            "document_uuid": document.document_uuid.to_string(),
            "revision": document.revision(),
            "element_count": document.elements().len(),
            "class_count": document.classes().len(),
            "operation_count": document.operation_tree().len(),
            "next_ids": document.allocation_preview(),
            "open_generation": self.api_document_generation,
            "session_id": api_session_id(),
            "selected_element_id": self.selected_element.map(|id| id.get()),
            "selected_operation_id": self.selected_operation.map(|id| id.get())
        }))
    }

    fn api_elements_list(&self) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or_else(no_document_error)?;
        let elements = document.elements().collect::<Vec<_>>();
        serde_json::to_value(elements)
            .map_err(|error| format!("could not serialize elements: {error}"))
    }

    fn api_element_get(&self, params: &Value) -> Result<Value, String> {
        let element_id = numeric_element_id(params)?;
        let document = self.document.as_ref().ok_or_else(no_document_error)?;
        let element = document
            .element(ElementId::new(element_id))
            .ok_or_else(|| format!("element {element_id} was not found"))?;
        serde_json::to_value(element)
            .map_err(|error| format!("could not serialize element {element_id}: {error}"))
    }

    fn api_operations_list(&self) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or_else(no_document_error)?;
        let operations = document
            .operation_tree()
            .map(api_operation_view)
            .collect::<Vec<_>>();
        serde_json::to_value(operations)
            .map_err(|error| format!("could not serialize operations: {error}"))
    }

    fn api_classes_list(&self) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or_else(no_document_error)?;
        let classes = document.classes().collect::<Vec<_>>();
        serde_json::to_value(classes)
            .map_err(|error| format!("could not serialize classes: {error}"))
    }

    fn api_geometry_failures(&self) -> Result<Value, String> {
        self.document.as_ref().ok_or_else(no_document_error)?;
        let failures = self
            .geometry_cache
            .geometry_errors()
            .map(|failure| {
                json!({
                    "element_id": failure.element_id.get(),
                    "host_id": failure.host_id,
                    "opening_id": failure.opening_id,
                    "error_code": failure.error_code,
                    "canonical_error_code":failure.canonical_error_code,
                    "backend":failure.backend,
                    "representation_quality":failure.representation_quality,
                    "code": format!("{:?}", failure.code),
                    "message": failure.message
                })
            })
            .collect::<Vec<_>>();
        let resolution_failures: Vec<_> = self
            .geometry_cache
            .resolution_errors()
            .map(|(id, error)| json!({"element_id":id,"message":error}))
            .collect();
        Ok(
            json!({ "count": failures.len()+resolution_failures.len(), "failures": failures, "resolution_failures":resolution_failures }),
        )
    }

    fn api_element_select(&mut self, cx: &mut Cx, params: &Value) -> Result<Value, String> {
        let element_id = numeric_element_id(params)?;
        let id = self
            .document
            .as_ref()
            .ok_or_else(no_document_error)?
            .elements()
            .find(|element| element.id.get() == element_id)
            .map(|element| element.id)
            .ok_or_else(|| format!("element {element_id} was not found"))?;
        self.select_element(cx, id);
        Ok(json!({ "selected_element_id": id.get() }))
    }

    pub(super) fn api_document_save(
        &mut self,
        cx: &mut Cx,
        params: &Value,
    ) -> Result<Value, String> {
        let path = self.request_path(cx, params);
        if path.trim().is_empty() {
            return Err("document.save requires a non-empty path".to_owned());
        }
        let result = (|| -> Result<(),String> {
            let bytes = self.document.as_ref().ok_or_else(no_document_error)?
                .to_json_string().map_err(|error|error.to_string())?;
            #[cfg(target_os = "linux")]
            if self.gui_persistence_enabled() {
                return self.gui_persistence_write(&path,bytes.as_bytes());
            }
            file_guard::FilePolicy::from_environment()?.write_atomic(Path::new(&path),bytes.as_bytes())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.ui.widget(cx, ids!(path_input)).set_text(cx, &path);
                self.set_status(cx, &format!("Файл сохранен: {path}"));
                Ok(json!({ "path": path, "saved": true }))
            }
            Err(error) => {
                self.set_status(cx, &format!("Не удалось сохранить файл: {error}"));
                Err(error)
            }
        }
    }

    pub(super) fn api_document_open(
        &mut self,
        cx: &mut Cx,
        params: &Value,
    ) -> Result<Value, String> {
        let path = self.request_path(cx, params);
        if path.trim().is_empty() {
            return Err("document.open requires a non-empty path".to_owned());
        }
        #[cfg(target_os = "linux")]
        let bytes = if self.gui_persistence_enabled() {
            self.gui_persistence_read(&path)?
        } else {
            file_guard::FilePolicy::from_environment()?.read(Path::new(&path),file_guard::MAX_PROJECT_BYTES)?.1
        };
        #[cfg(not(target_os = "linux"))]
        let (_, bytes) = file_guard::FilePolicy::from_environment()?.read(Path::new(&path),file_guard::MAX_PROJECT_BYTES)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        match Document::from_json_str(text) {
            Ok(document) => {
                let name = document.name().to_owned();
                let revision = document.revision();
                self.document = Some(document);
                self.api_document_generation = self.api_document_generation.saturating_add(1);
                self.geometry_cache.invalidate_all();
                self.selected_element = None;
                self.selected_operation = None;
                self.semantic_selection = None;
                self.history_page = 0;
                self.sync_document_defaults();
                self.ui.widget(cx, ids!(path_input)).set_text(cx, &path);
                self.refresh_unified_after_commit(cx, true);
                self.set_status(cx, &format!("Открыт файл: {path}"));
                // Open shows retained evidence without dispatching a solver or
                // granting it this process's current-job/save authority.
                let archive_load = self.circular_refresh_catalog(cx)
                    .and_then(|()| self.circular_native_restore_archive_after_open(cx));
                if let Err(error) = archive_load {
                    self.set_status(cx, &format!("Файл открыт; архив расчёта недоступен: {error}"));
                }
                Ok(json!({
                    "path": path,
                    "opened": true,
                    "name": name,
                    "revision": revision
                }))
            }
            Err(error) => {
                let error = error.to_string();
                self.set_status(cx, &format!("Не удалось открыть файл: {error}"));
                Err(error)
            }
        }
    }

    pub(super) fn api_command_apply(
        &mut self,
        cx: &mut Cx,
        params: &Value,
    ) -> Result<Value, RequestFailure> {
        if let Some(expected) = params.get("expected_session_id") {
            if expected.as_str() != Some(api_session_id()) {
                return Err("application session changed since the allocation quote".into());
            }
        }
        if let Some(expected) = params.get("expected_document_id") {
            let id = expected
                .as_str()
                .ok_or("expected_document_id must be a string")?;
            if self
                .document
                .as_ref()
                .map(|d| d.document_uuid.to_string())
                .as_deref()
                != Some(id)
            {
                return Err("document identity changed since the allocation quote".into());
            }
        }
        if let Some(expected) = params.get("expected_open_generation") {
            let generation = expected
                .as_u64()
                .ok_or("expected_open_generation must be an unsigned integer")?;
            if generation != self.api_document_generation {
                return Err("document was reopened since the allocation quote".into());
            }
        }
        let command_id = params
            .get("command_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "command.apply requires a non-empty string command_id".to_owned())?
            .to_owned();
        let expected_revision = params
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                "command.apply requires an unsigned integer expected_revision".to_owned()
            })?;
        let commands_value = params
            .get("commands")
            .filter(|value| value.is_array())
            .ok_or_else(|| "command.apply requires commands as an array".to_owned())?;
        reject_external_circular_snapshot_value(commands_value)?;
        let commands: Vec<Command> = serde_json::from_value(commands_value.clone())
            .map_err(|error| format!("invalid typed command batch: {error}"))?;
        reject_external_circular_snapshot_commands(&commands)?;
        if params
            .get("transaction_mode")
            .is_some_and(|mode| mode.as_str() != Some("atomic"))
        {
            return Err("v0.2 supports only atomic transaction_mode".into());
        }

        let receipt = self.apply_typed_commands(cx, command_id, expected_revision, commands)?;
        let replayed = receipt.replayed;
        let mut result = serde_json::to_value(receipt)
            .map_err(|error| format!("could not serialize command receipt: {error}"))?;
        result["geometry_rebuild"] = if replayed {
            json!({"rebuilt_element_count":0,"rebuild_duration_ms":0,"rebuild_duration_us":0,"rebuilt_element_ids":[],"removed_element_ids":[],"quantity_rebuilt_element_count":0,"spatial_updated_element_count":0})
        } else {
            self.rebuild_observation()
        };
        Ok(result)
    }

    /// Shared commit/refresh path for validated adapters and trusted native owners.
    /// Native callers establish their actual job/context authority before entry.
    pub(super) fn apply_typed_commands(
        &mut self,
        cx: &mut Cx,
        command_id: String,
        expected_revision: u64,
        commands: Vec<Command>,
    ) -> Result<CommitReceipt, RequestFailure> {
        let receipt = {
            let document = self.document.as_mut().ok_or_else(no_document_error)?;
            let mut transaction = document.begin_transaction(command_id, expected_revision);
            for command in commands {
                transaction.push(command);
            }
            document.commit(transaction).map_err(RequestFailure::from)?
        };

        if !receipt.replayed {
            self.sync_document_defaults();
            self.selected_element = None;
            self.selected_operation = None;
            self.semantic_selection = None;
            self.refresh_unified_after_commit(cx, false);
            if let Some(operation_id) = receipt.operation_ids.last().copied() {
                self.select_operation(cx, operation_id);
            }
        }
        self.set_status(
            cx,
            &format!("Команда API применена · ревизия {}", receipt.revision),
        );
        Ok(receipt)
    }

    fn api_undo(&mut self, cx: &mut Cx) -> Result<Value, String> {
        let receipt = self
            .document
            .as_mut()
            .ok_or_else(no_document_error)?
            .undo()
            .map_err(|error| error.to_string())?;
        match receipt {
            Some(receipt) => {
                self.selected_element = None;
                self.selected_operation = None;
                self.semantic_selection = None;
                self.sync_document_defaults();
                self.refresh_unified_after_commit(cx, false);
                self.set_status(cx, "Отмена выполнена");
                serde_json::to_value(receipt)
                    .map(|receipt| json!({ "changed": true, "receipt": receipt }))
                    .map_err(|error| format!("could not serialize undo receipt: {error}"))
            }
            None => {
                self.set_status(cx, "Нет операций для отмены");
                let revision = self.document.as_ref().map_or(0, Document::revision);
                Ok(json!({ "changed": false, "revision": revision }))
            }
        }
    }

    fn api_redo(&mut self, cx: &mut Cx) -> Result<Value, String> {
        let receipt = self
            .document
            .as_mut()
            .ok_or_else(no_document_error)?
            .redo()
            .map_err(|error| error.to_string())?;
        match receipt {
            Some(receipt) => {
                self.selected_element = None;
                self.selected_operation = None;
                self.semantic_selection = None;
                self.sync_document_defaults();
                self.refresh_unified_after_commit(cx, false);
                self.set_status(cx, "Повтор выполнен");
                serde_json::to_value(receipt)
                    .map(|receipt| json!({ "changed": true, "receipt": receipt }))
                    .map_err(|error| format!("could not serialize redo receipt: {error}"))
            }
            None => {
                self.set_status(cx, "Нет операций для повтора");
                let revision = self.document.as_ref().map_or(0, Document::revision);
                Ok(json!({ "changed": false, "revision": revision }))
            }
        }
    }

    fn api_export_png(&mut self, cx: &mut Cx, params: &Value) -> Result<Value, String> {
        let mut path = PathBuf::from(self.request_path(cx, params));
        if path.as_os_str().is_empty() {
            return Err("view.export_png requires a non-empty path".to_owned());
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            path.set_extension("png");
        }
        path = file_guard::FilePolicy::from_environment()?.output(&path)?;
        // Circular uses its own CadViewport. Never export the hidden BIM view
        // while the circular panel is active, including its 2D section mode.
        let export_viewport = if self.circular_ui.panel.active {
            self.ui.widget(cx, ids!(circular_viewport))
        } else {
            self.viewport_widget(cx)
        };
        let result = if self.circular_ui.panel.active && !self.circular_ui.panel.view_3d {
            if let Some(mut viewport) = export_viewport.borrow_mut::<CadViewport>() {
                viewport.cancel_gpu_export(cx);
            }
            Err("GPU_VIEWPORT_UNAVAILABLE: circular 2D view has no 3D viewport; select circular 3D before GPU PNG export".to_owned())
        } else {
            export_viewport
                .borrow_mut::<CadViewport>()
                .ok_or_else(|| "3D viewport is not ready".to_owned())
                .and_then(|mut viewport| viewport.try_export_gpu_png(cx, &path))
        };
        match result {
            Ok(mut observation) => {
                self.set_status(
                    cx,
                    &format!("Экспорт PNG с GPU завершён · {}", path.display()),
                );
                observation["path"] = json!(path);
                observation["exported"] = json!(true);
                Ok(observation)
            }
            Err(error) => {
                self.set_status(cx, &format!("Ошибка экспорта PNG: {error}"));
                Err(error)
            }
        }
    }

    fn request_path(&self, cx: &Cx, params: &Value) -> String {
        params
            .get("path")
            .and_then(Value::as_str)
            .or_else(|| params.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| self.path_text(cx))
    }

    pub(super) fn require_api_token(&self, request: &agent_api::ApiCommand) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        let expected = crate::linux_lifecycle_wire::expected_token()
            .ok_or("LIFECYCLE_AUTH_NOT_INITIALIZED")?.to_owned();
        #[cfg(not(target_os = "linux"))]
        let expected = std::env::var("AI_REVIT_API_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                "mutating API methods are disabled because AI_REVIT_API_TOKEN is not set to a non-empty value".to_owned()
            })?;
        let Some(provided) = request.token.as_deref().filter(|token| !token.is_empty()) else {
            return Err("mutating API method requires a valid token".to_owned());
        };
        if !api_token_matches(provided.as_bytes(), expected.as_bytes()) {
            return Err("mutating API method requires a valid token".to_owned());
        }
        Ok(())
    }
}

fn api_token_matches(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

#[cfg(test)]
mod unified_auth_tests {
    #[test]
    fn exact_token_required_including_length_and_nul_bytes() {
        assert!(super::api_token_matches(b"same-value", b"same-value"));
        for invalid in [b"same-valu".as_slice(), b"same-value\0", b"Same-value", b""] {
            assert!(!super::api_token_matches(invalid, b"same-value"));
        }
    }
}

#[cfg(test)]
mod geotech_circular_command_guard_tests {
    use super::*;

    #[test]
    fn escaped_prepared_snapshot_in_batch_is_denied_before_payload_decode() {
        // This intentionally invalid snapshot must never reach typed decoding.
        // Escaped JSON enum keys have already become their semantic names.
        let commands: Value = serde_json::from_str(
            r#"[{"delete_element":{"element_id":1}},{"\u0065ngineering":{"save_circular_s\u006eapshot":{"snapshot":"PRIVATE_PAYLOAD_SENTINEL"}}}]"#,
        )
        .unwrap();
        let error = reject_external_circular_snapshot_value(&commands).unwrap_err();
        assert!(error.message.starts_with("OwnedCircularSnapshotRequired"));
        assert!(error.message.len() < 160);
        assert!(error.validation_results.is_empty());
        let details = error.geometry_error.unwrap();
        assert_eq!(details["code"], "OwnedCircularSnapshotRequired");
        assert!(!error.message.contains("PRIVATE_PAYLOAD_SENTINEL"));
        assert!(!details.to_string().contains("PRIVATE_PAYLOAD_SENTINEL"));
    }

    #[test]
    fn ordinary_typed_commands_and_reserved_user_keys_keep_the_existing_path() {
        use ai_revit_core::engineering::{AnalysisCaseId, EngineeringCommand};
        let commands = vec![
            Command::DeleteElement {
                element_id: ElementId::new(u64::MAX),
            },
            Command::Engineering(EngineeringCommand::SaveCalculationSnapshot {
                analysis_id: AnalysisCaseId::new(7),
            }),
        ];
        assert!(reject_external_circular_snapshot_commands(&commands).is_ok());
        let wire = serde_json::to_value(&commands).unwrap();
        assert!(reject_external_circular_snapshot_value(&wire).is_ok());
        let decoded: Vec<Command> = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, commands);
        // The reserved text inside unrelated user data is not an enum command.
        for ordinary in [
            json!([{"set_property":{"value":{"save_circular_snapshot":"user value"}}}]),
            json!([{"engineering":{"create_ground":{"name":"save_circular_snapshot"}}}]),
            json!([{"engineering":{"update_circular":{"metadata":{"save_circular_snapshot":true}}}}]),
        ] {
            assert!(reject_external_circular_snapshot_value(&ordinary).is_ok());
        }
    }

    #[test]
    fn saved_archive_operation_uses_borrowed_summary_and_ordinary_shape_is_unchanged() {
        let fixture = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures()
            .join("specified.native4.json");
        // Opening and projecting saved evidence must not calculate a new result.
        let document = Document::open(fixture).unwrap();
        let saved = document
            .operation_tree()
            .find(|operation| {
                matches!(
                    operation.kind,
                    OperationKind::EngineeringChanged {
                        command: ai_revit_core::engineering::EngineeringCommand::SaveCircularSnapshot { .. },
                        ..
                    }
                )
            })
            .expect("actual saved archive operation");
        let OperationKind::EngineeringChanged {
            command:
                ai_revit_core::engineering::EngineeringCommand::SaveCircularSnapshot {
                    snapshot, ..
                },
            ..
        } = &saved.kind
        else {
            unreachable!();
        };
        let projected = serde_json::to_value(api_operation_view(saved)).unwrap();
        assert_eq!(
            projected["projection_format"],
            "circular-archive-save-operation-summary-v1"
        );
        assert_eq!(projected["id"], serde_json::to_value(saved.id).unwrap());
        assert_eq!(
            projected["command_id"],
            serde_json::to_value(&saved.command_id).unwrap()
        );
        assert_eq!(
            projected["parent"],
            serde_json::to_value(saved.parent).unwrap()
        );
        assert_eq!(projected["command_kind"], "save_circular_snapshot");
        assert_eq!(
            projected["archive_digest"],
            snapshot.archive().archive_digest
        );
        assert_eq!(projected["report_digest"], snapshot.archive().report_digest);
        assert_eq!(projected["bundle_digest"], snapshot.archive().bundle_digest);
        let encoded = serde_json::to_vec(&projected).unwrap();
        assert!(
            encoded.len() < 4096,
            "saved fixture produces only small metadata"
        );
        assert!(!projected.as_object().unwrap().contains_key("snapshot"));
        assert!(!projected.as_object().unwrap().contains_key("archive"));
        assert!(!projected.as_object().unwrap().contains_key("kind"));
        // It cannot be decoded into an authoritative operation/command payload.
        assert!(serde_json::from_value::<ai_revit_core::OperationRecord>(projected).is_err());
        let ordinary = document
            .operation_tree()
            .find(|operation| {
                !matches!(
                    api_operation_view(operation),
                    ApiOperationView::CircularArchiveSave(_)
                )
            })
            .expect("ordinary operation in same saved Document");
        assert_eq!(
            serde_json::to_value(api_operation_view(ordinary)).unwrap(),
            serde_json::to_value(ordinary).unwrap()
        );
    }
}

fn api_token_is_configured() -> bool {
    #[cfg(target_os = "linux")]
    { return crate::linux_lifecycle_wire::expected_token().is_some(); }
    #[cfg(not(target_os = "linux"))]
    {
    std::env::var("AI_REVIT_API_TOKEN")
        .ok()
        .is_some_and(|token| !token.is_empty())
    }
}

fn no_document_error() -> String {
    "no document is open".to_owned()
}

fn numeric_element_id(params: &Value) -> Result<u64, String> {
    params
        .get("element_id")
        .and_then(Value::as_u64)
        .or_else(|| params.as_u64())
        .ok_or_else(|| "element_id must be a non-negative integer".to_owned())
}
