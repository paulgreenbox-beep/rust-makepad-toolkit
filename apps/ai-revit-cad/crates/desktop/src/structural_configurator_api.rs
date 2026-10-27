//! Document binding helpers for the steel building configurator.
//!
//! Configurations, generated models and saved analyses stay in the single Core
//! Document. The UI may retain a text draft, but it cannot rebase that draft
//! silently after an open-generation, document or input revision change.

use super::App;
use crate::drafting_canvas::DraftingCanvasWidgetRefExt;
use crate::structural_configurator_render::{ScLoadTarget, ScPick};
use crate::viewport::CadViewportWidgetRefExt;
use ai_revit_core::{
    Command, ConfiguratorAnalysisReadiness, ConfiguratorAnalysisRequest,
    ConfiguratorAnalysisSnapshot, Document, EngineeringSnapshotStatus, EngineeringSourceIdentity,
    StructuralConfiguratorCommand,
};
use makepad_widgets::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, fs::File, io::Read};
use structural_configurator::{BuildingConfig, GeneratedBuilding, SemanticId};

/// Native user intents; the Core adapter translates these into Document commands.
#[derive(Clone, Debug)]
pub(super) enum ScIntent {
    CreateDemo,
    Apply(BuildingConfig),
    Regenerate(BuildingConfig),
    Analyze { section_id: Option<SemanticId> },
}

/// Owned read-only projection used by the presentation cache and text draft.
/// A generated BIM projection is independent of bounded solver capacity.
#[derive(Clone, Debug)]
pub(super) struct ScProjection {
    pub binding: ScBinding,
    pub config: BuildingConfig,
    pub generated: Option<GeneratedBuilding>,
    pub generated_current: bool,
    pub analysis_current: bool,
    pub analysis: Option<ConfiguratorAnalysisSnapshot>,
    pub analysis_status: String,
    pub summary: String,
    pub report: String,
}

/// Presentation intent captured against an authoritative Core input.
/// Input revisions and fingerprints are supplied by the Core domain adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScBinding {
    pub document_uuid: String,
    pub runtime_id: String,
    pub open_generation: u64,
    pub document_revision: u64,
    pub input_revision: u64,
    pub input_fingerprint: String,
}

pub(super) fn bind_input(
    document: &Document,
    open_generation: u64,
    input_revision: u64,
    input_fingerprint: &str,
) -> Result<ScBinding, String> {
    if input_fingerprint.is_empty() {
        return Err("Cannot bind a configurator draft without a stored input fingerprint".into());
    }
    Ok(ScBinding {
        document_uuid: document.document_uuid.to_string(),
        runtime_id: document.runtime_instance_id(),
        open_generation,
        document_revision: document.revision(),
        input_revision,
        input_fingerprint: input_fingerprint.to_owned(),
    })
}

pub(super) fn require_current_input(
    document: &Document,
    open_generation: u64,
    input_revision: u64,
    input_fingerprint: &str,
    expected: &ScBinding,
) -> Result<(), String> {
    if expected.document_uuid != document.document_uuid.to_string()
        || expected.runtime_id != document.runtime_instance_id()
        || expected.open_generation != open_generation
    {
        return Err("The configurator draft belongs to an earlier or different Document instance. Reload before applying.".into());
    }
    if expected.document_revision != document.revision()
        || expected.input_revision != input_revision
        || expected.input_fingerprint != input_fingerprint
    {
        return Err("The Document or configurator input changed while this draft was edited. The draft was retained; reload before applying.".into());
    }
    Ok(())
}

