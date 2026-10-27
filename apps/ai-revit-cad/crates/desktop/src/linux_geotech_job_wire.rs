//! Closed additions only; legacy GuiRequest and eight-target Target stay exact.
use crate::linux_lifecycle_wire::{self as life,Binding};
use serde::Deserialize;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Deserialize)]
pub enum Target {#[serde(rename="gcc_analyze")] Analyze,#[serde(rename="gcc_report")] Report,#[serde(rename="gcc_snapshot_save")] Calculation}
impl Target {
    pub fn name(self)->&'static str{match self{Self::Analyze=>"gcc_analyze",Self::Report=>"gcc_report",Self::Calculation=>"gcc_snapshot_save"}}
    pub fn prefix(self)->char{match self{Self::Analyze=>'a',Self::Report=>'r',Self::Calculation=>'c'}}
    pub fn registry(self)->&'static str{match self{Self::Analyze=>"source13-analysis-registry-v1",Self::Report=>"source13-report-registry-v1",Self::Calculation=>"source13-calculation-registry-v1"}}
}
pub fn method(method:&str)->Option<(Target,bool)>{Some(match method{
    "diagnostic.gui.analysis_snapshot"=>(Target::Analyze,false),"diagnostic.gui.analysis_input"=>(Target::Analyze,true),
    "diagnostic.gui.report_snapshot"=>(Target::Report,false),"diagnostic.gui.report_input"=>(Target::Report,true),
    "diagnostic.gui.calculation_snapshot"=>(Target::Calculation,false),"diagnostic.gui.calculation_input"=>(Target::Calculation,true),_=>return None})}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]pub struct SnapshotParams {pub expected:Binding}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]pub struct InputParams {
    pub expected:Binding,pub snapshot_id:String,pub geometry_generation:u64,pub target_id:Target,pub command_id:String,pub event:Click,
}
#[derive(Clone,Copy,Deserialize)]#[serde(tag="kind",deny_unknown_fields)]pub enum Click {#[serde(rename="click")] At {u:f64,v:f64}}
impl InputParams {pub fn validate(&self,target:Target)->Result<(),String>{
    life::ensure(self.target_id==target&&self.geometry_generation>0&&self.snapshot_id==format!("{}{:016x}",target.prefix(),self.geometry_generation)
        &&life::identifier(&self.command_id),"JOB_TARGET_OR_GENERATION")?;
    let Click::At{u,v}=self.event;life::ensure(u.is_finite()&&v.is_finite()&&u>0.0&&u<1.0&&v>0.0&&v<1.0,"JOB_POINT")
}}
#[derive(Deserialize)]#[serde(tag="method",deny_unknown_fields)]pub enum Request {
    #[serde(rename="diagnostic.gui.analysis_snapshot")] AnalyzeSnapshot {version:u8,id:String,token:String,params:SnapshotParams},
    #[serde(rename="diagnostic.gui.analysis_input")] AnalyzeInput {version:u8,id:String,token:String,params:InputParams},
    #[serde(rename="diagnostic.gui.report_snapshot")] ReportSnapshot {version:u8,id:String,token:String,params:SnapshotParams},
    #[serde(rename="diagnostic.gui.report_input")] ReportInput {version:u8,id:String,token:String,params:InputParams},
    #[serde(rename="diagnostic.gui.calculation_snapshot")] CalculationSnapshot {version:u8,id:String,token:String,params:SnapshotParams},
    #[serde(rename="diagnostic.gui.calculation_input")] CalculationInput {version:u8,id:String,token:String,params:InputParams},
}
impl Request {pub fn validate(&self)->Result<(),String>{
    let(version,id,token)=match self{Self::AnalyzeSnapshot{version,id,token,..}|Self::AnalyzeInput{version,id,token,..}
        |Self::ReportSnapshot{version,id,token,..}|Self::ReportInput{version,id,token,..}|Self::CalculationSnapshot{version,id,token,..}
        |Self::CalculationInput{version,id,token,..}=>(version,id,token)};
    life::ensure(*version==1&&life::identifier(id)&&token.len()==64&&token.bytes().all(|b|b.is_ascii_hexdigit()),"JOB_ENVELOPE")?;
    let expected=match self{Self::AnalyzeSnapshot{params,..}|Self::ReportSnapshot{params,..}|Self::CalculationSnapshot{params,..}=>&params.expected,
        Self::AnalyzeInput{params,..}|Self::ReportInput{params,..}|Self::CalculationInput{params,..}=>&params.expected};expected.validate()?;
    match self{Self::AnalyzeInput{params,..}=>params.validate(Target::Analyze),Self::ReportInput{params,..}=>params.validate(Target::Report),
        Self::CalculationInput{params,..}=>params.validate(Target::Calculation),_=>Ok(())}
}}
pub fn original_bytes(bytes:&[u8])->bool{bytes.len()<=4096&&serde_json::from_slice::<Request>(bytes).is_ok_and(|r|r.validate().is_ok())}
#[cfg(test)]mod tests{use super::*;
    #[test]fn registry_is_closed_and_old_methods_not_reinterpreted(){
        for name in ["diagnostic.gui.input","diagnostic.gui.click","diagnostic.gui.search_input","document.save"]{assert!(method(name).is_none());}
        assert_eq!(method("diagnostic.gui.report_input"),Some((Target::Report,true)));
        assert!(serde_json::from_str::<Target>("\"gcc_search\"").is_err());
    }
    fn valid()->String{serde_json::json!({"method":"diagnostic.gui.analysis_input","version":1,"id":"request01","token":"a".repeat(64),"params":{
        "expected":{"profile_id":life::PROFILE,"run_id":"run01","session_id":"session01","owner":"owner01","pid":1,"start_ticks":1,
            "pid_namespace":"pid:[1]","elf_sha256":"b".repeat(64),"window":{"index":0,"generation":0}},
        "snapshot_id":"a0020000000000001","geometry_generation":9007199254740993u64,"target_id":"gcc_analyze","command_id":"command01",
        "event":{"kind":"click","u":0.5,"v":0.5}}}).to_string()}
    #[test]fn valid_baseline_then_duplicates_unknown_and_method_target_mismatch_refuse(){
        let raw=valid();assert!(original_bytes(raw.as_bytes()));
        for changed in [raw.replace("\"version\":1","\"version\":1,\"version\":2"),raw.replace("\"u\":0.5","\"u\":0.5,\"u\":0.6"),
            raw.replace("\"pid\":1","\"pid\":1,\"pid\":2"),raw.replace("gcc_analyze","gcc_report"),
            raw.replace("\"version\":1","\"version\":2"),raw.replace("\"kind\":\"click\"","\"kind\":\"click\",\"path\":\"/tmp/x\"")]{
                assert_ne!(changed,raw);assert!(!original_bytes(changed.as_bytes()));}
        let request:Request=serde_json::from_str(&raw).unwrap();let Request::AnalyzeInput{params,..}=request else{panic!()};
        assert_eq!(params.geometry_generation,9007199254740993);assert_eq!(params.snapshot_id,"a0020000000000001");
    }
}
