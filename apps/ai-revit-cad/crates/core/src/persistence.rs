//! Exact native2..7 feature envelopes, including retained authority.
use crate::*;
use std::cell::Cell;
use std::fmt::Write as _;

pub const GEO_FOUNDATION_NATIVE_FORMAT_VERSION: u16 = 7;
pub const GEO_FOUNDATION_FEATURE: &str = "geo_foundation_v1";

const MAX_IGNORED_FIELD_PATHS: usize = 8;
const MAX_IGNORED_FIELD_PATH_BYTES: usize = 256;
const MAX_NATIVE_SHAPE_DEPTH: usize = 128;

thread_local! {
    /// Every public native reader is strict through nested raw JSON boundaries
    /// before migration. Thread-local scope is restored even during unwinding.
    static STRICT_NATIVE_AUTHORITY: Cell<bool> = const { Cell::new(false) };
}

struct NativeAuthorityDecodeScope {
    previous: bool,
}
impl NativeAuthorityDecodeScope {
    fn enter() -> Self {
        Self {
            previous: STRICT_NATIVE_AUTHORITY.with(|strict| strict.replace(true)),
        }
    }
}
impl Drop for NativeAuthorityDecodeScope {
    fn drop(&mut self) {
        STRICT_NATIVE_AUTHORITY.with(|strict| strict.set(self.previous));
    }
}

/// A callback must not retain arbitrary field names or a path for every unknown
/// key. Formatting writes directly into this bounded UTF-8 diagnostic buffer.
struct BoundedIgnoredPath(String);
impl fmt::Write for BoundedIgnoredPath {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let mut end = (MAX_IGNORED_FIELD_PATH_BYTES - self.0.len()).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push_str(&value[..end]);
        Ok(())
    }
}

/// Decode typed authority, including a nested `RawValue` reopened by another
/// Deserialize implementation. Maps and `serde_json::Value` retain their keys;
/// public native readers reject ignored schema fields before any migration.
pub(super) fn decode_authority<'de, T>(source: &'de str) -> Result<T, serde_json::Error>
where
    T: Deserialize<'de>,
{
    if !STRICT_NATIVE_AUTHORITY.with(Cell::get) {
        return serde_json::from_str(source);
    }
    let mut deserializer = serde_json::Deserializer::from_str(source);
    let mut ignored_count = 0usize;
    let mut ignored_paths = Vec::new();
    let value = serde_ignored::deserialize(&mut deserializer, |path| {
        ignored_count = ignored_count.saturating_add(1);
        if ignored_paths.len() < MAX_IGNORED_FIELD_PATHS {
            let mut bounded = BoundedIgnoredPath(String::with_capacity(MAX_IGNORED_FIELD_PATH_BYTES));
            let _ = write!(&mut bounded, "{path}");
            ignored_paths.push(bounded.0);
        }
    })?;
    // serde_ignored follows Deserialize and does not consume trailing JSON.
    deserializer.end()?;
    if ignored_count != 0 {
        return Err(<serde_json::Error as serde::de::Error>::custom(format!(
            "Native authority contains {ignored_count} unsupported field(s): {}; legacy inputs are rejected before migration to prevent field loss",
            ignored_paths.join(", ")
        )));
    }
    Ok(value)
}

/// Stream the original JSON before any typed map or Value can replace an
/// earlier object member. String decoding makes escaped-equivalent names the
/// same key. Values are consumed without retaining a second JSON tree; each
/// object retains only its unique decoded keys until that object closes.
/// The owning reader's source admission and serde_json's recursion bound stay
/// in force. Explicit metadata values receive this same lossless-key check.
fn reject_duplicate_native_object_keys(source: &str) -> Result<(), serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(source);
    serde::de::DeserializeSeed::deserialize(NativeKeyTree, &mut deserializer)?;
    deserializer.end()
}

struct NativeKeyTree;
impl<'de> serde::de::DeserializeSeed<'de> for NativeKeyTree {
    type Value = ();
    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where D: serde::Deserializer<'de> {
        deserializer.deserialize_any(NativeKeyTreeVisitor)
    }
}

