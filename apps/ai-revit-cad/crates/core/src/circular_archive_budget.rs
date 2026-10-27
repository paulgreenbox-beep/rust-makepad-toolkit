//! Generic accounting for typed circular snapshots retained anywhere in a
//! serializable document tree.
//!
//! The walk follows serde's view of the owning value, so snapshots nested in
//! operation history, commands, checkpoints, and other compounds are counted
//! without maintaining a parallel list of model fields.

use crate::circular_archive::{
    bounded_serialized_size, CIRCULAR_SNAPSHOT_MARKER, MAX_CIRCULAR_SNAPSHOT_BYTES,
};
use crate::CoreError;
use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
};
use std::fmt::{self, Display};

pub const MAX_CIRCULAR_RETAINED_BYTES: usize = 64 << 20;

const RETAINED_BYTES_SCOPE: &str = "retained circular snapshot bytes";
const RETAINED_OCCURRENCES_SCOPE: &str = "retained circular snapshot occurrences";
const RETAINED_PAYLOAD_SCOPE: &str = "retained command payload bytes";
const SERIALIZATION_ERROR_MESSAGE: &str = "archive retention serialization failed";
const PAYLOAD_ERROR_MESSAGE: &str = "payload_json archive field must serialize as a string";

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveRetentionUsage {
    pub snapshot_occurrences: usize,
    pub snapshot_bytes: usize,
    pub max_snapshot_bytes: usize,
    pub payload_json_bytes: usize,
}

impl ArchiveRetentionUsage {
    pub fn total_bytes(self) -> Result<usize, CoreError> {
        if self.snapshot_occurrences == 0 {
            return Ok(0);
        }
        self.snapshot_bytes
            .checked_add(self.payload_json_bytes)
            .ok_or(CoreError::CircularArchiveLimit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })
    }

    pub fn combine(self, other: Self) -> Result<Self, CoreError> {
        Ok(Self {
            snapshot_occurrences: self
                .snapshot_occurrences
                .checked_add(other.snapshot_occurrences)
                .ok_or(CoreError::CircularArchiveLimit {
                    scope: RETAINED_OCCURRENCES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                })?,
            snapshot_bytes: self
                .snapshot_bytes
                .checked_add(other.snapshot_bytes)
                .ok_or(CoreError::CircularArchiveLimit {
                    scope: RETAINED_BYTES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                })?,
            max_snapshot_bytes: self.max_snapshot_bytes.max(other.max_snapshot_bytes),
            payload_json_bytes: self
                .payload_json_bytes
                .checked_add(other.payload_json_bytes)
                .ok_or(CoreError::CircularArchiveLimit {
                    scope: RETAINED_PAYLOAD_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                })?,
        })
    }
}

/// Count each owned typed snapshot occurrence in a serializable value.
///
/// Snapshot wires are measured with the same bounded JSON byte counter used
/// by snapshot integrity checks, then treated as opaque so their inner fields
/// cannot be counted a second time. A `payload_json` field is charged by its
/// UTF-8 string length; values with that name in serialized maps are ordinary
/// map entries and do not receive the special accounting.
pub fn audit_circular_archive_retention<T: Serialize + ?Sized>(
    value: &T,
) -> Result<ArchiveRetentionUsage, CoreError> {
    let mut usage = ArchiveRetentionUsage::default();
    value
        .serialize(RetentionSerializer { usage: &mut usage })
        .map_err(RetentionError::into_core_error)?;
    Ok(usage)
}

struct RetentionSerializer<'a> {
    usage: &'a mut ArchiveRetentionUsage,
}

#[derive(Debug)]
enum RetentionError {
    Core(CoreError),
    InvalidPayload,
    Other,
}

impl RetentionError {
    fn into_core_error(self) -> CoreError {
        match self {
            Self::Core(error) => error,
            Self::InvalidPayload => CoreError::InvalidDocument(PAYLOAD_ERROR_MESSAGE.to_owned()),
            Self::Other => CoreError::InvalidDocument(SERIALIZATION_ERROR_MESSAGE.to_owned()),
        }
    }
}

impl fmt::Display for RetentionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(SERIALIZATION_ERROR_MESSAGE)
    }
}

impl std::error::Error for RetentionError {}

