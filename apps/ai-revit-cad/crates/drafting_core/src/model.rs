use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const LAB_SCHEMA_VERSION: u32 = 1;
pub const MAX_DOCUMENT_ENTITIES: usize = 20_000;
pub const MAX_DOCUMENT_VERTICES: usize = 1_000_000;
pub const MAX_DRAWINGS: usize = 128;
pub const MAX_LAYERS: usize = 1024;
pub const MAX_HISTORY_RECORDS: usize = 4096;
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_COMMAND_CHANGES: usize = 61_024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftStyle {
    pub color: [f32; 4],
    pub width_px: f64,
}
impl Default for DraftStyle {
    fn default() -> Self {
        Self {
            color: [0.65, 0.85, 0.96, 1.],
            width_px: 1.5,
        }
    }
}
impl DraftStyle {
    pub fn validate(&self) -> DraftResult<()> {
        if self
            .color
            .iter()
            .any(|c| !c.is_finite() || !(0.0..=1.0).contains(c))
            || !self.width_px.is_finite()
            || self.width_px <= 0.
            || self.width_px > 64.
        {
            return Err(DraftError::new(
                "invalid_style",
                "color must be RGBA 0..1 and logical width in (0,64]",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftLayer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub style: DraftStyle,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityProvenance {
    pub import_asset_id: ImportAssetId,
    pub source_handle: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftEntity {
    pub id: DraftEntityId,
    pub layer_id: LayerId,
    pub geometry: EntityGeometry,
    pub style: DraftStyle,
    pub revision: u64,
    pub source: Option<EntityProvenance>,
}
impl DraftEntity {
    pub fn kind(&self) -> EntityKind {
        self.geometry.kind()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitResolution {
    pub declared_unit: Option<String>,
    pub resolved_unit: String,
    pub scale_to_m: f64,
    pub decision_source: String,
}
impl UnitResolution {
    pub fn validate(&self) -> DraftResult<()> {
        if self.resolved_unit.trim().is_empty()
            || self.decision_source.trim().is_empty()
            || !self.scale_to_m.is_finite()
            || self.scale_to_m <= 0.
            || self.scale_to_m > 1e6
            || self.scale_to_m < 1e-12
        {
            return Err(DraftError::new(
                "invalid_unit_resolution",
                "explicit resolved units and a finite positive bounded scale are required",
            ));
        }
        if self.declared_unit.as_ref().is_some_and(|s| s.len() > 64)
            || self.resolved_unit.len() > 64
            || self.decision_source.len() > 256
        {
            return Err(DraftError::new("string_budget", "unit strings too long"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSourceSpec {
    pub name: String,
    pub sha256: String,
    pub unit_resolution: UnitResolution,
    pub report: serde_json::Value,
}
impl ImportSourceSpec {
    pub fn validate(&self) -> DraftResult<()> {
        validate_name(&self.name)?;
        self.unit_resolution.validate()?;
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(DraftError::new(
                "invalid_source_hash",
                "SHA-256 must contain 64 hexadecimal characters",
            ));
        }
        let mut writer = LimitWriter::new(std::io::sink(), 1024 * 1024);
        let result = serde_json::to_writer(&mut writer, &self.report);
        if writer.exceeded {
            return Err(DraftError::new(
                "import_report_budget",
                "import metadata exceeds 1 MiB",
            ));
        }
        result.map_err(|e| DraftError::new("serialization", e.to_string()))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportAsset {
    pub id: ImportAssetId,
    pub source: ImportSourceSpec,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDrawing {
    pub id: DrawingId,
    pub name: String,
    pub revision: u64,
    pub plane: DrawingPlane,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub entities: BTreeMap<DraftEntityId, DraftEntity>,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub layers: BTreeMap<LayerId, DraftLayer>,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub import_sources: BTreeMap<ImportAssetId, ImportAsset>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftSnapshot {
    pub document_id: String,
    pub document_revision: u64,
    pub drawing_id: DrawingId,
    pub name: String,
    pub revision: u64,
    pub plane: DrawingPlane,
    pub entities: BTreeMap<DraftEntityId, DraftEntity>,
    pub layers: BTreeMap<LayerId, DraftLayer>,
    pub import_sources: BTreeMap<ImportAssetId, ImportAsset>,
    pub source_fingerprint: String,
}
impl DraftSnapshot {
    pub fn bounds(&self, visible_only: bool) -> DraftResult<Option<Bounds2>> {
        let mut bounds: Option<Bounds2> = None;
        for e in self.entities.values() {
            if visible_only && !self.layers.get(&e.layer_id).is_some_and(|l| l.visible) {
                continue;
            }
            let b = e.geometry.bounds()?;
            bounds = Some(bounds.map_or(b, |old| old.union(b)));
        }
        Ok(bounds)
    }
    pub fn entity(&self, id: DraftEntityId) -> DraftResult<&DraftEntity> {
        self.entities
            .get(&id)
            .ok_or_else(|| DraftError::new("entity_not_found", format!("entity {id} is absent")))
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdAllocators {
    #[serde(with = "crate::ids::decimal_u64")]
    pub entity_high_water: u64,
    #[serde(with = "crate::ids::decimal_u64")]
    pub drawing_high_water: u64,
    #[serde(with = "crate::ids::decimal_u64")]
    pub layer_high_water: u64,
    #[serde(with = "crate::ids::decimal_u64")]
    pub import_high_water: u64,
}
fn allocate(value: &mut u64, space: &str) -> DraftResult<u64> {
    let id = value.checked_add(1).ok_or_else(|| {
        DraftError::new(
            "id_exhausted",
            format!("{space} identifier space is exhausted"),
        )
    })?;
    *value = id;
    Ok(id)
}
impl IdAllocators {
    pub(crate) fn entity(&mut self) -> DraftResult<DraftEntityId> {
        allocate(&mut self.entity_high_water, "entity").map(DraftEntityId::new)
    }
    pub(crate) fn drawing(&mut self) -> DraftResult<DrawingId> {
        allocate(&mut self.drawing_high_water, "drawing").map(DrawingId::new)
    }
    pub(crate) fn layer(&mut self) -> DraftResult<LayerId> {
        allocate(&mut self.layer_high_water, "layer").map(LayerId::new)
    }
    pub(crate) fn import(&mut self) -> DraftResult<ImportAssetId> {
        allocate(&mut self.import_high_water, "import").map(ImportAssetId::new)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentState {
    pub document_id: String,
    pub revision: u64,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub drawings: BTreeMap<DrawingId, DraftDrawing>,
    pub next_ids: IdAllocators,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabDocument {
    /// Process-local object identity. It never enters persistent JSON or
    /// persistent-content equality, but distinguishes same-UUID/revision loads.
    #[serde(skip, default = "new_runtime_instance_id")]
    pub(crate) runtime_instance_id: String,
    pub(crate) schema_version: u32,
    pub(crate) state: DocumentState,
    pub(crate) history: Vec<HistoryRecord>,
    pub(crate) history_cursor: usize,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub(crate) receipts: BTreeMap<String, ReplayRecord>,
}
fn new_runtime_instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
impl Clone for LabDocument {
    fn clone(&self) -> Self {
        Self {
            runtime_instance_id: new_runtime_instance_id(),
            schema_version: self.schema_version,
            state: self.state.clone(),
            history: self.history.clone(),
            history_cursor: self.history_cursor,
            receipts: self.receipts.clone(),
        }
    }
}
impl PartialEq for LabDocument {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.state == other.state
            && self.history == other.history
            && self.history_cursor == other.history_cursor
            && self.receipts == other.receipts
    }
}
impl Default for LabDocument {
    fn default() -> Self {
        Self::new()
    }
}
impl LabDocument {
    pub fn new() -> Self {
        let layer = DraftLayer {
            id: LayerId::new(1),
            name: "Layer0".into(),
            visible: true,
            locked: false,
            style: DraftStyle::default(),
        };
        let drawing = DraftDrawing {
            id: DrawingId::new(1),
            name: "Drawing 1".into(),
            revision: 0,
            plane: DrawingPlane::default(),
            entities: BTreeMap::new(),
            layers: BTreeMap::from([(layer.id, layer)]),
            import_sources: BTreeMap::new(),
        };
        Self {
            runtime_instance_id: new_runtime_instance_id(),
            schema_version: LAB_SCHEMA_VERSION,
            state: DocumentState {
                document_id: uuid::Uuid::new_v4().to_string(),
                revision: 0,
                drawings: BTreeMap::from([(drawing.id, drawing)]),
                next_ids: IdAllocators {
                    drawing_high_water: 1,
                    layer_high_water: 1,
                    ..Default::default()
                },
            },
            history: Vec::new(),
            history_cursor: 0,
            receipts: BTreeMap::new(),
        }
    }
    pub fn document_id(&self) -> &str {
        &self.state.document_id
    }
    pub fn runtime_instance_id(&self) -> &str {
        &self.runtime_instance_id
    }
    pub fn cache_source_key(&self, drawing_id: DrawingId, document_epoch: u64) -> DraftResult<CacheSourceKey> {
        let drawing = self.state.drawings.get(&drawing_id)
            .ok_or_else(|| DraftError::new("drawing_not_found", "drawing is absent"))?;
        Ok(CacheSourceKey {
            document_id: self.document_id().to_owned(),
            runtime_instance_id: self.runtime_instance_id.clone(),
            document_epoch,
            document_revision: self.revision(),
            drawing_id,
            drawing_revision: drawing.revision,
        })
    }
    pub fn revision(&self) -> u64 {
        self.state.revision
    }
    pub fn drawings(&self) -> &BTreeMap<DrawingId, DraftDrawing> {
        &self.state.drawings
    }
    pub fn allocators(&self) -> &IdAllocators {
        &self.state.next_ids
    }
    pub fn default_drawing_id(&self) -> DrawingId {
        self.state
            .drawings
            .keys()
            .next()
            .copied()
            .unwrap_or(DrawingId::new(0))
    }
    pub fn history_len(&self) -> usize {
        self.history.len()
    }
    pub fn history_cursor(&self) -> usize {
        self.history_cursor
    }
    pub fn can_undo(&self) -> bool {
        self.history_cursor > 0
    }
    pub fn can_redo(&self) -> bool {
        self.history_cursor < self.history.len()
    }
    pub fn history(&self) -> &[HistoryRecord] {
        &self.history
    }
    pub fn snapshot(&self, id: DrawingId) -> DraftResult<DraftSnapshot> {
        let d = self.state.drawings.get(&id).ok_or_else(|| {
            DraftError::new("drawing_not_found", format!("drawing {id} is absent"))
        })?;
        let bytes =
            serde_json::to_vec(d).map_err(|e| DraftError::new("serialization", e.to_string()))?;
        Ok(DraftSnapshot {
            document_id: self.state.document_id.clone(),
            document_revision: self.state.revision,
            drawing_id: id,
            name: d.name.clone(),
            revision: d.revision,
            plane: d.plane.clone(),
            entities: d.entities.clone(),
            layers: d.layers.clone(),
            import_sources: d.import_sources.clone(),
            source_fingerprint: fingerprint(&bytes),
        })
    }
    pub fn to_json(&self) -> DraftResult<String> {
        self.validate()?;
        let mut writer = LimitWriter::new(Vec::new(), MAX_JSON_BYTES);
        let result = serde_json::to_writer_pretty(&mut writer, self);
        map_serialization_result(result, writer.exceeded)?;
        String::from_utf8(writer.inner).map_err(|e| DraftError::new("serialization", e.to_string()))
    }
    pub fn from_json(text: &str) -> DraftResult<Self> {
        if text.len() > MAX_JSON_BYTES {
            return Err(DraftError::new(
                "document_byte_budget",
                "document input exceeds 64 MiB",
            ));
        }
        let doc: Self = serde_json::from_str(text)
            .map_err(|e| DraftError::new("invalid_document", e.to_string()))?;
        doc.validate()?;
        check_pretty_budget(&doc)?;
        Ok(doc)
    }
    pub fn save_json(&self, path: impl AsRef<std::path::Path>) -> DraftResult<()> {
        let path = path.as_ref();
        let text = self.to_json()?;
        let parent = path
            .parent()
            .ok_or_else(|| DraftError::new("invalid_path", "save path has no parent"))?;
        let temp = parent.join(format!(".drafting-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(DraftError::new("save_failed", e.to_string()));
        }
        Ok(())
    }
    pub fn load_json(path: impl AsRef<std::path::Path>) -> DraftResult<Self> {
        use std::io::Read;
        let mut file =
            std::fs::File::open(path).map_err(|e| DraftError::new("open_failed", e.to_string()))?;
        if file
            .metadata()
            .map_err(|e| DraftError::new("open_failed", e.to_string()))?
            .len()
            > MAX_JSON_BYTES as u64
        {
            return Err(DraftError::new(
                "document_byte_budget",
                "document input exceeds 64 MiB",
            ));
        }
        let mut text = String::new();
        file.by_ref()
            .take(MAX_JSON_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|e| DraftError::new("open_failed", e.to_string()))?;
        Self::from_json(&text)
    }
    pub fn validate(&self) -> DraftResult<()> {
        if self.schema_version != LAB_SCHEMA_VERSION {
            return Err(DraftError::new(
                "unsupported_schema",
                format!("schema {} is unsupported", self.schema_version),
            ));
        }
        if self.history.len() > MAX_HISTORY_RECORDS
            || self.history_cursor > self.history.len()
            || self.receipts.len() > MAX_HISTORY_RECORDS * 4
        {
            return Err(DraftError::new(
                "history_budget",
                "invalid history length or cursor",
            ));
        }
        validate_state(&self.state)?;
        self.validate_history()
    }
}
/// Count or collect exactly the bytes emitted by the persistent pretty encoder.
/// Refuse before writing the byte that would cross the cap.
pub(crate) struct LimitWriter<W> {
    inner: W,
    count: usize,
    limit: usize,
    exceeded: bool,
}
impl<W> LimitWriter<W> {
    fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            count: 0,
            limit,
            exceeded: false,
        }
    }
}
impl<W: std::io::Write> std::io::Write for LimitWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self.count.checked_add(bytes.len());
        if next.is_none_or(|n| n > self.limit) {
            self.exceeded = true;
            return Err(std::io::Error::other(
                "persistent pretty JSON exceeds byte budget",
            ));
        }
        self.inner.write_all(bytes)?;
        self.count = next.unwrap();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn map_serialization_result(
    result: Result<(), serde_json::Error>,
    exceeded: bool,
) -> DraftResult<()> {
    result.map_err(|e| {
        DraftError::new(
            if exceeded {
                "document_byte_budget"
            } else {
                "serialization"
            },
            e.to_string(),
        )
    })
}
pub(crate) fn check_pretty_budget<T: Serialize>(value: &T) -> DraftResult<()> {
    let mut writer = LimitWriter::new(std::io::sink(), MAX_JSON_BYTES);
    let result = serde_json::to_writer_pretty(&mut writer, value);
    map_serialization_result(result, writer.exceeded)
}

#[cfg(test)]
mod byte_budget_tests {
    use super::*;

    #[test]
    fn bounded_pretty_writer_never_collects_a_byte_past_its_cap() {
        let value = serde_json::json!({"escaped": "\n\"\\".repeat(100)});
        let expected = serde_json::to_vec_pretty(&value).unwrap();
        let mut exact = LimitWriter::new(Vec::new(), expected.len());
        serde_json::to_writer_pretty(&mut exact, &value).unwrap();
        assert_eq!(exact.inner, expected);
        assert!(!exact.exceeded);
        let cap = expected.len() - 1;
        let mut bounded = LimitWriter::new(Vec::new(), cap);
        assert!(serde_json::to_writer_pretty(&mut bounded, &value).is_err());
        assert!(bounded.exceeded);
        assert!(bounded.inner.len() <= cap);
        assert_eq!(bounded.count, bounded.inner.len());
    }
}
pub(crate) fn validate_name(name: &str) -> DraftResult<()> {
    if name.trim().is_empty() || name.len() > 512 || name.contains('\0') {
        return Err(DraftError::new(
            "invalid_name",
            "name is empty, contains NUL or exceeds 512 bytes",
        ));
    }
    Ok(())
}
fn deserialize_unique_map<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct UniqueVisitor<K, V>(std::marker::PhantomData<(K, V)>);
    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de>
        for UniqueVisitor<K, V>
    {
        type Value = BTreeMap<K, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a bounded map with unique keys")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            use serde::de::Error;
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if result.len() >= 32768 {
                    return Err(A::Error::custom("map entry budget exceeded"));
                }
                if result.insert(key, value).is_some() {
                    return Err(A::Error::custom("duplicate persistent map key"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(UniqueVisitor(std::marker::PhantomData))
}
pub(crate) fn fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}
pub(crate) fn validate_state(state: &DocumentState) -> DraftResult<()> {
    if state.document_id.is_empty()
        || state.document_id.len() > 128
        || state.drawings.is_empty()
        || state.drawings.len() > MAX_DRAWINGS
    {
        return Err(DraftError::new(
            "invalid_document",
            "invalid document identity/drawing count",
        ));
    }
    let mut entity_ids = BTreeSet::new();
    let mut layer_ids = BTreeSet::new();
    let mut asset_ids = BTreeSet::new();
    let mut vertices = 0usize;
    for (id, d) in &state.drawings {
        if *id != d.id
            || id.get() == 0
            || id.get() > state.next_ids.drawing_high_water
            || d.revision > state.revision
        {
            return Err(DraftError::new(
                "identity_invariant",
                "drawing ID/revision does not match authority",
            ));
        }
        validate_name(&d.name)?;
        d.plane.validate()?;
        if d.layers.is_empty() || d.layers.len() > MAX_LAYERS {
            return Err(DraftError::new("layer_budget", "invalid layer count"));
        }
        let mut names = BTreeSet::new();
        for (lid, l) in &d.layers {
            if *lid != l.id
                || lid.get() == 0
                || lid.get() > state.next_ids.layer_high_water
                || !layer_ids.insert(*lid)
                || !names.insert(l.name.clone())
            {
                return Err(DraftError::new(
                    "identity_invariant",
                    "layer ID or name collision",
                ));
            }
            validate_name(&l.name)?;
            l.style.validate()?;
        }
        for (aid, asset) in &d.import_sources {
            if *aid != asset.id
                || aid.get() == 0
                || aid.get() > state.next_ids.import_high_water
                || !asset_ids.insert(*aid)
            {
                return Err(DraftError::new(
                    "identity_invariant",
                    "import asset ID collision",
                ));
            }
            asset.source.validate()?;
        }
        let mut handles = BTreeSet::new();
        for (eid, e) in &d.entities {
            if *eid != e.id
                || eid.get() == 0
                || eid.get() > state.next_ids.entity_high_water
                || !entity_ids.insert(*eid)
                || !d.layers.contains_key(&e.layer_id)
                || e.revision > state.revision
            {
                return Err(DraftError::new(
                    "identity_invariant",
                    "entity ID/layer/revision invalid",
                ));
            }
            e.geometry.validate(GeometryLimits::default())?;
            e.style.validate()?;
            if let EntityGeometry::Polyline { vertices: v, .. } = &e.geometry {
                vertices = vertices
                    .checked_add(v.len())
                    .ok_or_else(|| DraftError::new("geometry_budget", "vertex count overflow"))?;
            }
            if let Some(source) = &e.source {
                if !d.import_sources.contains_key(&source.import_asset_id) {
                    return Err(DraftError::new(
                        "invalid_provenance",
                        "missing import asset",
                    ));
                }
                if let Some(handle) = &source.source_handle {
                    if handle.is_empty()
                        || handle.len() > 64
                        || !handles.insert((source.import_asset_id, handle.to_ascii_uppercase()))
                    {
                        return Err(DraftError::new(
                            "duplicate_source_handle",
                            "duplicate or invalid handle within import asset",
                        ));
                    }
                }
            }
        }
    }
    if entity_ids.len() > MAX_DOCUMENT_ENTITIES || vertices > MAX_DOCUMENT_VERTICES {
        return Err(DraftError::new(
            "document_geometry_budget",
            "document entity/vertex budget exceeded",
        ));
    }
    Ok(())
}
