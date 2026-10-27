//! Consumer-only ordinary Analyze/Report/SaveCalculation. SOURCE NOT EXECUTED.
use super::*;
use crate::{linux_geotech_persistence_contract as policy,geotech_circular_ui::{EditorContext,JobTicket,JobKind,ReportTicket,CircularUiIntent}};
use crate::{circular_host as host,circular_jobs as native_jobs,geotech_circular_reportworker::ReportKey};
use serde::Serialize;
#[path="linux_geotech_job_wire.rs"]pub(crate) mod jobwire;
#[path="linux_geotech_job_state.rs"]mod state;
use jobwire::Target as Role;
const REQUESTS:usize=32;const COMMANDS:usize=6;const POLLS:usize=8;
#[cfg(test)]#[path="linux_geotech_job_bridge_tests.rs"]mod tests;
#[derive(Default)]pub(super) struct Hooks {
    pub(super) ids:BTreeSet<String>,pub(super) attempts:BTreeSet<String>,issued:Option<Issued>,consumed:u64,pending:Option<String>,
    operations:BTreeMap<String,Command>,compute:Option<Compute>,report:Option<Report>,cleanup:Option<(String,life::Binding,state::Mouse)>,
    submit_attempts:u64,submit_accepted:u64,report_attempts:u64,report_accepted:u64,
}
#[derive(Clone)]struct Control {uid:u64,area:Area,rect:Rect,clip:Rect,enabled:bool,visible:bool}
#[derive(Clone)]struct Focus {uid:u64,area:Area,rect:Rect,clip:Rect,actual:bool,text:String,generation:u64}
#[derive(Clone)]struct Issued {base:Snapshot,role:Role,control:Control,focus:Focus,editor:EditorContext,next_ui:u64,
    version:state::Version,shape:Shape,fp:Value,compute:Option<Compute>,report:Option<Report>}
#[derive(Clone,PartialEq,Eq)]struct Shape {elements:BTreeMap<u64,String>,grounds:BTreeMap<u64,String>,cases:BTreeMap<u64,String>,
    archives:BTreeMap<u64,String>,operations:BTreeMap<u64,String>}
#[derive(Clone,PartialEq)]struct Compute {ticket:JobTicket,envelope:host::Submission,host_sequence:u64,request_digest:String,content_digest:String,summary:Value}
#[derive(Clone,PartialEq)]pub(crate) struct Report {pub(crate) ticket:ReportTicket,pub(crate) key:ReportKey,pub(crate) sequence:u64,
    pub(crate) archive_digest:String,pub(crate) archive_sha256:String,pub(crate) markdown_sha256:String}
#[derive(Clone)]struct Command {before:Issued,pair:state::Pair,clicked:bool,intent_started:bool,intent_returned:bool,intent_ok:bool,
    job_ticket:Option<JobTicket>,report_ticket:Option<ReportTicket>,envelope:Option<host::Submission>,submit_returned:bool,
    accepted:Option<(u64,String)>,report_key:Option<ReportKey>,report_sequence:Option<u64>,report_start_returned:bool,
    archive:state::ArchiveTransition,commit_attempted:bool,commit:Option<Value>,post_editor:Option<EditorContext>,terminal:Option<Value>,complete:bool,
    error:Option<String>,polls:usize,cleanup_returned:bool,focus_after_down:Option<bool>}
fn ensure(value:bool,error:&str)->Result<(),String>{life::ensure(value,error)}
fn canonical(value:Value)->Value{match value{Value::Object(map)=>{let mut entries:Vec<_>=map.into_iter().collect();entries.sort_by(|a,b|a.0.cmp(&b.0));
    let mut out=serde_json::Map::new();for(key,value)in entries{out.insert(key,canonical(value));}Value::Object(out)},Value::Array(values)=>Value::Array(values.into_iter().map(canonical).collect()),other=>other}}
fn digest<T:Serialize>(value:&T)->Result<String,String>{let value=canonical(serde_json::to_value(value).map_err(|_|"JOB_DIGEST_SERIALIZATION")?);
    let bytes=serde_json::to_vec(&value).map_err(|_|"JOB_DIGEST_SERIALIZATION")?;
    ensure(bytes.len()<=64<<20,"JOB_DIGEST_CAP")?;Ok(format!("{:x}",Sha256::digest(bytes)))}
fn version(model:&policy::ModelObservation)->state::Version {state::Version {uuid:model.document.uuid.clone(),runtime:model.document.runtime.clone(),
    generation:model.document.open_generation,revision:model.document.revision,semantic:model.document.semantic_digest,
    archive:model.document.archive_digest,report:model.document.report_digest}}
fn version_json(v:&state::Version)->Value{json!({"uuid":v.uuid,"runtime":v.runtime,"open_generation":v.generation,"revision":v.revision,
    "semantic_sha256":v.semantic.iter().map(|b|format!("{b:02x}")).collect::<String>(),
    "archive_sha256":v.archive.map(|v|v.iter().map(|b|format!("{b:02x}")).collect::<String>()),
    "report_sha256":v.report.map(|v|v.iter().map(|b|format!("{b:02x}")).collect::<String>())})}
fn fp()->Result<Value,String>{let report=geotech_core::circular::inspect_arithmetic_environment();ensure(report.accepted,"JOB_ACTUAL_UI_ARITHMETIC_REFUSED")?;
    // Sticky status flags are observations, not control settings. Pin all
    // inspected controls; the compute worker has its OWN mandatory actual report.
    Ok(json!({"architecture":report.architecture,"mxcsr_controls":report.mxcsr.map(|v|v&!0x3f),"x87_control_word":report.x87_control_word,
        "round_to_nearest":report.round_to_nearest,"flush_to_zero":report.flush_to_zero,"denormals_are_zero":report.denormals_are_zero,
        "exceptions_masked":report.exceptions_masked,"gradual_underflow_probe_passed":report.gradual_underflow_probe_passed,"accepted":report.accepted}))}