pub(super) fn projection(document: &Document, epoch: u64) -> Result<Option<ScProjection>, String> {
    let domain = document.structural_configurator();
    let Some(current) = domain.current() else {
        return Ok(None);
    };
    let binding = bind_input(
        document,
        epoch,
        current.source_revision.operation_id.get(),
        &current.input_fingerprint,
    )?;
    let analysis = domain
        .snapshots()
        .rev()
        .find(|(id, _)| domain.snapshot_status(*id) == Some(EngineeringSnapshotStatus::Current))
        .map(|(_, snapshot)| snapshot.clone());
    let readiness = ai_revit_core::configured_analysis_readiness(
        current,
        &ConfiguratorAnalysisRequest::WholeBuilding,
    )
    .map_err(|error| error.to_string())?;
    let analysis_status = if analysis.is_some() {
        "SCOPED_NUMERICAL_RESULT".to_owned()
    } else if domain.snapshots().next().is_some() {
        format!("STALE; {}", readiness_name(&readiness))
    } else {
        readiness_name(&readiness).to_owned()
    };
    let generated = &current.generated;
    let scope_summary = analysis
        .as_ref()
        .map(|snapshot| {
            format!(
                "Numerical result applies only to sections {:?}; FrameSelfWeightOnly",
                snapshot.scope.section_ids,
            )
        })
        .unwrap_or_else(|| "No current numerical result".into());
    let summary =
        format!(
        "{} sections · {} nodes · {} members · {} surfaces\n{}\n{} · whole-building readiness: {}",
        current.building.sections.len(), generated.nodes.len(), generated.members.len(),
        generated.surfaces.len(), scope_summary, analysis_status, readiness_name(&readiness),
    );
    let mut lines = vec![
        format!("Building: {}", current.building.id),
        format!("Stored input fingerprint: {}", current.input_fingerprint),
        format!("Stored config fingerprint: {}", generated.config_fingerprint),
        format!("Stored graph fingerprint: {}", generated.graph_fingerprint),
        format!("Input source operation: {}", current.source_revision.operation_id.get()),
        format!("Whole-building analysis readiness: {readiness:?}"),
        "Load scope: FrameSelfWeightOnly. Panel stiffness, panel weight, equipment, occupancy, snow, wind and code combinations are excluded or unverified.".into(),
        "Structural numerical residual checks do not establish member strength, stability, ULS/SLS, reinforcement or engineering approval. No overall SAFE status.".into(),
    ];
    lines.extend(generated.limitations.iter().cloned());
    for (section, limitation) in &generated.generation_limits {
        lines.push(format!("Section {section}: generation HOLD: {limitation}"));
    }
    for group in current.foundation_groups.values() {
        lines.push(format!(
            "Explicit foundation group {}: AllSupportsOfMemberSections {:?}; synthetic input; groundwater/bearing/normative settlement/RC holds remain.",
            group.foundation_id, group.member_section_ids,
        ));
    }
    if current.foundation_groups.is_empty() {
        lines.push(
            "Foundation HOLD: no explicit foundation group is assigned; no transfer is inferred."
                .into(),
        );
    }
    if let Some(snapshot) = &analysis {
        lines.push(format!(
            "Current snapshot request: {:?}; complete scope: {:?}",
            snapshot.request, snapshot.scope.section_ids
        ));
        lines.push(format!(
            "Snapshot fingerprint: {}",
            snapshot.content_fingerprint
        ));
        lines.push(format!(
            "Numerical diagnostics: {:?}",
            snapshot.structural.structural_result.diagnostics
        ));
        lines.push(format!(
            "Producer: {}; source {}; executable {:?}",
            snapshot.structural.producer.solver_version,
            snapshot.structural.producer.source_manifest_sha256,
            snapshot.structural.producer.executable_sha256
        ));
        for reaction in &snapshot.reactions.reactions {
            lines.push(format!(
                "Support {} at {:?} m: {:?} [kN,kNm]; ownership {:?}; foundation {:?}",
                reaction.support_node_id,
                reaction.position_m,
                reaction.force_moment,
                reaction.ownership,
                reaction.foundation_group_id
            ));
        }
        for foundation in &snapshot.foundations {
            lines.push(format!(
                "Foundation {}: {:?}; supports {}; failure {:?}",
                foundation.foundation_id,
                foundation.status,
                foundation.support_node_ids.len(),
                foundation.failure
            ));
            if let Some(result) = &foundation.result {
                lines.push(format!("Foundation PRELIMINARY_ESTIMATE: contact {:?}; settlement plane {:?}; equilibrium {:?}; limits {:?}; self-weight ledger {:?}",
                    result.contact, result.plane, result.equilibrium, result.engineering_limits, result.self_weight_ledger));
            }
        }
    } else {
        lines.push("No current matching analysis snapshot. Prior snapshots remain archive-only; no current result overlay is authorized.".into());
    }
    for (id, snapshot) in domain.snapshots() {
        if domain.snapshot_status(id) != Some(EngineeringSnapshotStatus::Current) {
            lines.push(format!(
                "Archived snapshot {}: STALE; input {}; graph {}; scope {:?}",
                id.get(),
                snapshot.source.input_fingerprint,
                snapshot.source.generated.graph_fingerprint,
                snapshot.scope.section_ids
            ));
        }
    }
    Ok(Some(ScProjection {
        binding,
        config: current.building.clone(),
        generated: Some(generated.clone()),
        generated_current: true,
        analysis_current: analysis.is_some(),
        analysis,
        analysis_status,
        summary,
        report: lines.join("\n"),
    }))
}

