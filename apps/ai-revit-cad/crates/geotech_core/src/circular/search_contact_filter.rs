//! Conservative contact-X enclosures for search range filters. Nominal
//! PreparedMass contacts identify the supported unique lower-arc topology;
//! they are never widened by a heuristic tolerance and called interval roots.
use super::{
    contact_interval::Interval as I,
    search_meter::{WorkKind, charge},
    types::*,
    validation::{CResult, cancelled},
};
use std::sync::atomic::AtomicBool;

pub(crate) struct ContactEnclosures {
    pub left_x: [f64; 2],
    pub right_x: [f64; 2],
    pub scope: String,
}

fn square(value: I) -> Option<I> { Some(value.mul(value)?.positive_part()) }
fn local(value: f64, origin: f64, margin: f64) -> Option<I> {
    I::point(value)?.sub(I::point(origin)?)?.expand(margin)
}

/// Encloses every possible lower-branch terrain intersection in the declared
/// raw-source geometry envelope. An uncertain discriminant, branch, support,
/// arithmetic result or root grouping returns None, so filters fail closed.
pub(crate) fn enclose_terrain_contacts(
    input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    geometry: &FailureMassGeometry, cancel: &AtomicBool,
) -> CResult<Option<ContactEnclosures>> {
    cancelled(cancel)?;
    let origin = geometry.numerical_quality.local_origin;
    let margin = I::point(geometry.numerical_quality.intersection_tolerance_m)
        .and_then(|v| v.scale(config.contact.geometry_margin_multiplier)).map(|v| v.hi);
    let Some(margin) = margin else { return Ok(None); };
    let Some((cx, cz, radius)) = (|| Some((
        local(input.slip_surface.center.x, origin.x, margin)?,
        local(input.slip_surface.center.z, origin.z, margin)?,
        I::point(input.slip_surface.radius_m)?.expand(margin)?,
    )))() else { return Ok(None); };
    if radius.lo <= 0.0 { return Ok(None); }
    // Only two closed root groups are retained, independent of source size.
    // A third disjoint possible group is unresolved rather than selected away.
    let mut groups: Vec<I> = Vec::with_capacity(2);
    for pair in input.ground.terrain_polyline.windows(2) {
        cancelled(cancel)?;
        charge(WorkKind::GeometryWork, 1)?;
        let Some((ax, az, dx, dz, discriminant, minus_b, twice_a)) = (|| {
            let ax = local(pair[0].x, origin.x, margin)?;
            let az = local(pair[0].z, origin.z, margin)?;
            let bx = local(pair[1].x, origin.x, margin)?;
            let bz = local(pair[1].z, origin.z, margin)?;
            let dx = bx.sub(ax)?;
            let dz = bz.sub(az)?;
            if dx.lo <= 0.0 { return None; }
            let qx = ax.sub(cx)?;
            let qz = az.sub(cz)?;
            let a = square(dx)?.add(square(dz)?)?;
            if a.lo <= 0.0 { return None; }
            let b = qx.mul(dx)?.add(qz.mul(dz)?)?.scale(2.0)?;
            let c = square(qx)?.add(square(qz)?)?.sub(square(radius)?)?;
            let discriminant = square(b)?.sub(a.mul(c)?.scale(4.0)?)?;
            Some((ax, az, dx, dz, discriminant, b.scale(-1.0)?, a.scale(2.0)?))
        })() else { return Ok(None); };
        if discriminant.hi < 0.0 { continue; }
        if discriminant.lo < 0.0 { return Ok(None); }
        let Some(root) = discriminant.sqrt() else { return Ok(None); };
        for sign in [-1.0, 1.0] {
            cancelled(cancel)?;
            charge(WorkKind::GeometryWork, 1)?;
            let Some(t) = minus_b.add(root.scale(sign).unwrap()).and_then(|v| v.div(twice_a)) else {
                return Ok(None);
            };
            let Some(t) = t.intersect(I::new(0.0, 1.0).unwrap()) else { continue; };
            let Some((x, z)) = (|| Some((ax.add(t.mul(dx)?)?, az.add(t.mul(dz)?)?)))() else {
                return Ok(None);
            };
            if z.lo > cz.hi { continue; }
            if z.hi >= cz.lo { return Ok(None); }
            let Some(mut x) = x.add(I::point(origin.x).unwrap()) else { return Ok(None); };
            let mut index = 0;
            while index < groups.len() {
                charge(WorkKind::GeometryWork, 1)?;
                if x.lo <= groups[index].hi && groups[index].lo <= x.hi {
                    x = x.hull(groups.remove(index));
                    index = 0;
                } else { index += 1; }
            }
            if groups.len() >= 2 { return Ok(None); }
            groups.push(x);
        }
    }
    cancelled(cancel)?;
    if groups.len() != 2 { return Ok(None); }
    if groups[0].lo > groups[1].lo { groups.swap(0, 1); }
    if groups[0].hi >= groups[1].lo { return Ok(None); }
    for (group, nominal) in [(groups[0], geometry.circle.left_point.x),
                             (groups[1], geometry.circle.right_point.x)] {
        if nominal < group.lo || nominal > group.hi { return Ok(None); }
    }
    Ok(Some(ContactEnclosures {
        left_x: [groups[0].lo, groups[0].hi], right_x: [groups[1].lo, groups[1].hi],
        scope: "OUTWARD_ALGEBRAIC_TERRAIN_ROOT_X_ENCLOSURES: raw source vertex/centre/radius envelope; conditional on supported unique lower-arc ground topology; unresolved discriminants/branches never pass a range filter".into(),
    }))
}