fn shape(doc:&Document)->Result<Shape,String>{
    ensure(doc.elements().len()<=4096&&doc.ground_sections().len()<=64&&doc.circular_cases().len()<=64&&doc.operation_tree().len()<=4096,"JOB_DOCUMENT_SHAPE_CAP")?;
    Ok(Shape {elements:doc.elements().map(|e|Ok((e.id.get(),digest(e)?))).collect::<Result<_,String>>()?,
        grounds:doc.ground_sections().iter().map(|(id,g)|Ok((id.get(),digest(g)?))).collect::<Result<_,String>>()?,
        cases:doc.circular_cases().iter().map(|(id,c)|Ok((id.get(),digest(&(c.analysis_id,&c.name,&c.definition,c.source_operation_id))?))).collect::<Result<_,String>>()?,
        archives:doc.circular_cases().iter().map(|(id,c)|Ok((id.get(),digest(&c.circular_snapshot)?))).collect::<Result<_,String>>()?,
        operations:doc.operation_tree().map(|op|Ok((op.id.get(),digest(op)?))).collect::<Result<_,String>>()?})
}
impl App {
    fn gui_job_check(&mut self,cx:&mut Cx,before:&Issued,command:Option<&Command>)->Result<(),String>{
        self.gui_persistence_observe(cx)?;let current=self.gui_snapshot_now(cx,self.gui_hooks.draw_serial)?;
        ensure(before.base.binding==current.binding&&before.base.window==current.window&&before.base.created.elapsed()<MAX_SNAPSHOT_AGE,"JOB_IMMUTABLE_LEASE_OR_ORIGINAL_TTL")?;
        ensure(!Cx::has_studio_web_socket()&&supported_app_argv()&&fp()?==before.fp,"JOB_SOURCE_OR_ARITHMETIC_CONTROLS_CHANGED")?;
        let control=self.gui_job_control(cx,before.role)?;
        ensure(control.uid==before.control.uid&&control.visible&&rect_json(control.rect)==rect_json(before.control.rect)
            &&rect_json(control.clip)==rect_json(before.control.clip)&&area_lease_matches(before.control.area,control.area,true,
                current_area_valid(&cx.draw_lists,control.area),current.redraw>before.base.redraw),"JOB_BUTTON_LAYOUT_OR_AREA_CHANGED")?;
        if !command.is_some_and(|c|c.clicked){ensure(control.enabled,"JOB_BUTTON_DISABLED_BEFORE_ACTION")?;}
        let focus=self.gui_job_focus(cx)?;
        ensure(focus.uid==before.focus.uid&&focus.text==before.focus.text&&focus.generation==before.focus.generation
            &&rect_json(focus.rect)==rect_json(before.focus.rect)&&rect_json(focus.clip)==rect_json(before.focus.clip)
            &&area_lease_matches(before.focus.area,focus.area,true,current_area_valid(&cx.draw_lists,focus.area),current.redraw>before.base.redraw),"JOB_PATH_TYPE_UID_AREA_OR_TEXT_CHANGED")?;
        let expected_focus=command.and_then(|c|c.focus_after_down).unwrap_or(before.focus.actual);
        ensure(focus.actual==expected_focus,"JOB_REAL_CX_FOCUS_CHANGED")?;
        let editor=self.circular_ui.panel.editor_context()?;
        let expected_editor=command.and_then(|c|c.post_editor.as_ref()).unwrap_or(&before.editor);
        ensure(&editor==expected_editor&&!self.circular_ui.panel.dirty&&!self.circular_ui.panel.stale,"JOB_TYPED_EDITOR_CONTEXT_CHANGED")?;
        let observed=version(&self.gui_persistence_model_observation()?);
        ensure(command.map_or(before.version==observed,|c|c.archive.observe(&observed)),"JOB_DOCUMENT_TRANSITION_CHANGED")?;
        if !command.is_some_and(|c|c.commit.is_some()){ensure(shape(self.document.as_ref().ok_or("JOB_NO_DOCUMENT")?)?==before.shape,"JOB_HISTORY_OR_IDS_CHANGED")?;}
        Ok(())
    }
    fn gui_job_queue(&mut self,id:&str)->Result<(),String>{let command=self.gui_hooks.jobs.operations.get(id).ok_or("JOB_COMMAND_UNKNOWN")?;
        let Some(member)=command.pair.next()else{return Ok(());};let tuple=command.pair.tuple(member);
        ensure(!Cx::has_studio_web_socket()&&supported_app_argv(),"JOB_CHANNEL_ASSURANCE")?;
        let event=if member==state::Member::Down{StudioToApp::MouseDown(RemoteMouseDown{button_raw_bits:1,x:tuple.x,y:tuple.y,time:tuple.time,modifiers:RemoteKeyModifiers::default()})}
            else{StudioToApp::MouseUp(RemoteMouseUp{button_raw_bits:1,x:tuple.x,y:tuple.y,time:tuple.time,modifiers:RemoteKeyModifiers::default()})};
        self.gui_hooks.sender.as_ref().ok_or("JOB_CHANNEL_MISSING")?.try_send(event).map_err(|_|"JOB_QUEUE_REFUSED")?;
        // Post-success enqueue state MUST precede any order refusal.
        self.gui_hooks.jobs.operations.get_mut(id).ok_or("JOB_COMMAND_UNKNOWN")?.pair.sent(member).map_err(str::to_owned)?;
        SignalToUI::set_ui_signal();Ok(())
    }
    pub(crate) fn gui_job_before_event(&mut self,cx:&mut Cx,event:&Event)->Option<bool>{
        if let Some((_,binding,tuple))=self.gui_hooks.jobs.cleanup.clone(){if mouse_is(event,tuple){return Some(self.gui_lifecycle_frozen(cx).ok().as_ref()==Some(&binding));}}
        let id=self.gui_hooks.jobs.pending.clone()?;
        if self.gui_hooks.jobs.operations.get(&id).is_some_and(|c|c.error.is_some()){
            return Some(!matches!(event,Event::MouseDown(_)|Event::MouseUp(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)|Event::Actions(_)));
        }
        let result=(||->Result<(),String>{let mut command=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;
            self.gui_job_check(cx,&command.before,Some(&command))?;
            if matches!(event,Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)|Event::TextRangeReplace(_)|Event::TextCopy(_)|Event::TextCut(_)
                |Event::ImeAction(_)|Event::SelectionHandleDrag(_)|Event::TouchUpdate(_)|Event::LongPress(_)|Event::Drag(_)|Event::Drop(_)|Event::DragEnd
                |Event::BackPressed{..}|Event::MacosMenuCommand(_)|Event::LiveEdit|Event::ScriptReapply|Event::Scroll(_)){return Err("JOB_UNSUPPORTED_INPUT".into());}
            if let Some(tuple)=mouse(event){if tuple.down{ensure(!cx.fingers.any_areas_captured(),"JOB_FOREIGN_CAPTURE")?;}
                command.pair.admit(tuple).map_err(str::to_owned)?;}
            if let Event::Actions(actions)=event{
                for role in [Role::Analyze,Role::Report,Role::Calculation]{if self.gui_job_clicked(cx,role,actions){
                    ensure(role==command.before.role&&!command.clicked&&command.pair.down_returned()
                        &&self.gui_job_control(cx,role)?.uid==command.before.control.uid,"JOB_FOREIGN_OR_REPEATED_ACTION")?;command.clicked=true;
                }}
                for target in Target::ALL{ensure(!self.gui_persistence_button_clicked(cx,target,actions),"JOB_FOREIGN_IDLE_BUTTON")?;}
            }
            self.gui_hooks.jobs.operations.insert(id.clone(),command);Ok(())})();
        if let Err(error)=result{self.gui_job_abort(cx,&error);return Some(false);}
        Some(!matches!(event,Event::MouseMove(_)))
    }
    pub(crate) fn gui_job_after_event(&mut self,cx:&mut Cx,event:&Event)->bool{
        if let Some((id,binding,tuple))=self.gui_hooks.jobs.cleanup.clone(){if mouse_is(event,tuple){
            let same=self.gui_lifecycle_frozen(cx).ok().as_ref()==Some(&binding);
            if let Some(command)=self.gui_hooks.jobs.operations.get_mut(&id){command.pair.release_returned(same);command.cleanup_returned=same;}
            self.gui_hooks.jobs.cleanup=None;return true;}}
        let Some(id)=self.gui_hooks.jobs.pending.clone()else{return false;};
        if let Some(error)=self.gui_hooks.jobs.operations.get(&id).and_then(|c|c.error.clone()){
            if self.gui_hooks.fatal.is_none(){self.gui_job_abort(cx,&error);}return true;
        }
        let result=(||->Result<bool,String>{let mut command=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;
            // A real ordinary Down may transfer key focus. Sample it directly
            // immediately after return, latch once, then preserve that sample.
            if matches!(event,Event::MouseDown(_)){command.focus_after_down=Some(self.gui_job_focus(cx)?.actual);command.pair.returned(state::Member::Down).map_err(str::to_owned)?;}
            if matches!(event,Event::MouseUp(_)){command.pair.returned(state::Member::Up).map_err(str::to_owned)?;}
            self.gui_job_check(cx,&command.before,Some(&command))?;
            self.gui_hooks.jobs.operations.insert(id.clone(),command.clone());
            if command.pair.next().is_some(){self.gui_job_queue(&id)?;}
            if !matches!(event,Event::Draw(_)){cx.redraw_all();return Ok(false);}
            let serial=self.gui_hooks.draw_serial.checked_add(1).ok_or("JOB_DRAW_GENERATION_EXHAUSTED")?;
            let current=self.gui_snapshot_now(cx,serial)?;self.gui_hooks.draw_serial=serial;self.gui_hooks.snapshot=Some(current.clone());
            if !command.pair.both_returned()||!command.clicked||!command.intent_returned{return Ok(false);}
            ensure(command.intent_ok,"JOB_ORDINARY_HANDLER_FAILED")?;
            if self.circular_ui.panel.active&&!self.circular_ui.panel.view_3d&&!self.gui_post_click_2d_draw_ready(cx,&command.before.base,&current){
                cx.redraw_all();return Ok(false);
            }
            let terminal=match command.before.role{
                Role::Analyze=>{
                    let Some((sequence,request))=command.accepted.clone()else{ensure(!command.submit_returned,"JOB_SUBMIT_NOT_ACCEPTED")?;return Ok(false);};
                    let ticket=command.job_ticket.as_ref().ok_or("JOB_NATIVE_TICKET_MISSING")?;let envelope=command.envelope.as_ref().ok_or("JOB_ORIGINAL_QUOTE_MISSING")?;
                    let Some(compute)=self.gui_job_compute_pending(ticket,envelope,sequence,&request)?else{return Ok(false);};
                    self.gui_hooks.jobs.compute=Some(compute.clone());self.gui_hooks.jobs.report=None;
                    json!({"compute":compute_json(&compute),"worker_terminal":true,"worker_reaped":true,"actual_worker_arithmetic_accepted":true})
                }
                Role::Report=>{
                    if !self.circular_is_quiescent(){return Ok(false);}
                    let report=self.gui_job_report_readback()?.ok_or("JOB_REPORT_COMPLETION_MISSING")?;
                    ensure(command.report_sequence==Some(report.sequence)&&command.report_key.as_ref()==Some(&report.key)
                        &&command.report_ticket.as_ref()==Some(&report.ticket),"JOB_REPORT_TERMINAL_BINDING")?;
                    self.gui_job_compute(command.before.compute.as_ref().ok_or("JOB_COMPUTE_REQUIRED")?)?;
                    self.gui_hooks.jobs.report=Some(report.clone());json!({"report":report_json(&report),"formatter_reaped":true})
                }
                Role::Calculation=>{ensure(command.archive.committed()&&command.commit.is_some(),"JOB_ARCHIVE_COMMIT_MISSING")?;
                    ensure(self.circular_is_quiescent(),"JOB_ARCHIVE_WORKER_NOT_QUIET")?;json!({"archive_commit":command.commit})}
            };
            ensure(current.serial>command.before.base.serial,"JOB_FRESH_DRAW_REQUIRED")?;
            command.pair.finish().map_err(str::to_owned)?;command.terminal=Some(json!({"observation":terminal,"document":version_json(&version(&self.gui_persistence_model_observation()?)),
                "post_snapshot":current.json(),"actual_path_focus":self.gui_job_focus(cx)?.actual,"exact_renderer_serial_proved":false,"acceptance":"NOT_GRANTED"}));command.complete=true;
            self.gui_hooks.jobs.operations.insert(id.clone(),command);self.gui_hooks.jobs.pending=None;self.gui_hooks.jobs.issued=None;
            self.gui_hooks.snapshot=Some(current);Ok(true)
        })();
        if let Err(error)=result{self.gui_job_abort(cx,&error);}true
    }
    fn gui_job_status(&self,id:&str)->Result<Value,String>{let c=self.gui_hooks.jobs.operations.get(id).ok_or("JOB_COMMAND_UNKNOWN")?;
        let certainty=match c.pair.certainty(){state::Certainty::NotDispatched=>"NOT_DISPATCHED",state::Certainty::OutcomeUnknown=>"OUTCOME_UNKNOWN",state::Certainty::ObservedComplete=>"OBSERVED_COMPLETE"};
        let value=json!({"phase":if c.complete{"ORDINARY_JOB_AND_POST_DRAW_OBSERVED"}else if c.error.is_some()&&certainty=="NOT_DISPATCHED"{"REFUSED"}else if c.error.is_some(){"ABORTED_OUTCOME_UNKNOWN"}else{"PENDING"},
            "job":{"target_id":c.before.role.name(),"dispatch_certainty":certainty,"ordinary_button_clicked":c.clicked,
                "mouse_pair_returned":c.pair.both_returned(),"release_required":c.pair.release_required(),"cleanup_release_returned":c.cleanup_returned,
                "intent_returned":c.intent_returned,"intent_ok":c.intent_ok,"actual_submit_returned":c.submit_returned,"actual_submit_accepted":c.accepted,
                "submit_attempts":self.gui_hooks.jobs.submit_attempts,"submit_accepted_count":self.gui_hooks.jobs.submit_accepted,
                "report_attempts":self.gui_hooks.jobs.report_attempts,"report_accepted_count":self.gui_hooks.jobs.report_accepted,"archive_commit_attempted":c.commit_attempted,
                "post":c.terminal,"error":c.error,"retry_allowed":false,"full_Scenario_D_acceptance":false}});
        ensure(serde_json::to_vec(&value).map_err(|_|"JOB_STATUS_SERIALIZATION")?.len()<=256<<10,"JOB_STATUS_CAP_NO_TRUNCATION")?;Ok(value)}
    pub(crate) fn gui_job_worker_window(&self)->bool{let Some(id)=self.gui_hooks.jobs.pending.as_ref()else{return false;};let Some(c)=self.gui_hooks.jobs.operations.get(id)else{return false;};
        match c.before.role{
            Role::Analyze=>self.circular_report.is_quiescent()&&self.circular_native.current.as_ref().is_some_and(|current|
                c.job_ticket.as_ref()==Some(&current.ticket)&&c.envelope.as_ref()==Some(&current.submission)
                &&c.envelope.as_ref().is_some_and(|e|e.sequence==current.host_sequence)),
            Role::Report=>self.circular_host.as_ref().is_none_or(|h|h.is_quiescent())&&c.report_key.as_ref()==self.circular_report.key()
                &&c.report_key.is_some()&&c.report_sequence==Some(self.circular_report.sequence()),Role::Calculation=>false}
    }
    fn gui_job_abort(&mut self,cx:&mut Cx,error:&str){let Some(id)=self.gui_hooks.jobs.pending.clone()else{self.gui_abort(error);return;};
        if let Some(mut command)=self.gui_hooks.jobs.operations.get(&id).cloned(){command.pair.abort();command.error=Some(error.into());
            if command.before.role==Role::Analyze{if let(Some(ticket),Some(envelope),Some(current))=(command.job_ticket.as_ref(),command.envelope.as_ref(),self.circular_native.current.as_ref()){
                if &current.ticket==ticket&&&current.submission==envelope&&current.host_sequence==envelope.sequence{
                    let ctx=self.circular_context();if let(Some(doc),Some(controller))=(self.document.as_ref(),self.circular_host.as_mut()){let _=controller.cancel(doc,&ctx,envelope.sequence);}
                }}}
            if command.before.role==Role::Report&&command.report_key.as_ref()==self.circular_report.key()&&command.report_key.is_some()
                &&command.report_sequence==Some(self.circular_report.sequence()){self.circular_report.cancel();}
            let same_target=self.gui_job_control(cx,command.before.role).ok().is_some_and(|target|target.uid==command.before.control.uid
                &&area_lease_matches(command.before.control.area,target.area,true,current_area_valid(&cx.draw_lists,target.area),cx.redraw_id>command.before.base.redraw));
            if command.pair.release_required()&&same_target&&self.gui_hooks.jobs.cleanup.is_none()&&self.gui_lifecycle_frozen(cx).ok().as_ref()==Some(&command.before.base.binding){
                let tuple=command.pair.tuple(state::Member::Up);let event=StudioToApp::MouseUp(RemoteMouseUp{button_raw_bits:1,x:tuple.x,y:tuple.y,time:tuple.time,modifiers:RemoteKeyModifiers::default()});
                if self.gui_hooks.sender.as_ref().is_some_and(|s|s.try_send(event).is_ok()){self.gui_hooks.jobs.cleanup=Some((id.clone(),command.before.base.binding.clone(),tuple));SignalToUI::set_ui_signal();}
            }
            self.gui_hooks.jobs.operations.insert(id,command);
        }self.gui_abort(error);
    }
}
fn mouse(event:&Event)->Option<state::Mouse>{let(down,v)=match event{Event::MouseDown(v)=>(true,(v.abs,v.time,v.window_id,v.button,v.modifiers)),Event::MouseUp(v)=>(false,(v.abs,v.time,v.window_id,v.button,v.modifiers)),_=>return None};
    Some(state::Mouse{down,x:v.0.x,y:v.0.y,time:v.1,window:(v.2.0,v.2.1),primary:v.3.bits()==1,modifiers:v.4.control||v.4.shift||v.4.alt||v.4.logo})}
