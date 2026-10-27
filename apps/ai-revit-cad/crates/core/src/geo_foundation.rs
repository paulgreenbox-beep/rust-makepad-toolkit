//! GEO authority, foundation placement and immutable results in the one Document.
//! Persistence, edits and history validate retained data; they never run a solver.
use crate::*;
use sha2::{Digest, Sha256};

pub const MAX_GEO_FOUNDATION_DOMAIN_BYTES: usize = 4 << 20;
pub const MAX_GEO_FOUNDATION_NATIVE_BYTES: usize = 64 << 20;
pub const MAX_GEO_FOUNDATION_SNAPSHOTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationSourceRevision {
    pub document_uuid: Uuid,
    #[serde(with = "crate::engineering::operation_source_id")]
    pub operation_id: OperationId,
}

/// Links contain identities only. Soil records remain in GeoState.records.soils.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationLayoutInput {
    pub layout: foundation_core::FoundationLayout,
    #[serde(deserialize_with = "crate::drafting::unique_map")]
    pub ground_links: BTreeMap<String, geo_core::GeoSnapshotSelection>,
    /// Explicit authored assumptions and their declared source IDs. An absent
    /// entry is geometry-only; no stiffness source is inferred from GEO soil E.
    #[serde(default, deserialize_with = "crate::drafting::unique_map")]
    pub bedding_assumptions: BTreeMap<String, foundation_core::geo_context::FoundationBeddingAssumption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationLayoutState {
    pub input: GeoFoundationLayoutInput,
    pub source_revision: GeoFoundationSourceRevision,
    pub input_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationAnalysisInput {
    pub foundation_id: String,
    pub placement: foundation_core::FoundationPlacement,
    pub layout_source_revision: GeoFoundationSourceRevision,
    pub placement_fingerprint: String,
    pub load_binding: GeoFoundationLoadBinding,
    pub ground: geo_core::GeoSnapshot,
    pub scenario: geo_core::GeoScenario,
    pub geo_context: foundation_core::FoundationGeoContext,
    pub bedding_assumption: foundation_core::geo_context::FoundationBeddingAssumption,
    /// The existing numerical model has explicit assumed Winkler stiffness.
    /// GEO elastic modulus is never converted into a universal bedding value.
    pub effective_input: foundation_core::RigidMatInput,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationAnalysisSnapshot {
    pub schema_version: u16,
    pub input: GeoFoundationAnalysisInput,
    pub foundation_result: foundation_core::RigidMatResult,
    pub producer: EngineeringSourceIdentity,
    pub engineering_approval: EngineeringApprovalStatus,
    pub check_records: FoundationStructuralCheckRecords,
    pub content_fingerprint: String,
}

impl GeoFoundationAnalysisSnapshot {
    pub fn new(input: GeoFoundationAnalysisInput, foundation_result: foundation_core::RigidMatResult,
        producer: EngineeringSourceIdentity) -> Result<Self, CoreError> {
        let mut snapshot = Self {
            schema_version: 1, input, foundation_result, producer,
            engineering_approval: EngineeringApprovalStatus::NotImplemented,
            check_records: FoundationStructuralCheckRecords::default(),
            content_fingerprint: String::new(),
        };
        snapshot.content_fingerprint = snapshot.expected_fingerprint()?;
        snapshot.validate()?;
        Ok(snapshot)
    }
    fn expected_fingerprint(&self) -> Result<String, CoreError> {
        geo_fingerprint(&(self.schema_version, &self.input, &self.foundation_result,
            &self.producer, self.engineering_approval, &self.check_records))
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        validate_analysis_input(&self.input)?;
        if self.schema_version != 1 || self.engineering_approval != EngineeringApprovalStatus::NotImplemented
            || self.check_records != FoundationStructuralCheckRecords::default()
            || self.content_fingerprint != self.expected_fingerprint()? {
            return Err(geo_error("invalid immutable GEO/foundation snapshot or unsupported approval"));
        }
        validate_producer(&self.producer)?;
        foundation_core::validate_result_binding(&self.input.effective_input, &self.foundation_result)
            .map_err(geo_error)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationDomain {
    geo: geo_core::GeoState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout: Option<GeoFoundationLayoutState>,
    #[serde(default, deserialize_with = "crate::drafting::unique_map")]
    snapshots: BTreeMap<EngineeringSnapshotId, GeoFoundationAnalysisSnapshot>,
}
impl Default for GeoFoundationDomain {
    fn default() -> Self {
        Self { geo: geo_core::GeoState::empty(), layout: None, snapshots: BTreeMap::new() }
    }
}
impl GeoFoundationDomain {
    pub fn is_empty(&self) -> bool { self.geo == geo_core::GeoState::empty() && self.layout.is_none() && self.snapshots.is_empty() }
    pub fn geo(&self) -> &geo_core::GeoState { &self.geo }
    pub fn layout(&self) -> Option<&GeoFoundationLayoutState> { self.layout.as_ref() }
    pub fn snapshots(&self) -> impl DoubleEndedIterator<Item = (EngineeringSnapshotId, &GeoFoundationAnalysisSnapshot)> + ExactSizeIterator {
        self.snapshots.iter().map(|(id, snapshot)| (*id, snapshot))
    }
    pub fn snapshot(&self, id: EngineeringSnapshotId) -> Option<&GeoFoundationAnalysisSnapshot> { self.snapshots.get(&id) }
    /// Ground/placement dependency status only; use the Document wrapper for
    /// overall status including a source-bound structural reaction archive.
    pub fn ground_snapshot_status(&self, id: EngineeringSnapshotId) -> Option<EngineeringSnapshotStatus> {
        self.snapshots.get(&id).map(|snapshot| {
            if self.matches_analysis_dependencies(&snapshot.input).unwrap_or(false) {
                EngineeringSnapshotStatus::Current
            } else { EngineeringSnapshotStatus::Stale }
        })
    }
    fn matches_analysis_dependencies(&self, input: &GeoFoundationAnalysisInput) -> Result<bool, CoreError> {
        let Some(layout) = &self.layout else { return Ok(false); };
        let Some(placement) = layout.input.layout.elements.iter().find(|p| placement_id(p) == input.foundation_id) else { return Ok(false); };
        let Some(selection) = layout.input.ground_links.get(&input.foundation_id) else { return Ok(false); };
        Ok(placement == &input.placement && selection == &input.ground.selection
            && layout.input.bedding_assumptions.get(&input.foundation_id)==Some(&input.bedding_assumption)
            && geo_core::snapshot_currentness(&self.geo, &input.ground).map_err(geo_error)? == geo_core::SnapshotCurrentness::Current)
    }
    pub(super) fn high_water(&self) -> u64 {
        self.geo.head_revision.iter().map(|r| r.operation_id)
            .chain(self.geo.snapshots.values().map(|s| s.source_revision.operation_id))
            .chain(self.layout.iter().map(|s| s.source_revision.operation_id.get()))
            .chain(self.snapshots.iter().flat_map(|(id,s)| [id.get(), s.input.layout_source_revision.operation_id.get(), s.input.ground.source_revision.operation_id]))
            .chain(self.snapshots.values().filter_map(|s|match &s.input.load_binding{GeoFoundationLoadBinding::Configured{snapshot_id,..}=>Some(snapshot_id.get()),GeoFoundationLoadBinding::Manual{..}=>None}))
            .max().unwrap_or(0)
    }
    pub(super) fn validate(&self, uuid: Uuid) -> Result<(), CoreError> {
        geo_core::validate_geo_state(&self.geo).map_err(geo_error)?;
        for source in self.geo.head_revision.iter().chain(self.geo.snapshots.values().map(|s| &s.source_revision)) {
            validate_geo_source(source, uuid)?;
        }
        if let Some(layout) = &self.layout {
            validate_layout_input(&layout.input)?;
            for link in layout.input.ground_links.values(){
                if !self.geo.records.models.contains_key(&link.model_id){return Err(geo_error("live foundation link references a missing GEO model"));}
                if let Some(id)=&link.scenario_id{
                    if self.geo.records.scenarios.get(id).is_none_or(|s|s.model_id!=link.model_id){return Err(geo_error("live foundation link references a missing or foreign-model scenario"));}
                }
            }
            for(id,assumption)in &layout.input.bedding_assumptions{
                let selection=layout.input.ground_links.get(id).ok_or_else(||geo_error("declared bedding requires an explicit foundation ground link"))?;
                let placement=layout.input.layout.elements.iter().find(|p|placement_id(p)==id.as_str()).ok_or_else(||geo_error("declared bedding foundation is absent from its layout"))?;
                let source=self.geo.head_revision.clone().ok_or_else(||geo_error("declared bedding requires an authored GEO source"))?;
                let snapshot=geo_core::make_geo_snapshot(&self.geo,geo_core::GeoSnapshotId::new("layout-bedding-validation").map_err(geo_error)?,selection,source).map_err(geo_error)?;
                let scenario_id=selection.scenario_id.as_ref().ok_or_else(||geo_error("declared bedding requires a named GEO scenario and selected source closure"))?;
                let scenario=snapshot.records.scenarios.get(scenario_id).ok_or_else(||geo_error("declared bedding scenario is absent"))?;
                let context=query_placement_ground(&snapshot,scenario,placement)?;
                // Incomplete geometric data may remain an explicit draft;
                // missing/foreign stiffness sources are invalid authored links.
                foundation_core::geo_context::validate_bedding_assumption(&context,assumption).map_err(geo_error)?;
            }
            validate_core_source(&layout.source_revision, uuid)?;
            if layout.input_fingerprint != geo_fingerprint(&layout.input)? { return Err(geo_error("layout input fingerprint mismatch")); }
        }
        if self.snapshots.len() > MAX_GEO_FOUNDATION_SNAPSHOTS { return Err(geo_error("GEO/foundation snapshot limit exceeded")); }
        for (id, snapshot) in &self.snapshots {
            snapshot.validate()?;
            validate_geo_source(&snapshot.input.ground.source_revision, uuid)?;
            validate_core_source(&snapshot.input.layout_source_revision, uuid)?;
            if id.get() == 0 || id.get() <= snapshot.input.layout_source_revision.operation_id.get()
                || id.get() <= snapshot.input.ground.source_revision.operation_id {
                return Err(geo_error("snapshot identity must follow its immutable sources"));
            }
        }
        circular_archive::bounded_serialized_size(self, MAX_GEO_FOUNDATION_DOMAIN_BYTES)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GeoFoundationCommand {
    /// Explicit bootstrap/adoption retains source records; it invents no site facts.
    ReplaceGeoState { state: geo_core::GeoState },
    EditGeo { command: geo_core::GeoCommand },
    ReplaceFoundationLayout { input: GeoFoundationLayoutInput },
    SaveAnalysisSnapshot { snapshot: GeoFoundationAnalysisSnapshot },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFoundationDelta {
    pub state_changed: bool,
    pub layout_changed: bool,
    pub affected_models: Vec<String>,
    #[serde(default)]
    pub created_geo_snapshots: Vec<geo_core::GeoSnapshotId>,
    #[serde(default)]
    pub removed_geo_snapshots: Vec<geo_core::GeoSnapshotId>,
    #[serde(default)]
    pub affected_geo_snapshots: Vec<geo_core::GeoSnapshotId>,
    pub created_snapshots: Vec<EngineeringSnapshotId>,
    pub removed_snapshots: Vec<EngineeringSnapshotId>,
    pub affected_snapshots: Vec<EngineeringSnapshotId>,
}
impl GeoFoundationDelta {
    pub fn is_empty(&self) -> bool {
        !self.state_changed && !self.layout_changed && self.affected_models.is_empty()
            && self.created_geo_snapshots.is_empty() && self.removed_geo_snapshots.is_empty() && self.affected_geo_snapshots.is_empty()
            && self.created_snapshots.is_empty() && self.removed_snapshots.is_empty() && self.affected_snapshots.is_empty()
    }
}

pub(super) fn summarize_geo_foundation(changes: &[Change]) -> GeoFoundationDelta {
    let mut transitions = changes.iter().filter_map(|c| if let Change::GeoFoundation {before,after}=c {Some((before,after))}else{None});
    let Some((first, mut last)) = transitions.next() else { return GeoFoundationDelta::default(); };
    for (_, after) in transitions { last = after; }
    domain_diff(first, last)
}
fn domain_diff(before: &GeoFoundationDomain, after: &GeoFoundationDomain) -> GeoFoundationDelta {
    let affected_models = before.geo.records.models.keys().chain(after.geo.records.models.keys()).collect::<BTreeSet<_>>()
        .into_iter().filter(|id| selected_model_fingerprint(&before.geo,id).ok() != selected_model_fingerprint(&after.geo,id).ok())
        .map(|id| id.as_str().to_owned()).collect();
    let ids: BTreeSet<_> = before.snapshots.keys().chain(after.snapshots.keys()).copied().collect();
    let geo_ids:BTreeSet<_>=before.geo.snapshots.keys().chain(after.geo.snapshots.keys()).cloned().collect();
    let geo_status=|domain:&GeoFoundationDomain,id:&geo_core::GeoSnapshotId|domain.geo.snapshots.get(id)
        .map(|snapshot|geo_core::snapshot_currentness(&domain.geo,snapshot).ok()==Some(geo_core::SnapshotCurrentness::Current));
    GeoFoundationDelta {
        state_changed: before.geo != after.geo,
        layout_changed: before.layout != after.layout,
        affected_models,
        created_geo_snapshots:after.geo.snapshots.keys().filter(|id|!before.geo.snapshots.contains_key(*id)).cloned().collect(),
        removed_geo_snapshots:before.geo.snapshots.keys().filter(|id|!after.geo.snapshots.contains_key(*id)).cloned().collect(),
        affected_geo_snapshots:geo_ids.into_iter().filter(|id|before.geo.snapshots.get(id)!=after.geo.snapshots.get(id)||geo_status(before,id)!=geo_status(after,id)).collect(),
        created_snapshots: after.snapshots.keys().filter(|id|!before.snapshots.contains_key(id)).copied().collect(),
        removed_snapshots: before.snapshots.keys().filter(|id|!after.snapshots.contains_key(id)).copied().collect(),
        affected_snapshots: ids.into_iter().filter(|id| before.snapshots.get(id)!=after.snapshots.get(id)
            || before.ground_snapshot_status(*id)!=after.ground_snapshot_status(*id)).collect(),
    }
}
fn selected_model_fingerprint(state: &geo_core::GeoState, id: &geo_core::GroundModelId) -> Result<String, CoreError> {
    let source = state.head_revision.clone().ok_or_else(||geo_error("GEO state has no source revision"))?;
    let snapshot = geo_core::make_geo_snapshot(state, geo_core::GeoSnapshotId::new("model-delta").map_err(geo_error)?,
        &geo_core::GeoSnapshotSelection {model_id:id.clone(),scenario_id:None}, source).map_err(geo_error)?;
    geo_fingerprint(&(&snapshot.selection,&snapshot.records))
}
pub(super) fn geo_foundation_projection(domain:&GeoFoundationDomain, changes:Option<&[Change]>) -> Option<GeoFoundationDomain> {
    if changes.is_some_and(|cs|!cs.iter().any(|c|matches!(c,Change::GeoFoundation{..}))) { return None; }
    if changes.is_none() && domain.is_empty() {None}else{Some(domain.clone())}
}
fn configured_load_current(domain:&StructuralConfiguratorDomain,binding:&GeoFoundationLoadBinding)->bool{
    match binding{
        GeoFoundationLoadBinding::Manual{..}=>true,
        GeoFoundationLoadBinding::Configured{snapshot_id,content_fingerprint}=>domain.snapshot_status(*snapshot_id)==Some(EngineeringSnapshotStatus::Current)
            &&domain.snapshot(*snapshot_id).is_some_and(|s|&s.content_fingerprint==content_fingerprint),
    }
}
pub(super) fn add_geo_load_status_effects(delta:&mut GeoFoundationDelta,domain:&GeoFoundationDomain,changes:&[Change]){
    let mut transitions=changes.iter().filter_map(|c|if let Change::StructuralConfigurator{before,after}=c{Some((before,after))}else{None});
    let Some((first,mut last))=transitions.next()else{return;};
    for(_,after)in transitions{last=after;}
    delta.affected_snapshots.extend(domain.snapshots.iter().filter_map(|(id,snapshot)|
        (configured_load_current(first,&snapshot.input.load_binding)!=configured_load_current(last,&snapshot.input.load_binding)).then_some(*id)));
    delta.affected_snapshots.sort();delta.affected_snapshots.dedup();
}

impl Document {
    pub fn geo_foundation(&self) -> &GeoFoundationDomain { &self.geo_foundation }
    pub fn geo_state(&self) -> &geo_core::GeoState { self.geo_foundation.geo() }
    /// Read-only query against an immutable selection of the actual current head.
    /// No operation, revision, history entry or numerical solver is created.
    pub fn query_geo(&self,selection:&geo_core::GeoSnapshotSelection,query:&geo_core::GeoQuery)->Result<geo_core::GeoQueryResult,CoreError>{
        let state=self.geo_state();
        let source=state.head_revision.clone().ok_or_else(||geo_error("query requires an authored GEO source revision"))?;
        let snapshot=geo_core::make_geo_snapshot(state,geo_core::GeoSnapshotId::new("current-query").map_err(geo_error)?,selection,source).map_err(geo_error)?;
        let scenario_id=selection.scenario_id.as_ref().ok_or_else(||geo_error("query requires an explicit water scenario; missing is never dry"))?;
        let scenario=snapshot.records.scenarios.get(scenario_id).ok_or_else(||geo_error("selected scenario is missing"))?;
        let resolved=geo_core::resolve_geo_scenario(&snapshot,scenario).map_err(geo_error)?;
        geo_core::query_ground(&resolved,query).map_err(geo_error)
    }
    pub(super) fn apply_geo_foundation_command(&mut self, command:GeoFoundationCommand, operation_id:OperationId) -> Result<bim::Applied,CoreError> {
        if let GeoFoundationCommand::SaveAnalysisSnapshot{snapshot}=&command{
            self.verify_geo_snapshot_origin(&snapshot.input.ground)?;
            self.verify_geo_analysis_layout_source(&snapshot.input)?;
            self.validate_geo_foundation_load_binding(&snapshot.input)?;
            if !self.geo_foundation_load_is_current(&snapshot.input.load_binding)?{return Err(geo_error("new foundation result has stale structural load sources"));}
        }
        let before=self.geo_foundation.clone();
        let after=geo_foundation_transition(&before,&command,self.document_uuid,operation_id)?;
        self.geo_foundation=after.clone();
        Ok((OperationKind::GeoFoundationChanged{command},None,vec![],vec![],"GEO/foundation state".into(),vec![Change::GeoFoundation{before,after}],vec![]))
    }
    pub(super) fn has_geo_foundation_history(&self)->bool {
        let change=|c:&Change|matches!(c,Change::GeoFoundation{..})||matches!(c,Change::Operation{before,after,..}if before.iter().chain(after).any(|o|matches!(o.kind,OperationKind::GeoFoundationChanged{..})));
        !self.geo_foundation.is_empty()||self.operations.values().any(|o|matches!(o.kind,OperationKind::GeoFoundationChanged{..}))
            ||self.revision_states.values().any(|s|s.geo_foundation.is_some()||s.delta.as_ref().is_some_and(|d|d.changes.iter().any(&change)))
            ||self.undo_stack.iter().chain(&self.redo_stack).any(|e|e.changes.iter().any(&change))
            ||self.command_receipts.values().any(|r|r.recorded_commands.as_ref().is_some_and(|cs|cs.iter().any(|c|matches!(c,Command::GeoFoundation(_)))))
    }
    pub(super) fn validate_geo_foundation(&self)->Result<(),CoreError> {
        self.geo_foundation.validate(self.document_uuid)?;
        if self.next_operation_id<=self.geo_foundation.high_water(){return Err(geo_error("operation counter loses GEO high-water mark"));}
        for source in self.geo_foundation.geo.head_revision.iter().chain(self.geo_foundation.geo.snapshots.values().map(|s|&s.source_revision)) {
            let operation=self.operations.get(&OperationId(source.operation_id)).ok_or_else(||geo_error("missing GEO source operation"))?;
            if !matches!(operation.kind,OperationKind::GeoFoundationChanged{..}) {return Err(geo_error("GEO source references another domain"));}
        }
        if let Some(layout)=&self.geo_foundation.layout {
            let operation=self.operations.get(&layout.source_revision.operation_id).ok_or_else(||geo_error("missing foundation placement source operation"))?;
            if !matches!(&operation.kind,OperationKind::GeoFoundationChanged{command:GeoFoundationCommand::ReplaceFoundationLayout{input}} if input==&layout.input){return Err(geo_error("layout disagrees with its source operation"));}
        }
        for (id,snapshot) in &self.geo_foundation.snapshots {
            self.validate_geo_foundation_load_binding(&snapshot.input)?;
            let operation=self.operations.get(&OperationId(id.get())).ok_or_else(||geo_error("missing GEO/foundation archive save operation"))?;
            if !matches!(&operation.kind,OperationKind::GeoFoundationChanged{command:GeoFoundationCommand::SaveAnalysisSnapshot{snapshot:recorded}} if recorded==snapshot){return Err(geo_error("archive disagrees with its save operation"));}
        }
        Ok(())
    }
    pub(super) fn validate_local_geo_foundation(&self,changes:&[Change])->Result<(),CoreError> {
        if changes.iter().any(|c|matches!(c,Change::GeoFoundation{..})) {
            for change in changes {if let Change::GeoFoundation{before,after}=change{before.validate(self.document_uuid)?;after.validate(self.document_uuid)?;}}
            self.validate_geo_foundation()?;
            for change in changes{if let Change::Operation{after:Some(operation),..}=change{
                if operation.revision==self.revision {
                    if let OperationKind::GeoFoundationChanged{command:GeoFoundationCommand::SaveAnalysisSnapshot{snapshot}}=&operation.kind{
                        if !self.geo_foundation_load_is_current(&snapshot.input.load_binding)?{return Err(geo_error("new foundation archive must bind the final current structural source of its atomic transaction"));}
                    }
                }
            }}
        }
        Ok(())
    }
    pub(super) fn check_geo_foundation_pending(&self, changes:&[Change], commands:Option<&[Command]>,payload_bytes:usize)->Result<(),CoreError> {
        if !self.has_geo_foundation_history() && !changes.iter().any(|c|matches!(c,Change::GeoFoundation{..})){return Ok(());}
        let mut bytes=circular_archive::bounded_serialized_size(self,MAX_GEO_FOUNDATION_NATIVE_BYTES)?;
        bytes=bytes.checked_add(circular_archive::bounded_serialized_size(changes,MAX_GEO_FOUNDATION_NATIVE_BYTES)?)
            .and_then(|n|payload_bytes.checked_mul(2).and_then(|p|n.checked_add(p))).ok_or_else(||geo_error("retention byte overflow"))?;
        if let Some(commands)=commands {bytes=bytes.checked_add(circular_archive::bounded_serialized_size(commands,MAX_GEO_FOUNDATION_NATIVE_BYTES)?.checked_mul(4).ok_or_else(||geo_error("receipt reservation overflow"))?).ok_or_else(||geo_error("retention byte overflow"))?;}
        if bytes>MAX_GEO_FOUNDATION_NATIVE_BYTES-65536{return Err(geo_error("retained Native7 exceeds its 64 MiB bounded schema"));}
        Ok(())
    }
    pub(super) fn geo_foundation_diff(&self,from:u64,to:u64)->Result<GeoFoundationDelta,CoreError>{
        let before_state=self.reconstruct_revision_state(from)?;
        let after_state=self.reconstruct_revision_state(to)?;
        let before=before_state.geo_foundation.unwrap_or_default();
        let after=after_state.geo_foundation.unwrap_or_default();
        let old_config=before_state.structural_configurator.unwrap_or_default();
        let new_config=after_state.structural_configurator.unwrap_or_default();
        let mut delta=domain_diff(&before,&after);
        for(id,snapshot)in before.snapshots.iter().chain(&after.snapshots){
            if configured_load_current(&old_config,&snapshot.input.load_binding)!=configured_load_current(&new_config,&snapshot.input.load_binding){delta.affected_snapshots.push(*id);}
        }
        delta.affected_snapshots.sort();delta.affected_snapshots.dedup();Ok(delta)
    }
    /// Explicit save/open audit; local edits do not scan retained history.
    pub(super) fn audit_geo_foundation_sources(&self)->Result<(),CoreError>{
        if !self.has_geo_foundation_history(){return Ok(());}
        circular_archive::bounded_serialized_size(self,MAX_GEO_FOUNDATION_NATIVE_BYTES-4096)?;
        let mut domains=vec![&self.geo_foundation];
        for state in self.revision_states.values(){
            domains.extend(state.geo_foundation.iter());
            if let Some(delta)=&state.delta{for change in &delta.changes{if let Change::GeoFoundation{before,after}=change{domains.push(before);domains.push(after);}}}
        }
        for entry in self.undo_stack.iter().chain(&self.redo_stack){for change in &entry.changes{if let Change::GeoFoundation{before,after}=change{domains.push(before);domains.push(after);}}}
        let mut checked=BTreeSet::new();
        for domain in domains{
            for snapshot in domain.geo.snapshots.values().chain(domain.snapshots.values().map(|s|&s.input.ground)){
                if checked.insert(snapshot.content_fingerprint.clone()){self.verify_geo_snapshot_origin(snapshot)?;}
            }
            for snapshot in domain.snapshots.values(){
                self.validate_geo_foundation_load_binding(&snapshot.input)?;
                self.verify_geo_analysis_layout_source(&snapshot.input)?;
            }
        }
        Ok(())
    }
    fn verify_geo_analysis_layout_source(&self,analysis:&GeoFoundationAnalysisInput)->Result<(),CoreError>{
        let source=self.retained_operation(analysis.layout_source_revision.operation_id).ok_or_else(||geo_error("retained archive layout source operation is missing"))?;
        let OperationKind::GeoFoundationChanged{command:GeoFoundationCommand::ReplaceFoundationLayout{input}}=&source.kind else{return Err(geo_error("retained archive layout source belongs to another command"));};
        let placement=input.layout.elements.iter().find(|p|placement_id(p)==analysis.foundation_id);
        if placement!=Some(&analysis.placement)||input.ground_links.get(&analysis.foundation_id)!=Some(&analysis.ground.selection)
            ||input.bedding_assumptions.get(&analysis.foundation_id)!=Some(&analysis.bedding_assumption){return Err(geo_error("immutable analysis placement/link/bedding disagrees with its original source operation"));}Ok(())
    }
    fn verify_geo_snapshot_origin(&self,snapshot:&geo_core::GeoSnapshot)->Result<(),CoreError>{
        validate_geo_source(&snapshot.source_revision,self.document_uuid)?;
        let id=OperationId(snapshot.source_revision.operation_id);
        let source=self.retained_operation(id).ok_or_else(||geo_error("retained GEO snapshot source operation is missing"))?;
        if !matches!(source.kind,OperationKind::GeoFoundationChanged{..}){return Err(geo_error("snapshot source belongs to another domain"));}
        let after=self.retained_geo_source_domain(id).ok_or_else(||geo_error("snapshot has no source operation/effect journal"))?;
        let expected=geo_core::make_geo_snapshot(&after.geo,snapshot.id.clone(),&snapshot.selection,snapshot.source_revision.clone()).map_err(geo_error)?;
        if &expected!=snapshot{return Err(geo_error("immutable GEO snapshot disagrees with its original selected source closure"));}
        Ok(())
    }
    fn retained_geo_source_domain(&self,id:OperationId)->Option<&GeoFoundationDomain>{
        fn from_changes(changes:&[Change],id:OperationId)->Option<&GeoFoundationDomain>{
            let mut domain=None;
            for change in changes{match change{
                Change::GeoFoundation{after,..}=>domain=Some(after),
                Change::Operation{id:current,before:None,after:Some(_)}=>{if *current==id{return domain;}domain=None;},
                _=>{}
            }}None
        }
        self.revision_states.values().find_map(|state|state.delta.as_ref().and_then(|d|from_changes(&d.changes,id)))
            .or_else(||self.undo_stack.iter().chain(&self.redo_stack).find_map(|e|from_changes(&e.changes,id)))
    }
}

fn geo_foundation_transition(before:&GeoFoundationDomain,command:&GeoFoundationCommand,uuid:Uuid,operation_id:OperationId)->Result<GeoFoundationDomain,CoreError>{
    let mut after=before.clone();
    let source=geo_core::GeoSourceRevision{document_uuid:uuid.to_string(),operation_id:operation_id.get()};
    match command {
        GeoFoundationCommand::ReplaceGeoState{state}=>{
            geo_core::validate_geo_state(state).map_err(geo_error)?;
            if !state.snapshots.is_empty(){return Err(geo_error("bootstrap/adoption cannot import snapshots without their Document journals"));}
            // Adopt records through the canonical mutation path: every imported
            // record receives this actual Document operation, and existing
            // immutable GEO snapshots remain retained, becoming stale as needed.
            after.geo=geo_core::apply_geo_command(&before.geo,&geo_core::GeoCommand::ImportRecords{records:state.records.clone()},source).map_err(geo_error)?.state;
        }
        GeoFoundationCommand::EditGeo{command}=>after.geo=geo_core::apply_geo_command(&before.geo,command,source).map_err(geo_error)?.state,
        GeoFoundationCommand::ReplaceFoundationLayout{input}=>{
            validate_layout_input(input)?;
            for link in input.ground_links.values(){if !before.geo.records.models.contains_key(&link.model_id){return Err(geo_error("foundation link references a missing GEO model"));}}
            after.layout=Some(GeoFoundationLayoutState{input:input.clone(),source_revision:GeoFoundationSourceRevision{document_uuid:uuid,operation_id},input_fingerprint:geo_fingerprint(input)?});
        }
        GeoFoundationCommand::SaveAnalysisSnapshot{snapshot}=>{
            snapshot.validate()?;
            if !before.matches_analysis_dependencies(&snapshot.input)?{return Err(geo_error("GEO-linked result is stale or has no current placement/ground link"));}
            validate_geo_source(&snapshot.input.ground.source_revision,uuid)?;
            validate_core_source(&snapshot.input.layout_source_revision,uuid)?;
            if after.snapshots.insert(EngineeringSnapshotId::new(operation_id.get()),snapshot.clone()).is_some(){return Err(geo_error("immutable archive identity already exists"));}
        }
    }
    after.validate(uuid)?;
    Ok(after)
}
pub(super) fn geo_foundation_effects_match(command:&GeoFoundationCommand,changes:&[Change],operation:&OperationRecord)->bool{
    let [Change::GeoFoundation{before,after}]=changes else{return false;};
    let source=after.geo.head_revision.as_ref().or(before.geo.head_revision.as_ref());
    let uuid=source.and_then(|s|Uuid::parse_str(&s.document_uuid).ok()).or_else(||after.layout.as_ref().or(before.layout.as_ref()).map(|l|l.source_revision.document_uuid));
    uuid.is_some_and(|uuid|geo_foundation_transition(before,command,uuid,operation.id).is_ok_and(|expected|&expected==after))
}

fn validate_geo_source(source:&geo_core::GeoSourceRevision,uuid:Uuid)->Result<(),CoreError>{
    if source.operation_id==0||source.document_uuid!=uuid.to_string(){return Err(geo_error("GEO source belongs to another document or has no operation"));}Ok(())
}
fn validate_core_source(source:&GeoFoundationSourceRevision,uuid:Uuid)->Result<(),CoreError>{
    if source.operation_id.get()==0||source.document_uuid!=uuid{return Err(geo_error("foundation source belongs to another document or has no operation"));}Ok(())
}
fn validate_producer(producer:&EngineeringSourceIdentity)->Result<(),CoreError>{
    let sha=|s:&str|s.len()==64&&s.bytes().all(|b|b.is_ascii_hexdigit());
    if !sha(&producer.source_manifest_sha256)||producer.executable_sha256.as_ref().is_some_and(|s|!sha(s))||producer.solver_version.trim().is_empty()||producer.solver_version.len()>128||producer.source_label.trim().is_empty()||producer.source_label.len()>1024{return Err(geo_error("invalid recorded GEO/foundation producer identity"));}Ok(())
}
fn validate_layout_input(input:&GeoFoundationLayoutInput)->Result<(),CoreError>{
    foundation_core::validate_foundation_layout(&input.layout).map_err(geo_error)?;
    let ids:BTreeSet<_>=input.layout.elements.iter().map(placement_id).collect();
    if input.ground_links.keys().chain(input.bedding_assumptions.keys()).any(|id|!ids.contains(id.as_str())){return Err(geo_error("ground/bedding link references a foundation absent from its layout"));}
    Ok(())
}
fn query_placement_ground(ground:&geo_core::GeoSnapshot,scenario:&geo_core::GeoScenario,placement:&foundation_core::FoundationPlacement)->Result<foundation_core::FoundationGeoContext,CoreError>{
    let resolved=geo_core::resolve_geo_scenario(ground,scenario).map_err(geo_error)?;
    let layout=foundation_core::FoundationLayout{id:"immutable-placement".into(),elements:vec![placement.clone()]};
    foundation_core::geo_context::resolve_geo_context(&resolved,&layout).map_err(geo_error)
}
fn apply_placement_to_rigid_input(placement:&foundation_core::FoundationPlacement,input:&mut foundation_core::RigidMatInput)->Result<(),CoreError>{
    let foundation_core::FoundationPlacement::Raft{id,length_x_m,width_y_m,thickness_m,..}=placement else{return Err(geo_error("numerical reuse currently supports an explicit rectangular raft only; other placements are geometry/context only"));};
    input.foundation_id=foundation_core::FoundationId::new(id.as_str()).map_err(geo_error)?;
    input.geometry.width_x_m=*length_x_m;
    input.geometry.length_y_m=*width_y_m;
    input.geometry.thickness_m=*thickness_m;
    foundation_core::validate_input(input).map_err(geo_error)
}
fn validate_analysis_input(input:&GeoFoundationAnalysisInput)->Result<(),CoreError>{
    if input.foundation_id!=placement_id(&input.placement)||input.foundation_id!=input.effective_input.foundation_id.as_str()
        ||input.placement_fingerprint!=geo_fingerprint(&input.placement)?{
        return Err(geo_error("analysis input does not bind its foundation placement"));
    }
    let uuid=Uuid::parse_str(&input.ground.source_revision.document_uuid).map_err(geo_error)?;
    validate_geo_source(&input.ground.source_revision,uuid)?;
    validate_core_source(&input.layout_source_revision,uuid)?;
    match &input.load_binding{
        GeoFoundationLoadBinding::Manual{source_reference}if source_reference.trim().is_empty()||source_reference.len()>4096=>return Err(geo_error("manual load requires an explicit bounded source reference")),
        GeoFoundationLoadBinding::Configured{snapshot_id,content_fingerprint}if snapshot_id.get()==0||content_fingerprint.len()!=64||!content_fingerprint.bytes().all(|b|b.is_ascii_hexdigit())=>return Err(geo_error("invalid configured load source identity")),
        _=>{}
    }
    geo_core::validate_snapshot_binding(&input.ground).map_err(geo_error)?;
    let mut effective=input.effective_input.clone();
    apply_placement_to_rigid_input(&input.placement,&mut effective)?;
    if effective!=input.effective_input{return Err(geo_error("numerical input geometry differs from the selected foundation placement"));}
    if !matches!(input.scenario.water,geo_core::WaterScenario::ExplicitDry{..}){
        return Err(geo_error("foundation numerical adapter requires an explicit dry scenario; unknown/observed groundwater is unsupported"));
    }
    let context=query_placement_ground(&input.ground,&input.scenario,&input.placement)?;
    if context!=input.geo_context||context.readiness.state!=foundation_core::geo_context::FoundationGeoReadinessState::BoundInputsOnly{return Err(geo_error("foundation GEO sampling is unknown, outside coverage, conflicting or disagrees with its immutable source"));}
    for sample in &context.samples{
        let column=sample.result.column.as_ref().ok_or_else(||geo_error("foundation base has no modeled ground column"))?;
        if !column.layers.iter().any(|layer|sample.foundation_base_elevation_m<=layer.top_abs_m+1e-9&&sample.foundation_base_elevation_m>=layer.bottom_abs_m-1e-9){return Err(geo_error("foundation base lies above terrain, below the modeled ground, or inside an unresolved interval"));}
    }
    validate_manual_bedding_input(&input.bedding_assumption,&input.effective_input)?;
    let bedding=foundation_core::geo_context::validate_bedding_assumption(&context,&input.bedding_assumption).map_err(geo_error)?;
    if bedding.state!=foundation_core::geo_context::FoundationGeoReadinessState::BoundInputsOnly{return Err(geo_error("foundation bedding assumption is incomplete"));}
    Ok(())
}
fn validate_manual_bedding_input(assumption:&foundation_core::geo_context::FoundationBeddingAssumption,input:&foundation_core::RigidMatInput)->Result<(),CoreError>{
    let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{k_kn_m3:declared,assumption_note,source_ids}=assumption else{return Err(geo_error("GEO foundation numerical adapter currently supports an explicit uniform manual bedding assumption only; per-IGE allocation is unsupported"));};
    let foundation_core::SupportField::Uniform{k_kn_m3,..}=&input.soil.support else{return Err(geo_error("GEO foundation numerical adapter currently supports uniform numerical bedding only; per-cell allocation is unsupported"));};
    if declared!=k_kn_m3||assumption_note!=&input.soil.assumption_note||source_ids.is_empty(){return Err(geo_error("numerical stiffness/note must match the exact declared layout bedding assumption with explicit source IDs"));}
    Ok(())
}

impl Document{
    /// Prepare source-bound input without analysis or a domain/history mutation.
    pub fn prepare_geo_foundation_analysis(&self,foundation_id:&str,mut input:foundation_core::RigidMatInput)->Result<GeoFoundationAnalysisInput,CoreError>{
        let domain=self.geo_foundation();
        let layout=domain.layout().ok_or_else(||geo_error("no foundation layout"))?;
        let placement=layout.input.layout.elements.iter().find(|p|placement_id(p)==foundation_id).ok_or_else(||geo_error("unknown foundation placement"))?.clone();
        let selection=layout.input.ground_links.get(foundation_id).ok_or_else(||geo_error("foundation has no explicit GEO link"))?;
        let bedding_assumption=layout.input.bedding_assumptions.get(foundation_id).ok_or_else(||geo_error("foundation is geometry-only: no explicit authored bedding assumption/source is declared"))?.clone();
        let source=domain.geo.head_revision.clone().ok_or_else(||geo_error("foundation query requires an authored GEO source"))?;
        let snapshot_id=geo_core::GeoSnapshotId::new(format!("foundation-input-{}",geo_fingerprint(&(foundation_id,selection,&source))?)).map_err(geo_error)?;
        let ground=geo_core::make_geo_snapshot(&domain.geo,snapshot_id,selection,source).map_err(geo_error)?;
        let scenario_id=selection.scenario_id.as_ref().ok_or_else(||geo_error("foundation requires an explicit GEO/water scenario"))?;
        let scenario=ground.records.scenarios.get(scenario_id).ok_or_else(||geo_error("foundation scenario missing"))?.clone();
        let geo_context=query_placement_ground(&ground,&scenario,&placement)?;
        apply_placement_to_rigid_input(&placement,&mut input)?;
        if let foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform{assumption_note,..}=&bedding_assumption{
            // The effective solver DTO is a projection of the authored note,
            // not a second editable stiffness/source registry.
            input.soil.assumption_note=assumption_note.clone();
        }
        validate_manual_bedding_input(&bedding_assumption,&input)?;
        let prepared=GeoFoundationAnalysisInput{foundation_id:foundation_id.into(),placement_fingerprint:geo_fingerprint(&placement)?,placement,
            load_binding:GeoFoundationLoadBinding::Manual{source_reference:"EXPLICIT_MANUAL_CONTROL: caller-supplied loads; no structural reaction transfer".into()},
            layout_source_revision:layout.source_revision.clone(),ground,scenario,geo_context,bedding_assumption,effective_input:input};
        validate_analysis_input(&prepared)?;
        Ok(prepared)
    }
    pub fn prepare_geo_foundation_analysis_manual(&self,foundation_id:&str,input:foundation_core::RigidMatInput,source_reference:impl Into<String>)->Result<GeoFoundationAnalysisInput,CoreError>{
        let mut prepared=self.prepare_geo_foundation_analysis(foundation_id,input)?;
        prepared.load_binding=GeoFoundationLoadBinding::Manual{source_reference:source_reference.into()};
        validate_analysis_input(&prepared)?;Ok(prepared)
    }
}

/// Explicit numerical action. No persistence/history/query path calls this.
pub fn analyze_geo_foundation(input:GeoFoundationAnalysisInput,producer:EngineeringSourceIdentity)->Result<GeoFoundationAnalysisSnapshot,CoreError>{
    validate_analysis_input(&input)?;
    let result=foundation_core::solve(&input.effective_input).map_err(geo_error)?;
    GeoFoundationAnalysisSnapshot::new(input,result,producer)
}
fn geo_fingerprint(value:&impl Serialize)->Result<String,CoreError>{Ok(format!("{:x}",Sha256::digest(serde_json::to_vec(value)?)))}
fn geo_error(error:impl fmt::Display)->CoreError{CoreError::InvalidDocument(format!("GEO/foundation: {error}"))}

pub fn placement_id(placement:&foundation_core::FoundationPlacement)->&str{
    match placement{foundation_core::FoundationPlacement::Raft{id,..}|foundation_core::FoundationPlacement::Strip{id,..}|foundation_core::FoundationPlacement::Isolated{id,..}|foundation_core::FoundationPlacement::Stepped{id,..}=>id.as_str()}
}
