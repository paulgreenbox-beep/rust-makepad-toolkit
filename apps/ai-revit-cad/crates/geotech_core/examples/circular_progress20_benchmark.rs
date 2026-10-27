//! Bounded direct-public-API diagnostic benchmark.
//!
//! Usage: circular_progress20_benchmark homog256.json layered256.json W1256.json
//! The positional labels are fixed. Each strict CLI request must request 256 slices.
//! Timings cover diagnose_specified_circle, including its internal result-byte-budget
//! serialization. Caller file I/O, request parsing, result validation, and output
//! JSON are outside the timed region. This is not a pure numeric-kernel measurement.
//! Diagnostic output is never engineering acceptance; API transport, UI, app, and
//! GUI time are unmeasured. The cooperative 30-second budget cannot preempt one call.

use geotech_core::circular::{
    ArithmeticEnvironmentReport, CIRCULAR_SOLVER_VERSION, CircularAnalysisConfig,
    CircularAnalysisInput, CircularSlopeResult, diagnose_specified_circle,
    input_fingerprint, inspect_arithmetic_environment, source_fingerprint,
};
use serde::{Deserialize, Serialize, Serializer};
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    time::{Duration, Instant},
};

const INPUT_BUDGET_BYTES: usize = 16 * 1024 * 1024;
const WARMUPS: usize = 3;
const SAMPLES: usize = 20;
const COOPERATIVE_BUDGET: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: CircularAnalysisInput,
    config: CircularAnalysisConfig,
}

struct LoadedCase {
    label: &'static str,
    request_sha256: String,
    request: Request,
    input_fingerprint: String,
}

#[derive(Serialize)]
struct Context {
    debug_assertions: bool,
    profile_hint: &'static str,
    operating_system: &'static str,
    architecture: &'static str,
    executable: String,
}

#[derive(Serialize)]
struct CaseReport {
    label: &'static str,
    requested_slices: usize,
    warmups: usize,
    n: usize,
    request_sha256: String,
    analysis_id: String,
    source_revision: String,
    input_fingerprint: String,
    solver_version: String,
    source_fingerprint: String,
    sample_nanos: Vec<u128>,
    median_nanos: f64,
    p95_nearest_rank_nanos: u128,
    diagnostic_scope: &'static str,
}