struct NativeKeyTreeVisitor;
impl<'de> serde::de::Visitor<'de> for NativeKeyTreeVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native JSON with unique decoded object keys")
    }
    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where A: serde::de::MapAccess<'de> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if keys.contains(&key) {
                let mut diagnostic = BoundedIgnoredPath(String::with_capacity(MAX_IGNORED_FIELD_PATH_BYTES));
                let _ = write!(&mut diagnostic, "{key}");
                return Err(<A::Error as serde::de::Error>::custom(format!(
                    "Native authority contains duplicate decoded object key '{}'; rejected before typed decoding and migration to prevent field loss",
                    diagnostic.0
                )));
            }
            keys.insert(key);
            map.next_value_seed(NativeKeyTree)?;
        }
        Ok(())
    }
    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where A: serde::de::SeqAccess<'de> {
        while sequence.next_element_seed(NativeKeyTree)?.is_some() {}
        Ok(())
    }
    fn visit_bool<E>(self, _: bool) -> Result<(), E> { Ok(()) }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> { Ok(()) }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> { Ok(()) }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> { Ok(()) }
    fn visit_str<E>(self, _: &str) -> Result<(), E> { Ok(()) }
    fn visit_unit<E>(self) -> Result<(), E> { Ok(()) }
}

/// Internally tagged Serde enums can buffer their variant fields before
/// decoding them through a `ContentDeserializer`; ignored fields then bypass
/// an outer serde_ignored callback. Native reading requires every input
/// key to survive typed serialization. This is a key-shape audit, not a value
/// comparison or a calculation. The canonical writer is always admitted;
/// handwritten explicit skipped null/empty fields and aliases are rejected.
fn audit_native_authority_shape<T>(source: &str, authority: &T) -> Result<(), serde_json::Error>
where
    T: Serialize + ?Sized,
{
    let invalid = |message: String| <serde_json::Error as serde::de::Error>::custom(message);
    // The owning reader has admitted the source using its version's existing
    // limits. Native2..4 historically have no Native5 64 MiB hard source cap;
    // do not add one merely by auditing their lossless migration. Allow bounded
    // default-field expansion while preserving that existing source admission.
    let projection_limit = source.len().checked_add(65536)
        .ok_or_else(|| invalid("Native canonical audit source-size overflow before migration".into()))?
        .max(MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES);
    // Bound the second owning JSON projection before allocating it. The raw
    // input is already admitted by the native source cap. Neither tree is
    // cloned; traversal borrows nodes and retains one bounded diagnostic path.
    circular_archive::bounded_serialized_size(authority, projection_limit)
        .map_err(|_| invalid("Native canonical audit projection exceeds its bounded schema".into()))?;
    let source_shape: serde_json::Value = serde_json::from_str(source)?;
    let authority_shape = serde_json::to_value(authority)?;
    let mut path = BoundedIgnoredPath(String::with_capacity(MAX_IGNORED_FIELD_PATH_BYTES));
    let _ = path.write_str("$");
    compare_native_authority_key_shapes(&source_shape, &authority_shape, &mut path, 0)
}

fn compare_native_authority_key_shapes(
    source: &serde_json::Value,
    authority: &serde_json::Value,
    path: &mut BoundedIgnoredPath,
    depth: usize,
) -> Result<(), serde_json::Error> {
    let invalid = |reason: &str, path: &str| {
        <serde_json::Error as serde::de::Error>::custom(format!(
            "Native canonical authority {reason} at {path}; unsupported fields and explicit skipped null/empty fields or aliases are rejected before migration to prevent field loss"
        ))
    };
    if depth > MAX_NATIVE_SHAPE_DEPTH {
        return Err(invalid("exceeds the bounded key-shape depth", &path.0));
    }
    match source {
        serde_json::Value::Object(source_fields) => {
            let Some(authority_fields) = authority.as_object() else {
                return Err(invalid("does not retain an input object", &path.0));
            };
            for (key, value) in source_fields {
                let previous_path_bytes = path.0.len();
                let _ = write!(path, ".{key}");
                let Some(retained) = authority_fields.get(key) else {
                    return Err(invalid("does not retain an input field", &path.0));
                };
                compare_native_authority_key_shapes(value, retained, path, depth + 1)?;
                path.0.truncate(previous_path_bytes);
            }
        }
        serde_json::Value::Array(source_items) => {
            let Some(authority_items) = authority.as_array() else {
                return Err(invalid("does not retain an input array", &path.0));
            };
            if source_items.len() != authority_items.len() {
                return Err(invalid("changes an input array length", &path.0));
            }
            for (index, (value, retained)) in source_items.iter().zip(authority_items).enumerate() {
                let previous_path_bytes = path.0.len();
                let _ = write!(path, "[{index}]");
                compare_native_authority_key_shapes(value, retained, path, depth + 1)?;
                path.0.truncate(previous_path_bytes);
            }
        }
        // Scalar values are validated by their typed schema and integrity
        // checks. This supplementary pass checks only retained field shape.
        _ => {}
    }
    Ok(())
}

