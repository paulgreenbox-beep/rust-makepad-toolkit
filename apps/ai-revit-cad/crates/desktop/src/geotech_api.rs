//! Shared engineering command/solver boundary for the native UI and loopback API.
//! A bounded derived cache never participates in document history or authority.
use super::*;
use ai_revit_core::engineering::{AnalysisCaseId, EngineeringCommand, SlopeAnalysisCase};
use geotech_core::{
    CandidateSearchResult, CandidateSearchSpec, GeotechError, SlopeAnalysisInput,
    SlopeAnalysisResult,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

const MAX_CACHE_ENTRIES: usize = 8;
const MAX_CACHE_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    document: String,
    runtime: String,
    epoch: u64,
    analysis: u64,
    input: String,
    solver: String,
    mode: String,
}

struct CachedAnalysis {
    key: CacheKey,
    input: SlopeAnalysisInput,
    result: SlopeAnalysisResult,
    search: Option<CandidateSearchResult>,
    payload_bytes: usize,
}

#[derive(Default)]
pub(super) struct AnalysisCache {
    entries: VecDeque<Arc<CachedAnalysis>>,
    payload_bytes: usize,
    analyze_executions: u64,
    search_executions: u64,
    hits: u64,
    last_error: Option<GeotechError>,
}

impl AnalysisCache {
    pub(super) fn observation(&self) -> Value {
        json!({"analyze_executions":self.analyze_executions,"search_executions":self.search_executions,
            "cache_hits":self.hits,"entries":self.entries.len(),"serialized_payload_bytes":self.payload_bytes,
            "max_entries":MAX_CACHE_ENTRIES,"max_serialized_payload_bytes":MAX_CACHE_PAYLOAD_BYTES,
            "counter_scope":"Explicit desktop analyze/search calls; validation, standalone headless execution and explicit persisted snapshot commands are separate scopes",
            "memory_scope":"Serialized cached payload budget, not total allocator/private bytes"})
    }

    fn get(&mut self, key: &CacheKey) -> Option<Arc<CachedAnalysis>> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index)?;
        self.entries.push_front(Arc::clone(&entry));
        self.hits += 1;
        Some(entry)
    }

    fn current(&self, key: &CacheKey) -> Option<Arc<CachedAnalysis>> {
        self.entries
            .iter()
            .find(|entry| same_family(&entry.key, key))
            .map(Arc::clone)
    }

    fn insert(&mut self, entry: Arc<CachedAnalysis>) {
        if entry.payload_bytes > MAX_CACHE_PAYLOAD_BYTES {
            return;
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES
            || self.payload_bytes.saturating_add(entry.payload_bytes) > MAX_CACHE_PAYLOAD_BYTES
        {
            let Some(old) = self.entries.pop_back() else {
                break;
            };
            self.payload_bytes -= old.payload_bytes;
        }
        self.payload_bytes += entry.payload_bytes;
        self.entries.push_front(entry);
    }

    fn invalidate_selection(&mut self, key: &CacheKey) {
        // Remove every mode for this input family. A bounded tombstone list
        // could otherwise expire and expose an old success after many failures.
        self.entries.retain(|entry| !same_family(&entry.key, key));
        self.payload_bytes = self.entries.iter().map(|entry| entry.payload_bytes).sum();
    }
}

fn same_family(a: &CacheKey, b: &CacheKey) -> bool {
    a.document == b.document
        && a.runtime == b.runtime
        && a.epoch == b.epoch
        && a.analysis == b.analysis
        && a.input == b.input
        && a.solver == b.solver
}

fn id_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        .filter(|id| *id > 0)
}

pub(super) fn requires_token(method: &str, params: &Value) -> bool {
    matches!(
        method,
        "geotech.slope.create"
            | "geotech.slope.update"
            | "geotech.slope.analyze"
            | "geotech.slope.candidates"
    ) || (method == "geotech.slope.report" && params.get("path").is_some())
}

fn validate_public_mutation(params: &Value) -> Result<(), String> {
    for name in ["command_id", "expected_session_id", "expected_document_id"] {
        if !params
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(format!(
                "geotech mutation requires {name}; mutations use the authorized command.apply boundary"
            ));
        }
    }
    for name in ["expected_revision", "expected_open_generation"] {
        if params.get(name).and_then(Value::as_u64).is_none() {
            return Err(format!("geotech mutation requires unsigned {name}"));
        }
    }
    Ok(())
}

