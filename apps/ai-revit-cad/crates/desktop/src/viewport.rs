// Adapted from AI CAD Viewer (crates/viewer_app/src/viewport.rs); standalone copy under the source project's MIT license.
//! GPU viewport for the standalone CAD viewer.
//!
//! The scene and its BVH stay in `viewer_core`; this widget owns only GPU
//! resources and pointer interaction. Its color pass is composited into the
//! ordinary Makepad widget tree so panels can sit beside the 3D view.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::gpu_export::GpuReadback;

use makepad_widgets::*;
use viewer_core::{
    Camera as CoreCamera, NavigationPivot, ObjectId, PickHit, ProjectedPoint, SceneObject,
    Transform3, Vec2 as CoreVec2, Vec3 as CoreVec3, Viewer, ViewerEvent,
    Viewport as CoreViewport,
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawCadSceneTexture = mod.std.set_type_default() do #(DrawCadSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color)
        }
    }

    mod.draw.DrawSectionEdge = mod.std.set_type_default() do #(DrawSectionEdge::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: true
        pixel: fn() {
            let point = self.pos * self.rect_size
            let edge = self.edge_end - self.edge_start
            let fraction = clamp(
                dot(point - self.edge_start, edge) / max(dot(edge, edge), 0.0001),
                0.0, 1.0
            )
            let distance = length(point - (self.edge_start + edge * fraction))
            let coverage = 1.0 - smoothstep(0.75, 2.25, distance)
            return Pal.premul(vec4(
                self.line_color.xyz,
                self.line_color.w * coverage
            ))
        }
    }

    mod.draw.DrawCadMesh = mod.std.set_type_default() do #(DrawCadMesh::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        base_color_texture: texture_2d(float)
        has_texture: 0.0
        review_srgb_output: 0.0
        geotech_presentation: 0.0
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)
        v_uv: varying(vec2f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        get_base_color_texture: fn() {
            if self.has_texture <= 0.5 {
                return vec4(1.0, 1.0, 1.0, 1.0)
            }
            let sample = self.base_color_texture.sample_as_bgra(vec2(
                fract(self.v_uv.x), fract(self.v_uv.y)
            ))
            return vec4(
                pow(max(sample.x, 0.0), 2.2),
                pow(max(sample.y, 0.0), 2.2),
                pow(max(sample.z, 0.0), 2.2),
                sample.w
            )
        }

        encode_review_srgb: fn(color_linear: vec3f) -> vec3f {
            return vec3(
                pow(max(color_linear.x, 0.0), 1.0 / 2.2),
                pow(max(color_linear.y, 0.0), 1.0 / 2.2),
                pow(max(color_linear.z, 0.0), 1.0 / 2.2)
            )
        }

        vertex: fn() {
            let local = vec4(
                self.geom.px,
                self.geom.py,
                self.geom.pz,
                1.0
            )
            let model_view = self.draw_list.view_transform * self.transform
            let world = model_view * local
            let section_world = self.transform * local
            let normal_view = self.draw_list.view_transform * self.normal_transform
            let encoded_x = floor(self.geom.nrm / 4096.0)
            let encoded_y = self.geom.nrm - encoded_x * 4096.0
            let normal = self.oct_decode(vec2(encoded_x, encoded_y) / 2047.5 - vec2(1.0, 1.0))
            let world_normal = normal_view * vec4(normal.x, normal.y, normal.z, 0.0)
            self.v_world = section_world.xyz
            self.v_world_clip = vec4(world.x, world.y, world.z, 1.0)
            self.v_normal = normalize(world_normal.xyz)
            self.v_uv = vec2(self.geom.uv, self.geom.color)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let normal = normalize(self.v_normal)
            let key = max(dot(normal, normalize(vec3(-0.35, 0.84, 0.42))), 0.0)
            let fill = max(dot(normal, normalize(vec3(0.58, 0.35, -0.62))), 0.0)
            let lit = 0.25 + key * 0.60 + fill * 0.18
            // The circular-section viewport is a diagram: keep front/back
            // geological caps readable without changing the BIM renderer.
            if self.geotech_presentation > 0.5 {
                let diagram_key = abs(dot(normal, normalize(vec3(-0.35, 0.84, 0.42))))
                let diagram_fill = abs(dot(normal, normalize(vec3(0.58, 0.35, -0.62))))
                let diagram_lit = min(1.0, 0.58 + diagram_key * 0.30 + diagram_fill * 0.12)
                let diagram_albedo = self.get_base_color_texture()
                return Pal.premul(vec4(self.color.xyz * diagram_albedo.xyz * diagram_lit,
                    self.color.w * diagram_albedo.w))
            }
            let albedo = self.get_base_color_texture()
            let lit_rgb = self.color.xyz * albedo.xyz * lit
            let alpha = self.color.w * albedo.w
            if self.review_srgb_output > 0.5 {
                return Pal.premul(vec4(self.encode_review_srgb(lit_rgb), alpha))
            }
            return Pal.premul(vec4(lit_rgb, alpha))
        }

        fragment: fn() {
            if self.clip_enabled > 0.5 {
                if self.v_world.x < self.clip_min.x || self.v_world.x > self.clip_max.x ||
                    self.v_world.y < self.clip_min.y || self.v_world.y > self.clip_max.y ||
                    self.v_world.z < self.clip_min.z || self.v_world.z > self.clip_max.z {
                    discard()
                }
            }
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip)
        }
    }

    mod.widgets.CadViewportBase = #(CadViewport::register_widget(vm))
    mod.widgets.CadViewport = set_type_default() do mod.widgets.CadViewportBase{
        width: Fill
        height: Fill
        clear_color: #x101820
        draw_bg: mod.draw.DrawCadSceneTexture{}
        draw_mesh: mod.draw.DrawCadMesh{}
        draw_marker: mod.draw.DrawCadMesh{alpha_blend: true}
        draw_section_edge: mod.draw.DrawSectionEdge{}
        context_menu: mod.widgets.PopupMenu{width: 206.0}
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawCadSceneTexture {
    #[deref]
    draw_super: DrawQuad,
}

impl DrawCadSceneTexture {
    fn set_scene_texture(&mut self, texture: &Texture) {
        self.draw_super.draw_vars.set_texture(0, texture);
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSectionEdge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    edge_start: Vec2f,
    #[live]
    edge_end: Vec2f,
    #[live]
    line_color: Vec4f,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawCadMesh {
    #[deref]
    draw_vars: DrawVars,
    #[live]
    color: Vec4f,
    #[live(0.0)]
    has_texture: f32,
    #[live(0.0)]
    review_srgb_output: f32,
    #[live(0.0)]
    geotech_presentation: f32,
    #[live]
    transform: Mat4f,
    #[live]
    normal_transform: Mat4f,
    #[live(0.0)]
    depth_clip: f32,
    #[live]
    clip_min: Vec3f,
    #[live]
    clip_max: Vec3f,
    #[live(0.0)]
    clip_enabled: f32,
}

impl DrawCadMesh {
    fn set_material_texture(&mut self, texture: Option<&Texture>) {
        self.has_texture = if let Some(texture) = texture {
            self.draw_vars.set_texture(0, texture);
            1.0
        } else {
            0.0
        };
    }

    fn draw(&mut self, cx: &mut CxDraw, geometry_id: GeometryId) {
        self.draw_vars.geometry_id = Some(geometry_id);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

struct CachedGeometry {
    signature: u64,
    epoch: u64,
    geometry: Geometry,
}

struct CachedMaterialTexture {
    texture: Texture,
    decoded_bytes: usize,
}

#[derive(Clone, Copy)]
struct PivotMarker {
    point: CoreVec3,
    normal: CoreVec3,
    fade_start: Option<f64>,
}

#[derive(Clone, Copy)]
enum DragMode {
    Orbit,
    Pan { depth: f64 },
}

#[derive(Clone, Copy)]
enum ContextCommand {
    Isolate,
    Hide,
    Show,
    ShowOnlySelected,
    Fit,
    Inspect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureFitMode {
    Current,
    FitAll,
    FitSelection,
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureRenderOptions {
    pub camera: Option<CoreCamera>,
    pub include_selection: bool,
    pub include_overlays: bool,
}

// Draw records commands; a subsequent Paint/NextFrame confirms the preceding
// submission. Never read a texture while a newer stamp is still unsubmitted.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GpuFrameState {
    scene_epoch: u64,
    scene_signature: u64,
    camera: CoreCamera,
    logical_viewport: CoreViewport,
    dpi_factor: f64,
    clear_color: [u32; 4],
    review_srgb_output: bool,
    geotech_presentation: bool,
    include_selection: bool,
    include_overlays: bool,
}

#[derive(Clone, Copy)]
struct GpuFrameIdentity {
    state: GpuFrameState,
    frame_id: u64,
    render_time: f64,
    shaders_ready_at_draw: bool,
}

struct PendingGpuExport {
    path: PathBuf,
    frame: GpuFrameIdentity,
    readback: GpuReadback,
    started: Instant,
}

const GPU_EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct DragState {
    last_abs: DVec2,
    mode: DragMode,
}

#[derive(Clone, Copy)]
struct ClickState {
    start_abs: DVec2,
    additive: bool,
}

/// A semantic change raised by the viewport for the surrounding panels.
#[derive(Clone, Debug, Default)]
pub enum CadViewportAction {
    #[default]
    None,
    Viewer(ViewerEvent),
    StateChanged,
    Inspect(ObjectId),
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct CadViewport {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawCadSceneTexture,
    #[live]
    draw_mesh: DrawCadMesh,
    #[live]
    draw_marker: DrawCadMesh,
    #[live]
    draw_section_edge: DrawSectionEdge,
    #[live]
    context_menu: PopupMenu,
    #[live(vec4(0.063, 0.094, 0.125, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust]
    rect: Rect,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    fh87_review_srgb_output: bool,
    #[rust(false)]
    geotech_presentation: bool,
    #[rust]
    pending_fit_margin: Option<f64>,
    #[rust(false)]
    suppress_section_overlay: bool,
    #[rust]
    capture_render_options: Option<CaptureRenderOptions>,
    #[rust]
    viewer: Viewer,
    #[rust]
    geometries: BTreeMap<ObjectId, CachedGeometry>,
    #[rust]
    texture_cache: BTreeMap<PathBuf, CachedMaterialTexture>,
    #[rust]
    texture_cache_order: VecDeque<PathBuf>,
    #[rust]
    texture_cache_bytes: usize,
    #[rust]
    failed_texture_paths: BTreeSet<PathBuf>,
    #[rust]
    mesh_epoch: u64,
    #[rust]
    gpu_frame_serial: u64,
    #[rust]
    gpu_frame_recorded: Option<GpuFrameIdentity>,
    #[rust]
    gpu_frame_ready: Option<GpuFrameIdentity>,
    #[rust]
    gpu_frame_next: NextFrame,
    #[rust]
    gpu_readback: Option<PendingGpuExport>,
    #[rust]
    gpu_export_timeout: Timer,
    #[rust]
    gpu_export_timed_out: Option<PathBuf>,
    #[rust]
    pivot_geometry: Option<Geometry>,
    #[rust]
    pivot_marker: Option<PivotMarker>,
    #[rust]
    marker_next_frame: NextFrame,
    #[rust]
    drag: Option<DragState>,
    #[rust]
    click: Option<ClickState>,
    #[rust]
    last_primary_click: Option<(f64, DVec2)>,
    #[rust]
    context_target: Option<ObjectId>,
    #[rust]
    context_pos: DVec2,
}

impl CadViewport {
    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    /// Returns the current viewport rectangle in window UI coordinates.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Opt-in diagram lighting for the standalone circular section only.
    pub fn set_geotech_presentation(&mut self, cx: &mut Cx, enabled: bool) {
        if self.geotech_presentation != enabled {
            self.geotech_presentation = enabled;
            self.cancel_gpu_export(cx);
            self.invalidate_gpu_frame();
            self.redraw(cx);
        }
    }

    /// The first scene can arrive before the widget has its final aspect ratio.
    /// Apply Fit in draw_walk after the actual logical viewport is established.
    pub fn fit_scene_when_measured(&mut self, cx: &mut Cx, margin: f64) {
        if margin.is_finite() && margin >= 1.0 {
            self.pending_fit_margin = Some(margin);
            self.cancel_gpu_export(cx);
            self.invalidate_gpu_frame();
            self.redraw(cx);
        }
    }

    /// Opens the object context menu at an absolute window position.
    pub fn open_context_menu(&mut self, cx: &mut Cx, object_id: ObjectId, position: DVec2) {
        let Some(object) = self.viewer.scene().get(object_id) else {
            return;
        };
        let should_select = object.visible && !object.selected;
        if should_select {
            let events = self.viewer.select(Some(object_id), false);
            self.emit_many(cx, events);
        }
        self.context_target = Some(object_id);
        self.context_pos = position;
        self.context_menu.redraw(cx);
        cx.redraw_all();
    }

    fn close_context_menu(&mut self, cx: &mut Cx) {
        if self.context_target.take().is_some() {
            self.context_menu.redraw(cx);
            cx.redraw_all();
        }
    }

    fn apply_context_command(&mut self, cx: &mut Cx, command: ContextCommand) {
        let Some(object_id) = self.context_target else {
            return;
        };
        let mut events = Vec::new();
        match command {
            ContextCommand::Isolate => events.extend(self.viewer.isolate(object_id)),
            ContextCommand::Hide => events.extend(self.viewer.hide(object_id)),
            ContextCommand::Show => events.extend(self.viewer.show(object_id)),
            ContextCommand::ShowOnlySelected => {
                let mut selected = self.viewer.scene().selected_ids();
                if selected.is_empty() {
                    events.extend(self.viewer.show(object_id));
                    events.extend(self.viewer.select(Some(object_id), false));
                    selected = self.viewer.scene().selected_ids();
                }
                events.extend(self.viewer.isolate_many(&selected));
            }
            ContextCommand::Fit => {
                if let Ok(Some(event)) = self.viewer.fit_object(object_id, 1.18) {
                    events.push(event);
                }
            }
            ContextCommand::Inspect => {
                cx.widget_action(self.uid, CadViewportAction::Inspect(object_id));
            }
        }
        self.emit_many(cx, events);
        self.close_context_menu(cx);
        self.redraw(cx);
    }

    /// Reads semantic interaction events without invalidating cached mesh data.
    pub fn drain_events(&mut self) -> Vec<ViewerEvent> {
        self.viewer.drain_events()
    }

    /// Call `redraw` after changing the scene through this mutable reference.
    pub fn viewer_mut(&mut self) -> &mut Viewer {
        self.mesh_epoch = self.mesh_epoch.wrapping_add(1);
        self.invalidate_gpu_frame();
        &mut self.viewer
    }

    /// Use for Section Box state changes, which never modify cached meshes.
    pub fn section_viewer_mut(&mut self) -> &mut Viewer {
        &mut self.viewer
    }

    pub fn set_viewer(&mut self, cx: &mut Cx, mut viewer: Viewer) {
        self.cancel_gpu_export(cx);
        self.mesh_epoch = self.mesh_epoch.wrapping_add(1);
        self.invalidate_gpu_frame();
        if self.rect.size.x > 1.0 && self.rect.size.y > 1.0 {
            let _ = viewer.set_viewport(CoreViewport::new(self.rect.size.x, self.rect.size.y));
        }
        self.viewer = viewer;
        // Dropping a Geometry only frees its Makepad pool ID. The pooled slot
        // keeps its CPU vertex and index buffers until another geometry reuses
        // it, so discard those buffers before closing a large scene.
        for entry in self.geometries.values() {
            entry.geometry.update(cx, Vec::new(), Vec::new());
        }
        self.geometries.clear();
        // Texture handles also use a Makepad pool. Release decoded pixels from
        // its slots before dropping this scene's cache entries.
        for entry in self.texture_cache.values() {
            release_material_texture_pixels(cx, &entry.texture);
        }
        self.texture_cache.clear();
        self.texture_cache_order.clear();
        self.texture_cache_bytes = 0;
        self.failed_texture_paths.clear();
        self.drag = None;
        self.click = None;
        self.last_primary_click = None;
        self.context_target = None;
        self.pivot_marker = None;
        self.marker_next_frame = NextFrame::default();
        self.redraw(cx);
        cx.widget_action(self.uid, CadViewportAction::StateChanged);
    }

    pub fn set_fh87_review_srgb_output(&mut self, enabled: bool) {
        self.fh87_review_srgb_output = enabled;
    }

    pub fn section_overlay_visible(&self) -> bool {
        !self.suppress_section_overlay
    }

    pub fn set_section_overlay_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.suppress_section_overlay = !visible;
        self.redraw(cx);
    }

    pub fn capture_camera_for_fit(
        &self,
        fit_mode: CaptureFitMode,
    ) -> Result<Option<CoreCamera>, String> {
        let (bounds, margin) = match fit_mode {
            CaptureFitMode::Current => return Ok(None),
            CaptureFitMode::FitAll => (
                self.viewer
                    .scene()
                    .visible_bounds()
                    .ok_or_else(|| "capture fit_all needs visible geometry".to_owned())?,
                1.15,
            ),
            CaptureFitMode::FitSelection => {
                let bounds = self
                    .viewer
                    .scene()
                    .selected_ids()
                    .iter()
                    .filter_map(|id| {
                        self.viewer
                            .scene()
                            .get(*id)
                            .and_then(SceneObject::world_bounds)
                    })
                    .reduce(|left, right| left.union(right))
                    .ok_or_else(|| "capture fit_selection needs selected geometry".to_owned())?;
                (bounds, 1.18)
            }
        };

        let mut camera = *self.viewer.camera();
        camera
            .fit_bounds(bounds, self.viewer.viewport(), margin)
            .map_err(|error| format!("cannot fit capture camera: {error}"))?;
        Ok(Some(camera))
    }

    pub fn set_capture_render_options(
        &mut self,
        cx: &mut Cx,
        options: Option<CaptureRenderOptions>,
    ) -> Option<CaptureRenderOptions> {
        self.cancel_gpu_export(cx);
        self.invalidate_gpu_frame();
        let previous = std::mem::replace(&mut self.capture_render_options, options);
        self.redraw(cx);
        previous
    }

    fn invalidate_gpu_frame(&mut self) {
        self.gpu_frame_recorded = None;
        self.gpu_frame_ready = None;
        self.gpu_frame_next = NextFrame::default();
        // Release native staging immediately on scene replacement/mutation.
        // Its one-shot watchdog may remain until it fires or is stopped below.
        self.gpu_readback = None;
    }

    fn gpu_frame_state(&self, cx: &Cx) -> GpuFrameState {
        let options = self.capture_render_options;
        let include_selection = options.map_or(true, |options| options.include_selection);
        let include_overlays = options.map_or(true, |options| options.include_overlays);
        GpuFrameState {
            scene_epoch: self.mesh_epoch,
            scene_signature: render_scene_signature(
                &self.viewer, include_selection, include_overlays, self.pivot_marker,
            ),
            camera: options.and_then(|options| options.camera).unwrap_or(*self.viewer.camera()),
            logical_viewport: self.viewer.viewport(),
            dpi_factor: cx.passes[self.pass.draw_pass_id()].dpi_factor.unwrap_or(1.0),
            clear_color: [self.clear_color.x.to_bits(), self.clear_color.y.to_bits(),
                self.clear_color.z.to_bits(), self.clear_color.w.to_bits()],
            review_srgb_output: self.fh87_review_srgb_output,
            geotech_presentation: self.geotech_presentation,
            include_selection,
            include_overlays,
        }
    }

    /// UI/render thread only. Retrying a pending export reuses its sole staging
    /// resource. Keep capture options unchanged across retries; never spin on
    /// this method while blocking Makepad event delivery.
    pub fn try_export_gpu_png(
        &mut self,
        cx: &mut Cx,
        path: &Path,
    ) -> Result<serde_json::Value, String> {
        if !cfg!(target_os = "windows") {
            return Err("GPU_EXPORT_UNSUPPORTED: D3D11 export requires Windows; select CPU fallback explicitly".to_owned());
        }
        if !self.rect.size.x.is_finite() || !self.rect.size.y.is_finite()
            || self.rect.size.x <= 1.0 || self.rect.size.y <= 1.0
        {
            self.cancel_gpu_export(cx);
            self.invalidate_gpu_frame();
            return Err("GPU_VIEWPORT_UNAVAILABLE: viewport is collapsed or has no valid dimensions".to_owned());
        }
        if self.gpu_export_timed_out.as_deref() == Some(path) {
            self.gpu_export_timed_out = None;
            return Err("GPU_READBACK_TIMEOUT: export cancelled after 5 seconds".to_owned());
        }
        if self.gpu_readback.as_ref().is_some_and(|pending| pending.path != path) {
            return Err("GPU_EXPORT_BUSY: another output path owns the active readback".to_owned());
        }
        let current = self.gpu_frame_state(cx);
        if self.gpu_readback.as_ref().is_some_and(|pending| pending.frame.state != current) {
            self.cancel_gpu_export(cx);
            self.gpu_frame_ready = None;
            self.redraw(cx);
            return Err("GPU_FRAME_NOT_READY: scene or camera changed; retry after redraw".to_owned());
        }
        if self.gpu_readback.is_none() {
            let frame = self.gpu_frame_ready.filter(|frame| frame.state == current);
            if !self.initialized || self.gpu_frame_recorded.is_some()
                || cx.passes[self.pass.draw_pass_id()].paint_dirty || frame.is_none()
            {
                self.redraw(cx);
                return Err("GPU_FRAME_NOT_READY: retry after the requested viewport frame is submitted".to_owned());
            }
            self.cancel_gpu_export(cx);
            let readback = GpuReadback::begin(cx, &self.color_texture)?;
            self.gpu_readback = Some(PendingGpuExport {
                path: path.to_owned(), frame: frame.unwrap(), readback, started: Instant::now(),
            });
            // Also release staging if an API client abandons polling.
            self.gpu_export_timeout = cx.start_timeout(GPU_EXPORT_TIMEOUT.as_secs_f64());
        }
        let pending = self.gpu_readback.as_mut().unwrap();
        if pending.started.elapsed() >= GPU_EXPORT_TIMEOUT {
            self.cancel_gpu_export(cx);
            return Err("GPU_READBACK_TIMEOUT: export cancelled after 5 seconds".to_owned());
        }
        let pixels = match pending.readback.try_read() {
            Ok(Some(pixels)) => pixels,
            Ok(None) => return Err("GPU_READBACK_PENDING: retry on a later UI timer tick".to_owned()),
            Err(error) => {
                self.cancel_gpu_export(cx);
                return Err(error);
            }
        };
        let pending = self.gpu_readback.take().unwrap();
        cx.stop_timer(self.gpu_export_timeout);
        self.gpu_export_timeout = Timer::default();
        let frame = pending.frame;
        let width = pending.readback.width;
        let height = pending.readback.height;
        let rgba_bytes = pending.readback.rgba_bytes;
        let row_pitch = pending.readback.row_pitch;
        // Unmapped by try_read; release native staging before PNG encoding.
        drop(pending);
        crate::capture::write_rgba_png(path, width, height, &pixels)?;
        Ok(serde_json::json!({
            "exported": true, "path": path.display().to_string(),
            "backend": "d3d11_gpu_readback", "gpu_rendered": true, "cpu_fallback": false,
            "width": width, "height": height, "rgba_bytes": rgba_bytes, "row_pitch": row_pitch,
            "frame_id": frame.frame_id, "render_time": frame.render_time,
            "scene_epoch": frame.state.scene_epoch,
            "scene_signature": format!("{:016x}", frame.state.scene_signature),
            "camera": frame.state.camera.state(),
            "logical_viewport": frame.state.logical_viewport, "dpi_factor": frame.state.dpi_factor,
            "include_selection": frame.state.include_selection,
            "include_overlays": frame.state.include_overlays,
            "geotech_presentation": frame.state.geotech_presentation,
            "overlay_scope": "scene_pass_only",
            "excluded_overlays": ["section_box_outline", "context_menu"],
            "viewport_only": true, "scene_shaders_ready": true,
        }))
    }

    pub fn has_pending_gpu_export(&self) -> bool {
        self.gpu_readback.is_some()
    }

    pub fn cancel_gpu_export(&mut self, cx: &mut Cx) {
        self.gpu_readback = None;
        cx.stop_timer(self.gpu_export_timeout);
        self.gpu_export_timeout = Timer::default();
        self.gpu_export_timed_out = None;
    }

    fn emit(&self, cx: &mut Cx, event: ViewerEvent) {
        cx.widget_action(self.uid, CadViewportAction::Viewer(event));
    }

    fn emit_many(&self, cx: &mut Cx, events: Vec<ViewerEvent>) {
        for event in events {
            self.emit(cx, event);
        }
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    fn local_screen(&self, abs: DVec2) -> CoreVec2 {
        CoreVec2::new(abs.x - self.rect.pos.x, abs.y - self.rect.pos.y)
    }

    fn update_hover(&mut self, cx: &mut Cx, abs: DVec2) {
        let Ok(mut events) = self.viewer.update_hover(self.local_screen(abs)) else {
            return;
        };
        events.extend(
            self.viewer
                .set_highlighted(self.viewer.hover_hit().map(|hit| hit.object_id)),
        );
        if !events.is_empty() {
            self.emit_many(cx, events);
            self.redraw(cx);
        }
    }

    fn fit_object(&mut self, cx: &mut Cx, object_id: ObjectId) {
        if let Ok(Some(event)) = self.viewer.fit_object(object_id, 1.18) {
            self.emit(cx, event);
            self.redraw(cx);
        }
    }

    fn fit_selection(&mut self, cx: &mut Cx) {
        if let Ok(Some(event)) = self.viewer.fit_selection(1.18) {
            self.emit(cx, event);
            self.redraw(cx);
        }
    }

    /// Semantic automation for the same picking path used by a viewport tap.
    /// Finds a visible triangle by stable ID, then raycasts its projected point.
    /// It fails for occluded objects; it never substitutes a direct ID selection.
    pub fn invoke_pick_element(&mut self, cx: &mut Cx, object_id: u64) -> Result<u64, String> {
        let object = self.viewer.scene().get(object_id).filter(|o|o.visible)
            .ok_or_else(||format!("visible viewport object {object_id} does not exist"))?;
        let camera = *self.viewer.camera();
        let viewport = self.viewer.viewport();
        let screen = object.mesh.triangles().iter().find_map(|triangle| {
            let center = object.transform.transform_point((triangle.vertices[0]+triangle.vertices[1]+triangle.vertices[2])/3.0);
            let projected = camera.project_point(center,viewport).ok().flatten()?;
            let screen = projected.screen;
            if screen.x<0.0 || screen.y<0.0 || screen.x>viewport.width || screen.y>viewport.height { return None; }
            self.viewer.pick_screen(screen).ok().flatten().filter(|hit|hit.object_id==object_id).map(|_|screen)
        }).ok_or_else(||format!("object {object_id} has no visible pick point in the current view"))?;
        let absolute = self.rect.pos + dvec2(screen.x,screen.y);
        self.select_click(cx,absolute,false,false);
        if self.viewer.scene().selected_ids()==[object_id] { Ok(object_id) }
        else { Err("viewport pick did not select the requested object".to_owned()) }
    }

    fn select_click(&mut self, cx: &mut Cx, abs: DVec2, additive: bool, double_click: bool) {
        let screen = self.local_screen(abs);
        let hit = self.viewer.pick_screen(screen).ok().flatten();
        if additive {
            if let Ok(Some(hit)) = self.viewer.raycast(screen) {
                let events = self.viewer.toggle_selection(hit.object_id);
                self.emit_many(cx, events);
            }
        } else if let Ok(mut events) = self.viewer.select_at(screen, false) {
            if hit.is_none()
                && !events.iter().any(|event| {
                    matches!(event, &ViewerEvent::SelectionChanged { .. })
                })
            {
                events.push(ViewerEvent::SelectionChanged {
                    selected_object_ids: Vec::new(),
                });
            }
            self.emit_many(cx, events);
        }
        if double_click {
            if let Some(hit) = hit {
                self.fit_object(cx, hit.object_id);
            }
        }
        cx.widget_action(self.uid, CadViewportAction::StateChanged);
        self.redraw(cx);
    }

    /// Sets a straight-on wall elevation using the segment's start-to-end direction.
    /// The segment is horizontal in XY, Z is up, and the camera sits on the
    /// direction cross Z side so screen-right follows wall_start to wall_end.
    pub fn set_wall_front_camera(
        &mut self,
        cx: &mut Cx,
        wall_start: CoreVec3,
        wall_end: CoreVec3,
        wall_height: f64,
        wall_thickness: f64,
    ) -> Result<(), String> {
        if !wall_start.is_finite()
            || !wall_end.is_finite()
            || !wall_height.is_finite()
            || !wall_thickness.is_finite()
            || wall_height <= 0.0
            || wall_thickness <= 0.0
            || (wall_start.z - wall_end.z).abs() > 1.0e-6
        {
            return Err("wall front camera requires level endpoints and positive finite dimensions".into());
        }
        let viewport = self.viewer.viewport();
        if !viewport.is_valid() {
            return Err("wall front camera requires a valid viewport".into());
        }
        let horizontal = CoreVec3::new(wall_end.x - wall_start.x, wall_end.y - wall_start.y, 0.0);
        let width = horizontal.length();
        if !width.is_finite() || width <= 1.0e-9 {
            return Err("wall front camera requires a nonzero horizontal wall span".into());
        }

        let direction = horizontal / width;
        let world_up = CoreVec3::Z;
        let front = direction.cross(world_up).normalized();
        let target = (wall_start + wall_end) * 0.5 + world_up * (wall_height * 0.5);
        let mut camera = *self.viewer.camera();
        let tan_vertical = (camera.vertical_fov_degrees.to_radians() * 0.5).tan();
        let tan_horizontal = tan_vertical * viewport.aspect_ratio();
        if !tan_vertical.is_finite()
            || !tan_horizontal.is_finite()
            || tan_vertical <= 0.0
            || tan_horizontal <= 0.0
        {
            return Err("wall front camera has invalid perspective bounds".into());
        }
        let margin = 1.18;
        let distance = ((width * 0.5 + wall_thickness * 0.5) * margin / tan_horizontal)
            .max(wall_height * 0.5 * margin / tan_vertical)
            .max(wall_thickness * 0.5 + camera.near * 2.0);
        if !target.is_finite() || !distance.is_finite() {
            return Err("wall front camera pose is not finite".into());
        }

        camera.position = target + front * distance;
        camera.target = target;
        camera.pivot = target;
        camera.up = world_up;
        camera.far = camera.far.max(distance + width + wall_height + wall_thickness + 1.0);
        self.apply_camera_pose(cx, camera, target)
    }

    /// Moves the camera and its free navigation pivot to a world point while
    /// preserving the current view direction and camera-to-target distance.
    pub fn aim_at_world(&mut self, cx: &mut Cx, point: CoreVec3) -> Result<(), String> {
        if !point.is_finite() {
            return Err("aim point must be finite".into());
        }
        let mut camera = *self.viewer.camera();
        let shift = point - camera.target;
        camera.position += shift;
        camera.target = point;
        camera.pivot = point;
        self.apply_camera_pose(cx, camera, point)
    }

    /// Aims at the visible geometry bound of a stable scene object ID.
    pub fn aim_at_element(&mut self, cx: &mut Cx, object_id: ObjectId) -> Result<(), String> {
        let point = self
            .viewer
            .scene()
            .get(object_id)
            .filter(|object| object.visible)
            .and_then(SceneObject::world_bounds)
            .map(|bounds| bounds.center())
            .ok_or_else(|| format!("visible viewport object {object_id} has no geometry bounds"))?;
        self.aim_at_world(cx, point)
    }

    /// Fits a visible host mesh from its plane-normal side.
    /// normal points from the host toward the camera; up supplies the
    /// tangential screen-up direction.
    pub fn set_host_plane_camera(
        &mut self,
        cx: &mut Cx,
        object_id: ObjectId,
        normal: CoreVec3,
        up: CoreVec3,
    ) -> Result<(), String> {
        if !normal.is_finite() || !up.is_finite() {
            return Err("host camera normal and up must be finite".into());
        }
        let normal_length = normal.length();
        let up_length = up.length();
        if !normal_length.is_finite()
            || !up_length.is_finite()
            || normal_length <= 1.0e-9
            || up_length <= 1.0e-9
        {
            return Err("host camera normal and up must be nonzero".into());
        }
        let normal = normal / normal_length;
        let up = up / up_length;
        let tangent_up = up - normal * up.dot(normal);
        let tangent_length = tangent_up.length();
        if !tangent_length.is_finite() || tangent_length <= 1.0e-9 {
            return Err("host camera normal and up must not be parallel".into());
        }
        let tangent_up = tangent_up / tangent_length;

        let bounds = self
            .viewer
            .scene()
            .get(object_id)
            .filter(|object| object.visible)
            .and_then(SceneObject::world_bounds)
            .ok_or_else(|| format!("visible viewport object {object_id} has no geometry bounds"))?;
        let target = bounds.center();
        if !target.is_finite() {
            return Err("host camera bounds center is not finite".into());
        }
        let viewport = self.viewer.viewport();
        if !viewport.is_valid() {
            return Err("host camera requires a valid viewport".into());
        }

        let mut camera = *self.viewer.camera();
        let initial_distance = (camera.near * 2.0).max(1.0e-3);
        camera.position = target + normal * initial_distance;
        camera.target = target;
        camera.pivot = target;
        camera.up = tangent_up;
        camera
            .fit_bounds(bounds, viewport, 1.18)
            .map_err(|error| format!("cannot fit host camera: {error}"))?;
        self.apply_camera_pose(cx, camera, target)
    }

    fn apply_camera_pose(
        &mut self,
        cx: &mut Cx,
        camera: CoreCamera,
        pivot: CoreVec3,
    ) -> Result<(), String> {
        camera
            .validate()
            .map_err(|error| format!("invalid viewport camera pose: {error}"))?;
        let mut state = self.viewer.capture_state();
        state.camera = camera.state();
        state.navigation_pivot = NavigationPivot {
            object_id: None,
            world_point: pivot,
        };
        state.hover_hit = None;
        let events = self
            .viewer
            .restore_state(&state)
            .map_err(|error| format!("cannot apply viewport camera pose: {error}"))?;
        self.emit_many(cx, events);
        self.redraw(cx);
        Ok(())
    }

    /// Projects a world point to viewport-local top-left-origin pixel coordinates.
    pub fn project_world(
        &self,
        point: CoreVec3,
    ) -> Result<Option<ProjectedPoint>, String> {
        if !point.is_finite() {
            return Err("projection point must be finite".into());
        }
        self.viewer
            .camera()
            .project_point(point, self.viewer.viewport())
            .map_err(|error| format!("cannot project viewport point: {error}"))
    }

    /// Returns the actual visible-mesh BVH hit at viewport-local pixel coordinates.
    /// Coordinates use a top-left origin; this observation does not change selection.
    pub fn raycast_screen(&self, x: f64, y: f64) -> Result<Option<PickHit>, String> {
        let viewport = self.viewer.viewport();
        if !x.is_finite()
            || !y.is_finite()
            || !viewport.is_valid()
            || x < 0.0
            || y < 0.0
            || x > viewport.width
            || y > viewport.height
        {
            return Err("raycast coordinates must be finite viewport-local pixels".into());
        }
        self.viewer
            .pick_screen(CoreVec2::new(x, y))
            .map_err(|error| format!("viewport raycast failed: {error}"))
    }

    /// Projects a world point and performs the same native selection dispatch as a viewport tap.
    /// A ray miss clears the selection through the regular viewer action path.
    pub fn invoke_pick_world(
        &mut self,
        cx: &mut Cx,
        point: CoreVec3,
    ) -> Result<Option<ObjectId>, String> {
        let projected = self
            .project_world(point)?
            .ok_or_else(|| "pick point is behind the camera".to_owned())?;
        self.invoke_pick_screen(cx, projected.screen.x, projected.screen.y)
    }

    /// Performs the native viewport tap selection path at viewport-local pixels.
    /// A miss clears the current selection through the regular viewer action path.
    pub fn invoke_pick_screen(
        &mut self,
        cx: &mut Cx,
        x: f64,
        y: f64,
    ) -> Result<Option<ObjectId>, String> {
        let hit = self.raycast_screen(x, y)?;
        let absolute = self.rect.pos + dvec2(x, y);
        self.select_click(cx, absolute, false, false);
        Ok(hit.map(|hit| hit.object_id))
    }

    fn finish_primary_click(
        &mut self,
        cx: &mut Cx,
        abs: DVec2,
        additive: bool,
        time: f64,
        platform_double: bool,
    ) {
        let recent = self
            .last_primary_click
            .is_some_and(|(previous_time, previous_abs)| {
                time - previous_time < 0.5 && (abs - previous_abs).length() < 6.0
            });
        self.last_primary_click = Some((time, abs));
        self.select_click(cx, abs, additive, platform_double || recent);
    }

    fn orbit_drag(&mut self, cx: &mut Cx, delta: DVec2) {
        let yaw = -delta.x * 0.007;
        let pitch = -delta.y * 0.007;
        if let Ok(event) = self.viewer.orbit(yaw, pitch) {
            self.emit(cx, event);
            self.redraw(cx);
        }
    }

    fn pan_at_depth(&mut self, cx: &mut Cx, depth: f64, delta: DVec2) {
        if let Ok(events) = self
            .viewer
            .pan_pixels_at_depth(CoreVec2::new(delta.x, delta.y), depth)
        {
            self.emit_many(cx, events);
            self.redraw(cx);
        }
    }

    fn draw_scene(
        &mut self,
        cx: &mut Cx3d,
        scene: SceneState3D,
        camera: CoreCamera,
        options: Option<CaptureRenderOptions>,
    ) -> bool {
        let mut shaders_ready = true;
        self.draw_list.begin_always(cx);
        cx.begin_scene_3d(scene);
        let previous_world = cx.set_scene_world_transform_3d(Mat4f::identity());

        let viewer = &self.viewer;
        let include_selection = options.map_or(true, |options| options.include_selection);
        let include_overlays = options.map_or(true, |options| options.include_overlays);
        let geometries = &mut self.geometries;
        let texture_cache = &mut self.texture_cache;
        let texture_cache_order = &mut self.texture_cache_order;
        let texture_cache_bytes = &mut self.texture_cache_bytes;
        let failed_texture_paths = &mut self.failed_texture_paths;
        let draw_mesh = &mut self.draw_mesh;
        let review_srgb_output = self.fh87_review_srgb_output;
        draw_mesh.geotech_presentation = if self.geotech_presentation { 1.0 } else { 0.0 };
        let section_box = viewer.section_box();
        draw_mesh.clip_enabled = if section_box.enabled { 1.0 } else { 0.0 };
        draw_mesh.clip_min = core_vec3(section_box.bounds.min());
        draw_mesh.clip_max = core_vec3(section_box.bounds.max());
        geometries.retain(|id, _| viewer.scene().get(*id).is_some());
        // Draw translucent debug volumes after opaque geometry, back to front.
        // They still test depth, but must not hide subsequently drawn geometry.
        let mut objects: Vec<_> = viewer.scene().objects().filter(|object| {
            object.visible
                && object.transform.is_valid()
                && object_in_frustum(object, &camera, viewer.viewport())
        }).map(|object| {
            let transparent = object.color[3] < 1.0;
            let depth = if transparent {
                object.world_bounds().map_or(0.0, |bounds| {
                    (bounds.center() - camera.position).dot(camera.forward())
                })
            } else {
                0.0
            };
            (transparent, depth, object)
        }).collect();
        objects.sort_by(|left, right| {
            left.0.cmp(&right.0)
                .then_with(|| right.1.total_cmp(&left.1))
                .then_with(|| left.2.object_id.cmp(&right.2.object_id))
        });
        for (transparent, _, object) in objects {
            let Some(geometry_id) =
                ensure_object_geometry(cx.cx, geometries, object, self.mesh_epoch)
            else {
                continue;
            };
            draw_mesh.transform = model_matrix(object.transform);
            draw_mesh.normal_transform = normal_matrix(object.transform);
            draw_mesh.draw_vars.options.alpha_blend = transparent;
            draw_mesh.draw_vars.options.depth_write = !transparent;
            let material_texture = object.render_texture.as_ref().and_then(|mapping| {
                ensure_material_texture(
                    cx.cx,
                    texture_cache,
                    texture_cache_order,
                    texture_cache_bytes,
                    failed_texture_paths,
                    &mapping.path,
                )
            });
            let selected = object.selected && include_selection;
            let highlighted = object.highlighted && include_overlays;
            draw_mesh.color = if material_texture.is_some() && !selected && !highlighted {
                vec4(1.0, 1.0, 1.0, object.color[3])
            } else {
                display_color(object, selected, highlighted)
            };
            draw_mesh.review_srgb_output =
                if review_srgb_output && material_texture.is_some() && !selected && !highlighted {
                    1.0
                } else {
                    0.0
                };
            draw_mesh.set_material_texture(material_texture.as_ref());
            draw_mesh.depth_clip = 0.0;
            draw_mesh.draw(cx, geometry_id);
            shaders_ready &= draw_shader_ready(cx.cx, &draw_mesh.draw_vars);
        }

        if include_overlays {
            if let Some(marker) = self.pivot_marker {
                let opacity = marker.fade_start.map_or(1.0, |start| {
                    (1.0 - (scene.time - start) / 0.4).clamp(0.0, 1.0)
                }) as f32;
                if opacity > 0.0 {
                    let geometry_id = ensure_pivot_geometry(cx.cx, &mut self.pivot_geometry);
                    let depth = (marker.point - camera.position)
                        .dot(camera.forward())
                        .max(camera.near);
                    let radius =
                        (2.0 * depth * (camera.vertical_fov_degrees.to_radians() * 0.5).tan()
                            / viewer.viewport().height
                            * 8.0)
                            .max(0.001);
                    let center = marker.point + marker.normal * (radius * 0.6);
                    self.draw_marker.transform = scaled_translation(center, radius);
                    self.draw_marker.normal_transform = Mat4f::identity();
                    self.draw_marker.color = vec4(1.0, 0.78, 0.22, opacity);
                    self.draw_marker.depth_clip = 0.0;
                    self.draw_marker.clip_enabled = 0.0;
                    self.draw_marker.draw(cx, geometry_id);
                    shaders_ready &= draw_shader_ready(cx.cx, &self.draw_marker.draw_vars);
                }
            }
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        self.draw_list.end(cx);
        shaders_ready
    }

    /// Projects the section volume into the widget's 2D overlay. Its twelve
    /// edges never enter the scene registry, color geometry cache, or picking.
    fn draw_section_overlay(
        &mut self,
        cx: &mut Cx2d,
        rect: Rect,
        camera: CoreCamera,
        include_overlays: bool,
    ) {
        if self.suppress_section_overlay || !include_overlays {
            return;
        }
        let section_box = self.viewer.section_box();
        if !section_box.enabled {
            return;
        }
        let viewport = self.viewer.viewport();
        if camera.validate().is_err() || !viewport.is_valid() {
            return;
        }
        let bounds = section_box.bounds;
        let corners: [CoreVec3; 8] = std::array::from_fn(|index| {
            CoreVec3::new(
                if index & 1 == 0 {
                    bounds.xmin
                } else {
                    bounds.xmax
                },
                if index & 2 == 0 {
                    bounds.ymin
                } else {
                    bounds.ymax
                },
                if index & 4 == 0 {
                    bounds.zmin
                } else {
                    bounds.zmax
                },
            )
        });
        self.draw_section_edge.line_color = vec4(0.35, 0.77, 1.0, 0.72);
        for axis in 0..3 {
            let axis_bit = 1 << axis;
            for corner in 0..8 {
                if corner & axis_bit != 0 {
                    continue;
                }
                let Some((start, end)) = project_section_edge(
                    camera,
                    viewport,
                    corners[corner],
                    corners[corner | axis_bit],
                ) else {
                    continue;
                };
                let left = start.x.min(end.x) - 3.0;
                let top = start.y.min(end.y) - 3.0;
                let width = (start.x - end.x).abs() + 6.0;
                let height = (start.y - end.y).abs() + 6.0;
                self.draw_section_edge.edge_start =
                    vec2((start.x - left) as f32, (start.y - top) as f32);
                self.draw_section_edge.edge_end = vec2((end.x - left) as f32, (end.y - top) as f32);
                self.draw_section_edge.draw_abs(
                    cx,
                    Rect {
                        pos: rect.pos + dvec2(left, top),
                        size: dvec2(width, height),
                    },
                );
            }
        }
    }
}

impl WidgetNode for CadViewport {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for CadViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.gpu_frame_next.is_event(event).is_some() {
            self.gpu_frame_next = NextFrame::default();
            // Windows delivers NextFrame at the next Paint, before that Paint's
            // Draw. The preceding Draw has therefore reached handle_repaint.
            let recorded = self.gpu_frame_recorded.take();
            self.gpu_frame_ready = if cx.passes[self.pass.draw_pass_id()].paint_dirty
                || recorded.is_some_and(|frame| !frame.shaders_ready_at_draw)
            {
                None
            } else {
                recorded
            };
        }
        if self.gpu_export_timeout.is_event(event).is_some() {
            self.gpu_export_timeout = Timer::default();
            self.gpu_export_timed_out = self.gpu_readback.take().map(|pending| pending.path);
        }
        if self.context_target.is_some() {
            let inside_menu = match event {
                Event::MouseDown(fe) => self.context_menu.menu_contains_pos(cx, fe.abs),
                Event::MouseUp(fe) => self.context_menu.menu_contains_pos(cx, fe.abs),
                _ => false,
            };
            let mut chosen = None;
            self.context_menu
                .handle_event_with(cx, event, self.area, &mut |_, action| {
                    if let PopupMenuAction::WasSelected(item_id) = action {
                        chosen = context_command_for_item(item_id);
                    }
                });
            if let Some(command) = chosen {
                self.apply_context_command(cx, command);
                return;
            }
            if let Event::KeyDown(fe) = event {
                if fe.key_code == KeyCode::Escape {
                    self.close_context_menu(cx);
                    return;
                }
            }
            if let Event::MouseDown(fe) = event {
                if !inside_menu {
                    self.close_context_menu(cx);
                } else if fe.button.is_primary() || fe.button.is_secondary() {
                    return;
                }
            }
            if let Event::MouseUp(fe) = event {
                if inside_menu && (fe.button.is_primary() || fe.button.is_secondary()) {
                    return;
                }
            }
        }
        if let Event::MouseDown(fe) = event {
            if fe.button.is_secondary() && self.rect.contains(fe.abs) {
                match self.viewer.pick_screen(self.local_screen(fe.abs)) {
                    Ok(Some(hit)) => self.open_context_menu(cx, hit.object_id, fe.abs),
                    _ => self.close_context_menu(cx),
                }
                return;
            }
        }
        if let Some(frame) = self.marker_next_frame.is_event(event) {
            if self.pivot_marker.is_some_and(|marker| {
                marker
                    .fade_start
                    .is_some_and(|start| frame.time - start >= 0.4)
            }) {
                self.pivot_marker = None;
                self.redraw(cx);
            } else if self
                .pivot_marker
                .is_some_and(|marker| marker.fade_start.is_some())
            {
                self.marker_next_frame = cx.new_next_frame();
                self.redraw(cx);
            }
        }
        // The offscreen pass can replace its hit area during redraw. Keep a
        // middle-button drag attached to raw mouse events across those passes.
        if let Event::MouseMove(fe) = event {
            if let Some(mut drag) = self.drag {
                let delta = fe.abs - drag.last_abs;
                drag.last_abs = fe.abs;
                self.drag = Some(drag);
                if delta.length() > 0.0 {
                    match drag.mode {
                        DragMode::Orbit => self.orbit_drag(cx, delta),
                        DragMode::Pan { depth } => self.pan_at_depth(cx, depth, delta),
                    }
                }
                return;
            }
        }
        if let Event::MouseUp(fe) = event {
            if fe.button.is_middle() && self.drag.take().is_some() {
                if let Some(marker) = &mut self.pivot_marker {
                    marker.fade_start = Some(fe.time);
                    self.marker_next_frame = cx.new_next_frame();
                }
                cx.set_cursor(MouseCursor::Default);
                self.redraw(cx);
                return;
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe)
                if fe
                    .device
                    .mouse_button()
                    .is_some_and(|button| button.is_middle()) =>
            {
                cx.set_key_focus(self.area);
                if let Ok(events) = self.viewer.set_pivot_at_screen(self.local_screen(fe.abs)) {
                    self.emit_many(cx, events);
                }
                let hit = self.viewer.hover_hit();
                let highlight_events = self.viewer.set_highlighted(hit.map(|hit| hit.object_id));
                self.emit_many(cx, highlight_events);
                let camera = self.viewer.camera();
                let pivot = self.viewer.navigation_pivot().world_point;
                let normal = hit.map_or(-camera.forward(), |hit| hit.world_normal);
                let depth = (pivot - camera.position)
                    .dot(camera.forward())
                    .max(camera.near);
                let mode = if fe.modifiers.shift {
                    DragMode::Pan { depth }
                } else {
                    DragMode::Orbit
                };
                self.pivot_marker = Some(PivotMarker {
                    point: pivot,
                    normal,
                    fade_start: None,
                });
                self.drag = Some(DragState {
                    last_abs: fe.abs,
                    mode,
                });
                cx.set_cursor(MouseCursor::Grabbing);
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                self.click = Some(ClickState {
                    start_abs: fe.abs,
                    additive: fe.modifiers.control,
                });
            }
            Hit::FingerMove(fe) => {
                if let Some(mut drag) = self.drag {
                    let delta = fe.abs - drag.last_abs;
                    drag.last_abs = fe.abs;
                    self.drag = Some(drag);
                    if delta.length() > 0.0 {
                        match drag.mode {
                            DragMode::Orbit => self.orbit_drag(cx, delta),
                            DragMode::Pan { depth } => self.pan_at_depth(cx, depth, delta),
                        }
                    }
                }
            }
            Hit::FingerUp(fe)
                if fe
                    .device
                    .mouse_button()
                    .is_some_and(|button| button.is_middle()) =>
            {
                self.drag = None;
                if let Some(marker) = &mut self.pivot_marker {
                    marker.fade_start = Some(fe.time);
                    self.marker_next_frame = cx.new_next_frame();
                }
                cx.set_cursor(MouseCursor::Default);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if let Some(click) = self.click.take() {
                    if fe.is_over && fe.was_tap() && (fe.abs - click.start_abs).length() <= 6.0 {
                        self.finish_primary_click(
                            cx,
                            fe.abs,
                            click.additive,
                            fe.time,
                            fe.tap_count >= 2,
                        );
                    }
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => self.update_hover(cx, fe.abs),
            Hit::FingerHoverOut(_) => {
                let mut events = self.viewer.clear_hover();
                events.extend(self.viewer.set_highlighted(None));
                if !events.is_empty() {
                    self.emit_many(cx, events);
                    self.redraw(cx);
                }
            }
            Hit::FingerScroll(fe) => {
                let scroll = if fe.scroll.y.abs() > f64::EPSILON {
                    fe.scroll.y
                } else {
                    fe.scroll.x
                };
                if scroll.abs() > f64::EPSILON {
                    // On Windows Makepad reports a forward wheel notch as a negative Y delta.
                    let factor = if scroll < 0.0 { 1.18 } else { 1.0 / 1.18 };
                    if let Ok(event) = self
                        .viewer
                        .dolly_at_screen(self.local_screen(fe.abs), factor)
                    {
                        self.emit(cx, event);
                        self.redraw(cx);
                    }
                }
            }
            Hit::KeyDown(fe) => match fe.key_code {
                KeyCode::Escape => {
                    let events = self.viewer.select(None, false);
                    self.emit_many(cx, events);
                    self.redraw(cx);
                }
                KeyCode::KeyF => self.fit_selection(cx),
                KeyCode::Home => {
                    if let Ok(Some(event)) = self.viewer.fit_scene(1.18) {
                        self.emit(cx, event);
                        self.redraw(cx);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        if let Event::MouseUp(fe) = event {
            if fe.button.is_primary() {
                if let Some(click) = self.click.take() {
                    if self.rect.contains(fe.abs) && (fe.abs - click.start_abs).length() <= 6.0 {
                        self.finish_primary_click(cx, fe.abs, click.additive, fe.time, false);
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.rect = rect;
        if !rect.size.x.is_finite() || !rect.size.y.is_finite()
            || rect.size.x <= 1.0 || rect.size.y <= 1.0
        {
            self.cancel_gpu_export(cx.cx);
            self.invalidate_gpu_frame();
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        let _ = self
            .viewer
            .set_viewport(CoreViewport::new(rect.size.x, rect.size.y));
        if let Some(margin) = self.pending_fit_margin.take() {
            let _ = self.viewer.fit_scene(margin);
        }
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        let options = self.capture_render_options;
        let camera = options
            .and_then(|options| options.camera)
            .unwrap_or(*self.viewer.camera());
        let include_overlays = options.map_or(true, |options| options.include_overlays);
        if let Some(scene) = scene_state(camera, rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene);
            let cx3d = &mut Cx3d::new(cx.cx);
            let shaders_ready_at_draw = self.draw_scene(cx3d, scene, camera, options);
            self.gpu_frame_serial = self.gpu_frame_serial.wrapping_add(1);
            self.gpu_frame_ready = None;
            self.gpu_frame_recorded = Some(GpuFrameIdentity {
                state: self.gpu_frame_state(cx.cx),
                frame_id: self.gpu_frame_serial,
                render_time: cx.time(),
                shaders_ready_at_draw,
            });
            self.gpu_frame_next = cx.new_next_frame();
        } else {
            self.invalidate_gpu_frame();
        }
        cx.end_pass(&self.pass);

        self.draw_bg.set_scene_texture(&self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = cx.cx.update_area_refs(self.area, self.draw_bg.area());
        cx.set_pass_area(&self.pass, self.area);
        self.draw_section_overlay(cx, rect, camera, include_overlays);
        if include_overlays && self.context_target.is_some() {
            let pass_size = cx.current_pass_size();
            let menu_pos = dvec2(
                self.context_pos
                    .x
                    .clamp(0.0, (pass_size.x - 210.0).max(0.0)),
                self.context_pos
                    .y
                    .clamp(0.0, (pass_size.y - 204.0).max(0.0)),
            );
            self.context_menu.begin(cx);
            self.context_menu
                .draw_item(cx, id!(ctx_isolate).into(), "Isolate object");
            self.context_menu
                .draw_item(cx, id!(ctx_hide).into(), "Hide object");
            self.context_menu
                .draw_item(cx, id!(ctx_show).into(), "Show object");
            self.context_menu
                .draw_item(cx, id!(ctx_selected).into(), "Show only selected");
            self.context_menu
                .draw_item(cx, id!(ctx_fit).into(), "Fit object");
            self.context_menu
                .draw_item(cx, id!(ctx_inspect).into(), "Inspect properties");
            self.context_menu.end(cx, self.area, menu_pos - rect.pos);
        }
        DrawStep::done()
    }
}

impl CadViewportRef {
    pub fn set_viewer(&self, cx: &mut Cx, viewer: Viewer) {
        if let Some(mut viewport) = self.borrow_mut() {
            viewport.set_viewer(cx, viewer);
        }
    }

    pub fn actions(&self, actions: &Actions) -> Vec<CadViewportAction> {
        actions
            .filter_widget_actions(self.widget_uid())
            .map(|action| action.cast::<CadViewportAction>())
            .filter(|action| !matches!(action, CadViewportAction::None))
            .collect()
    }
}

fn context_command_for_item(item: PopupMenuItemId) -> Option<ContextCommand> {
    let id = item.0;
    if id == id!(ctx_isolate) {
        Some(ContextCommand::Isolate)
    } else if id == id!(ctx_hide) {
        Some(ContextCommand::Hide)
    } else if id == id!(ctx_show) {
        Some(ContextCommand::Show)
    } else if id == id!(ctx_selected) {
        Some(ContextCommand::ShowOnlySelected)
    } else if id == id!(ctx_fit) {
        Some(ContextCommand::Fit)
    } else if id == id!(ctx_inspect) {
        Some(ContextCommand::Inspect)
    } else {
        None
    }
}

// Native shader creation is asynchronous on a cold Windows shader cache.
// Record readiness while issuing scene draws, so a later compiler completion
// cannot incorrectly bless a previously submitted background-only frame.
fn draw_shader_ready(cx: &Cx, vars: &DrawVars) -> bool {
    vars.draw_shader_id
        .and_then(|id| cx.draw_shaders.shaders.get(id.index))
        .is_some_and(|shader| shader.os_shader_id.is_some())
}

// Geometry identity is the mutation epoch; this hashes only render metadata,
// avoiding an extra traversal of every triangle for each export/retry.
fn render_scene_signature(
    viewer: &Viewer,
    include_selection: bool,
    include_overlays: bool,
    marker: Option<PivotMarker>,
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let point = |point: CoreVec3| [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()];
    for object in viewer.scene().objects() {
        object.object_id.hash(&mut hash);
        object.visible.hash(&mut hash);
        if !object.visible { continue; }
        (object.selected && include_selection).hash(&mut hash);
        (object.highlighted && include_overlays).hash(&mut hash);
        object.color.map(f32::to_bits).hash(&mut hash);
        for basis in [CoreVec3::ZERO, CoreVec3::X, CoreVec3::Y, CoreVec3::Z] {
            point(object.transform.transform_point(basis)).hash(&mut hash);
        }
        object.render_texture.is_some().hash(&mut hash);
        if let Some(texture) = &object.render_texture {
            texture.path.hash(&mut hash);
            texture.repeat_m.to_bits().hash(&mut hash);
        }
    }
    let section = viewer.section_box();
    section.enabled.hash(&mut hash);
    if section.enabled {
        point(section.bounds.min()).hash(&mut hash);
        point(section.bounds.max()).hash(&mut hash);
    }
    if include_overlays {
        marker.is_some().hash(&mut hash);
        if let Some(marker) = marker {
            point(marker.point).hash(&mut hash);
            point(marker.normal).hash(&mut hash);
            marker.fade_start.map(f64::to_bits).hash(&mut hash);
        }
    }
    hash.finish()
}

fn scene_state(camera: CoreCamera, rect: Rect, time: f64) -> Option<SceneState3D> {
    camera.validate().ok()?;
    let eye = core_vec3(camera.position);
    let target = core_vec3(camera.target);
    let up = core_vec3(camera.up);
    let view = Mat4f::look_at(eye, target, up);
    let projection = Mat4f::perspective(
        camera.vertical_fov_degrees as f32,
        (rect.size.x / rect.size.y) as f32,
        camera.near as f32,
        camera.far as f32,
    );
    Some(SceneState3D {
        time,
        camera_pos: eye,
        view,
        projection,
        viewport_rect: rect,
    })
}

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let uniforms = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    uniforms.camera_projection = scene.projection;
    uniforms.camera_projection_r = scene.projection;
    uniforms.camera_view = scene.view;
    uniforms.camera_view_r = scene.view;
    uniforms.depth_projection = scene.projection;
    uniforms.depth_projection_r = scene.projection;
    uniforms.depth_view = scene.view;
    uniforms.depth_view_r = scene.view;
    uniforms.camera_inv = camera_inv;
    uniforms.camera_inv_r = camera_inv;
}

fn core_vec3(value: CoreVec3) -> Vec3f {
    vec3(value.x as f32, value.y as f32, value.z as f32)
}

/// Clips one world-space box edge to the camera near plane, then to the
/// viewport rectangle. This keeps overlay quads small even for a box whose
/// corner is behind the eye or far outside the screen.
fn project_section_edge(
    camera: CoreCamera,
    viewport: CoreViewport,
    start: CoreVec3,
    end: CoreVec3,
) -> Option<(CoreVec2, CoreVec2)> {
    let forward = camera.forward();
    let depth_margin = (camera.far - camera.near) * 1.0e-6;
    let near = camera.near + depth_margin;
    let far = camera.far - depth_margin;
    let start_depth = (start - camera.position).dot(forward);
    let end_depth = (end - camera.position).dot(forward);
    let depth_delta = end_depth - start_depth;
    let (enter, leave) = if depth_delta.abs() < 1.0e-12 {
        if start_depth < near || start_depth > far {
            return None;
        }
        (0.0, 1.0)
    } else {
        let near_fraction = (near - start_depth) / depth_delta;
        let far_fraction = (far - start_depth) / depth_delta;
        (
            near_fraction.min(far_fraction).max(0.0),
            near_fraction.max(far_fraction).min(1.0),
        )
    };
    if enter > leave {
        return None;
    }
    let edge = end - start;
    let end = start + edge * leave;
    let start = start + edge * enter;
    let start = camera.project_point(start, viewport).ok()??.screen;
    let end = camera.project_point(end, viewport).ok()??.screen;
    clip_section_edge_to_viewport(start, end, viewport)
}

fn clip_section_edge_to_viewport(
    start: CoreVec2,
    end: CoreVec2,
    viewport: CoreViewport,
) -> Option<(CoreVec2, CoreVec2)> {
    if !start.x.is_finite() || !start.y.is_finite() || !end.x.is_finite() || !end.y.is_finite() {
        return None;
    }
    let delta = end - start;
    let mut enter: f64 = 0.0;
    let mut leave: f64 = 1.0;
    for (slope, distance) in [
        (-delta.x, start.x),
        (delta.x, viewport.width - start.x),
        (-delta.y, start.y),
        (delta.y, viewport.height - start.y),
    ] {
        if slope.abs() < 1.0e-12 {
            if distance < 0.0 {
                return None;
            }
        } else {
            let fraction = distance / slope;
            if slope < 0.0 {
                enter = enter.max(fraction);
            } else {
                leave = leave.min(fraction);
            }
            if enter > leave {
                return None;
            }
        }
    }
    Some((
        CoreVec2::new(start.x + delta.x * enter, start.y + delta.y * enter),
        CoreVec2::new(start.x + delta.x * leave, start.y + delta.y * leave),
    ))
}

fn matrix_from_columns(x: CoreVec3, y: CoreVec3, z: CoreVec3, translation: CoreVec3) -> Mat4f {
    Mat4f {
        v: [
            x.x as f32,
            x.y as f32,
            x.z as f32,
            0.0,
            y.x as f32,
            y.y as f32,
            y.z as f32,
            0.0,
            z.x as f32,
            z.y as f32,
            z.z as f32,
            0.0,
            translation.x as f32,
            translation.y as f32,
            translation.z as f32,
            1.0,
        ],
    }
}

fn model_matrix(transform: Transform3) -> Mat4f {
    matrix_from_columns(
        transform.transform_vector(CoreVec3::X),
        transform.transform_vector(CoreVec3::Y),
        transform.transform_vector(CoreVec3::Z),
        transform.translation,
    )
}

fn normal_matrix(transform: Transform3) -> Mat4f {
    let orientation = if transform.scale.x * transform.scale.y * transform.scale.z < 0.0 {
        -1.0
    } else {
        1.0
    };
    let inverse = CoreVec3::new(
        orientation / transform.scale.x,
        orientation / transform.scale.y,
        orientation / transform.scale.z,
    );
    matrix_from_columns(
        transform.rotation.rotate_vec3(CoreVec3::X * inverse.x),
        transform.rotation.rotate_vec3(CoreVec3::Y * inverse.y),
        transform.rotation.rotate_vec3(CoreVec3::Z * inverse.z),
        CoreVec3::ZERO,
    )
}

fn scaled_translation(position: CoreVec3, scale: f64) -> Mat4f {
    matrix_from_columns(
        CoreVec3::X * scale,
        CoreVec3::Y * scale,
        CoreVec3::Z * scale,
        position,
    )
}

fn display_color(object: &SceneObject, selected: bool, highlighted: bool) -> Vec4f {
    let [r, g, b, alpha] = object.color;
    if selected && highlighted {
        vec4(1.0, 0.80, 0.34, alpha)
    } else if selected {
        vec4(1.0, 0.62, 0.20, alpha)
    } else if highlighted {
        vec4(0.43, 0.86, 1.0, alpha)
    } else {
        vec4(r, g, b, alpha)
    }
}

/// A bounding sphere around the world AABB keeps this cull conservative, even
/// for rotated or nonuniformly scaled meshes and near-plane intersections.
fn object_in_frustum(object: &SceneObject, camera: &CoreCamera, viewport: CoreViewport) -> bool {
    let Some(bounds) = object.world_bounds() else {
        return true;
    };
    if camera.validate().is_err() || !viewport.is_valid() {
        return true;
    }
    let center = bounds.center();
    let radius = bounds.radius();
    if !center.is_finite() || !radius.is_finite() {
        return true;
    }
    let offset = center - camera.position;
    let depth = offset.dot(camera.forward());
    if depth + radius < camera.near || depth - radius > camera.far {
        return false;
    }
    if depth <= radius {
        return true;
    }
    let tangent_y = (camera.vertical_fov_degrees.to_radians() * 0.5).tan();
    let tangent_x = tangent_y * viewport.aspect_ratio();
    let right = offset.dot(camera.right()).abs();
    let up = offset.dot(camera.camera_up()).abs();
    right <= depth * tangent_x + radius * (1.0 + tangent_x * tangent_x).sqrt()
        && up <= depth * tangent_y + radius * (1.0 + tangent_y * tangent_y).sqrt()
}

const MAX_CACHED_MATERIAL_TEXTURES: usize = 32;
const MAX_CACHED_MATERIAL_TEXTURE_BYTES: usize = 64 * 1024 * 1024;

fn release_material_texture_pixels(cx: &mut Cx, texture: &Texture) {
    match texture.get_format(cx) {
        TextureFormat::VecBGRAu8_32 { data, .. }
        | TextureFormat::VecMipBGRAu8_32 { data, .. } => {
            data.take();
        }
        _ => {}
    }
}

fn png_declared_rgba_bytes(bytes: &[u8]) -> Result<usize, String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err("invalid PNG header".to_owned());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap()) as usize;
    if width == 0 || height == 0 {
        return Err("PNG dimensions must be non-zero".to_owned());
    }
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "PNG dimensions overflow addressable memory".to_owned())
}

fn ensure_material_texture(
    cx: &mut Cx,
    cache: &mut BTreeMap<PathBuf, CachedMaterialTexture>,
    cache_order: &mut VecDeque<PathBuf>,
    cache_bytes: &mut usize,
    failed_paths: &mut BTreeSet<PathBuf>,
    path: &Path,
) -> Option<Texture> {
    if let Some(texture) = cache.get(path) {
        return Some(texture.texture.clone());
    }
    if failed_paths.contains(path) {
        return None;
    }
    let loaded = std::fs::read(path)
        .map_err(|error| format!("cannot read texture {}: {error}", path.display()))
        .and_then(|bytes| {
            let decoded_bytes = png_declared_rgba_bytes(&bytes)
                .map_err(|error| format!("cannot inspect texture {}: {error}", path.display()))?;
            if decoded_bytes > MAX_CACHED_MATERIAL_TEXTURE_BYTES {
                return Err(format!(
                    "texture {} expands to {decoded_bytes} bytes; limit is {}",
                    path.display(),
                    MAX_CACHED_MATERIAL_TEXTURE_BYTES
                ));
            }
            let image = ImageBuffer::from_png(&bytes)
                .map_err(|error| format!("cannot decode texture {}: {error}", path.display()))?;
            Ok((image, decoded_bytes))
        });
    let (image, decoded_bytes) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("[FH-87 texture] {error}");
            failed_paths.insert(path.to_owned());
            while failed_paths.len() > 32 {
                if let Some(oldest) = failed_paths.first().cloned() {
                    failed_paths.remove(&oldest);
                }
            }
            return None;
        }
    };
    let texture = image.into_new_texture(cx);
    cache.insert(
        path.to_owned(),
        CachedMaterialTexture {
            texture: texture.clone(),
            decoded_bytes,
        },
    );
    *cache_bytes = cache_bytes.saturating_add(decoded_bytes);
    cache_order.push_back(path.to_owned());
    while cache_order.len() > MAX_CACHED_MATERIAL_TEXTURES
        || *cache_bytes > MAX_CACHED_MATERIAL_TEXTURE_BYTES
    {
        if let Some(oldest) = cache_order.pop_front() {
            if let Some(removed) = cache.remove(&oldest) {
                *cache_bytes = cache_bytes.saturating_sub(removed.decoded_bytes);
            }
        }
    }
    Some(texture)
}

fn planar_texture_uv(point: CoreVec3, normal: CoreVec3, repeat_m: f64) -> [f32; 2] {
    let abs = CoreVec3::new(normal.x.abs(), normal.y.abs(), normal.z.abs());
    let (u, v) = if abs.z >= abs.x && abs.z >= abs.y {
        (point.x, point.y)
    } else if abs.y >= abs.x {
        (point.x, point.z)
    } else {
        (point.y, point.z)
    };
    let divisor = if repeat_m.is_finite() && repeat_m > 0.001 {
        repeat_m
    } else {
        1.0
    };
    let u = u / divisor;
    let v = v / divisor;
    [
        if u.is_finite() && u.abs() <= f32::MAX as f64 {
            u as f32
        } else {
            0.0
        },
        if v.is_finite() && v.abs() <= f32::MAX as f64 {
            v as f32
        } else {
            0.0
        },
    ]
}

fn oct_encode(normal: CoreVec3) -> (f32, f32) {
    let divisor = normal.x.abs() + normal.y.abs() + normal.z.abs();
    let x = normal.x / divisor;
    let y = normal.y / divisor;
    let z = normal.z / divisor;
    if z >= 0.0 {
        (x as f32, y as f32)
    } else {
        let sx = if x >= 0.0 { 1.0 } else { -1.0 };
        let sy = if y >= 0.0 { 1.0 } else { -1.0 };
        (((1.0 - y.abs()) * sx) as f32, ((1.0 - x.abs()) * sy) as f32)
    }
}

fn packed_octahedral_normal(normal: CoreVec3) -> f32 {
    let (x, y) = oct_encode(normal);
    // Two 12-bit coordinates fit exactly in one f32 integer value. This also
    // avoids the bitcast half-float operations unavailable in Windows HLSL.
    let quantize = |coordinate: f32| ((coordinate + 1.0) * 2047.5).round().clamp(0.0, 4095.0) as u32;
    (quantize(x) * 4096 + quantize(y)) as f32
}

fn mesh_signature(object: &SceneObject) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    object.mesh.triangles().len().hash(&mut hash);
    for triangle in object.mesh.triangles() {
        triangle.triangle_id.hash(&mut hash);
        for vertex in triangle.vertices {
            vertex.x.to_bits().hash(&mut hash);
            vertex.y.to_bits().hash(&mut hash);
            vertex.z.to_bits().hash(&mut hash);
        }
    }
    if let Some(texture) = &object.render_texture {
        texture.path.hash(&mut hash);
        texture.repeat_m.to_bits().hash(&mut hash);
    }
    hash.finish()
}

fn mesh_buffers(object: &SceneObject) -> (Vec<u32>, Vec<f32>) {
    let mut indices = Vec::with_capacity(object.mesh.triangles().len() * 3);
    let mut vertices = Vec::with_capacity(object.mesh.triangles().len() * 3 * 6);
    let repeat_m = object
        .render_texture
        .as_ref()
        .map(|texture| texture.repeat_m)
        .unwrap_or(1.0);
    for triangle in object.mesh.triangles() {
        let normal = triangle.normal();
        if !normal.is_finite()
            || normal.length_squared() <= 1.0e-20
            || triangle.vertices.iter().any(|point| {
                !point.is_finite()
                    || point.x.abs() > f32::MAX as f64
                    || point.y.abs() > f32::MAX as f64
                    || point.z.abs() > f32::MAX as f64
            })
        {
            continue;
        }
        let packed_normal = packed_octahedral_normal(normal);
        for point in triangle.vertices {
            let uv = object
                .render_texture
                .as_ref()
                .map(|_| planar_texture_uv(point, normal, repeat_m))
                .unwrap_or([0.0, 0.0]);
            vertices.extend_from_slice(&[
                point.x as f32,
                point.y as f32,
                point.z as f32,
                packed_normal,
                uv[0],
                uv[1],
            ]);
            indices.push(indices.len() as u32);
        }
    }
    (indices, vertices)
}

fn ensure_object_geometry(
    cx: &mut Cx,
    cache: &mut BTreeMap<ObjectId, CachedGeometry>,
    object: &SceneObject,
    epoch: u64,
) -> Option<GeometryId> {
    if object.mesh.triangles().is_empty() {
        return None;
    }
    if let Some(entry) = cache.get_mut(&object.object_id) {
        if entry.epoch == epoch {
            return Some(entry.geometry.geometry_id());
        }
        let signature = mesh_signature(object);
        if entry.signature != signature {
            let (indices, vertices) = mesh_buffers(object);
            if indices.is_empty() {
                cache.remove(&object.object_id);
                return None;
            }
            entry.geometry.update(cx, indices, vertices);
            entry.signature = signature;
        }
        entry.epoch = epoch;
        return Some(entry.geometry.geometry_id());
    }
    let (indices, vertices) = mesh_buffers(object);
    if indices.is_empty() {
        return None;
    }
    let geometry = Geometry::new(cx);
    geometry.update(cx, indices, vertices);
    let id = geometry.geometry_id();
    cache.insert(
        object.object_id,
        CachedGeometry {
            signature: mesh_signature(object),
            epoch,
            geometry,
        },
    );
    Some(id)
}

fn ensure_pivot_geometry(cx: &mut Cx, slot: &mut Option<Geometry>) -> GeometryId {
    let geometry = slot.get_or_insert_with(|| {
        let geometry = Geometry::new(cx);
        let up = CoreVec3::Y;
        let down = -CoreVec3::Y;
        let right = CoreVec3::X;
        let left = -CoreVec3::X;
        let front = CoreVec3::Z;
        let back = -CoreVec3::Z;
        let faces = [
            [up, front, right],
            [up, right, back],
            [up, back, left],
            [up, left, front],
            [down, right, front],
            [down, back, right],
            [down, left, back],
            [down, front, left],
        ];
        let mut indices = Vec::with_capacity(24);
        let mut vertices = Vec::with_capacity(24 * 6);
        for face in faces {
            let normal = (face[1] - face[0]).cross(face[2] - face[0]).normalized();
            let packed_normal = packed_octahedral_normal(normal);
            for point in face {
                vertices.extend_from_slice(&[
                    point.x as f32,
                    point.y as f32,
                    point.z as f32,
                    packed_normal,
                    0.0,
                    0.0,
                ]);
                indices.push(indices.len() as u32);
            }
        }
        geometry.update(cx, indices, vertices);
        geometry
    });
    geometry.geometry_id()
}

