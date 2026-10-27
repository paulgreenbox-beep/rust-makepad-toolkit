//! Transient native 2D workspace. The only durable authority is `App.document`.
//! Canvas/controller are reused from the accepted Lab; all edits use Core commands.
use super::*;
use crate::drafting_canvas::{
    CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput, CanvasTelemetry, PointerButton,
    PointerPhase, ScreenSegment,
};
use crate::drafting_controller::{Controller, ControllerEffect, Tool};
use ai_revit_core::{DraftCommand, DraftEntityId, DraftLayerId, DrawingId};
use drafting_core::{
    Bounds2, CacheSourceKey, CachedFrame, Camera2D, CapturedExport, DisplayOptions, DraftStyle,
    DraftingCache, DraftingCacheLimits, DrawingPlane, DrawingSource, Geometry,
    Point2 as DraftPoint, tessellate_geometry,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

const DRAWING_ROWS: [LiveId; 4] = [
    id!(drawing_tree_0),
    id!(drawing_tree_1),
    id!(drawing_tree_2),
    id!(drawing_tree_3),
];
const MAX_PREVIEW_ENTITIES: usize = 256;
const MAX_PREVIEW_SEGMENTS: usize = 32_768;

/// Derived underlay broad phase; built only when BIM projection generation changes.
/// Camera/hover queries never scan the BIM document or every underlay primitive.
#[derive(Default)]
struct UnderlayIndex {
    nodes: Vec<UnderlayNode>,
    root: Option<usize>,
}
struct UnderlayNode {
    bounds: Bounds2,
    children: Option<(usize, usize)>,
    segments: Vec<usize>,
}
impl UnderlayIndex {
    fn build(segments: &[bim_underlay::UnderlaySegment]) -> Self {
        let mut index = Self::default();
        let ids = (0..segments.len()).collect::<Vec<_>>();
        if !ids.is_empty() {
            index.root = Some(index.append(segments, ids));
        }
        index
    }
    fn append(&mut self, segments: &[bim_underlay::UnderlaySegment], mut ids: Vec<usize>) -> usize {
        let first = &segments[ids[0]];
        let mut bounds = Bounds2::from_points(first.start, first.end);
        for &id in ids.iter().skip(1) {
            bounds = bounds.union(Bounds2::from_points(segments[id].start, segments[id].end));
        }
        let node = self.nodes.len();
        self.nodes.push(UnderlayNode {
            bounds,
            children: None,
            segments: Vec::new(),
        });
        if ids.len() <= 16 {
            self.nodes[node].segments = ids;
            return node;
        }
        let x_axis = bounds.max.x - bounds.min.x >= bounds.max.y - bounds.min.y;
        ids.sort_unstable_by(|a, b| {
            let a = &segments[*a];
            let b = &segments[*b];
            let ac = if x_axis {
                a.start.x * 0.5 + a.end.x * 0.5
            } else {
                a.start.y * 0.5 + a.end.y * 0.5
            };
            let bc = if x_axis {
                b.start.x * 0.5 + b.end.x * 0.5
            } else {
                b.start.y * 0.5 + b.end.y * 0.5
            };
            ac.total_cmp(&bc)
        });
        let right = ids.split_off(ids.len() / 2);
        let left = self.append(segments, ids);
        let right = self.append(segments, right);
        self.nodes[node].children = Some((left, right));
        node
    }
    fn query(&self, point: DraftPoint, tolerance: f64) -> Vec<usize> {
        let mut result = Vec::new();
        let Some(root) = self.root else {
            return result;
        };
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let node = &self.nodes[id];
            if point.x + tolerance < node.bounds.min.x
                || point.x - tolerance > node.bounds.max.x
                || point.y + tolerance < node.bounds.min.y
                || point.y - tolerance > node.bounds.max.y
            {
                continue;
            }
            if let Some((left, right)) = node.children {
                stack.push(right);
                stack.push(left);
            } else {
                result.extend_from_slice(&node.segments);
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PickOwner {
    Draft(DraftEntityId),
    Bim(ElementId),
}
impl PickOwner {
    fn value(self, drawing: DrawingId) -> Value {
        match self {
            Self::Draft(id) => json!({"domain":"drafting","drawing_id":drawing,"entity_id":id}),
            Self::Bim(id) => json!({"domain":"bim","element_id":id.get()}),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
struct UiMutationContext {
    source: CacheSourceKey,
    entity_ids: Vec<DraftEntityId>,
    layer_id: Option<DraftLayerId>,
}

/// UI state and derived resources only. No `LabSession` or secondary document.
pub(super) struct DraftingWorkspace {
    pub active_2d: bool,
    pub active_drawing: Option<DrawingId>,
    pub camera: Camera2D,
    cameras: BTreeMap<DrawingId, Camera2D>,
    pub selected_ids: Vec<DraftEntityId>,
    pub hover_id: Option<DraftEntityId>,
    selected_bim: Option<ElementId>,
    hover_bim: Option<ElementId>,
    pub active_layer: Option<DraftLayerId>,
    controller: Controller,
    intent: Option<CacheSourceKey>,
    intent_selection: Vec<DraftEntityId>,
    intent_layer: Option<DraftLayerId>,
    selection_intent: Option<UiMutationContext>,
    inspector_intent: Option<UiMutationContext>,
    inspector_dirty: bool,
    layer_intent: Option<UiMutationContext>,
    pointer: Option<DraftPoint>,
    pan_anchor: Option<DraftPoint>,
    last_click: Option<DraftPoint>,
    candidate_owners: Vec<PickOwner>,
    candidate_index: usize,
    cache: Option<DraftingCache>,
    source: Option<CacheSourceKey>,
    pub frame: Option<Arc<CachedFrame>>,
    native_model: Arc<Vec<ScreenSegment>>,
    native_highlights: Arc<Vec<ScreenSegment>>,
    owner_segments: BTreeMap<PickOwner, Vec<usize>>,
    underlay: Option<bim_underlay::UnderlayFrame>,
    underlay_index: UnderlayIndex,
    model_underlay_generation: u64,
    model_camera_revision: u64,
    model_base: Option<Arc<drafting_core::CachedFrameBase>>,
    generation: u64,
    dpi_factor: f64,
    navigation_revision: Option<(u64, u64)>,
    epoch: u64,
    drawing_options: Vec<DrawingId>,
    layer_options: Vec<DraftLayerId>,
    drawing_page: usize,
    camera_change: Option<CanvasCameraChange>,
    inspector_key: Option<(DrawingId, DraftEntityId, u64)>,
    name_input_owner: Option<DrawingId>,
    pub status: String,
    pub native_performance: Value,
    native_profiler: drafting_gpu_probe::NativeProfiler,
    native_samples: VecDeque<Value>,
    last_draw_count: u64,
    native_frame_id: u64,
    physical_pointer_events: u64,
    physical_keyboard_events: u64,
    handler_events: u64,
    last_origin: String,
    pending_action: Option<(u64, Instant)>,
    action_sequence: u64,
    pub scene_epoch: u64,
    pub scene_revision: u64,
}
impl Default for DraftingWorkspace {
    fn default() -> Self {
        Self {
            active_2d: false,
            active_drawing: None,
            camera: Camera2D::default(),
            cameras: BTreeMap::new(),
            selected_ids: Vec::new(),
            hover_id: None,
            selected_bim: None,
            hover_bim: None,
            active_layer: None,
            controller: Controller::default(),
            intent: None,
            intent_selection: Vec::new(),
            intent_layer: None,
            selection_intent: None,
            inspector_intent: None,
            inspector_dirty: false,
            layer_intent: None,
            pointer: None,
            pan_anchor: None,
            last_click: None,
            candidate_owners: Vec::new(),
            candidate_index: 0,
            cache: None,
            source: None,
            frame: None,
            native_model: Arc::new(Vec::new()),
            native_highlights: Arc::new(Vec::new()),
            owner_segments: BTreeMap::new(),
            underlay: None,
            underlay_index: UnderlayIndex::default(),
            model_underlay_generation: u64::MAX,
            model_camera_revision: u64::MAX,
            model_base: None,
            generation: 0,
            dpi_factor: 1.0,
            navigation_revision: None,
            epoch: u64::MAX,
            drawing_options: Vec::new(),
            layer_options: Vec::new(),
            drawing_page: 0,
            camera_change: None,
            inspector_key: None,
            name_input_owner: None,
            status: "Готово · м · средняя кнопка: pan · колесо: zoom".into(),
            native_performance: json!({"status":"not_drawn","scope":"Canvas child texture pass; composition and Present excluded"}),
            native_profiler: Default::default(),
            native_samples: VecDeque::new(),
            last_draw_count: 0,
            native_frame_id: 0,
            physical_pointer_events: 0,
            physical_keyboard_events: 0,
            handler_events: 0,
            last_origin: String::new(),
            pending_action: None,
            action_sequence: 0,
            scene_epoch: u64::MAX,
            scene_revision: 0,
        }
    }
}

impl App {
    pub(super) fn drafting_current_drawing_id(&self) -> Option<DrawingId> {
        self.drafting_ui.active_drawing
    }

    /// Validate the already prepared and drawn frame; never refresh to make an
    /// old input current. Draw telemetry deliberately survives a failed prepare.
    pub(super) fn validate_drawing_snapshot(&self, cx: &Cx, token: &str) -> Result<(), String> {
        let stale = || "stale_ui_context: no current prepared and drawn native snapshot".to_owned();
        let document = self.document.as_ref().ok_or_else(stale)?;
        let drawing = self.drafting_current_drawing_id().ok_or_else(stale)?;
        let source = document
            .cache_source_key(drawing, self.api_document_generation)
            .map_err(|_| stale())?;
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        let widget = canvas.borrow().ok_or_else(stale)?;
        require_current_native_snapshot(
            token,
            self.drafting_ui.frame.as_deref(),
            &source,
            &self.drafting_ui.camera,
            widget.dpi_factor.max(0.25),
            self.drafting_ui.model_underlay_generation,
            &widget.telemetry,
        )
    }

    pub(super) fn mirror_bim_selection(&mut self, id: ElementId) {
        self.drafting_ui.selected_bim = Some(id);
        self.drafting_ui.selected_ids.clear();
        self.drafting_ui.selection_intent = None;
        self.drafting_ui.inspector_intent = None;
        self.drafting_ui.inspector_dirty = false;
        self.drafting_ui.inspector_key = None;
    }

    /// One refresh boundary for both API and native Core transactions.
    pub(super) fn refresh_unified_after_commit(&mut self, cx: &mut Cx, fit: bool) {
        // Invalidate after every intermediate transaction/history step, before
        // another timer publication can observe an ABA-restored source.
        self.circular_after_mutation();
        // Native save acknowledgement still needs the proposal's old editor
        // context; defer only its UI catalog observation until that acknowledgement.
        if !self.circular_native_commit_in_progress() {
            self.refresh_circular(cx);
        }
        if self.drafting_ui.selected_bim != self.selected_element {
            self.drafting_ui.selected_bim = self.selected_element;
        }
        let must_rebuild = self.document.as_ref().is_some_and(|document| {
            if fit || self.drafting_ui.scene_epoch != self.api_document_generation {
                return true;
            }
            if self.drafting_ui.scene_revision == document.revision() {
                return false;
            }
            if self.drafting_ui.scene_revision.checked_add(1) != Some(document.revision()) {
                return true;
            }
            document
                .history_delta(document.revision())
                .map(|delta| {
                    !delta.created_elements.is_empty()
                        || !delta.updated_elements.is_empty()
                        || !delta.deleted_elements.is_empty()
                        || !delta.affected_elements.is_empty()
                })
                .unwrap_or(true)
        });
        if must_rebuild {
            self.refresh_scene(cx, fit);
        } else {
            self.refresh_history(cx);
            self.refresh_inspector(cx);
            self.refresh_object_tree(cx);
            self.refresh_drafting(cx, true);
            self.drafting_ui.scene_revision = self.document.as_ref().map_or(0, Document::revision);
        }
    }

    pub(super) fn set_workspace(&mut self, cx: &mut Cx, value: &str) -> Result<Value, String> {
        let active_2d = match value.to_ascii_lowercase().as_str() {
            "2d" | "drafting" => true,
            "3d" | "bim" => false,
            _ => return Err("workspace must be 2d or 3d".into()),
        };
        // Every generic 2D/3D entry point releases the engineering presentation
        // owner before the common host is reused. This changes no domain data.
        if self.geo_foundation_ui.active { self.geo_foundation_set_active(cx, false); }
        if self.structural_configurator_ui.active { self.structural_configurator_set_active(cx, false); }
        if self.foundation_structural_ui.active { self.foundation_structural_set_active(cx, false); }
        if self.circular_ui.panel.active { self.circular_set_active(cx, false); }
        if self.geotech_ui.active { self.geotech_set_active(cx, false); }
        self.record_drafting_action();
        if self.drafting_ui.active_2d != active_2d {
            self.drafting_ui.controller.cancel();
            self.drafting_ui.intent = None;
            self.drafting_ui.pointer = None;
            self.drafting_ui.pan_anchor = None;
            cx.revert_key_focus();
        }
        self.drafting_ui.active_2d = active_2d;
        self.refresh_workspace_visibility(cx);
        self.refresh_drafting(cx, true);
        self.drawing_ui_state(cx)
    }

    pub(super) fn refresh_workspace_visibility(&mut self, cx: &mut Cx) {
        let two = self.drafting_ui.active_2d;
        for (id, visible) in [
            (ids!(model_toolbar), !two),
            (ids!(drafting_toolbar), two),
            // Visibility belongs to ordinary View hosts: the custom 3D widget
            // uses Widget's default visibility and otherwise keeps a Fill slot.
            (ids!(viewport_3d_host), !two),
            (ids!(drafting_2d_host), two),
            (ids!(drafting_command_panel), two),
        ] {
            self.ui.widget(cx, id).set_visible(cx, visible);
        }
        self.ui
            .button(cx, ids!(workspace_3d))
            .set_text(cx, if two { "3D" } else { "● 3D" });
        self.ui
            .button(cx, ids!(workspace_2d))
            .set_text(cx, if two { "● 2D" } else { "2D" });
        self.ui
            .label(cx, ids!(viewport_title))
            .set_text(cx, if two { "2D-чертёж" } else { "3D-вид" });
        self.ui.label(cx, ids!(navigation_hint)).set_text(
            cx,
            if two {
                "Средняя кнопка — pan · колесо — zoom · клик — выбор"
            } else {
                "Тянуть — вращать · колесо — масштаб · клик — выбрать"
            },
        );
        let draft_inspector = two && self.drafting_ui.selected_bim.is_none();
        self.ui
            .widget(cx, ids!(drafting_inspector))
            .set_visible(cx, draft_inspector);
        self.ui
            .widget(cx, ids!(inspector_scroll))
            .set_visible(cx, !draft_inspector);
        self.ui
            .widget(cx, ids!(drawing_source_banner))
            .set_visible(cx, two && self.drafting_ui.selected_bim.is_some());
        self.ui
            .widget(cx, ids!(draft_open_bim_source))
            .set_visible(cx, two && self.drafting_ui.selected_bim.is_some());
        self.geotech_workspace_visibility(cx);
        self.circular_workspace_visibility(cx);
        self.foundation_structural_apply_visibility(cx);
        self.structural_configurator_apply_visibility(cx);
        self.geo_foundation_apply_visibility(cx);
    }

    pub(super) fn activate_drawing(&mut self, cx: &mut Cx, id: DrawingId) -> Result<Value, String> {
        if !self
            .document
            .as_ref()
            .is_some_and(|d| d.drawing(id).is_some())
        {
            return Err(format!("drawing {id} does not exist"));
        }
        if self.geo_foundation_ui.active { self.geo_foundation_set_active(cx, false); }
        if self.structural_configurator_ui.active { self.structural_configurator_set_active(cx, false); }
        if self.foundation_structural_ui.active { self.foundation_structural_set_active(cx, false); }
        if self.circular_ui.panel.active { self.circular_set_active(cx, false); }
        if self.geotech_ui.active { self.geotech_set_active(cx, false); }
        self.record_drafting_action();
        if let Some(previous) = self.drafting_ui.active_drawing {
            self.drafting_ui
                .cameras
                .insert(previous, self.drafting_ui.camera.clone());
        }
        if self.drafting_ui.active_drawing != Some(id) {
            let width = self.drafting_ui.camera.viewport_width;
            let height = self.drafting_ui.camera.viewport_height;
            self.drafting_ui.active_drawing = Some(id);
            self.drafting_ui.camera = self
                .drafting_ui
                .cameras
                .get(&id)
                .cloned()
                .unwrap_or_else(|| Camera2D::new(width, height));
            if self.drafting_ui.camera.viewport_width != width
                || self.drafting_ui.camera.viewport_height != height
            {
                self.drafting_ui
                    .camera
                    .resize(width, height)
                    .map_err(|e| e.to_string())?;
            }
            self.drafting_ui.cache = None;
            self.drafting_ui.source = None;
            self.drafting_ui.selected_ids.clear();
            self.drafting_ui.selected_bim = None;
            self.drafting_ui.hover_id = None;
            self.drafting_ui.hover_bim = None;
            self.drafting_ui.controller.cancel();
            self.drafting_ui.intent = None;
            self.drafting_ui.active_layer = None;
            self.drafting_ui.inspector_key = None;
            self.drafting_ui.selection_intent = None;
            self.drafting_ui.inspector_intent = None;
            self.drafting_ui.inspector_dirty = false;
            self.drafting_ui.layer_intent = None;
        }
        self.drafting_ui.active_2d = true;
        self.showing_objects = true;
        self.refresh_object_tree(cx);
        self.refresh_workspace_visibility(cx);
        self.refresh_drafting(cx, true);
        self.drawing_ui_state(cx)
    }

    fn synchronize_drafting_cache(&mut self) -> Result<(), String> {
        let drawing = self
            .drafting_ui
            .active_drawing
            .ok_or("create or select a drawing")?;
        let document = self.document.as_ref().ok_or("no document is open")?;
        let source = document
            .cache_source_key(drawing, self.api_document_generation)
            .map_err(|e| e.to_string())?;
        if self.drafting_ui.cache.is_none() {
            self.drafting_ui.cache = Some(
                DraftingCache::new(DraftingCacheLimits::default()).map_err(|e| e.to_string())?,
            );
        }
        let cache = self.drafting_ui.cache.as_mut().expect("cache initialized");
        let reset = self.drafting_ui.source.as_ref().is_none_or(|old| {
            old.document_id != source.document_id
                || old.runtime_instance_id != source.runtime_instance_id
                || old.document_epoch != source.document_epoch
                || old.drawing_id != source.drawing_id
        });
        if reset {
            cache
                .reset(document, drawing, self.api_document_generation)
                .map_err(|e| e.to_string())?;
        } else if self.drafting_ui.source.as_ref() != Some(&source) {
            if cache
                .sync_from_source(document, self.api_document_generation)
                .is_err()
            {
                cache
                    .reset(document, drawing, self.api_document_generation)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.drafting_ui.source = Some(source);
        self.drafting_ui
            .selected_ids
            .retain(|id| document.draft_entity(drawing, *id).is_some());
        Ok(())
    }

    fn prepare_drafting_cache(&mut self) -> Result<Arc<CachedFrame>, String> {
        self.synchronize_drafting_cache()?;
        let document = self.document.as_ref().ok_or("no document")?;
        let options = DisplayOptions {
            generation: self.drafting_ui.generation,
            selected_ids: self.drafting_ui.selected_ids.clone(),
            hover_id: self.drafting_ui.hover_id,
            dpi_factor: self.drafting_ui.dpi_factor,
            ..Default::default()
        };
        self.drafting_ui
            .cache
            .as_mut()
            .ok_or("no cache")?
            .prepare(
                document,
                self.api_document_generation,
                &self.drafting_ui.camera,
                &options,
            )
            .map_err(|e| e.to_string())
    }

    pub(super) fn refresh_drafting(&mut self, cx: &mut Cx, _force: bool) {
        if self.circular_ui.panel.active {
            if !self.circular_native_commit_in_progress() { self.circular_before_draw(cx); }
            return;
        }
        self.geotech_sync_case(cx);
        if self.geotech_ui.active {
            self.refresh_workspace_visibility(cx);
            self.refresh_geotech_canvas(cx);
            return;
        }
        if self.drafting_ui.epoch != self.api_document_generation {
            let was_2d = self.drafting_ui.active_2d;
            let scene_epoch = self.drafting_ui.scene_epoch;
            let scene_revision = self.drafting_ui.scene_revision;
            self.drafting_ui = DraftingWorkspace::default();
            self.drafting_ui.active_2d = was_2d;
            self.drafting_ui.epoch = self.api_document_generation;
            self.drafting_ui.scene_epoch = scene_epoch;
            self.drafting_ui.scene_revision = scene_revision;
        }
        let drawing_ids = self
            .document
            .as_ref()
            .map(|d| d.drawings().keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        if !self
            .drafting_ui
            .active_drawing
            .is_some_and(|id| drawing_ids.contains(&id))
        {
            self.drafting_ui.active_drawing = drawing_ids.first().copied();
            self.drafting_ui.cache = None;
            self.drafting_ui.source = None;
            self.drafting_ui.frame = None;
            self.drafting_ui.active_layer = None;
            self.drafting_ui.selected_ids.clear();
            self.drafting_ui.selection_intent = None;
            self.drafting_ui.inspector_intent = None;
            self.drafting_ui.inspector_dirty = false;
            self.drafting_ui.inspector_key = None;
            self.drafting_ui.layer_intent = None;
            self.drafting_ui.hover_id = None;
            self.drafting_ui.hover_bim = None;
            self.drafting_ui.controller.cancel();
            self.drafting_ui.intent = None;
            self.drafting_ui.underlay = None;
            self.drafting_ui.underlay_index = UnderlayIndex::default();
            self.drafting_ui.model_base = None;
        }
        let navigation_revision = self
            .document
            .as_ref()
            .map(|d| (self.api_document_generation, d.revision()));
        if self.drafting_ui.navigation_revision != navigation_revision
            || self.drafting_ui.drawing_options != drawing_ids
        {
            self.refresh_drawing_navigation(cx, &drawing_ids);
            self.drafting_ui.navigation_revision = navigation_revision;
        }
        self.refresh_workspace_visibility(cx);
        if !self.drafting_ui.active_2d {
            return;
        }
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        if let Some(widget) = canvas.borrow() {
            let (width, height) = (widget.rect.size.x, widget.rect.size.y);
            if width > 1.0
                && height > 1.0
                && (self.drafting_ui.camera.viewport_width != width
                    || self.drafting_ui.camera.viewport_height != height)
            {
                if self.drafting_ui.camera.resize(width, height).is_ok() {
                    self.drafting_ui.camera_change = Some(CanvasCameraChange::Resize);
                }
            }
            self.drafting_ui.dpi_factor = widget.dpi_factor.max(0.25);
        }
        let prepare_started = Instant::now();
        let Some(drawing) = self.drafting_ui.active_drawing else {
            self.drafting_ui.status = "Создайте чертёж кнопкой «+ Чертёж».".into();
            if let Some(mut canvas) = self
                .ui
                .drafting_canvas(cx, ids!(drafting_canvas))
                .borrow_mut()
            {
                canvas.set_frame(cx, CanvasFrame::default());
            }
            self.refresh_drafting_inspector(cx);
            return;
        };
        let mut display = match self.prepare_drafting_cache() {
            Ok(frame) => frame,
            Err(error) => {
                self.drafting_ui.status = error.clone();
                self.set_status(cx, &error);
                self.drafting_ui.frame = None;
                self.drafting_ui.model_base = None;
                self.drafting_ui.owner_segments.clear();
                self.drafting_ui.hover_id = None;
                self.drafting_ui.hover_bim = None;
                if let Some(mut canvas) = self
                    .ui
                    .drafting_canvas(cx, ids!(drafting_canvas))
                    .borrow_mut()
                {
                    canvas.set_frame(cx, CanvasFrame::default());
                }
                self.refresh_drafting_inspector(cx);
                return;
            }
        };
        let underlay = match self.bim_underlay_frame(drawing) {
            Ok(frame) => frame,
            Err(error) => bim_underlay::UnderlayFrame {
                segments: Arc::new(Vec::new()),
                source_revision: display.source.document_revision,
                generation: 0,
                status: json!({"status":"error","message":error}),
            },
        };
        let base_changed = self
            .drafting_ui
            .model_base
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &display.base))
            || self.drafting_ui.model_underlay_generation != underlay.generation
            || self.drafting_ui.model_camera_revision != self.drafting_ui.camera.revision;
        if base_changed {
            let mut segments =
                Vec::with_capacity(display.base.primitives.len() + underlay.segments.len());
            self.drafting_ui.owner_segments.clear();
            for segment in underlay.segments.iter() {
                let start = self.drafting_ui.camera.world_to_screen(segment.start);
                let end = self.drafting_ui.camera.world_to_screen(segment.end);
                if !segment_in_view(start, end, &self.drafting_ui.camera) {
                    continue;
                }
                self.drafting_ui
                    .owner_segments
                    .entry(PickOwner::Bim(segment.element_id))
                    .or_default()
                    .push(segments.len());
                segments.push(ScreenSegment {
                    start,
                    end,
                    color: [0.39, 0.48, 0.57, 1.0],
                    width: 1.2,
                });
            }
            for primitive in display.base.primitives.iter() {
                self.drafting_ui
                    .owner_segments
                    .entry(PickOwner::Draft(primitive.owner_entity_id))
                    .or_default()
                    .push(segments.len());
                segments.push(ScreenSegment {
                    start: primitive.start,
                    end: primitive.end,
                    color: primitive.appearance.color,
                    width: primitive.appearance.width_px as f32,
                });
            }
            self.drafting_ui.native_model = Arc::new(segments);
            self.drafting_ui.model_base = Some(Arc::clone(&display.base));
            self.drafting_ui.model_underlay_generation = underlay.generation;
            self.drafting_ui.model_camera_revision = self.drafting_ui.camera.revision;
        }
        if self
            .drafting_ui
            .underlay
            .as_ref()
            .is_none_or(|old| old.generation != underlay.generation)
        {
            self.drafting_ui.underlay_index = UnderlayIndex::build(&underlay.segments);
        }
        self.drafting_ui.underlay = Some(underlay);
        self.ui.label(cx, ids!(drafting_quality)).set_text(
            cx,
            if self
                .drafting_ui
                .underlay
                .as_ref()
                .is_some_and(|f| !f.segments.is_empty())
            {
                "analytic_2d + approximate_mesh_projection · м"
            } else {
                "analytic_2d · м"
            },
        );
        let hover_before = (self.drafting_ui.hover_id, self.drafting_ui.hover_bim);
        if self.drafting_ui.pointer.is_some() {
            if let Err(error) = self.sync_drafting_pointer() {
                self.drafting_ui.status = error;
            }
        }
        if hover_before != (self.drafting_ui.hover_id, self.drafting_ui.hover_bim) {
            if let Ok(current) = self.prepare_drafting_cache() {
                display = current;
            }
        }
        let mut highlights = Vec::new();
        let mut owners = self
            .drafting_ui
            .selected_ids
            .iter()
            .copied()
            .map(|id| (PickOwner::Draft(id), true))
            .collect::<Vec<_>>();
        if let Some(id) = self.drafting_ui.selected_bim {
            owners.push((PickOwner::Bim(id), true));
        }
        if let Some(id) = self
            .drafting_ui
            .hover_id
            .filter(|id| !self.drafting_ui.selected_ids.contains(id))
        {
            owners.push((PickOwner::Draft(id), false));
        }
        if let Some(id) = self
            .drafting_ui
            .hover_bim
            .filter(|id| Some(*id) != self.drafting_ui.selected_bim)
        {
            owners.push((PickOwner::Bim(id), false));
        }
        for (owner, selected) in owners {
            if let Some(indices) = self.drafting_ui.owner_segments.get(&owner) {
                for &index in indices {
                    let mut segment = self.drafting_ui.native_model[index];
                    segment.color = if selected {
                        [1.0, 0.74, 0.22, 1.0]
                    } else {
                        [0.35, 1.0, 0.9, 1.0]
                    };
                    segment.width += 1.0;
                    highlights.push(segment);
                }
            }
        }
        if self.drafting_ui.native_highlights.as_ref() != &highlights {
            self.drafting_ui.native_highlights = Arc::new(highlights);
        }
        let mut preview_geometry = self.drafting_ui.controller.preview();
        if self.drafting_ui.controller.tool == Tool::Move && self.drafting_intent_current().is_ok()
        {
            let delta = self.drafting_ui.controller.pending_move.or_else(|| {
                let first = self.drafting_ui.controller.points.first()?;
                let pointer = self.drafting_ui.controller.pointer?;
                Some(DraftPoint::new(pointer.x - first.x, pointer.y - first.y))
            });
            if let Some(delta) = delta {
                if self.drafting_ui.intent_selection.len() <= MAX_PREVIEW_ENTITIES {
                    if let Some(document) = self.document.as_ref() {
                        preview_geometry.extend(
                            self.drafting_ui
                                .intent_selection
                                .iter()
                                .filter_map(|id| document.draft_entity(drawing, *id))
                                .map(|entity| entity.geometry.translated(delta)),
                        );
                    }
                }
            }
        }
        let options = DisplayOptions {
            dpi_factor: display.dpi_factor,
            ..Default::default()
        };
        let preview = preview_segments(&preview_geometry, &self.drafting_ui.camera, &options)
            .unwrap_or_else(|error| {
                self.drafting_ui.status = error;
                Vec::new()
            });
        let frame = CanvasFrame {
            snapshot_token: format!(
                "{}:bim:{}",
                display.token, self.drafting_ui.model_underlay_generation
            ),
            source_generation: display.generations.geometry,
            model_generation: display.generations.geometry,
            document_revision: display.source.document_revision,
            camera_revision: display.camera.revision,
            camera_change: self.drafting_ui.camera_change.take(),
            cpu_frame_prepare_ms: Some(prepare_started.elapsed().as_secs_f64() * 1000.0),
            segments: Arc::clone(&self.drafting_ui.native_model),
            highlights: Arc::clone(&self.drafting_ui.native_highlights),
            preview,
            crosshair: self.drafting_ui.pointer,
            analysis: Default::default(),
        };
        self.drafting_ui.frame = Some(display);
        if let Some(mut canvas) = self
            .ui
            .drafting_canvas(cx, ids!(drafting_canvas))
            .borrow_mut()
        {
            canvas.set_frame(cx, frame);
        }
        self.ui
            .label(cx, ids!(draft_command_prompt))
            .set_text(cx, &self.drafting_ui.controller.prompt());
        let coordinates = self
            .drafting_ui
            .controller
            .pointer
            .map(|p| format!("X={:.6} Y={:.6} м", p.x, p.y))
            .unwrap_or_else(|| "м".into());
        self.set_status(
            cx,
            &format!(
                "{} · {} · rev {}",
                self.drafting_ui.status,
                coordinates,
                self.document.as_ref().map_or(0, Document::revision)
            ),
        );
        self.refresh_drafting_inspector(cx);
    }

    fn refresh_drawing_navigation(&mut self, cx: &mut Cx, ids: &[DrawingId]) {
        let labels = ids
            .iter()
            .filter_map(|id| {
                self.document
                    .as_ref()?
                    .drawing(*id)
                    .map(|drawing| format!("{} · #{}", drawing.name, id))
            })
            .collect::<Vec<_>>();
        if self.drafting_ui.drawing_options != ids {
            self.ui.drop_down(cx, ids!(drawing_selector)).set_labels(
                cx,
                if labels.is_empty() {
                    vec!["Нет чертежей".into()]
                } else {
                    labels.clone()
                },
            );
            self.drafting_ui.drawing_options = ids.to_vec();
        } else if !labels.is_empty() {
            self.ui
                .drop_down(cx, ids!(drawing_selector))
                .set_labels(cx, labels);
        }
        if let Some(index) = ids
            .iter()
            .position(|id| Some(*id) == self.drafting_ui.active_drawing)
        {
            self.ui
                .drop_down(cx, ids!(drawing_selector))
                .set_selected_item(cx, index);
        }
        if self.drafting_ui.name_input_owner != self.drafting_ui.active_drawing {
            if let Some(drawing) = self
                .drafting_ui
                .active_drawing
                .and_then(|id| self.document.as_ref()?.drawing(id))
            {
                self.ui
                    .text_input(cx, ids!(drawing_name_input))
                    .set_text(cx, &drawing.name);
            }
            self.drafting_ui.name_input_owner = self.drafting_ui.active_drawing;
        }
        let pages = ids.len().div_ceil(DRAWING_ROWS.len()).max(1);
        self.drafting_ui.drawing_page = self.drafting_ui.drawing_page.min(pages - 1);
        for (row, widget_id) in DRAWING_ROWS.iter().enumerate() {
            let widget = self.ui.widget(cx, &[*widget_id]);
            if let Some(id) = ids.get(self.drafting_ui.drawing_page * 4 + row) {
                let drawing = self
                    .document
                    .as_ref()
                    .and_then(|d| d.drawing(*id))
                    .expect("drawing listed");
                widget.set_text(
                    cx,
                    &format!(
                        "{}{} · {} объектов",
                        if Some(*id) == self.drafting_ui.active_drawing {
                            "▶ "
                        } else {
                            "  "
                        },
                        drawing.name,
                        drawing.entities.len()
                    ),
                );
                widget.set_visible(cx, true);
            } else {
                widget.set_visible(cx, false);
            }
        }
        self.ui.label(cx, ids!(drawing_tree_page)).set_text(
            cx,
            &format!("{} / {}", self.drafting_ui.drawing_page + 1, pages),
        );
        self.ui
            .button(cx, ids!(drawing_tree_previous))
            .set_enabled(cx, self.drafting_ui.drawing_page > 0);
        self.ui
            .button(cx, ids!(drawing_tree_next))
            .set_enabled(cx, self.drafting_ui.drawing_page + 1 < pages);
    }

    fn refresh_drafting_inspector(&mut self, cx: &mut Cx) {
        let drawing = self
            .drafting_ui
            .active_drawing
            .and_then(|id| self.document.as_ref()?.drawing(id));
        let source = drawing.and_then(|drawing| {
            self.document
                .as_ref()?
                .cache_source_key(drawing.id, self.api_document_generation)
                .ok()
        });
        let layers = drawing
            .map(|drawing| drawing.layers.values().collect::<Vec<_>>())
            .unwrap_or_default();
        self.drafting_ui.layer_options = layers.iter().map(|layer| layer.id).collect();
        if !self
            .drafting_ui
            .active_layer
            .is_some_and(|id| self.drafting_ui.layer_options.contains(&id))
        {
            self.drafting_ui.active_layer = self.drafting_ui.layer_options.first().copied();
        }
        self.drafting_ui.layer_intent = source.clone().map(|source| UiMutationContext {
            source,
            entity_ids: Vec::new(),
            layer_id: self.drafting_ui.active_layer,
        });
        self.ui
            .drop_down(cx, ids!(draft_layer_selector))
            .set_labels(
                cx,
                if layers.is_empty() {
                    vec!["Нет слоя".into()]
                } else {
                    layers
                        .iter()
                        .map(|layer| format!("{} · #{}", layer.name, layer.id))
                        .collect()
                },
            );
        if let Some(index) = self
            .drafting_ui
            .layer_options
            .iter()
            .position(|id| Some(*id) == self.drafting_ui.active_layer)
        {
            self.ui
                .drop_down(cx, ids!(draft_layer_selector))
                .set_selected_item(cx, index);
            let layer = layers[index];
            self.ui.label(cx, ids!(draft_layer_summary)).set_text(
                cx,
                &format!(
                    "{} · {}",
                    if layer.visible {
                        "видим"
                    } else {
                        "скрыт"
                    },
                    if layer.locked {
                        "заблокирован"
                    } else {
                        "доступен"
                    }
                ),
            );
        }
        let selected = self.drafting_ui.selected_ids.first().copied();
        let entity = drawing.and_then(|drawing| selected.and_then(|id| drawing.entities.get(&id)));
        if let (Some(drawing), Some(entity)) = (drawing, entity) {
            self.ui.label(cx, ids!(draft_selected_label)).set_text(
                cx,
                &format!(
                    "DraftEntity #{} · Drawing #{} · analytic_2d",
                    entity.id, drawing.id
                ),
            );
            let key = (drawing.id, entity.id, entity.revision);
            let same_entity = self
                .drafting_ui
                .inspector_key
                .is_some_and(|old| old.0 == drawing.id && old.1 == entity.id);
            if !same_entity || !self.drafting_ui.inspector_dirty {
                let value = json!({"domain":"drafting","drawing_id":drawing.id,"entity":entity,"representation_quality":"analytic_2d","units":"metres/radians"});
                self.ui.text_input(cx, ids!(draft_inspect)).set_text(
                    cx,
                    &serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
                if self.drafting_ui.inspector_key != Some(key) {
                    self.ui.text_input(cx, ids!(draft_geometry_input)).set_text(
                        cx,
                        &serde_json::to_string_pretty(&entity.geometry).unwrap_or_default(),
                    );
                }
                self.drafting_ui.inspector_key = Some(key);
                self.drafting_ui.inspector_dirty = false;
                self.drafting_ui.inspector_intent =
                    source.clone().map(|source| UiMutationContext {
                        source,
                        entity_ids: vec![entity.id],
                        layer_id: Some(entity.layer_id),
                    });
            } else if self
                .drafting_ui
                .inspector_intent
                .as_ref()
                .map(|intent| &intent.source)
                != source.as_ref()
            {
                self.ui.label(cx, ids!(draft_selected_label)).set_text(
                    cx,
                    &format!(
                        "DraftEntity #{} изменён вне редактора · выберите заново",
                        entity.id
                    ),
                );
            }
        } else {
            self.drafting_ui.inspector_key = None;
            self.drafting_ui.inspector_intent = None;
            self.drafting_ui.inspector_dirty = false;
            self.ui
                .label(cx, ids!(draft_selected_label))
                .set_text(cx, "Нет выбранных CAD-объектов");
            self.ui
                .text_input(cx, ids!(draft_inspect))
                .set_text(cx, "Выберите объект на Canvas или по ID.");
        }
        let underlay=self.drafting_ui.underlay.as_ref().map(|frame|json!({"source_revision":frame.source_revision,"quality":"approximate_mesh_projection","details":frame.status})).unwrap_or_else(||json!({"status":"no_reference"}));
        self.ui
            .text_input(cx, ids!(draft_underlay_summary))
            .set_text(
                cx,
                &serde_json::to_string_pretty(&underlay).unwrap_or_default(),
            );
        self.ui.label(cx, ids!(drawing_source_banner)).set_text(
            cx,
            &format!(
                "BIM Element #{} · approximate_mesh_projection · исходная rev {}",
                self.drafting_ui.selected_bim.map_or(0, ElementId::get),
                self.drafting_ui
                    .underlay
                    .as_ref()
                    .map_or(0, |f| f.source_revision)
            ),
        );
        self.ui
            .button(cx, ids!(draft_open_bim_source))
            .set_enabled(cx, self.drafting_ui.selected_bim.is_some());
    }

    fn drafting_intent_current(&self) -> Result<(), String> {
        let intent = self
            .drafting_ui
            .intent
            .as_ref()
            .ok_or("no construction intent")?;
        let drawing = self.drafting_ui.active_drawing.ok_or("no drawing")?;
        require_ui_intent(
            intent,
            self.document.as_ref().ok_or("no document")?,
            drawing,
            self.api_document_generation,
        )
    }

    fn capture_drafting_intent(&mut self) -> Result<(), String> {
        if self.drafting_ui.intent.is_none() {
            let drawing = self
                .drafting_ui
                .active_drawing
                .ok_or("create or select a drawing")?;
            self.drafting_ui.intent = Some(
                self.document
                    .as_ref()
                    .ok_or("no document")?
                    .cache_source_key(drawing, self.api_document_generation)
                    .map_err(|e| e.to_string())?,
            );
            self.drafting_ui.intent_selection = self.drafting_ui.selected_ids.clone();
            self.drafting_ui.intent_layer = self.drafting_ui.active_layer;
        }
        self.drafting_intent_current()
    }

    pub(super) fn drawing_tool(&mut self, cx: &mut Cx, name: &str) -> Result<Value, String> {
        self.record_drafting_action();
        let tool = Tool::parse(name)?;
        self.drafting_ui.controller.begin(tool);
        self.drafting_ui.intent = None;
        self.drafting_ui.intent_selection.clear();
        self.drafting_ui.generation = self.drafting_ui.generation.saturating_add(1);
        self.refresh_drafting(cx, true);
        Ok(
            json!({"tool":tool.name(),"revision":self.document.as_ref().map_or(0,Document::revision)}),
        )
    }

    pub(super) fn drawing_numeric(&mut self, cx: &mut Cx, input: &str) -> Result<Value, String> {
        self.record_drafting_action();
        self.capture_drafting_intent()?;
        let effect = self.drafting_ui.controller.numeric(input)?;
        let value = self.apply_drafting_effect(cx, effect)?;
        self.refresh_drafting(cx, true);
        Ok(value)
    }

    fn apply_drafting_effect(
        &mut self,
        cx: &mut Cx,
        effect: ControllerEffect,
    ) -> Result<Value, String> {
        let drawing = self.drafting_ui.active_drawing.ok_or("no active drawing")?;
        let command = match effect {
            ControllerEffect::None => return Ok(json!({"preview_only":true})),
            ControllerEffect::SelectAt(world) => {
                return self.pick_drafting_at(cx, self.drafting_ui.camera.world_to_screen(world));
            }
            ControllerEffect::Create(geometry) => {
                self.drafting_intent_current()?;
                let layer = self
                    .drafting_ui
                    .intent_layer
                    .ok_or("no captured active layer")?;
                DraftCommand::CreateDraftEntity {
                    drawing_id: drawing,
                    layer_id: layer,
                    geometry,
                    style: DraftStyle::default(),
                }
            }
            ControllerEffect::Move(delta) => {
                self.drafting_intent_current()?;
                if self.drafting_ui.intent_selection.is_empty() {
                    return Err("Выберите ручные CAD-объекты перед MOVE.".into());
                }
                DraftCommand::MoveDraftEntity {
                    drawing_id: drawing,
                    entity_ids: self.drafting_ui.intent_selection.clone(),
                    delta,
                }
            }
            ControllerEffect::Delete => {
                if self.drafting_ui.selected_ids.is_empty() {
                    return Err("Удаление доступно для выбранных ручных CAD-объектов.".into());
                }
                DraftCommand::DeleteDraftEntity {
                    drawing_id: drawing,
                    entity_ids: self.drafting_ui.selected_ids.clone(),
                }
            }
        };
        let receipt = self.commit_drafting_command(cx, "draft-tool", command)?;
        self.drafting_ui.intent = None;
        self.drafting_ui.intent_selection.clear();
        Ok(receipt)
    }

    fn commit_drafting_command(
        &mut self,
        cx: &mut Cx,
        label: &str,
        command: DraftCommand,
    ) -> Result<Value, String> {
        let command_id = self.next_command_id(label);
        let geometry_editor_commit = matches!(&command, DraftCommand::UpdateDraftEntity { .. });
        let guarded_context = match &command {
            DraftCommand::CreateDraftEntity { .. } | DraftCommand::MoveDraftEntity { .. } => {
                Some(UiMutationContext {
                    source: self
                        .drafting_ui
                        .intent
                        .clone()
                        .ok_or("stale_ui_intent: начните построение заново")?,
                    entity_ids: self.drafting_ui.intent_selection.clone(),
                    layer_id: self.drafting_ui.intent_layer,
                })
            }
            DraftCommand::DeleteDraftEntity { .. } => Some(
                self.drafting_ui
                    .selection_intent
                    .clone()
                    .ok_or("stale_ui_intent: выберите объекты заново")?,
            ),
            DraftCommand::UpdateDraftEntity { .. } => Some(
                self.drafting_ui
                    .inspector_intent
                    .clone()
                    .ok_or("stale_ui_intent: выберите объект заново")?,
            ),
            DraftCommand::UpdateDraftLayer { .. } => Some(
                self.drafting_ui
                    .layer_intent
                    .clone()
                    .ok_or("stale_ui_intent: выберите слой заново")?,
            ),
            _ => None,
        };
        let receipt = {
            let document = self.document.as_mut().ok_or("no document is open")?;
            if let Some(context) = guarded_context {
                commit_ui_mutation(
                    document,
                    self.api_document_generation,
                    &context,
                    command_id,
                    command,
                )?
            } else {
                let mut transaction = document.begin_transaction(command_id, document.revision());
                transaction.push(Command::Draft(command));
                document.commit(transaction).map_err(|e| e.to_string())?
            }
        };
        if let Some(id) = receipt.created_drawings.first().copied() {
            if let Some(previous) = self.drafting_ui.active_drawing {
                self.drafting_ui
                    .cameras
                    .insert(previous, self.drafting_ui.camera.clone());
            }
            self.drafting_ui.active_drawing = Some(id);
            let mut camera = Camera2D::new(
                self.drafting_ui.camera.viewport_width,
                self.drafting_ui.camera.viewport_height,
            );
            if let Some(saved) = self.drafting_ui.cameras.get(&id) {
                camera = saved.clone();
            }
            self.drafting_ui.camera = camera;
            self.drafting_ui.selected_ids.clear();
            self.drafting_ui.selected_bim = None;
            self.drafting_ui.hover_id = None;
            self.drafting_ui.hover_bim = None;
            self.drafting_ui.active_layer = None;
            self.drafting_ui.inspector_key = None;
            self.drafting_ui.selection_intent = None;
            self.drafting_ui.inspector_intent = None;
            self.drafting_ui.inspector_dirty = false;
            self.drafting_ui.layer_intent = None;
            self.drafting_ui.cache = None;
            self.drafting_ui.source = None;
        }
        if !receipt.created_draft_entities.is_empty() {
            self.drafting_ui.selected_ids = receipt.created_draft_entities.clone();
            self.drafting_ui.selected_bim = None;
        }
        if geometry_editor_commit {
            self.drafting_ui.inspector_dirty = false;
            self.drafting_ui.inspector_key = None;
        }
        self.capture_selection_intent();
        self.drafting_ui.generation = self.drafting_ui.generation.saturating_add(1);
        self.drafting_ui.status = format!(
            "Применена транзакция · rev {}",
            self.document.as_ref().map_or(0, Document::revision)
        );
        self.select_latest_operation();
        self.refresh_unified_after_commit(cx, false);
        serde_json::to_value(receipt).map_err(|e| e.to_string())
    }

    pub(super) fn drawing_select_ids(
        &mut self,
        cx: &mut Cx,
        ids: Vec<DraftEntityId>,
    ) -> Result<Value, String> {
        let drawing = self.drafting_ui.active_drawing.ok_or("no active drawing")?;
        let document = self.document.as_ref().ok_or("no document")?;
        if ids.len() > drafting_core::MAX_DOCUMENT_ENTITIES
            || ids
                .iter()
                .any(|id| document.draft_entity(drawing, *id).is_none())
        {
            return Err("selection contains an absent ID or exceeds the selection budget".into());
        }
        self.drafting_ui.selected_ids = ids;
        self.capture_selection_intent();
        self.drafting_ui.inspector_dirty = false;
        self.drafting_ui.inspector_key = None;
        self.drafting_ui.selected_bim = None;
        self.selected_element = None;
        self.semantic_selection = None;
        self.selected_operation = None;
        self.sync_viewport_selection(cx);
        self.drafting_ui.generation = self.drafting_ui.generation.saturating_add(1);
        self.refresh_drafting(cx, true);
        Ok(
            json!({"domain":"drafting","drawing_id":drawing,"entity_ids":self.drafting_ui.selected_ids}),
        )
    }

    fn capture_selection_intent(&mut self) {
        self.drafting_ui.selection_intent = self.document.as_ref().and_then(|document| {
            let drawing = self.drafting_ui.active_drawing?;
            if self.drafting_ui.selected_ids.is_empty() {
                return None;
            }
            Some(UiMutationContext {
                source: document
                    .cache_source_key(drawing, self.api_document_generation)
                    .ok()?,
                entity_ids: self.drafting_ui.selected_ids.clone(),
                layer_id: None,
            })
        });
    }

    fn drafting_pick_candidates(&mut self, screen: DraftPoint) -> Result<Vec<PickOwner>, String> {
        // Semantic picking is analytic. A rejected display budget must not block
        // inspection or selection of an otherwise valid authoritative drawing.
        self.synchronize_drafting_cache()?;
        let drawing = self.drafting_ui.active_drawing.ok_or("no active drawing")?;
        let underlay = match self.bim_underlay_frame(drawing) {
            Ok(frame) => frame,
            Err(error) => bim_underlay::UnderlayFrame {
                segments: Arc::new(Vec::new()),
                source_revision: self.document.as_ref().map_or(0, Document::revision),
                generation: 0,
                status: json!({"status":"error","message":error}),
            },
        };
        if self
            .drafting_ui
            .underlay
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(&old.segments, &underlay.segments))
        {
            self.drafting_ui.underlay_index = UnderlayIndex::build(&underlay.segments);
        }
        self.drafting_ui.underlay = Some(underlay);
        let document = self.document.as_ref().ok_or("no document")?;
        let cache = self.drafting_ui.cache.as_mut().ok_or("no cache")?;
        let hits = pick_manual_semantics(
            cache,
            document,
            self.api_document_generation,
            &self.drafting_ui.camera,
            screen,
        )?;
        let mut candidates = hits
            .into_iter()
            .map(|hit| (hit.distance_px, PickOwner::Draft(hit.entity_id)))
            .collect::<Vec<_>>();
        if let Some(underlay) = &self.drafting_ui.underlay {
            let mut distances: BTreeMap<ElementId, f64> = BTreeMap::new();
            let world = self.drafting_ui.camera.screen_to_world(screen);
            for index in self
                .drafting_ui
                .underlay_index
                .query(world, 7.0 / self.drafting_ui.camera.pixels_per_metre)
            {
                let segment = &underlay.segments[index];
                let distance = segment_distance(world, segment.start, segment.end)
                    * self.drafting_ui.camera.pixels_per_metre;
                if distance <= 7.0 {
                    let old = distances.entry(segment.element_id).or_insert(distance);
                    *old = old.min(distance);
                }
            }
            candidates.extend(distances.into_iter().map(|(id, d)| (d, PickOwner::Bim(id))));
        }
        candidates.sort_by(|(da, a), (db, b)| da.total_cmp(db).then_with(|| a.cmp(b)));
        Ok(candidates.into_iter().map(|(_, owner)| owner).collect())
    }

    fn pick_drafting_at(&mut self, cx: &mut Cx, screen: DraftPoint) -> Result<Value, String> {
        let candidates = self.drafting_pick_candidates(screen)?;
        let repeat = self
            .drafting_ui
            .last_click
            .is_some_and(|last| last.distance(screen) < 3.0)
            && self.drafting_ui.candidate_owners == candidates;
        self.drafting_ui.candidate_index = if repeat && !candidates.is_empty() {
            (self.drafting_ui.candidate_index + 1) % candidates.len()
        } else {
            0
        };
        self.drafting_ui.last_click = Some(screen);
        self.drafting_ui.candidate_owners = candidates;
        let owner = self
            .drafting_ui
            .candidate_owners
            .get(self.drafting_ui.candidate_index)
            .copied();
        self.drafting_ui.selected_ids.clear();
        self.drafting_ui.selected_bim = None;
        match owner {
            Some(PickOwner::Draft(id)) => {
                self.drafting_ui.selected_ids.push(id);
                self.selected_element = None;
                self.semantic_selection = None;
            }
            Some(PickOwner::Bim(id)) => {
                self.drafting_ui.selected_bim = Some(id);
                self.select_element(cx, id);
            }
            None => {
                self.selected_element = None;
                self.semantic_selection = None;
            }
        }
        self.capture_selection_intent();
        self.drafting_ui.inspector_dirty = false;
        self.drafting_ui.inspector_key = None;
        self.drafting_ui.generation = self.drafting_ui.generation.saturating_add(1);
        self.refresh_workspace_visibility(cx);
        Ok(
            json!({"selection":owner.map(|o|o.value(self.drafting_ui.active_drawing.expect("drawing set"))),
            "candidates":self.drafting_ui.candidate_owners.iter().map(|o|o.value(self.drafting_ui.active_drawing.expect("drawing set"))).collect::<Vec<_>>() }),
        )
    }

    fn sync_drafting_pointer(&mut self) -> Result<(), String> {
        let world = refresh_controller_pointer(
            &mut self.drafting_ui.controller,
            &self.drafting_ui.camera,
            &mut self.drafting_ui.pointer,
        );
        let owner = if let (Some(point), Some(_)) = (self.drafting_ui.pointer, world) {
            self.drafting_pick_candidates(point)?.first().copied()
        } else {
            self.drafting_ui.controller.clear_pointer();
            None
        };
        self.drafting_ui.hover_id = match owner {
            Some(PickOwner::Draft(id)) => Some(id),
            _ => None,
        };
        self.drafting_ui.hover_bim = match owner {
            Some(PickOwner::Bim(id)) => Some(id),
            _ => None,
        };
        Ok(())
    }

    pub(super) fn drawing_native_input(
        &mut self,
        cx: &mut Cx,
        input: CanvasInput,
        origin: &str,
    ) -> Result<Value, String> {
        self.record_drafting_action();
        if !self.drafting_ui.active_2d {
            return Err("2D workspace is inactive".into());
        }
        let before = self.document.as_ref().map_or(0, Document::revision);
        self.drafting_ui.last_origin = origin.into();
        if origin == "physical" {
            match input {
                CanvasInput::Pointer { .. } | CanvasInput::Scroll { .. } => {
                    self.drafting_ui.physical_pointer_events += 1
                }
                _ => self.drafting_ui.physical_keyboard_events += 1,
            }
        } else {
            self.drafting_ui.handler_events += 1;
        }
        let result = match input {
            CanvasInput::Pointer {
                phase,
                button,
                screen,
            } => {
                if !screen.is_finite() {
                    return Err("pointer coordinates must be finite".into());
                }
                if phase == PointerPhase::Leave {
                    self.drafting_ui.pointer = None;
                    self.drafting_ui.controller.clear_pointer();
                    self.drafting_ui.hover_id = None;
                    self.drafting_ui.hover_bim = None;
                    Ok(json!({"left":true}))
                } else if button == PointerButton::Middle || button == PointerButton::Right {
                    match phase {
                        PointerPhase::Down => self.drafting_ui.pan_anchor = Some(screen),
                        PointerPhase::Move => {
                            if let Some(last) = self.drafting_ui.pan_anchor.replace(screen) {
                                self.drawing_pan(cx, screen.x - last.x, screen.y - last.y)?;
                            }
                        }
                        PointerPhase::Up => self.drafting_ui.pan_anchor = None,
                        _ => {}
                    }
                    self.drafting_ui.pointer = Some(screen);
                    self.sync_drafting_pointer()?;
                    Ok(json!({"camera_only":true}))
                } else {
                    self.drafting_ui.pointer = Some(screen);
                    self.sync_drafting_pointer()?;
                    if phase == PointerPhase::Down {
                        if self.drafting_ui.controller.tool != Tool::Select {
                            self.capture_drafting_intent()?;
                        }
                        let world = self.drafting_ui.camera.screen_to_world(screen);
                        let effect = self.drafting_ui.controller.click(world)?;
                        self.apply_drafting_effect(cx, effect)
                    } else {
                        Ok(json!({"pointer_only":true}))
                    }
                }
            }
            CanvasInput::Scroll { screen, delta_y } => {
                if !delta_y.is_finite() {
                    return Err("scroll delta must be finite".into());
                }
                self.drawing_zoom(cx, screen, (-delta_y * 0.002).clamp(-4.0, 4.0).exp())?;
                self.drafting_ui.pointer = Some(screen);
                self.drafting_ui.camera_change = Some(CanvasCameraChange::Zoom);
                self.sync_drafting_pointer()?;
                Ok(json!({"camera_only":true}))
            }
            CanvasInput::Cancel => {
                self.drafting_ui.controller.cancel();
                self.drafting_ui.intent = None;
                Ok(json!({"cancelled":true}))
            }
            CanvasInput::Confirm => {
                self.drafting_intent_current()?;
                let effect = self.drafting_ui.controller.confirm()?;
                self.apply_drafting_effect(cx, effect)
            }
            CanvasInput::Delete => self.apply_drafting_effect(cx, ControllerEffect::Delete),
        };
        if let Err(error) = &result {
            self.drafting_ui.status = error.clone();
        }
        self.drafting_ui.generation = self.drafting_ui.generation.saturating_add(1);
        self.refresh_drafting(cx, true);
        result.map(|value|json!({"origin":origin,"revision_before":before,"revision_after":self.document.as_ref().map_or(0,Document::revision),"result":value}))
    }

    pub(super) fn drawing_fit(&mut self, cx: &mut Cx) -> Result<Value, String> {
        self.synchronize_drafting_cache()?;
        let document = self.document.as_ref().ok_or("no document")?;
        let mut bounds = self
            .drafting_ui
            .cache
            .as_ref()
            .ok_or("no cache")?
            .bounds(document, self.api_document_generation, true)
            .map_err(|e| e.to_string())?;
        if let Some(underlay) = &self.drafting_ui.underlay {
            for segment in underlay.segments.iter() {
                let b = Bounds2::from_points(segment.start, segment.end);
                bounds = Some(bounds.map_or(b, |old| old.union(b)));
            }
        }
        let mut proposed = self.drafting_ui.camera.clone();
        if let Some(bounds) = bounds {
            proposed
                .fit_bounds(bounds, 24.0)
                .map_err(|e| e.to_string())?;
        }
        self.apply_drafting_camera(cx, proposed, CanvasCameraChange::Fit)
    }

    pub(super) fn drawing_set_camera(
        &mut self,
        cx: &mut Cx,
        mut camera: Camera2D,
    ) -> Result<Value, String> {
        camera.validate().map_err(|e| e.to_string())?;
        camera.revision = self
            .drafting_ui
            .camera
            .revision
            .checked_add(1)
            .ok_or("camera revision exhausted")?;
        self.apply_drafting_camera(cx, camera, CanvasCameraChange::Other)
    }

    pub(super) fn drawing_pan(&mut self, cx: &mut Cx, dx: f64, dy: f64) -> Result<Value, String> {
        let mut proposed = self.drafting_ui.camera.clone();
        proposed.pan_pixels(dx, dy).map_err(|e| e.to_string())?;
        self.apply_drafting_camera(cx, proposed, CanvasCameraChange::Pan)
    }

    pub(super) fn drawing_zoom(
        &mut self,
        cx: &mut Cx,
        screen: DraftPoint,
        factor: f64,
    ) -> Result<Value, String> {
        let mut proposed = self.drafting_ui.camera.clone();
        proposed
            .zoom_at(screen, factor)
            .map_err(|e| e.to_string())?;
        self.apply_drafting_camera(cx, proposed, CanvasCameraChange::Zoom)
    }

    fn apply_drafting_camera(
        &mut self,
        cx: &mut Cx,
        camera: Camera2D,
        change: CanvasCameraChange,
    ) -> Result<Value, String> {
        camera.validate().map_err(|e| e.to_string())?;
        let previous = self.drafting_ui.camera.clone();
        let previous_pointer = self.drafting_ui.controller.pointer;
        let previous_hover = (self.drafting_ui.hover_id, self.drafting_ui.hover_bim);
        if self.drafting_ui.active_drawing.is_some() {
            self.synchronize_drafting_cache()?;
            let document = self.document.as_ref().ok_or("no document")?;
            let options = DisplayOptions {
                generation: self.drafting_ui.generation,
                selected_ids: self.drafting_ui.selected_ids.clone(),
                hover_id: self.drafting_ui.hover_id,
                dpi_factor: self.drafting_ui.dpi_factor,
                ..Default::default()
            };
            stage_camera_view(
                self.drafting_ui.cache.as_mut().ok_or("no cache")?,
                document,
                self.api_document_generation,
                &mut self.drafting_ui.camera,
                camera,
                &options,
            )?;
        } else {
            self.drafting_ui.camera = camera;
        }
        if let Err(error) = self.sync_drafting_pointer() {
            self.drafting_ui.camera = previous;
            self.drafting_ui.controller.pointer = previous_pointer;
            self.drafting_ui.hover_id = previous_hover.0;
            self.drafting_ui.hover_bim = previous_hover.1;
            let _ = self.prepare_drafting_cache();
            return Err(error);
        }
        self.record_drafting_action();
        self.drafting_ui.camera_change = Some(change);
        self.refresh_drafting(cx, true);
        Ok(json!({"camera":self.drafting_ui.camera,"document_mutated":false}))
    }

    pub(super) fn drafting_capture_export(&mut self) -> Result<CapturedExport, String> {
        let frame = self.prepare_drafting_cache()?;
        let document = self.document.as_ref().ok_or("no document")?;
        self.drafting_ui
            .cache
            .as_mut()
            .ok_or("no cache")?
            .materialize_export(document, self.api_document_generation, &frame)
            .map_err(|e| e.to_string())
    }

    pub(super) fn drawing_performance(&self) -> Value {
        json!({"cache":self.drafting_ui.cache.as_ref().map(DraftingCache::performance),"native":self.drafting_ui.native_performance,
            "scopes":{"cpu_prepare":"derived cache + screen segment preparation","gpu_canvas":"actual isolated child texture pass","composition":"excluded","present":"not measured","api_round_trip":"caller measured"}})
    }

    fn record_drafting_action(&mut self) {
        self.drafting_ui.action_sequence = self.drafting_ui.action_sequence.saturating_add(1);
        self.drafting_ui.pending_action = Some((self.drafting_ui.action_sequence, Instant::now()));
    }

    pub(super) fn drawing_ui_state(&self, cx: &Cx) -> Result<Value, String> {
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        let widget = canvas.borrow();
        let bounds=widget.as_ref().map(|w|json!({"x":w.rect.pos.x,"y":w.rect.pos.y,"width":w.rect.size.x,"height":w.rect.size.y,"coordinate_space":"window_logical_pixels"}));
        let draw=widget.as_ref().map(|w|json!({"count":w.telemetry.draw_count,"snapshot_token":w.telemetry.snapshot_token,
            "model_generation":w.telemetry.model_generation,"document_revision":w.telemetry.document_revision,"camera_revision":w.telemetry.camera_revision,
            "submitted_segments":w.telemetry.submitted_segments,"retained_model_segments":w.telemetry.retained_model_segments,"custom_instance_payload_bytes":w.telemetry.instance_payload_bytes}));
        let drawing = self.drafting_ui.active_drawing;
        let selection = drawing
            .map(|drawing| {
                self.drafting_ui
                    .selected_ids
                    .iter()
                    .map(|id| PickOwner::Draft(*id).value(drawing))
                    .chain(
                        self.drafting_ui
                            .selected_bim
                            .map(|id| PickOwner::Bim(id).value(drawing)),
                    )
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(
            json!({"active_workspace":if self.drafting_ui.active_2d{"2d"}else{"3d"},"active_drawing_id":drawing,
            "camera":self.drafting_ui.camera,"selection":selection,"selected_ids":self.drafting_ui.selected_ids,"hover_id":self.drafting_ui.hover_id,
            "hover_bim_element_id":self.drafting_ui.hover_bim.map(ElementId::get),"active_layer_id":self.drafting_ui.active_layer,
            "tool":self.drafting_ui.controller.tool.name(),"prompt":self.drafting_ui.controller.prompt(),"pending":self.drafting_ui.controller.is_pending(),
            "ui_mutation_context":{"selection":self.drafting_ui.selection_intent,"geometry_editor":self.drafting_ui.inspector_intent,
                "geometry_editor_dirty":self.drafting_ui.inspector_dirty,"layer":self.drafting_ui.layer_intent},
            "points":self.drafting_ui.controller.points,"preview_geometry":self.drafting_ui.controller.preview(),"segment_bulge":self.drafting_ui.controller.segment_bulge,
            "pointer_screen":self.drafting_ui.pointer,"pointer_world":self.drafting_ui.controller.pointer,"construction_context":self.drafting_ui.intent,
            "canvas_bounds":bounds,"dpi_factor":widget.as_ref().map(|w|w.dpi_factor),"draw":draw,"document_epoch":self.api_document_generation,
            "last_origin":self.drafting_ui.last_origin,"event_counts":{"physical_pointer":self.drafting_ui.physical_pointer_events,"physical_keyboard":self.drafting_ui.physical_keyboard_events,"native_handler":self.drafting_ui.handler_events},
            "native_performance":self.drafting_ui.native_performance,"underlay":self.drafting_ui.underlay.as_ref().map(|f|&f.status),
            "single_window":true,"native_frame_id":self.drafting_ui.native_frame_id}),
        )
    }

    pub(super) fn after_drafting_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        started: Instant,
        allocations: drafting_allocation_probe::AllocationSnapshot,
    ) {
        if matches!(event, Event::Timer(_) | Event::NextFrame(_)) {
            let polled = self.drafting_ui.native_profiler.poll();
            if !polled.is_null() {
                self.drafting_ui.native_performance["gpu"] = polled;
            }
        }
        if !self.drafting_ui.active_2d || !matches!(event, Event::Draw(_)) {
            return;
        }
        let cache_view = self.ui.view(cx, ids!(drafting_canvas_cache));
        let target = cache_view.cached_texture_id().map(|id| format!("{id:?}"));
        let compositor = cache_view
            .borrow()
            .map(|v| v.draw_bg.area())
            .unwrap_or(Area::Empty);
        let canvas = self.ui.drafting_canvas(cx, ids!(drafting_canvas));
        let Some(widget) = canvas.borrow() else {
            return;
        };
        self.drafting_ui.native_frame_id = self.drafting_ui.native_frame_id.saturating_add(1);
        let Some(source) = self.drafting_ui.source.clone() else {
            return;
        };
        let metadata = json!({"frame_id":self.drafting_ui.native_frame_id,"draw_count":widget.telemetry.draw_count,
            "snapshot_token":widget.telemetry.snapshot_token,"document_revision":widget.telemetry.document_revision,
            "camera_revision":widget.telemetry.camera_revision,"source_generation":widget.telemetry.source_generation,
            "model_generation":widget.telemetry.model_generation,"document_epoch":source.document_epoch,"runtime_instance_id":source.runtime_instance_id,
            "cached_target_id":target,"diagnostic_upload_active":false,"drawing_id":source.drawing_id});
        let areas = widget.native_draw_areas();
        let pass = widget.native_pass_area();
        let pending_action = self.drafting_ui.pending_action.take();
        let cpu = json!({"frame_prepare_ms":widget.telemetry.cpu_frame_prepare_ms,"instance_prepare_ms":widget.telemetry.cpu_instance_prepare_ms,
            "batch_submit_ms":widget.telemetry.cpu_submit_ms,"native_draw_event_ms":started.elapsed().as_secs_f64()*1000.0,
            "action_id":pending_action.map(|(id,_)|id),"native_action_to_frame_submission_ms":pending_action.map(|(_,at)|at.elapsed().as_secs_f64()*1000.0),
            "interdraw_interval_ms":widget.telemetry.interdraw_interval_ms,"scope":"native event through UI submission; not FPS or Present"});
        let drew = self.drafting_ui.last_draw_count != widget.telemetry.draw_count;
        self.drafting_ui.last_draw_count = widget.telemetry.draw_count;
        drop(widget);
        let gpu = self.drafting_ui.native_profiler.profile(
            cx,
            pass,
            compositor,
            &[areas.0, areas.1],
            metadata.clone(),
        );
        let sample = json!({"metadata":metadata,"cpu":cpu,"gpu":gpu,"canvas_drew":drew,
            "allocations":drafting_allocation_probe::delta(allocations,drafting_allocation_probe::snapshot()).to_json()});
        self.drafting_ui.native_samples.push_back(sample.clone());
        if self.drafting_ui.native_samples.len() > 128 {
            self.drafting_ui.native_samples.pop_front();
        }
        self.drafting_ui.native_performance = json!({"latest":sample,"samples":self.drafting_ui.native_samples,
            "scope":"Canvas child texture pass; parent composition and Present excluded"});
    }

    pub(super) fn handle_drafting_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.circular_ui.panel.active { return; }
        if self.geotech_ui.active { return; }
        if self
            .ui
            .text_input(cx, ids!(draft_geometry_input))
            .changed(actions)
            .is_some()
        {
            // Keep the source captured when this editor was displayed. External
            // commits must neither overwrite typed geometry nor rebase its intent.
            self.drafting_ui.inspector_dirty = true;
        }
        if self.ui.button(cx, ids!(workspace_3d)).clicked(actions) {
            let _ = self.set_workspace(cx, "3d");
        }
        if self.ui.button(cx, ids!(workspace_2d)).clicked(actions) {
            let _ = self.set_workspace(cx, "2d");
        }
        let mut result: Option<Result<Value, String>> = None;
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(drawing_selector))
            .selected(actions)
        {
            if let Some(id) = self.drafting_ui.drawing_options.get(index).copied() {
                result = Some(self.activate_drawing(cx, id));
            }
        }
        for (row, widget_id) in DRAWING_ROWS.iter().enumerate() {
            if self.ui.button(cx, &[*widget_id]).clicked(actions) {
                if let Some(id) = self
                    .drafting_ui
                    .drawing_options
                    .get(self.drafting_ui.drawing_page * 4 + row)
                    .copied()
                {
                    result = Some(self.activate_drawing(cx, id));
                }
            }
        }
        if self
            .ui
            .button(cx, ids!(drawing_tree_previous))
            .clicked(actions)
        {
            self.drafting_ui.drawing_page = self.drafting_ui.drawing_page.saturating_sub(1);
            self.drafting_ui.navigation_revision = None;
            self.refresh_drafting(cx, true);
        }
        if self.ui.button(cx, ids!(drawing_tree_next)).clicked(actions) {
            self.drafting_ui.drawing_page = self.drafting_ui.drawing_page.saturating_add(1);
            self.drafting_ui.navigation_revision = None;
            self.refresh_drafting(cx, true);
        }
        if self
            .ui
            .button(cx, ids!(drawing_view_reference))
            .clicked(actions)
        {
            if let Some(id) = self.drafting_ui.active_drawing {
                result = Some(self.activate_drawing(cx, id));
            }
        }
        if self.ui.button(cx, ids!(draft_new_drawing)).clicked(actions) {
            let name = self.ui.text_input(cx, ids!(drawing_name_input)).text();
            result = Some(self.commit_drafting_command(
                cx,
                "drawing-create",
                DraftCommand::CreateDrawing {
                    name,
                    plane: DrawingPlane::default(),
                },
            ));
        }
        if self
            .ui
            .button(cx, ids!(draft_rename_drawing))
            .clicked(actions)
        {
            if let Some(id) = self.drafting_ui.active_drawing {
                let name = self.ui.text_input(cx, ids!(drawing_name_input)).text();
                result = Some(self.commit_drafting_command(
                    cx,
                    "drawing-rename",
                    DraftCommand::RenameDrawing {
                        drawing_id: id,
                        name,
                    },
                ));
            }
        }
        if self
            .ui
            .button(cx, ids!(draft_bim_reference))
            .clicked(actions)
        {
            if let Some(id) = self.drafting_ui.active_drawing {
                result = Some(self.create_default_bim_reference(cx, id));
            }
        }
        for (widget, tool) in [
            (ids!(draft_select), "select"),
            (ids!(draft_line), "line"),
            (ids!(draft_circle), "circle"),
            (ids!(draft_arc), "arc"),
            (ids!(draft_polyline), "polyline"),
            (ids!(draft_move), "move"),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                result = Some(self.drawing_tool(cx, tool));
            }
        }
        for (widget, input) in [
            (ids!(draft_delete), CanvasInput::Delete),
            (ids!(draft_confirm), CanvasInput::Confirm),
            (ids!(draft_cancel), CanvasInput::Cancel),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                result = Some(self.drawing_native_input(cx, input, "native_toolbar"));
            }
        }
        if self.ui.button(cx, ids!(draft_fit)).clicked(actions) {
            result = Some(self.drawing_fit(cx));
        }
        if let Some((text, _)) = self
            .ui
            .text_input(cx, ids!(draft_numeric_input))
            .returned(actions)
        {
            // This action comes from the actual TextInput Return event. API calls
            // use the same controller helper without claiming physical input.
            self.drafting_ui.last_origin = "native_text_input".into();
            self.drafting_ui.physical_keyboard_events =
                self.drafting_ui.physical_keyboard_events.saturating_add(1);
            let submitted = self.drawing_numeric(cx, &text);
            if submitted.is_ok() {
                self.ui
                    .text_input(cx, ids!(draft_numeric_input))
                    .set_text(cx, "");
            }
            result = Some(submitted);
        }
        for (widget, closed) in [
            (ids!(draft_finish_polyline), false),
            (ids!(draft_close_polyline), true),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                result = Some(
                    self.drafting_ui
                        .controller
                        .finish_polyline(closed)
                        .map(|_| json!({"preview_only":true})),
                );
                self.refresh_drafting(cx, true);
            }
        }
        if self.ui.button(cx, ids!(draft_set_bulge)).clicked(actions) {
            let value = self
                .ui
                .text_input(cx, ids!(draft_bulge_input))
                .text()
                .parse::<f64>()
                .map_err(|_| "Введите конечный bulge".to_owned());
            result = Some(
                value
                    .and_then(|value| self.drafting_ui.controller.set_bulge(value))
                    .map(|_| json!({"preview_only":true})),
            );
            self.refresh_drafting(cx, true);
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(draft_layer_selector))
            .selected(actions)
        {
            self.drafting_ui.active_layer = self.drafting_ui.layer_options.get(index).copied();
            self.refresh_drafting_inspector(cx);
        }
        for (widget, visibility) in [
            (ids!(draft_layer_visible), true),
            (ids!(draft_layer_locked), false),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                if let (Some(drawing), Some(layer)) = (
                    self.drafting_ui.active_drawing,
                    self.drafting_ui.active_layer,
                ) {
                    if let Some(value) = self
                        .document
                        .as_ref()
                        .and_then(|d| d.drawing(drawing))
                        .and_then(|d| d.layers.get(&layer))
                    {
                        let command = DraftCommand::UpdateDraftLayer {
                            drawing_id: drawing,
                            layer_id: layer,
                            name: None,
                            visible: visibility.then_some(!value.visible),
                            locked: (!visibility).then_some(!value.locked),
                            style: None,
                        };
                        result = Some(self.commit_drafting_command(cx, "layer-update", command));
                    }
                }
            }
        }
        if self
            .ui
            .button(cx, ids!(draft_select_by_id))
            .clicked(actions)
        {
            let value = self.ui.text_input(cx, ids!(draft_select_id)).text();
            result = Some(
                value
                    .trim()
                    .parse::<u64>()
                    .map(DraftEntityId::new)
                    .map_err(|_| "Введите десятичный DraftEntity ID".into())
                    .and_then(|id| self.drawing_select_ids(cx, vec![id])),
            );
        }
        if self
            .ui
            .button(cx, ids!(draft_apply_geometry))
            .clicked(actions)
        {
            if let (Some(drawing), Some(entity)) = (
                self.drafting_ui.active_drawing,
                self.drafting_ui.selected_ids.first().copied(),
            ) {
                let input = self.ui.text_input(cx, ids!(draft_geometry_input)).text();
                result = Some(
                    serde_json::from_str::<Geometry>(&input)
                        .map_err(|e| e.to_string())
                        .and_then(|geometry| {
                            self.commit_drafting_command(
                                cx,
                                "geometry-edit",
                                DraftCommand::UpdateDraftEntity {
                                    drawing_id: drawing,
                                    entity_id: entity,
                                    geometry,
                                },
                            )
                        }),
                );
            }
        }
        if self
            .ui
            .button(cx, ids!(draft_open_bim_source))
            .clicked(actions)
        {
            if let Some(id) = self.drafting_ui.selected_bim {
                self.select_element(cx, id);
                result = Some(self.set_workspace(cx, "3d"));
            }
        }
        for (widget, kind) in [
            (ids!(draft_export_png), "png"),
            (ids!(draft_export_svg), "svg"),
            (ids!(draft_export_dxf), "dxf"),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                let base = PathBuf::from(self.path_text(cx));
                let stem = base
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("drawing");
                let path = base.with_file_name(format!(
                    "{stem}-drawing-{}.{}",
                    self.drafting_ui
                        .active_drawing
                        .map_or("none".into(), |id| id.to_string()),
                    kind
                ));
                result = Some(self.drawing_export_file(cx, kind, &path));
            }
        }
        let canvas_actions = self
            .ui
            .drafting_canvas(cx, ids!(drafting_canvas))
            .actions(actions);
        for action in canvas_actions {
            match action {
                CanvasAction::Input(input) => {
                    result = Some(self.drawing_native_input(cx, input, "physical"));
                }
                CanvasAction::Resize {
                    width,
                    height,
                    dpi_factor,
                } => {
                    if width > 1.0 && height > 1.0 {
                        if let Err(error) = self.drafting_ui.camera.resize(width, height) {
                            result = Some(Err(error.to_string()));
                        }
                        self.drafting_ui.camera_change = Some(CanvasCameraChange::Resize);
                        self.drafting_ui.dpi_factor = dpi_factor;
                        self.refresh_drafting(cx, true);
                    }
                }
                CanvasAction::None => {}
            }
        }
        if let Some(Err(error)) = result {
            self.drafting_ui.status = error.clone();
            self.set_status(cx, &error);
        }
    }
}

fn require_ui_intent(
    intent: &CacheSourceKey,
    document: &(impl DrawingSource + ?Sized),
    drawing: DrawingId,
    epoch: u64,
) -> Result<(), String> {
    let current = document
        .cache_source_key(drawing, epoch)
        .map_err(|e| e.to_string())?;
    if intent != &current {
        return Err(
            "stale_ui_intent: документ или ревизия изменены; отмените preview и повторите".into(),
        );
    }
    Ok(())
}

fn commit_ui_mutation(
    document: &mut Document,
    epoch: u64,
    context: &UiMutationContext,
    command_id: String,
    command: DraftCommand,
) -> Result<ai_revit_core::CommitReceipt, String> {
    require_ui_intent(&context.source, document, context.source.drawing_id, epoch)?;
    let (drawing, ids_match) = match &command {
        DraftCommand::CreateDraftEntity {
            drawing_id,
            layer_id,
            ..
        } => (*drawing_id, context.layer_id == Some(*layer_id)),
        DraftCommand::MoveDraftEntity {
            drawing_id,
            entity_ids,
            ..
        }
        | DraftCommand::DeleteDraftEntity {
            drawing_id,
            entity_ids,
        } => (
            *drawing_id,
            !entity_ids.is_empty() && context.entity_ids == *entity_ids,
        ),
        DraftCommand::UpdateDraftEntity {
            drawing_id,
            entity_id,
            ..
        } => (*drawing_id, context.entity_ids.as_slice() == [*entity_id]),
        DraftCommand::UpdateDraftLayer {
            drawing_id,
            layer_id,
            ..
        } => (*drawing_id, context.layer_id == Some(*layer_id)),
        _ => return Err("unsupported guarded UI mutation".into()),
    };
    if drawing != context.source.drawing_id || !ids_match {
        return Err(
            "stale_ui_intent: целевой чертёж, объект или слой изменён; выберите заново".into(),
        );
    }
    let mut transaction = document.begin_transaction(command_id, context.source.document_revision);
    transaction.push(Command::Draft(command));
    document
        .commit(transaction)
        .map_err(|error| error.to_string())
}

/// This is the camera commit boundary used by native and API navigation. Failed
/// tessellation leaves the caller's camera unchanged, including its revision.
fn stage_camera_view(
    cache: &mut DraftingCache,
    document: &(impl DrawingSource + ?Sized),
    epoch: u64,
    current: &mut Camera2D,
    proposed: Camera2D,
    options: &DisplayOptions,
) -> Result<Arc<CachedFrame>, String> {
    proposed.validate().map_err(|e| e.to_string())?;
    let frame = cache
        .prepare(document, epoch, &proposed, options)
        .map_err(|e| e.to_string())?;
    *current = proposed;
    Ok(frame)
}

fn pick_manual_semantics(
    cache: &mut DraftingCache,
    document: &(impl DrawingSource + ?Sized),
    epoch: u64,
    camera: &Camera2D,
    screen: DraftPoint,
) -> Result<Vec<drafting_core::PickHit>, String> {
    cache
        .pick(document, epoch, camera, screen, 7.0)
        .map_err(|error| error.to_string())
}

fn refresh_controller_pointer(
    controller: &mut Controller,
    camera: &Camera2D,
    screen: &mut Option<DraftPoint>,
) -> Option<DraftPoint> {
    *screen = screen.filter(|point| {
        point.is_finite()
            && point.x >= 0.0
            && point.y >= 0.0
            && point.x <= camera.viewport_width
            && point.y <= camera.viewport_height
    });
    if let Some(point) = screen
        .map(|point| camera.screen_to_world(point))
        .filter(|point| point.is_finite())
    {
        controller.hover(point);
        Some(point)
    } else {
        controller.clear_pointer();
        None
    }
}

/// A matching old Draw token is insufficient when preparation failed or a
/// newer source/camera/underlay is waiting for Draw. Check the exact binding
/// before any API controller action can publish transient state or mutate Core.
fn require_current_native_snapshot(
    requested: &str,
    prepared: Option<&CachedFrame>,
    current_source: &CacheSourceKey,
    current_camera: &Camera2D,
    current_dpi_factor: f64,
    underlay_generation: u64,
    drawn: &CanvasTelemetry,
) -> Result<(), String> {
    let stale = || "stale_ui_context: native snapshot is not the current prepared and drawn frame".to_owned();
    let prepared = prepared.ok_or_else(stale)?;
    let native_token = format!("{}:bim:{}", prepared.token, underlay_generation);
    if prepared.source != *current_source
        || prepared.camera != *current_camera
        || prepared.dpi_factor != current_dpi_factor
        || drawn.draw_count == 0
        || requested != native_token
        || drawn.snapshot_token != native_token
        || drawn.document_revision != prepared.source.document_revision
        || drawn.camera_revision != prepared.camera.revision
        || drawn.source_generation != prepared.generations.geometry
        || drawn.model_generation != prepared.generations.geometry
    {
        return Err(stale());
    }
    Ok(())
}

fn preview_segments(
    geometries: &[Geometry],
    camera: &Camera2D,
    options: &DisplayOptions,
) -> Result<Vec<ScreenSegment>, String> {
    if geometries.len() > MAX_PREVIEW_ENTITIES {
        return Err("preview_budget: at most 256 entities".into());
    }
    let mut preview = Vec::new();
    for geometry in geometries {
        let mut bounded = options.clone();
        bounded.max_primitives = MAX_PREVIEW_SEGMENTS.saturating_sub(preview.len());
        if bounded.max_primitives == 0 {
            return Err("preview_budget: segment budget exceeded".into());
        }
        preview.extend(
            tessellate_geometry(geometry, camera, &bounded)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|segment| ScreenSegment {
                    start: segment.start,
                    end: segment.end,
                    color: [1.0, 0.67, 0.28, 0.95],
                    width: 2.0,
                }),
        );
    }
    Ok(preview)
}
fn segment_distance(p: DraftPoint, a: DraftPoint, b: DraftPoint) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx * dx + dy * dy;
    if length <= f64::EPSILON {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0);
    p.distance(DraftPoint::new(a.x + t * dx, a.y + t * dy))
}
fn segment_in_view(a: DraftPoint, b: DraftPoint, camera: &Camera2D) -> bool {
    a.is_finite()
        && b.is_finite()
        && a.x.max(b.x) >= -8.0
        && a.y.max(b.y) >= -8.0
        && a.x.min(b.x) <= camera.viewport_width + 8.0
        && a.y.min(b.y) <= camera.viewport_height + 8.0
}

