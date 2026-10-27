//! Minimal headless JSON adapter for the public `geotech_core` API.
//!
//! Read one strict request from stdin, or pass one UTF-8 JSON file path as the
//! only argument. Every response is one JSON object on stdout. No UI, renderer,
//! GPU, service, or alternate numerical implementation is used here.

use geotech_core::{
    CandidateSearchSpec, ErrorCode, GeotechError, SlopeAnalysisInput, SOLVER_VERSION,
    analyze, geometry, input_fingerprint, report, search, source_fingerprint, validate_input,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CliRequest {
    /// One of: analyze, geometry, candidates, report, validate.
    operation: String,
    input: SlopeAnalysisInput,
    /// Required for candidates; rejected for all other operations.
    #[serde(default)]
    spec: Option<CandidateSearchSpec>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum CliResponse {
    Success { ok: bool, result: Value },
    Failure { ok: bool, error: GeotechError },
}

fn success(result: Value) -> CliResponse {
    CliResponse::Success { ok: true, result }
}

fn failure(error: GeotechError) -> CliResponse {
    CliResponse::Failure { ok: false, error }
}

fn json_value<T: Serialize>(value: T) -> Result<Value, GeotechError> {
    serde_json::to_value(value).map_err(|error| {
        GeotechError::new(
            ErrorCode::NumericOverflow,
            format!("Result cannot be represented as finite JSON: {error}"),
        )
    })
}

fn request_text() -> Result<String, GeotechError> {
    let mut args = env::args_os();
    let _program = args.next();
    match (args.next(), args.next()) {
        (None, None) => {
            let mut text = String::new();
            io::stdin().read_to_string(&mut text).map_err(|error| {
                GeotechError::new(
                    ErrorCode::InvalidParameter,
                    format!("Could not read UTF-8 JSON request from stdin: {error}"),
                )
            })?;
            Ok(text)
        }
        (Some(path), None) => {
            let path = PathBuf::from(path);
            fs::read_to_string(&path).map_err(|error| {
                GeotechError::new(
                    ErrorCode::InvalidParameter,
                    format!("Could not read JSON request at {}: {error}", path.display()),
                )
            })
        }
        _ => Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            "Pass no arguments to read stdin, or exactly one JSON input file path",
        )),
    }
}

fn dispatch(request: CliRequest) -> Result<Value, GeotechError> {
    let CliRequest {
        operation,
        input,
        spec,
    } = request;

    match operation.as_str() {
        "analyze" => {
            if spec.is_some() {
                return Err(GeotechError::new(
                    ErrorCode::InvalidParameter,
                    "The spec field is only valid for the candidates operation",
                ));
            }
            json_value(analyze(&input)?)
        }
        "geometry" => {
            if spec.is_some() {
                return Err(GeotechError::new(
                    ErrorCode::InvalidParameter,
                    "The spec field is only valid for the candidates operation",
                ));
            }
            validate_input(&input)?;
            json_value(geometry(&input.section)?)
        }
        "candidates" => {
            let spec = spec.ok_or_else(|| {
                GeotechError::new(
                    ErrorCode::InvalidSearch,
                    "The candidates operation requires an explicit spec",
                )
            })?;
            json_value(search(&input, &spec)?)
        }
        "report" => {
            if spec.is_some() {
                return Err(GeotechError::new(
                    ErrorCode::InvalidParameter,
                    "The spec field is only valid for the candidates operation",
                ));
            }
            let analysis = analyze(&input)?;
            let text = report(&input, &analysis, None)?;
            json_value(json!({ "analysis": analysis, "report": text }))
        }
        "validate" => {
            if spec.is_some() {
                return Err(GeotechError::new(
                    ErrorCode::InvalidParameter,
                    "The spec field is only valid for the candidates operation",
                ));
            }
            validate_input(&input)?;
            json_value(json!({
                "valid": true,
                "input_fingerprint": input_fingerprint(&input)?,
                "source_fingerprint": source_fingerprint(),
                "solver_version": SOLVER_VERSION,
            }))
        }
        other => Err(GeotechError::new(
            ErrorCode::InvalidParameter,
            format!("Unknown operation '{other}'; expected analyze, geometry, candidates, report or validate"),
        )),
    }
}

fn run() -> i32 {
    let response = match request_text() {
        Err(error) => failure(error),
        Ok(text) => match serde_json::from_str::<CliRequest>(&text) {
            Err(error) => failure(GeotechError::new(
                ErrorCode::InvalidParameter,
                format!("Invalid strict JSON request: {error}"),
            )),
            Ok(request) => match dispatch(request) {
                Ok(result) => success(result),
                Err(error) => failure(error),
            },
        },
    };

    let succeeded = matches!(&response, CliResponse::Success { .. });
    let rendered = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"ok":false,"error":{"code":"NUMERIC_OVERFLOW","message":"Could not encode CLI response","diagnostic_geometry":[]}}"#.into()
    });
    println!("{rendered}");
    if succeeded { 0 } else { 2 }
}

fn main() {
    process::exit(run());
}
