//! SOURCE13 PRIVATE CANDIDATE. v1 click retained; v2 App input source wired.
//! No runtime capability is gained merely from the request schema.
use crate::linux_lifecycle_wire::{self as life,Binding};
use crate::linux_geotech_persistence_contract as policy;
use serde::Deserialize;
pub const REQUEST_CAP:usize=4096;
pub const REQUEST_COUNT:usize=64;
pub const COMMAND_COUNT:usize=16;
pub const CLICK_COUNT:usize=8;
pub const TEXT_COUNT:usize=2;
pub const CAPTURE_COUNT:usize=8;
pub const STATUS_POLLS:usize=4;
pub const PNG_CAP:usize=16 << 20;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Deserialize)]
pub enum Target {
    #[serde(rename="workspace_circular")] WorkspaceCircular,
    #[serde(rename="gcc_2d")] Gcc2d,
    #[serde(rename="gcc_3d")] Gcc3d,
    #[serde(rename="gcc_fit")] GccFit,
    #[serde(rename="path_input")] PathInput,
    #[serde(rename="save_document")] SaveDocument,
    #[serde(rename="open_document")] OpenDocument,
    #[serde(rename="new_document")] NewDocument,
}
impl Target {
    pub const ALL:[Self;8]=[Self::WorkspaceCircular,Self::Gcc2d,Self::Gcc3d,Self::GccFit,
        Self::PathInput,Self::SaveDocument,Self::OpenDocument,Self::NewDocument];
    pub fn policy(self)->policy::Target {
        match self {Self::WorkspaceCircular=>policy::Target::WorkspaceCircular,Self::Gcc2d=>policy::Target::Gcc2d,
            Self::Gcc3d=>policy::Target::Gcc3d,Self::GccFit=>policy::Target::GccFit,Self::PathInput=>policy::Target::PathInput,
            Self::SaveDocument=>policy::Target::SaveDocument,Self::OpenDocument=>policy::Target::OpenDocument,
            Self::NewDocument=>policy::Target::NewDocument}
    }
    pub fn name(self)->&'static str {self.policy().name()}
    pub fn is_text_input(self)->bool {self==Self::PathInput}
    pub fn legacy(self)->bool {matches!(self,Self::WorkspaceCircular|Self::Gcc2d|Self::Gcc3d|Self::GccFit)}
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotParams {pub expected:Binding}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickParams {pub expected:Binding,pub snapshot_id:String,pub target:Target,pub command_id:String}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputParams {
    pub expected:Binding,pub snapshot_id:String,pub geometry_generation:u64,pub target_id:Target,
    pub command_id:String,pub event:InputEvent,
}
#[derive(Deserialize)]
#[serde(tag="kind",deny_unknown_fields)]
pub enum InputEvent {
    #[serde(rename="click")] Click {u:f64,v:f64},
    #[serde(rename="replace_text")] ReplaceText {text:String,expected_text_generation:u64},
}
impl InputParams {
    pub fn validate(&self)->Result<(),String> {
        life::ensure(life::identifier(&self.snapshot_id) && life::identifier(&self.command_id)
            && policy::exact_snapshot_generation(&self.snapshot_id,self.geometry_generation),"GUI_INPUT_GENERATION_INVALID")?;
        match &self.event {
            InputEvent::Click {u,v}=>life::ensure(policy::valid_point(*u,*v),"GUI_INPUT_POINT_INVALID"),
            InputEvent::ReplaceText {text,expected_text_generation}=>life::ensure(self.target_id==Target::PathInput
                && *expected_text_generation!=0 && policy::valid_input_text(text),"GUI_INPUT_TEXT_INVALID"),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureParams {pub expected:Binding,pub snapshot_id:String,pub command_id:String}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {pub expected:Binding,pub command_id:String}
#[derive(Deserialize)]
#[serde(tag="method",deny_unknown_fields)]
pub enum GuiRequest {
    #[serde(rename="diagnostic.gui.snapshot")]
    Snapshot {version:u8,id:String,token:String,params:SnapshotParams},
    #[serde(rename="diagnostic.gui.click")]
    Click {version:u8,id:String,token:String,params:ClickParams},
    #[serde(rename="diagnostic.gui.input")]
    Input {version:u8,id:String,token:String,params:InputParams},
    #[serde(rename="diagnostic.gui.capture")]
    Capture {version:u8,id:String,token:String,params:CaptureParams},
    #[serde(rename="diagnostic.gui.capture_document")]
    DocumentCapture {version:u8,id:String,token:String,params:CaptureParams},
    #[serde(rename="diagnostic.gui.status")]
    Status {version:u8,id:String,token:String,params:StatusParams},
}
impl GuiRequest {
    pub fn validate(&self)->Result<(),String> {
        let (version,wanted,id,token,binding)=match self {
            Self::Snapshot {version,id,token,params}=>(version,1,id,token,&params.expected),
            Self::Click {version,id,token,params}=>{
                life::ensure(life::identifier(&params.snapshot_id) && life::identifier(&params.command_id)
                    && params.target.legacy(),"GUI_LEGACY_CLICK_TARGET_OR_ID_INVALID")?;
                (version,1,id,token,&params.expected)
            }
            Self::Input {version,id,token,params}=>{
                params.validate()?;(version,2,id,token,&params.expected)
            }
            Self::Capture {version,id,token,params}=>{
                life::ensure(life::identifier(&params.snapshot_id) && life::identifier(&params.command_id),"GUI_IDENTIFIER_INVALID")?;
                (version,1,id,token,&params.expected)
            }
            Self::DocumentCapture {version,id,token,params}=>{
                life::ensure(life::identifier(&params.snapshot_id) && life::identifier(&params.command_id),"GUI_IDENTIFIER_INVALID")?;
                (version,2,id,token,&params.expected)
            }
            Self::Status {version,id,token,params}=>{
                life::ensure(life::identifier(&params.command_id),"GUI_IDENTIFIER_INVALID")?;
                (version,1,id,token,&params.expected)
            }
        };
        life::ensure(*version==wanted && life::identifier(id) && token.len()==64
            && token.bytes().all(|b|b.is_ascii_hexdigit()),"GUI_ENVELOPE_INVALID")?;
        binding.validate()?;
        life::ensure(binding.window.index==0 && binding.window.generation==0,"GUI_WINDOW_GENERATION_UNSUPPORTED")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value,json};
    fn fixture()->Value {json!({"version":2,"id":"input-1","token":"a".repeat(64),"method":"diagnostic.gui.input",
        "params":{"expected":{"profile_id":life::PROFILE,"run_id":"run1","session_id":"s1","owner":"o1",
            "pid":1,"start_ticks":1,"pid_namespace":"pid:[1]","elf_sha256":"b".repeat(64),"window":{"index":0,"generation":0}},
        "snapshot_id":"g0000000000000001","geometry_generation":1,"target_id":"path_input","command_id":"text-1",
        "event":{"kind":"replace_text","text":"/var/tmp/owned/input.ai-revit.json","expected_text_generation":1}}})}
    fn accepts(raw:&str)->bool {serde_json::from_str::<GuiRequest>(raw).is_ok_and(|v|v.validate().is_ok())}
    #[test] fn original_byte_duplicate_and_unknown_fields_are_rejected_at_every_new_level() {
        let raw=fixture().to_string();assert!(accepts(&raw));
        for corrupt in [raw.replace("\"geometry_generation\":1","\"geometry_generation\":1,\"geometry_generation\":1"),
            raw.replace("\"kind\":\"replace_text\"","\"kind\":\"replace_text\",\"kind\":\"replace_text\""),
            raw.replace("\"expected_text_generation\":1","\"expected_text_generation\":1,\"unexpected\":0"),
            raw.replace("\"version\":2","\"version\":2,\"version\":2")] {assert!(!accepts(&corrupt));}
    }
    #[test] fn full_u64_generation_is_exact_and_bool_float_or_mismatched_snapshot_refuses() {
        let mut v=fixture();v["params"]["geometry_generation"]=json!(9_007_199_254_740_993u64);
        v["params"]["snapshot_id"]=json!("g0020000000000001");
        v["params"]["event"]["expected_text_generation"]=json!(u64::MAX);
        assert!(accepts(&v.to_string()));
        let GuiRequest::Input {params,..}=serde_json::from_value::<GuiRequest>(v.clone()).unwrap() else {panic!()};
        assert_eq!(params.geometry_generation,9_007_199_254_740_993); 
        for value in [json!(true),json!(1.0),json!(0),json!(9_007_199_254_740_992u64)] {
            let mut corrupt=v.clone();corrupt["params"]["geometry_generation"]=value;assert!(!accepts(&corrupt.to_string()));
        }
    }
    #[test] fn text_cannot_target_button_and_v1_is_not_silently_reinterpreted() {
        let mut v=fixture();v["params"]["target_id"]=json!("save_document");assert!(!accepts(&v.to_string()));
        v=fixture();v["version"]=json!(1);assert!(!accepts(&v.to_string()));
        v=fixture();v["params"]["event"]["text"]=json!("/var/tmp/owned/../other.json");assert!(!accepts(&v.to_string()));
        v=fixture();v["params"]["event"]=json!({"kind":"keydown","key":"A"});assert!(!accepts(&v.to_string()));
    }
}
