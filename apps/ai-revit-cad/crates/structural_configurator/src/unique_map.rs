//! Fail-closed deserialization for public maps keyed by semantic identity.
//! Serde's ordinary BTreeMap visitor replaces an earlier value on duplicate keys.

use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

const MAX_MAP_ENTRIES: usize = 100_000;

/// Deserialize a map while rejecting duplicate parsed keys and oversized maps.
///
/// The key check happens before reading its value, so a repeated identity cannot
/// silently replace a generated object or trigger unnecessary value allocation.
pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord + fmt::Display,
    V: Deserialize<'de>,
{
    struct UniqueMap<K, V>(PhantomData<(K, V)>);

    impl<'de, K, V> Visitor<'de> for UniqueMap<K, V>
    where
        K: Deserialize<'de> + Ord + fmt::Display,
        V: Deserialize<'de>,
    {
        type Value = BTreeMap<K, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map with distinct keys and at most 100000 entries")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some(key) = access.next_key::<K>()? {
                if values.contains_key(&key) {
                    return Err(A::Error::custom(format!("duplicate map key: {key}")));
                }
                if values.len() >= MAX_MAP_ENTRIES {
                    return Err(A::Error::custom(format!(
                        "map exceeds {MAX_MAP_ENTRIES} entries"
                    )));
                }
                let value = access.next_value::<V>()?;
                values.insert(key, value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueMap(PhantomData))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Checked {
        #[serde(deserialize_with = "deserialize")]
        values: BTreeMap<String, u32>,
    }

    #[test]
    fn duplicate_serialized_key_is_rejected_instead_of_replaced() {
        let raw = r#"{"values":{"node-a":1,"node-a":2}}"#;
        let error = serde_json::from_str::<Checked>(raw).expect_err("duplicate must fail");
        assert!(error.to_string().contains("duplicate map key: node-a"));
    }

    #[test]
    fn distinct_serialized_keys_are_preserved() {
        let raw = r#"{"values":{"node-a":1,"node-b":2}}"#;
        let checked = serde_json::from_str::<Checked>(raw).expect("distinct keys");
        assert_eq!(checked.values.len(), 2);
        assert_eq!(checked.values["node-a"], 1);
        assert_eq!(checked.values["node-b"], 2);
    }
}