/// Streaming header check before open allocates the native source string.
/// Unknown values are consumed without retaining them; old versions retain
/// their existing source-size behavior and circular archive admission.
pub(super) fn preflight_native5_reader(reader: impl std::io::Read, source_bytes: usize) -> Result<(), CoreError> {
    if source_bytes <= MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES { return Ok(()); }
    #[derive(Deserialize)]
    struct Header { format_version: u16 }
    let header: Header = serde_json::from_reader(reader)?;
    if header.format_version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION {
        return Err(CoreError::InvalidDocument("native5 retained source exceeds its 64 MiB bounded schema".into()));
    }
    Ok(())
}

fn required_features(version: u16, archive: bool) -> Option<Vec<&'static str>> {
    if archive && version < CIRCULAR_NATIVE_FORMAT_VERSION { return None; }
    match version {
        NATIVE_FORMAT_VERSION => Some(vec!["typed_drafting_v1"]),
        PLANAR_NATIVE_FORMAT_VERSION => Some(vec!["typed_drafting_v1", "geotech_slope_v1"]),
        CIRCULAR_NATIVE_FORMAT_VERSION | FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION | STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION | GEO_FOUNDATION_NATIVE_FORMAT_VERSION => {
            let mut features = vec!["typed_drafting_v1", "geotech_slope_v1", "geotech_circular_v1"];
            if archive { features.push("geotech_circular_archive_v1"); }
            if version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION { features.push(FOUNDATION_STRUCTURAL_FEATURE); }
            if version >= STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION { features.push(STRUCTURAL_CONFIGURATOR_FEATURE); }
            if version == GEO_FOUNDATION_NATIVE_FORMAT_VERSION { features.push(GEO_FOUNDATION_FEATURE); }
            Some(features)
        },
        _ => None,
    }
}
#[derive(Serialize)]
pub(super) struct NativeEnvelope<'a> {
    format_id: &'static str,
    format_version: u16,
    required_features: Vec<&'static str>,
    document: &'a Document,
}
impl<'a> NativeEnvelope<'a> {
    pub(super) fn new(document: &'a Document) -> Result<Self, CoreError> {
        let archive = document.circular_archive_retention_usage()?.snapshot_occurrences != 0;
        Ok(Self {
            format_id: NATIVE_FORMAT_ID,
            format_version: document.format_version,
            required_features: required_features(document.format_version, archive)
                .ok_or(CoreError::UnsupportedFormatVersion(document.format_version))?,
            document,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<'a> {
    format_id: String,
    format_version: u16,
    required_features: Vec<String>,
    #[serde(borrow)]
    document: &'a serde_json::value::RawValue,
}
pub(super) fn decode_native_document(source: &str) -> Result<(Document, bool), CoreError> {
    let archive_probe = circular_archive_preflight::preflight_circular_archive_native(source)?;
    #[derive(Deserialize)]
    struct Probe {
        document: Option<serde::de::IgnoredAny>,
        format_version: u16,
    }
    let probe: Probe = serde_json::from_str(source)?;
    if probe.format_version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION && source.len() > MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES {
        return Err(CoreError::InvalidDocument("native5 retained source exceeds its 64 MiB bounded schema".into()));
    }
    // Run on the admitted original bytes before Envelope/Document/Value maps
    // can discard a duplicate, including an ignored tagged variant in its first
    // value. Bare legacy input uses this exact same pre-migration path.
    reject_duplicate_native_object_keys(source)?;
    let enveloped = probe.document.is_some();
    let mut document = if enveloped {
        let envelope: Envelope<'_> = serde_json::from_str(source)?;
        if envelope.format_version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION && source.len() > MAX_FOUNDATION_STRUCTURAL_NATIVE_BYTES {
            return Err(CoreError::InvalidDocument("native5 retained source exceeds its 64 MiB bounded schema".into()));
        }
        if envelope.format_id != NATIVE_FORMAT_ID {
            return Err(CoreError::WrongFormat(envelope.format_id));
        }
        let expected = required_features(envelope.format_version, archive_probe.snapshot_occurrences != 0)
            .ok_or(CoreError::UnsupportedFormatVersion(envelope.format_version))?;
        if envelope.required_features != expected {
            return Err(CoreError::InvalidDocument(
                "unsupported required project features".into(),
            ));
        }
        let document: Document = {
            let _strict_scope = NativeAuthorityDecodeScope::enter();
            let document: Document = decode_authority(envelope.document.get())?;
            audit_native_authority_shape(envelope.document.get(), &document)?;
            document
        };
        if document.format_id != NATIVE_FORMAT_ID
            || document.format_version != envelope.format_version
        {
            return Err(CoreError::InvalidDocument(
                "envelope/document version mismatch".into(),
            ));
        }
        document
    } else {
        let _strict_scope = NativeAuthorityDecodeScope::enter();
        let document: Document = decode_authority(source)?;
        audit_native_authority_shape(source, &document)?;
        document
    };
    // Native4 introduces a new exact feature contract. Its public writer always
    // emits the envelope; legacy bare native1/2/3 input remains supported.
    if !enveloped && document.format_version >= CIRCULAR_NATIVE_FORMAT_VERSION {
        return Err(CoreError::InvalidDocument(
            "Native4/5 requires its exact required_features envelope".into(),
        ));
    }
    let legacy = match (document.format_id.as_str(), document.format_version) {
        ("ai-revit-provisional-json", 1) => {
            // No old project could contain typed drafting collections. Reject a
            // forged downgrade instead of guessing or dropping these records.
            if !document.drafting.drawings.is_empty()
                || !document.drafting.views.is_empty()
                || document.has_engineering_history()
                || document.has_foundation_structural_history()
                || document.has_configurator_history()
                || document.has_geo_foundation_history()
            {
                return Err(CoreError::InvalidDocument(
                    "legacy format contains drafting data".into(),
                ));
            }
            document.format_id = NATIVE_FORMAT_ID.into();
            document.format_version = NATIVE_FORMAT_VERSION;
            true
        }
        (NATIVE_FORMAT_ID, NATIVE_FORMAT_VERSION) => false,
        (NATIVE_FORMAT_ID, 3) => false,
        (NATIVE_FORMAT_ID, 4) => false,
        (NATIVE_FORMAT_ID, FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION) => false,
        (NATIVE_FORMAT_ID, STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION) => false,
        (NATIVE_FORMAT_ID, GEO_FOUNDATION_NATIVE_FORMAT_VERSION) => false,
        (NATIVE_FORMAT_ID, version) => return Err(CoreError::UnsupportedFormatVersion(version)),
        _ => return Err(CoreError::WrongFormat(document.format_id.clone())),
    };
    if document.has_circular_engineering_history()
        && document.format_version < CIRCULAR_NATIVE_FORMAT_VERSION
    {
        return Err(CoreError::InvalidDocument(
            "Ground/circular records or retained history require native4 and geotech_circular_v1; downgraded documents are rejected".into(),
        ));
    }
    if document.has_engineering_history() && document.format_version < PLANAR_NATIVE_FORMAT_VERSION {
        return Err(CoreError::InvalidDocument("Engineering records/history require at least format3 and geotech_slope_v1; downgraded documents are rejected".into()));
    }
    if document.has_foundation_structural_history() && document.format_version < FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION {
        return Err(CoreError::InvalidDocument("structural/foundation records or retained history require native5 and foundation_structural_v1; downgraded documents are rejected".into()));
    }
    if document.has_configurator_history() && document.format_version < STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION { return Err(CoreError::InvalidDocument("configurator records/retained history require at least native6 and structural_configurator_v1; downgraded documents are rejected".into())); }
    if document.has_geo_foundation_history() && document.format_version != GEO_FOUNDATION_NATIVE_FORMAT_VERSION {
        return Err(CoreError::InvalidDocument("GEO/foundation records or retained history require native7 and geo_foundation_v1; downgraded documents are rejected".into()));
    }
    Ok((document, legacy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_duplicate_key_guard_rejects_both_orders_escaped_and_nested_authority() {
        #[derive(Serialize, Deserialize)]
        #[serde(tag = "kind")]
        enum BufferedLegacy { Entry { known: u64 } }
        let overwritten_unknown = r#"{"same":{"kind":"Entry","known":1,"future_variant_field":true},"same":{"kind":"Entry","known":2}}"#;
        {
            let _strict = NativeAuthorityDecodeScope::enter();
            // Neither the ignored-field callback nor a Value-based key audit
            // can recover the earlier member once BTreeMap/Value replaced it.
            let overwritten: BTreeMap<String, BufferedLegacy> = decode_authority(overwritten_unknown).unwrap();
            audit_native_authority_shape(overwritten_unknown, &overwritten).unwrap();
        }
        assert!(reject_duplicate_native_object_keys(overwritten_unknown).is_err());
        for source in [
            r#"{"records":{"same":{"kind":"Entry","known":1,"future_variant_field":true},"same":{"kind":"Entry","known":2}}}"#,
            r#"{"records":{"same":{"kind":"Entry","known":2},"same":{"kind":"Entry","known":1,"future_variant_field":true}}}"#,
            r#"{"records":{"\u0073ame":{"known":1},"same":{"known":2}}}"#,
            r#"{"records":{"same":{"known":1},"\u0073ame":{"known":2}}}"#,
            r#"{"metadata":{"deep":[{"\u0433рунт":1,"грунт":2}]}}"#,
            r#"{"metadata":{"deep":[{"грунт":2,"\u0433рунт":1}]}}"#,
        ] {
            let error = reject_duplicate_native_object_keys(source).unwrap_err().to_string();
            assert!(error.contains("duplicate decoded object key"), "{error}");
        }
    }

    #[test]
    fn public_native_readers_reject_duplicate_maps_before_typed_decoding_and_migration() {
        for version in 1..=7 {
            let mut document = Document::new(format!("Duplicate-key Native{version}")).unwrap();
            document.format_version = version;
            if version == 1 { document.format_id = "ai-revit-provisional-json".into(); }
            let sources = if version <= 3 {
                let bare = serde_json::to_string(&document).unwrap();
                if version == 1 { vec![bare] }
                else { vec![bare, document.to_json_string().unwrap()] }
            } else { vec![document.to_json_string().unwrap()] };
            for source in sources {
                Document::from_json_str(&source).unwrap();
                let compact = serde_json::to_string(&serde_json::from_str::<serde_json::Value>(&source).unwrap()).unwrap();
                for entries in [r#""1":null,"\u0031":null"#, r#""\u0031":null,"1":null"#] {
                    let duplicate = compact.replacen("\"operations\":{}", &format!("\"operations\":{{{entries}}}"), 1);
                    assert_ne!(duplicate, compact, "fixture requires an empty operations authority map");
                    let error = Document::from_json_str(&duplicate).unwrap_err().to_string();
                    assert!(error.contains("duplicate decoded object key"), "Native{version}: {error}");
                }
            }
        }
    }

    #[test]
    fn native_duplicate_key_guard_retains_unique_metadata_and_bounds_failures() {
        let metadata = r#"{"metadata":{"source":{"unit":"kPa","value":1.5},"flags":[true,true,null],"text":"{\"same\":1,\"same\":2}","\u0410":"escaped value","B":"other"}}"#;
        reject_duplicate_native_object_keys(metadata).unwrap();
        let value: serde_json::Value = serde_json::from_str(metadata).unwrap();
        assert_eq!(value["metadata"]["text"], "{\"same\":1,\"same\":2}");
        assert_eq!(value["metadata"]["А"], "escaped value");
        assert!(reject_duplicate_native_object_keys("{} {}").is_err());
        assert!(reject_duplicate_native_object_keys("{\"broken\":").is_err());
        let deep = format!("{}0{}", "[".repeat(256), "]".repeat(256));
        assert!(reject_duplicate_native_object_keys(&deep).is_err());
        let key = serde_json::to_string(&"г".repeat(3000)).unwrap();
        let duplicate = format!("{{{key}:0,{key}:1}}");
        let error = reject_duplicate_native_object_keys(&duplicate).unwrap_err().to_string();
        assert!(error.contains("duplicate decoded object key"));
        assert!(error.len() < 1024, "duplicate diagnostic must not retain an unbounded key");
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct RetainedAuthority {
        known: u64,
        metadata: BTreeMap<String, serde_json::Value>,
    }

    #[derive(Debug)]
    struct ReopenedAuthority(RetainedAuthority);
    impl<'de> Deserialize<'de> for ReopenedAuthority {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let raw = <&'de serde_json::value::RawValue>::deserialize(deserializer)?;
            decode_authority(raw.get())
                .map(Self)
                .map_err(<D::Error as serde::de::Error>::custom)
        }
    }

    #[derive(Debug, Deserialize)]
    struct RetainedContainer {
        revision_states: BTreeMap<String, ReopenedAuthority>,
    }

    #[test]
    fn native7_strict_decode_rejects_nested_raw_authority_unknown_fields() {
        let source = r#"{"revision_states":{"1":{"known":1,"metadata":{},"future_authority":true}}}"#;
        // The internal helper is ordinary serde outside an owning reader
        // scope. Every public native reader now enters that strict scope.
        assert!(decode_authority::<RetainedContainer>(source).is_ok());
        let _strict = NativeAuthorityDecodeScope::enter();
        let error = decode_authority::<RetainedContainer>(source).unwrap_err();
        assert!(error.to_string().contains("future_authority"));
    }

    #[test]
    fn native7_strict_decode_preserves_explicit_metadata_maps_and_values() {
        let source = r#"{"revision_states":{"1":{"known":7,"metadata":{"future":{"deep":[1,{"more":true}]}}}}}"#;
        let _strict = NativeAuthorityDecodeScope::enter();
        let decoded: RetainedContainer = decode_authority(source).unwrap();
        let retained = &decoded.revision_states["1"].0;
        assert_eq!(retained.known, 7);
        assert_eq!(retained.metadata["future"]["deep"][1]["more"], true);
    }

    #[test]
    fn native7_strict_scope_restores_prior_context_after_nested_scope_and_panic() {
        assert!(!STRICT_NATIVE_AUTHORITY.with(Cell::get));
        {
            let _outer = NativeAuthorityDecodeScope::enter();
            {
                let _inner = NativeAuthorityDecodeScope::enter();
                assert!(STRICT_NATIVE_AUTHORITY.with(Cell::get));
            }
            assert!(STRICT_NATIVE_AUTHORITY.with(Cell::get));
        }
        assert!(!STRICT_NATIVE_AUTHORITY.with(Cell::get));
        let caught = std::panic::catch_unwind(|| {
            let _strict = NativeAuthorityDecodeScope::enter();
            panic!("synthetic decoder unwind");
        });
        assert!(caught.is_err());
        assert!(!STRICT_NATIVE_AUTHORITY.with(Cell::get));
        assert!(decode_authority::<RetainedAuthority>(
            r#"{"known":1,"metadata":{},"legacy_extra":1}"#
        ).is_ok());
    }

    #[test]
    fn native7_strict_decode_rejects_trailing_json_and_bounds_unknown_paths() {
        let _strict = NativeAuthorityDecodeScope::enter();
        assert!(decode_authority::<RetainedAuthority>(
            r#"{"known":1,"metadata":{}} {"extra_document":true}"#
        ).is_err());
        let mut value = serde_json::json!({"known": 1, "metadata": {}});
        for index in 0..32 {
            value.as_object_mut().unwrap().insert(
                format!("{}-{index}", "界".repeat(1000)),
                serde_json::Value::Bool(true),
            );
        }
        let source = value.to_string();
        let error = decode_authority::<RetainedAuthority>(&source).unwrap_err().to_string();
        assert!(error.contains("32 unsupported field(s)"));
        assert!(error.len() < 4096);
    }

    #[test]
    fn native7_feature_envelope_is_exact_and_cumulative_with_native6() {
        let native6 = required_features(STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION, false).unwrap();
        let native7 = required_features(GEO_FOUNDATION_NATIVE_FORMAT_VERSION, false).unwrap();
        let mut expected = native6;
        expected.push(GEO_FOUNDATION_FEATURE);
        assert_eq!(native7, expected);
        let archived = required_features(GEO_FOUNDATION_NATIVE_FORMAT_VERSION, true).unwrap();
        assert!(archived.contains(&"geotech_circular_archive_v1"));
        assert_eq!(archived.last(), Some(&GEO_FOUNDATION_FEATURE));
    }

    #[test]
    fn native7_canonical_shape_closes_internally_tagged_variant_callback_gap() {
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "kind")]
        enum LegacyTagged {
            Entry { known: u64 },
        }
        let source = r#"{"kind":"Entry","known":1,"future_variant_field":{"more":true}}"#;
        let _strict = NativeAuthorityDecodeScope::enter();
        // Internally tagged variant fields are decoded from Serde's buffered
        // ContentDeserializer, beyond the outer ignored-field callback.
        let decoded: LegacyTagged = decode_authority(source).unwrap();
        let error = audit_native_authority_shape(source, &decoded).unwrap_err();
        assert!(error.to_string().contains("future_variant_field"));
    }

    #[test]
    fn native7_canonical_shape_retains_metadata_and_rejects_noncanonical_skipped_fields() {
        let source = r#"{"known":1,"metadata":{"future":{"arbitrary":[1,{"key":true}]}}}"#;
        let retained: RetainedAuthority = serde_json::from_str(source).unwrap();
        audit_native_authority_shape(source, &retained).unwrap();

        #[derive(Serialize, Deserialize)]
        struct CanonicalOptional {
            #[serde(alias = "old_known")]
            known: u64,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            optional: Option<String>,
        }
        for source in [
            r#"{"known":1,"optional":null}"#,
            r#"{"old_known":1}"#,
        ] {
            let decoded: CanonicalOptional = serde_json::from_str(source).unwrap();
            let error = audit_native_authority_shape(source, &decoded).unwrap_err();
            assert!(error.to_string().contains("canonical authority"));
        }
    }

    #[test]
    fn native7_canonical_shape_selfwriter_document_roundtrip() {
        let mut document = Document::new("Native7 canonical writer").unwrap();
        document.format_version = GEO_FOUNDATION_NATIVE_FORMAT_VERSION;
        let source = document.to_json_string().unwrap();
        let reopened = Document::from_json_str(&source).unwrap();
        assert_eq!(reopened.document_uuid, document.document_uuid);
        assert_eq!(reopened.to_json_string().unwrap(), source);
    }

    #[test]
    fn native7_rejects_unknown_tagged_field_in_actual_retained_legacy_command_wire() {
        fn inject_provenance_field(value: &mut serde_json::Value) -> bool {
            match value {
                serde_json::Value::Object(fields) => {
                    if fields.get("kind").and_then(serde_json::Value::as_str) == Some("SYNTHETIC")
                        && fields.contains_key("note")
                    {
                        fields.insert("future_provenance_authority".into(), true.into());
                        return true;
                    }
                    fields.values_mut().any(inject_provenance_field)
                }
                serde_json::Value::Array(values) => values.iter_mut().any(inject_provenance_field),
                _ => false,
            }
        }
        let commands = vec![Command::StructuralConfigurator(
            StructuralConfiguratorCommand::UpdateBuildingAndRegenerate {
                building: structural_configurator::fixture(structural_configurator::FixtureKind::IndependentJoint),
                foundation_groups: vec![],
            },
        )];
        let payload_json = serde_json::to_string(&commands).unwrap();
        let mut retained_commands = serde_json::to_value(&commands).unwrap();
        assert!(inject_provenance_field(&mut retained_commands));
        let mut document = Document::new("Retained legacy tagged wire fixture").unwrap();
        document.format_version = STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION;
        let mut envelope = serde_json::to_value(NativeEnvelope::new(&document).unwrap()).unwrap();
        // This is a deserialization-boundary fixture, not an accepted operation
        // or receipt. No generation or solver call is needed to exercise the
        // actual StoredCommand -> Command -> BuildingConfig -> Provenance wire.
        envelope["document"]["command_receipts"] = serde_json::json!({
            "retained-source": {
                "payload_json": payload_json,
                "recorded_commands": retained_commands,
                "receipt": {
                    "command_id": "retained-source", "revision": 1,
                    "operation_ids": [], "created_elements": []
                }
            }
        });
        let legacy_error = decode_native_document(&envelope.to_string()).unwrap_err().to_string();
        assert!(legacy_error.contains("Native canonical authority"));
        assert!(legacy_error.contains("before migration"));
        envelope["format_version"] = GEO_FOUNDATION_NATIVE_FORMAT_VERSION.into();
        envelope["document"]["format_version"] = GEO_FOUNDATION_NATIVE_FORMAT_VERSION.into();
        envelope["required_features"] = serde_json::to_value(
            required_features(GEO_FOUNDATION_NATIVE_FORMAT_VERSION, false).unwrap()
        ).unwrap();
        let error = decode_native_document(&envelope.to_string()).unwrap_err().to_string();
        assert!(error.contains("Native canonical authority"));
    }

    #[test]
    fn native_public_writers_2_through_6_open_and_reject_unknown_authority_before_migration() {
        for version in 2..=STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION {
            let mut document = Document::new(format!("Known native{version} writer fixture")).unwrap();
            document.format_version = version;
            let source = document.to_json_string().unwrap();
            let reopened = Document::from_json_str(&source).unwrap();
            assert_eq!(reopened.document_uuid, document.document_uuid);
            assert_eq!(reopened.format_version, version);
            let mut unknown: serde_json::Value = serde_json::from_str(&source).unwrap();
            unknown["document"]["future_legacy_authority"] = serde_json::json!({"must_not_be_lost": true});
            let error = Document::from_json_str(&unknown.to_string()).unwrap_err().to_string();
            assert!(error.contains("future_legacy_authority"), "native{version}: {error}");
            assert!(error.contains("before migration"), "native{version}: {error}");
        }
    }

    #[test]
    fn bare_native1_known_writer_fields_migrate_but_unknown_authority_is_rejected_first() {
        let document = Document::new("Known bare native1 fields").unwrap();
        let mut bare = serde_json::to_value(&document).unwrap();
        bare["format_id"] = "ai-revit-provisional-json".into();
        bare["format_version"] = 1.into();
        let reopened = Document::from_json_str(&bare.to_string()).unwrap();
        assert_eq!(reopened.document_uuid, document.document_uuid);
        assert_eq!(reopened.format_version, NATIVE_FORMAT_VERSION);
        bare["future_legacy_authority"] = serde_json::json!({"must_not_be_lost": true});
        let error = Document::from_json_str(&bare.to_string()).unwrap_err().to_string();
        assert!(error.contains("future_legacy_authority"));
        assert!(error.contains("before migration"));
    }

    #[test]
    fn native7_canonical_shape_bounds_path_and_depth_without_cloning_nodes() {
        let key = "界".repeat(1000);
        let mut source_fields = serde_json::Map::new();
        source_fields.insert(key, true.into());
        let source = serde_json::Value::Object(source_fields);
        let authority = serde_json::json!({});
        let mut path = BoundedIgnoredPath(String::from("$"));
        let error = compare_native_authority_key_shapes(&source, &authority, &mut path, 0).unwrap_err();
        assert!(error.to_string().len() < 1024);
        let mut source = serde_json::Value::Null;
        let mut authority = serde_json::Value::Null;
        for _ in 0..=MAX_NATIVE_SHAPE_DEPTH {
            source = serde_json::json!({"nested": source});
            authority = serde_json::json!({"nested": authority});
        }
        let mut path = BoundedIgnoredPath(String::from("$"));
        let error = compare_native_authority_key_shapes(&source, &authority, &mut path, 0).unwrap_err();
        assert!(error.to_string().contains("bounded key-shape depth"));
        assert!(error.to_string().len() < 1024);
    }
}
