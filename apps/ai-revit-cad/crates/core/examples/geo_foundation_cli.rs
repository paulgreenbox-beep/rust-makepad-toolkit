//! Bounded ordinary CLI over the same typed Document commands as the UI.
//! Successful requests do not imply native or engineering acceptance.
use ai_revit_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_REQUEST: usize = 4 << 20;
const MAX_ACTIONS: usize = 16;
type Result<T> = std::result::Result<T, String>;

#[derive(Serialize)]
struct SourceIdentity {
    source_manifest_sha256: Option<String>,
    executable_sha256: String,
    package_version: String,
    scope: &'static str,
}
impl SourceIdentity {
    fn producer(&self) -> Result<EngineeringSourceIdentity> {
        Ok(EngineeringSourceIdentity {
            source_manifest_sha256: self.source_manifest_sha256.clone().ok_or("compiled source manifest binding required")?,
            executable_sha256: Some(self.executable_sha256.clone()),
            solver_version: self.package_version.clone(),
            source_label: "GEO/Foundation CLI compiled source and executable identity; engineering approval remains absent".into(),
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    DemoState,
    CreateDemo {
        path: String,
    },
    Create {
        path: String,
        name: String,
        commands: Vec<Command>,
    },
    Document {
        path: String,
        actions: Vec<Action>,
        output_path: Option<String>,
    },
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Inspect,
    Commit {
        command_id: String,
        commands: Vec<Command>,
    },
    Query {
        selection: geo_core::GeoSnapshotSelection,
        query: geo_core::GeoQuery,
    },
    SoilAt {
        selection: geo_core::GeoSnapshotSelection,
        point_m: [f64; 3],
    },
    GroundMap {
        foundation_id: String,
        nx: usize,
        ny: usize,
    },
    AnalyzeStructural,
    AnalyzeManualFoundation {
        foundation_id: String,
        input: foundation_core::RigidMatInput,
        source_reference: String,
    },
    AnalyzeFoundationFromStructure {
        foundation_id: String,
        snapshot_id: u64,
        input: foundation_core::RigidMatInput,
    },
    Undo,
    Redo,
}

fn main() {
    let worker = std::thread::Builder::new()
        .name("geo-foundation-cli".into())
        .stack_size(8 << 20)
        .spawn(run);
    let (status, response) = match worker {
        Ok(worker) => match worker.join() {
            Ok(result) => result,
            Err(_) => (
                1,
                json!({"ok":false,"error":{"code":"worker_panic","message":"bounded CLI worker failed"}}),
            ),
        },
        Err(e) => (
            1,
            json!({"ok":false,"error":{"code":"worker_start_failed","message":e.to_string()}}),
        ),
    };
    println!(
        "{}",
        serde_json::to_string(&response).expect("JSON response")
    );
    std::process::exit(status);
}
fn run() -> (i32, Value) {
    let identity = identity();
    let result = identity
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|id| execute(read_request()?, id));
    match result {
        Ok(outputs) => (
            0,
            json!({"ok":true,"source_identity":identity.ok(),"outputs":outputs,"native_acceptance":"NOT_TESTED","engineering_acceptance":"NOT_VERIFIED"}),
        ),
        Err(e) => (
            1,
            json!({"ok":false,"source_identity":identity.ok(),"error":{"code":"request_failed","message":e}}),
        ),
    }
}
fn read_request() -> Result<Request> {
    let mut args = env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: geo_foundation_cli REQUEST.json")?;
    if args.next().is_some() {
        return Err("exactly one request file is required".into());
    }
    let file = File::open(path).map_err(err)?;
    if file.metadata().map_err(err)?.len() > MAX_REQUEST as u64 {
        return Err("request exceeds 4 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_REQUEST as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > MAX_REQUEST {
        return Err("request exceeds 4 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(err)
}
fn identity() -> Result<SourceIdentity> {
    let compiled = option_env!("AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256")
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned);
    let mut f = File::open(env::current_exe().map_err(err)?).map_err(err)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = f.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(SourceIdentity {
        source_manifest_sha256: compiled,
        executable_sha256: format!("{:x}", hasher.finalize()),
        package_version: env!("CARGO_PKG_VERSION").into(),
        scope: "new GEO/Foundation development; no inherited acceptance",
    })
}
fn execute(request: Request, identity: &SourceIdentity) -> Result<Value> {
    match request {
        Request::DemoState => Ok(json!({"state":geo_core::synthetic_demo_state().map_err(err)?})),
        Request::CreateDemo { path } => create(
            &path,
            "Synthetic GEO + foundation + two-section steel frame".into(),
            geo_foundation_demo_commands().map_err(err)?,
        ),
        Request::Create {
            path,
            name,
            commands,
        } => create(&path, name, commands),
        Request::Document {
            path,
            actions,
            output_path,
        } => {
            if actions.is_empty() || actions.len() > MAX_ACTIONS {
                return Err("document session requires 1..16 explicit actions".into());
            }
            let input_path = fs::canonicalize(&path).map_err(err)?;
            let destination = output_path
                .as_ref()
                .map(|p| checked_output(p, Some(&input_path)))
                .transpose()?;
            let mut document = Document::open(&input_path).map_err(err)?;
            let mut observations = Vec::new();
            for action in actions {
                observations.push(apply_action(&mut document, action, identity)?);
            }
            if let Some(destination) = &destination {
                save_new(&document, destination)?;
            }
            Ok(
                json!({"observations":observations,"saved_to":destination,"document":document,"summary":summary(&document)?}),
            )
        }
    }
}
fn create(path: &str, name: String, commands: Vec<Command>) -> Result<Value> {
    if commands.is_empty() || commands.len() > 64 {
        return Err("create requires 1..64 typed commands".into());
    }
    let destination = checked_output(path, None)?;
    let mut document = Document::new(name).map_err(err)?;
    let receipt = commit(&mut document, "geo-foundation-create".into(), commands)?;
    save_new(&document, &destination)?;
    Ok(
        json!({"saved_to":destination,"receipt":receipt,"document":document,"summary":summary(&document)?}),
    )
}
fn apply_action(
    document: &mut Document,
    action: Action,
    identity: &SourceIdentity,
) -> Result<Value> {
    match action {
        Action::Inspect => summary(document),
        Action::Commit {
            command_id,
            commands,
        } => {
            if commands.is_empty() || commands.len() > 64 {
                return Err("commit requires 1..64 typed commands".into());
            }
            let receipt = commit(document, command_id, commands)?;
            Ok(json!({"commit":receipt,"summary":summary(document)?}))
        }
        Action::Query { selection, query } => {
            let mut response =
                serde_json::to_value(document.query_geo_bound(&selection, &query).map_err(err)?)
                    .map_err(err)?;
            response
                .as_object_mut()
                .ok_or("bound query was not an object")?
                .insert("query".into(), serde_json::to_value(query).map_err(err)?);
            Ok(response)
        }
        Action::SoilAt { selection, point_m } => {
            serde_json::to_value(document.soil_at(&selection, point_m).map_err(err)?).map_err(err)
        }
        Action::GroundMap {
            foundation_id,
            nx,
            ny,
        } => Ok(
            json!({"ground_map":document.foundation_ground_map(&foundation_id,nx,ny).map_err(err)?}),
        ),
        Action::Undo => {
            let receipt = document.undo().map_err(err)?;
            Ok(json!({"undo":receipt,"summary":summary(document)?}))
        }
        Action::Redo => {
            let receipt = document.redo().map_err(err)?;
            Ok(json!({"redo":receipt,"summary":summary(document)?}))
        }
        Action::AnalyzeStructural => {
            let source = document
                .structural_configurator()
                .current()
                .ok_or("no current parametric structural model")?;
            let snapshot = analyze_configured_building(
                source,
                ConfiguratorAnalysisRequest::WholeBuilding,
                identity.producer()?,
            )
            .map_err(err)?;
            let receipt = commit(
                document,
                format!("geo-structural-analysis-{}", revision(document)?),
                vec![Command::StructuralConfigurator(
                    StructuralConfiguratorCommand::SaveAnalysisSnapshot {
                        snapshot: snapshot.clone(),
                    },
                )],
            )?;
            Ok(json!({"snapshot":snapshot,"commit":receipt,"summary":summary(document)?}))
        }
        Action::AnalyzeManualFoundation {
            foundation_id,
            input,
            source_reference,
        } => {
            let prepared = document
                .prepare_geo_foundation_analysis_manual(&foundation_id, input, &source_reference)
                .map_err(err)?;
            let snapshot = analyze_geo_foundation(prepared, identity.producer()?).map_err(err)?;
            let receipt = commit(
                document,
                format!("geo-manual-foundation-analysis-{}", revision(document)?),
                vec![Command::GeoFoundation(
                    GeoFoundationCommand::SaveAnalysisSnapshot {
                        snapshot: snapshot.clone(),
                    },
                )],
            )?;
            Ok(json!({"snapshot":snapshot,"commit":receipt,"summary":summary(document)?}))
        }
        Action::AnalyzeFoundationFromStructure {
            foundation_id,
            snapshot_id,
            input,
        } => {
            let prepared = document
                .prepare_geo_foundation_analysis_from_configured(
                    &foundation_id,
                    EngineeringSnapshotId::new(snapshot_id),
                    input,
                )
                .map_err(err)?;
            let snapshot = analyze_geo_foundation(prepared, identity.producer()?).map_err(err)?;
            let receipt = commit(
                document,
                format!("geo-configured-foundation-analysis-{}", revision(document)?),
                vec![Command::GeoFoundation(
                    GeoFoundationCommand::SaveAnalysisSnapshot {
                        snapshot: snapshot.clone(),
                    },
                )],
            )?;
            Ok(json!({"snapshot":snapshot,"commit":receipt,"summary":summary(document)?}))
        }
    }
}
fn commit(document: &mut Document, id: String, commands: Vec<Command>) -> Result<CommitReceipt> {
    let mut tx = Transaction::new(id, revision(document)?);
    for command in commands {
        tx = tx.with(command);
    }
    document.commit(tx).map_err(err)
}
fn revision(document: &Document) -> Result<u64> {
    Ok(document.revision())
}
fn summary(document: &Document) -> Result<Value> {
    let states:Vec<_>=document.geo_foundation().snapshots().map(|(id,s)|json!({"id":id,"content_fingerprint":s.content_fingerprint,"status":document.geo_foundation_snapshot_status(id),"ground_status":document.geo_foundation().ground_snapshot_status(id)})).collect();
    let structural:Vec<_>=document.structural_configurator().snapshots().map(|(id,s)|json!({"id":id,"content_fingerprint":s.content_fingerprint,"status":document.structural_configurator().snapshot_status(id)})).collect();
    let records = &document.geo_state().records;
    Ok(
        json!({"document_uuid":document.document_uuid,"revision":revision(document)?,
        "geo_counts":{"sources":records.sources.len(),"soils":records.soils.len(),"iges":records.iges.len(),"boreholes":records.boreholes.len(),"models":records.models.len(),"sections":records.sections.len(),"water_observations":records.water_observations.len()},
        "model_ids":records.models.keys().collect::<Vec<_>>(),"foundation_layout":document.geo_foundation().layout(),
        "foundation_archives":states,"structural_archives":structural,
        "engineering_site_approval":"NOT_VERIFIED","normative_methods":"ULTIMATE_BEARING_NOT_VERIFIED; LAYERED_SETTLEMENT_NOT_VERIFIED"}),
    )
}
fn checked_output(requested: &str, input: Option<&Path>) -> Result<PathBuf> {
    let root = env::var_os("AI_REVIT_GF_OUTPUT_ROOT")
        .ok_or("AI_REVIT_GF_OUTPUT_ROOT is required for saves")?;
    let root = fs::canonicalize(root).map_err(err)?;
    if !root.is_dir() {
        return Err("output root must be an existing directory".into());
    }
    let requested = Path::new(requested);
    let parent = requested
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(err)?;
    if !parent.starts_with(root) {
        return Err("output parent is outside the admitted root".into());
    }
    let filename = requested.file_name().ok_or("output filename missing")?;
    let destination = parent.join(filename);
    if destination.exists() || input == Some(destination.as_path()) {
        return Err("output must be new; input and existing files are preserved".into());
    }
    Ok(destination)
}
fn save_new(document: &Document, path: &Path) -> Result<()> {
    // Use the public Native envelope writer and refuse an existing destination.
    // Validation/serialization failure must not create an empty result file.
    let bytes = document.to_json_string().map_err(err)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(err)?;
    file.write_all(bytes.as_bytes()).map_err(err)?;
    file.sync_all().map_err(err)
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
