//! App-local Source13 v2 bridge. SOURCE ONLY; no Toolkit/backend modification.
//! Child of linux_geotech_gui so existing private lease/capture guards remain owned there.
use super::*;
use crate::{linux_geotech_persistence_contract as policy,linux_geotech_input_state as state,
    linux_geotech_persistence_io as io,linux_geotech_persistence_registration as registration};
use makepad_widgets::makepad_draw::makepad_platform::studio::{KeyEvent,TextInputEvent};

static V2:OnceLock<bool>=OnceLock::new();
static REGISTRATION:OnceLock<registration::Registration>=OnceLock::new();
pub(super) fn initialize(v2:bool)->Result<(),String> {
    V2.set(v2).map_err(|_|"PERSISTENCE_PROFILE_ALREADY_INITIALIZED")?;
    if v2 {REGISTRATION.set(registration::read()?).map_err(|_|"PERSISTENCE_REGISTRATION_ALREADY_INITIALIZED")?;}
    Ok(())
}
pub(super) fn enabled()->bool {V2.get()==Some(&true)}
#[derive(Default)]
pub(super) struct Hooks {
    io:Option<io::RootLease>,registration:Option<registration::Registration>,owned:Option<registration::RegisteredFile>,
    path:Option<state::PathState>,model:Option<policy::ModelObservation>,model_key:Option<(String,u64,u64)>,
    operations:BTreeMap<String,Input>,attempts:BTreeSet<String>,texts:usize,permit:Option<(String,Target)>,
    cleanup:Option<Cleanup>,analyze_dispatches:u64,update_sequence:u64,
}
#[derive(Clone)]
struct Input {
    before:Snapshot,target:Target,uid:u64,point:DVec2,down_time:f64,up_time:f64,
    focus:Option<state::PathFocusTracker>,transition:policy::TransitionTracker,text:Option<Text>,
    down_queued:bool,up_queued:bool,down:bool,up:bool,down_returned:bool,up_returned:bool,
    clicked:bool,handler_started:bool,handler_returned:bool,handler_ok:bool,file:Option<Value>,complete:bool,
    cleanup_returned:bool,
}
#[derive(Clone)]
struct Text {machine:state::TextReplace,recipe:state::ExactRecipe,desired:String}
#[derive(Clone)]
enum CleanupKind {Key(KeyEvent),Mouse{point:DVec2,time:f64}}
#[derive(Clone)]
struct Cleanup {binding:life::Binding,command:String,kind:CleanupKind}
fn hash(bytes:&[u8])->String {format!("{:x}",Sha256::digest(bytes))}
fn fingerprint(value:&Value)->Result<[u8;32],String> {Ok(Sha256::digest(serde_json::to_vec(value).map_err(|_|"GUI_V2_FINGERPRINT")?).into())}
fn hex(value:&[u8;32])->String {value.iter().map(|v|format!("{v:02x}")).collect()}
fn area_slot(area:Area)->String {match area {
    Area::Instance(v)=>format!("I:{:?}:{}:{}:{}",v.draw_list_id,v.draw_item_id,v.instance_offset,v.instance_count),
    Area::Rect(v)=>format!("R:{:?}:{}",v.draw_list_id,v.rect_id),Area::Empty=>"EMPTY".into(),
}}
fn model_value(m:&policy::ModelObservation)->Value {json!({"uuid":m.document.uuid,"runtime":m.document.runtime,
    "open_generation":m.document.open_generation,"revision":m.document.revision,"semantic_sha256":hex(&m.document.semantic_digest),
    "archive_sha256":m.document.archive_digest.as_ref().map(hex),"report_sha256":m.document.report_digest.as_ref().map(hex),
    "analyze_dispatches":m.analyze_dispatches,"worker_quiescent":m.worker_quiescent,
    "worker_quiescence_scope":"calculation_host; whole-App quiet required at document completion"})}
fn persisted(m:&policy::ModelObservation)->registration::Persisted {registration::Persisted {uuid:m.document.uuid.clone(),
    revision:m.document.revision,semantic_sha256:hex(&m.document.semantic_digest),archive_sha256:m.document.archive_digest.as_ref().map(hex),
    report_sha256:m.document.report_digest.as_ref().map(hex)}}
fn key(time:f64)->KeyEvent {KeyEvent {key_code:KeyCode::KeyA,is_repeat:false,
    modifiers:RemoteKeyModifiers {control:true,shift:false,alt:false,logo:false}.into_key_modifiers(),time}}
fn target_needs_handler(t:Target)->bool {matches!(t,Target::SaveDocument|Target::OpenDocument|Target::NewDocument)}
pub(super) fn record_actual_submit_attempt(hooks:&mut Hooks)->Result<(),String>{
    hooks.analyze_dispatches=hooks.analyze_dispatches.checked_add(1).ok_or("GUI_ANALYZE_ATTEMPT_COUNT_EXHAUSTED")?;Ok(())
}
#[cfg(test)]pub(super) fn actual_submit_attempts(hooks:&Hooks)->u64{hooks.analyze_dispatches}