#[derive(Serialize)]
struct BenchmarkReport {
    benchmark: &'static str,
    measurement_scope: &'static str,
    caller_input_io_and_parse: &'static str,
    caller_result_validation_and_output_json: &'static str,
    core_output_budget_serialization: &'static str,
    api_transport_ui_app_gui: &'static str,
    cooperative_budget: &'static str,
    total_input_bytes: usize,
    environment_before: ArithmeticEnvironmentReport,
    environment_after: ArithmeticEnvironmentReport,
    solver_version: String,
    source_fingerprint: String,
    context: Context,
    cases: Vec<CaseReport>,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let started = Instant::now();
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "Usage: circular_progress20_benchmark homog256.json layered256.json W1256.json"
                .into(),
        );
    }

    let environment_before = inspect_arithmetic_environment();
    if !environment_before.accepted {
        return Err(format!(
            "Calling-thread arithmetic environment rejected before benchmark: {}",
            serde_json::to_string(&environment_before).unwrap_or_else(|_| "unserializable report".into())
        ));
    }
    let version_before = CIRCULAR_SOLVER_VERSION.to_owned();
    let source_before = source_fingerprint();
    check_budget(started, "initial environment and source checks")?;

    let labels = ["homog256", "layered256", "W1256"];
    let mut total_input_bytes = 0usize;
    let mut cases = Vec::with_capacity(labels.len());
    for (label, path) in labels.into_iter().zip(args.iter()) {
        let (bytes, request_sha256) = read_bounded_request(path, &mut total_input_bytes)?;
        let request: Request = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Invalid strict circular request for {label} ({}): {error}",
                path.to_string_lossy()
            )
        })?;
        if request.config.slices.target_slices != 256 {
            return Err(format!(
                "{label} must request exactly 256 slices; got {}",
                request.config.slices.target_slices
            ));
        }
        let expected_fingerprint = input_fingerprint(&request.input, &request.config)
            .map_err(|error| format!("{label} input binding validation failed: {error}"))?;
        cases.push(LoadedCase {
            label,
            request_sha256,
            request,
            input_fingerprint: expected_fingerprint,
        });
        check_budget(started, "bounded input loading and caller-side validation")?;
    }

    // Every measured interval contains only this public call, its returned Result,
    // and black_box. The core performs its own output-budget serialization inside it.
    let measured_cases = run_cases(started, &cases, &version_before, &source_before);

    // Observe the same calling thread after the final API call, including if a case
    // returned an error; then fail closed before publishing any benchmark report.
    let environment_after = inspect_arithmetic_environment();
    let version_after = CIRCULAR_SOLVER_VERSION.to_owned();
    let source_after = source_fingerprint();
    check_budget(started, "post-run environment and source checks")?;
    if !environment_after.accepted {
        return Err("Calling-thread arithmetic environment rejected after benchmark".into());
    }
    if version_after != version_before || source_after != source_before {
        return Err("Solver version or source fingerprint changed during benchmark".into());
    }
    let case_reports = measured_cases?;

    let report = BenchmarkReport {
        benchmark: "circular_progress20_direct_public_api_diagnostic",
        measurement_scope: "diagnose_specified_circle public API call; includes core internal output-budget serialization; not a pure numeric kernel or phase breakdown",
        caller_input_io_and_parse: "outside timed region",
        caller_result_validation_and_output_json: "outside timed region",
        core_output_budget_serialization: "included inside public API call",
        api_transport_ui_app_gui: "unmeasured",
        cooperative_budget: "30 seconds overall; checked before and after synchronous work; one call cannot be preempted",
        total_input_bytes,
        environment_before,
        environment_after,
        solver_version: version_before,
        source_fingerprint: source_before,
        context: Context {
            debug_assertions: cfg!(debug_assertions),
            profile_hint: option_env!("PROFILE").unwrap_or("unknown"),
            operating_system: env::consts::OS,
            architecture: env::consts::ARCH,
            executable: env::current_exe()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unavailable".into()),
        },
        cases: case_reports,
    };
    let output = serde_json::to_vec(&report)
        .map_err(|error| format!("Could not serialize benchmark report: {error}"))?;
    check_budget(started, "benchmark report serialization")?;
    let stdout = std::io::stdout();
    let mut output_stream = stdout.lock();
    output_stream
        .write_all(&output)
        .and_then(|()| output_stream.write_all(b"\n"))
        .and_then(|()| output_stream.flush())
        .map_err(|error| format!("Could not write benchmark report: {error}"))?;
    check_budget(started, "benchmark report output")
}

fn check_budget(started: Instant, phase: &str) -> Result<(), String> {
    if started.elapsed() >= COOPERATIVE_BUDGET {
        return Err(format!(
            "Cooperative 30-second benchmark budget exceeded during {phase}; a synchronous API call is not preempted"
        ));
    }
    Ok(())
}

