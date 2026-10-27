use serde::{Deserialize, Deserializer, Serialize, Serializer};
/// Wire IDs/revisions/sequences/stamps never pass through JSON f64 or integer
/// coercion. Canonical decimal strings: zero or no leading zeroes, u64 range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct DecimalU64(pub u64);
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = DecimalU64;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("canonical u64 decimal string")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
                if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit())
                    || (s.len() > 1 && s.starts_with('0')) {
                    return Err(E::custom("noncanonical decimal u64"));
                }
                s.parse::<u64>().map(DecimalU64).map_err(E::custom)
            }
        }
        d.deserialize_str(Visitor)
    }
}