fn mouse_is(event:&Event,expected:state::Mouse)->bool{mouse(event).is_some_and(|actual|actual.down==expected.down&&actual.window==expected.window
    &&actual.primary==expected.primary&&actual.modifiers==expected.modifiers&&actual.x.to_bits()==expected.x.to_bits()
    &&actual.y.to_bits()==expected.y.to_bits()&&actual.time.to_bits()==expected.time.to_bits())}
fn compute_json(c:&Compute)->Value{json!({"host_sequence":c.host_sequence,"ui_ticket_sequence":c.ticket.sequence,"request_digest":c.request_digest,"content_digest":c.content_digest,
    "original_envelope_sha256":digest(&c.envelope).ok(),"case_id":c.ticket.identity.analysis_id.get(),"input_fingerprint":c.ticket.identity.input_fingerprint,"result":c.summary})}
fn report_json(r:&Report)->Value{json!({"sequence":r.sequence,"compute_sequence":r.key.compute_sequence,"request_digest":r.key.request_digest,"content_digest":r.key.content_digest,
    "archive_digest":r.archive_digest,"archive_sha256":r.archive_sha256,"markdown_sha256":r.markdown_sha256})}

fn active(hooks:&mut GuiHooks)->Result<&mut Command,String>{let id=hooks.jobs.pending.as_ref().ok_or("JOB_ORDINARY_PERMIT_MISSING")?.clone();
    let command=hooks.jobs.operations.get_mut(&id).ok_or("JOB_ORDINARY_PERMIT_MISSING")?;
    ensure(command.clicked&&command.intent_started&&!command.intent_returned&&command.error.is_none(),"JOB_PERMIT_CONSUMED_OR_NOT_CLICKED")?;Ok(command)}
