use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralIdError;

impl fmt::Display for StructuralIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ID must be a positive canonical decimal string within u64")
    }
}
impl std::error::Error for StructuralIdError {}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);
        impl $name {
            pub fn new(value: u64) -> Result<Self, StructuralIdError> {
                if value == 0 {
                    Err(StructuralIdError)
                } else {
                    Ok(Self(value))
                }
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }
        impl TryFrom<u64> for $name {
            type Error = StructuralIdError;
            fn try_from(value: u64) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl FromStr for $name {
            type Err = StructuralIdError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if value.is_empty()
                    || value.starts_with('0')
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(StructuralIdError);
                }
                Self::new(value.parse().map_err(|_| StructuralIdError)?)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }
    };
}

id_type!(StructuralNodeId);
id_type!(StructuralMemberId);
id_type!(StructuralMaterialId);
id_type!(StructuralSectionId);
id_type!(StructuralConnectionId);
id_type!(StructuralPanelId);
id_type!(StructuralLoadCaseId);
id_type!(StructuralLoadCombinationId);
id_type!(StructuralAnalysisRunId);

// Concise names retain a single identity type at every integration boundary.
pub type NodeId = StructuralNodeId;
pub type MemberId = StructuralMemberId;
pub type MaterialId = StructuralMaterialId;
pub type SectionId = StructuralSectionId;
pub type ConnectionId = StructuralConnectionId;
pub type PanelId = StructuralPanelId;
pub type LoadCaseId = StructuralLoadCaseId;
pub type LoadCombinationId = StructuralLoadCombinationId;
pub type AnalysisRunId = StructuralAnalysisRunId;