fn readiness_name(readiness: &ConfiguratorAnalysisReadiness) -> &'static str {
    match readiness {
        ConfiguratorAnalysisReadiness::Ready { .. } => "READY_FOR_EXPLICIT_ANALYZE",
        ConfiguratorAnalysisReadiness::AnalysisModelCapacityExceeded { .. } => {
            "ANALYSIS_MODEL_CAPACITY_EXCEEDED"
        }
        ConfiguratorAnalysisReadiness::InterfaceLoadPathNotVerified { .. } => {
            "INTERFACE_LOAD_PATH_NOT_VERIFIED"
        }
        ConfiguratorAnalysisReadiness::GenerationBlocked { .. } => "GENERATION_BLOCKED",
        ConfiguratorAnalysisReadiness::DisconnectedLoadPathsNotAcknowledged => {
            "DISCONNECTED_LOAD_PATHS_NOT_ACKNOWLEDGED"
        }
    }
}

/// Uses the shared fixture's coordinates and defaults explicitly authorized for
/// its common synthetic raft. No section parameters are copied into the UI.
fn demo_command() -> Result<Command, String> {
    let (building, foundation_groups) = ai_revit_core::configured_fixture_input(
        structural_configurator::FixtureKind::SteppedConnected,
    )
    .map_err(|error| error.to_string())?;
    Ok(Command::StructuralConfigurator(
        StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
            building,
            foundation_groups,
        },
    ))
}

