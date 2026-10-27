//! Bounded, issued preview plans for the existing unified command dispatcher.
//! The registry retains requests, never a second document or transaction history.
use super::*;
use crate::file_guard::{FilePolicy, MAX_DXF_BYTES, MAX_LAB_BYTES};
use ai_revit_core::{LabImportPreview, ProjectObjectRef};
use drafting_interop::{ImportOptions, import_preview};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_PLANS: usize = 8;
const MAX_STAGING_BYTES: usize = 64 * 1024 * 1024;
const PLAN_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_MAPPING_PAGE: usize = 256;
const DEFAULT_MAPPING_PAGE: usize = 128;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "drawing.import.dxf.preview"
            | "drawing.import.dxf.commit"
            | "drawing.import.lab.preview"
            | "drawing.import.lab.commit"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dxf,
    Lab,
}
impl Kind {
    fn name(self) -> &'static str {
        if self == Self::Dxf { "dxf" } else { "lab" }
    }
    fn limit(self) -> u64 {
        if self == Self::Dxf {
            MAX_DXF_BYTES
        } else {
            MAX_LAB_BYTES
        }
    }
}

struct PreviewPlan {
    binding: Value,
    commands: Vec<Command>,
    commands_sha256: String,
    summary: Value,
    lab_mapping: Option<Value>,
    dxf_handles: Vec<Option<String>>,
    serialized_staging_bytes: usize,
}
struct Entry {
    plan: Arc<PreviewPlan>,
    issued: Instant,
    accepted_command_id: Option<String>,
    accepted_active_drawing_id: Option<Value>,
}
#[derive(Default)]
struct Registry {
    entries: BTreeMap<String, Entry>,
    bytes: usize,
}
static PREVIEWS: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    PREVIEWS.get_or_init(|| Mutex::new(Registry::default()))
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn token(binding: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(binding).map_err(|e| e.to_string())?;
    Ok(format!("import-preview-v1:{}", sha256(&bytes)))
}

impl Registry {
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, entry| now.saturating_duration_since(entry.issued) <= PLAN_TTL);
        self.bytes = self
            .entries
            .values()
            .map(|e| e.plan.serialized_staging_bytes)
            .sum();
    }
    fn issue(
        &mut self,
        plan: PreviewPlan,
        now: Instant,
    ) -> Result<(String, Arc<PreviewPlan>), String> {
        self.prune(now);
        if plan.serialized_staging_bytes > MAX_STAGING_BYTES {
            return Err("import plan exceeds the 64 MiB serialized staging budget".into());
        }
        let key = token(&plan.binding)?;
        if let Some(existing) = self.entries.get(&key) {
            if existing.plan.commands_sha256 != plan.commands_sha256 {
                return Err("preview binding produced inconsistent command payloads".into());
            }
            return Ok((key, Arc::clone(&existing.plan)));
        }
        while self.entries.len() >= MAX_PLANS
            || self.bytes.saturating_add(plan.serialized_staging_bytes) > MAX_STAGING_BYTES
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.issued)
                .map(|(k, _)| k.clone())
                .ok_or("unable to admit bounded import plan")?;
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(old.plan.serialized_staging_bytes);
            }
        }
        let plan = Arc::new(plan);
        self.bytes += plan.serialized_staging_bytes;
        self.entries.insert(
            key.clone(),
            Entry {
                plan: Arc::clone(&plan),
                issued: now,
                accepted_command_id: None,
                accepted_active_drawing_id: None,
            },
        );
        Ok((key, plan))
    }
    fn get(
        &mut self,
        key: &str,
        now: Instant,
    ) -> Result<(Arc<PreviewPlan>, Option<String>, Option<Value>), String> {
        self.prune(now);
        let entry = self.entries.get(key).ok_or(
            "preview token is unknown, expired, evicted or from another process; preview again",
        )?;
        Ok((
            Arc::clone(&entry.plan),
            entry.accepted_command_id.clone(),
            entry.accepted_active_drawing_id.clone(),
        ))
    }
}

