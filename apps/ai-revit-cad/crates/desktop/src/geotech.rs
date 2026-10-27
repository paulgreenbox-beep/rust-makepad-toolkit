//! Engineering workspace. The shared Document and command dispatcher own persistence.
use super::*;
use crate::drafting_canvas::{
    AnalysisCanvasLayer, CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput, PointerButton,
    PointerPhase,
};
use crate::geotech_render::GeotechRender;
use ai_revit_core::engineering::AnalysisCaseId;
use drafting_core::{Camera2D, Point2 as DraftPoint};
use geotech_core::{
    AnalysisConditions, CandidateSearchResult, CandidateSearchSpec, InputUnits, Section2D,
    SlopeAnalysisInput, SlopeAnalysisResult, SoilMaterial,
};
use std::sync::Arc;
use viewer_core::Viewer;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.GeotechParameters = SolidView{
        width: 330 height: Fill flow: Down padding: 12 spacing: 7
        draw_bg.color: #x17212b
        Label{width: Fill text: "Engineering / Geotechnics" draw_text.color: #x90a8be}
        H3{width: Fill text: "Slope Stability" draw_text.color: #xe5edf6}
        geotech_case_selector := DropDown{width: Fill labels: ["New analysis case"]}
        View{width: Fill height: 30 flow: Right spacing: 6
            geotech_new := Button{width: Fill text: "Новая"}
            geotech_reload := Button{width: Fill text: "Загрузить"}
        }
        ScrollYView{width: Fill height: Fill flow: Down spacing: 7
            geotech_demo := Button{width: Fill text: "Демонстрационный пример"}
            Label{width: Fill text: "Geometry · x horizontal, z up · m" draw_text.color: #xb5c9dc}
            geotech_geometry_mode := DropDown{width: Fill labels: ["Simple slope · H, β, α, B", "Section2D JSON · m"]}
            geotech_simple_fields := View{width: Fill height: Fit flow: Down spacing: 5
                View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                    Label{width: 135 text: "Height H, m" draw_text.color: #xaebdcb}
                    geotech_height := TextInput{width: Fill height: 30 empty_text: "Required"}
                }
                View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                    Label{width: 135 text: "Slope β, degrees" draw_text.color: #xaebdcb}
                    geotech_beta := TextInput{width: Fill height: 30 empty_text: "0 < α < β < 90"}
                }
                View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                    Label{width: 135 text: "Slip α, degrees" draw_text.color: #xaebdcb}
                    geotech_alpha := TextInput{width: Fill height: 30 empty_text: "Required"}
                }
                View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                    Label{width: 135 text: "Width B, m" draw_text.color: #xaebdcb}
                    geotech_width := TextInput{width: Fill height: 30 empty_text: "Required"}
                }
            }
            geotech_advanced_fields := View{width: Fill height: Fit flow: Down spacing: 5 visible: false
                Label{width: Fill text: "Ordered terrain, toe, crest, planar slip, width" draw_text.color: #xaebdcb}
                geotech_section_json := TextInput{width: Fill height: 180 is_multiline: true empty_text: "Section2D JSON; coordinates in m"}
            }
            Hr{}
            Label{width: Fill text: "Homogeneous soil · effective parameters" draw_text.color: #xb5c9dc}
            geotech_soil_name := TextInput{width: Fill height: 30 empty_text: "Soil name · required"}
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Unit weight γ, kN/m³" draw_text.color: #xaebdcb}
                geotech_gamma := TextInput{width: Fill height: 30 empty_text: "Required"}
            }
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Cohesion c′, kPa" draw_text.color: #xaebdcb}
                geotech_cohesion := TextInput{width: Fill height: 30 empty_text: "Required, ≥ 0"}
            }
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Friction φ′, degrees" draw_text.color: #xaebdcb}
                geotech_phi := TextInput{width: Fill height: 30 empty_text: "Required"}
            }
            geotech_provenance := TextInput{width: Fill height: 56 is_multiline: true empty_text: "Parameter source / provenance · required"}
            Label{width: Fill text: "Dry · static · self-weight only. Water, loads, anchors and seismic actions unsupported." draw_text.color: #x9badbe}
            Hr{}
            Label{width: Fill text: "Optional project requirement" draw_text.color: #xb5c9dc}
            geotech_standard := TextInput{width: Fill height: 30 empty_text: "Standard / project context (optional)"}
            geotech_required_fs := TextInput{width: Fill height: 30 empty_text: "Required Fs (optional; no automatic code factor)"}
            Hr{}
            Label{width: Fill text: "Candidate search · explicit angular family" draw_text.color: #xb5c9dc}
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Start α, degrees" draw_text.color: #xaebdcb}
                geotech_search_start := TextInput{width: Fill height: 30 empty_text: "Required for search"}
            }
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "End α, degrees" draw_text.color: #xaebdcb}
                geotech_search_end := TextInput{width: Fill height: 30 empty_text: "Required for search"}
            }
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Coarse step, degrees" draw_text.color: #xaebdcb}
                geotech_search_step := TextInput{width: Fill height: 30 empty_text: "Required for search"}
            }
            View{width: Fill height: 30 flow: Right align: Align{y:0.5}
                Label{width: 135 text: "Refinement rounds" draw_text.color: #xaebdcb}
                geotech_search_refinement := TextInput{width: Fill height: 30 text: "0"}
            }
        }
    }
    mod.widgets.GeotechResults = SolidView{
        width: 350 height: Fill flow: Down padding: 12 spacing: 7
        draw_bg.color: #x17212b
        H3{width: Fill text: "Calculation results" draw_text.color: #xe5edf6}
        ScrollYView{width: Fill height: Fill flow: Down spacing: 8
            geotech_status := Label{width: Fill text: "Define geometry and soil, then Calculate." draw_text.color: #xf2c47c}
            geotech_results_text := TextInput{width: Fill height: 280 is_multiline: true is_read_only: true text: "No calculated result."}
            Label{width: Fill text: "Fs describes this dry planar surface only. Engineering review required." draw_text.color: #xe4b77a}
            Label{width: Fill text: "3D: extruded_2d_section. Side friction and spatial slip surfaces are excluded." draw_text.color: #x9badbe}
            Label{width: Fill text: "3D legend: amber mass; red slip base; green terrain surface (open context)." draw_text.color: #xb9c9b0}
            geotech_search_text := TextInput{width: Fill height: 105 is_multiline: true is_read_only: true text: "Candidate search has not run."}
            geotech_report_text := TextInput{width: Fill height: 340 is_multiline: true is_read_only: true text: "Report appears here after calculation."}
            geotech_report_path := TextInput{width: Fill height: 30 text: "geotech-slope-report.txt" empty_text: "Report output path"}
            geotech_save_report := Button{width: Fill text: "Сохранить отчёт"}
        }
    }
    mod.widgets.GeotechActions = SolidView{
        width: Fill height: 46 flow: Right spacing: 7 padding: 7 align: Align{y:0.5}
        draw_bg.color: #x1d2b39
        geotech_save := Button{text: "Сохранить постановку"}
        geotech_calculate := Button{text: "Calculate"}
        geotech_search := Button{text: "Candidate Search"}
        geotech_2d := Button{text: "2D"}
        geotech_3d := Button{text: "3D"}
        geotech_fit := Button{text: "Fit"}
        geotech_zoom_in := Button{width: 30 text: "+" enabled: false}
        geotech_zoom_out := Button{width: 30 text: "−" enabled: false}
        geotech_pan_left := Button{width: 30 text: "←" enabled: false}
        geotech_pan_right := Button{width: 30 text: "→" enabled: false}
        geotech_pan_up := Button{width: 30 text: "↑" enabled: false}
        geotech_pan_down := Button{width: 30 text: "↓" enabled: false}
        geotech_report := Button{text: "Report"}
        geotech_snapshot := Button{text: "Снимок расчёта" enabled: false}
        geotech_case_label := Label{width: Fill text: "New case · inputs are not yet stored" draw_text.color: #xaec0d1}
    }
}

pub(super) struct GeotechWorkspace {
    pub active: bool,
    pub active_case: Option<AnalysisCaseId>,
    pub result_input: Option<SlopeAnalysisInput>,
    pub result: Option<Arc<SlopeAnalysisResult>>,
    displayed_case: Option<AnalysisCaseId>,
    pub search: Option<Arc<CandidateSearchResult>>,
    pub calculation_count: u64,
    pub dirty: bool,
    view_3d: bool,
    geometry_mode: usize,
    camera: Camera2D,
    camera_change: Option<CanvasCameraChange>,
    pan_anchor: Option<DraftPoint>,
    render: Option<GeotechRender>,
    screen: Arc<AnalysisCanvasLayer>,
    screen_key: Option<(u64, u64)>,
    generation: u64,
    document_binding: Option<(u64, u64)>,
    form_binding: Option<(u64, u64)>,
    form_document_id: Option<String>,
    source_fingerprint: String,
    cases: Vec<AnalysisCaseId>,
}
impl Default for GeotechWorkspace {
    fn default() -> Self {
        Self {
            active: false,
            active_case: None,
            result_input: None,
            result: None,
            displayed_case: None,
            search: None,
            calculation_count: 0,
            dirty: false,
            view_3d: false,
            geometry_mode: 0,
            camera: Camera2D::default(),
            camera_change: None,
            pan_anchor: None,
            render: None,
            screen: Arc::new(AnalysisCanvasLayer::default()),
            screen_key: None,
            generation: 0,
            document_binding: None,
            form_binding: None,
            form_document_id: None,
            source_fingerprint: String::new(),
            cases: Vec::new(),
        }
    }
}

pub(super) const GEOTECH_BUTTONS: &[(&str, LiveId)] = &[
    ("geotech.open", id!(workspace_engineering)),
    ("geotech.new", id!(geotech_new)),
    ("geotech.reload", id!(geotech_reload)),
    ("geotech.demo", id!(geotech_demo)),
    ("geotech.save", id!(geotech_save)),
    ("geotech.calculate", id!(geotech_calculate)),
    ("geotech.search", id!(geotech_search)),
    ("geotech.2d", id!(geotech_2d)),
    ("geotech.3d", id!(geotech_3d)),
    ("geotech.fit", id!(geotech_fit)),
    ("geotech.camera.zoom_in", id!(geotech_zoom_in)),
    ("geotech.camera.zoom_out", id!(geotech_zoom_out)),
    ("geotech.camera.pan_left", id!(geotech_pan_left)),
    ("geotech.camera.pan_right", id!(geotech_pan_right)),
    ("geotech.camera.pan_up", id!(geotech_pan_up)),
    ("geotech.camera.pan_down", id!(geotech_pan_down)),
    ("geotech.report", id!(geotech_report)),
    ("geotech.report.save", id!(geotech_save_report)),
    ("geotech.snapshot", id!(geotech_snapshot)),
];

#[derive(Clone, Copy)]
enum GeotechCameraCommand {
    ZoomIn,
    ZoomOut,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
}
pub(super) const GEOTECH_INPUTS: &[(&str, LiveId)] = &[
    ("geotech.height", id!(geotech_height)),
    ("geotech.beta", id!(geotech_beta)),
    ("geotech.alpha", id!(geotech_alpha)),
    ("geotech.width", id!(geotech_width)),
    ("geotech.section", id!(geotech_section_json)),
    ("geotech.soil.name", id!(geotech_soil_name)),
    ("geotech.soil.gamma", id!(geotech_gamma)),
    ("geotech.soil.cohesion", id!(geotech_cohesion)),
    ("geotech.soil.phi", id!(geotech_phi)),
    ("geotech.soil.provenance", id!(geotech_provenance)),
    ("geotech.standard", id!(geotech_standard)),
    ("geotech.required_fs", id!(geotech_required_fs)),
    ("geotech.search.start", id!(geotech_search_start)),
    ("geotech.search.end", id!(geotech_search_end)),
    ("geotech.search.step", id!(geotech_search_step)),
    ("geotech.search.refinement", id!(geotech_search_refinement)),
    ("geotech.report.path", id!(geotech_report_path)),
];

impl App {
    fn geotech_binding(&self) -> (u64, u64) {
        (
            self.api_document_generation,
            self.document.as_ref().map_or(0, Document::revision),
        )
    }

    fn geotech_capture_form_context(&mut self) {
        self.geotech_ui.form_binding = Some(self.geotech_binding());
        self.geotech_ui.form_document_id =
            self.document.as_ref().map(|d| d.document_uuid.to_string());
    }

    fn geotech_form_current(&self) -> bool {
        self.geotech_ui.form_binding == Some(self.geotech_binding())
            && self.geotech_ui.form_document_id
                == self.document.as_ref().map(|d| d.document_uuid.to_string())
    }

    pub(super) fn geotech_accept_result(
        &mut self,
        cx: &mut Cx,
        id: AnalysisCaseId,
        input: SlopeAnalysisInput,
        result: SlopeAnalysisResult,
        search: Option<CandidateSearchResult>,
    ) {
        // A pure external calculation must not rebind or discard a pending editor.
        if self.geotech_ui.dirty {
            return;
        }
        let report_selection_changed = self.geotech_ui.displayed_case != Some(id)
            || self
                .geotech_ui
                .result
                .as_ref()
                .map(|previous| previous.input_fingerprint.as_str())
                != Some(result.input_fingerprint.as_str())
            || self
                .geotech_ui
                .search
                .as_ref()
                .map(|previous| &previous.spec)
                != search.as_ref().map(|selected| &selected.spec);
        if self.geotech_ui.active_case != Some(id) {
            if let Some(source) = self
                .document
                .as_ref()
                .and_then(|d| d.slope_case(id))
                .map(|c| c.input.clone())
            {
                self.geotech_fill_form(cx, &source);
            }
        }
        self.geotech_ui.source_fingerprint = search.as_ref().map_or_else(
            || result.input_fingerprint.clone(),
            |s| s.source_input_fingerprint.clone(),
        );
        self.geotech_ui.render = GeotechRender::new(&input, &result)
            .map_err(|e| self.set_status(cx, &e))
            .ok();
        self.geotech_ui.active_case = Some(id);
        self.geotech_refresh_case_selector(cx);
        self.geotech_ui.result_input = Some(input);
        self.geotech_ui.result = Some(Arc::new(result));
        self.geotech_ui.displayed_case = Some(id);
        self.geotech_ui.search = search.map(Arc::new);
        if report_selection_changed {
            self.ui.text_input(cx, ids!(geotech_report_text)).set_text(
                cx,
                "Report not generated for the current selected result. Click Report.",
            );
        }
        self.geotech_ui.calculation_count = self.geotech_ui.calculation_count.saturating_add(1);
        self.geotech_ui.generation = self.geotech_ui.generation.saturating_add(1);
        self.geotech_ui.screen_key = None;
        self.geotech_ui.dirty = false;
        self.geotech_capture_form_context();
        self.geotech_update_result_text(cx);
        self.geotech_refresh_actions(cx);
        self.geotech_refresh_scene(cx, true);
        self.geotech_fit(cx);
        self.refresh_geotech_canvas(cx);
    }

    pub(super) fn geotech_invalidate(&mut self, cx: &mut Cx) {
        self.geotech_ui.result = None;
        self.geotech_ui.displayed_case = None;
        self.geotech_ui.result_input = None;
        self.geotech_ui.search = None;
        self.geotech_ui.render = None;
        self.geotech_ui.screen_key = None;
        self.geotech_ui.generation = self.geotech_ui.generation.saturating_add(1);
        self.ui
            .text_input(cx, ids!(geotech_results_text))
            .set_text(cx, "No current calculation. Calculate the stored input.");
        self.ui
            .text_input(cx, ids!(geotech_report_text))
            .set_text(cx, "Report invalidated by changed source input.");
        self.ui
            .text_input(cx, ids!(geotech_search_text))
            .set_text(cx, "Candidate search has not run for the current input.");
        self.ui
            .cad_viewport(cx, ids!(geotech_viewport))
            .set_viewer(cx, Viewer::default());
        self.geotech_refresh_actions(cx);
        self.refresh_geotech_canvas(cx);
    }

    fn geotech_result_fresh(&self) -> bool {
        let Some(result) = self.geotech_ui.result.as_ref() else {
            return false;
        };
        let Some(input) = self.geotech_ui.result_input.as_ref() else {
            return false;
        };
        let Some(case) = self
            .geotech_ui
            .active_case
            .and_then(|id| self.document.as_ref()?.slope_case(id))
        else {
            return false;
        };
        !self.geotech_ui.dirty
            && self.geotech_ui.displayed_case == self.geotech_ui.active_case
            && geotech_core::input_fingerprint(&case.input).ok().as_deref()
                == Some(self.geotech_ui.source_fingerprint.as_str())
            && geotech_core::input_fingerprint(input).ok().as_deref()
                == Some(result.input_fingerprint.as_str())
            && result.solver_version == geotech_core::SOLVER_VERSION
            && result.source_fingerprint == geotech_core::source_fingerprint()
    }

    fn geotech_refresh_actions(&self, cx: &mut Cx) {
        let allowed = self.geotech_result_fresh()
            && self.geotech_ui.search.is_none()
            && self.geotech_form_current();
        self.ui
            .button(cx, ids!(geotech_snapshot))
            .set_enabled(cx, allowed);
        let camera_enabled = self.geotech_ui.active
            && !self.geotech_ui.view_3d
            && self.geotech_ui.render.is_some()
            && self.geotech_result_fresh();
        for control in [
            id!(geotech_zoom_in),
            id!(geotech_zoom_out),
            id!(geotech_pan_left),
            id!(geotech_pan_right),
            id!(geotech_pan_up),
            id!(geotech_pan_down),
        ] {
            self.ui
                .button(cx, &[control])
                .set_enabled(cx, camera_enabled);
        }
    }

    /// Project only the current case list/selection. Never rebind a pending form.
    fn geotech_refresh_case_selector(&mut self, cx: &mut Cx) {
        self.geotech_ui.cases = self
            .document
            .as_ref()
            .map(|document| document.slope_cases().keys().copied().collect())
            .unwrap_or_default();
        let labels = std::iter::once("New analysis case".to_string())
            .chain(
                self.geotech_ui
                    .cases
                    .iter()
                    .map(|id| format!("Slope analysis #{}", id.get())),
            )
            .collect::<Vec<_>>();
        self.ui
            .drop_down(cx, ids!(geotech_case_selector))
            .set_labels(cx, labels);
        let selected = self
            .geotech_ui
            .active_case
            .and_then(|id| self.geotech_ui.cases.iter().position(|case| *case == id))
            .map_or(0, |index| index + 1);
        self.ui
            .drop_down(cx, ids!(geotech_case_selector))
            .set_selected_item(cx, selected);
    }

    /// Revision inspection only; never invokes the solver, geometry or search.
    pub(super) fn geotech_sync_case(&mut self, cx: &mut Cx) {
        let binding = self.geotech_binding();
        if self.geotech_ui.document_binding == Some(binding) {
            return;
        }
        let epoch_changed = self
            .geotech_ui
            .document_binding
            .is_some_and(|old| old.0 != binding.0);
        self.geotech_ui.document_binding = Some(binding);
        if epoch_changed {
            self.geotech_invalidate(cx);
            if self.geotech_ui.dirty {
                self.geotech_status(cx,"Document replaced while parameters were being edited. Pending text retained; explicitly reload or create a new form.");
                return;
            }
            self.geotech_ui.active_case = None;
            self.geotech_capture_form_context();
        }
        if self.document.is_none() {
            return;
        }
        self.geotech_refresh_case_selector(cx);
        if let Some(id) = self.geotech_ui.active_case {
            match self.geotech_dispatch(
                cx,
                "geotech.slope.inspect",
                &json!({"analysis_id":id.get()}),
            ) {
                Ok(case) => {
                    let fingerprint = case
                        .get("input_fingerprint")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !self.geotech_ui.source_fingerprint.is_empty()
                        && fingerprint != self.geotech_ui.source_fingerprint
                    {
                        self.geotech_invalidate(cx);
                    }
                    self.geotech_ui.source_fingerprint = fingerprint.to_string();
                    if !self.geotech_ui.dirty {
                        if let Ok(input) =
                            serde_json::from_value::<SlopeAnalysisInput>(case["input"].clone())
                        {
                            self.geotech_fill_form(cx, &input);
                        }
                    }
                }
                Err(_) => {
                    self.geotech_ui.active_case = None;
                    self.geotech_invalidate(cx);
                }
            }
        }
        if self.geotech_ui.dirty && self.geotech_ui.form_binding != Some(binding) {
            self.geotech_status(cx,"Document changed while parameters were being edited. Pending text retained; reload before saving.");
        }
    }

    pub(super) fn geotech_set_active(&mut self, cx: &mut Cx, active: bool) {
        if active && self.circular_ui.panel.active { self.circular_set_active(cx, false); }
        self.geotech_ui.active = active;
        cx.revert_key_focus();
        if active {
            self.geotech_sync_case(cx);
            if self.geotech_ui.form_binding.is_none() {
                self.geotech_capture_form_context();
            }
        }
        self.refresh_workspace_visibility(cx);
        self.refresh_drafting(cx, true);
    }

    pub(super) fn geotech_set_view(&mut self, cx: &mut Cx, three: bool) {
        self.geotech_ui.view_3d = three;
        self.refresh_workspace_visibility(cx);
        self.refresh_geotech_canvas(cx);
    }

    pub(super) fn geotech_input_changed(&mut self, cx: &mut Cx) {
        self.geotech_ui.dirty = true;
        self.geotech_ui.screen_key = None;
        self.geotech_ui.generation = self.geotech_ui.generation.saturating_add(1);
        self.geotech_status(
            cx,
            "Edited input · previous visual result is invalid until saved and calculated.",
        );
        self.ui
            .cad_viewport(cx, ids!(geotech_viewport))
            .set_viewer(cx, Viewer::default());
        self.ui.text_input(cx, ids!(geotech_results_text)).set_text(cx, "Edited input: previous numerical result is stale. Save and calculate the current parameters.");
        self.ui.text_input(cx, ids!(geotech_report_text)).set_text(
            cx,
            "Report unavailable for edited input. Save and calculate first.",
        );
        self.ui.text_input(cx, ids!(geotech_search_text)).set_text(
            cx,
            "Previous candidate result is stale for the edited input.",
        );
        self.refresh_geotech_canvas(cx);
        self.geotech_refresh_actions(cx);
    }

    pub(super) fn geotech_search_input_changed(&mut self, cx: &mut Cx) {
        if self.geotech_ui.search.is_some() {
            self.geotech_invalidate(cx);
            self.geotech_status(
                cx,
                "Search limits edited. Run Candidate Search explicitly for the new family.",
            );
        }
    }

    pub(super) fn geotech_workspace_visibility(&mut self, cx: &mut Cx) {
        self.geotech_refresh_actions(cx);
        let active = self.geotech_ui.active;
        let three = self.geotech_ui.view_3d;
        for (path, visible) in [
            (ids!(geotech_parameters), active),
            (ids!(geotech_results), active),
            (ids!(geotech_actions), active),
            (ids!(geotech_viewport_host), active && three),
            (ids!(history_panel), !active),
            (ids!(inspector_panel), !active),
        ] {
            self.ui.widget(cx, path).set_visible(cx, visible);
        }
        if active {
            for path in [
                ids!(model_toolbar),
                ids!(drafting_toolbar),
                ids!(drafting_command_panel),
                ids!(viewport_3d_host),
            ] {
                self.ui.widget(cx, path).set_visible(cx, false);
            }
            self.ui
                .widget(cx, ids!(drafting_2d_host))
                .set_visible(cx, !three);
            self.ui.label(cx, ids!(viewport_title)).set_text(
                cx,
                if three {
                    "Engineering / Geotechnics / Slope Stability · 3D extrusion"
                } else {
                    "Engineering / Geotechnics / Slope Stability · 2D section"
                },
            );
            self.ui.label(cx, ids!(navigation_hint)).set_text(
                cx,
                if three {
                    "Orbit / pan / zoom · extruded_2d_section"
                } else {
                    "Middle drag — pan · wheel — zoom · Fit"
                },
            );
        }
        self.ui.button(cx, ids!(workspace_engineering)).set_text(
            cx,
            if active {
                "● Engineering"
            } else {
                "Engineering"
            },
        );
    }

    fn geotech_fill_form(&mut self, cx: &mut Cx, input: &SlopeAnalysisInput) {
        let section = &input.section;
        let height = section.crest.z - section.slope_toe.z;
        let beta = height
            .atan2((section.crest.x - section.slope_toe.x).abs())
            .to_degrees();
        let alpha = (section.slip_surface.upper.z - section.slip_surface.lower.z)
            .atan2((section.slip_surface.upper.x - section.slip_surface.lower.x).abs())
            .to_degrees();
        for (id, value) in [
            (id!(geotech_height), height.to_string()),
            (id!(geotech_beta), beta.to_string()),
            (id!(geotech_alpha), alpha.to_string()),
            (id!(geotech_width), section.extrusion_width_m.to_string()),
            (id!(geotech_soil_name), input.soil.name.clone()),
            (
                id!(geotech_gamma),
                input.soil.unit_weight_kn_per_m3.to_string(),
            ),
            (
                id!(geotech_cohesion),
                input.soil.effective_cohesion_kpa.to_string(),
            ),
            (
                id!(geotech_phi),
                input.soil.effective_friction_angle_deg.to_string(),
            ),
            (id!(geotech_provenance), input.soil.provenance.clone()),
            (
                id!(geotech_standard),
                input.standard.clone().unwrap_or_default(),
            ),
            (
                id!(geotech_required_fs),
                input
                    .required_factor_of_safety
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                id!(geotech_section_json),
                serde_json::to_string_pretty(section).unwrap_or_default(),
            ),
        ] {
            self.ui.text_input(cx, &[id]).set_text(cx, &value);
        }
        // Restored arbitrary sections always remain in their original coordinate system.
        self.geotech_ui.geometry_mode = 1;
        self.ui
            .drop_down(cx, ids!(geotech_geometry_mode))
            .set_selected_item(cx, 1);
        self.geotech_geometry_visibility(cx);
        self.geotech_capture_form_context();
    }

    fn geotech_geometry_visibility(&mut self, cx: &mut Cx) {
        let simple = self.geotech_ui.geometry_mode == 0;
        self.ui
            .widget(cx, ids!(geotech_simple_fields))
            .set_visible(cx, simple);
        self.ui
            .widget(cx, ids!(geotech_advanced_fields))
            .set_visible(cx, !simple);
    }

    fn geotech_read_input(&self, cx: &Cx) -> Result<SlopeAnalysisInput, String> {
        let number =
            |id: LiveId, name: &str| parse_number(&self.ui.text_input(cx, &[id]).text(), name);
        let section = if self.geotech_ui.geometry_mode == 0 {
            geotech_core::triangular_section(
                number(id!(geotech_height), "H, m")?,
                number(id!(geotech_beta), "β, degrees")?,
                number(id!(geotech_alpha), "α, degrees")?,
                number(id!(geotech_width), "B, m")?,
            )
            .map_err(|e| format!("{e:?}"))?
        } else {
            serde_json::from_str::<Section2D>(
                &self.ui.text_input(cx, ids!(geotech_section_json)).text(),
            )
            .map_err(|e| format!("Section2D JSON: {e}"))?
        };
        let required = self.ui.text_input(cx, ids!(geotech_required_fs)).text();
        let standard = self.ui.text_input(cx, ids!(geotech_standard)).text();
        Ok(SlopeAnalysisInput {
            section,
            soil: SoilMaterial {
                name: self.ui.text_input(cx, ids!(geotech_soil_name)).text(),
                unit_weight_kn_per_m3: number(id!(geotech_gamma), "γ, kN/m³")?,
                effective_cohesion_kpa: number(id!(geotech_cohesion), "c′, kPa")?,
                effective_friction_angle_deg: number(id!(geotech_phi), "φ′, degrees")?,
                provenance: self.ui.text_input(cx, ids!(geotech_provenance)).text(),
            },
            conditions: AnalysisConditions::default(),
            units: InputUnits::default(),
            required_factor_of_safety: if required.trim().is_empty() {
                None
            } else {
                Some(parse_number(&required, "required Fs")?)
            },
            standard: if standard.trim().is_empty() {
                None
            } else {
                Some(standard)
            },
        })
    }

    fn geotech_store_form(&mut self, cx: &mut Cx) -> Result<AnalysisCaseId, String> {
        let edited = self.geotech_ui.dirty;
        if !self.geotech_form_current() {
            return Err("Source document changed. Reload the calculation case before saving edited parameters.".into());
        }
        let input = self.geotech_read_input(cx)?;
        let (expected_open_generation, expected_revision) = self
            .geotech_ui
            .form_binding
            .ok_or("No captured form context; reload first")?;
        let guards = json!({"expected_document_id":self.geotech_ui.form_document_id,"expected_open_generation":expected_open_generation,"expected_revision":expected_revision});
        let mut params = guards;
        params["input"] = serde_json::to_value(&input).map_err(|e| e.to_string())?;
        let result = if let Some(id) = self.geotech_ui.active_case {
            params["analysis_id"] = json!(id.get());
            let current = self.geotech_dispatch(
                cx,
                "geotech.slope.inspect",
                &json!({"analysis_id":id.get()}),
            )?;
            if current.get("input")
                == Some(&serde_json::to_value(&input).map_err(|e| e.to_string())?)
            {
                current
            } else {
                self.geotech_dispatch(cx, "geotech.slope.update", &params)?
            }
        } else {
            self.geotech_dispatch(cx, "geotech.slope.create", &params)?
        };
        let id = AnalysisCaseId::new(
            result
                .get("analysis_id")
                .and_then(value_id)
                .ok_or("No analysis_id returned")?,
        );
        if edited {
            self.geotech_invalidate(cx);
        }
        self.geotech_ui.active_case = Some(id);
        self.geotech_ui.dirty = false;
        self.geotech_refresh_case_selector(cx);
        self.geotech_capture_form_context();
        self.geotech_ui.source_fingerprint = result
            .get("input_fingerprint")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.ui.label(cx, ids!(geotech_case_label)).set_text(
            cx,
            &format!(
                "Analysis #{} · unified document revision {}",
                id.get(),
                self.geotech_binding().1
            ),
        );
        Ok(id)
    }

    fn geotech_load_selected(&mut self, cx: &mut Cx) -> Result<(), String> {
        let id = self
            .geotech_ui
            .active_case
            .ok_or("Choose an existing analysis case")?;
        let case = self.geotech_dispatch(
            cx,
            "geotech.slope.inspect",
            &json!({"analysis_id":id.get()}),
        )?;
        let input = serde_json::from_value::<SlopeAnalysisInput>(case["input"].clone())
            .map_err(|e| e.to_string())?;
        self.geotech_invalidate(cx);
        self.geotech_ui.dirty = false;
        self.geotech_fill_form(cx, &input);
        self.geotech_ui.source_fingerprint =
            case["input_fingerprint"].as_str().unwrap_or("").into();
        self.geotech_status(
            cx,
            "Stored input loaded. Calculate explicitly to update derived results.",
        );
        Ok(())
    }

    fn geotech_demo(&mut self, cx: &mut Cx) {
        self.geotech_ui.active_case = None;
        self.geotech_refresh_case_selector(cx);
        self.geotech_ui.dirty = true;
        self.geotech_capture_form_context();
        for (id, value) in [
            (id!(geotech_height), "10"),
            (id!(geotech_beta), "45"),
            (id!(geotech_alpha), "30"),
            (id!(geotech_width), "1"),
            (
                id!(geotech_soil_name),
                "Demonstration soil — golden fixture",
            ),
            (id!(geotech_gamma), "18"),
            (id!(geotech_cohesion), "0"),
            (id!(geotech_phi), "30"),
            (
                id!(geotech_provenance),
                "GEO-SLOPE-001 Golden Test 1/2; demonstration, not site investigation",
            ),
            (id!(geotech_standard), ""),
            (id!(geotech_required_fs), ""),
            (id!(geotech_search_start), "10"),
            (id!(geotech_search_end), "44"),
            (id!(geotech_search_step), "2"),
            (id!(geotech_search_refinement), "3"),
        ] {
            self.ui.text_input(cx, &[id]).set_text(cx, value);
        }
        self.geotech_ui.geometry_mode = 0;
        self.ui
            .drop_down(cx, ids!(geotech_geometry_mode))
            .set_selected_item(cx, 0);
        self.geotech_geometry_visibility(cx);
        self.geotech_invalidate(cx);
        self.geotech_status(
            cx,
            "Demonstration only. Calculate stores a new example case in the unified document.",
        );
    }

    fn geotech_status(&mut self, cx: &mut Cx, message: &str) {
        self.ui
            .label(cx, ids!(geotech_status))
            .set_text(cx, message);
        self.set_status(cx, message);
    }

    fn geotech_update_result_text(&mut self, cx: &mut Cx) {
        let Some(result) = self.geotech_ui.result.as_ref() else {
            return;
        };
        let g = &result.geometry;
        let f = &result.forces_per_m;
        let t = &result.total_forces;
        let text = format!(
            "Fs = {:.9}\n\nA = {:.9} m²\nL = {:.9} m\nB = {:.6} m\nV = {:.9} m³\nSlip base = {:.9} m²\n\nPer metre width:\nW = {:.9} kN/m\nN = {:.9} kN/m\nT = {:.9} kN/m\nR = {:.9} kN/m\n\nFull width:\nW total = {:.9} kN\nN total = {:.9} kN\nT total = {:.9} kN\nR total = {:.9} kN\n\nGEOMETRY_VALID = {}\nCALCULATION_COMPLETED = {}\nNUMERICALLY_VERIFIED = {}\nENGINEERING_REVIEW_REQUIRED = {}\n\nMethod: {}\nSolver: {}\nWarnings: {:?}",
            result.factor_of_safety,
            g.area_m2,
            g.slip_length_m,
            g.width_m,
            g.volume_m3,
            g.slip_base_area_m2,
            f.weight_kn_per_m,
            f.normal_kn_per_m,
            f.driving_kn_per_m,
            f.resistance_kn_per_m,
            t.weight_kn,
            t.normal_kn,
            t.driving_kn,
            t.resistance_kn,
            result.status.geometry_valid,
            result.status.calculation_completed,
            result.status.numerically_verified,
            result.status.engineering_review_required,
            result.method,
            result.solver_version,
            result.warnings
        );
        let source=self.geotech_ui.result_input.as_ref().map(|input|{
            let requirement=input.required_factor_of_safety.map(|required|format!("\nRequired Fs: {required}\nRequirement comparison: {:?}",result.requirement_comparison)).unwrap_or_default();
            format!("\n\nSoil: {}\nParameter source: {}\nStandard: {}{}\n3D display origin: toe ({:.6}, {:.6}) m",input.soil.name,input.soil.provenance,input.standard.as_deref().unwrap_or("not specified"),requirement,input.section.slope_toe.x,input.section.slope_toe.z)
        }).unwrap_or_default();
        self.ui
            .text_input(cx, ids!(geotech_results_text))
            .set_text(cx, &format!("{text}{source}"));
        let search = self
            .geotech_ui
            .search
            .as_ref()
            .map(|s| {
                format!(
                    "{}\nDomain: {}° … {}°; step {}°\nTested: {}; rejected: {}\nCriterion: {}",
                    s.classification,
                    s.spec.start_angle_deg,
                    s.spec.end_angle_deg,
                    s.spec.coarse_step_deg,
                    s.tested_surfaces,
                    s.candidates
                        .iter()
                        .filter(|c| c.rejection.is_some())
                        .count(),
                    s.criterion
                )
            })
            .unwrap_or_else(|| {
                "One specified planar slip surface. Candidate search has not run.".into()
            });
        self.ui
            .text_input(cx, ids!(geotech_search_text))
            .set_text(cx, &search);
        self.ui
            .label(cx, ids!(geotech_status))
            .set_text(cx, "Calculation completed · engineering review required");
    }

    fn geotech_refresh_scene(&mut self, cx: &mut Cx, fit: bool) {
        let (Some(render), Some(input)) = (
            self.geotech_ui.render.as_ref(),
            self.geotech_ui.result_input.as_ref(),
        ) else {
            return;
        };
        let mut viewer = Viewer::default();
        *viewer.scene_mut() = render.scene(input);
        viewer.camera_mut().position = Vec3::new(5.0, -25.0, 14.0);
        viewer.camera_mut().up = Vec3::new(0.0, 0.0, 1.0);
        if fit {
            let _ = viewer.fit_scene(1.15);
        }
        self.ui
            .cad_viewport(cx, ids!(geotech_viewport))
            .set_viewer(cx, viewer);
    }

    fn geotech_fit(&mut self, cx: &mut Cx) {
        if let Some(render) = self.geotech_ui.render.as_ref() {
            let _ = self.geotech_ui.camera.fit_bounds(render.bounds, 65.0);
            self.geotech_ui.camera_change = Some(CanvasCameraChange::Fit);
        }
        if self.geotech_ui.view_3d {
            if let Some(mut viewport) = self
                .ui
                .cad_viewport(cx, ids!(geotech_viewport))
                .borrow_mut()
            {
                let _ = viewport.viewer_mut().fit_scene(1.15);
                viewport.redraw(cx);
            }
        }
    }

    fn geotech_camera_command(
        &mut self,
        cx: &mut Cx,
        command: GeotechCameraCommand,
    ) -> Result<(), String> {
        if !self.geotech_ui.active
            || self.geotech_ui.view_3d
            || self.geotech_ui.render.is_none()
            || !self.geotech_result_fresh()
        {
            return Err(
                "2D camera controls require a current rendered section in the 2D workspace.".into(),
            );
        }
        let camera = &mut self.geotech_ui.camera;
        let (result, change) = match command {
            GeotechCameraCommand::ZoomIn | GeotechCameraCommand::ZoomOut => {
                let anchor =
                    DraftPoint::new(camera.viewport_width * 0.5, camera.viewport_height * 0.5);
                let factor = if matches!(command, GeotechCameraCommand::ZoomIn) {
                    1.25
                } else {
                    0.8
                };
                (camera.zoom_at(anchor, factor), CanvasCameraChange::Zoom)
            }
            // Arrow names describe camera-centre movement in section coordinates.
            GeotechCameraCommand::PanLeft => {
                (camera.pan_pixels(64.0, 0.0), CanvasCameraChange::Pan)
            }
            GeotechCameraCommand::PanRight => {
                (camera.pan_pixels(-64.0, 0.0), CanvasCameraChange::Pan)
            }
            GeotechCameraCommand::PanUp => (camera.pan_pixels(0.0, 64.0), CanvasCameraChange::Pan),
            GeotechCameraCommand::PanDown => {
                (camera.pan_pixels(0.0, -64.0), CanvasCameraChange::Pan)
            }
        };
        result.map_err(|error| format!("2D camera: {error:?}"))?;
        self.geotech_ui.camera_change = Some(change);
        self.refresh_geotech_canvas(cx);
        Ok(())
    }

    pub(super) fn refresh_geotech_canvas(&mut self, cx: &mut Cx) {
        if !self.geotech_ui.active || self.geotech_ui.view_3d {
            return;
        }
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        if let Some(widget) = canvas.borrow() {
            let size = widget.rect.size;
            if size.x > 1.0
                && size.y > 1.0
                && (self.geotech_ui.camera.viewport_width != size.x
                    || self.geotech_ui.camera.viewport_height != size.y)
            {
                let _ = self.geotech_ui.camera.resize(size.x, size.y);
                self.geotech_ui.camera_change = Some(CanvasCameraChange::Resize);
            }
        }
        let key = (self.geotech_ui.generation, self.geotech_ui.camera.revision);
        if self.geotech_ui.screen_key != Some(key) {
            let layer = if !self.geotech_ui.dirty {
                match (
                    &self.geotech_ui.render,
                    &self.geotech_ui.result_input,
                    &self.geotech_ui.result,
                ) {
                    (Some(render), Some(input), Some(result)) => {
                        render.canvas(input, result, &self.geotech_ui.camera)
                    }
                    _ => AnalysisCanvasLayer::default(),
                }
            } else {
                AnalysisCanvasLayer::default()
            };
            self.geotech_ui.screen = Arc::new(layer);
            self.geotech_ui.screen_key = Some(key);
        }
        let frame = CanvasFrame {
            snapshot_token: format!(
                "geotech:{}:{}",
                self.geotech_ui.active_case.map_or(0, AnalysisCaseId::get),
                self.geotech_ui.source_fingerprint
            ),
            source_generation: self.api_document_generation,
            model_generation: self.geotech_ui.generation,
            document_revision: self.geotech_binding().1,
            camera_revision: self.geotech_ui.camera.revision,
            camera_change: self.geotech_ui.camera_change.take(),
            analysis: Arc::clone(&self.geotech_ui.screen),
            ..Default::default()
        };
        if let Some(mut canvas) = canvas.borrow_mut() {
            canvas.set_frame(cx, frame);
        }
    }

    pub(super) fn geotech_ui_state(&self) -> Value {
        json!({"active":self.geotech_ui.active,"analysis_id":self.geotech_ui.active_case.map(AnalysisCaseId::get),"view":if self.geotech_ui.view_3d{"3d"}else{"2d"},"dirty_form":self.geotech_ui.dirty,"published_result_count":self.geotech_ui.calculation_count,"camera_revision":self.geotech_ui.camera.revision,"derived_generation":self.geotech_ui.generation,"analysis_triangles":self.geotech_ui.screen.triangles.len(),"analysis_segments":self.geotech_ui.screen.segments.len(),"analysis_labels":self.geotech_ui.screen.labels.len(),"render_authority":"f64_section","3d_representation":"extruded_2d_section"})
    }

    pub(super) fn geotech_observation(&self, cx: &Cx) -> Value {
        let mut value = self.geotech_ui_state();
        value["displayed_analysis_id"] =
            json!(self.geotech_ui.displayed_case.map(AnalysisCaseId::get));
        value["camera_2d"] = serde_json::to_value(&self.geotech_ui.camera).unwrap_or(Value::Null);
        value["camera_2d_coordinate_system"] = json!("section x/z, metres; camera.center.y is section z; viewport dimensions are logical pixels");
        value["render_origin_in_section_m"] = self
            .geotech_ui
            .result_input
            .as_ref()
            .map(|input| json!([input.section.slope_toe.x, 0.0, input.section.slope_toe.z]))
            .unwrap_or(Value::Null);
        value["render_coordinate_mapping"] = json!(
            "local 3D x=x_section-toe.x, y=0..B, z=z_section-toe.z; authoritative mesh/API retain section coordinates"
        );
        let fresh = self.geotech_result_fresh();
        value["fresh"] = json!(fresh);
        value["form_context_current"] = json!(self.geotech_form_current());
        value["form_expected_document_id"] = json!(self.geotech_ui.form_document_id);
        value["form_expected_open_generation"] = json!(self.geotech_ui.form_binding.map(|v| v.0));
        value["form_expected_revision"] = json!(self.geotech_ui.form_binding.map(|v| v.1));
        value["displayed_input_fingerprint"] = json!(self.geotech_ui.source_fingerprint);
        value["analysis_cache"] = self.geotech_results.observation();
        value["displayed_result"] = if fresh {
            self.geotech_ui
                .result
                .as_ref()
                .and_then(|r| serde_json::to_value(r.as_ref()).ok())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        value["displayed_results_text"] =
            json!(self.ui.text_input(cx, ids!(geotech_results_text)).text());
        value["displayed_report"] = json!(self.ui.text_input(cx, ids!(geotech_report_text)).text());
        if let Some(canvas) = self.ui.drafting_canvas(cx, ids!(drafting_canvas)).borrow() {
            value["canvas"] = json!({"snapshot_token":canvas.telemetry.snapshot_token,"analysis_triangles":canvas.telemetry.analysis_triangles,"analysis_segments":canvas.telemetry.analysis_segments,"analysis_labels":canvas.telemetry.analysis_labels,"analysis_batch_redrawn":canvas.telemetry.analysis_batch_redrawn,"draw_count":canvas.telemetry.draw_count,"camera_revision":canvas.telemetry.camera_revision});
        }
        value
    }

    pub(super) fn handle_geotech_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self
            .ui
            .button(cx, ids!(workspace_engineering))
            .clicked(actions)
        {
            self.geotech_set_active(cx, !self.geotech_ui.active);
        }
        if !self.geotech_ui.active {
            return;
        }
        if GEOTECH_INPUTS
            .iter()
            .filter(|(alias, _)| {
                !alias.starts_with("geotech.search.") && *alias != "geotech.report.path"
            })
            .any(|(_, id)| self.ui.text_input(cx, &[*id]).changed(actions).is_some())
        {
            self.geotech_input_changed(cx);
        }
        if GEOTECH_INPUTS
            .iter()
            .filter(|(alias, _)| alias.starts_with("geotech.search."))
            .any(|(_, id)| self.ui.text_input(cx, &[*id]).changed(actions).is_some())
        {
            self.geotech_search_input_changed(cx);
        }
        if let Some(mode) = self
            .ui
            .drop_down(cx, ids!(geotech_geometry_mode))
            .selected(actions)
        {
            self.geotech_ui.geometry_mode = mode;
            self.geotech_geometry_visibility(cx);
            self.geotech_input_changed(cx);
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(geotech_case_selector))
            .selected(actions)
        {
            self.geotech_ui.active_case = index
                .checked_sub(1)
                .and_then(|i| self.geotech_ui.cases.get(i).copied());
            self.geotech_invalidate(cx);
            if self.geotech_ui.active_case.is_some() {
                if let Err(error) = self.geotech_load_selected(cx) {
                    self.geotech_status(cx, &error);
                }
            } else {
                self.geotech_capture_form_context();
            }
        }
        if self.ui.button(cx, ids!(geotech_new)).clicked(actions) {
            self.geotech_ui.active_case = None;
            self.geotech_refresh_case_selector(cx);
            self.geotech_ui.dirty = false;
            self.geotech_capture_form_context();
            for (alias, id) in GEOTECH_INPUTS {
                if !alias.starts_with("geotech.search.") && *alias != "geotech.report.path" {
                    self.ui.text_input(cx, &[*id]).set_text(cx, "");
                }
            }
            self.geotech_invalidate(cx);
            self.geotech_status(
                cx,
                "New analysis case · enter geometry and soil parameters.",
            );
        }
        if self.ui.button(cx, ids!(geotech_demo)).clicked(actions) {
            self.geotech_demo(cx);
        }
        let outcome = (|| -> Result<(), String> {
            if self.ui.button(cx, ids!(geotech_reload)).clicked(actions) {
                self.geotech_load_selected(cx)?;
            }
            if self.ui.button(cx, ids!(geotech_save)).clicked(actions) {
                self.geotech_store_form(cx)?;
                self.geotech_status(cx, "Input saved through the unified transaction history.");
            }
            if self.ui.button(cx, ids!(geotech_calculate)).clicked(actions) {
                let id = self.geotech_store_form(cx)?;
                self.geotech_dispatch(
                    cx,
                    "geotech.slope.analyze",
                    &json!({"analysis_id":id.get()}),
                )?;
            }
            if self.ui.button(cx, ids!(geotech_search)).clicked(actions) {
                let number = |id, name| parse_number(&self.ui.text_input(cx, &[id]).text(), name);
                let refinement = self
                    .ui
                    .text_input(cx, ids!(geotech_search_refinement))
                    .text()
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "Refinement rounds must be a non-negative integer")?;
                let spec = CandidateSearchSpec {
                    start_angle_deg: number(id!(geotech_search_start), "search start, degrees")?,
                    end_angle_deg: number(id!(geotech_search_end), "search end, degrees")?,
                    coarse_step_deg: number(id!(geotech_search_step), "search step, degrees")?,
                    refinement_rounds: refinement,
                };
                let id = self.geotech_store_form(cx)?;
                let response = self.geotech_dispatch(
                    cx,
                    "geotech.slope.candidates",
                    &json!({"analysis_id":id.get(),"spec":spec}),
                )?;
                if response.get("status").and_then(Value::as_str) == Some("no_admissible_candidate")
                {
                    self.geotech_invalidate(cx);
                    let search = &response["search"];
                    let mut diagnostics = format!(
                        "No admissible candidate. Tested: {}.\nFirst 12 rejection diagnostics:\n",
                        search["tested_surfaces"]
                    );
                    if let Some(candidates) = search["candidates"].as_array() {
                        for candidate in candidates.iter().take(12) {
                            diagnostics.push_str(&format!(
                                "α={}°: {}\n",
                                candidate["angle_deg"], candidate["rejection"]["message"]
                            ));
                        }
                    }
                    self.ui
                        .text_input(cx, ids!(geotech_search_text))
                        .set_text(cx, &diagnostics);
                    self.geotech_status(
                        cx,
                        "No admissible slip surface in the explicitly examined family.",
                    );
                }
            }
            for (path, three) in [
                (ids!(geotech_2d), false),
                (ids!(geotech_3d), true),
                (ids!(workspace_2d), false),
                (ids!(workspace_3d), true),
            ] {
                if self.ui.button(cx, path).clicked(actions) {
                    self.geotech_set_view(cx, three);
                }
            }
            if self.ui.button(cx, ids!(geotech_fit)).clicked(actions) {
                self.geotech_fit(cx);
                self.refresh_geotech_canvas(cx);
            }
            for (control, command) in [
                (id!(geotech_zoom_in), GeotechCameraCommand::ZoomIn),
                (id!(geotech_zoom_out), GeotechCameraCommand::ZoomOut),
                (id!(geotech_pan_left), GeotechCameraCommand::PanLeft),
                (id!(geotech_pan_right), GeotechCameraCommand::PanRight),
                (id!(geotech_pan_up), GeotechCameraCommand::PanUp),
                (id!(geotech_pan_down), GeotechCameraCommand::PanDown),
            ] {
                if self.ui.button(cx, &[control]).clicked(actions) {
                    self.geotech_camera_command(cx, command)?;
                }
            }
            if self.ui.button(cx, ids!(geotech_snapshot)).clicked(actions) {
                if !self.geotech_result_fresh() || !self.geotech_form_current() {
                    return Err(
                        "Snapshot requires a current calculated input and unchanged form context."
                            .into(),
                    );
                }
                if self.geotech_ui.search.is_some() {
                    return Err("Search-minimum snapshot requires first saving the selected candidate as a separate analysis case.".into());
                }
                let id = self
                    .geotech_ui
                    .active_case
                    .ok_or("No current analysis case")?;
                self.geotech_save_snapshot(cx, id)?;
                self.geotech_status(
                    cx,
                    "Versioned calculation snapshot saved in unified document history.",
                );
            }
            if self.ui.button(cx, ids!(geotech_report)).clicked(actions)
                || self
                    .ui
                    .button(cx, ids!(geotech_save_report))
                    .clicked(actions)
            {
                if self.geotech_ui.dirty {
                    return Err(
                        "Edited input has no current report. Save and calculate first.".into(),
                    );
                }
                if !self.geotech_result_fresh() {
                    return Err("No current displayed calculation. Calculate or run an admissible candidate search first.".into());
                }
                let id = self
                    .geotech_ui
                    .active_case
                    .ok_or("No stored analysis case")?;
                let mut params = json!({"analysis_id":id.get()});
                if self
                    .ui
                    .button(cx, ids!(geotech_save_report))
                    .clicked(actions)
                {
                    params["path"] =
                        json!(self.ui.text_input(cx, ids!(geotech_report_path)).text());
                }
                let value = self.geotech_dispatch(cx, "geotech.slope.report", &params)?;
                self.ui.text_input(cx, ids!(geotech_report_text)).set_text(
                    cx,
                    value["report"]
                        .as_str()
                        .ok_or("No calculation report returned")?,
                );
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            self.geotech_status(cx, &error);
        }
        for action in self
            .ui
            .drafting_canvas(cx, ids!(drafting_canvas))
            .actions(actions)
        {
            self.geotech_canvas_action(cx, action);
        }
    }

    fn geotech_canvas_action(&mut self, cx: &mut Cx, action: CanvasAction) {
        if self.geotech_ui.view_3d {
            return;
        }
        let mut changed = false;
        match action {
            CanvasAction::Resize { width, height, .. } => {
                if self.geotech_ui.camera.resize(width, height).is_ok() {
                    self.geotech_ui.camera_change = Some(CanvasCameraChange::Resize);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Scroll { screen, delta_y }) => {
                if self
                    .geotech_ui
                    .camera
                    .zoom_at(screen, (-delta_y * 0.0015).exp().clamp(0.2, 5.0))
                    .is_ok()
                {
                    self.geotech_ui.camera_change = Some(CanvasCameraChange::Zoom);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Pointer {
                phase,
                button,
                screen,
            }) => match phase {
                PointerPhase::Down if button == PointerButton::Middle => {
                    self.geotech_ui.pan_anchor = Some(screen)
                }
                PointerPhase::Move => {
                    if let Some(old) = self.geotech_ui.pan_anchor.replace(screen) {
                        if self
                            .geotech_ui
                            .camera
                            .pan_pixels(screen.x - old.x, screen.y - old.y)
                            .is_ok()
                        {
                            self.geotech_ui.camera_change = Some(CanvasCameraChange::Pan);
                            changed = true;
                        }
                    } else {
                        self.geotech_ui.pan_anchor = None;
                    }
                }
                PointerPhase::Up | PointerPhase::Leave => self.geotech_ui.pan_anchor = None,
                _ => {}
            },
            CanvasAction::Input(CanvasInput::Cancel) => self.geotech_ui.pan_anchor = None,
            _ => {}
        }
        if changed {
            self.refresh_geotech_canvas(cx);
        }
    }
}

fn parse_number(text: &str, name: &str) -> Result<f64, String> {
    let value = text
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("{name}: enter a number in the shown units"))?;
    if !value.is_finite() {
        return Err(format!("{name}: finite value required"));
    }
    Ok(value)
}
fn value_id(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geotech_parameter_parser_rejects_units_and_nonfinite() {
        assert_eq!(parse_number("18,5", "gamma").unwrap(), 18.5);
        for bad in ["NaN", "inf", "18 kN/m3", ""] {
            assert!(parse_number(bad, "gamma").is_err());
        }
    }
}
