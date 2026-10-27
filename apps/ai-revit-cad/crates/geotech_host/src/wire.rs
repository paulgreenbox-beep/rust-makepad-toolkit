//! Explicit wire-only decimal projection for the legacy core ground model_revision.
//! Internal JobRequest/core/archive codecs remain unchanged. No f64 conversion.
//! The bounded Value is only a Serde codec bridge for actual typed source tuples,
//! never a result placeholder or an alternative source authority.
use super::*;
use serde::{Deserializer, Serializer, de::DeserializeOwned};
fn encode<T: Serialize, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    let cap = transport::MAX_REQUEST_LINE_BYTES - transport::WRAPPER_RESERVE_BYTES;
    finite::audit_finite(value, cap).map_err(serde::ser::Error::custom)?;
    hash(value, cap).map_err(serde::ser::Error::custom)?;
    let mut wire = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    let stamp = wire.get_mut("ground").and_then(|v| v.get_mut("model_revision"))
        .ok_or_else(|| serde::ser::Error::custom("missing typed ground stamp"))?;
    let id = stamp.as_u64().ok_or_else(|| serde::ser::Error::custom("ground stamp must be u64"))?;
    *stamp = serde_json::Value::String(id.to_string());
    wire.serialize(serializer)
}
fn decode<'de, T: DeserializeOwned, D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
    // Existing transport bounds the TOTAL raw line. Direct consumers must use
    // decode_submission_bytes, which bounds bytes before this codec allocation.
    let mut wire = serde_json::Value::deserialize(deserializer)?;
    let cap = transport::MAX_REQUEST_LINE_BYTES - transport::WRAPPER_RESERVE_BYTES;
    finite::audit_finite(&wire, cap).map_err(serde::de::Error::custom)?;
    hash(&wire, cap).map_err(serde::de::Error::custom)?;
    let stamp = wire.get_mut("ground").and_then(|v| v.get_mut("model_revision"))
        .ok_or_else(|| serde::de::Error::custom("missing typed ground stamp"))?;
    let id: DecimalU64 = serde_json::from_value(stamp.clone()).map_err(serde::de::Error::custom)?;
    *stamp = serde_json::Value::Number(id.0.into());
    serde_json::from_value(wire).map_err(serde::de::Error::custom)
}
pub(super) mod input {
    use super::*;
    pub fn serialize<S: Serializer>(value: &CircularAnalysisInput, s: S) -> Result<S::Ok, S::Error> { encode(value, s) }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CircularAnalysisInput, D::Error> { decode(d) }
}
pub(super) mod search {
    use super::*;
    pub fn serialize<S: Serializer>(value: &SearchPublicRequestV1, s: S) -> Result<S::Ok, S::Error> { encode(value, s) }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SearchPublicRequestV1, D::Error> { decode(d) }
}
pub fn decode_submission_bytes(bytes: &[u8]) -> Result<Submission, HostError> {
    if bytes.len() > transport::MAX_REQUEST_LINE_BYTES - transport::WRAPPER_RESERVE_BYTES {
        return Err(HostError::TransportBudget);
    }
    let envelope: Submission = serde_json::from_slice(bytes).map_err(|_| HostError::InvalidEnvelope)?;
    finite::audit_finite(&envelope, transport::MAX_REQUEST_LINE_BYTES - transport::WRAPPER_RESERVE_BYTES)?;
    Ok(envelope)
}