impl App {
    pub(super) fn structural_configurator_execute(
        &mut self,
        cx: &mut Cx,
        intent: ScIntent,
        expected: Option<ScBinding>,
    ) -> Result<(), String> {
        let document = self
            .document
            .as_ref()
            .ok_or_else(|| "Open a Document before using the configurator".to_owned())?;
        let is_demo = matches!(&intent, ScIntent::CreateDemo);
        if !is_demo {
            let current = document
                .structural_configurator()
                .current()
                .ok_or_else(|| {
                    "No stored building is available; create the shared demo first".to_owned()
                })?;
            let expected = expected.as_ref().ok_or_else(|| {
                "The draft has no current Document binding; reload first".to_owned()
            })?;
            require_current_input(
                document,
                self.api_document_generation,
                current.source_revision.operation_id.get(),
                &current.input_fingerprint,
                expected,
            )?;
        }
        let command = match intent {
            ScIntent::CreateDemo => demo_command()?,
            ScIntent::Apply(building) | ScIntent::Regenerate(building) => {
                let current = document
                    .structural_configurator()
                    .current()
                    .ok_or_else(|| "No stored building".to_owned())?;
                // Assignments are authoritative domain input. Section edits do
                // not merge or repartition independent foundation groups.
                let foundation_groups = current.foundation_groups.values().cloned().collect();
                Command::StructuralConfigurator(
                    StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
                        building,
                        foundation_groups,
                    },
                )
            }
            ScIntent::Analyze { section_id } => {
                let current = document
                    .structural_configurator()
                    .current()
                    .ok_or_else(|| "No stored building".to_owned())?;
                let request = match section_id {
                    Some(seed_section_id) => {
                        ConfiguratorAnalysisRequest::ConnectedComponent { seed_section_id }
                    }
                    None => ConfiguratorAnalysisRequest::WholeBuilding,
                };
                let readiness = ai_revit_core::configured_analysis_readiness(current, &request)
                    .map_err(|error| error.to_string())?;
                if !matches!(&readiness, ConfiguratorAnalysisReadiness::Ready { .. }) {
                    return Err(format!("{}: {readiness:?}", readiness_name(&readiness)));
                }
                let snapshot = ai_revit_core::analyze_configured_building(
                    current,
                    request,
                    producer_from_environment()?,
                )
                .map_err(|error| error.to_string())?;
                Command::StructuralConfigurator(
                    StructuralConfiguratorCommand::SaveAnalysisSnapshot { snapshot },
                )
            }
        };
        let revision = document.revision();
        let command_id = self.next_command_id("structural-configurator");
        self.apply_typed_commands(cx, command_id, revision, vec![command])
            .map_err(|error| error.message)?;
        Ok(())
    }
}

/// Bind this executable's compiled source pin to the current runtime receipt.
/// The optional executable value is only a guard; the actual hash is always saved.
fn producer_from_environment() -> Result<EngineeringSourceIdentity, String> {
    let compiled = option_env!("AI_REVIT_SC_COMPILED_SOURCE_MANIFEST_SHA256").ok_or_else(|| {
        "AI_REVIT_SC_COMPILED_SOURCE_MANIFEST_SHA256 was not bound at compile time".to_owned()
    })?;
    let compiled = normalize_sha256("AI_REVIT_SC_COMPILED_SOURCE_MANIFEST_SHA256", compiled)?;
    let runtime = env::var("AI_REVIT_SC_SOURCE_MANIFEST_SHA256")
        .map_err(|_| "AI_REVIT_SC_SOURCE_MANIFEST_SHA256 is required".to_owned())?;
    let runtime = normalize_sha256("AI_REVIT_SC_SOURCE_MANIFEST_SHA256", &runtime)?;
    if runtime != compiled {
        return Err(
            "Runtime source manifest does not match the compiled configurator source pin".into(),
        );
    }
    let actual = current_executable_sha256()?;
    match env::var("AI_REVIT_SC_EXECUTABLE_SHA256") {
        Ok(expected) if !expected.trim().is_empty() => {
            if normalize_sha256("AI_REVIT_SC_EXECUTABLE_SHA256", &expected)? != actual {
                return Err("Running executable differs from AI_REVIT_SC_EXECUTABLE_SHA256".into());
            }
        }
        Ok(_) | Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(_)) => {
            return Err("AI_REVIT_SC_EXECUTABLE_SHA256 must be Unicode SHA-256 hex".into())
        }
    }
    Ok(EngineeringSourceIdentity {
        source_manifest_sha256: runtime,
        executable_sha256: Some(actual),
        solver_version: format!("structural-configurator-stage1/{}", env!("CARGO_PKG_VERSION")),
        source_label: "Root-recorded configurator source identity; FrameSelfWeightOnly, no engineering acceptance".into(),
    })
}