impl App {
    pub(super) fn drawing_import_request(
        &mut self,
        cx: &mut Cx,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        if !handles(method) {
            return Err("unknown drawing import route".into());
        }
        if !params.is_object() {
            return Err("import params must be an object".into());
        }
        let kind = if method.starts_with("drawing.import.dxf.") {
            Kind::Dxf
        } else {
            Kind::Lab
        };
        let committing = method.ends_with(".commit");
        let path = string(params, "path", 32768)?;
        let policy = FilePolicy::from_environment()?;
        let (canonical, bytes) = policy.read(Path::new(path), kind.limit())?;
        let options = parse_options(kind, params, &canonical)?;
        let requested_drawing = optional_drawing(params)?;
        let drawing = if kind == Kind::Dxf {
            Some(
                requested_drawing
                    .or_else(|| self.drafting_current_drawing_id())
                    .ok_or("DXF import requires an existing drawing_id")?,
            )
        } else {
            None
        };
        if let Some(id) = requested_drawing.or(drawing) {
            if self.document.as_ref().and_then(|d| d.drawing(id)).is_none() {
                return Err("requested drawing is absent".into());
            }
        }
        let live = self.import_binding(
            kind,
            &canonical,
            &bytes,
            &options,
            requested_drawing,
            drawing,
        )?;
        optional_context_guards(params, &live, !committing)?;
        let page = mapping_page_params(params)?;

        if !committing {
            let plan = self.build_import_plan(kind, &bytes, &options, live, drawing)?;
            let (key, plan) = registry()
                .lock()
                .map_err(|_| "preview registry is unavailable")?
                .issue(plan, Instant::now())?;
            return preview_reply(&key, &plan, page);
        }

        let key = string(params, "preview_token", 256)?;
        let command_id = string(params, "command_id", 256)?;
        if command_id.trim().is_empty() || command_id.contains('\0') {
            return Err("command_id must be nonempty and contain no NUL".into());
        }
        let expected = params
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or("import commit requires unsigned expected_revision from the preview")?;
        let (plan, accepted, accepted_active) = registry()
            .lock()
            .map_err(|_| "preview registry is unavailable")?
            .get(key, Instant::now())?;
        let comparable_live = if accepted.is_some() {
            accepted_replay_context(&plan.binding, &live, accepted_active.as_ref())?
        } else {
            live.clone()
        };
        let replay = validate_commit_binding(
            &plan.binding,
            &comparable_live,
            key,
            expected,
            command_id,
            accepted.as_deref(),
        )?;
        if !plan
            .summary
            .get("can_apply")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err("preview cannot be applied; resolve units/critical losses and request a new preview".into());
        }
        if !replay {
            // Re-run the bounded adapter/allocator preview against the exact
            // current bytes/context. Caller-provided preview objects are ignored.
            let recomputed = self.build_import_plan(kind, &bytes, &options, live, drawing)?;
            if recomputed.commands_sha256 != plan.commands_sha256
                || token(&recomputed.binding)? != key
            {
                return Err("import preview changed; preview again before committing".into());
            }
        }
        // The only mutation path. On retries Core proves exact payload identity
        // and returns its authoritative historical receipt without reapplying.
        let receipt =
            self.apply_unified_commands(cx, command_id, expected, plan.commands.clone())?;
        if replay && receipt.get("replayed").and_then(Value::as_bool) != Some(true) {
            return Err("Core did not confirm an idempotent replay".into());
        }
        // Request staging is disposable. A staging failure after an authoritative
        // commit must never turn that successful commit into an error response.
        let post_commit_active = json!(self.drafting_current_drawing_id());
        let replay_available = if let Ok(mut registry) = registry().lock() {
            if let Some(entry) = registry.entries.get_mut(key) {
                entry.accepted_command_id = Some(command_id.to_owned());
                entry.accepted_active_drawing_id = Some(post_commit_active);
                true
            } else {
                false
            }
        } else {
            false
        };
        let mapping = if kind == Kind::Lab {
            lab_mapping_page(plan.lab_mapping.as_ref().unwrap_or(&Value::Null), page)
        } else {
            dxf_mapping_page(&plan, Some(&receipt), page)
        };
        Ok(
            json!({"committed":true,"preview_token":key,"receipt":receipt,"mapping":mapping,
            "loss_report":plan.summary.get("loss_report"),"source":plan.binding,"staging":staging_policy(&plan),"idempotent_retry_plan_available":replay_available}),
        )
    }

    fn import_binding(
        &self,
        kind: Kind,
        canonical: &Path,
        bytes: &[u8],
        options: &Value,
        requested: Option<DrawingId>,
        target: Option<DrawingId>,
    ) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        Ok(
            json!({"binding_version":1,"process_id":std::process::id(),"kind":kind.name(),
            "canonical_path":canonical.to_string_lossy(),"file_sha256":sha256(bytes),"file_bytes":bytes.len(),"options":options,
            "document_id":document.document_uuid.to_string(),"runtime_instance_id":document.runtime_instance_id(),
            "document_epoch":self.api_document_generation,"revision":document.revision(),
            "requested_drawing_id":requested,"target_drawing_id":target,"active_drawing_id":self.drafting_current_drawing_id()}),
        )
    }

    fn build_import_plan(
        &self,
        kind: Kind,
        bytes: &[u8],
        options: &Value,
        binding: Value,
        drawing: Option<DrawingId>,
    ) -> Result<PreviewPlan, String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        let source_name = options
            .get("source_name")
            .and_then(Value::as_str)
            .ok_or("source_name unavailable")?;
        let (commands, summary, lab_mapping, dxf_handles) = if kind == Kind::Dxf {
            let options = ImportOptions {
                source_name: source_name.into(),
                unit_override: options
                    .get("unit_override")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                accept_critical_losses: options
                    .get("accept_critical_losses")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            let preview = import_preview(bytes, &options)?;
            let commands = if preview.can_apply && !preview.decision_required {
                vec![Command::Draft(DraftCommand::ImportDraftingEntities {
                    batch: preview.to_batch(drawing.ok_or("DXF target drawing missing")?)?,
                })]
            } else {
                vec![]
            };
            let summary = json!({"format":"bounded_ascii_dxf","can_apply":preview.can_apply,"decision_required":preview.decision_required,
                "entity_count":preview.entities.len(),"layer_count":preview.layers.len(),"unit_resolution":preview.source.as_ref().map(|s|&s.unit_resolution),
                "loss_report":preview.report,"project_ids_assigned_on_commit":true});
            (
                commands,
                summary,
                None,
                preview
                    .entities
                    .iter()
                    .map(|e| e.source_handle.clone())
                    .collect(),
            )
        } else {
            let text = std::str::from_utf8(bytes).map_err(|_| "Lab import must be UTF-8 JSON")?;
            let LabImportPreview {
                commands,
                mapping,
                source_document_id,
                source_revision,
                history_imported,
                ..
            } = document
                .prepare_lab_import(text, source_name)
                .map_err(|e| e.to_string())?;
            let mapping = serde_json::to_value(mapping).map_err(|e| e.to_string())?;
            let summary = json!({"format":"drafting_lab_v0.1.1_json","can_apply":true,"decision_required":false,
                "source_document_id":source_document_id,"source_revision":source_revision,"history_imported":history_imported,
                "mapping_counts":mapping_counts(&mapping),"loss_report":{"critical_losses":[],"warnings":[{"code":"lab_history_not_imported","message":"Source Lab history remains external provenance; one new unified transaction imports analytic state"}]}});
            (commands, summary, Some(mapping), Vec::new())
        };
        let payload = serde_json::to_vec(&commands).map_err(|e| e.to_string())?;
        let mut staged = payload.len();
        staged = staged.saturating_add(
            serde_json::to_vec(&summary)
                .map_err(|e| e.to_string())?
                .len(),
        );
        staged = staged.saturating_add(
            serde_json::to_vec(&lab_mapping)
                .map_err(|e| e.to_string())?
                .len(),
        );
        staged = staged.saturating_add(
            serde_json::to_vec(&dxf_handles)
                .map_err(|e| e.to_string())?
                .len(),
        );
        staged = staged.saturating_add(
            serde_json::to_vec(&binding)
                .map_err(|e| e.to_string())?
                .len(),
        );
        if staged > MAX_STAGING_BYTES {
            return Err(
                "import exceeds serialized staging budget; no project state was changed".into(),
            );
        }
        Ok(PreviewPlan {
            binding,
            commands_sha256: sha256(&payload),
            commands,
            summary,
            lab_mapping,
            dxf_handles,
            serialized_staging_bytes: staged,
        })
    }
}

