//! App-owned adapters for the single Document GEO/Foundation domain.
//! Text drafts and read-only projections are not a second geology authority.
use ai_revit_core::{
    Command, Document, EngineeringSnapshotId, EngineeringSnapshotStatus,
    GeoFoundationAnalysisSnapshot, GeoFoundationCommand, GeoFoundationLayoutInput,
    GeoFoundationLayoutState,
    EngineeringSourceIdentity,
};
use geo_core::{
    GeoCommand, GeoQuery, GeoQueryResult, GeoSnapshot, GeoSnapshotId,
    GeoSnapshotSelection, GeoState, ResolvedGeoSnapshot, SnapshotCurrentness,
};
use makepad_widgets::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, fs::File, io::Read};
use super::App;
use crate::drafting_canvas::DraftingCanvasWidgetRefExt;
use crate::viewport::CadViewportWidgetRefExt;

/// Build one ordered, atomic edit bundle for assigning an interpreted section
/// to its GroundModel. Core validates every command, so remove the old link
/// before changing the section owner, then add the new link after the section
/// upsert. A same-owner edit leaves the model untouched when already linked.
pub(super) fn section_edit_commands(
    state: &GeoState,
    section: geo_core::InterpretedSection,
) -> Result<Vec<GeoCommand>, String> {
    let section_id = section.id.clone();
    let target_id = section.model_id.clone();
    let target_model = state
        .records
        .models
        .get(&target_id)
        .cloned()
        .ok_or_else(|| format!("Section target GroundModel {} does not exist", target_id.as_str()))?;
    let current_owner = state
        .records
        .sections
        .get(&section_id)
        .map(|stored| stored.model_id.clone());

    if let Some(owner_id) = current_owner.as_ref() {
        if !state.records.models.contains_key(owner_id) {
            return Err(format!(
                "Stored Section {} refers to missing owner GroundModel {}",
                section_id.as_str(),
                owner_id.as_str(),
            ));
        }
    }

    for (model_id, model) in &state.records.models {
        if model.section_ids.iter().any(|linked| linked == &section_id)
            && Some(model_id) != current_owner.as_ref()
        {
            return Err(format!(
                "Section {} is already linked from unrelated GroundModel {}",
                section_id.as_str(),
                model_id.as_str(),
            ));
        }
    }

    let mut commands = Vec::with_capacity(3);
    if let Some(owner_id) = current_owner.as_ref() {
        if owner_id != &target_id {
            let mut old_owner = state
                .records
                .models
                .get(owner_id)
                .cloned()
                .ok_or_else(|| format!("Stored Section owner GroundModel {} does not exist", owner_id.as_str()))?;
            if old_owner.section_ids.contains(&section_id) {
                old_owner.section_ids.retain(|linked| linked != &section_id);
                commands.push(GeoCommand::UpsertModel { record: old_owner });
            }
        }
    }

    commands.push(GeoCommand::UpsertSection { record: section });
    if !target_model.section_ids.contains(&section_id) {
        let mut target_model = target_model;
        target_model.section_ids.push(section_id);
        commands.push(GeoCommand::UpsertModel { record: target_model });
    }
    Ok(commands)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GfBinding {
    pub document_uuid: String,
    pub runtime_id: String,
    pub open_generation: u64,
    pub document_revision: u64,
    pub records_fingerprint: String,
}

#[derive(Clone, Debug)]
pub(super) struct GfProjection {
    pub binding: GfBinding,
    pub state: GeoState,
    pub selected_snapshot: Option<GeoSnapshot>,
    /// False means a read-only closure of current records, never a saved record.
    pub snapshot_is_stored: bool,
    pub resolved: Option<ResolvedGeoSnapshot>,
    pub snapshot_currentness: Option<SnapshotCurrentness>,
    pub layout: Option<GeoFoundationLayoutState>,
    pub foundation_snapshots: Vec<(EngineeringSnapshotId, GeoFoundationAnalysisSnapshot, EngineeringSnapshotStatus)>,
    /// Existing explicitly synthetic numerical controls, not GEO soil records.
    pub control_templates: Vec<(foundation_core::FoundationId, foundation_core::RigidMatInput)>,
    pub configured_load_sources: Vec<GfConfiguredLoadSource>,
    pub status: String,
    pub report: String,
}

#[derive(Clone, Debug)]
pub(super) struct GfConfiguredLoadSource {
    pub snapshot_id: EngineeringSnapshotId,
    pub content_fingerprint: String,
    pub status: EngineeringSnapshotStatus,
    pub foundation_ids: Vec<foundation_core::FoundationId>,
}

/// Native form values for an explicit vertical-resultant numerical control.
/// This is presentation input; it is not a soil record or a stored registry.
#[derive(Clone, Debug)]
pub(super) struct GfManualNumericalControls {
    pub n_kn: f64,
    pub physical_mx_knm: f64,
    pub physical_my_knm: f64,
    pub bedding_k_kn_m3: f64,
    pub gap_m: f64,
    pub include_raft_own_weight: bool,
    pub source_reference: String,
    pub bedding_note: String,
}

pub(super) fn apply_manual_numerical_controls(
    template: &foundation_core::RigidMatInput, controls: &GfManualNumericalControls,
) -> Result<foundation_core::RigidMatInput, String> {
    if ![controls.n_kn, controls.physical_mx_knm, controls.physical_my_knm,
        controls.bedding_k_kn_m3, controls.gap_m].iter().all(|value| value.is_finite())
        || controls.n_kn <= 0.0 || controls.bedding_k_kn_m3 <= 0.0
        || controls.source_reference.trim().is_empty() || controls.source_reference.len() > 4096
        || controls.bedding_note.trim().is_empty()
    {
        return Err("MODEL_INCOMPLETE: finite N>0, k>0, moments/gap and explicit load/stiffness references are required".into());
    }
    let mut input = template.clone();
    // The GUI uses physical global moments; the existing pressure model's Mx
    // convention is opposite. Horizontal force/torsion are excluded by this
    // explicitly selected vertical-resultant control, not inferred from GEO.
    input.load = foundation_core::PressureLoad::manual(
        controls.n_kn, -controls.physical_mx_knm, controls.physical_my_knm,
        controls.source_reference.clone(),
    );
    input.soil.support = foundation_core::SupportField::Uniform {
        k_kn_m3: controls.bedding_k_kn_m3, gap_m: controls.gap_m,
    };
    input.soil.assumption_note = controls.bedding_note.clone();
    input.self_weight_policy = if controls.include_raft_own_weight {
        foundation_core::SelfWeightPolicy::IncludeOnce {
            source_id: format!("geo-foundation-manual-own-weight:{}", input.foundation_id),
        }
    } else { foundation_core::SelfWeightPolicy::ExcludedExplicitly };
    // Retained synthetic numerical metadata stays a declared control template.
    // Neither these values nor k become GeoState soil parameter authority.
    foundation_core::validate_input(&input).map_err(|error| error.to_string())?;
    Ok(input)
}

pub(super) fn apply_bedding_numerical_controls(
    template: &foundation_core::RigidMatInput, k_kn_m3: f64, gap_m: f64,
    assumption_note: &str, include_raft_own_weight: bool,
) -> Result<foundation_core::RigidMatInput, String> {
    if !k_kn_m3.is_finite() || k_kn_m3 <= 0.0 || !gap_m.is_finite() || assumption_note.trim().is_empty() {
        return Err("MODEL_INCOMPLETE: finite positive manual k, gap and a named stiffness assumption are required".into());
    }
    let mut input = template.clone();
    input.soil.support = foundation_core::SupportField::Uniform { k_kn_m3, gap_m };
    input.soil.assumption_note = assumption_note.to_owned();
    input.self_weight_policy = if include_raft_own_weight {
        foundation_core::SelfWeightPolicy::IncludeOnce {
            source_id: format!("geo-foundation-manual-own-weight:{}", input.foundation_id),
        }
    } else { foundation_core::SelfWeightPolicy::ExcludedExplicitly };
    // The configured adapter replaces the template's load with the exact saved
    // group ledger. This helper supplies stiffness/weight controls only.
    Ok(input)
}

/// Explicit Inspector request over the map's immutable source, never a Draw
/// callback. The raw column result retains exact supporting triangles/sources;
/// its status differs from the map's classification at the foundation sole.
pub(super) fn ground_map_sample_query(
    map: &ai_revit_core::GeoFoundationGroundMap, index: usize,
) -> Result<GeoQueryResult, String> {
    let cell = map.cells.get(index).filter(|cell| cell.index == index)
        .ok_or_else(|| "FOUNDATION_MAP_HOLD: selected midpoint is absent".to_owned())?;
    let scenario_id = map.ground.selection.scenario_id.as_ref()
        .ok_or_else(|| "FOUNDATION_MAP_HOLD: cached map has no explicit scenario".to_owned())?;
    let scenario = map.ground.records.scenarios.get(scenario_id)
        .ok_or_else(|| "FOUNDATION_MAP_HOLD: cached scenario is absent from its immutable closure".to_owned())?;
    let resolved = geo_core::resolve_geo_scenario(&map.ground, scenario)
        .map_err(|error| format!("FOUNDATION_MAP_HOLD: {error}"))?;
    geo_core::query_ground(&resolved, &GeoQuery::ColumnAt { xy_m: cell.xy_m })
        .map_err(|error| format!("FOUNDATION_MAP_HOLD: {error}"))
}

fn records_fingerprint(state: &GeoState) -> Result<String, String> {
    serde_json::to_vec(&state.records).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|error| error.to_string())
}

