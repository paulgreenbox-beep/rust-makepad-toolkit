//! Synthetic source inputs for integration examples. No expected query output or
//! golden result is loaded by this factory.
use crate::{model::*, validate_geo_state};
use std::collections::BTreeMap;

pub const SYNTHETIC_DEMO_MODEL_ID: &str = "GEO-DEMO-1";
pub const SYNTHETIC_DEMO_SCENARIO_ID: &str = "DRY-DEMO-1";

fn known(value: f64, unit: ParameterUnit, source: &SourceId) -> Parameter<f64> {
    Parameter::Known {
        value,
        source_ids: vec![source.clone()],
        metadata: ParameterMetadata {
            unit,
            value_class: ValueClass::Synthetic,
            strength_context: StrengthContext::NotApplicable,
            saturation: SaturationContext::Unspecified,
            confidence_class: ConfidenceClass::Synthetic,
            engineering_verified: false,
            source_page_or_record: None,
            source_date: None,
            source_revision: Some("synthetic-affine-v1".into()),
        },
    }
}

fn synthetic_strength(value: f64, unit: ParameterUnit, source: &SourceId) -> Parameter<f64> {
    let mut result = known(value, unit, source);
    if let Parameter::Known { metadata, .. } = &mut result {
        metadata.strength_context = StrengthContext::EffectiveDrained;
    }
    result
}

fn source_surfaces(x: f64, y: f64) -> [f64; 4] {
    [
        100.0 + 0.1 * x + 0.05 * y,
        98.5 + 0.08 * x + 0.05 * y,
        95.0 + 0.1 * x + 0.04 * y,
        88.0 + 0.1 * x + 0.05 * y,
    ]
}

