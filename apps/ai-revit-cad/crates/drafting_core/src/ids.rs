use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::{fmt, str::FromStr};

/// Decimal string encoding protects all persistent u64 IDs from JS rounding.
pub(crate) mod decimal_u64 {
    use super::*;
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value = text.parse::<u64>().map_err(D::Error::custom)?;
        if text != value.to_string() {
            return Err(D::Error::custom("noncanonical decimal ID"));
        }
        Ok(value)
    }
}
macro_rules! typed_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(#[serde(with = "decimal_u64")] u64);
        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u64 {
                self.0
            }
            pub const fn value(self) -> u64 {
                self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}
typed_id!(DraftEntityId);
typed_id!(DrawingId);
typed_id!(LayerId);
typed_id!(ImportAssetId);
typed_id!(RenderPrimitiveId);
