//! Allocation-free bounded serde traversal rejects non-finite scalars before
//! canonical JSON (which would otherwise encode them as null) and before clone.
use super::JobError;
use serde::{Serialize, Serializer, ser::*};
type Error = serde_json::Error;
type Result<T> = std::result::Result<T, Error>;
fn error(message: &str) -> Error {
    <Error as serde::ser::Error>::custom(message)
}
struct Audit {
    remaining: usize,
    budget_exceeded: bool,
}
impl Audit {
    fn count(&mut self, n: usize) -> Result<()> {
        if n > self.remaining {
            self.budget_exceeded = true;
            return Err(error("input traversal limit"));
        }
        self.remaining -= n;
        Ok(())
    }
}
pub(super) fn audit_finite(
    value: &impl Serialize,
    limit: usize,
) -> std::result::Result<(), JobError> {
    let mut audit = Audit {
        remaining: limit,
        budget_exceeded: false,
    };
    value.serialize(&mut audit).map_err(|_| {
        if audit.budget_exceeded {
            JobError::InputBudget
        } else {
            JobError::InvalidInput
        }
    })
}
macro_rules! scalar {
    ($($method:ident($ty:ty)),*) => { $(fn $method(self, _value: $ty) -> Result<()> { self.count(1) })* };
}
impl<'a> Serializer for &'a mut Audit {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_i128(i128),
        serialize_u128(u128),
        serialize_char(char)
    );
    fn serialize_f32(self, value: f32) -> Result<()> {
        if !value.is_finite() {
            return Err(error("non-finite input"));
        }
        self.count(1)
    }
    fn serialize_f64(self, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(error("non-finite input"));
        }
        self.count(1)
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.count(value.len().saturating_add(1))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        self.count(value.len().saturating_add(1))
    }
    fn serialize_none(self) -> Result<()> {
        self.count(1)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        self.count(1)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.count(1)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.count(variant.len().saturating_add(1))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        self.count(variant.len())?;
        value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self> {
        self.count(len.unwrap_or(0).saturating_add(1))?;
        Ok(self)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self> {
        self.count(len.saturating_add(1))?;
        Ok(self)
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Self> {
        self.serialize_tuple(len)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self> {
        self.count(variant.len())?;
        self.serialize_tuple(len)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self> {
        self.serialize_seq(len)
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self> {
        self.serialize_tuple(len)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self> {
        self.count(variant.len())?;
        self.serialize_tuple(len)
    }
}
macro_rules! compound {
    ($trait:ident, $method:ident) => {
        impl $trait for &mut Audit {
            type Ok = ();
            type Error = Error;
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
                value.serialize(&mut **self)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}
compound!(SerializeSeq, serialize_element);
compound!(SerializeTuple, serialize_element);
compound!(SerializeTupleStruct, serialize_field);
compound!(SerializeTupleVariant, serialize_field);
impl SerializeMap for &mut Audit {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        value.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}
impl SerializeStruct for &mut Audit {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.count(key.len())?;
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}
impl SerializeStructVariant for &mut Audit {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.count(key.len())?;
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}
