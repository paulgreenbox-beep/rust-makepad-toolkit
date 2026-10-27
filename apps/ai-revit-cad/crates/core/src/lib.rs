//! Provisional persistent model core for the AI Revit MVP.
//!
//! The v1 native document is a provisional JSON format. It stores parametric
//! wall, floor, and roof specifications, class/property data, and operation
//! history. It does not store an exact BRep or claim to be a complete BIM
//! exchange format.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use uuid::Uuid;

mod bim;
mod drafting;
pub mod configured_building;
pub use configured_building::*;
pub mod configured_analysis;
pub use configured_analysis::*;
pub mod foundation_structural;
pub use foundation_structural::*;
pub mod geo_foundation;
pub use geo_foundation::*;
mod geo_foundation_loads;
pub use geo_foundation_loads::GeoFoundationLoadBinding;
mod geo_foundation_map;
pub use geo_foundation_map::*;
mod geo_foundation_demo;
pub use geo_foundation_demo::geo_foundation_demo_commands;
pub mod geo_slope_adapter;
pub mod engineering;
pub use engineering::*;
mod circular_search;
pub use circular_search::*;
mod circular_archive;
pub use circular_archive::*;
mod circular_archive_budget;
pub use circular_archive_budget::{ArchiveRetentionUsage, MAX_CIRCULAR_RETAINED_BYTES};
mod circular_archive_preflight;
mod circular_archive_limits;
mod circular_archive_lineage;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;
mod persistence;
pub use drafting::*;
use drafting::{
    DraftAllocators, DrawingMetadata, RuntimeDocumentId, check_fresh_draft_allocation,
    drafting_command_effects_match, inverse_draft_change, qualified_affected, summarize_drafting,
};
use persistence::{NativeEnvelope, decode_native_document};
pub use persistence::{GEO_FOUNDATION_NATIVE_FORMAT_VERSION, GEO_FOUNDATION_FEATURE};
pub mod geometry_validation;
mod history;
mod indexes;
mod receipt_integrity;
mod scalability;
mod transactions;
pub use bim::*;
use indexes::{CollectionKind, RelationshipIndexes, ScanCounters};
pub use receipt_integrity::ReceiptPayloadDiagnostic;
pub use scalability::*;
use scalability::{AllocationCounters, RevisionDelta};

#[cfg(test)]
mod bim_tests;

#[cfg(test)]
mod configured_building_tests;
#[cfg(test)]
mod foundation_structural_tests;
#[cfg(test)]
mod geo_foundation_tests;
#[cfg(test)]
mod geotech_tests;
#[cfg(test)]
mod ground_foundation_tests;
#[cfg(test)]
mod circular_search_tests;
#[cfg(test)]
mod circular_archive_tests;
#[cfg(test)]
mod circular_archive_lineage_tests;
#[cfg(test)]
mod independent_archive04_review;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod v04_tests;
#[cfg(test)]
mod v05_tests;

