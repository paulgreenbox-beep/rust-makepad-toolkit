//! Authoritative engineering cases share the BIM/Drafting transaction and history.
//! Derived calculations remain pure; only an explicit snapshot command persists them.
use crate::*;
use geotech_core::{SlopeAnalysisInput, SlopeAnalysisResult};
pub use geotech_core::circular::{GroundDefinition, GroundReadinessReport, GroundReadinessState};
use geotech_core::circular::{self as circular, CircularAnalysisConfig, CircularAnalysisInput,
    CircularSlipSurface, AnalysisBasis};
use sha2::{Digest, Sha256};

macro_rules! engineering_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(#[serde(with = "crate::drafting::decimal_id")] u64);
        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
engineering_id!(AnalysisCaseId);
engineering_id!(GeotechModelId);
engineering_id!(SlopeSectionId);
engineering_id!(SoilMaterialId);
engineering_id!(SlipSurfaceId);
engineering_id!(GroundSectionId);

/// New source stamps use canonical decimal strings without changing the
/// historical OperationId codec used by legacy BIM and planar archives.
pub(crate) mod operation_source_id {
    use super::OperationId;
    use serde::{Deserializer,Serializer};
    pub fn serialize<S:Serializer>(id:&OperationId,serializer:S)->Result<S::Ok,S::Error>{
        crate::drafting::decimal_id::serialize(&id.get(),serializer)
    }
    pub fn deserialize<'de,D:Deserializer<'de>>(deserializer:D)->Result<OperationId,D::Error>{
        crate::drafting::decimal_id::deserialize(deserializer).map(OperationId)
    }
}

fn circular_document_error(error: circular::CircularError) -> CoreError {
    CoreError::InvalidDocument(format!("Circular engineering {:?}: {}",error.code,error.message))
}

/// A durable geology owner. Empty source collections form an explicit draft;
/// neither a circle nor a material is synthesized to make it look ready.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundSection {
    pub ground_section_id: GroundSectionId,
    pub name: String,
    pub definition: GroundDefinition,
    #[serde(with="operation_source_id")]
    pub source_operation_id: OperationId,
}
impl GroundSection {
    fn validate_local(&self)->Result<(),CoreError>{
        validate_name(&self.name,"ground section")?;
        if self.ground_section_id.get()==0||self.source_operation_id.get()<self.ground_section_id.get() {
            return Err(CoreError::InvalidDocument("Ground section has a zero owner or pre-creation source identity".into()));
        }
        circular::validate_ground_structure(&self.definition).map_err(circular_document_error)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularCaseDefinition {
    pub ground_section_id: GroundSectionId,
    pub slip_surface: CircularSlipSurface,
    pub basis: AnalysisBasis,
    pub units: geotech_core::InputUnits,
    pub extrusion_width_m: f64,
    pub config: CircularAnalysisConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_spec: Option<CircularSearchSpecV1>,
}
impl CircularCaseDefinition {
    pub(crate) fn validate_local(&self)->Result<(),CoreError>{
        let circle=&self.slip_surface;
        if self.ground_section_id.get()==0||circle.slip_id.0.trim().is_empty()||circle.slip_id.0.len()>256
            ||!circle.center.x.is_finite()||!circle.center.z.is_finite()
            ||!circle.radius_m.is_finite()||circle.radius_m<=0.0
            ||!self.extrusion_width_m.is_finite()||self.extrusion_width_m<=0.0
            ||self.units!=geotech_core::InputUnits::default(){
            return Err(CoreError::InvalidDocument("Circular case has invalid local identity, circle, units or width".into()));
        }
        if let Some(points)=circle.selected_endpoints {
            if [points.entry_point,points.exit_point].iter().any(|p|!p.x.is_finite()||!p.z.is_finite()){
                return Err(CoreError::InvalidDocument("Circular selected endpoint is nonfinite".into()));
            }
        }
        circular::validate_config(&self.config).map_err(circular_document_error)?;
        if let Some(spec) = &self.search_spec {
            spec.validate_for_config(&self.config)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularAnalysisCase {
    pub analysis_id: AnalysisCaseId,
    pub name: String,
    pub definition: CircularCaseDefinition,
    #[serde(with="operation_source_id")]
    pub source_operation_id: OperationId,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub circular_snapshot:Option<CircularCalculationSnapshot>,
}
impl CircularAnalysisCase {
    fn validate_local(&self)->Result<(),CoreError>{
        self.validate_source_local()?;
        if let Some(snapshot)=&self.circular_snapshot{snapshot.validate_integrity()?;}
        Ok(())
    }
    fn validate_source_local(&self)->Result<(),CoreError>{
        validate_name(&self.name,"circular analysis")?;
        if self.analysis_id.get()==0||self.source_operation_id.get()<self.analysis_id.get(){
            return Err(CoreError::InvalidDocument("Circular case has a zero owner or pre-creation source identity".into()));
        }
        self.definition.validate_local()
    }
}

/// Immutable reproduction input. No calculation or snapshot is created here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCircularAnalysis {
    pub input: CircularAnalysisInput,
    pub config: CircularAnalysisConfig,
    #[serde(with="operation_source_id")]
    pub ground_source_operation_id: OperationId,
    #[serde(with="operation_source_id")]
    pub case_source_operation_id: OperationId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalculationSnapshot {
    pub schema_version: u16,
    pub input: SlopeAnalysisInput,
    pub result: SlopeAnalysisResult,
    pub report: String,
    pub input_fingerprint: String,
    pub source_fingerprint: String,
    pub solver_version: String,
    pub content_fingerprint: String,
}
impl CalculationSnapshot {
    fn create(input: &SlopeAnalysisInput) -> Result<Self, CoreError> {
        let result = geotech_core::analyze(input)?;
        let report = geotech_core::report(input, &result, None)?;
        let mut value = Self {
            schema_version: 1,
            input: input.clone(),
            input_fingerprint: result.input_fingerprint.clone(),
            source_fingerprint: result.source_fingerprint.clone(),
            solver_version: result.solver_version.clone(),
            result,
            report,
            content_fingerprint: String::new(),
        };
        value.content_fingerprint = value.digest()?;
        Ok(value)
    }
    fn digest(&self) -> Result<String, CoreError> {
        let bytes = serde_json::to_vec(&(
            self.schema_version,
            &self.input,
            &self.result,
            &self.report,
            &self.input_fingerprint,
            &self.source_fingerprint,
            &self.solver_version,
        ))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
    fn validate(&self, input: &SlopeAnalysisInput) -> Result<(), CoreError> {
        if self.schema_version != 1
            || &self.input != input
            || self.input_fingerprint != geotech_core::input_fingerprint(input)?
            || self.result.input_fingerprint != self.input_fingerprint
            || self.result.source_fingerprint != self.source_fingerprint
            || self.result.solver_version != self.solver_version
            || self.content_fingerprint != self.digest()?
            || self.result.status.numerically_verified
            || !self.result.status.engineering_review_required
        {
            return Err(CoreError::InvalidDocument(
                "Engineering calculation snapshot is corrupt, stale or claims unsupported approval"
                    .into(),
            ));
        }
        // Archived, fingerprinted output is preserved without rerunning the solver on
        // open, history movement, inspection, or a later input mutation.
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlopeAnalysisCase {
    pub analysis_id: AnalysisCaseId,
    pub model_id: GeotechModelId,
    pub section_id: SlopeSectionId,
    pub soil_material_id: SoilMaterialId,
    pub slip_surface_id: SlipSurfaceId,
    pub input: SlopeAnalysisInput,
    pub name: String,
    pub calculation_snapshot: Option<CalculationSnapshot>,
}
impl SlopeAnalysisCase {
    fn validate(&self) -> Result<(), CoreError> {
        validate_name(&self.name, "slope analysis")?;
        let id = self.analysis_id.get();
        if id == 0
            || [
                self.model_id.get(),
                self.section_id.get(),
                self.soil_material_id.get(),
                self.slip_surface_id.get(),
            ]
            .iter()
            .any(|value| *value != id)
        {
            return Err(CoreError::InvalidDocument(
                "Engineering aggregate has missing or cross-linked typed source identities".into(),
            ));
        }
        geotech_core::validate_input(&self.input)?;
        if let Some(s) = &self.calculation_snapshot {
            s.validate(&self.input)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineeringDomain {
    #[serde(default, deserialize_with = "crate::drafting::unique_map")]
    pub(super) slope_cases: BTreeMap<AnalysisCaseId, SlopeAnalysisCase>,
    #[serde(default,skip_serializing_if="BTreeMap::is_empty",deserialize_with="crate::drafting::unique_map")]
    pub(super) ground_sections: BTreeMap<GroundSectionId,GroundSection>,
    #[serde(default,skip_serializing_if="BTreeMap::is_empty",deserialize_with="crate::drafting::unique_map")]
    pub(super) circular_cases: BTreeMap<AnalysisCaseId,CircularAnalysisCase>,
}
impl EngineeringDomain {
    pub fn is_empty(&self) -> bool {
        self.slope_cases.is_empty()&&self.ground_sections.is_empty()&&self.circular_cases.is_empty()
    }
    pub fn model(&self, id: GeotechModelId) -> Option<&SlopeAnalysisCase> {
        self.slope_cases.values().find(|c| c.model_id == id)
    }
    pub fn section(&self, id: SlopeSectionId) -> Option<&geotech_core::Section2D> {
        self.slope_cases
            .values()
            .find(|c| c.section_id == id)
            .map(|c| &c.input.section)
    }
    pub fn soil_material(&self, id: SoilMaterialId) -> Option<&geotech_core::SoilMaterial> {
        self.slope_cases
            .values()
            .find(|c| c.soil_material_id == id)
            .map(|c| &c.input.soil)
    }
    pub fn slip_surface(&self, id: SlipSurfaceId) -> Option<&geotech_core::PlanarSlipSurface> {
        self.slope_cases
            .values()
            .find(|c| c.slip_surface_id == id)
            .map(|c| &c.input.section.slip_surface)
    }
    pub fn ground_section(&self,id:GroundSectionId)->Option<&GroundSection>{self.ground_sections.get(&id)}
    pub fn circular_case(&self,id:AnalysisCaseId)->Option<&CircularAnalysisCase>{self.circular_cases.get(&id)}
    pub fn ground_material(&self,id:GroundSectionId,material_id:&circular::SoilMaterialId)->Option<&circular::GroundMaterial>{
        self.ground_sections.get(&id)?.definition.materials.iter().find(|m|&m.material_id==material_id)
    }
    /// Compact projections may omit an unchanged referenced ground.
    pub(super) fn validate_local(&self) -> Result<(), CoreError> {
        for (id, c) in &self.slope_cases {
            if *id != c.analysis_id {
                return Err(CoreError::InvalidDocument(
                    "Engineering case key/identity mismatch".into(),
                ));
            }
            c.validate()?;
        }
        for(id,ground)in &self.ground_sections{
            if *id!=ground.ground_section_id{return Err(CoreError::InvalidDocument("Ground section key/identity mismatch".into()));}
            ground.validate_local()?;
        }
        for(id,case)in &self.circular_cases{
            if *id!=case.analysis_id||self.slope_cases.contains_key(id){return Err(CoreError::InvalidDocument("Circular case key mismatch or planar/circular analysis identity collision".into()));}
            case.validate_local()?;
        }
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<(),CoreError>{
        self.validate_local()?;
        for case in self.circular_cases.values(){
            if !self.ground_sections.contains_key(&case.definition.ground_section_id){return Err(CoreError::InvalidDocument(format!("Circular analysis {} references missing ground {}",case.analysis_id,case.definition.ground_section_id)));}
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineeringCommand {
    CreateGround { definition:GroundDefinition,name:String },
    UpdateGround { ground_section_id:GroundSectionId,definition:GroundDefinition,name:Option<String> },
    CreateCircular { definition:CircularCaseDefinition,name:String },
    UpdateCircular { analysis_id:AnalysisCaseId,definition:CircularCaseDefinition,name:Option<String> },
    SaveCircularSnapshot { analysis_id:AnalysisCaseId,snapshot:CircularCalculationSnapshot },
    CreateSlope {
        input: SlopeAnalysisInput,
        name: String,
    },
    UpdateSlope {
        analysis_id: AnalysisCaseId,
        input: SlopeAnalysisInput,
        name: Option<String>,
    },
    SaveCalculationSnapshot {
        analysis_id: AnalysisCaseId,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EngineeringDelta {
    #[serde(default)]
    pub affected_cases: Vec<AnalysisCaseId>,
    pub created_cases: Vec<AnalysisCaseId>,
    pub updated_cases: Vec<AnalysisCaseId>,
    pub deleted_cases: Vec<AnalysisCaseId>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub affected_ground_sections:Vec<GroundSectionId>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub created_ground_sections:Vec<GroundSectionId>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub updated_ground_sections:Vec<GroundSectionId>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub deleted_ground_sections:Vec<GroundSectionId>,
}

pub(super) fn summarize_engineering(changes:&[Change],post_domain:&EngineeringDomain,
    post_dependencies:&BTreeMap<GroundSectionId,BTreeSet<AnalysisCaseId>>)->EngineeringDelta{
    let mut planar=BTreeMap::new();let mut circular=BTreeMap::new();let mut grounds=BTreeMap::new();
    for change in changes{match change{
        Change::EngineeringCase{id,before,after}=>{planar.entry(*id).or_insert((before.as_ref(),after.as_ref())).1=after.as_ref();},
        Change::CircularEngineeringCase{id,before,after}=>{circular.entry(*id).or_insert((before.as_ref(),after.as_ref())).1=after.as_ref();},
        Change::EngineeringGroundSection{id,before,after}=>{grounds.entry(*id).or_insert((before.as_ref(),after.as_ref())).1=after.as_ref();},_=>()
    }}
    let mut out=EngineeringDelta::default();let mut affected=BTreeSet::new();
    for(id,(a,b))in planar{
        affected.insert(id);match(a,b){(None,Some(_))=>out.created_cases.push(id),(Some(_),None)=>out.deleted_cases.push(id),
            (Some(a),Some(b))if a!=b=>out.updated_cases.push(id),_=>()}
    }
    for(id,(a,b))in &circular{
        affected.insert(*id);match(a,b){(None,Some(_))=>out.created_cases.push(*id),(Some(_),None)=>out.deleted_cases.push(*id),
            (Some(a),Some(b))if a!=b=>out.updated_cases.push(*id),_=>()}
    }
    let mut source_changed=BTreeSet::new();
    for(id,(a,b))in grounds{
        out.affected_ground_sections.push(id);
        match(a,b){(None,Some(_))=>{out.created_ground_sections.push(id);source_changed.insert(id);},
            (Some(_),None)=>{out.deleted_ground_sections.push(id);source_changed.insert(id);},
            (Some(a),Some(b))=>{if a!=b{out.updated_ground_sections.push(id);}
                if a.source_operation_id!=b.source_operation_id||a.definition!=b.definition{source_changed.insert(id);}},_=>()}
    }
    for ground_id in &source_changed{
        if let Some(cases)=post_dependencies.get(ground_id){for id in cases{
            if post_domain.circular_cases.get(id).is_some_and(|c|c.definition.ground_section_id==*ground_id){affected.insert(*id);}
        }}
    }
    // Preserve before-state dependents removed/rebound within this same batch.
    for(id,(before,after))in circular{
        if before.into_iter().chain(after).any(|c|source_changed.contains(&c.definition.ground_section_id)){affected.insert(id);}
    }
    out.affected_cases=affected.into_iter().collect();
    out.created_cases.sort();out.created_cases.dedup();out.updated_cases.sort();out.updated_cases.dedup();out.deleted_cases.sort();out.deleted_cases.dedup();
    out
}
pub(super) fn engineering_projection(
    domain: &EngineeringDomain,
    changes: Option<&[Change]>,
) -> Option<EngineeringDomain> {
    if let Some(changes) = changes {
        let mut out = EngineeringDomain::default();
        let mut changed = false;
        for c in changes {
            match c{
                Change::EngineeringCase{id,..}=>{changed=true;if let Some(case)=domain.slope_cases.get(id){out.slope_cases.insert(*id,case.clone());}},
                Change::EngineeringGroundSection{id,..}=>{changed=true;if let Some(ground)=domain.ground_sections.get(id){out.ground_sections.insert(*id,ground.clone());}},
                Change::CircularEngineeringCase{id,..}=>{changed=true;if let Some(case)=domain.circular_cases.get(id){out.circular_cases.insert(*id,case.clone());}},_=>()
            }
        }
        changed.then_some(out)
    } else {
        (!domain.is_empty()).then(|| domain.clone())
    }
}
pub(super) fn engineering_command_effects_match(
    command: &EngineeringCommand,
    changes: &[Change],
    operation_id:OperationId,
) -> bool {
    if changes.len()!=1{return false;}
    match(command,&changes[0]){
        (EngineeringCommand::CreateGround{definition,name},Change::EngineeringGroundSection{id,before:None,after:Some(b)})=>{
            return id.get()==operation_id.get()&&b.ground_section_id==*id&&b.source_operation_id==operation_id&&&b.definition==definition&&&b.name==name;
        }
        (EngineeringCommand::UpdateGround{ground_section_id,definition,name},Change::EngineeringGroundSection{id,before:Some(a),after:Some(b)})=>{
            return ground_section_id==id&&a.ground_section_id==*id&&b.ground_section_id==*id&&&b.definition==definition
                &&b.name==name.as_deref().unwrap_or(&a.name)
                &&b.source_operation_id==if a.definition==b.definition{a.source_operation_id}else{operation_id};
        }
        (EngineeringCommand::CreateCircular{definition,name},Change::CircularEngineeringCase{id,before:None,after:Some(b)})=>{
            return id.get()==operation_id.get()&&b.analysis_id==*id&&b.source_operation_id==operation_id&&&b.definition==definition&&&b.name==name&&b.circular_snapshot.is_none();
        }
        (EngineeringCommand::UpdateCircular{analysis_id,definition,name},Change::CircularEngineeringCase{id,before:Some(a),after:Some(b)})=>{
            return analysis_id==id&&a.analysis_id==*id&&b.analysis_id==*id&&&b.definition==definition
                &&b.name==name.as_deref().unwrap_or(&a.name)
                &&b.source_operation_id==if a.definition==b.definition{a.source_operation_id}else{operation_id}
                &&b.circular_snapshot==a.circular_snapshot;
        }
        (EngineeringCommand::SaveCircularSnapshot{analysis_id,snapshot},Change::CircularEngineeringCase{id,before:Some(a),after:Some(b)})=>{
            return analysis_id==id&&a.analysis_id==*id&&b.analysis_id==*id
                &&a.definition==b.definition&&a.name==b.name&&a.source_operation_id==b.source_operation_id
                &&snapshot.source_case_definition==a.definition
                &&snapshot.archive.bundle.context.case_source_operation.0==a.source_operation_id.get()
                &&snapshot.validate_integrity().is_ok()
                &&b.circular_snapshot.as_ref().is_some_and(|saved|saved==snapshot);
        }
        (EngineeringCommand::CreateGround{..}|EngineeringCommand::UpdateGround{..}
            |EngineeringCommand::CreateCircular{..}|EngineeringCommand::UpdateCircular{..}
            |EngineeringCommand::SaveCircularSnapshot{..},_)=>return false,
        _=>()
    }
    let records: Vec<_> = changes
        .iter()
        .filter_map(|c| {
            if let Change::EngineeringCase { id, before, after } = c {
                Some((id, before, after))
            } else {
                None
            }
        })
        .collect();
    if records.len() != 1 {
        return false;
    }
    let (id, before, after) = records[0];
    match (command, before, after) {
        (EngineeringCommand::CreateSlope { input, name }, None, Some(c)) => {
            &c.input == input
                && &c.name == name
                && c.analysis_id == *id
                && c.calculation_snapshot.is_none()
        }
        (
            EngineeringCommand::UpdateSlope {
                analysis_id,
                input,
                name,
            },
            Some(a),
            Some(b),
        ) => {
            analysis_id == id
                && &b.input == input
                && b.name == name.as_deref().unwrap_or(&a.name)
                && b.calculation_snapshot.is_none()
                && a.analysis_id == b.analysis_id
                && a.model_id == b.model_id
                && a.section_id == b.section_id
                && a.soil_material_id == b.soil_material_id
                && a.slip_surface_id == b.slip_surface_id
        }
        (EngineeringCommand::SaveCalculationSnapshot { analysis_id }, Some(a), Some(b)) => {
            analysis_id == id
                && a.input == b.input
                && a.name == b.name
                && a.analysis_id == b.analysis_id
                && a.model_id == b.model_id
                && a.section_id == b.section_id
                && a.soil_material_id == b.soil_material_id
                && a.slip_surface_id == b.slip_surface_id
                && b.calculation_snapshot
                    .as_ref()
                    .is_some_and(|s| s.validate(&b.input).is_ok())
        }
        _ => false,
    }
}
impl Document {
    pub fn engineering(&self) -> &EngineeringDomain {
        &self.engineering
    }
    pub fn ground_sections(&self)->&BTreeMap<GroundSectionId,GroundSection>{&self.engineering.ground_sections}
    pub fn ground_section(&self,id:GroundSectionId)->Option<&GroundSection>{self.engineering.ground_sections.get(&id)}
    pub fn circular_cases(&self)->&BTreeMap<AnalysisCaseId,CircularAnalysisCase>{&self.engineering.circular_cases}
    pub fn circular_case(&self,id:AnalysisCaseId)->Option<&CircularAnalysisCase>{self.engineering.circular_cases.get(&id)}
    pub fn ground_material(&self,id:GroundSectionId,material_id:&circular::SoilMaterialId)->Option<&circular::GroundMaterial>{
        self.engineering.ground_material(id,material_id)
    }
    pub fn ground_readiness(&self,id:GroundSectionId)->Result<GroundReadinessReport,CoreError>{
        let ground=self.ground_section(id).ok_or_else(||CoreError::InvalidDocument(format!("Unknown ground section {id}")))?;
        circular::inspect_ground_readiness(&ground.definition).map_err(circular_document_error)
    }
    pub fn resolve_circular_input(&self,id:AnalysisCaseId)->Result<ResolvedCircularAnalysis,CoreError>{
        let case=self.circular_case(id).ok_or_else(||CoreError::InvalidDocument(format!("Unknown circular analysis {id}")))?;
        case.validate_local()?;
        self.resolve_circular_input_after_archive_validation(id)
    }
    /// Read-only source projection for an editor preparing archive inspection.
    /// Checks the actual source owners and readiness, without hashing or
    /// formatting retained reports. It grants no archive, job or save authority;
    /// the captured snapshot must pass its separate worker integrity check.
    pub fn resolve_circular_editor_input(&self,id:AnalysisCaseId)->Result<ResolvedCircularAnalysis,CoreError>{
        self.verify_circular_editor_sources(id)?;
        self.resolve_circular_input_after_archive_validation(id)
    }
    pub(crate) fn verify_circular_editor_sources(&self,id:AnalysisCaseId)->Result<(),CoreError>{
        let case=self.circular_case(id).ok_or_else(||CoreError::InvalidDocument(format!("Unknown circular analysis {id}")))?;
        self.verify_circular_owner_source(case)?;
        let ground=self.ground_section(case.definition.ground_section_id).ok_or_else(||CoreError::InvalidDocument(format!("Missing ground for circular analysis {id}")))?;
        self.verify_ground_owner(ground)
    }
    // Shared source resolver for an archive whose integrity is checked on the
    // caller's worker. Keep input construction/readiness in its original owner.
    pub(crate) fn resolve_circular_input_after_archive_validation(&self,id:AnalysisCaseId)->Result<ResolvedCircularAnalysis,CoreError>{
        let case=self.circular_case(id).ok_or_else(||CoreError::InvalidDocument(format!("Unknown circular analysis {id}")))?;
        case.validate_source_local()?;
        let ground=self.ground_section(case.definition.ground_section_id).ok_or_else(||CoreError::InvalidDocument(format!("Circular analysis {id} references missing ground {}",case.definition.ground_section_id)))?;
        if !circular::inspect_ground_readiness(&ground.definition).map_err(circular_document_error)?.is_ready(){
            return Err(CoreError::InvalidDocument(format!("GROUND_NOT_READY: ground {} is an explicit incomplete draft",ground.ground_section_id)));
        }
        let d=&ground.definition;
        let input=CircularAnalysisInput{analysis_id:id.to_string(),
            source_revision:format!("ground-operation:{}/case-operation:{}",ground.source_operation_id,case.source_operation_id),
            ground:circular::GroundSection2D{
                section_id:circular::GroundSectionId(ground.ground_section_id.to_string()),
                coordinate_system:d.coordinate_system,terrain_polyline:d.terrain_polyline.clone(),
                layer_interfaces:d.layer_interfaces.clone(),soil_regions:d.soil_regions.clone(),
                bottom_boundary:d.bottom_boundary.clone(),materials:d.materials.clone(),
                model_revision:ground.source_operation_id.get()},
            slip_surface:case.definition.slip_surface.clone(),basis:case.definition.basis,units:case.definition.units,
            extrusion_width_m:case.definition.extrusion_width_m};
        Ok(ResolvedCircularAnalysis{input,config:case.definition.config,
            ground_source_operation_id:ground.source_operation_id,case_source_operation_id:case.source_operation_id})
    }
    pub fn slope_cases(&self) -> &BTreeMap<AnalysisCaseId, SlopeAnalysisCase> {
        &self.engineering.slope_cases
    }
    pub fn slope_case(&self, id: AnalysisCaseId) -> Option<&SlopeAnalysisCase> {
        self.engineering.slope_cases.get(&id)
    }
    pub fn analyze_slope(&self, id: AnalysisCaseId) -> Result<SlopeAnalysisResult, CoreError> {
        let case = self
            .slope_case(id)
            .ok_or_else(|| CoreError::InvalidDocument(format!("Unknown slope analysis {id}")))?;
        Ok(geotech_core::analyze(&case.input)?)
    }
    pub fn slope_report(&self, id: AnalysisCaseId) -> Result<String, CoreError> {
        let case = self
            .slope_case(id)
            .ok_or_else(|| CoreError::InvalidDocument(format!("Unknown slope analysis {id}")))?;
        let result = geotech_core::analyze(&case.input)?;
        Ok(geotech_core::report(&case.input, &result, None)?)
    }
    pub(super) fn apply_engineering_command(
        &mut self,
        command: EngineeringCommand,
        operation_id: OperationId,
    ) -> Result<bim::Applied, CoreError> {
        match &command{
            EngineeringCommand::CreateGround{..}|EngineeringCommand::UpdateGround{..}=>return self.apply_ground_command(command,operation_id),
            EngineeringCommand::CreateCircular{..}|EngineeringCommand::UpdateCircular{..}
                |EngineeringCommand::SaveCircularSnapshot{..}=>return self.apply_circular_command(command,operation_id),_=>()
        }
        let original = command.clone();
        let (id, before, after) = match command {
            EngineeringCommand::CreateSlope { input, name } => {
                let raw = operation_id.get();
                let id = AnalysisCaseId::new(raw);
                if self.slope_case(id).is_some() || self.circular_case(id).is_some() {
                    return Err(CoreError::InvalidDocument(
                        "Engineering allocation reused an existing case identity".into(),
                    ));
                }
                let c = SlopeAnalysisCase {
                    analysis_id: id,
                    model_id: GeotechModelId::new(raw),
                    section_id: SlopeSectionId::new(raw),
                    soil_material_id: SoilMaterialId::new(raw),
                    slip_surface_id: SlipSurfaceId::new(raw),
                    input,
                    name,
                    calculation_snapshot: None,
                };
                c.validate()?;
                (id, None, c)
            }
            EngineeringCommand::UpdateSlope {
                analysis_id,
                input,
                name,
            } => {
                let before = self.slope_case(analysis_id).cloned().ok_or_else(|| {
                    CoreError::InvalidDocument(format!("Unknown slope analysis {analysis_id}"))
                })?;
                let mut after = before.clone();
                after.input = input;
                if let Some(name) = name {
                    after.name = name;
                }
                after.calculation_snapshot = None;
                after.validate()?;
                (analysis_id, Some(before), after)
            }
            EngineeringCommand::SaveCalculationSnapshot { analysis_id } => {
                let before = self.slope_case(analysis_id).cloned().ok_or_else(|| {
                    CoreError::InvalidDocument(format!("Unknown slope analysis {analysis_id}"))
                })?;
                let mut after = before.clone();
                after.calculation_snapshot = Some(CalculationSnapshot::create(&after.input)?);
                (analysis_id, Some(before), after)
            }
            _=>return Err(CoreError::InvalidDocument("Unexpected engineering command owner".into())),
        };
        let parent = self
            .indexes()
            .operations_by_analysis
            .get(&id)
            .and_then(|ids| ids.iter().next_back().copied());
        self.engineering.slope_cases.insert(id, after);
        self.format_version = self.format_version.max(3);
        Ok((
            OperationKind::EngineeringChanged {
                analysis_id: id,
                command: original,
            },
            parent,
            vec![],
            vec![],
            format!("Slope stability analysis {id}"),
            vec![Change::EngineeringCase {
                id,
                before,
                after: self.slope_case(id).cloned(),
            }],
            vec![],
        ))
    }
    fn apply_ground_command(&mut self,command:EngineeringCommand,operation_id:OperationId)->Result<bim::Applied,CoreError>{
        let original=command.clone();
        let(id,before,after)=match command{
            EngineeringCommand::CreateGround{definition,name}=>{
                let id=GroundSectionId::new(operation_id.get());
                if self.ground_section(id).is_some(){return Err(CoreError::InvalidDocument("Ground allocation reused an existing identity".into()));}
                let after=GroundSection{ground_section_id:id,name,definition,source_operation_id:operation_id};
                after.validate_local()?;(id,None,after)
            }
            EngineeringCommand::UpdateGround{ground_section_id,definition,name}=>{
                let before=self.ground_section(ground_section_id).cloned().ok_or_else(||CoreError::InvalidDocument(format!("Unknown ground section {ground_section_id}")))?;
                let source_operation_id=if before.definition==definition{before.source_operation_id}else{operation_id};
                let after=GroundSection{ground_section_id,name:name.unwrap_or_else(||before.name.clone()),definition,source_operation_id};
                after.validate_local()?;(ground_section_id,Some(before),after)
            }
            _=>return Err(CoreError::InvalidDocument("Ground command owner mismatch".into()))
        };
        let parent=self.indexes().operations_by_ground_section.get(&id).and_then(|ids|ids.iter().next_back().copied());
        // All validation is complete before this first source mutation.
        self.engineering.ground_sections.insert(id,after.clone());self.format_version=self.format_version.max(4);
        Ok((OperationKind::EngineeringGroundChanged{ground_section_id:id,command:original},parent,vec![],vec![],
            format!("Ground section {id}"),vec![Change::EngineeringGroundSection{id,before,after:Some(after)}],vec![]))
    }
    fn apply_circular_command(&mut self,command:EngineeringCommand,operation_id:OperationId)->Result<bim::Applied,CoreError>{
        let original=command.clone();
        let(id,before,after)=match command{
            EngineeringCommand::CreateCircular{definition,name}=>{
                let id=AnalysisCaseId::new(operation_id.get());
                if self.slope_case(id).is_some()||self.circular_case(id).is_some(){return Err(CoreError::InvalidDocument("Circular allocation reused an existing analysis identity".into()));}
                let after=CircularAnalysisCase{analysis_id:id,name,definition,source_operation_id:operation_id,circular_snapshot:None};
                after.validate_local()?;(id,None,after)
            }
            EngineeringCommand::UpdateCircular{analysis_id,definition,name}=>{
                let before=self.circular_case(analysis_id).cloned().ok_or_else(||CoreError::InvalidDocument(format!("Unknown circular analysis {analysis_id}")))?;
                let source_operation_id=if before.definition==definition{before.source_operation_id}else{operation_id};
                let mut after=before.clone();
                after.name=name.unwrap_or_else(||before.name.clone());
                after.definition=definition;
                after.source_operation_id=source_operation_id;
                after.validate_local()?;(analysis_id,Some(before),after)
            }
            EngineeringCommand::SaveCircularSnapshot{analysis_id,snapshot}=>{
                let before=self.circular_case(analysis_id).cloned().ok_or_else(||CoreError::InvalidDocument(format!("Unknown circular analysis {analysis_id}")))?;
                snapshot.validate_for_current_document(self,analysis_id)?;
                if snapshot.source_case_definition!=before.definition
                    ||snapshot.archive.bundle.context.case_source_operation.0!=before.source_operation_id.get(){
                    return Err(CoreError::InvalidDocument("Circular calculation snapshot is not bound to the current case source".into()));
                }
                let mut after=before.clone();
                after.circular_snapshot=Some(snapshot);
                after.validate_local()?;(analysis_id,Some(before),after)
            }
            _=>return Err(CoreError::InvalidDocument("Circular command owner mismatch".into()))
        };
        if self.ground_section(after.definition.ground_section_id).is_none(){return Err(CoreError::InvalidDocument(format!("Circular analysis {id} references missing ground {}",after.definition.ground_section_id)));}
        let parent=self.indexes().operations_by_analysis.get(&id).and_then(|ids|ids.iter().next_back().copied());
        self.engineering.circular_cases.insert(id,after.clone());self.format_version=self.format_version.max(4);
        Ok((OperationKind::EngineeringChanged{analysis_id:id,command:original},parent,vec![],vec![],
            format!("Circular stability analysis {id}"),vec![Change::CircularEngineeringCase{id,before,after:Some(after)}],vec![]))
    }
    pub(super) fn has_engineering_history(&self) -> bool {
        !self.engineering.is_empty()
            || self
                .operations
                .values()
                .any(|o| matches!(o.kind, OperationKind::EngineeringChanged { .. }|OperationKind::EngineeringGroundChanged{..}))
            || self.revision_states.values().any(|s| {
                s.engineering.is_some()
                    || s.delta.as_ref().is_some_and(|d| {
                        d.changes
                            .iter()
                            .any(|c| matches!(c, Change::EngineeringCase { .. }|Change::EngineeringGroundSection{..}|Change::CircularEngineeringCase{..}))
                    })
            })
            || self.command_receipts.values().any(|c| {
                c.recorded_commands.as_ref().is_some_and(|commands| {
                    commands
                        .iter()
                        .any(|c| matches!(c, Command::Engineering(_)))
                })
            })
    }
    pub(super) fn has_circular_engineering_history(&self)->bool{
        let command=|c:&EngineeringCommand|matches!(c,EngineeringCommand::CreateGround{..}|EngineeringCommand::UpdateGround{..}
            |EngineeringCommand::CreateCircular{..}|EngineeringCommand::UpdateCircular{..}
            |EngineeringCommand::SaveCircularSnapshot{..});
        let change=|c:&Change|matches!(c,Change::EngineeringGroundSection{..}|Change::CircularEngineeringCase{..})
            ||matches!(c,Change::Operation{before,after,..}if before.iter().chain(after).any(|o|matches!(&o.kind,
                OperationKind::EngineeringGroundChanged{..})||matches!(&o.kind,OperationKind::EngineeringChanged{command:c,..}if command(c))));
        !self.engineering.ground_sections.is_empty()||!self.engineering.circular_cases.is_empty()
            ||self.operations.values().any(|o|matches!(&o.kind,OperationKind::EngineeringGroundChanged{..})
                ||matches!(&o.kind,OperationKind::EngineeringChanged{command:c,..}if command(c)))
            ||self.revision_states.values().any(|s|s.engineering.as_ref().is_some_and(|e|!e.ground_sections.is_empty()||!e.circular_cases.is_empty())
                ||s.delta.as_ref().is_some_and(|d|d.changes.iter().any(&change)))
            ||self.undo_stack.iter().chain(&self.redo_stack).any(|e|e.changes.iter().any(&change))
            ||self.command_receipts.values().any(|r|r.recorded_commands.as_ref().is_some_and(|commands|
                commands.iter().any(|c|matches!(c,Command::Engineering(c)if command(c)))))
    }
    pub(super) fn refresh_format_version(&mut self) {
        self.format_version = if self.has_geo_foundation_history(){GEO_FOUNDATION_NATIVE_FORMAT_VERSION}else if self.has_configurator_history(){STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION}else if self.has_foundation_structural_history(){FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION}else if self.has_circular_engineering_history(){4}else if self.has_engineering_history() {
            3
        } else {
            NATIVE_FORMAT_VERSION
        };
    }
    pub(super) fn validate_local_engineering(&self, changes: &[Change]) -> Result<(), CoreError> {
        for change in changes {
            validate_engineering_change_records(change)?;
            match change{
                Change::EngineeringCase{id,..}=>{if let Some(c)=self.slope_case(*id){c.validate()?;
                    if self.circular_case(*id).is_some(){return Err(CoreError::InvalidDocument("Planar/circular analysis identity collision".into()));}}},
                Change::EngineeringGroundSection{id,..}=>{
                    if let Some(g)=self.ground_section(*id){g.validate_local()?;self.verify_ground_owner(g)?;}
                    else if self.indexes().circular_cases_by_ground_section.get(id).is_some_and(|ids|!ids.is_empty()){
                        return Err(CoreError::InvalidDocument(format!("Removed ground {id} has surviving circular references")));}
                }
                Change::CircularEngineeringCase{id,..}=>{if let Some(c)=self.circular_case(*id){c.validate_local()?;
                    if self.slope_case(*id).is_some()||self.ground_section(c.definition.ground_section_id).is_none(){return Err(CoreError::InvalidDocument("Circular case collides with planar identity or references a missing ground".into()));}
                    self.verify_circular_owner(c)?;
                }},_=>()
            }
        }
        Ok(())
    }
    pub(super) fn engineering_diff(
        &self,
        from: u64,
        to: u64,
    ) -> Result<EngineeringDelta, CoreError> {
        let a = self
            .reconstruct_revision_state(from)?
            .engineering
            .unwrap_or_default();
        let b = self
            .reconstruct_revision_state(to)?
            .engineering
            .unwrap_or_default();
        let ids: BTreeSet<_> = a
            .slope_cases
            .keys()
            .chain(b.slope_cases.keys())
            .copied()
            .collect();
        let mut changes: Vec<_> = ids
            .into_iter()
            .filter(|id| a.slope_cases.get(id) != b.slope_cases.get(id))
            .map(|id| Change::EngineeringCase {
                id,
                before: a.slope_cases.get(&id).cloned(),
                after: b.slope_cases.get(&id).cloned(),
            })
            .collect();
        let ground_ids:BTreeSet<_>=a.ground_sections.keys().chain(b.ground_sections.keys()).copied().collect();
        for id in ground_ids{if a.ground_sections.get(&id)!=b.ground_sections.get(&id){changes.push(Change::EngineeringGroundSection{
            id,before:a.ground_sections.get(&id).cloned(),after:b.ground_sections.get(&id).cloned()});}}
        let circular_ids:BTreeSet<_>=a.circular_cases.keys().chain(b.circular_cases.keys()).copied().collect();
        for id in circular_ids{if a.circular_cases.get(&id)!=b.circular_cases.get(&id){changes.push(Change::CircularEngineeringCase{
            id,before:a.circular_cases.get(&id).cloned(),after:b.circular_cases.get(&id).cloned()});}}
        let mut dependencies:BTreeMap<GroundSectionId,BTreeSet<AnalysisCaseId>>=BTreeMap::new();
        for case in b.circular_cases.values(){dependencies.entry(case.definition.ground_section_id).or_default().insert(case.analysis_id);}
        Ok(summarize_engineering(&changes,&b,&dependencies))
    }
    pub(super) fn verify_engineering_owners(&self) -> Result<(), CoreError> {
        for (id, case) in &self.engineering.slope_cases {
            if !matches!(self.operations.get(&OperationId(id.get())).map(|o|&o.kind),
                Some(OperationKind::EngineeringChanged{analysis_id,command:EngineeringCommand::CreateSlope{..}})if analysis_id==id)
            {
                return Err(CoreError::InvalidDocument(
                    "Engineering case has no matching authoritative creation operation".into(),
                ));
            }
            case.validate()?;
        }
        for ground in self.engineering.ground_sections.values(){ground.validate_local()?;self.verify_ground_owner(ground)?;}
        for case in self.engineering.circular_cases.values(){case.validate_local()?;self.verify_circular_owner(case)?;}
        Ok(())
    }
    pub(crate) fn verify_ground_owner(&self,ground:&GroundSection)->Result<(),CoreError>{
        let id=ground.ground_section_id;
        let creation=self.retained_operation(OperationId(id.get()));
        if !matches!(creation.map(|o|&o.kind),Some(OperationKind::EngineeringGroundChanged{
            ground_section_id,command:EngineeringCommand::CreateGround{..}})if *ground_section_id==id){
            return Err(CoreError::InvalidDocument(format!("Ground {id} has no matching authoritative creation operation")));
        }
        let source=self.retained_operation(ground.source_operation_id);
        let matches=match source.map(|o|&o.kind){
            Some(OperationKind::EngineeringGroundChanged{ground_section_id,command:EngineeringCommand::CreateGround{definition,..}})=>
                *ground_section_id==id&&ground.source_operation_id.get()==id.get()&&definition==&ground.definition,
            Some(OperationKind::EngineeringGroundChanged{ground_section_id,command:EngineeringCommand::UpdateGround{ground_section_id:command_id,definition,..}})=>
                *ground_section_id==id&&*command_id==id&&definition==&ground.definition,_=>false};
        if !matches{return Err(CoreError::InvalidDocument(format!("Ground {id} source stamp has no matching same-owner numerical definition")));}
        Ok(())
    }
    pub(crate) fn verify_circular_owner(&self,case:&CircularAnalysisCase)->Result<(),CoreError>{
        self.verify_circular_owner_source(case)?;
        if let Some(snapshot)=&case.circular_snapshot{snapshot.validate_historical(self,case.analysis_id)?;}
        Ok(())
    }
    // Source-only half used by the archive inspector. Report integrity is
    // verified separately on its owned worker, never in a metadata poll.
    pub(crate) fn verify_circular_owner_source(&self,case:&CircularAnalysisCase)->Result<(),CoreError>{
        let id=case.analysis_id;
        let creation=self.retained_operation(OperationId(id.get()));
        if !matches!(creation.map(|o|&o.kind),Some(OperationKind::EngineeringChanged{
            analysis_id,command:EngineeringCommand::CreateCircular{..}})if *analysis_id==id){
            return Err(CoreError::InvalidDocument(format!("Circular analysis {id} has no matching authoritative creation operation")));
        }
        let source=self.retained_operation(case.source_operation_id);
        let matches=match source.map(|o|&o.kind){
            Some(OperationKind::EngineeringChanged{analysis_id,command:EngineeringCommand::CreateCircular{definition,..}})=>
                *analysis_id==id&&case.source_operation_id.get()==id.get()&&definition==&case.definition,
            Some(OperationKind::EngineeringChanged{analysis_id,command:EngineeringCommand::UpdateCircular{analysis_id:command_id,definition,..}})=>
                *analysis_id==id&&*command_id==id&&definition==&case.definition,_=>false};
        if !matches{return Err(CoreError::InvalidDocument(format!("Circular analysis {id} source stamp has no matching same-owner numerical definition")));}
        Ok(())
    }
}

/// Historical before/after records are checked without consulting today's
/// source maps. Cross-record ownership is checked in a reconstructed full state.
pub(super) fn validate_engineering_change_records(change:&Change)->Result<(),CoreError>{
    match change{
        Change::EngineeringCase{id,before,after}=>for c in before.iter().chain(after){
            if c.analysis_id!=*id{return Err(CoreError::InvalidDocument("Engineering journal key mismatch".into()));}c.validate()?;
        },
        Change::EngineeringGroundSection{id,before,after}=>for g in before.iter().chain(after){
            if g.ground_section_id!=*id{return Err(CoreError::InvalidDocument("Ground journal key mismatch".into()));}g.validate_local()?;
        },
        Change::CircularEngineeringCase{id,before,after}=>for c in before.iter().chain(after){
            if c.analysis_id!=*id{return Err(CoreError::InvalidDocument("Circular journal key mismatch".into()));}c.validate_local()?;
        },_=>()
    }
    Ok(())
}