pub(super) fn command_label(command: &DraftCommand) -> &'static str {
    match command {
        DraftCommand::CreateDrawing { .. } => "Создание чертежа",
        DraftCommand::RenameDrawing { .. } => "Имя чертежа",
        DraftCommand::CreateDraftEntity { .. } => "Создание геометрии",
        DraftCommand::UpdateDraftEntity { .. } => "Изменение геометрии",
        DraftCommand::MoveDraftEntity { .. } => "Перемещение",
        DraftCommand::DeleteDraftEntity { .. } => "Удаление геометрии",
        DraftCommand::CreateDraftLayer { .. } => "Создание слоя",
        DraftCommand::UpdateDraftLayer { .. } => "Изменение слоя",
        DraftCommand::CreateViewDefinition { .. } => "BIM reference",
        DraftCommand::UpdateViewDefinition { .. } => "Изменение BIM reference",
        DraftCommand::ImportDraftingEntities { .. } => "Импорт CAD",
        DraftCommand::ImportLabDocument { .. } => "Импорт Drafting Lab",
    }
}

#[cfg(test)]
mod unified_workspace_regressions {
    use super::*;

    fn commit(
        document: &mut Document,
        name: &str,
        command: DraftCommand,
    ) -> ai_revit_core::CommitReceipt {
        let mut transaction = document.begin_transaction(name, document.revision());
        transaction.push(Command::Draft(command));
        document.commit(transaction).unwrap()
    }
    fn fixture() -> (Document, DrawingId, DraftLayerId) {
        let mut document = Document::new("Unified UI contract").unwrap();
        let receipt = commit(
            &mut document,
            "drawing",
            DraftCommand::CreateDrawing {
                name: "Plan".into(),
                plane: DrawingPlane::default(),
            },
        );
        let drawing = receipt.created_drawings[0];
        let layer = *document
            .drawing(drawing)
            .unwrap()
            .layers
            .keys()
            .next()
            .unwrap();
        (document, drawing, layer)
    }
    fn add_line(
        document: &mut Document,
        drawing: DrawingId,
        layer: DraftLayerId,
        name: &str,
        x: f64,
    ) -> DraftEntityId {
        commit(
            document,
            name,
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: Geometry::Line {
                    start: DraftPoint::new(0.0, 0.0),
                    end: DraftPoint::new(x, 1.0),
                },
                style: DraftStyle::default(),
            },
        )
        .created_draft_entities[0]
    }

    fn snapshot_frame(document: &Document, drawing: DrawingId, camera: &Camera2D) -> Arc<CachedFrame> {
        let mut cache = DraftingCache::new(Default::default()).unwrap();
        cache.reset(document, drawing, 2).unwrap();
        cache
            .prepare(document, 2, camera, &DisplayOptions::default())
            .unwrap()
    }

    fn drawn_snapshot(frame: &CachedFrame, underlay: u64) -> CanvasTelemetry {
        CanvasTelemetry {
            draw_count: 1,
            snapshot_token: format!("{}:bim:{}", frame.token, underlay),
            document_revision: frame.source.document_revision,
            camera_revision: frame.camera.revision,
            source_generation: frame.generations.geometry,
            model_generation: frame.generations.geometry,
            ..Default::default()
        }
    }

    #[test]
    fn native_snapshot_guard_rejects_failed_prepare_before_draw() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "baseline", 2.0);
        let camera = Camera2D::default();
        let old_frame = snapshot_frame(&document, drawing, &camera);
        let old_draw = drawn_snapshot(&old_frame, 7);
        commit(
            &mut document,
            "over-budget-circle",
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: Geometry::Circle { center: DraftPoint::new(0., 0.), radius: 1e8 },
                style: DraftStyle::default(),
            },
        );
        let mut cache = DraftingCache::new(Default::default()).unwrap();
        cache.reset(&document, drawing, 2).unwrap();
        let failure = cache.prepare(&document, 2, &camera, &DisplayOptions::default()).unwrap_err();
        assert!(failure.to_string().contains("tessellation"), "{failure}");
        let before = document.to_json_string().unwrap();
        let current = document.cache_source_key(drawing, 2).unwrap();
        let error = require_current_native_snapshot(
            &old_draw.snapshot_token, None, &current, &camera, 1., 7, &old_draw,
        ).unwrap_err();
        assert!(error.starts_with("stale_ui_context"));
        assert_eq!(document.to_json_string().unwrap(), before);
        assert_eq!(old_draw.document_revision, old_frame.source.document_revision);
        assert_ne!(old_draw.document_revision, current.document_revision);
    }

    #[test]
    fn native_snapshot_guard_waits_for_new_prepared_frame_to_be_drawn() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "baseline", 2.0);
        let camera = Camera2D::default();
        let old_frame = snapshot_frame(&document, drawing, &camera);
        let old_draw = drawn_snapshot(&old_frame, 7);
        add_line(&mut document, drawing, layer, "new-source", 4.0);
        let new_frame = snapshot_frame(&document, drawing, &camera);
        let new_draw = drawn_snapshot(&new_frame, 7);
        assert_ne!(old_draw.snapshot_token, new_draw.snapshot_token);
        for token in [&old_draw.snapshot_token, &new_draw.snapshot_token] {
            assert!(require_current_native_snapshot(
                token, Some(&new_frame), &new_frame.source, &camera, 1., 7, &old_draw,
            ).is_err());
        }
        assert!(require_current_native_snapshot(
            &new_draw.snapshot_token, Some(&new_frame), &new_frame.source, &camera, 1., 7, &new_draw,
        ).is_ok());
    }

    #[test]
    fn native_snapshot_guard_rejects_runtime_or_epoch_replacement() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "baseline", 2.0);
        let camera = Camera2D::default();
        let frame = snapshot_frame(&document, drawing, &camera);
        let drawn = drawn_snapshot(&frame, 7);
        let replacement = document.clone();
        assert_eq!(replacement.document_uuid, document.document_uuid);
        assert_eq!(replacement.revision(), document.revision());
        let runtime_replaced = replacement.cache_source_key(drawing, 2).unwrap();
        assert_ne!(runtime_replaced.runtime_instance_id, frame.source.runtime_instance_id);
        let epoch_replaced = document.cache_source_key(drawing, 3).unwrap();
        for source in [&runtime_replaced, &epoch_replaced] {
            assert!(require_current_native_snapshot(
                &drawn.snapshot_token, Some(&frame), source, &camera, 1., 7, &drawn,
            ).is_err());
        }
    }

    #[test]
    fn native_snapshot_guard_rejects_camera_or_dpi_changes() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "baseline", 2.0);
        let camera = Camera2D::default();
        let frame = snapshot_frame(&document, drawing, &camera);
        let drawn = drawn_snapshot(&frame, 7);
        let mut moved = camera.clone();
        moved.pan_pixels(20., 10.).unwrap();
        assert!(require_current_native_snapshot(
            &drawn.snapshot_token, Some(&frame), &frame.source, &moved, 1., 7, &drawn,
        ).is_err());
        assert!(require_current_native_snapshot(
            &drawn.snapshot_token, Some(&frame), &frame.source, &camera, 1.25, 7, &drawn,
        ).is_err());
    }

    #[test]
    fn native_snapshot_guard_binds_full_bim_token_and_draw_provenance() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "baseline", 2.0);
        let camera = Camera2D::default();
        let frame = snapshot_frame(&document, drawing, &camera);
        let drawn = drawn_snapshot(&frame, 7);
        assert!(require_current_native_snapshot(
            &drawn.snapshot_token, Some(&frame), &frame.source, &camera, 1., 7, &drawn,
        ).is_ok());
        assert!(require_current_native_snapshot(
            &drawn.snapshot_token, Some(&frame), &frame.source, &camera, 1., 8, &drawn,
        ).is_err());
        let mut invalid = vec![drawn.clone(); 5];
        invalid[0].draw_count = 0;
        invalid[1].document_revision += 1;
        invalid[2].camera_revision += 1;
        invalid[3].source_generation += 1;
        invalid[4].model_generation += 1;
        for telemetry in invalid {
            assert!(require_current_native_snapshot(
                &drawn.snapshot_token, Some(&frame), &frame.source, &camera, 1., 7, &telemetry,
            ).is_err());
        }
    }

    fn ui_mutations(
        drawing: DrawingId,
        layer: DraftLayerId,
        entity: DraftEntityId,
    ) -> Vec<DraftCommand> {
        vec![
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: Geometry::Circle {
                    center: DraftPoint::new(2.0, 0.0),
                    radius: 1.0,
                },
                style: DraftStyle::default(),
            },
            DraftCommand::MoveDraftEntity {
                drawing_id: drawing,
                entity_ids: vec![entity],
                delta: DraftPoint::new(5.0, 0.0),
            },
            DraftCommand::DeleteDraftEntity {
                drawing_id: drawing,
                entity_ids: vec![entity],
            },
            DraftCommand::UpdateDraftEntity {
                drawing_id: drawing,
                entity_id: entity,
                geometry: Geometry::Line {
                    start: DraftPoint::new(7.0, 0.0),
                    end: DraftPoint::new(8.0, 1.0),
                },
            },
            DraftCommand::UpdateDraftLayer {
                drawing_id: drawing,
                layer_id: layer,
                name: None,
                visible: Some(false),
                locked: None,
                style: None,
            },
        ]
    }

    #[test]
    fn every_ui_mutation_rejects_replacement_with_same_revision_and_target_ids() {
        let (mut original, drawing, layer) = fixture();
        let entity = add_line(&mut original, drawing, layer, "line", 2.0);
        let context = UiMutationContext {
            source: original.cache_source_key(drawing, 4).unwrap(),
            entity_ids: vec![entity],
            layer_id: Some(layer),
        };
        for command in ui_mutations(drawing, layer, entity) {
            let mut replacement = original.clone();
            assert_eq!(replacement.document_uuid, original.document_uuid);
            assert_eq!(replacement.revision(), original.revision());
            assert_eq!(replacement.drawing(drawing), original.drawing(drawing));
            let before = replacement.to_json_string().unwrap();
            let error = commit_ui_mutation(
                &mut replacement,
                4,
                &context,
                "stale-replacement".into(),
                command,
            )
            .unwrap_err();
            assert!(error.starts_with("stale_ui_intent"), "{error}");
            assert_eq!(replacement.to_json_string().unwrap(), before);
        }
    }

    #[test]
    fn every_ui_mutation_rejects_external_revision_instead_of_rebasing_typed_values() {
        for index in 0..5 {
            let (mut document, drawing, layer) = fixture();
            let entity = add_line(&mut document, drawing, layer, "line", 2.0);
            let context = UiMutationContext {
                source: document.cache_source_key(drawing, 2).unwrap(),
                entity_ids: vec![entity],
                layer_id: Some(layer),
            };
            let command = ui_mutations(drawing, layer, entity).remove(index);
            add_line(&mut document, drawing, layer, "external-edit", 9.0);
            let before = document.to_json_string().unwrap();
            let error =
                commit_ui_mutation(&mut document, 2, &context, "stale-rebase".into(), command)
                    .unwrap_err();
            assert!(error.starts_with("stale_ui_intent"), "{error}");
            assert_eq!(document.to_json_string().unwrap(), before);
        }
    }

    #[test]
    fn guarded_ui_mutation_cannot_retarget_captured_entity_layer_or_drawing() {
        let (mut document, drawing, layer) = fixture();
        let entity = add_line(&mut document, drawing, layer, "line", 2.0);
        let other = add_line(&mut document, drawing, layer, "other", 3.0);
        let context = UiMutationContext {
            source: document.cache_source_key(drawing, 1).unwrap(),
            entity_ids: vec![entity],
            layer_id: Some(layer),
        };
        let before = document.to_json_string().unwrap();
        for command in [
            DraftCommand::DeleteDraftEntity {
                drawing_id: drawing,
                entity_ids: vec![other],
            },
            DraftCommand::UpdateDraftEntity {
                drawing_id: drawing,
                entity_id: other,
                geometry: Geometry::Line {
                    start: DraftPoint::new(0.0, 0.0),
                    end: DraftPoint::new(1.0, 0.0),
                },
            },
            DraftCommand::UpdateDraftLayer {
                drawing_id: drawing,
                layer_id: DraftLayerId::new(layer.get() + 1),
                name: None,
                visible: Some(false),
                locked: None,
                style: None,
            },
            DraftCommand::DeleteDraftEntity {
                drawing_id: DrawingId::new(drawing.get() + 1),
                entity_ids: vec![entity],
            },
        ] {
            assert!(
                commit_ui_mutation(&mut document, 1, &context, "retarget".into(), command)
                    .unwrap_err()
                    .starts_with("stale_ui_intent")
            );
            assert_eq!(document.to_json_string().unwrap(), before);
        }
    }

    #[test]
    fn every_current_ui_mutation_commits_once_through_unified_history() {
        for index in 0..5 {
            let (mut document, drawing, layer) = fixture();
            let entity = add_line(&mut document, drawing, layer, "line", 2.0);
            let context = UiMutationContext {
                source: document.cache_source_key(drawing, 1).unwrap(),
                entity_ids: vec![entity],
                layer_id: Some(layer),
            };
            let command = ui_mutations(drawing, layer, entity).remove(index);
            let before_revision = document.revision();
            let original_geometry = document
                .draft_entity(drawing, entity)
                .unwrap()
                .geometry
                .clone();
            let receipt =
                commit_ui_mutation(&mut document, 1, &context, "current-ui".into(), command)
                    .unwrap();
            assert_eq!(document.revision(), before_revision + 1);
            let created = receipt.created_draft_entities.first().copied();
            document.undo().unwrap();
            assert_eq!(
                document.draft_entity(drawing, entity).unwrap().geometry,
                original_geometry
            );
            assert_eq!(document.drawing(drawing).unwrap().entities.len(), 1);
            assert!(document.drawing(drawing).unwrap().layers[&layer].visible);
            document.redo().unwrap();
            match index {
                0 => assert!(document.draft_entity(drawing, created.unwrap()).is_some()),
                1 | 3 => assert_ne!(
                    document.draft_entity(drawing, entity).unwrap().geometry,
                    original_geometry
                ),
                2 => assert!(document.draft_entity(drawing, entity).is_none()),
                4 => assert!(!document.drawing(drawing).unwrap().layers[&layer].visible),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn construction_intent_rejects_replacement_with_same_persistent_revision_and_ids() {
        let (mut document, drawing, layer) = fixture();
        add_line(&mut document, drawing, layer, "line", 2.0);
        let intent = document.cache_source_key(drawing, 5).unwrap();
        let replacement = document.clone();
        assert_eq!(document.document_uuid, replacement.document_uuid);
        assert_eq!(document.revision(), replacement.revision());
        assert_eq!(document.drawing(drawing), replacement.drawing(drawing));
        assert!(
            require_ui_intent(&intent, &replacement, drawing, 5)
                .unwrap_err()
                .contains("stale_ui_intent")
        );
        assert!(require_ui_intent(&intent, &document, drawing, 5).is_ok());
    }
    #[test]
    fn stale_construction_intent_cannot_be_rebased_to_latest_revision() {
        let (mut document, drawing, layer) = fixture();
        let intent = document.cache_source_key(drawing, 1).unwrap();
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("0 0").unwrap();
        controller.numeric("2 0").unwrap();
        let pending = controller.pending_geometry.clone();
        add_line(&mut document, drawing, layer, "external", 4.0);
        assert!(require_ui_intent(&intent, &document, drawing, 1).is_err());
        assert_eq!(controller.pending_geometry, pending);
        assert_eq!(document.drawing(drawing).unwrap().entities.len(), 1);
    }
    #[test]
    fn stale_construction_intent_rejects_divergent_saved_branches_at_equal_revision() {
        let (document, drawing, layer) = fixture();
        let saved = document.to_json_string().unwrap();
        let mut left = Document::from_json_str(&saved).unwrap();
        let mut right = Document::from_json_str(&saved).unwrap();
        let left_id = add_line(&mut left, drawing, layer, "left", 2.0);
        let right_id = add_line(&mut right, drawing, layer, "right", 5.0);
        assert_eq!(left_id, right_id);
        assert_eq!(left.document_uuid, right.document_uuid);
        assert_eq!(left.revision(), right.revision());
        let intent = left.cache_source_key(drawing, 3).unwrap();
        let reloaded = Document::from_json_str(&right.to_json_string().unwrap()).unwrap();
        assert!(require_ui_intent(&intent, &reloaded, drawing, 3).is_err());
        assert_ne!(
            left.draft_entity(drawing, left_id).unwrap().geometry,
            reloaded.draft_entity(drawing, right_id).unwrap().geometry
        );
    }
    #[test]
    fn document_epoch_blocks_reuse_after_geometry_and_history_roundtrip() {
        let (mut document, drawing, layer) = fixture();
        let entity = add_line(&mut document, drawing, layer, "line", 2.0);
        let geometry = document
            .draft_entity(drawing, entity)
            .unwrap()
            .geometry
            .clone();
        document.undo().unwrap();
        document.redo().unwrap();
        assert_eq!(
            document.draft_entity(drawing, entity).unwrap().geometry,
            geometry
        );
        let current = document.cache_source_key(drawing, 9).unwrap();
        assert!(require_ui_intent(&current, &document, drawing, 10).is_err());
        assert!(require_ui_intent(&current, &document, drawing, 9).is_ok());
    }
    #[test]
    fn preview_budget_reports_failure_without_partial_preview() {
        let geometry = Geometry::Line {
            start: DraftPoint::new(0.0, 0.0),
            end: DraftPoint::new(1.0, 1.0),
        };
        let geometries = vec![geometry; MAX_PREVIEW_ENTITIES + 1];
        assert!(
            preview_segments(
                &geometries,
                &Camera2D::default(),
                &DisplayOptions::default()
            )
            .unwrap_err()
            .contains("preview_budget")
        );
        assert!(
            preview_segments(&[], &Camera2D::default(), &DisplayOptions::default())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn current_controller_intent_uses_unified_history_and_preserves_entity_id() {
        let (mut document, drawing, layer) = fixture();
        let source = document.cache_source_key(drawing, 1).unwrap();
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("0 0").unwrap();
        controller.numeric("3 1").unwrap();
        require_ui_intent(&source, &document, drawing, 1).unwrap();
        let ControllerEffect::Create(geometry) = controller.confirm().unwrap() else {
            panic!("creation")
        };
        let before = document.revision();
        let receipt = commit(
            &mut document,
            "native-line",
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: geometry.clone(),
                style: DraftStyle::default(),
            },
        );
        let id = receipt.created_draft_entities[0];
        assert_eq!(document.revision(), before + 1);
        document.undo().unwrap();
        assert!(document.draft_entity(drawing, id).is_none());
        document.redo().unwrap();
        assert_eq!(
            document.draft_entity(drawing, id).unwrap().geometry,
            geometry
        );
    }
    #[test]
    fn stationary_controller_cursor_reprojects_after_fit_resize_and_camera_changes() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("0 0").unwrap();
        let mut screen = Some(DraftPoint::new(140.0, 40.0));
        let mut camera = Camera2D::new(200.0, 100.0);
        let first = refresh_controller_pointer(&mut controller, &camera, &mut screen).unwrap();
        let points = controller.points.clone();
        camera
            .fit_bounds(
                Bounds2::from_points(DraftPoint::new(-10.0, -5.0), DraftPoint::new(10.0, 5.0)),
                24.0,
            )
            .unwrap();
        let fitted = refresh_controller_pointer(&mut controller, &camera, &mut screen).unwrap();
        assert_ne!(first, fitted);
        camera.resize(400.0, 200.0).unwrap();
        let resized = refresh_controller_pointer(&mut controller, &camera, &mut screen).unwrap();
        assert_ne!(fitted, resized);
        camera.pan_pixels(20.0, -10.0).unwrap();
        let changed = refresh_controller_pointer(&mut controller, &camera, &mut screen).unwrap();
        assert_ne!(resized, changed);
        assert_eq!(
            controller.pointer,
            Some(camera.screen_to_world(screen.unwrap()))
        );
        assert_eq!(controller.points, points);
        assert_eq!(controller.preview().len(), 1);
    }
    #[test]
    fn leaving_or_resizing_past_cursor_clears_live_preview_without_losing_points() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("0 0").unwrap();
        let mut camera = Camera2D::new(200.0, 100.0);
        let mut screen = Some(DraftPoint::new(190.0, 90.0));
        refresh_controller_pointer(&mut controller, &camera, &mut screen);
        assert_eq!(controller.preview().len(), 1);
        let points = controller.points.clone();
        camera.resize(100.0, 50.0).unwrap();
        assert!(refresh_controller_pointer(&mut controller, &camera, &mut screen).is_none());
        assert!(screen.is_none());
        assert!(controller.pointer.is_none());
        assert!(controller.preview().is_empty());
        assert_eq!(controller.points, points);
        screen = None;
        refresh_controller_pointer(&mut controller, &camera, &mut screen);
        assert_eq!(controller.points, points);
    }
    #[test]
    fn failed_camera_display_preflight_preserves_camera_revision_and_semantics() {
        let (mut document, drawing, layer) = fixture();
        commit(
            &mut document,
            "circle",
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: Geometry::Circle {
                    center: DraftPoint::new(0.0, 0.0),
                    radius: 1.0,
                },
                style: DraftStyle::default(),
            },
        );
        let revision = document.revision();
        let mut cache = DraftingCache::new(DraftingCacheLimits::default()).unwrap();
        cache.reset(&document, drawing, 1).unwrap();
        let mut camera = Camera2D::new(200.0, 100.0);
        let previous = camera.clone();
        let mut proposed = camera.clone();
        proposed.pixels_per_metre = 1e12;
        proposed.revision += 1;
        let error = stage_camera_view(
            &mut cache,
            &document,
            1,
            &mut camera,
            proposed,
            &DisplayOptions::default(),
        )
        .unwrap_err();
        assert!(error.contains("budget"), "{error}");
        assert_eq!(camera, previous);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.drawing(drawing).unwrap().entities.len(), 1);
        assert!(
            cache
                .prepare(&document, 1, &camera, &DisplayOptions::default())
                .is_ok()
        );
    }

    #[test]
    fn analytic_pick_survives_current_view_display_budget_failure_without_tessellating() {
        let (mut document, drawing, layer) = fixture();
        let entity = commit(
            &mut document,
            "circle",
            DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: Geometry::Circle {
                    center: DraftPoint::new(0.0, 0.0),
                    radius: 1.0,
                },
                style: DraftStyle::default(),
            },
        )
        .created_draft_entities[0];
        let revision = document.revision();
        let mut cache = DraftingCache::new(DraftingCacheLimits::default()).unwrap();
        cache.reset(&document, drawing, 1).unwrap();
        let mut camera = Camera2D::new(200.0, 100.0);
        camera.center = DraftPoint::new(1.0, 0.0);
        camera.pixels_per_metre = 1e12;
        let error = cache
            .prepare(&document, 1, &camera, &DisplayOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("budget"));
        let before = cache.performance();
        let hits = pick_manual_semantics(
            &mut cache,
            &document,
            1,
            &camera,
            DraftPoint::new(100.0, 50.0),
        )
        .unwrap();
        let after = cache.performance();
        assert_eq!(hits.first().map(|hit| hit.entity_id), Some(entity));
        assert_eq!(after.tessellation_misses, before.tessellation_misses);
        assert_eq!(after.geometry_clones, before.geometry_clones);
        assert_eq!(after.index_updates, before.index_updates);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.drawing(drawing).unwrap().entities.len(), 1);
    }
}