impl App {
    /// Explicit evidence persistence. A searched candidate must first become
    /// an authoritative case; never substitute the original plane silently.
    pub(super) fn geotech_save_snapshot(
        &mut self,
        cx: &mut Cx,
        id: AnalysisCaseId,
    ) -> Result<Value, String> {
        if self.geotech_ui.dirty {
            return Err("Save edited parameters and calculate before recording a snapshot".into());
        }
        let case = self.geotech_case(id)?;
        let key = self.geotech_cache_key(id, &case.input, String::new())?;
        let entry = self
            .geotech_results
            .current(&key)
            .ok_or("Calculate the current prescribed plane first")?;
        if entry.search.is_some() || entry.result.input_fingerprint != key.input {
            return Err("A search minimum must be stored as its own analysis case before a document calculation snapshot is recorded".into());
        }
        let document = self.document.as_ref().ok_or("no document is open")?;
        let expected_revision = document.revision();
        let document_id = document.document_uuid.to_string();
        let command_id = self.next_command_id("geotech-snapshot");
        self.api_command_apply(cx,&json!({"command_id":command_id,"expected_revision":expected_revision,
            "expected_document_id":document_id,"expected_open_generation":self.api_document_generation,
            "expected_session_id":agent_handler::api_session_id(),"transaction_mode":"atomic",
            "commands":[Command::Engineering(EngineeringCommand::SaveCalculationSnapshot{analysis_id:id})]}))
            .map_err(|error|error.message)
    }

    pub(super) fn dispatch_geotech_request(
        &mut self,
        cx: &mut Cx,
        request: &agent_api::ApiCommand,
    ) -> Result<Value, agent_handler::RequestFailure> {
        self.geotech_results.last_error = None;
        if requires_token(&request.method, &request.params) {
            self.require_api_token(request)?;
        }
        if matches!(
            request.method.as_str(),
            "geotech.slope.create" | "geotech.slope.update"
        ) {
            validate_public_mutation(&request.params)?;
        }
        match self.geotech_dispatch(cx, &request.method, &request.params) {
            Ok(result) => Ok(result),
            Err(message) => {
                if let Some(error) = self.geotech_results.last_error.take() {
                    Err(agent_handler::RequestFailure::geometry_error(
                        message,
                        json!({"module":"geotech_core","error":error}),
                    ))
                } else {
                    Err(message.into())
                }
            }
        }
    }

    fn geotech_failure(&mut self, error: GeotechError) -> String {
        let text = error.to_string();
        self.geotech_results.last_error = Some(error);
        text
    }

    fn geotech_case_id(&self, params: &Value) -> Result<AnalysisCaseId, String> {
        if let Some(value) = params.get("analysis_id") {
            return id_value(value)
                .map(AnalysisCaseId::new)
                .ok_or_else(|| "analysis_id must be a positive decimal integer".into());
        }
        if let Some(id) = self.geotech_ui.active_case {
            return Ok(id);
        }
        let document = self.document.as_ref().ok_or("no document is open")?;
        if document.slope_cases().len() == 1 {
            return Ok(*document.slope_cases().keys().next().unwrap());
        }
        Err("analysis_id is required when there is no uniquely selected slope case".into())
    }

    fn geotech_case(&self, id: AnalysisCaseId) -> Result<SlopeAnalysisCase, String> {
        self.document
            .as_ref()
            .ok_or("no document is open")?
            .slope_case(id)
            .cloned()
            .ok_or_else(|| format!("unknown slope analysis {id}"))
    }

