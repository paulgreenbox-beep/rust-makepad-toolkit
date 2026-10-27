use super::{
    circular_geometry::{PreparedMass, material_areas_cancellable, world},
    ground_model::eval,
    types::*,
    validation::*,
};
use crate::Point2;
use std::sync::atomic::AtomicBool;
use super::search_meter::{self, charge, WorkKind};

pub(crate) fn generate_prepared(
    prepared: &PreparedMass<'_>,
    config: &CircularAnalysisConfig,
    target: usize,
    cancel: &AtomicBool,
) -> CResult<Vec<Slice>> {
    if target == 0 || target > config.limits.max_slices {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Slice target exceeds max_slices",
        ));
    }
    let mut breaks = prepared.breakpoints.clone();
    if (prepared.right.x - prepared.left.x) / target as f64 <= prepared.ground.tolerance {
        return Err(error(
            CircularErrorCode::DegenerateMass,
            "Requested grid has unresolved slice widths",
        ));
    }
    // Mandatory material/terrain roots take precedence over nearby uniform nodes.
    for i in 1..target {
        cancelled(cancel)?;
        charge(WorkKind::GeometryWork, 1)?;
        let x =
            (prepared.right.x - prepared.left.x).mul_add(i as f64 / target as f64, prepared.left.x);
        let index = prepared.breakpoints.partition_point(|v| *v < x);
        if prepared
            .breakpoints
            .get(index)
            .is_some_and(|b| (*b - x).abs() <= prepared.ground.tolerance)
            || index
                .checked_sub(1)
                .and_then(|i| prepared.breakpoints.get(i))
                .is_some_and(|b| (*b - x).abs() <= prepared.ground.tolerance)
        {
            continue;
        }
        if breaks.len() > config.limits.max_slices {
            return Err(error(
                CircularErrorCode::ResourceLimit,
                "Mandatory plus uniform slices exceed max_slices",
            ));
        }
        breaks.push(x);
    }
    search_meter::sort_by(&mut breaks, cancel, f64::total_cmp)?;
    result_budget(
        breaks.len() - 1,
        prepared.ground.source.materials.len(),
        config,
    )?;
    let mut output = Vec::with_capacity(breaks.len() - 1);
    for (slice_id, pair) in breaks.windows(2).enumerate() {
        cancelled(cancel)?;
        charge(WorkKind::SliceRows, 1)?;
        let (left, right) = (pair[0], pair[1]);
        let b = right - left;
        if b <= prepared.ground.tolerance {
            return Err(error(
                CircularErrorCode::DegenerateMass,
                "Slice width is below the geometric/world resolution",
            ));
        }
        let mid = left + b * 0.5;
        let base = prepared.circle.z(mid)?;
        let mut base_region = None;
        for i in 0..prepared.ground.source.soil_regions.len() {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            let upper = eval(&prepared.ground.boundaries[i], mid);
            let lower = eval(&prepared.ground.boundaries[i + 1], mid);
            if (base - upper).abs() <= prepared.ground.tolerance
                || (base - lower).abs() <= prepared.ground.tolerance
            {
                return Err(error(
                    CircularErrorCode::AmbiguousBaseMaterial,
                    "Slice base is not resolvably separated from a material boundary",
                ));
            }
            if base < upper && base > lower {
                if base_region.replace(i).is_some() {
                    return Err(error(
                        CircularErrorCode::AmbiguousBaseMaterial,
                        "Multiple base regions",
                    ));
                }
            }
        }
        let region = base_region.ok_or_else(|| {
            error(
                CircularErrorCode::AmbiguousBaseMaterial,
                "No unique material along the entire slice base",
            )
        })?;
        let (length, dz, _, arc_length) = prepared.circle.element(left, right)?;
        let cos = b / length;
        if cos <= config.tolerances.min_base_cos {
            return Err(error(
                CircularErrorCode::NearVerticalBase,
                "Slice chord is too close to vertical",
            ));
        }
        let sin = -prepared.geometry.circle.surface.slide_direction.sign() * dz / length;
        let angle = sin.atan2(cos);
        let areas = material_areas_cancellable(&prepared.ground, &prepared.circle, &[left, right], config, cancel)?;
        let mut area = Sum::default();
        let mut weight = Sum::default();
        for a in &areas {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            area.add(a.area_m2);
            let material = prepared
                .ground
                .source
                .materials
                .iter()
                .find(|m| m.material_id == a.material_id)
                .unwrap();
            weight.add(a.area_m2 * material.properties.unit_weight_kn_per_m3);
        }
        let area = positive(area.total(), "slice area")?;
        let weight = positive(weight.total(), "slice weight per metre")?;
        output.push(Slice {
            slice_id,
            x_left: left + prepared.ground.origin.x,
            x_right: right + prepared.ground.origin.x,
            horizontal_width_m: b,
            area_by_material: areas,
            total_area_m2: area,
            weight_kn_per_m: weight,
            base_left: world(
                Point2::new(left, prepared.circle.z(left)?),
                prepared.ground.origin,
            ),
            base_right: world(
                Point2::new(right, prepared.circle.z(right)?),
                prepared.ground.origin,
            ),
            base_inclination_rad: angle,
            base_length_m: length,
            arc_length_m: arc_length,
            base_material_id: prepared.ground.material(region).material_id.clone(),
            numerical_quality: prepared.ground.quality(area),
        });
    }
    Ok(output)
}

pub fn generate_slices(
    input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig,
) -> CResult<Vec<Slice>> {
    let prepared = PreparedMass::new(input, config)?;
    generate_prepared(
        &prepared,
        config,
        config.slices.target_slices,
        &AtomicBool::new(false),
    )
}

pub fn slice_mechanics(
    ground: &GroundSection2D,
    slices: &[Slice],
) -> CResult<Vec<SliceMechanicsInput>> {
    slice_mechanics_cancellable(ground, slices, &AtomicBool::new(false))
}

pub(crate) fn slice_mechanics_cancellable(
    ground: &GroundSection2D,
    slices: &[Slice],
    cancel: &AtomicBool,
) -> CResult<Vec<SliceMechanicsInput>> {
    if slices.len() > 65536 || ground.materials.len() > 256 {
        return Err(error(
            CircularErrorCode::ResourceLimit,
            "Mechanics table exceeds bounded limits",
        ));
    }
    let mut output = Vec::with_capacity(slices.len());
    for slice in slices {
        cancelled(cancel)?;
        charge(WorkKind::GeometryWork, 1)?;
        let soil = &ground
            .materials
            .iter()
            .find(|m| m.material_id == slice.base_material_id)
            .ok_or_else(|| {
                error(
                    CircularErrorCode::MissingMaterial,
                    "Slice base material does not exist",
                )
            })?
            .properties;
        soil.validate()
            .map_err(|e| error(CircularErrorCode::InvalidParameter, e.to_string()))?;
        output.push(SliceMechanicsInput {
            horizontal_width_m: slice.horizontal_width_m,
            base_length_m: slice.base_length_m,
            base_inclination_rad: slice.base_inclination_rad,
            weight_kn_per_m: slice.weight_kn_per_m,
            effective_cohesion_kpa: soil.effective_cohesion_kpa,
            effective_friction_angle_rad: soil.effective_friction_angle_deg.to_radians(),
        });
    }
    Ok(output)
}
