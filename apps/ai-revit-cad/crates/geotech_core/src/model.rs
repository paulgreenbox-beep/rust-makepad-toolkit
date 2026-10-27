use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point2 {
    pub x: f64,
    pub z: f64,
}
impl Point2 {
    pub const fn new(x: f64, z: f64) -> Self {
        Self { x, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarSlipSurface {
    pub lower: Point2,
    pub upper: Point2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section2D {
    pub terrain_polyline: Vec<Point2>,
    pub slope_toe: Point2,
    pub crest: Point2,
    pub slip_surface: PlanarSlipSurface,
    pub extrusion_width_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoilMaterial {
    pub name: String,
    pub unit_weight_kn_per_m3: f64,
    pub effective_cohesion_kpa: f64,
    pub effective_friction_angle_deg: f64,
    pub provenance: String,
}
impl SoilMaterial {
    /// Explicit mass-density conversion: gamma = rho * g / 1000 kN/m3.
    pub fn from_density_kg_m3(
        name: String,
        density_kg_m3: f64,
        gravity_m_per_s2: f64,
        effective_cohesion_kpa: f64,
        effective_friction_angle_deg: f64,
        provenance: String,
    ) -> Result<Self, GeotechError> {
        if !density_kg_m3.is_finite()
            || !gravity_m_per_s2.is_finite()
            || density_kg_m3 <= 0.0
            || gravity_m_per_s2 <= 0.0
        {
            return Err(GeotechError::new(
                ErrorCode::InvalidParameter,
                "Density and explicit gravity must be finite and positive",
            ));
        }
        let value = Self {
            name,
            unit_weight_kn_per_m3: density_kg_m3 * gravity_m_per_s2 / 1000.0,
            effective_cohesion_kpa,
            effective_friction_angle_deg,
            provenance: format!(
                "{provenance}; explicit density={density_kg_m3} kg/m3, g={gravity_m_per_s2} m/s2"
            ),
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> Result<(), GeotechError> {
        finite(self.unit_weight_kn_per_m3, "soil.unit_weight_kn_per_m3")?;
        finite(self.effective_cohesion_kpa, "soil.effective_cohesion_kpa")?;
        finite(
            self.effective_friction_angle_deg,
            "soil.effective_friction_angle_deg",
        )?;
        if self.name.trim().is_empty() || self.provenance.trim().is_empty() {
            return Err(GeotechError::new(
                ErrorCode::InvalidParameter,
                "Soil name and parameter provenance are required",
            ));
        }
        if self.unit_weight_kn_per_m3 <= 0.0
            || self.effective_cohesion_kpa < 0.0
            || !(0.0..90.0).contains(&self.effective_friction_angle_deg)
        {
            return Err(GeotechError::new(
                ErrorCode::InvalidParameter,
                "Require gamma > 0 kN/m3, cohesion >= 0 kPa, 0 <= friction angle < 90 deg",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    #[serde(rename = "m")]
    Metre,
    #[serde(rename = "mm")]
    Millimetre,
    #[serde(rename = "ft")]
    Foot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AngleUnit {
    #[serde(rename = "deg")]
    Degrees,
    #[serde(rename = "rad")]
    Radians,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitWeightUnit {
    #[serde(rename = "kN/m3")]
    KilonewtonPerCubicMetre,
    #[serde(rename = "N/m3")]
    NewtonPerCubicMetre,
    #[serde(rename = "kg/m3")]
    KilogramPerCubicMetre,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StressUnit {
    #[serde(rename = "kPa")]
    Kilopascal,
    #[serde(rename = "Pa")]
    Pascal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputUnits {
    pub length: LengthUnit,
    pub angle: AngleUnit,
    pub unit_weight: UnitWeightUnit,
    pub stress: StressUnit,
}
impl Default for InputUnits {
    fn default() -> Self {
        Self {
            length: LengthUnit::Metre,
            angle: AngleUnit::Degrees,
            unit_weight: UnitWeightUnit::KilonewtonPerCubicMetre,
            stress: StressUnit::Kilopascal,
        }
    }
}
impl InputUnits {
    pub(crate) fn validate(self) -> Result<(), GeotechError> {
        if self != Self::default() {
            return Err(GeotechError::new(
                ErrorCode::InvalidUnits,
                "v0.1 requires explicit m, deg, kN/m3 and kPa; convert unsupported units before analysis",
            ));
        }
        Ok(())
    }
}

/// Future extension boundary only. Every non-None pore-pressure model is rejected in v0.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PorePressureContract {
    pub resultant_kn_per_m: f64,
    pub model: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalLoad {
    pub description: String,
    pub force_x_kn_per_m: f64,
    pub force_z_kn_per_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AnalysisConditions {
    pub pore_pressure: Option<PorePressureContract>,
    pub water_table_elevation_m: Option<f64>,
    pub external_loads: Vec<ExternalLoad>,
    pub anchors: bool,
    pub seismic_horizontal_coefficient: f64,
    pub seismic_vertical_coefficient: f64,
    pub lateral_3d_friction: bool,
    pub layered_soil: bool,
}
impl AnalysisConditions {
    pub(crate) fn validate(&self) -> Result<(), GeotechError> {
        finite(
            self.seismic_horizontal_coefficient,
            "conditions.seismic_horizontal_coefficient",
        )?;
        finite(
            self.seismic_vertical_coefficient,
            "conditions.seismic_vertical_coefficient",
        )?;
        if let Some(w) = self.water_table_elevation_m {
            finite(w, "conditions.water_table_elevation_m")?;
        }
        if let Some(p) = &self.pore_pressure {
            finite(
                p.resultant_kn_per_m,
                "conditions.pore_pressure.resultant_kn_per_m",
            )?;
        }
        for p in &self.external_loads {
            finite(p.force_x_kn_per_m, "external_load.force_x_kn_per_m")?;
            finite(p.force_z_kn_per_m, "external_load.force_z_kn_per_m")?;
        }
        if self.pore_pressure.is_some()
            || self.water_table_elevation_m.is_some()
            || !self.external_loads.is_empty()
            || self.anchors
            || self.seismic_horizontal_coefficient != 0.0
            || self.seismic_vertical_coefficient != 0.0
            || self.lateral_3d_friction
            || self.layered_soil
        {
            return Err(GeotechError::new(
                ErrorCode::UnsupportedConditions,
                "Supported: dry homogeneous soil, drained effective parameters, static self-weight only. Water/pore pressure, loads, anchors, seismicity, layers and lateral 3D friction are unsupported",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlopeAnalysisInput {
    pub section: Section2D,
    pub soil: SoilMaterial,
    pub units: InputUnits,
    pub conditions: AnalysisConditions,
    pub required_factor_of_safety: Option<f64>,
    pub standard: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureGeometry {
    /// Counterclockwise ring; the final-to-first edge closes it without a repeated vertex.
    pub polygon: Vec<Point2>,
    pub slip_surface: PlanarSlipSurface,
    pub area_m2: f64,
    pub slip_length_m: f64,
    pub width_m: f64,
    pub volume_m3: f64,
    pub slip_base_area_m2: f64,
    pub slip_angle_deg: f64,
    pub numeric_tolerance_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForcesPerMetre {
    pub weight_kn_per_m: f64,
    pub normal_kn_per_m: f64,
    pub driving_kn_per_m: f64,
    pub cohesion_kn_per_m: f64,
    pub friction_kn_per_m: f64,
    pub resistance_kn_per_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TotalForces {
    pub weight_kn: f64,
    pub normal_kn: f64,
    pub driving_kn: f64,
    pub cohesion_kn: f64,
    pub friction_kn: f64,
    pub resistance_kn: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalculationStatus {
    pub geometry_valid: bool,
    pub calculation_completed: bool,
    pub numerically_verified: bool,
    pub engineering_review_required: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementComparison {
    NotSpecified,
    MeetsSpecifiedValue,
    BelowSpecifiedValue,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlopeAnalysisResult {
    pub method: String,
    pub status: CalculationStatus,
    pub geometry: FailureGeometry,
    pub forces_per_m: ForcesPerMetre,
    pub total_forces: TotalForces,
    pub factor_of_safety: f64,
    pub requirement_comparison: RequirementComparison,
    pub input_fingerprint: String,
    pub source_fingerprint: String,
    pub solver_version: String,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtrudedMesh {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub slip_triangles: Vec<[u32; 3]>,
    pub cross_section_triangles: Vec<[u32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSearchSpec {
    pub start_angle_deg: f64,
    pub end_angle_deg: f64,
    pub coarse_step_deg: f64,
    pub refinement_rounds: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub angle_deg: f64,
    pub result: Option<SlopeAnalysisResult>,
    pub rejection: Option<GeotechError>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateSearchResult {
    pub classification: String,
    pub spec: CandidateSearchSpec,
    pub criterion: String,
    pub tested_surfaces: usize,
    pub candidates: Vec<CandidateRecord>,
    pub minimum: Option<SlopeAnalysisResult>,
    pub minimum_input: Option<SlopeAnalysisInput>,
    pub source_input_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    NonFiniteInput,
    InvalidParameter,
    InvalidUnits,
    UnsupportedConditions,
    UnsupportedTerrain,
    InvalidTerrain,
    SelfIntersection,
    EndpointNotOnTerrain,
    AmbiguousIntersection,
    SlipOutsideGround,
    DegeneratePolygon,
    ClockwisePolygon,
    NoDrivingForce,
    NonPositiveEffectiveNormal,
    NumericOverflow,
    Cancelled,
    InvalidSearch,
    InvalidMesh,
    StaleResult,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeotechError {
    pub code: ErrorCode,
    pub message: String,
    pub diagnostic_geometry: Vec<Point2>,
}
impl GeotechError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostic_geometry: vec![],
        }
    }
    pub(crate) fn at(mut self, points: impl Into<Vec<Point2>>) -> Self {
        self.diagnostic_geometry = points.into();
        self
    }
}
impl fmt::Display for GeotechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for GeotechError {}
pub(crate) fn finite(value: f64, name: &str) -> Result<(), GeotechError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(GeotechError::new(
            ErrorCode::NonFiniteInput,
            format!("{name} must be finite"),
        ))
    }
}
pub(crate) fn positive_output(value: f64, name: &str) -> Result<f64, GeotechError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(GeotechError::new(
            ErrorCode::NumericOverflow,
            format!("{name} is zero, negative, underflowed or non-finite"),
        ))
    }
}