fn read_bounded_request(
    path: &OsString,
    total_bytes: &mut usize,
) -> Result<(Vec<u8>, String), String> {
    let display_path = path.to_string_lossy();
    let mut file = File::open(path)
        .map_err(|error| format!("Could not open request {display_path}: {error}"))?;
    let before = file
        .metadata()
        .map_err(|error| format!("Could not inspect request {display_path}: {error}"))?;
    if !before.is_file() {
        return Err(format!("Request is not a regular file: {display_path}"));
    }
    let remaining = INPUT_BUDGET_BYTES.saturating_sub(*total_bytes);
    if before.len() > remaining as u64 {
        return Err(format!(
            "Total request input exceeds the 16 MiB budget at {display_path}"
        ));
    }
    let capacity = usize::try_from(before.len())
        .map_err(|_| format!("Request length cannot fit memory bounds: {display_path}"))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(remaining as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read request {display_path}: {error}"))?;
    let after_length = file
        .metadata()
        .map_err(|error| format!("Could not recheck request {display_path}: {error}"))?
        .len();
    if bytes.len() != capacity || after_length != before.len() {
        return Err(format!(
            "Request length changed while reading or was truncated: {display_path}"
        ));
    }
    *total_bytes = total_bytes
        .checked_add(bytes.len())
        .ok_or_else(|| "Request byte count overflow".to_string())?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    Ok((bytes, hash))
}

fn run_cases(
    started: Instant,
    cases: &[LoadedCase],
    expected_version: &str,
    expected_source: &str,
) -> Result<Vec<CaseReport>, String> {
    let mut reports = Vec::with_capacity(cases.len());
    for case in cases {
        let mut sample_nanos = Vec::with_capacity(SAMPLES);
        for warmup in 0..WARMUPS {
            check_budget(started, "before warmup API call")?;
            let call_started = Instant::now();
            let returned = std::hint::black_box(diagnose_specified_circle(
                &case.request.input,
                &case.request.config,
            ));
            let _elapsed_nanos = call_started.elapsed().as_nanos();
            check_budget(started, "after warmup API call")?;
            let result = returned.map_err(|error| {
                format!("{} warmup {warmup} diagnostic API call failed: {error}", case.label)
            })?;
            validate_result(case, &result, expected_version, expected_source)?;
            check_budget(started, "after warmup result validation")?;
        }
        for sample in 0..SAMPLES {
            check_budget(started, "before measured API call")?;
            let call_started = Instant::now();
            let returned = std::hint::black_box(diagnose_specified_circle(
                &case.request.input,
                &case.request.config,
            ));
            let elapsed_nanos = call_started.elapsed().as_nanos();
            check_budget(started, "after measured API call")?;
            let result = returned.map_err(|error| {
                format!("{} sample {sample} diagnostic API call failed: {error}", case.label)
            })?;
            validate_result(case, &result, expected_version, expected_source)?;
            sample_nanos.push(elapsed_nanos);
            check_budget(started, "after measured result validation")?;
        }
        let (median_nanos, p95_nearest_rank_nanos) = summarize(&sample_nanos)?;
        reports.push(CaseReport {
            label: case.label,
            requested_slices: case.request.config.slices.target_slices,
            warmups: WARMUPS,
            n: SAMPLES,
            request_sha256: case.request_sha256.clone(),
            analysis_id: case.request.input.analysis_id.clone(),
            source_revision: case.request.input.source_revision.clone(),
            input_fingerprint: case.input_fingerprint.clone(),
            solver_version: expected_version.to_owned(),
            source_fingerprint: expected_source.to_owned(),
            sample_nanos,
            median_nanos,
            p95_nearest_rank_nanos,
            diagnostic_scope: "diagnostic_only=true, accepted=false; not engineering acceptance",
        });
        check_budget(started, "case summary")?;
    }
    Ok(reports)
}

fn validate_result(
    case: &LoadedCase,
    result: &CircularSlopeResult,
    expected_version: &str,
    expected_source: &str,
) -> Result<(), String> {
    if result.analysis_id != case.request.input.analysis_id
        || result.source_revision != case.request.input.source_revision
        || result.input_fingerprint != case.input_fingerprint
        || result.solver_version != expected_version
        || result.source_fingerprint != expected_source
    {
        return Err(format!(
            "{} result binding or source identity did not match the request",
            case.label
        ));
    }
    if result.numerically_verified || !result.engineering_review_required
        || !result.diagnostic_only || result.accepted
    {
        return Err(format!(
            "{} result did not preserve the required diagnostic-only, accepted=false scope",
            case.label
        ));
    }
    audit_finite(result)
        .map_err(|error| format!("{} result contains a non-finite field: {error}", case.label))
}

fn summarize(samples: &[u128]) -> Result<(f64, u128), String> {
    if samples.len() != SAMPLES {
        return Err(format!(
            "Expected {SAMPLES} samples, got {}",
            samples.len()
        ));
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let lower_middle = sorted[SAMPLES / 2 - 1];
    let upper_middle = sorted[SAMPLES / 2];
    // Successful samples are bounded by 30 s, so these nanosecond values and
    // their possible half-nanosecond median are exactly representable in f64.
    let median = lower_middle as f64 + (upper_middle - lower_middle) as f64 / 2.0;
    let nearest_rank_index = (95 * SAMPLES).div_ceil(100) - 1;
    Ok((median, sorted[nearest_rank_index]))
}

// A no-allocation serde walk detects NaN/Inf throughout the public result.
// It intentionally does not inspect signs or physical-contact status.
type AuditResult<T> = std::result::Result<T, serde_json::Error>;
fn non_finite_error() -> serde_json::Error {
    <serde_json::Error as serde::ser::Error>::custom("non-finite result scalar")
}
struct FiniteAudit;

fn audit_finite<T: Serialize + ?Sized>(value: &T) -> AuditResult<()> {
    value.serialize(&mut FiniteAudit)
}

impl<'a> Serializer for &'a mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, _value: bool) -> AuditResult<()> { Ok(()) }
    fn serialize_i8(self, _value: i8) -> AuditResult<()> { Ok(()) }
    fn serialize_i16(self, _value: i16) -> AuditResult<()> { Ok(()) }
    fn serialize_i32(self, _value: i32) -> AuditResult<()> { Ok(()) }
    fn serialize_i64(self, _value: i64) -> AuditResult<()> { Ok(()) }
    fn serialize_i128(self, _value: i128) -> AuditResult<()> { Ok(()) }
    fn serialize_u8(self, _value: u8) -> AuditResult<()> { Ok(()) }
    fn serialize_u16(self, _value: u16) -> AuditResult<()> { Ok(()) }
    fn serialize_u32(self, _value: u32) -> AuditResult<()> { Ok(()) }
    fn serialize_u64(self, _value: u64) -> AuditResult<()> { Ok(()) }
    fn serialize_u128(self, _value: u128) -> AuditResult<()> { Ok(()) }
    fn serialize_char(self, _value: char) -> AuditResult<()> { Ok(()) }
    fn serialize_f32(self, value: f32) -> AuditResult<()> {
        if value.is_finite() { Ok(()) } else { Err(non_finite_error()) }
    }
    fn serialize_f64(self, value: f64) -> AuditResult<()> {
        if value.is_finite() { Ok(()) } else { Err(non_finite_error()) }
    }
    fn serialize_str(self, _value: &str) -> AuditResult<()> { Ok(()) }
    fn serialize_bytes(self, _value: &[u8]) -> AuditResult<()> { Ok(()) }
    fn serialize_none(self) -> AuditResult<()> { Ok(()) }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> AuditResult<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> AuditResult<()> { Ok(()) }
    fn serialize_unit_struct(self, _name: &'static str) -> AuditResult<()> { Ok(()) }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> AuditResult<()> { Ok(()) }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> AuditResult<()> { value.serialize(self) }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        value: &T,
    ) -> AuditResult<()> { value.serialize(self) }
    fn serialize_seq(self, _len: Option<usize>) -> AuditResult<Self> { Ok(self) }
    fn serialize_tuple(self, _len: usize) -> AuditResult<Self> { Ok(self) }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> AuditResult<Self> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> AuditResult<Self> { Ok(self) }
    fn serialize_map(self, _len: Option<usize>) -> AuditResult<Self> { Ok(self) }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> AuditResult<Self> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> AuditResult<Self> { Ok(self) }
}

impl SerializeSeq for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> AuditResult<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeTuple for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> AuditResult<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeTupleStruct for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> AuditResult<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeTupleVariant for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> AuditResult<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeMap for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> AuditResult<()> {
        key.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> AuditResult<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeStruct for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> AuditResult<()> { value.serialize(&mut **self) }
    fn end(self) -> AuditResult<()> { Ok(()) }
}

impl SerializeStructVariant for &mut FiniteAudit {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> AuditResult<()> { value.serialize(&mut **self) }
    fn end(self) -> AuditResult<()> { Ok(()) }
}