pub(super) fn binding(document: &Document, epoch: u64) -> Result<GfBinding, String> {
    Ok(GfBinding {
        document_uuid: document.document_uuid.to_string(),
        runtime_id: document.runtime_instance_id(),
        open_generation: epoch,
        document_revision: document.revision(),
        records_fingerprint: records_fingerprint(document.geo_state())?,
    })
}

fn require_current(document: &Document, epoch: u64, expected: &GfBinding) -> Result<(), String> {
    if binding(document, epoch)? != *expected {
        return Err("STALE_DRAFT: the Document instance or edited source changed; text is retained. Reload explicitly before applying.".into());
    }
    Ok(())
}

pub(super) fn projection(
    document: &Document, epoch: u64, selection: Option<&GeoSnapshotSelection>,
    snapshot_id: Option<&GeoSnapshotId>,
) -> Result<GfProjection, String> {
    let domain = document.geo_foundation();
    let state = domain.geo();
    let mut report = vec![
        "GEO records in this Document are the only editable soil/IGE/borehole authority.".to_owned(),
        "Known parameter values retain source IDs; Known is not reviewed or engineering-approved. Missing is not zero; water Unknown is not dry.".into(),
    ];
    let (snapshot, stored) = if let Some(id) = snapshot_id {
        match state.snapshots.get(id) {
            Some(snapshot) => (Some(snapshot.clone()), true),
            None => { report.push(format!("Selected saved snapshot {id} is unavailable.")); (None, false) }
        }
    } else if let Some(selection) = selection {
        if let Some(head) = state.head_revision.as_ref() {
            // Same read-only closure pattern as Document::query_geo. No command,
            // operation ID, revision or persisted snapshot is created here.
            match geo_core::make_geo_snapshot(
                state, GeoSnapshotId::new("current-view").map_err(|error| error.to_string())?,
                selection, head.clone(),
            ) {
                Ok(snapshot) => (Some(snapshot), false),
                Err(error) => { report.push(format!("Current GEO selection is incomplete: {error}")); (None, false) }
            }
        } else { (None, false) }
    } else { (None, false) };
    let currentness = snapshot.as_ref().map(|snapshot| geo_core::snapshot_currentness(state, snapshot))
        .transpose().map_err(|error| error.to_string())?;
    let resolved = snapshot.as_ref().and_then(|snapshot| {
        let Some(id) = snapshot.selection.scenario_id.as_ref() else {
            report.push("Select an explicit scenario for ground queries; absent water context is never treated as dry.".into());
            return None;
        };
        let Some(scenario) = snapshot.records.scenarios.get(id) else {
            report.push(format!("Selected scenario {id} is not in the snapshot closure."));
            return None;
        };
        match geo_core::resolve_geo_scenario(snapshot, scenario) {
            Ok(resolved) => Some(resolved),
            Err(error) => { report.push(format!("GEO scenario resolution HOLD: {error}")); None }
        }
    });
    let status = match (snapshot.as_ref(), currentness, resolved.as_ref()) {
        (None, _, _) => "MODEL_INCOMPLETE",
        (_, Some(SnapshotCurrentness::Stale), _) => "STALE_SOURCE",
        (_, _, None) => "SCENARIO_INCOMPLETE",
        _ => "SOURCE_BOUND_GEOMETRY",
    }.to_owned();
    if let Some(snapshot) = snapshot.as_ref() {
        report.push(format!("GEO model {}; scenario {:?}; selection fingerprint {}; closure content fingerprint {}; source operation {}; currentness {:?}; origin {}.",
            snapshot.selection.model_id, snapshot.selection.scenario_id, snapshot.selection_fingerprint,
            snapshot.content_fingerprint, snapshot.source_revision.operation_id, currentness,
            if stored { "stored immutable snapshot" } else { "read-only current-records projection; not persisted" }));
    }
    if let Some(resolved) = resolved.as_ref() {
        report.push(format!("Water scenario: {:?}. No extrapolation outside explicit TIN coverage; unknown/conflicting zones remain unknown.", resolved.scenario.water));
    }
    let foundations: Vec<_> = domain.snapshots().map(|(id, snapshot)| {
        (id, snapshot.clone(), document.geo_foundation_snapshot_status(id).unwrap_or(EngineeringSnapshotStatus::Stale))
    }).collect();
    let configured = document.structural_configurator();
    let control_templates = configured.current().map(|current| current.foundation_groups.values()
        .map(|group| (group.foundation_id.clone(), group.input.clone())).collect()).unwrap_or_default();
    let configured_load_sources = configured.snapshots().map(|(id, snapshot)| GfConfiguredLoadSource {
        snapshot_id: id, content_fingerprint: snapshot.content_fingerprint.clone(),
        status: configured.snapshot_status(id).unwrap_or(EngineeringSnapshotStatus::Stale),
        foundation_ids: snapshot.source.foundation_groups.keys().cloned().collect(),
    }).collect();
    for (id, snapshot, status) in &foundations {
        report.push(format!("Foundation snapshot {}: {:?}; placement {}; GEO closure {}; result {}; engineering approval {:?}; load source {:?}.",
            id.get(), status, snapshot.input.foundation_id, snapshot.input.ground.selection_fingerprint,
            snapshot.content_fingerprint, snapshot.engineering_approval, snapshot.input.load_binding));
    }
    report.push("Foundation pressure/settlement are conditional numerical-model outputs only; bearing, normative settlement, reinforcement and ULS/SLS remain HOLD/NOT_IMPLEMENTED unless separately implemented and evidenced.".into());
    Ok(GfProjection {
        binding: binding(document, epoch)?, state: state.clone(),
        selected_snapshot: snapshot, snapshot_is_stored: stored, resolved,
        snapshot_currentness: currentness, layout: domain.layout().cloned(),
        foundation_snapshots: foundations, status, report: report.join("\n"),
        control_templates, configured_load_sources,
    })
}