pub const NATIVE_FORMAT_ID: &str = "ai-revit-unified-project";
pub const NATIVE_FORMAT_VERSION: u16 = 2;
pub const PLANAR_NATIVE_FORMAT_VERSION: u16 = 3;
pub const CIRCULAR_NATIVE_FORMAT_VERSION: u16 = 4;
pub const BUILTIN_WALL_CLASS_ID: &str = "core.wall";
pub const BUILTIN_FLOOR_CLASS_ID: &str = "core.floor";
pub const BUILTIN_ROOF_CLASS_ID: &str = "core.roof";
pub const BUILTIN_OPENING_CLASS_ID: &str = "core.opening";
pub const BUILTIN_DOOR_CLASS_ID: &str = "core.door";
pub const BUILTIN_WINDOW_CLASS_ID: &str = "core.window";
pub const BUILTIN_MATERIAL_CLASS_ID: &str = "core.material";
pub const BUILTIN_SPACE_CLASS_ID: &str = "core.space";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Persistent model element identity. This is not a renderer or toolkit entity id.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ElementId(u64);
impl ElementId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Persistent operation identity in the document's build history.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct OperationId(u64);
impl OperationId {
    pub const fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

macro_rules! string_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn is_empty(&self) -> bool {
                self.0.trim().is_empty()
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
string_id!(ClassId);
string_id!(PropertyId);
string_id!(CommandId);

/// Plan coordinate in the document's SI coordinate system (meters).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}
impl Point2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Parametric wall specification. XY coordinates and vertical dimensions are meters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallSpec {
    pub start: Point2,
    pub end: Point2,
    pub base_elevation_m: f64,
    pub height_m: f64,
    pub thickness_m: f64,
}

/// Parametric slab/floor specification. Rings use implicit closure; repeating the
/// first vertex at the end is accepted and retained in the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloorSpec {
    pub boundary: Vec<Point2>,
    #[serde(default)]
    pub holes: Vec<Vec<Point2>>,
    pub elevation_m: f64,
    pub thickness_m: f64,
}

/// Parametric roof plane. slope_x/y are dimensionless dz/dx and dz/dy; zero is flat.
/// Reference elevation is at the mean of normalized footprint vertices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoofSpec {
    pub footprint: Vec<Point2>,
    pub elevation_m: f64,
    pub thickness_m: f64,
    pub slope_x: f64,
    pub slope_y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelElementType {
    Wall,
    Floor,
    Roof,
    Opening,
    Door,
    Window,
    Space,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Wall(WallSpec),
    Floor(FloorSpec),
    Roof(RoofSpec),
    Opening(OpeningSpec),
    Door(DoorSpec),
    Window(WindowSpec),
    PolygonalOpening(PolygonalOpeningSpec),
    Space(SpaceSpec),
}
impl ElementKind {
    pub const fn element_type(&self) -> ModelElementType {
        match self {
            Self::Wall(_) => ModelElementType::Wall,
            Self::Floor(_) => ModelElementType::Floor,
            Self::Roof(_) => ModelElementType::Roof,
            Self::Opening(_) | Self::PolygonalOpening(_) => ModelElementType::Opening,
            Self::Door(_) => ModelElementType::Door,
            Self::Window(_) => ModelElementType::Window,
            Self::Space(_) => ModelElementType::Space,
        }
    }
    pub const fn category(&self) -> ElementCategory {
        match self {
            Self::Wall(_) => ElementCategory::Wall,
            Self::Floor(_) => ElementCategory::Floor,
            Self::Roof(_) => ElementCategory::Roof,
            Self::Opening(_) | Self::PolygonalOpening(_) => ElementCategory::Opening,
            Self::Door(_) => ElementCategory::Door,
            Self::Window(_) => ElementCategory::Window,
            Self::Space(_) => ElementCategory::Space,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElementRecord {
    pub id: ElementId,
    pub name: String,
    pub class_id: ClassId,
    pub kind: ElementKind,
    pub properties: BTreeMap<PropertyId, PropertyValue>,
    pub created_by: OperationId,
    #[serde(default)]
    pub type_id: Option<TypeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementCategory {
    Generic,
    Wall,
    Floor,
    Roof,
    Opening,
    Door,
    Window,
    Material,
    Space,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Unitless,
    Meter,
    SquareMeter,
    CubicMeter,
    Radian,
    Kilogram,
    Second,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyType {
    Text,
    Integer,
    Boolean,
    Number { unit: Unit },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyValue {
    Text(String),
    Integer(i64),
    Boolean(bool),
    Number { value: f64, unit: Unit },
}
impl PropertyValue {
    pub const fn property_type(&self) -> PropertyType {
        match self {
            Self::Text(_) => PropertyType::Text,
            Self::Integer(_) => PropertyType::Integer,
            Self::Boolean(_) => PropertyType::Boolean,
            Self::Number { unit, .. } => PropertyType::Number { unit: *unit },
        }
    }
    fn is_finite(&self) -> bool {
        match self {
            Self::Number { value, .. } => value.is_finite(),
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyDef {
    pub id: PropertyId,
    pub name: String,
    pub value_type: PropertyType,
    pub required: bool,
    pub default_value: Option<PropertyValue>,
}
impl PropertyDef {
    pub fn new(
        id: impl Into<PropertyId>,
        name: impl Into<String>,
        value_type: PropertyType,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            value_type,
            required: false,
            default_value: None,
        }
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn with_default(mut self, value: PropertyValue) -> Self {
        self.default_value = Some(value);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassDef {
    pub id: ClassId,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<ClassId>,
    #[serde(default = "generic_category")]
    pub category: ElementCategory,
    #[serde(default)]
    pub properties: BTreeMap<PropertyId, PropertyDef>,
}
fn generic_category() -> ElementCategory {
    ElementCategory::Generic
}
impl ClassDef {
    pub fn new(id: impl Into<ClassId>, name: impl Into<String>, category: ElementCategory) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            parent_id: None,
            category,
            properties: BTreeMap::new(),
        }
    }
    pub fn child(
        id: impl Into<ClassId>,
        name: impl Into<String>,
        parent_id: impl Into<ClassId>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            parent_id: Some(parent_id.into()),
            category: ElementCategory::Generic,
            properties: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    GeoFoundation(GeoFoundationCommand),
    StructuralConfigurator(StructuralConfiguratorCommand),
    FoundationStructural(FoundationStructuralCommand),
    Engineering(EngineeringCommand),
    Draft(DraftCommand),
    DefineClass(ClassDef),
    DefineProperty {
        class_id: ClassId,
        definition: PropertyDef,
    },
    CreateElement {
        name: String,
        class_id: ClassId,
        kind: ElementKind,
        properties: BTreeMap<PropertyId, PropertyValue>,
    },
    SetProperty {
        element_id: ElementId,
        property_id: PropertyId,
        value: PropertyValue,
    },
    DeleteElement {
        element_id: ElementId,
    },
    CreateType {
        name: String,
        class_id: ClassId,
        properties: BTreeMap<PropertyId, PropertyValue>,
    },
    UpdateType {
        type_id: TypeId,
        name: Option<String>,
        properties: BTreeMap<PropertyId, PropertyValue>,
    },
    AssignType {
        element_id: ElementId,
        type_id: TypeId,
    },
    CreateLevel {
        name: String,
        elevation_m: f64,
        height_m: Option<f64>,
    },
    AssignLevel {
        element_id: ElementId,
        level_id: LevelId,
    },
    CreateRelationship {
        relationship_type: RelationshipType,
        source_id: EntityRef,
        target_id: EntityRef,
        #[serde(default)]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    RemoveRelationship {
        relationship_id: RelationshipId,
    },
    HostElement {
        host_id: ElementId,
        element_id: ElementId,
    },
    CreateOpening {
        name: String,
        spec: OpeningSpec,
        #[serde(default)]
        properties: BTreeMap<PropertyId, PropertyValue>,
        host_id: ElementId,
        #[serde(default)]
        type_id: Option<TypeId>,
    },
    CreatePolygonalOpening {
        name: String,
        spec: PolygonalOpeningSpec,
        #[serde(default)]
        properties: BTreeMap<PropertyId, PropertyValue>,
        host_id: ElementId,
        #[serde(default)]
        type_id: Option<TypeId>,
    },
    CreateSpace {
        name: String,
        #[serde(default)]
        level_id: Option<LevelId>,
        #[serde(default)]
        boundary: Option<Vec<Point2>>,
        #[serde(default)]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    DeleteElementWithStrategy {
        element_id: ElementId,
        strategy: DeleteStrategy,
    },
    ClearPropertyOverride {
        element_id: ElementId,
        property_id: PropertyId,
    },
    UpdateElement {
        element_id: ElementId,
        kind: ElementKind,
    },
}

#[derive(Clone, Debug)]
pub struct Transaction {
    command_id: CommandId,
    expected_revision: u64,
    commands: Vec<Command>,
}
impl Transaction {
    pub fn new(command_id: impl Into<CommandId>, expected_revision: u64) -> Self {
        Self {
            command_id: command_id.into(),
            expected_revision,
            commands: Vec::new(),
        }
    }
    pub fn push(&mut self, command: Command) -> &mut Self {
        self.commands.push(command);
        self
    }
    pub fn with(mut self, command: Command) -> Self {
        self.commands.push(command);
        self
    }
    pub fn rollback(self) {}
    pub fn command_id(&self) -> &CommandId {
        &self.command_id
    }
    pub const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    GeoFoundationChanged { command: GeoFoundationCommand },
    StructuralConfiguratorChanged { command: StructuralConfiguratorCommand },
    FoundationStructuralChanged { command: FoundationStructuralCommand },
    EngineeringGroundChanged {
        ground_section_id: GroundSectionId,
        command: EngineeringCommand,
    },
    EngineeringChanged {
        analysis_id: AnalysisCaseId,
        command: EngineeringCommand,
    },
    DraftingChanged {
        command: DraftCommand,
    },
    ClassDefined {
        class_id: ClassId,
        name: String,
        parent_id: Option<ClassId>,
    },
    PropertyDefined {
        class_id: ClassId,
        property_id: PropertyId,
        name: String,
    },
    ElementCreated {
        element_id: ElementId,
        class_id: ClassId,
        element_type: ModelElementType,
        name: String,
    },
    PropertySet {
        element_id: ElementId,
        class_id: ClassId,
        property_id: PropertyId,
    },
    ElementDeleted {
        element_id: ElementId,
        class_id: ClassId,
        name: String,
    },
    BimChanged {
        action: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: OperationId,
    pub command_id: CommandId,
    pub command_index: u32,
    pub revision: u64,
    /// Previous operation for the same element or class, if one exists.
    pub parent: Option<OperationId>,
    pub inputs: Vec<ElementId>,
    pub outputs: Vec<ElementId>,
    pub label: String,
    pub kind: OperationKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Receipt for the first successful application of a command id.
///
/// Undo does not release the command id: replay returns this historical receipt
/// with replayed=true and does not reapply the command. Use redo or a new id
/// when you intend to restore or intentionally reapply the change.
pub struct CommitReceipt {
    #[serde(default)]
    pub affected_objects: Vec<ProjectObjectRef>,
    #[serde(default)]
    pub created_drawings: Vec<DrawingId>,
    #[serde(default)]
    pub created_draft_entities: Vec<DraftEntityId>,
    pub command_id: CommandId,
    pub revision: u64,
    pub operation_ids: Vec<OperationId>,
    pub created_elements: Vec<ElementId>,
    /// True when this request returned a previously stored idempotent result.
    #[serde(default)]
    pub replayed: bool,
    #[serde(default)]
    pub transaction_id: CommandId,
    #[serde(default)]
    pub revision_before: u64,
    #[serde(default)]
    pub revision_after: u64,
    #[serde(default)]
    pub affected_ids: Vec<ElementId>,
    #[serde(default)]
    pub dirty_ids: Vec<ElementId>,
    #[serde(default)]
    pub validation_results: Vec<ValidationResult>,
    #[serde(default)]
    pub created_types: Vec<TypeId>,
    #[serde(default)]
    pub created_levels: Vec<LevelId>,
    #[serde(default)]
    pub created_relationships: Vec<RelationshipId>,
    /// Wall-clock time for the complete atomic commit, including validation and history.
    #[serde(default)]
    pub transaction_duration_us: u64,
    #[serde(default)]
    pub dirty_propagation_duration_us: u64,
    /// Relationship records and indexed relationship contributions visited during this operation.
    #[serde(default)]
    pub scanned_relationship_count: u64,
    /// Instrumented complete element-collection passes, including retained revision snapshots.
    #[serde(default)]
    pub document_scan_count: u64,
    #[serde(default)]
    pub snapshot_serialization_duration_us: u64,
    /// Compact JSON bytes of the newly recorded revision snapshot.
    #[serde(default)]
    pub snapshot_serialized_bytes: u64,
    #[serde(default)]
    pub delta: TransactionDelta,
    #[serde(default)]
    pub performance: CorePerformance,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryReceipt {
    #[serde(default)]
    pub affected_objects: Vec<ProjectObjectRef>,
    #[serde(default)]
    pub transaction_id: CommandId,
    #[serde(default)]
    pub revision_before: u64,
    #[serde(default)]
    pub revision_after: u64,
    #[serde(default)]
    pub affected_ids: Vec<ElementId>,
    #[serde(default)]
    pub dirty_ids: Vec<ElementId>,
    #[serde(default)]
    pub validation_results: Vec<ValidationResult>,
    pub command_id: CommandId,
    pub revision: u64,
    pub operation_ids: Vec<OperationId>,
    #[serde(default)]
    pub transaction_duration_us: u64,
    #[serde(default)]
    pub dirty_propagation_duration_us: u64,
    #[serde(default)]
    pub scanned_relationship_count: u64,
    #[serde(default)]
    pub document_scan_count: u64,
    #[serde(default)]
    pub snapshot_serialization_duration_us: u64,
    #[serde(default)]
    pub snapshot_serialized_bytes: u64,
    #[serde(default)]
    pub delta: TransactionDelta,
    #[serde(default)]
    pub performance: CorePerformance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredCommand {
    payload_json: String,
    /// Typed original request retained at first commit, independently of the
    /// retry lookup string. Legacy records remain absent rather than importing
    /// an unverified payload as its own authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded_commands: Option<Vec<Command>>,
    receipt: CommitReceipt,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct HistoryEntry {
    command_id: CommandId,
    operation_ids: Vec<OperationId>,
    changes: Vec<Change>,
}
/// Only a legacy migration needs a stack anchor: old snapshot formats did not
/// retain the operation journal timeline needed to derive their stack order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct HistoryStackAnchor {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    counters: AllocationCounters,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Change {
    GeoFoundation { before: GeoFoundationDomain, after: GeoFoundationDomain },
    StructuralConfigurator { before: StructuralConfiguratorDomain, after: StructuralConfiguratorDomain },
    FoundationStructural { before: FoundationStructuralDomain, after: FoundationStructuralDomain },
    EngineeringGroundSection {
        id: GroundSectionId,
        before: Option<GroundSection>,
        after: Option<GroundSection>,
    },
    CircularEngineeringCase {
        id: AnalysisCaseId,
        before: Option<CircularAnalysisCase>,
        after: Option<CircularAnalysisCase>,
    },
    EngineeringCase {
        id: AnalysisCaseId,
        before: Option<SlopeAnalysisCase>,
        after: Option<SlopeAnalysisCase>,
    },
    Draft(drafting_core::DraftChange),
    DraftDrawingMetadata {
        id: DrawingId,
        before: DrawingMetadata,
        after: DrawingMetadata,
    },
    DraftView {
        id: ViewDefinitionId,
        before: Option<DraftViewDefinition>,
        after: Option<DraftViewDefinition>,
    },
    Class {
        id: ClassId,
        before: Option<ClassDef>,
        after: Option<ClassDef>,
    },
    Element {
        id: ElementId,
        before: Option<ElementRecord>,
        after: Option<ElementRecord>,
    },
    Operation {
        id: OperationId,
        before: Option<OperationRecord>,
        after: Option<OperationRecord>,
    },
    Type {
        id: TypeId,
        before: Option<ElementType>,
        after: Option<ElementType>,
    },
    Level {
        id: LevelId,
        before: Option<Level>,
        after: Option<Level>,
    },
    Relationship {
        id: RelationshipId,
        before: Option<Relationship>,
        after: Option<Relationship>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    #[serde(default, skip_serializing_if = "GeoFoundationDomain::is_empty")]
    geo_foundation: GeoFoundationDomain,
    #[serde(default, skip_serializing_if = "StructuralConfiguratorDomain::is_empty")]
    structural_configurator: StructuralConfiguratorDomain,
    /// Bounded, derived generator work; never part of Native6 or its history.
    #[serde(skip)]
    configurator_generator_runtime: configured_building::ConfiguratorGeneratorRuntime,
    #[serde(default, skip_serializing_if = "FoundationStructuralDomain::is_empty")]
    foundation_structural: FoundationStructuralDomain,
    /// Derived resource accounting, never an authority or persisted field.
    #[serde(skip)]
    circular_archive_usage: OnceLock<ArchiveRetentionUsage>,
    #[serde(default, skip_serializing_if = "EngineeringDomain::is_empty")]
    engineering: EngineeringDomain,
    #[serde(default)]
    drafting: DraftingDomain,
    #[serde(default)]
    draft_ids: DraftAllocators,
    #[serde(skip)]
    runtime_document_id: RuntimeDocumentId,
    format_id: String,
    format_version: u16,
    pub document_uuid: Uuid,
    name: String,
    revision: u64,
    next_element_id: u64,
    next_operation_id: u64,
    classes: BTreeMap<ClassId, ClassDef>,
    elements: BTreeMap<ElementId, ElementRecord>,
    operations: BTreeMap<OperationId, OperationRecord>,
    command_receipts: BTreeMap<CommandId, StoredCommand>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    #[serde(default = "one")]
    next_type_id: u64,
    #[serde(default = "one")]
    next_level_id: u64,
    #[serde(default = "one")]
    next_relationship_id: u64,
    #[serde(default)]
    types: BTreeMap<TypeId, ElementType>,
    #[serde(default)]
    levels: BTreeMap<LevelId, Level>,
    #[serde(default)]
    relationships: BTreeMap<RelationshipId, Relationship>,
    #[serde(default)]
    dirty_elements: BTreeSet<ElementId>,
    #[serde(default)]
    revision_states: BTreeMap<u64, RevisionState>,
    #[serde(default)]
    history_policy: HistoryPolicy,
    #[serde(default = "extended_schema")]
    semantic_schema: u16,
    /// Derived from authoritative relationships; never persisted. Lazy initialization
    /// also supports callers deserializing Document through serde directly.
    #[serde(skip)]
    relationship_indexes: OnceLock<RelationshipIndexes>,
    #[serde(skip)]
    scan_counters: ScanCounters,
}

impl Document {
    /// Create an empty document with built-in classes for walls, floors, and roofs.
    pub fn new(name: impl Into<String>) -> Result<Self, CoreError> {
        let name = name.into();
        validate_name(&name, "document")?;
        let mut classes = BTreeMap::new();
        for class in [
            ClassDef::new(BUILTIN_WALL_CLASS_ID, "Wall", ElementCategory::Wall),
            ClassDef::new(BUILTIN_FLOOR_CLASS_ID, "Floor", ElementCategory::Floor),
            ClassDef::new(BUILTIN_ROOF_CLASS_ID, "Roof", ElementCategory::Roof),
        ] {
            classes.insert(class.id.clone(), class);
        }
        let mut document = Self {
            geo_foundation: GeoFoundationDomain::default(),
            structural_configurator: StructuralConfiguratorDomain::default(),
            configurator_generator_runtime:
                configured_building::ConfiguratorGeneratorRuntime::default(),
            foundation_structural: FoundationStructuralDomain::default(),
            circular_archive_usage: OnceLock::from(ArchiveRetentionUsage::default()),
            engineering: EngineeringDomain::default(),
            drafting: DraftingDomain::default(),
            draft_ids: DraftAllocators::default(),
            runtime_document_id: RuntimeDocumentId::default(),
            format_id: NATIVE_FORMAT_ID.to_owned(),
            format_version: NATIVE_FORMAT_VERSION,
            document_uuid: Uuid::new_v4(),
            name,
            revision: 0,
            next_element_id: 1,
            next_operation_id: 1,
            classes,
            elements: BTreeMap::new(),
            operations: BTreeMap::new(),
            command_receipts: BTreeMap::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_type_id: 1,
            next_level_id: 1,
            next_relationship_id: 1,
            types: BTreeMap::new(),
            levels: BTreeMap::new(),
            relationships: BTreeMap::new(),
            dirty_elements: BTreeSet::new(),
            revision_states: BTreeMap::new(),
            semantic_schema: 3,
            history_policy: HistoryPolicy::default(),
            relationship_indexes: OnceLock::from(RelationshipIndexes::default()),
            scan_counters: ScanCounters::default(),
        };
        document.install_bim_classes();
        document.record_revision()?;
        Ok(document)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub fn classes(&self) -> impl DoubleEndedIterator<Item = &ClassDef> + ExactSizeIterator {
        self.scan_counters.enumerate(
            self.classes.values(),
            self.classes.len(),
            "class_enumeration",
            "caller requested class records",
            CollectionKind::Other,
        )
    }
    pub fn class(&self, id: &ClassId) -> Option<&ClassDef> {
        self.classes.get(id)
    }
    pub fn elements(&self) -> impl DoubleEndedIterator<Item = &ElementRecord> + ExactSizeIterator {
        self.scan_counters.enumerate(
            self.elements.values(),
            self.elements.len(),
            "element_enumeration",
            "caller requested semantic element enumeration",
            CollectionKind::Elements,
        )
    }
    pub fn element(&self, id: ElementId) -> Option<&ElementRecord> {
        self.elements.get(&id)
    }

    /// Stable ID order; each record's parent field gives the build-tree relationship.
    pub fn operation_tree(
        &self,
    ) -> impl DoubleEndedIterator<Item = &OperationRecord> + ExactSizeIterator {
        self.scan_counters.enumerate(
            self.operations.values(),
            self.operations.len(),
            "operation_enumeration",
            "caller requested operation history records",
            CollectionKind::Other,
        )
    }
    pub fn operation(&self, id: OperationId) -> Option<&OperationRecord> {
        self.operations.get(&id)
    }
    pub fn operation_children(
        &self,
        parent: OperationId,
    ) -> impl Iterator<Item = &OperationRecord> {
        self.indexes()
            .operations_by_parent
            .get(&parent)
            .into_iter()
            .flatten()
            .filter_map(|id| self.operations.get(id))
    }

    pub fn property_definition(
        &self,
        class_id: &ClassId,
        property_id: &PropertyId,
    ) -> Option<&PropertyDef> {
        let mut current = Some(class_id);
        while let Some(id) = current {
            let class = self.classes.get(id)?;
            if let Some(definition) = class.properties.get(property_id) {
                return Some(definition);
            }
            current = class.parent_id.as_ref();
        }
        // Geometric fields are available to legacy category classes which did
        // not inherit the built-in class in the first document schema.
        self.builtin_property_definition(class_id, property_id)
    }

    pub fn begin_transaction(
        &self,
        command_id: impl Into<CommandId>,
        expected_revision: u64,
    ) -> Transaction {
        Transaction::new(command_id, expected_revision)
    }

    pub fn to_json_string(&self) -> Result<String, CoreError> {
        self.verify_invariants()?;
        let _scan=self.scan_counters.track("document_json_serialization",self.elements.len(),self.relationships.len(),"explicit native document export including retained checkpoint/delta history and receipts",true);
        let envelope = NativeEnvelope::new(self)?;
        if self.format_version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION || self.circular_archive_retention_usage()?.snapshot_occurrences != 0 {
            Ok(serde_json::to_string(&envelope)?)
        } else { Ok(serde_json::to_string_pretty(&envelope)?) }
    }
    pub fn from_json_str(source: &str) -> Result<Self, CoreError> {
        let (mut document, legacy) = decode_native_document(source)?;
        document.reserve_circular_archive_audit()?;
        document.migrate_v1()?;
        if legacy {
            for stored in document.command_receipts.values_mut() {
                stored.receipt.affected_objects = qualified_affected(
                    &stored.receipt.affected_ids,
                    &stored.receipt.delta.drafting,
                    &stored.receipt.delta.engineering,
                    &stored.receipt.delta.foundation_structural,
                    &stored.receipt.delta.structural_configurator,
                    &stored.receipt.delta.geo_foundation,
                );
            }
        }
        document.verify_invariants()?;
        Ok(document)
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = File::open(path)?;
        let admitted = circular_archive_preflight::preflight_circular_archive_reader(BufReader::new(&mut file))?;
        file.seek(SeekFrom::Start(0))?;
        persistence::preflight_native5_reader(BufReader::new(&mut file), admitted.source_bytes)?;
        file.seek(SeekFrom::Start(0))?;
        let mut source = String::new();
        let bound = admitted.source_bytes.checked_add(1).ok_or_else(|| CoreError::InvalidDocument("native file length overflow".into()))?;
        BufReader::new(file).take(bound as u64).read_to_string(&mut source)?;
        if source.len() != admitted.source_bytes {
            return Err(CoreError::InvalidDocument("native file changed during bounded read".into()));
        }
        Self::from_json_str(&source)
    }

    /// Save to a sibling temporary file and rename on the same volume. Write
    /// failures leave an existing destination intact.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), CoreError> {
        self.verify_invariants()?;
        let path = path.as_ref();
        let (temporary_path, file) = create_temporary_sibling(path)?;
        let _cleanup = TempPath(temporary_path.clone());
        let mut writer = BufWriter::new(file);
        let _scan=self.scan_counters.track("document_save_serialization",self.elements.len(),self.relationships.len(),"explicit atomic native document save including retained checkpoint/delta history and receipts",true);
        let envelope = NativeEnvelope::new(self)?;
        if self.format_version >= FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION || self.circular_archive_retention_usage()?.snapshot_occurrences != 0 {
            serde_json::to_writer(&mut writer, &envelope)?;
        } else { serde_json::to_writer_pretty(&mut writer, &envelope)?; }
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temporary_path, path)?;
        Ok(())
    }

    fn apply_command(
        &mut self,
        command: Command,
        operation_id: OperationId,
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
        match command {
            Command::GeoFoundation(command) => self.apply_geo_foundation_command(command, operation_id),
            Command::StructuralConfigurator(command) => self.apply_configurator_command(command, operation_id),
            Command::FoundationStructural(command) => self.apply_foundation_structural_command(command, operation_id),
            Command::Engineering(command) => self.apply_engineering_command(command, operation_id),
            Command::Draft(command) => self.apply_draft_command(command, operation_id),
            Command::DefineClass(definition) => {
                self.validate_class_definition(&definition)?;
                if self.classes.contains_key(&definition.id) {
                    return Err(CoreError::ClassAlreadyExists(definition.id));
                }
                let parent = definition
                    .parent_id
                    .as_ref()
                    .and_then(|id| self.last_operation_for_class(id));
                let id = definition.id.clone();
                let name = definition.name.clone();
                let after = definition.clone();
                self.classes.insert(id.clone(), definition);
                Ok((
                    OperationKind::ClassDefined {
                        class_id: id.clone(),
                        name: name.clone(),
                        parent_id: after.parent_id.clone(),
                    },
                    parent,
                    Vec::new(),
                    Vec::new(),
                    format!("Define class: {name}"),
                    vec![Change::Class {
                        id,
                        before: None,
                        after: Some(after),
                    }],
                    Vec::new(),
                ))
            }
            Command::DefineProperty {
                class_id,
                definition,
            } => {
                self.validate_property_definition(&class_id, &definition)?;
                let parent = self.last_operation_for_class(&class_id);
                let class = self
                    .classes
                    .get_mut(&class_id)
                    .ok_or_else(|| CoreError::UnknownClass(class_id.clone()))?;
                let before = class.clone();
                let class_name = class.name.clone();
                class
                    .properties
                    .insert(definition.id.clone(), definition.clone());
                let after = class.clone();
                Ok((
                    OperationKind::PropertyDefined {
                        class_id: class_id.clone(),
                        property_id: definition.id.clone(),
                        name: definition.name.clone(),
                    },
                    parent,
                    Vec::new(),
                    Vec::new(),
                    format!("Define property: {class_name}.{}", definition.name),
                    vec![Change::Class {
                        id: class_id,
                        before: Some(before),
                        after: Some(after),
                    }],
                    Vec::new(),
                ))
            }
            Command::CreateElement {
                name,
                class_id,
                kind,
                properties,
            } => {
                validate_name(&name, "element")?;
                validate_element_kind(&kind).map_err(|error| {
                    if matches!(kind, ElementKind::Opening(_)) {
                        CoreError::BimValidation {
                            entity: EntityRef::Element(ElementId::new(self.next_element_id)),
                            message: error.to_string(),
                        }
                    } else {
                        error
                    }
                })?;
                self.validate_class_for_kind(&class_id, &kind)?;
                let properties = self.validate_and_fill_properties(&class_id, properties)?;
                let id = ElementId(self.next_element_id);
                self.next_element_id = self
                    .next_element_id
                    .checked_add(1)
                    .ok_or(CoreError::IdExhausted("element"))?;
                let record = ElementRecord {
                    id,
                    name: name.clone(),
                    class_id: class_id.clone(),
                    kind: kind.clone(),
                    properties,
                    created_by: operation_id,
                    type_id: None,
                };
                self.elements.insert(id, record.clone());
                Ok((
                    OperationKind::ElementCreated {
                        element_id: id,
                        class_id,
                        element_type: kind.element_type(),
                        name: name.clone(),
                    },
                    None,
                    Vec::new(),
                    vec![id],
                    format!("Create {:?}: {name}", kind.element_type()),
                    vec![Change::Element {
                        id,
                        before: None,
                        after: Some(record),
                    }],
                    vec![id],
                ))
            }
            Command::SetProperty {
                element_id,
                property_id,
                value,
            } => {
                if !value.is_finite() {
                    return Err(CoreError::NonFiniteProperty(property_id));
                }
                let element = self
                    .elements
                    .get(&element_id)
                    .ok_or(CoreError::UnknownElement(element_id))?;
                let definition = self
                    .property_definition(&element.class_id, &property_id)
                    .or_else(|| {
                        element
                            .type_id
                            .and_then(|type_id| self.element_type(type_id))
                            .and_then(|t| self.property_definition(&t.class_id, &property_id))
                    })
                    .ok_or_else(|| CoreError::UnknownProperty(property_id.clone()))?;
                validate_property_value(definition, &value)?;
                let class_id = element.class_id.clone();
                let parent = self.last_operation_for_element(element_id);
                let before = element.clone();
                let mut after = before.clone();
                after.properties.insert(property_id.clone(), value);
                self.elements.insert(element_id, after.clone());
                Ok((
                    OperationKind::PropertySet {
                        element_id,
                        class_id,
                        property_id: property_id.clone(),
                    },
                    parent,
                    vec![element_id],
                    vec![element_id],
                    format!("Set property: {element_id}.{property_id}"),
                    vec![Change::Element {
                        id: element_id,
                        before: Some(before),
                        after: Some(after),
                    }],
                    Vec::new(),
                ))
            }
            Command::DeleteElement { element_id } => self.apply_bim_command(
                Command::DeleteElementWithStrategy {
                    element_id,
                    strategy: DeleteStrategy::Reject,
                },
                operation_id,
            ),
            command => self.apply_bim_command(command, operation_id),
        }
    }

    fn apply_change(&mut self, change: &Change, forward: bool) {
        self.index_nonrelationship_change(change, forward);
        match change {
            Change::GeoFoundation { before, after } => self.geo_foundation = (if forward { after } else { before }).clone(),
            Change::StructuralConfigurator { before, after } => self.structural_configurator = (if forward { after } else { before }).clone(),
            Change::FoundationStructural { before, after } => self.foundation_structural = (if forward { after } else { before }).clone(),
            Change::EngineeringGroundSection { id, before, after } => set_optional(
                &mut self.engineering.ground_sections,
                *id,
                if forward { after } else { before },
            ),
            Change::CircularEngineeringCase { id, before, after } => set_optional(
                &mut self.engineering.circular_cases,
                *id,
                if forward { after } else { before },
            ),
            Change::EngineeringCase { id, before, after } => set_optional(
                &mut self.engineering.slope_cases,
                *id,
                if forward { after } else { before },
            ),
            Change::Draft(change) => self.drafting.apply_change(change, forward),
            Change::DraftDrawingMetadata { id, before, after } => {
                if let Some(drawing) = self.drafting.drawings.get_mut(id) {
                    (if forward { after } else { before }).install(drawing);
                }
            }
            Change::DraftView { id, before, after } => set_optional(
                &mut self.drafting.views,
                *id,
                if forward { after } else { before },
            ),
            Change::Class { id, before, after } => set_optional(
                &mut self.classes,
                id.clone(),
                if forward { after } else { before },
            ),
            Change::Element { id, before, after } => set_optional(
                &mut self.elements,
                *id,
                if forward { after } else { before },
            ),
            Change::Type { id, before, after } => {
                set_optional(&mut self.types, *id, if forward { after } else { before })
            }
            Change::Level { id, before, after } => {
                set_optional(&mut self.levels, *id, if forward { after } else { before })
            }
            Change::Relationship { id, before, after } => {
                let current = self.relationships.get(id).cloned();
                let value = if forward { after } else { before };
                self.index_relationship_change(current.as_ref(), value.as_ref());
                set_optional(&mut self.relationships, *id, value);
            }
            Change::Operation { id, before, after } => set_optional(
                &mut self.operations,
                *id,
                if forward { after } else { before },
            ),
        }
    }

    fn validate_class_definition(&self, definition: &ClassDef) -> Result<(), CoreError> {
        validate_id(definition.id.as_str(), "class")?;
        validate_name(&definition.name, "class")?;
        if let Some(parent_id) = &definition.parent_id {
            let parent_category = self.effective_category(parent_id)?;
            if definition.category != ElementCategory::Generic
                && parent_category != ElementCategory::Generic
                && definition.category != parent_category
            {
                return Err(CoreError::ClassCategoryConflict {
                    class_id: definition.id.clone(),
                    parent_id: parent_id.clone(),
                });
            }
        }
        let mut seen = BTreeSet::new();
        let mut current = definition.parent_id.as_ref();
        while let Some(parent_id) = current {
            let class = self
                .classes
                .get(parent_id)
                .ok_or_else(|| CoreError::UnknownClass(parent_id.clone()))?;
            seen.insert(class.id.clone());
            current = class.parent_id.as_ref();
        }
        if seen.contains(&definition.id) {
            return Err(CoreError::ClassCycle(definition.id.clone()));
        }
        for (property_id, property) in &definition.properties {
            if property_id != &property.id {
                return Err(CoreError::InvalidDocument(format!(
                    "class property map key {property_id} does not match its definition id"
                )));
            }
            validate_property_definition_fields(property)?;
            if self
                .property_definition_in_parent(definition.parent_id.as_ref(), property_id)
                .is_some()
            {
                return Err(CoreError::PropertyAlreadyDefined(property_id.clone()));
            }
        }
        Ok(())
    }

    fn validate_property_definition(
        &self,
        class_id: &ClassId,
        definition: &PropertyDef,
    ) -> Result<(), CoreError> {
        let class = self
            .classes
            .get(class_id)
            .ok_or_else(|| CoreError::UnknownClass(class_id.clone()))?;
        validate_property_definition_fields(definition)?;
        if self.property_definition(class_id, &definition.id).is_some()
            || class.properties.contains_key(&definition.id)
            || self.property_definition_in_descendants(class_id, &definition.id)
        {
            return Err(CoreError::PropertyAlreadyDefined(definition.id.clone()));
        }
        Ok(())
    }

    fn validate_class_for_kind(
        &self,
        class_id: &ClassId,
        kind: &ElementKind,
    ) -> Result<(), CoreError> {
        let category = self.effective_category(class_id)?;
        if category != ElementCategory::Generic && category != kind.category() {
            return Err(CoreError::ClassCategoryMismatch {
                class_id: class_id.clone(),
                expected: kind.category(),
                actual: category,
            });
        }
        Ok(())
    }

    fn effective_category(&self, class_id: &ClassId) -> Result<ElementCategory, CoreError> {
        let mut current = Some(class_id);
        let mut visited = BTreeSet::new();
        let mut inherited = ElementCategory::Generic;
        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                return Err(CoreError::ClassCycle(id.clone()));
            }
            let class = self
                .classes
                .get(id)
                .ok_or_else(|| CoreError::UnknownClass(id.clone()))?;
            if class.category != ElementCategory::Generic {
                if inherited != ElementCategory::Generic && inherited != class.category {
                    return Err(CoreError::ClassCategoryConflict {
                        class_id: class.id.clone(),
                        parent_id: class.parent_id.clone().unwrap_or_else(|| class.id.clone()),
                    });
                }
                inherited = class.category;
            }
            current = class.parent_id.as_ref();
        }
        Ok(inherited)
    }

    fn validate_and_fill_properties(
        &self,
        class_id: &ClassId,
        values: BTreeMap<PropertyId, PropertyValue>,
    ) -> Result<BTreeMap<PropertyId, PropertyValue>, CoreError> {
        let definitions = self.properties_for_class(class_id)?;
        for (id, value) in &values {
            let definition = definitions
                .get(id)
                .ok_or_else(|| CoreError::UnknownProperty(id.clone()))?;
            if !value.is_finite() {
                return Err(CoreError::NonFiniteProperty(id.clone()));
            }
            validate_property_value(definition, value)?;
        }
        // Required fields are checked after the complete atomic batch so a
        // following AssignType can provide them through inheritance.
        Ok(values)
    }

    fn properties_for_class(
        &self,
        class_id: &ClassId,
    ) -> Result<BTreeMap<PropertyId, PropertyDef>, CoreError> {
        let mut chain = Vec::new();
        let mut current = Some(class_id);
        let mut visited = BTreeSet::new();
        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                return Err(CoreError::ClassCycle(id.clone()));
            }
            let class = self
                .classes
                .get(id)
                .ok_or_else(|| CoreError::UnknownClass(id.clone()))?;
            chain.push(class);
            current = class.parent_id.as_ref();
        }
        let mut definitions = self.builtin_properties_for_class(class_id);
        for class in chain.into_iter().rev() {
            definitions.extend(
                class
                    .properties
                    .iter()
                    .map(|(id, def)| (id.clone(), def.clone())),
            );
        }
        Ok(definitions)
    }

    fn property_definition_in_parent(
        &self,
        parent_id: Option<&ClassId>,
        property_id: &PropertyId,
    ) -> Option<&PropertyDef> {
        let mut current = parent_id;
        while let Some(id) = current {
            let class = self.classes.get(id)?;
            if let Some(definition) = class.properties.get(property_id) {
                return Some(definition);
            }
            current = class.parent_id.as_ref();
        }
        None
    }

    fn property_definition_in_descendants(
        &self,
        class_id: &ClassId,
        property_id: &PropertyId,
    ) -> bool {
        let mut scan = self.scan_counters.track(
            "class_descendant_property_lookup",
            0,
            0,
            "indexed class descendants only",
            false,
        );
        let indexes = self.indexes();
        let mut pending: Vec<_> = indexes
            .class_children
            .get(class_id)
            .into_iter()
            .flatten()
            .collect();
        let mut visited = BTreeSet::new();
        let mut found = false;
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            if self
                .classes
                .get(id)
                .is_some_and(|class| class.properties.contains_key(property_id))
            {
                found = true;
                break;
            }
            pending.extend(indexes.class_children.get(id).into_iter().flatten());
        }
        scan.set_reason(format!("visited {} descendant class records through class_children; unrelated classes excluded", visited.len()));
        found
    }

    fn last_operation_for_element(&self, element_id: ElementId) -> Option<OperationId> {
        self.indexes()
            .operations_by_element
            .get(&element_id)?
            .iter()
            .next_back()
            .copied()
    }
    fn last_operation_for_class(&self, class_id: &ClassId) -> Option<OperationId> {
        self.indexes()
            .operations_by_class
            .get(class_id)?
            .iter()
            .next_back()
            .copied()
    }

    fn verify_invariants(&self) -> Result<(), CoreError> {
        self.audit_circular_archive_budget()?;
        self.reserve_circular_archive_audit()?;
        let _scan = self.scan_counters.track(
            "document_invariant_audit",
            self.elements.len(),
            self.relationships.len(),
            "explicit open/save audit of persisted authoritative records",
            true,
        );
        if self.format_id != NATIVE_FORMAT_ID {
            return Err(CoreError::WrongFormat(self.format_id.clone()));
        }
        if ![NATIVE_FORMAT_VERSION, PLANAR_NATIVE_FORMAT_VERSION, CIRCULAR_NATIVE_FORMAT_VERSION, FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION, STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION, GEO_FOUNDATION_NATIVE_FORMAT_VERSION]
            .contains(&self.format_version)
        {
            return Err(CoreError::UnsupportedFormatVersion(self.format_version));
        }
        self.verify_drafting_history()?;
        if self.has_circular_engineering_history()
            && self.format_version < CIRCULAR_NATIVE_FORMAT_VERSION
        {
            return Err(CoreError::InvalidDocument(
                "Ground/circular authority or retained history requires native format 4".into(),
            ));
        }
        if self.has_engineering_history() && self.format_version < PLANAR_NATIVE_FORMAT_VERSION {
            return Err(CoreError::InvalidDocument(
                "Engineering history requires native format 3".into(),
            ));
        }
        self.engineering.validate()?;
        self.verify_engineering_owners()?;
        self.validate_foundation_structural()?;
        self.validate_configurator()?;
        self.validate_geo_foundation()?;
        if self.has_configurator_history() && self.format_version < STRUCTURAL_CONFIGURATOR_NATIVE_FORMAT_VERSION { return Err(CoreError::InvalidDocument("configurator history requires native6 or later".into())); }
        if self.has_geo_foundation_history() && self.format_version != GEO_FOUNDATION_NATIVE_FORMAT_VERSION { return Err(CoreError::InvalidDocument("GEO/foundation history requires native7".into())); }
        self.validate_foundation_structural_retention()?;
        if self.has_foundation_structural_history() && self.format_version < FOUNDATION_STRUCTURAL_NATIVE_FORMAT_VERSION {
            return Err(CoreError::InvalidDocument("structural/foundation history requires native5".into()));
        }
        if self.document_uuid.is_nil() {
            return Err(CoreError::InvalidDocument(
                "document_uuid must not be nil".to_owned(),
            ));
        }
        validate_name(&self.name, "document")?;
        if self.next_element_id == 0 || self.next_operation_id == 0 {
            return Err(CoreError::InvalidDocument(
                "persistent ID counters must be non-zero".to_owned(),
            ));
        }

        let mut max_element_id = 0;
        self.scan_counters.document_scan();
        for (id, element) in &self.elements {
            if id.get() == 0 || *id != element.id {
                return Err(CoreError::InvalidDocument(
                    "element map key does not match a non-zero element id".to_owned(),
                ));
            }
            max_element_id = max_element_id.max(id.get());
            validate_name(&element.name, "element")?;
            validate_element_kind(&element.kind)?;
            self.validate_class_for_kind(&element.class_id, &element.kind)?;
            let creator = self.operations.get(&element.created_by).ok_or_else(|| {
                CoreError::InvalidDocument(format!(
                    "element {id} references missing creation operation {}",
                    element.created_by
                ))
            })?;
            if !creator.outputs.contains(id) {
                return Err(CoreError::InvalidDocument(format!(
                    "creation operation {} does not list element {id} as output",
                    element.created_by
                )));
            }
            let definitions = self.element_property_definitions(element)?;
            for (property_id, value) in &element.properties {
                if let Some(definition) = definitions.get(property_id) {
                    if !value.is_finite() {
                        return Err(CoreError::NonFiniteProperty(property_id.clone()));
                    }
                    validate_property_value(definition, value)?;
                }
                // Unknown property ids are retained for forward-compatible round trips.
            }
        }
        if self.next_element_id <= max_element_id {
            return Err(CoreError::InvalidDocument(
                "next_element_id must exceed every persisted element id".to_owned(),
            ));
        }

        for (id, class) in &self.classes {
            if id != &class.id || id.is_empty() {
                return Err(CoreError::InvalidDocument(
                    "class map key does not match a non-empty class id".to_owned(),
                ));
            }
            validate_name(&class.name, "class")?;
            for (property_id, property) in &class.properties {
                if property_id != &property.id {
                    return Err(CoreError::InvalidDocument(format!(
                        "class property map key {property_id} does not match its definition id"
                    )));
                }
                validate_property_definition_fields(property)?;
            }
            self.effective_category(id)?;
            let mut inherited_ids = BTreeSet::new();
            let mut current = class.parent_id.as_ref();
            while let Some(parent_id) = current {
                let parent = self
                    .classes
                    .get(parent_id)
                    .ok_or_else(|| CoreError::UnknownClass(parent_id.clone()))?;
                inherited_ids.extend(parent.properties.keys().cloned());
                current = parent.parent_id.as_ref();
            }
            if class
                .properties
                .keys()
                .any(|property_id| inherited_ids.contains(property_id))
            {
                return Err(CoreError::InvalidDocument(format!(
                    "class {} redeclares an inherited property",
                    class.id
                )));
            }
        }

        let mut max_operation_id = 0;
        for (id, operation) in &self.operations {
            if id.get() == 0 || *id != operation.id {
                return Err(CoreError::InvalidDocument(
                    "operation map key does not match a non-zero operation id".to_owned(),
                ));
            }
            max_operation_id = max_operation_id.max(id.get());
            if let Some(parent) = operation.parent {
                if parent >= *id || !self.operations.contains_key(&parent) {
                    return Err(CoreError::InvalidDocument(format!(
                        "operation {id} has a missing or non-prior parent {parent}"
                    )));
                }
            }
        }
        if self.next_operation_id <= max_operation_id {
            return Err(CoreError::InvalidDocument(
                "next_operation_id must exceed every persisted operation id".to_owned(),
            ));
        }
        for (command_id, stored) in &self.command_receipts {
            if command_id.is_empty() || stored.receipt.command_id != *command_id {
                return Err(CoreError::InvalidDocument(
                    "command receipt key does not match its non-empty command id".to_owned(),
                ));
            }
        }
        self.validate_bim()?;
        self.verify_revision_states_and_counters()?;
        self.audit_geo_foundation_sources()?;
        self.verify_index_consistency()?;
        Ok(())
    }
}

fn set_optional<K, V>(map: &mut BTreeMap<K, V>, key: K, value: &Option<V>)
where
    K: Ord + Clone,
    V: Clone,
{
    if let Some(value) = value {
        map.insert(key, value.clone());
    } else {
        map.remove(&key);
    }
}
fn validate_id(value: &str, kind: &'static str) -> Result<(), CoreError> {
    if value.trim().is_empty() {
        Err(CoreError::EmptyIdentifier(kind))
    } else {
        Ok(())
    }
}
fn elapsed_us(start: Instant) -> u64 {
    start.elapsed().as_micros().min(u64::MAX as u128) as u64
}
fn validate_name(value: &str, kind: &'static str) -> Result<(), CoreError> {
    if value.trim().is_empty() {
        Err(CoreError::EmptyName(kind))
    } else {
        Ok(())
    }
}
fn validate_property_definition_fields(definition: &PropertyDef) -> Result<(), CoreError> {
    validate_id(definition.id.as_str(), "property")?;
    validate_name(&definition.name, "property")?;
    if let Some(default_value) = &definition.default_value {
        if !default_value.is_finite() {
            return Err(CoreError::NonFiniteProperty(definition.id.clone()));
        }
        validate_property_value(definition, default_value)?;
    }
    Ok(())
}
fn validate_property_value(
    definition: &PropertyDef,
    value: &PropertyValue,
) -> Result<(), CoreError> {
    if definition.value_type != value.property_type() {
        return Err(CoreError::PropertyTypeMismatch {
            property_id: definition.id.clone(),
            expected: definition.value_type,
            actual: value.property_type(),
        });
    }
    if !value.is_finite() {
        return Err(CoreError::NonFiniteProperty(definition.id.clone()));
    }
    Ok(())
}

fn validate_element_kind(kind: &ElementKind) -> Result<(), CoreError> {
    match kind {
        ElementKind::Space(spec) => {
            if let Some(boundary) = &spec.boundary {
                geometry_validation::validate_planar_contour(
                    boundary,
                    geometry_validation::GeometryOptions::default(),
                )
                .map_err(|e| CoreError::InvalidGeometry(e.message.into()))?;
            }
            return Ok(());
        }
        ElementKind::PolygonalOpening(spec) => {
            validate_point(spec.center)?;
            validate_finite(spec.elevation_m, "hosted elevation")?;
            validate_finite(spec.rotation_rad, "hosted rotation")?;
            validate_positive(spec.depth_m, "hosted depth")?;
        }
        ElementKind::Opening(spec) | ElementKind::Door(spec) | ElementKind::Window(spec) => {
            validate_point(spec.center)?;
            validate_finite(spec.elevation_m, "hosted elevation")?;
            validate_finite(spec.rotation_rad, "hosted rotation")?;
            validate_positive(spec.width_m, "hosted width")?;
            validate_positive(spec.height_m, "hosted height")?;
            validate_positive(spec.depth_m, "hosted depth")?;
        }
        ElementKind::Wall(spec) => {
            validate_point(spec.start)?;
            validate_point(spec.end)?;
            validate_finite(spec.base_elevation_m, "wall base elevation")?;
            validate_positive(spec.height_m, "wall height")?;
            validate_positive(spec.thickness_m, "wall thickness")?;
        }
        ElementKind::Floor(spec) => {
            validate_finite(spec.elevation_m, "floor elevation")?;
            validate_positive(spec.thickness_m, "floor thickness")?;
            for point in spec.boundary.iter().chain(spec.holes.iter().flatten()) {
                validate_point(*point)?;
            }
        }
        ElementKind::Roof(spec) => {
            validate_finite(spec.elevation_m, "roof elevation")?;
            validate_positive(spec.thickness_m, "roof thickness")?;
            validate_finite(spec.slope_x, "roof slope_x")?;
            validate_finite(spec.slope_y, "roof slope_y")?;
            for point in &spec.footprint {
                validate_point(*point)?;
            }
        }
    }
    geometry_validation::validate_element_geometry(
        kind,
        geometry_validation::GeometryOptions::default(),
    )
    .map(|_| ())
    .map_err(|error| CoreError::InvalidGeometry(format!("{} ({:?})", error.message, error.code)))
}
fn validate_point(point: Point2) -> Result<(), CoreError> {
    validate_finite(point.x, "point.x")?;
    validate_finite(point.y, "point.y")
}
fn validate_finite(value: f64, label: &'static str) -> Result<(), CoreError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(CoreError::NonFiniteDimension(label))
    }
}
fn validate_positive(value: f64, label: &'static str) -> Result<(), CoreError> {
    validate_finite(value, label)?;
    if value > 0.0 {
        Ok(())
    } else {
        Err(CoreError::NonPositiveDimension(label))
    }
}
fn create_temporary_sibling(destination: &Path) -> Result<(PathBuf, File), CoreError> {
    let file_name = destination
        .file_name()
        .ok_or(CoreError::InvalidPath)?
        .to_string_lossy();
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..64 {
        let serial = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            serial
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(CoreError::Io(error)),
        }
    }
    Err(CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique sibling temporary file",
    )))
}
struct TempPath(PathBuf);
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum CoreError {
    CircularArchiveLimit { scope: &'static str, limit: usize },
    Geotech(geotech_core::GeotechError),
    Io(std::io::Error),
    Json(serde_json::Error),
    WrongFormat(String),
    UnsupportedFormatVersion(u16),
    InvalidPath,
    InvalidDocument(String),
    EmptyName(&'static str),
    EmptyIdentifier(&'static str),
    EmptyCommandId,
    EmptyTransaction,
    TransactionTooLarge,
    RevisionConflict {
        expected: u64,
        actual: u64,
    },
    RevisionExhausted,
    IdExhausted(&'static str),
    CommandIdReused(CommandId),
    ClassAlreadyExists(ClassId),
    UnknownClass(ClassId),
    ClassCycle(ClassId),
    ClassCategoryConflict {
        class_id: ClassId,
        parent_id: ClassId,
    },
    ClassCategoryMismatch {
        class_id: ClassId,
        expected: ElementCategory,
        actual: ElementCategory,
    },
    PropertyAlreadyDefined(PropertyId),
    UnknownProperty(PropertyId),
    RequiredPropertyMissing(PropertyId),
    PropertyTypeMismatch {
        property_id: PropertyId,
        expected: PropertyType,
        actual: PropertyType,
    },
    UnknownElement(ElementId),
    NonFiniteProperty(PropertyId),
    NonFiniteDimension(&'static str),
    NonPositiveDimension(&'static str),
    InvalidGeometry(String),
    BimValidation {
        entity: EntityRef,
        message: String,
    },
    UnknownType(TypeId),
    UnknownLevel(LevelId),
    UnknownRelationship(RelationshipId),
    UnknownRevision(u64),
}
impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<geotech_core::GeotechError> for CoreError {
    fn from(error: geotech_core::GeotechError) -> Self {
        Self::Geotech(error)
    }
}
impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CircularArchiveLimit { scope, limit } => write!(f, "circular archive {scope} exceeds {limit} bytes"),
            Self::Geotech(error) => write!(f, "Geotechnical calculation: {error}"),
            Self::BimValidation { entity, message } => {
                write!(f, "BIM validation for {entity:?}: {message}")
            }
            Self::UnknownType(id) => write!(f, "type {id} does not exist"),
            Self::UnknownLevel(id) => write!(f, "level {id} does not exist"),
            Self::UnknownRelationship(id) => write!(f, "relationship {id} does not exist"),
            Self::UnknownRevision(revision) => write!(f, "revision {revision} is not available"),
            Self::Io(error) => write!(f, "document I/O failed: {error}"),
            Self::Json(error) => write!(f, "document JSON is invalid: {error}"),
            Self::WrongFormat(format) => write!(f, "unsupported document format {format}"),
            Self::UnsupportedFormatVersion(version) => {
                write!(f, "unsupported document format version {version}")
            }
            Self::InvalidPath => f.write_str("document path must include a file name"),
            Self::InvalidDocument(message) => write!(f, "document invariant failed: {message}"),
            Self::EmptyName(kind) => write!(f, "{kind} name must not be empty"),
            Self::EmptyIdentifier(kind) => write!(f, "{kind} id must not be empty"),
            Self::EmptyCommandId => f.write_str("command id must not be empty"),
            Self::EmptyTransaction => f.write_str("transaction must contain at least one command"),
            Self::TransactionTooLarge => f.write_str("transaction contains too many commands"),
            Self::RevisionConflict { expected, actual } => write!(
                f,
                "revision conflict: expected {expected}, document is at {actual}"
            ),
            Self::RevisionExhausted => f.write_str("document revision counter is exhausted"),
            Self::IdExhausted(kind) => write!(f, "{kind} id counter is exhausted"),
            Self::CommandIdReused(id) => {
                write!(f, "command id {id} was reused with a different batch")
            }
            Self::ClassAlreadyExists(id) => write!(f, "class {id} already exists"),
            Self::UnknownClass(id) => write!(f, "class {id} does not exist"),
            Self::ClassCycle(id) => write!(f, "class inheritance cycle reaches {id}"),
            Self::ClassCategoryConflict {
                class_id,
                parent_id,
            } => write!(f, "class {class_id} conflicts with parent {parent_id}"),
            Self::ClassCategoryMismatch {
                class_id,
                expected,
                actual,
            } => write!(
                f,
                "class {class_id} has category {actual:?}, expected {expected:?}"
            ),
            Self::PropertyAlreadyDefined(id) => write!(
                f,
                "property {id} is already defined in this class hierarchy"
            ),
            Self::UnknownProperty(id) => write!(f, "property {id} is not defined for this class"),
            Self::RequiredPropertyMissing(id) => write!(f, "required property {id} is missing"),
            Self::PropertyTypeMismatch {
                property_id,
                expected,
                actual,
            } => write!(
                f,
                "property {property_id} expects {expected:?}, received {actual:?}"
            ),
            Self::UnknownElement(id) => write!(f, "element {id} does not exist"),
            Self::NonFiniteProperty(id) => write!(f, "property {id} must be finite"),
            Self::NonFiniteDimension(label) => write!(f, "{label} must be finite"),
            Self::NonPositiveDimension(label) => write!(f, "{label} must be positive"),
            Self::InvalidGeometry(message) => write!(f, "invalid parametric geometry: {message}"),
        }
    }
}
impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}
