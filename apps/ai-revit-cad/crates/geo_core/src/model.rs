use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const MAX_RECORDS: usize = 4096;
pub const MAX_MODEL_WELLS: usize = 256;
pub const MAX_TRIANGLES: usize = 512;
pub const MAX_CONTACTS: usize = 32;
pub const MAX_SECTION_POINTS: usize = 256;
pub const MAX_ABS_COORDINATE_M: f64 = 1_000_000.0;
pub const MAX_MODEL_SPAN_M: f64 = 100_000.0;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug,Clone,PartialEq,Eq,PartialOrd,Ord,Hash,Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value:impl Into<String>)->Result<Self,GeoError>{
                let value=value.into();
                if value.is_empty()||value.len()>128||!value.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-.@:/%".contains(&b)){
                    return Err(GeoError::InvalidId{value});
                }
                Ok(Self(value))
            }
            pub fn as_str(&self)->&str{&self.0}
        }
        impl std::fmt::Display for $name{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{std::fmt::Display::fmt(&self.0,f)}}
        impl AsRef<str> for $name{fn as_ref(&self)->&str{&self.0}}
        impl std::borrow::Borrow<str> for $name{fn borrow(&self)->&str{&self.0}}
        impl TryFrom<&str> for $name{type Error=GeoError;fn try_from(v:&str)->Result<Self,GeoError>{Self::new(v)}}
        impl TryFrom<String> for $name{type Error=GeoError;fn try_from(v:String)->Result<Self,GeoError>{Self::new(v)}}
        impl<'de> Deserialize<'de> for $name{fn deserialize<D:serde::Deserializer<'de>>(d:D)->Result<Self,D::Error>{Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)}}
    };
}
id_type!(GeoSoilId);id_type!(IgeId);id_type!(BoreholeId);id_type!(GroundModelId);
id_type!(GroundScenarioId);id_type!(GeoSnapshotId);id_type!(SourceId);id_type!(HorizonId);
id_type!(TriangleId);id_type!(GeoSectionId);id_type!(WaterObservationId);
id_type!(BoreholeIntervalId);

#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSourceRevision { pub document_uuid:String, pub operation_id:u64 }

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum SourceOrigin { Synthetic, Measured, Interpreted, ImportedLegacy }
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecord {
    pub id:SourceId,pub title:String,pub origin:SourceOrigin,pub reference:String,
    pub content_sha256:Option<String>,pub observed_at:Option<String>,pub revision:u64,
    pub source_page_or_record:Option<String>,pub source_date:Option<String>,pub source_revision:Option<String>,
}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum ParameterUnit {KnPerM3,Kpa,Degree,Metre,Dimensionless,#[default] Unspecified}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum ValueClass {Measured,Mean,Characteristic,Design,Synthetic,#[default] Unspecified}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum StrengthContext {EffectiveDrained,TotalUndrained,NotApplicable,#[default] Unspecified}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum SaturationContext {Dry,Natural,Saturated,NotApplicable,#[default] Unspecified}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum ConfidenceClass {Synthetic,SourceDeclared,Reviewed,#[default] Unverified}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize,Default)]
#[serde(deny_unknown_fields)]
pub struct ParameterMetadata {
    pub unit:ParameterUnit,pub value_class:ValueClass,pub strength_context:StrengthContext,
    pub saturation:SaturationContext,pub confidence_class:ConfidenceClass,pub engineering_verified:bool,
    pub source_page_or_record:Option<String>,pub source_date:Option<String>,pub source_revision:Option<String>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterCandidate<T> {pub value:T,pub source_ids:Vec<SourceId>,pub metadata:ParameterMetadata}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="status",rename_all="SCREAMING_SNAKE_CASE",deny_unknown_fields)]
