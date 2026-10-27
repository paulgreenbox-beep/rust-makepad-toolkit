//! Authoritative BIM entities, graph, property resolution and revision observation.
use super::*;

pub(super) fn one() -> u64 {
    1
}
pub(super) fn extended_schema() -> u16 {
    1
}

fn engineering_operation_high_water(domain: &EngineeringDomain) -> u64 {
    let mut maximum = domain.slope_cases.keys().map(|id| id.get()).max().unwrap_or(0);
    for (id, ground) in &domain.ground_sections {
        maximum = maximum.max(id.get()).max(ground.source_operation_id.get());
    }
    for (id, case) in &domain.circular_cases {
        maximum = maximum.max(id.get()).max(case.source_operation_id.get());
    }
    maximum
}

macro_rules! numeric_id {
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
        pub struct $name(u64);
        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }
        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}
numeric_id!(TypeId);
numeric_id!(LevelId);
numeric_id!(RelationshipId);

/// Read-only allocation quote. It reserves nothing; apply a batch using the
/// same expected revision so another edit cannot consume a quoted ID unnoticed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAllocationPreview {
    pub revision: u64,
    pub element_id: ElementId,
    pub type_id: TypeId,
    pub level_id: LevelId,
    pub relationship_id: RelationshipId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityRef {
    Element(ElementId),
    Type(TypeId),
    Level(LevelId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RectangularSpec {
    pub center: Point2,
    pub elevation_m: f64,
    pub width_m: f64,
    pub height_m: f64,
    pub depth_m: f64,
    #[serde(default)]
    pub rotation_rad: f64,
}
pub type OpeningSpec = RectangularSpec;
/// Planar contour in local placement coordinates. The host determines the
/// plane basis; depth follows the host extrusion axis (vertical for slabs and
/// roofs, across the baseline for walls).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolygonalOpeningSpec {
    pub center: Point2,
    pub elevation_m: f64,
    pub boundary: Vec<Point2>,
    pub depth_m: f64,
    #[serde(default)]
    pub rotation_rad: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpaceSpec {
    #[serde(default)]
    pub boundary: Option<Vec<Point2>>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}
pub type Space = ElementRecord;
pub type DoorSpec = RectangularSpec;
pub type WindowSpec = RectangularSpec;
pub type SlabSpec = FloorSpec;
pub type ElementInstance = ElementRecord;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElementType {
    pub id: TypeId,
    pub name: String,
    pub class_id: ClassId,
    /// Explicit type properties only. Class defaults are resolved at read time.
    pub properties: BTreeMap<PropertyId, PropertyValue>,
}
pub type WallType = ElementType;
pub type SlabType = ElementType;
pub type RoofType = ElementType;
pub type OpeningType = ElementType;
pub type DoorType = ElementType;
pub type WindowType = ElementType;
pub type MaterialType = ElementType;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub id: LevelId,
    pub name: String,
    pub elevation_m: f64,
    pub height_m: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipType {
    Hosts,
    HostedBy,
    Contains,
    ContainedIn,
    ConnectsTo,
    SupportedBy,
    Supports,
    Bounds,
    References,
    InstanceOf,
    BelongsToLevel,
    BoundsSpace,
    ContainedInSpace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    #[serde(rename = "relationship_id", alias = "id")]
    pub id: RelationshipId,
    pub relationship_type: RelationshipType,
    pub source_id: EntityRef,
    pub target_id: EntityRef,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
    pub revision_created: u64,
    pub revision_deleted: Option<u64>,
}
impl Relationship {
    pub fn is_active(&self) -> bool {
        self.revision_deleted.is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteStrategy {
    #[default]
    Reject,
    Detach,
    Cascade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertySource {
    InstanceOverride,
    Type,
    ClassDefault,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedProperty {
    pub property: PropertyId,
    pub value: PropertyValue,
    pub source: PropertySource,
    pub type_id: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub entity: EntityRef,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RevisionDiff {
    #[serde(default, skip_serializing_if = "GeoFoundationDelta::is_empty")]
    pub geo_foundation: GeoFoundationDelta,
    #[serde(default, skip_serializing_if = "FoundationStructuralDelta::is_empty")]
    pub foundation_structural: FoundationStructuralDelta,
    #[serde(default, skip_serializing_if = "StructuralConfiguratorDelta::is_empty")]
    pub structural_configurator: StructuralConfiguratorDelta,
    #[serde(default)]
    pub engineering: EngineeringDelta,
    #[serde(default)]
    pub drafting: DraftingDelta,
    pub revision_from: u64,
    pub revision_to: u64,
    pub created: Vec<ElementId>,
    pub modified: Vec<ElementId>,
    pub deleted: Vec<ElementId>,
    pub relationships_created: Vec<RelationshipId>,
    pub relationships_deleted: Vec<RelationshipId>,
    pub types_modified: Vec<TypeId>,
    pub types_created: Vec<TypeId>,
    pub types_deleted: Vec<TypeId>,
    pub levels_created: Vec<LevelId>,
    pub levels_modified: Vec<LevelId>,
    pub levels_deleted: Vec<LevelId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub element_id: ElementId,
    pub direct_dependencies: Vec<ElementId>,
    pub dependents: Vec<ElementId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticNeighbor {
    pub entity: EntityRef,
    pub relationship_ids: Vec<RelationshipId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeleteImpact {
    pub element_id: ElementId,
    pub dependents: Vec<ElementId>,
    pub hosted_elements: Vec<ElementId>,
    pub relationship_ids: Vec<RelationshipId>,
}

/// Actual compact JSON sizes; history includes undo/redo entries, retained
/// revision snapshots and idempotent command records. It is a subset of the
/// document size and does not recursively call this measurement.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMetrics {
    pub history_entry_count: usize,
    pub history_serialized_bytes: u64,
    pub document_serialized_bytes: u64,
    pub checkpoint_count: usize,
    pub delta_count: usize,
    pub serialized_history_bytes: u64,
    pub avg_delta_bytes: u64,
    pub largest_delta_bytes: u64,
    pub reconstruction_time_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct RevisionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) geo_foundation: Option<GeoFoundationDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) structural_configurator: Option<StructuralConfiguratorDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) foundation_structural: Option<FoundationStructuralDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) engineering: Option<EngineeringDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) drafting: Option<DraftingDomain>,
    pub(super) elements: BTreeMap<ElementId, ElementRecord>,
    pub(super) types: BTreeMap<TypeId, ElementType>,
    pub(super) levels: BTreeMap<LevelId, Level>,
    pub(super) relationships: BTreeMap<RelationshipId, Relationship>,
    /// Preserve explicit instance state alongside the resolved projection so a
    /// source change remains observable even when its numeric value is equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) instances: Option<BTreeMap<ElementId, ElementRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) property_sources: Option<BTreeMap<ElementId, BTreeMap<PropertyId, PropertySource>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) delta: Option<RevisionDelta>,
    #[serde(default = "checkpoint_default")]
    pub(super) checkpoint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) classes: Option<BTreeMap<ClassId, ClassDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) operations: Option<BTreeMap<OperationId, OperationRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) relationship_records: Option<BTreeMap<RelationshipId, Relationship>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) history_stack_anchor: Option<HistoryStackAnchor>,
}
fn checkpoint_default() -> bool {
    true
}

pub(super) type Applied = (
    OperationKind,
    Option<OperationId>,
    Vec<ElementId>,
    Vec<ElementId>,
    String,
    Vec<Change>,
    Vec<ElementId>,
);

fn bim_error(entity: EntityRef, message: impl Into<String>) -> CoreError {
    CoreError::BimValidation {
        entity,
        message: message.into(),
    }
}
fn meter(value: f64) -> PropertyValue {
    PropertyValue::Number {
        value,
        unit: Unit::Meter,
    }
}

fn parametric_properties(kind: &ElementKind) -> BTreeMap<PropertyId, PropertyValue> {
    let values = match kind {
        ElementKind::Wall(spec) => vec![
            ("height", meter(spec.height_m)),
            ("thickness", meter(spec.thickness_m)),
        ],
        ElementKind::Floor(spec) => vec![("thickness", meter(spec.thickness_m))],
        ElementKind::Roof(spec) => vec![
            ("thickness", meter(spec.thickness_m)),
            (
                "slope_x",
                PropertyValue::Number {
                    value: spec.slope_x,
                    unit: Unit::Unitless,
                },
            ),
            (
                "slope_y",
                PropertyValue::Number {
                    value: spec.slope_y,
                    unit: Unit::Unitless,
                },
            ),
        ],
        ElementKind::Opening(spec) | ElementKind::Window(spec) | ElementKind::Door(spec) => vec![
            ("width", meter(spec.width_m)),
            ("height", meter(spec.height_m)),
            ("depth", meter(spec.depth_m)),
            (
                "rotation",
                PropertyValue::Number {
                    value: spec.rotation_rad,
                    unit: Unit::Radian,
                },
            ),
        ],
        ElementKind::PolygonalOpening(spec) => vec![
            ("depth", meter(spec.depth_m)),
            (
                "rotation",
                PropertyValue::Number {
                    value: spec.rotation_rad,
                    unit: Unit::Radian,
                },
            ),
        ],
        ElementKind::Space(_) => Vec::new(),
    };
    values
        .into_iter()
        .map(|(id, value)| (id.into(), value))
        .collect()
}