/// A bounded synthetic dry model with explicit connectivity and three layers.
/// Values are source assumptions, not measured or engineering-verified records.
pub fn synthetic_demo_state() -> Result<GeoState, GeoError> {
    let source_id = SourceId::new("SRC-SYNTHETIC-PLANES")?;
    let mut records = GeoRecords::default();
    records.sources.insert(source_id.clone(), SourceRecord {
        id: source_id.clone(),
        title: "Synthetic affine geology and material assumptions".into(),
        origin: SourceOrigin::Synthetic,
        reference: "Synthetic input definition synthetic-affine-v1; local metres, z up; no field observations".into(),
        content_sha256: None,
        observed_at: None,
        revision: 1,
        source_page_or_record: Some("source planes and material input table".into()),
        source_date: None,
        source_revision: Some("synthetic-affine-v1".into()),
    });

    let materials = [
        ("FILL", "Fill", 18.0, 5.0, 22.0, 8000.0),
        ("LOAM", "Loam", 19.0, 18.0, 24.0, 18000.0),
        ("SAND", "Sand", 19.5, 0.0, 32.0, 30000.0),
    ];
    let mut ige_ids = Vec::new();
    for (suffix, name, gamma, cohesion, phi, modulus) in materials {
        let soil_id = GeoSoilId::new(format!("SOIL-{suffix}"))?;
        let ige_id = IgeId::new(format!("IGE-{suffix}"))?;
        records.soils.insert(soil_id.clone(), GeoSoilRecord {
            id: soil_id.clone(), name: name.into(), revision: 1,
            parameters: SoilParameters {
                unit_weight_kn_m3: known(gamma, ParameterUnit::KnPerM3, &source_id),
                cohesion_kpa: synthetic_strength(cohesion, ParameterUnit::Kpa, &source_id),
                friction_angle_deg: synthetic_strength(phi, ParameterUnit::Degree, &source_id),
                elastic_modulus_kpa: known(modulus, ParameterUnit::Kpa, &source_id),
                poisson_ratio: Parameter::Missing { reason: "NOT_PROVIDED_IN_SYNTHETIC_SOURCE".into() },
            },
        });
        records.iges.insert(ige_id.clone(), IgeRecord {
            id: ige_id.clone(), name: name.into(), soil_id,
            source_ids: vec![source_id.clone()], revision: 1,
        });
        ige_ids.push(ige_id);
    }

    let well_inputs = [
        ("BH-0-0", [0.0, 0.0]),
        ("BH-10-0", [10.0, 0.0]),
        ("BH-10-12", [10.0, 12.0]),
        ("BH-0-12", [0.0, 12.0]),
    ];
    let mut borehole_ids = Vec::new();
    let mut contact_elevations: [BTreeMap<BoreholeId, Parameter<f64>>; 3] =
        std::array::from_fn(|_| BTreeMap::new());
    for (name, [x, y]) in well_inputs {
        let id = BoreholeId::new(name)?;
        let surfaces = source_surfaces(x, y);
        let mut intervals = Vec::new();
        for index in 0..3 {
            intervals.push(BoreholeInterval {
                id: BoreholeIntervalId::new(format!("{name}-L{}", index + 1))?,
                top_depth_m: surfaces[0] - surfaces[index],
                bottom_depth_m: surfaces[0] - surfaces[index + 1],
                ige_id: ige_ids[index].clone(),
                source_ids: vec![source_id.clone()],
            });
            contact_elevations[index].insert(id.clone(), known(surfaces[index + 1], ParameterUnit::Metre, &source_id));
        }
        records.boreholes.insert(id.clone(), Borehole {
            id: id.clone(), collar_m: [x, y, surfaces[0]],
            source_ids: vec![source_id.clone()], intervals, revision: 1,
            total_depth_m: known(surfaces[0] - surfaces[3], ParameterUnit::Metre, &source_id),
            drilling_date: None, crs: Some("SYNTHETIC_LOCAL_METRES".into()),
            collar_vertical_datum: Some("SYNTHETIC_Z_UP".into()),
            depth_datum: "BELOW_COLLAR".into(),
            survey_source_ids: vec![source_id.clone()],
            evidence_source_ids: vec![source_id.clone()],
        });
        borehole_ids.push(id);
    }

    let horizon_names = ["H-FILL-BASE", "H-LOAM-BASE", "H-SURVEY-BASE"];
    let mut contacts = Vec::new();
    for (index, elevations_abs_m) in contact_elevations.into_iter().enumerate() {
        contacts.push(LayerContact {
            id: HorizonId::new(horizon_names[index])?, ige_id: ige_ids[index].clone(),
            elevations_abs_m, source_ids: vec![source_id.clone()],
            vertical_datum: Some("SYNTHETIC_Z_UP".into()),
        });
    }
    let model_id = GroundModelId::new(SYNTHETIC_DEMO_MODEL_ID)?;
    let scenario_id = GroundScenarioId::new(SYNTHETIC_DEMO_SCENARIO_ID)?;
    let section_id = GeoSectionId::new("SECTION-MID-Y")?;
    records.models.insert(model_id.clone(), GroundModel {
        id: model_id.clone(), name: "Synthetic three-layer bounded TIN".into(),
        frame: GeoCoordinateFrame {
            name: "Synthetic local metric frame".into(), crs: Some("SYNTHETIC_LOCAL_METRES".into()),
            vertical_datum: Some("SYNTHETIC_Z_UP".into()),
            missing_borehole_crs: MissingBoreholeCrsPolicy::RequireExplicit,
        },
        borehole_ids: borehole_ids.clone(),
        triangles: vec![
            GeoTriangle { id: TriangleId::new("TRI0")?,
                vertices: [borehole_ids[0].clone(), borehole_ids[1].clone(), borehole_ids[2].clone()],
                source_ids: vec![source_id.clone()] },
            GeoTriangle { id: TriangleId::new("TRI1")?,
                vertices: [borehole_ids[0].clone(), borehole_ids[2].clone(), borehole_ids[3].clone()],
                source_ids: vec![source_id.clone()] },
        ],
        contacts, allow_pinch: false, unknown_zones: vec![],
        interpolation: InterpolationMethod::ExplicitTinPiecewisePlanar,
        source_ids: vec![source_id.clone()], section_ids: vec![section_id.clone()], revision: 1,
    });
    records.scenarios.insert(scenario_id.clone(), GeoScenario {
        id: scenario_id, name: "Explicit synthetic dry assumption".into(), model_id: model_id.clone(),
        water: WaterScenario::ExplicitDry { source_ids: vec![source_id.clone()] },
        source_ids: vec![source_id.clone()], revision: 1,
    });
    records.sections.insert(section_id.clone(), InterpretedSection {
        id: section_id, model_id, polyline_xy_m: vec![[0.0, 6.0], [10.0, 6.0]],
        source_ids: vec![source_id], revision: 1,
    });
    let state = GeoState {
        records,
        head_revision: Some(GeoSourceRevision {
            document_uuid: "00000000-0000-0000-0000-000000000001".into(), operation_id: 1,
        }),
        snapshots: BTreeMap::new(),
    };
    validate_geo_state(&state)?;
    Ok(state)
}