impl ser::Error for RetentionError {
    fn custom<T: Display>(_message: T) -> Self {
        // Do not format an arbitrary serializer error into an unbounded String.
        Self::Other
    }
}

impl<'a> RetentionSerializer<'a> {
    fn compound(self) -> RetentionCompound<'a> {
        RetentionCompound { usage: self.usage }
    }
}

impl<'a> serde::Serializer for RetentionSerializer<'a> {
    type Ok = ();
    type Error = RetentionError;
    type SerializeSeq = RetentionCompound<'a>;
    type SerializeTuple = RetentionCompound<'a>;
    type SerializeTupleStruct = RetentionCompound<'a>;
    type SerializeTupleVariant = RetentionCompound<'a>;
    type SerializeMap = RetentionCompound<'a>;
    type SerializeStruct = RetentionCompound<'a>;
    type SerializeStructVariant = RetentionCompound<'a>;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_str(self, _value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        if name == CIRCULAR_SNAPSHOT_MARKER {
            let bytes = bounded_serialized_size(value, MAX_CIRCULAR_SNAPSHOT_BYTES)
                .map_err(RetentionError::Core)?;
            let usage = self.usage;
            usage.snapshot_occurrences = usage
                .snapshot_occurrences
                .checked_add(1)
                .ok_or(RetentionError::Core(CoreError::CircularArchiveLimit {
                    scope: RETAINED_OCCURRENCES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                }))?;
            usage.snapshot_bytes = usage
                .snapshot_bytes
                .checked_add(bytes)
                .ok_or(RetentionError::Core(CoreError::CircularArchiveLimit {
                    scope: RETAINED_BYTES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                }))?;
            usage.max_snapshot_bytes = usage.max_snapshot_bytes.max(bytes);
            Ok(())
        } else {
            value.serialize(self)
        }
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(self.compound())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(self.compound())
    }

    fn collect_str<T: Display + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        // This scalar has no archive structure. Avoid calling Display::fmt.
        Ok(())
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

struct RetentionCompound<'a> {
    usage: &'a mut ArchiveRetentionUsage,
}

impl RetentionCompound<'_> {
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), RetentionError> {
        value.serialize(RetentionSerializer { usage: self.usage })
    }

}

impl SerializeSeq for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeTuple for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeTupleStruct for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeTupleVariant for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeMap for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        RetentionCompound::serialize_value(self, key)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        RetentionCompound::serialize_value(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeStruct for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if key == "payload_json" {
            let bytes = value
                .serialize(PayloadStringSerializer)
                .map_err(|_| RetentionError::InvalidPayload)?;
            self.add_payload_len_by_count(bytes)?;
            return Ok(());
        }
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl SerializeStructVariant for RetentionCompound<'_> {
    type Ok = ();
    type Error = RetentionError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if key == "payload_json" {
            let bytes = value
                .serialize(PayloadStringSerializer)
                .map_err(|_| RetentionError::InvalidPayload)?;
            self.add_payload_len_by_count(bytes)?;
            return Ok(());
        }
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl RetentionCompound<'_> {
    fn add_payload_len_by_count(&mut self, byte_count: usize) -> Result<(), RetentionError> {
        self.usage.payload_json_bytes = self
            .usage
            .payload_json_bytes
            .checked_add(byte_count)
            .ok_or(RetentionError::Core(CoreError::CircularArchiveLimit {
                scope: RETAINED_PAYLOAD_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            }))?;
        Ok(())
    }
}

/// A probe serializer for the one special struct field. It deliberately
/// accepts only a direct string and does not invoke arbitrary formatting.
struct PayloadStringSerializer;

impl serde::Serializer for PayloadStringSerializer {
    type Ok = usize;
    type Error = RetentionError;
    type SerializeSeq = ser::Impossible<usize, RetentionError>;
    type SerializeTuple = ser::Impossible<usize, RetentionError>;
    type SerializeTupleStruct = ser::Impossible<usize, RetentionError>;
    type SerializeTupleVariant = ser::Impossible<usize, RetentionError>;
    type SerializeMap = ser::Impossible<usize, RetentionError>;
    type SerializeStruct = ser::Impossible<usize, RetentionError>;
    type SerializeStructVariant = ser::Impossible<usize, RetentionError>;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(value.len())
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn collect_str<T: Display + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(RetentionError::InvalidPayload)
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{
        audit_circular_archive_retention, ArchiveRetentionUsage, MAX_CIRCULAR_RETAINED_BYTES,
    };
    use crate::{circular_archive::CIRCULAR_SNAPSHOT_MARKER, CoreError};
    use serde::{Serialize, Serializer};
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct SyntheticWire {
        value: &'static str,
    }