fn string<'a>(params: &'a Value, key: &str, max: usize) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= max && !s.contains('\0'))
        .ok_or_else(|| {
            format!("{key} must be a nonempty string of at most {max} bytes without NUL")
        })
}
fn optional_drawing(params: &Value) -> Result<Option<DrawingId>, String> {
    match params.get("drawing_id") {
        None => Ok(None),
        Some(value) => {
            let text = value
                .as_str()
                .ok_or("drawing_id must be a canonical decimal string")?;
            let id = text.parse::<u64>().map_err(|_| "invalid drawing_id")?;
            if id == 0 || text != id.to_string() {
                return Err("drawing_id must be a canonical nonzero decimal string".into());
            }
            Ok(Some(DrawingId::new(id)))
        }
    }
}
fn parse_options(kind: Kind, params: &Value, canonical: &Path) -> Result<Value, String> {
    let options = params.get("options");
    if options.is_some_and(|v| !v.is_object()) {
        return Err("options must be an object".into());
    }
    if let Some(object) = options.and_then(Value::as_object) {
        for key in object.keys() {
            if key != "source_name"
                && !(kind == Kind::Dxf
                    && matches!(key.as_str(), "unit_override" | "accept_critical_losses"))
            {
                return Err(format!("unsupported import option {key}"));
            }
        }
    }
    let default_name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("input filename must be UTF-8")?;
    let name = options
        .and_then(|v| v.get("source_name"))
        .or_else(|| params.get("source_name"));
    let name = match name {
        Some(v) => v.as_str().ok_or("source_name must be a string")?,
        None => default_name,
    };
    if name.trim().is_empty() || name.len() > 256 || name.contains('\0') {
        return Err("source_name must contain 1..256 bytes without NUL".into());
    }
    if kind == Kind::Lab {
        return Ok(json!({"source_name":name}));
    }
    let unit = match options.and_then(|v| v.get("unit_override")) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() && s.len() <= 64 && !s.contains('\0') => {
            Some(s.as_str())
        }
        _ => {
            return Err(
                "unit_override must be null or a nonempty string of at most 64 bytes".into(),
            );
        }
    };
    let critical = match options.and_then(|v| v.get("accept_critical_losses")) {
        None => false,
        Some(Value::Bool(v)) => *v,
        _ => return Err("accept_critical_losses must be a boolean".into()),
    };
    Ok(json!({"source_name":name,"unit_override":unit,"accept_critical_losses":critical}))
}
fn optional_context_guards(
    params: &Value,
    live: &Value,
    check_revision: bool,
) -> Result<(), String> {
    for (parameter, field) in [
        ("document_id", "document_id"),
        ("runtime_instance_id", "runtime_instance_id"),
        ("document_epoch", "document_epoch"),
    ] {
        if let Some(expected) = params.get(parameter) {
            if Some(expected) != live.get(field) {
                return Err(format!("stale import context: {parameter} changed"));
            }
        }
    }
    if check_revision {
        if let Some(expected) = params.get("expected_revision") {
            if Some(expected) != live.get("revision") {
                return Err("stale import project revision".into());
            }
        }
    }
    Ok(())
}
fn validate_commit_binding(
    issued: &Value,
    live: &Value,
    key: &str,
    expected: u64,
    command: &str,
    accepted: Option<&str>,
) -> Result<bool, String> {
    if issued.get("revision").and_then(Value::as_u64) != Some(expected) {
        return Err("expected_revision differs from the issued preview".into());
    }
    if token(issued)? != key {
        return Err("invalid issued preview binding".into());
    }
    let replay = match accepted {
        Some(id) if id == command => true,
        Some(_) => {
            return Err("preview token was already committed under another command_id".into());
        }
        None => false,
    };
    let mut comparable = live.clone();
    if replay {
        comparable["revision"] = json!(expected);
    }
    if &comparable != issued {
        return Err("stale import preview: file, options, document/runtime/epoch, drawing or revision changed; preview again".into());
    }
    Ok(replay)
}