/// Intrinsic definitions are a read-time fallback for old classes. Keeping
/// them outside old class records avoids introducing new inheritance conflicts.
fn intrinsic_property_definitions() -> &'static BTreeMap<ClassId, ClassDef> {
    static DEFAULTS: std::sync::OnceLock<BTreeMap<ClassId, ClassDef>> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(|| {
        let mut classes = BTreeMap::new();
        for (id, name, category) in [
            (BUILTIN_WALL_CLASS_ID, "Wall", ElementCategory::Wall),
            (BUILTIN_FLOOR_CLASS_ID, "Floor", ElementCategory::Floor),
            (BUILTIN_ROOF_CLASS_ID, "Roof", ElementCategory::Roof),
            (
                BUILTIN_OPENING_CLASS_ID,
                "Opening",
                ElementCategory::Opening,
            ),
            (BUILTIN_DOOR_CLASS_ID, "Door", ElementCategory::Door),
            (BUILTIN_WINDOW_CLASS_ID, "Window", ElementCategory::Window),
            (BUILTIN_SPACE_CLASS_ID, "Space", ElementCategory::Space),
            (
                BUILTIN_MATERIAL_CLASS_ID,
                "Material",
                ElementCategory::Material,
            ),
        ] {
            classes.insert(ClassId::from(id), ClassDef::new(id, name, category));
        }
        for (id, dimensions) in [
            (
                BUILTIN_WALL_CLASS_ID,
                vec![("thickness", 0.2), ("height", 3.0)],
            ),
            (BUILTIN_FLOOR_CLASS_ID, vec![("thickness", 0.2)]),
            (BUILTIN_ROOF_CLASS_ID, vec![("thickness", 0.15)]),
            (
                BUILTIN_OPENING_CLASS_ID,
                vec![("width", 1.2), ("height", 1.2), ("depth", 0.2)],
            ),
            (
                BUILTIN_DOOR_CLASS_ID,
                vec![("width", 0.9), ("height", 2.1), ("depth", 0.1)],
            ),
            (
                BUILTIN_WINDOW_CLASS_ID,
                vec![("width", 1.2), ("height", 1.2), ("depth", 0.1)],
            ),
        ] {
            if let Some(class) = classes.get_mut(&ClassId::from(id)) {
                for (property, value) in dimensions {
                    class.properties.entry(property.into()).or_insert_with(|| {
                        PropertyDef::new(
                            property,
                            property,
                            PropertyType::Number { unit: Unit::Meter },
                        )
                        .with_default(meter(value))
                    });
                }
            }
        }
        if let Some(class) = classes.get_mut(&ClassId::from(BUILTIN_ROOF_CLASS_ID)) {
            for key in ["slope_x", "slope_y"] {
                class.properties.entry(key.into()).or_insert_with(|| {
                    PropertyDef::new(
                        key,
                        key,
                        PropertyType::Number {
                            unit: Unit::Unitless,
                        },
                    )
                    .with_default(PropertyValue::Number {
                        value: 0.0,
                        unit: Unit::Unitless,
                    })
                });
            }
        }
        for id in [
            BUILTIN_OPENING_CLASS_ID,
            BUILTIN_WINDOW_CLASS_ID,
            BUILTIN_DOOR_CLASS_ID,
        ] {
            if let Some(class) = classes.get_mut(&ClassId::from(id)) {
                class
                    .properties
                    .entry("rotation".into())
                    .or_insert_with(|| {
                        PropertyDef::new(
                            "rotation",
                            "rotation",
                            PropertyType::Number { unit: Unit::Radian },
                        )
                        .with_default(PropertyValue::Number {
                            value: 0.0,
                            unit: Unit::Radian,
                        })
                    });
            }
        }

        classes
    })
}

impl Document {
    pub fn allocation_preview(&self) -> IdAllocationPreview {
        IdAllocationPreview {
            revision: self.revision,
            element_id: ElementId::new(self.next_element_id),
            type_id: TypeId::new(self.next_type_id),
            level_id: LevelId::new(self.next_level_id),
            relationship_id: RelationshipId::new(self.next_relationship_id),
        }
    }

    fn builtin_class_id_for_category(category: ElementCategory) -> Option<ClassId> {
        match category {
            ElementCategory::Wall => Some(BUILTIN_WALL_CLASS_ID.into()),
            ElementCategory::Floor => Some(BUILTIN_FLOOR_CLASS_ID.into()),
            ElementCategory::Roof => Some(BUILTIN_ROOF_CLASS_ID.into()),
            ElementCategory::Opening => Some(BUILTIN_OPENING_CLASS_ID.into()),
            ElementCategory::Door => Some(BUILTIN_DOOR_CLASS_ID.into()),
            ElementCategory::Window => Some(BUILTIN_WINDOW_CLASS_ID.into()),
            ElementCategory::Material => Some(BUILTIN_MATERIAL_CLASS_ID.into()),
            ElementCategory::Space => Some(BUILTIN_SPACE_CLASS_ID.into()),
            ElementCategory::Generic => None,
        }
    }
    pub(super) fn builtin_property_definition(
        &self,
        class_id: &ClassId,
        property_id: &PropertyId,
    ) -> Option<&PropertyDef> {
        let builtin = Self::builtin_class_id_for_category(self.effective_category(class_id).ok()?)?;
        self.classes
            .get(&builtin)
            .and_then(|class| class.properties.get(property_id))
            .or_else(|| {
                intrinsic_property_definitions()
                    .get(&builtin)?
                    .properties
                    .get(property_id)
            })
    }
    pub(super) fn builtin_properties_for_class(
        &self,
        class_id: &ClassId,
    ) -> BTreeMap<PropertyId, PropertyDef> {
        Self::builtin_class_id_for_category(
            self.effective_category(class_id)
                .unwrap_or(ElementCategory::Generic),
        )
        .map(|id| {
            let mut definitions = intrinsic_property_definitions()
                .get(&id)
                .map(|class| class.properties.clone())
                .unwrap_or_default();
            if let Some(class) = self.classes.get(&id) {
                definitions.extend(class.properties.clone());
            }
            definitions
        })
        .unwrap_or_default()
    }
    pub fn types(&self) -> impl DoubleEndedIterator<Item = &ElementType> + ExactSizeIterator {
        self.scan_counters.enumerate(
            self.types.values(),
            self.types.len(),
            "type_enumeration",
            "caller requested element types",
            CollectionKind::Other,
        )
    }
    pub fn materials(&self) -> impl Iterator<Item = &MaterialType> {
        self.types().filter(|t| {
            self.effective_category(&t.class_id).ok() == Some(ElementCategory::Material)
        })
    }
    pub fn element_type(&self, id: TypeId) -> Option<&ElementType> {
        self.types.get(&id)
    }
    pub fn levels(&self) -> impl DoubleEndedIterator<Item = &Level> + ExactSizeIterator {
        self.scan_counters.enumerate(
            self.levels.values(),
            self.levels.len(),
            "level_enumeration",
            "caller requested levels",
            CollectionKind::Other,
        )
    }
    pub fn level(&self, id: LevelId) -> Option<&Level> {
        self.levels.get(&id)
    }
    pub fn relationships(&self) -> impl Iterator<Item = &Relationship> {
        let inner = self.indexes().active_ids.iter().filter_map(|id| {
            self.scan_counters.relationship_scan(1);
            self.relationships.get(id)
        });
        self.scan_counters.enumerate(
            inner,
            self.indexes().active_ids.len(),
            "relationship_enumeration",
            "caller requested active relationship graph records",
            CollectionKind::Relationships,
        )
    }
    /// Includes tombstones so a relationship's permanent ID remains observable after deletion.
    pub fn relationship(&self, id: RelationshipId) -> Option<&Relationship> {
        self.relationships.get(&id)
    }
    pub fn element_relationships(&self, id: ElementId) -> Vec<&Relationship> {
        self.indexes()
            .by_entity
            .get(&EntityRef::Element(id))
            .into_iter()
            .flatten()
            .filter_map(|id| {
                self.scan_counters.relationship_scan(1);
                self.relationships.get(id)
            })
            .collect()
    }
    pub fn element_level(&self, id: ElementId) -> Option<LevelId> {
        self.indexes()
            .element_levels
            .get(&id)?
            .keys()
            .next()
            .copied()
    }
    pub fn type_instances(&self, id: TypeId) -> Vec<ElementId> {
        self.indexes()
            .type_instances
            .get(&id)
            .into_iter()
            .flat_map(|ids| ids.keys())
            .copied()
            .collect()
    }
    pub fn level_elements(&self, id: LevelId) -> Vec<ElementId> {
        self.indexes()
            .level_elements
            .get(&id)
            .into_iter()
            .flat_map(|ids| ids.keys())
            .copied()
            .collect()
    }
    /// Normalized HOSTS/HOSTED_BY lookup. Valid documents have one host per child.
    pub fn host_of(&self, id: ElementId) -> Option<ElementId> {
        self.indexes().host_of.get(&id)?.keys().next().copied()
    }
    pub fn hosted_element_ids(&self, host_id: ElementId) -> Vec<ElementId> {
        self.indexes()
            .hosts
            .get(&host_id)
            .into_iter()
            .flat_map(|ids| ids.keys())
            .copied()
            .collect()
    }
    pub fn hosted_opening_ids(&self, host_id: ElementId) -> Vec<ElementId> {
        self.hosted_element_ids(host_id)
            .into_iter()
            .filter(|id| {
                self.element(*id).is_some_and(|e| {
                    matches!(
                        e.kind,
                        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                    )
                })
            })
            .collect()
    }
    pub fn history_metrics(&self) -> Result<HistoryMetrics, CoreError> {
        fn encoded_len<T: Serialize + ?Sized>(value: &T) -> Result<u64, CoreError> {
            u64::try_from(circular_archive::bounded_serialized_size(value, usize::MAX)?)
                .map_err(|_| CoreError::InvalidDocument("serialized history size exceeds u64".into()))
        }
        let _scan = self.scan_counters.track(
            "history_size_measurement",
            self.elements.len(),
            self.relationships.len(),
            "explicit request for actual serialized history and document byte sizes",
            true,
        );
        let history_bytes = encoded_len(&(
            &self.undo_stack,
            &self.redo_stack,
            &self.revision_states,
            &self.command_receipts,
        ))?;
        let mut sizes = Vec::new();
        for state in self.revision_states.values().filter(|s| !s.checkpoint) {
            sizes.push(encoded_len(state)?);
        }
        let reconstruction_start = Instant::now();
        let reconstructed = self.reconstruct_revision_state(self.revision)?;
        let reconstruction_time_ms = reconstruction_start
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        drop(reconstructed);
        Ok(HistoryMetrics {
            history_entry_count: self.undo_stack.len() + self.redo_stack.len(),
            history_serialized_bytes: history_bytes,
            document_serialized_bytes: encoded_len(self)?,
            checkpoint_count: self
                .revision_states
                .values()
                .filter(|s| s.checkpoint)
                .count(),
            delta_count: sizes.len(),
            serialized_history_bytes: history_bytes,
            avg_delta_bytes: if sizes.is_empty() {
                0
            } else {
                sizes.iter().sum::<u64>() / sizes.len() as u64
            },
            largest_delta_bytes: sizes.iter().copied().max().unwrap_or(0),
            reconstruction_time_ms,
        })
    }
    pub fn dirty_ids(&self) -> Vec<ElementId> {
        self.dirty_elements.iter().copied().collect()
    }

    pub(super) fn element_property_definitions(
        &self,
        element: &ElementRecord,
    ) -> Result<BTreeMap<PropertyId, PropertyDef>, CoreError> {
        let mut definitions = self.properties_for_class(&element.class_id)?;
        if let Some(type_id) = element.type_id {
            let element_type = self
                .types
                .get(&type_id)
                .ok_or(CoreError::UnknownType(type_id))?;
            // A compatible type can extend a category with additional fields.
            // Instance class definitions retain priority for shared fields.
            for (id, definition) in self.properties_for_class(&element_type.class_id)? {
                definitions.entry(id).or_insert(definition);
            }
        }
        Ok(definitions)
    }