impl App {
    pub(crate) fn gui_persistence_enabled(&self)->bool {enabled()}
    pub(crate) fn gui_persistence_model_observation(&self)->Result<policy::ModelObservation,String>{self.gui_v2_model()}
    pub(crate) fn gui_persistence_button_clicked(&self,cx:&mut Cx,target:Target,actions:&Actions)->bool{self.gui_v2_button_clicked(cx,target,actions)}
    pub(crate) fn gui_persistence_archive_restore_window(&self)->bool {
        if !enabled(){return false;}
        self.gui_hooks.pending.as_ref().and_then(|id|self.gui_hooks.persistence.operations.get(id)).is_some_and(|input|
            crate::linux_geotech_persistence_adapter_guard::archive_inspection_window(input.target.policy(),input.handler_started,
                input.file.as_ref().is_some_and(|file|file["input_hash_verified_before_parse"]==true),
                self.circular_host.as_ref().is_none_or(|h|h.is_quiescent()),input.complete))
    }
    pub(crate) fn gui_persistence_install(&mut self,binding:&life::Binding)->Result<(),String> {
        if !enabled() || self.gui_hooks.persistence.io.is_some() {return Ok(());}
        let registration=REGISTRATION.get().ok_or("PERSISTENCE_REGISTRATION_NOT_INITIALIZED")?.clone();registration.bind(binding)?;
        let io=io::RootLease::acquire(registration.paths()?,registration.root.native()).map_err(|_|"PERSISTENCE_ROOT_NOT_READY")?;
        self.gui_hooks.persistence.owned=registration.owned_output.clone();
        self.gui_hooks.persistence.update_sequence=registration.update_sequence;
        self.gui_hooks.persistence.registration=Some(registration);self.gui_hooks.persistence.io=Some(io);Ok(())
    }
    pub(crate) fn gui_persistence_observe(&mut self,cx:&mut Cx)->Result<(),String> {
        if !enabled() {return Ok(());}
        let text=self.path_text(cx);life::ensure(text.len()<=policy::PATH_BYTES&&!text.chars().any(char::is_control),"GUI_PATH_READBACK_CAP")?;
        let old=self.gui_hooks.persistence.path.as_ref();let generation=match old {
            None=>1,Some(old)=>policy::next_text_generation(old.generation,&old.text,&text).map_err(str::to_owned)?,
        };
        self.gui_hooks.persistence.path=Some(state::PathState {text,generation});
        let Some(doc)=self.document.as_ref() else {return Ok(());};
        let cache_key=(doc.runtime_instance_id().to_string(),doc.revision(),self.api_document_generation);
        if self.gui_hooks.persistence.model_key.as_ref()!=Some(&cache_key) {
            let bytes=doc.to_json_string().map_err(|_|"GUI_DOCUMENT_SERIALIZATION")?;
            life::ensure(bytes.len() as u64<=io::FILE_CAP,"GUI_DOCUMENT_DIGEST_CAP")?;
            let archive_rows:Vec<_>=doc.circular_cases().iter().filter_map(|(id,c)|c.circular_snapshot.as_ref().map(|s|(id,s))).collect();
            life::ensure(archive_rows.len()<=64,"GUI_ARCHIVE_DIGEST_COUNT_CAP")?;
            let archive_digest=if archive_rows.is_empty(){None}else{Some(Sha256::digest(serde_json::to_vec(&archive_rows).map_err(|_|"GUI_ARCHIVE_DIGEST")?).into())};
            let report_rows:Vec<_>=archive_rows.iter().map(|(id,s)|(*id,s.archive())).collect();
            let report_digest=if report_rows.is_empty(){None}else{Some(Sha256::digest(serde_json::to_vec(&report_rows).map_err(|_|"GUI_REPORT_DIGEST")?).into())};
            let mut demo=policy::DemoShape {walls:0,floors:0,roofs:0,elements:doc.elements().len() as u64};
            for e in doc.elements() {match &e.kind {ElementKind::Wall(_)=>demo.walls+=1,ElementKind::Floor(_)=>demo.floors+=1,
                ElementKind::Roof(_)=>demo.roofs+=1,_=>{}}}
            let model=policy::ModelObservation {document:policy::DocumentKey {uuid:doc.document_uuid.to_string(),runtime:cache_key.0.clone(),
                open_generation:self.api_document_generation,revision:doc.revision(),semantic_digest:Sha256::digest(bytes.as_bytes()).into(),
                archive_digest,report_digest},analyze_dispatches:self.gui_hooks.persistence.analyze_dispatches,
                worker_quiescent:self.circular_host.as_ref().is_none_or(|h|h.is_quiescent()),demo:Some(demo)};
            self.gui_hooks.persistence.model=Some(model);self.gui_hooks.persistence.model_key=Some(cache_key);
        }
        let quiet=self.circular_host.as_ref().is_none_or(|h|h.is_quiescent());let count=self.gui_hooks.persistence.analyze_dispatches;
        if let Some(model)=self.gui_hooks.persistence.model.as_mut(){model.worker_quiescent=quiet;model.analyze_dispatches=count;}Ok(())
    }
    pub(crate) fn gui_persistence_snapshot(&self,cx:&mut Cx)->Result<Value,String> {
        if !enabled(){return Ok(Value::Null);}
        let input=self.ui.text_input(cx,ids!(path_input));let widget=self.ui.widget(cx,ids!(path_input));let area=widget.area();
        life::ensure(widget.borrow::<TextInput>().is_some(),"GUI_PATH_ACTUAL_WIDGET_TYPE_CHANGED")?;
        let valid=current_area_valid(&cx.draw_lists,area);let path=self.gui_hooks.persistence.path.as_ref().ok_or("GUI_PATH_READBACK_NOT_READY")?;
        life::ensure(input.text()==path.text,"GUI_PATH_OBSERVATION_STALE")?;
        let model=self.gui_hooks.persistence.model.as_ref().ok_or("GUI_DOCUMENT_READBACK_NOT_READY")?;
        Ok(json!({"input_schema_version":2,"path":{"text":path.text,"text_bytes":path.text.len(),"text_generation":path.generation,
            "widget_uid":widget.try_widget_uid().map(|v|v.0.to_string()),"current_area":valid,"area_slot":area_slot(area),
            "is_read_only":input.is_read_only(),"makepad_key_focus":valid&&!area.is_empty()&&cx.has_key_focus(area)},
            "document":model_value(model),"first_save_foundation_only":false,"native_view":self.gui_job_native_view_currentness(),
            "owned_update":{"scope":"OWNED_EXPECTED_VERSION_ONLY_SOURCE_UNEXECUTED",
                "allowed":self.gui_hooks.persistence.registration.as_ref().is_some_and(|r|r.allow_owned_update),
                "sequence":self.gui_hooks.persistence.update_sequence,"previous_versions_retained":true}}))
    }
    fn gui_v2_path(&self)->Result<state::PathState,String>{self.gui_hooks.persistence.path.clone().ok_or("GUI_PATH_READBACK_NOT_READY".into())}
    fn gui_v2_model(&self)->Result<policy::ModelObservation,String>{self.gui_hooks.persistence.model.clone().ok_or("GUI_DOCUMENT_READBACK_NOT_READY".into())}
    fn gui_v2_guard(&self,cx:&mut Cx,before:&Snapshot)->Result<state::Guard,String> {
        let binding=self.gui_lifecycle_frozen(cx)?;life::ensure(binding==before.binding&&before.created.elapsed()<=MAX_SNAPSHOT_AGE,"GUI_V2_BINDING_OR_TTL_CHANGED")?;
        let target=self.gui_measured_target(cx,Target::PathInput);let original=before.targets.iter().find(|t|t.target==Target::PathInput).ok_or("GUI_PATH_TARGET_MISSING")?;
        life::ensure(target.available&&target.uid==original.uid&&rect_json(target.rect)==rect_json(original.rect)&&rect_json(target.clip)==rect_json(original.clip)
            &&area_lease_matches(original.area,target.area,true,current_area_valid(&cx.draw_lists,target.area),cx.redraw_id>before.redraw),"GUI_PATH_AREA_CHANGED")?;
        let geom=&cx.windows[WindowId(0,0)].window_geom;
        let layout=json!({"width":geom.inner_size.x,"height":geom.inner_size.y,"dpi":geom.dpi_factor,"position":[geom.position.x,geom.position.y]});
        life::ensure(before.window["width"]==layout["width"]&&before.window["height"]==layout["height"]
            &&before.window["dpi_factor"]==layout["dpi"]&&before.window["position"]==layout["position"],"GUI_V2_LAYOUT_CHANGED")?;
        let model=self.gui_v2_model()?;
        let source=json!({"document":model_value(&model)});
        Ok(state::Guard {context:state::ImmutableContext {binding:fingerprint(&serde_json::to_value(&binding).map_err(|_|"GUI_BINDING_ENCODE")?)?,
            layout:fingerprint(&layout)?,source:fingerprint(&source)?,target_uid:target.uid.ok_or("GUI_PATH_UID_MISSING")?,
            area_slot:Sha256::digest(area_slot(target.area).as_bytes()).into(),issued_geometry_generation:before.serial},
            area_current_and_lease_matches:true,actual_key_focus:cx.has_key_focus(target.area),writable:!self.ui.text_input(cx,ids!(path_input)).is_read_only(),
            visible:target.visible,elapsed_ms:before.created.elapsed().as_millis().min(u64::MAX as u128) as u64,original_ttl_ms:5000})
    }
    fn gui_v2_context(&mut self,cx:&mut Cx,input:&mut Input)->Result<(),String> {
        self.gui_persistence_observe(cx)?;let current=self.gui_snapshot_now(cx,self.gui_hooks.draw_serial)?;
        life::ensure(input.before.binding==current.binding&&input.before.window==current.window&&input.before.created.elapsed()<=MAX_SNAPSHOT_AGE,"GUI_V2_IMMUTABLE_CONTEXT_CHANGED")?;
        for original in &input.before.targets {
            // Only the target-declared workspace/document transition may hide
            // circular controls. Stable file-bar controls always remain exact.
            let circular=original.target.legacy()&&original.target!=Target::WorkspaceCircular;
            if circular&&matches!(input.target,Target::WorkspaceCircular|Target::NewDocument|Target::OpenDocument){continue;}
            let now=current.targets.iter().find(|t|t.target==original.target).ok_or("GUI_V2_TARGET_REGISTRY_CHANGED")?;
            life::ensure(original.uid==now.uid&&original.available==now.available&&original.enabled==now.enabled&&original.visible==now.visible
                &&rect_json(original.rect)==rect_json(now.rect)&&rect_json(original.clip)==rect_json(now.clip)
                &&area_lease_matches(original.area,now.area,now.available,current_area_valid(&cx.draw_lists,now.area),current.redraw>input.before.redraw),"GUI_V2_TARGET_LAYOUT_OR_AREA_CHANGED")?;
        }
        life::ensure(input.transition.observe_between_members(&self.gui_v2_model()?),"GUI_V2_DOCUMENT_TRANSITION_REFUSED")?;
        let path=self.gui_v2_path()?;
        if input.text.is_none(){life::ensure(input.before.model.pointer("/persistence/path/text")==Some(&json!(path.text))
            &&input.before.model.pointer("/persistence/path/text_generation")==Some(&json!(path.generation)),"GUI_V2_PATH_CHANGED_DURING_CLICK")?;}
        if let Some(focus)=input.focus.as_mut(){life::ensure(focus.observe_member(&self.gui_v2_guard(cx,&input.before)?,&path),"GUI_PATH_FOCUS_TRANSITION_REFUSED")?;}Ok(())
    }
    fn gui_v2_button_clicked(&self,cx:&mut Cx,target:Target,actions:&Actions)->bool {match target {
        Target::WorkspaceCircular=>self.ui.button(cx,ids!(workspace_circular)).clicked(actions),Target::Gcc2d=>self.ui.button(cx,ids!(gcc_2d)).clicked(actions),
        Target::Gcc3d=>self.ui.button(cx,ids!(gcc_3d)).clicked(actions),Target::GccFit=>self.ui.button(cx,ids!(gcc_fit)).clicked(actions),
        Target::SaveDocument=>self.ui.button(cx,ids!(save_document)).clicked(actions),Target::OpenDocument=>self.ui.button(cx,ids!(open_document)).clicked(actions),
        Target::NewDocument=>self.ui.button(cx,ids!(new_document)).clicked(actions),Target::PathInput=>false}}
    pub(crate) fn gui_persistence_begin(&mut self,cx:&mut Cx,target:Target)->Result<(),String> {
        if !enabled(){return Ok(());}let id=self.gui_hooks.pending.clone().ok_or("GUI_V2_ORDINARY_HANDLER_NOT_ADMITTED")?;
        let mut input=self.gui_hooks.persistence.operations.get(&id).cloned().ok_or("GUI_V2_ORDINARY_HANDLER_NOT_ADMITTED")?;
        self.gui_v2_context(cx,&mut input)?;
        life::ensure(crate::linux_geotech_persistence_adapter_guard::handler_permitted(input.target.policy(),target.policy(),input.uid,
            self.gui_target_widget(cx,target).try_widget_uid().map(|uid|uid.0),input.down,input.clicked,input.handler_started,
            self.gui_hooks.persistence.permit.is_none()),"GUI_V2_HANDLER_REPLAY_OR_ORDER")?;
        if matches!(target,Target::OpenDocument|Target::NewDocument){life::ensure(self.api_document_generation.checked_add(1).is_some(),"GUI_DOCUMENT_GENERATION_EXHAUSTED")?;}
        input.handler_started=true;self.gui_hooks.persistence.operations.insert(id.clone(),input);self.gui_hooks.persistence.permit=Some((id,target));Ok(())
    }
    pub(crate) fn gui_persistence_end(&mut self,target:Target,ok:bool) {
        if !enabled(){return;}if let Some((id,admitted))=self.gui_hooks.persistence.permit.take(){
            if let Some(input)=self.gui_hooks.persistence.operations.get_mut(&id){input.handler_returned=admitted==target;input.handler_ok=ok&&admitted==target;}
        }
    }
    fn gui_v2_io_permit(&self,target:Target)->Result<String,String> {
        let (id,admitted)=self.gui_hooks.persistence.permit.as_ref().ok_or("PERSISTENCE_ORDINARY_BUTTON_PERMIT_REQUIRED")?;
        life::ensure(*admitted==target&&self.gui_hooks.pending.as_ref()==Some(id),"PERSISTENCE_ORDINARY_BUTTON_PERMIT_CHANGED")?;Ok(id.clone())
    }
    pub(crate) fn gui_persistence_read(&mut self,path:&str)->Result<Vec<u8>,String> {
        let id=self.gui_v2_io_permit(Target::OpenDocument)?;
        let reg=self.gui_hooks.persistence.registration.as_ref().ok_or("PERSISTENCE_NOT_INSTALLED")?;let paths=reg.paths()?;
        let expected=if path==paths.input(){reg.input.as_ref()}else if path==paths.output(){self.gui_hooks.persistence.owned.as_ref()}else{None}
            .ok_or("PERSISTENCE_READ_PATH_NOT_REGISTERED")?.clone();
        let read=self.gui_hooks.persistence.io.as_ref().ok_or("PERSISTENCE_NOT_INSTALLED")?.read_registered(path,expected.stamp.native())
            .map_err(|_|"PERSISTENCE_READ_IDENTITY_OR_IO_REFUSED")?;
        life::ensure(hash(&read.bytes)==expected.sha256,"PERSISTENCE_INPUT_HASH_CHANGED")?;
        if let Some(input)=self.gui_hooks.persistence.operations.get_mut(&id){input.file=Some(json!({"operation":"read_registered","path":path,
            "bytes":read.bytes.len(),"sha256":expected.sha256,"registered_document":expected.document,"input_hash_verified_before_parse":true}));}Ok(read.bytes)
    }
    pub(crate) fn gui_persistence_write(&mut self,path:&str,bytes:&[u8])->Result<(),String> {
        let id=self.gui_v2_io_permit(Target::SaveDocument)?;
        let reg=self.gui_hooks.persistence.registration.as_ref().ok_or("PERSISTENCE_NOT_INSTALLED")?.clone();
        life::ensure(reg.paths()?.save_path(path),"PERSISTENCE_WRITE_PATH_NOT_REGISTERED")?;
        let model=self.gui_v2_model()?;
        life::ensure(hash(bytes)==hex(&model.document.semantic_digest),"PERSISTENCE_SERIALIZATION_READBACK_CHANGED")?;
        let (stamp,file)=if let Some(previous)=self.gui_hooks.persistence.owned.clone(){
            life::ensure(reg.allow_owned_update,"PERSISTENCE_OWNED_UPDATE_NOT_ADMITTED")?;
            let sequence=self.gui_hooks.persistence.update_sequence.checked_add(1).ok_or("PERSISTENCE_UPDATE_SEQUENCE_EXHAUSTED")?;
            life::ensure(sequence<=io::owned_update::UPDATE_LIMIT,"PERSISTENCE_UPDATE_SEQUENCE_EXHAUSTED")?;
            let io=self.gui_hooks.persistence.io.as_mut().ok_or("PERSISTENCE_NOT_INSTALLED")?;
            let old=io.read_registered(path,previous.stamp.native()).map_err(|_|"PERSISTENCE_OWNED_EXPECTED_READ_REFUSED")?;
            life::ensure(hash(&old.bytes)==previous.sha256,"PERSISTENCE_OWNED_EXPECTED_HASH_CHANGED")?;
            // Reserve once before any Update I/O. Failed/ambiguous operations do
            // not decrement this counter or reuse the previous basename.
            self.gui_hooks.persistence.update_sequence=sequence;
            let receipt=match io.write_owned_expected(path,previous.stamp.native(),&old.bytes,bytes,sequence){
                Ok(receipt)=>receipt,
                Err(error)=>{
                    if let Some(input)=self.gui_hooks.persistence.operations.get_mut(&id){input.file=Some(json!({
                        "operation":"owned_expected_update","sequence":sequence,"error":error.code,
                        "output_may_have_changed":error.output_may_have_changed,
                        "previous_name_may_exist":error.previous_name_may_exist,"retry_allowed":false}));}
                    return Err(format!("PERSISTENCE_{}_REFUSED_OR_OUTCOME_UNKNOWN",error.code));
                }
            };
            (receipt.file,json!({"operation":"owned_expected_update","path":path,"bytes":bytes.len(),
                "sequence":receipt.sequence,"expected_previous_sha256":previous.sha256,"previous_document":previous.document,
                "previous_name":receipt.previous_name,"previous_stamp":registration::Stamp::from_native(receipt.previous),
                "expected_bytes_verified_at_io":receipt.expected_bytes_verified,"atomic_exchange_verified":receipt.atomic_exchange_verified,
                "root_dev":receipt.root.dev,"root_ino":receipt.root.ino,"no_unlink_or_truncation":true}))
        }else{
            life::ensure(reg.allow_first_save,"PERSISTENCE_FIRST_SAVE_NOT_ADMITTED")?;
            let receipt=self.gui_hooks.persistence.io.as_ref().ok_or("PERSISTENCE_NOT_INSTALLED")?.write_new(path,bytes)
                .map_err(|_|"PERSISTENCE_FIRST_SAVE_REFUSED_OR_OUTCOME_UNKNOWN")?;
            (receipt.file,json!({"operation":"create_new","path":path,"bytes":bytes.len(),"no_replace":receipt.no_replace,
                "root_dev":receipt.root.dev,"root_ino":receipt.root.ino}))
        };
        let owned=registration::RegisteredFile {stamp:registration::Stamp::from_native(stamp),sha256:hash(bytes),document:persisted(&model)};
        if let Some(input)=self.gui_hooks.persistence.operations.get_mut(&id){let mut file=file;
            file["sha256"]=json!(owned.sha256);file["file_stamp"]=json!(owned.stamp);file["document"]=json!(owned.document);input.file=Some(file);}
        self.gui_hooks.persistence.owned=Some(owned);Ok(())
    }
    pub(crate) fn gui_persistence_input(&mut self,cx:&mut Cx,p:wire::InputParams)->Result<Value,String> {
        if !enabled() {return Ok(json!({"phase":"REFUSED","dispatch_certainty":"NOT_DISPATCHED",
            "error":"GUI_INPUT_ADAPTER_UNSUPPORTED","command_id":p.command_id}));}
        self.gui_command_available(&p.command_id)?;
        life::ensure(self.gui_hooks.persistence.attempts.len()<wire::COMMAND_COUNT
            &&self.gui_hooks.persistence.attempts.insert(p.command_id.clone()),"GUI_INPUT_COMMAND_REPLAY_OR_CAP")?;
        let attempt=(||->Result<Input,String>{
            p.validate()?;self.gui_persistence_observe(cx)?;
            life::ensure(!cx.fingers.any_areas_captured(),"GUI_INPUT_CAPTURE_BUSY")?;
            let before=self.gui_live_snapshot(cx,&p.snapshot_id,false)?;
            let target=before.targets.iter().find(|t|t.target==p.target_id&&t.available).ok_or("GUI_INPUT_TARGET_UNAVAILABLE")?.clone();
            let reg=self.gui_hooks.persistence.registration.as_ref().ok_or("PERSISTENCE_NOT_INSTALLED")?;
            let path=self.gui_v2_path()?;let model=self.gui_v2_model()?;
            let rule=if p.target_id==Target::NewDocument{policy::Transition::NewDemo}else if p.target_id==Target::OpenDocument{
                let paths=reg.paths()?;let expected=if path.text==paths.input(){reg.input.as_ref()}else if path.text==paths.output(){self.gui_hooks.persistence.owned.as_ref()}else{None};
                policy::Transition::Open(expected.ok_or("PERSISTENCE_OPEN_PATH_NOT_REGISTERED")?.document.native()?)
            }else{policy::Transition::Preserve};
            let time=cx.seconds_since_app_start();life::ensure(time.is_finite(),"GUI_CLOCK_INVALID")?;
            let mut input=Input {before:before.clone(),target:p.target_id,uid:target.uid.ok_or("GUI_TARGET_UID_MISSING")?,point:target.clip.pos+target.clip.size*0.5,
                down_time:time,up_time:time+0.001,focus:None,transition:policy::TransitionTracker::new(model,rule),text:None,
                down_queued:false,up_queued:false,down:false,up:false,down_returned:false,up_returned:false,clicked:false,handler_started:false,
                handler_returned:false,handler_ok:false,file:None,complete:false,cleanup_returned:false};
            match &p.event {
                wire::InputEvent::Click {u,v}=>{
                    life::ensure(self.gui_hooks.clicks<wire::CLICK_COUNT,"GUI_CLICK_CAP")?;
                    input.point=target.clip.pos+target.clip.size*dvec2(*u,*v);
                    let client=Rect {pos:dvec2(0.0,0.0),size:dvec2(before.window["width"].as_f64().ok_or("GUI_WIDTH")?,before.window["height"].as_f64().ok_or("GUI_HEIGHT")?)};
                    life::ensure(contains(target.rect,input.point)&&contains(target.clip,input.point)&&contains(client,input.point),"GUI_INPUT_POINT_OUTSIDE_CLIENT")?;
                    if p.target_id==Target::PathInput {input.focus=Some(state::PathFocusTracker::reserve(&self.gui_v2_guard(cx,&before)?,path,before.serial).map_err(|_|"GUI_PATH_FOCUS_NOT_READY")?);}
                }
                wire::InputEvent::ReplaceText {text,expected_text_generation}=>{
                    life::ensure(self.gui_hooks.persistence.texts<wire::TEXT_COUNT&&reg.paths()?.registered(text),"GUI_TEXT_CAP_OR_PATH_UNREGISTERED")?;
                    let machine=state::TextReplace::reserve(&self.gui_v2_guard(cx,&before)?,path,text.clone(),*expected_text_generation,before.serial)
                        .map_err(|_|"GUI_REAL_KEY_FOCUS_OR_TEXT_GENERATION_REQUIRED")?;
                    let recipe=state::ExactRecipe::new(time,time+0.001,text.clone()).map_err(|_|"GUI_TEXT_RECIPE")?;
                    input.text=Some(Text {machine,recipe,desired:text.clone()});
                }
            }Ok(input)
        })();
        let input=match attempt {Ok(v)=>v,Err(code)=>return Ok(json!({"command_id":p.command_id,"phase":"REFUSED","dispatch_certainty":"NOT_DISPATCHED","error":code}))};
        consume_issued(&mut self.gui_hooks.issued,&mut self.gui_hooks.consumed_serial,&p.snapshot_id)?;
        self.gui_hooks.pending=Some(p.command_id.clone());
        self.gui_hooks.operations.insert(p.command_id.clone(),Operation {kind:Kind::V2,before:input.before.clone(),phase:"QUEUED",down:false,up:false,
            down_returned:false,up_returned:false,clicked:false,post:None,image:None,error:None,polls:0});
        let is_text=input.text.is_some();self.gui_hooks.persistence.operations.insert(p.command_id.clone(),input);
        if is_text{self.gui_hooks.persistence.texts+=1;}else{self.gui_hooks.clicks+=1;}
        if let Err(code)=self.gui_v2_queue_next(&p.command_id){self.gui_v2_abort(cx,&code);return Ok(json!({"command_id":p.command_id,"phase":"ABORTED_OUTCOME_UNKNOWN","dispatch_certainty":"OUTCOME_UNKNOWN","error":code}));}
        Ok(json!({"command_id":p.command_id,"phase":"QUEUED","dispatch_certainty":"OUTCOME_UNKNOWN","ordinary_input_proved":false}))
    }
    fn gui_v2_queue_next(&mut self,id:&str)->Result<(),String> {
        let mut input=self.gui_hooks.persistence.operations.get(id).cloned().ok_or("GUI_INPUT_COMMAND_UNKNOWN")?;
        let sender=self.gui_hooks.sender.as_ref().ok_or("GUI_CHANNEL_MISSING")?;
        if let Some(text)=input.text.as_mut(){
            let member=text.machine.next_member().ok_or("GUI_TEXT_NEXT_MEMBER_UNAVAILABLE")?;
            let event=match member {state::Member::SelectAllDown=>StudioToApp::KeyDown(key(input.down_time)),
                state::Member::SelectAllUp=>StudioToApp::KeyUp(key(input.up_time)),
                state::Member::TextInput=>StudioToApp::TextInput(TextInputEvent {input:text.desired.clone(),..Default::default()})};
            sender.try_send(event).map_err(|_|"GUI_TEXT_QUEUE_FAILED")?;
            let sent=text.machine.sent(member);self.gui_hooks.persistence.operations.insert(id.into(),input);
            sent.map_err(|_|"GUI_TEXT_POST_ENQUEUE_STATE_REFUSED")?;
        }else{
            sender.try_send(StudioToApp::MouseDown(RemoteMouseDown {button_raw_bits:1,x:input.point.x,y:input.point.y,time:input.down_time,modifiers:RemoteKeyModifiers::default()}))
                .map_err(|_|"GUI_DOWN_QUEUE_FAILED")?;input.down_queued=true;
            self.gui_hooks.persistence.operations.insert(id.into(),input.clone());
            sender.try_send(StudioToApp::MouseUp(RemoteMouseUp {button_raw_bits:1,x:input.point.x,y:input.point.y,time:input.up_time,modifiers:RemoteKeyModifiers::default()}))
                .map_err(|_|"GUI_UP_QUEUE_FAILED")?;input.up_queued=true;self.gui_hooks.persistence.operations.insert(id.into(),input);
        }SignalToUI::set_ui_signal();Ok(())
    }
    pub(crate) fn gui_persistence_before_event(&mut self,cx:&mut Cx,event:&Event)->Option<bool> {
        if !enabled()||self.gui_hooks.sender.is_none(){return None;}
        if let Some(cleanup)=self.gui_hooks.persistence.cleanup.clone(){
            let exact=self.gui_lifecycle_frozen(cx).ok().as_ref()==Some(&cleanup.binding);
            let matching=match (&cleanup.kind,event){
                (CleanupKind::Key(expected),Event::KeyUp(actual))=>actual==expected,
                (CleanupKind::Mouse{point,time},Event::MouseUp(actual))=>actual.window_id==WindowId(0,0)&&actual.button.bits()==1
                    &&same_point(actual.abs,*point)&&actual.time.to_bits()==time.to_bits()&&!actual.modifiers.control&&!actual.modifiers.shift
                    &&!actual.modifiers.alt&&!actual.modifiers.logo,_=>false};
            if matching{return Some(exact);}
        }
        if self.gui_hooks.fatal.is_some(){return Some(!matches!(event,Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)|Event::MouseDown(_)|Event::MouseUp(_)|Event::Actions(_)));}
        if matches!(event,Event::LiveEdit|Event::ScriptReapply|Event::TextRangeReplace(_)|Event::TextCopy(_)|Event::TextCut(_)|Event::ImeAction(_)
            |Event::SelectionHandleDrag(_)|Event::TouchUpdate(_)|Event::LongPress(_)|Event::Drag(_)|Event::Drop(_)|Event::DragEnd|Event::Scroll(_)
            |Event::BackPressed{..}|Event::MacosMenuCommand(_)){
            self.gui_v2_abort(cx,"GUI_V2_UNSUPPORTED_INPUT");return Some(false);
        }
        if matches!(event,Event::MouseMove(_)){return Some(false);}
        let Some(id)=self.gui_hooks.pending.clone()else{
            if matches!(event,Event::MouseDown(_)|Event::MouseUp(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)){
                self.gui_v2_abort(cx,"GUI_V2_INPUT_WITHOUT_PENDING_OPERATION");return Some(false);
            }return Some(true);
        };
        let Some(mut input)=self.gui_hooks.persistence.operations.get(&id).cloned()else{return Some(!matches!(event,Event::MouseDown(_)|Event::MouseUp(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)));};
        let checked=(||->Result<(),String>{
            if matches!(event,Event::MouseDown(_)|Event::MouseUp(_)|Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)|Event::Actions(_)){
                self.gui_v2_context(cx,&mut input)?;
            }
            if let Event::Actions(actions)=event {
                for target in wire::Target::ALL {
                    if self.gui_v2_button_clicked(cx,target,actions){
                        life::ensure(input.text.is_none()&&input.target==target&&input.down&&!input.clicked
                            &&self.gui_target_widget(cx,target).try_widget_uid().map(|v|v.0)==Some(input.uid),"GUI_V2_FOREIGN_OR_DUPLICATE_BUTTON_ACTION")?;
                        input.clicked=true;
                    }
                }
                return Ok(());
            }
            if let Some(text)=input.text.as_mut(){
                let ordinary=match event {
                    Event::KeyDown(k)=>Some(state::OrdinaryInput::KeyDown(state::KeyTuple {key_is_a:k.key_code==KeyCode::KeyA,repeat:k.is_repeat,
                        control:k.modifiers.control,shift:k.modifiers.shift,alt:k.modifiers.alt,logo:k.modifiers.logo,time:k.time})),
                    Event::KeyUp(k)=>Some(state::OrdinaryInput::KeyUp(state::KeyTuple {key_is_a:k.key_code==KeyCode::KeyA,repeat:k.is_repeat,
                        control:k.modifiers.control,shift:k.modifiers.shift,alt:k.modifiers.alt,logo:k.modifiers.logo,time:k.time})),
                    Event::TextInput(t)=>Some(state::OrdinaryInput::Text {input:&t.input,replace_last:t.replace_last,was_paste:t.was_paste,
                        composition_none:t.composition.is_none(),full_state_none:t.full_state_sync.is_none(),replace_range_none:t.replace_range.is_none()}),
                    Event::MouseDown(_)|Event::MouseUp(_)=>return Err("GUI_V2_MOUSE_DURING_TEXT".into()),_=>None};
                if let Some(ordinary)=ordinary {
                    let member=text.recipe.project(&ordinary).ok_or("GUI_V2_FOREIGN_ORDINARY_TEXT_EVENT")?;
                    text.machine.admit(member,&self.gui_v2_guard(cx,&input.before)?,&self.gui_v2_path()?).map_err(|_|"GUI_V2_TEXT_ADMISSION_REFUSED")?;
                }return Ok(());
            }
            let (down,point,time,window,button,mods)=match event {
                Event::MouseDown(v)=>(true,v.abs,v.time,v.window_id,v.button,v.modifiers),
                Event::MouseUp(v)=>(false,v.abs,v.time,v.window_id,v.button,v.modifiers),
                Event::KeyDown(_)|Event::KeyUp(_)|Event::TextInput(_)=>return Err("GUI_V2_KEY_DURING_CLICK".into()),_=>return Ok(())};
            life::ensure(window==WindowId(0,0)&&button.bits()==1&&same_point(point,input.point)&&!mods.control&&!mods.shift&&!mods.alt&&!mods.logo
                &&time.to_bits()==(if down{input.down_time}else{input.up_time}).to_bits()
                &&((down&&input.down_queued&&!input.down&&!input.up)||(!down&&input.up_queued&&input.down&&!input.up)),"GUI_V2_ORDINARY_MOUSE_TUPLE_OR_ORDER")?;
            if down{life::ensure(!cx.fingers.any_areas_captured(),"GUI_EXISTING_CAPTURE")?;input.down=true;}else{input.up=true;}Ok(())
        })();
        self.gui_hooks.persistence.operations.insert(id,input);
        match checked{Ok(())=>Some(true),Err(code)=>{self.gui_v2_abort(cx,&code);Some(false)}}
    }
    pub(crate) fn gui_persistence_after_event(&mut self,cx:&mut Cx,event:&Event)->bool {
        if !enabled()||self.gui_hooks.sender.is_none(){return false;}
        if let Some(cleanup)=self.gui_hooks.persistence.cleanup.clone(){
            let matching=match (&cleanup.kind,event){(CleanupKind::Key(k),Event::KeyUp(actual))=>actual==k,
                (CleanupKind::Mouse{point,time},Event::MouseUp(actual))=>same_point(actual.abs,*point)&&actual.time.to_bits()==time.to_bits(),_=>false};
            if matching{
                let binding=self.gui_lifecycle_frozen(cx).ok();
                if binding.as_ref()==Some(&cleanup.binding){
                    if let Some(input)=self.gui_hooks.persistence.operations.get_mut(&cleanup.command){
                        input.cleanup_returned=true;
                        if let Some(text)=input.text.as_mut(){
                            if let Ok(value)=serde_json::to_value(&cleanup.binding){if let Ok(fp)=fingerprint(&value){let _=text.machine.release_observed_return(fp,true);}}
                        }
                    }
                }
                self.gui_hooks.persistence.cleanup=None;return true;
            }
        }
        if self.gui_hooks.fatal.is_some(){return false;}
        if let Err(code)=self.gui_persistence_observe(cx){self.gui_v2_abort(cx,&code);return true;}
        let Some(id)=self.gui_hooks.pending.clone()else{return false;};
        let Some(mut input)=self.gui_hooks.persistence.operations.get(&id).cloned()else{return false;};
        let update=(||->Result<(),String>{
            self.gui_v2_context(cx,&mut input)?;
            if let Some(text)=input.text.as_mut(){
                let member=match event{Event::KeyDown(_)=>Some(state::Member::SelectAllDown),Event::KeyUp(_)=>Some(state::Member::SelectAllUp),
                    Event::TextInput(_)=>Some(state::Member::TextInput),_=>None};
                if let Some(member)=member {text.machine.returned(member,&self.gui_v2_guard(cx,&input.before)?,&self.gui_v2_path()?)
                    .map_err(|_|"GUI_V2_TEXT_RETURN_REFUSED")?;}
                if let Event::Actions(actions)=event{
                    let widget=self.ui.widget(cx,ids!(path_input));life::ensure(widget.try_widget_uid().map(|v|v.0)==Some(input.uid),"GUI_TEXT_UID_CHANGED")?;
                    if let Some(changed)=self.ui.text_input(cx,ids!(path_input)).changed(actions){
                        text.machine.observe_changed_action(&changed).map_err(|_|"GUI_V2_CHANGED_ACTION_REFUSED")?;
                    }
                }
            }else{
                let path=self.gui_v2_path()?;
                if matches!(event,Event::MouseDown(_))&&input.down{
                    input.down_returned=true;if let Some(focus)=input.focus.as_mut(){life::ensure(focus.ordinary_down_returned(&self.gui_v2_guard(cx,&input.before)?,&path),"GUI_PATH_DOWN_RETURN_REFUSED")?;}
                }
                if matches!(event,Event::MouseUp(_))&&input.up{
                    input.up_returned=true;if let Some(focus)=input.focus.as_mut(){life::ensure(focus.ordinary_up_returned(&self.gui_v2_guard(cx,&input.before)?,&path),"GUI_PATH_UP_RETURN_REFUSED")?;}
                }
            }Ok(())
        })();
        self.gui_hooks.persistence.operations.insert(id.clone(),input.clone());
        if let Err(code)=update{self.gui_v2_abort(cx,&code);return true;}
        if let Some(text)=&input.text{
            if text.machine.next_member().is_some(){if let Err(code)=self.gui_v2_queue_next(&id){self.gui_v2_abort(cx,&code);}return true;}
        }
        if !matches!(event,Event::Draw(_)){return true;}
        let Some(serial)=self.gui_hooks.draw_serial.checked_add(1)else{self.gui_v2_abort(cx,"GUI_DRAW_SERIAL_EXHAUSTED");return true;};
        let current=match self.gui_snapshot_now(cx,serial){Ok(v)=>v,Err(code)=>{self.gui_v2_abort(cx,&code);return true;}};
        self.gui_hooks.draw_serial=serial;self.gui_hooks.snapshot=Some(current.clone());
        let finished=(||->Result<bool,String>{
            self.gui_v2_context(cx,&mut input)?;
            if let Some(text)=input.text.as_mut(){
                let old=input.before.model.pointer("/persistence/path/text").and_then(Value::as_str).ok_or("GUI_TEXT_BEFORE_MISSING")?;
                if old!=text.desired&&!text.machine.changed_action_seen(){cx.redraw_all();return Ok(false);}
                text.machine.finish_after_draw(serial,&self.gui_v2_guard(cx,&input.before)?,&self.gui_v2_path()?).map_err(|_|"GUI_V2_TEXT_POST_DRAW_REFUSED")?;
            }else{
                if !input.down_returned||!input.up_returned{return Ok(false);}
                if let Some(focus)=input.focus.as_mut(){
                    let guard=self.gui_v2_guard(cx,&input.before)?;
                    if !guard.actual_key_focus{cx.redraw_all();return Ok(false);}
                    life::ensure(focus.finish_after_draw(serial,&guard,&self.gui_v2_path()?),"GUI_REAL_PATH_FOCUS_POST_DRAW_REQUIRED")?;
                }else{if !input.clicked{return Ok(false);}}
                if target_needs_handler(input.target){
                    if !input.handler_returned{return Ok(false);}
                    life::ensure(input.handler_ok,"GUI_V2_ORDINARY_DOCUMENT_HANDLER_FAILED")?;
                    if !self.circular_is_quiescent(){cx.redraw_all();return Ok(false);}
                    life::ensure(input.transition.finish_after_draw(&self.gui_v2_model()?),"GUI_V2_FINAL_DOCUMENT_TRANSITION_REQUIRED")?;
                    if matches!(input.target,Target::OpenDocument|Target::SaveDocument){life::ensure(input.file.is_some(),"GUI_V2_FILE_RECEIPT_REQUIRED")?;}
                }
                if input.target.legacy()&&self.circular_ui.panel.active&&!self.circular_ui.panel.view_3d {
                    if !self.gui_post_click_2d_draw_ready(cx,&input.before,&current){cx.redraw_all();return Ok(false);}
                }
            }Ok(true)
        })();
        self.gui_hooks.persistence.operations.insert(id.clone(),input.clone());
        match finished{Ok(false)=>return true,Err(code)=>{self.gui_v2_abort(cx,&code);return true;},Ok(true)=>{}}
        if let Err(code)=issue_snapshot(&mut self.gui_hooks.issued,self.gui_hooks.consumed_serial,current.clone()){
            self.gui_v2_abort(cx,&code);return true;
        }
        input.complete=true;self.gui_hooks.persistence.operations.insert(id.clone(),input.clone());
        if let Some(op)=self.gui_hooks.operations.get_mut(&id){
            op.phase="ORDINARY_INPUT_AND_POST_DRAW_OBSERVED";op.down=input.down;op.up=input.up;op.down_returned=input.down_returned;
            op.up_returned=input.up_returned;op.clicked=input.clicked;
            op.post=Some(json!({"snapshot":current.json(),"target":input.target.name(),"path_focus_proved":input.focus.is_some(),
                "handler_returned":input.handler_returned,"handler_ok":input.handler_ok,"file":input.file,
                "dispatch_certainty":"OBSERVED_COMPLETE","Fit_handler_success":"NOT_INFERRED_FROM_CLICK","full_G_H_acceptance":false}));
        }
        self.gui_hooks.pending=None;self.gui_hooks.last_action=Some(current);true
    }
    fn gui_v2_abort(&mut self,cx:&mut Cx,code:&str){
        if self.gui_hooks.persistence.cleanup.is_none(){
            if let Some(id)=self.gui_hooks.pending.clone(){if let Some(mut input)=self.gui_hooks.persistence.operations.get(&id).cloned(){
                let kind=if let Some(text)=input.text.as_mut(){text.machine.abort("GUI_V2_ABORTED");
                    text.machine.release_required().then(||CleanupKind::Key(key(input.up_time)))
                }else{(input.down_queued&&!input.up_returned).then_some(CleanupKind::Mouse{point:input.point,time:input.up_time})};
                self.gui_hooks.persistence.operations.insert(id.clone(),input.clone());
                if let Some(kind)=kind{
                    if self.gui_lifecycle_frozen(cx).ok().as_ref()==Some(&input.before.binding)&&!Cx::has_studio_web_socket()&&supported_app_argv(){
                        let event=match &kind{CleanupKind::Key(k)=>StudioToApp::KeyUp(*k),CleanupKind::Mouse{point,time}=>StudioToApp::MouseUp(RemoteMouseUp {
                            button_raw_bits:1,x:point.x,y:point.y,time:*time,modifiers:RemoteKeyModifiers::default()})};
                        if self.gui_hooks.sender.as_ref().is_some_and(|s|s.try_send(event).is_ok()){
                            self.gui_hooks.persistence.cleanup=Some(Cleanup {binding:input.before.binding,command:id,kind});SignalToUI::set_ui_signal();
                        }
                    }
                }
            }}
        }
        self.gui_hooks.persistence.permit=None;self.gui_abort(code);
    }
    pub(crate) fn gui_persistence_status(&self,id:&str)->Option<Value>{
        let input=self.gui_hooks.persistence.operations.get(id)?;
        let certainty=if input.complete{"OBSERVED_COMPLETE"}else if input.text.as_ref().is_some_and(|t|t.machine.certainty()==state::Certainty::NotDispatched)
            &&!input.down_queued{"NOT_DISPATCHED"}else{"OUTCOME_UNKNOWN"};
        Some(json!({"target":input.target.name(),"dispatch_certainty":certainty,"complete":input.complete,
            "ordinary_button_clicked":input.clicked,"actual_focus_as_button_clicked":false,
            "text_changed_action_seen":input.text.as_ref().map(|t|t.machine.changed_action_seen()),
            "ordinary_key_up_returned":input.text.as_ref().map(|t|t.machine.ordinary_key_up_returned()),
            "cleanup_release_returned":input.cleanup_returned,"release_required":input.text.as_ref().map(|t|t.machine.release_required()),
            "file":input.file,"full_acceptance":false}))
    }
    pub(crate) fn gui_persistence_document_capture(&mut self,cx:&mut Cx,p:wire::CaptureParams)->Result<Value,String>{
        life::ensure(enabled(),"GUI_DOCUMENT_CAPTURE_UNSUPPORTED")?;self.gui_command_available(&p.command_id)?;
        life::ensure(self.gui_hooks.captures<wire::CAPTURE_COUNT&&!cx.fingers.any_areas_captured()&&self.circular_is_quiescent(),"GUI_DOCUMENT_CAPTURE_BUSY_OR_CAP")?;
        let snapshot=self.gui_live_snapshot(cx,&p.snapshot_id,false)?;
        life::ensure(self.gui_hooks.last_action.as_ref().is_some_and(|v|same_source(v,&snapshot)),"GUI_DOCUMENT_CAPTURE_POST_ACTION_REQUIRED")?;
        let path=PathBuf::from(life::ROOT).join(format!("source13-document-capture-{:02}.png",self.gui_hooks.captures+1));require_absent(&path)?;
        consume_issued(&mut self.gui_hooks.issued,&mut self.gui_hooks.consumed_serial,&p.snapshot_id)?;self.gui_hooks.captures+=1;
        self.gui_hooks.pending=Some(p.command_id.clone());self.gui_hooks.operations.insert(p.command_id.clone(),Operation {kind:Kind::DocumentCapture{path,requested:None},
            before:snapshot,phase:"WAITING_POST_DRAW",down:false,up:false,down_returned:false,up_returned:false,clicked:false,post:None,image:None,error:None,polls:0});
        cx.redraw_all();Ok(json!({"command_id":p.command_id,"phase":"WAITING_POST_DRAW","GPU_completion_proved":false}))
    }
}

#[cfg(test)]mod tests {
    use super::*;
    #[test]fn target_handler_authority_is_closed_and_focus_is_not_a_button_handler(){
        assert!(!target_needs_handler(Target::PathInput));assert!(!target_needs_handler(Target::GccFit));
        assert!(target_needs_handler(Target::SaveDocument));assert!(target_needs_handler(Target::OpenDocument));assert!(target_needs_handler(Target::NewDocument));
    }
    #[test]fn digest_fingerprints_are_exact_and_runtime_is_not_persisted_authority(){
        assert_ne!(fingerprint(&json!({"generation":9_007_199_254_740_993u64})).unwrap(),fingerprint(&json!({"generation":9_007_199_254_740_992u64})).unwrap());
        assert_eq!(hex(&registration::digest(&"a".repeat(64)).unwrap()),"a".repeat(64));
    }
}