    struct SyntheticSnapshot;

    impl Serialize for SyntheticSnapshot {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_newtype_struct(
                CIRCULAR_SNAPSHOT_MARKER,
                &SyntheticWire { value: "same" },
            )
        }
    }

    #[derive(Serialize)]
    struct Nested<'a> {
        items: Vec<(u8, &'a SyntheticSnapshot)>,
    }

    #[test]
    fn counts_identical_marker_occurrences_without_digest_deduplication() {
        let value = [SyntheticSnapshot, SyntheticSnapshot, SyntheticSnapshot];
        let usage = audit_circular_archive_retention(&value).expect("scan synthetic markers");
        let one_wire = SyntheticWire { value: "same" };
        let one_bytes = crate::circular_archive::bounded_serialized_size(
            &one_wire,
            crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES,
        )
        .expect("measure synthetic wire");
        assert_eq!(usage.snapshot_occurrences, 3);
        assert_eq!(usage.snapshot_bytes, one_bytes * 3);
        assert_eq!(usage.max_snapshot_bytes, one_bytes);
    }

    #[test]
    fn finds_markers_inside_nested_compounds() {
        let one = SyntheticSnapshot;
        let value = Nested {
            items: vec![(7, &one), (9, &one)],
        };
        let usage = audit_circular_archive_retention(&value).expect("scan nested markers");
        assert_eq!(usage.snapshot_occurrences, 2);
    }

    #[derive(Serialize)]
    struct PayloadDocument {
        payload_json: String,
        snapshot: Option<SyntheticSnapshot>,
    }

    #[test]
    fn counts_payload_utf8_bytes_but_legacy_total_stays_zero() {
        let legacy = PayloadDocument {
            payload_json: "é🙂".to_owned(),
            snapshot: None,
        };
        let usage = audit_circular_archive_retention(&legacy).expect("scan legacy payload");
        assert_eq!(usage.payload_json_bytes, "é🙂".len());
        assert_eq!(usage.total_bytes().expect("legacy total"), 0);

        let with_snapshot = PayloadDocument {
            payload_json: "é🙂".to_owned(),
            snapshot: Some(SyntheticSnapshot),
        };
        let usage = audit_circular_archive_retention(&with_snapshot).expect("scan payload and marker");
        assert_eq!(usage.payload_json_bytes, "é🙂".len());
        assert!(usage.total_bytes().expect("total") > "é🙂".len());
    }

    #[test]
    fn map_key_named_payload_json_is_not_a_struct_payload_field() {
        let mut map = BTreeMap::new();
        map.insert("payload_json", "not a receipt field".to_owned());
        let usage = audit_circular_archive_retention(&map).expect("scan map");
        assert_eq!(usage.payload_json_bytes, 0);
    }

    #[test]
    fn combine_checks_overflow_and_preserves_maximum_snapshot_size() {
        let left = ArchiveRetentionUsage {
            snapshot_occurrences: 1,
            snapshot_bytes: 1,
            max_snapshot_bytes: 1,
            payload_json_bytes: 0,
        };
        let right = ArchiveRetentionUsage {
            snapshot_occurrences: 2,
            snapshot_bytes: 2,
            max_snapshot_bytes: 2,
            payload_json_bytes: 0,
        };
        let combined = left.combine(right).expect("combine usage");
        assert_eq!(combined.snapshot_occurrences, 3);
        assert_eq!(combined.snapshot_bytes, 3);
        assert_eq!(combined.max_snapshot_bytes, 2);

        let overflowing = ArchiveRetentionUsage {
            snapshot_occurrences: usize::MAX,
            ..ArchiveRetentionUsage::default()
        };
        assert!(matches!(
            overflowing.combine(left),
            Err(CoreError::CircularArchiveLimit {
                limit: MAX_CIRCULAR_RETAINED_BYTES,
                ..
            })
        ));
    }
}