    pub fn resolved_properties(
        &self,
        id: ElementId,
    ) -> Result<BTreeMap<PropertyId, ResolvedProperty>, CoreError> {
        let element = self.element(id).ok_or(CoreError::UnknownElement(id))?;
        let mut values = BTreeMap::new();
        let definitions = self.element_property_definitions(element)?;
        for (property, definition) in &definitions {
            if let Some(value) = &definition.default_value {
                values.insert(
                    property.clone(),
                    ResolvedProperty {
                        property: property.clone(),
                        value: value.clone(),
                        source: PropertySource::ClassDefault,
                        type_id: None,
                    },
                );
            }
        }
        if let Some(type_id) = element.type_id {
            let element_type = self
                .types
                .get(&type_id)
                .ok_or(CoreError::UnknownType(type_id))?;
            for (property, value) in &element_type.properties {
                values.insert(
                    property.clone(),
                    ResolvedProperty {
                        property: property.clone(),
                        value: value.clone(),
                        source: PropertySource::Type,
                        type_id: Some(type_id),
                    },
                );
            }
        }
        // Untyped v1 dimensions are explicit instance parameters. Rotation
        // remains explicit placement even when a type is assigned. Report these
        // values without copying them into persistent property overrides.
        for (property, value) in parametric_properties(&element.kind) {
            if (element.type_id.is_none() || property.as_str() == "rotation")
                && definitions
                    .get(&property)
                    .is_some_and(|definition| definition.value_type == value.property_type())
            {
                values.insert(
                    property.clone(),
                    ResolvedProperty {
                        property,
                        value,
                        source: PropertySource::InstanceOverride,
                        type_id: element.type_id,
                    },
                );
            }
        }
        for (property, value) in &element.properties {
            values.insert(
                property.clone(),
                ResolvedProperty {
                    property: property.clone(),
                    value: value.clone(),
                    source: PropertySource::InstanceOverride,
                    type_id: element.type_id,
                },
            );
        }
        Ok(values)
    }

    /// Derived parameter projection. The persistent instance stores its placement
    /// and explicit overrides; inherited values are never written into it.
    pub fn effective_element(&self, id: ElementId) -> Result<ElementRecord, CoreError> {
        let mut element = self
            .element(id)
            .ok_or(CoreError::UnknownElement(id))?
            .clone();
        let resolved = self.resolved_properties(id)?;
        let number = |key: &str, old: f64| -> f64 {
            // Old untyped v1 specifications retain their dimensions. With a type,
            // dimensions resolve through override > type > class default.
            if element.type_id.is_none() && !element.properties.contains_key(&PropertyId::from(key))
            {
                return old;
            }
            match resolved.get(&PropertyId::from(key)).map(|r| &r.value) {
                Some(PropertyValue::Number { value, .. }) => *value,
                _ => old,
            }
        };
        match &mut element.kind {
            ElementKind::Wall(s) => {
                s.height_m = number("height", s.height_m);
                s.thickness_m = number("thickness", s.thickness_m);
            }
            ElementKind::Floor(s) => {
                s.thickness_m = number("thickness", s.thickness_m);
            }
            ElementKind::Roof(s) => {
                s.thickness_m = number("thickness", s.thickness_m);
                s.slope_x = number("slope_x", s.slope_x);
                s.slope_y = number("slope_y", s.slope_y);
            }
            ElementKind::Opening(s) | ElementKind::Window(s) | ElementKind::Door(s) => {
                s.width_m = number("width", s.width_m);
                s.height_m = number("height", s.height_m);
                s.depth_m = number("depth", s.depth_m);
                s.rotation_rad = number("rotation", s.rotation_rad);
            }
            ElementKind::PolygonalOpening(s) => {
                s.depth_m = number("depth", s.depth_m);
                s.rotation_rad = number("rotation", s.rotation_rad);
            }
            ElementKind::Space(_) => {}
        }
        Ok(element)
    }
    pub fn resolved_element(&self, id: ElementId) -> Result<ElementRecord, CoreError> {
        self.effective_element(id)
    }

    pub fn dependencies(&self, id: ElementId) -> Vec<ElementId> {
        let (ids, visited) = self.dependents_with_visit_count(id);
        self.scan_counters.relationship_scan(visited);
        ids
    }
    fn dependents_with_visit_count(&self, id: ElementId) -> (Vec<ElementId>, usize) {
        let mut result = BTreeSet::new();
        let mut pending = vec![id];
        let mut visited = 0;
        while let Some(current) = pending.pop() {
            for (dependent, links) in self
                .indexes()
                .dependents
                .get(&current)
                .into_iter()
                .flatten()
            {
                visited += links.len();
                if *dependent != id && result.insert(*dependent) {
                    pending.push(*dependent);
                }
            }
        }
        (result.into_iter().collect(), visited)
    }
    pub fn dependency_info(&self, id: ElementId) -> Result<DependencyInfo, CoreError> {
        self.element(id).ok_or(CoreError::UnknownElement(id))?;
        let direct_dependencies = self
            .indexes()
            .dependencies
            .get(&id)
            .into_iter()
            .flat_map(|ids| ids.keys())
            .copied()
            .collect();
        Ok(DependencyInfo {
            element_id: id,
            direct_dependencies,
            dependents: self.dependencies(id),
        })
    }
    pub fn semantic_neighbors(
        &self,
        id: ElementId,
        filter: Option<&[RelationshipType]>,
        max_results: usize,
    ) -> Result<Vec<SemanticNeighbor>, CoreError> {
        self.element(id).ok_or(CoreError::UnknownElement(id))?;
        let mut result: BTreeMap<EntityRef, Vec<RelationshipId>> = BTreeMap::new();
        for r in self.element_relationships(id) {
            if filter.is_some_and(|types| !types.contains(&r.relationship_type)) {
                continue;
            }
            let other = if r.source_id == EntityRef::Element(id) {
                r.target_id
            } else {
                r.source_id
            };
            result.entry(other).or_default().push(r.id);
        }
        Ok(result
            .into_iter()
            .take(max_results)
            .map(|(entity, relationship_ids)| SemanticNeighbor {
                entity,
                relationship_ids,
            })
            .collect())
    }
    pub fn delete_impact(&self, id: ElementId) -> Result<DeleteImpact, CoreError> {
        self.element(id).ok_or(CoreError::UnknownElement(id))?;
        Ok(DeleteImpact {
            element_id: id,
            dependents: self.dependencies(id),
            hosted_elements: self.hosted_element_ids(id),
            relationship_ids: self
                .element_relationships(id)
                .iter()
                .map(|r| r.id)
                .collect(),
        })
    }

    pub(super) fn install_bim_classes(&mut self) {
        for (id, name, category) in [
            (
                BUILTIN_OPENING_CLASS_ID,
                "Opening",
                ElementCategory::Opening,
            ),
            (BUILTIN_DOOR_CLASS_ID, "Door", ElementCategory::Door),
            (BUILTIN_WINDOW_CLASS_ID, "Window", ElementCategory::Window),
            (BUILTIN_SPACE_CLASS_ID, "Space", ElementCategory::Space),
            (
                BUILTIN_MATERIAL_CLASS_ID,
                "Material",
                ElementCategory::Material,
            ),
        ] {
            self.classes
                .entry(id.into())
                .or_insert_with(|| ClassDef::new(id, name, category));
        }
        for (id, defaults) in intrinsic_property_definitions() {
            if let Some(class) = self.classes.get_mut(id) {
                for (property_id, definition) in &defaults.properties {
                    class
                        .properties
                        .entry(property_id.clone())
                        .or_insert_with(|| definition.clone());
                }
            }
        }
    }

    /// Explicit old-v1 -> extended-v1 migration. Existing IDs, parameters,
    /// receipts and history are retained. Historical states become available at
    /// the loaded revision; unavailable older revisions are reported as errors.
    pub(super) fn migrate_v1(&mut self) -> Result<(), CoreError> {
        // Migration can add owning checkpoint/anchor copies. Never reuse the
        // pre-migration reservation as a cache of the repaired document.
        self.circular_archive_usage = OnceLock::new();
        for state in self.revision_states.values_mut() {
            if state.checkpoint && state.drafting.is_none() {
                state.drafting = Some(DraftingDomain::default());
            }
        }
        self.indexes();
        if self.history_policy.checkpoint_interval == 0 {
            return Err(CoreError::InvalidDocument(
                "checkpoint interval must be positive".into(),
            ));
        }
        if self.semantic_schema == 0 || self.semantic_schema > 3 {
            return Err(CoreError::InvalidDocument(format!(
                "unsupported semantic schema {}",
                self.semantic_schema
            )));
        }
        self.classes
            .entry(BUILTIN_SPACE_CLASS_ID.into())
            .or_insert_with(|| {
                ClassDef::new(BUILTIN_SPACE_CLASS_ID, "Space", ElementCategory::Space)
            });
        if self.revision_states.is_empty() {
            // Add intrinsic geometric property definitions to old built-ins.
            // User classes and legacy untyped geometry remain unchanged.
            let old_classes = self.classes.clone();
            self.install_bim_classes();
            // Preserve old built-ins too. Adding a geometric property to a
            // parent would invalidate a previously valid child's definition.
            for (id, class) in old_classes {
                self.classes.insert(id, class);
            }
            self.semantic_schema = 2;
            self.record_revision()?;
        }
        // The current legacy state is known from authoritative maps. Retain a
        // complete anchor here so every new v0.4 delta can replay immediately.
        // Earlier legacy revisions remain available for their original diff
        // data; missing historical maps are never guessed from future state.
        if let Some(state) = self
            .revision_states
            .get_mut(&self.revision)
            .filter(|s| s.checkpoint && s.delta.is_none())
        {
            if state.classes.is_none() {
                state.classes = Some(self.classes.clone());
            }
            if state.operations.is_none() {
                state.operations = Some(self.operations.clone());
            }
            if state.relationship_records.is_none() {
                state.relationship_records = Some(self.relationships.clone());
            }
            if self.revision != 0 && state.history_stack_anchor.is_none() {
                state.history_stack_anchor = Some(HistoryStackAnchor {
                    undo: self.undo_stack.clone(),
                    redo: self.redo_stack.clone(),
                    counters: AllocationCounters {
                        drafting: self.draft_ids.clone(),
                        element: self.next_element_id,
                        operation: self.next_operation_id,
                        element_type: self.next_type_id,
                        level: self.next_level_id,
                        relationship: self.next_relationship_id,
                    },
                });
            }
        }
        // Upgrade legacy idempotent receipts too: retries must report the
        // original transaction revision even after migration or undo.
        let legacy_receipts: Vec<_> = self
            .command_receipts
            .iter()
            .filter(|(_, stored)| stored.receipt.transaction_id.is_empty())
            .map(|(id, stored)| (id.clone(), stored.receipt.clone()))
            .collect();
        for (id, old) in legacy_receipts {
            let mut affected: BTreeSet<_> = old.created_elements.iter().copied().collect();
            for operation_id in &old.operation_ids {
                if let Some(operation) = self.operations.get(operation_id) {
                    affected.extend(operation.inputs.iter().chain(&operation.outputs).copied());
                }
            }
            let mut dirty = affected.clone();
            for element_id in &affected {
                dirty.extend(self.dependencies(*element_id));
            }
            let receipt = &mut self
                .command_receipts
                .get_mut(&id)
                .expect("legacy receipt exists")
                .receipt;
            receipt.transaction_id = id;
            receipt.revision_before = old.revision.saturating_sub(1);
            receipt.revision_after = old.revision;
            receipt.affected_ids = affected.into_iter().collect();
            receipt.dirty_ids = dirty.into_iter().collect();
        }
        // record_revision may have filled the cache before inserting its new
        // owning state. The final repaired model always needs a fresh audit.
        self.circular_archive_usage = OnceLock::new();
        Ok(())
    }

