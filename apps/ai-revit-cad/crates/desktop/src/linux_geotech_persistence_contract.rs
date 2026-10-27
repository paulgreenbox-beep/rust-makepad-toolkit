//! SOURCE13 PRIVATE CANDIDATE: pure policy only; no GUI dispatch or acceptance.
//! Compile the separate pure_helpers.rs test harness without a Makepad build.


pub const PATH_BYTES: usize = 512;
pub const INPUT_BASENAME: &str = "input.ai-revit.json";
pub const OUTPUT_BASENAME: &str = "saved.ai-revit.json";
pub const STAGE_BASENAME: &str = ".saved.ai-revit.json.stage";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    WorkspaceCircular, Gcc2d, Gcc3d, GccFit,
    PathInput, SaveDocument, OpenDocument, NewDocument,
}
impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Self::WorkspaceCircular => "workspace_circular", Self::Gcc2d => "gcc_2d",
            Self::Gcc3d => "gcc_3d", Self::GccFit => "gcc_fit", Self::PathInput => "path_input",
            Self::SaveDocument => "save_document", Self::OpenDocument => "open_document",
            Self::NewDocument => "new_document",
        }
    }
}
pub fn normalized_absolute_path(value: &str) -> bool {
    // Unix path policy is deliberately platform independent for the pure harness.
    // Reject aliases before any platform Path processing or I/O.
    !value.is_empty() && value.len() <= PATH_BYTES && value.starts_with('/') && value != "/"
        && !value.ends_with('/') && !value.contains('\\')
        && value.chars().all(|c| !c.is_control())
        && value[1..].split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredPaths { root: String }
impl RegisteredPaths {
    /// Trusted startup only. This constructor grants no request-time registration.
    pub fn new(root: &str) -> Result<Self, &'static str> {
        if !normalized_absolute_path(root) || root.len() + OUTPUT_BASENAME.len() + 1 > PATH_BYTES {
            return Err("PERSISTENCE_TRUSTED_ROOT_INVALID");
        }
        Ok(Self { root: root.to_owned() })
    }
    pub fn root(&self) -> &str { &self.root }
    pub fn input(&self) -> String { format!("{}/{}", self.root, INPUT_BASENAME) }
    pub fn output(&self) -> String { format!("{}/{}", self.root, OUTPUT_BASENAME) }
    pub fn registered(&self, text: &str) -> bool {
        normalized_absolute_path(text) && (text == self.input() || text == self.output())
    }
    pub fn save_path(&self, text: &str) -> bool { normalized_absolute_path(text) && text == self.output() }
}
pub fn valid_input_text(text: &str) -> bool {
    normalized_absolute_path(text) && text.chars().count() <= PATH_BYTES
}
pub fn valid_point(u: f64, v: f64) -> bool {
    u.is_finite() && v.is_finite() && u > 0.0 && u < 1.0 && v > 0.0 && v < 1.0
}
pub fn exact_snapshot_generation(id: &str, generation: u64) -> bool {
    generation != 0 && id == format!("g{generation:016x}")
}
pub fn next_text_generation(old: u64, old_text: &str, new_text: &str) -> Result<u64, &'static str> {
    if old == 0 { return Err("PERSISTENCE_TEXT_GENERATION_ZERO"); }
    if old_text == new_text { Ok(old) }
    else { old.checked_add(1).ok_or("PERSISTENCE_TEXT_GENERATION_EXHAUSTED") }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentKey {
    pub uuid: String, pub runtime: String, pub open_generation: u64, pub revision: u64,
    // Digest of canonical persisted document semantics, excluding process/runtime.
    pub semantic_digest: [u8; 32], pub archive_digest: Option<[u8; 32]>,
    pub report_digest: Option<[u8; 32]>,
}
impl DocumentKey {
    pub fn valid(&self) -> bool {
        !self.uuid.is_empty() && self.uuid.len() <= 128 && !self.runtime.is_empty()
            && self.runtime.len() <= 128 && self.open_generation != 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedKey {
    pub uuid: String, pub revision: u64, pub semantic_digest: [u8; 32],
    pub archive_digest: Option<[u8; 32]>, pub report_digest: Option<[u8; 32]>,
}
impl PersistedKey {
    pub fn matches(&self, d: &DocumentKey) -> bool {
        self.uuid == d.uuid && self.revision == d.revision && self.semantic_digest == d.semantic_digest
            && self.archive_digest == d.archive_digest && self.report_digest == d.report_digest
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoShape { pub walls: u64, pub floors: u64, pub roofs: u64, pub elements: u64 }
impl DemoShape {
    pub fn current_demo(self) -> bool { self == Self { walls:4, floors:1, roofs:1, elements:6 } }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelObservation {
    pub document: DocumentKey, pub analyze_dispatches: u64, pub worker_quiescent: bool,
    pub demo: Option<DemoShape>,
}
#[derive(Clone, Debug)]
pub enum Transition {
    Preserve,
    NewDemo,
    Open(PersistedKey),
}
impl Transition {
    /// Sole document-state relaxation. All binding/layout/Area/TTL guards remain
    /// mandatory at the App adapter. This helper cannot authorize an input event.
    pub fn final_matches(&self, before: &ModelObservation, after: &ModelObservation) -> bool {
        if !before.document.valid() || !after.document.valid() || !before.worker_quiescent
            || !after.worker_quiescent || before.analyze_dispatches != after.analyze_dispatches {
            return false;
        }
        match self {
            Self::Preserve => before.document == after.document,
            Self::NewDemo | Self::Open(_) => {
                if before.document.open_generation.checked_add(1) != Some(after.document.open_generation)
                    || before.document.runtime == after.document.runtime { return false; }
                match self {
                    Self::NewDemo => before.document.uuid != after.document.uuid
                        && after.document.archive_digest.is_none() && after.document.report_digest.is_none()
                        && after.demo.is_some_and(DemoShape::current_demo),
                    Self::Open(expected) => expected.matches(&after.document),
                    Self::Preserve => unreachable!(),
                }
            }
        }
    }
    /// A normal Button may emit its action during Down, before Up. Accept only
    /// the exact original observation or a complete declared target transition;
    /// never accept an arbitrary intermediate/revision state.
    pub fn between_members(&self, before: &ModelObservation, current: &ModelObservation) -> bool {
        (before == current && before.worker_quiescent && before.document.valid())
            || self.final_matches(before, current)
    }
}

/// Pins the first complete allowed document transition. A second different New
/// document cannot exploit the broad "distinct UUID" predicate during one pair.
#[derive(Clone, Debug)]
pub struct TransitionTracker {
    before: ModelObservation, rule: Transition, committed: Option<ModelObservation>,
}
impl TransitionTracker {
    pub fn new(before: ModelObservation, rule: Transition) -> Self {Self {before,rule,committed:None}}
    pub fn observe_between_members(&mut self, current: &ModelObservation) -> bool {
        if let Some(committed)=&self.committed {return committed==current;}
        if self.before==*current {return self.before.document.valid() && current.worker_quiescent;}
        if !self.rule.final_matches(&self.before,current) {return false;}
        self.committed=Some(current.clone());true
    }
    pub fn finish_after_draw(&mut self, current: &ModelObservation) -> bool {
        self.rule.final_matches(&self.before,current) && self.observe_between_members(current)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> ModelObservation {
        ModelObservation { document: DocumentKey {uuid:"saved-uuid".into(), runtime:"r1".into(),
            open_generation:7,revision:u64::MAX,semantic_digest:[2;32],archive_digest:Some([3;32]),report_digest:Some([4;32])},
            analyze_dispatches:11,worker_quiescent:true,demo:None }
    }
    #[test] fn snapshots_and_text_generations_preserve_integers_above_double_precision() {
        let n=9_007_199_254_740_993;
        assert!(exact_snapshot_generation("g0020000000000001",n));
        assert!(!exact_snapshot_generation("g0020000000000000",n));
        assert_eq!(next_text_generation(n,"old","new"),Ok(n+1));
        assert_eq!(next_text_generation(u64::MAX,"same","same"),Ok(u64::MAX));
        assert!(next_text_generation(u64::MAX,"old","new").is_err());
    }
    #[test] fn registered_paths_reject_aliases_controls_traversal_and_nearby_names() {
        let p=RegisteredPaths::new("/var/tmp/owned-001").unwrap();
        assert!(p.registered(&p.input())); assert!(p.save_path(&p.output()));
        for s in ["/var/tmp/owned-001/../owned-001/input.ai-revit.json","/var/tmp/owned-001//input.ai-revit.json",
            "/var/tmp/owned-001/./input.ai-revit.json","/var/tmp/owned-001/input.ai-revit.json/",
            "/var/tmp/owned-001/input.ai-revit.json\n","/var/tmp/owned-001/input.ai-revit.json\0",
            "/var/tmp/owned-001/input.ai-revit.json.bak","input.ai-revit.json","/var/tmp/owned-001/other.json"] {
            assert!(!p.registered(s),"{s:?}");
        }
        assert!(!p.save_path(&p.input()));
        for x in [0.0,1.0,f64::NAN,f64::INFINITY] { assert!(!valid_point(x,0.5)); }
    }
    #[test] fn open_may_restore_same_uuid_but_requires_new_runtime_exact_generation_and_digests() {
        let before=model(); let mut after=before.clone();
        after.document.open_generation+=1; after.document.runtime="r2".into();
        let expected=PersistedKey {uuid:after.document.uuid.clone(),revision:after.document.revision,
            semantic_digest:after.document.semantic_digest,archive_digest:after.document.archive_digest,
            report_digest:after.document.report_digest};
        let rule=Transition::Open(expected);
        assert!(rule.between_members(&before,&before)); assert!(rule.final_matches(&before,&after));
        for field in 0..5 {
            let mut corrupt=after.clone();
            match field {0=>corrupt.document.runtime=before.document.runtime.clone(),1=>corrupt.document.open_generation+=1,
                2=>corrupt.document.archive_digest=None,3=>corrupt.analyze_dispatches+=1,_=>corrupt.document.semantic_digest=[9;32]};
            assert!(!rule.between_members(&before,&corrupt));
        }
    }
    #[test] fn new_is_distinct_demo_and_never_empty_or_saturating_generation() {
        let before=model(); let mut after=before.clone();
        after.document.uuid="new-uuid".into(); after.document.runtime="r2".into(); after.document.open_generation+=1;
        after.document.archive_digest=None; after.document.report_digest=None;
        after.demo=Some(DemoShape {walls:4,floors:1,roofs:1,elements:6});
        assert!(Transition::NewDemo.final_matches(&before,&after));
        let mut empty=after.clone(); empty.demo=Some(DemoShape {walls:0,floors:0,roofs:0,elements:0});
        assert!(!Transition::NewDemo.final_matches(&before,&empty));
        let mut saturated=before.clone(); saturated.document.open_generation=u64::MAX;
        after.document.open_generation=u64::MAX;
        assert!(!Transition::NewDemo.final_matches(&saturated,&after));
        assert!(!Transition::Preserve.between_members(&before,&after));
    }
    #[test] fn down_action_transition_does_not_allow_partial_open_or_unrelated_revision() {
        let before=model(); let mut partial=before.clone(); partial.document.open_generation+=1;
        assert!(!Transition::NewDemo.between_members(&before,&partial));
        let mut changed=before.clone(); changed.document.revision-=1;
        assert!(!Transition::Preserve.between_members(&before,&changed));
        let mut worker=before.clone(); worker.worker_quiescent=false;
        assert!(!Transition::Preserve.between_members(&before,&worker));
    }
    #[test] fn first_new_transition_is_latched_across_mouse_up_and_post_draw() {
        let before=model();let mut first=before.clone();
        first.document.uuid="new-one".into();first.document.runtime="runtime-two".into();first.document.open_generation+=1;
        first.document.archive_digest=None;first.document.report_digest=None;
        first.demo=Some(DemoShape {walls:4,floors:1,roofs:1,elements:6});
        let mut second=first.clone();second.document.uuid="new-two".into();second.document.runtime="runtime-three".into();
        assert!(Transition::NewDemo.final_matches(&before,&second));
        let mut tracker=TransitionTracker::new(before.clone(),Transition::NewDemo);
        assert!(tracker.observe_between_members(&before));assert!(tracker.observe_between_members(&first));
        assert!(!tracker.observe_between_members(&before));assert!(!tracker.observe_between_members(&second));
        assert!(tracker.finish_after_draw(&first));
    }}