impl App {
    /// Explicit sample installation in the current Document. The common Core
    /// factory is shared with CLI; it creates no second Document and runs no solver.
    pub(super) fn geo_foundation_load_demo(&mut self, cx: &mut Cx) -> Result<(), String> {
        let document = self.document.as_ref().ok_or_else(|| "Open a Document before loading the synthetic example".to_owned())?;
        let revision = document.revision();
        let commands = ai_revit_core::geo_foundation_demo_commands().map_err(|error| error.to_string())?;
        let command_id = self.next_command_id("geo-foundation-shared-example");
        self.apply_typed_commands(cx, command_id, revision, commands).map_err(|error| error.message)?;
        Ok(())
    }

    /// Source-bound midpoint samples, not continuous coverage or numerical design.
    pub(super) fn geo_foundation_ground_map(
        &self, foundation_id: &str, nx: usize, ny: usize,
    ) -> Result<ai_revit_core::GeoFoundationGroundMap, String> {
        self.document.as_ref().ok_or_else(|| "No Document is open".to_owned())?
            .foundation_ground_map(foundation_id, nx, ny).map_err(|error| error.to_string())
    }

    /// Internal read-only consumer preparation. This function is deliberately
    /// not published through a new agent route or added to the existing ui.state.
    pub(super) fn geo_foundation_ui_state(&self, cx: &Cx) -> Value {
        let ui = &self.geo_foundation_ui;
        let document = self.document.as_ref();
        let live = document.map(|document| document.geo_state());
        let view_snapshot = ui.projection.as_ref().and_then(|projection| projection.selected_snapshot.as_ref());
        let currentness = match (live, view_snapshot) {
            (Some(state), Some(snapshot)) => geo_core::snapshot_currentness(state, snapshot).ok(),
            _ => None,
        };
        let viewport_ref = self.ui.cad_viewport(cx, ids!(gf_viewport));
        let actual_view = viewport_ref.borrow().map(|viewport| {
            let viewer = viewport.viewer();
            json!({
                "scene_object_count": viewer.scene().objects().count(),
                "scene_visible_object_count": viewer.scene().objects().filter(|object| object.visible).count(),
                "selected_application_object_ids": viewer.scene().selected_ids().into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                "camera": serde_json::to_value(viewer.camera().state()).ok(),
                "viewport": {"width": viewer.viewport().width, "height": viewer.viewport().height},
            })
        });
        let canvas_ref = self.ui.drafting_canvas(cx, ids!(gf_canvas));
        let canvas = canvas_ref.borrow().and_then(|canvas| {
            let rect = canvas.rect;
            if [rect.pos.x, rect.pos.y, rect.size.x, rect.size.y].iter().all(|value| value.is_finite())
                && rect.size.x > 0.0 && rect.size.y > 0.0
            {
                Some(json!({"x": rect.pos.x, "y": rect.pos.y, "width": rect.size.x, "height": rect.size.y, "dpi_factor": canvas.dpi_factor}))
            } else { None }
        });
        json!({
            "kind": "INTERNAL_APP_STATE_PREPARATION",
            "active": ui.active,
            "view": format!("{:?}", ui.view),
            "tab": format!("{:?}", ui.active_tab),
            "document": document.map(|document| json!({
                "uuid": document.document_uuid.to_string(), "runtime_id": document.runtime_instance_id(),
                "open_generation": self.api_document_generation, "revision": document.revision(),
            })),
            "geo_head": live.and_then(|state| state.head_revision.as_ref()),
            "selected_model": ui.selected_model.as_ref().map(|id| id.as_str()),
            "selected_scenario": ui.selected_scenario.as_ref().map(|id| id.as_str()),
            "selected_soil": ui.selected_soil.as_ref().map(|id| id.as_str()),
            "selected_ige": ui.selected_ige.as_ref().map(|id| id.as_str()),
            "selected_borehole": ui.selected_borehole.as_ref().map(|id| id.as_str()),
            "selected_section": ui.selected_section.as_ref().map(|id| id.as_str()),
            "selected_foundation": ui.selected_foundation,
            "selection": ui.selection.as_ref().map(|pick| format!("{pick:?}")),
            "view_selection_fingerprint": view_snapshot.map(|snapshot| snapshot.selection_fingerprint.as_str()),
            "view_content_fingerprint": view_snapshot.map(|snapshot| snapshot.content_fingerprint.as_str()),
            "view_snapshot_is_stored": ui.projection.as_ref().map(|projection| projection.snapshot_is_stored),
            "view_source_currentness": currentness,
            "scene_source_fingerprint": ui.scene_source_fingerprint,
            "scene_foundation_snapshot_fingerprint": ui.scene_foundation_snapshot_fingerprint,
            "dirty": ui.draft_dirty,
            "layout_dirty": ui.layout_dirty,
            "stale_draft": ui.binding_stale,
            "actual_view": actual_view,
            "actual_canvas_bounds": canvas,
            "aliases_kind": "SOURCE_DECLARATIONS",
            "aliases": crate::geo_foundation_ui::GEO_FOUNDATION_ALIASES,
            "native_acceptance": "NOT_TESTED",
        })
    }