    fn geotech_cache_key(
        &self,
        id: AnalysisCaseId,
        input: &SlopeAnalysisInput,
        mode: String,
    ) -> Result<CacheKey, String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        Ok(CacheKey {
            document: document.document_uuid.to_string(),
            runtime: format!("{:?}", document.runtime_instance_id()),
            epoch: self.api_document_generation,
            analysis: id.get(),
            input: geotech_core::input_fingerprint(input).map_err(|e| e.to_string())?,
            solver: format!(
                "{}:{}",
                geotech_core::SOLVER_VERSION,
                geotech_core::source_fingerprint()
            ),
            mode,
        })
    }

    fn geotech_case_value(&self, case: &SlopeAnalysisCase) -> Result<Value, String> {
        let mut value = serde_json::to_value(case).map_err(|e| e.to_string())?;
        let document = self.document.as_ref().ok_or("no document is open")?;
        value["input_fingerprint"] =
            json!(geotech_core::input_fingerprint(&case.input).map_err(|e| e.to_string())?);
        value["revision"] = json!(document.revision());
        value["document_uuid"] = json!(document.document_uuid);
        value["open_generation"] = json!(self.api_document_generation);
        value["solver_version"] = json!(geotech_core::SOLVER_VERSION);
        value["solver_source_fingerprint"] = json!(geotech_core::source_fingerprint());
        Ok(value)
    }

    /// UI entry point. Public requests authenticate above; all mutations still
    /// enter the same api_command_apply transaction implementation.
    pub(super) fn geotech_dispatch(
        &mut self,
        cx: &mut Cx,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        if method != "geotech.slope.inspect" {
            self.geotech_results.last_error = None;
        }
        if !params.is_object() {
            return Err("geotech parameters must be a JSON object".into());
        }
        match method {
            "geotech.slope.create" | "geotech.slope.update" => {
                self.geotech_mutate(cx, method, params)
            }
            "geotech.slope.inspect" => {
                if params.get("analysis_id").is_some() {
                    let id = self.geotech_case_id(params)?;
                    let mut value = self.geotech_case_value(&self.geotech_case(id)?)?;
                    value["ui"] = self.geotech_observation(cx);
                    Ok(value)
                } else {
                    let document = self.document.as_ref().ok_or("no document is open")?;
                    let cases = document
                        .slope_cases()
                        .values()
                        .map(|c| self.geotech_case_value(c))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(
                        json!({"cases":cases,"revision":document.revision(),"ui":self.geotech_observation(cx)}),
                    )
                }
            }
            "geotech.slope.analyze" | "geotech.slope.candidates" => {
                self.geotech_calculate(cx, method, params)
            }
            "geotech.slope.geometry" => {
                let id = self.geotech_case_id(params)?;
                let case = self.geotech_case(id)?;
                let key = self.geotech_cache_key(id, &case.input, String::new())?;
                let (input, result) = match self.geotech_results.current(&key) {
                    Some(entry) => (entry.input.clone(), entry.result.geometry.clone()),
                    None => (
                        case.input.clone(),
                        geotech_core::geometry(&case.input.section)
                            .map_err(|e| self.geotech_failure(e))?,
                    ),
                };
                let mesh = geotech_core::extrude(&result).map_err(|e| self.geotech_failure(e))?;
                Ok(
                    json!({"analysis_id":id,"source_input_fingerprint":key.input,"input_fingerprint":geotech_core::input_fingerprint(&input).map_err(|e|self.geotech_failure(e))?,
                    "geometry":result,"mesh":mesh,"3d_representation":"extruded_2d_section","document_mutated":false}),
                )
            }
            "geotech.slope.report" => {
                let id = self.geotech_case_id(params)?;
                let case = self.geotech_case(id)?;
                let key = self.geotech_cache_key(id, &case.input, String::new())?;
                let entry = self
                    .geotech_results
                    .current(&key)
                    .ok_or("no current calculation is cached; analyze this input snapshot first")?;
                let report =
                    geotech_core::report(&entry.input, &entry.result, entry.search.as_ref())
                        .map_err(|e| self.geotech_failure(e))?;
                let path = if let Some(path) = params.get("path") {
                    let path = path
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .ok_or("report path must be a nonempty string")?;
                    let output =
                        file_guard::FilePolicy::from_environment()?.output(Path::new(path))?;
                    // Exclusive creation also protects a report appearing after path validation.
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&output)
                        .map_err(|e| format!("report requires a new destination: {e}"))?;
                    file.write_all(report.as_bytes())
                        .map_err(|e| e.to_string())?;
                    file.sync_all().map_err(|e| e.to_string())?;
                    Some(output)
                } else {
                    None
                };
                Ok(
                    json!({"analysis_id":id,"source_input_fingerprint":key.input,"input_fingerprint":entry.result.input_fingerprint,
                    "report":report,"path":path,"document_mutated":false,"engineering_verification":"required"}),
                )
            }
            _ => Err(format!("unknown geotechnical API method: {method}")),
        }
    }

    fn geotech_mutate(
        &mut self,
        cx: &mut Cx,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        let input: SlopeAnalysisInput = serde_json::from_value(
            params
                .get("input")
                .ok_or("slope input is required")?
                .clone(),
        )
        .map_err(|e| format!("invalid typed slope input: {e}"))?;
        geotech_core::validate_input(&input).map_err(|e| self.geotech_failure(e))?;
        let command = if method == "geotech.slope.create" {
            EngineeringCommand::CreateSlope {
                input,
                name: params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Slope stability")
                    .to_owned(),
            }
        } else {
            EngineeringCommand::UpdateSlope {
                analysis_id: self.geotech_case_id(params)?,
                input,
                name: params
                    .get("name")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or("slope name must be a string")
                    })
                    .transpose()?,
            }
        };
        let document = self
            .document
            .as_ref()
            .ok_or("create or open an AI Revit document first")?;
        let revision = document.revision();
        let document_id = document.document_uuid.to_string();
        let mut apply = params.clone();
        if apply.get("command_id").is_none() {
            apply["command_id"] = json!(self.next_command_id("geotech"));
        }
        if apply.get("expected_revision").is_none() {
            apply["expected_revision"] = json!(revision);
        }
        if apply.get("expected_document_id").is_none() {
            apply["expected_document_id"] = json!(document_id);
        }
        if apply.get("expected_open_generation").is_none() {
            apply["expected_open_generation"] = json!(self.api_document_generation);
        }
        if apply.get("expected_session_id").is_none() {
            apply["expected_session_id"] = json!(agent_handler::api_session_id());
        }
        apply["transaction_mode"] = json!("atomic");
        apply["commands"] = json!([Command::Engineering(command.clone())]);
        let receipt = match self.api_command_apply(cx, &apply) {
            Ok(receipt) => receipt,
            Err(error) => return Err(error.message),
        };
        let id = match command {
            EngineeringCommand::UpdateSlope { analysis_id, .. } => analysis_id,
            _ => receipt
                .get("delta")
                .and_then(|v| v.get("engineering"))
                .and_then(|v| v.get("created_cases"))
                .and_then(Value::as_array)
                .and_then(|v| v.first())
                .and_then(id_value)
                .map(AnalysisCaseId::new)
                .ok_or("engineering receipt omitted the created analysis identity")?,
        };
        // A pure API mutation does not select a different editor case. The UI
        // save path sets its own selection; an explicit analyze selects and
        // fills the stored source form through geotech_accept_result.
        match self.geotech_case(id) {
            Ok(case) => {
                let mut result = self.geotech_case_value(&case)?;
                result["receipt"] = receipt;
                result["mutation_path"] = json!("authorized_command.apply/unified_transaction");
                Ok(result)
            }
            Err(_) if receipt.get("replayed").and_then(Value::as_bool) == Some(true) => Ok(
                json!({"analysis_id":id,"receipt":receipt,"case_current":false,"mutation_path":"historical_idempotent_receipt"}),
            ),
            Err(error) => Err(error),
        }
    }

    fn geotech_calculate(
        &mut self,
        cx: &mut Cx,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        let id = self.geotech_case_id(params)?;
        let case = self.geotech_case(id)?;
        let spec: Option<CandidateSearchSpec> = if method == "geotech.slope.candidates" {
            Some(
                serde_json::from_value(
                    params
                        .get("spec")
                        .ok_or("candidate search requires explicit spec")?
                        .clone(),
                )
                .map_err(|e| format!("invalid search specification: {e}"))?,
            )
        } else {
            None
        };
        let mode = match spec {
            Some(spec) => format!(
                "search:{}",
                serde_json::to_string(&spec).map_err(|e| e.to_string())?
            ),
            None => "analyze".into(),
        };
        let key = self.geotech_cache_key(id, &case.input, mode)?;
        let (entry, reused) = if let Some(entry) = self.geotech_results.get(&key) {
            (entry, true)
        } else {
            let (input, result, search) = if let Some(spec) = spec {
                self.geotech_results.search_executions += 1;
                let search = match geotech_core::search(&case.input, &spec) {
                    Ok(search) => search,
                    Err(error) => {
                        self.geotech_results.invalidate_selection(&key);
                        self.geotech_invalidate(cx);
                        return Err(self.geotech_failure(error));
                    }
                };
                let (Some(input), Some(result)) =
                    (search.minimum_input.clone(), search.minimum.clone())
                else {
                    self.geotech_results.invalidate_selection(&key);
                    self.geotech_invalidate(cx);
                    return Ok(
                        json!({"analysis_id":id,"status":"no_admissible_candidate","search":search,"document_mutated":false}),
                    );
                };
                (input, result, Some(search))
            } else {
                self.geotech_results.analyze_executions += 1;
                let result =
                    geotech_core::analyze(&case.input).map_err(|e| self.geotech_failure(e))?;
                (case.input.clone(), result, None)
            };
            let payload_bytes = serde_json::to_vec(&(&input, &result, &search))
                .map_err(|e| e.to_string())?
                .len();
            if payload_bytes > MAX_CACHE_PAYLOAD_BYTES {
                self.geotech_results.invalidate_selection(&key);
                self.geotech_invalidate(cx);
                return Err("calculation payload exceeds the explicit desktop cache budget".into());
            }
            let entry = Arc::new(CachedAnalysis {
                key,
                input,
                result,
                search,
                payload_bytes,
            });
            self.geotech_results.insert(Arc::clone(&entry));
            (entry, false)
        };
        if !self.geotech_ui.dirty {
            self.geotech_accept_result(
                cx,
                id,
                entry.input.clone(),
                entry.result.clone(),
                entry.search.clone(),
            );
        }
        Ok(
            json!({"analysis_id":id,"input_fingerprint":entry.result.input_fingerprint,"source_input_fingerprint":entry.key.input,
            "result":entry.result,"search":entry.search,"cache_reused":reused,"revision":self.document.as_ref().map(Document::revision),
            "document_mutated":false,"engineering_verification":"required","analysis_cache":self.geotech_results.observation()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> CacheKey {
        CacheKey {
            document: "doc".into(),
            runtime: "runtime1".into(),
            epoch: 1,
            analysis: 1,
            input: "fingerprint".into(),
            solver: "v1:source".into(),
            mode: "analyze".into(),
        }
    }
    #[test]
    fn runtime_reopen_input_solver_and_case_boundaries_prevent_cache_reuse() {
        let original = key();
        for variant in 0..6 {
            let mut changed = original.clone();
            match variant {
                0 => changed.document = "other".into(),
                1 => changed.runtime = "runtime2".into(),
                2 => changed.epoch += 1,
                3 => changed.analysis += 1,
                4 => changed.input = "changed".into(),
                _ => changed.solver = "v2".into(),
            }
            assert!(!same_family(&original, &changed));
        }
        let mut search = original.clone();
        search.mode = "search:explicit_spec".into();
        assert!(same_family(&original, &search));
        assert_ne!(original, search);
    }
    #[test]
    fn public_mutations_preserve_command_apply_authority_and_export_authentication() {
        let complete = json!({"command_id":"id","expected_session_id":"session","expected_document_id":"document","expected_revision":0,"expected_open_generation":0});
        assert!(validate_public_mutation(&complete).is_ok());
        for field in [
            "command_id",
            "expected_session_id",
            "expected_document_id",
            "expected_revision",
            "expected_open_generation",
        ] {
            let mut missing = complete.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(validate_public_mutation(&missing).is_err());
        }
        assert!(requires_token("geotech.slope.create", &json!({})));
        assert!(requires_token("geotech.slope.analyze", &json!({})));
        assert!(!requires_token("geotech.slope.inspect", &json!({})));
        assert!(!requires_token("geotech.slope.report", &json!({})));
        assert!(requires_token(
            "geotech.slope.report",
            &json!({"path":"report.txt"})
        ));
        for invalid in [json!(0), json!(-1), json!(1.5), json!("-1")] {
            assert!(id_value(&invalid).is_none());
        }
    }

    #[test]
    fn failed_search_hides_prior_calculation_until_explicit_success() {
        let input = SlopeAnalysisInput {
            section: geotech_core::triangular_section(10.0, 45.0, 30.0, 1.0).unwrap(),
            soil: geotech_core::SoilMaterial {
                name: "synthetic regression".into(),
                unit_weight_kn_per_m3: 18.0,
                effective_cohesion_kpa: 0.0,
                effective_friction_angle_deg: 30.0,
                provenance: "synthetic test values".into(),
            },
            units: Default::default(),
            conditions: Default::default(),
            required_factor_of_safety: None,
            standard: None,
        };
        let entry = Arc::new(CachedAnalysis {
            key: key(),
            result: geotech_core::analyze(&input).unwrap(),
            input,
            search: None,
            payload_bytes: 1,
        });
        let mut cache = AnalysisCache::default();
        cache.insert(Arc::clone(&entry));
        assert!(cache.current(&key()).is_some());
        let mut failed_search = key();
        failed_search.mode = "search:no_admissible_candidate".into();
        cache.invalidate_selection(&failed_search);
        assert!(cache.current(&key()).is_none());
        assert!(cache.get(&key()).is_none());
        cache.insert(entry);
        assert!(cache.current(&key()).is_some());
        assert_eq!(cache.hits, 0);
    }
}
