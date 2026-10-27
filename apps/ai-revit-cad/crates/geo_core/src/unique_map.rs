use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use crate::model::MAX_RECORDS;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

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
            write!(formatter, "a map with distinct keys and at most {MAX_RECORDS} entries")
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
                if values.len() >= MAX_RECORDS {
                    return Err(A::Error::custom(format!(
                        "map exceeds {MAX_RECORDS} entries"
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
    use serde::Deserialize;
    use std::fmt::Write as _;

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(deserialize_with = "super::deserialize")]
        entries: BTreeMap<String, bool>,
    }

    fn map_json(entry_count: usize) -> String {
        let mut json = String::from("{\"entries\":{");
        for index in 0..entry_count {
            if index != 0 {
                json.push(',');
            }
            write!(json, "\"k{index}\":true").unwrap();
        }
        json.push_str("}}");
        json
    }

    #[test]
    fn each_map_is_bounded_by_max_records() {
        let at_limit: Envelope = serde_json::from_str(&map_json(MAX_RECORDS)).unwrap();
        assert_eq!(at_limit.entries.len(), MAX_RECORDS);

        assert!(serde_json::from_str::<Envelope>(&map_json(MAX_RECORDS + 1)).is_err());
    }
}