    pub(super) fn geo_foundation_execute(
        &mut self, cx: &mut Cx, commands: Vec<GeoCommand>, expected: GfBinding,
    ) -> Result<(), String> {
        if commands.is_empty() { return Err("No GEO changes were supplied".into()); }
        let document = self.document.as_ref().ok_or_else(|| "Open a Document before editing GEO".to_owned())?;
        require_current(document, self.api_document_generation, &expected)?;
        let revision = document.revision();
        let commands = commands.into_iter().map(|command| Command::GeoFoundation(GeoFoundationCommand::EditGeo { command })).collect();
        let command_id = self.next_command_id("geo-foundation-edit");
        self.apply_typed_commands(cx, command_id, revision, commands).map_err(|error| error.message)?;
        Ok(())
    }

    pub(super) fn geo_foundation_replace_layout(
        &mut self, cx: &mut Cx, input: GeoFoundationLayoutInput, expected: GfBinding,
    ) -> Result<(), String> {
        let document = self.document.as_ref().ok_or_else(|| "Open a Document before editing Foundation".to_owned())?;
        require_current(document, self.api_document_generation, &expected)?;
        let revision = document.revision();
        let command_id = self.next_command_id("geo-foundation-layout");
        self.apply_typed_commands(cx, command_id, revision, vec![
            Command::GeoFoundation(GeoFoundationCommand::ReplaceFoundationLayout { input }),
        ]).map_err(|error| error.message)?;
        Ok(())
    }