fn normalize_sha256(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{name} must contain exactly 64 hexadecimal characters"
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn current_executable_sha256() -> Result<String, String> {
    let path = env::current_exe()
        .map_err(|error| format!("Cannot identify current executable: {error}"))?;
    let mut file = File::open(&path)
        .map_err(|error| format!("Cannot read current executable {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot hash current executable: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn pick_readback(pick: &ScPick) -> Value {
    let (kind, id) = match pick {
        ScPick::Node(id) => ("NODE", id),
        ScPick::Member(id) => ("MEMBER", id),
        ScPick::Surface(id) => ("SURFACE", id),
        ScPick::Section(id) => ("SECTION", id),
        ScPick::Interface(id) => ("INTERFACE", id),
        ScPick::Support(id) => ("SUPPORT", id),
        ScPick::Displacement(id) => ("DISPLACEMENT", id),
        ScPick::DeformedMember(id) => ("DEFORMED_MEMBER", id),
        ScPick::Reaction(id) => ("REACTION", id),
        ScPick::AppliedLoad {
            load_case_id,
            source_id,
            target,
        } => {
            let target = match target {
                ScLoadTarget::Node(id) => json!({"kind": "NODE", "semantic_id": id.as_str()}),
                ScLoadTarget::Member(id) => json!({"kind": "MEMBER", "semantic_id": id.as_str()}),
            };
            return json!({"kind": "APPLIED_LOAD", "load_case_id": load_case_id.to_string(), "source_id": source_id, "target": target});
        }
        ScPick::Foundation(id) => {
            return json!({"kind": "FOUNDATION", "foundation_id": id.as_str()})
        }
        ScPick::FoundationCell {
            foundation_id,
            cell_index,
        } => {
            return json!({
                "kind": "FOUNDATION_CELL", "foundation_id": foundation_id.as_str(), "cell_index": cell_index,
            })
        }
        ScPick::GroundLayer {
            foundation_id,
            layer_index,
        } => {
            return json!({
                "kind": "GROUND_LAYER", "foundation_id": foundation_id.as_str(), "layer_index": layer_index,
            })
        }
    };
    json!({"kind": kind, "semantic_id": id.as_str()})
}

impl App {
    /// Read-only application state. Runtime GUI qualification is recorded by the
    /// external owner; declared aliases and generated geometry are not its proof.
    pub(super) fn structural_configurator_ui_state(&self, cx: &Cx) -> Value {
        let state = &self.structural_configurator_ui;
        let (stored, projection_error) = match self.document.as_ref() {
            Some(document) => match projection(document, self.api_document_generation) {
                Ok(value) => (value, None),
                Err(error) => (None, Some(error)),
            },
            None => (None, None),
        };
        let document = self.document.as_ref().map(|document| {
            json!({
                "uuid": document.document_uuid.to_string(),
                "runtime_id": document.runtime_instance_id(),
                "open_generation": self.api_document_generation,
                "revision": document.revision(),
            })
        });
        let draft_binding = state.binding.as_ref().map(|binding| {
            json!({
                "document_uuid": binding.document_uuid,
                "runtime_id": binding.runtime_id,
                "open_generation": binding.open_generation,
                "document_revision": binding.document_revision,
                "input_revision": binding.input_revision,
                "input_fingerprint": binding.input_fingerprint,
            })
        });
        let stored_generated = stored.as_ref().and_then(|value| value.generated.as_ref());
        let capacity = stored_generated
            .and_then(|generated| serde_json::to_value(&generated.analysis_status).ok());

        // These are read from the actual borrowed native widget's Viewer.
        // No expected count from GeneratedBuilding is substituted on failure.
        let viewport_ref = self.ui.cad_viewport(cx, ids!(sc_model_viewport));
        let actual_view = viewport_ref.borrow().map(|viewport| {
            let viewer = viewport.viewer();
            let object_count = viewer.scene().objects().count();
            let visible_count = viewer.scene().objects().filter(|object| object.visible).count();
            let selected_ids: Vec<_> = viewer.scene().selected_ids().into_iter()
                .map(|id| id.to_string()).collect();
            const READBACK_OBJECT_LIMIT: usize = 1024;
            let objects: Vec<_> = viewer.scene().objects().take(READBACK_OBJECT_LIMIT)
                .map(|object| {
                    let sources = state.scene_sources.get(&object.object_id);
                    json!({
                        "application_object_id": object.object_id.to_string(),
                        "visible": object.visible,
                        "semantic_target": state.scene_picks.get(&object.object_id).map(pick_readback),
                        "self_weight_source_id": sources.and_then(|value| value.self_weight_source_id.as_ref()).map(|id| id.as_str()),
                        "load_source_id": sources.and_then(|value| value.load_source_id.as_ref()).map(|id| id.as_str()),
                        "applied_load_source_ids": sources.map(|value| &value.applied_load_source_ids),
                        "foundation_id": sources.and_then(|value| value.foundation_id.as_ref()).map(|id| id.as_str()),
                    })
                }).collect();
            json!({
                "object_count": object_count,
                "visible_object_count": visible_count,
                "selected_application_object_ids": selected_ids,
                "camera": serde_json::to_value(viewer.camera().state()).ok(),
                "viewport": {"width": viewer.viewport().width, "height": viewer.viewport().height},
                "objects": objects,
                "object_bindings_total": object_count,
                "object_bindings_truncated": object_count > READBACK_OBJECT_LIMIT,
            })
        });
        let canvas_ref = self.ui.drafting_canvas(cx, ids!(sc_plan_canvas));
        let canvas_bounds = canvas_ref.borrow().and_then(|canvas| {
            let rect = canvas.rect;
            if rect.pos.x.is_finite()
                && rect.pos.y.is_finite()
                && rect.size.x.is_finite()
                && rect.size.y.is_finite()
                && rect.size.x > 0.0
                && rect.size.y > 0.0
            {
                Some(json!({
                    "x": rect.pos.x, "y": rect.pos.y,
                    "width": rect.size.x, "height": rect.size.y,
                    "dpi_factor": canvas.dpi_factor,
                }))
            } else {
                None
            }
        });
        json!({
            "readback_kind": "APP_OWNED_STATE",
            "active": state.active,
            "active_view": state.view.name(),
            "document": document,
            "draft_binding": draft_binding,
            "stored_input_revision": stored.as_ref().map(|value| value.binding.input_revision),
            "stored_input_fingerprint": stored.as_ref().map(|value| value.binding.input_fingerprint.as_str()),
            "stored_config_fingerprint": stored_generated.map(|value| value.config_fingerprint.as_str()),
            "stored_graph_fingerprint": stored_generated.map(|value| value.graph_fingerprint.as_str()),
            "stored_generated_current": stored.as_ref().map(|value| value.generated_current),
            "stored_analysis_current": stored.as_ref().map(|value| value.analysis_current),
            "analysis_scope": stored.as_ref().and_then(|value| value.analysis.as_ref()).map(|snapshot| &snapshot.scope.section_ids),
            "current_snapshot_fingerprint": stored.as_ref().and_then(|value| value.analysis.as_ref()).map(|snapshot| snapshot.content_fingerprint.as_str()),
            "analysis_status": stored.as_ref().map(|value| value.analysis_status.as_str()),
            "analysis_capacity": capacity,
            "selected_section": state.selected_section.as_ref().map(|id| id.as_str()),
            "selected_corner": state.selected_corner.as_ref().map(|id| id.as_str()),
            "selected_element": state.selection.as_ref().map(pick_readback),
            "dirty": state.draft_dirty,
            "stale": state.binding_stale,
            "scene_graph_fingerprint": state.scene_graph_fingerprint,
            "scene_overlay_snapshot_fingerprint": state.scene_overlay_snapshot_fingerprint,
            "actual_view": actual_view,
            "canvas_bounds": canvas_bounds,
            "projection_error": projection_error,
            "aliases_kind": "SOURCE_DECLARATIONS",
            "aliases": crate::structural_configurator_ui::STRUCTURAL_CONFIGURATOR_ALIASES,
        })
    }
}