pub(crate) fn job_native_start(hooks:&mut GuiHooks,ticket:&JobTicket,search:Option<&crate::geotech_circular_ui::SearchBox>)->Result<(),String>{
    let c=active(hooks)?;ensure(c.before.role==Role::Analyze&&c.job_ticket.as_ref()==Some(ticket)&&ticket.kind==JobKind::Analyze
        &&ticket.search_spec.is_none()&&search.is_none()&&c.envelope.is_none(),"JOB_NATIVE_START_NOT_ADMITTED")
}
pub(crate) fn job_submit_before(hooks:&mut GuiHooks,ticket:&JobTicket,envelope:&host::Submission)->Result<(),String>{
    {let c=active(hooks)?;ensure(c.before.role==Role::Analyze&&c.job_ticket.as_ref()==Some(ticket)&&c.envelope.is_none()
        &&envelope.sequence.0!=0&&envelope.expected_revision.0==c.before.version.revision
        &&envelope.binding.case_id.0==ticket.identity.analysis_id.get()&&envelope.binding.ground_id.0==ticket.identity.ground_id.get()
        &&envelope.binding.case_source_operation.0==ticket.identity.case_source_operation_id.get()
        &&envelope.binding.ground_source_operation.0==ticket.identity.ground_source_operation_id.get()
        &&envelope.binding.runtime_id==c.before.version.runtime&&envelope.binding.document_uuid==c.before.version.uuid
        &&envelope.binding.open_generation.0==c.before.version.generation&&envelope.binding.mode==host::SubmissionMode::Specified
        &&matches!(&envelope.request,host::FullRequest::Specified{..}),"JOB_SUBMIT_ORIGINAL_BINDING")?;
        let _=digest(envelope)?;c.envelope=Some(envelope.clone());}
    // These are attempts, immediately before the real call. A later error
    // never decrements them or authorizes controller.submit replay.
    hooks.jobs.submit_attempts=hooks.jobs.submit_attempts.checked_add(1).ok_or("JOB_SUBMIT_COUNT_EXHAUSTED")?;
    super::persistence::record_actual_submit_attempt(&mut hooks.persistence)?;Ok(())
}
pub(crate) fn job_submit_returned(hooks:&mut GuiHooks,ticket:&JobTicket,envelope:&host::Submission,status:Option<&host::HostStatus>)->Result<(),String>{
    {let c=active(hooks)?;ensure(c.job_ticket.as_ref()==Some(ticket)&&c.envelope.as_ref()==Some(envelope)&&!c.submit_returned,"JOB_SUBMIT_RETURN_REPLAY")?;
        c.submit_returned=true;if let Some(status)=status{ensure(status.sequence==envelope.sequence&&!status.request_digest.is_empty(),"JOB_ACCEPTED_SEQUENCE")?;
            c.accepted=Some((status.sequence.0,status.request_digest.clone()));}}
    if status.is_some(){hooks.jobs.submit_accepted=hooks.jobs.submit_accepted.checked_add(1).ok_or("JOB_ACCEPTED_COUNT_EXHAUSTED")?;}Ok(())
}
pub(crate) fn job_report_before(hooks:&mut GuiHooks,ticket:&ReportTicket,sequence:u64,saved:&native_jobs::PublishedResult)->Result<(),String>{
    {let c=active(hooks)?;let compute=c.before.compute.as_ref().ok_or("JOB_REPORT_REQUIRES_OWNED_COMPUTE")?;
        ensure(c.before.role==Role::Report&&c.report_ticket.as_ref()==Some(ticket)&&c.report_key.is_none()&&sequence==compute.host_sequence
            &&saved.request_digest==compute.request_digest&&saved.content_digest==compute.content_digest&&saved.worker_arithmetic.accepted,"JOB_REPORT_START_BINDING")?;
        c.report_key=Some(ReportKey{compute_sequence:sequence,binding:saved.binding.clone(),request_digest:saved.request_digest.clone(),content_digest:saved.content_digest.clone()});}
    hooks.jobs.report_attempts=hooks.jobs.report_attempts.checked_add(1).ok_or("JOB_REPORT_COUNT_EXHAUSTED")?;Ok(())
}
pub(crate) fn job_report_returned(hooks:&mut GuiHooks,sequence:Option<u64>)->Result<(),String>{
    {let c=active(hooks)?;ensure(c.before.role==Role::Report&&c.report_key.is_some()&&!c.report_start_returned,"JOB_REPORT_RETURN_REPLAY")?;
        c.report_start_returned=true;c.report_sequence=sequence;}
    if sequence.is_some(){hooks.jobs.report_accepted=hooks.jobs.report_accepted.checked_add(1).ok_or("JOB_REPORT_ACCEPT_COUNT_EXHAUSTED")?;}Ok(())
}
pub(crate) fn job_report_metadata(ticket:&ReportTicket,key:&ReportKey,sequence:u64,archive:&geotech_report::ReportArchive)->Result<Report,String>{
    Ok(Report{ticket:ticket.clone(),key:key.clone(),sequence,archive_digest:archive.archive_digest.clone(),archive_sha256:digest(archive)?,
        markdown_sha256:format!("{:x}",Sha256::digest(archive.markdown.as_bytes()))})
}
impl App {
    pub(crate) fn gui_job_intent_begin(&mut self,cx:&mut Cx,intent:&CircularUiIntent)->Result<(),String>{
        if !self.gui_persistence_enabled(){return Ok(());}
        let (role,job,report)=match intent{CircularUiIntent::Analyze(t)=>(Role::Analyze,Some(t.clone()),None),
            CircularUiIntent::Report(t)=>(Role::Report,None,Some(t.clone())),CircularUiIntent::SaveSnapshot(t)=>(Role::Calculation,None,Some(t.clone())),
            CircularUiIntent::ViewChanged|CircularUiIntent::Fit=>return Ok(()),_=>return Err("JOB_OTHER_BUSINESS_INTENT_NOT_ADMITTED".into())};
        let id=self.gui_hooks.jobs.pending.clone().ok_or("JOB_NO_ORDINARY_COMMAND")?;let mut c=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;
        self.gui_job_check(cx,&c.before,Some(&c))?;
        ensure(c.before.role==role&&c.clicked&&c.pair.down_returned()&&!c.intent_started,"JOB_TYPED_INTENT_PERMIT")?;
        let identity=c.before.editor.resolved_identity.as_ref().ok_or("JOB_RESOLVED_IDENTITY_REQUIRED")?;
        if let Some(t)=job.as_ref(){ensure(t.sequence==c.before.next_ui&&t.search_generation==c.before.editor.search_generation&&t.kind==JobKind::Analyze
            &&&t.identity==identity&&t.search_spec.is_none(),"JOB_EXACT_NATIVE_TICKET")?;}
        if let Some(t)=report.as_ref(){ensure(&t.identity==identity&&t.search_generation==c.before.editor.search_generation&&t.candidate_source.is_none(),"JOB_EXACT_REPORT_TICKET")?;}
        c.intent_started=true;c.job_ticket=job;c.report_ticket=report;self.gui_hooks.jobs.operations.insert(id,c);Ok(())
    }
    pub(crate) fn gui_job_archive_before(&mut self,cx:&mut Cx,ticket:&ReportTicket)->Result<(),String>{
        if !self.gui_persistence_enabled(){return Ok(());}let id=self.gui_hooks.jobs.pending.clone().ok_or("JOB_ARCHIVE_NO_ORDINARY_PERMIT")?;
        let c=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;self.gui_job_check(cx,&c.before,Some(&c))?;
        ensure(c.before.role==Role::Calculation&&c.clicked&&c.intent_started&&!c.intent_returned&&!c.commit_attempted
            &&c.report_ticket.as_ref()==Some(ticket)&&c.before.report.as_ref()==self.gui_job_report_readback()?.as_ref(),"JOB_ARCHIVE_ENTRY_PERMIT")?;
        self.gui_job_compute(c.before.compute.as_ref().ok_or("JOB_ARCHIVE_OWNED_COMPUTE_REQUIRED")?)
    }
    pub(crate) fn gui_job_snapshot_commit_before(&mut self,cx:&mut Cx,expected:&crate::geotech_circular_ui::DocumentBinding,
        case_id:ai_revit_core::AnalysisCaseId,snapshot:&ai_revit_core::CircularCalculationSnapshot)->Result<(),String>{
        if !self.gui_persistence_enabled(){return Ok(());}let id=self.gui_hooks.jobs.pending.clone().ok_or("JOB_ARCHIVE_NO_ORDINARY_PERMIT")?;
        let c=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;self.gui_job_check(cx,&c.before,Some(&c))?;
        let report=c.before.report.as_ref().ok_or("JOB_ARCHIVE_OWNED_REPORT_REQUIRED")?;
        ensure(c.before.role==Role::Calculation&&c.clicked&&c.intent_started&&!c.intent_returned&&!c.commit_attempted&&c.commit.is_none()
            &&expected==&c.before.editor.document&&c.report_ticket.as_ref().is_some_and(|t|t.identity.analysis_id==case_id)
            &&snapshot.archive().archive_digest==report.archive_digest&&digest(snapshot.archive())?==report.archive_sha256,"JOB_ARCHIVE_ACTUAL_COMMIT_PERMIT")?;
        self.gui_hooks.jobs.operations.get_mut(&id).ok_or("JOB_COMMAND_UNKNOWN")?.commit_attempted=true;Ok(())
    }
    pub(crate) fn gui_job_intent_end(&mut self,cx:&mut Cx,ok:bool){if !self.gui_persistence_enabled(){return;}
        let Some(id)=self.gui_hooks.jobs.pending.clone()else{return;};let Some(mut c)=self.gui_hooks.jobs.operations.get(&id).cloned()else{return;};
        if !c.intent_started||c.intent_returned{return;}c.intent_returned=true;c.intent_ok=ok;
        if c.before.role==Role::Calculation&&c.commit.is_some(){match self.circular_ui.panel.editor_context(){
            Ok(editor)=>{let mut normalized=editor.clone();normalized.document.revision=c.before.editor.document.revision;
                if let(Some(now),Some(old))=(normalized.resolved_identity.as_mut(),c.before.editor.resolved_identity.as_ref()){now.document.revision=old.document.revision;}
                let revision=c.commit.as_ref().and_then(|r|r["revision"].as_u64());
                if normalized!=c.before.editor||revision!=Some(editor.document.revision){c.error=Some("JOB_POST_COMMIT_EDITOR_SOURCE_CHANGED".into());c.intent_ok=false;}
                else{c.post_editor=Some(editor);}}
            Err(error)=>{c.error=Some(error);c.intent_ok=false;}}
        }
        let error=c.error.clone();self.gui_hooks.jobs.operations.insert(id,c);if let Some(error)=error{self.gui_job_abort(cx,&error);}cx.redraw_all();
    }
    fn gui_job_compute_pending(&mut self,ticket:&JobTicket,envelope:&host::Submission,sequence:u64,request:&str)->Result<Option<Compute>,String>{
        let current=self.circular_native.current.as_ref().ok_or("JOB_NATIVE_CURRENT_MISSING")?;
        ensure(&current.ticket==ticket&&&current.submission==envelope&&current.host_sequence.0==sequence,"JOB_NATIVE_CURRENT_CHANGED")?;
        let delivered=current.delivered;let ctx=self.circular_context();let doc=self.document.as_ref().ok_or("JOB_NO_DOCUMENT")?;
        let controller=self.circular_host.as_mut().ok_or("JOB_HOST_MISSING")?;let status=controller.status(doc,&ctx,host::DecimalU64(sequence)).map_err(|_|"JOB_HOST_STATUS")?;
        ensure(status.sequence.0==sequence&&status.request_digest==request,"JOB_HOST_SEQUENCE_OR_REQUEST_CHANGED")?;
        match status.phase{native_jobs::JobPhase::Pending|native_jobs::JobPhase::Running=>return Ok(None),native_jobs::JobPhase::Completed=>{},_=>return Err("JOB_WORKER_FAILED_CANCELLED_OR_STALE".into())}
        if status.worker_owned||!controller.is_quiescent()||!delivered{return Ok(None);}
        ensure(status.result_available&&status.worker_arithmetic.as_ref().is_some_and(|a|a.accepted),"JOB_ACTUAL_WORKER_ARITHMETIC_OR_RESULT")?;
        let saved=controller.current_result(doc,&ctx,host::DecimalU64(sequence)).map_err(|_|"JOB_INDEPENDENT_CURRENT_RESULT")?;
        ensure(saved.request_digest==request&&!saved.content_digest.is_empty()&&saved.worker_arithmetic.accepted
            &&digest(&saved.worker_arithmetic)?==digest(status.worker_arithmetic.as_ref().ok_or("JOB_WORKER_ARITHMETIC")?)?,"JOB_PUBLISHED_RESULT_BINDING")?;
        ensure(saved.binding.mode==native_jobs::JobMode::Specified&&saved.binding.document_uuid==envelope.binding.document_uuid&&saved.binding.runtime_id==envelope.binding.runtime_id
            &&saved.binding.session_id==envelope.binding.session_id&&saved.binding.open_generation==envelope.binding.open_generation.0
            &&saved.binding.analysis_id.0==envelope.binding.case_id.0&&saved.binding.ground_id.0==envelope.binding.ground_id.0
            &&saved.binding.case_source_operation.0==envelope.binding.case_source_operation.0&&saved.binding.ground_source_operation.0==envelope.binding.ground_source_operation.0
            &&saved.binding.solver_source_fingerprint==envelope.binding.solver_source_fingerprint&&saved.binding.solver_version==envelope.binding.solver_version,"JOB_RESULT_SOURCE_IDS")?;
        match (&envelope.request,saved.request()){(host::FullRequest::Specified{input:a,config:ac},native_jobs::JobRequest::Specified{input:b,config:bc})=>ensure(a==b&&ac==bc,"JOB_RESULT_QUOTED_INPUT_CHANGED")?,_=>return Err("JOB_RESULT_NOT_SPECIFIED".into())}
        let summary=match saved.output(){native_jobs::JobOutput::Specified(result)=>{
            ensure(result.slices.len()<=512,"JOB_RAW_SLICE_COUNT_CAP")?;
            let raw=serde_json::to_string(&result.slices).map_err(|_|"JOB_RAW_SLICE_SERIALIZATION")?;ensure(raw.len()<=128<<10,"JOB_RAW_SLICE_BYTES_CAP")?;
            json!({"analysis_id":result.analysis_id,"source_revision":result.source_revision,"input_fingerprint":result.input_fingerprint,
                "solver_fingerprint":result.source_fingerprint,"solver_version":result.solver_version,"slice_count":result.slices.len(),
                "raw_slice_projection_json":raw,"raw_slice_projection_sha256":format!("{:x}",Sha256::digest(raw.as_bytes())),
                "raw_slice_projection_scope":"ALL_FIELDS_OF_ACTUAL_BORROWED_CIRCULAR_RESULT_SLICES",
                "numerically_verified":result.numerically_verified,"engineering_review_required":result.engineering_review_required,
                "accepted":result.accepted,"diagnostic_only":result.diagnostic_only,"currentness":"CURRENT_OWNED_HOST_RESULT",
                "actual_worker_arithmetic":&saved.worker_arithmetic})},_=>return Err("JOB_RESULT_SEARCH_NOT_ADMITTED".into())};
        Ok(Some(Compute{ticket:ticket.clone(),envelope:envelope.clone(),host_sequence:sequence,request_digest:request.into(),content_digest:saved.content_digest.clone(),summary}))
    }
    fn gui_job_compute(&mut self,expected:&Compute)->Result<(),String>{let observed=self.gui_job_compute_pending(&expected.ticket,&expected.envelope,expected.host_sequence,&expected.request_digest)?;
        ensure(observed.as_ref()==Some(expected),"JOB_OWNED_COMPUTE_NO_LONGER_CURRENT")}
    pub(crate) fn gui_job_archive_committed(&mut self,cx:&mut Cx,ticket:&ReportTicket,receipt:&ai_revit_core::CommitReceipt,archive_digest:&str)->Result<(),String>{
        if !self.gui_persistence_enabled(){return Ok(());}let id=self.gui_hooks.jobs.pending.clone().ok_or("JOB_ARCHIVE_NO_PERMIT")?;
        let mut c=self.gui_hooks.jobs.operations.get(&id).cloned().ok_or("JOB_COMMAND_UNKNOWN")?;
        let record=json!({"command_id":receipt.command_id,"transaction_id":receipt.transaction_id,"revision":receipt.revision,
            "revision_before":receipt.revision_before,"revision_after":receipt.revision_after,"operation_ids":receipt.operation_ids.iter().map(|id|id.get()).collect::<Vec<_>>(),
            "replayed":receipt.replayed,"case_id":ticket.identity.analysis_id.get(),"archive_digest":archive_digest,
            "commit_source":"ACTUAL_COMMON_TYPED_SAVE_CIRCULAR_SNAPSHOT_RECEIPT"});
        c.commit=Some(record.clone());self.gui_hooks.jobs.operations.insert(id.clone(),c.clone());
        ensure(c.before.role==Role::Calculation&&c.report_ticket.as_ref()==Some(ticket)&&c.intent_started&&!c.intent_returned&&c.commit_attempted
            &&c.before.report.as_ref().is_some_and(|r|r.archive_digest==archive_digest),"JOB_ARCHIVE_ORDINARY_PERMIT_OR_REPORT")?;
        self.gui_persistence_observe(cx)?;let after=version(&self.gui_persistence_model_observation()?);
        c.archive.commit(receipt.revision_before,receipt.revision_after,receipt.replayed,receipt.operation_ids.len(),after.clone()).map_err(str::to_owned)?;
        ensure(receipt.revision==after.revision,"JOB_ARCHIVE_RECEIPT_REVISION")?;
        let doc=self.document.as_ref().ok_or("JOB_NO_DOCUMENT")?;let post=shape(doc)?;
        ensure(post.elements==c.before.shape.elements&&post.grounds==c.before.shape.grounds&&post.cases==c.before.shape.cases,"JOB_ARCHIVE_CHANGED_SOURCE_IDS_OR_ELEMENTS")?;
        ensure(post.operations.len()==c.before.shape.operations.len()+1&&c.before.shape.operations.iter().all(|(id,hash)|post.operations.get(id)==Some(hash)),"JOB_ARCHIVE_HISTORY_PREFIX_CHANGED")?;
        let operation=receipt.operation_ids[0];ensure(!c.before.shape.operations.contains_key(&operation.get())&&doc.operation(operation).is_some_and(|op|matches!(&op.kind,
            ai_revit_core::OperationKind::EngineeringChanged{command:ai_revit_core::engineering::EngineeringCommand::SaveCircularSnapshot{analysis_id,snapshot},..}
                if *analysis_id==ticket.identity.analysis_id&&snapshot.archive().archive_digest==archive_digest)),"JOB_ARCHIVE_ACTUAL_OPERATION")?;
        let case=ticket.identity.analysis_id.get();ensure(post.archives.len()==c.before.shape.archives.len()
            &&c.before.shape.archives.iter().filter(|(id,_)|**id!=case).all(|(id,hash)|post.archives.get(id)==Some(hash)),"JOB_ARCHIVE_OTHER_CASE_CHANGED")?;
        let saved=doc.circular_case(ticket.identity.analysis_id).and_then(|c|c.circular_snapshot.as_ref()).ok_or("JOB_ARCHIVE_NOT_PERSISTED")?;
        ensure(saved.archive().archive_digest==archive_digest,"JOB_ARCHIVE_SAVED_DIGEST")?;
        let mut record=record;record["history_prefix_sha256"]=json!(digest(&c.before.shape.operations)?);record["history_after_sha256"]=json!(digest(&post.operations)?);
        record["preserved_operation_ids"]=json!(c.before.shape.operations.keys().copied().collect::<Vec<_>>());record["operation_ids_after"]=json!(post.operations.keys().copied().collect::<Vec<_>>());
        record["preserved_element_ids"]=json!(post.elements.keys().copied().collect::<Vec<_>>());record["preserved_ground_ids"]=json!(post.grounds.keys().copied().collect::<Vec<_>>());
        record["preserved_case_ids"]=json!(post.cases.keys().copied().collect::<Vec<_>>());record["actual_saved_archive_sha256"]=json!(digest(saved.archive())?);
        record["document_after"]=version_json(&after);record["currentness"]=json!("SAVED_ARCHIVE_AFTER_EXPLICIT_ORDINARY_COMMIT");c.commit=Some(record);
        self.gui_hooks.jobs.operations.insert(id,c);Ok(())
    }
}
fn context_json(c:&EditorContext)->Value{let identity=c.resolved_identity.as_ref().map(|i|json!({"document":{
    "uuid":i.document.document_uuid,"session":i.document.session_id,"open_generation":i.document.open_generation,"revision":i.document.revision},
    "ground_id":i.ground_id.get(),"case_id":i.analysis_id.get(),"ground_source_operation":i.ground_source_operation_id.get(),
    "case_source_operation":i.case_source_operation_id.get(),"config":i.config,"source_revision":i.source_revision,
    "input_fingerprint":i.input_fingerprint,"solver_fingerprint":i.source_fingerprint,"solver_version":i.solver_version}));
    json!({"document":{"uuid":c.document.document_uuid,"session":c.document.session_id,"open_generation":c.document.open_generation,"revision":c.document.revision},
        "load_generation":c.load_generation,"draft_generation":c.draft_generation,"ground_generation":c.ground_generation,
        "case_generation":c.case_generation,"search_generation":c.search_generation,"identity":identity,
        "ground_source":c.ground_source.map(|(id,op)|(id.get(),op.get())),"case_source":c.case_source.map(|(id,op)|(id.get(),op.get()))})}
