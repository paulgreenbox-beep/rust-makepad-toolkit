//! Deterministic, version-bound SHA-256 digests for typed archived payloads.
//!
//! The digest binds an explicit schema version and archived solver-version
//! label. It does not consult the current solver version or execute geometry.
//! JSON object keys are sorted, `-0.0` is canonicalized to `0.0`, and
//! non-finite floats are rejected. Payloads are bounded to 16 MiB of canonical
//! JSON, depth 128, and one million serialized values.

use super::types::{CircularError, CircularErrorCode};
use serde::Serialize;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

const MAX_CANONICAL_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_CANONICAL_DEPTH: usize = 128;
const MAX_CANONICAL_NODES: usize = 1_000_000;
const MAX_ARCHIVED_VERSION_BYTES: usize = 256;
const DIGEST_DOMAIN: &str = "geotech_version_bound_digest_v1";

#[derive(Debug, Clone, Copy)]
enum FailureKind {
    InvalidPayload,
    NonFinite,
    ResourceLimit,
}

#[derive(Debug)]
struct DigestError {
    kind: FailureKind,
    message: String,
}

impl DigestError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::InvalidPayload,
            message: message.into(),
        }
    }

    fn non_finite(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::NonFinite,
            message: message.into(),
        }
    }

    fn resource(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::ResourceLimit,
            message: message.into(),
        }
    }
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DigestError {}

impl serde::ser::Error for DigestError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self::invalid("Custom canonical payload serialization failed")
    }
}

/// Return a deterministic SHA-256 digest bound to the supplied archived
/// schema and solver-version identifiers.
///
/// `payload` must be a typed `Serialize` value. This helper does not accept
/// caller-supplied raw JSON, run a solver, or substitute the current solver
/// version for `archived_solver_version`.
///
/// Serializer-owned formatting buffers are bounded before growth. Arbitrary
/// user `Serialize` or `Display` implementations can allocate or perform work
/// outside this serializer's control; these limits are not a peak-RSS bound.
pub fn canonical_version_bound_digest<T: Serialize + ?Sized>(
    schema_version: u16,
    archived_solver_version: &str,
    payload: &T,
) -> Result<String, CircularError> {
    if schema_version == 0 {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            "schema_version must be a positive integer",
        ));
    }
    if archived_solver_version.trim().is_empty()
        || archived_solver_version.len() > MAX_ARCHIVED_VERSION_BYTES
    {
        return Err(error(
            CircularErrorCode::InvalidParameter,
            format!(
                "archived_solver_version must contain 1..={MAX_ARCHIVED_VERSION_BYTES} UTF-8 bytes"
            ),
        ));
    }

    #[derive(Serialize)]
    struct VersionEnvelope<'a, T: ?Sized> {
        digest_domain: &'static str,
        schema_version: u16,
        archived_solver_version: &'a str,
        payload: &'a T,
    }

    let envelope = VersionEnvelope {
        digest_domain: DIGEST_DOMAIN,
        schema_version,
        archived_solver_version,
        payload,
    };
    let nodes = Rc::new(Cell::new(0));
    let bytes = Rc::new(Cell::new(0));
    let value = envelope
        .serialize(CheckedJsonSerializer {
            depth: 0,
            nodes,
            bytes,
        })
        .map_err(map_digest_error)?;
    let mut canonical = Vec::new();
    let mut written_nodes = 0usize;
    write_canonical(&value, 0, &mut written_nodes, &mut canonical).map_err(map_digest_error)?;

    let digest = Sha256::digest(canonical);
    let mut output = String::with_capacity("sha256:".len() + 64);
    output.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    Ok(output)
}

fn map_digest_error(error: DigestError) -> CircularError {
    let code = match error.kind {
        FailureKind::InvalidPayload => CircularErrorCode::InvalidParameter,
        FailureKind::NonFinite => CircularErrorCode::NonFiniteInput,
        FailureKind::ResourceLimit => CircularErrorCode::ResourceLimit,
    };
    CircularError::new(code, error.message)
}

fn error(code: CircularErrorCode, message: impl Into<String>) -> CircularError {
    CircularError::new(code, message)
}