#[cfg(test)]
mod accepted_controller_regressions {
    use crate::drafting_controller::{Controller, ControllerEffect, Tool};
    use drafting_core::{Geometry, Point2, bulge_arc};

    #[test]
    fn line_preview_never_emits_a_creation_before_confirmation() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        assert!(matches!(
            controller.click(Point2 { x: 1.0, y: 2.0 }).unwrap(),
            ControllerEffect::None
        ));
        controller.hover(Point2 { x: 5.0, y: 2.0 });
        assert_eq!(controller.preview().len(), 1);
        assert!(matches!(
            controller.numeric("5 2").unwrap(),
            ControllerEffect::None
        ));
        assert!(controller.is_pending());
        assert!(matches!(
            controller.confirm().unwrap(),
            ControllerEffect::Create(Geometry::Line { .. })
        ));
        assert!(!controller.is_pending());
    }

    #[test]
    fn cancel_removes_partial_and_ready_preview_without_command() {
        let mut controller = Controller::default();
        controller.begin(Tool::Circle);
        controller.numeric("1000000000 1000000000").unwrap();
        controller.numeric("1.25").unwrap();
        assert!(controller.is_pending());
        assert_eq!(controller.preview().len(), 1);
        controller.cancel();
        assert!(controller.preview().is_empty());
        assert!(controller.confirm().is_err());
        assert!(controller.points.is_empty());
    }

    #[test]
    fn signed_arc_numeric_input_preserves_analytic_sweep() {
        let mut controller = Controller::default();
        controller.begin(Tool::Arc);
        controller.numeric("10 4").unwrap();
        controller.numeric("2.5 30 -180").unwrap();
        let ControllerEffect::Create(Geometry::Arc {
            radius,
            sweep_angle,
            ..
        }) = controller.confirm().unwrap()
        else {
            panic!("arc")
        };
        assert_eq!(radius, 2.5);
        assert!((sweep_angle + std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn closed_polyline_keeps_the_closing_bulge() {
        let mut controller = Controller::default();
        controller.begin(Tool::Polyline);
        controller.numeric("0 0 0.5").unwrap();
        controller.numeric("4 0").unwrap();
        controller.numeric("4 4 -0.75").unwrap();
        controller.finish_polyline(true).unwrap();
        let ControllerEffect::Create(Geometry::Polyline { vertices, closed }) =
            controller.confirm().unwrap()
        else {
            panic!("polyline")
        };
        assert!(closed);
        assert_eq!(vertices[0].bulge, 0.5);
        assert_eq!(vertices[1].bulge, 0.0);
        assert_eq!(vertices[2].bulge, -0.75);
        let closing_arc = bulge_arc(vertices[2].point, vertices[0].point, vertices[2].bulge)
            .unwrap()
            .expect("negative closing bulge is an arc");
        assert!(closing_arc.sweep_angle < 0.0);
    }

    #[test]
    fn bad_numeric_input_does_not_advance_interactive_state() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        assert!(controller.numeric("NaN 3").is_err());
        assert!(controller.numeric("Infinity 3").is_err());
        assert!(controller.numeric("1 2 3 4 5").is_err());
        assert!(controller.points.is_empty());
        controller.begin(Tool::Polyline);
        assert!(controller.numeric("1 2 1000001").is_err());
        assert!(controller.numeric("1000000000001 0").is_err());
        assert!(controller.points.is_empty());
    }

    #[test]
    fn move_is_transient_until_confirmation() {
        let mut controller = Controller::default();
        controller.begin(Tool::Move);
        assert!(controller.numeric("2000000000001 0").is_err());
        assert!(!controller.is_pending());
        controller.numeric("3 -2").unwrap();
        assert!(controller.is_pending());
        assert!(matches!(
            controller.confirm().unwrap(),
            ControllerEffect::Move(Point2 { x: 3.0, y: -2.0 })
        ));
    }

    #[test]
    fn degenerate_numeric_geometry_fails_without_advancing_controller_state() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("1 2").unwrap();
        let line_pointer = controller.pointer;
        assert!(controller.numeric("1 2").is_err());
        assert_eq!(controller.points, vec![Point2 { x: 1.0, y: 2.0 }]);
        assert_eq!(controller.pointer, line_pointer);
        assert!(!controller.is_pending());
        assert!(controller.preview().is_empty());

        controller.begin(Tool::Circle);
        controller.numeric("0 0").unwrap();
        let circle_pointer = controller.pointer;
        assert!(controller.numeric("0").is_err());
        assert!(controller.numeric("-1").is_err());
        assert_eq!(controller.points, vec![Point2 { x: 0.0, y: 0.0 }]);
        assert_eq!(controller.pointer, circle_pointer);
        assert!(!controller.is_pending());
        assert!(controller.preview().is_empty());

        controller.begin(Tool::Arc);
        controller.numeric("2 3").unwrap();
        let arc_pointer = controller.pointer;
        assert!(controller.numeric("1000000000000 1000000000000").is_err());
        assert!(controller.numeric("2 3").is_err());
        assert!(controller.numeric("1 45 0").is_err());
        assert_eq!(controller.points, vec![Point2 { x: 2.0, y: 3.0 }]);
        assert_eq!(controller.pointer, arc_pointer);
        assert!(!controller.is_pending());
        assert!(controller.preview().is_empty());

        controller.numeric("4 3").unwrap();
        let arc_points = controller.points.clone();
        let arc_start_pointer = controller.pointer;
        assert!(controller.numeric("0").is_err());
        assert_eq!(controller.points, arc_points);
        assert_eq!(controller.pointer, arc_start_pointer);
        assert!(!controller.is_pending());

        controller.begin(Tool::Polyline);
        controller.numeric("0 0").unwrap();
        let polyline_pointer = controller.pointer;
        assert!(controller.numeric("0 0").is_err());
        assert_eq!(controller.points, vec![Point2 { x: 0.0, y: 0.0 }]);
        assert_eq!(controller.pointer, polyline_pointer);
        assert!(!controller.is_pending());
    }

    #[test]
    fn arc_end_at_centre_is_rejected_without_advancing_state() {
        let mut controller = Controller::default();
        controller.begin(Tool::Arc);
        controller.numeric("0 0").unwrap();
        controller.numeric("1 1").unwrap();
        let points = controller.points.clone();
        let pointer = controller.pointer;

        assert!(controller.numeric("0 0").is_err());
        assert_eq!(controller.points, points);
        assert_eq!(controller.pointer, pointer);
        assert!(!controller.is_pending());
    }

    #[test]
    fn transient_preview_is_validated_and_leave_clears_only_the_pointer() {
        let mut controller = Controller::default();
        controller.begin(Tool::Line);
        controller.numeric("0 0").unwrap();
        controller.hover(Point2 { x: 0.0, y: 0.0 });
        assert!(controller.preview().is_empty());

        controller.hover(Point2 { x: 2.0, y: 1.0 });
        assert_eq!(controller.preview().len(), 1);
        controller.clear_pointer();
        assert_eq!(controller.pointer, None);
        assert!(controller.preview().is_empty());
        assert_eq!(controller.points, vec![Point2 { x: 0.0, y: 0.0 }]);
    }

    #[test]
    fn finish_polyline_cannot_replace_a_ready_preview() {
        let mut controller = Controller::default();
        controller.begin(Tool::Polyline);
        controller.numeric("0 0").unwrap();
        controller.numeric("2 0").unwrap();
        controller.finish_polyline(false).unwrap();

        assert!(controller.finish_polyline(true).is_err());
        let ControllerEffect::Create(Geometry::Polyline { closed, .. }) =
            controller.confirm().unwrap()
        else {
            panic!("polyline")
        };
        assert!(!closed);
    }

    #[test]
    fn set_bulge_cannot_mutate_a_ready_polyline_preview() {
        let mut controller = Controller::default();
        controller.begin(Tool::Polyline);
        controller.numeric("0 0 0.5").unwrap();
        controller.numeric("2 0").unwrap();
        controller.finish_polyline(false).unwrap();

        let pending_geometry = controller.pending_geometry.clone();
        let bulges = controller.bulges.clone();
        let segment_bulge = controller.segment_bulge;
        assert!(controller.set_bulge(-0.75).is_err());
        assert_eq!(controller.pending_geometry, pending_geometry);
        assert_eq!(controller.bulges, bulges);
        assert_eq!(controller.segment_bulge, segment_bulge);

        let ControllerEffect::Create(Geometry::Polyline { vertices, closed }) =
            controller.confirm().unwrap()
        else {
            panic!("polyline")
        };
        assert!(!closed);
        assert_eq!(vertices[0].bulge, 0.5);
        assert_eq!(vertices[1].bulge, 0.0);
    }

    #[test]
    fn invalid_closing_bulge_keeps_polyline_points_and_can_be_repaired() {
        let mut controller = Controller::default();
        controller.begin(Tool::Polyline);
        controller.numeric("0 0").unwrap();
        controller.numeric("1000000000000 0").unwrap();
        controller.set_bulge(1_000_000.0).unwrap();
        let points = controller.points.clone();
        let bulges = controller.bulges.clone();

        assert!(controller.finish_polyline(true).is_err());
        assert!(!controller.is_pending());
        assert_eq!(controller.points, points);
        assert_eq!(controller.bulges, bulges);

        controller.set_bulge(0.0).unwrap();
        controller.finish_polyline(false).unwrap();
        assert!(controller.is_pending());
    }

    #[test]
    fn interactive_polyline_limit_rejects_the_next_point_atomically() {
        let mut controller = Controller::default();
        controller.begin(Tool::Polyline);
        for x in 0..4096 {
            controller
                .click(Point2 {
                    x: x as f64,
                    y: 0.0,
                })
                .unwrap();
        }
        let points = controller.points.clone();
        let bulges = controller.bulges.clone();
        let pointer = controller.pointer;

        assert!(controller.click(Point2 { x: 4096.0, y: 0.0 }).is_err());
        assert_eq!(controller.points, points);
        assert_eq!(controller.bulges, bulges);
        assert_eq!(controller.pointer, pointer);
        assert!(!controller.is_pending());
    }
}
