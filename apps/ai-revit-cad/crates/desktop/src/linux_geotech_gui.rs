//! NOT_ADMITTED consumer candidate. Ordinary widget input; limited GPU association.
use super::*;
use crate::{linux_geotech_gui_wire as wire, linux_lifecycle_wire as life};
use wire::Target;
use crate::drafting_canvas::{CanvasAction, DraftingCanvasRef};
use makepad_widgets::makepad_draw::makepad_platform::studio::{StudioToApp, RemoteMouseDown, RemoteMouseUp, RemoteKeyModifiers};
use makepad_widgets::makepad_draw::makepad_platform::makepad_network::SignalToUI;
use makepad_widgets::makepad_draw::makepad_platform::CxDrawListPool;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, OpenOptions}, io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::PathBuf,
    sync::{OnceLock, mpsc::{sync_channel, SyncSender}}, time::{Duration, Instant}};
#[path = "linux_geotech_persistence_bridge.rs"]
mod persistence;
#[path = "linux_geotech_job_bridge.rs"]
mod jobs;
pub(crate) use jobs::{job_native_start,job_submit_before,job_submit_returned,job_report_before,job_report_returned,job_report_metadata,Report};
pub(crate) fn strict_job_original(method:&str,bytes:&[u8])->Option<bool>{
    jobs::jobwire::method(method).map(|_|jobs::jobwire::original_bytes(bytes))
}

const MAX_SNAPSHOT_AGE: Duration = Duration::from_millis(5000); // provisional, UNMEASURED phase2 bound
static PROFILE_ENABLED: OnceLock<bool> = OnceLock::new();
static CAPTURE_REJECTION_REPORTED: OnceLock<()> = OnceLock::new();
/// Exactly the current trusted App launcher tail; client argv has a separate contract.
fn supported_app_argv() -> bool {
    let mut args = std::env::args_os();
    args.next().is_some() && args.next().is_some_and(|value| value == "--linux-backend=x11") && args.next().is_none()
}
/// Called before native entry/worker threads. Only a future trusted Root phase2
/// startup may set this fixed opt-in; request parameters cannot enable it.
pub(super) fn initialize_profile() -> Result<(),String> {
    let requested = std::env::var_os("AI_REVIT_PHASE2_GUI_PROFILE");
    let v2 = requested.as_ref().is_some_and(|value| value == "restricted-v2");
    let enabled = match requested {
        None => false,
        Some(value) if value == "restricted-v1" || value == "restricted-v2" => true,
        _ => return Err("GUI_STARTUP_PROFILE_INVALID".into()),
    };
    life::ensure(!enabled || supported_app_argv(),"GUI_STARTUP_ARGV_REFUSED")?;
    // Runtime Studio settings are consumed by SDK app_main before Event::Startup.
    // Reject ambiguity before native entry; never clear or rewrite caller env.
    if enabled {
        for name in ["STUDIO_HOST", "STUDIO", "STUDIO_BUILD", "STUDIO_CRATE"] {
            life::ensure(std::env::var_os(name).map_or(true, |value| value.is_empty()),
                "GUI_STARTUP_STUDIO_ENV_REFUSED")?;
        }
    }
    persistence::initialize(v2)?;
    PROFILE_ENABLED.set(enabled).map_err(|_|"GUI_STARTUP_PROFILE_ALREADY_SET".into())
}

#[derive(Default)]
pub(super) struct GuiHooks {
    sender: Option<SyncSender<StudioToApp>>, binding: Option<life::Binding>,
    snapshot: Option<Snapshot>, issued: Option<Snapshot>, consumed_serial: Option<u64>, draw_serial: u64, ids: BTreeSet<String>,
    operations: BTreeMap<String, Operation>, pending: Option<String>,
    clicks: usize, captures: usize, fatal: Option<String>, last_action: Option<Snapshot>,
    persistence: persistence::Hooks,
    jobs: jobs::Hooks,
}
#[derive(Clone)]
struct MeasuredTarget { target: Target, uid: Option<u64>, area: Area, rect: Rect, clip: Rect,
    enabled: bool, visible: bool, available: bool }
impl MeasuredTarget {
    fn json(&self) -> Value { json!({"target":self.target.name(),"widget_uid":self.uid.map(|v|v.to_string()),
        "rect":rect_json(self.rect),"clipped_rect":rect_json(self.clip),"enabled":self.enabled,
        "visible":self.visible,"available":self.available}) }
}
#[derive(Clone)]
struct Snapshot { serial: u64, redraw: u64, created: Instant, binding: life::Binding, window: Value,
    model: Value, canvas: Value, targets: Vec<MeasuredTarget> }
impl Snapshot {
    fn id(&self) -> String { format!("g{:016x}",self.serial) }
    fn targets_json(&self) -> Value { Value::Array(self.targets.iter().map(MeasuredTarget::json).collect()) }
    fn json(&self) -> Value { let mut value = json!({"snapshot_id":self.id(),"app_draw_serial":self.serial,"redraw_id":self.redraw,
        "binding":&self.binding,"window":&self.window,"model":&self.model,"canvas":&self.canvas,
        "targets":self.targets_json(),"max_age_ms":5000,"max_age_measured":false,
        "freshness":"ISSUED_APP_DRAW_REVALIDATED_AGAINST_LATEST_DRAW_WITH_ORIGINAL_5000MS_BOUND",
        "GPU_completion_proved":false,"acceptance":"NOT_GRANTED"});
        // Fresh clock sample only; never refreshes this Snapshot's created time.
        // Zero is a fail-closed unavailable/expired sample, never an extension.
        value["lifecycle_remaining_ms"] = json!(crate::linux_lifecycle::gui_trusted_remaining_ms().unwrap_or(0));
        if self.model.get("persistence").is_some() {
            value["input_schema_version"] = json!(2);
            value["geometry_generation"] = json!(self.serial);
        }
        value
    }
}
#[derive(Clone)]
enum Kind { Click { target: Target, uid: u64, point: DVec2, down_time: f64, up_time: f64 },
    Capture { path: PathBuf, requested: Option<Snapshot> },
    DocumentCapture { path: PathBuf, requested: Option<Snapshot> }, V2 }
#[derive(Clone)]
struct Operation { kind: Kind, before: Snapshot, phase: &'static str,
    down: bool, up: bool, down_returned: bool, up_returned: bool, clicked: bool,
    post: Option<Value>, image: Option<Value>, error: Option<String>, polls: usize }
impl Operation {
    fn json(&self) -> Value {
        json!({"phase":self.phase,"down_admitted":self.down,"up_admitted":self.up,
            "ordinary_UI_down_returned":self.down_returned,"ordinary_UI_up_returned":self.up_returned,
            "ordinary_button_clicked":self.clicked,"before":self.before.json(),"post":&self.post,
            "image":&self.image,"error":&self.error,"exact_renderer_draw_serial_proved":false,
            "compositor_presentation_proved":false,"full_GUI_acceptance":false,
            "fit_success_inferred_from_click":false,"acceptance":"NOT_GRANTED"})
    }
}
fn rect_json(rect: Rect) -> Value { json!({"x":rect.pos.x,"y":rect.pos.y,"width":rect.size.x,"height":rect.size.y}) }
fn good_rect(rect: Rect) -> bool { [rect.pos.x,rect.pos.y,rect.size.x,rect.size.y].into_iter().all(f64::is_finite)
    && rect.size.x > 0.0 && rect.size.y > 0.0 }
fn same_point(a: DVec2, b: DVec2) -> bool { a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits() }
fn contains(rect: Rect, p: DVec2) -> bool { good_rect(rect) && p.x >= rect.pos.x && p.y >= rect.pos.y &&
    p.x < rect.pos.x+rect.size.x && p.y < rect.pos.y+rect.size.y }