    pub(super) fn geo_foundation_query(
        &self, selection: &GeoSnapshotSelection, query: &GeoQuery,
    ) -> Result<GeoQueryResult, String> {
        self.document.as_ref().ok_or_else(|| "No Document is open".to_owned())?
            .query_geo(selection, query).map_err(|error| error.to_string())
    }

    /// Explicit native numerical action. This method publishes no agent route;
    /// input is a declared conditional solver control, not an editable GEO registry.
    pub(super) fn geo_foundation_analyze(
        &mut self, cx: &mut Cx, foundation_id: &str,
        input: foundation_core::RigidMatInput, source_reference: &str,
        expected: GfBinding,
    ) -> Result<(), String> {
        let document = self.document.as_ref().ok_or_else(|| "No Document is open".to_owned())?;
        require_current(document, self.api_document_generation, &expected)?;
        if source_reference.trim().is_empty() {
            return Err("MODEL_INCOMPLETE: an explicit manual load source reference is required".into());
        }
        let prepared = document.prepare_geo_foundation_analysis_manual(foundation_id, input, source_reference)
            .map_err(|error| format!("FOUNDATION_INPUT_HOLD: {error}"))?;
        let producer = producer_from_environment()?;
        let snapshot = ai_revit_core::analyze_geo_foundation(prepared, producer)
            .map_err(|error| format!("FOUNDATION_ANALYSIS_ATTEMPT_FAILED: {error}"))?;
        let revision = document.revision();
        let command_id = self.next_command_id("geo-foundation-analysis");
        self.apply_typed_commands(cx, command_id, revision, vec![
            Command::GeoFoundation(GeoFoundationCommand::SaveAnalysisSnapshot { snapshot }),
        ]).map_err(|error| error.message)?;
        Ok(())
    }

    pub(super) fn geo_foundation_analyze_from_configured(
        &mut self, cx: &mut Cx, foundation_id: &str,
        snapshot_id: EngineeringSnapshotId, explicit_input: foundation_core::RigidMatInput,
        expected: GfBinding,
    ) -> Result<(), String> {
        let document = self.document.as_ref().ok_or_else(|| "No Document is open".to_owned())?;
        require_current(document, self.api_document_generation, &expected)?;
        let prepared = document.prepare_geo_foundation_analysis_from_configured(foundation_id, snapshot_id, explicit_input)
            .map_err(|error| format!("FOUNDATION_LOAD_SOURCE_HOLD: {error}"))?;
        let snapshot = ai_revit_core::analyze_geo_foundation(prepared, producer_from_environment()?)
            .map_err(|error| format!("FOUNDATION_ANALYSIS_ATTEMPT_FAILED: {error}"))?;
        let revision = document.revision();
        let command_id = self.next_command_id("geo-foundation-configured-analysis");
        self.apply_typed_commands(cx, command_id, revision, vec![
            Command::GeoFoundation(GeoFoundationCommand::SaveAnalysisSnapshot { snapshot }),
        ]).map_err(|error| error.message)?;
        Ok(())
    }
}

