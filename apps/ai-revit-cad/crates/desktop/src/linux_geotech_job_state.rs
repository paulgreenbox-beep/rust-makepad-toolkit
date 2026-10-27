//! Pure adapter guards; these values are not actual Cx/worker evidence by themselves.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum Member {Down,Up}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum Certainty {NotDispatched,OutcomeUnknown,ObservedComplete}
#[derive(Clone,Copy,Debug,PartialEq)]pub struct Mouse {pub down:bool,pub x:f64,pub y:f64,pub time:f64,pub window:(usize,u64),pub primary:bool,pub modifiers:bool}
#[derive(Clone,Debug)]pub struct Pair {x:f64,y:f64,down:f64,up:f64,sent:bool,held:bool,queued:Option<Member>,admitted:Option<Member>,
    down_returned:bool,up_returned:bool,aborted:bool,complete:bool}
impl Pair {
    pub fn new(x:f64,y:f64,down:f64)->Result<Self,&'static str>{if !x.is_finite()||!y.is_finite()||!down.is_finite()||down<0.0{return Err("JOB_MOUSE_RECIPE");}
        let up=down+0.001;if !up.is_finite()||up<=down{return Err("JOB_MOUSE_CLOCK");}Ok(Self{x,y,down,up,sent:false,held:false,
            queued:None,admitted:None,down_returned:false,up_returned:false,aborted:false,complete:false})}
    pub fn tuple(&self,m:Member)->Mouse{Mouse{down:m==Member::Down,x:self.x,y:self.y,time:if m==Member::Down{self.down}else{self.up},window:(0,0),primary:true,modifiers:false}}
    pub fn next(&self)->Option<Member>{if self.aborted||self.complete||self.queued.is_some()||self.admitted.is_some(){None}else if !self.down_returned{Some(Member::Down)}else if !self.up_returned{Some(Member::Up)}else{None}}
    pub fn sent(&mut self,m:Member)->Result<(),&'static str>{let expected=self.next();self.sent=true;if m==Member::Down{self.held=true;}
        if expected!=Some(m){self.abort();return Err("JOB_MOUSE_QUEUE_ORDER");}self.queued=Some(m);Ok(())}
    pub fn admit(&mut self,actual:Mouse)->Result<Member,&'static str>{let m=if actual.down{Member::Down}else{Member::Up};let expected=self.tuple(m);
        if self.aborted||self.queued!=Some(m)||self.admitted.is_some()||actual.window!=(0,0)||!actual.primary||actual.modifiers
            ||actual.x.to_bits()!=expected.x.to_bits()||actual.y.to_bits()!=expected.y.to_bits()||actual.time.to_bits()!=expected.time.to_bits(){self.abort();return Err("JOB_MOUSE_TUPLE_OR_ORDER");}
        self.admitted=Some(m);Ok(m)}
    pub fn returned(&mut self,m:Member)->Result<(),&'static str>{if self.aborted||self.admitted!=Some(m){self.abort();return Err("JOB_MOUSE_RETURN_ORDER");}
        if m==Member::Down{self.down_returned=true;}else{self.up_returned=true;self.held=false;}self.queued=None;self.admitted=None;Ok(())}
    pub fn down_returned(&self)->bool{self.down_returned}
    pub fn both_returned(&self)->bool{self.down_returned&&self.up_returned&&!self.held}
    pub fn release_required(&self)->bool{self.held}
    pub fn abort(&mut self){self.aborted=true;self.complete=false;}
    pub fn release_returned(&mut self,same_binding:bool){if self.aborted&&same_binding{self.held=false;}}
    pub fn finish(&mut self)->Result<(),&'static str>{if self.aborted||!self.both_returned(){return Err("JOB_MOUSE_NOT_COMPLETE");}self.complete=true;Ok(())}
    pub fn certainty(&self)->Certainty{if self.complete{Certainty::ObservedComplete}else if self.sent{Certainty::OutcomeUnknown}else{Certainty::NotDispatched}}
}
#[derive(Clone,Debug,PartialEq,Eq)]pub struct Version {pub uuid:String,pub runtime:String,pub generation:u64,pub revision:u64,pub semantic:[u8;32],pub archive:Option<[u8;32]>,pub report:Option<[u8;32]>}
#[derive(Clone,Debug)]pub struct ArchiveTransition {before:Version,after:Option<Version>}
impl ArchiveTransition {
    pub fn new(before:Version)->Self{Self{before,after:None}}
    pub fn observe(&self,current:&Version)->bool{self.after.as_ref().unwrap_or(&self.before)==current}
    pub fn commit(&mut self,before_revision:u64,after_revision:u64,replayed:bool,operation_count:usize,current:Version)->Result<(),&'static str>{
        if self.after.is_some()||replayed||operation_count!=1||before_revision!=self.before.revision
            ||self.before.revision.checked_add(1)!=Some(after_revision)||current.revision!=after_revision
            ||current.uuid!=self.before.uuid||current.runtime!=self.before.runtime||current.generation!=self.before.generation
            ||current.semantic==self.before.semantic||current.archive.is_none()||current.report.is_none(){return Err("JOB_ARCHIVE_RECEIPT_OR_IDENTITY");}
        self.after=Some(current);Ok(())}
    pub fn committed(&self)->bool{self.after.is_some()}
}
#[cfg(test)]mod tests{use super::*;
    #[test]fn unexpected_enqueue_latches_unknown_and_release_even_after_abort(){let mut p=Pair::new(1.0,2.0,3.0).unwrap();assert!(p.sent(Member::Up).is_err());assert_eq!(p.certainty(),Certainty::OutcomeUnknown);
        assert!(p.sent(Member::Down).is_err());assert!(p.release_required());p.release_returned(false);assert!(p.release_required());p.release_returned(true);assert!(!p.release_required());assert_eq!(p.certainty(),Certainty::OutcomeUnknown);}
    #[test]fn foreign_tuple_and_release_order_cannot_be_adopted(){let mut p=Pair::new(1.0,2.0,3.0).unwrap();p.sent(Member::Down).unwrap();let mut actual=p.tuple(Member::Down);actual.modifiers=true;
        assert!(p.admit(actual).is_err());assert!(p.returned(Member::Up).is_err());assert_eq!(p.certainty(),Certainty::OutcomeUnknown);}
    #[test]fn pair_cannot_complete_from_clicked_or_down_only(){let mut p=Pair::new(1.0,2.0,3.0).unwrap();p.sent(Member::Down).unwrap();p.admit(p.tuple(Member::Down)).unwrap();p.returned(Member::Down).unwrap();
        assert!(p.finish().is_err());assert!(p.release_required());p.sent(Member::Up).unwrap();p.admit(p.tuple(Member::Up)).unwrap();p.returned(Member::Up).unwrap();p.finish().unwrap();assert_eq!(p.certainty(),Certainty::ObservedComplete);}
    #[test]fn late_successful_down_after_completion_rearms_unknown_release(){let mut p=Pair::new(1.0,2.0,3.0).unwrap();for member in [Member::Down,Member::Up]{
        p.sent(member).unwrap();p.admit(p.tuple(member)).unwrap();p.returned(member).unwrap();}p.finish().unwrap();
        assert!(p.sent(Member::Down).is_err());assert_eq!(p.certainty(),Certainty::OutcomeUnknown);assert!(p.release_required());
        p.release_returned(true);assert!(!p.release_required());assert_eq!(p.certainty(),Certainty::OutcomeUnknown);}
    fn version()->Version{Version{uuid:"u".into(),runtime:"r".into(),generation:9007199254740993,revision:2,semantic:[1;32],archive:Some([2;32]),report:Some([3;32])}}
    #[test]fn archive_changes_require_nonreplayed_exact_one_revision_receipt(){for invalid in 0..4{let before=version();let mut after=before.clone();after.revision+=1;after.semantic=[4;32];let mut t=ArchiveTransition::new(before);
        let(mut revision,mut replayed,mut count)=(3,false,1);match invalid{0=>revision=4,1=>replayed=true,2=>count=2,_=>after.runtime="other".into()}
        assert!(t.commit(2,revision,replayed,count,after).is_err());assert!(!t.committed());}}
    #[test]fn first_archive_after_state_is_latched_and_u64_generation_exact(){let before=version();let mut after=before.clone();after.revision=3;after.semantic=[4;32];let mut t=ArchiveTransition::new(before);
        t.commit(2,3,false,1,after.clone()).unwrap();assert!(t.observe(&after));after.generation+=1;assert!(!t.observe(&after));assert!(t.commit(2,3,false,1,after).is_err());}
    #[test]fn archive_revision_overflow_cannot_wrap_or_saturate(){let mut before=version();before.revision=u64::MAX;let mut after=before.clone();after.revision=0;after.semantic=[4;32];
        let mut t=ArchiveTransition::new(before);assert!(t.commit(u64::MAX,0,false,1,after).is_err());assert!(!t.committed());}
}
