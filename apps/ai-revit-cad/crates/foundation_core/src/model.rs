use serde::{Deserialize, Serialize};
use std::fmt;

pub const STANDARD_GRAVITY_M_S2: f64 = 9.80665;
pub const MAX_MESH_CELLS: usize = 65_536;
pub const MAX_MESH_AXIS: usize = 1_024;
pub const MAX_ACTIVE_SET_ITERATIONS: usize = 128;

macro_rules! decimal_id {
    ($name:ident) => {
        /// Canonical positive decimal string: no sign, whitespace or leading zero.
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl AsRef<str>) -> Result<Self, FoundationError> {
                let value = value.as_ref();
                if value.is_empty()
                    || value.starts_with('0')
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(FoundationError::InvalidInput {
                        field: stringify!($name).into(),
                        reason: "expected a positive canonical decimal string".into(),
                    });
                }
                Ok(Self(value.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::new(text).map_err(serde::de::Error::custom)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

decimal_id!(FoundationId);
decimal_id!(SoilScenarioId);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatGeometry {
    pub width_x_m: f64,
    pub length_y_m: f64,
    pub thickness_m: f64,
    pub concrete_unit_weight_kn_m3: f64,
}

impl Default for MatGeometry {
    fn default() -> Self {
        Self {
            width_x_m: 10.0,
            length_y_m: 12.0,
            thickness_m: 0.3,
            concrete_unit_weight_kn_m3: 25.0,
        }
    }
}

impl MatGeometry {
    pub fn area_m2(&self) -> f64 {
        self.width_x_m * self.length_y_m
    }
    pub fn self_weight_kn(&self) -> f64 {
        self.area_m2() * self.thickness_m * self.concrete_unit_weight_kn_m3
    }
    pub fn ix_m4(&self) -> f64 {
        self.width_x_m * self.length_y_m.powi(3) / 12.0
    }
    pub fn iy_m4(&self) -> f64 {
        self.length_y_m * self.width_x_m.powi(3) / 12.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceType {
    SyntheticAssumption,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticSoilLayer {
    pub name: String,
    pub depth_top_m: f64,
    /// None denotes an unspecified lower geological boundary, not a surveyed
    /// stratum end. The finite display extent below is a separate assumption.
    pub depth_bottom_m: Option<f64>,
    pub visualization_bottom_depth_m: f64,
    /// Finite display thickness. For an open-ended bottom layer this is not a
    /// verified geological thickness and is never used in the spring solution.
    pub thickness_m: f64,
    /// Illustrative metadata only; never used to infer spring stiffness.
    pub young_modulus_kpa: f64,
    pub poisson_ratio: f64,
    pub unit_weight_kn_m3: f64,
    pub effective_cohesion_kpa: f64,
    pub effective_friction_angle_deg: f64,
    pub source_type: SourceType,
    pub engineering_site_verified: bool,
    pub parameter_assumption_note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum SupportField {
    Uniform {
        k_kn_m3: f64,
        gap_m: f64,
    },
    /// Row major, x changes fastest. Every spring receives its own cell area.
    PerCell {
        k_kn_m3: Vec<f64>,
        gap_m: Vec<f64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoilScenario {
    pub id: SoilScenarioId,
    pub name: String,
    pub source_type: SourceType,
    pub engineering_site_verified: bool,
    pub layers: Vec<SyntheticSoilLayer>,
    /// Independent synthetic spring assumption, not a layer-homogenisation result.
    pub support: SupportField,
    pub assumption_note: String,
}

/// Synthetic layer variants for sensitivity studies. None is site data.
pub fn synthetic_scenarios() -> Vec<SoilScenario> {
    fn layer(name: &str, depth: f64, e: f64) -> SyntheticSoilLayer {
        SyntheticSoilLayer { name: name.into(), depth_top_m:0.0,depth_bottom_m:Some(depth),visualization_bottom_depth_m:depth,
            thickness_m: depth, young_modulus_kpa: e,
            poisson_ratio: 0.3, unit_weight_kn_m3: 18.0,
            effective_cohesion_kpa:0.0,effective_friction_angle_deg:30.0,
            source_type: SourceType::SyntheticAssumption, engineering_site_verified: false,
            parameter_assumption_note:"SYNTHETIC_ASSUMPTION: illustrative parameters only; no borehole, site verification, bearing calculation or universal E-to-k conversion.".into() }
    }
    let mut three_layers = vec![
        layer("Synthetic upper layer", 1.5, 8_000.0),
        layer("Synthetic middle layer", 3.5, 18_000.0),
        layer("Synthetic lower sand", 7.0, 30_000.0),
    ];
    for (l, (gamma, c, phi)) in
        three_layers
            .iter_mut()
            .zip([(18.0, 5.0, 22.0), (19.0, 18.0, 24.0), (19.5, 0.0, 32.0)])
    {
        l.unit_weight_kn_m3 = gamma;
        l.effective_cohesion_kpa = c;
        l.effective_friction_angle_deg = phi;
    }
    let variants = [
        (
            "1",
            "Homogeneous synthetic soil",
            vec![layer("Homogeneous", 12.0, 20_000.0)],
            10_000.0,
        ),
        (
            "2",
            "Two synthetic layers",
            vec![layer("Upper", 4.0, 15_000.0), layer("Lower", 8.0, 30_000.0)],
            10_000.0,
        ),
        (
            "3",
            "Three synthetic layers: 0–1.5 m, 1.5–5 m, sand below 5 m",
            three_layers,
            10_000.0,
        ),
        (
            "4",
            "Weak upper synthetic layer",
            vec![
                layer("Weak upper", 3.0, 5_000.0),
                layer("Lower", 9.0, 30_000.0),
            ],
            5_000.0,
        ),
        (
            "5",
            "Weak lower synthetic layer",
            vec![
                layer("Upper", 4.0, 30_000.0),
                layer("Weak lower", 8.0, 5_000.0),
            ],
            5_000.0,
        ),
        (
            "6",
            "Low assumed bedding stiffness",
            vec![layer("Homogeneous", 12.0, 20_000.0)],
            2_500.0,
        ),
        (
            "7",
            "High assumed bedding stiffness",
            vec![layer("Homogeneous", 12.0, 20_000.0)],
            40_000.0,
        ),
    ];
    variants.into_iter().map(|(id, name, mut layers, k)| {
        let last = layers.len()-1;
        let mut top = 0.0;
        for (index,l) in layers.iter_mut().enumerate() {
            l.depth_top_m=top;
            top += l.thickness_m;
            l.visualization_bottom_depth_m=top;
            l.depth_bottom_m=if index == last { None } else { Some(top) };
            if index == last { l.parameter_assumption_note.push_str(" Bottom geological depth is unspecified; the finite visualization extent is an explicit display assumption."); }
        }
        SoilScenario {
        id: SoilScenarioId::new(id).expect("literal scenario ID"), name: name.into(),
        source_type: SourceType::SyntheticAssumption, engineering_site_verified: false, layers,
        support: SupportField::Uniform { k_kn_m3: k, gap_m: 0.0 },
        assumption_note: "SYNTHETIC_ASSUMPTION: layered E values are illustrative metadata. Bedding k is an independently specified sensitivity assumption; no universal E-to-k conversion or site verification.".into(),
        }
    }).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum Groundwater {
    NotAssessed,
    DeclaredAbsent {
        source_id: String,
    },
    /// Recognised input, explicitly refused by the Stage 1 solver.
    WaterTable {
        depth_below_base_m: f64,
        source_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LoadComponentKind {
    SyntheticReferenceMass,
    ProvidedStructuralReaction,
    UserAppliedResultant,
    FoundationSelfWeight,
    Snow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadComponent {
    pub source_id: String,
    pub description: String,
    pub kind: LoadComponentKind,
    pub n_kn: f64,
    pub mx_pressure_knm: f64,
    pub my_pressure_knm: f64,
    pub fx_kn: f64,
    pub fy_kn: f64,
    pub mz_knm: f64,
}

/// Original external force and free moment, in global RH z-up axes.
/// The origin for translation is the centre of the raft base. No distribution
/// of a reference mass into fictional structural reactions is performed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocatedStructuralReaction {
    pub source_id: String,
    pub location_m: [f64; 3],
    pub external_force_kn: [f64; 3],
    pub external_free_moment_knm: [f64; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureLoad {
    pub n_kn: f64,
    pub mx_pressure_knm: f64,
    pub my_pressure_knm: f64,
    pub fx_kn: f64,
    pub fy_kn: f64,
    pub mz_knm: f64,
    pub components: Vec<LoadComponent>,
    pub original_structural_reactions: Vec<LocatedStructuralReaction>,
}

impl PressureLoad {
    /// Creates an explicit manual resultant, with no claim of structural verification.
    pub fn manual(
        n_kn: f64,
        mx_pressure_knm: f64,
        my_pressure_knm: f64,
        source_id: impl Into<String>,
    ) -> Self {
        Self {
            n_kn,
            mx_pressure_knm,
            my_pressure_knm,
            fx_kn: 0.0,
            fy_kn: 0.0,
            mz_knm: 0.0,
            components: vec![LoadComponent {
                source_id: source_id.into(),
                description: "Explicit manual resultant".into(),
                kind: LoadComponentKind::UserAppliedResultant,
                n_kn,
                mx_pressure_knm,
                my_pressure_knm,
                fx_kn: 0.0,
                fy_kn: 0.0,
                mz_knm: 0.0,
            }],
            original_structural_reactions: Vec::new(),
        }
    }
}

/// 300 t is a manual synthetic total-mass reference; it is not an actual
/// structural reaction distribution. 300,000 kg * 9.80665 / 1000 = 2941.995 kN.
pub fn reference_load_300_tonnes() -> PressureLoad {
    let mut load = PressureLoad::manual(
        300.0 * STANDARD_GRAVITY_M_S2,
        0.0,
        0.0,
        "synthetic-reference-300t",
    );
    load.components[0].kind = LoadComponentKind::SyntheticReferenceMass;
    load.components[0].description =
        "Manual synthetic mass reference: 300 t at standard gravity; no reaction distribution"
            .into();
    load
}

/// Translate each supplied original six-component reaction using `M + r x F`.
/// Supplied reactions remain unverified engineering input and are retained intact.
pub fn load_from_structural_reactions(
    reactions: Vec<LocatedStructuralReaction>,
) -> Result<PressureLoad, FoundationError> {
    if reactions.is_empty() {
        return Err(FoundationError::InvalidInput {
            field: "structural_reactions".into(),
            reason: "no supplied reactions".into(),
        });
    }
    let mut load = PressureLoad {
        n_kn: 0.0,
        mx_pressure_knm: 0.0,
        my_pressure_knm: 0.0,
        fx_kn: 0.0,
        fy_kn: 0.0,
        mz_knm: 0.0,
        components: Vec::new(),
        original_structural_reactions: reactions,
    };
    for r in &load.original_structural_reactions {
        if r.source_id.trim().is_empty()
            || r.location_m
                .iter()
                .chain(&r.external_force_kn)
                .chain(&r.external_free_moment_knm)
                .any(|v| !v.is_finite())
        {
            return Err(FoundationError::InvalidInput {
                field: "structural_reaction".into(),
                reason: "source ID and finite six-component reaction/location required".into(),
            });
        }
        let [x, y, z] = r.location_m;
        let [fx, fy, fz] = r.external_force_kn;
        let [mx, my, mz] = r.external_free_moment_knm;
        let c = LoadComponent { source_id: r.source_id.clone(), description: "Supplied reaction translated with r cross F; engineering verification external to this kernel".into(),
            kind: LoadComponentKind::ProvidedStructuralReaction, n_kn: -fz,
            mx_pressure_knm: -(mx + y * fz - z * fy), my_pressure_knm: my + z * fx - x * fz,
            fx_kn: fx, fy_kn: fy, mz_knm: mz + x * fy - y * fx };
        load.n_kn += c.n_kn;
        load.mx_pressure_knm += c.mx_pressure_knm;
        load.my_pressure_knm += c.my_pressure_knm;
        load.fx_kn += c.fx_kn;
        load.fy_kn += c.fy_kn;
        load.mz_knm += c.mz_knm;
        load.components.push(c);
    }
    if ![
        load.n_kn,
        load.mx_pressure_knm,
        load.my_pressure_knm,
        load.fx_kn,
        load.fy_kn,
        load.mz_knm,
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return Err(FoundationError::NonFinite {
            stage: "translated structural reactions".into(),
        });
    }
    Ok(load)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum SelfWeightPolicy {
    /// Only the raft is added, as one named centred load component.
    IncludeOnce {
        source_id: String,
    },
    /// The raft is already part of this specific supplied load component.
    IncludedInComponent {
        source_id: String,
    },
    ExcludedExplicitly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfWeightLedger {
    pub foundation_weight_kn: f64,
    pub foundation_weight_added_kn: f64,
    pub policy: SelfWeightPolicy,
    pub applied_components: Vec<LoadComponent>,
    pub total_n_kn: f64,
    pub total_mx_pressure_knm: f64,
    pub total_my_pressure_knm: f64,
    pub total_fx_kn: f64,
    pub total_fy_kn: f64,
    pub total_mz_knm: f64,
    pub original_structural_reactions: Vec<LocatedStructuralReaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshSpec {
    pub nx: usize,
    pub ny: usize,
}
impl Default for MeshSpec {
    fn default() -> Self {
        Self { nx: 40, ny: 48 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidMatInput {
    pub foundation_id: FoundationId,
    pub geometry: MatGeometry,
    pub load: PressureLoad,
    pub mesh: MeshSpec,
    pub soil: SoilScenario,
    pub groundwater: Groundwater,
    pub self_weight_policy: SelfWeightPolicy,
    pub max_iterations: usize,
}

impl Default for RigidMatInput {
    fn default() -> Self {
        Self {
            foundation_id: FoundationId::new("1").expect("literal ID"),
            geometry: MatGeometry::default(),
            load: reference_load_300_tonnes(),
            mesh: MeshSpec::default(),
            soil: synthetic_scenarios().remove(0),
            groundwater: Groundwater::NotAssessed,
            self_weight_policy: SelfWeightPolicy::ExcludedExplicitly,
            max_iterations: 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshInfo {
    pub nx: usize,
    pub ny: usize,
    pub cell_count: usize,
    pub dx_m: f64,
    pub dy_m: f64,
    pub cell_area_m2: f64,
    pub quadrature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellResult {
    pub index: usize,
    pub x_m: f64,
    pub y_m: f64,
    pub area_m2: f64,
    pub k_kn_m3: f64,
    pub spring_k_kn_m: f64,
    pub gap_m: f64,
    pub plane_settlement_m: f64,
    pub compression_m: f64,
    pub q_kpa: f64,
    pub vertical_force_kn: f64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlPointResult {
    pub label: String,
    pub x_m: f64,
    pub y_m: f64,
    pub settlement_down_m: f64,
    pub displacement_global_z_m: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettlementPlane {
    pub w0_m: f64,
    pub dw_dx: f64,
    pub dw_dy: f64,
    pub rotation_about_x_rad: f64,
    pub rotation_about_y_rad: f64,
    pub tilt_magnitude_rad: f64,
    pub control_points: Vec<ControlPointResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquilibriumResult {
    pub applied_n_kn: f64,
    pub applied_mx_pressure_knm: f64,
    pub applied_my_pressure_knm: f64,
    pub original_external_mx_knm: f64,
    pub original_external_my_knm: f64,
    pub recovered_n_kn: f64,
    pub recovered_mx_pressure_knm: f64,
    pub recovered_my_pressure_knm: f64,
    pub residual_n_kn: f64,
    pub residual_mx_pressure_knm: f64,
    pub residual_my_pressure_knm: f64,
    pub normalized_residual: f64,
    pub maximum_sign_violation_m: f64,
    pub iterations: usize,
    pub scaled_pivot_ratio: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContactState {
    FullSampledContact,
    PartialSampledContact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContactResult {
    pub state: ContactState,
    pub active_cells: usize,
    /// Midpoint-cell area, not an exact continuum contact polygon area.
    pub active_sampled_area_m2: f64,
    pub total_area_m2: f64,
    pub sampled_qmin_kpa: f64,
    pub sampled_qmax_kpa: f64,
    pub resultant_x_m: f64,
    pub resultant_y_m: f64,
    pub sampled_support_hull_x_m: [f64; 2],
    pub sampled_support_hull_y_m: [f64; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CornerPressure {
    pub x_m: f64,
    pub y_m: f64,
    pub q_kpa: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContinuumApplicability {
    HomogeneousFullContactCandidate,
    NotApplicableToNonuniformStiffnessOrGaps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FullContactContinuumComparison {
    pub applicability: ContinuumApplicability,
    /// Unilateral feasibility test of the full-contact analytical candidate.
    /// Negative candidate pressures are never used as physical tensile reactions.
    pub candidate_has_tension: bool,
    pub candidate_qmin_kpa: f64,
    pub candidate_qmax_kpa: f64,
    pub candidate_corner_pressures: Vec<CornerPressure>,
    /// Physical corners evaluated from the *discrete solved plane*, available
    /// only for uniform k/gap. Distinct from both cell samples and exact continuum.
    pub solved_plane_corner_pressures: Option<Vec<CornerPressure>>,
    pub solved_plane_corner_qmin_kpa: Option<f64>,
    pub solved_plane_corner_qmax_kpa: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssessmentStatus {
    UltimateBearingNotVerified,
    PreliminaryEstimate,
    NotImplemented,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizontalActionsStatus {
    pub status: AssessmentStatus,
    pub fx_kn: f64,
    pub fy_kn: f64,
    pub mz_knm: f64,
    pub nonzero_unconsumed_actions: bool,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineeringLimits {
    pub source_type: SourceType,
    pub engineering_site_verified: bool,
    pub ultimate_bearing: AssessmentStatus,
    pub settlement: AssessmentStatus,
    pub reinforcement: AssessmentStatus,
    pub groundwater: AssessmentStatus,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidMatResult {
    pub schema_version: u32,
    pub foundation_id: FoundationId,
    pub soil_scenario_id: SoilScenarioId,
    pub input_sha256: String,
    pub mesh: MeshInfo,
    pub cells: Vec<CellResult>,
    pub plane: SettlementPlane,
    pub equilibrium: EquilibriumResult,
    pub contact: ContactResult,
    pub continuum_full_contact: FullContactContinuumComparison,
    pub self_weight_ledger: SelfWeightLedger,
    pub horizontal_actions_status: HorizontalActionsStatus,
    pub engineering_limits: EngineeringLimits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FoundationError {
    InvalidInput {
        field: String,
        reason: String,
    },
    UnsupportedGroundwater,
    NegativeVerticalLoad {
        n_kn: f64,
    },
    ZeroCompression,
    MeshBudgetExceeded {
        requested: usize,
        maximum: usize,
    },
    ResultantOutsideSupportHull {
        x_m: f64,
        y_m: f64,
    },
    SingularStiffness {
        iteration: usize,
    },
    IllConditionedStiffness {
        iteration: usize,
        scaled_pivot_ratio: f64,
    },
    ActiveSetCycle {
        iterations: usize,
    },
    DidNotConverge {
        iterations: usize,
    },
    NonFinite {
        stage: String,
    },
    NumericalResidual {
        normalized_residual: f64,
    },
    ResultBindingMismatch,
}

impl fmt::Display for FoundationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FoundationError {}
