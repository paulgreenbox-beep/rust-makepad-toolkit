//! Circular App workspace adapter. New WIP module; main registration is owner-owned.
//! CHEAPEST_VERIFICATION_PATH: desktop owner tests, then saved-file Open composition.
//! Archive presentation uses its retained tuple and existing physical overlay gates.
use crate::{
    App,
    drafting_canvas::{
        AnalysisCanvasLayer, CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput,
        DraftingCanvasWidgetRefExt, PointerButton, PointerPhase, ScreenLabel,
    },
    geotech_circular_render::{
        GeotechCircularRender, GroundDraftRender, LocalSelectionOverlay, OverlayFlags,
    },
    geotech_circular_ui::{
        BoundaryTarget, CIRCULAR_BUTTONS, CIRCULAR_INPUTS, CIRCULAR_SELECTORS, CIRCULAR_TOGGLES,
        CircularCaseEditorProjection, CircularPanel, CircularUiIntent, DocumentBinding,
    },
    viewport::CadViewportWidgetRefExt,
};
use ai_revit_core::{
    Document,
    engineering::{AnalysisCaseId, CircularAnalysisCase, GroundSection, GroundSectionId},
};
use drafting_core::{Bounds2, Camera2D, Point2 as ScreenPoint};
use geotech_core::{
    Point2,
    circular::{CandidateId, CircularSlipSurface},
};
use makepad_widgets::*;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};
use viewer_core::{Scene, Vec3, Viewer};

const PAGE: usize = 64;
const CATALOG_RECORD_BYTES: usize = 256 << 10;
const CATALOG_AGGREGATE_BYTES: usize = 4 << 20;
const HANDLE_RADIUS: f64 = 8.0;

enum Display {
    Ground(GroundDraftRender),
    Analysis(GeotechCircularRender),
}
impl Display {
    fn view_context(&self) -> (Point2, Option<f64>) {
        match self {
            Self::Ground(r) => r.view_context(),
            Self::Analysis(r) => (r.origin, Some(r.extrusion_width_m)),
        }
    }
    fn bounds(&self) -> Bounds2 {
        match self {
            Self::Ground(r) => r.bounds,
            Self::Analysis(r) => r.bounds,
        }
    }
    fn canvas(&self, camera: &Camera2D, flags: OverlayFlags) -> AnalysisCanvasLayer {
        match self {
            Self::Ground(r) => r.canvas(camera),
            Self::Analysis(r) => r.canvas(camera, flags),
        }
    }
    fn scene(&self, flags: OverlayFlags) -> Result<Scene, String> {
        match self {
            Self::Ground(r) => r.scene(),
            Self::Analysis(r) => r.scene(flags),
        }
    }
}
#[derive(Clone, PartialEq)]
struct DisplaySource {
    draft_generation: u64,
    dirty: bool,
    stale: bool,
    width: Option<f64>,
    resolved_source: Option<String>,
    eligible_result: usize,
    presentation_state: &'static str,
}
#[derive(Clone, PartialEq)]
struct LocalSelectionSource {
    base: Option<DisplaySource>,
    circle: Option<CircularSlipSurface>,
    slice: Option<usize>,
    candidate: Option<CandidateId>,
    result: usize,
    search: usize,
}
#[derive(Clone)]
enum PointOwner {
    Terrain,
    Bottom,
    Interface(String),
}
struct PointDrag {
    owner: PointOwner,
    index: usize,
    binding: DocumentBinding,
    generation: u64,
}
impl PointOwner {
    fn target(&self) -> BoundaryTarget {
        match self {
            Self::Terrain => BoundaryTarget::Terrain,
            Self::Bottom => BoundaryTarget::Bottom,
            Self::Interface(id) => BoundaryTarget::Interface(id.clone()),
        }
    }
}