    pub(super) fn revision_state(&self) -> Result<RevisionState, CoreError> {
        let _scan = self.scan_counters.track(
            "full_revision_checkpoint",
            self.elements.len(),
            self.relationships.len(),
            "configurable checkpoint or explicit persisted-state audit",
            true,
        );
        let mut elements = BTreeMap::new();
        let mut property_sources = BTreeMap::new();
        self.scan_counters.document_scan();
        for e in self.elements.values() {
            let mut resolved = self.effective_element(e.id)?;
            let properties = self.resolved_properties(e.id)?;
            property_sources.insert(
                e.id,
                properties
                    .iter()
                    .map(|(id, property)| (id.clone(), property.source))
                    .collect(),
            );
            resolved.properties = properties
                .into_iter()
                .map(|(id, p)| (id, p.value))
                .collect();
            elements.insert(e.id, resolved);
        }
        Ok(RevisionState {
            geo_foundation: geo_foundation::geo_foundation_projection(&self.geo_foundation, None),
            structural_configurator: configured_building::configurator_projection(&self.structural_configurator, None),
            foundation_structural: foundation_structural::foundation_structural_projection(&self.foundation_structural, None),
            engineering: engineering::engineering_projection(&self.engineering, None),
            drafting: Some(self.drafting.clone()),
            elements,
            types: self.types.clone(),
            levels: self.levels.clone(),
            relationships: self.relationships().map(|r| (r.id, r.clone())).collect(),
            instances: Some(self.elements.clone()),
            property_sources: Some(property_sources),
            checkpoint: true,
            delta: None,
            classes: Some(self.classes.clone()),
            operations: Some(self.operations.clone()),
            relationship_records: Some(self.relationships.clone()),
            history_stack_anchor: None,
        })
    }

    pub(super) fn record_revision(&mut self) -> Result<(), CoreError> {
        self.reserve_circular_archive_audit()?;
        let total_start = Instant::now();
        let state = self.revision_state()?;
        let serialization_start = Instant::now();
        let bytes = circular_archive::bounded_serialized_size(&state, usize::MAX)?;
        self.scan_counters
            .snapshot(elapsed_us(serialization_start), bytes);
        self.scan_counters.snapshot_total(elapsed_us(total_start));
        self.revision_states.insert(self.revision, state);
        Ok(())
    }