fn producer_from_environment() -> Result<EngineeringSourceIdentity, String> {
    let compiled = option_env!("AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256")
        .ok_or_else(|| "SOURCE_BINDING_HOLD: AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256 is not compiled in".to_owned())?;
    let compiled = normalize_sha256("AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256", compiled)?;
    let runtime = env::var("AI_REVIT_GF_SOURCE_MANIFEST_SHA256")
        .map_err(|_| "SOURCE_BINDING_HOLD: AI_REVIT_GF_SOURCE_MANIFEST_SHA256 is required".to_owned())?;
    let runtime = normalize_sha256("AI_REVIT_GF_SOURCE_MANIFEST_SHA256", &runtime)?;
    if runtime != compiled { return Err("SOURCE_BINDING_HOLD: runtime manifest differs from the compiled source pin".into()); }
    let actual = current_executable_sha256()?;
    match env::var("AI_REVIT_GF_EXECUTABLE_SHA256") {
        Ok(value) if !value.trim().is_empty() => {
            if normalize_sha256("AI_REVIT_GF_EXECUTABLE_SHA256", &value)? != actual {
                return Err("SOURCE_BINDING_HOLD: running executable differs from AI_REVIT_GF_EXECUTABLE_SHA256".into());
            }
        }
        Ok(_) | Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(_)) => return Err("SOURCE_BINDING_HOLD: executable guard must contain SHA-256 hex".into()),
    }
    Ok(EngineeringSourceIdentity {
        source_manifest_sha256: runtime, executable_sha256: Some(actual),
        solver_version: format!("geo-foundation-modular/{}", env!("CARGO_PKG_VERSION")),
        source_label: "Root-recorded GEO/Foundation source; conditional Winkler output, no native or engineering acceptance".into(),
    })
}

fn normalize_sha256(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("SOURCE_BINDING_HOLD: {name} requires 64 hexadecimal characters"));
    }
    Ok(value.to_ascii_lowercase())
}

