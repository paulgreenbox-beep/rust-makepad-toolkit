//! Typed analytic drafting owned by the same document, journal and revision as BIM.
//! Lab value types are reused; LabDocument and its history are never resident here.
use crate::*;
use drafting_core::{DraftChange, DraftError, GeometryLimits};
pub use drafting_core::{
    DraftDrawing, DraftEntity, DraftEntityId, DraftLayer, DraftStyle, DrawingId, DrawingPlane,
    EntityGeometry as DraftGeometry, ImportAsset as DraftImportAsset, ImportAssetId, ImportBatch,
    ImportEntity, ImportLayer, ImportSourceSpec, LayerId as DraftLayerId, Point2 as DraftPoint2,
    PolylineVertex, UnitResolution,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ViewDefinitionId(#[serde(with = "decimal_id")] u64);
impl ViewDefinitionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Display for ViewDefinitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
pub(super) mod decimal_id {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        let value = text.parse::<u64>().map_err(D::Error::custom)?;
        if text != value.to_string() {
            return Err(D::Error::custom("noncanonical decimal identifier"));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "domain", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectObjectRef {
    GeoState,
    GeoModel { model_id: String },
    GeoSnapshot { snapshot_id: geo_core::GeoSnapshotId },
    GeoFoundationLayout,
    GeoFoundationAnalysisSnapshot { snapshot_id: EngineeringSnapshotId },
    ConfiguredBuilding { building_id: structural_configurator::SemanticId },
    ConfiguredSection { building_id: structural_configurator::SemanticId, section_id: structural_configurator::SemanticId },
    ConfiguratorAnalysisSnapshot { snapshot_id: EngineeringSnapshotId },
    FoundationStructuralInput,
    EngineeringSnapshot { snapshot_id: EngineeringSnapshotId },
    GroundSection {
        ground_section_id: GroundSectionId,
    },
    AnalysisCase {
        analysis_id: AnalysisCaseId,
    },
    Bim {
        element_id: ElementId,
    },
    Drafting {
        drawing_id: DrawingId,
        entity_id: DraftEntityId,
    },
    Drawing {
        drawing_id: DrawingId,
    },
    DraftLayer {
        drawing_id: DrawingId,
        layer_id: DraftLayerId,
    },
    ImportAsset {
        drawing_id: DrawingId,
        import_asset_id: ImportAssetId,
    },
    ViewDefinition {
        drawing_id: DrawingId,
        view_definition_id: ViewDefinitionId,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftCoordinateSystem {
    #[default]
    Xy,
}

/// Durable element identities and an explicit bounded XY projection request.
/// Source geometry/revision and missing-source status are derived, never frozen here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftSourceReference {
    pub element_ids: Vec<ElementId>,
    #[serde(default)]
    pub elevation_m: Option<f64>,
    #[serde(default)]
    pub coordinate_system: DraftCoordinateSystem,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftViewDefinition {
    pub id: ViewDefinitionId,
    pub drawing_id: DrawingId,
    pub name: String,
    pub source: DraftSourceReference,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftingDomain {
    #[serde(deserialize_with = "unique_map")]
    pub drawings: BTreeMap<DrawingId, DraftDrawing>,
    #[serde(deserialize_with = "unique_map")]
    pub views: BTreeMap<ViewDefinitionId, DraftViewDefinition>,
    #[serde(default)]
    pub vertex_count: usize,
}
impl DraftingDomain {
    pub fn entity_count(&self) -> usize {
        self.drawings.values().map(|d| d.entities.len()).sum()
    }
    pub fn source_references(&self) -> impl Iterator<Item = &DraftSourceReference> {
        self.views.values().map(|v| &v.source)
    }
}
pub(super) fn unique_map<'de, D, K, V>(d: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct Unique<K, V>(std::marker::PhantomData<(K, V)>);
    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de> for Unique<K, V> {
        type Value = BTreeMap<K, V>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map without duplicate keys")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut m: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((k, v)) = m.next_entry()? {
                if result.insert(k, v).is_some() {
                    return Err(serde::de::Error::custom("duplicate identifier key"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Unique(std::marker::PhantomData))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftAllocators {
    #[serde(with = "decimal_id")]
    pub drawing: u64,
    #[serde(with = "decimal_id")]
    pub entity: u64,
    #[serde(with = "decimal_id")]
    pub layer: u64,
    #[serde(with = "decimal_id")]
    pub import_asset: u64,
    #[serde(with = "decimal_id")]
    pub view: u64,
}
impl Default for DraftAllocators {
    fn default() -> Self {
        Self {
            drawing: 1,
            entity: 1,
            layer: 1,
            import_asset: 1,
            view: 1,
        }
    }
}
impl DraftAllocators {
    pub(super) fn retains(&self, old: &Self) -> bool {
        self.drawing >= old.drawing
            && self.entity >= old.entity
            && self.layer >= old.layer
            && self.import_asset >= old.import_asset
            && self.view >= old.view
    }
    fn allocate(counter: &mut u64, kind: &'static str) -> Result<u64, CoreError> {
        let id = *counter;
        *counter = counter.checked_add(1).ok_or(CoreError::IdExhausted(kind))?;
        Ok(id)
    }
}

/// A clone is a separate runtime source even if persistent UUID/revision match.
#[derive(Debug)]
pub(super) struct RuntimeDocumentId(Uuid);
impl Default for RuntimeDocumentId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Clone for RuntimeDocumentId {
    fn clone(&self) -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DraftingDelta {
    #[serde(default)]
    pub affected_objects: Vec<ProjectObjectRef>,
    pub created_drawings: Vec<DrawingId>,
    pub updated_drawings: Vec<DrawingId>,
    pub deleted_drawings: Vec<DrawingId>,
    pub created_entities: Vec<DraftEntityId>,
    pub updated_entities: Vec<DraftEntityId>,
    pub deleted_entities: Vec<DraftEntityId>,
    pub changed_layers: Vec<DraftLayerId>,
    pub changed_import_assets: Vec<ImportAssetId>,
    pub created_views: Vec<ViewDefinitionId>,
    pub updated_views: Vec<ViewDefinitionId>,
    pub deleted_views: Vec<ViewDefinitionId>,
    pub affected_drawings: Vec<DrawingId>,
}

/// Immutable import payload; source Lab history is deliberately excluded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabImportDocument {
    pub source_name: String,
    pub source_sha256: String,
    pub source_document_id: String,
    pub source_revision: u64,
    pub drawings: Vec<DraftDrawing>,
    #[serde(default)]
    pub id_mapping: Option<LabIdMapping>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LabIdMapping {
    pub drawings: BTreeMap<DrawingId, DrawingId>,
    pub entities: BTreeMap<DraftEntityId, DraftEntityId>,
    pub layers: BTreeMap<DraftLayerId, DraftLayerId>,
    pub import_assets: BTreeMap<ImportAssetId, ImportAssetId>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabImportPreview {
    pub expected_revision: u64,
    pub commands: Vec<Command>,
    pub mapping: LabIdMapping,
    pub source_document_id: String,
    pub source_revision: u64,
    pub history_imported: bool,
}
impl LabImportDocument {
    fn validate(&self) -> Result<(), CoreError> {
        validate_name(&self.source_name, "Lab source")?;
        if self.source_sha256.len() != 64
            || !self.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.source_document_id.is_empty()
            || self.source_document_id.len() > 256
            || self.drawings.is_empty()
        {
            return Err(draft_error("invalid Lab import source metadata"));
        }
        let drawings = self
            .drawings
            .iter()
            .map(|d| (d.id, d.clone()))
            .collect::<BTreeMap<_, _>>();
        if drawings.len() != self.drawings.len() {
            return Err(draft_error("duplicate Lab source drawing IDs"));
        }
        DraftingDomain {
            vertex_count: drawings.values().map(drawing_vertices).sum(),
            drawings,
            views: BTreeMap::new(),
        }
        .validate(self.source_revision)
    }
}
fn lab_provenance(source: &LabImportDocument) -> ImportSourceSpec {
    ImportSourceSpec {
        name: source.source_name.clone(),
        sha256: source.source_sha256.clone(),
        unit_resolution: UnitResolution {
            declared_unit: Some("metre".into()),
            resolved_unit: "metre".into(),
            scale_to_m: 1.,
            decision_source: "validated_lab_analytic_coordinates".into(),
        },
        report: serde_json::json!({"format":"drafting_lab_v0.1.1","source_document_id":source.source_document_id,"source_revision":source.source_revision,"history_imported":false,"coordinate_policy":"analytic_metres_preserved","id_mapping":source.id_mapping}),
    }
}
fn lab_id_mapping(
    source: &LabImportDocument,
    counters: &mut DraftAllocators,
) -> Result<LabIdMapping, CoreError> {
    let mut map = LabIdMapping::default();
    for d in &source.drawings {
        map.drawings.insert(
            d.id,
            DrawingId::new(DraftAllocators::allocate(&mut counters.drawing, "drawing")?),
        );
        for id in d.layers.keys() {
            map.layers.insert(
                *id,
                DraftLayerId::new(DraftAllocators::allocate(
                    &mut counters.layer,
                    "draft layer",
                )?),
            );
        }
        DraftAllocators::allocate(&mut counters.import_asset, "Lab provenance asset")?;
        for id in d.import_sources.keys() {
            map.import_assets.insert(
                *id,
                ImportAssetId::new(DraftAllocators::allocate(
                    &mut counters.import_asset,
                    "draft import asset",
                )?),
            );
        }
        for id in d.entities.keys() {
            map.entities.insert(
                *id,
                DraftEntityId::new(DraftAllocators::allocate(
                    &mut counters.entity,
                    "draft entity",
                )?),
            );
        }
    }
    Ok(map)
}
impl Document {
    pub fn prepare_lab_import(
        &self,
        json: &str,
        source_name: &str,
    ) -> Result<LabImportPreview, CoreError> {
        use sha2::{Digest, Sha256};
        let lab = check(drafting_core::LabDocument::from_json(json))?;
        let source = LabImportDocument {
            source_name: source_name.into(),
            source_sha256: format!("{:x}", Sha256::digest(json.as_bytes())),
            source_document_id: lab.document_id().into(),
            source_revision: lab.revision(),
            drawings: lab.drawings().values().cloned().collect(),
            id_mapping: None,
        };
        self.prepare_typed_lab_import(source)
    }
    pub fn prepare_typed_lab_import(
        &self,
        mut source: LabImportDocument,
    ) -> Result<LabImportPreview, CoreError> {
        source.validate()?;
        self.validate_lab_import_budget(&source)?;
        let mapping = lab_id_mapping(&source, &mut self.draft_ids.clone())?;
        source.id_mapping = Some(mapping.clone());
        Ok(LabImportPreview {
            expected_revision: self.revision,
            source_document_id: source.source_document_id.clone(),
            source_revision: source.source_revision,
            history_imported: false,
            commands: vec![Command::Draft(DraftCommand::ImportLabDocument { source })],
            mapping,
        })
    }
    fn validate_lab_import_budget(&self, source: &LabImportDocument) -> Result<(), CoreError> {
        if self.drawings().len().saturating_add(source.drawings.len()) > drafting_core::MAX_DRAWINGS
            || self.drafting.entity_count().saturating_add(
                source
                    .drawings
                    .iter()
                    .map(|d| d.entities.len())
                    .sum::<usize>(),
            ) > drafting_core::MAX_DOCUMENT_ENTITIES
            || self
                .drafting
                .vertex_count
                .saturating_add(source.drawings.iter().map(drawing_vertices).sum::<usize>())
                > drafting_core::MAX_DOCUMENT_VERTICES
        {
            return Err(draft_error("Lab import exceeds project geometry budget"));
        }
        Ok(())
    }
    fn apply_lab_import(
        &mut self,
        source: &LabImportDocument,
        revision: u64,
        changes: &mut Vec<Change>,
    ) -> Result<(), CoreError> {
        source.validate()?;
        self.validate_lab_import_budget(source)?;
        let mut predicted = self.draft_ids.clone();
        let mapping = lab_id_mapping(source, &mut predicted)?;
        if source.id_mapping.as_ref() != Some(&mapping) {
            return Err(draft_error(
                "Lab import requires a current preview with matching allocation mapping",
            ));
        }
        for old in &source.drawings {
            let id = DrawingId::new(DraftAllocators::allocate(
                &mut self.draft_ids.drawing,
                "drawing",
            )?);
            let mut layers = BTreeMap::new();
            for l in old.layers.values() {
                let lid = DraftLayerId::new(DraftAllocators::allocate(
                    &mut self.draft_ids.layer,
                    "draft layer",
                )?);
                let mut value = l.clone();
                value.id = lid;
                layers.insert(lid, value);
            }
            let provenance_id = ImportAssetId::new(DraftAllocators::allocate(
                &mut self.draft_ids.import_asset,
                "Lab provenance asset",
            )?);
            let mut assets = BTreeMap::from([(
                provenance_id,
                DraftImportAsset {
                    id: provenance_id,
                    source: lab_provenance(source),
                },
            )]);
            for asset in old.import_sources.values() {
                let aid = ImportAssetId::new(DraftAllocators::allocate(
                    &mut self.draft_ids.import_asset,
                    "draft import asset",
                )?);
                assets.insert(
                    aid,
                    DraftImportAsset {
                        id: aid,
                        source: asset.source.clone(),
                    },
                );
            }
            let mut entities = BTreeMap::new();
            for e in old.entities.values() {
                let eid = DraftEntityId::new(DraftAllocators::allocate(
                    &mut self.draft_ids.entity,
                    "draft entity",
                )?);
                let entity = DraftEntity {
                    id: eid,
                    layer_id: mapping.layers[&e.layer_id],
                    geometry: e.geometry.clone(),
                    style: e.style.clone(),
                    revision,
                    source: Some(match &e.source {
                        Some(s) => drafting_core::EntityProvenance {
                            import_asset_id: mapping.import_assets[&s.import_asset_id],
                            source_handle: s.source_handle.clone(),
                        },
                        None => drafting_core::EntityProvenance {
                            import_asset_id: provenance_id,
                            source_handle: Some(format!("lab-entity:{}", e.id)),
                        },
                    }),
                };
                entities.insert(eid, entity);
            }
            let drawing = DraftDrawing {
                id,
                name: old.name.clone(),
                revision,
                plane: old.plane.clone(),
                entities,
                layers,
                import_sources: assets,
            };
            self.drafting.drawings.insert(id, drawing.clone());
            changes.push(Change::Draft(DraftChange::Drawing {
                drawing_id: id,
                before: None,
                after: Some(drawing),
            }));
        }
        debug_assert_eq!(self.draft_ids, predicted);
        Ok(())
    }
}
fn lab_drawing_effects_match(
    source: &LabImportDocument,
    old: &DraftDrawing,
    new: &DraftDrawing,
) -> bool {
    let Some(mapping) = source.id_mapping.as_ref() else {
        return false;
    };
    if mapping.drawings.get(&old.id) != Some(&new.id) {
        return false;
    }
    if old.name != new.name
        || old.plane != new.plane
        || old.layers.len() != new.layers.len()
        || old.entities.len() != new.entities.len()
        || old.import_sources.len() + 1 != new.import_sources.len()
    {
        return false;
    }
    let mut layer_map = BTreeMap::new();
    for (a, b) in old.layers.values().zip(new.layers.values()) {
        if a.name != b.name
            || a.visible != b.visible
            || a.locked != b.locked
            || a.style != b.style
            || mapping.layers.get(&a.id) != Some(&b.id)
        {
            return false;
        }
        layer_map.insert(a.id, b.id);
    }
    let mut imports = new.import_sources.values();
    let Some(provenance) = imports.next() else {
        return false;
    };
    if provenance.source != lab_provenance(source) {
        return false;
    }
    let mut asset_map = BTreeMap::new();
    for (a, b) in old.import_sources.values().zip(imports) {
        if a.source != b.source || mapping.import_assets.get(&a.id) != Some(&b.id) {
            return false;
        }
        asset_map.insert(a.id, b.id);
    }
    old.entities
        .values()
        .zip(new.entities.values())
        .all(|(a, b)| {
            mapping.entities.get(&a.id) == Some(&b.id)
                && a.geometry == b.geometry
                && a.style == b.style
                && layer_map.get(&a.layer_id) == Some(&b.layer_id)
                && b.source
                    == Some(match &a.source {
                        Some(s) => drafting_core::EntityProvenance {
                            import_asset_id: asset_map[&s.import_asset_id],
                            source_handle: s.source_handle.clone(),
                        },
                        None => drafting_core::EntityProvenance {
                            import_asset_id: provenance.id,
                            source_handle: Some(format!("lab-entity:{}", a.id)),
                        },
                    })
        })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftCommand {
    CreateDrawing {
        name: String,
        #[serde(default)]
        plane: DrawingPlane,
    },
    RenameDrawing {
        drawing_id: DrawingId,
        name: String,
    },
    CreateDraftEntity {
        drawing_id: DrawingId,
        layer_id: DraftLayerId,
        geometry: DraftGeometry,
        #[serde(default)]
        style: DraftStyle,
    },
    UpdateDraftEntity {
        drawing_id: DrawingId,
        entity_id: DraftEntityId,
        geometry: DraftGeometry,
    },
    MoveDraftEntity {
        drawing_id: DrawingId,
        entity_ids: Vec<DraftEntityId>,
        delta: DraftPoint2,
    },
    DeleteDraftEntity {
        drawing_id: DrawingId,
        entity_ids: Vec<DraftEntityId>,
    },
    CreateDraftLayer {
        drawing_id: DrawingId,
        name: String,
        visible: bool,
        locked: bool,
        #[serde(default)]
        style: DraftStyle,
    },
    UpdateDraftLayer {
        drawing_id: DrawingId,
        layer_id: DraftLayerId,
        name: Option<String>,
        visible: Option<bool>,
        locked: Option<bool>,
        style: Option<DraftStyle>,
    },
    CreateViewDefinition {
        drawing_id: DrawingId,
        name: String,
        source: DraftSourceReference,
    },
    UpdateViewDefinition {
        view_definition_id: ViewDefinitionId,
        name: Option<String>,
        source: Option<DraftSourceReference>,
    },
    ImportDraftingEntities {
        batch: ImportBatch,
    },
    ImportLabDocument {
        source: LabImportDocument,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct DrawingMetadata {
    pub name: String,
    pub plane: DrawingPlane,
    pub revision: u64,
}
impl DrawingMetadata {
    pub(super) fn of(d: &DraftDrawing) -> Self {
        Self {
            name: d.name.clone(),
            plane: d.plane.clone(),
            revision: d.revision,
        }
    }
    pub(super) fn install(&self, d: &mut DraftDrawing) {
        d.name = self.name.clone();
        d.plane = self.plane.clone();
        d.revision = self.revision;
    }
}

fn vertices(g: &DraftGeometry) -> usize {
    match g {
        DraftGeometry::Line { .. } => 2,
        DraftGeometry::Circle { .. } | DraftGeometry::Arc { .. } => 1,
        DraftGeometry::Polyline { vertices, .. } => vertices.len(),
    }
}
fn drawing_vertices(d: &DraftDrawing) -> usize {
    d.entities.values().map(|e| vertices(&e.geometry)).sum()
}
fn draft_vertices_changed(changes: &[Change]) -> (usize, usize) {
    let mut removed = 0usize;
    let mut added = 0usize;
    for c in changes {
        if let Change::Draft(c) = c {
            match c {
                DraftChange::Entity { before, after, .. } => {
                    removed += before.as_ref().map_or(0, |e| vertices(&e.geometry));
                    added += after.as_ref().map_or(0, |e| vertices(&e.geometry));
                }
                DraftChange::Drawing { before, after, .. } => {
                    removed += before.as_ref().map_or(0, drawing_vertices);
                    added += after.as_ref().map_or(0, drawing_vertices);
                }
                _ => {}
            }
        }
    }
    (removed, added)
}
impl DraftingDomain {
    pub(super) fn apply_change(&mut self, change: &DraftChange, forward: bool) {
        match change {
            DraftChange::Entity {
                drawing_id,
                id,
                before,
                after,
            } => {
                if let Some(d) = self.drawings.get_mut(drawing_id) {
                    let old = d.entities.get(id).map_or(0, |e| vertices(&e.geometry));
                    let value = if forward { after } else { before };
                    let new = value.as_ref().map_or(0, |e| vertices(&e.geometry));
                    self.vertex_count = self.vertex_count.saturating_sub(old).saturating_add(new);
                    set_optional(&mut d.entities, *id, value);
                }
            }
            DraftChange::Layer {
                drawing_id,
                id,
                before,
                after,
            } => {
                if let Some(d) = self.drawings.get_mut(drawing_id) {
                    set_optional(&mut d.layers, *id, if forward { after } else { before });
                }
            }
            DraftChange::ImportAsset {
                drawing_id,
                id,
                before,
                after,
            } => {
                if let Some(d) = self.drawings.get_mut(drawing_id) {
                    set_optional(
                        &mut d.import_sources,
                        *id,
                        if forward { after } else { before },
                    );
                }
            }
            DraftChange::Drawing {
                drawing_id,
                before,
                after,
            } => {
                let old = self.drawings.get(drawing_id).map_or(0, drawing_vertices);
                let value = if forward { after } else { before };
                let new = value.as_ref().map_or(0, drawing_vertices);
                self.vertex_count = self.vertex_count.saturating_sub(old).saturating_add(new);
                set_optional(&mut self.drawings, *drawing_id, value);
            }
        }
    }
    pub(super) fn matches_before(&self, change: &DraftChange) -> bool {
        match change {
            DraftChange::Entity {
                drawing_id,
                id,
                before,
                ..
            } => {
                self.drawings
                    .get(drawing_id)
                    .and_then(|d| d.entities.get(id))
                    == before.as_ref()
            }
            DraftChange::Layer {
                drawing_id,
                id,
                before,
                ..
            } => self.drawings.get(drawing_id).and_then(|d| d.layers.get(id)) == before.as_ref(),
            DraftChange::ImportAsset {
                drawing_id,
                id,
                before,
                ..
            } => {
                self.drawings
                    .get(drawing_id)
                    .and_then(|d| d.import_sources.get(id))
                    == before.as_ref()
            }
            DraftChange::Drawing {
                drawing_id, before, ..
            } => self.drawings.get(drawing_id) == before.as_ref(),
        }
    }
    fn validate(&self, revision: u64) -> Result<(), CoreError> {
        if self.drawings.len() > drafting_core::MAX_DRAWINGS
            || self.entity_count() > drafting_core::MAX_DOCUMENT_ENTITIES
            || self.vertex_count > drafting_core::MAX_DOCUMENT_VERTICES
            || self.vertex_count != self.drawings.values().map(drawing_vertices).sum::<usize>()
        {
            return Err(draft_error("drawing/entity/vertex count or budget invalid"));
        }
        let mut entities = BTreeSet::new();
        let mut layers = BTreeSet::new();
        let mut assets = BTreeSet::new();
        for (id, d) in &self.drawings {
            if id.get() == 0 || *id != d.id || d.revision > revision {
                return Err(draft_error("drawing identity/revision mismatch"));
            }
            validate_name(&d.name, "drawing")?;
            check(d.plane.validate())?;
            if d.layers.is_empty() || d.layers.len() > drafting_core::MAX_LAYERS {
                return Err(draft_error("invalid layer count"));
            }
            let mut names = BTreeSet::new();
            for (lid, l) in &d.layers {
                if lid.get() == 0
                    || *lid != l.id
                    || !layers.insert(*lid)
                    || !names.insert(l.name.to_lowercase())
                {
                    return Err(draft_error("layer identity/name collision"));
                }
                validate_name(&l.name, "draft layer")?;
                check(l.style.validate())?;
            }
            for (aid, a) in &d.import_sources {
                if aid.get() == 0 || *aid != a.id || !assets.insert(*aid) {
                    return Err(draft_error("import asset identity collision"));
                }
                check(a.source.validate())?;
            }
            let mut handles = BTreeSet::new();
            for (eid, e) in &d.entities {
                if *eid != e.id || !entities.insert(*eid) {
                    return Err(draft_error("entity identity collision"));
                }
                validate_entity(e, d, revision)?;
                if let Some(s) = &e.source {
                    if let Some(h) = &s.source_handle {
                        if h.is_empty()
                            || h.len() > 256
                            || !handles.insert((s.import_asset_id, h.to_uppercase()))
                        {
                            return Err(draft_error(
                                "external handle collision inside import asset",
                            ));
                        }
                    }
                }
            }
        }
        for (id, v) in &self.views {
            if id.get() == 0 || *id != v.id || !self.drawings.contains_key(&v.drawing_id) {
                return Err(draft_error("view identity/drawing mismatch"));
            }
            validate_name(&v.name, "draft view")?;
            validate_source_value(&v.source)?;
        }
        Ok(())
    }
}
fn validate_source_value(source: &DraftSourceReference) -> Result<(), CoreError> {
    if source.element_ids.len() > 20_000
        || source.element_ids.iter().any(|id| id.get() == 0)
        || source
            .element_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != source.element_ids.len()
        || source.elevation_m.is_some_and(|x| !x.is_finite())
    {
        return Err(draft_error("invalid BIM reference request"));
    }
    Ok(())
}
fn validate_entity(e: &DraftEntity, d: &DraftDrawing, revision: u64) -> Result<(), CoreError> {
    if e.id.get() == 0 || e.revision > revision || !d.layers.contains_key(&e.layer_id) {
        return Err(draft_error("entity identity/layer/revision invalid"));
    }
    check(e.geometry.validate(GeometryLimits::default()))?;
    check(e.style.validate())?;
    if e.source
        .as_ref()
        .is_some_and(|s| !d.import_sources.contains_key(&s.import_asset_id))
    {
        return Err(draft_error("entity import provenance is absent"));
    }
    Ok(())
}
pub(super) fn inverse_draft_change(change: &DraftChange) -> DraftChange {
    match change {
        DraftChange::Entity {
            drawing_id,
            id,
            before,
            after,
        } => DraftChange::Entity {
            drawing_id: *drawing_id,
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        DraftChange::Layer {
            drawing_id,
            id,
            before,
            after,
        } => DraftChange::Layer {
            drawing_id: *drawing_id,
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        DraftChange::ImportAsset {
            drawing_id,
            id,
            before,
            after,
        } => DraftChange::ImportAsset {
            drawing_id: *drawing_id,
            id: *id,
            before: after.clone(),
            after: before.clone(),
        },
        DraftChange::Drawing {
            drawing_id,
            before,
            after,
        } => DraftChange::Drawing {
            drawing_id: *drawing_id,
            before: after.clone(),
            after: before.clone(),
        },
    }
}
fn zero_max() -> DraftAllocators {
    DraftAllocators {
        drawing: 0,
        entity: 0,
        layer: 0,
        import_asset: 0,
        view: 0,
    }
}
fn max_domain(max: &mut DraftAllocators, domain: &DraftingDomain) {
    for d in domain.drawings.values() {
        max_drawing(max, d);
    }
    for v in domain.views.values() {
        max.view = max.view.max(v.id.get());
    }
}
fn max_drawing(max: &mut DraftAllocators, d: &DraftDrawing) {
    max.drawing = max.drawing.max(d.id.get());
    for e in d.entities.values() {
        max.entity = max.entity.max(e.id.get());
    }
    for l in d.layers.values() {
        max.layer = max.layer.max(l.id.get());
    }
    for a in d.import_sources.values() {
        max.import_asset = max.import_asset.max(a.id.get());
    }
}
fn max_change(max: &mut DraftAllocators, c: &Change) {
    match c {
        Change::Draft(DraftChange::Entity { drawing_id, id, .. }) => {
            max.drawing = max.drawing.max(drawing_id.get());
            max.entity = max.entity.max(id.get());
        }
        Change::Draft(DraftChange::Layer { drawing_id, id, .. }) => {
            max.drawing = max.drawing.max(drawing_id.get());
            max.layer = max.layer.max(id.get());
        }
        Change::Draft(DraftChange::ImportAsset { drawing_id, id, .. }) => {
            max.drawing = max.drawing.max(drawing_id.get());
            max.import_asset = max.import_asset.max(id.get());
        }
        Change::Draft(DraftChange::Drawing {
            drawing_id,
            before,
            after,
        }) => {
            max.drawing = max.drawing.max(drawing_id.get());
            for d in before.iter().chain(after) {
                max_drawing(max, d);
            }
        }
        Change::DraftDrawingMetadata { id, .. } => max.drawing = max.drawing.max(id.get()),
        Change::DraftView { id, before, after } => {
            max.view = max.view.max(id.get());
            for v in before.iter().chain(after) {
                max.drawing = max.drawing.max(v.drawing_id.get());
            }
        }
        _ => {}
    }
}
fn counters_above(c: &DraftAllocators, max: &DraftAllocators) -> bool {
    c.drawing > max.drawing
        && c.entity > max.entity
        && c.layer > max.layer
        && c.import_asset > max.import_asset
        && c.view > max.view
}
pub(super) fn check_fresh_draft_allocation(
    change: &Change,
    prior: &DraftAllocators,
) -> Result<(), CoreError> {
    let good = match change {
        Change::Draft(DraftChange::Entity {
            id,
            before: None,
            after: Some(_),
            ..
        }) => id.get() >= prior.entity,
        Change::Draft(DraftChange::Layer {
            id,
            before: None,
            after: Some(_),
            ..
        }) => id.get() >= prior.layer,
        Change::Draft(DraftChange::ImportAsset {
            id,
            before: None,
            after: Some(_),
            ..
        }) => id.get() >= prior.import_asset,
        Change::Draft(DraftChange::Drawing {
            drawing_id,
            before: None,
            after: Some(d),
        }) => {
            drawing_id.get() >= prior.drawing
                && d.layers.keys().all(|id| id.get() >= prior.layer)
                && d.entities.keys().all(|id| id.get() >= prior.entity)
                && d.import_sources
                    .keys()
                    .all(|id| id.get() >= prior.import_asset)
        }
        Change::DraftView {
            id,
            before: None,
            after: Some(_),
        } => id.get() >= prior.view,
        _ => true,
    };
    if !good {
        return Err(draft_error(
            "new historical commit reuses a previously allocated drafting ID",
        ));
    }
    Ok(())
}

impl Document {
    pub(super) fn validate_local_drafting(&self, changes: &[Change]) -> Result<(), CoreError> {
        if self.drafting.entity_count() > drafting_core::MAX_DOCUMENT_ENTITIES
            || self.drafting.vertex_count > drafting_core::MAX_DOCUMENT_VERTICES
        {
            return Err(draft_error("document drafting budget exceeded"));
        }
        for c in changes {
            match c {
                Change::Draft(DraftChange::Entity { drawing_id, id, .. }) => {
                    if let Some(e) = self.draft_entity(*drawing_id, *id) {
                        validate_entity(e, self.require_drawing(*drawing_id)?, self.revision)?;
                    }
                }
                Change::Draft(DraftChange::Layer { drawing_id, id, .. }) => {
                    if let Some(l) = self.require_drawing(*drawing_id)?.layers.get(id) {
                        if id.get() == 0 || *id != l.id {
                            return Err(draft_error("layer ID mismatch"));
                        }
                        validate_name(&l.name, "draft layer")?;
                        check(l.style.validate())?;
                    }
                }
                Change::Draft(DraftChange::ImportAsset { drawing_id, id, .. }) => {
                    if let Some(a) = self.require_drawing(*drawing_id)?.import_sources.get(id) {
                        if id.get() == 0 || *id != a.id {
                            return Err(draft_error("asset ID mismatch"));
                        }
                        check(a.source.validate())?;
                    }
                }
                Change::Draft(DraftChange::Drawing { drawing_id, .. }) => {
                    if let Some(d) = self.drawing(*drawing_id) {
                        if d.id != *drawing_id || drawing_id.get() == 0 {
                            return Err(draft_error("drawing ID mismatch"));
                        }
                        validate_name(&d.name, "drawing")?;
                        check(d.plane.validate())?;
                    }
                }
                Change::DraftDrawingMetadata { id, .. } => {
                    if let Some(d) = self.drawing(*id) {
                        validate_name(&d.name, "drawing")?;
                        check(d.plane.validate())?;
                        if d.revision > self.revision {
                            return Err(draft_error("future drawing revision"));
                        }
                    }
                }
                Change::DraftView { id, .. } => {
                    if let Some(v) = self.view_definition(*id) {
                        if *id != v.id || id.get() == 0 {
                            return Err(draft_error("view ID mismatch"));
                        }
                        self.require_drawing(v.drawing_id)?;
                        validate_name(&v.name, "draft view")?;
                        validate_source_value(&v.source)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub(super) fn verify_drafting_history(&self) -> Result<(), CoreError> {
        self.drafting.validate(self.revision)?;
        let mut max = zero_max();
        max_domain(&mut max, &self.drafting);
        for (revision, state) in &self.revision_states {
            if let Some(domain) = &state.drafting {
                domain.validate(*revision)?;
                max_domain(&mut max, domain);
            }
            if let Some(delta) = &state.delta {
                let mut local = zero_max();
                for c in &delta.changes {
                    max_change(&mut max, c);
                    max_change(&mut local, c);
                    validate_draft_change_record(c, *revision)?;
                }
                if !counters_above(&delta.counters.drafting, &local) {
                    return Err(draft_error(
                        "historical drafting allocator does not exceed journal IDs",
                    ));
                }
            }
        }
        for entry in self.undo_stack.iter().chain(&self.redo_stack) {
            for c in &entry.changes {
                max_change(&mut max, c);
                validate_draft_change_record(c, self.revision)?;
            }
        }
        for stored in self.command_receipts.values() {
            for id in &stored.receipt.created_drawings {
                max.drawing = max.drawing.max(id.get());
            }
            for id in &stored.receipt.created_draft_entities {
                max.entity = max.entity.max(id.get());
            }
        }
        if !counters_above(&self.draft_ids, &max) {
            return Err(draft_error("allocator loses historical high-water mark"));
        }
        Ok(())
    }
    pub(super) fn drafting_diff(&self, from: u64, to: u64) -> Result<DraftingDelta, CoreError> {
        let a = self
            .reconstruct_revision_state(from)?
            .drafting
            .unwrap_or_default();
        let b = self
            .reconstruct_revision_state(to)?
            .drafting
            .unwrap_or_default();
        let mut changes = Vec::new();
        for id in a
            .drawings
            .keys()
            .chain(b.drawings.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            match (a.drawings.get(&id), b.drawings.get(&id)) {
                (None, Some(d)) => changes.push(Change::Draft(DraftChange::Drawing {
                    drawing_id: id,
                    before: None,
                    after: Some(d.clone()),
                })),
                (Some(d), None) => changes.push(Change::Draft(DraftChange::Drawing {
                    drawing_id: id,
                    before: Some(d.clone()),
                    after: None,
                })),
                (Some(x), Some(y)) => {
                    if DrawingMetadata::of(x) != DrawingMetadata::of(y) {
                        changes.push(Change::DraftDrawingMetadata {
                            id,
                            before: DrawingMetadata::of(x),
                            after: DrawingMetadata::of(y),
                        });
                    }
                    for eid in x
                        .entities
                        .keys()
                        .chain(y.entities.keys())
                        .copied()
                        .collect::<BTreeSet<_>>()
                    {
                        if x.entities.get(&eid) != y.entities.get(&eid) {
                            changes.push(Change::Draft(DraftChange::Entity {
                                drawing_id: id,
                                id: eid,
                                before: x.entities.get(&eid).cloned(),
                                after: y.entities.get(&eid).cloned(),
                            }));
                        }
                    }
                    for lid in x
                        .layers
                        .keys()
                        .chain(y.layers.keys())
                        .copied()
                        .collect::<BTreeSet<_>>()
                    {
                        if x.layers.get(&lid) != y.layers.get(&lid) {
                            changes.push(Change::Draft(DraftChange::Layer {
                                drawing_id: id,
                                id: lid,
                                before: x.layers.get(&lid).cloned(),
                                after: y.layers.get(&lid).cloned(),
                            }));
                        }
                    }
                    for aid in x
                        .import_sources
                        .keys()
                        .chain(y.import_sources.keys())
                        .copied()
                        .collect::<BTreeSet<_>>()
                    {
                        if x.import_sources.get(&aid) != y.import_sources.get(&aid) {
                            changes.push(Change::Draft(DraftChange::ImportAsset {
                                drawing_id: id,
                                id: aid,
                                before: x.import_sources.get(&aid).cloned(),
                                after: y.import_sources.get(&aid).cloned(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }
        for id in a
            .views
            .keys()
            .chain(b.views.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            if a.views.get(&id) != b.views.get(&id) {
                changes.push(Change::DraftView {
                    id,
                    before: a.views.get(&id).cloned(),
                    after: b.views.get(&id).cloned(),
                });
            }
        }
        Ok(summarize_drafting(&changes))
    }
}
fn validate_draft_change_record(c: &Change, revision: u64) -> Result<(), CoreError> {
    match c {
        Change::Draft(DraftChange::Entity {
            drawing_id,
            id,
            before,
            after,
        }) => {
            if drawing_id.get() == 0 || id.get() == 0 {
                return Err(draft_error("zero journal entity ID"));
            }
            for e in before.iter().chain(after) {
                if e.id != *id || e.revision > revision {
                    return Err(draft_error("journal entity identity/revision mismatch"));
                }
                check(e.geometry.validate(GeometryLimits::default()))?;
                check(e.style.validate())?;
            }
        }
        Change::Draft(DraftChange::Layer {
            drawing_id,
            id,
            before,
            after,
        }) => {
            if drawing_id.get() == 0 || id.get() == 0 {
                return Err(draft_error("zero journal layer ID"));
            }
            for l in before.iter().chain(after) {
                if l.id != *id {
                    return Err(draft_error("journal layer ID mismatch"));
                }
                validate_name(&l.name, "draft layer")?;
                check(l.style.validate())?;
            }
        }
        Change::Draft(DraftChange::ImportAsset {
            drawing_id,
            id,
            before,
            after,
        }) => {
            if drawing_id.get() == 0 || id.get() == 0 {
                return Err(draft_error("zero journal asset ID"));
            }
            for a in before.iter().chain(after) {
                if a.id != *id {
                    return Err(draft_error("journal asset ID mismatch"));
                }
                check(a.source.validate())?;
            }
        }
        Change::Draft(DraftChange::Drawing {
            drawing_id,
            before,
            after,
        }) => {
            for d in before.iter().chain(after) {
                if d.id != *drawing_id {
                    return Err(draft_error("journal drawing ID mismatch"));
                }
                let domain = DraftingDomain {
                    drawings: BTreeMap::from([(*drawing_id, d.clone())]),
                    views: BTreeMap::new(),
                    vertex_count: drawing_vertices(d),
                };
                domain.validate(revision)?;
            }
        }
        Change::DraftDrawingMetadata { id, before, after } => {
            if id.get() == 0 {
                return Err(draft_error("zero metadata ID"));
            }
            for m in [before, after] {
                validate_name(&m.name, "drawing")?;
                check(m.plane.validate())?;
                if m.revision > revision {
                    return Err(draft_error("future drawing metadata"));
                }
            }
        }
        Change::DraftView { id, before, after } => {
            for v in before.iter().chain(after) {
                if id.get() == 0 || v.id != *id {
                    return Err(draft_error("journal view ID mismatch"));
                }
                validate_name(&v.name, "view")?;
                validate_source_value(&v.source)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn summarize_drafting(changes: &[Change]) -> DraftingDelta {
    let mut d = DraftingDelta::default();
    let mut drawings = BTreeMap::new();
    let mut entities = BTreeMap::new();
    let mut views = BTreeMap::new();
    let mut affected = BTreeSet::new();
    let mut layers = BTreeSet::new();
    let mut assets = BTreeSet::new();
    let mut metadata = BTreeSet::new();
    for c in changes {
        match c {
            Change::Draft(DraftChange::Entity {
                drawing_id,
                id,
                before,
                after,
            }) => {
                affected.insert(*drawing_id);
                let e = entities
                    .entry(*id)
                    .or_insert((before.clone(), after.clone()));
                e.1 = after.clone();
            }
            Change::Draft(DraftChange::Layer { drawing_id, id, .. }) => {
                affected.insert(*drawing_id);
                layers.insert(*id);
            }
            Change::Draft(DraftChange::ImportAsset { drawing_id, id, .. }) => {
                affected.insert(*drawing_id);
                assets.insert(*id);
            }
            Change::Draft(DraftChange::Drawing {
                drawing_id,
                before,
                after,
            }) => {
                affected.insert(*drawing_id);
                let e = drawings
                    .entry(*drawing_id)
                    .or_insert((before.clone(), after.clone()));
                e.1 = after.clone();
                let ids = before
                    .iter()
                    .flat_map(|d| d.entities.keys())
                    .chain(after.iter().flat_map(|d| d.entities.keys()))
                    .copied()
                    .collect::<BTreeSet<_>>();
                for id in ids {
                    let a = before.as_ref().and_then(|d| d.entities.get(&id)).cloned();
                    let b = after.as_ref().and_then(|d| d.entities.get(&id)).cloned();
                    let e = entities.entry(id).or_insert((a, b.clone()));
                    e.1 = b;
                }
                for d in before.iter().chain(after) {
                    layers.extend(d.layers.keys().copied());
                    assets.extend(d.import_sources.keys().copied());
                }
            }
            Change::DraftDrawingMetadata { id, before, after } => {
                if before != after {
                    affected.insert(*id);
                    metadata.insert(*id);
                }
            }
            Change::DraftView { id, before, after } => {
                for v in before.iter().chain(after) {
                    affected.insert(v.drawing_id);
                }
                let e = views.entry(*id).or_insert((before.clone(), after.clone()));
                e.1 = after.clone();
            }
            _ => {}
        }
    }
    for (id, (a, b)) in drawings {
        match (a, b) {
            (None, Some(_)) => d.created_drawings.push(id),
            (Some(_), None) => d.deleted_drawings.push(id),
            (Some(a), Some(b)) if a != b => {
                metadata.insert(id);
            }
            _ => {}
        }
    }
    for (id, (a, b)) in entities {
        match (a, b) {
            (None, Some(_)) => d.created_entities.push(id),
            (Some(_), None) => d.deleted_entities.push(id),
            (Some(a), Some(b)) if a != b => d.updated_entities.push(id),
            _ => {}
        }
    }
    for (id, (a, b)) in views {
        match (a, b) {
            (None, Some(_)) => d.created_views.push(id),
            (Some(_), None) => d.deleted_views.push(id),
            (Some(a), Some(b)) if a != b => d.updated_views.push(id),
            _ => {}
        }
    }
    d.updated_drawings = metadata
        .into_iter()
        .filter(|id| !d.created_drawings.contains(id) && !d.deleted_drawings.contains(id))
        .collect();
    d.changed_layers = layers.into_iter().collect();
    d.changed_import_assets = assets.into_iter().collect();
    d.affected_drawings = affected.into_iter().collect();
    let mut objects = BTreeSet::new();
    for c in changes {
        match c {
            Change::Draft(DraftChange::Entity { drawing_id, id, .. }) => {
                objects.insert(ProjectObjectRef::Drafting {
                    drawing_id: *drawing_id,
                    entity_id: *id,
                });
            }
            Change::Draft(DraftChange::Layer { drawing_id, id, .. }) => {
                objects.insert(ProjectObjectRef::DraftLayer {
                    drawing_id: *drawing_id,
                    layer_id: *id,
                });
            }
            Change::Draft(DraftChange::ImportAsset { drawing_id, id, .. }) => {
                objects.insert(ProjectObjectRef::ImportAsset {
                    drawing_id: *drawing_id,
                    import_asset_id: *id,
                });
            }
            Change::Draft(DraftChange::Drawing {
                drawing_id,
                before,
                after,
            }) => {
                objects.insert(ProjectObjectRef::Drawing {
                    drawing_id: *drawing_id,
                });
                for drawing in before.iter().chain(after) {
                    for id in drawing.entities.keys() {
                        objects.insert(ProjectObjectRef::Drafting {
                            drawing_id: *drawing_id,
                            entity_id: *id,
                        });
                    }
                    for id in drawing.layers.keys() {
                        objects.insert(ProjectObjectRef::DraftLayer {
                            drawing_id: *drawing_id,
                            layer_id: *id,
                        });
                    }
                    for id in drawing.import_sources.keys() {
                        objects.insert(ProjectObjectRef::ImportAsset {
                            drawing_id: *drawing_id,
                            import_asset_id: *id,
                        });
                    }
                }
            }
            Change::DraftDrawingMetadata { id, .. } => {
                objects.insert(ProjectObjectRef::Drawing { drawing_id: *id });
            }
            Change::DraftView { id, before, after } => {
                for v in before.iter().chain(after) {
                    objects.insert(ProjectObjectRef::ViewDefinition {
                        drawing_id: v.drawing_id,
                        view_definition_id: *id,
                    });
                }
            }
            _ => {}
        }
    }
    d.affected_objects = objects.into_iter().collect();
    d
}

pub(super) fn qualified_affected(
    ids: &[ElementId],
    drafting: &DraftingDelta,
    engineering: &EngineeringDelta,
    foundation_structural: &FoundationStructuralDelta,
    configurator: &StructuralConfiguratorDelta,
    geo: &GeoFoundationDelta,
) -> Vec<ProjectObjectRef> {
    ids.iter()
        .map(|id| ProjectObjectRef::Bim { element_id: *id })
        .chain(geo.state_changed.then_some(ProjectObjectRef::GeoState))
        .chain(geo.affected_models.iter().map(|id| ProjectObjectRef::GeoModel { model_id: id.clone() }))
        .chain(geo.affected_geo_snapshots.iter().map(|id| ProjectObjectRef::GeoSnapshot { snapshot_id: id.clone() }))
        .chain(geo.layout_changed.then_some(ProjectObjectRef::GeoFoundationLayout))
        .chain(geo.affected_snapshots.iter().map(|id| ProjectObjectRef::GeoFoundationAnalysisSnapshot { snapshot_id: *id }))
        .chain(foundation_structural.input_changed.then_some(ProjectObjectRef::FoundationStructuralInput))
        .chain(foundation_structural.affected_snapshots.iter().map(|id| ProjectObjectRef::EngineeringSnapshot { snapshot_id: *id }))
        .chain(configurator.affected_building_ids.iter().map(|id| ProjectObjectRef::ConfiguredBuilding { building_id: id.clone() }))
        .chain(configurator.affected_sections.iter().map(|s| ProjectObjectRef::ConfiguredSection { building_id: s.building_id.clone(), section_id: s.section_id.clone() }))
        .chain(configurator.affected_snapshots.iter().map(|id| ProjectObjectRef::ConfiguratorAnalysisSnapshot { snapshot_id: *id }))
        .chain(drafting.affected_objects.iter().cloned())
        .chain(
            engineering
                .affected_ground_sections
                .iter()
                .map(|id| ProjectObjectRef::GroundSection { ground_section_id: *id }),
        )
        .chain(
            engineering
                .affected_cases
                .iter()
                .map(|id| ProjectObjectRef::AnalysisCase { analysis_id: *id }),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Match the request to its original analytic effects independently of its retry string.
pub(super) fn drafting_command_effects_match(command: &DraftCommand, changes: &[Change]) -> bool {
    let records: Vec<_> = changes
        .iter()
        .filter_map(|c| {
            if let Change::Draft(c) = c {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    let metas: Vec<_> = changes
        .iter()
        .filter_map(|c| {
            if let Change::DraftDrawingMetadata { id, before, after } = c {
                Some((id, before, after))
            } else {
                None
            }
        })
        .collect();
    let views: Vec<_> = changes
        .iter()
        .filter_map(|c| {
            if let Change::DraftView { id, before, after } = c {
                Some((id, before, after))
            } else {
                None
            }
        })
        .collect();
    let metadata_for = |id: DrawingId| {
        metas
            .iter()
            .all(|(mid, a, b)| **mid == id && a.name == b.name && a.plane == b.plane)
    };
    match command {
        DraftCommand::CreateDrawing{name,plane}=>records.len()==1&&metas.is_empty()&&views.is_empty()&&matches!(records[0],DraftChange::Drawing{before:None,after:Some(d),..}if &d.name==name&&&d.plane==plane&&d.entities.is_empty()&&d.import_sources.is_empty()&&d.layers.len()==1&&d.layers.values().next().is_some_and(|l|l.name=="Layer0"&&l.visible&&!l.locked&&l.style==DraftStyle::default())),
        DraftCommand::RenameDrawing{drawing_id,name}=>records.is_empty()&&views.is_empty()&&metas.len()==1&&*metas[0].0==*drawing_id&&metas[0].2.name==*name&&metas[0].1.plane==metas[0].2.plane,
        DraftCommand::CreateDraftEntity{drawing_id,layer_id,geometry,style}=>records.len()==1&&views.is_empty()&&metadata_for(*drawing_id)&&matches!(records[0],DraftChange::Entity{drawing_id:d,before:None,after:Some(e),..}if d==drawing_id&&e.layer_id==*layer_id&&e.geometry==*geometry&&e.style==*style&&e.source.is_none()),
        DraftCommand::UpdateDraftEntity{drawing_id,entity_id,geometry}=>records.len()==1&&views.is_empty()&&metadata_for(*drawing_id)&&matches!(records[0],DraftChange::Entity{drawing_id:d,id,before:Some(a),after:Some(b)}if d==drawing_id&&id==entity_id&&b.geometry==*geometry&&a.layer_id==b.layer_id&&a.style==b.style&&a.source==b.source),
        DraftCommand::MoveDraftEntity{drawing_id,entity_ids,delta}=>views.is_empty()&&metadata_for(*drawing_id)&&records.len()==entity_ids.len()&&records.iter().all(|c|matches!(c,DraftChange::Entity{drawing_id:d,id,before:Some(a),after:Some(b)}if d==drawing_id&&entity_ids.contains(id)&&b.geometry==a.geometry.translated(*delta)&&a.layer_id==b.layer_id&&a.style==b.style&&a.source==b.source)),
        DraftCommand::DeleteDraftEntity{drawing_id,entity_ids}=>views.is_empty()&&metadata_for(*drawing_id)&&records.len()==entity_ids.len()&&records.iter().all(|c|matches!(c,DraftChange::Entity{drawing_id:d,id,before:Some(_),after:None}if d==drawing_id&&entity_ids.contains(id))),
        DraftCommand::CreateDraftLayer{drawing_id,name,visible,locked,style}=>records.len()==1&&views.is_empty()&&metadata_for(*drawing_id)&&matches!(records[0],DraftChange::Layer{drawing_id:d,before:None,after:Some(l),..}if d==drawing_id&&l.name==*name&&l.visible==*visible&&l.locked==*locked&&l.style==*style),
        DraftCommand::UpdateDraftLayer{drawing_id,layer_id,name,visible,locked,style}=>records.len()==1&&views.is_empty()&&metadata_for(*drawing_id)&&matches!(records[0],DraftChange::Layer{drawing_id:d,id,before:Some(a),after:Some(b)}if d==drawing_id&&id==layer_id&&b.name==name.as_ref().unwrap_or(&a.name).as_str()&&b.visible==visible.unwrap_or(a.visible)&&b.locked==locked.unwrap_or(a.locked)&&&b.style==style.as_ref().unwrap_or(&a.style)),
        DraftCommand::CreateViewDefinition{drawing_id,name,source}=>records.is_empty()&&metadata_for(*drawing_id)&&views.len()==1&&views[0].1.is_none()&&views[0].2.as_ref().is_some_and(|v|v.drawing_id==*drawing_id&&v.name==*name&&v.source==*source),
        DraftCommand::UpdateViewDefinition{view_definition_id,name,source}=>records.is_empty()&&views.len()==1&&*views[0].0==*view_definition_id&&match (&views[0].1,&views[0].2){(Some(a),Some(b))=>metadata_for(a.drawing_id)&&a.drawing_id==b.drawing_id&&&b.name==name.as_ref().unwrap_or(&a.name)&&&b.source==source.as_ref().unwrap_or(&a.source),_=>false},
        DraftCommand::ImportDraftingEntities{batch}=>{
            if !views.is_empty()||!metadata_for(batch.drawing_id)||records.len()!=batch.layers.len()+batch.entities.len()+1{return false;}
            let imported:Vec<_>=records.iter().filter_map(|c|match c{DraftChange::Entity{drawing_id,before:None,after:Some(e),..}if *drawing_id==batch.drawing_id=>Some(e),_=>None}).collect();
            let source=records.iter().find_map(|c|match c{DraftChange::ImportAsset{drawing_id,before:None,after:Some(a),..}if *drawing_id==batch.drawing_id=>Some(a),_=>None});
            source.is_some_and(|a|a.source==batch.source&&imported.len()==batch.entities.len()&&imported.iter().zip(&batch.entities).all(|(e,p)|e.geometry==p.geometry&&e.style==p.style&&e.source.as_ref().is_some_and(|s|s.import_asset_id==a.id&&s.source_handle==p.source_handle)&&records.iter().any(|c|matches!(c,DraftChange::Layer{after:Some(l),..}if l.id==e.layer_id&&(l.name==p.layer_name||l.name==format!("{} [asset {}]",p.layer_name,a.id))))))
        }
        DraftCommand::ImportLabDocument{source}=>source.validate().is_ok()&&metas.is_empty()&&views.is_empty()&&records.len()==source.drawings.len()&&records.iter().zip(&source.drawings).all(|(c,old)|matches!(c,DraftChange::Drawing{before:None,after:Some(new),..}if lab_drawing_effects_match(source,old,new))),
    }
}

impl drafting_core::DrawingSource for Document {
    fn drawing(&self, id: DrawingId) -> Option<&DraftDrawing> {
        Document::drawing(self, id)
    }
    fn cache_source_key(
        &self,
        id: DrawingId,
        epoch: u64,
    ) -> drafting_core::DraftResult<drafting_core::CacheSourceKey> {
        let drawing = self
            .drawing(id)
            .ok_or_else(|| DraftError::new("drawing_not_found", "drawing is absent"))?;
        Ok(drafting_core::CacheSourceKey {
            document_id: self.document_uuid.to_string(),
            runtime_instance_id: self.runtime_document_id.0.to_string(),
            document_epoch: epoch,
            document_revision: self.revision,
            drawing_id: id,
            drawing_revision: drawing.revision,
        })
    }
    fn snapshot(&self, id: DrawingId) -> drafting_core::DraftResult<drafting_core::DraftSnapshot> {
        use sha2::{Digest, Sha256};
        let d = self
            .drawing(id)
            .ok_or_else(|| DraftError::new("drawing_not_found", "drawing is absent"))?;
        let bytes =
            serde_json::to_vec(d).map_err(|e| DraftError::new("serialization", e.to_string()))?;
        Ok(drafting_core::DraftSnapshot {
            document_id: self.document_uuid.to_string(),
            document_revision: self.revision,
            drawing_id: id,
            name: d.name.clone(),
            revision: d.revision,
            plane: d.plane.clone(),
            entities: d.entities.clone(),
            layers: d.layers.clone(),
            import_sources: d.import_sources.clone(),
            source_fingerprint: format!("{:x}", Sha256::digest(&bytes)),
        })
    }
    fn confirmed_drafting_changes(
        &self,
        from: u64,
    ) -> drafting_core::DraftResult<Vec<DraftChange>> {
        if from.checked_add(1) != Some(self.revision) {
            return Err(DraftError::new(
                "cache_revision_gap",
                "confirmed source feed requires one contiguous project revision",
            ));
        }
        let delta = self
            .revision_states
            .get(&self.revision)
            .and_then(|s| s.delta.as_ref())
            .ok_or_else(|| {
                DraftError::new(
                    "cache_revision_gap",
                    "current revision has no retained authoritative journal",
                )
            })?;
        if delta.summary.revision_before != from || delta.summary.revision_after != self.revision {
            return Err(DraftError::new(
                "cache_revision_gap",
                "authoritative journal interval mismatch",
            ));
        }
        Ok(delta
            .changes
            .iter()
            .filter_map(|c| {
                if let Change::Draft(c) = c {
                    Some(c.clone())
                } else {
                    None
                }
            })
            .collect())
    }
}
impl Document {
    pub fn runtime_instance_id(&self) -> String {
        self.runtime_document_id.0.to_string()
    }
    pub fn drafting_snapshot(
        &self,
        id: DrawingId,
    ) -> Result<drafting_core::DraftSnapshot, CoreError> {
        check(drafting_core::DrawingSource::snapshot(self, id))
    }
}

fn draft_error(message: impl Into<String>) -> CoreError {
    CoreError::InvalidDocument(format!("drafting: {}", message.into()))
}
fn check<T>(result: Result<T, DraftError>) -> Result<T, CoreError> {
    result.map_err(|e| draft_error(e.to_string()))
}
fn require_unique_ids(ids: &[DraftEntityId]) -> Result<(), CoreError> {
    if ids.is_empty()
        || ids.len() > drafting_core::MAX_DOCUMENT_ENTITIES
        || ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(draft_error(
            "entity IDs must be nonempty, unique and within budget",
        ));
    }
    Ok(())
}

impl Document {
    pub fn drafting(&self) -> &DraftingDomain {
        &self.drafting
    }
    pub fn drawings(&self) -> &BTreeMap<DrawingId, DraftDrawing> {
        &self.drafting.drawings
    }
    pub fn drawing(&self, id: DrawingId) -> Option<&DraftDrawing> {
        self.drafting.drawings.get(&id)
    }
    pub fn draft_entity(&self, drawing: DrawingId, id: DraftEntityId) -> Option<&DraftEntity> {
        self.drawing(drawing)?.entities.get(&id)
    }
    pub fn view_definitions(&self) -> &BTreeMap<ViewDefinitionId, DraftViewDefinition> {
        &self.drafting.views
    }
    pub fn view_definition(&self, id: ViewDefinitionId) -> Option<&DraftViewDefinition> {
        self.drafting.views.get(&id)
    }
    fn require_drawing(&self, id: DrawingId) -> Result<&DraftDrawing, CoreError> {
        self.drawing(id)
            .ok_or_else(|| draft_error(format!("drawing {id} is absent")))
    }
    fn editable_entity(
        &self,
        drawing: DrawingId,
        id: DraftEntityId,
    ) -> Result<&DraftEntity, CoreError> {
        let d = self.require_drawing(drawing)?;
        let e = d
            .entities
            .get(&id)
            .ok_or_else(|| draft_error(format!("entity {id} is absent")))?;
        if d.layers.get(&e.layer_id).is_none_or(|l| l.locked) {
            return Err(draft_error("entity layer is absent or locked"));
        }
        Ok(e)
    }
    fn validate_reference(
        &self,
        source: &DraftSourceReference,
        require_existing: bool,
    ) -> Result<(), CoreError> {
        if source.element_ids.len() > 20_000
            || source.element_ids.iter().any(|id| id.get() == 0)
            || source
                .element_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != source.element_ids.len()
        {
            return Err(draft_error("invalid or duplicate BIM source IDs"));
        }
        if source.elevation_m.is_some_and(|v| !v.is_finite()) {
            return Err(draft_error("nonfinite reference elevation"));
        }
        if require_existing
            && source
                .element_ids
                .iter()
                .any(|id| self.element(*id).is_none())
        {
            return Err(draft_error("BIM source element is absent"));
        }
        Ok(())
    }
    fn touch_drawing(&mut self, id: DrawingId, changes: &mut Vec<Change>) -> Result<(), CoreError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        let d = self
            .drafting
            .drawings
            .get_mut(&id)
            .ok_or_else(|| draft_error("drawing absent"))?;
        let before = DrawingMetadata::of(d);
        d.revision = revision;
        let after = DrawingMetadata::of(d);
        if before != after {
            changes.push(Change::DraftDrawingMetadata { id, before, after });
        }
        Ok(())
    }

    pub(super) fn apply_draft_command(
        &mut self,
        command: DraftCommand,
        _operation: OperationId,
    ) -> Result<
        (
            OperationKind,
            Option<OperationId>,
            Vec<ElementId>,
            Vec<ElementId>,
            String,
            Vec<Change>,
            Vec<ElementId>,
        ),
        CoreError,
    > {
        let original = command.clone();
        let mut changes = Vec::new();
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(CoreError::RevisionExhausted)?;
        let original_counters = self.draft_ids.clone();
        let original_vertices = self.drafting.vertex_count;
        let application = (|| -> Result<(), CoreError> {
            match command {
                DraftCommand::CreateDrawing { name, plane } => {
                    validate_name(&name, "drawing")?;
                    check(plane.validate())?;
                    if self.drafting.drawings.len() >= drafting_core::MAX_DRAWINGS {
                        return Err(draft_error("drawing budget exhausted"));
                    }
                    let id = DrawingId::new(DraftAllocators::allocate(
                        &mut self.draft_ids.drawing,
                        "drawing",
                    )?);
                    let layer_id = DraftLayerId::new(DraftAllocators::allocate(
                        &mut self.draft_ids.layer,
                        "draft layer",
                    )?);
                    let layer = DraftLayer {
                        id: layer_id,
                        name: "Layer0".into(),
                        visible: true,
                        locked: false,
                        style: DraftStyle::default(),
                    };
                    let d = DraftDrawing {
                        id,
                        name,
                        revision: next_revision,
                        plane,
                        entities: BTreeMap::new(),
                        layers: BTreeMap::from([(layer_id, layer)]),
                        import_sources: BTreeMap::new(),
                    };
                    self.drafting.drawings.insert(id, d.clone());
                    changes.push(Change::Draft(DraftChange::Drawing {
                        drawing_id: id,
                        before: None,
                        after: Some(d),
                    }));
                }
                DraftCommand::RenameDrawing { drawing_id, name } => {
                    validate_name(&name, "drawing")?;
                    self.require_drawing(drawing_id)?;
                    let d = self.drafting.drawings.get_mut(&drawing_id).unwrap();
                    let before = DrawingMetadata::of(d);
                    d.name = name;
                    d.revision = next_revision;
                    changes.push(Change::DraftDrawingMetadata {
                        id: drawing_id,
                        before,
                        after: DrawingMetadata::of(d),
                    });
                }
                DraftCommand::CreateDraftEntity {
                    drawing_id,
                    layer_id,
                    geometry,
                    style,
                } => {
                    check(geometry.validate(GeometryLimits::default()))?;
                    check(style.validate())?;
                    let d = self.require_drawing(drawing_id)?;
                    if d.layers.get(&layer_id).is_none_or(|l| l.locked) {
                        return Err(draft_error("target layer is absent or locked"));
                    }
                    if self.drafting.entity_count() >= drafting_core::MAX_DOCUMENT_ENTITIES {
                        return Err(draft_error("entity budget exhausted"));
                    }
                    let id = DraftEntityId::new(DraftAllocators::allocate(
                        &mut self.draft_ids.entity,
                        "draft entity",
                    )?);
                    let e = DraftEntity {
                        id,
                        layer_id,
                        geometry,
                        style,
                        revision: next_revision,
                        source: None,
                    };
                    self.drafting
                        .drawings
                        .get_mut(&drawing_id)
                        .unwrap()
                        .entities
                        .insert(id, e.clone());
                    changes.push(Change::Draft(DraftChange::Entity {
                        drawing_id,
                        id,
                        before: None,
                        after: Some(e),
                    }));
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::UpdateDraftEntity {
                    drawing_id,
                    entity_id,
                    geometry,
                } => {
                    check(geometry.validate(GeometryLimits::default()))?;
                    let before = self.editable_entity(drawing_id, entity_id)?.clone();
                    let mut after = before.clone();
                    after.geometry = geometry;
                    after.revision = next_revision;
                    self.drafting
                        .drawings
                        .get_mut(&drawing_id)
                        .unwrap()
                        .entities
                        .insert(entity_id, after.clone());
                    changes.push(Change::Draft(DraftChange::Entity {
                        drawing_id,
                        id: entity_id,
                        before: Some(before),
                        after: Some(after),
                    }));
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::MoveDraftEntity {
                    drawing_id,
                    entity_ids,
                    delta,
                } => {
                    require_unique_ids(&entity_ids)?;
                    if !delta.is_finite() {
                        return Err(draft_error("nonfinite move delta"));
                    }
                    let mut staged = Vec::with_capacity(entity_ids.len());
                    for id in entity_ids {
                        let before = self.editable_entity(drawing_id, id)?.clone();
                        let mut after = before.clone();
                        after.geometry = before.geometry.translated(delta);
                        check(after.geometry.validate(GeometryLimits::default()))?;
                        after.revision = next_revision;
                        staged.push((id, before, after));
                    }
                    for (id, before, after) in staged {
                        self.drafting
                            .drawings
                            .get_mut(&drawing_id)
                            .unwrap()
                            .entities
                            .insert(id, after.clone());
                        changes.push(Change::Draft(DraftChange::Entity {
                            drawing_id,
                            id,
                            before: Some(before),
                            after: Some(after),
                        }));
                    }
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::DeleteDraftEntity {
                    drawing_id,
                    entity_ids,
                } => {
                    require_unique_ids(&entity_ids)?;
                    let staged = entity_ids
                        .iter()
                        .map(|id| self.editable_entity(drawing_id, *id).cloned())
                        .collect::<Result<Vec<_>, _>>()?;
                    for e in staged {
                        self.drafting
                            .drawings
                            .get_mut(&drawing_id)
                            .unwrap()
                            .entities
                            .remove(&e.id);
                        changes.push(Change::Draft(DraftChange::Entity {
                            drawing_id,
                            id: e.id,
                            before: Some(e),
                            after: None,
                        }));
                    }
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::CreateDraftLayer {
                    drawing_id,
                    name,
                    visible,
                    locked,
                    style,
                } => {
                    validate_name(&name, "draft layer")?;
                    check(style.validate())?;
                    let d = self.require_drawing(drawing_id)?;
                    if d.layers.len() >= drafting_core::MAX_LAYERS
                        || d.layers
                            .values()
                            .any(|l| l.name.eq_ignore_ascii_case(&name))
                    {
                        return Err(draft_error(
                            "layer budget exhausted or duplicate layer name",
                        ));
                    }
                    let id = DraftLayerId::new(DraftAllocators::allocate(
                        &mut self.draft_ids.layer,
                        "draft layer",
                    )?);
                    let l = DraftLayer {
                        id,
                        name,
                        visible,
                        locked,
                        style,
                    };
                    self.drafting
                        .drawings
                        .get_mut(&drawing_id)
                        .unwrap()
                        .layers
                        .insert(id, l.clone());
                    changes.push(Change::Draft(DraftChange::Layer {
                        drawing_id,
                        id,
                        before: None,
                        after: Some(l),
                    }));
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::UpdateDraftLayer {
                    drawing_id,
                    layer_id,
                    name,
                    visible,
                    locked,
                    style,
                } => {
                    let d = self.require_drawing(drawing_id)?;
                    let before = d
                        .layers
                        .get(&layer_id)
                        .ok_or_else(|| draft_error("layer absent"))?
                        .clone();
                    let mut after = before.clone();
                    if let Some(v) = name {
                        validate_name(&v, "draft layer")?;
                        if d.layers
                            .values()
                            .any(|l| l.id != layer_id && l.name.eq_ignore_ascii_case(&v))
                        {
                            return Err(draft_error("duplicate layer name"));
                        }
                        after.name = v;
                    }
                    if let Some(v) = visible {
                        after.visible = v;
                    }
                    if let Some(v) = locked {
                        after.locked = v;
                    }
                    if let Some(v) = style {
                        check(v.validate())?;
                        after.style = v;
                    }
                    self.drafting
                        .drawings
                        .get_mut(&drawing_id)
                        .unwrap()
                        .layers
                        .insert(layer_id, after.clone());
                    changes.push(Change::Draft(DraftChange::Layer {
                        drawing_id,
                        id: layer_id,
                        before: Some(before),
                        after: Some(after),
                    }));
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::CreateViewDefinition {
                    drawing_id,
                    name,
                    source,
                } => {
                    self.require_drawing(drawing_id)?;
                    validate_name(&name, "draft view")?;
                    self.validate_reference(&source, true)?;
                    let id = ViewDefinitionId::new(DraftAllocators::allocate(
                        &mut self.draft_ids.view,
                        "draft view",
                    )?);
                    let v = DraftViewDefinition {
                        id,
                        drawing_id,
                        name,
                        source,
                    };
                    self.drafting.views.insert(id, v.clone());
                    changes.push(Change::DraftView {
                        id,
                        before: None,
                        after: Some(v),
                    });
                    self.touch_drawing(drawing_id, &mut changes)?;
                }
                DraftCommand::UpdateViewDefinition {
                    view_definition_id,
                    name,
                    source,
                } => {
                    let before = self
                        .drafting
                        .views
                        .get(&view_definition_id)
                        .ok_or_else(|| draft_error("view absent"))?
                        .clone();
                    let mut after = before.clone();
                    if let Some(v) = name {
                        validate_name(&v, "draft view")?;
                        after.name = v;
                    }
                    if let Some(v) = source {
                        self.validate_reference(&v, true)?;
                        after.source = v;
                    }
                    self.drafting
                        .views
                        .insert(view_definition_id, after.clone());
                    changes.push(Change::DraftView {
                        id: view_definition_id,
                        before: Some(before),
                        after: Some(after.clone()),
                    });
                    self.touch_drawing(after.drawing_id, &mut changes)?;
                }
                DraftCommand::ImportDraftingEntities { batch } => {
                    self.apply_draft_import(batch, next_revision, &mut changes)?;
                }
                DraftCommand::ImportLabDocument { source } => {
                    self.apply_lab_import(&source, next_revision, &mut changes)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = application {
            for change in changes.iter().rev() {
                self.apply_change(change, false);
            }
            self.draft_ids = original_counters;
            self.drafting.vertex_count = original_vertices;
            return Err(error);
        }
        let (removed, added) = draft_vertices_changed(&changes);
        self.drafting.vertex_count = original_vertices
            .saturating_sub(removed)
            .saturating_add(added);
        if self.drafting.vertex_count > drafting_core::MAX_DOCUMENT_VERTICES {
            for change in changes.iter().rev() {
                self.apply_change(change, false);
            }
            self.draft_ids = original_counters;
            self.drafting.vertex_count = original_vertices;
            return Err(draft_error("document vertex budget exceeded"));
        }
        Ok((
            OperationKind::DraftingChanged { command: original },
            None,
            vec![],
            vec![],
            "Drafting change".into(),
            changes,
            vec![],
        ))
    }

    fn apply_draft_import(
        &mut self,
        batch: ImportBatch,
        revision: u64,
        changes: &mut Vec<Change>,
    ) -> Result<(), CoreError> {
        check(batch.source.validate())?;
        self.require_drawing(batch.drawing_id)?;
        if batch.layers.len() > drafting_core::MAX_LAYERS
            || batch.entities.len() > drafting_core::MAX_DOCUMENT_ENTITIES
            || self
                .drafting
                .entity_count()
                .saturating_add(batch.entities.len())
                > drafting_core::MAX_DOCUMENT_ENTITIES
        {
            return Err(draft_error("import budget exceeded"));
        }
        let mut layer_names = BTreeSet::new();
        for l in &batch.layers {
            validate_name(&l.name, "import layer")?;
            check(l.style.validate())?;
            if !layer_names.insert(l.name.to_lowercase()) {
                return Err(draft_error("duplicate import layer name"));
            }
        }
        let mut handles = BTreeSet::new();
        for e in &batch.entities {
            check(e.geometry.validate(GeometryLimits::default()))?;
            check(e.style.validate())?;
            if !layer_names.contains(&e.layer_name.to_lowercase()) {
                return Err(draft_error("import entity references absent layer"));
            }
            if let Some(h) = &e.source_handle {
                if h.is_empty() || h.len() > 256 || !handles.insert(h.to_uppercase()) {
                    return Err(draft_error(
                        "duplicate or invalid source handle in import asset",
                    ));
                }
            }
        }
        let current_layers = self.require_drawing(batch.drawing_id)?.layers.len();
        if current_layers.saturating_add(batch.layers.len()) > drafting_core::MAX_LAYERS {
            return Err(draft_error("import layer budget exceeded"));
        }
        // Allocate only after all untrusted payload geometry and references pass.
        let asset_id = ImportAssetId::new(DraftAllocators::allocate(
            &mut self.draft_ids.import_asset,
            "draft import asset",
        )?);
        let asset = DraftImportAsset {
            id: asset_id,
            source: batch.source,
        };
        let mut layers = BTreeMap::new();
        for l in batch.layers {
            let id = DraftLayerId::new(DraftAllocators::allocate(
                &mut self.draft_ids.layer,
                "draft layer",
            )?);
            // Imports have independent layer identity even when the display name already exists.
            let existing = self.require_drawing(batch.drawing_id)?;
            let mut name = l.name.clone();
            if existing
                .layers
                .values()
                .any(|v| v.name.eq_ignore_ascii_case(&name))
            {
                name = format!("{} [asset {}]", name, asset_id);
            }
            let layer = DraftLayer {
                id,
                name,
                visible: l.visible,
                locked: l.locked,
                style: l.style,
            };
            layers.insert(l.name.to_lowercase(), id);
            self.drafting
                .drawings
                .get_mut(&batch.drawing_id)
                .unwrap()
                .layers
                .insert(id, layer.clone());
            changes.push(Change::Draft(DraftChange::Layer {
                drawing_id: batch.drawing_id,
                id,
                before: None,
                after: Some(layer),
            }));
        }
        self.drafting
            .drawings
            .get_mut(&batch.drawing_id)
            .unwrap()
            .import_sources
            .insert(asset_id, asset.clone());
        changes.push(Change::Draft(DraftChange::ImportAsset {
            drawing_id: batch.drawing_id,
            id: asset_id,
            before: None,
            after: Some(asset),
        }));
        for e in batch.entities {
            let id = DraftEntityId::new(DraftAllocators::allocate(
                &mut self.draft_ids.entity,
                "draft entity",
            )?);
            let entity = DraftEntity {
                id,
                layer_id: layers[&e.layer_name.to_lowercase()],
                geometry: e.geometry,
                style: e.style,
                revision,
                source: Some(drafting_core::EntityProvenance {
                    import_asset_id: asset_id,
                    source_handle: e.source_handle,
                }),
            };
            self.drafting
                .drawings
                .get_mut(&batch.drawing_id)
                .unwrap()
                .entities
                .insert(id, entity.clone());
            changes.push(Change::Draft(DraftChange::Entity {
                drawing_id: batch.drawing_id,
                id,
                before: None,
                after: Some(entity),
            }));
        }
        self.touch_drawing(batch.drawing_id, changes)
    }
}