impl Issued {fn id(&self)->String{format!("{}{:016x}",self.role.prefix(),self.base.serial)}
    fn json(&self)->Value{json!({"registry":self.role.registry(),"snapshot_id":self.id(),"geometry_generation":self.base.serial,
        "app_draw_serial":self.base.serial,"binding":self.base.binding,"window":self.base.window,"max_age_ms":5000,
        "target":{"target_id":self.role.name(),"widget_uid":self.control.uid.to_string(),"rect":rect_json(self.control.rect),
            "clipped_rect":rect_json(self.control.clip),"enabled":self.control.enabled,"visible":self.control.visible},
        "path":{"widget_uid":self.focus.uid.to_string(),"text":self.focus.text,"text_generation":self.focus.generation,"actual_key_focus":self.focus.actual,
            "current_area":true,"focus_sample_scope":"REAL_CX_AT_THIS_SNAPSHOT"},"editor":context_json(&self.editor),"next_ui_job_sequence":self.next_ui,
        "document":version_json(&self.version),"actual_ui_arithmetic_controls":self.fp,"worker_quiescent":true,
        "submit_attempts":null,"acceptance":"NOT_GRANTED"})}}
impl App {
    fn gui_job_widget(&self,cx:&mut Cx,role:Role)->WidgetRef{match role {Role::Analyze=>self.ui.widget(cx,ids!(gcc_analyze)),
        Role::Report=>self.ui.widget(cx,ids!(gcc_report)),Role::Calculation=>self.ui.widget(cx,ids!(gcc_snapshot_save))}}
    fn gui_job_clicked(&self,cx:&mut Cx,role:Role,actions:&Actions)->bool{match role{Role::Analyze=>self.ui.button(cx,ids!(gcc_analyze)).clicked(actions),
        Role::Report=>self.ui.button(cx,ids!(gcc_report)).clicked(actions),Role::Calculation=>self.ui.button(cx,ids!(gcc_snapshot_save)).clicked(actions)}}
    fn gui_job_control(&self,cx:&mut Cx,role:Role)->Result<Control,String>{let widget=self.gui_job_widget(cx,role);
        let uid=widget.try_widget_uid().ok_or("JOB_BUTTON_UID")?.0;let button=widget.borrow::<Button>().ok_or("JOB_ACTUAL_BUTTON_TYPE")?;
        let enabled=button.enabled();drop(button);let area=widget.area();ensure(uid!=0&&current_area_valid(&cx.draw_lists,area),"JOB_BUTTON_CURRENT_AREA")?;
        let rect=area.rect(cx);let clip=area.clipped_rect(cx);ensure(good_rect(rect)&&good_rect(clip),"JOB_BUTTON_RECT")?;
        Ok(Control {uid,area,rect,clip,enabled,visible:widget.visible()&&self.circular_ui.panel.active&&self.ui.widget(cx,ids!(circular_actions)).visible()})}
    fn gui_job_focus(&self,cx:&mut Cx)->Result<Focus,String>{let widget=self.ui.widget(cx,ids!(path_input));let uid=widget.try_widget_uid().ok_or("JOB_PATH_UID")?.0;
        ensure(widget.borrow::<TextInput>().is_some(),"JOB_PATH_ACTUAL_TYPE")?;let area=widget.area();ensure(uid!=0&&current_area_valid(&cx.draw_lists,area),"JOB_PATH_CURRENT_AREA")?;
        let model=self.gui_persistence_snapshot(cx)?;let path=&model["path"];
        Ok(Focus{uid,area,rect:area.rect(cx),clip:area.clipped_rect(cx),actual:cx.has_key_focus(area),text:path["text"].as_str().ok_or("JOB_PATH_TEXT")?.into(),
            generation:path["text_generation"].as_u64().ok_or("JOB_PATH_GENERATION")?})}
    pub(crate) fn gui_job_busy(&self)->bool{self.gui_hooks.jobs.pending.is_some()}
    pub(crate) fn gui_job_routed(&self,request:&agent_api::ApiCommand)->bool{jobwire::method(&request.method).is_some()
        ||request.method=="diagnostic.gui.status"&&request.params.get("command_id").and_then(Value::as_str).is_some_and(|id|self.gui_hooks.jobs.operations.contains_key(id))}
    pub(crate) fn gui_job_dispatch(&mut self,cx:&mut Cx,request:&agent_api::ApiCommand)->Result<Value,String>{
        if !self.gui_persistence_enabled(){return Ok(json!({"phase":"REFUSED","dispatch_certainty":"NOT_DISPATCHED","error":"JOB_PROFILE_NOT_ADMITTED"}));}
        let expected:life::Binding=serde_json::from_value(request.params.get("expected").cloned().ok_or("JOB_EXPECTED")?).map_err(|_|"JOB_EXPECTED")?;
        let binding=self.gui_lifecycle_observe_request(cx,request,&expected)?;self.gui_install(cx,&binding)?;
        let id=request.id.as_str().filter(|id|life::identifier(id)).ok_or("JOB_REQUEST_ID")?;
        ensure(self.gui_hooks.jobs.ids.len()<REQUESTS&&!self.gui_hooks.ids.contains(id)&&self.gui_hooks.jobs.ids.insert(id.into()),"JOB_REQUEST_REPLAY_OR_CAP")?;
        if request.method=="diagnostic.gui.status"{let p:wire::StatusParams=serde_json::from_value(request.params.clone()).map_err(|_|"JOB_STATUS_PARAMS")?;
            let command=self.gui_hooks.jobs.operations.get_mut(&p.command_id).ok_or("JOB_COMMAND_UNKNOWN")?;ensure(command.polls<POLLS,"JOB_STATUS_CAP")?;command.polls+=1;
            return Ok(self.gui_job_status(&p.command_id)?);}
        ensure(self.gui_hooks.fatal.is_none(),"JOB_ABORTED_USE_LIFECYCLE_CLOSE")?;
        let(role,input)=jobwire::method(&request.method).ok_or("JOB_METHOD_UNKNOWN")?;
        if !input{let _:jobwire::SnapshotParams=serde_json::from_value(request.params.clone()).map_err(|_|"JOB_SNAPSHOT_PARAMS")?;
            ensure(!self.gui_job_busy()&&self.gui_hooks.pending.is_none()&&self.circular_is_quiescent(),"JOB_BUSY")?;
            if let Some(held)=self.gui_hooks.jobs.issued.clone(){ensure(held.role==role,"JOB_HELD_OTHER_REGISTRY")?;self.gui_job_check(cx,&held,None)?;return Ok(held.json());}
            self.gui_persistence_observe(cx)?;
            let base=self.gui_latest_snapshot(cx,false)?;ensure(base.serial>self.gui_hooks.jobs.consumed,"JOB_WAIT_FRESH_DRAW")?;
            let control=self.gui_job_control(cx,role)?;ensure(control.enabled&&control.visible,"JOB_BUTTON_UNAVAILABLE_NOT_DISPATCHED")?;
            let editor=self.circular_ui.panel.editor_context()?;ensure(editor.resolved_identity.is_some()&&!self.circular_ui.panel.dirty&&!self.circular_ui.panel.stale,"JOB_EDITOR_NOT_CURRENT")?;
            let compute=if role!=Role::Analyze{let compute=self.gui_hooks.jobs.compute.clone().ok_or("JOB_OWNED_ANALYZE_REQUIRED")?;self.gui_job_compute(&compute)?;Some(compute)}else{None};
            let report=if role==Role::Calculation{let report=self.gui_hooks.jobs.report.clone().ok_or("JOB_OWNED_REPORT_REQUIRED")?;ensure(self.gui_job_report_readback()?.as_ref()==Some(&report),"JOB_REPORT_NOT_CURRENT")?;Some(report)}else{None};
            let held=Issued{base,role,control,focus:self.gui_job_focus(cx)?,editor,next_ui:self.circular_ui.panel.diagnostic_next_job_sequence().ok_or("JOB_UI_SEQUENCE_EXHAUSTED")?,
                version:version(&self.gui_persistence_model_observation()?),shape:shape(self.document.as_ref().ok_or("JOB_NO_DOCUMENT")?)?,fp:fp()?,compute,report};
            self.gui_hooks.jobs.issued=Some(held.clone());return Ok(held.json());
        }
        let p:jobwire::InputParams=serde_json::from_value(request.params.clone()).map_err(|_|"JOB_INPUT_PARAMS")?;
        ensure(!self.gui_job_busy()&&self.gui_hooks.pending.is_none()&&self.gui_hooks.jobs.operations.len()<COMMANDS
            &&!self.gui_hooks.operations.contains_key(&p.command_id)&&self.gui_hooks.jobs.attempts.insert(p.command_id.clone()),"JOB_COMMAND_REPLAY_OR_BUSY")?;
        let reserved=(||->Result<Command,String>{p.validate(role)?;let before=self.gui_hooks.jobs.issued.clone().ok_or("JOB_SNAPSHOT_MISSING")?;
            ensure(before.role==role&&before.id()==p.snapshot_id&&before.base.serial==p.geometry_generation,"JOB_SNAPSHOT_GENERATION")?;self.gui_job_check(cx,&before,None)?;
            ensure(before.control.enabled&&before.control.visible&&!cx.fingers.any_areas_captured(),"JOB_TARGET_OR_CAPTURE_NOT_READY")?;
            let jobwire::Click::At{u,v}=p.event;let point=before.control.clip.pos+before.control.clip.size*dvec2(u,v);
            ensure(contains(before.control.rect,point)&&contains(before.control.clip,point),"JOB_POINT_OUTSIDE_TARGET")?;
            let pair=state::Pair::new(point.x,point.y,cx.seconds_since_app_start()).map_err(str::to_owned)?;
            let archive=state::ArchiveTransition::new(before.version.clone());Ok(Command {before,pair,clicked:false,intent_started:false,intent_returned:false,intent_ok:false,
                job_ticket:None,report_ticket:None,envelope:None,submit_returned:false,accepted:None,report_key:None,report_sequence:None,report_start_returned:false,
                archive,commit_attempted:false,commit:None,post_editor:None,terminal:None,complete:false,error:None,polls:0,cleanup_returned:false,focus_after_down:None})})();
        let command=match reserved{Ok(c)=>c,Err(error)=>return Ok(json!({"phase":"REFUSED","dispatch_certainty":"NOT_DISPATCHED","command_id":p.command_id,"error":error}))};
        self.gui_hooks.jobs.consumed=command.before.base.serial;self.gui_hooks.jobs.issued=None;
        // Revoke any older idle v1/v2 lease without refreshing its lifetime.
        if let Some(held)=self.gui_hooks.issued.take(){self.gui_hooks.consumed_serial=Some(self.gui_hooks.consumed_serial.unwrap_or(0).max(held.serial));}
        self.gui_hooks.jobs.pending=Some(p.command_id.clone());self.gui_hooks.jobs.operations.insert(p.command_id.clone(),command);
        if let Err(error)=self.gui_job_queue(&p.command_id){self.gui_job_abort(cx,&error);}
        Ok(json!({"command_id":p.command_id,"phase":"QUEUED_OR_OUTCOME_UNKNOWN","retry_allowed":false,"acceptance":"NOT_GRANTED"}))
    }
}
