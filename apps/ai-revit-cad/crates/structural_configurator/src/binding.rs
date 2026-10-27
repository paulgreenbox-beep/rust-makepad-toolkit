//! Parameter-derived semantic checks for a deserialized generated graph.
//! This module builds expected ID/geometry catalogs, not a GeneratedBuilding,
//! and never runs the generator, solver, or a transaction.

use crate::generator::{
    compute_offsets, esc, preflight_section_station_budget, section_prefix, section_stations,
    wall_points, MAX_BIM_ITEMS,
};
use crate::geometry as geom;
use crate::model::*;
use crate::validation::{undeclared_contact_pairs, validate_config};
use std::collections::{BTreeMap, BTreeSet};

const EPS: f64 = 1.0e-8;
const MAX_BINDING_ANCHORS: usize = 512;
const GENERATION_LIMIT: &str =
    "STRUCTURAL_GENERATION_NOT_IMPLEMENTED_FOR_THIS_SHAPE_SYSTEM_OR_ROOF";

fn mismatch(message: impl Into<String>) -> ConfiguratorError {
    ConfiguratorError::GeneratedBindingMismatch {
        message: message.into(),
    }
}
fn sid(value: impl Into<String>) -> Result<SemanticId, ConfiguratorError> {
    SemanticId::new(value).map_err(mismatch)
}
fn near(a: f64, b: f64) -> bool {
    // Archive parameter binding is stricter than solver result tolerances.
    // The scale term covers floating-point roundoff without admitting a
    // millimetre displacement at a large project translation.
    a.is_finite()
        && b.is_finite()
        && (a - b).abs() <= 1.0e-9 + 64.0 * f64::EPSILON * a.abs().max(b.abs())
}
fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| near(a[i], b[i]))
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}
fn dot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn xy_at(bounds: &geom::Bounds, u: f64, v: f64) -> [f64; 2] {
    geom::point_at(bounds, u, v)
}
fn inside(s: &SectionConfig, bounds: &geom::Bounds, u: f64, v: f64) -> bool {
    geom::inside_polygon(xy_at(bounds, u, v), &s.corners)
}
fn world(
    config: &BuildingConfig,
    offsets: &BTreeMap<SemanticId, [f64; 2]>,
    section: &SemanticId,
    xy: [f64; 2],
    z: f64,
) -> [f64; 3] {
    let d = offsets.get(section).copied().unwrap_or([0.0; 2]);
    geom::world_point(&config.transform, [xy[0] + d[0], xy[1] + d[1]], z)
}
fn supported(s: &SectionConfig) -> bool {
    let bounds = geom::bounds(s);
    let shape = geom::is_rectangle(s, &bounds)
        || (s.roof.kind == RoofKind::Flat && geom::is_orthogonal_l(s, &bounds));
    let details = s.roof.unsupported_details.as_ref().is_some_and(|d| {
        !d.corner_tops_abs_m.is_empty()
            || !d.break_lines_m.is_empty()
            || !d.ridge_lines_m.is_empty()
            || !d.facets_m.is_empty()
    });
    shape
        && s.structural_system == StructuralSystem::SteelFrame
        && matches!(
            s.roof.kind,
            RoofKind::Flat | RoofKind::MonoPitch | RoofKind::Gable
        )
        && !details
}

#[derive(Clone)]
struct ExpectedNode {
    point: [f64; 3],
    owners: BTreeSet<SemanticId>,
    restrained: Option<[bool; 6]>,
}
#[derive(Clone)]
struct ExpectedMember {
    start: SemanticId,
    end: SemanticId,
    role: MemberRole,
    owners: BTreeSet<SemanticId>,
    material_id: SemanticId,
    profile_id: SemanticId,
    reference: [f64; 3],
    releases: Option<[[bool; 6]; 2]>,
}
struct ExpectedSurface {
    section_id: SemanticId,
    role: SurfaceRole,
    vertices: Vec<[f64; 3]>,
    panel: Option<PanelSpec>,
}
#[derive(Default)]
struct Catalog {
    nodes: BTreeMap<SemanticId, ExpectedNode>,
    members: BTreeMap<SemanticId, ExpectedMember>,
    member_edges: BTreeMap<(SemanticId, SemanticId), SemanticId>,
    surfaces: BTreeMap<SemanticId, ExpectedSurface>,
    outlines: BTreeMap<SemanticId, SectionOutline>,
    generation_limits: BTreeMap<SemanticId, String>,
    interfaces: Vec<GeneratedInterface>,
    estimated_nodes: usize,
    estimated_members: usize,
}

fn resource(reason: &str) -> ConfiguratorError {
    ConfiguratorError::ResourceLimit {
        reason: reason.into(),
    }
}
fn checked_product(
    a: usize,
    b: usize,
    cap: usize,
    reason: &str,
) -> Result<usize, ConfiguratorError> {
    let product = a.checked_mul(b).ok_or_else(|| resource(reason))?;
    if product > cap {
        Err(resource(reason))
    } else {
        Ok(product)
    }
}
fn checked_add(a: usize, b: usize, cap: usize, reason: &str) -> Result<usize, ConfiguratorError> {
    let total = a.checked_add(b).ok_or_else(|| resource(reason))?;
    if total > cap {
        Err(resource(reason))
    } else {
        Ok(total)
    }
}
fn preflight(
    config: &BuildingConfig,
    generated: &GeneratedBuilding,
) -> Result<(), ConfiguratorError> {
    if config.sections.len() > 64
        || config.interfaces.len() > 256
        || config.materials.len() > 4096
        || config.profiles.len() > 4096
        || config.foundation_assignments.len() > MAX_BIM_ITEMS
    {
        return Err(resource(
            "Binding source entity count exceeds bounded review",
        ));
    }
    if generated.nodes.len() > MAX_BIM_ITEMS
        || generated.members.len() > MAX_BIM_ITEMS
        || generated.surfaces.len() > MAX_BIM_ITEMS
        || generated.outlines.len() > MAX_BIM_ITEMS
        || generated.interfaces.len() > MAX_BIM_ITEMS
        || generated.bim_quantities.steel.len() > MAX_BIM_ITEMS
        || generated.reaction_routing.len() > MAX_BIM_ITEMS
    {
        return Err(resource(
            "Raw generated archive exceeds bounded BIM item count",
        ));
    }
    let mut source_points = 0usize;
    let mut anchors = 0usize;
    for section in &config.sections {
        if section.corners.len() > 256 {
            return Err(resource("Section exceeds bounded corner count"));
        }
        source_points = checked_add(
            source_points,
            section.corners.len(),
            MAX_BIM_ITEMS,
            "Source geometry exceeds bounded point count",
        )?;
        // O(corner count) and arithmetic only: reject a huge supported grid
        // before validation or geometry::stations can allocate its vectors.
        if supported(section) {
            preflight_section_station_budget(section)?;
        }
        for grid in [&section.grid.frame_stations, &section.grid.column_stations] {
            if let GridSpacing::Explicit { stations } = grid {
                if stations.len() > MAX_BIM_ITEMS {
                    return Err(resource(
                        "Explicit grid source exceeds bounded station count",
                    ));
                }
                source_points = checked_add(
                    source_points,
                    stations.len(),
                    MAX_BIM_ITEMS,
                    "Source grid exceeds bounded point count",
                )?;
            }
        }
        if let Some(details) = &section.roof.unsupported_details {
            source_points = checked_add(
                source_points,
                details.corner_tops_abs_m.len(),
                MAX_BIM_ITEMS,
                "Roof details exceed bounded source count",
            )?;
            for polyline in details
                .break_lines_m
                .iter()
                .chain(&details.ridge_lines_m)
                .chain(&details.facets_m)
            {
                source_points = checked_add(
                    source_points,
                    polyline.len(),
                    MAX_BIM_ITEMS,
                    "Roof detail points exceed bounded source count",
                )?;
            }
        }
        for panel in [&section.roof_panels, &section.wall_panels]
            .into_iter()
            .flatten()
        {
            source_points = checked_add(
                source_points,
                panel.requested_features.len(),
                MAX_BIM_ITEMS,
                "Panel feature source exceeds bounded count",
            )?;
        }
    }
    for interface in &config.interfaces {
        anchors = checked_add(
            anchors,
            interface.anchors.len(),
            MAX_BINDING_ANCHORS,
            "Connected anchor catalog exceeds bounded review",
        )?;
    }
    Ok(())
}