/// Only the active editor/render is retained. Catalogs contain bounded pages,
/// their actual ground references, and the active records needed for observation.
pub(super) struct CircularWorkspace {
    pub(super) panel: CircularPanel,
    camera: Camera2D,
    binding: Option<DocumentBinding>,
    ground_cursor: Option<GroundSectionId>,
    case_cursor: Option<AnalysisCaseId>,
    next_ground: Option<GroundSectionId>,
    next_case: Option<AnalysisCaseId>,
    source: Option<DisplaySource>,
    display: Option<Display>,
    local_source: Option<LocalSelectionSource>,
    local_overlay: Option<LocalSelectionOverlay>,
    local_error: Option<String>,
    pub display_generation: u64,
    screen: Arc<AnalysisCanvasLayer>,
    screen_key: Option<(u64, u64, OverlayFlags)>,
    scene_key: Option<(u64, OverlayFlags)>,
    frame: Option<CanvasFrame>,
    camera_change: Option<CanvasCameraChange>,
    pan_anchor: Option<ScreenPoint>,
    point_drag: Option<PointDrag>,
    fit_pending: bool,
    previous_drafting_2d: bool,
    catalog_status: String,
    view_status: String,
    cached_flags: OverlayFlags,
    catalog_failed: bool,
    views_cleared: bool,
    render_error: Option<String>,
    scene_error: Option<String>,
}
impl Default for CircularWorkspace {
    fn default() -> Self {
        Self {
            panel: CircularPanel::default(),
            camera: Camera2D::default(),
            binding: None,
            ground_cursor: None,
            case_cursor: None,
            next_ground: None,
            next_case: None,
            source: None,
            display: None,
            local_source: None,
            local_overlay: None,
            local_error: None,
            display_generation: 0,
            screen: Arc::default(),
            screen_key: None,
            scene_key: None,
            frame: None,
            camera_change: None,
            pan_anchor: None,
            point_drag: None,
            fit_pending: true,
            previous_drafting_2d: false,
            catalog_status: String::new(),
            view_status: String::new(),
            cached_flags: OverlayFlags::default(),
            catalog_failed: false,
            views_cleared: false,
            render_error: None,
            scene_error: None,
        }
    }
}
impl App {
    pub(super) fn circular_camera(&self) -> &Camera2D {
        &self.circular_ui.camera
    }
    pub(super) fn circular_display_bounds(&self) -> Option<Bounds2> {
        let base = self.circular_ui.display.as_ref().map(Display::bounds);
        let overlay = self.circular_ui.local_overlay.as_ref().map(|v| v.bounds);
        match (base, overlay) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (base, overlay) => base.or(overlay),
        }
    }
    pub(super) fn circular_canvas_frame(&self) -> Option<&CanvasFrame> {
        self.circular_ui.frame.as_ref()
    }
    pub(super) fn circular_screen(&self) -> &AnalysisCanvasLayer {
        self.circular_ui.screen.as_ref()
    }
    /// Before the actual widget draw; stable camera frames never revalidate ground.
    pub(super) fn circular_before_draw(&mut self, cx: &mut Cx) {
        if !self.circular_ui.panel.active {
            return;
        }
        match self.circular_binding() {
            Ok(live) if self.circular_ui.binding.as_ref() == Some(&live) => {
                if self.circular_ui.catalog_failed {
                    return;
                }
                self.circular_workspace_visibility(cx);
                if !self.circular_ui.panel.view_3d {
                    self.refresh_circular_canvas(cx);
                }
            }
            _ => self.refresh_circular(cx),
        }
    }
    pub(super) fn circular_binding(&self) -> Result<DocumentBinding, String> {
        let d = self.document.as_ref().ok_or("Нет открытого документа")?;
        Ok(DocumentBinding {
            document_uuid: d.document_uuid.to_string(),
            session_id: d.runtime_instance_id(),
            open_generation: self.api_document_generation,
            revision: d.revision(),
        })
    }

    pub(super) fn circular_set_active(&mut self, cx: &mut Cx, active: bool) {
        if self.circular_ui.panel.active == active {
            return;
        }
        self.circular_ui.pan_anchor = None;
        self.circular_ui.point_drag = None;
        if active {
            if self.circular_ui.catalog_failed {
                self.circular_ui.binding = None;
            }
            self.circular_ui.previous_drafting_2d = self.drafting_ui.active_2d;
            self.drafting_ui.active_2d = false;
            self.geotech_ui.active = false; // preserve planar state; its button remains available
        } else {
            self.drafting_ui.active_2d = self.circular_ui.previous_drafting_2d;
        }
        self.circular_ui.panel.active = active;
        cx.revert_key_focus();
        self.refresh_workspace_visibility(cx);
        self.circular_workspace_visibility(cx);
        if active {
            self.refresh_circular(cx);
        }
    }

    /// Call last from the owner's common visibility function.
    pub(super) fn circular_workspace_visibility(&mut self, cx: &mut Cx) {
        let active = self.circular_ui.panel.active;
        for (path, visible) in [
            (ids!(circular_parameters_host), active),
            (ids!(circular_results), active),
            (ids!(circular_actions), active),
            (ids!(circular_view_status), active),
            (
                ids!(circular_viewport_host),
                active && self.circular_ui.panel.view_3d,
            ),
        ] {
            self.ui.widget(cx, path).set_visible(cx, visible);
        }
        self.ui.button(cx, ids!(workspace_circular)).set_text(
            cx,
            if active {
                "● Круговой расчёт"
            } else {
                "Круговой расчёт"
            },
        );
        if !active {
            return;
        }
        for path in [
            ids!(geotech_parameters),
            ids!(geotech_results),
            ids!(geotech_actions),
            ids!(geotech_viewport_host),
            ids!(history_panel),
            ids!(inspector_panel),
            ids!(model_toolbar),
            ids!(drafting_toolbar),
            ids!(drafting_command_panel),
            ids!(viewport_3d_host),
        ] {
            self.ui.widget(cx, path).set_visible(cx, false);
        }
        let three = self.circular_ui.panel.view_3d;
        self.ui
            .widget(cx, ids!(drafting_2d_host))
            .set_visible(cx, !three);
        self.ui.label(cx, ids!(viewport_title)).set_text(
            cx,
            if three {
                "Круговой расчёт · 3D-экструзия 2D-сечения"
            } else {
                "Круговой расчёт · разрез X/Z"
            },
        );
        self.ui.label(cx, ids!(navigation_hint)).set_text(
            cx,
            if three {
                "Вращение / pan / zoom · пространственное равновесие не рассчитывается"
            } else {
                "Средняя кнопка — pan · колесо — zoom · точка — редактирование черновика"
            },
        );
    }

    /// Read-only pages. Never clone Document or an unbounded engineering map.
    pub(super) fn circular_refresh_catalog(&mut self, cx: &mut Cx) -> Result<(), String> {
        let live = self.circular_binding()?;
        let changed_runtime = self
            .circular_ui
            .binding
            .as_ref()
            .is_none_or(|old| !same_runtime(old, &live));
        if changed_runtime {
            self.circular_ui.ground_cursor = None;
            self.circular_ui.case_cursor = None;
            self.circular_ui.panel.attach_document(live.clone());
            self.circular_ui.source = None;
            self.circular_ui.point_drag = None;
            self.circular_ui.pan_anchor = None;
            self.circular_ui.fit_pending = true;
        }
        let (selected_ground, selected_case) = self.circular_ui.panel.catalog_record_ids();
        let doc = self.document.as_ref().ok_or("Нет открытого документа")?;
        // An identified sole archive can be outside the first catalog page.
        // Admit it in the existing single active-case slot before making a
        // load ticket; metadata lookup does not copy its retained evidence.
        let catalog_case = selected_case.or_else(|| {
            let mut archived = doc.circular_cases().values()
                .filter(|case| case.circular_snapshot.is_some());
            let first = archived.next().map(|case| case.analysis_id);
            archived.next().is_none().then_some(first).flatten()
        });
        let (mut grounds, next_ground, ground_count) =
            ground_page(doc, self.circular_ui.ground_cursor);
        let (mut cases, next_case, case_count) = case_page(doc, self.circular_ui.case_cursor);
        if let Some(id) = selected_ground {
            if let Some(g) = doc.ground_section(id) {
                grounds.entry(id).or_insert(g);
            }
        }
        if let Some(id) = catalog_case {
            if let Some(c) = doc.circular_case(id) {
                cases.entry(id).or_insert(c);
            }
        }
        for case in cases.values() {
            let id = case.definition.ground_section_id;
            if let std::collections::btree_map::Entry::Vacant(slot) = grounds.entry(id) {
                let ground = doc
                    .ground_section(id)
                    .ok_or("Case ссылается на отсутствующий разрез")?;
                slot.insert(ground);
            }
        }
        // 64 primary grounds + 64 case references + active/autoload references.
        if grounds.len() > 130 || cases.len() > 65 {
            return Err("Превышен лимит страницы каталога".into());
        }
        // Borrowed records only until every unique record and the complete
        // compact catalog envelope pass their serialized-byte budgets.
        audit_catalog_bytes(&grounds, &cases).map_err(|error| error.to_string())?;
        let status = format!(
            "Разрезы: {} из {} (+{} связанных/активных){} · случаи: {} из {} (+{} активных){}",
            ground_count,
            doc.ground_sections().len(),
            grounds.len() - ground_count,
            if next_ground.is_some() {
                " · есть ещё"
            } else {
                " · конец"
            },
            case_count,
            doc.circular_cases().len(),
            cases.len() - case_count,
            if next_case.is_some() {
                " · есть ещё"
            } else {
                " · конец"
            }
        );
        let projected_cases = cases
            .into_values()
            .map(|case| CircularCaseEditorProjection::from_case(case).map(Arc::new))
            .collect::<Result<Vec<_>, String>>()?;
        self.circular_ui.panel.set_catalog(
            live.clone(),
            grounds
                .into_values()
                .map(|ground| Arc::new((*ground).clone()))
                .collect(),
            projected_cases,
        )?;
        self.circular_ui.catalog_failed = false;
        self.circular_ui.binding = Some(live);
        self.circular_ui.next_ground = next_ground;
        self.circular_ui.next_case = next_case;
        self.circular_ui.catalog_status = status;
        self.circular_catalog_labels(cx);
        Ok(())
    }

    fn circular_catalog_labels(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(circular_catalog_status))
            .set_text(cx, &self.circular_ui.catalog_status);
        for (id, enabled) in [
            (
                id!(circular_ground_prev),
                self.circular_ui.ground_cursor.is_some(),
            ),
            (
                id!(circular_ground_next),
                self.circular_ui.next_ground.is_some(),
            ),
            (
                id!(circular_case_prev),
                self.circular_ui.case_cursor.is_some(),
            ),
            (
                id!(circular_case_next),
                self.circular_ui.next_case.is_some(),
            ),
        ] {
            self.ui.button(cx, &[id]).set_enabled(cx, enabled);
        }
    }

    pub(super) fn handle_circular_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Run before drafting/planar actions so shared-canvas input has one owner.
        if self.circular_ui.panel.active
            && [
                ids!(workspace_3d),
                ids!(workspace_2d),
                ids!(workspace_engineering),
            ]
            .into_iter()
            .any(|id| self.ui.button(cx, id).clicked(actions))
        {
            self.circular_set_active(cx, false);
        }
        if self
            .ui
            .button(cx, ids!(workspace_circular))
            .clicked(actions)
        {
            self.circular_set_active(cx, !self.circular_ui.panel.active);
        }
        if !self.circular_ui.panel.active {
            return;
        }
        let live = match self.circular_binding() {
            Ok(live) => live,
            Err(_) => {
                self.refresh_circular(cx);
                return;
            }
        };
        let document_changed = self.circular_ui.binding.as_ref() != Some(&live);
        if document_changed {
            if let Err(error) = self.circular_refresh_catalog(cx) {
                self.circular_catalog_failed(cx, &error);
            }
        }
        let mut page_changed = false;
        if self
            .ui
            .button(cx, ids!(circular_ground_next))
            .clicked(actions)
        {
            if let Some(next) = self.circular_ui.next_ground {
                self.circular_ui.ground_cursor = Some(next);
                page_changed = true;
            }
        }
        if self
            .ui
            .button(cx, ids!(circular_case_next))
            .clicked(actions)
        {
            if let Some(next) = self.circular_ui.next_case {
                self.circular_ui.case_cursor = Some(next);
                page_changed = true;
            }
        }
        if let Some(doc) = self.document.as_ref() {
            if self
                .ui
                .button(cx, ids!(circular_ground_prev))
                .clicked(actions)
            {
                self.circular_ui.ground_cursor = self.circular_ui.ground_cursor.and_then(|id| {
                    doc.ground_sections()
                        .range(..=id)
                        .rev()
                        .nth(PAGE)
                        .map(|(id, _)| *id)
                });
                page_changed = true;
            }
            if self
                .ui
                .button(cx, ids!(circular_case_prev))
                .clicked(actions)
            {
                self.circular_ui.case_cursor = self.circular_ui.case_cursor.and_then(|id| {
                    doc.circular_cases()
                        .range(..=id)
                        .rev()
                        .nth(PAGE)
                        .map(|(id, _)| *id)
                });
                page_changed = true;
            }
        }
        if page_changed {
            if let Err(error) = self.circular_refresh_catalog(cx) {
                self.circular_catalog_failed(cx, &error);
            }
        }
        if self.circular_ui.catalog_failed {
            return;
        }
        self.circular_native_validate_archive_view();
        let form_action = CIRCULAR_INPUTS
            .iter()
            .chain(CIRCULAR_BUTTONS)
            .chain(CIRCULAR_SELECTORS)
            .chain(CIRCULAR_TOGGLES)
            .any(|(_, id)| {
                actions
                    .filter_widget_actions(self.ui.widget(cx, &[*id]).widget_uid())
                    .next()
                    .is_some()
            });
        let intents = if form_action {
            self.circular_ui.panel.handle_actions(&self.ui, cx, actions)
        } else {
            Vec::new()
        };
        for intent in intents {
            if matches!(
                &intent,
                CircularUiIntent::LoadGround(_)
                    | CircularUiIntent::LoadCase(_)
                    | CircularUiIntent::NewCase(_)
            ) {
                self.circular_ui.fit_pending = true;
            }
            // The shared owner implements this method, including pure view arms.
            if let Err(error) = self.circular_dispatch_ui_intent(cx, intent) {
                self.circular_view_error(cx, &error);
            }
        }
        if !self.circular_ui.panel.view_3d {
            let actions = self
                .ui
                .drafting_canvas(cx, ids!(drafting_canvas))
                .actions(actions);
            for action in actions {
                if let Err(error) = self.circular_canvas_input(cx, action) {
                    self.circular_view_error(cx, &error);
                }
            }
        }
        if form_action || page_changed || document_changed {
            self.refresh_circular(cx);
        }
    }

    /// Root's dispatcher can delegate only these pure view intents here.
    /// All other variants must retain their shared authoritative implementation.
    pub(super) fn circular_apply_view_intent(
        &mut self,
        cx: &mut Cx,
        intent: &CircularUiIntent,
    ) -> Result<bool, String> {
        match intent {
            CircularUiIntent::ViewChanged => {
                self.refresh_circular(cx);
                Ok(true)
            }
            CircularUiIntent::Fit => {
                self.circular_fit(cx)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Root calls this after result/receipt callbacks if a same-source result was replaced.
    pub(super) fn circular_invalidate_display(&mut self) {
        self.circular_ui.source = None;
        self.circular_ui.local_source = None;
        self.circular_ui.local_overlay = None;
        self.circular_ui.local_error = None;
        self.circular_ui.screen_key = None;
        self.circular_ui.scene_key = None;
    }

    pub(super) fn refresh_circular(&mut self, cx: &mut Cx) {
        if !self.circular_ui.panel.active {
            return;
        }
        let live = match self.circular_binding() {
            Ok(live) => live,
            Err(error) => {
                if self.circular_ui.binding.take().is_some() {
                    self.circular_ui.panel.attach_document(DocumentBinding {
                        document_uuid: String::new(),
                        session_id: "closed".into(),
                        open_generation: self.api_document_generation,
                        revision: 0,
                    });
                }
                self.circular_ui.display = None;
                self.circular_ui.source = None;
                self.circular_ui.screen_key = None;
                self.circular_ui.point_drag = None;
                self.circular_ui.pan_anchor = None;
                self.circular_ui.panel.refresh(&self.ui, cx);
                self.circular_clear_views(cx);
                self.circular_view_error(cx, &error);
                return;
            }
        };
        if self.circular_ui.binding.as_ref() != Some(&live) {
            if let Err(error) = self.circular_refresh_catalog(cx) {
                self.circular_catalog_failed(cx, &error);
                return;
            }
        }
        if self.circular_ui.catalog_failed {
            return;
        }
        self.circular_ui.panel.refresh(&self.ui, cx);
        self.circular_workspace_visibility(cx);
        self.circular_refresh_snapshot_action(cx);
        self.circular_ui.cached_flags = self.circular_ui.panel.effective_flags();
        let panel = &self.circular_ui.panel;
        let archived = panel.archived_for_overlays();
        let eligible_result = panel.result_for_overlays().or_else(|| archived.map(|v| v.2));
        let source = DisplaySource {
            draft_generation: panel.draft_generation,
            dirty: panel.dirty,
            stale: panel.stale,
            width: panel.preview_width(),
            resolved_source: panel.saved_input().map(|i| i.source_revision.clone()),
            eligible_result: eligible_result.map_or(0, |r| r as *const _ as usize),
            presentation_state: panel.presentation_state(),
        };
        if self.circular_ui.source.as_ref() != Some(&source) {
            let display = (|| -> Result<Option<Display>, String> {
                if let Some((input, config, result)) = archived {
                    return GeotechCircularRender::new(
                        &input.ground,
                        Some(result),
                        panel.preview_width().unwrap_or(input.extrusion_width_m),
                        config,
                    )
                    .map(|r| Some(Display::Analysis(r)));
                }
                if !panel.dirty || panel.current_result().is_some() {
                    if let Some(input) = panel.saved_input() {
                        let config = panel
                            .editor_context()?
                            .loaded_config
                            .ok_or("Полный config не загружен")?;
                        return GeotechCircularRender::new(
                            &input.ground,
                            panel.result_for_overlays(),
                            panel.preview_width().unwrap_or(input.extrusion_width_m),
                            &config,
                        )
                        .map(|r| Some(Display::Analysis(r)));
                    }
                }
                panel
                    .pending_ground()
                    .map(|(ground, _)| {
                        GroundDraftRender::new(ground, panel.preview_width()).map(Display::Ground)
                    })
                    .transpose()
            })();
            self.circular_ui.render_error = None;
            self.circular_ui.scene_error = None;
            self.circular_ui.display = match display {
                Ok(value) => value,
                Err(error) => {
                    self.circular_ui.render_error = Some(error.clone());
                    self.circular_view_error(cx, &error);
                    None
                }
            };
            self.circular_ui.source = Some(source);
            self.circular_ui.display_generation =
                self.circular_ui.display_generation.saturating_add(1);
            self.circular_ui.screen_key = None;
            self.circular_ui.scene_key = None;
        }
        self.circular_refresh_local_selection();
        // Fit belongs to the measured host. The default Camera2D viewport is
        // not the canvas size, especially after opening a side-panel workspace.
        // Keep the request pending until refresh_circular_canvas sees real bounds.
        if self.circular_ui.panel.view_3d {
            self.refresh_circular_viewport(cx);
        } else {
            self.refresh_circular_canvas(cx);
        }
    }

    /// Rebuild local geometry only when the source or explicit selection changes.
    /// Camera redraws use the existing display/camera keys and never enter here.
    fn circular_refresh_local_selection(&mut self) {
        let panel = &self.circular_ui.panel;
        let local_result = panel.current_result()
            .or_else(|| panel.archived_for_overlays().map(|v| v.2));
        let source = LocalSelectionSource {
            base: self.circular_ui.source.clone(),
            circle: panel.circle_preview().cloned(),
            slice: panel.selected_slice(),
            candidate: panel.selected_candidate(),
            result: local_result.map_or(0, |r| r as *const _ as usize),
            search: panel
                .current_search_result()
                .map_or(0, |r| r as *const _ as usize),
        };
        if self.circular_ui.local_source.as_ref() == Some(&source) {
            return;
        }
        let mut candidate_error = None;
        let built = (|| -> Result<Option<LocalSelectionOverlay>, String> {
            if source.circle.is_none() && source.slice.is_none() && source.candidate.is_none() {
                return Ok(None);
            }
            let (origin, width) = self
                .circular_ui
                .display
                .as_ref()
                .ok_or("LOCAL_SELECTION_BASE_DISPLAY_UNAVAILABLE")?
                .view_context();
            let candidate = if source.candidate.is_some() {
                match panel.selected_candidate_table() {
                    Ok(table) => table,
                    Err(error) => {
                        // Retention is selective. Missing fine evidence disables
                        // this candidate arc, never another valid local selection.
                        candidate_error =
                            Some(format!("Предпросмотр кандидата недоступен: {error}"));
                        None
                    }
                }
            } else {
                None
            };
            let overlay = LocalSelectionOverlay::new(
                origin,
                width,
                panel.circle_preview(),
                local_result,
                source.slice,
                candidate,
            );
            match overlay {
                Err(error) if candidate.is_some() => {
                    candidate_error = Some(format!("Предпросмотр кандидата недоступен: {error}"));
                    // A rejected candidate path must not hide another valid
                    // local selection. The retry contains no candidate data.
                    LocalSelectionOverlay::new(
                        origin,
                        width,
                        panel.circle_preview(),
                        local_result,
                        source.slice,
                        None,
                    )
                }
                other => other,
            }
        })();
        self.circular_ui.local_error = candidate_error;
        self.circular_ui.local_overlay = match built {
            Ok(value) => value,
            Err(error) => {
                self.circular_ui.local_error = Some(error);
                None
            }
        };
        self.circular_ui.local_source = Some(source);
        self.circular_ui.display_generation = self.circular_ui.display_generation.saturating_add(1);
        // Do not clear scene_key: a changed generation rebuilds the scene while
        // retaining the user's 3D camera. A base-source change still clears it.
    }
    fn circular_selection_status(&self, base: &str) -> String {
        let mut status = base.to_string();
        if let Some(overlay) = &self.circular_ui.local_overlay {
            status.push_str(" · ");
            status.push_str(&overlay.summary());
        }
        if let Some(error) = &self.circular_ui.local_error {
            status.push_str(" · ");
            status.push_str(error);
        }
        status.chars().take(2048).collect()
    }

    pub(super) fn refresh_circular_canvas(&mut self, cx: &mut Cx) {
        if !self.circular_ui.panel.active || self.circular_ui.panel.view_3d {
            return;
        }
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        if let Some(widget) = canvas.borrow() {
            let size = widget.rect.size;
            if size.x > 1.0
                && size.y > 1.0
                && (size.x != self.circular_ui.camera.viewport_width
                    || size.y != self.circular_ui.camera.viewport_height)
            {
                if self.circular_ui.camera.resize(size.x, size.y).is_ok() {
                    self.circular_ui.camera_change = Some(CanvasCameraChange::Resize);
                    self.circular_ui.fit_pending = true;
                }
            }
        }
        let measured = canvas.borrow().is_some_and(|widget|
            widget.rect.size.x > 1.0 && widget.rect.size.y > 1.0);
        if measured && self.circular_ui.fit_pending {
            if let Some(bounds) = self.circular_display_bounds() {
                if bounds.min != bounds.max
                    && self.circular_ui.camera.fit_bounds(bounds, 28.0).is_ok()
                {
                    self.circular_ui.camera_change = Some(CanvasCameraChange::Fit);
                    self.circular_ui.fit_pending = false;
                }
            }
        }
        let flags = self.circular_ui.cached_flags;
        let key = (
            self.circular_ui.display_generation,
            self.circular_ui.camera.revision,
            flags,
        );
        if self.circular_ui.screen_key != Some(key) {
            if self
                .circular_ui
                .screen_key
                .is_some_and(|old| old.2 != flags)
            {
                self.circular_ui.point_drag = None;
            }
            let mut layer = self.circular_ui.display.as_ref().map_or_else(
                || {
                    message_layer(
                        self.circular_ui
                            .render_error
                            .as_deref()
                            .unwrap_or("Нет актуального разреза для отображения"),
                    )
                },
                |r| r.canvas(&self.circular_ui.camera, flags),
            );
            if let Some(overlay) = &self.circular_ui.local_overlay {
                let local = overlay.canvas(&self.circular_ui.camera);
                layer.segments.extend(local.segments);
                layer.labels.extend(local.labels);
            }
            self.circular_ui.screen = Arc::new(layer);
            self.circular_ui.screen_key = Some(key);
        }
        let binding = self.circular_ui.binding.as_ref();
        let token = binding.map_or(String::new(), |b| {
            format!(
                "circular:{}:{}:{}:{}:{}:{}:{}",
                b.document_uuid,
                b.session_id,
                b.open_generation,
                b.revision,
                self.circular_ui.display_generation,
                self.circular_ui.camera.revision,
                flag_bits(flags)
            )
        });
        let frame = CanvasFrame {
            snapshot_token: token,
            source_generation: self.api_document_generation,
            model_generation: self.circular_ui.display_generation,
            document_revision: binding.map_or(0, |b| b.revision),
            camera_revision: self.circular_ui.camera.revision,
            camera_change: self.circular_ui.camera_change.take(),
            analysis: Arc::clone(&self.circular_ui.screen),
            ..Default::default()
        };
        self.circular_ui.views_cleared = false;
        self.circular_ui.frame = Some(frame.clone());
        if let Some(mut canvas) = canvas.borrow_mut() {
            canvas.set_frame(cx, frame);
        }
        let base_status = self.circular_ui.render_error.clone().unwrap_or_else(|| {
            // Read the same cached result state as this display, without
            // repeating Ground readiness checks on camera-only redraws.
            let current_result = self
                .circular_ui
                .local_source
                .as_ref()
                .is_some_and(|source| source.result != 0);
            let result_layers_available = self
                .circular_ui
                .source
                .as_ref()
                .is_some_and(|source| source.eligible_result != 0);
            let presentation = self.circular_ui.source.as_ref()
                .map_or("NO_RESULT", |source| source.presentation_state);
            if presentation == "ARCHIVED_CURRENT" {
                if result_layers_available {
                    "2D-разрез · сохранённый архив актуален; расчётные слои доступны".into()
                } else {
                    "2D-разрез · сохранённый архив актуален; расчётные слои скрыты по условиям допуска".into()
                }
            } else if presentation == "ARCHIVED_STALE" {
                "2D-разрез · архив устарел; показана текущая геометрия без расчётных слоёв".into()
            } else if !current_result {
                "2D-разрез · нет актуального результата расчёта".into()
            } else if result_layers_available {
                "2D-разрез · актуальный результат; расчётные слои доступны".into()
            } else {
                "2D-разрез · актуальный результат; расчётные слои скрыты по условиям допуска".into()
            }
        });
        self.circular_ui.view_status = self.circular_selection_status(&base_status);
        self.ui
            .label(cx, ids!(circular_view_status))
            .set_text(cx, &self.circular_ui.view_status);
    }

    pub(super) fn refresh_circular_viewport(&mut self, cx: &mut Cx) {
        if !self.circular_ui.panel.active || !self.circular_ui.panel.view_3d {
            return;
        }
        let flags = self.circular_ui.cached_flags;
        let key = (self.circular_ui.display_generation, flags);
        if self.circular_ui.scene_key == Some(key) {
            if let Some(error) = self.circular_ui.scene_error.clone() {
                self.circular_view_error(cx, &error);
            }
            return;
        }
        let scene = self
            .circular_ui
            .display
            .as_ref()
            .map_or_else(|| Ok(Scene::new()), |r| r.scene(flags))
            .and_then(|scene| match self.circular_ui.local_overlay.as_ref() {
                Some(overlay) => overlay.append_scene(scene),
                None => Ok(scene),
            });
        let scene = match scene {
            Ok(scene) => scene,
            Err(error) => {
                self.circular_clear_views(cx);
                self.circular_ui.scene_key = Some(key);
                self.circular_ui.scene_error = Some(error.clone());
                self.circular_view_error(cx, &error);
                return;
            }
        };
        let viewport = self.ui.cad_viewport(cx, ids!(circular_viewport));
        let old_camera = viewport.borrow().map(|v| v.viewer().camera().clone());
        let mut viewer = Viewer::default();
        *viewer.scene_mut() = scene;
        if self.circular_ui.scene_key.is_some() {
            if let Some(camera) = old_camera {
                *viewer.camera_mut() = camera;
            }
        } else {
            viewer.camera_mut().position = Vec3::new(5.0, -25.0, 14.0);
            viewer.camera_mut().up = Vec3::new(0.0, 0.0, 1.0);
            let _ = viewer.fit_scene(1.15);
        }
        viewport.set_viewer(cx, viewer);
        if let Some(mut widget) = viewport.borrow_mut() {
            widget.set_geotech_presentation(cx, true);
            if self.circular_ui.scene_key.is_none() {
                widget.fit_scene_when_measured(cx, 1.10);
            }
        }
        self.circular_ui.views_cleared = false;
        self.circular_ui.scene_key = Some(key);
        let base_status = self.circular_ui.render_error.clone().unwrap_or_else(|| {
            let origin = match self.circular_ui.panel.presentation_state() {
                "ARCHIVED_CURRENT" => " · сохранённый архив актуален",
                "ARCHIVED_STALE" => " · архив устарел; расчётные слои скрыты",
                "CURRENT_CALCULATED" => " · расчёт выполнен в этом сеансе",
                _ => "",
            };
            format!("3D EXTRUSION OF 2D MODEL · NOT SPATIAL EQUILIBRIUM{origin}")
        });
        self.circular_ui.view_status = self.circular_selection_status(&base_status);
        self.ui
            .label(cx, ids!(circular_view_status))
            .set_text(cx, &self.circular_ui.view_status);
    }

    pub(super) fn circular_fit(&mut self, cx: &mut Cx) -> Result<(), String> {
        if !self.circular_ui.panel.active {
            return Err("Круговой режим не активен".into());
        }
        self.refresh_circular(cx);
        if self.circular_ui.panel.view_3d {
            let viewport = self.ui.cad_viewport(cx, ids!(circular_viewport));
            let mut widget = viewport.borrow_mut().ok_or("3D-вид не готов")?;
            widget.fit_scene_when_measured(cx, 1.10);
        } else {
            let bounds = self
                .circular_display_bounds()
                .ok_or("Нет разреза для вписывания")?;
            self.circular_ui
                .camera
                .fit_bounds(bounds, 28.0)
                .map_err(|e| e.to_string())?;
            self.circular_ui.camera_change = Some(CanvasCameraChange::Fit);
            self.refresh_circular_canvas(cx);
        }
        Ok(())
    }

    pub(super) fn circular_canvas_input(
        &mut self,
        cx: &mut Cx,
        action: CanvasAction,
    ) -> Result<(), String> {
        if !self.circular_ui.panel.active || self.circular_ui.panel.view_3d {
            return Err("Круговой 2D-вид не активен".into());
        }
        let before_generation = self.circular_ui.panel.draft_generation;
        match action {
            CanvasAction::Resize { width, height, .. } => {
                self.circular_ui
                    .camera
                    .resize(width, height)
                    .map_err(|e| e.to_string())?;
                self.circular_ui.camera_change = Some(CanvasCameraChange::Resize);
                self.circular_ui.point_drag = None;
            }
            CanvasAction::Input(CanvasInput::Scroll { screen, delta_y }) => {
                if !delta_y.is_finite() {
                    return Err("Недопустимое колесо".into());
                }
                self.circular_ui
                    .camera
                    .zoom_at(screen, (-delta_y * 0.0015).clamp(-4.0, 4.0).exp())
                    .map_err(|e| e.to_string())?;
                self.circular_ui.camera_change = Some(CanvasCameraChange::Zoom);
                self.circular_ui.point_drag = None;
            }
            CanvasAction::Input(CanvasInput::Pointer {
                phase,
                button,
                screen,
            }) => {
                if !screen.is_finite() {
                    return Err("Недопустимые координаты указателя".into());
                }
                match phase {
                    PointerPhase::Down if button == PointerButton::Middle => {
                        self.circular_ui.pan_anchor = Some(screen);
                        self.circular_ui.point_drag = None;
                    }
                    PointerPhase::Down if button == PointerButton::Left => {
                        self.circular_require_drawn_frame(cx)?;
                        let binding = self.circular_binding()?;
                        let panel = &self.circular_ui.panel;
                        self.circular_ui.point_drag =
                            panel.pending_ground().and_then(|(ground, generation)| {
                                nearest_point(
                                    ground,
                                    &self.circular_ui.camera,
                                    screen,
                                    matches!(self.circular_ui.display, Some(Display::Ground(_)))
                                        || self.circular_ui.cached_flags.ground,
                                    matches!(self.circular_ui.display, Some(Display::Ground(_)))
                                        || self.circular_ui.cached_flags.soil_layers,
                                )
                                .map(|(owner, index)| PointDrag {
                                    owner,
                                    index,
                                    binding,
                                    generation,
                                })
                            });
                    }
                    PointerPhase::Move => {
                        if let Some(old) = self.circular_ui.pan_anchor {
                            self.circular_ui
                                .camera
                                .pan_pixels(screen.x - old.x, screen.y - old.y)
                                .map_err(|e| e.to_string())?;
                            self.circular_ui.pan_anchor = Some(screen);
                            self.circular_ui.camera_change = Some(CanvasCameraChange::Pan);
                        } else if let Some(mut drag) = self.circular_ui.point_drag.take() {
                            self.circular_require_drawn_frame(cx)?;
                            let world = self.circular_ui.camera.screen_to_world(screen);
                            drag.generation = self.circular_ui.panel.drag_point(
                                &drag.binding,
                                drag.generation,
                                drag.owner.target(),
                                drag.index,
                                Point2::new(world.x, world.y),
                            )?;
                            self.circular_ui.point_drag = Some(drag);
                        }
                    }
                    PointerPhase::Up | PointerPhase::Leave => {
                        self.circular_ui.pan_anchor = None;
                        self.circular_ui.point_drag = None;
                    }
                    _ => {}
                }
            }
            CanvasAction::Input(CanvasInput::Cancel) => {
                self.circular_ui.pan_anchor = None;
                self.circular_ui.point_drag = None;
            }
            _ => {} // Delete/Confirm never imply a Document mutation.
        }
        if before_generation != self.circular_ui.panel.draft_generation {
            self.refresh_circular(cx);
        } else {
            self.refresh_circular_canvas(cx);
        }
        Ok(())
    }

    fn circular_require_drawn_frame(&self, cx: &mut Cx) -> Result<(), String> {
        let frame = self
            .circular_ui
            .frame
            .as_ref()
            .ok_or("Разрез ещё не подготовлен")?;
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        let widget = canvas.borrow().ok_or("Холст ещё не готов")?;
        let live = self.circular_binding()?;
        if self.circular_ui.binding.as_ref() != Some(&live)
            || self.circular_ui.catalog_failed
            || self.circular_ui.display.is_none()
            || self.circular_ui.render_error.is_some()
            || self.circular_ui.screen.segments.is_empty()
            || self.circular_ui.camera.validate().is_err()
            || !widget.rect.size.x.is_finite()
            || !widget.rect.size.y.is_finite()
            || widget.rect.size.x != self.circular_ui.camera.viewport_width
            || widget.rect.size.y != self.circular_ui.camera.viewport_height
            || frame.snapshot_token.is_empty()
            || widget.telemetry.snapshot_token != frame.snapshot_token
            || widget.telemetry.camera_revision != self.circular_ui.camera.revision
            || widget.telemetry.model_generation != self.circular_ui.display_generation
            || self.circular_ui.source.as_ref().is_none_or(|s| {
                s.draft_generation != self.circular_ui.panel.draft_generation
                    || s.stale != self.circular_ui.panel.stale
            })
        {
            return Err("STALE_CANVAS · дождитесь отображения текущего разреза".into());
        }
        Ok(())
    }

    fn circular_catalog_failed(&mut self, cx: &mut Cx, error: &str) {
        self.circular_ui.catalog_failed = true;
        self.circular_ui.binding = self.circular_binding().ok();
        if let Some(live) = self.circular_ui.binding.clone() {
            self.circular_ui.panel.attach_document(live);
        }
        self.circular_ui.panel.stale = true;
        self.circular_ui.display = None;
        self.circular_ui.source = None;
        self.circular_ui.point_drag = None;
        self.circular_ui.pan_anchor = None;
        if let Ok(context) = self.circular_ui.panel.editor_context() {
            let _ = self
                .circular_ui
                .panel
                .set_host_gates(&context, false, false);
        }
        self.circular_ui.panel.refresh(&self.ui, cx);
        self.circular_clear_views(cx);
        self.circular_view_error(cx, error);
    }
    fn circular_clear_views(&mut self, cx: &mut Cx) {
        self.circular_ui.local_source = None;
        self.circular_ui.local_overlay = None;
        self.circular_ui.local_error = None;
        self.circular_ui.frame = None;
        self.circular_ui.screen = Arc::default();
        self.circular_ui.screen_key = None;
        self.circular_ui.scene_key = None;
        if self.circular_ui.views_cleared {
            return;
        }
        self.circular_ui.views_cleared = true;
        if let Some(mut widget) = self
            .ui
            .drafting_canvas(cx, ids!(drafting_canvas))
            .borrow_mut()
        {
            widget.set_frame(cx, CanvasFrame::default());
        }
        self.ui
            .cad_viewport(cx, ids!(circular_viewport))
            .set_viewer(cx, Viewer::default());
    }
    fn circular_view_error(&mut self, cx: &mut Cx, error: &str) {
        self.circular_ui.view_status = error.chars().take(2048).collect();
        self.ui
            .label(cx, ids!(circular_view_status))
            .set_text(cx, &self.circular_ui.view_status);
        self.set_status(cx, &self.circular_ui.view_status);
    }
    pub(super) fn circular_observation(&self) -> serde_json::Value {
        serde_json::json!({"panel":self.circular_ui.panel.observation(),"display_generation":self.circular_ui.display_generation,
            "camera_revision":self.circular_ui.camera.revision,"catalog":self.circular_ui.catalog_status,
            "canvas_camera":{"viewport_width":self.circular_ui.camera.viewport_width,
                "viewport_height":self.circular_ui.camera.viewport_height,
                "center":[self.circular_ui.camera.center.x,self.circular_ui.camera.center.y],
                "pixels_per_metre":self.circular_ui.camera.pixels_per_metre},
            "initial_fit_pending":self.circular_ui.fit_pending,
            "display_bounds":self.circular_display_bounds().map(|bounds|serde_json::json!({
                "min":[bounds.min.x,bounds.min.y],"max":[bounds.max.x,bounds.max.y]})),
            "view_status":self.circular_ui.view_status,"frame_token":self.circular_ui.frame.as_ref().map(|f|&f.snapshot_token),
            "screen_triangles":self.circular_ui.screen.triangles.len(),"screen_segments":self.circular_ui.screen.segments.len(),
            "local_selection":self.circular_ui.local_overlay.as_ref().map(LocalSelectionOverlay::summary),
            "local_selection_error":self.circular_ui.local_error,
            "selected_slice":self.circular_ui.panel.selected_slice().map(|id|id.to_string()),
            "selected_candidate":self.circular_ui.panel.selected_candidate().map(|id|id.to_string()),
            "circle_preview":self.circular_ui.panel.circle_preview().is_some(),
            "native_acceptance":"NOT_RUN"})
    }
}

fn same_runtime(a: &DocumentBinding, b: &DocumentBinding) -> bool {
    a.document_uuid == b.document_uuid
        && a.session_id == b.session_id
        && a.open_generation == b.open_generation
}
fn ground_page(
    doc: &Document,
    cursor: Option<GroundSectionId>,
) -> (
    BTreeMap<GroundSectionId, &GroundSection>,
    Option<GroundSectionId>,
    usize,
) {
    let mut iter = doc
        .ground_sections()
        .range((cursor.map_or(Unbounded, Excluded), Unbounded));
    let rows: BTreeMap<_, _> = iter
        .by_ref()
        .take(PAGE)
        .map(|(id, ground)| (*id, ground))
        .collect();
    let next = iter
        .next()
        .and_then(|_| rows.last_key_value().map(|(id, _)| *id));
    let count = rows.len();
    (rows, next, count)
}
fn case_page(
    doc: &Document,
    cursor: Option<AnalysisCaseId>,
) -> (
    BTreeMap<AnalysisCaseId, &CircularAnalysisCase>,
    Option<AnalysisCaseId>,
    usize,
) {
    let mut iter = doc
        .circular_cases()
        .range((cursor.map_or(Unbounded, Excluded), Unbounded));
    let rows: BTreeMap<_, _> = iter
        .by_ref()
        .take(PAGE)
        .map(|(id, case)| (*id, case))
        .collect();
    let next = iter
        .next()
        .and_then(|_| rows.last_key_value().map(|(id, _)| *id));
    let count = rows.len();
    (rows, next, count)
}
#[derive(Clone, Copy, Debug)]
enum CatalogRecordKind {
    Ground,
    Case,
}
impl CatalogRecordKind {
    fn name(self) -> &'static str {
        match self {
            Self::Ground => "GroundSection",
            Self::Case => "CircularAnalysisCase",
        }
    }
}
#[derive(Debug)]
enum CatalogBudgetError {
    GroundRecordTooLarge { id: u64 },
    CaseRecordTooLarge { id: u64 },
    AggregateTooLarge,
    Serialization { kind: CatalogRecordKind, id: u64 },
    CaseEditorProjection { id: u64, detail: String },
}
impl std::fmt::Display for CatalogBudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GroundRecordTooLarge { id } => write!(
                f,
                "CATALOG_GROUND_BYTES_EXCEEDED · GroundSection {id}: JSON > {CATALOG_RECORD_BYTES} байт; страница не скопирована"
            ),
            Self::CaseRecordTooLarge { id } => write!(
                f,
                "CATALOG_CASE_BYTES_EXCEEDED · CircularAnalysisCase {id}: JSON > {CATALOG_RECORD_BYTES} байт; страница не скопирована"
            ),
            Self::AggregateTooLarge => write!(
                f,
                "CATALOG_AGGREGATE_BYTES_EXCEEDED · JSON каталога > {CATALOG_AGGREGATE_BYTES} байт; страница не скопирована"
            ),
            Self::CaseEditorProjection { id, detail } => write!(
                f,
                "CATALOG_CASE_EDITOR_PROJECTION_FAILED · CircularAnalysisCase {id}: {detail}; страница не скопирована"
            ),
            Self::Serialization { kind, id } => write!(
                f,
                "CATALOG_SERIALIZATION_FAILED · {} {id}; страница не скопирована",
                kind.name()
            ),
        }
    }
}
struct CatalogSizeCounter {
    bytes: usize,
    exceeded: bool,
}
impl Write for CatalogSizeCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.bytes.checked_add(bytes.len()) {
            Some(total) if total <= CATALOG_RECORD_BYTES => {
                self.bytes = total;
                Ok(bytes.len())
            }
            _ => {
                self.exceeded = true;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "catalog record byte budget exceeded",
                ))
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn catalog_record_bytes<T: serde::Serialize>(
    kind: CatalogRecordKind,
    id: u64,
    record: &T,
) -> Result<usize, CatalogBudgetError> {
    let mut counter = CatalogSizeCounter {
        bytes: 0,
        exceeded: false,
    };
    if serde_json::to_writer(&mut counter, record).is_err() {
        return Err(if counter.exceeded {
            match kind {
                CatalogRecordKind::Ground => CatalogBudgetError::GroundRecordTooLarge { id },
                CatalogRecordKind::Case => CatalogBudgetError::CaseRecordTooLarge { id },
            }
        } else {
            CatalogBudgetError::Serialization { kind, id }
        });
    }
    Ok(counter.bytes)
}
/// Exact compact JSON size of {"grounds":[records],"cases":[editor projections]}.
/// Core cases remain borrowed: archives are outside this editor payload,
/// not omitted from or replaced in their authoritative Document records.
/// Only borrowed, deduplicated records are inspected. No serialized buffer or
/// record payload is allocated here. These are serialized-byte limits, not
/// a bound on Rust heap allocation or process RSS.
fn audit_catalog_bytes(
    grounds: &BTreeMap<GroundSectionId, &GroundSection>,
    cases: &BTreeMap<AnalysisCaseId, &CircularAnalysisCase>,
) -> Result<usize, CatalogBudgetError> {
    let mut bytes = br#"{"grounds":[],"cases":[]}"#.len()
        + grounds.len().saturating_sub(1)
        + cases.len().saturating_sub(1);
    for (id, ground) in grounds {
        bytes = bytes
            .checked_add(catalog_record_bytes(
                CatalogRecordKind::Ground,
                id.get(),
                *ground,
            )?)
            .filter(|total| *total <= CATALOG_AGGREGATE_BYTES)
            .ok_or(CatalogBudgetError::AggregateTooLarge)?;
    }
    for (id, case) in cases {
        let view = CircularCaseEditorProjection::view(case);
        view.serialized_bytes()
            .map_err(|detail| CatalogBudgetError::CaseEditorProjection {
                id: id.get(),
                detail,
            })?;
        bytes = bytes
            .checked_add(catalog_record_bytes(
                CatalogRecordKind::Case,
                id.get(),
                &view,
            )?)
            .filter(|total| *total <= CATALOG_AGGREGATE_BYTES)
            .ok_or(CatalogBudgetError::AggregateTooLarge)?;
    }
    Ok(bytes)
}
fn message_layer(text: &str) -> AnalysisCanvasLayer {
    AnalysisCanvasLayer {
        labels: vec![ScreenLabel {
            position: ScreenPoint::new(16.0, 20.0),
            text: text.into(),
            color: [0.95, 0.72, 0.4, 1.0],
            placement: crate::drafting_canvas::ScreenLabelPlacement::Hud,
        }],
        ..Default::default()
    }
}
fn nearest_point(
    ground: &geotech_core::circular::GroundDefinition,
    camera: &Camera2D,
    screen: ScreenPoint,
    ground_visible: bool,
    interfaces_visible: bool,
) -> Option<(PointOwner, usize)> {
    let mut best = HANDLE_RADIUS * HANDLE_RADIUS;
    let mut found = None;
    let mut visit = |owner: PointOwner, line: &[Point2]| {
        for (index, p) in line.iter().enumerate() {
            let p = camera.world_to_screen(ScreenPoint::new(p.x, p.z));
            let distance = (p.x - screen.x).powi(2) + (p.y - screen.y).powi(2);
            if distance.is_finite() && distance < best {
                best = distance;
                found = Some((owner.clone(), index));
            }
        }
    };
    if ground_visible {
        visit(PointOwner::Terrain, &ground.terrain_polyline);
        visit(PointOwner::Bottom, &ground.bottom_boundary);
    }
    if interfaces_visible {
        for interface in &ground.layer_interfaces {
            visit(
                PointOwner::Interface(interface.interface_id.0.clone()),
                &interface.polyline,
            );
        }
    }
    found
}

fn flag_bits(f: OverlayFlags) -> u8 {
    u8::from(f.ground)
        | (u8::from(f.soil_layers) << 1)
        | (u8::from(f.failure_mass) << 2)
        | (u8::from(f.slices) << 3)
        | (u8::from(f.forces) << 4)
        | (u8::from(f.critical_circle) << 5)
}