#[derive(Clone)]
struct CheckedJsonSerializer {
    depth: usize,
    nodes: Rc<Cell<usize>>,
    bytes: Rc<Cell<usize>>,
}

impl CheckedJsonSerializer {
    fn note_at(nodes: &Rc<Cell<usize>>, depth: usize) -> Result<(), DigestError> {
        if depth > MAX_CANONICAL_DEPTH {
            return Err(DigestError::resource(format!(
                "Canonical payload depth exceeds {MAX_CANONICAL_DEPTH}"
            )));
        }
        let next = nodes
            .get()
            .checked_add(1)
            .ok_or_else(|| DigestError::resource("Canonical payload node count overflow"))?;
        if next > MAX_CANONICAL_NODES {
            return Err(DigestError::resource(format!(
                "Canonical payload exceeds {MAX_CANONICAL_NODES} values"
            )));
        }
        nodes.set(next);
        Ok(())
    }

    fn finish(self, value: Value) -> Result<Value, DigestError> {
        Self::note_at(&self.nodes, self.depth)?;
        Ok(value)
    }

    fn note_bytes(bytes: &Rc<Cell<usize>>, amount: usize) -> Result<(), DigestError> {
        let next = bytes
            .get()
            .checked_add(amount)
            .ok_or_else(|| DigestError::resource("Canonical payload byte count overflow"))?;
        if next > MAX_CANONICAL_JSON_BYTES {
            return Err(DigestError::resource(format!(
                "Canonical payload string data exceeds {MAX_CANONICAL_JSON_BYTES} bytes"
            )));
        }
        bytes.set(next);
        Ok(())
    }

    fn child(&self) -> Self {
        Self {
            depth: self.depth.saturating_add(1),
            nodes: self.nodes.clone(),
            bytes: self.bytes.clone(),
        }
    }

    fn compound(self, kind: CompoundKind) -> Result<CheckedCompound, DigestError> {
        Self::note_at(&self.nodes, self.depth)?;
        Ok(CheckedCompound {
            kind,
            child_depth: self.depth.saturating_add(1),
            nodes: self.nodes,
            bytes: self.bytes,
        })
    }
}

struct BoundedStringWriter {
    output: String,
    limit: usize,
    overflowed: bool,
    allocation_failed: bool,
}

impl BoundedStringWriter {
    fn new(limit: usize) -> Self {
        Self {
            output: String::new(),
            limit,
            overflowed: false,
            allocation_failed: false,
        }
    }
}

impl fmt::Write for BoundedStringWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.overflowed || self.allocation_failed {
            return Err(fmt::Error);
        }
        let remaining = self.limit.saturating_sub(self.output.len());
        if text.len() > remaining {
            self.overflowed = true;
            return Err(fmt::Error);
        }
        let current_len = self.output.len();
        let Some(required) = current_len.checked_add(text.len()) else {
            self.overflowed = true;
            return Err(fmt::Error);
        };
        if required > self.output.capacity() {
            let target = required
                .max(self.output.capacity().saturating_mul(2))
                .max(64)
                .min(self.limit);
            if self.output.try_reserve_exact(target - current_len).is_err() {
                self.allocation_failed = true;
                return Err(fmt::Error);
            }
        }
        self.output.push_str(text);
        Ok(())
    }
}

impl Serializer for CheckedJsonSerializer {
    type Ok = Value;
    type Error = DigestError;
    type SerializeSeq = CheckedCompound;
    type SerializeTuple = CheckedCompound;
    type SerializeTupleStruct = CheckedCompound;
    type SerializeTupleVariant = CheckedCompound;
    type SerializeMap = CheckedCompound;
    type SerializeStruct = CheckedCompound;
    type SerializeStructVariant = CheckedCompound;