fn current_executable_sha256() -> Result<String, String> {
    let path = env::current_exe().map_err(|error| format!("SOURCE_BINDING_HOLD: cannot identify executable: {error}"))?;
    let mut file = File::open(&path).map_err(|error| format!("SOURCE_BINDING_HOLD: cannot read executable: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| format!("SOURCE_BINDING_HOLD: cannot hash executable: {error}"))?;
        if count == 0 { break; }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod section_edit_tests {
    use super::section_edit_commands;
    use ai_revit_core::{
        geo_foundation_demo_commands, Command, Document, GeoFoundationCommand,
    };
    use geo_core::{
        GeoCommand, GeoSectionId, GeoState, GroundModel, GroundModelId, InterpretedSection,
    };

    fn commit_commands(
        document: &mut Document,
        command_id: &str,
        commands: impl IntoIterator<Item = Command>,
    ) {
        let mut transaction = document.begin_transaction(command_id, document.revision());
        for command in commands {
            transaction.push(command);
        }
        document.commit(transaction).expect("fixture transaction is valid");
    }

    fn commit_geo_commands(
        document: &mut Document,
        command_id: &str,
        commands: Vec<GeoCommand>,
    ) {
        commit_commands(
            document,
            command_id,
            commands.into_iter().map(|command| {
                Command::GeoFoundation(GeoFoundationCommand::EditGeo { command })
            }),
        );
    }

    fn demo_document() -> Document {
        let mut document = Document::new("Section edit helper test")
            .expect("test Document should initialize");
        let commands = geo_foundation_demo_commands()
            .expect("shared synthetic GEO fixture should be valid")
            .into_iter()
            .filter(|command| matches!(command, Command::GeoFoundation(_)));
        commit_commands(&mut document, "section-helper-demo", commands);
        document
    }

    fn first_model_id(document: &Document) -> GroundModelId {
        document
            .geo_state()
            .records
            .models
            .keys()
            .next()
            .expect("demo has a GroundModel")
            .clone()
    }

    fn first_section(document: &Document) -> InterpretedSection {
        document
            .geo_state()
            .records
            .sections
            .values()
            .next()
            .expect("demo has an interpreted section")
            .clone()
    }

    fn section(id: &str, model_id: GroundModelId) -> InterpretedSection {
        InterpretedSection {
            id: GeoSectionId::new(id).expect("test section ID is valid"),
            model_id,
            polyline_xy_m: vec![[0.0, 0.0], [1.0, 1.0]],
            source_ids: Vec::new(),
            revision: 0,
        }
    }

    fn model_with_link(mut model: GroundModel, section_id: &GeoSectionId) -> GroundModel {
        if !model.section_ids.contains(section_id) {
            model.section_ids.push(section_id.clone());
        }
        model
    }

    fn assert_same_except_revision(actual: &GroundModel, expected: &GroundModel) {
        let mut actual = actual.clone();
        actual.revision = expected.revision;
        assert_eq!(&actual, expected);
    }

    #[test]
    fn creation_links_section_and_preserves_the_rest_of_the_model() {
        let mut document = demo_document();
        let state = document.geo_state().clone();
        let target_id = first_model_id(&document);
        let before_model = state.records.models[&target_id].clone();
        let new_section = section("SECTION-LINK-CREATE", target_id.clone());
        assert!(!before_model.section_ids.contains(&new_section.id));

        let expected_model = model_with_link(before_model.clone(), &new_section.id);
        let commands = section_edit_commands(&state, new_section.clone()).unwrap();
        assert_eq!(
            commands,
            vec![
                GeoCommand::UpsertSection {
                    record: new_section.clone(),
                },
                GeoCommand::UpsertModel {
                    record: expected_model.clone(),
                },
            ],
        );

        commit_geo_commands(&mut document, "section-link-create", commands);
        let after = document.geo_state();
        assert_eq!(after.records.sections[&new_section.id].model_id, target_id);
        assert_eq!(
            after.records.models[&target_id].section_ids.iter()
                .filter(|id| *id == &new_section.id).count(),
            1,
        );
        assert_same_except_revision(&after.records.models[&target_id], &expected_model);
    }

    #[test]
    fn repeated_same_owner_update_only_upserts_section_without_duplicate_link() {
        let mut document = demo_document();
        let model_id = first_model_id(&document);
        let created = section("SECTION-LINK-REPEAT", model_id.clone());
        let initial = section_edit_commands(document.geo_state(), created.clone()).unwrap();
        commit_geo_commands(&mut document, "section-link-repeat-create", initial);
        let model_before = document.geo_state().records.models[&model_id].clone();

        let mut update = document.geo_state().records.sections[&created.id].clone();
        update.polyline_xy_m = vec![[2.0, 3.0], [4.0, 5.0]];
        let commands = section_edit_commands(document.geo_state(), update.clone()).unwrap();
        assert_eq!(
            commands,
            vec![GeoCommand::UpsertSection {
                record: update.clone(),
            }],
        );
        commit_geo_commands(&mut document, "section-link-repeat-update", commands);

        let after = document.geo_state();
        assert_eq!(after.records.sections[&created.id].polyline_xy_m, update.polyline_xy_m);
        assert_eq!(after.records.models[&model_id], model_before);
        assert_eq!(
            after.records.models[&model_id].section_ids.iter()
                .filter(|id| *id == &created.id).count(),
            1,
        );
    }

    #[test]
    fn same_owner_legacy_unlinked_section_repairs_the_model_link() {
        let mut document = demo_document();
        let mut existing = first_section(&document);
        let model_id = existing.model_id.clone();
        let mut legacy_model = document.geo_state().records.models[&model_id].clone();
        legacy_model.section_ids.retain(|id| id != &existing.id);
        commit_geo_commands(
            &mut document,
            "section-link-legacy-unlinked",
            vec![GeoCommand::UpsertModel {
                record: legacy_model,
            }],
        );
        existing.polyline_xy_m = vec![[6.0, 7.0], [8.0, 9.0]];

        let mut expected_model = document.geo_state().records.models[&model_id].clone();
        expected_model.section_ids.push(existing.id.clone());
        let commands = section_edit_commands(document.geo_state(), existing.clone()).unwrap();
        assert_eq!(
            commands,
            vec![
                GeoCommand::UpsertSection {
                    record: existing.clone(),
                },
                GeoCommand::UpsertModel {
                    record: expected_model.clone(),
                },
            ],
        );
        commit_geo_commands(&mut document, "section-link-legacy-repair", commands);

        let after = document.geo_state();
        assert_eq!(
            after.records.models[&model_id].section_ids.iter()
                .filter(|id| *id == &existing.id).count(),
            1,
        );
        assert_same_except_revision(&after.records.models[&model_id], &expected_model);
    }

    #[test]
    fn reassignment_cleans_old_owner_then_links_target_without_losing_other_ids() {
        let mut document = demo_document();
        let moving_section = first_section(&document);
        let old_owner_id = moving_section.model_id.clone();
        let target_id = GroundModelId::new("MODEL-SECTION-DESTINATION")
            .expect("test model ID is valid");
        let mut target = document.geo_state().records.models[&old_owner_id].clone();
        target.id = target_id.clone();
        target.name = "Section destination model".into();
        target.section_ids.clear();
        commit_geo_commands(
            &mut document,
            "section-link-create-destination-model",
            vec![GeoCommand::UpsertModel { record: target }],
        );

        let keeper = section("SECTION-DESTINATION-KEEP", target_id.clone());
        let keeper_commands = section_edit_commands(document.geo_state(), keeper.clone()).unwrap();
        commit_geo_commands(&mut document, "section-link-create-destination-section", keeper_commands);
        let old_before = document.geo_state().records.models[&old_owner_id].clone();
        let target_before = document.geo_state().records.models[&target_id].clone();
        let mut moved = moving_section.clone();
        moved.model_id = target_id.clone();
        let mut expected_old = old_before.clone();
        expected_old.section_ids.retain(|id| id != &moved.id);
        let expected_target = model_with_link(target_before.clone(), &moved.id);

        let commands = section_edit_commands(document.geo_state(), moved.clone()).unwrap();
        assert_eq!(
            commands,
            vec![
                GeoCommand::UpsertModel {
                    record: expected_old.clone(),
                },
                GeoCommand::UpsertSection {
                    record: moved.clone(),
                },
                GeoCommand::UpsertModel {
                    record: expected_target.clone(),
                },
            ],
        );
        commit_geo_commands(&mut document, "section-link-reassign", commands);

        let after = document.geo_state();
        assert_eq!(after.records.sections[&moved.id].model_id, target_id);
        assert!(!after.records.models[&old_owner_id].section_ids.contains(&moved.id));
        assert!(after.records.models[&target_id].section_ids.contains(&keeper.id));
        assert_eq!(
            after.records.models[&target_id].section_ids.iter()
                .filter(|id| *id == &moved.id).count(),
            1,
        );
        assert_same_except_revision(&after.records.models[&old_owner_id], &expected_old);
        assert_same_except_revision(&after.records.models[&target_id], &expected_target);
    }

    #[test]
    fn legacy_unlinked_reassignment_does_not_revise_old_model() {
        let mut document = demo_document();
        let mut moving = first_section(&document);
        let old_owner_id = moving.model_id.clone();
        let target_id = GroundModelId::new("MODEL-LEGACY-DESTINATION")
            .expect("test model ID is valid");
        let mut target = document.geo_state().records.models[&old_owner_id].clone();
        target.id = target_id.clone();
        target.name = "Legacy section destination model".into();
        target.section_ids.clear();
        commit_geo_commands(
            &mut document,
            "section-link-legacy-reassign-target",
            vec![GeoCommand::UpsertModel { record: target }],
        );

        let mut legacy_old_owner = document.geo_state().records.models[&old_owner_id].clone();
        legacy_old_owner.section_ids.retain(|id| id != &moving.id);
        commit_geo_commands(
            &mut document,
            "section-link-legacy-reassign-unlink",
            vec![GeoCommand::UpsertModel {
                record: legacy_old_owner,
            }],
        );
        let old_before = document.geo_state().records.models[&old_owner_id].clone();
        moving.model_id = target_id.clone();
        let mut expected_target = document.geo_state().records.models[&target_id].clone();
        expected_target.section_ids.push(moving.id.clone());

        let commands = section_edit_commands(document.geo_state(), moving.clone()).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands,
            vec![
                GeoCommand::UpsertSection {
                    record: moving.clone(),
                },
                GeoCommand::UpsertModel {
                    record: expected_target.clone(),
                },
            ],
        );
        commit_geo_commands(&mut document, "section-link-legacy-reassign", commands);

        let after = document.geo_state();
        assert_eq!(after.records.sections[&moving.id].model_id, target_id);
        assert_eq!(after.records.models[&old_owner_id], old_before);
        assert_same_except_revision(&after.records.models[&target_id], &expected_target);
    }

    #[test]
    fn missing_target_and_unrelated_section_link_are_rejected() {
        let empty = GeoState::empty();
        let missing_target = section(
            "SECTION-LINK-MISSING-TARGET",
            GroundModelId::new("MODEL-MISSING-TARGET").unwrap(),
        );
        assert!(section_edit_commands(&empty, missing_target).is_err());

        let document = demo_document();
        let mut state = document.geo_state().clone();
        let stored = first_section(&document);
        let old_owner = state.records.models[&stored.model_id].clone();
        let target_id = GroundModelId::new("MODEL-LINK-TARGET").unwrap();

        let mut target_prelinked_state = state.clone();
        let mut target_prelinked = old_owner.clone();
        target_prelinked.id = target_id.clone();
        target_prelinked.section_ids = vec![stored.id.clone()];
        target_prelinked_state.records.models.insert(target_id.clone(), target_prelinked);
        let mut reassigned = stored.clone();
        reassigned.model_id = target_id.clone();
        let error = section_edit_commands(&target_prelinked_state, reassigned).unwrap_err();
        assert!(error.contains("unrelated GroundModel"));

        let mut target = old_owner.clone();
        target.id = target_id.clone();
        target.section_ids.clear();
        state.records.models.insert(target_id.clone(), target);
        let foreign_id = GroundModelId::new("MODEL-FOREIGN-LINK").unwrap();
        let mut foreign = old_owner.clone();
        foreign.id = foreign_id.clone();
        foreign.section_ids = vec![stored.id.clone()];
        state.records.models.insert(foreign_id, foreign);
        let mut proposed = stored.clone();
        proposed.model_id = target_id;

        let error = section_edit_commands(&state, proposed).unwrap_err();
        assert!(error.contains("unrelated GroundModel"));

        let mut not_stored_state = document.geo_state().clone();
        let not_stored_target_id = GroundModelId::new("MODEL-NOT-STORED-TARGET").unwrap();
        let not_stored_section = section("SECTION-NOT-STORED", not_stored_target_id.clone());
        let mut not_stored_target = old_owner;
        not_stored_target.id = not_stored_target_id.clone();
        not_stored_target.section_ids = vec![not_stored_section.id.clone()];
        not_stored_state.records.models.insert(not_stored_target_id, not_stored_target);
        let error = section_edit_commands(&not_stored_state, not_stored_section).unwrap_err();
        assert!(error.contains("unrelated GroundModel"));
    }

    #[test]
    fn late_invalid_third_command_rolls_back_section_bundle_and_document_history() {
        let mut document = demo_document();
        let new_section = section("SECTION-LINK-ROLLBACK", first_model_id(&document));
        let commands = section_edit_commands(document.geo_state(), new_section).unwrap();
        assert_eq!(commands.len(), 2);
        let persistent_before = document.to_json_string().unwrap();
        let revision_before = document.revision();
        let mut transaction = document.begin_transaction("section-link-rollback", revision_before);
        for command in commands {
            transaction.push(Command::GeoFoundation(GeoFoundationCommand::EditGeo {
                command,
            }));
        }
        transaction.push(Command::CreateLevel {
            name: "Late invalid command".into(),
            elevation_m: f64::NAN,
            height_m: None,
        });

        assert!(document.commit(transaction).is_err());
        assert_eq!(document.revision(), revision_before);
        // Persistent native JSON includes journal/history and receipts.
        assert_eq!(document.to_json_string().unwrap(), persistent_before);
    }
}
