use serde::{Deserialize, Serialize};

/// Explicitly supported physical units for DXF coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DxfUnitChoice {
    Meter,
    Millimeter,
    Centimeter,
    Inch,
    Foot,
}

pub type OutputUnit = DxfUnitChoice;

impl DxfUnitChoice {
    /// Parse explicit user-facing unit names. No missing or unitless value is inferred.
    pub fn parse_name(value: &str) -> Result<Self, String> {
        if value.len() > 32 {
            return Err("DXF unit name exceeds the 32-byte limit".into());
        }
        match value.trim().to_ascii_lowercase().as_str() {
            "m" | "meter" | "meters" | "metre" | "metres" => Ok(Self::Meter),
            "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => {
                Ok(Self::Millimeter)
            }
            "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" => {
                Ok(Self::Centimeter)
            }
            "in" | "inch" | "inches" => Ok(Self::Inch),
            "ft" | "foot" | "feet" => Ok(Self::Foot),
            _ => Err(format!("unsupported explicit DXF unit choice {value:?}")),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Meter => "metre",
            Self::Millimeter => "millimetre",
            Self::Centimeter => "centimetre",
            Self::Inch => "inch",
            Self::Foot => "foot",
        }
    }

    pub const fn meters_per_unit(self) -> f64 {
        match self {
            Self::Meter => 1.0,
            Self::Millimeter => 0.001,
            Self::Centimeter => 0.01,
            Self::Inch => 0.0254,
            Self::Foot => 0.3048,
        }
    }

    pub const fn insunits_code(self) -> i64 {
        match self {
            Self::Inch => 1,
            Self::Foot => 2,
            Self::Millimeter => 4,
            Self::Centimeter => 5,
            Self::Meter => 6,
        }
    }

    pub(crate) fn from_insunits(code: i64) -> Result<Option<Self>, String> {
        match code {
            0 => Ok(None),
            1 => Ok(Some(Self::Inch)),
            2 => Ok(Some(Self::Foot)),
            4 => Ok(Some(Self::Millimeter)),
            5 => Ok(Some(Self::Centimeter)),
            6 => Ok(Some(Self::Meter)),
            other => Err(format!(
                "DXF $INSUNITS code {other} is outside the supported unit set"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_explicit_unit_aliases() {
        assert_eq!(
            DxfUnitChoice::parse_name("metre").unwrap(),
            DxfUnitChoice::Meter
        );
        assert_eq!(
            DxfUnitChoice::parse_name("MM").unwrap(),
            DxfUnitChoice::Millimeter
        );
        assert_eq!(
            DxfUnitChoice::parse_name("inches").unwrap(),
            DxfUnitChoice::Inch
        );
        assert!(DxfUnitChoice::parse_name("unitless").is_err());
    }
}