    fn serialize_bool(self, value: bool) -> Result<Value, DigestError> {
        self.finish(Value::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_i16(self, value: i16) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_i32(self, value: i32) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_i64(self, value: i64) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_i128(self, value: i128) -> Result<Value, DigestError> {
        let json = serde_json::to_value(value).map_err(|e| DigestError::invalid(e.to_string()))?;
        self.finish(json)
    }

    fn serialize_u8(self, value: u8) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_u16(self, value: u16) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_u32(self, value: u32) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_u64(self, value: u64) -> Result<Value, DigestError> {
        self.finish(Value::Number(Number::from(value)))
    }

    fn serialize_u128(self, value: u128) -> Result<Value, DigestError> {
        let json = serde_json::to_value(value).map_err(|e| DigestError::invalid(e.to_string()))?;
        self.finish(json)
    }

    fn serialize_f32(self, value: f32) -> Result<Value, DigestError> {
        self.serialize_f64(value as f64)
    }

    fn serialize_f64(self, value: f64) -> Result<Value, DigestError> {
        if !value.is_finite() {
            return Err(DigestError::non_finite(
                "Canonical payload contains a non-finite float",
            ));
        }
        let number = Number::from_f64(value).ok_or_else(|| {
            DigestError::non_finite("Canonical payload contains a non-finite float")
        })?;
        self.finish(Value::Number(number))
    }

    fn serialize_char(self, value: char) -> Result<Value, DigestError> {
        Self::note_bytes(&self.bytes, value.len_utf8())?;
        self.finish(Value::String(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Value, DigestError> {
        Self::note_bytes(&self.bytes, value.len())?;
        self.finish(Value::String(value.to_owned()))
    }

    fn collect_str<T: ?Sized + fmt::Display>(self, value: &T) -> Result<Value, DigestError> {
        Self::note_at(&self.nodes, self.depth)?;
        let current_bytes = self.bytes.get();
        let remaining = MAX_CANONICAL_JSON_BYTES
            .checked_sub(current_bytes)
            .ok_or_else(|| {
                DigestError::resource(format!(
                    "Canonical payload string data exceeds {MAX_CANONICAL_JSON_BYTES} bytes"
                ))
            })?;
        let mut writer = BoundedStringWriter::new(remaining);
        let format_result = fmt::write(&mut writer, format_args!("{value}"));
        if writer.overflowed {
            return Err(DigestError::resource(format!(
                "Canonical payload string data exceeds {MAX_CANONICAL_JSON_BYTES} bytes"
            )));
        }
        if writer.allocation_failed {
            return Err(DigestError::resource(
                "Canonical payload collect_str allocation failed",
            ));
        }
        format_result
            .map_err(|_| DigestError::invalid("Canonical payload collect_str formatting failed"))?;

        Self::note_bytes(&self.bytes, writer.output.len())?;
        Ok(Value::String(writer.output))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Value, DigestError> {
        Self::note_at(&self.nodes, self.depth)?;
        Self::note_bytes(&self.bytes, value.len())?;
        let child_depth = self.depth.saturating_add(1);
        for _ in value {
            Self::note_at(&self.nodes, child_depth)?;
        }
        Ok(Value::Array(
            value
                .iter()
                .map(|byte| Value::Number(Number::from(*byte)))
                .collect(),
        ))
    }

    fn serialize_none(self) -> Result<Value, DigestError> {
        self.finish(Value::Null)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Value, DigestError> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value, DigestError> {
        self.finish(Value::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, DigestError> {
        self.finish(Value::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Value, DigestError> {
        Self::note_bytes(&self.bytes, variant.len())?;
        self.finish(Value::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Value, DigestError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value, DigestError> {
        Self::note_at(&self.nodes, self.depth)?;
        Self::note_bytes(&self.bytes, variant.len())?;
        let mut object = Map::new();
        object.insert(variant.to_owned(), value.serialize(self.child())?);
        Ok(Value::Object(object))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<CheckedCompound, DigestError> {
        self.compound(CompoundKind::Sequence(Vec::new()))
    }

    fn serialize_tuple(self, _len: usize) -> Result<CheckedCompound, DigestError> {
        self.compound(CompoundKind::Sequence(Vec::new()))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<CheckedCompound, DigestError> {
        self.compound(CompoundKind::Sequence(Vec::new()))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<CheckedCompound, DigestError> {
        Self::note_bytes(&self.bytes, variant.len())?;
        self.compound(CompoundKind::TupleVariant {
            variant: variant.to_owned(),
            values: Vec::new(),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<CheckedCompound, DigestError> {
        self.compound(CompoundKind::Map {
            fields: BTreeMap::new(),
            pending_key: None,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<CheckedCompound, DigestError> {
        self.compound(CompoundKind::Struct(BTreeMap::new()))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<CheckedCompound, DigestError> {
        Self::note_bytes(&self.bytes, variant.len())?;
        self.compound(CompoundKind::StructVariant {
            variant: variant.to_owned(),
            fields: BTreeMap::new(),
        })
    }
}

enum CompoundKind {
    Sequence(Vec<Value>),
    TupleVariant {
        variant: String,
        values: Vec<Value>,
    },
    Map {
        fields: BTreeMap<String, Value>,
        pending_key: Option<String>,
    },
    Struct(BTreeMap<String, Value>),
    StructVariant {
        variant: String,
        fields: BTreeMap<String, Value>,
    },
}

struct CheckedCompound {
    kind: CompoundKind,
    child_depth: usize,
    nodes: Rc<Cell<usize>>,
    bytes: Rc<Cell<usize>>,
}

impl CheckedCompound {
    fn value<T: ?Sized + Serialize>(&self, value: &T) -> Result<Value, DigestError> {
        value.serialize(CheckedJsonSerializer {
            depth: self.child_depth,
            nodes: self.nodes.clone(),
            bytes: self.bytes.clone(),
        })
    }

    fn note_key_bytes(&self, key: &str) -> Result<(), DigestError> {
        CheckedJsonSerializer::note_bytes(&self.bytes, key.len())
    }

    fn push_sequence<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        let value = self.value(value)?;
        match &mut self.kind {
            CompoundKind::Sequence(values) | CompoundKind::TupleVariant { values, .. } => {
                values.push(value);
                Ok(())
            }
            _ => Err(DigestError::invalid("Invalid canonical sequence state")),
        }
    }

    fn insert_field<T: ?Sized + Serialize>(
        &mut self,
        key: String,
        value: &T,
    ) -> Result<(), DigestError> {
        let value = self.value(value)?;
        let fields = match &mut self.kind {
            CompoundKind::Map { fields, .. }
            | CompoundKind::Struct(fields)
            | CompoundKind::StructVariant { fields, .. } => fields,
            _ => return Err(DigestError::invalid("Invalid canonical object state")),
        };
        if fields.insert(key.clone(), value).is_some() {
            return Err(DigestError::invalid(format!(
                "Duplicate canonical object key {key}"
            )));
        }
        Ok(())
    }

    fn end(self) -> Result<Value, DigestError> {
        match self.kind {
            CompoundKind::Sequence(values) => Ok(Value::Array(values)),
            CompoundKind::TupleVariant { variant, values } => {
                let mut object = Map::new();
                object.insert(variant, Value::Array(values));
                Ok(Value::Object(object))
            }
            CompoundKind::Map {
                fields,
                pending_key,
            } => {
                if pending_key.is_some() {
                    return Err(DigestError::invalid(
                        "Canonical map ended with a key lacking a value",
                    ));
                }
                Ok(Value::Object(fields.into_iter().collect()))
            }
            CompoundKind::Struct(fields) => Ok(Value::Object(fields.into_iter().collect())),
            CompoundKind::StructVariant { variant, fields } => {
                let mut object = Map::new();
                object.insert(variant, Value::Object(fields.into_iter().collect()));
                Ok(Value::Object(object))
            }
        }
    }
}

impl SerializeSeq for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        self.push_sequence(value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeTuple for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        self.push_sequence(value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeTupleStruct for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        self.push_sequence(value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeTupleVariant for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        self.push_sequence(value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeMap for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), DigestError> {
        // Run arbitrary key serializers through the same depth, node, and
        // string-byte guards as payload values before accepting scalar keys.
        let key_value = self.value(key)?;
        let (key, serialized_string) = match key_value {
            Value::String(key) => (key, true),
            Value::Number(number) => (number.to_string(), false),
            Value::Bool(value) => (value.to_string(), false),
            _ => return Err(DigestError::invalid("Canonical object key is not scalar")),
        };
        // String keys have already charged their bytes in the checked
        // serializer. Numeric/bool keys become short decimal text here.
        if !serialized_string {
            self.note_key_bytes(&key)?;
        }
        match &mut self.kind {
            CompoundKind::Map { pending_key, .. } if pending_key.is_none() => {
                *pending_key = Some(key);
                Ok(())
            }
            CompoundKind::Map { .. } => Err(DigestError::invalid(
                "Canonical map supplied a second key before its value",
            )),
            _ => Err(DigestError::invalid("Invalid canonical map state")),
        }
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        let key = match &mut self.kind {
            CompoundKind::Map { pending_key, .. } => pending_key
                .take()
                .ok_or_else(|| DigestError::invalid("Canonical map value has no key"))?,
            _ => return Err(DigestError::invalid("Invalid canonical map state")),
        };
        self.insert_field(key, value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeStruct for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        self.note_key_bytes(key)?;
        self.insert_field(key.to_owned(), value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

impl SerializeStructVariant for CheckedCompound {
    type Ok = Value;
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        self.note_key_bytes(key)?;
        self.insert_field(key.to_owned(), value)
    }

    fn end(self) -> Result<Value, DigestError> {
        CheckedCompound::end(self)
    }
}

fn write_canonical(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    output: &mut Vec<u8>,
) -> Result<(), DigestError> {
    if depth > MAX_CANONICAL_DEPTH {
        return Err(DigestError::resource(format!(
            "Canonical payload depth exceeds {MAX_CANONICAL_DEPTH}"
        )));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| DigestError::resource("Canonical payload node count overflow"))?;
    if *nodes > MAX_CANONICAL_NODES {
        return Err(DigestError::resource(format!(
            "Canonical payload exceeds {MAX_CANONICAL_NODES} values"
        )));
    }
    match value {
        Value::Null => append(output, b"null")?,
        Value::Bool(false) => append(output, b"false")?,
        Value::Bool(true) => append(output, b"true")?,
        Value::Number(number) => {
            if number.is_f64() {
                let float = number.as_f64().ok_or_else(|| {
                    DigestError::non_finite("Canonical payload contains an invalid float")
                })?;
                if !float.is_finite() {
                    return Err(DigestError::non_finite(
                        "Canonical payload contains a non-finite float",
                    ));
                }
                if float == 0.0 {
                    append(output, b"0.0")?;
                } else {
                    append(output, number.to_string().as_bytes())?;
                }
            } else {
                append(output, number.to_string().as_bytes())?;
            }
        }
        Value::String(text) => write_string(text, output)?,
        Value::Array(values) => {
            append(output, b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    append(output, b",")?;
                }
                write_canonical(value, depth + 1, nodes, output)?;
            }
            append(output, b"]")?;
        }
        Value::Object(object) => {
            append(output, b"{")?;
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    append(output, b",")?;
                }
                write_string(key, output)?;
                append(output, b":")?;
                write_canonical(&object[key], depth + 1, nodes, output)?;
            }
            append(output, b"}")?;
        }
    }
    Ok(())
}

fn write_string(value: &str, output: &mut Vec<u8>) -> Result<(), DigestError> {
    append(output, b"\"")?;
    for character in value.chars() {
        match character {
            '"' => append(output, b"\\\"")?,
            '\\' => append(output, b"\\\\")?,
            '\u{08}' => append(output, b"\\b")?,
            '\u{0c}' => append(output, b"\\f")?,
            '\n' => append(output, b"\\n")?,
            '\r' => append(output, b"\\r")?,
            '\t' => append(output, b"\\t")?,
            value if value <= '\u{1f}' => {
                let byte = value as u32 as u8;
                const HEX: &[u8; 16] = b"0123456789abcdef";
                append(output, b"\\u00")?;
                append(
                    output,
                    &[HEX[(byte >> 4) as usize], HEX[(byte & 0xf) as usize]],
                )?;
            }
            value => {
                let mut encoded = [0u8; 4];
                append(output, value.encode_utf8(&mut encoded).as_bytes())?;
            }
        }
    }
    append(output, b"\"")
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), DigestError> {
    let new_len = output
        .len()
        .checked_add(bytes.len())
        .ok_or_else(|| DigestError::resource("Canonical JSON byte count overflow"))?;
    if new_len > MAX_CANONICAL_JSON_BYTES {
        return Err(DigestError::resource(format!(
            "Canonical JSON exceeds {MAX_CANONICAL_JSON_BYTES} bytes"
        )));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::{Error as _, SerializeMap, SerializeSeq};
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct TypedPayload {
        values: BTreeMap<String, f64>,
        count: u32,
    }

    fn payload(value: f64) -> TypedPayload {
        let mut values = BTreeMap::new();
        values.insert("z".into(), value);
        values.insert("a".into(), 2.5);
        TypedPayload { values, count: 3 }
    }

    #[test]
    fn binds_schema_and_archived_solver_version_and_normalizes_negative_zero() {
        let positive = canonical_version_bound_digest(1, "archived-0.1", &payload(0.0)).unwrap();
        let negative = canonical_version_bound_digest(1, "archived-0.1", &payload(-0.0)).unwrap();
        assert_eq!(positive, negative);
        assert_ne!(
            positive,
            canonical_version_bound_digest(2, "archived-0.1", &payload(0.0)).unwrap()
        );
        assert_ne!(
            positive,
            canonical_version_bound_digest(1, "archived-0.2", &payload(0.0)).unwrap()
        );
        assert!(positive.starts_with("sha256:"));
    }

    #[test]
    fn rejects_non_finite_typed_payload_values() {
        let error =
            canonical_version_bound_digest(1, "archived-0.1", &payload(f64::NAN)).unwrap_err();
        assert_eq!(error.code, CircularErrorCode::NonFiniteInput);
    }

    #[test]
    fn rejects_missing_digest_version_bindings() {
        assert_eq!(
            canonical_version_bound_digest(0, "archived-0.1", &payload(1.0))
                .unwrap_err()
                .code,
            CircularErrorCode::InvalidParameter
        );
        assert_eq!(
            canonical_version_bound_digest(1, "   ", &payload(1.0))
                .unwrap_err()
                .code,
            CircularErrorCode::InvalidParameter
        );
    }

    struct DeepKey(usize);

    impl Serialize for DeepKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut sequence = serializer.serialize_seq(Some(1))?;
            if self.0 == 0 {
                sequence.serialize_element("key")?;
            } else {
                sequence.serialize_element(&Self(self.0 - 1))?;
            }
            sequence.end()
        }
    }

    struct PayloadWithDeepMapKey;

    impl Serialize for PayloadWithDeepMapKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry(&DeepKey(MAX_CANONICAL_DEPTH + 4), &1_u8)?;
            map.end()
        }
    }

    #[test]
    fn rejects_deep_custom_map_key_before_unbounded_key_materialization() {
        let error =
            canonical_version_bound_digest(1, "archived-0.1", &PayloadWithDeepMapKey).unwrap_err();
        assert_eq!(error.code, CircularErrorCode::ResourceLimit);
        assert!(error.message.contains("depth"));
    }

    struct StreamingDisplay {
        writes: Rc<Cell<usize>>,
        chunks: usize,
    }

    impl fmt::Display for StreamingDisplay {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..self.chunks {
                self.writes.set(self.writes.get() + 1);
                formatter.write_str("x")?;
            }
            Ok(())
        }
    }

    fn serializer_with_remaining_bytes(remaining: usize) -> CheckedJsonSerializer {
        CheckedJsonSerializer {
            depth: 0,
            nodes: Rc::new(Cell::new(0)),
            bytes: Rc::new(Cell::new(MAX_CANONICAL_JSON_BYTES - remaining)),
        }
    }

    #[test]
    fn collect_str_rejects_streamed_output_at_the_remaining_byte_limit() {
        let remaining = 7;
        let calls = Rc::new(Cell::new(0));
        let bytes = Rc::new(Cell::new(MAX_CANONICAL_JSON_BYTES - remaining));
        let serializer = CheckedJsonSerializer {
            depth: 0,
            nodes: Rc::new(Cell::new(0)),
            bytes: bytes.clone(),
        };

        let error = serializer
            .collect_str(&StreamingDisplay {
                writes: calls.clone(),
                chunks: 100_000,
            })
            .unwrap_err();

        assert!(matches!(error.kind, FailureKind::ResourceLimit));
        assert_eq!(calls.get(), remaining + 1);
        assert_eq!(bytes.get(), MAX_CANONICAL_JSON_BYTES - remaining);
    }

    #[test]
    fn bounded_string_writer_preserves_many_one_byte_chunks() {
        let limit = 4_097;
        let mut writer = BoundedStringWriter::new(limit);

        for _ in 0..limit {
            fmt::Write::write_str(&mut writer, "x").unwrap();
        }

        assert_eq!(writer.output, "x".repeat(limit));
        assert!(!writer.overflowed);
        assert!(!writer.allocation_failed);
    }

    #[test]
    fn collect_str_checks_depth_and_node_limits_before_formatting() {
        let calls = Rc::new(Cell::new(0));
        let display = StreamingDisplay {
            writes: calls.clone(),
            chunks: 1,
        };

        for (depth, node_count) in [(MAX_CANONICAL_DEPTH + 1, 0), (0, MAX_CANONICAL_NODES)] {
            let serializer = CheckedJsonSerializer {
                depth,
                nodes: Rc::new(Cell::new(node_count)),
                bytes: Rc::new(Cell::new(MAX_CANONICAL_JSON_BYTES - 7)),
            };
            let error = serializer.collect_str(&display).unwrap_err();

            assert!(matches!(error.kind, FailureKind::ResourceLimit));
            assert_eq!(calls.get(), 0);
        }
    }

    struct StreamingDisplayKey {
        writes: Rc<Cell<usize>>,
        chunks: usize,
    }

    impl Serialize for StreamingDisplayKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(&StreamingDisplay {
                writes: self.writes.clone(),
                chunks: self.chunks,
            })
        }
    }

    struct PayloadWithStreamingDisplayKey {
        writes: Rc<Cell<usize>>,
    }

    impl Serialize for PayloadWithStreamingDisplayKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry(
                &StreamingDisplayKey {
                    writes: self.writes.clone(),
                    chunks: 100_000,
                },
                &1_u8,
            )?;
            map.end()
        }
    }

    #[test]
    fn collect_str_map_keys_use_the_same_remaining_byte_limit() {
        let remaining = 7;
        let calls = Rc::new(Cell::new(0));
        let error = PayloadWithStreamingDisplayKey {
            writes: calls.clone(),
        }
        .serialize(serializer_with_remaining_bytes(remaining))
        .unwrap_err();

        assert!(matches!(error.kind, FailureKind::ResourceLimit));
        assert_eq!(calls.get(), remaining + 1);
    }

    struct DisplayPayload(u32);

    impl Serialize for DisplayPayload {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(&self.0)
        }
    }

    struct StringPayload(&'static str);

    impl Serialize for StringPayload {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.0)
        }
    }

    #[test]
    fn collect_str_preserves_display_text_canonical_digest() {
        let display_digest =
            canonical_version_bound_digest(1, "archived-0.1", &DisplayPayload(42)).unwrap();
        let string_digest =
            canonical_version_bound_digest(1, "archived-0.1", &StringPayload("42")).unwrap();

        assert_eq!(display_digest, string_digest);
    }

    struct FailingDisplay;

    impl fmt::Display for FailingDisplay {
        fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn collect_str_distinguishes_formatter_errors_from_resource_limits() {
        let error = serializer_with_remaining_bytes(7)
            .collect_str(&FailingDisplay)
            .unwrap_err();

        assert!(matches!(error.kind, FailureKind::InvalidPayload));
        assert!(error.message.contains("formatting failed"));
    }

    struct PayloadWithCustomErrorMessage {
        writes: Rc<Cell<usize>>,
    }

    impl Serialize for PayloadWithCustomErrorMessage {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom(&StreamingDisplay {
                writes: self.writes.clone(),
                chunks: 100_000,
            }))
        }
    }

    #[test]
    fn custom_serializer_errors_do_not_format_user_diagnostics() {
        let calls = Rc::new(Cell::new(0));
        let error = canonical_version_bound_digest(
            1,
            "archived-0.1",
            &PayloadWithCustomErrorMessage {
                writes: calls.clone(),
            },
        )
        .unwrap_err();

        assert_eq!(error.code, CircularErrorCode::InvalidParameter);
        assert!(error.message.contains("serialization failed"));
        assert_eq!(calls.get(), 0);
    }
}