/// A successful Lab import can activate its first drawing during the shared UI
/// refresh. Bind retries to that observed postcommit state, while preserving all
/// original file/options/runtime/target checks. A later manual switch refuses.
fn accepted_replay_context(
    issued: &Value,
    live: &Value,
    accepted_active: Option<&Value>,
) -> Result<Value, String> {
    let active = accepted_active.ok_or("accepted preview has no postcommit drawing binding")?;
    if live.get("active_drawing_id") != Some(active) {
        return Err("stale import replay: active drawing changed after the accepted commit".into());
    }
    let mut comparable = live.clone();
    comparable["active_drawing_id"] = issued
        .get("active_drawing_id")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(comparable)
}

fn staging_policy(plan: &PreviewPlan) -> Value {
    json!({"scope":"serialized retained request/metadata bytes; excludes parser peak and process heap","serialized_bytes":plan.serialized_staging_bytes,
        "max_serialized_bytes":MAX_STAGING_BYTES,"max_plans":MAX_PLANS,"ttl_seconds":PLAN_TTL.as_secs(),"volatile":true,
        "eviction":"oldest plan when the shared count/serialized-byte budget is reached"})
}
fn preview_reply(key: &str, plan: &PreviewPlan, page: (usize, usize)) -> Result<Value, String> {
    let mapping = if let Some(map) = &plan.lab_mapping {
        lab_mapping_page(map, page)
    } else {
        dxf_mapping_page(plan, None, page)
    };
    Ok(
        json!({"preview_token":key,"expected_revision":plan.binding.get("revision"),"context":plan.binding,
        "preview":plan.summary,"mapping":mapping,"staging":staging_policy(plan)}),
    )
}
fn mapping_page_params(params: &Value) -> Result<(usize, usize), String> {
    let read = |key: &str, default| -> Result<usize, String> {
        match params.get(key) {
            None => Ok(default),
            Some(v) => v
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| format!("{key} must be an unsigned integer")),
        }
    };
    let offset = read("mapping_offset", 0)?;
    let limit = read("mapping_limit", DEFAULT_MAPPING_PAGE)?;
    if limit == 0 || limit > MAX_MAPPING_PAGE {
        return Err(format!("mapping_limit must be 1..{MAX_MAPPING_PAGE}"));
    }
    Ok((offset, limit))
}
fn mapping_counts(map: &Value) -> Value {
    json!({"drawings":map.get("drawings").and_then(Value::as_object).map_or(0,|v|v.len()),"entities":map.get("entities").and_then(Value::as_object).map_or(0,|v|v.len()),"layers":map.get("layers").and_then(Value::as_object).map_or(0,|v|v.len()),"import_assets":map.get("import_assets").and_then(Value::as_object).map_or(0,|v|v.len())})
}
fn lab_mapping_page(map: &Value, (offset, limit): (usize, usize)) -> Value {
    let kinds = ["drawings", "entities", "layers", "import_assets"];
    let total = kinds
        .iter()
        .map(|kind| {
            map.get(*kind)
                .and_then(Value::as_object)
                .map_or(0, |m| m.len())
        })
        .sum::<usize>();
    let entries = kinds
        .iter()
        .flat_map(|kind| {
            map.get(*kind)
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(move |m| {
                    m.iter()
                        .map(move |(old, new)| json!({"kind":kind,"old_id":old,"new_id":new}))
                })
        })
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next = offset.saturating_add(entries.len());
    json!({"entries":entries,"offset":offset,"limit":limit,"total":total,"next_offset":if next<total{Some(next)}else{None},"complete":offset==0&&next>=total,"counts":mapping_counts(map)})
}
fn dxf_mapping_page(
    plan: &PreviewPlan,
    receipt: Option<&Value>,
    (offset, limit): (usize, usize),
) -> Value {
    let ids = receipt
        .and_then(|r| r.get("created_draft_entities"))
        .and_then(Value::as_array);
    let asset = receipt.and_then(|r| r.pointer("/delta/drafting/changed_import_assets/0"));
    let drawing = plan
        .binding
        .get("target_drawing_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
        .map(DrawingId::new);
    let entries=plan.dxf_handles.iter().enumerate().skip(offset).take(limit).map(|(i,handle)|{
        let entity=ids.and_then(|ids|ids.get(i));
        let qualified=entity.and_then(Value::as_str).and_then(|s|s.parse::<u64>().ok()).zip(drawing).map(|(entity_id,drawing_id)|ProjectObjectRef::Drafting{drawing_id,entity_id:ai_revit_core::DraftEntityId::new(entity_id)});
        json!({"source_entity_index":i,"source_handle":handle,"import_asset_id":asset,"project_object":qualified})
    }).collect::<Vec<_>>();
    let next = offset.saturating_add(entries.len());
    let total = plan.dxf_handles.len();
    json!({"entries":entries,"offset":offset,"limit":limit,"total":total,"next_offset":if next<total{Some(next)}else{None},"complete":offset==0&&next>=total,"project_ids_assigned":receipt.is_some()})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Value {
        json!({"binding_version":1,"kind":"dxf","canonical_path":"C:/allowed/source.dxf","file_sha256":"a".repeat(64),"options":{"unit_override":"mm","accept_critical_losses":false},"document_id":"doc","runtime_instance_id":"run","document_epoch":1,"revision":7,"requested_drawing_id":"1","target_drawing_id":"1","active_drawing_id":"1"})
    }
    fn plan(binding: Value, bytes: usize) -> PreviewPlan {
        PreviewPlan {
            binding,
            commands: vec![],
            commands_sha256: "payload".into(),
            summary: json!({"can_apply":true}),
            lab_mapping: None,
            dxf_handles: vec![],
            serialized_staging_bytes: bytes,
        }
    }
    #[test]
    fn import_tokens_bind_file_options_identity_revision_and_drawing() {
        let issued = binding();
        let key = token(&issued).unwrap();
        assert!(!validate_commit_binding(&issued, &issued, &key, 7, "apply", None).unwrap());
        for (field, value) in [
            ("file_sha256", json!("b".repeat(64))),
            ("canonical_path", json!("C:/other/source.dxf")),
            ("options", json!({"unit_override":"inch"})),
            ("document_id", json!("other")),
            ("runtime_instance_id", json!("other")),
            ("document_epoch", json!(2)),
            ("revision", json!(8)),
            ("target_drawing_id", json!("2")),
            ("active_drawing_id", json!("2")),
        ] {
            let mut live = issued.clone();
            live[field] = value;
            assert!(
                validate_commit_binding(&issued, &live, &key, 7, "apply", None).is_err(),
                "{field}"
            );
        }
        assert!(validate_commit_binding(&issued, &issued, "forged", 7, "apply", None).is_err());
        assert!(validate_commit_binding(&issued, &issued, &key, 8, "apply", None).is_err());
    }
    #[test]
    fn import_retry_keeps_identity_and_requires_original_command_id() {
        let issued = binding();
        let key = token(&issued).unwrap();
        let mut live = issued.clone();
        live["revision"] = json!(99);
        assert!(validate_commit_binding(&issued, &live, &key, 7, "apply", None).is_err());
        assert!(validate_commit_binding(&issued, &live, &key, 7, "other", Some("apply")).is_err());
        assert!(validate_commit_binding(&issued, &live, &key, 7, "apply", Some("apply")).unwrap());
        live["runtime_instance_id"] = json!("reopened");
        assert!(validate_commit_binding(&issued, &live, &key, 7, "apply", Some("apply")).is_err());
    }
    #[test]
    fn lab_import_retry_accepts_own_autoactivation_but_refuses_manual_switch() {
        let mut issued = binding();
        issued["kind"] = json!("lab");
        issued["requested_drawing_id"] = Value::Null;
        issued["target_drawing_id"] = Value::Null;
        issued["active_drawing_id"] = Value::Null;
        let key = token(&issued).unwrap();
        let mut live = issued.clone();
        live["revision"] = json!(8);
        live["active_drawing_id"] = json!("1");
        let accepted_active = json!("1");
        let comparable = accepted_replay_context(&issued, &live, Some(&accepted_active)).unwrap();
        assert!(
            validate_commit_binding(
                &issued,
                &comparable,
                &key,
                7,
                "import-lab",
                Some("import-lab")
            )
            .unwrap()
        );
        live["active_drawing_id"] = json!("2");
        assert!(accepted_replay_context(&issued, &live, Some(&accepted_active)).is_err());
        live["active_drawing_id"] = json!("1");
        live["runtime_instance_id"] = json!("reopened");
        let comparable = accepted_replay_context(&issued, &live, Some(&accepted_active)).unwrap();
        assert!(
            validate_commit_binding(
                &issued,
                &comparable,
                &key,
                7,
                "import-lab",
                Some("import-lab")
            )
            .is_err()
        );
    }
    #[test]
    fn import_registry_is_issued_bounded_expiring_and_volatile() {
        let now = Instant::now();
        let mut registry = Registry::default();
        assert!(registry.get("never-issued", now).is_err());
        let (first, _) = registry.issue(plan(binding(), 32), now).unwrap();
        assert!(registry.get(&first, now).is_ok());
        for i in 0..MAX_PLANS {
            let mut b = binding();
            b["revision"] = json!(100 + i);
            registry
                .issue(plan(b, 32), now + Duration::from_millis(i as u64 + 1))
                .unwrap();
        }
        assert_eq!(registry.entries.len(), MAX_PLANS);
        assert!(registry.get(&first, now + Duration::from_secs(1)).is_err());
        assert!(
            registry
                .issue(plan(binding(), MAX_STAGING_BYTES + 1), now)
                .is_err()
        );
        registry.prune(now + PLAN_TTL + Duration::from_secs(1));
        assert!(registry.entries.is_empty());
        assert_eq!(registry.bytes, 0);
    }
    #[test]
    fn import_mapping_is_bounded_and_paged_without_command_payloads() {
        let mapping = json!({"drawings":{"1":"11"},"entities":{"2":"22","3":"33"},"layers":{"1":"44"},"import_assets":{}});
        let page = lab_mapping_page(&mapping, (1, 2));
        assert_eq!(page["entries"].as_array().unwrap().len(), 2);
        assert_eq!(page["total"], 4);
        assert_eq!(page["next_offset"], 3);
        assert!(mapping_page_params(&json!({"mapping_limit":257})).is_err());
        let mut p = plan(binding(), 32);
        p.lab_mapping = Some(mapping);
        let reply = preview_reply("issued", &p, (0, 2)).unwrap();
        assert!(reply.get("commands").is_none());
        assert_eq!(reply["mapping"]["entries"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn import_options_require_explicit_typed_loss_acceptance() {
        let path = PathBuf::from("source.dxf");
        assert!(
            parse_options(
                Kind::Dxf,
                &json!({"options":{"accept_critical_losses":"yes"}}),
                &path
            )
            .is_err()
        );
        assert!(parse_options(Kind::Dxf, &json!({"options":{"unit_override":1}}), &path).is_err());
        assert!(
            parse_options(Kind::Lab, &json!({"options":{"unit_override":"mm"}}), &path).is_err()
        );
        let value = parse_options(
            Kind::Dxf,
            &json!({"options":{"unit_override":"mm","accept_critical_losses":true}}),
            &path,
        )
        .unwrap();
        assert_eq!(value["accept_critical_losses"], true);
        assert_eq!(value["unit_override"], "mm");
    }
}
