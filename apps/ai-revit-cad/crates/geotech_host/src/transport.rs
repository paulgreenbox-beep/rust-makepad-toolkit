//! Existing transport's TOTAL JSON line budget; no network/dispatcher is started.
use super::*;
pub const MAX_REQUEST_LINE_BYTES: usize = 1_048_576;
pub const WRAPPER_RESERVE_BYTES: usize = 16 << 10;
pub struct TransportHeader<'a> {
    pub id: &'a serde_json::Value,
    pub method: &'a str,
    pub token: Option<&'a str>,
}
#[derive(Serialize)]
struct Line<'a> {
    id: &'a serde_json::Value,
    method: &'a str,
    params: &'a Submission,
    token: Option<&'a str>,
}
/// Exact bounded existing id/method/params/token line + LF. Secret token is never
/// retained in controller metadata, request digests, receipts or diagnostic logs.
pub fn encode_submission_line(envelope: &Submission, header: TransportHeader<'_>)
    -> Result<Vec<u8>, HostError> {
    finite::audit_finite(&(header.id, header.method, header.token), WRAPPER_RESERVE_BYTES)?;
    hash(&(header.id, header.method, header.token), WRAPPER_RESERVE_BYTES - 256)
        .map_err(|_| HostError::TransportBudget)?;
    finite::audit_finite(envelope, MAX_REQUEST_LINE_BYTES - WRAPPER_RESERVE_BYTES)?;
    hash(envelope, MAX_REQUEST_LINE_BYTES - WRAPPER_RESERVE_BYTES)
        .map_err(|_| HostError::TransportBudget)?;
    let mut sink = BoundedBytes::new(MAX_REQUEST_LINE_BYTES);
    serde_json::to_writer(&mut sink, &Line { id: header.id, method: header.method,
        params: envelope, token: header.token }).map_err(|_| HostError::TransportBudget)?;
    sink.write_all(b"\n").map_err(|_| HostError::TransportBudget)?;
    Ok(sink.bytes)
}