/// Observe only; SDK drains the current canvas UID's actions after this Draw callback.
fn canvas_resize_pending(canvas: &DraftingCanvasRef, actions: &Actions) -> bool {
    canvas.actions(actions).into_iter().any(|action|matches!(action,CanvasAction::Resize {..}))
}
fn post_click_2d_draw_ready(live_uid: bool, pending_resize: bool, prepared: bool,
    rect: Rect, dpi: f64, camera_viewport: DVec2, before_count: Option<u64>, current_count: Option<u64>) -> bool {
    live_uid && !pending_resize && prepared && good_rect(rect) && dpi.is_finite() && dpi>0.0 &&
        camera_viewport.x==rect.size.x && camera_viewport.y==rect.size.y &&
        matches!((before_count,current_count),(Some(before),Some(current)) if current>before)
}
fn same_source(a: &Snapshot, b: &Snapshot) -> bool {
    a.binding == b.binding && a.window == b.window && a.model == b.model && a.targets_json() == b.targets_json()
}
/// Check the current handle's full generation/freed state and its own list epoch
/// BEFORE rect/clipped_rect. Never query geometry or validity of an issued Area.
fn current_area_valid(pool: &CxDrawListPool, area: Area) -> bool {
    let (id,epoch,nonempty)=match area {
        Area::Instance(a)=>(a.draw_list_id,a.redraw_id,a.instance_count>0),
        Area::Rect(a)=>(a.draw_list_id,a.redraw_id,true),
        Area::Empty=>return false,
    };
    if !nonempty || pool.is_id_freed(id) {return false;}
    pool.checked_index(id).is_some_and(|list|list.redraw_id == epoch)
}
/// A newer completed Draw may preserve one issued lease, never renew its age.
/// Both the issued and latest epochs are ordered; latest must still be live.
fn lease_draw_admissible(age: Duration, issued_serial: u64, issued_redraw: u64,
    latest_serial: u64, latest_redraw: u64, live_serial: u64, live_redraw: u64) -> bool {
    age <= MAX_SNAPSHOT_AGE && issued_serial <= latest_serial && issued_redraw <= latest_redraw &&
        latest_serial == live_serial && latest_redraw == live_redraw
}
/// SDK Area Eq includes redraw_id. All other slot/generation fields stay exact.
fn same_area_slot(a: Area, b: Area) -> bool {
    match (a,b) {
        (Area::Instance(a),Area::Instance(b)) => a.draw_list_id == b.draw_list_id &&
            a.draw_item_id == b.draw_item_id && a.instance_offset == b.instance_offset &&
            a.instance_count == b.instance_count,
        (Area::Rect(a),Area::Rect(b)) => a.draw_list_id == b.draw_list_id && a.rect_id == b.rect_id,
        _ => false,
    }
}
fn area_lease_matches(a: Area, b: Area, available: bool, current_valid: bool, newer_draw: bool) -> bool {
    if !available { return a == b; } // unchanged unavailable/Empty targets are not promoted
    if !current_valid || matches!(b,Area::Empty) {return false;}
    if a == b {return true;}
    if !newer_draw || !same_area_slot(a,b) {return false;}
    // Area epochs belong to their own DrawList, not the global Cx redraw stamp.
    match (a,b) {
        (Area::Instance(a),Area::Instance(b))=>b.redraw_id > a.redraw_id,
        (Area::Rect(a),Area::Rect(b))=>b.redraw_id > a.redraw_id,
        _=>false,
    }
}
/// Diagnostic only: fixed bits, never source values, paths, UIDs or error text.
/// Only the failed Capture admission path may spend the one stderr slot.
#[derive(Clone, Copy)]
enum RejectionStage { Latest, Issued }
impl RejectionStage {
    fn label(self) -> &'static str { match self {Self::Latest=>"LATEST",Self::Issued=>"ISSUED"} }
}
#[derive(Clone, Copy)]
enum RejectionCause { Source, TargetCount, Area }
impl RejectionCause {
    fn label(self) -> &'static str {match self {Self::Source=>"SOURCE",Self::TargetCount=>"TARGET_COUNT",Self::Area=>"AREA"}}
}
#[derive(Clone, Copy, Default)]
struct AreaGuardTrace { evaluated: u8, failed: u8, next: u8 }
impl AreaGuardTrace {
    fn observe(&mut self, result: bool) -> bool {
        // Production snapshots contain exactly Target::ALL in this order.
        if self.next<4 {
            let bit=1u8<<self.next;self.evaluated |= bit;
            if !result {self.failed |= bit;}
        }
        self.next=self.next.saturating_add(1);
        result // preserve the original .all short-circuit result
    }
}
fn available_mask(snapshot: &Snapshot) -> u8 {
    Target::ALL.into_iter().enumerate().fold(0,|mask,(bit,target)|
        mask | (u8::from(snapshot.targets.iter().any(|v|v.target==target && v.available))<<bit))
}fn key_difference_mask(a: &Value, b: &Value, keys: &[&str]) -> u64 {
    keys.iter().enumerate().fold(0,|mask,(bit,key)|
        mask | (u64::from(a.get(*key) != b.get(*key)) << bit))
}
fn target_difference_mask(a: &Snapshot, b: &Snapshot, target: Target, pool: &CxDrawListPool) -> u16 {
    let left:Vec<_>=a.targets.iter().filter(|v|v.target==target).collect();
    let right:Vec<_>=b.targets.iter().filter(|v|v.target==target).collect();
    if left.is_empty() && right.is_empty() {return 0;}
    if left.len()!=1 || right.len()!=1 {return 1;}
    let (a,b)=(left[0],right[0]);
    let mut mask=0u16;
    match (a.area,b.area) {
        (Area::Empty,Area::Empty)=>{},
        (Area::Instance(a),Area::Instance(b))=>{
            mask |= u16::from(a.draw_list_id!=b.draw_list_id)<<2;
            mask |= u16::from(a.draw_item_id!=b.draw_item_id)<<3;
            mask |= u16::from(a.instance_offset!=b.instance_offset)<<4;
            mask |= u16::from(a.instance_count!=b.instance_count)<<5;
            mask |= u16::from(a.redraw_id!=b.redraw_id)<<6;
        },
        (Area::Rect(a),Area::Rect(b))=>{
            mask |= u16::from(a.draw_list_id!=b.draw_list_id)<<2;
            mask |= u16::from(a.rect_id!=b.rect_id)<<3;
            mask |= u16::from(a.redraw_id!=b.redraw_id)<<6;
        },
        _=>mask |= 1<<1,
    }
    // Availability is already measured. Current pool validity is read without
    // geometry, only for a target that was/is available; issued Area is never queried.
    mask |= u16::from((a.available || b.available) && !current_area_valid(pool,b.area))<<7;
    mask |= u16::from(a.uid!=b.uid)<<8;
    mask |= u16::from(rect_json(a.rect)!=rect_json(b.rect))<<9;
    mask |= u16::from(rect_json(a.clip)!=rect_json(b.clip))<<10;
    mask |= u16::from(a.enabled!=b.enabled)<<11;
    mask |= u16::from(a.visible!=b.visible)<<12;
    mask |= u16::from(a.available!=b.available)<<13;
    mask
}
fn capture_rejection_line(stage: RejectionStage, cause: RejectionCause, trace: AreaGuardTrace,
    a: &Snapshot, b: &Snapshot, pool: &CxDrawListPool) -> String {
    let source=u8::from(a.binding!=b.binding) | (u8::from(a.window!=b.window)<<1) |
        (u8::from(a.model!=b.model)<<2) | (u8::from(a.targets_json()!=b.targets_json())<<3);
    let window=key_difference_mask(&a.window,&b.window,
        &["index","generation","dpi_factor","width","height","position","native_xid_verified"]);
    let model=key_difference_mask(&a.model,&b.model,
        &["document","circular","active","view_3d","view3d","worker_quiescent"]);
    let document=key_difference_mask(&a.model["document"],&b.model["document"],
        &["uuid","runtime","open_generation","revision"]);
    let circular=key_difference_mask(&a.model["circular"],&b.model["circular"],
        &["panel","display_generation","camera_revision","catalog","view_status","frame_token",
          "screen_triangles","screen_segments","local_selection","local_selection_error",
          "selected_slice","selected_candidate","circle_preview","native_acceptance"]);
    let panel=key_difference_mask(&a.model["circular"]["panel"],&b.model["circular"]["panel"],
        &["active","dirty","stale","ground_dirty","case_dirty","search_dirty","ground_id","case_id",
          "ground_source_operation_id","case_source_operation_id","analysis_host_gate","search_host_gate",
          "draft_generation","search_generation","status","view_dimension","presentation_state","archive",
          "archive_error","display_result_origin","display_slice_count","display_fellenius_fs",
          "display_bishop_equation_fs","display_bishop_accepted_fs","calculation_dimension","result_current",
          "core_accepted","diagnostic_only","input_fingerprint","source_fingerprint","solver_version",
          "slice_page","slice_page_size","job_sequence","cancelling","geometry_endpoint_review",
          "finite_table_review","whole_arc_review","whole_arc_applicability","search_classification",
          "saved_search_domain","typed_search_status","typed_search_accepted","search_request_fingerprint",
          "render_result_eligible","requested_overlays"]);
    let view3d=key_difference_mask(&a.model["view3d"],&b.model["view3d"],
        &["camera","objects","visible_objects","rect","host_visible","completed_GPU_serial"]);
    let targets=Target::ALL.map(|target|target_difference_mask(a,b,target,pool));
    format!("GUI_SNAPSHOT_REJECTION_V1 context=CAPTURE stage={} guard={} area_eval={:x} area_failed={:x} available_before={:x} available_after={:x} newer_draw={} source_diff={:02x} window_diff={:02x} model_diff={:02x} document_diff={:02x} circular_diff={:04x} panel_diff={:016x} view3d_diff={:02x} target_area_meta_diff={:04x},{:04x},{:04x},{:04x}",
        stage.label(),cause.label(),trace.evaluated,trace.failed,available_mask(a),available_mask(b),u8::from(b.serial>a.serial),
        source,window,model,document,circular,panel,view3d,targets[0],targets[1],targets[2],targets[3])
}
fn report_capture_rejection(enabled: bool, stage: RejectionStage, cause: RejectionCause, trace: AreaGuardTrace,
    a: &Snapshot, b: &Snapshot, pool: &CxDrawListPool) {
    if enabled && CAPTURE_REJECTION_REPORTED.set(()).is_ok() {
        // Fixed bounded line; failed/closed stderr must not replace the original refusal.
        let _=writeln!(std::io::stderr().lock(),"{}",capture_rejection_line(stage,cause,trace,a,b,pool));
    }
}
/// The actual Snapshot route calls this handoff: reads never replace a held
/// lease, and a consumed Draw serial can never be issued again.
fn issue_snapshot(issued: &mut Option<Snapshot>, consumed: Option<u64>, candidate: Snapshot) -> Result<Snapshot,String> {
    if let Some(held)=issued.as_ref() {
        life::ensure(consumed.map_or(true,|last|held.serial > last),"GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
        return Ok(held.clone());
    }
    life::ensure(consumed.map_or(true,|last|candidate.serial > last),"GUI_WAIT_FRESH_DRAW")?;
    *issued=Some(candidate.clone());
    Ok(candidate)
}
fn consume_issued(issued: &mut Option<Snapshot>, consumed: &mut Option<u64>, id: &str) -> Result<(),String> {
    let held=issued.as_ref().ok_or("GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
    life::ensure(held.id() == id && consumed.map_or(true,|last|held.serial > last),
        "GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
    *consumed=Some(held.serial); // highwater is recorded before the first effect
    *issued=None;
    Ok(())
}
impl App {
    fn gui_target_widget(&self, cx: &mut Cx, target: Target) -> WidgetRef {
        match target { Target::WorkspaceCircular => self.ui.widget(cx,ids!(workspace_circular)),
            Target::Gcc2d => self.ui.widget(cx,ids!(gcc_2d)), Target::Gcc3d => self.ui.widget(cx,ids!(gcc_3d)),
            Target::GccFit => self.ui.widget(cx,ids!(gcc_fit)),
            Target::PathInput => self.ui.widget(cx,ids!(path_input)),
            Target::SaveDocument => self.ui.widget(cx,ids!(save_document)),
            Target::OpenDocument => self.ui.widget(cx,ids!(open_document)),
            Target::NewDocument => self.ui.widget(cx,ids!(new_document)) }
    }
    fn gui_measured_target(&self, cx: &mut Cx, target: Target) -> MeasuredTarget {
        let widget = self.gui_target_widget(cx,target);
        let uid = widget.try_widget_uid().map(|v|v.0);
        let enabled = if target == Target::PathInput { widget.borrow::<TextInput>().map(|input|!input.is_read_only()).unwrap_or(false) }
            else { widget.borrow::<Button>().map(|v|v.enabled()).unwrap_or(false) };
        let visible = widget.visible() && (!target.legacy() || target == Target::WorkspaceCircular ||
            (self.circular_ui.panel.active && self.ui.widget(cx,ids!(circular_actions)).visible()));
        let area = widget.area(); let valid = uid.is_some() && current_area_valid(&cx.draw_lists,area);
        let rect = if valid { area.rect(cx) } else { Rect::default() };
        let clip = if valid { area.clipped_rect(cx) } else { Rect::default() };
        MeasuredTarget { target,uid,area,rect,clip,enabled,visible,
            available:valid && enabled && visible && good_rect(rect) && good_rect(clip) }
    }
    fn gui_snapshot_now(&self, cx: &mut Cx, serial: u64) -> Result<Snapshot, String> {
        let binding = self.gui_lifecycle_frozen(cx)?;
        life::ensure(self.gui_hooks.binding.as_ref() == Some(&binding), "GUI_INSTALLED_BINDING_CHANGED")?;
        life::ensure(!Cx::has_studio_web_socket() && supported_app_argv(), "GUI_PROFILE_ASSURANCE_CHANGED")?;
        let geom = &cx.windows[WindowId(0,0)].window_geom;
        life::ensure(geom.dpi_factor.is_finite() && geom.dpi_factor > 0.0 &&
            geom.inner_size.x.is_finite() && geom.inner_size.y.is_finite() &&
            geom.inner_size.x > 0.0 && geom.inner_size.y > 0.0, "GUI_CLIENT_GEOMETRY_INVALID")?;
        let window = json!({"index":0,"generation":0,"dpi_factor":geom.dpi_factor,
            "width":geom.inner_size.x,"height":geom.inner_size.y,
            "position":[geom.position.x,geom.position.y],"native_xid_verified":false});
        let circular = self.circular_observation();
        let document = self.document.as_ref().map(|d|json!({"uuid":d.document_uuid.to_string(),
            "runtime":d.runtime_instance_id(),"open_generation":self.api_document_generation,"revision":d.revision()}));
        let camera3d = if self.circular_ui.panel.active && self.circular_ui.panel.view_3d {
            let viewport = self.ui.cad_viewport(cx,ids!(circular_viewport));
            let value = viewport.borrow().ok_or("GUI_3D_VIEWPORT_NOT_READY")?;
            let count = value.viewer().scene().objects().take(4097).count();
            life::ensure(count <= 4096,"GUI_3D_SCENE_CAP")?;
            let visible = value.viewer().scene().objects().filter(|v|v.visible).take(4097).count();
            json!({"camera":format!("{:?}",value.viewer().camera()),"objects":count,"visible_objects":visible,"rect":rect_json(value.rect()),
                "host_visible":self.ui.widget(cx,ids!(circular_viewport_host)).visible(),"completed_GPU_serial":null})
        } else { Value::Null };
        let mut model = json!({"document":document,"circular":&circular,"active":self.circular_ui.panel.active,
            "view_3d":self.circular_ui.panel.view_3d,"view3d":camera3d,"worker_quiescent":self.circular_is_quiescent()});
        if persistence::enabled() { model["persistence"] = self.gui_persistence_snapshot(cx)?; }
        life::ensure(serde_json::to_vec(&model).map_err(|_|"GUI_STATE_SERIALIZATION")?.len() <= 64 << 10,"GUI_STATE_CAP")?;
        let canvas_ref = self.ui.drafting_canvas(cx,ids!(drafting_canvas));
        let canvas = if let Some(value) = canvas_ref.borrow() {
            let t = &value.telemetry;
            let revision = self.document.as_ref().map_or(0,Document::revision);
            let matches = self.circular_ui.panel.active && !self.circular_ui.panel.view_3d &&
                canvas_ref.visible() && self.ui.widget(cx,ids!(drafting_2d_host)).visible() &&
                circular.get("frame_token").and_then(Value::as_str).is_some_and(|v|!v.is_empty() && v == t.snapshot_token) &&
                circular.get("display_generation").and_then(Value::as_u64) == Some(t.model_generation) &&
                circular.get("camera_revision").and_then(Value::as_u64) == Some(t.camera_revision) &&
                t.source_generation == self.api_document_generation && t.document_revision == revision;
            json!({"draw_count":t.draw_count,"snapshot_token":&t.snapshot_token,"source_generation":t.source_generation,
                "model_generation":t.model_generation,"document_revision":t.document_revision,
                "camera_revision":t.camera_revision,"matches_prepared_2d":matches})
        } else { json!({"matches_prepared_2d":false}) };
        let targets = Target::ALL.into_iter().filter(|target|persistence::enabled() || target.legacy())
            .map(|target|self.gui_measured_target(cx,target)).collect();
        Ok(Snapshot { serial,redraw:cx.redraw_id,created:Instant::now(),binding,window,model,canvas,targets })
    }
    /// Pre-issuance readiness only: never update camera, drain actions or refresh a lease.
    fn gui_post_click_2d_draw_ready(&self, cx: &mut Cx, before: &Snapshot, current: &Snapshot) -> bool {
        let canvas=self.ui.drafting_canvas(cx,ids!(drafting_canvas));
        let uid=canvas.widget_uid();
        if uid.0==0 {return false;}
        let pending_resize=canvas_resize_pending(&canvas,&cx.new_actions);
        let (rect,dpi,draw_count)=match canvas.borrow() {
            Some(widget)=>(widget.rect,widget.dpi_factor,widget.telemetry.draw_count),
            None=>return false,
        };
        let camera=self.circular_camera();
        let current_count=current.canvas.get("draw_count").and_then(Value::as_u64);
        let prepared=current.canvas.get("matches_prepared_2d").and_then(Value::as_bool)==Some(true) &&
            current_count==Some(draw_count);
        post_click_2d_draw_ready(canvas.widget_uid()==uid,pending_resize,prepared,rect,dpi,
            dvec2(camera.viewport_width,camera.viewport_height),
            before.canvas.get("draw_count").and_then(Value::as_u64),current_count)
    }
    /// Latest completed Draw is measured again within its exact live Area epoch.
    fn gui_latest_snapshot(&self, cx: &mut Cx, capture_diagnostic: bool) -> Result<Snapshot, String> {
        let latest=self.gui_hooks.snapshot.as_ref().ok_or("GUI_WAIT_FRESH_DRAW")?;
        life::ensure(latest.created.elapsed() <= MAX_SNAPSHOT_AGE &&
            latest.serial == self.gui_hooks.draw_serial && latest.redraw == cx.redraw_id,
            "GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
        let mut current=self.gui_snapshot_now(cx,latest.serial)?;
        let mut cause=RejectionCause::Source;let mut trace=AreaGuardTrace::default();
        let matches=same_source(latest,&current) &&
            {cause=RejectionCause::TargetCount;latest.targets.len() == current.targets.len()} &&
            {cause=RejectionCause::Area;latest.targets.iter().zip(&current.targets)
                .all(|(a,b)|trace.observe(a.area == b.area))};
        if !matches {report_capture_rejection(capture_diagnostic,RejectionStage::Latest,cause,trace,latest,&current,&cx.draw_lists);}
        life::ensure(matches,"GUI_SNAPSHOT_STATE_OR_AREA_CHANGED")?;
        current.created=latest.created; // validation never creates a new Draw time
        Ok(current)
    }
    fn gui_validate_issued(&self, cx: &mut Cx, issued: &Snapshot, capture_diagnostic: bool) -> Result<Snapshot, String> {
        let current=self.gui_latest_snapshot(cx,capture_diagnostic)?;
        life::ensure(lease_draw_admissible(issued.created.elapsed(),issued.serial,issued.redraw,
            current.serial,current.redraw,self.gui_hooks.draw_serial,cx.redraw_id) &&
            issued.created <= current.created,"GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
        let newer_draw=current.serial > issued.serial;
        let mut cause=RejectionCause::Source;let mut trace=AreaGuardTrace::default();
        let matches=same_source(issued,&current) &&
            {cause=RejectionCause::TargetCount;issued.targets.len() == current.targets.len()} &&
            {cause=RejectionCause::Area;issued.targets.iter().zip(&current.targets).all(|(a,b)|
                trace.observe(area_lease_matches(a.area,b.area,b.available,current_area_valid(&cx.draw_lists,b.area),newer_draw)))};
        if !matches {report_capture_rejection(capture_diagnostic,RejectionStage::Issued,cause,trace,issued,&current,&cx.draw_lists);}
        life::ensure(matches,"GUI_SNAPSHOT_STATE_OR_AREA_CHANGED")?;
        Ok(current)
    }
    fn gui_live_snapshot(&self, cx: &mut Cx, id: &str, capture_diagnostic: bool) -> Result<Snapshot, String> {
        let issued=self.gui_hooks.issued.as_ref().ok_or("GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
        life::ensure(issued.id() == id,"GUI_SNAPSHOT_STALE_OR_CONSUMED")?;
        self.gui_validate_issued(cx,issued,capture_diagnostic)?;
        Ok(issued.clone()) // preserve the issued Draw/id/age, not a refreshed lease
    }
    fn gui_abort(&mut self, code: &str) {
        self.gui_hooks.fatal = Some(code.to_owned()); self.gui_hooks.snapshot = None; self.gui_hooks.issued = None;
        if let Some(id) = self.gui_hooks.pending.take() {
            if let Some(op) = self.gui_hooks.operations.get_mut(&id) { op.phase = "ABORTED_OUTCOME_UNKNOWN"; op.error = Some(code.into()); }
        }
    }
    /// Ordinary/S0 behavior is unchanged without trusted startup opt-in and an
    /// authenticated GUI request. Full-closure receiver-write/hot-watcher/studio
    /// audit remains a Root admission obligation, not an occupancy-query claim.
    fn gui_install(&mut self, cx: &mut Cx, binding: &life::Binding) -> Result<(), String> {
        if self.gui_hooks.sender.is_some() { return Ok(()); }
        life::ensure(PROFILE_ENABLED.get()==Some(&true) && self.gui_hooks.fatal.is_none() && !Cx::has_studio_web_socket() &&
            supported_app_argv(),"GUI_PROFILE_NOT_EXCLUSIVE")?;
        life::ensure(self.gui_lifecycle_frozen(cx)? == *binding,"GUI_INSTALL_BINDING_CHANGED")?;
        self.gui_persistence_install(binding)?;
        self.gui_persistence_observe(cx)?;
        let (sender,receiver) = sync_channel(2);
        Cx::set_control_channel(receiver);
        self.gui_hooks.sender = Some(sender); self.gui_hooks.binding = Some(binding.clone());
        cx.redraw_all();
        Ok(())
    }
    pub(super) fn dispatch_geotech_gui_request(&mut self, cx: &mut Cx,
        request: &agent_api::ApiCommand) -> Result<Value, String> {
        self.require_api_token(request)?;
        life::ensure(PROFILE_ENABLED.get()==Some(&true),"GUI_TRUSTED_STARTUP_OPT_IN_REQUIRED")?;
        if self.gui_job_routed(request){return self.gui_job_dispatch(cx,request);}
        life::ensure(!self.gui_job_busy(),"GUI_OWNED_JOB_PENDING")?;
        let expected: life::Binding = serde_json::from_value(request.params.get("expected").cloned().ok_or("GUI_EXPECTED_REQUIRED")?)
            .map_err(|_|"GUI_EXPECTED_INVALID")?;
        let binding = self.gui_lifecycle_observe_request(cx,request,&expected)?; // one existing observer call only
        let id = request.id.as_str().filter(|v|life::identifier(v)).ok_or("GUI_REQUEST_ID_INVALID")?;
        life::ensure(self.gui_hooks.ids.len() < wire::REQUEST_COUNT && !self.gui_hooks.jobs.ids.contains(id) && self.gui_hooks.ids.insert(id.into()),"GUI_REQUEST_REPLAY_OR_CAP")?;
        if request.method != "diagnostic.gui.status" { life::ensure(self.gui_hooks.fatal.is_none(),"GUI_ABORTED_USE_LIFECYCLE_CLOSE")?; }
        self.gui_install(cx,&binding)?;
        match request.method.as_str() {
            "diagnostic.gui.snapshot" => {
                let _: wire::SnapshotParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                life::ensure(self.gui_hooks.pending.is_none(),"GUI_OPERATION_PENDING")?;
                let ready=match self.gui_hooks.issued.clone() {
                    Some(held)=>self.gui_validate_issued(cx,&held,false).map(|_|held),
                    None=>self.gui_latest_snapshot(cx,false),
                };
                match ready {
                    Ok(value)=>match issue_snapshot(&mut self.gui_hooks.issued,self.gui_hooks.consumed_serial,value) {
                        Ok(held)=>Ok(held.json()),
                        Err(code)=>{cx.redraw_all();Err(code)}
                    },
                    Err(code)=>{
                        // Keep an unconsumed lease even when a read cannot validate
                        // it. Repeated reads must not erase its id or refresh its TTL.
                        if self.gui_hooks.issued.is_some() {return Err(code);}
                        self.gui_hooks.snapshot=None;cx.redraw_all();Err("GUI_WAIT_FRESH_DRAW".into())
                    }
                }
            }
            "diagnostic.gui.input" => {
                let p: wire::InputParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                self.gui_persistence_input(cx,p)
            }
            "diagnostic.gui.capture_document" => {
                let p: wire::CaptureParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                self.gui_persistence_document_capture(cx,p)
            }
            "diagnostic.gui.click" => {
                let p: wire::ClickParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                life::ensure(p.target.legacy(),"GUI_LEGACY_CLICK_TARGET_OR_ID_INVALID")?;
                if persistence::enabled() {
                    let generation=self.gui_hooks.issued.as_ref().ok_or("GUI_SNAPSHOT_STALE_OR_CONSUMED")?.serial;
                    return self.gui_persistence_input(cx,wire::InputParams { expected:p.expected,snapshot_id:p.snapshot_id,
                        geometry_generation:generation,target_id:p.target,command_id:p.command_id,event:wire::InputEvent::Click {u:0.5,v:0.5} });
                }
                self.gui_command_available(&p.command_id)?;
                life::ensure(self.gui_hooks.clicks < wire::CLICK_COUNT && !cx.fingers.any_areas_captured(),"GUI_CLICK_CAP_OR_CAPTURE_BUSY")?;
                let snapshot = self.gui_live_snapshot(cx,&p.snapshot_id,false)?;
                let target = snapshot.targets.iter().find(|v|v.target == p.target && v.available).ok_or("GUI_TARGET_UNAVAILABLE")?;
                let point = target.clip.pos + target.clip.size * 0.5;
                let width=snapshot.window.get("width").and_then(Value::as_f64).ok_or("GUI_CLIENT_WIDTH")?;
                let height=snapshot.window.get("height").and_then(Value::as_f64).ok_or("GUI_CLIENT_HEIGHT")?;
                life::ensure(contains(target.rect,point) && contains(target.clip,point) &&
                    contains(Rect {pos:dvec2(0.0,0.0),size:dvec2(width,height)},point),"GUI_CLICK_POINT_OUTSIDE_VISIBLE_CLIENT")?;
                let uid = target.uid.ok_or("GUI_TARGET_UID_MISSING")?;
                let time = cx.seconds_since_app_start(); life::ensure(time.is_finite(),"GUI_CLOCK_INVALID")?;
                let kind = Kind::Click { target:p.target,uid,point,down_time:time,up_time:time+0.001 };
                consume_issued(&mut self.gui_hooks.issued,&mut self.gui_hooks.consumed_serial,&p.snapshot_id)?; self.gui_hooks.clicks += 1;
                self.gui_hooks.pending = Some(p.command_id.clone());
                self.gui_hooks.operations.insert(p.command_id.clone(),Operation { kind,before:snapshot,phase:"QUEUED",
                    down:false,up:false,down_returned:false,up_returned:false,clicked:false,post:None,image:None,error:None,polls:0 });
                let mods = RemoteKeyModifiers::default();
                let sent = self.gui_hooks.sender.as_ref().ok_or("GUI_CHANNEL_MISSING").and_then(|sender| {
                    sender.try_send(StudioToApp::MouseDown(RemoteMouseDown { button_raw_bits:1,x:point.x,y:point.y,time,modifiers:mods }))
                        .map_err(|_|"GUI_DOWN_QUEUE_FAILED")?;
                    sender.try_send(StudioToApp::MouseUp(RemoteMouseUp { button_raw_bits:1,x:point.x,y:point.y,time:time+0.001,modifiers:mods }))
                        .map_err(|_|"GUI_UP_QUEUE_FAILED")
                });
                if let Err(code) = sent { self.gui_abort(code); return Err(code.into()); }
                SignalToUI::set_ui_signal();
                Ok(json!({"command_id":p.command_id,"phase":"QUEUED","ordinary_click_proved":false}))
            }
            "diagnostic.gui.capture" => {
                let p: wire::CaptureParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                self.gui_command_available(&p.command_id)?;
                life::ensure(self.gui_hooks.captures < wire::CAPTURE_COUNT && !cx.fingers.any_areas_captured(),"GUI_CAPTURE_CAP_OR_INPUT_BUSY")?;
                let snapshot = self.gui_live_snapshot(cx,&p.snapshot_id,true)?;
                life::ensure(self.gui_hooks.last_action.as_ref().is_some_and(|v|same_source(v,&snapshot)),"GUI_CURRENT_POST_ACTION_DRAW_REQUIRED")?;
                life::ensure(snapshot.model.get("active").and_then(Value::as_bool) == Some(true),"GUI_CIRCULAR_CONTENT_INACTIVE")?;
                let content=self.gui_validate_issued(cx,&snapshot,true)?; // current content, original lease age unchanged
                let eligible=if self.circular_ui.panel.view_3d {
                    snapshot.model.pointer("/view3d/visible_objects").and_then(Value::as_u64).is_some_and(|v|v>0) &&
                    snapshot.model.pointer("/view3d/host_visible").and_then(Value::as_bool)==Some(true) &&
                    snapshot.model.pointer("/view3d/rect/width").and_then(Value::as_f64).is_some_and(|v|v>0.0) &&
                    snapshot.model.pointer("/view3d/rect/height").and_then(Value::as_f64).is_some_and(|v|v>0.0)
                } else {content.canvas.get("matches_prepared_2d").and_then(Value::as_bool)==Some(true)};
                life::ensure(eligible,"GUI_ELIGIBLE_POST_ACTION_CONTENT_REQUIRED")?;
                let path = PathBuf::from(life::ROOT).join(format!("phase2-capture-{:02}.png",self.gui_hooks.captures+1));
                require_absent(&path)?;
                consume_issued(&mut self.gui_hooks.issued,&mut self.gui_hooks.consumed_serial,&p.snapshot_id)?; self.gui_hooks.captures += 1;
                self.gui_hooks.pending = Some(p.command_id.clone());
                self.gui_hooks.operations.insert(p.command_id.clone(),Operation { kind:Kind::Capture { path,requested:None },
                    before:snapshot,phase:"WAITING_POST_DRAW",down:false,up:false,down_returned:false,up_returned:false,
                    clicked:false,post:None,image:None,error:None,polls:0 });
                cx.redraw_all();
                Ok(json!({"command_id":p.command_id,"phase":"WAITING_POST_DRAW","GPU_completion_proved":false}))
            }
            "diagnostic.gui.status" => {
                let p: wire::StatusParams = serde_json::from_value(request.params.clone()).map_err(|_|"GUI_PARAMS_INVALID")?;
                let op = self.gui_hooks.operations.get_mut(&p.command_id).ok_or("GUI_COMMAND_UNKNOWN")?;
                life::ensure(op.polls < wire::STATUS_POLLS,"GUI_STATUS_POLL_CAP")?; op.polls += 1;
                self.gui_capture_status(cx,&p.command_id)?;
                let mut result=self.gui_hooks.operations.get(&p.command_id).ok_or("GUI_COMMAND_UNKNOWN")?.json();
                if let Some(input)=self.gui_persistence_status(&p.command_id){result["input"]=input;}
                Ok(result)
            }
            _ => Err("GUI_METHOD_UNKNOWN".into()),
        }
    }
    fn gui_command_available(&self, id: &str) -> Result<(), String> {
        life::ensure(life::identifier(id) && self.gui_hooks.pending.is_none() && !self.gui_job_busy() && !self.gui_hooks.jobs.attempts.contains(id) &&
            self.gui_hooks.operations.len() < wire::COMMAND_COUNT && !self.gui_hooks.operations.contains_key(id),"GUI_COMMAND_REPLAY_CAP_OR_BUSY")
    }
    pub(super) fn gui_before_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.gui_hooks.sender.is_none() { return true; }
        if let Some(allowed)=self.gui_job_before_event(cx,event){return allowed;}
        if let Some(allowed)=self.gui_persistence_before_event(cx,event){return allowed;}
        // Refuse genuine input commands outside the pending primary Down/Up pair.
        // Internal focus, passive/device, window/lifecycle and service events still flow.
        if matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) |
            Event::TextRangeReplace(_) | Event::TextCopy(_) | Event::TextCut(_) | Event::ImeAction(_) |
            Event::SelectionHandleDrag(_) | Event::TouchUpdate(_) | Event::LongPress(_) |
            Event::Drag(_) | Event::Drop(_) | Event::DragEnd | Event::BackPressed { .. } | Event::MacosMenuCommand(_)) {
            self.gui_abort("GUI_UNSUPPORTED_INPUT"); return false;
        }
        if matches!(event,Event::LiveEdit | Event::ScriptReapply) { self.gui_abort("GUI_HOT_RELOAD_REFUSED"); return true; }
        // Passive pointer motion is outside this four-button proof mode.
        if matches!(event,Event::MouseMove(_)) { return false; }
        if matches!(event,Event::Scroll(_)) { self.gui_abort("GUI_UNEXPECTED_SCROLL"); return false; }
        let (down,point,time,window,button,modifiers) = match event {
            Event::MouseDown(v) => (true,v.abs,v.time,v.window_id,v.button,v.modifiers),
            Event::MouseUp(v) => (false,v.abs,v.time,v.window_id,v.button,v.modifiers),
            _ => return true,
        };
        if self.gui_hooks.fatal.is_some() { return false; }
        let Some(id) = self.gui_hooks.pending.clone() else { self.gui_abort("GUI_UNEXPECTED_MOUSE_INPUT"); return false; };
        let Some(op) = self.gui_hooks.operations.get(&id).cloned() else { self.gui_abort("GUI_PENDING_MISSING"); return false; };
        let Kind::Click { point:expected,down_time,up_time,.. } = op.kind else { self.gui_abort("GUI_MOUSE_DURING_CAPTURE"); return false; };
        let check = (|| -> Result<(),String> {
            life::ensure(window == WindowId(0,0) && button.bits() == 1 && !modifiers.shift && !modifiers.control &&
                !modifiers.alt && !modifiers.logo && same_point(point,expected) &&
                time.to_bits() == (if down {down_time}else{up_time}).to_bits() &&
                ((down && !op.down && !op.up) || (!down && op.down && !op.up)),"GUI_INPUT_TUPLE_CHANGED")?;
            self.gui_validate_issued(cx,&op.before,false).map_err(|_|"GUI_INPUT_SNAPSHOT_CHANGED".to_owned())?;
            if down { life::ensure(!cx.fingers.any_areas_captured(),"GUI_EXISTING_CAPTURE")?; }
            Ok(())
        })();
        if let Err(code) = check { self.gui_abort(&code); return false; }
        if let Some(value) = self.gui_hooks.operations.get_mut(&id) { if down {value.down=true}else{value.up=true} }
        true
    }
    pub(super) fn gui_after_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.gui_job_after_event(cx,event){return;}
        if self.gui_persistence_after_event(cx,event){return;}
        if self.gui_hooks.sender.is_none() || self.gui_hooks.fatal.is_some() { return; }
        if let Some(id)=self.gui_hooks.pending.clone() {
            if let Some(op)=self.gui_hooks.operations.get_mut(&id) {
                if matches!(event,Event::MouseDown(_)) && op.down {op.down_returned=true;}
                if matches!(event,Event::MouseUp(_)) && op.up {op.up_returned=true;}
            }
        }
        if let Event::Actions(actions) = event {
            if let Some(id) = self.gui_hooks.pending.clone() {
                if let Some(op) = self.gui_hooks.operations.get(&id).cloned() {
                    if let Kind::Click { target,uid,.. } = op.kind {
                        let widget = self.gui_target_widget(cx,target);
                        let clicked = match target { Target::WorkspaceCircular => self.ui.button(cx,ids!(workspace_circular)).clicked(actions),
                            Target::Gcc2d => self.ui.button(cx,ids!(gcc_2d)).clicked(actions),
                            Target::Gcc3d => self.ui.button(cx,ids!(gcc_3d)).clicked(actions),
                            Target::GccFit => self.ui.button(cx,ids!(gcc_fit)).clicked(actions),
                            Target::PathInput|Target::SaveDocument|Target::OpenDocument|Target::NewDocument=>false };
                        if clicked {
                            if !op.down || !op.up || !op.down_returned || !op.up_returned ||
                                widget.try_widget_uid().map(|v|v.0) != Some(uid) { self.gui_abort("GUI_CLICK_UID_OR_ORDER_CHANGED"); return; }
                            if let Some(value) = self.gui_hooks.operations.get_mut(&id) { value.clicked=true; value.phase="CLICKED_WAITING_DRAW"; }
                            cx.redraw_all();
                        }
                    }
                }
            }
        }
        if !matches!(event,Event::Draw(_)) { return; }
        let Some(serial) = self.gui_hooks.draw_serial.checked_add(1) else { self.gui_abort("GUI_DRAW_SERIAL_EXHAUSTED"); return; };
        let current = match self.gui_snapshot_now(cx,serial) { Ok(v)=>v,Err(code)=>{self.gui_abort(&code);return;} };
        self.gui_hooks.draw_serial=serial; self.gui_hooks.snapshot=Some(current.clone());
        let Some(id) = self.gui_hooks.pending.clone() else { return; };
        let Some(op) = self.gui_hooks.operations.get(&id).cloned() else { self.gui_abort("GUI_PENDING_MISSING"); return; };
        match op.kind {
            Kind::Click { target,.. } if op.clicked && op.down_returned && op.up_returned => {
                // Intended workspace/view/camera changes may differ; window and document must not.
                if op.before.created.elapsed() > MAX_SNAPSHOT_AGE || op.before.binding != current.binding ||
                    op.before.window != current.window || op.before.model.get("document") != current.model.get("document") {
                    self.gui_abort("GUI_CLICK_POST_DRAW_CONTEXT_CHANGED_OR_EXPIRED"); return;
                }
                if self.circular_ui.panel.active && !self.circular_ui.panel.view_3d {
                    // TTL/binding/window/document checks above always run before waiting.
                    if self.gui_hooks.issued.is_some() {self.gui_abort("GUI_OPERATION_PENDING");return;}
                    if !self.gui_post_click_2d_draw_ready(cx,&op.before,&current) {
                        // SDK handles queued Resize normally after this callback; ask for its next Draw.
                        cx.redraw_all();return;
                    }
                }
                let expected_view = match target { Target::Gcc2d => Some(false),Target::Gcc3d => Some(true),_=>None };
                let view_matches = expected_view.map(|v|self.circular_ui.panel.view_3d == v);
                let toggled = if target == Target::WorkspaceCircular { Some(current.model.get("active") != op.before.model.get("active")) } else { None };
                if let Some(value)=self.gui_hooks.operations.get_mut(&id) {
                    value.phase="ORDINARY_CLICK_AND_POST_DRAW_OBSERVED";
                    value.post=Some(json!({"snapshot":current.json(),"view_postcondition":view_matches,"workspace_toggled":toggled,
                        "Fit_handler_success":"NOT_INFERRED_FROM_CLICK","3D_semantics":"extrusion_of_2D_section"}));
                }
                if self.gui_hooks.issued.is_some() {self.gui_abort("GUI_OPERATION_PENDING");return;}
                if let Err(code)=issue_snapshot(&mut self.gui_hooks.issued,self.gui_hooks.consumed_serial,current.clone()) {
                    self.gui_abort(&code);return;
                }
                self.gui_hooks.pending=None;
                self.gui_hooks.last_action=Some(current.clone());
            }
            Kind::DocumentCapture { path,requested:None } => {
                if !same_source(&op.before,&current) || !self.circular_is_quiescent() {
                    self.gui_abort("GUI_DOCUMENT_CAPTURE_SOURCE_OR_WORKER_CHANGED");return;
                }
                if let Err(code)=require_absent(&path){self.gui_abort(&code);return;}
                if let Some(value)=self.gui_hooks.operations.get_mut(&id){
                    value.kind=Kind::DocumentCapture {path:path.clone(),requested:Some(current.clone())};
                    value.phase="CAPTURE_REQUESTED_ASSOCIATION_PENDING";value.post=Some(current.json());
                }
                cx.capture_next_frame_to_file(path);
            }
            Kind::Capture { path,requested:None } => {
                if !same_source(&op.before,&current) { self.gui_abort("GUI_CAPTURE_SOURCE_CHANGED_BEFORE_DRAW"); return; }
                let view3d = self.circular_ui.panel.view_3d;
                let eligible = if view3d { current.model.pointer("/view3d/visible_objects").and_then(Value::as_u64).is_some_and(|v|v>0) &&
                    current.model.pointer("/view3d/host_visible").and_then(Value::as_bool) == Some(true) }
                    else { current.canvas.get("matches_prepared_2d").and_then(Value::as_bool)==Some(true) &&
                        current.canvas.get("draw_count").and_then(Value::as_u64) > op.before.canvas.get("draw_count").and_then(Value::as_u64) };
                if !eligible { self.gui_abort("GUI_CURRENT_CONTENT_UNAVAILABLE"); return; }
                if let Err(code)=require_absent(&path) { self.gui_abort(&code);return; }
                if let Some(value)=self.gui_hooks.operations.get_mut(&id) {
                    value.kind=Kind::Capture { path:path.clone(),requested:Some(current.clone()) };
                    value.phase="CAPTURE_REQUESTED_ASSOCIATION_PENDING"; value.post=Some(current.json());
                }
                cx.capture_next_frame_to_file(path);
            }
            _ => {}
        }
    }
    fn gui_capture_status(&mut self, cx: &mut Cx, id: &str) -> Result<(),String> {
        let op=self.gui_hooks.operations.get(id).cloned().ok_or("GUI_COMMAND_UNKNOWN")?;
        let (path,requested)=match op.kind {
            Kind::Capture {path,requested:Some(v)}|Kind::DocumentCapture {path,requested:Some(v)}=>(path,v),
            _=>return Ok(()),
        };
        if op.image.is_some() || op.error.is_some() {return Ok(());}
        let current=self.gui_snapshot_now(cx,self.gui_hooks.draw_serial)?;
        if !same_source(&requested,&current) || current.serial < requested.serial {
            self.gui_abort("GUI_CAPTURE_SOURCE_CHANGED_AFTER_DRAW"); return Ok(());
        }
        let image=match read_png_header(&path,&current.window) { Ok(v)=>v,Err(code)=>{self.gui_abort(&code);return Ok(());} };
        if let Some(image)=image {
            let after=self.gui_snapshot_now(cx,self.gui_hooks.draw_serial)?;
            if !same_source(&current,&after) { self.gui_abort("GUI_CAPTURE_SOURCE_CHANGED_DURING_READ");return Ok(()); }
            if let Some(value)=self.gui_hooks.operations.get_mut(id) { value.phase="FILE_OBSERVED_REQUIRES_FULL_PNG_ARTIFACT_VALIDATION";value.image=Some(image);
                value.post=Some(json!({"requested_after_App_draw":requested.json(),"stable_before_read":current.json(),"stable_after_read":after.json(),
                    "GPU_completion_serial":null,"candidate_tier":"GPU_FRAME_SINGLE_WINDOW_ASSOCIATION",
                    "single_window_source_association_observed":true,"native_window_generation_callback":false,
                    "full_PNG_CRC_nonblank_validation_required_in_Root_artifact_stage":true})); }
            self.gui_hooks.pending=None;
        }
        Ok(())
    }
}
fn require_absent(path: &PathBuf) -> Result<(),String> {
    let root=fs::symlink_metadata(life::ROOT).map_err(|_|"GUI_PRIVATE_ROOT_MISSING")?;
    let uid=fs::metadata("/proc/self").map_err(|_|"GUI_PRIVATE_UID")?.uid();
    life::ensure(root.is_dir() && !root.file_type().is_symlink() && root.mode() & 0o777 == 0o700 && root.uid()==uid,"GUI_PRIVATE_ROOT_INVALID")?;
    match fs::symlink_metadata(path) {Err(v) if v.kind()==std::io::ErrorKind::NotFound=>Ok(()),_=>Err("GUI_CAPTURE_PATH_NOT_FRESH".into())}
}
fn read_png_header(path: &PathBuf, window: &Value) -> Result<Option<Value>,String> {
    let mut file=match OpenOptions::new().read(true).custom_flags(0x20000|0x800|0x80000).open(path) {
        Err(v) if v.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Ok(v)=>v,Err(_)=>return Err("GUI_CAPTURE_OPEN_FAILED".into())};
    let before=file.metadata().map_err(|_|"GUI_CAPTURE_METADATA")?;
    let uid=fs::metadata("/proc/self").map_err(|_|"GUI_PRIVATE_UID")?.uid();
    life::ensure(before.is_file() && before.uid()==uid && before.nlink()==1 && before.len() <= wire::PNG_CAP as u64,"GUI_CAPTURE_FILE_CAP_OR_TYPE")?;
    if before.len()<33 {return Ok(None);}
    let mut raw=Vec::new(); Read::by_ref(&mut file).take((wire::PNG_CAP+1) as u64).read_to_end(&mut raw).map_err(|_|"GUI_CAPTURE_READ")?;
    let after=file.metadata().map_err(|_|"GUI_CAPTURE_METADATA")?;
    let key=|v:&fs::Metadata|(v.dev(),v.ino(),v.len(),v.mtime(),v.mtime_nsec());
    if key(&before)!=key(&after) {return Ok(None);}
    life::ensure(raw.len()<=wire::PNG_CAP && raw.len()>=33 && &raw[..8]==b"\x89PNG\r\n\x1a\n" &&
        &raw[12..16]==b"IHDR" && raw[24]==8 && raw[25]==6,"GUI_PNG_HEADER_INVALID")?;
    let width=u32::from_be_bytes(raw[16..20].try_into().map_err(|_|"GUI_PNG_DIMENSIONS")?);
    let height=u32::from_be_bytes(raw[20..24].try_into().map_err(|_|"GUI_PNG_DIMENSIONS")?);
    let dpi=window.get("dpi_factor").and_then(Value::as_f64).ok_or("GUI_PNG_DPI")?;
    let w=window.get("width").and_then(Value::as_f64).ok_or("GUI_PNG_WIDTH")?;
    let h=window.get("height").and_then(Value::as_f64).ok_or("GUI_PNG_HEIGHT")?;
    life::ensure(width>0 && height>0 && width<=2048 && height<=1536 && width==(w*dpi).floor() as u32 &&
        height==(h*dpi).floor() as u32,"GUI_PNG_DIMENSIONS_MISMATCH_OR_CAP")?;
    life::remaining(life::trusted()?.deadline)?;
    Ok(Some(json!({"path":path.to_str(),"bytes":raw.len(),"sha256":format!("{:x}",Sha256::digest(&raw)),"width":width,"height":height,
        "validation":"PNG_HEADER_DIMENSIONS_STABLE_FD_SHA_ONLY","PNG_CRC_and_decode_validated":false,
        "GPU_exact_completion_proved":false,"compositor_presentation_proved":false})))
}

#[cfg(test)]
mod issued_lease_tests {
    use super::*;
    use makepad_widgets::makepad_draw::makepad_platform::{CxDrawListPool,InstanceArea,RectArea};
    fn fixture() -> Snapshot {
        Snapshot { serial:1,redraw:10,created:Instant::now(),
            binding:life::Binding { profile_id:life::PROFILE.into(),run_id:"run01".into(),session_id:"session01".into(),
                owner:"owner01".into(),pid:1,start_ticks:1,pid_namespace:"pid:[1]".into(),elf_sha256:"a".repeat(64),
                window:life::WindowKey {index:0,generation:0} },
            window:json!({"index":0,"generation":0,"width":1440.0,"height":900.0,"dpi_factor":1.0,"position":[0.0,0.0]}),
            model:json!({"document":{"uuid":"doc01","runtime":"runtime01","open_generation":2,"revision":4},
                "active":true,"view_3d":false,"worker_quiescent":true,"circular":{"display_generation":1}}),
            canvas:json!({"draw_count":1}),
            targets:vec![MeasuredTarget {target:Target::WorkspaceCircular,uid:Some(7),area:Area::Empty,
                rect:Rect {pos:dvec2(1.0,2.0),size:dvec2(30.0,20.0)},
                clip:Rect {pos:dvec2(1.0,2.0),size:dvec2(30.0,20.0)},enabled:true,visible:false,available:false}] }
    }
    #[test] fn new_completed_draw_can_preserve_original_lease() {
        assert!(lease_draw_admissible(Duration::from_millis(4000),1,10,2,11,2,11));
        assert!(lease_draw_admissible(Duration::from_millis(5000),1,10,2,11,2,11));
        assert!(!lease_draw_admissible(Duration::from_millis(5001),1,10,200,210,200,210));
    }
    #[test] fn clock_serial_and_latest_epoch_regressions_refused() {
        for (is,ir,ls,lr,live_s,live_r) in [(2,10,1,11,1,11),(1,11,2,10,2,10),
            (1,10,2,11,3,11),(1,10,2,11,2,12)] {
            assert!(!lease_draw_admissible(Duration::from_millis(1),is,ir,ls,lr,live_s,live_r));
        }
    }
    #[test] fn instance_area_only_redraw_field_may_change_cross_epoch() {
        let mut pool=CxDrawListPool::default();let first=pool.alloc();let other=pool.alloc();
        let a=InstanceArea {draw_list_id:first.id(),draw_item_id:2,instance_offset:3,instance_count:4,redraw_id:10};
        let b=InstanceArea {redraw_id:11,..a};
        assert!(area_lease_matches(Area::Instance(a),Area::Instance(b),true,true,true));
        assert!(!area_lease_matches(Area::Instance(a),Area::Instance(b),true,true,false));
        for changed in [InstanceArea {draw_list_id:other.id(),..b},InstanceArea {draw_item_id:9,..b},
            InstanceArea {instance_offset:9,..b},InstanceArea {instance_count:9,..b}] {
            assert!(!area_lease_matches(Area::Instance(a),Area::Instance(changed),true,true,true));
        }
    }
    #[test] fn rect_area_slot_and_variant_remain_exact() {
        let mut pool=CxDrawListPool::default();let first=pool.alloc();let other=pool.alloc();
        let a=RectArea {draw_list_id:first.id(),rect_id:3,redraw_id:10};
        let b=RectArea {redraw_id:11,..a};
        assert!(area_lease_matches(Area::Rect(a),Area::Rect(b),true,true,true));
        assert!(!area_lease_matches(Area::Rect(a),Area::Rect(RectArea {rect_id:4,..b}),true,true,true));
        assert!(!area_lease_matches(Area::Rect(a),Area::Rect(RectArea {draw_list_id:other.id(),..b}),true,true,true));
        let instance=InstanceArea {draw_list_id:first.id(),draw_item_id:3,instance_offset:0,instance_count:1,redraw_id:11};
        assert!(!area_lease_matches(Area::Rect(a),Area::Instance(instance),true,true,true));
    }
    #[test] fn selected_available_area_requires_valid_current_epoch() {
        let mut pool=CxDrawListPool::default();let first=pool.alloc();
        let a=Area::Rect(RectArea {draw_list_id:first.id(),rect_id:1,redraw_id:10});
        assert!(!area_lease_matches(a,a,true,false,false));
        assert!(area_lease_matches(a,a,true,true,false));
        assert!(!area_lease_matches(Area::Empty,Area::Empty,true,true,true));
    }
    #[test] fn unavailable_empty_stays_allowed_without_promotion() {
        assert!(area_lease_matches(Area::Empty,Area::Empty,false,false,true));
        let mut pool=CxDrawListPool::default();let first=pool.alloc();
        let a=Area::Rect(RectArea {draw_list_id:first.id(),rect_id:1,redraw_id:10});
        assert!(!area_lease_matches(Area::Empty,a,false,true,true));
        assert!(area_lease_matches(a,a,false,false,true));
        let b=Area::Rect(RectArea {draw_list_id:first.id(),rect_id:1,redraw_id:11});
        assert!(!area_lease_matches(a,b,false,false,true));
    }
    #[test] fn full_source_window_document_model_and_target_changes_refused() {
        let original=fixture();assert!(same_source(&original,&original.clone()));
        let mut changed=Vec::new();
        for key in ["width","height","dpi_factor","position","generation"] {
            let mut s=original.clone();s.window[key]=json!("changed");changed.push(s);
        }
        for key in ["uuid","runtime","open_generation","revision"] {
            let mut s=original.clone();s.model["document"][key]=json!("changed");changed.push(s);
        }
        for key in ["active","view_3d","worker_quiescent","circular"] {
            let mut s=original.clone();s.model[key]=json!("changed");changed.push(s);
        }
        let mut s=original.clone();s.binding.start_ticks+=1;changed.push(s);
        let mut s=original.clone();s.targets[0].uid=Some(8);changed.push(s);
        let mut s=original.clone();s.targets[0].rect.pos.x+=1.0;changed.push(s);
        let mut s=original.clone();s.targets[0].clip.size.x+=1.0;changed.push(s);
        let mut s=original.clone();s.targets[0].enabled=false;changed.push(s);
        let mut s=original.clone();s.targets[0].visible=true;changed.push(s);
        let mut s=original.clone();s.targets[0].available=true;changed.push(s);
        for s in changed {assert!(!same_source(&original,&s));}
    }
    #[test] fn consume_checks_id_and_takes_only_one_original_lease() {
        let original=fixture();let created=original.created;let id=original.id();let mut held=Some(original);let mut consumed=None;
        assert!(consume_issued(&mut held,&mut consumed,"wrong-id").is_err());
        assert_eq!(held.as_ref().unwrap().created,created);
        assert_eq!(held.as_ref().unwrap().id(),id);
        assert!(consume_issued(&mut held,&mut consumed,&id).is_ok());assert!(held.is_none());
        assert!(consume_issued(&mut held,&mut consumed,&id).is_err());
    }
    #[test] fn draw_time_changes_do_not_change_semantic_source() {
        let issued=fixture();let mut latest=issued.clone();latest.serial=2;latest.redraw=11;
        latest.canvas=json!({"draw_count":2});
        assert!(same_source(&issued,&latest));
        assert!(lease_draw_admissible(Duration::from_millis(4999),issued.serial,issued.redraw,
            latest.serial,latest.redraw,latest.serial,latest.redraw));
        assert!(!lease_draw_admissible(Duration::from_millis(5001),issued.serial,issued.redraw,
            latest.serial,latest.redraw,latest.serial,latest.redraw));
    }
    #[test] fn dropped_draw_list_is_rejected_before_geometry() {
        let mut pool=CxDrawListPool::default();let handle=pool.alloc();let id=handle.id();
        pool[id].redraw_id=10;
        let area=Area::Rect(RectArea {draw_list_id:id,rect_id:0,redraw_id:10});
        assert!(current_area_valid(&pool,area));drop(handle);
        assert!(pool.checked_index(id).is_some()); // SDK retains dropped slot before reuse
        assert!(pool.is_id_freed(id));assert!(!current_area_valid(&pool,area));
    }
    #[test] fn reallocated_draw_list_generation_cannot_inherit_issued_area() {
        let mut pool=CxDrawListPool::default();let first=pool.alloc();let old_id=first.id();
        pool[old_id].redraw_id=10;
        let old_area=Area::Rect(RectArea {draw_list_id:old_id,rect_id:0,redraw_id:10});
        drop(first);let second=pool.alloc();let new_id=second.id();pool[new_id].redraw_id=11;
        assert_ne!(old_id,new_id);
        let new_area=Area::Rect(RectArea {draw_list_id:new_id,rect_id:0,redraw_id:11});
        assert!(!current_area_valid(&pool,old_area));assert!(current_area_valid(&pool,new_area));
        assert!(!area_lease_matches(old_area,new_area,true,true,true));
    }
    #[test] fn stale_rect_and_empty_instance_fail_current_epoch_guard() {
        let mut pool=CxDrawListPool::default();let handle=pool.alloc();let id=handle.id();pool[id].redraw_id=11;
        let stale=Area::Rect(RectArea {draw_list_id:id,rect_id:0,redraw_id:10});
        let fresh=Area::Rect(RectArea {draw_list_id:id,rect_id:0,redraw_id:11});
        assert!(!current_area_valid(&pool,stale));assert!(current_area_valid(&pool,fresh));
        let empty=Area::Instance(InstanceArea {draw_list_id:id,draw_item_id:0,instance_offset:0,instance_count:0,redraw_id:11});
        assert!(!current_area_valid(&pool,empty));
    }
    #[test] fn cached_list_epoch_is_independent_of_other_list_or_global_draw() {
        let mut pool=CxDrawListPool::default();let first=pool.alloc();let other=pool.alloc();
        pool[first.id()].redraw_id=10;pool[other.id()].redraw_id=999;
        let a=Area::Rect(RectArea {draw_list_id:first.id(),rect_id:0,redraw_id:10});
        assert!(current_area_valid(&pool,a));
        assert!(area_lease_matches(a,a,true,true,true));
        assert!(lease_draw_admissible(Duration::from_millis(1),1,100,2,101,2,101));
        let future=Area::Rect(RectArea {draw_list_id:first.id(),rect_id:0,redraw_id:11});
        assert!(area_lease_matches(a,future,true,true,true));
        assert!(!area_lease_matches(future,a,true,true,true)); // per-list epoch cannot regress
        assert!(!area_lease_matches(a,future,true,true,false));
    }
    #[test] fn repeated_snapshot_handoff_keeps_unconsumed_original_id_and_ttl() {
        let mut original=fixture();original.created=Instant::now()-Duration::from_secs(10);
        let created=original.created;let original_id=original.id();let mut held=Some(original);
        let mut fresh=fixture();fresh.serial=2;fresh.redraw=11;
        let reply=issue_snapshot(&mut held,None,fresh).unwrap();
        assert_eq!(reply.id(),original_id);assert_eq!(reply.created,created);
        assert_eq!(held.as_ref().unwrap().created,created);
        assert!(!lease_draw_admissible(Duration::from_secs(10),reply.serial,reply.redraw,2,11,2,11));
    }
    #[test] fn repeated_snapshot_route_cannot_reissue_consumed_serial() {
        let original=fixture();let id=original.id();let mut held=None;let mut consumed=None;
        let first=issue_snapshot(&mut held,consumed,original.clone()).unwrap();
        assert_eq!(first.id(),id);
        consume_issued(&mut held,&mut consumed,&id).unwrap();
        assert_eq!(consumed,Some(original.serial));
        assert!(issue_snapshot(&mut held,consumed,original.clone()).is_err());assert!(held.is_none());
        let mut older=original.clone();older.serial=0;
        assert!(issue_snapshot(&mut held,consumed,older).is_err());assert!(held.is_none());
        let mut newer=original;newer.serial=2;newer.redraw=11;
        let next=issue_snapshot(&mut held,consumed,newer).unwrap();assert_eq!(next.serial,2);
        assert_eq!(consumed,Some(1)); // issuing a new Draw does not rewrite consumption history
    }
    #[test] fn capture_rejection_masks_identify_source_and_area_without_values() {
        let mut pool=CxDrawListPool::default();let handle=pool.alloc();let id=handle.id();pool[id].redraw_id=11;
        let mut before=fixture();before.targets[0].available=true;
        before.targets[0].area=Area::Instance(InstanceArea {draw_list_id:id,draw_item_id:2,instance_offset:3,instance_count:4,redraw_id:10});
        let mut after=before.clone();
        after.targets[0].area=Area::Instance(InstanceArea {draw_list_id:id,draw_item_id:9,instance_offset:8,instance_count:7,redraw_id:11});
        assert!(same_source(&before,&after)); // Area is outside source JSON, still refused by the unchanged Area guard.
        assert_eq!(target_difference_mask(&before,&after,Target::WorkspaceCircular,&pool),0x78);
        let line=capture_rejection_line(RejectionStage::Issued,RejectionCause::Area,AreaGuardTrace::default(),&before,&after,&pool);
        assert!(line.contains("source_diff=00"));assert!(line.contains("target_area_meta_diff=0078,0000,0000,0000"));
        after.model["circular"]["camera_revision"]=json!(9001);
        after.model["circular"]["frame_token"]=json!("MUST_NOT_ECHO_FRAME_TOKEN");
        after.model["circular"]["screen_triangles"]=json!(12345);
        after.model["circular"]["screen_segments"]=json!(67890);
        let line=capture_rejection_line(RejectionStage::Issued,RejectionCause::Area,AreaGuardTrace::default(),&before,&after,&pool);
        assert!(line.contains("source_diff=04"));assert!(line.contains("model_diff=02"));assert!(line.contains("circular_diff=00e4"));
        assert!(!line.contains("9001"));assert!(!line.contains("12345"));assert!(!line.contains("67890"));
        assert!(!line.contains("MUST_NOT_ECHO"));
    }
    #[test] fn capture_rejection_diagnostic_is_bounded_static_and_stage_specific() {
        let pool=CxDrawListPool::default();let before=fixture();let mut after=before.clone();
        after.binding.run_id="MUST_NOT_ECHO_RUN".into();after.window["position"]=json!([1234567,7654321]);
        after.model["document"]["uuid"]=json!("MUST_NOT_ECHO_DOCUMENT");
        after.model["circular"]["view_status"]=json!("MUST_NOT_ECHO_STATUS\n/private/path/token");
        after.model["circular"]["panel"]=json!({"status":"MUST_NOT_ECHO_PANEL"});
        after.targets[0].uid=Some(987654321);
        let latest=capture_rejection_line(RejectionStage::Latest,RejectionCause::Source,AreaGuardTrace::default(),&before,&after,&pool);
        let issued=capture_rejection_line(RejectionStage::Issued,RejectionCause::Source,AreaGuardTrace::default(),&before,&after,&pool);
        assert_eq!(latest.replace("stage=LATEST","stage=ISSUED"),issued);
        assert!(latest.starts_with("GUI_SNAPSHOT_REJECTION_V1 context=CAPTURE stage=LATEST "));
        assert!(latest.contains("source_diff=0f"));assert!(latest.contains("window_diff=20"));assert!(latest.contains("document_diff=01"));
        assert!(latest.contains("circular_diff=0011"));assert!(latest.contains("panel_diff=0000000000004000"));
        assert!(latest.contains("target_area_meta_diff=0100,0000,0000,0000"));
        for secret in ["MUST_NOT_ECHO","1234567","7654321","987654321","/private/path/token","\n"] {
            assert!(!latest.contains(secret));
        }
        assert!(latest.len()<512 && latest.is_ascii());
        assert!(latest.bytes().all(|v|v.is_ascii_uppercase() || v.is_ascii_lowercase() || v.is_ascii_digit() || b"_= ,".contains(&v)));
    }

    #[test] fn capture_rejection_trace_records_only_original_visited_predicates() {
        let mut pool=CxDrawListPool::default();let handle=pool.alloc();let id=handle.id();pool[id].redraw_id=11;
        let mut before=fixture();
        before.targets=Target::ALL.into_iter().map(|target| {
            let mut value=before.targets[0].clone();value.target=target;
            value.area=Area::Rect(RectArea {draw_list_id:id,rect_id:0,redraw_id:10});value
        }).collect();
        let mut after=before.clone();after.serial=2;after.redraw=11;
        after.targets[1].area=Area::Rect(RectArea {draw_list_id:id,rect_id:0,redraw_id:11});
        // All targets unavailable: raw Eq still rejects the second epoch change,
        // despite a newer completed Draw. Later targets must not be evaluated.
        assert!(same_source(&before,&after));assert_eq!(available_mask(&before),0);assert_eq!(available_mask(&after),0);
        let mut trace=AreaGuardTrace::default();let mut visited=0;
        let matches=same_source(&before,&after) && before.targets.len()==after.targets.len() &&
            before.targets.iter().zip(&after.targets).all(|(a,b)| {
                visited+=1;assert!(visited<=2);
                trace.observe(area_lease_matches(a.area,b.area,b.available,current_area_valid(&pool,b.area),true))
            });
        assert!(!matches);assert_eq!(visited,2);assert_eq!(trace.evaluated,0x3);assert_eq!(trace.failed,0x2);
        let line=capture_rejection_line(RejectionStage::Issued,RejectionCause::Area,trace,&before,&after,&pool);
        assert!(line.contains("guard=AREA area_eval=3 area_failed=2 available_before=0 available_after=0 newer_draw=1"));
        assert!(line.contains("source_diff=00"));assert!(line.contains("target_area_meta_diff=0000,0040,0000,0000"));
        after.binding.run_id="MUST_NOT_ECHO_EARLY_SOURCE".into();
        let mut source_trace=AreaGuardTrace::default();let mut source_visits=0;
        let matches=same_source(&before,&after) && before.targets.len()==after.targets.len() &&
            before.targets.iter().zip(&after.targets).all(|(a,b)| {
                source_visits+=1;source_trace.observe(a.area==b.area)
            });
        assert!(!matches);assert_eq!(source_visits,0);assert_eq!(source_trace.evaluated,0);assert_eq!(source_trace.failed,0);
        let line=capture_rejection_line(RejectionStage::Latest,RejectionCause::Source,source_trace,&before,&after,&pool);
        assert!(line.contains("guard=SOURCE area_eval=0 area_failed=0"));
        assert!(!line.contains("MUST_NOT_ECHO"));
    }

    struct ReadinessUidWidget {uid:WidgetUid}
    impl ScriptApply for ReadinessUidWidget {}
    impl WidgetNode for ReadinessUidWidget {
        fn widget_uid(&self)->WidgetUid {self.uid}
        fn walk(&mut self,_cx:&mut Cx)->Walk {Walk::default()}
        fn area(&self)->Area {Area::Empty}
        fn redraw(&mut self,_cx:&mut Cx) {}
    }
    impl Widget for ReadinessUidWidget {}
    fn readiness_canvas(uid:WidgetUid)->DraftingCanvasRef {
        use crate::drafting_canvas::DraftingCanvasWidgetRefExt;
        // UID adapter only; no Canvas draw/geometry or App runtime is claimed by this fixture.
        WidgetRef::new_with_inner(Box::new(ReadinessUidWidget {uid})).as_drafting_canvas()
    }
    fn readiness_action(uid:WidgetUid, action:CanvasAction)->makepad_widgets::makepad_draw::makepad_platform::Action {
        Box::new(WidgetAction {widget_uid:uid,data:None,action:Box::new(action),group:None})
    }
    #[test] fn post_click_resize_queue_uses_current_UID_without_consuming_actions() {
        use crate::drafting_canvas::CanvasInput;
        use makepad_widgets::makepad_draw::makepad_platform::ActionsBuf;
        let uid=WidgetUid::new();let foreign=WidgetUid::new();let canvas=readiness_canvas(uid);
        let mut actions:ActionsBuf=vec![
            readiness_action(foreign,CanvasAction::Resize {width:1440.0,height:900.0,dpi_factor:1.0}),
            readiness_action(uid,CanvasAction::Input(CanvasInput::Cancel))];
        assert!(!canvas_resize_pending(&canvas,&actions));
        actions.push(readiness_action(uid,CanvasAction::Resize {width:1440.0,height:900.0,dpi_factor:1.0}));
        // Same width/height does not permit ignoring a queued position/DPI-originated Resize.
        assert!(canvas_resize_pending(&canvas,&actions));assert!(canvas_resize_pending(&canvas,&actions));
        assert_eq!(actions.len(),3);
        let identities:Vec<_>=actions.iter().map(|a|a.downcast_ref::<WidgetAction>().unwrap().widget_uid).collect();
        assert_eq!(identities,vec![foreign,uid,uid]);
        match actions[2].downcast_ref::<WidgetAction>().unwrap().action.downcast_ref::<CanvasAction>().unwrap() {
            CanvasAction::Resize {width,height,dpi_factor}=>assert_eq!((*width,*height,*dpi_factor),(1440.0,900.0,1.0)),
            _=>panic!("typed Resize action changed"),
        }
        let rect=Rect {pos:dvec2(10.0,20.0),size:dvec2(1440.0,900.0)};
        assert!(!post_click_2d_draw_ready(true,true,true,rect,1.0,rect.size,Some(1),Some(2)));
        assert!(post_click_2d_draw_ready(true,false,true,rect,1.0,rect.size,Some(1),Some(2)));
    }
    #[test] fn post_click_2d_readiness_requires_new_prepared_draw() {
        let rect=Rect {pos:dvec2(10.0,20.0),size:dvec2(1440.0,900.0)};
        assert!(post_click_2d_draw_ready(true,false,true,rect,1.0,rect.size,Some(4),Some(5)));
        for (before,current) in [(Some(4),Some(4)),(Some(4),Some(3)),(None,Some(5)),(Some(4),None),(None,None)] {
            assert!(!post_click_2d_draw_ready(true,false,true,rect,1.0,rect.size,before,current));
        }
        assert!(!post_click_2d_draw_ready(false,false,true,rect,1.0,rect.size,Some(4),Some(5)));
        assert!(!post_click_2d_draw_ready(true,false,false,rect,1.0,rect.size,Some(4),Some(5)));
        assert!(!post_click_2d_draw_ready(true,true,true,rect,1.0,rect.size,Some(4),Some(5)));
    }
    #[test] fn post_click_2d_readiness_refuses_geometry_and_DPI_mismatch() {
        let rect=Rect {pos:dvec2(10.0,20.0),size:dvec2(1440.0,900.0)};
        for dpi in [0.0,-1.0,f64::NAN,f64::INFINITY] {
            assert!(!post_click_2d_draw_ready(true,false,true,rect,dpi,rect.size,Some(1),Some(2)));
        }
        for invalid in [
            Rect {pos:dvec2(f64::NAN,20.0),..rect},
            Rect {pos:dvec2(10.0,f64::INFINITY),..rect},
            Rect {size:dvec2(0.0,900.0),..rect},Rect {size:dvec2(1440.0,-1.0),..rect},
            Rect {size:dvec2(f64::NAN,900.0),..rect},Rect {size:dvec2(1440.0,f64::INFINITY),..rect}] {
            assert!(!post_click_2d_draw_ready(true,false,true,invalid,1.0,invalid.size,Some(1),Some(2)));
        }
        for camera in [dvec2(1439.0,900.0),dvec2(1440.0,899.0),dvec2(f64::NAN,900.0),dvec2(1440.0,f64::INFINITY)] {
            assert!(!post_click_2d_draw_ready(true,false,true,rect,1.0,camera,Some(1),Some(2)));
        }
    }
}
