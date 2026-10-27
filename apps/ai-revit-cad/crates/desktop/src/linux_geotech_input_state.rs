//! SOURCE13 PRIVATE CANDIDATE: operation state, not a Makepad event adapter.
//! Actual focus must come from cx.has_key_focus(current TextInput.area()).
use super::linux_geotech_persistence_contract::{next_text_generation, valid_input_text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certainty { NotDispatched, OutcomeUnknown, ObservedComplete }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal { pub code: &'static str, pub certainty: Certainty }
pub fn unsupported_input() -> Refusal {
    Refusal {code:"PERSISTENCE_INPUT_ADAPTER_UNSUPPORTED",certainty:Certainty::NotDispatched}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableContext {
    // Canonical fingerprints computed from the already validated full native
    // Binding/layout/source tuple. Never accept these as request-provided proof.
    pub binding: [u8; 32], pub layout: [u8; 32], pub source: [u8; 32],
    pub target_uid: u64, pub area_slot: [u8; 32], pub issued_geometry_generation: u64,
}
#[derive(Clone, Debug)]
pub struct Guard {
    pub context: ImmutableContext, pub area_current_and_lease_matches: bool,
    pub actual_key_focus: bool, pub writable: bool, pub visible: bool,
    pub elapsed_ms: u64, pub original_ttl_ms: u64,
}
fn immutable_guard_valid(frozen:&ImmutableContext,guard:&Guard)->bool {
    guard.context == *frozen && frozen.target_uid != 0 && frozen.issued_geometry_generation != 0
        && guard.area_current_and_lease_matches && guard.writable && guard.visible
        && guard.original_ttl_ms == 5000 && guard.elapsed_ms <= guard.original_ttl_ms
}#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathState { pub text: String, pub generation: u64 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Member { SelectAllDown, SelectAllUp, TextInput }
/// Exact projection of a real ordinary Makepad event, constructed ONLY in the
/// App adapter by matching KeyCode::KeyA / TextInputEvent, never from RPC data.
#[derive(Clone, Copy, Debug)]
pub struct KeyTuple {
    pub key_is_a: bool, pub repeat: bool, pub control: bool, pub shift: bool,
    pub alt: bool, pub logo: bool, pub time: f64,
}
#[derive(Clone, Debug)]
pub enum OrdinaryInput<'a> {
    KeyDown(KeyTuple), KeyUp(KeyTuple),
    Text {input: &'a str,replace_last:bool,was_paste:bool,composition_none:bool,
        full_state_none:bool,replace_range_none:bool},
}
#[derive(Clone, Debug)]
pub struct ExactRecipe {down:f64,up:f64,text:String}
impl ExactRecipe {
    pub fn new(down:f64,up:f64,text:String)->Result<Self,&'static str> {
        if !down.is_finite() || !up.is_finite() || down<0.0 || up<=down || !valid_input_text(&text) {
            return Err("PERSISTENCE_RECIPE_INVALID");
        }
        Ok(Self {down,up,text})
    }
    fn key_matches(&self,k:KeyTuple,time:f64)->bool {
        k.key_is_a && !k.repeat && k.control && !k.shift && !k.alt && !k.logo
            && k.time.to_bits()==time.to_bits()
    }
    pub fn project(&self,event:&OrdinaryInput<'_>)->Option<Member> {
        match event {
            OrdinaryInput::KeyDown(k) if self.key_matches(*k,self.down)=>Some(Member::SelectAllDown),
            OrdinaryInput::KeyUp(k) if self.key_matches(*k,self.up)=>Some(Member::SelectAllUp),
            OrdinaryInput::Text {input,replace_last,was_paste,composition_none,full_state_none,replace_range_none}
                if *input==self.text.as_str() && !*replace_last && !*was_paste && *composition_none && *full_state_none
                    && *replace_range_none=>Some(Member::TextInput),
            _=>None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase { Down, Up, Text, PostDraw, Complete, Aborted }
#[derive(Clone, Debug)]
pub struct TextReplace {
    frozen: ImmutableContext, before: PathState, desired: String, after_generation: u64,
    before_draw: u64, phase: Phase, queued: Option<Member>, admitted: Option<Member>,
    sent_any: bool, down_returned: bool, up_returned: bool, text_returned: bool,
    changed_action_seen: bool, key_may_be_down: bool, release_returned: bool,
    error: Option<&'static str>,
}
impl TextReplace {
    pub fn reserve(guard: &Guard, before: PathState, desired: String,
        expected_text_generation: u64, before_draw: u64) -> Result<Self, Refusal> {
        let refuse=|code|Refusal {code,certainty:Certainty::NotDispatched};
        if before.generation != expected_text_generation || !valid_input_text(&desired)
            || before.text.len() > 512 || before.text.chars().any(char::is_control) || before_draw == 0 { return Err(refuse("PERSISTENCE_TEXT_OR_GENERATION_INVALID")); }
        if !Self::guard_valid(&guard.context,guard) { return Err(refuse("PERSISTENCE_FOCUS_OR_LEASE_NOT_READY")); }
        let after_generation=next_text_generation(before.generation,&before.text,&desired).map_err(refuse)?;
        Ok(Self {frozen:guard.context.clone(),before,desired,after_generation,before_draw,phase:Phase::Down,
            queued:None,admitted:None,sent_any:false,down_returned:false,up_returned:false,text_returned:false,
            changed_action_seen:false,key_may_be_down:false,release_returned:false,error:None})
    }
    fn guard_valid(frozen: &ImmutableContext, guard: &Guard) -> bool {
        immutable_guard_valid(frozen,guard) && guard.actual_key_focus
    }
    pub fn next_member(&self) -> Option<Member> {
        if self.queued.is_some() || self.admitted.is_some() {return None;}
        match self.phase {Phase::Down=>Some(Member::SelectAllDown),Phase::Up=>Some(Member::SelectAllUp),
            Phase::Text=>Some(Member::TextInput),_=>None}
    }
    pub fn certainty(&self) -> Certainty {
        if self.phase == Phase::Complete {Certainty::ObservedComplete}
        else if self.sent_any {Certainty::OutcomeUnknown} else {Certainty::NotDispatched}
    }
    fn fail<T>(&mut self, code: &'static str) -> Result<T,Refusal> {
        self.phase=Phase::Aborted; self.error=Some(code);
        Err(Refusal {code,certainty:self.certainty()})
    }
    /// Call only AFTER successful try_send of this operation's exact ordinary
    /// event. A failed first enqueue is not a dispatch; do not call this method.
    pub fn sent(&mut self, member: Member) -> Result<(),Refusal> {
        // A successful enqueue is irrevocable knowledge even if an adapter
        // reports a member that violates this operation's expected order.
        let expected=self.next_member();
        self.sent_any=true;
        if member == Member::SelectAllDown {
            self.key_may_be_down=true;
            // A late owned Down after an earlier cleanup creates a NEW release
            // obligation. Do not reuse the earlier release-return observation.
            self.release_returned=false;
        }
        if expected != Some(member) {return self.fail("PERSISTENCE_EVENT_QUEUE_ORDER");}
        self.queued=Some(member);
        Ok(())
    }
    pub fn admit(&mut self, member: Member, guard: &Guard, current: &PathState) -> Result<(),Refusal> {
        if self.phase == Phase::Aborted || self.phase == Phase::Complete || self.queued != Some(member) || self.admitted.is_some() || current != &self.before
            || !Self::guard_valid(&self.frozen,guard) {return self.fail("PERSISTENCE_MEMBER_GUARD_CHANGED");}
        self.admitted=Some(member); Ok(())
    }
    pub fn returned(&mut self, member: Member, guard: &Guard, current: &PathState) -> Result<(),Refusal> {
        if self.phase == Phase::Aborted || self.phase == Phase::Complete || self.admitted != Some(member) || !Self::guard_valid(&self.frozen,guard) {
            return self.fail("PERSISTENCE_MEMBER_RETURN_GUARD_CHANGED");
        }
        let text_ok=if member == Member::TextInput {
            current.text == self.desired && current.generation == self.after_generation
        } else {current == &self.before};
        if !text_ok {return self.fail("PERSISTENCE_ORDINARY_TEXT_READBACK_MISMATCH");}
        match member {
            Member::SelectAllDown=>{self.down_returned=true;self.phase=Phase::Up;},
            Member::SelectAllUp=>{self.up_returned=true;self.key_may_be_down=false;self.phase=Phase::Text;},
            Member::TextInput=>{self.text_returned=true;self.phase=Phase::PostDraw;},
        }
        self.queued=None; self.admitted=None; Ok(())
    }
    /// Receives only actual TextInputRef.changed(actions), with exact UID.
    /// A same-text Changed action is recorded honestly and does not increment
    /// generation unless observed bytes changed. It is never manufactured.
    pub fn observe_changed_action(&mut self, text: &str) -> Result<(),Refusal> {
        if self.phase == Phase::Aborted || self.phase == Phase::Complete || !self.sent_any
            || !(self.admitted == Some(Member::TextInput) || self.text_returned)
            || text != self.desired {return self.fail("PERSISTENCE_CHANGED_ACTION_UNEXPECTED");}
        self.changed_action_seen=true; Ok(())
    }
    pub fn finish_after_draw(&mut self, serial: u64, guard: &Guard, current: &PathState) -> Result<(),Refusal> {
        if self.phase != Phase::PostDraw || serial <= self.before_draw || !self.down_returned
            || !self.up_returned || !self.text_returned || self.key_may_be_down
            || !Self::guard_valid(&self.frozen,guard) || current.text != self.desired
            || current.generation != self.after_generation
            || (self.before.text != self.desired && !self.changed_action_seen) {
            return self.fail("PERSISTENCE_POST_DRAW_NOT_PROVED");
        }
        self.phase=Phase::Complete; Ok(())
    }
    pub fn abort(&mut self, code: &'static str) -> Refusal {
        self.phase=Phase::Aborted; self.error=Some(code);
        Refusal {code,certainty:self.certainty()}
    }
    pub fn release_required(&self) -> bool {self.key_may_be_down && !self.release_returned}
    /// Focus loss cannot suppress release of an operation-owned key. The App
    /// adapter still requires exact original native binding and owned channel.
    pub fn may_release(&self, current_binding: [u8;32], channel_owned: bool) -> bool {
        self.phase == Phase::Aborted && self.release_required() && channel_owned && current_binding == self.frozen.binding
    }
    pub fn release_observed_return(&mut self, current_binding: [u8;32], channel_owned: bool) -> Result<(),Refusal> {
        if !self.may_release(current_binding,channel_owned) {return self.fail("PERSISTENCE_RELEASE_BINDING_UNKNOWN");}
        self.release_returned=true; self.key_may_be_down=false; Ok(())
    }
    pub fn changed_action_seen(&self) -> bool {self.changed_action_seen}
    pub fn ordinary_key_up_returned(&self) -> bool {self.up_returned}
    pub fn error(&self) -> Option<&'static str> {self.error}
}

/// Target-specific path-click proof. Focus may legitimately change on Down;
/// path bytes/document/layout/Area do not. This is not a Button.clicked proof.
/// Existing ordinary mouse tuple/order/dispatch-certainty machinery is still
/// mandatory. Caller records these return flags only after real UI handlers.
#[derive(Clone, Debug)]
pub struct PathFocusTracker {
    frozen:ImmutableContext,before:PathState,before_draw:u64,focus_seen:bool,
    down_returned:bool,up_returned:bool,failed:bool,
}
impl PathFocusTracker {
    pub fn reserve(guard:&Guard,before:PathState,before_draw:u64)->Result<Self,Refusal> {
        let mut tracker=Self {frozen:guard.context.clone(),before,before_draw,focus_seen:guard.actual_key_focus,
            down_returned:false,up_returned:false,failed:false};
        if before_draw==0 || tracker.before.generation==0 || tracker.before.text.len()>512
            || tracker.before.text.chars().any(char::is_control) || !tracker.observe_member(guard,&tracker.before.clone()) {
            return Err(Refusal {code:"PERSISTENCE_PATH_CLICK_NOT_READY",certainty:Certainty::NotDispatched});
        }
        Ok(tracker)
    }
    pub fn observe_member(&mut self,guard:&Guard,path:&PathState)->bool {
        // All immutable guards remain exact. The only permitted focus change
        // is observed false-to-true acquisition, with actual focus at post-Draw.
        let ok=!self.failed && immutable_guard_valid(&self.frozen,guard) && path==&self.before
            && (!self.focus_seen || guard.actual_key_focus);
        if !ok {self.failed=true;return false;}
        self.focus_seen |= guard.actual_key_focus;true
    }
    pub fn ordinary_down_returned(&mut self,guard:&Guard,path:&PathState)->bool {
        if self.down_returned || self.up_returned || !self.observe_member(guard,path) {self.failed=true;return false;}
        self.down_returned=true;true
    }
    pub fn ordinary_up_returned(&mut self,guard:&Guard,path:&PathState)->bool {
        if !self.down_returned || self.up_returned || !self.observe_member(guard,path) {self.failed=true;return false;}
        self.up_returned=true;true
    }
    pub fn finish_after_draw(&mut self,serial:u64,guard:&Guard,path:&PathState)->bool {
        self.observe_member(guard,path) && serial>self.before_draw && self.down_returned && self.up_returned
            && guard.actual_key_focus && self.focus_seen
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn guard() -> Guard {Guard {context:ImmutableContext {binding:[1;32],layout:[2;32],source:[3;32],
        target_uid:91,area_slot:[4;32],issued_geometry_generation:9_007_199_254_740_993},
        area_current_and_lease_matches:true,actual_key_focus:true,writable:true,visible:true,elapsed_ms:1,original_ttl_ms:5000}}
    fn path() -> PathState {PathState {text:"/var/tmp/owned/input.ai-revit.json".into(),generation:9_007_199_254_740_993}}
    fn reserved() -> TextReplace {TextReplace::reserve(&guard(),path(),"/var/tmp/owned/saved.ai-revit.json".into(),path().generation,4).unwrap()}
    fn complete_member(op:&mut TextReplace,m:Member,g:&Guard,p:&PathState) {
        op.sent(m).unwrap();op.admit(m,g,&path()).unwrap();op.returned(m,g,p).unwrap();
    }
    #[test] fn missing_real_focus_refuses_without_any_dispatch() {
        let mut g=guard();g.actual_key_focus=false;
        let e=TextReplace::reserve(&g,path(),path().text,path().generation,4).unwrap_err();
        assert_eq!(e.certainty,Certainty::NotDispatched);
        assert_eq!(unsupported_input().certainty,Certainty::NotDispatched);
    }
    #[test] fn focus_loss_after_down_demands_owned_release_and_keeps_outcome_unknown() {
        let mut op=reserved();complete_member(&mut op,Member::SelectAllDown,&guard(),&path());
        let mut g=guard();g.actual_key_focus=false;
        op.sent(Member::SelectAllUp).unwrap();
        assert_eq!(op.admit(Member::SelectAllUp,&g,&path()).unwrap_err().certainty,Certainty::OutcomeUnknown);
        assert!(op.release_required());assert!(!op.may_release([9;32],true));
        op.release_observed_return(guard().context.binding,true).unwrap();
        assert!(!op.release_required());assert_eq!(op.certainty(),Certainty::OutcomeUnknown);
        assert_eq!(op.next_member(),None);
    }
    #[test] fn changed_document_or_area_between_down_and_up_cannot_be_replayed() {
        for mutation in 0..3 {
            let mut op=reserved();complete_member(&mut op,Member::SelectAllDown,&guard(),&path());
            let mut g=guard(); match mutation {0=>g.context.source=[9;32],1=>g.area_current_and_lease_matches=false,
                _=>g.context.issued_geometry_generation-=1};
            op.sent(Member::SelectAllUp).unwrap();assert!(op.admit(Member::SelectAllUp,&g,&path()).is_err());
            assert_eq!(op.certainty(),Certainty::OutcomeUnknown); assert_eq!(op.next_member(),None);
        }
    }
    #[test] fn text_needs_up_return_and_changed_readback_and_fresh_draw() {
        let mut op=reserved();let refused=op.sent(Member::TextInput).unwrap_err();
        assert_eq!(refused.certainty,Certainty::OutcomeUnknown);assert!(!op.release_required());
        let mut op=reserved();complete_member(&mut op,Member::SelectAllDown,&guard(),&path());
        complete_member(&mut op,Member::SelectAllUp,&guard(),&path());
        let after=PathState {text:op.desired.clone(),generation:path().generation+1};
        op.sent(Member::TextInput).unwrap();op.admit(Member::TextInput,&guard(),&path()).unwrap();
        op.observe_changed_action(&after.text).unwrap();op.returned(Member::TextInput,&guard(),&after).unwrap();
        let mut stale=op.clone();assert!(stale.finish_after_draw(4,&guard(),&after).is_err());
        op.finish_after_draw(5,&guard(),&after).unwrap();assert_eq!(op.certainty(),Certainty::ObservedComplete);
        assert!(op.ordinary_key_up_returned());assert!(op.changed_action_seen());
    }
    #[test] fn same_text_keeps_generation_without_inventing_changed_action() {
        let mut op=TextReplace::reserve(&guard(),path(),path().text,path().generation,4).unwrap();
        complete_member(&mut op,Member::SelectAllDown,&guard(),&path());
        complete_member(&mut op,Member::SelectAllUp,&guard(),&path());
        complete_member(&mut op,Member::TextInput,&guard(),&path());
        op.finish_after_draw(5,&guard(),&path()).unwrap();assert!(!op.changed_action_seen());
    }
    #[test] fn lost_completion_after_dispatch_is_unknown_even_with_key_release() {
        let mut op=reserved();op.sent(Member::SelectAllDown).unwrap();
        assert_eq!(op.abort("PERSISTENCE_REPLY_LOST").certainty,Certainty::OutcomeUnknown);
        op.release_observed_return(guard().context.binding,true).unwrap();
        assert_eq!(op.certainty(),Certainty::OutcomeUnknown);
        let mut untouched=reserved();assert_eq!(untouched.abort("PERSISTENCE_QUEUE_UNAVAILABLE").certainty,Certainty::NotDispatched);
    }
    #[test] fn late_return_cannot_revive_aborted_operation_or_prequeue_changed_action() {
        let mut op=reserved();op.sent(Member::SelectAllDown).unwrap();op.admit(Member::SelectAllDown,&guard(),&path()).unwrap();
        assert!(!op.may_release(guard().context.binding,true));
        op.abort("PERSISTENCE_TIMEOUT");assert!(op.returned(Member::SelectAllDown,&guard(),&path()).is_err());
        assert_eq!(op.next_member(),None);assert_eq!(op.certainty(),Certainty::OutcomeUnknown);
        let mut op=reserved();complete_member(&mut op,Member::SelectAllDown,&guard(),&path());
        complete_member(&mut op,Member::SelectAllUp,&guard(),&path());op.sent(Member::TextInput).unwrap();
        let desired=op.desired.clone();assert!(op.observe_changed_action(&desired).is_err());
        assert_eq!(op.certainty(),Certainty::OutcomeUnknown);
    }    #[test] fn recipe_refuses_foreign_keys_repeat_modifiers_times_and_paste_or_composition() {
        let recipe=ExactRecipe::new(10.0,10.001,path().text).unwrap();
        let down=KeyTuple {key_is_a:true,repeat:false,control:true,shift:false,alt:false,logo:false,time:10.0};
        assert_eq!(recipe.project(&OrdinaryInput::KeyDown(down)),Some(Member::SelectAllDown));
        let up=KeyTuple {time:10.001,..down};
        assert_eq!(recipe.project(&OrdinaryInput::KeyUp(up)),Some(Member::SelectAllUp));
        for mutation in 0..5 {let mut wrong=down;match mutation {0=>wrong.key_is_a=false,1=>wrong.repeat=true,
            2=>wrong.logo=true,3=>wrong.control=false,_=>wrong.time=10.001};
            assert_eq!(recipe.project(&OrdinaryInput::KeyDown(wrong)),None);}
        let p=path();let input=OrdinaryInput::Text {input:&p.text,replace_last:false,was_paste:false,
            composition_none:true,full_state_none:true,replace_range_none:true};
        assert_eq!(recipe.project(&input),Some(Member::TextInput));
        let pasted=OrdinaryInput::Text {input:&p.text,replace_last:false,was_paste:true,
            composition_none:true,full_state_none:true,replace_range_none:true};assert_eq!(recipe.project(&pasted),None);
        assert!(ExactRecipe::new(f64::NAN,10.001,p.text).is_err());
    }    #[test] fn path_click_allows_real_focus_acquisition_but_cannot_infer_it_from_mouse_pair() {
        let mut before=guard();before.actual_key_focus=false;
        let mut tracker=PathFocusTracker::reserve(&before,path(),4).unwrap();
        assert!(tracker.ordinary_down_returned(&before,&path()));assert!(tracker.ordinary_up_returned(&before,&path()));
        assert!(!tracker.finish_after_draw(5,&before,&path()));
        let mut tracker=PathFocusTracker::reserve(&before,path(),4).unwrap();
        assert!(tracker.ordinary_down_returned(&before,&path()));
        assert!(tracker.ordinary_up_returned(&guard(),&path()));assert!(tracker.finish_after_draw(5,&guard(),&path()));
        assert!(!tracker.observe_member(&before,&path()));
    }
    #[test] fn path_click_does_not_allow_text_or_source_change_under_focus_exception() {
        let mut before=guard();before.actual_key_focus=false;
        let mut tracker=PathFocusTracker::reserve(&before,path(),4).unwrap();assert!(tracker.ordinary_down_returned(&before,&path()));
        let mut changed=path();changed.generation+=1;changed.text="other".into();
        assert!(!tracker.ordinary_up_returned(&guard(),&changed));assert!(!tracker.finish_after_draw(5,&guard(),&path()));
        let mut tracker=PathFocusTracker::reserve(&before,path(),4).unwrap();let mut other=guard();other.context.source=[8;32];
        assert!(!tracker.observe_member(&other,&path()));
    }    #[test] fn successful_unexpected_first_enqueue_is_unknown_and_cannot_authorize_retry() {
        let mut op=reserved();
        let refusal=op.sent(Member::TextInput).unwrap_err();
        assert_eq!(refusal.code,"PERSISTENCE_EVENT_QUEUE_ORDER");
        assert_eq!(refusal.certainty,Certainty::OutcomeUnknown);
        assert_eq!(op.certainty(),Certainty::OutcomeUnknown);
        assert!(!op.release_required());assert_eq!(op.next_member(),None);
    }
    #[test] fn late_successfully_enqueued_owned_down_rearms_release_after_abort_and_cleanup() {
        let mut never_sent=reserved();
        assert_eq!(never_sent.abort("PERSISTENCE_PREQUEUE_REFUSED").certainty,Certainty::NotDispatched);
        let refusal=never_sent.sent(Member::SelectAllDown).unwrap_err();
        assert_eq!(refusal.certainty,Certainty::OutcomeUnknown);assert!(never_sent.release_required());
        assert!(!never_sent.may_release([9;32],true));assert!(never_sent.may_release(guard().context.binding,true));
        assert_eq!(never_sent.next_member(),None);
        let mut cleaned=reserved();cleaned.sent(Member::SelectAllDown).unwrap();cleaned.abort("PERSISTENCE_LOST_RETURN");
        cleaned.release_observed_return(guard().context.binding,true).unwrap();assert!(!cleaned.release_required());
        let refusal=cleaned.sent(Member::SelectAllDown).unwrap_err();
        assert_eq!(refusal.certainty,Certainty::OutcomeUnknown);assert!(cleaned.release_required());
        assert!(cleaned.may_release(guard().context.binding,true));assert_eq!(cleaned.next_member(),None);
    }
    #[test] fn failed_first_enqueue_does_not_call_sent_and_retains_zero_dispatch_certainty() {
        let mut op=reserved();
        // Simulates try_send failure: the adapter MUST NOT call sent().
        assert_eq!(op.certainty(),Certainty::NotDispatched);assert!(!op.release_required());
        let refusal=op.abort("PERSISTENCE_FIRST_TRY_SEND_FAILED");
        assert_eq!(refusal.certainty,Certainty::NotDispatched);assert!(!op.release_required());
        assert_eq!(op.next_member(),None);
    }}