pub enum Parameter<T> {
    Missing {reason:String},
    Known {value:T,source_ids:Vec<SourceId>,metadata:ParameterMetadata},
    Conflicting {candidates:Vec<ParameterCandidate<T>>},
}
impl<T> Default for Parameter<T>{fn default()->Self{Self::Missing{reason:"NOT_PROVIDED".into()}}}
impl<T> Parameter<T>{pub fn known(&self)->Option<&T>{if let Self::Known{value,..}=self{Some(value)}else{None}}}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize,Default)]
#[serde(deny_unknown_fields)]
pub struct SoilParameters {
    pub unit_weight_kn_m3:Parameter<f64>,pub cohesion_kpa:Parameter<f64>,
    pub friction_angle_deg:Parameter<f64>,pub elastic_modulus_kpa:Parameter<f64>,pub poisson_ratio:Parameter<f64>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSoilRecord {pub id:GeoSoilId,pub name:String,pub parameters:SoilParameters,pub revision:u64}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IgeRecord {pub id:IgeId,pub name:String,pub soil_id:GeoSoilId,pub source_ids:Vec<SourceId>,pub revision:u64}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoreholeInterval {pub id:BoreholeIntervalId,pub top_depth_m:f64,pub bottom_depth_m:f64,pub ige_id:IgeId,pub source_ids:Vec<SourceId>}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Borehole {
    pub id:BoreholeId,pub collar_m:[f64;3],pub source_ids:Vec<SourceId>,
    pub intervals:Vec<BoreholeInterval>,pub revision:u64,
    pub total_depth_m:Parameter<f64>,pub drilling_date:Option<String>,pub crs:Option<String>,
    #[serde(default)]
    pub collar_vertical_datum:Option<String>,
    #[serde(default)]
    pub depth_datum:String,
    pub survey_source_ids:Vec<SourceId>,pub evidence_source_ids:Vec<SourceId>,
}
/// A missing well CRS has no numeric interpretation unless the model explicitly
/// permits inheritance. This policy is persisted and content-fingerprinted.
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum MissingBoreholeCrsPolicy {#[default] Unspecified,RequireExplicit,InheritModel}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoCoordinateFrame {
    pub name:String,pub crs:Option<String>,pub vertical_datum:Option<String>,
    #[serde(default)]
    pub missing_borehole_crs:MissingBoreholeCrsPolicy,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoTriangle {pub id:TriangleId,pub vertices:[BoreholeId;3],pub source_ids:Vec<SourceId>}
/// Ordered bottoms of layers. Terrain is the top of the first layer; each
/// contact identifies the IGE between the preceding surface and this bottom.
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerContact {
    pub id:HorizonId,pub ige_id:IgeId,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub elevations_abs_m:BTreeMap<BoreholeId,Parameter<f64>>,
    pub source_ids:Vec<SourceId>,
    #[serde(default)]
    pub vertical_datum:Option<String>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownZone {pub polygon_xy_m:Vec<[f64;2]>,pub reason:String,pub source_ids:Vec<SourceId>}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum InterpolationMethod { ExplicitTinPiecewisePlanar }
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundModel {
    pub id:GroundModelId,pub name:String,pub frame:GeoCoordinateFrame,
    pub borehole_ids:Vec<BoreholeId>,pub triangles:Vec<GeoTriangle>,
    pub contacts:Vec<LayerContact>,pub allow_pinch:bool,pub unknown_zones:Vec<UnknownZone>,
    pub interpolation:InterpolationMethod,pub source_ids:Vec<SourceId>,
    pub section_ids:Vec<GeoSectionId>,pub revision:u64,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterObservation {
    pub id:WaterObservationId,pub xy_m:[f64;2],pub elevation_abs_m:Parameter<f64>,
    pub observed_at:Option<String>,pub revision:u64,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="kind",rename_all="SCREAMING_SNAKE_CASE",deny_unknown_fields)]
pub enum WaterScenario {
    Unknown,
    ExplicitDry {source_ids:Vec<SourceId>},
    Observations {observation_ids:Vec<WaterObservationId>},
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoScenario {
    pub id:GroundScenarioId,pub name:String,pub model_id:GroundModelId,
    pub water:WaterScenario,pub source_ids:Vec<SourceId>,pub revision:u64,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterpretedSection {
    pub id:GeoSectionId,pub model_id:GroundModelId,pub polyline_xy_m:Vec<[f64;2]>,
    pub source_ids:Vec<SourceId>,pub revision:u64,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize,Default)]
#[serde(deny_unknown_fields)]
pub struct GeoRecords {
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub sources:BTreeMap<SourceId,SourceRecord>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub soils:BTreeMap<GeoSoilId,GeoSoilRecord>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub iges:BTreeMap<IgeId,IgeRecord>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub boreholes:BTreeMap<BoreholeId,Borehole>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub models:BTreeMap<GroundModelId,GroundModel>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub scenarios:BTreeMap<GroundScenarioId,GeoScenario>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub water_observations:BTreeMap<WaterObservationId,WaterObservation>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub sections:BTreeMap<GeoSectionId,InterpretedSection>,
}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSnapshotSelection {pub model_id:GroundModelId,pub scenario_id:Option<GroundScenarioId>}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSnapshot {
    pub id:GeoSnapshotId,pub source_revision:GeoSourceRevision,pub selection:GeoSnapshotSelection,
    pub records:GeoRecords,pub selection_fingerprint:String,pub content_fingerprint:String,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize,Default)]
#[serde(deny_unknown_fields)]
pub struct GeoState {
    pub records:GeoRecords,pub head_revision:Option<GeoSourceRevision>,
    #[serde(deserialize_with="crate::unique_map::deserialize")]
    pub snapshots:BTreeMap<GeoSnapshotId,GeoSnapshot>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="command",rename_all="SCREAMING_SNAKE_CASE",deny_unknown_fields)]
pub enum GeoCommand {
    ImportRecords{records:GeoRecords},
    UpsertSource{record:SourceRecord},UpsertSoil{record:GeoSoilRecord},UpsertIge{record:IgeRecord},
    UpsertBorehole{record:Borehole},UpsertModel{record:GroundModel},UpsertScenario{record:GeoScenario},
    UpsertWaterObservation{record:WaterObservation},UpsertSection{record:InterpretedSection},
    SaveSnapshot{id:GeoSnapshotId,selection:GeoSnapshotSelection},
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize,Default)]
#[serde(deny_unknown_fields)]
pub struct GeoDelta {pub changed_records:Vec<String>,pub created_snapshot_ids:Vec<GeoSnapshotId>}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoMutation {pub state:GeoState,pub delta:GeoDelta}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum SnapshotCurrentness {Current,Stale}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedGeoSnapshot {pub snapshot:GeoSnapshot,pub scenario:GeoScenario}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum QueryStatus {Known,Unknown,Conflicting,OutsideCoverage,Unsupported}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueriedLayer {
    pub ige_id:IgeId,pub top_abs_m:f64,pub bottom_abs_m:f64,pub pinched:bool,pub source_ids:Vec<SourceId>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundColumn {
    pub xy_m:[f64;2],pub terrain_abs_m:f64,pub layers:Vec<QueriedLayer>,
    pub triangle_ids:Vec<TriangleId>,pub source_ids:Vec<SourceId>,pub method:InterpolationMethod,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSectionSample {pub station_m:f64,pub result:GeoQueryResult}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSectionResult {pub samples:Vec<GeoSectionSample>,pub borehole_projections:Vec<GeoBoreholeProjection>,pub method:InterpolationMethod}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoBoreholeProjection {pub borehole_id:BoreholeId,pub chainage_m:f64,pub offset_m:f64,pub source_ids:Vec<SourceId>}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedLayerVolume {pub ige_id:IgeId,pub volume_m3:f64,pub source_ids:Vec<SourceId>,pub method:InterpolationMethod}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoQueryResult {
    pub status:QueryStatus,pub column:Option<GroundColumn>,pub section:Option<Box<GeoSectionResult>>,
    pub volumes:Vec<DerivedLayerVolume>,pub reasons:Vec<String>,pub source_ids:Vec<SourceId>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="query",rename_all="SCREAMING_SNAKE_CASE",deny_unknown_fields)]
pub enum GeoQuery {
    ColumnAt{xy_m:[f64;2]},
    Section{polyline_xy_m:Vec<[f64;2]>,max_spacing_m:f64},
    LayerVolumes,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="code",rename_all="SCREAMING_SNAKE_CASE",deny_unknown_fields)]
pub enum GeoError {
    InvalidId{value:String},InvalidInput{path:String,message:String},
    MissingReference{entity:String,id:String},ResourceLimit{reason:String},
    GeometryConflict{reason:String},SnapshotBindingMismatch{reason:String},
    Unsupported{reason:String},Serialization{message:String},
}
impl std::fmt::Display for GeoError{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{self:?}")}}
impl std::error::Error for GeoError{}

