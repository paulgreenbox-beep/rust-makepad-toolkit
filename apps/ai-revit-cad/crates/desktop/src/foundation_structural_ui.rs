//! Native Stage 1 foundation/structures workspace.
//!
//! The UI owns only presentation, camera, selection and an explicitly bound
//! text draft. Persistent inputs and calculation snapshots stay in App's one
//! `Document` through the normal typed-command path.

use super::*;
use crate::drafting_canvas::{
    AnalysisCanvasLayer, CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput,
    DraftingCanvasWidgetRefExt, PointerButton, PointerPhase,
};
use crate::foundation_structural_api as fs_api;
use crate::foundation_structural_render::{
    self as fs_render, MatDisplay, RenderData, Stage1Pick, Stage1View,
};
use crate::viewport::{CadViewportAction, CadViewportWidgetRefExt};
use drafting_core::{Bounds2, Camera2D, Point2 as DraftPoint};
use makepad_widgets::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use viewer_core::{ObjectId, Vec3, Viewer, ViewerEvent};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.FoundationStructuralWorkspace = SolidView{
        width: Fill height: Fill flow: Right visible: false
        draw_bg.color: #x111922

        ScrollYView{
            width: 328 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            H3{width: Fill text: "Engineering Foundations / Structures" draw_text.color: #xe5edf6}
            View{width: Fill height: 32 flow: Right spacing: 5
                fs_create_demo := Button{width: Fill text: "Create demo"}
                fs_reload := Button{width: Fill text: "Reload"}
            }
            View{width: Fill height: 32 flow: Right spacing: 5
                fs_apply := Button{width: Fill text: "Apply input"}
                fs_analyze := Button{width: Fill text: "Analyze"}
            }
            fs_load_source_host := View{width: Fill height: Fit flow: Down visible: false
                Label{width: Fill text: "Load source · independent input modes" draw_text.color: #xb6c8d8}
                fs_load_source := DropDown{width: Fill labels: ["Structural reactions", "Manual synthetic 300 t"]}
            }
            Label{width: Fill text: "Material · elastic inputs only · kN/m²" draw_text.color: #xb6c8d8}
            View{width: Fill height: 30 flow: Right spacing: 5
                fs_material_e := TextInput{width: Fill height: 30 empty_text: "E, kN/m²"}
                fs_material_g := TextInput{width: Fill height: 30 empty_text: "G, kN/m²"}
            }
            Label{width: Fill text: "Section · area m²; inertias/torsion m⁴" draw_text.color: #xb6c8d8}
            View{width: Fill height: 30 flow: Right spacing: 5
                fs_area := TextInput{width: Fill height: 30 empty_text: "A, m²"}
                fs_iy := TextInput{width: Fill height: 30 empty_text: "Iy, m⁴"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                fs_iz := TextInput{width: Fill height: 30 empty_text: "Iz, m⁴"}
                fs_torsion := TextInput{width: Fill height: 30 empty_text: "J, m⁴"}
            }
            Label{width: Fill text: "Synthetic soil · layer E is metadata, independent of k" draw_text.color: #xb6c8d8}
            View{width: Fill height: 30 flow: Right spacing: 5
                fs_soil_e := TextInput{width: Fill height: 30 empty_text: "Layer E, kPa"}
                fs_soil_poisson := TextInput{width: Fill height: 30 empty_text: "ν"}
            }
            fs_bedding_k := TextInput{width: Fill height: 30 empty_text: "Uniform bedding k, kN/m³"}
            fs_mat_thickness := TextInput{width: Fill height: 30 empty_text: "Rigid mat thickness, m"}
            Label{width: Fill height: Fit text: "Site verification remains domain-owned. Structural reactions transfer only through the matched Core snapshot. Manual 300 t is not a frame reaction." draw_text.color: #xe4b77a}
        }

        View{width: Fill height: Fill flow: Down
            SolidView{width: Fill height: 40 flow: Right spacing: 5 padding: 4 draw_bg.color: #x202a34
                fs_view_foundation_plan := Button{width: Fill text: "Foundation plan"}
                fs_view_frame_3d := Button{width: Fill text: "Frame 3D"}
                fs_view_pressure_2d := Button{width: Fill text: "Pressure 2D"}
                fs_view_ground_3d := Button{width: Fill text: "Ground 3D"}
            }
            View{width: Fill height: 30 flow: Right spacing: 6 align: Align{y:0.5} padding: Inset{left: 8 right: 8}
                Label{width: Fit text: "Deformation scale · display only · 0–10000" draw_text.color: #xb6c8d8}
                fs_deformation_scale := Slider{width: Fill height: 28 min: 0 max: 10000 step: 1 default: 1 precision: 0}
            }
            fs_status := Label{width: Fill height: Fit text: "MODEL_INCOMPLETE" draw_text.color: #xf2c47c}
            fs_attempt_status := Label{width: Fill height: Fit text: "" draw_text.color: #xe4b77a}
            fs_summary := Label{width: Fill height: Fit text: "Create a demo or open a Document." draw_text.color: #xc4d2df}
            fs_scope_hold := Label{width: Fill height: Fit text: "Synthetic / mesh-derived display · ULS/SLS (КЖ): HOLD · no overall SAFE status" draw_text.color: #xe4b77a}
            fs_plan_host := View{width: Fill height: Fill visible: false
                fs_canvas := mod.widgets.DraftingCanvas{}
            }
            fs_scene_host := View{width: Fill height: Fill visible: false
                fs_viewport := mod.widgets.CadViewport{}
            }
        }

        ScrollYView{width: 350 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            H3{width: Fill text: "Inspector / report" draw_text.color: #xe5edf6}
            fs_binding_label := Label{width: Fill height: Fit text: "No Document binding" draw_text.color: #x9badbe}
            fs_inspector_text := TextInput{width: Fill height: 132 is_multiline: true is_read_only: true text: "No typed selection."}
            Hr{}
            Label{width: Fill text: "Read-only report · archive freshness is explicit" draw_text.color: #xb6c8d8}
            fs_report_text := TextInput{width: Fill height: 420 is_multiline: true is_read_only: true text: "No current structural/foundation input."}
        }
    }
}

pub(super) struct FoundationStructuralUi {
    pub active: bool,
    pub view: Stage1View,
    pub deformation_scale: f64,
    pub selection: Option<Stage1Pick>,
    pub binding: Option<fs_api::FsBinding>,
    pub editor_values: Option<fs_api::FsEditValues>,
    pub draft_dirty: bool,
    pub binding_stale: bool,
    pub render_generation: u64,
    projection: Option<fs_api::FsProjection>,
    camera: Camera2D,
    camera_change: Option<CanvasCameraChange>,
    pan_anchor: Option<DraftPoint>,
    scene_picks: BTreeMap<ObjectId, Stage1Pick>,
    screen: Arc<AnalysisCanvasLayer>,
    screen_key: Option<String>,
    render_key: Option<String>,
    failed_input_hash: Option<String>,
    attempt_status: Option<String>,
    attempt_message: String,
}

impl Default for FoundationStructuralUi {
    fn default() -> Self {
        Self {
            active: false,
            view: Stage1View::FoundationPlan,
            deformation_scale: 1.0,
            selection: None,
            binding: None,
            editor_values: None,
            draft_dirty: false,
            binding_stale: false,
            render_generation: 0,
            projection: None,
            camera: Camera2D::default(),
            camera_change: None,
            pan_anchor: None,
            scene_picks: BTreeMap::new(),
            screen: Arc::new(AnalysisCanvasLayer::default()),
            screen_key: None,
            render_key: None,
            failed_input_hash: None,
            attempt_status: None,
            attempt_message: String::new(),
        }
    }
}

impl App {
    pub(super) fn handle_foundation_structural_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self
            .ui
            .button(cx, ids!(workspace_foundation_structural))
            .clicked(actions)
        {
            self.foundation_structural_set_active(cx, !self.foundation_structural_ui.active);
        }
        if self.foundation_structural_ui.active {
            let switched_workspace = [
                id!(workspace_3d),
                id!(workspace_2d),
                id!(workspace_engineering),
                id!(workspace_circular),
            ]
            .into_iter()
            .any(|id| self.ui.button(cx, &[id]).clicked(actions));
            if switched_workspace {
                self.foundation_structural_set_active(cx, false);
                return;
            }
        }
        if !self.foundation_structural_ui.active {
            return;
        }

        let mut editor_changed = false;
        for id in [
            id!(fs_material_e),
            id!(fs_material_g),
            id!(fs_area),
            id!(fs_iy),
            id!(fs_iz),
            id!(fs_torsion),
            id!(fs_soil_e),
            id!(fs_soil_poisson),
            id!(fs_bedding_k),
            id!(fs_mat_thickness),
        ] {
            if self.ui.text_input(cx, &[id]).changed(actions).is_some() {
                editor_changed = true;
            }
        }
        if self
            .ui
            .drop_down(cx, ids!(fs_load_source))
            .selected(actions)
            .is_some()
            && self.foundation_structural_ui.editor_values.is_some()
            && !self.foundation_structural_ui.binding_stale
        {
            editor_changed = true;
        }
        if editor_changed
            && self.foundation_structural_ui.editor_values.is_some()
            && !self.foundation_structural_ui.binding_stale
        {
            self.foundation_structural_mark_draft(cx);
        }

        if let Some(scale) = self
            .ui
            .slider(cx, ids!(fs_deformation_scale))
            .slided(actions)
        {
            if scale.is_finite() {
                self.foundation_structural_ui.deformation_scale = scale.clamp(0.0, 10_000.0);
                self.foundation_structural_render_changed(cx, false);
            }
        }

        if self.ui.button(cx, ids!(fs_create_demo)).clicked(actions) {
            self.foundation_structural_create_demo(cx);
            return;
        }
        if self.ui.button(cx, ids!(fs_reload)).clicked(actions) {
            self.foundation_structural_reload(cx);
            return;
        }
        if self.ui.button(cx, ids!(fs_apply)).clicked(actions) {
            self.foundation_structural_apply(cx);
            return;
        }
        if self.ui.button(cx, ids!(fs_analyze)).clicked(actions) {
            self.foundation_structural_analyze(cx);
            return;
        }

        for (id, view) in [
            (id!(fs_view_foundation_plan), Stage1View::FoundationPlan),
            (id!(fs_view_frame_3d), Stage1View::Frame3D),
            (id!(fs_view_pressure_2d), Stage1View::Pressure2D),
            (id!(fs_view_ground_3d), Stage1View::Ground3D),
        ] {
            if self.ui.button(cx, &[id]).clicked(actions) {
                self.foundation_structural_ui.view = view;
                self.foundation_structural_ui.selection = None;
                self.foundation_structural_ui.pan_anchor = None;
                self.foundation_structural_render_changed(cx, true);
                break;
            }
        }

        let canvas_actions = self
            .ui
            .drafting_canvas(cx, ids!(fs_canvas))
            .actions(actions);
        for action in canvas_actions {
            self.foundation_structural_canvas_action(cx, action);
        }

        let viewport_actions = self.ui.cad_viewport(cx, ids!(fs_viewport)).actions(actions);
        let mut picked_object = None;
        let mut selection_changed = false;
        for action in viewport_actions {
            match action {
                CadViewportAction::Viewer(
                    ViewerEvent::ObjectPicked { .. } | ViewerEvent::SelectionChanged { .. },
                ) => selection_changed = true,
                CadViewportAction::Inspect(object_id) => {
                    picked_object = Some(object_id);
                    selection_changed = true;
                }
                _ => {}
            }
        }
        if selection_changed {
            if picked_object.is_none() {
                picked_object = self
                    .ui
                    .cad_viewport(cx, ids!(fs_viewport))
                    .borrow()
                    .and_then(|viewport| viewport.viewer().scene().selected_ids().first().copied());
            }
            self.foundation_structural_ui.selection = picked_object
                .and_then(|object_id| self.foundation_structural_ui.scene_picks.get(&object_id))
                .copied();
            self.foundation_structural_render_changed(cx, false);
        }
    }

    pub(super) fn foundation_structural_apply_visibility(&self, cx: &mut Cx) {
        let active = self.foundation_structural_ui.active;
        self.ui
            .widget(cx, ids!(fs_workspace))
            .set_visible(cx, active);
        self.ui
            .button(cx, ids!(workspace_foundation_structural))
            .set_text(
                cx,
                if active {
                    "● Structures"
                } else {
                    "Structures"
                },
            );
        if !active {
            return;
        }
        let plan = matches!(
            self.foundation_structural_ui.view,
            Stage1View::FoundationPlan | Stage1View::Pressure2D
        );
        for (id, visible) in [
            (ids!(fs_plan_host), plan),
            (ids!(fs_scene_host), !plan),
            (ids!(history_panel), false),
            (ids!(inspector_panel), false),
            (ids!(viewport_panel), false),
            (ids!(geotech_parameters), false),
            (ids!(geotech_results), false),
            (ids!(geotech_actions), false),
            (ids!(circular_parameters_host), false),
            (ids!(circular_results), false),
            (ids!(circular_actions), false),
            (ids!(circular_view_status), false),
            (ids!(model_toolbar), false),
            (ids!(drafting_toolbar), false),
            (ids!(drafting_command_panel), false),
            (ids!(viewport_3d_host), false),
            (ids!(drafting_2d_host), false),
        ] {
            self.ui.widget(cx, id).set_visible(cx, visible);
        }
        self.ui.button(cx, ids!(fs_view_foundation_plan)).set_text(
            cx,
            if self.foundation_structural_ui.view == Stage1View::FoundationPlan {
                "● Foundation plan"
            } else {
                "Foundation plan"
            },
        );
        self.ui.button(cx, ids!(fs_view_frame_3d)).set_text(
            cx,
            if self.foundation_structural_ui.view == Stage1View::Frame3D {
                "● Frame 3D"
            } else {
                "Frame 3D"
            },
        );
        self.ui.button(cx, ids!(fs_view_pressure_2d)).set_text(
            cx,
            if self.foundation_structural_ui.view == Stage1View::Pressure2D {
                "● Pressure 2D"
            } else {
                "Pressure 2D"
            },
        );
        self.ui.button(cx, ids!(fs_view_ground_3d)).set_text(
            cx,
            if self.foundation_structural_ui.view == Stage1View::Ground3D {
                "● Ground 3D"
            } else {
                "Ground 3D"
            },
        );
        let edit_enabled = self.foundation_structural_ui.editor_values.is_some()
            && !self.foundation_structural_ui.binding_stale
            && self.document.is_some();
        for id in [
            id!(fs_material_e),
            id!(fs_material_g),
            id!(fs_area),
            id!(fs_iy),
            id!(fs_iz),
            id!(fs_torsion),
            id!(fs_soil_e),
            id!(fs_soil_poisson),
            id!(fs_bedding_k),
            id!(fs_mat_thickness),
        ] {
            self.ui
                .text_input(cx, &[id])
                .set_is_read_only(cx, !edit_enabled);
        }
        self.ui
            .widget(cx, ids!(fs_load_source_host))
            .set_visible(cx, edit_enabled);
        self.ui
            .button(cx, ids!(fs_apply))
            .set_enabled(cx, edit_enabled);
        self.ui.button(cx, ids!(fs_analyze)).set_enabled(
            cx,
            self.document.is_some()
                && !self.foundation_structural_ui.draft_dirty
                && !self.foundation_structural_ui.binding_stale,
        );
    }

    pub(super) fn foundation_structural_after_event(&mut self, cx: &mut Cx) {
        if !self.foundation_structural_ui.active {
            return;
        }
        let Some(document) = self.document.as_ref() else {
            let draft_dirty = self.foundation_structural_ui.draft_dirty;
            self.foundation_structural_ui.binding_stale = draft_dirty;
            self.foundation_structural_ui.projection = None;
            self.foundation_structural_ui.binding = None;
            self.foundation_structural_ui.selection = None;
            if !draft_dirty {
                self.foundation_structural_clear_editor(cx);
            }
            self.foundation_structural_clear_views(cx);
            self.ui
                .label(cx, ids!(fs_status))
                .set_text(cx, "MODEL_INCOMPLETE");
            self.ui.label(cx, ids!(fs_binding_label)).set_text(
                cx,
                if draft_dirty {
                    "No Document open · retained draft is unbound"
                } else {
                    "No Document open · no active binding"
                },
            );
            self.ui
                .label(cx, ids!(fs_summary))
                .set_text(
                    cx,
                    if draft_dirty {
                        "MODEL_INCOMPLETE · no current Document analysis is available. The input draft is preserved; reopen and reload a Document before applying it."
                    } else {
                        "MODEL_INCOMPLETE · no current Document analysis is available. Open or create a Document to continue."
                    },
                );
            let report = if draft_dirty {
                "MODEL_INCOMPLETE · no current Document analysis is available.\n\nThe visible input draft is preserved but unbound; it cannot be applied or analyzed until a Document is reopened and reloaded."
            } else {
                "MODEL_INCOMPLETE · no current Document analysis is available.\n\nNo Document is open."
            };
            self.ui
                .text_input(cx, ids!(fs_report_text))
                .set_text(cx, report);
            self.foundation_structural_apply_visibility(cx);
            return;
        };
        let live_binding = fs_api::binding(document, self.api_document_generation);
        let Some(previous) = self.foundation_structural_ui.binding.clone() else {
            self.foundation_structural_refresh(cx, true);
            return;
        };
        if previous == live_binding {
            return;
        }
        let replaced = previous.document_uuid != live_binding.document_uuid
            || previous.runtime_id != live_binding.runtime_id
            || previous.open_generation != live_binding.open_generation;
        if self.foundation_structural_ui.draft_dirty {
            self.foundation_structural_ui.binding_stale = true;
            self.foundation_structural_ui.failed_input_hash = self
                .foundation_structural_ui
                .projection
                .as_ref()
                .map(|projection| projection.input_hash.clone());
            self.foundation_structural_clear_views(cx);
            self.foundation_structural_show_attempt(
                cx,
                None,
                "Document changed while this draft was edited. Reload before applying; the draft was retained.",
            );
            self.foundation_structural_apply_visibility(cx);
            return;
        }
        if replaced {
            self.foundation_structural_ui.selection = None;
            self.foundation_structural_ui.failed_input_hash = None;
            self.foundation_structural_ui.attempt_status = None;
            self.foundation_structural_ui.attempt_message.clear();
        }
        self.foundation_structural_refresh(cx, false);
    }

    pub(super) fn foundation_structural_set_active(&mut self, cx: &mut Cx, active: bool) {
        if active {
            let _ = self.set_workspace(cx, "3d");
        }
        self.foundation_structural_ui.active = active;
        cx.revert_key_focus();
        if active {
            self.foundation_structural_apply_visibility(cx);
            self.foundation_structural_after_event(cx);
            self.foundation_structural_render_changed(cx, true);
        } else {
            self.foundation_structural_ui.pan_anchor = None;
            self.refresh_workspace_visibility(cx);
            self.refresh_drafting(cx, true);
            self.foundation_structural_apply_visibility(cx);
        }
    }

    pub(super) fn foundation_structural_ui_state(&self, cx: &Cx) -> Value {
        let state = &self.foundation_structural_ui;
        let projection = state.projection.as_ref();
        let report = self.ui.text_input(cx, ids!(fs_report_text)).text();
        json!({
            "active": state.active,
            "view": stage1_view_name(state.view),
            "primary_status": projection.map(|value| value.status.as_str()).unwrap_or("MODEL_INCOMPLETE"),
            "attempt_status": state.attempt_status,
            "attempt_message": state.attempt_message,
            "input_hash": projection.map(|value| value.input_hash.as_str()),
            "document_uuid": state.binding.as_ref().map(|value| value.document_uuid.as_str()),
            "runtime_id": state.binding.as_ref().map(|value| value.runtime_id.as_str()),
            "open_generation": state.binding.as_ref().map(|value| value.open_generation),
            "document_revision": state.binding.as_ref().map(|value| value.revision),
            "draft_dirty": state.draft_dirty,
            "binding_stale": state.binding_stale,
            "current_result": projection.is_some_and(|value| value.current) && !self.foundation_structural_suppressed(),
            "render_generation": state.render_generation,
            "deformation_scale": state.deformation_scale,
            "selection": state.selection.map(|value| format!("{value:?}")),
            "report": report,
        })
    }

    fn foundation_structural_refresh(&mut self, cx: &mut Cx, fit: bool) {
        let projection = match self.document.as_ref() {
            Some(document) => fs_api::projection(document, self.api_document_generation),
            None => Ok(None),
        };
        let projection = match projection {
            Ok(projection) => projection,
            Err(error) => {
                self.foundation_structural_show_attempt(cx, Some("INVALID_INPUT"), &error);
                self.foundation_structural_clear_views(cx);
                return;
            }
        };
        let Some(projection) = projection else {
            self.foundation_structural_ui.projection = None;
            self.foundation_structural_ui.binding = None;
            self.foundation_structural_clear_editor(cx);
            self.foundation_structural_ui.selection = None;
            self.foundation_structural_ui.draft_dirty = false;
            self.foundation_structural_ui.binding_stale = false;
            self.ui
                .label(cx, ids!(fs_status))
                .set_text(cx, "MODEL_INCOMPLETE");
            self.ui
                .label(cx, ids!(fs_binding_label))
                .set_text(cx, "Document has no stored foundation/structural input.");
            self.ui.label(cx, ids!(fs_summary)).set_text(
                cx,
                "Use Create demo or reload a Document that contains a typed input.",
            );
            self.ui
                .text_input(cx, ids!(fs_report_text))
                .set_text(cx, "MODEL_INCOMPLETE · input only · no analysis snapshot.");
            self.foundation_structural_clear_views(cx);
            self.foundation_structural_apply_visibility(cx);
            return;
        };

        if !self.foundation_structural_ui.draft_dirty {
            self.foundation_structural_ui.binding = Some(projection.binding.clone());
            self.foundation_structural_ui.binding_stale = false;
            self.foundation_structural_ui.editor_values = projection.editor;
            if let Some(values) = projection.editor {
                self.foundation_structural_fill_editor(cx, values);
            } else {
                self.foundation_structural_clear_editor(cx);
            }
        }
        let render_key = foundation_structural_render_key(
            &projection,
            self.foundation_structural_ui.view,
            self.foundation_structural_ui.deformation_scale,
            self.foundation_structural_ui.selection,
            self.foundation_structural_ui.draft_dirty,
            self.foundation_structural_ui.binding_stale,
            self.foundation_structural_suppressed_for(&projection),
        );
        if self.foundation_structural_ui.render_key.as_deref() != Some(render_key.as_str()) {
            self.foundation_structural_ui.render_generation = self
                .foundation_structural_ui
                .render_generation
                .saturating_add(1);
            self.foundation_structural_ui.render_key = Some(render_key);
            self.foundation_structural_ui.screen_key = None;
        }
        self.foundation_structural_ui.projection = Some(projection);
        self.foundation_structural_update_labels(cx);
        self.foundation_structural_update_inspector(cx);
        if self.foundation_structural_ui.draft_dirty || self.foundation_structural_ui.binding_stale
        {
            self.foundation_structural_clear_views(cx);
        } else {
            self.foundation_structural_render_changed(cx, fit);
        }
        self.foundation_structural_apply_visibility(cx);
    }

    fn foundation_structural_render_changed(&mut self, cx: &mut Cx, fit: bool) {
        if !self.foundation_structural_ui.active {
            return;
        }
        if self.foundation_structural_ui.draft_dirty || self.foundation_structural_ui.binding_stale
        {
            self.foundation_structural_clear_views(cx);
            return;
        }
        let Some(projection) = self.foundation_structural_ui.projection.clone() else {
            self.foundation_structural_clear_views(cx);
            return;
        };
        self.foundation_structural_render_plan(cx, &projection, fit);
        self.foundation_structural_render_scene(cx, &projection, fit);
        self.foundation_structural_update_inspector(cx);
        self.foundation_structural_apply_visibility(cx);
    }

    fn foundation_structural_render_plan(
        &mut self,
        cx: &mut Cx,
        projection: &fs_api::FsProjection,
        fit: bool,
    ) {
        if !matches!(
            self.foundation_structural_ui.view,
            Stage1View::FoundationPlan | Stage1View::Pressure2D
        ) {
            return;
        }
        let size = self
            .ui
            .drafting_canvas(cx, ids!(fs_canvas))
            .borrow()
            .map(|canvas| canvas.rect.size);
        if let Some(size) = size {
            if size.x > 1.0
                && size.y > 1.0
                && (self.foundation_structural_ui.camera.viewport_width != size.x
                    || self.foundation_structural_ui.camera.viewport_height != size.y)
            {
                if self
                    .foundation_structural_ui
                    .camera
                    .resize(size.x, size.y)
                    .is_ok()
                {
                    self.foundation_structural_ui.camera_change = Some(CanvasCameraChange::Resize);
                    self.foundation_structural_ui.screen_key = None;
                }
            }
        }

        let Some(mut mat) = self.foundation_structural_display_mat(projection) else {
            self.foundation_structural_set_canvas_frame(
                cx,
                projection,
                AnalysisCanvasLayer::default(),
            );
            return;
        };
        if fit {
            let bounds = Bounds2::from_points(
                DraftPoint::new(-mat.length_x_m * 0.5, -mat.width_y_m * 0.5),
                DraftPoint::new(mat.length_x_m * 0.5, mat.width_y_m * 0.5),
            );
            if self
                .foundation_structural_ui
                .camera
                .fit_bounds(bounds, 48.0)
                .is_ok()
            {
                self.foundation_structural_ui.camera_change = Some(CanvasCameraChange::Fit);
                self.foundation_structural_ui.screen_key = None;
            }
        }
        let suppressed = self.foundation_structural_suppressed_for(projection);
        if suppressed {
            mat.pressure_cells.clear();
        }
        let result = if suppressed {
            None
        } else {
            projection.structural_result.as_ref()
        };
        let data = RenderData {
            model: &projection.model,
            structural_result: result,
            mat: Some(&mat),
            deformation_scale: self.foundation_structural_ui.deformation_scale,
        };
        let camera = &self.foundation_structural_ui.camera;
        let view = self.foundation_structural_ui.view;
        let layer_key = format!(
            "{}:{}:{:?}:{}:{}:{:?}:{}",
            projection.input_hash,
            if projection.current {
                projection.binding.revision
            } else {
                0
            },
            view,
            camera.revision,
            self.foundation_structural_ui.deformation_scale.to_bits(),
            self.foundation_structural_ui.selection,
            suppressed,
        );
        if self.foundation_structural_ui.screen_key.as_deref() != Some(layer_key.as_str()) {
            match fs_render::plan_layer(
                &data,
                view,
                camera,
                self.foundation_structural_ui.selection.as_ref(),
            ) {
                Ok(layer) => {
                    self.foundation_structural_ui.screen = Arc::new(layer);
                    self.foundation_structural_ui.screen_key = Some(layer_key);
                }
                Err(error) => {
                    self.foundation_structural_ui.screen = Arc::new(AnalysisCanvasLayer::default());
                    self.foundation_structural_ui.screen_key = None;
                    self.foundation_structural_show_attempt(cx, Some("NOT_IMPLEMENTED"), &error);
                }
            }
        }
        self.foundation_structural_set_canvas_frame(
            cx,
            projection,
            self.foundation_structural_ui.screen.as_ref().clone(),
        );
    }

    fn foundation_structural_set_canvas_frame(
        &mut self,
        cx: &mut Cx,
        projection: &fs_api::FsProjection,
        layer: AnalysisCanvasLayer,
    ) {
        let frame = CanvasFrame {
            snapshot_token: format!(
                "foundation-structural:{}:{}",
                projection.input_hash,
                if projection.current {
                    projection.binding.revision
                } else {
                    0
                },
            ),
            source_generation: projection.binding.open_generation,
            model_generation: self.foundation_structural_ui.render_generation,
            document_revision: projection.binding.revision,
            camera_revision: self.foundation_structural_ui.camera.revision,
            camera_change: self.foundation_structural_ui.camera_change.take(),
            analysis: Arc::new(layer),
            ..CanvasFrame::default()
        };
        if let Some(mut canvas) = self.ui.drafting_canvas(cx, ids!(fs_canvas)).borrow_mut() {
            canvas.set_frame(cx, frame);
        }
    }

    fn foundation_structural_render_scene(
        &mut self,
        cx: &mut Cx,
        projection: &fs_api::FsProjection,
        fit: bool,
    ) {
        if !matches!(
            self.foundation_structural_ui.view,
            Stage1View::Frame3D | Stage1View::Ground3D
        ) {
            return;
        }
        let suppressed = self.foundation_structural_suppressed_for(projection);
        let mat = self
            .foundation_structural_display_mat(projection)
            .map(|mut mat| {
                if suppressed {
                    mat.pressure_cells.clear();
                }
                mat
            });
        let data = RenderData {
            model: &projection.model,
            structural_result: if suppressed {
                None
            } else {
                projection.structural_result.as_ref()
            },
            mat: mat.as_ref(),
            deformation_scale: self.foundation_structural_ui.deformation_scale,
        };
        let view = self.foundation_structural_ui.view;
        let key = format!(
            "{}:{}:{:?}:{}:{}",
            projection.input_hash,
            if projection.current {
                projection.binding.revision
            } else {
                0
            },
            view,
            self.foundation_structural_ui.deformation_scale.to_bits(),
            suppressed,
        );
        if self.foundation_structural_ui.render_key.as_deref() == Some(key.as_str()) && !fit {
            return;
        }
        match fs_render::scene(&data, view) {
            Ok((scene, picks)) => {
                self.foundation_structural_ui.scene_picks = picks;
                if self
                    .foundation_structural_ui
                    .selection
                    .is_some_and(|selected| {
                        !self
                            .foundation_structural_ui
                            .scene_picks
                            .values()
                            .any(|pick| *pick == selected)
                    })
                {
                    self.foundation_structural_ui.selection = None;
                }
                let mut viewer = Viewer::default();
                *viewer.scene_mut() = scene;
                viewer.camera_mut().position = Vec3::new(5.0, -25.0, 14.0);
                viewer.camera_mut().up = Vec3::new(0.0, 0.0, 1.0);
                if let Some(mut viewport) = self.ui.cad_viewport(cx, ids!(fs_viewport)).borrow_mut()
                {
                    viewport.set_viewer(cx, viewer);
                    if fit {
                        viewport.fit_scene_when_measured(cx, 1.15);
                    }
                }
                self.foundation_structural_ui.render_key = Some(key);
            }
            Err(error) => {
                self.foundation_structural_ui.scene_picks.clear();
                self.foundation_structural_ui.selection = None;
                self.ui
                    .cad_viewport(cx, ids!(fs_viewport))
                    .set_viewer(cx, Viewer::default());
                self.foundation_structural_show_attempt(cx, Some("NOT_IMPLEMENTED"), &error);
            }
        }
    }

    fn foundation_structural_display_mat(
        &self,
        projection: &fs_api::FsProjection,
    ) -> Option<MatDisplay> {
        projection.mat.clone()
    }

    fn foundation_structural_suppressed(&self) -> bool {
        self.foundation_structural_ui.draft_dirty
            || self.foundation_structural_ui.binding_stale
            || self
                .foundation_structural_ui
                .projection
                .as_ref()
                .is_some_and(|projection| self.foundation_structural_suppressed_for(projection))
    }

    fn foundation_structural_suppressed_for(&self, projection: &fs_api::FsProjection) -> bool {
        self.foundation_structural_ui.draft_dirty
            || self.foundation_structural_ui.binding_stale
            || self.foundation_structural_ui.failed_input_hash.as_deref()
                == Some(projection.input_hash.as_str())
    }

    fn foundation_structural_canvas_action(&mut self, cx: &mut Cx, action: CanvasAction) {
        if !matches!(
            self.foundation_structural_ui.view,
            Stage1View::FoundationPlan | Stage1View::Pressure2D
        ) {
            return;
        }
        let mut changed = false;
        match action {
            CanvasAction::Resize { width, height, .. } => {
                if self
                    .foundation_structural_ui
                    .camera
                    .resize(width, height)
                    .is_ok()
                {
                    self.foundation_structural_ui.camera_change = Some(CanvasCameraChange::Resize);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Scroll { screen, delta_y }) => {
                if delta_y.is_finite()
                    && self
                        .foundation_structural_ui
                        .camera
                        .zoom_at(screen, (-delta_y * 0.0015).clamp(-4.0, 4.0).exp())
                        .is_ok()
                {
                    self.foundation_structural_ui.camera_change = Some(CanvasCameraChange::Zoom);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Pointer {
                phase,
                button,
                screen,
            }) => match phase {
                PointerPhase::Down if button == PointerButton::Middle => {
                    self.foundation_structural_ui.pan_anchor = Some(screen);
                }
                PointerPhase::Down if button == PointerButton::Left => {
                    self.foundation_structural_pick_plan(screen);
                    changed = true;
                }
                PointerPhase::Move => {
                    if let Some(old) = self.foundation_structural_ui.pan_anchor.replace(screen) {
                        if self
                            .foundation_structural_ui
                            .camera
                            .pan_pixels(screen.x - old.x, screen.y - old.y)
                            .is_ok()
                        {
                            self.foundation_structural_ui.camera_change =
                                Some(CanvasCameraChange::Pan);
                            changed = true;
                        }
                    }
                }
                PointerPhase::Up | PointerPhase::Leave => {
                    self.foundation_structural_ui.pan_anchor = None;
                }
                _ => {}
            },
            CanvasAction::Input(CanvasInput::Cancel) => {
                self.foundation_structural_ui.pan_anchor = None;
            }
            _ => {}
        }
        if changed {
            self.foundation_structural_render_changed(cx, false);
        }
    }

    fn foundation_structural_pick_plan(&mut self, screen: DraftPoint) {
        if self.foundation_structural_suppressed() {
            self.foundation_structural_ui.selection = None;
            return;
        }
        let Some(projection) = self.foundation_structural_ui.projection.as_ref() else {
            self.foundation_structural_ui.selection = None;
            return;
        };
        let mat = self.foundation_structural_display_mat(projection);
        let data = RenderData {
            model: &projection.model,
            structural_result: projection.structural_result.as_ref(),
            mat: mat.as_ref(),
            deformation_scale: self.foundation_structural_ui.deformation_scale,
        };
        self.foundation_structural_ui.selection = fs_render::pick_plan(
            &data,
            self.foundation_structural_ui.view,
            &self.foundation_structural_ui.camera,
            screen,
        );
    }

    fn foundation_structural_mark_draft(&mut self, cx: &mut Cx) {
        self.foundation_structural_ui.draft_dirty = true;
        self.foundation_structural_ui.screen_key = None;
        self.foundation_structural_ui.selection = None;
        self.foundation_structural_ui.pan_anchor = None;
        self.foundation_structural_clear_views(cx);
        self.ui
            .label(cx, ids!(fs_attempt_status))
            .set_text(cx, "Unapplied draft · previous pressure/deformation hidden");
        self.ui.button(cx, ids!(fs_analyze)).set_enabled(cx, false);
    }

    fn foundation_structural_fill_editor(&mut self, cx: &mut Cx, values: fs_api::FsEditValues) {
        for (id, value) in [
            (id!(fs_material_e), values.material_e_kn_m2),
            (id!(fs_material_g), values.material_g_kn_m2),
            (id!(fs_area), values.area_m2),
            (id!(fs_iy), values.iy_m4),
            (id!(fs_iz), values.iz_m4),
            (id!(fs_torsion), values.torsion_m4),
            (id!(fs_soil_e), values.soil_e_kpa),
            (id!(fs_soil_poisson), values.soil_poisson),
            (id!(fs_bedding_k), values.bedding_k_kn_m3),
            (id!(fs_mat_thickness), values.mat_thickness_m),
        ] {
            self.ui
                .text_input(cx, &[id])
                .set_text(cx, &value.to_string());
        }
        self.ui
            .drop_down(cx, ids!(fs_load_source))
            .set_selected_item(cx, usize::from(values.manual_synthetic));
        let edit_enabled = !self.foundation_structural_ui.binding_stale;
        self.ui
            .widget(cx, ids!(fs_load_source_host))
            .set_visible(cx, edit_enabled);
        self.foundation_structural_ui.editor_values = Some(values);
        for id in [
            id!(fs_material_e),
            id!(fs_material_g),
            id!(fs_area),
            id!(fs_iy),
            id!(fs_iz),
            id!(fs_torsion),
            id!(fs_soil_e),
            id!(fs_soil_poisson),
            id!(fs_bedding_k),
            id!(fs_mat_thickness),
        ] {
            self.ui
                .text_input(cx, &[id])
                .set_is_read_only(cx, !edit_enabled);
        }
        self.ui
            .button(cx, ids!(fs_apply))
            .set_enabled(cx, edit_enabled);
    }

    fn foundation_structural_clear_editor(&mut self, cx: &mut Cx) {
        self.foundation_structural_ui.editor_values = None;
        for id in [
            id!(fs_material_e),
            id!(fs_material_g),
            id!(fs_area),
            id!(fs_iy),
            id!(fs_iz),
            id!(fs_torsion),
            id!(fs_soil_e),
            id!(fs_soil_poisson),
            id!(fs_bedding_k),
            id!(fs_mat_thickness),
        ] {
            self.ui.text_input(cx, &[id]).set_text(cx, "");
            self.ui.text_input(cx, &[id]).set_is_read_only(cx, true);
        }
        self.ui
            .drop_down(cx, ids!(fs_load_source))
            .set_selected_item(cx, 0);
        self.ui
            .widget(cx, ids!(fs_load_source_host))
            .set_visible(cx, false);
        self.ui.button(cx, ids!(fs_apply)).set_enabled(cx, false);
    }

    fn foundation_structural_read_editor(&self, cx: &Cx) -> Result<fs_api::FsEditValues, String> {
        let mut values = self.foundation_structural_ui.editor_values.ok_or_else(|| {
            "No editable foundation/material/section input is available".to_owned()
        })?;
        let parse = |id, label| parse_stage_number(&self.ui.text_input(cx, &[id]).text(), label);
        values.material_e_kn_m2 = parse(id!(fs_material_e), "Material E, kN/m²")?;
        values.material_g_kn_m2 = parse(id!(fs_material_g), "Material G, kN/m²")?;
        values.area_m2 = parse(id!(fs_area), "Section area, m²")?;
        values.iy_m4 = parse(id!(fs_iy), "Section Iy, m⁴")?;
        values.iz_m4 = parse(id!(fs_iz), "Section Iz, m⁴")?;
        values.torsion_m4 = parse(id!(fs_torsion), "Section J, m⁴")?;
        values.soil_e_kpa = parse(id!(fs_soil_e), "Soil layer E metadata, kPa")?;
        values.soil_poisson = parse(id!(fs_soil_poisson), "Soil Poisson ratio")?;
        values.bedding_k_kn_m3 = parse(id!(fs_bedding_k), "Uniform bedding k, kN/m³")?;
        values.mat_thickness_m = parse(id!(fs_mat_thickness), "Mat thickness, m")?;
        values.manual_synthetic = self.ui.drop_down(cx, ids!(fs_load_source)).selected_item() == 1;
        Ok(values)
    }

    fn foundation_structural_create_demo(&mut self, cx: &mut Cx) {
        let prepared = fs_api::demo_input().map(fs_api::replace_input_command);
        match prepared {
            Ok(command) => {
                self.foundation_structural_commit(cx, "foundation-structural-demo", command)
            }
            Err(error) => {
                self.foundation_structural_show_attempt(cx, Some("NOT_IMPLEMENTED"), &error)
            }
        }
    }

    fn foundation_structural_reload(&mut self, cx: &mut Cx) {
        self.foundation_structural_ui.draft_dirty = false;
        self.foundation_structural_ui.binding_stale = false;
        self.foundation_structural_ui.selection = None;
        self.foundation_structural_ui.pan_anchor = None;
        self.foundation_structural_refresh(cx, false);
    }

    fn foundation_structural_apply(&mut self, cx: &mut Cx) {
        let binding = match self.foundation_structural_ui.binding.clone() {
            Some(binding) => binding,
            None => {
                self.foundation_structural_show_attempt(
                    cx,
                    Some("INVALID_INPUT"),
                    "Reload or create a typed input before applying.",
                );
                return;
            }
        };
        let prepared = (|| -> Result<Command, String> {
            let document = self
                .document
                .as_ref()
                .ok_or_else(|| "No Document is open".to_owned())?;
            fs_api::require_current(document, self.api_document_generation, &binding)?;
            let current = fs_api::current_input(document)
                .ok_or_else(|| "No stored foundation/structural input is available".to_owned())?;
            let values = self.foundation_structural_read_editor(cx)?;
            let edited = fs_api::edit_input(&current.definition, &values)?;
            Ok(fs_api::replace_input_command(edited))
        })();
        match prepared {
            Ok(command) => {
                self.foundation_structural_ui.draft_dirty = false;
                self.foundation_structural_ui.binding_stale = false;
                self.foundation_structural_ui.failed_input_hash = None;
                self.foundation_structural_commit(cx, "foundation-structural-input", command);
            }
            Err(error) => {
                self.foundation_structural_ui.draft_dirty = true;
                self.foundation_structural_ui.failed_input_hash = self
                    .foundation_structural_ui
                    .projection
                    .as_ref()
                    .map(|projection| projection.input_hash.clone());
                self.foundation_structural_show_attempt(cx, Some("INVALID_INPUT"), &error);
                self.foundation_structural_clear_views(cx);
            }
        }
    }

    fn foundation_structural_analyze(&mut self, cx: &mut Cx) {
        if self.foundation_structural_ui.draft_dirty {
            self.foundation_structural_show_attempt(
                cx,
                Some("INVALID_INPUT"),
                "Apply the edited input before Analyze.",
            );
            return;
        }
        let binding = match self.foundation_structural_ui.binding.clone() {
            Some(binding) => binding,
            None => {
                self.foundation_structural_show_attempt(
                    cx,
                    Some("NOT_IMPLEMENTED"),
                    "No typed Document input is available for Analyze.",
                );
                return;
            }
        };
        let prepared = (|| -> Result<Command, String> {
            let document = self
                .document
                .as_ref()
                .ok_or_else(|| "No Document is open".to_owned())?;
            fs_api::require_current(document, self.api_document_generation, &binding)?;
            let input = fs_api::current_input(document)
                .ok_or_else(|| "No current typed input is available".to_owned())?;
            let producer = fs_api::producer_from_environment()?;
            let snapshot = fs_api::analyze_snapshot(input, producer)?;
            Ok(fs_api::save_snapshot_command(snapshot))
        })();
        match prepared {
            Ok(command) => {
                self.foundation_structural_ui.failed_input_hash = None;
                self.foundation_structural_ui.attempt_status = None;
                self.foundation_structural_ui.attempt_message.clear();
                self.foundation_structural_commit(cx, "foundation-structural-analysis", command);
            }
            Err(error) => {
                let status = if error.contains("AI_REVIT_FS_")
                    || error.contains("compiled source binding")
                    || error.contains("running executable SHA-256")
                    || error.contains("source manifest SHA-256")
                {
                    "NOT_IMPLEMENTED"
                } else {
                    classify_analysis_error(&error)
                };
                self.foundation_structural_ui.failed_input_hash = self
                    .foundation_structural_ui
                    .projection
                    .as_ref()
                    .map(|projection| projection.input_hash.clone());
                self.foundation_structural_show_attempt(cx, Some(status), &error);
                self.foundation_structural_clear_views(cx);
            }
        }
    }

    fn foundation_structural_commit(&mut self, cx: &mut Cx, label: &str, command: Command) {
        let Some(document) = self.document.as_ref() else {
            self.foundation_structural_show_attempt(
                cx,
                Some("INVALID_INPUT"),
                "No Document is open.",
            );
            return;
        };
        let expected_revision = document.revision();
        let command_id = self.next_command_id(label);
        match self.apply_typed_commands(cx, command_id, expected_revision, vec![command]) {
            Ok(_) => {
                self.foundation_structural_ui.draft_dirty = false;
                self.foundation_structural_ui.binding_stale = false;
                self.foundation_structural_ui.selection = None;
                self.foundation_structural_ui.pan_anchor = None;
                self.foundation_structural_refresh(cx, true);
            }
            Err(error) => {
                if label == "foundation-structural-input" {
                    // The transaction rejected the proposed edit. Keep the
                    // visible text marked as unapplied so Analyze cannot run
                    // against the previously stored definition by surprise.
                    self.foundation_structural_ui.draft_dirty = true;
                }
                self.foundation_structural_ui.failed_input_hash = self
                    .foundation_structural_ui
                    .projection
                    .as_ref()
                    .map(|projection| projection.input_hash.clone());
                self.foundation_structural_show_attempt(cx, Some("INVALID_INPUT"), &error.message);
                self.foundation_structural_clear_views(cx);
            }
        }
    }

    fn foundation_structural_update_labels(&mut self, cx: &mut Cx) {
        let Some(projection) = self.foundation_structural_ui.projection.as_ref() else {
            return;
        };
        self.ui
            .label(cx, ids!(fs_status))
            .set_text(cx, &projection.status);
        let binding = &projection.binding;
        self.ui.label(cx, ids!(fs_binding_label)).set_text(
            cx,
            &format!(
                "Document {} · runtime {} · generation {} · revision {}",
                binding.document_uuid,
                binding.runtime_id,
                binding.open_generation,
                binding.revision,
            ),
        );
        self.ui
            .label(cx, ids!(fs_summary))
            .set_text(cx, &projection.summary);
        let mut report = projection.report.clone();
        if self.foundation_structural_suppressed_for(projection) {
            report.push_str("\n\nDisplay gate: pressure/deformation are hidden after a failed or unapplied attempt. Any retained snapshot is report-only until the current input is analyzed successfully.");
        }
        if let Some(status) = self.foundation_structural_ui.attempt_status.as_deref() {
            report.push_str(&format!(
                "\n\nSeparate Analyze attempt status: {status}. {}",
                self.foundation_structural_ui.attempt_message
            ));
        }
        if self.foundation_structural_ui.binding_stale {
            report.push_str("\n\nSTALE_BINDING: pending text is retained; reload the current Document before applying.");
        }
        self.ui
            .text_input(cx, ids!(fs_report_text))
            .set_text(cx, &report);
        let attempt = self
            .foundation_structural_ui
            .attempt_status
            .as_deref()
            .map(|status| {
                format!(
                    "Attempt: {status} · {}",
                    self.foundation_structural_ui.attempt_message
                )
            })
            .unwrap_or_else(|| {
                if self.foundation_structural_ui.binding_stale {
                    "STALE_BINDING · reload before applying".into()
                } else if self.foundation_structural_ui.draft_dirty {
                    "Unapplied draft · previous pressure/deformation hidden".into()
                } else {
                    String::new()
                }
            });
        self.ui
            .label(cx, ids!(fs_attempt_status))
            .set_text(cx, &attempt);
        self.ui.button(cx, ids!(fs_apply)).set_enabled(
            cx,
            projection.editor.is_some() && !self.foundation_structural_ui.binding_stale,
        );
        self.ui.button(cx, ids!(fs_analyze)).set_enabled(
            cx,
            !self.foundation_structural_ui.draft_dirty
                && !self.foundation_structural_ui.binding_stale
                && self.document.is_some(),
        );
        self.ui
            .slider(cx, ids!(fs_deformation_scale))
            .set_value(cx, self.foundation_structural_ui.deformation_scale);
    }

    fn foundation_structural_update_inspector(&mut self, cx: &mut Cx) {
        let text = match self.foundation_structural_ui.projection.as_ref() {
            Some(projection) => foundation_structural_inspector(
                projection,
                self.foundation_structural_ui.selection,
                !self.foundation_structural_suppressed_for(projection),
            ),
            None => "No current typed Document input.".into(),
        };
        self.ui
            .text_input(cx, ids!(fs_inspector_text))
            .set_text(cx, &text);
    }

    fn foundation_structural_show_attempt(
        &mut self,
        cx: &mut Cx,
        status: Option<&str>,
        message: &str,
    ) {
        self.foundation_structural_ui.attempt_status = status.map(str::to_owned);
        self.foundation_structural_ui.attempt_message = message.to_owned();
        let text = status
            .map(|code| format!("Attempt: {code} · {message}"))
            .unwrap_or_else(|| message.to_owned());
        self.ui
            .label(cx, ids!(fs_attempt_status))
            .set_text(cx, &text);
        if let Some(projection) = self.foundation_structural_ui.projection.as_ref() {
            let mut report = projection.report.clone();
            report.push_str(&format!("\n\nSeparate operation attempt: {}", text));
            self.ui
                .text_input(cx, ids!(fs_report_text))
                .set_text(cx, &report);
        }
    }

    fn foundation_structural_clear_views(&mut self, cx: &mut Cx) {
        self.foundation_structural_ui.scene_picks.clear();
        self.foundation_structural_ui.screen_key = None;
        self.foundation_structural_ui.screen = Arc::new(AnalysisCanvasLayer::default());
        if let Some(mut canvas) = self.ui.drafting_canvas(cx, ids!(fs_canvas)).borrow_mut() {
            canvas.set_frame(cx, CanvasFrame::default());
        }
        self.ui
            .cad_viewport(cx, ids!(fs_viewport))
            .set_viewer(cx, Viewer::default());
    }
}

fn parse_stage_number(text: &str, label: &str) -> Result<f64, String> {
    let value = text
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("{label}: enter a number in the shown units"))?;
    if !value.is_finite() {
        return Err(format!("{label}: finite value required"));
    }
    Ok(value)
}

fn classify_analysis_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if [
        "unsupported",
        "not implemented",
        "unsupportedgroundwater",
        "not available in stage",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "NOT_IMPLEMENTED"
    } else {
        "NUMERICAL_FAILURE"
    }
}

fn foundation_structural_render_key(
    projection: &fs_api::FsProjection,
    view: Stage1View,
    deformation_scale: f64,
    selection: Option<Stage1Pick>,
    draft_dirty: bool,
    binding_stale: bool,
    suppressed: bool,
) -> String {
    let result_generation = if projection.current {
        projection.binding.revision
    } else {
        0
    };
    format!(
        "{}:{}:{}:{:?}:{}:{:?}:{}:{}:{}",
        projection.input_hash,
        result_generation,
        projection.current,
        view,
        deformation_scale.to_bits(),
        selection,
        draft_dirty,
        binding_stale,
        suppressed,
    )
}

fn stage1_view_name(view: Stage1View) -> &'static str {
    match view {
        Stage1View::FoundationPlan => "foundation_plan",
        Stage1View::Frame3D => "frame_3d",
        Stage1View::Pressure2D => "pressure_2d",
        Stage1View::Ground3D => "ground_3d",
    }
}

fn foundation_structural_inspector(
    projection: &fs_api::FsProjection,
    selection: Option<Stage1Pick>,
    result_visible: bool,
) -> String {
    let result = if result_visible {
        projection.structural_result.as_ref()
    } else {
        None
    };
    match selection {
        Some(Stage1Pick::Node(id)) => {
            let Some(node) = projection
                .model
                .nodes
                .iter()
                .find(|node| node.id.get() == id)
            else {
                return format!("Selected structural node {id} is not present in this input.");
            };
            let mut lines = vec![
                format!("Structural node {}", node.id.get()),
                format!(
                    "Global XYZ: {:.4}, {:.4}, {:.4} m",
                    node.position_m[0], node.position_m[1], node.position_m[2]
                ),
            ];
            if let Some(result) = result {
                if let Some(value) = result.nodes.iter().find(|value| value.node == node.id) {
                    lines.push(format!(
                        "Displacement [ux,uy,uz,rx,ry,rz]: {:?}",
                        value.displacement
                    ));
                }
                if let Some(value) = result.reactions.iter().find(|value| value.node == node.id) {
                    lines.push(format!(
                        "Support reaction [Fx,Fy,Fz,Mx,My,Mz]: {:?}",
                        value.force_moment
                    ));
                }
            } else {
                lines.push("No current displacement/reaction result is displayed.".into());
            }
            lines.join("\n")
        }
        Some(Stage1Pick::Member(id)) => {
            let Some(member) = projection
                .model
                .members
                .iter()
                .find(|member| member.id.get() == id)
            else {
                return format!("Selected structural member {id} is not present in this input.");
            };
            let material = projection
                .model
                .materials
                .iter()
                .find(|material| material.id == member.material);
            let section = projection
                .model
                .sections
                .iter()
                .find(|section| section.id == member.section);
            let mut lines = vec![
                format!(
                    "Structural member {} · {:?}",
                    member.id.get(),
                    member.formulation
                ),
                format!(
                    "Nodes {} → {} · material {} · section {}",
                    member.start.get(),
                    member.end.get(),
                    member.material.get(),
                    member.section.get()
                ),
            ];
            if let Some(material) = material {
                lines.push(format!(
                    "{} · E {:.3e} kN/m² · G {:.3e} kN/m² · weight density {:.3} kN/m³",
                    material.name,
                    material.elastic_modulus_kn_m2,
                    material.shear_modulus_kn_m2,
                    material.weight_density_kn_m3
                ));
            }
            if let Some(section) = section {
                lines.push(format!(
                    "{} · A {:.6} m² · Iy {:.6} m⁴ · Iz {:.6} m⁴ · J {:.6} m⁴",
                    section.name,
                    section.area_m2,
                    section.iy_m4,
                    section.iz_m4,
                    section.torsional_constant_m4
                ));
            }
            if let Some(value) = result.and_then(|result| {
                result
                    .members
                    .iter()
                    .find(|value| value.member == member.id)
            }) {
                lines.push(format!(
                    "Length {:.4} m · local end forces {:?}",
                    value.length_m, value.local_end_forces
                ));
            } else {
                lines.push("No current member result is displayed.".into());
            }
            lines.join("\n")
        }
        Some(Stage1Pick::Foundation) => {
            let Some(mat) = projection.mat.as_ref() else {
                return "Foundation input is absent.".into();
            };
            format!(
                "Rigid rectangular raft · {:?}\nGlobal base reference: ({:.3}, {:.3}, {:.3}) m\nFootprint {:.3} × {:.3} m · thickness {:.3} m\nSoil display layers {} · current pressure samples {}\n{}",
                mat.source,
                mat.reference_point_m[0],
                mat.reference_point_m[1],
                mat.reference_point_m[2],
                mat.length_x_m,
                mat.width_y_m,
                mat.thickness_m,
                mat.soil_layers.len(),
                mat.pressure_cells.len(),
                if mat.pressure_cells.is_empty() {
                    "No current pressure field."
                } else {
                    "Pressure values are mesh-sampled."
                },
            )
        }
        Some(Stage1Pick::GroundLayer(index)) => {
            let Some(layer) = projection
                .mat
                .as_ref()
                .and_then(|mat| mat.soil_layers.get(index))
            else {
                return format!("Ground layer {index} is not present in this input.");
            };
            let editor = projection.editor;
            let first_layer = if index == 0 {
                editor.map(|values| format!("\nSynthetic E metadata {:.3} kPa · ν {:.4}; independent bedding k {:.3} kN/m³", values.soil_e_kpa, values.soil_poisson, values.bedding_k_kn_m3)).unwrap_or_default()
            } else {
                String::new()
            };
            format!(
                "Ground layer {} · domain top depth {:.3} m · thickness {:.3} m\nSynthetic/display geometry only; site verification remains domain-owned.{first_layer}",
                index + 1,
                layer.depth_top_m,
                layer.thickness_m
            )
        }
        None => format!(
            "No typed element selected. Click a structural node/member in 3D or the raft footprint in plan.\nPrimary model status: {}\n{}",
            projection.status, projection.summary,
        ),
    }
}