    pub(super) fn verify_revision_states_and_counters(&self) -> Result<(), CoreError> {
        self.verify_history_replay_integrity()?;
        let current = self.reconstruct_revision_state(self.revision)?;
        let expected = self.revision_state()?;
        if current.elements != expected.elements
            || current.geo_foundation != expected.geo_foundation
            || current.structural_configurator != expected.structural_configurator
            || current.foundation_structural != expected.foundation_structural
            || current.engineering != expected.engineering
            || current.drafting.as_ref() != Some(&self.drafting)
            || current.types != expected.types
            || current.levels != expected.levels
            || current
                .classes
                .as_ref()
                .is_some_and(|classes| classes != &self.classes)
            || current
                .operations
                .as_ref()
                .is_some_and(|operations| operations != &self.operations)
            || current
                .relationship_records
                .as_ref()
                .is_some_and(|records| records != &self.relationships)
            || current.relationships != expected.relationships
            || current
                .instances
                .as_ref()
                .is_some_and(|instances| instances != &self.elements)
            || current
                .property_sources
                .as_ref()
                .is_some_and(|sources| Some(sources) != expected.property_sources.as_ref())
        {
            return Err(CoreError::InvalidDocument(
                "current revision snapshot disagrees with document".into(),
            ));
        }
        let mut max_element = self
            .elements
            .keys()
            .next_back()
            .map(|id| id.get())
            .unwrap_or(0);
        let mut max_type = self
            .types
            .keys()
            .next_back()
            .map(|id| id.get())
            .unwrap_or(0);
        let mut max_level = self
            .levels
            .keys()
            .next_back()
            .map(|id| id.get())
            .unwrap_or(0);
        let mut max_relationship = self
            .relationships
            .keys()
            .next_back()
            .map(|id| id.get())
            .unwrap_or(0);
        let mut max_operation = self
            .operations
            .keys()
            .next_back()
            .map(|id| id.get())
            .unwrap_or(0);
        max_operation = max_operation.max(engineering_operation_high_water(&self.engineering));
        max_operation = max_operation.max(self.foundation_structural.high_water());
        max_operation = max_operation.max(self.structural_configurator.high_water());
        max_operation = max_operation.max(self.geo_foundation.high_water());
        let _scan = self.scan_counters.track("persisted_history_audit", self.revision_states.values().map(|s|s.elements.len()).sum(), self.revision_states.values().map(|s|s.relationships.len()).sum(), "validate persisted checkpoints, deltas, sources and historical allocation high-water marks", true);
        for (revision, state) in &self.revision_states {
            self.verify_delta_projection(*revision, state)?;
            if !state.checkpoint && state.delta.is_none() {
                return Err(CoreError::InvalidDocument(
                    "compact revision lacks an authoritative delta".into(),
                ));
            }
            if let Some(delta) = &state.delta {
                if delta.summary.revision_after != *revision
                    || delta.summary.revision_before.checked_add(1) != Some(*revision)
                {
                    return Err(CoreError::InvalidDocument(
                        "delta revision range disagrees with its history key".into(),
                    ));
                }
                for change in &delta.changes {
                    match change {
                        Change::GeoFoundation { before, after } => { before.validate(self.document_uuid)?; after.validate(self.document_uuid)?; max_operation = max_operation.max(before.high_water()).max(after.high_water()); }
                        Change::StructuralConfigurator { before, after } => { before.validate(self.document_uuid)?; after.validate(self.document_uuid)?; max_operation = max_operation.max(before.high_water()).max(after.high_water()); }
                        Change::FoundationStructural { before, after } => {
                            before.validate(self.document_uuid)?; after.validate(self.document_uuid)?;
                            max_operation = max_operation.max(before.high_water()).max(after.high_water());
                        }
                        Change::EngineeringCase { id, before, after } => {
                            max_operation = max_operation.max(id.get());
                            for c in before.iter().chain(after) {
                                if c.analysis_id != *id {
                                    return Err(CoreError::InvalidDocument(
                                        "Engineering delta identity mismatch".into(),
                                    ));
                                }
                            }
                            engineering::validate_engineering_change_records(change)?;
                        }
                        Change::EngineeringGroundSection { id, before, after } => {
                            max_operation = max_operation.max(id.get());
                            for ground in before.iter().chain(after) {
                                max_operation = max_operation.max(ground.source_operation_id.get());
                            }
                            engineering::validate_engineering_change_records(change)?;
                        }
                        Change::CircularEngineeringCase { id, before, after } => {
                            max_operation = max_operation.max(id.get());
                            for case in before.iter().chain(after) {
                                max_operation = max_operation.max(case.source_operation_id.get());
                            }
                            engineering::validate_engineering_change_records(change)?;
                        }
                        Change::Element { id, before, after } => {
                            max_element = max_element.max(id.get());
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta element ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Type { id, before, after } => {
                            max_type = max_type.max(id.get());
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta type ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Level { id, before, after } => {
                            max_level = max_level.max(id.get());
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta level ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Relationship { id, before, after } => {
                            max_relationship = max_relationship.max(id.get());
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta relationship ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Operation { id, before, after } => {
                            max_operation = max_operation.max(id.get());
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta operation ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Class { id, before, after } => {
                            if before.iter().chain(after).any(|e| e.id != *id) {
                                return Err(CoreError::InvalidDocument(
                                    "delta class ID mismatch".into(),
                                ));
                            }
                        }
                        Change::Draft(_)
                        | Change::DraftDrawingMetadata { .. }
                        | Change::DraftView { .. } => {}
                    }
                }
                let mut local = AllocationCounters::default();
                for change in &delta.changes {
                    match change {
                        Change::GeoFoundation { before, after } => local.operation = local.operation.max(before.high_water()).max(after.high_water()),
                        Change::StructuralConfigurator { before, after } => local.operation = local.operation.max(before.high_water()).max(after.high_water()),
                        Change::FoundationStructural { before, after } => local.operation = local.operation.max(before.high_water()).max(after.high_water()),
                        Change::EngineeringCase { id, .. } => local.operation = local.operation.max(id.get()),
                        Change::EngineeringGroundSection { id, before, after } => {
                            local.operation = local.operation.max(id.get());
                            for value in before.iter().chain(after) {
                                local.operation = local.operation.max(value.source_operation_id.get());
                            }
                        }
                        Change::CircularEngineeringCase { id, before, after } => {
                            local.operation = local.operation.max(id.get());
                            for value in before.iter().chain(after) {
                                local.operation = local.operation.max(value.source_operation_id.get());
                            }
                        }
                        Change::Element { id, .. } => local.element = local.element.max(id.get()),
                        Change::Type { id, .. } => {
                            local.element_type = local.element_type.max(id.get())
                        }
                        Change::Level { id, .. } => local.level = local.level.max(id.get()),
                        Change::Relationship { id, .. } => {
                            local.relationship = local.relationship.max(id.get())
                        }
                        Change::Operation { id, .. } => {
                            local.operation = local.operation.max(id.get())
                        }
                        _ => {}
                    }
                }
                for (counter, max) in [
                    (delta.counters.element, local.element),
                    (delta.counters.element_type, local.element_type),
                    (delta.counters.level, local.level),
                    (delta.counters.relationship, local.relationship),
                    (delta.counters.operation, local.operation),
                ] {
                    if counter == 0 || counter <= max {
                        return Err(CoreError::InvalidDocument(
                            "delta allocation counters must exceed its journal IDs".into(),
                        ));
                    }
                }
            }
            if *revision > self.revision {
                return Err(CoreError::InvalidDocument(
                    "revision snapshot is from the future".into(),
                ));
            }
            if let Some(domain) = &state.structural_configurator { domain.validate(self.document_uuid)?; max_operation = max_operation.max(domain.high_water()); }
            if let Some(domain) = &state.geo_foundation { domain.validate(self.document_uuid)?; max_operation = max_operation.max(domain.high_water()); }
            if let Some(domain) = &state.foundation_structural {
                domain.validate(self.document_uuid)?;
                max_operation = max_operation.max(domain.high_water());
            }
            if let Some(domain) = &state.engineering {
                // A noncheckpoint projection may omit an unchanged referenced
                // ground. Cross-record validation belongs to replay above.
                domain.validate_local()?;
                if state.checkpoint { domain.validate()?; }
                max_operation = max_operation.max(engineering_operation_high_water(domain));
            }
            self.scan_counters.document_scan();
            for (id, element) in &state.elements {
                if id.get() == 0 || *id != element.id {
                    return Err(CoreError::InvalidDocument(
                        "revision element ID/key mismatch".into(),
                    ));
                }
                max_element = max_element.max(id.get());
            }
            for (id, element_type) in &state.types {
                if id.get() == 0 || *id != element_type.id {
                    return Err(CoreError::InvalidDocument(
                        "revision type ID/key mismatch".into(),
                    ));
                }
                max_type = max_type.max(id.get());
            }
            for (id, level) in &state.levels {
                if id.get() == 0 || *id != level.id {
                    return Err(CoreError::InvalidDocument(
                        "revision level ID/key mismatch".into(),
                    ));
                }
                max_level = max_level.max(id.get());
            }
            self.scan_counters
                .relationship_scan(state.relationships.len());
            for (id, relationship) in &state.relationships {
                if id.get() == 0
                    || *id != relationship.id
                    || !relationship.is_active()
                    || relationship.revision_created > *revision
                {
                    return Err(CoreError::InvalidDocument(
                        "revision relationship ID/state mismatch".into(),
                    ));
                }
                max_relationship = max_relationship.max(id.get());
            }
            if let Some(sources) = &state.property_sources {
                if sources.keys().ne(state.elements.keys())
                    || sources.iter().any(|(id, properties)| {
                        properties.keys().ne(state.elements[id].properties.keys())
                    })
                {
                    return Err(CoreError::InvalidDocument(
                        "revision property source keys do not match elements".into(),
                    ));
                }
            }
            if let Some(instances) = &state.instances {
                self.scan_counters.document_scan();
                if instances.keys().ne(state.elements.keys())
                    || instances
                        .iter()
                        .any(|(id, element)| id.get() == 0 || *id != element.id)
                {
                    return Err(CoreError::InvalidDocument(
                        "revision instance ID/key mismatch".into(),
                    ));
                }
            }
        }
        for entry in self.undo_stack.iter().chain(&self.redo_stack) {
            for change in &entry.changes {
                match change {
                    Change::GeoFoundation { before, after } => { before.validate(self.document_uuid)?; after.validate(self.document_uuid)?; max_operation = max_operation.max(before.high_water()).max(after.high_water()); }
                    Change::StructuralConfigurator { before, after } => { before.validate(self.document_uuid)?; after.validate(self.document_uuid)?; max_operation = max_operation.max(before.high_water()).max(after.high_water()); }
                    Change::FoundationStructural { before, after } => {
                        before.validate(self.document_uuid)?; after.validate(self.document_uuid)?;
                        max_operation = max_operation.max(before.high_water()).max(after.high_water());
                    }
                    Change::EngineeringCase { id, .. } => {
                        max_operation = max_operation.max(id.get())
                    }
                    Change::EngineeringGroundSection { id, before, after } => {
                        max_operation = max_operation.max(id.get());
                        for value in before.iter().chain(after) {
                            max_operation = max_operation.max(value.source_operation_id.get());
                        }
                        engineering::validate_engineering_change_records(change)?;
                    }
                    Change::CircularEngineeringCase { id, before, after } => {
                        max_operation = max_operation.max(id.get());
                        for value in before.iter().chain(after) {
                            max_operation = max_operation.max(value.source_operation_id.get());
                        }
                        engineering::validate_engineering_change_records(change)?;
                    }
                    Change::Element { id, .. } => max_element = max_element.max(id.get()),
                    Change::Type { id, .. } => max_type = max_type.max(id.get()),
                    Change::Level { id, .. } => max_level = max_level.max(id.get()),
                    Change::Relationship { id, .. } => {
                        max_relationship = max_relationship.max(id.get())
                    }
                    Change::Operation { id, .. } => max_operation = max_operation.max(id.get()),
                    Change::Class { .. }
                    | Change::Draft(_)
                    | Change::DraftDrawingMetadata { .. }
                    | Change::DraftView { .. } => {}
                }
            }
        }
        for stored in self.command_receipts.values() {
            for id in &stored.receipt.created_elements {
                max_element = max_element.max(id.get());
            }
            for id in &stored.receipt.created_types {
                max_type = max_type.max(id.get());
            }
            for id in &stored.receipt.created_levels {
                max_level = max_level.max(id.get());
            }
            for id in &stored.receipt.created_relationships {
                max_relationship = max_relationship.max(id.get());
            }
            for id in &stored.receipt.operation_ids {
                max_operation = max_operation.max(id.get());
            }
        }
        for (kind, counter, max) in [
            ("element", self.next_element_id, max_element),
            ("operation", self.next_operation_id, max_operation),
            ("type", self.next_type_id, max_type),
            ("level", self.next_level_id, max_level),
            ("relationship", self.next_relationship_id, max_relationship),
        ] {
            if counter == 0 || counter <= max {
                return Err(CoreError::InvalidDocument(format!(
                    "next_{kind}_id must exceed historical IDs"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn affected_from_changes(&self, changes: &[Change]) -> Vec<ElementId> {
        let mut ids = BTreeSet::new();
        for c in changes {
            match c {
                Change::GeoFoundation { .. }
                | Change::StructuralConfigurator { .. }
                | Change::FoundationStructural { .. }
                | Change::EngineeringCase { .. }
                | Change::EngineeringGroundSection { .. }
                | Change::CircularEngineeringCase { .. } => {}
                Change::Element { id, .. } => {
                    ids.insert(*id);
                }
                Change::Type { id, .. } => {
                    ids.extend(self.type_instances(*id));
                }
                Change::Level { id, .. } => {
                    ids.extend(self.level_elements(*id));
                }
                Change::Relationship { before, after, .. } => {
                    for r in before.iter().chain(after) {
                        for entity in [r.source_id, r.target_id] {
                            if let EntityRef::Element(id) = entity {
                                ids.insert(id);
                            }
                        }
                    }
                }
                Change::Class { id, .. } => {
                    let mut classes = vec![id.clone()];
                    let mut seen = BTreeSet::new();
                    while let Some(class) = classes.pop() {
                        if !seen.insert(class.clone()) {
                            continue;
                        }
                        ids.extend(self.class_elements(&class));
                        for type_id in self
                            .indexes()
                            .types_by_class
                            .get(&class)
                            .into_iter()
                            .flatten()
                        {
                            ids.extend(self.type_instances(*type_id));
                        }
                        classes.extend(
                            self.indexes()
                                .class_children
                                .get(&class)
                                .into_iter()
                                .flatten()
                                .cloned(),
                        );
                    }
                }
                Change::Operation { .. }
                | Change::Draft(_)
                | Change::DraftDrawingMetadata { .. }
                | Change::DraftView { .. } => {}
            }
        }
        ids.into_iter().collect()
    }
    fn hosting_edges(&self) -> Vec<(ElementId, ElementId)> {
        let mut edges = Vec::new();
        for (host, children) in &self.indexes().hosts {
            for (child, links) in children {
                self.scan_counters.relationship_scan(links.len());
                // Preserve duplicates for validation of multiple host links.
                edges.extend(links.iter().map(|_| (*host, *child)));
            }
        }
        edges
    }
    fn entity_exists(&self, entity: EntityRef) -> bool {
        match entity {
            EntityRef::Element(id) => self.elements.contains_key(&id),
            EntityRef::Type(id) => self.types.contains_key(&id),
            EntityRef::Level(id) => self.levels.contains_key(&id),
        }
    }

    pub(super) fn validate_bim(&self) -> Result<(), CoreError> {
        let _scan = self.scan_counters.track(
            "full_bim_audit",
            self.elements.len(),
            self.relationships.len(),
            "explicit open/save semantic graph validation",
            true,
        );
        for (id, element_type) in &self.types {
            if *id != element_type.id || id.get() == 0 {
                return Err(bim_error(EntityRef::Type(*id), "type ID/key mismatch"));
            }
            validate_name(&element_type.name, "type")?;
            self.effective_category(&element_type.class_id)?;
            self.validate_type_properties(element_type)?;
        }
        for (id, level) in &self.levels {
            if *id != level.id || id.get() == 0 {
                return Err(bim_error(EntityRef::Level(*id), "level ID/key mismatch"));
            }
            validate_name(&level.name, "level")?;
            validate_finite(level.elevation_m, "level elevation")?;
            if let Some(height) = level.height_m {
                validate_positive(height, "level height")?;
            }
        }
        self.scan_counters
            .relationship_scan(self.relationships.len());
        for (id, r) in &self.relationships {
            if *id != r.id
                || id.get() == 0
                || r.revision_created > self.revision
                || r.revision_deleted
                    .is_some_and(|deleted| deleted < r.revision_created || deleted > self.revision)
            {
                return Err(CoreError::InvalidDocument(format!(
                    "relationship {id} has invalid identity/revision"
                )));
            }
        }
        let mut unique = BTreeSet::new();
        let mut assigned_types: BTreeMap<ElementId, TypeId> = BTreeMap::new();
        let mut assigned_levels = BTreeSet::new();
        for r in self.relationships() {
            for entity in [r.source_id, r.target_id] {
                if !self.entity_exists(entity) {
                    return Err(bim_error(
                        entity,
                        format!("relationship {} references missing entity", r.id),
                    ));
                }
            }
            self.validate_relationship_endpoints(r)?;
            if !unique.insert((r.relationship_type, r.source_id, r.target_id)) {
                return Err(bim_error(
                    r.source_id,
                    format!("duplicate relationship {}", r.id),
                ));
            }
            match (r.relationship_type, r.source_id, r.target_id) {
                (
                    RelationshipType::InstanceOf,
                    EntityRef::Element(id),
                    EntityRef::Type(type_id),
                ) => {
                    let element = &self.elements[&id];
                    self.validate_type_compatibility(element, type_id)?;
                    if assigned_types.insert(id, type_id).is_some() {
                        return Err(bim_error(
                            EntityRef::Element(id),
                            "multiple INSTANCE_OF links",
                        ));
                    }
                }
                (RelationshipType::BelongsToLevel, EntityRef::Element(id), EntityRef::Level(_)) => {
                    if !assigned_levels.insert(id) {
                        return Err(bim_error(
                            EntityRef::Element(id),
                            "multiple BELONGS_TO_LEVEL links",
                        ));
                    }
                }
                (RelationshipType::References, _, _) => {}
                (RelationshipType::InstanceOf | RelationshipType::BelongsToLevel, _, _) => {
                    return Err(bim_error(
                        r.source_id,
                        format!("relationship {} has invalid endpoint kinds", r.id),
                    ));
                }
                (_, EntityRef::Element(_), EntityRef::Element(_)) => {}
                _ => {
                    return Err(bim_error(
                        r.source_id,
                        format!("relationship {} requires element endpoints", r.id),
                    ));
                }
            }
        }
        let hosting = self.hosting_edges();
        let mut hosts = BTreeMap::new();
        for (host, child) in &hosting {
            if host == child {
                return Err(bim_error(
                    EntityRef::Element(*child),
                    "an element cannot host itself",
                ));
            }
            if hosts.insert(*child, *host).is_some() {
                return Err(bim_error(
                    EntityRef::Element(*child),
                    "element has multiple hosts",
                ));
            }
        }
        for child in hosts.keys() {
            let mut seen = BTreeSet::new();
            let mut current = *child;
            while let Some(host) = hosts.get(&current) {
                if !seen.insert(current) {
                    return Err(bim_error(
                        EntityRef::Element(*child),
                        "hosting hierarchy contains a cycle",
                    ));
                }
                current = *host;
            }
        }
        self.scan_counters.document_scan();
        for element in self.elements.values() {
            if element.type_id != assigned_types.get(&element.id).copied() {
                return Err(bim_error(
                    EntityRef::Element(element.id),
                    "type_id and INSTANCE_OF graph disagree",
                ));
            }
            match &element.kind {
                ElementKind::Opening(_) | ElementKind::PolygonalOpening(_) => {
                    let host = hosts.get(&element.id).ok_or_else(|| {
                        bim_error(
                            EntityRef::Element(element.id),
                            "Opening requires a valid host",
                        )
                    })?;
                    if !matches!(
                        self.elements[host].kind,
                        ElementKind::Wall(_) | ElementKind::Floor(_) | ElementKind::Roof(_)
                    ) {
                        return Err(bim_error(
                            EntityRef::Element(element.id),
                            "Opening host must be Wall, Slab or Roof",
                        ));
                    }
                }
                ElementKind::Window(_) | ElementKind::Door(_) => {
                    let host = hosts.get(&element.id).ok_or_else(|| {
                        bim_error(
                            EntityRef::Element(element.id),
                            "Window/Door requires an Opening host",
                        )
                    })?;
                    if !matches!(
                        self.elements[host].kind,
                        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                    ) {
                        return Err(bim_error(
                            EntityRef::Element(element.id),
                            "Window/Door host must be Opening",
                        ));
                    }
                }
                _ => {}
            }
            if let Some(host_id) = hosts.get(&element.id) {
                self.validate_hosted_placement(element.id, *host_id)?;
            }
            validate_element_kind(&self.effective_element(element.id)?.kind)
                .map_err(|error| bim_error(EntityRef::Element(element.id), error.to_string()))?;
            for (id, definition) in self.element_property_definitions(element)? {
                if definition.required && !self.resolved_properties(element.id)?.contains_key(&id) {
                    return Err(bim_error(
                        EntityRef::Element(element.id),
                        format!("required property {id} is missing"),
                    ));
                }
            }
        }
        for (name, counter, max) in [
            (
                "type",
                self.next_type_id,
                self.types
                    .keys()
                    .next_back()
                    .map(|id| id.get())
                    .unwrap_or(0),
            ),
            (
                "level",
                self.next_level_id,
                self.levels
                    .keys()
                    .next_back()
                    .map(|id| id.get())
                    .unwrap_or(0),
            ),
            (
                "relationship",
                self.next_relationship_id,
                self.relationships
                    .keys()
                    .next_back()
                    .map(|id| id.get())
                    .unwrap_or(0),
            ),
        ] {
            if counter == 0 || counter <= max {
                return Err(CoreError::InvalidDocument(format!(
                    "next_{name}_id must exceed stored IDs"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn validate_local_changes(
        &self,
        changes: &[Change],
        affected: &[ElementId],
        dirty: &[ElementId],
    ) -> Result<(), CoreError> {
        self.validate_local_drafting(changes)?;
        self.validate_local_engineering(changes)?;
        self.validate_local_foundation_structural(changes)?;
        self.validate_local_configurator(changes)?;
        self.validate_local_geo_foundation(changes)?;
        let elements: BTreeSet<_> = affected.iter().chain(dirty).copied().collect();
        let mut relationships = BTreeSet::new();
        for id in &elements {
            relationships.extend(
                self.indexes()
                    .by_entity
                    .get(&EntityRef::Element(*id))
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
        for c in changes {
            match c {
                Change::Type { id, .. } => {
                    if let Some(t) = self.types.get(id) {
                        validate_name(&t.name, "type")?;
                        self.effective_category(&t.class_id)?;
                        self.validate_type_properties(t)?;
                    }
                }
                Change::Level { id, .. } => {
                    if let Some(l) = self.levels.get(id) {
                        validate_name(&l.name, "level")?;
                        validate_finite(l.elevation_m, "level elevation")?;
                        if let Some(h) = l.height_m {
                            validate_positive(h, "level height")?;
                        }
                    }
                }
                Change::Relationship { id, .. } => {
                    relationships.insert(*id);
                }
                _ => {}
            }
        }
        let _scan = self.scan_counters.track(
            "bounded_semantic_validation",
            elements.len(),
            relationships.len(),
            "validate changed entities, adjacent relationships and reachable dependents",
            false,
        );
        self.scan_counters.relationship_scan(relationships.len());
        for id in relationships {
            let Some(r) = self.relationships.get(&id) else {
                continue;
            };
            if r.id != id
                || id.get() == 0
                || r.revision_created > self.revision
                || r.revision_deleted
                    .is_some_and(|v| v < r.revision_created || v > self.revision)
            {
                return Err(CoreError::InvalidDocument(format!(
                    "relationship {id} has invalid identity/revision"
                )));
            }
            if !r.is_active() {
                continue;
            }
            for entity in [r.source_id, r.target_id] {
                if !self.entity_exists(entity) {
                    return Err(bim_error(
                        entity,
                        format!("relationship {id} references missing entity"),
                    ));
                }
            }
            let duplicate = self
                .indexes()
                .by_source
                .get(&r.source_id)
                .into_iter()
                .flatten()
                .filter_map(|id| self.relationships.get(id))
                .filter(|other| {
                    other.relationship_type == r.relationship_type && other.target_id == r.target_id
                })
                .count();
            if duplicate > 1 {
                return Err(bim_error(
                    r.source_id,
                    format!("duplicate relationship {id}"),
                ));
            }
            self.validate_relationship_endpoints(r)?;
        }
        for id in elements {
            let Some(element) = self.elements.get(&id) else {
                continue;
            };
            if element.id != id || id.get() == 0 {
                return Err(bim_error(EntityRef::Element(id), "element ID/key mismatch"));
            }
            validate_name(&element.name, "element")?;
            self.validate_class_for_kind(&element.class_id, &element.kind)?;
            let operation = self.operations.get(&element.created_by).ok_or_else(|| {
                bim_error(
                    EntityRef::Element(id),
                    "element creator operation is missing",
                )
            })?;
            if !operation.outputs.contains(&id) {
                return Err(bim_error(
                    EntityRef::Element(id),
                    "creator operation does not output this element",
                ));
            }
            let type_links: Vec<_> = self
                .relationships_from(EntityRef::Element(id))
                .into_iter()
                .filter(|r| r.relationship_type == RelationshipType::InstanceOf)
                .collect();
            if type_links.len() > 1 {
                return Err(bim_error(
                    EntityRef::Element(id),
                    "multiple INSTANCE_OF links",
                ));
            }
            let assigned = type_links.first().and_then(|r| {
                if let EntityRef::Type(id) = r.target_id {
                    Some(id)
                } else {
                    None
                }
            });
            if element.type_id != assigned {
                return Err(bim_error(
                    EntityRef::Element(id),
                    "type_id and INSTANCE_OF graph disagree",
                ));
            }
            if let Some(type_id) = assigned {
                self.validate_type_compatibility(element, type_id)?;
            }
            if self
                .indexes()
                .element_levels
                .get(&id)
                .into_iter()
                .flat_map(|m| m.values())
                .map(BTreeSet::len)
                .sum::<usize>()
                > 1
            {
                return Err(bim_error(
                    EntityRef::Element(id),
                    "multiple BELONGS_TO_LEVEL links",
                ));
            }
            if self
                .indexes()
                .host_of
                .get(&id)
                .into_iter()
                .flat_map(|m| m.values())
                .map(BTreeSet::len)
                .sum::<usize>()
                > 1
            {
                return Err(bim_error(
                    EntityRef::Element(id),
                    "element has multiple hosts",
                ));
            }
            let mut current = id;
            let mut seen = BTreeSet::new();
            while let Some(host) = self.host_of(current) {
                if !seen.insert(current) {
                    return Err(bim_error(
                        EntityRef::Element(id),
                        "hosting hierarchy contains a cycle",
                    ));
                }
                current = host;
            }
            let host = self.host_of(id);
            match &element.kind {
                ElementKind::Opening(_) | ElementKind::PolygonalOpening(_) => {
                    let host = host.ok_or_else(|| {
                        bim_error(EntityRef::Element(id), "Opening requires a valid host")
                    })?;
                    if !matches!(
                        self.elements[&host].kind,
                        ElementKind::Wall(_) | ElementKind::Floor(_) | ElementKind::Roof(_)
                    ) {
                        return Err(bim_error(
                            EntityRef::Element(id),
                            "Opening host must be Wall, Slab or Roof",
                        ));
                    }
                }
                ElementKind::Window(_) | ElementKind::Door(_) => {
                    let host = host.ok_or_else(|| {
                        bim_error(
                            EntityRef::Element(id),
                            "Window/Door requires an Opening host",
                        )
                    })?;
                    if !matches!(
                        self.elements[&host].kind,
                        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                    ) {
                        return Err(bim_error(
                            EntityRef::Element(id),
                            "Window/Door host must be Opening",
                        ));
                    }
                }
                _ => {}
            }
            if let Some(host) = host {
                self.validate_hosted_placement(id, host)?;
            }
            validate_element_kind(&self.effective_element(id)?.kind)
                .map_err(|e| bim_error(EntityRef::Element(id), e.to_string()))?;
            let resolved = self.resolved_properties(id)?;
            for (property, definition) in self.element_property_definitions(element)? {
                if definition.required && !resolved.contains_key(&property) {
                    return Err(bim_error(
                        EntityRef::Element(id),
                        format!("required property {property} is missing"),
                    ));
                }
                if let Some(value) = element.properties.get(&property) {
                    validate_property_value(&definition, value)?;
                }
            }
        }
        Ok(())
    }
    fn validate_relationship_endpoints(&self, r: &Relationship) -> Result<(), CoreError> {
        match (r.relationship_type, r.source_id, r.target_id) {
            (RelationshipType::InstanceOf, EntityRef::Element(id), EntityRef::Type(type_id)) => {
                self.validate_type_compatibility(&self.elements[&id], type_id)
            }
            (RelationshipType::BelongsToLevel, EntityRef::Element(_), EntityRef::Level(_))
            | (RelationshipType::References, _, _) => Ok(()),
            (RelationshipType::InstanceOf | RelationshipType::BelongsToLevel, _, _) => Err(
                bim_error(r.source_id, "relationship has invalid endpoint kinds"),
            ),
            (
                RelationshipType::BoundsSpace | RelationshipType::ContainedInSpace,
                EntityRef::Element(source),
                EntityRef::Element(target),
            ) => {
                if !matches!(self.elements[&target].kind, ElementKind::Space(_)) || source == target
                {
                    return Err(bim_error(
                        r.target_id,
                        "space relationship requires a distinct Space target",
                    ));
                }
                Ok(())
            }
            (_, EntityRef::Element(_), EntityRef::Element(_)) => Ok(()),
            _ => Err(bim_error(
                r.source_id,
                "relationship requires element endpoints",
            )),
        }
    }

    fn validate_hosted_placement(
        &self,
        child_id: ElementId,
        host_id: ElementId,
    ) -> Result<(), CoreError> {
        let child = self.effective_element(child_id)?;
        let host = self.effective_element(host_id)?;
        let tol = geometry_validation::DEFAULT_LINEAR_TOLERANCE_M;
        let fail = |message: &str| {
            bim_error(
                EntityRef::Element(child_id),
                format!("host {host_id}: {message}"),
            )
        };
        match (&child.kind, &host.kind) {
            (ElementKind::PolygonalOpening(opening), ElementKind::Wall(wall)) => {
                let length = (wall.end.x - wall.start.x).hypot(wall.end.y - wall.start.y);
                let ux = (wall.end.x - wall.start.x) / length;
                let uy = (wall.end.y - wall.start.y) / length;
                let dx = opening.center.x - wall.start.x;
                let dy = opening.center.y - wall.start.y;
                let along = dx * ux + dy * uy;
                let across = -dx * uy + dy * ux;
                let (sin, cos) = opening.rotation_rad.sin_cos();
                if (cos * uy - sin * ux).abs() > 1e-6 {
                    return Err(fail(
                        "void_non_planar: polygon placement must align with wall baseline",
                    ));
                }
                let alignment = cos * ux + sin * uy;
                let contour = geometry_validation::validate_planar_contour(
                    &opening.boundary,
                    geometry_validation::GeometryOptions::default(),
                )
                .map_err(|e| fail(e.message))?;
                if contour.iter().any(|p| {
                    let u = along + p.x * alignment;
                    let z = opening.elevation_m + p.y;
                    u < -tol
                        || u > length + tol
                        || z < wall.base_elevation_m - tol
                        || z > wall.base_elevation_m + wall.height_m + tol
                }) {
                    return Err(fail("void_outside_host: polygon does not fit wall bounds"));
                }
                if opening.depth_m / 2. + tol < across.abs() + wall.thickness_m / 2. {
                    return Err(fail(
                        "void_depth_insufficient: polygon must span full wall thickness",
                    ));
                }
            }
            (
                ElementKind::PolygonalOpening(opening),
                ElementKind::Floor(_) | ElementKind::Roof(_),
            ) => {
                let options = geometry_validation::GeometryOptions::default();
                let region = geometry_validation::validate_element_geometry(&host.kind, options)
                    .map_err(|e| fail(e.message))?;
                let contour = geometry_validation::validate_element_geometry(&child.kind, options)
                    .map_err(|e| fail(e.message))?;
                if !region
                    .contains_outline(&contour.boundary, tol)
                    .map_err(|e| fail(e.message))?
                {
                    return Err(fail(
                        "void_outside_host: polygon lies outside host region or intersects a source hole",
                    ));
                }
                let top = region.top_elevation_at(opening.center);
                let lower = opening.elevation_m - top;
                if lower > -region.thickness_m + tol || lower + opening.depth_m < -tol {
                    return Err(fail(
                        "void_depth_insufficient: polygon must span full host extrusion",
                    ));
                }
            }
            (ElementKind::Opening(opening), ElementKind::Wall(wall)) => {
                let dx = wall.end.x - wall.start.x;
                let dy = wall.end.y - wall.start.y;
                let length = dx.hypot(dy);
                let ux = dx / length;
                let uy = dy / length;
                let offset_x = opening.center.x - wall.start.x;
                let offset_y = opening.center.y - wall.start.y;
                let along = offset_x * ux + offset_y * uy;
                let across = -offset_x * uy + offset_y * ux;
                let (sin, cos) = opening.rotation_rad.sin_cos();
                if (cos * uy - sin * ux).abs() > 1.0e-6 {
                    return Err(fail("Opening rotation must align with wall baseline"));
                }
                if along - opening.width_m / 2.0 < -tol
                    || along + opening.width_m / 2.0 > length + tol
                    || across.abs() > wall.thickness_m / 2.0 + tol
                    || opening.elevation_m < wall.base_elevation_m - tol
                    || opening.elevation_m + opening.height_m
                        > wall.base_elevation_m + wall.height_m + tol
                {
                    return Err(fail("Opening placement or dimensions do not fit wall"));
                }
            }
            (ElementKind::Opening(opening), ElementKind::Floor(_) | ElementKind::Roof(_)) => {
                let options = geometry_validation::GeometryOptions::default();
                let region = geometry_validation::validate_element_geometry(&host.kind, options)
                    .map_err(|error| fail(error.message))?;
                let outline = geometry_validation::validate_element_geometry(&child.kind, options)
                    .map_err(|error| fail(error.message))?;
                if !region
                    .contains_outline(&outline.boundary, tol)
                    .map_err(|error| fail(error.message))?
                {
                    return Err(fail(
                        "Opening footprint lies outside host region or intersects a hole",
                    ));
                }
                for point in &outline.boundary {
                    let top = region.top_elevation_at(*point);
                    if opening.elevation_m > top + tol
                        || opening.elevation_m + opening.height_m < top - region.thickness_m - tol
                    {
                        return Err(fail(
                            "Opening vertical extent does not intersect slab or roof",
                        ));
                    }
                }
            }
            (
                ElementKind::Window(child) | ElementKind::Door(child),
                ElementKind::Opening(opening),
            ) => {
                let (sin, cos) = opening.rotation_rad.sin_cos();
                let offset_x = child.center.x - opening.center.x;
                let offset_y = child.center.y - opening.center.y;
                let along = offset_x * cos + offset_y * sin;
                let across = -offset_x * sin + offset_y * cos;
                if (child.rotation_rad - opening.rotation_rad).sin().abs() > 1.0e-6 {
                    return Err(fail("Window/Door rotation must align with Opening"));
                }
                if along.abs() + child.width_m / 2.0 > opening.width_m / 2.0 + tol
                    || across.abs() > (opening.depth_m + child.depth_m) / 2.0 + tol
                    || child.elevation_m < opening.elevation_m - tol
                    || child.elevation_m + child.height_m
                        > opening.elevation_m + opening.height_m + tol
                {
                    return Err(fail(
                        "Window/Door placement or dimensions do not fit Opening",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_type_properties(&self, element_type: &ElementType) -> Result<(), CoreError> {
        for (id, value) in &element_type.properties {
            let definition = self
                .property_definition(&element_type.class_id, id)
                .ok_or_else(|| CoreError::UnknownProperty(id.clone()))?;
            validate_property_value(definition, value)?;
            if matches!(id.as_str(), "thickness" | "height" | "width" | "depth") {
                if let PropertyValue::Number { value, .. } = value {
                    if *value <= 0.0 {
                        return Err(bim_error(
                            EntityRef::Type(element_type.id),
                            format!("{id} must be positive"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    fn validate_type_compatibility(
        &self,
        element: &ElementRecord,
        type_id: TypeId,
    ) -> Result<(), CoreError> {
        let element_type = self
            .types
            .get(&type_id)
            .ok_or(CoreError::UnknownType(type_id))?;
        let type_category = self.effective_category(&element_type.class_id)?;
        if type_category != element.kind.category() {
            return Err(bim_error(
                EntityRef::Element(element.id),
                format!(
                    "type {type_id} class is incompatible with {:?}",
                    element.kind.element_type()
                ),
            ));
        }
        // Type properties also have to exist with compatible units in the instance class.
        for (id, value) in &element_type.properties {
            if let Some(definition) = self.property_definition(&element.class_id, id) {
                validate_property_value(definition, value)?;
            }
        }
        Ok(())
    }

    fn add_relationship(
        &mut self,
        kind: RelationshipType,
        source: EntityRef,
        target: EntityRef,
        metadata: BTreeMap<String, serde_json::Value>,
        changes: &mut Vec<Change>,
    ) -> Result<RelationshipId, CoreError> {
        for entity in [source, target] {
            if !self.entity_exists(entity) {
                return Err(bim_error(entity, "relationship references missing entity"));
            }
        }
        let id = RelationshipId::new(self.next_relationship_id);
        self.next_relationship_id = self
            .next_relationship_id
            .checked_add(1)
            .ok_or(CoreError::IdExhausted("relationship"))?;
        let record = Relationship {
            id,
            relationship_type: kind,
            source_id: source,
            target_id: target,
            metadata,
            revision_created: self
                .revision
                .checked_add(1)
                .ok_or(CoreError::RevisionExhausted)?,
            revision_deleted: None,
        };
        self.index_relationship_change(None, Some(&record));
        self.relationships.insert(id, record.clone());
        changes.push(Change::Relationship {
            id,
            before: None,
            after: Some(record),
        });
        if let (
            RelationshipType::InstanceOf,
            EntityRef::Element(element_id),
            EntityRef::Type(type_id),
        ) = (kind, source, target)
        {
            let before = self.elements[&element_id].clone();
            let mut after = before.clone();
            after.type_id = Some(type_id);
            self.elements.insert(element_id, after.clone());
            changes.push(Change::Element {
                id: element_id,
                before: Some(before),
                after: Some(after),
            });
        }
        Ok(id)
    }
    fn remove_relationship(
        &mut self,
        id: RelationshipId,
        changes: &mut Vec<Change>,
    ) -> Result<(), CoreError> {
        let before = self
            .relationships
            .get(&id)
            .filter(|r| r.is_active())
            .ok_or(CoreError::UnknownRelationship(id))?
            .clone();
        let mut after = before.clone();
        after.revision_deleted = Some(
            self.revision
                .checked_add(1)
                .ok_or(CoreError::RevisionExhausted)?,
        );
        self.index_relationship_change(Some(&before), Some(&after));
        self.relationships.insert(id, after.clone());
        changes.push(Change::Relationship {
            id,
            before: Some(before.clone()),
            after: Some(after),
        });
        if let (RelationshipType::InstanceOf, EntityRef::Element(element_id)) =
            (before.relationship_type, before.source_id)
        {
            if let Some(before) = self.elements.get(&element_id).cloned() {
                let mut after = before.clone();
                after.type_id = None;
                self.elements.insert(element_id, after.clone());
                changes.push(Change::Element {
                    id: element_id,
                    before: Some(before),
                    after: Some(after),
                });
            }
        }
        Ok(())
    }

    pub(super) fn apply_bim_command(
        &mut self,
        command: Command,
        operation_id: OperationId,
    ) -> Result<Applied, CoreError> {
        let mut changes = Vec::new();
        match self.apply_bim_command_inner(command, operation_id, &mut changes) {
            Ok((kind, parent, inputs, outputs, label, created)) => {
                Ok((kind, parent, inputs, outputs, label, changes, created))
            }
            Err(error) => {
                for change in changes.iter().rev() {
                    self.apply_change(change, false);
                }
                Err(error)
            }
        }
    }
    fn apply_bim_command_inner(
        &mut self,
        command: Command,
        operation_id: OperationId,
        changes: &mut Vec<Change>,
    ) -> Result<
        (
            OperationKind,
            Option<OperationId>,
            Vec<ElementId>,
            Vec<ElementId>,
            String,
            Vec<ElementId>,
        ),
        CoreError,
    > {
        let mut created = Vec::new();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let action;
        match command {
            Command::CreateType {
                name,
                class_id,
                properties,
            } => {
                validate_name(&name, "type")?;
                self.effective_category(&class_id)?;
                let id = TypeId::new(self.next_type_id);
                self.next_type_id = self
                    .next_type_id
                    .checked_add(1)
                    .ok_or(CoreError::IdExhausted("type"))?;
                let record = ElementType {
                    id,
                    name,
                    class_id,
                    properties,
                };
                self.validate_type_properties(&record)?;
                self.types.insert(id, record.clone());
                changes.push(Change::Type {
                    id,
                    before: None,
                    after: Some(record),
                });
                action = format!("Create type {id}");
            }
            Command::UpdateType {
                type_id,
                name,
                properties,
            } => {
                let before = self
                    .types
                    .get(&type_id)
                    .ok_or(CoreError::UnknownType(type_id))?
                    .clone();
                let mut after = before.clone();
                if let Some(name) = name {
                    validate_name(&name, "type")?;
                    after.name = name;
                }
                after.properties.extend(properties);
                self.validate_type_properties(&after)?;
                self.types.insert(type_id, after.clone());
                changes.push(Change::Type {
                    id: type_id,
                    before: Some(before),
                    after: Some(after),
                });
                action = format!("Update type {type_id}");
            }
            Command::AssignType {
                element_id,
                type_id,
            } => {
                let element = self
                    .elements
                    .get(&element_id)
                    .ok_or(CoreError::UnknownElement(element_id))?;
                self.validate_type_compatibility(element, type_id)?;
                let old: Vec<_> = self
                    .element_relationships(element_id)
                    .iter()
                    .filter(|r| {
                        r.relationship_type == RelationshipType::InstanceOf
                            && r.source_id == EntityRef::Element(element_id)
                    })
                    .map(|r| r.id)
                    .collect();
                for id in old {
                    self.remove_relationship(id, changes)?;
                }
                self.add_relationship(
                    RelationshipType::InstanceOf,
                    EntityRef::Element(element_id),
                    EntityRef::Type(type_id),
                    BTreeMap::new(),
                    changes,
                )?;
                inputs.push(element_id);
                outputs.push(element_id);
                action = format!("Assign type {type_id} to {element_id}");
            }
            Command::CreateLevel {
                name,
                elevation_m,
                height_m,
            } => {
                validate_name(&name, "level")?;
                validate_finite(elevation_m, "level elevation")?;
                if let Some(height) = height_m {
                    validate_positive(height, "level height")?;
                }
                let id = LevelId::new(self.next_level_id);
                self.next_level_id = self
                    .next_level_id
                    .checked_add(1)
                    .ok_or(CoreError::IdExhausted("level"))?;
                let record = Level {
                    id,
                    name,
                    elevation_m,
                    height_m,
                };
                self.levels.insert(id, record.clone());
                changes.push(Change::Level {
                    id,
                    before: None,
                    after: Some(record),
                });
                action = format!("Create level {id}");
            }
            Command::AssignLevel {
                element_id,
                level_id,
            } => {
                self.elements
                    .get(&element_id)
                    .ok_or(CoreError::UnknownElement(element_id))?;
                self.levels
                    .get(&level_id)
                    .ok_or(CoreError::UnknownLevel(level_id))?;
                let old: Vec<_> = self
                    .element_relationships(element_id)
                    .iter()
                    .filter(|r| {
                        r.relationship_type == RelationshipType::BelongsToLevel
                            && r.source_id == EntityRef::Element(element_id)
                    })
                    .map(|r| r.id)
                    .collect();
                for id in old {
                    self.remove_relationship(id, changes)?;
                }
                self.add_relationship(
                    RelationshipType::BelongsToLevel,
                    EntityRef::Element(element_id),
                    EntityRef::Level(level_id),
                    BTreeMap::new(),
                    changes,
                )?;
                inputs.push(element_id);
                outputs.push(element_id);
                action = format!("Assign level {level_id} to {element_id}");
            }
            Command::CreateRelationship {
                relationship_type,
                source_id,
                target_id,
                metadata,
            } => {
                let id = self.add_relationship(
                    relationship_type,
                    source_id,
                    target_id,
                    metadata,
                    changes,
                )?;
                for e in [source_id, target_id] {
                    if let EntityRef::Element(id) = e {
                        inputs.push(id);
                    }
                }
                action = format!("Create relationship {id}");
            }
            Command::RemoveRelationship { relationship_id } => {
                self.remove_relationship(relationship_id, changes)?;
                action = format!("Remove relationship {relationship_id}");
            }
            Command::HostElement {
                host_id,
                element_id,
            } => {
                self.add_relationship(
                    RelationshipType::Hosts,
                    EntityRef::Element(host_id),
                    EntityRef::Element(element_id),
                    BTreeMap::new(),
                    changes,
                )?;
                inputs = vec![host_id, element_id];
                outputs = vec![element_id];
                action = format!("Host {element_id} in {host_id}");
            }
            command @ (Command::CreateOpening { .. } | Command::CreatePolygonalOpening { .. }) => {
                let (name, kind, properties, host_id, type_id) = match command {
                    Command::CreateOpening {
                        name,
                        spec,
                        properties,
                        host_id,
                        type_id,
                    } => (
                        name,
                        ElementKind::Opening(spec),
                        properties,
                        host_id,
                        type_id,
                    ),
                    Command::CreatePolygonalOpening {
                        name,
                        spec,
                        properties,
                        host_id,
                        type_id,
                    } => (
                        name,
                        ElementKind::PolygonalOpening(spec),
                        properties,
                        host_id,
                        type_id,
                    ),
                    _ => unreachable!(),
                };
                let (_, _, _, _, _, initial, ids) = self.apply_command(
                    Command::CreateElement {
                        name,
                        class_id: BUILTIN_OPENING_CLASS_ID.into(),
                        kind,
                        properties,
                    },
                    operation_id,
                )?;
                let id = ids[0];
                changes.extend(initial);
                created.extend(ids);
                outputs.push(id);
                inputs.push(host_id);
                self.add_relationship(
                    RelationshipType::Hosts,
                    EntityRef::Element(host_id),
                    EntityRef::Element(id),
                    BTreeMap::new(),
                    changes,
                )?;
                if let Some(type_id) = type_id {
                    self.add_relationship(
                        RelationshipType::InstanceOf,
                        EntityRef::Element(id),
                        EntityRef::Type(type_id),
                        BTreeMap::new(),
                        changes,
                    )?;
                }
                action = format!("Create Opening {id}");
            }
            Command::CreateSpace {
                name,
                level_id,
                boundary,
                metadata,
            } => {
                let (_, _, _, _, _, initial, ids) = self.apply_command(
                    Command::CreateElement {
                        name,
                        class_id: BUILTIN_SPACE_CLASS_ID.into(),
                        kind: ElementKind::Space(SpaceSpec { boundary, metadata }),
                        properties: BTreeMap::new(),
                    },
                    operation_id,
                )?;
                let id = ids[0];
                changes.extend(initial);
                created.extend(ids);
                outputs.push(id);
                if let Some(level) = level_id {
                    self.add_relationship(
                        RelationshipType::BelongsToLevel,
                        EntityRef::Element(id),
                        EntityRef::Level(level),
                        BTreeMap::new(),
                        changes,
                    )?;
                }
                action = format!("Create Space {id}");
            }
            Command::DeleteElementWithStrategy {
                element_id,
                strategy,
            } => {
                let impact = self.delete_impact(element_id)?;
                let other_elements = self.element_relationships(element_id).iter().any(|r| matches!((r.source_id, r.target_id), (EntityRef::Element(a), EntityRef::Element(b)) if a != b));
                if strategy == DeleteStrategy::Reject
                    && (!impact.dependents.is_empty() || other_elements)
                {
                    return Err(bim_error(
                        EntityRef::Element(element_id),
                        "delete rejected: relationships/dependents require explicit detach or cascade",
                    ));
                }
                let mut ids: BTreeSet<_> = [element_id].into_iter().collect();
                if strategy == DeleteStrategy::Cascade {
                    ids.extend(impact.dependents);
                }
                let links: BTreeSet<_> = ids
                    .iter()
                    .flat_map(|id| self.element_relationships(*id))
                    .map(|r| r.id)
                    .collect();
                for id in links {
                    self.remove_relationship(id, changes)?;
                }
                for id in ids {
                    let before = self
                        .elements
                        .remove(&id)
                        .ok_or(CoreError::UnknownElement(id))?;
                    changes.push(Change::Element {
                        id,
                        before: Some(before),
                        after: None,
                    });
                    inputs.push(id);
                }
                action = format!("Delete {element_id} using {strategy:?}");
            }
            Command::ClearPropertyOverride {
                element_id,
                property_id,
            } => {
                let before = self
                    .elements
                    .get(&element_id)
                    .ok_or(CoreError::UnknownElement(element_id))?
                    .clone();
                let mut after = before.clone();
                after.properties.remove(&property_id);
                self.elements.insert(element_id, after.clone());
                changes.push(Change::Element {
                    id: element_id,
                    before: Some(before),
                    after: Some(after),
                });
                inputs.push(element_id);
                outputs.push(element_id);
                action = format!("Clear override {element_id}.{property_id}");
            }
            Command::UpdateElement { element_id, kind } => {
                let before = self
                    .elements
                    .get(&element_id)
                    .ok_or(CoreError::UnknownElement(element_id))?
                    .clone();
                validate_element_kind(&kind).map_err(|error| {
                    if matches!(kind, ElementKind::Opening(_)) {
                        bim_error(EntityRef::Element(element_id), error.to_string())
                    } else {
                        error
                    }
                })?;
                self.validate_class_for_kind(&before.class_id, &kind)?;
                let mut after = before.clone();
                after.kind = kind;
                self.elements.insert(element_id, after.clone());
                changes.push(Change::Element {
                    id: element_id,
                    before: Some(before),
                    after: Some(after),
                });
                inputs.push(element_id);
                outputs.push(element_id);
                action = format!("Update element {element_id}");
            }
            _ => {
                return Err(CoreError::InvalidDocument(
                    "unsupported BIM command dispatch".to_owned(),
                ));
            }
        }
        let parent = inputs
            .first()
            .and_then(|id| self.last_operation_for_element(*id));
        // Opening creation must remain identifiable by the existing creator invariant.
        if !created.is_empty() {
            for id in &created {
                if !outputs.contains(id) {
                    outputs.push(*id);
                }
            }
        }
        Ok((
            OperationKind::BimChanged {
                action: action.clone(),
            },
            parent,
            inputs,
            outputs,
            action,
            created,
        ))
    }
}