fn add_node(
    catalog: &mut Catalog,
    id: SemanticId,
    node: ExpectedNode,
) -> Result<(), ConfiguratorError> {
    if let Some(old) = catalog.nodes.get(&id) {
        if !near3(old.point, node.point)
            || old.owners != node.owners
            || old.restrained != node.restrained
        {
            return Err(mismatch(format!("Expected node identity collision: {id}")));
        }
    } else {
        if catalog.nodes.len() >= MAX_BIM_ITEMS {
            return Err(mismatch("Binding node catalog exceeds 100000 items"));
        }
        catalog.nodes.insert(id, node);
    }
    Ok(())
}
fn edge_key(a: &SemanticId, b: &SemanticId) -> (SemanticId, SemanticId) {
    if a < b {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}
fn rebuild_member_edges(catalog: &mut Catalog) {
    catalog.member_edges.clear();
    for (id, member) in &catalog.members {
        catalog
            .member_edges
            .entry(edge_key(&member.start, &member.end))
            .or_insert_with(|| id.clone());
    }
}
fn member_reference(config: &BuildingConfig, rule: &MemberRule) -> [f64; 3] {
    let (sin, cos) = config.transform.rotation_rad.sin_cos();
    let r = rule.local_y_reference;
    [cos * r[0] - sin * r[1], sin * r[0] + cos * r[1], r[2]]
}
fn add_member(
    catalog: &mut Catalog,
    id: SemanticId,
    member: ExpectedMember,
) -> Result<SemanticId, ConfiguratorError> {
    let key = edge_key(&member.start, &member.end);
    if let Some(old_id) = catalog.member_edges.get(&key).cloned() {
        let old = catalog
            .members
            .get_mut(&old_id)
            .ok_or_else(|| mismatch("Expected member disappeared"))?;
        if old.material_id != member.material_id
            || old.profile_id != member.profile_id
            || old.reference != member.reference
            || old.releases != member.releases
        {
            return Err(mismatch(format!(
                "Conflicting expected physical member: {id}"
            )));
        }
        old.owners.extend(member.owners);
        return Ok(old_id);
    }
    if catalog.members.len() >= MAX_BIM_ITEMS {
        return Err(mismatch("Binding member catalog exceeds 100000 items"));
    }
    if catalog.members.insert(id.clone(), member).is_some() {
        return Err(mismatch(format!(
            "Expected member identity collision: {id}"
        )));
    }
    catalog.member_edges.insert(key, id.clone());
    Ok(id)
}
fn expected_member(
    config: &BuildingConfig,
    start: SemanticId,
    end: SemanticId,
    role: MemberRole,
    owners: BTreeSet<SemanticId>,
    rule: &MemberRule,
) -> ExpectedMember {
    ExpectedMember {
        start,
        end,
        role,
        owners,
        material_id: rule.material_id.clone(),
        profile_id: rule.profile_id.clone(),
        reference: member_reference(config, rule),
        releases: rule.end_releases,
    }
}
fn add_surface(
    catalog: &mut Catalog,
    config: &BuildingConfig,
    offsets: &BTreeMap<SemanticId, [f64; 2]>,
    s: &SectionConfig,
    suffix: &str,
    role: SurfaceRole,
    local: Vec<[f64; 3]>,
) -> Result<(), ConfiguratorError> {
    let vertices: Vec<_> = local
        .into_iter()
        .map(|p| world(config, offsets, &s.id, [p[0], p[1]], p[2]))
        .collect();
    if geom::area3(&vertices) <= EPS {
        return Ok(());
    }
    let id = sid(format!(
        "{}/surface/{suffix}",
        section_prefix(&config.id, &s.id)
    ))?;
    let panel = match role {
        SurfaceRole::RoofFacet => s.roof_panels.clone(),
        SurfaceRole::WallFace => s.wall_panels.clone(),
    };
    if catalog.surfaces.len() >= MAX_BIM_ITEMS {
        return Err(mismatch("Binding surface catalog exceeds 100000 items"));
    }
    if catalog
        .surfaces
        .insert(
            id.clone(),
            ExpectedSurface {
                section_id: s.id.clone(),
                role,
                vertices,
                panel,
            },
        )
        .is_some()
    {
        return Err(mismatch(format!(
            "Expected surface identity collision: {id}"
        )));
    }
    Ok(())
}
fn outline_and_surfaces(
    catalog: &mut Catalog,
    config: &BuildingConfig,
    offsets: &BTreeMap<SemanticId, [f64; 2]>,
    s: &SectionConfig,
) -> Result<(), ConfiguratorError> {
    let polygon: Vec<_> = s.corners.iter().map(|c| c.xy_m).collect();
    let generation_limit: Option<String> = if supported(s) {
        None
    } else {
        Some(GENERATION_LIMIT.into())
    };
    if let Some(reason) = &generation_limit {
        catalog
            .generation_limits
            .insert(s.id.clone(), reason.clone());
    }
    let outline = SectionOutline {
        section_id: s.id.clone(),
        world_corners_m: polygon
            .iter()
            .map(|p| world(config, offsets, &s.id, *p, s.roof.base_abs_m))
            .collect(),
        footprint_area_m2: geom::area2(&polygon).abs(),
        perimeter_m: geom::perimeter2(&polygon),
        generation_limit,
    };
    if catalog.outlines.len() >= MAX_BIM_ITEMS {
        return Err(mismatch("Binding outline catalog exceeds 100000 items"));
    }
    if catalog.outlines.insert(s.id.clone(), outline).is_some() {
        return Err(mismatch("Duplicate expected outline"));
    }
    for (name, vertices) in geom::roof_surfaces(s) {
        add_surface(
            catalog,
            config,
            offsets,
            s,
            &format!("roof/{}", esc(&name)),
            SurfaceRole::RoofFacet,
            vertices,
        )?;
    }
    if matches!(
        s.roof.kind,
        RoofKind::Flat | RoofKind::MonoPitch | RoofKind::Gable
    ) {
        let bounds = geom::bounds(s);
        for (i, a) in s.corners.iter().enumerate() {
            let b = &s.corners[(i + 1) % s.corners.len()];
            add_surface(
                catalog,
                config,
                offsets,
                s,
                &format!("wall/{}/{}", esc(&a.id), esc(&b.id)),
                SurfaceRole::WallFace,
                wall_points(s, &bounds, a.xy_m, b.xy_m),
            )?;
        }
    }
    Ok(())
}

fn node_id(
    config: &BuildingConfig,
    s: &SectionConfig,
    u: &SemanticId,
    v: &SemanticId,
    level: &str,
) -> Result<SemanticId, ConfiguratorError> {
    sid(format!(
        "{}/node/u/{}/v/{}/z/{}",
        section_prefix(&config.id, &s.id),
        esc(u),
        esc(v),
        esc(level)
    ))
}
fn section_graph(
    catalog: &mut Catalog,
    config: &BuildingConfig,
    offsets: &BTreeMap<SemanticId, [f64; 2]>,
    s: &SectionConfig,
) -> Result<(), ConfiguratorError> {
    if !supported(s) {
        return Ok(());
    }
    let bounds = geom::bounds(s);
    let (columns, frames, roof_u) = section_stations(s)?;
    let roof_cells = checked_product(
        roof_u.len(),
        frames.len(),
        MAX_BIM_ITEMS / 2,
        "Roof station product exceeds producer generation budget",
    )?;
    let column_lines = checked_product(
        columns.len(),
        frames.len(),
        MAX_BIM_ITEMS,
        "Column station product exceeds bounded review",
    )?;
    let rise = (s.roof.ridge_abs_m.max(s.roof.eave_abs_m) - s.roof.base_abs_m).max(0.0);
    let levels = (rise / s.grid.wall_girt_pitch_m).ceil();
    if !levels.is_finite() || levels > MAX_BIM_ITEMS as f64 {
        return Err(resource("Vertical level count exceeds bounded review"));
    }
    let levels = levels as usize + 2;
    let column_node_upper = checked_product(
        column_lines,
        levels,
        MAX_BIM_ITEMS,
        "Column node estimate exceeds bounded review",
    )?;
    let node_upper = checked_add(
        roof_cells,
        column_node_upper,
        MAX_BIM_ITEMS,
        "Section node estimate exceeds bounded review",
    )?;
    catalog.estimated_nodes = checked_add(
        catalog.estimated_nodes,
        node_upper,
        MAX_BIM_ITEMS,
        "Global node estimate exceeds bounded review",
    )?;
    let column_members = checked_product(
        column_lines,
        levels.saturating_sub(1),
        MAX_BIM_ITEMS,
        "Column member estimate exceeds bounded review",
    )?;
    let rafters = checked_product(
        frames.len(),
        roof_u.len().saturating_sub(1),
        MAX_BIM_ITEMS,
        "Rafter member estimate exceeds bounded review",
    )?;
    let longitudinal = checked_product(
        roof_u.len(),
        frames.len().saturating_sub(1),
        MAX_BIM_ITEMS,
        "Longitudinal member estimate exceeds bounded review",
    )?;
    let perimeter_pairs = if geom::is_rectangle(s, &bounds) {
        checked_product(
            2,
            columns.len().saturating_sub(1) + frames.len().saturating_sub(1),
            MAX_BIM_ITEMS,
            "Perimeter station estimate exceeds bounded review",
        )?
    } else {
        checked_product(
            6,
            columns.len().max(frames.len()),
            MAX_BIM_ITEMS,
            "L-outline perimeter estimate exceeds bounded review",
        )?
    };
    let girts = checked_product(
        perimeter_pairs,
        levels.saturating_sub(2),
        MAX_BIM_ITEMS,
        "Wall girt estimate exceeds bounded review",
    )?;
    let braces = if s.grid.braces {
        checked_product(
            2,
            frames.len().saturating_sub(1),
            MAX_BIM_ITEMS,
            "Brace estimate exceeds bounded review",
        )?
    } else {
        0
    };
    let mut member_upper = checked_add(
        column_members,
        rafters,
        MAX_BIM_ITEMS,
        "Section member estimate exceeds bounded review",
    )?;
    member_upper = checked_add(
        member_upper,
        longitudinal,
        MAX_BIM_ITEMS,
        "Section member estimate exceeds bounded review",
    )?;
    member_upper = checked_add(
        member_upper,
        girts,
        MAX_BIM_ITEMS,
        "Section member estimate exceeds bounded review",
    )?;
    member_upper = checked_add(
        member_upper,
        braces,
        MAX_BIM_ITEMS,
        "Section member estimate exceeds bounded review",
    )?;
    catalog.estimated_members = checked_add(
        catalog.estimated_members,
        member_upper,
        MAX_BIM_ITEMS,
        "Global member estimate exceeds bounded review",
    )?;
    let mut roof_nodes: BTreeMap<(SemanticId, SemanticId), SemanticId> = BTreeMap::new();
    let mut column_nodes: BTreeMap<(SemanticId, SemanticId), Vec<SemanticId>> = BTreeMap::new();
    let owners = BTreeSet::from([s.id.clone()]);
    for v in &frames {
        for u in &roof_u {
            if !inside(s, &bounds, u.position_m, v.position_m) {
                continue;
            }
            let xy = xy_at(&bounds, u.position_m, v.position_m);
            let id = node_id(config, s, &u.id, &v.id, "roof")?;
            add_node(
                catalog,
                id.clone(),
                ExpectedNode {
                    point: world(
                        config,
                        offsets,
                        &s.id,
                        xy,
                        geom::roof_height(s, &bounds, xy),
                    ),
                    owners: owners.clone(),
                    restrained: None,
                },
            )?;
            roof_nodes.insert((u.id.clone(), v.id.clone()), id);
        }
    }
    for v in &frames {
        for c in &columns {
            let Some(u) = roof_u
                .iter()
                .find(|u| (u.position_m - c.position_m).abs() < EPS)
            else {
                return Err(mismatch(format!(
                    "Column station missing from roof catalog: {}",
                    c.id
                )));
            };
            if !inside(s, &bounds, u.position_m, v.position_m) {
                continue;
            }
            let xy = xy_at(&bounds, u.position_m, v.position_m);
            let height = geom::roof_height(s, &bounds, xy);
            let mut line = Vec::new();
            let base = node_id(config, s, &u.id, &v.id, "base")?;
            add_node(
                catalog,
                base.clone(),
                ExpectedNode {
                    point: world(config, offsets, &s.id, xy, s.roof.base_abs_m),
                    owners: owners.clone(),
                    restrained: Some(s.base_restraints),
                },
            )?;
            line.push(base);
            let count = ((height - s.roof.base_abs_m) / s.grid.wall_girt_pitch_m).ceil() as usize;
            if count > 100_000 {
                return Err(mismatch(
                    "Vertical station catalog exceeds bounded generation",
                ));
            }
            for k in 1..count {
                let z = s.roof.base_abs_m + k as f64 * s.grid.wall_girt_pitch_m;
                if z < height - EPS {
                    let id = node_id(config, s, &u.id, &v.id, &format!("girt/g{k:03}"))?;
                    add_node(
                        catalog,
                        id.clone(),
                        ExpectedNode {
                            point: world(config, offsets, &s.id, xy, z),
                            owners: owners.clone(),
                            restrained: None,
                        },
                    )?;
                    line.push(id);
                }
            }
            let top = roof_nodes
                .get(&(u.id.clone(), v.id.clone()))
                .ok_or_else(|| mismatch("Column roof node absent from parameter catalog"))?
                .clone();
            line.push(top);
            for (k, pair) in line.windows(2).enumerate() {
                let id = sid(format!(
                    "{}/member/column/u/{}/v/{}/segment/{k}",
                    section_prefix(&config.id, &s.id),
                    esc(&u.id),
                    esc(&v.id)
                ))?;
                add_member(
                    catalog,
                    id,
                    expected_member(
                        config,
                        pair[0].clone(),
                        pair[1].clone(),
                        MemberRole::Column,
                        owners.clone(),
                        &s.members.column,
                    ),
                )?;
            }
            column_nodes.insert((u.id.clone(), v.id.clone()), line);
        }
    }
    for v in &frames {
        for pair in roof_u.windows(2) {
            if !inside(
                s,
                &bounds,
                (pair[0].position_m + pair[1].position_m) / 2.0,
                v.position_m,
            ) {
                continue;
            }
            if let (Some(a), Some(z)) = (
                roof_nodes.get(&(pair[0].id.clone(), v.id.clone())),
                roof_nodes.get(&(pair[1].id.clone(), v.id.clone())),
            ) {
                let id = sid(format!(
                    "{}/member/rafter/v/{}/from/{}/to/{}",
                    section_prefix(&config.id, &s.id),
                    esc(&v.id),
                    esc(&pair[0].id),
                    esc(&pair[1].id)
                ))?;
                add_member(
                    catalog,
                    id,
                    expected_member(
                        config,
                        a.clone(),
                        z.clone(),
                        MemberRole::Rafter,
                        owners.clone(),
                        &s.members.rafter,
                    ),
                )?;
            }
        }
    }
    for u in &roof_u {
        for pair in frames.windows(2) {
            if !inside(
                s,
                &bounds,
                u.position_m,
                (pair[0].position_m + pair[1].position_m) / 2.0,
            ) {
                continue;
            }
            if let (Some(a), Some(z)) = (
                roof_nodes.get(&(u.id.clone(), pair[0].id.clone())),
                roof_nodes.get(&(u.id.clone(), pair[1].id.clone())),
            ) {
                let is_column = columns
                    .iter()
                    .any(|c| (c.position_m - u.position_m).abs() < EPS);
                let (role, rule) = if is_column {
                    (MemberRole::Beam, &s.members.beam)
                } else {
                    (MemberRole::Purlin, &s.members.purlin)
                };
                let id = sid(format!(
                    "{}/member/longitudinal/u/{}/from/{}/to/{}",
                    section_prefix(&config.id, &s.id),
                    esc(&u.id),
                    esc(&pair[0].id),
                    esc(&pair[1].id)
                ))?;
                add_member(
                    catalog,
                    id,
                    expected_member(config, a.clone(), z.clone(), role, owners.clone(), rule),
                )?;
            }
        }
    }
    // Wall girt locations are generated only between matching interior levels
    // on adjacent column lines of a declared footprint edge.
    for (i, corner) in s.corners.iter().enumerate() {
        let end = &s.corners[(i + 1) % s.corners.len()];
        let a = corner.xy_m;
        let z = end.xy_m;
        let along_u = dot2([z[0] - a[0], z[1] - a[1]], bounds.u).abs() > EPS;
        let wa = world(config, offsets, &s.id, a, s.roof.base_abs_m);
        let wz = world(config, offsets, &s.id, z, s.roof.base_abs_m);
        let mut lines: Vec<_> = column_nodes
            .iter()
            .filter(|(_, nodes)| {
                let p = catalog.nodes[&nodes[0]].point;
                let cross = (p[0] - wa[0]) * (wz[1] - wa[1]) - (p[1] - wa[1]) * (wz[0] - wa[0]);
                cross.abs() < EPS
                    && (p[0] - wa[0]) * (p[0] - wz[0]) + (p[1] - wa[1]) * (p[1] - wz[1]) <= EPS
            })
            .map(|(key, nodes)| (key.clone(), nodes.clone()))
            .collect();
        lines.sort_by(|(ka, _), (kb, _)| {
            let station = |key: &(SemanticId, SemanticId)| -> f64 {
                if along_u {
                    roof_u
                        .iter()
                        .find(|x| x.id == key.0)
                        .map(|x| x.position_m)
                        .unwrap_or(f64::NAN)
                } else {
                    frames
                        .iter()
                        .find(|x| x.id == key.1)
                        .map(|x| x.position_m)
                        .unwrap_or(f64::NAN)
                }
            };
            station(ka).total_cmp(&station(kb))
        });
        for pair in lines.windows(2) {
            for first in pair[0]
                .1
                .iter()
                .skip(1)
                .take(pair[0].1.len().saturating_sub(2))
            {
                let height = catalog.nodes[first].point[2];
                if let Some(second) = pair[1]
                    .1
                    .iter()
                    .find(|n| (catalog.nodes[*n].point[2] - height).abs() < EPS)
                {
                    let id = sid(format!(
                        "{}/member/girt/edge/{}/from/u/{}/v/{}/to/u/{}/v/{}/{}",
                        section_prefix(&config.id, &s.id),
                        esc(&corner.id),
                        esc(&pair[0].0 .0),
                        esc(&pair[0].0 .1),
                        esc(&pair[1].0 .0),
                        esc(&pair[1].0 .1),
                        esc(first.rsplit("/z/").next().unwrap_or("girt"))
                    ))?;
                    add_member(
                        catalog,
                        id,
                        expected_member(
                            config,
                            first.clone(),
                            second.clone(),
                            MemberRole::WallGirt,
                            owners.clone(),
                            &s.members.wall_girt,
                        ),
                    )?;
                }
            }
        }
    }
    if s.grid.braces {
        for u in [columns.first(), columns.last()].into_iter().flatten() {
            for pair in frames.windows(2) {
                let Some(roof_station) = roof_u
                    .iter()
                    .find(|x| (x.position_m - u.position_m).abs() < EPS)
                else {
                    return Err(mismatch("Brace column station missing"));
                };
                let a = column_nodes.get(&(roof_station.id.clone(), pair[0].id.clone()));
                let z = column_nodes.get(&(roof_station.id.clone(), pair[1].id.clone()));
                if let (Some(a), Some(z)) = (a, z) {
                    let id = sid(format!(
                        "{}/member/brace/u/{}/from/{}/to/{}",
                        section_prefix(&config.id, &s.id),
                        esc(&roof_station.id),
                        esc(&pair[0].id),
                        esc(&pair[1].id)
                    ))?;
                    add_member(
                        catalog,
                        id,
                        expected_member(
                            config,
                            a[0].clone(),
                            z.last()
                                .ok_or_else(|| mismatch("Empty brace column"))?
                                .clone(),
                            MemberRole::Brace,
                            owners.clone(),
                            &s.members.brace,
                        ),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn edge(
    s: &SectionConfig,
    ids: &[SemanticId; 2],
) -> Result<([f64; 2], [f64; 2]), ConfiguratorError> {
    let a = s
        .corners
        .iter()
        .find(|c| c.id.as_str() == ids[0].as_str())
        .ok_or_else(|| mismatch("Missing interface corner A"))?;
    let b = s
        .corners
        .iter()
        .find(|c| c.id.as_str() == ids[1].as_str())
        .ok_or_else(|| mismatch("Missing interface corner B"))?;
    Ok((a.xy_m, b.xy_m))
}
fn connect_catalog(
    catalog: &mut Catalog,
    config: &BuildingConfig,
    offsets: &BTreeMap<SemanticId, [f64; 2]>,
    interface: &InterfaceSpec,
) -> Result<(), ConfiguratorError> {
    let mut result = GeneratedInterface {
        id: interface.id.clone(),
        section_a: interface.section_a.clone(),
        section_b: interface.section_b.clone(),
        status: InterfaceLoadPathStatus::InterfaceLoadPathNotVerified,
        shared_node_ids: vec![],
        shared_member_ids: vec![],
    };
    if matches!(interface.kind, InterfaceKind::IndependentWithJoint { .. }) {
        result.status = InterfaceLoadPathStatus::IndependentWithJoint;
        catalog.interfaces.push(result);
        return Ok(());
    }
    if !matches!(interface.kind, InterfaceKind::StructurallyConnected { .. }) {
        catalog.interfaces.push(result);
        return Ok(());
    }
    let a = config
        .sections
        .iter()
        .find(|s| s.id == interface.section_a)
        .ok_or_else(|| mismatch("Missing connected section A"))?;
    let b = config
        .sections
        .iter()
        .find(|s| s.id == interface.section_b)
        .ok_or_else(|| mismatch("Missing connected section B"))?;
    if !supported(a) || !supported(b) {
        return Err(mismatch(
            "Connected catalog cannot verify unsupported section",
        ));
    }
    let (e0, e1) = edge(a, &interface.edge_a)?;
    let ba = geom::bounds(a);
    let bb = geom::bounds(b);
    let dv = [e1[0] - e0[0], e1[1] - e0[1]];
    if dot2(dv, ba.u).abs() > EPS || dot2(dv, bb.u).abs() > EPS {
        return Err(mismatch(
            "Connected catalog requires full parallel frame edge",
        ));
    }
    for anchor in &interface.anchors {
        let xy = [
            e0[0] + anchor.fraction * dv[0],
            e0[1] + anchor.fraction * dv[1],
        ];
        let wp = world(config, offsets, &a.id, xy, a.roof.base_abs_m);
        let ha = geom::roof_height(a, &ba, xy);
        let hb = geom::roof_height(b, &bb, xy);
        let mut old: Vec<_> = catalog
            .nodes
            .iter()
            .filter(|(_, node)| {
                (node.owners.contains(&a.id) || node.owners.contains(&b.id))
                    && (node.point[0] - wp[0]).abs() < EPS
                    && (node.point[1] - wp[1]).abs() < EPS
            })
            .map(|(id, node)| (id.clone(), node.clone()))
            .collect();
        if old.is_empty() {
            return Err(mismatch(
                "Connected anchor has no parameter-derived column nodes",
            ));
        }
        if old.len() > 8192 {
            return Err(resource(
                "Connected anchor level catalog exceeds bounded review",
            ));
        }
        if old
            .iter()
            .any(|(_, n)| n.owners.iter().any(|o| o != &a.id && o != &b.id))
        {
            return Err(mismatch(
                "Multiple-interface junction has no bounded catalog contract",
            ));
        }
        old.sort_by(|x, y| x.1.point[2].total_cmp(&y.1.point[2]).then(x.0.cmp(&y.0)));
        let mut groups: Vec<Vec<(SemanticId, ExpectedNode)>> = vec![];
        for item in old {
            if groups
                .last()
                .is_some_and(|g| (g[0].1.point[2] - item.1.point[2]).abs() < EPS)
            {
                groups
                    .last_mut()
                    .ok_or_else(|| mismatch("Missing level group"))?
                    .push(item);
            } else {
                groups.push(vec![item]);
            }
        }
        let mut remap = BTreeMap::new();
        let mut levels: Vec<(f64, SemanticId, String)> = Vec::new();
        let prefix = format!(
            "building/{}/interface/{}/anchor/{}",
            esc(&config.id),
            esc(&interface.id),
            esc(&anchor.id)
        );
        for group in groups {
            let height = group[0].1.point[2];
            let tag = if (height - a.roof.base_abs_m).abs() < EPS {
                "base".to_string()
            } else {
                let (id, node) = group
                    .iter()
                    .min_by(|x, y| x.0.cmp(&y.0))
                    .ok_or_else(|| mismatch("Empty connected level"))?;
                let owner = node
                    .owners
                    .iter()
                    .next()
                    .ok_or_else(|| mismatch("Unowned connected node"))?;
                format!(
                    "{}:{}",
                    esc(owner),
                    id.rsplit("/z/").next().unwrap_or("roof")
                )
            };
            let id = sid(format!("{prefix}/node/level/{}", esc(&tag)))?;
            let owners: BTreeSet<_> = [(&a.id, ha), (&b.id, hb)]
                .into_iter()
                .filter(|(_, h)| height <= *h + EPS)
                .map(|(s, _)| s.clone())
                .collect();
            let node = ExpectedNode {
                point: [wp[0], wp[1], height],
                owners,
                restrained: if (height - a.roof.base_abs_m).abs() < EPS {
                    Some(a.base_restraints)
                } else {
                    None
                },
            };
            add_node(catalog, id.clone(), node)?;
            for (old_id, _) in group {
                remap.insert(old_id, id.clone());
            }
            levels.push((height, id.clone(), tag));
            result.shared_node_ids.push(id);
            if result.shared_node_ids.len() > MAX_BIM_ITEMS {
                return Err(resource("Shared node catalog exceeds bounded review"));
            }
        }
        let removed: Vec<_> = catalog
            .members
            .iter()
            .filter(|(_, member)| {
                member.role == MemberRole::Column
                    && remap.contains_key(&member.start)
                    && remap.contains_key(&member.end)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            catalog.members.remove(&id);
        }
        for member in catalog.members.values_mut() {
            if let Some(id) = remap.get(&member.start) {
                member.start = id.clone();
            }
            if let Some(id) = remap.get(&member.end) {
                member.end = id.clone();
            }
        }
        for old_id in remap.keys() {
            catalog.nodes.remove(old_id);
        }
        rebuild_member_edges(catalog);
        for pair in levels.windows(2) {
            let mid = (pair[0].0 + pair[1].0) / 2.0;
            let owners: BTreeSet<_> = [(&a.id, ha), (&b.id, hb)]
                .into_iter()
                .filter(|(_, h)| mid < *h + EPS)
                .map(|(s, _)| s.clone())
                .collect();
            if owners.is_empty() {
                continue;
            }
            let rule = if owners.contains(&a.id) {
                &a.members.column
            } else {
                &b.members.column
            };
            let id = sid(format!(
                "{prefix}/member/column/from/{}/to/{}",
                esc(&pair[0].2),
                esc(&pair[1].2)
            ))?;
            let inserted = add_member(
                catalog,
                id,
                expected_member(
                    config,
                    pair[0].1.clone(),
                    pair[1].1.clone(),
                    MemberRole::Column,
                    owners,
                    rule,
                ),
            )?;
            result.shared_member_ids.push(inserted);
            if result.shared_member_ids.len() > MAX_BIM_ITEMS {
                return Err(resource("Shared member catalog exceeds bounded review"));
            }
        }
    }
    // A single physical edge must not be counted twice when equal-height
    // boundary beams collapse to the same connected endpoints.
    let mut seen: BTreeMap<(SemanticId, SemanticId), SemanticId> = BTreeMap::new();
    let mut duplicates = vec![];
    let all: Vec<_> = catalog
        .members
        .iter()
        .map(|(id, m)| (id.clone(), m.clone()))
        .collect();
    for (id, member) in all {
        let key = if member.start < member.end {
            (member.start.clone(), member.end.clone())
        } else {
            (member.end.clone(), member.start.clone())
        };
        if let Some(existing_id) = seen.get(&key) {
            let existing = catalog
                .members
                .get_mut(existing_id)
                .ok_or_else(|| mismatch("Missing dedup member"))?;
            if existing.material_id != member.material_id
                || existing.profile_id != member.profile_id
                || existing.reference != member.reference
                || existing.releases != member.releases
            {
                return Err(mismatch(
                    "Conflicting physical member properties at connected edge",
                ));
            }
            existing.owners.extend(member.owners);
            duplicates.push(id);
            result.shared_member_ids.push(existing_id.clone());
        } else {
            seen.insert(key, id);
        }
    }
    for id in duplicates {
        catalog.members.remove(&id);
    }
    rebuild_member_edges(catalog);
    for (s, edge_ids) in [(a, &interface.edge_a), (b, &interface.edge_b)] {
        let id = sid(format!(
            "{}/surface/wall/{}/{}",
            section_prefix(&config.id, &s.id),
            esc(&edge_ids[0]),
            esc(&edge_ids[1])
        ))?;
        catalog.surfaces.remove(&id);
    }
    let ha = geom::roof_height(a, &ba, e0);
    let hb = geom::roof_height(b, &bb, e0);
    if (ha - hb).abs() > EPS {
        let taller = if ha > hb { a } else { b };
        let low = ha.min(hb);
        let high = ha.max(hb);
        add_surface(
            catalog,
            config,
            offsets,
            taller,
            &format!("interface/{}/step", esc(&interface.id)),
            SurfaceRole::WallFace,
            vec![
                [e0[0], e0[1], low],
                [e1[0], e1[1], low],
                [e1[0], e1[1], high],
                [e0[0], e0[1], high],
            ],
        )?;
    }
    result.status = InterfaceLoadPathStatus::ExplicitRigidIdealization;
    result.shared_node_ids.sort();
    result.shared_node_ids.dedup();
    result.shared_member_ids.sort();
    result.shared_member_ids.dedup();
    if catalog.interfaces.len() >= MAX_BIM_ITEMS {
        return Err(resource("Interface catalog exceeds bounded review"));
    }
    catalog.interfaces.push(result);
    Ok(())
}

fn same_keys<T, U>(expected: &BTreeMap<SemanticId, T>, actual: &BTreeMap<SemanticId, U>) -> bool {
    expected.keys().eq(actual.keys())
}
fn check_graph(
    config: &BuildingConfig,
    generated: &GeneratedBuilding,
    catalog: &Catalog,
) -> Result<(), ConfiguratorError> {
    if !same_keys(&catalog.nodes, &generated.nodes) {
        return Err(mismatch(
            "Generated node ID set differs from the parameter catalog",
        ));
    }
    if !same_keys(&catalog.members, &generated.members) {
        return Err(mismatch(
            "Generated member ID set differs from the parameter catalog",
        ));
    }
    if !same_keys(&catalog.surfaces, &generated.surfaces) {
        return Err(mismatch(
            "Generated surface ID set differs from the parameter catalog",
        ));
    }
    if !same_keys(&catalog.outlines, &generated.outlines) {
        return Err(mismatch(
            "Generated outline ID set differs from the parameter catalog",
        ));
    }
    if generated.generation_limits != catalog.generation_limits {
        return Err(mismatch(
            "Generation limits differ from supported parameter subset",
        ));
    }
    if generated.interfaces != catalog.interfaces {
        return Err(mismatch(
            "Interface status or shared entity IDs differ from parameter catalog",
        ));
    }
    for (id, expected) in &catalog.nodes {
        let actual = &generated.nodes[id];
        if actual.id != *id
            || !near3(actual.position_m, expected.point)
            || actual.owners != expected.owners
            || actual.restrained != expected.restrained
        {
            return Err(mismatch(format!("Node differs from parameters: {id}")));
        }
    }
    for (id, expected) in &catalog.members {
        let actual = &generated.members[id];
        let a = catalog
            .nodes
            .get(&expected.start)
            .ok_or_else(|| mismatch(format!("Expected member start missing: {id}")))?;
        let b = catalog
            .nodes
            .get(&expected.end)
            .ok_or_else(|| mismatch(format!("Expected member end missing: {id}")))?;
        let length = distance(a.point, b.point);
        if !length.is_finite()
            || length <= EPS
            || actual.id != *id
            || actual.start != expected.start
            || actual.end != expected.end
            || actual.role != expected.role
            || actual.owners != expected.owners
            || actual.material_id != expected.material_id
            || actual.profile_id != expected.profile_id
            || actual.local_y_reference != expected.reference
            || actual.end_releases != expected.releases
            || !near(actual.length_m, length)
            || actual.self_weight_source_id != sid(format!("load/self-weight/{id}"))?
        {
            return Err(mismatch(format!("Member differs from parameters: {id}")));
        }
    }
    for (id, expected) in &catalog.surfaces {
        let actual = &generated.surfaces[id];
        let area = geom::area3(&expected.vertices);
        let mass = expected
            .panel
            .as_ref()
            .and_then(|p| p.mass_kg_m2)
            .map(|kg_m2| kg_m2 * area);
        if actual.id != *id
            || actual.section_id != expected.section_id
            || actual.role != expected.role
            || actual.vertices_m.len() != expected.vertices.len()
            || actual
                .vertices_m
                .iter()
                .zip(&expected.vertices)
                .any(|(a, b)| !near3(*a, *b))
            || !near(actual.area_m2, area)
            || actual.stiffness_enabled
            || actual.panel_spec != expected.panel
            || actual.load_transfer_status != PanelLoadTransferStatus::NotVerified
            || actual.mass_kg.is_some() != mass.is_some()
            || actual.mass_kg.zip(mass).is_some_and(|(a, b)| !near(a, b))
            || actual.load_source_id != sid(format!("load/surface/{id}"))?
        {
            return Err(mismatch(format!("Surface differs from parameters: {id}")));
        }
    }
    for (id, expected) in &catalog.outlines {
        let actual = &generated.outlines[id];
        if actual.section_id != expected.section_id
            || actual.generation_limit != expected.generation_limit
            || actual.world_corners_m.len() != expected.world_corners_m.len()
            || actual
                .world_corners_m
                .iter()
                .zip(&expected.world_corners_m)
                .any(|(a, b)| !near3(*a, *b))
            || !near(actual.footprint_area_m2, expected.footprint_area_m2)
            || !near(actual.perimeter_m, expected.perimeter_m)
        {
            return Err(mismatch(format!("Outline differs from parameters: {id}")));
        }
    }
    let mut materials = config.materials.clone();
    materials.sort_by(|a, b| a.id.cmp(&b.id));
    let mut profiles = config.profiles.clone();
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    if generated.materials != materials || generated.profiles != profiles {
        return Err(mismatch(
            "Material/profile catalog differs from configuration",
        ));
    }
    Ok(())
}

fn check_ledgers_and_routing(
    config: &BuildingConfig,
    generated: &GeneratedBuilding,
) -> Result<(), ConfiguratorError> {
    let mut steel = Vec::new();
    let mut steel_volume = 0.0;
    let mut steel_weight = 0.0;
    for (id, member) in &generated.members {
        let profile = generated
            .profiles
            .iter()
            .find(|p| p.id == member.profile_id)
            .ok_or_else(|| mismatch(format!("Member profile missing: {id}")))?;
        let material = generated
            .materials
            .iter()
            .find(|m| m.id == member.material_id)
            .ok_or_else(|| mismatch(format!("Member material missing: {id}")))?;
        let volume = profile.area_m2 * member.length_m;
        let weight = volume * material.weight_density_kn_m3;
        if !volume.is_finite() || !weight.is_finite() {
            return Err(mismatch(format!("Nonfinite member quantity: {id}")));
        }
        steel.push(SteelQuantity {
            member_id: id.clone(),
            owners: member.owners.clone(),
            volume_m3: volume,
            weight_kn: weight,
            source_id: member.self_weight_source_id.clone(),
        });
        steel_volume += volume;
        steel_weight += weight;
    }
    let ledger = &generated.bim_quantities;
    if ledger.steel.len() != steel.len()
        || ledger.steel.iter().zip(&steel).any(|(a, b)| {
            a.member_id != b.member_id
                || a.owners != b.owners
                || a.source_id != b.source_id
                || !near(a.volume_m3, b.volume_m3)
                || !near(a.weight_kn, b.weight_kn)
        })
        || !near(ledger.steel_volume_m3, steel_volume)
        || !near(ledger.steel_weight_kn, steel_weight)
    {
        return Err(mismatch(
            "Steel quantity ledger differs from unique physical members",
        ));
    }
    let unknown = generated
        .surfaces
        .values()
        .filter(|s| s.mass_kg.is_none())
        .count();
    let mass = if unknown == 0 {
        Some(
            generated
                .surfaces
                .values()
                .filter_map(|s| s.mass_kg)
                .sum::<f64>(),
        )
    } else {
        None
    };
    if ledger.unknown_panel_surface_count != unknown
        || ledger.panel_mass_kg.is_some() != mass.is_some()
        || ledger
            .panel_mass_kg
            .zip(mass)
            .is_some_and(|(a, b)| !near(a, b))
    {
        return Err(mismatch(
            "Panel mass ledger differs from known/unknown surfaces",
        ));
    }
    let load = &generated.analysis_load_ledger;
    if load.scope != config.analysis_load_scope
        || generated.analysis_load_scope != config.analysis_load_scope
        || load.projected_member_ids != generated.members.keys().cloned().collect::<BTreeSet<_>>()
        || !near(load.projected_steel_weight_kn, steel_weight)
        || load.status != "PROPOSED_FRAME_SELF_WEIGHT_ONLY_NOT_SOLVER_RECEIPT"
        || load.excluded_loads
            != vec![
                "PANEL_SELF_WEIGHT_NOT_TRANSFERRED".to_string(),
                "SNOW_NOT_VERIFIED".to_string(),
                "WIND_NOT_VERIFIED".to_string(),
            ]
    {
        return Err(mismatch(
            "Analysis load proposal differs from the bounded frame-self-weight contract",
        ));
    }
    let mut routing = Vec::new();
    for node in generated.nodes.values().filter(|n| n.restrained.is_some()) {
        let mut foundations = BTreeMap::new();
        for assignment in config
            .foundation_assignments
            .iter()
            .filter(|a| a.support_node_id == node.id)
        {
            if !node.owners.contains(&assignment.section_id)
                || foundations
                    .insert(
                        assignment.section_id.clone(),
                        assignment.foundation_id.clone(),
                    )
                    .is_some()
            {
                return Err(mismatch(format!(
                    "Invalid foundation assignment for support {}",
                    node.id
                )));
            }
        }
        routing.push(ReactionRouting {
            support_node_id: node.id.clone(),
            section_owners: node.owners.clone(),
            foundation_by_section: foundations,
            requires_explicit_allocation: node.owners.len() > 1,
        });
    }
    if generated.reaction_routing != routing {
        return Err(mismatch(
            "Reaction routing differs from explicit support ownership",
        ));
    }
    for assignment in &config.foundation_assignments {
        if !routing.iter().any(|r| {
            r.support_node_id == assignment.support_node_id
                && r.section_owners.contains(&assignment.section_id)
        }) {
            return Err(mismatch(format!(
                "Foundation support assignment has no physical node: {}",
                assignment.support_node_id
            )));
        }
    }
    Ok(())
}

fn check_status(
    config: &BuildingConfig,
    generated: &GeneratedBuilding,
    catalog: &Catalog,
) -> Result<(), ConfiguratorError> {
    let mut blocked = Vec::new();
    if !catalog.generation_limits.is_empty() {
        blocked.push("STRUCTURAL_GENERATION_NOT_IMPLEMENTED".to_string());
    }
    if catalog
        .interfaces
        .iter()
        .any(|i| i.status == InterfaceLoadPathStatus::InterfaceLoadPathNotVerified)
    {
        blocked.push("INTERFACE_LOAD_PATH_NOT_VERIFIED".to_string());
    }
    if !config.allow_disconnected_analysis && catalog.outlines.len() > 1 {
        let Some(seed) = catalog.outlines.keys().next() else {
            return Err(mismatch("No section outline"));
        };
        let mut reached = BTreeSet::from([seed.clone()]);
        loop {
            let previous = reached.len();
            for i in &catalog.interfaces {
                if i.status == InterfaceLoadPathStatus::ExplicitRigidIdealization
                    && (reached.contains(&i.section_a) || reached.contains(&i.section_b))
                {
                    reached.insert(i.section_a.clone());
                    reached.insert(i.section_b.clone());
                }
            }
            if reached.len() == previous {
                break;
            }
        }
        if reached.len() != catalog.outlines.len() {
            blocked.push("DISCONNECTED_LOAD_PATHS_NOT_ACKNOWLEDGED".to_string());
        }
    }
    let expected = if generated.nodes.len() > structural_core::MAX_NODES
        || generated.members.len() > structural_core::MAX_MEMBERS
    {
        AnalysisProjectionStatus::AnalysisModelCapacityExceeded {
            nodes: generated.nodes.len(),
            members: generated.members.len(),
            max_nodes: structural_core::MAX_NODES,
            max_members: structural_core::MAX_MEMBERS,
        }
    } else if !blocked.is_empty() {
        AnalysisProjectionStatus::Blocked { reasons: blocked }
    } else {
        AnalysisProjectionStatus::ReadyForBoundedProjection
    };
    if generated.analysis_status != expected
        || generated.allow_disconnected_analysis != config.allow_disconnected_analysis
    {
        return Err(mismatch(
            "Analysis projection status differs from validated scope and capacity",
        ));
    }
    Ok(())
}

/// Check a graph against a parameter-derived catalog. Public SHA-256 fields are
/// checked separately and never treated as execution authenticity evidence.
pub(crate) fn validate_semantics(
    config: &BuildingConfig,
    generated: &GeneratedBuilding,
) -> Result<(), ConfiguratorError> {
    preflight(config, generated)?;
    if validate_config(config)
        .iter()
        .any(|issue| issue.severity == IssueSeverity::Error)
    {
        return Err(mismatch(
            "Cannot bind a graph to invalid configuration parameters",
        ));
    }
    if generated.id != config.id {
        return Err(mismatch("Building identity differs from configuration"));
    }
    let offsets = compute_offsets(config)?;
    let mut catalog = Catalog::default();
    let mut sections: Vec<_> = config.sections.iter().collect();
    sections.sort_by(|a, b| a.id.cmp(&b.id));
    for section in sections {
        outline_and_surfaces(&mut catalog, config, &offsets, section)?;
        section_graph(&mut catalog, config, &offsets, section)?;
    }
    let mut interfaces: Vec<_> = config.interfaces.iter().collect();
    interfaces.sort_by(|a, b| a.id.cmp(&b.id));
    for interface in interfaces {
        connect_catalog(&mut catalog, config, &offsets, interface)?;
    }
    for (a, b) in undeclared_contact_pairs(config)? {
        catalog.interfaces.push(GeneratedInterface {
            id: sid(format!(
                "building/{}/interface/implicit/{}/{}",
                esc(&config.id),
                esc(&a),
                esc(&b)
            ))?,
            section_a: a,
            section_b: b,
            status: InterfaceLoadPathStatus::InterfaceLoadPathNotVerified,
            shared_node_ids: vec![],
            shared_member_ids: vec![],
        });
    }
    check_graph(config, generated, &catalog)?;
    check_ledgers_and_routing(config, generated)?;
    check_status(config, generated, &catalog)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{checked_product, near, MAX_BIM_ITEMS};

    #[test]
    fn large_world_translation_does_not_hide_millimetre_node_change() {
        assert!(near(1_000_000.0, 1_000_000.0));
        assert!(!near(1_000_000.0, 1_000_000.001));
        assert!(!near(f64::NAN, 1_000_000.0));
    }

    #[test]
    fn oversized_station_product_refuses_before_catalog_expansion() {
        assert!(checked_product(1001, 1001, MAX_BIM_ITEMS / 2, "oversized grid").is_err());
    }
}
