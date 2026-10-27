use crate::cache::{CacheKey, CacheUse, RawSectionParts};
use crate::geometry as g;
use crate::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const GENERATOR_VERSION: &str = "structural-configurator-stage1-v1";
const EPS: f64 = 1e-8;
pub(crate) const MAX_BIM_ITEMS: usize = 100_000;

fn sid(value: impl Into<String>) -> Result<SemanticId, ConfiguratorError> {
    SemanticId::new(value).map_err(|reason| ConfiguratorError::ResourceLimit { reason })
}
pub(crate) fn esc(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"_-.@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn digest<T: serde::Serialize>(value: &T) -> Result<String, ConfiguratorError> {
    let bytes = serde_json::to_vec(value).map_err(|e| ConfiguratorError::Serialization {
        message: e.to_string(),
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn canonical_config(config: &BuildingConfig) -> BuildingConfig {
    let mut c = config.clone();
    c.sections.sort_by(|a, b| a.id.cmp(&b.id));
    c.interfaces.sort_by(|a, b| a.id.cmp(&b.id));
    for i in &mut c.interfaces {
        i.anchors.sort_by(|a, b| a.id.cmp(&b.id));
    }
    c.materials.sort_by(|a, b| a.id.cmp(&b.id));
    c.profiles.sort_by(|a, b| a.id.cmp(&b.id));
    c.foundation_assignments.sort_by(|a, b| {
        (&a.section_id, &a.support_node_id, &a.foundation_id).cmp(&(
            &b.section_id,
            &b.support_node_id,
            &b.foundation_id,
        ))
    });
    c
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
pub(crate) fn section_prefix(building: &SemanticId, section: &SemanticId) -> String {
    format!("building/{}/section/{}", esc(building), esc(section))
}
fn empty(config: &BuildingConfig) -> Result<GeneratedBuilding, ConfiguratorError> {
    Ok(output_shell(
        config,
        digest(&canonical_config(config))?,
        true,
    ))
}
fn output_shell(
    config: &BuildingConfig,
    config_fingerprint: String,
    include_catalogs: bool,
) -> GeneratedBuilding {
    GeneratedBuilding {
        id: config.id.clone(),
        generator_version: GENERATOR_VERSION.into(),
        config_fingerprint,
        graph_fingerprint: String::new(),
        nodes: BTreeMap::new(),
        members: BTreeMap::new(),
        surfaces: BTreeMap::new(),
        outlines: BTreeMap::new(),
        interfaces: vec![],
        materials: if include_catalogs {
            config.materials.clone()
        } else {
            vec![]
        },
        profiles: if include_catalogs {
            config.profiles.clone()
        } else {
            vec![]
        },
        reaction_routing: vec![],
        allow_disconnected_analysis: config.allow_disconnected_analysis,
        generation_limits: BTreeMap::new(),
        limitations: vec![
            "SYNTHETIC_OR_MANUAL_PROPERTIES_NOT_OFFICIAL_CATALOG".into(),
            "PANEL_STIFFNESS_DISABLED".into(),
            "ROOF_SNOW_WIND_LOAD_PATH_NOT_VERIFIED".into(),
            "FRAME_SELF_WEIGHT_ONLY_NOT_ENGINEERING_ACCEPTANCE".into(),
        ],
        analysis_status: AnalysisProjectionStatus::Blocked {
            reasons: vec!["NOT_PROJECTED".into()],
        },
        analysis_load_scope: config.analysis_load_scope,
        bim_quantities: BimQuantityLedger {
            steel: vec![],
            steel_volume_m3: 0.0,
            steel_weight_kn: 0.0,
            panel_mass_kg: None,
            unknown_panel_surface_count: 0,
        },
        analysis_load_ledger: AnalysisLoadLedger {
            scope: config.analysis_load_scope,
            projected_member_ids: BTreeSet::new(),
            projected_steel_weight_kn: 0.0,
            excluded_loads: vec![
                "PANEL_SELF_WEIGHT_NOT_TRANSFERRED".into(),
                "SNOW_NOT_VERIFIED".into(),
                "WIND_NOT_VERIFIED".into(),
            ],
            status: "PROPOSED_FRAME_SELF_WEIGHT_ONLY_NOT_SOLVER_RECEIPT".into(),
        },
    }
}
struct Builder<'a> {
    config: &'a BuildingConfig,
    out: GeneratedBuilding,
    offsets: BTreeMap<SemanticId, [f64; 2]>,
}
impl Builder<'_> {
    fn world(&self, section: &SemanticId, xy: [f64; 2], z: f64) -> [f64; 3] {
        let d = self.offsets.get(section).copied().unwrap_or([0.0; 2]);
        g::world_point(&self.config.transform, [xy[0] + d[0], xy[1] + d[1]], z)
    }
    fn node(
        &mut self,
        s: &SectionConfig,
        u: &SemanticId,
        v: &SemanticId,
        level: &str,
        xy: [f64; 2],
        z: f64,
        restrained: Option<[bool; 6]>,
    ) -> Result<SemanticId, ConfiguratorError> {
        let id = sid(format!(
            "{}/node/u/{}/v/{}/z/{}",
            section_prefix(&self.config.id, &s.id),
            esc(u),
            esc(v),
            esc(level)
        ))?;
        let point = self.world(&s.id, xy, z);
        if let Some(old) = self.out.nodes.get(&id) {
            if distance(old.position_m, point) > EPS || old.restrained != restrained {
                return Err(ConfiguratorError::GeneratedBindingMismatch {
                    message: format!("Conflicting generated node key {id}"),
                });
            }
            return Ok(id);
        }
        if self.out.nodes.len() >= MAX_BIM_ITEMS {
            return Err(ConfiguratorError::ResourceLimit {
                reason: "BIM node limit exceeded".into(),
            });
        }
        self.out.nodes.insert(
            id.clone(),
            GeneratedNode {
                id: id.clone(),
                position_m: point,
                owners: BTreeSet::from([s.id.clone()]),
                restrained,
                fingerprint: String::new(),
            },
        );
        Ok(id)
    }
    fn member(
        &mut self,
        id: SemanticId,
        role: MemberRole,
        start: SemanticId,
        end: SemanticId,
        owners: BTreeSet<SemanticId>,
        rule: &MemberRule,
    ) -> Result<SemanticId, ConfiguratorError> {
        let a = self
            .out
            .nodes
            .get(&start)
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "node".into(),
                id: start.to_string(),
            })?
            .position_m;
        let b = self
            .out
            .nodes
            .get(&end)
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "node".into(),
                id: end.to_string(),
            })?
            .position_m;
        let length = distance(a, b);
        if length <= EPS {
            return Err(ConfiguratorError::InvalidConfiguration {
                issues: vec![ValidationIssue {
                    code: "ZERO_GENERATED_MEMBER".into(),
                    path: id.to_string(),
                    message: "Member endpoints coincide".into(),
                    severity: IssueSeverity::Error,
                }],
            });
        }
        let (sn, cs) = self.config.transform.rotation_rad.sin_cos();
        let r = rule.local_y_reference;
        let reference = [cs * r[0] - sn * r[1], sn * r[0] + cs * r[1], r[2]];
        if let Some(old_id) = self
            .out
            .members
            .values()
            .find(|m| (m.start == start && m.end == end) || (m.start == end && m.end == start))
            .map(|m| m.id.clone())
        {
            let old = self.out.members.get_mut(&old_id).unwrap();
            if old.material_id != rule.material_id
                || old.profile_id != rule.profile_id
                || old.local_y_reference != reference
                || old.end_releases != rule.end_releases
            {
                return Err(ConfiguratorError::InterfaceConflict { interface_id:id.to_string(),reason:"Coincident members have incompatible properties/orientation; explicit offset detailing required".into() });
            }
            old.owners.extend(owners);
            return Ok(old_id);
        }
        if self.out.members.contains_key(&id) {
            return Err(ConfiguratorError::GeneratedBindingMismatch {
                message: format!("Conflicting member key {id}"),
            });
        }
        if self.out.members.len() >= MAX_BIM_ITEMS {
            return Err(ConfiguratorError::ResourceLimit {
                reason: "BIM member limit exceeded".into(),
            });
        }
        let source = sid(format!("load/self-weight/{id}"))?;
        self.out.members.insert(
            id.clone(),
            GeneratedMember {
                id: id.clone(),
                start,
                end,
                role,
                owners,
                material_id: rule.material_id.clone(),
                profile_id: rule.profile_id.clone(),
                local_y_reference: reference,
                end_releases: rule.end_releases,
                length_m: length,
                self_weight_source_id: source,
                fingerprint: String::new(),
            },
        );
        Ok(id)
    }
    fn surface(
        &mut self,
        s: &SectionConfig,
        suffix: &str,
        role: SurfaceRole,
        points: Vec<[f64; 3]>,
    ) -> Result<(), ConfiguratorError> {
        let id = sid(format!(
            "{}/surface/{suffix}",
            section_prefix(&self.config.id, &s.id)
        ))?;
        let vertices: Vec<_> = points
            .into_iter()
            .map(|p| self.world(&s.id, [p[0], p[1]], p[2]))
            .collect();
        let area = g::area3(&vertices);
        if area <= EPS {
            return Ok(());
        }
        let spec = match role {
            SurfaceRole::RoofFacet => s.roof_panels.clone(),
            SurfaceRole::WallFace => s.wall_panels.clone(),
        };
        let mass = spec.as_ref().and_then(|p| p.mass_kg_m2).map(|m| m * area);
        let surface = GeneratedSurface {
            id: id.clone(),
            section_id: s.id.clone(),
            role,
            vertices_m: vertices,
            area_m2: area,
            stiffness_enabled: false,
            load_source_id: sid(format!("load/surface/{id}"))?,
            fingerprint: String::new(),
            panel_spec: spec,
            mass_kg: mass,
            load_transfer_status: PanelLoadTransferStatus::NotVerified,
        };
        if let Some(old) = self.out.surfaces.get(&id) {
            if old != &surface {
                return Err(ConfiguratorError::GeneratedBindingMismatch {
                    message: format!("Conflicting surface key {id}"),
                });
            }
        } else {
            self.out.surfaces.insert(id, surface);
        }
        Ok(())
    }
}
fn merge_stations(mut primary: Vec<GridStation>, extra: Vec<GridStation>) -> Vec<GridStation> {
    primary.sort_by(|a, b| a.position_m.total_cmp(&b.position_m));
    let mut extra = extra;
    extra.sort_by(|a, b| a.position_m.total_cmp(&b.position_m));
    let mut accepted: Vec<GridStation> = Vec::new();
    let mut next_primary = 0usize;
    for station in extra {
        while next_primary < primary.len() && primary[next_primary].position_m < station.position_m
        {
            next_primary += 1;
        }
        let touches_next = primary
            .get(next_primary)
            .is_some_and(|p| (p.position_m - station.position_m).abs() < EPS);
        let touches_previous = next_primary
            .checked_sub(1)
            .and_then(|i| primary.get(i))
            .is_some_and(|p| (p.position_m - station.position_m).abs() < EPS);
        let touches_extra = accepted
            .last()
            .is_some_and(|p| (p.position_m - station.position_m).abs() < EPS);
        if !touches_next && !touches_previous && !touches_extra {
            accepted.push(station);
        }
    }
    primary.extend(accepted);
    primary.sort_by(|a, b| a.position_m.total_cmp(&b.position_m));
    primary
}
fn station_count_upper_bound(
    spacing: &GridSpacing,
    extent: f64,
) -> Result<usize, ConfiguratorError> {
    let limit = |reason: &str| ConfiguratorError::ResourceLimit {
        reason: reason.into(),
    };
    match spacing {
        GridSpacing::Explicit { stations } => {
            if stations.len() > MAX_BIM_ITEMS {
                return Err(limit("Source station count exceeds generation budget"));
            }
            Ok(stations.len())
        }
        GridSpacing::Uniform { pitch_m } => {
            if !extent.is_finite() || extent <= 0.0 || !pitch_m.is_finite() || *pitch_m <= 0.0 {
                return Err(limit("Cannot bound invalid uniform station spacing"));
            }
            let count = (extent / *pitch_m).ceil();
            if !count.is_finite() || count >= (MAX_BIM_ITEMS as f64) {
                return Err(limit("Uniform station count exceeds generation budget"));
            }
            (count as usize)
                .checked_add(1)
                .ok_or_else(|| limit("Station count overflow"))
        }
    }
}
/// O(1) uniform counts and O(corner count) bounds; no station-vector allocation.
/// Return conservative (column, frame, roof-u) counts for global preflight reuse.
pub(crate) fn preflight_section_station_budget(
    s: &SectionConfig,
) -> Result<(usize, usize, usize), ConfiguratorError> {
    let limit = |reason: &str| ConfiguratorError::ResourceLimit {
        reason: reason.into(),
    };
    if s.corners.len() > MAX_BIM_ITEMS {
        return Err(limit("Source corner count exceeds generation budget"));
    }
    let b = g::bounds(s);
    let width = b.u_max - b.u_min;
    let depth = b.v_max - b.v_min;
    let extra = if g::is_rectangle(s, &b) {
        0
    } else {
        s.corners.len()
    };
    let columns = station_count_upper_bound(&s.grid.column_stations, width)?
        .checked_add(extra)
        .ok_or_else(|| limit("Column station count overflow"))?;
    let frames = station_count_upper_bound(&s.grid.frame_stations, depth)?
        .checked_add(extra)
        .ok_or_else(|| limit("Frame station count overflow"))?;
    let purlins = station_count_upper_bound(
        &GridSpacing::Uniform {
            pitch_m: s.grid.purlin_pitch_m,
        },
        width,
    )?;
    let roof = columns
        .checked_add(purlins)
        .and_then(|n| n.checked_add(usize::from(s.roof.kind == RoofKind::Gable)))
        .ok_or_else(|| limit("Roof station count overflow"))?;
    let product = roof
        .checked_mul(frames)
        .ok_or_else(|| limit("Station product overflow"))?;
    if product > MAX_BIM_ITEMS / 2 {
        return Err(limit(
            "Pre-merge station product exceeds bounded generation budget",
        ));
    }
    Ok((columns, frames, roof))
}
pub(crate) fn section_stations(
    s: &SectionConfig,
) -> Result<(Vec<GridStation>, Vec<GridStation>, Vec<GridStation>), ConfiguratorError> {
    preflight_section_station_budget(s)?;
    let bound = g::bounds(s);
    let width = bound.u_max - bound.u_min;
    let depth = bound.v_max - bound.v_min;
    let namespaced = |source: &GridSpacing,
                      extent: f64,
                      prefix: &str|
     -> Result<Vec<GridStation>, ConfiguratorError> {
        g::stations(source, extent)?
            .into_iter()
            .map(|x| {
                Ok(GridStation {
                    id: sid(format!("{prefix}/{}", esc(&x.id)))?,
                    position_m: x.position_m,
                })
            })
            .collect()
    };
    let mut columns = namespaced(&s.grid.column_stations, width, "column")?;
    let mut frames = namespaced(&s.grid.frame_stations, depth, "frame")?;
    if !g::is_rectangle(s, &bound) {
        let mut cu = vec![];
        let mut cv = vec![];
        for c in &s.corners {
            cu.push(GridStation {
                id: sid(format!("corner/{}", esc(&c.id)))?,
                position_m: c.xy_m[0] * bound.u[0] + c.xy_m[1] * bound.u[1] - bound.u_min,
            });
            cv.push(GridStation {
                id: sid(format!("corner/{}", esc(&c.id)))?,
                position_m: c.xy_m[0] * bound.v[0] + c.xy_m[1] * bound.v[1] - bound.v_min,
            });
        }
        columns = merge_stations(columns, cu);
        frames = merge_stations(frames, cv);
    }
    let purlins = namespaced(
        &GridSpacing::Uniform {
            pitch_m: s.grid.purlin_pitch_m,
        },
        width,
        "purlin",
    )?;
    let mut roof_u = merge_stations(columns.clone(), purlins);
    if s.roof.kind == RoofKind::Gable {
        roof_u = merge_stations(
            roof_u,
            vec![GridStation {
                id: sid("ridge")?,
                position_m: width / 2.0,
            }],
        );
    }
    Ok((columns, frames, roof_u))
}
pub(crate) fn wall_points(
    s: &SectionConfig,
    b: &g::Bounds,
    a: [f64; 2],
    z: [f64; 2],
) -> Vec<[f64; 3]> {
    let mut p = vec![
        [a[0], a[1], s.roof.base_abs_m],
        [z[0], z[1], s.roof.base_abs_m],
        [z[0], z[1], g::roof_height(s, b, z)],
    ];
    if s.roof.kind == RoofKind::Gable {
        let au = a[0] * b.u[0] + a[1] * b.u[1];
        let zu = z[0] * b.u[0] + z[1] * b.u[1];
        let mid = (b.u_min + b.u_max) * 0.5;
        if (zu - au).abs() > EPS {
            let t = (mid - au) / (zu - au);
            if t > EPS && t < 1.0 - EPS {
                let xy = [a[0] + t * (z[0] - a[0]), a[1] + t * (z[1] - a[1])];
                p.push([xy[0], xy[1], g::roof_height(s, b, xy)]);
            }
        }
    }
    p.push([a[0], a[1], g::roof_height(s, b, a)]);
    p
}
fn section_generate(
    b: &mut Builder<'_>,
    s: &SectionConfig,
    diagnostics: &mut GenerationDiagnostics,
) -> Result<(), ConfiguratorError> {
    // The counter is at the actual raw producer entry, including outline-only
    // sections. A cache hit does not enter this function.
    diagnostics.raw_section_generation_calls = diagnostics
        .raw_section_generation_calls
        .checked_add(1)
        .ok_or_else(|| ConfiguratorError::ResourceLimit {
            reason: "Raw producer call counter overflow".into(),
        })?;
    diagnostics.raw_sections_recomputed.insert(s.id.clone());
    let xy: Vec<_> = s.corners.iter().map(|c| c.xy_m).collect();
    let bound = g::bounds(s);
    let valid_shape = g::is_rectangle(s, &bound)
        || (s.roof.kind == RoofKind::Flat && g::is_orthogonal_l(s, &bound));
    let details = s.roof.unsupported_details.as_ref().is_some_and(|d| {
        !d.corner_tops_abs_m.is_empty()
            || !d.break_lines_m.is_empty()
            || !d.ridge_lines_m.is_empty()
            || !d.facets_m.is_empty()
    });
    let supported = valid_shape
        && s.structural_system == StructuralSystem::SteelFrame
        && matches!(
            s.roof.kind,
            RoofKind::Flat | RoofKind::MonoPitch | RoofKind::Gable
        )
        && !details;
    let limit: Option<String> = if supported {
        None
    } else {
        Some("STRUCTURAL_GENERATION_NOT_IMPLEMENTED_FOR_THIS_SHAPE_SYSTEM_OR_ROOF".into())
    };
    if let Some(reason) = &limit {
        b.out.generation_limits.insert(s.id.clone(), reason.clone());
    }
    let outline = SectionOutline {
        section_id: s.id.clone(),
        world_corners_m: xy
            .iter()
            .map(|p| b.world(&s.id, *p, s.roof.base_abs_m))
            .collect(),
        footprint_area_m2: g::area2(&xy).abs(),
        perimeter_m: g::perimeter2(&xy),
        generation_limit: limit,
    };
    if b.out.outlines.insert(s.id.clone(), outline).is_some() {
        return Err(ConfiguratorError::GeneratedBindingMismatch {
            message: "Duplicate section outline".into(),
        });
    }
    for (id, points) in g::roof_surfaces(s) {
        b.surface(
            s,
            &format!("roof/{}", esc(&id)),
            SurfaceRole::RoofFacet,
            points,
        )?;
    }
    if matches!(
        s.roof.kind,
        RoofKind::Flat | RoofKind::MonoPitch | RoofKind::Gable
    ) {
        for (i, a) in s.corners.iter().enumerate() {
            let z = &s.corners[(i + 1) % s.corners.len()];
            b.surface(
                s,
                &format!("wall/{}/{}", esc(&a.id), esc(&z.id)),
                SurfaceRole::WallFace,
                wall_points(s, &bound, a.xy_m, z.xy_m),
            )?;
        }
    }
    if !supported {
        return Ok(());
    }
    let (columns, frames, roof_u) = section_stations(s)?;
    if roof_u.len().saturating_mul(frames.len()) > MAX_BIM_ITEMS / 2 {
        return Err(ConfiguratorError::ResourceLimit {
            reason: "BIM station grid exceeds bounded generation budget".into(),
        });
    }
    let mut roof_nodes: BTreeMap<(SemanticId, SemanticId), SemanticId> = BTreeMap::new();
    let mut column_nodes: BTreeMap<(SemanticId, SemanticId), Vec<SemanticId>> = BTreeMap::new();
    for v in &frames {
        for u in &roof_u {
            let p = g::point_at(&bound, u.position_m, v.position_m);
            if !g::inside_polygon(p, &s.corners) {
                continue;
            }
            let h = g::roof_height(s, &bound, p);
            let n = b.node(s, &u.id, &v.id, "roof", p, h, None)?;
            roof_nodes.insert((u.id.clone(), v.id.clone()), n);
        }
    }
    for v in &frames {
        for cu in &columns {
            let u = roof_u
                .iter()
                .find(|u| (u.position_m - cu.position_m).abs() < EPS)
                .unwrap();
            let p = g::point_at(&bound, u.position_m, v.position_m);
            if !g::inside_polygon(p, &s.corners) {
                continue;
            }
            let h = g::roof_height(s, &bound, p);
            let mut line = vec![b.node(
                s,
                &u.id,
                &v.id,
                "base",
                p,
                s.roof.base_abs_m,
                Some(s.base_restraints),
            )?];
            let count = ((h - s.roof.base_abs_m) / s.grid.wall_girt_pitch_m).ceil() as usize;
            if count > MAX_BIM_ITEMS {
                return Err(ConfiguratorError::ResourceLimit {
                    reason: "Vertical grid exceeds budget".into(),
                });
            }
            for k in 1..count {
                let z = s.roof.base_abs_m + k as f64 * s.grid.wall_girt_pitch_m;
                if z < h - EPS {
                    line.push(b.node(s, &u.id, &v.id, &format!("girt/g{k:03}"), p, z, None)?);
                }
            }
            line.push(roof_nodes[&(u.id.clone(), v.id.clone())].clone());
            for (k, pair) in line.windows(2).enumerate() {
                b.member(
                    sid(format!(
                        "{}/member/column/u/{}/v/{}/segment/{k}",
                        section_prefix(&b.config.id, &s.id),
                        esc(&u.id),
                        esc(&v.id)
                    ))?,
                    MemberRole::Column,
                    pair[0].clone(),
                    pair[1].clone(),
                    BTreeSet::from([s.id.clone()]),
                    &s.members.column,
                )?;
            }
            column_nodes.insert((u.id.clone(), v.id.clone()), line);
        }
    }
    for v in &frames {
        for pair in roof_u.windows(2) {
            let p = g::point_at(
                &bound,
                (pair[0].position_m + pair[1].position_m) / 2.0,
                v.position_m,
            );
            if !g::inside_polygon(p, &s.corners) {
                continue;
            }
            if let (Some(a), Some(z)) = (
                roof_nodes.get(&(pair[0].id.clone(), v.id.clone())),
                roof_nodes.get(&(pair[1].id.clone(), v.id.clone())),
            ) {
                b.member(
                    sid(format!(
                        "{}/member/rafter/v/{}/from/{}/to/{}",
                        section_prefix(&b.config.id, &s.id),
                        esc(&v.id),
                        esc(&pair[0].id),
                        esc(&pair[1].id)
                    ))?,
                    MemberRole::Rafter,
                    a.clone(),
                    z.clone(),
                    BTreeSet::from([s.id.clone()]),
                    &s.members.rafter,
                )?;
            }
        }
    }
    for u in &roof_u {
        for pair in frames.windows(2) {
            let p = g::point_at(
                &bound,
                u.position_m,
                (pair[0].position_m + pair[1].position_m) / 2.0,
            );
            if !g::inside_polygon(p, &s.corners) {
                continue;
            }
            if let (Some(a), Some(z)) = (
                roof_nodes.get(&(u.id.clone(), pair[0].id.clone())),
                roof_nodes.get(&(u.id.clone(), pair[1].id.clone())),
            ) {
                let is_col = columns
                    .iter()
                    .any(|c| (c.position_m - u.position_m).abs() < EPS);
                let (role, rule) = if is_col {
                    (MemberRole::Beam, &s.members.beam)
                } else {
                    (MemberRole::Purlin, &s.members.purlin)
                };
                b.member(
                    sid(format!(
                        "{}/member/longitudinal/u/{}/from/{}/to/{}",
                        section_prefix(&b.config.id, &s.id),
                        esc(&u.id),
                        esc(&pair[0].id),
                        esc(&pair[1].id)
                    ))?,
                    role,
                    a.clone(),
                    z.clone(),
                    BTreeSet::from([s.id.clone()]),
                    rule,
                )?;
            }
        }
    }
    // Wall girts join explicit column grid lines only, not arbitrary coordinate coincidences.
    for (i, edge_a) in s.corners.iter().enumerate() {
        let edge_b = &s.corners[(i + 1) % s.corners.len()];
        let a = edge_a.xy_m;
        let z = edge_b.xy_m;
        let du = (z[0] - a[0]) * bound.u[0] + (z[1] - a[1]) * bound.u[1];
        let along_u = du.abs() > EPS;
        let lines: Vec<_> = column_nodes
            .iter()
            .filter(|(_, ns)| {
                let p = b.out.nodes[&ns[0]].position_m;
                let wa = b.world(&s.id, a, s.roof.base_abs_m);
                let wz = b.world(&s.id, z, s.roof.base_abs_m);
                let cross = (p[0] - wa[0]) * (wz[1] - wa[1]) - (p[1] - wa[1]) * (wz[0] - wa[0]);
                cross.abs() < EPS
                    && (p[0] - wa[0]) * (p[0] - wz[0]) + (p[1] - wa[1]) * (p[1] - wz[1]) <= EPS
            })
            .map(|(k, n)| (k.clone(), n.clone()))
            .collect();
        let mut lines = lines;
        lines.sort_by(|(ka, _), (kb, _)| {
            let va = if along_u {
                roof_u.iter().find(|s| s.id == ka.0).unwrap().position_m
            } else {
                frames.iter().find(|s| s.id == ka.1).unwrap().position_m
            };
            let vb = if along_u {
                roof_u.iter().find(|s| s.id == kb.0).unwrap().position_m
            } else {
                frames.iter().find(|s| s.id == kb.1).unwrap().position_m
            };
            va.total_cmp(&vb)
        });
        for pair in lines.windows(2) {
            let na = &pair[0].1;
            let nb = &pair[1].1;
            for a in na.iter().skip(1).take(na.len().saturating_sub(2)) {
                let za = b.out.nodes[a].position_m[2];
                if let Some(z) = nb
                    .iter()
                    .find(|n| (b.out.nodes[*n].position_m[2] - za).abs() < EPS)
                {
                    b.member(
                        sid(format!(
                            "{}/member/girt/edge/{}/from/u/{}/v/{}/to/u/{}/v/{}/{}",
                            section_prefix(&b.config.id, &s.id),
                            esc(&edge_a.id),
                            esc(&pair[0].0.0),
                            esc(&pair[0].0.1),
                            esc(&pair[1].0.0),
                            esc(&pair[1].0.1),
                            esc(a.rsplit("/z/").next().unwrap_or("girt"))
                        ))?,
                        MemberRole::WallGirt,
                        a.clone(),
                        z.clone(),
                        BTreeSet::from([s.id.clone()]),
                        &s.members.wall_girt,
                    )?;
                }
            }
        }
    }
    if s.grid.braces {
        for u in [columns.first(), columns.last()].into_iter().flatten() {
            for pair in frames.windows(2) {
                let uc = &roof_u
                    .iter()
                    .find(|x| (x.position_m - u.position_m).abs() < EPS)
                    .unwrap()
                    .id;
                if let (Some(a), Some(z)) = (
                    column_nodes.get(&(uc.clone(), pair[0].id.clone())),
                    column_nodes.get(&(uc.clone(), pair[1].id.clone())),
                ) {
                    b.member(
                        sid(format!(
                            "{}/member/brace/u/{}/from/{}/to/{}",
                            section_prefix(&b.config.id, &s.id),
                            esc(uc),
                            esc(&pair[0].id),
                            esc(&pair[1].id)
                        ))?,
                        MemberRole::Brace,
                        a[0].clone(),
                        z.last().unwrap().clone(),
                        BTreeSet::from([s.id.clone()]),
                        &s.members.brace,
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
    let lookup = |id: &SemanticId| {
        s.corners
            .iter()
            .find(|c| &c.id == id)
            .map(|c| c.xy_m)
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "corner".into(),
                id: id.to_string(),
            })
    };
    Ok((lookup(&ids[0])?, lookup(&ids[1])?))
}
fn connect(
    b: &mut Builder<'_>,
    i: &InterfaceSpec,
) -> Result<GeneratedInterface, ConfiguratorError> {
    let mut result = GeneratedInterface {
        id: i.id.clone(),
        section_a: i.section_a.clone(),
        section_b: i.section_b.clone(),
        status: InterfaceLoadPathStatus::InterfaceLoadPathNotVerified,
        shared_node_ids: vec![],
        shared_member_ids: vec![],
    };
    if matches!(i.kind, InterfaceKind::IndependentWithJoint { .. }) {
        result.status = InterfaceLoadPathStatus::IndependentWithJoint;
        return Ok(result);
    }
    if !matches!(i.kind, InterfaceKind::StructurallyConnected { .. }) {
        return Ok(result);
    }
    let a = b
        .config
        .sections
        .iter()
        .find(|s| s.id == i.section_a)
        .unwrap()
        .clone();
    let z = b
        .config
        .sections
        .iter()
        .find(|s| s.id == i.section_b)
        .unwrap()
        .clone();
    let da = b.offsets.get(&a.id).copied().unwrap_or([0.0; 2]);
    let dz = b.offsets.get(&z.id).copied().unwrap_or([0.0; 2]);
    let conflict = |reason: &str| ConfiguratorError::InterfaceConflict {
        interface_id: i.id.to_string(),
        reason: reason.into(),
    };
    if (da[0] - dz[0]).abs() > EPS || (da[1] - dz[1]).abs() > EPS {
        return Err(conflict(
            "Connected sections have different explicit joint displacements",
        ));
    }
    if b.out.generation_limits.contains_key(&a.id) || b.out.generation_limits.contains_key(&z.id) {
        return Err(conflict(
            "Connected structural generation is unsupported for this section subset",
        ));
    }
    let (e0, e1) = edge(&a, &i.edge_a)?;
    let ba = g::bounds(&a);
    let bz = g::bounds(&z);
    let dv = [e1[0] - e0[0], e1[1] - e0[1]];
    if (dv[0] * ba.u[0] + dv[1] * ba.u[1]).abs() > EPS
        || (dv[0] * bz.u[0] + dv[1] * bz.u[1]).abs() > EPS
    {
        return Err(conflict(
            "Only shared edges parallel to both frame station directions are implemented",
        ));
    }
    for anchor in &i.anchors {
        let xy = [
            e0[0] + anchor.fraction * dv[0],
            e0[1] + anchor.fraction * dv[1],
        ];
        let wp = b.world(&a.id, xy, a.roof.base_abs_m);
        let ha = g::roof_height(&a, &ba, xy);
        let hz = g::roof_height(&z, &bz, xy);
        let mut old: Vec<_> = b
            .out
            .nodes
            .values()
            .filter(|n| {
                (n.owners.contains(&a.id) || n.owners.contains(&z.id))
                    && (n.position_m[0] - wp[0]).abs() < EPS
                    && (n.position_m[1] - wp[1]).abs() < EPS
            })
            .cloned()
            .collect();
        if old.is_empty() {
            return Err(conflict(
                "Interface anchor does not identify generated column stations",
            ));
        }
        if old
            .iter()
            .any(|n| n.owners.iter().any(|o| o != &a.id && o != &z.id))
        {
            return Err(conflict(
                "A multiple-interface column junction requires an explicit global-anchor contract",
            ));
        }
        old.sort_by(|a, b| {
            a.position_m[2]
                .total_cmp(&b.position_m[2])
                .then(a.id.cmp(&b.id))
        });
        let mut groups: Vec<Vec<GeneratedNode>> = vec![];
        for n in old {
            if groups
                .last()
                .is_some_and(|g| (g[0].position_m[2] - n.position_m[2]).abs() < EPS)
            {
                groups.last_mut().unwrap().push(n);
            } else {
                groups.push(vec![n]);
            }
        }
        let mut remap = BTreeMap::new();
        let mut levels = vec![];
        let prefix = format!(
            "building/{}/interface/{}/anchor/{}",
            esc(&b.config.id),
            esc(&i.id),
            esc(&anchor.id)
        );
        for group in groups {
            let height = group[0].position_m[2];
            let tag = if (height - a.roof.base_abs_m).abs() < EPS {
                "base".into()
            } else {
                let n = group.iter().min_by(|a, b| a.id.cmp(&b.id)).unwrap();
                format!(
                    "{}:{}",
                    esc(n.owners.iter().next().unwrap()),
                    n.id.rsplit("/z/").next().unwrap_or("roof")
                )
            };
            let id = sid(format!("{prefix}/node/level/{}", esc(&tag)))?;
            let owners: BTreeSet<_> = [(&a.id, ha), (&z.id, hz)]
                .into_iter()
                .filter(|(_, h)| height <= *h + EPS)
                .map(|(s, _)| s.clone())
                .collect();
            let node = GeneratedNode {
                id: id.clone(),
                position_m: [wp[0], wp[1], height],
                owners,
                restrained: if (height - a.roof.base_abs_m).abs() < EPS {
                    Some(a.base_restraints)
                } else {
                    None
                },
                fingerprint: String::new(),
            };
            if let Some(existing) = b.out.nodes.get(&id) {
                if existing != &node {
                    return Err(conflict("Conflicting interface node key"));
                }
            }
            b.out.nodes.insert(id.clone(), node);
            for n in group {
                remap.insert(n.id, id.clone());
            }
            levels.push((height, id.clone(), tag));
            result.shared_node_ids.push(id);
        }
        let remove: Vec<_> = b
            .out
            .members
            .values()
            .filter(|m| {
                m.role == MemberRole::Column
                    && remap.contains_key(&m.start)
                    && remap.contains_key(&m.end)
            })
            .map(|m| m.id.clone())
            .collect();
        for id in remove {
            b.out.members.remove(&id);
        }
        for m in b.out.members.values_mut() {
            if let Some(id) = remap.get(&m.start) {
                m.start = id.clone();
            }
            if let Some(id) = remap.get(&m.end) {
                m.end = id.clone();
            }
        }
        for old in remap.keys() {
            b.out.nodes.remove(old);
        }
        for pair in levels.windows(2) {
            let mid = (pair[0].0 + pair[1].0) / 2.0;
            let owners: BTreeSet<_> = [(&a.id, ha), (&z.id, hz)]
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
                &z.members.column
            };
            let id = b.member(
                sid(format!(
                    "{prefix}/member/column/from/{}/to/{}",
                    esc(&pair[0].2),
                    esc(&pair[1].2)
                ))?,
                MemberRole::Column,
                pair[0].1.clone(),
                pair[1].1.clone(),
                owners,
                rule,
            )?;
            result.shared_member_ids.push(id);
        }
    }
    // Explicitly connected equal-height boundary beams can be one physical member.
    let mut seen: BTreeMap<(SemanticId, SemanticId), SemanticId> = BTreeMap::new();
    let mut duplicates = vec![];
    let all: Vec<_> = b.out.members.values().cloned().collect();
    for m in all {
        let key = if m.start < m.end {
            (m.start.clone(), m.end.clone())
        } else {
            (m.end.clone(), m.start.clone())
        };
        if let Some(id) = seen.get(&key) {
            let old = b.out.members.get_mut(id).unwrap();
            if old.material_id != m.material_id
                || old.profile_id != m.profile_id
                || old.local_y_reference != m.local_y_reference
                || old.end_releases != m.end_releases
            {
                return Err(conflict(
                    "Coincident connected members have incompatible properties",
                ));
            }
            old.owners.extend(m.owners);
            duplicates.push(m.id);
            result.shared_member_ids.push(id.clone());
        } else {
            seen.insert(key, m.id);
        }
    }
    for id in duplicates {
        b.out.members.remove(&id);
    }
    for (s, ids) in [(&a, &i.edge_a), (&z, &i.edge_b)] {
        let id = sid(format!(
            "{}/surface/wall/{}/{}",
            section_prefix(&b.config.id, &s.id),
            esc(&ids[0]),
            esc(&ids[1])
        ))?;
        b.out.surfaces.remove(&id);
    }
    let ha = g::roof_height(&a, &ba, e0);
    let hz = g::roof_height(&z, &bz, e0);
    if (ha - hz).abs() > EPS {
        let s = if ha > hz { &a } else { &z };
        let low = ha.min(hz);
        let high = ha.max(hz);
        b.surface(
            s,
            &format!("interface/{}/step", esc(&i.id)),
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
    Ok(result)
}
fn finish(b: &mut GeneratedBuilding, config: &BuildingConfig) -> Result<(), ConfiguratorError> {
    b.bim_quantities.steel.clear();
    b.bim_quantities.steel_volume_m3 = 0.0;
    b.bim_quantities.steel_weight_kn = 0.0;
    for m in b.members.values_mut() {
        m.length_m = distance(b.nodes[&m.start].position_m, b.nodes[&m.end].position_m);
        let p = b
            .profiles
            .iter()
            .find(|p| p.id == m.profile_id)
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "profile".into(),
                id: m.profile_id.to_string(),
            })?;
        let mat = b
            .materials
            .iter()
            .find(|p| p.id == m.material_id)
            .ok_or_else(|| ConfiguratorError::MissingReference {
                entity: "material".into(),
                id: m.material_id.to_string(),
            })?;
        let volume = p.area_m2 * m.length_m;
        let weight = volume * mat.weight_density_kn_m3;
        b.bim_quantities.steel.push(SteelQuantity {
            member_id: m.id.clone(),
            owners: m.owners.clone(),
            volume_m3: volume,
            weight_kn: weight,
            source_id: m.self_weight_source_id.clone(),
        });
        b.bim_quantities.steel_volume_m3 += volume;
        b.bim_quantities.steel_weight_kn += weight;
        m.fingerprint.clear();
        m.fingerprint = digest(m)?;
    }
    let unknown = b.surfaces.values().filter(|s| s.mass_kg.is_none()).count();
    b.bim_quantities.unknown_panel_surface_count = unknown;
    b.bim_quantities.panel_mass_kg = if unknown == 0 {
        Some(b.surfaces.values().filter_map(|s| s.mass_kg).sum())
    } else {
        None
    };
    for n in b.nodes.values_mut() {
        n.fingerprint.clear();
        n.fingerprint = digest(n)?;
    }
    for s in b.surfaces.values_mut() {
        s.fingerprint.clear();
        s.fingerprint = digest(s)?;
    }
    b.analysis_load_ledger.projected_member_ids = b.members.keys().cloned().collect();
    b.analysis_load_ledger.projected_steel_weight_kn = b.bim_quantities.steel_weight_kn;
    for n in b.nodes.values().filter(|n| n.restrained.is_some()) {
        let mut foundations = BTreeMap::new();
        for a in config
            .foundation_assignments
            .iter()
            .filter(|a| a.support_node_id == n.id)
        {
            if !n.owners.contains(&a.section_id) {
                return Err(ConfiguratorError::MissingReference {
                    entity: "support section owner".into(),
                    id: a.section_id.to_string(),
                });
            }
            if foundations
                .insert(a.section_id.clone(), a.foundation_id.clone())
                .is_some()
            {
                return Err(ConfiguratorError::InvalidConfiguration {
                    issues: vec![ValidationIssue {
                        code: "DUPLICATE_FOUNDATION_ASSIGNMENT".into(),
                        path: n.id.to_string(),
                        message: "Duplicate support/section assignment".into(),
                        severity: IssueSeverity::Error,
                    }],
                });
            }
        }
        b.reaction_routing.push(ReactionRouting {
            support_node_id: n.id.clone(),
            section_owners: n.owners.clone(),
            foundation_by_section: foundations,
            requires_explicit_allocation: n.owners.len() > 1,
        });
    }
    for a in &config.foundation_assignments {
        if !b.reaction_routing.iter().any(|r| {
            r.support_node_id == a.support_node_id && r.section_owners.contains(&a.section_id)
        }) {
            return Err(ConfiguratorError::MissingReference {
                entity: "foundation support".into(),
                id: a.support_node_id.to_string(),
            });
        }
    }
    let mut blocked = vec![];
    if !b.generation_limits.is_empty() {
        blocked.push("STRUCTURAL_GENERATION_NOT_IMPLEMENTED".into());
    }
    if b.interfaces
        .iter()
        .any(|i| i.status == InterfaceLoadPathStatus::InterfaceLoadPathNotVerified)
    {
        blocked.push("INTERFACE_LOAD_PATH_NOT_VERIFIED".into());
    }
    if !b.allow_disconnected_analysis && b.outlines.len() > 1 {
        if let Some(seed) = b.outlines.keys().next() {
            if connected_analysis_scope(b, seed)?.section_ids.len() != b.outlines.len() {
                blocked.push("DISCONNECTED_LOAD_PATHS_NOT_ACKNOWLEDGED".into());
            }
        }
    }
    b.analysis_status = if b.nodes.len() > structural_core::MAX_NODES
        || b.members.len() > structural_core::MAX_MEMBERS
    {
        AnalysisProjectionStatus::AnalysisModelCapacityExceeded {
            nodes: b.nodes.len(),
            members: b.members.len(),
            max_nodes: structural_core::MAX_NODES,
            max_members: structural_core::MAX_MEMBERS,
        }
    } else if !blocked.is_empty() {
        AnalysisProjectionStatus::Blocked { reasons: blocked }
    } else {
        AnalysisProjectionStatus::ReadyForBoundedProjection
    };
    b.graph_fingerprint.clear();
    b.graph_fingerprint = digest(b)?;
    Ok(())
}
pub(crate) fn compute_offsets(
    config: &BuildingConfig,
) -> Result<BTreeMap<SemanticId, [f64; 2]>, ConfiguratorError> {
    let mut edges: BTreeMap<SemanticId, Vec<(SemanticId, [f64; 2], SemanticId)>> = config
        .sections
        .iter()
        .map(|s| (s.id.clone(), vec![]))
        .collect();
    let mut rigid: BTreeMap<SemanticId, Vec<SemanticId>> = config
        .sections
        .iter()
        .map(|s| (s.id.clone(), vec![]))
        .collect();
    let mut directed_joints = vec![];
    for i in &config.interfaces {
        let delta = match i.kind {
            InterfaceKind::StructurallyConnected { .. } => Some([0.0; 2]),
            InterfaceKind::IndependentWithJoint {
                width_m,
                offset_b_direction,
            } => Some([
                width_m * offset_b_direction[0],
                width_m * offset_b_direction[1],
            ]),
            _ => None,
        };
        if let Some(d) = delta {
            if !edges.contains_key(&i.section_a) || !edges.contains_key(&i.section_b) {
                return Err(ConfiguratorError::MissingReference {
                    entity: "interface section".into(),
                    id: i.id.to_string(),
                });
            }
            edges
                .get_mut(&i.section_a)
                .unwrap()
                .push((i.section_b.clone(), d, i.id.clone()));
            edges.get_mut(&i.section_b).unwrap().push((
                i.section_a.clone(),
                [-d[0], -d[1]],
                i.id.clone(),
            ));
            if matches!(i.kind, InterfaceKind::StructurallyConnected { .. }) {
                rigid
                    .get_mut(&i.section_a)
                    .unwrap()
                    .push(i.section_b.clone());
                rigid
                    .get_mut(&i.section_b)
                    .unwrap()
                    .push(i.section_a.clone());
            } else {
                directed_joints.push((i.section_a.clone(), i.section_b.clone(), i.id.clone()));
            }
        }
    }
    // Rigid sections translate together. A joint's directed source stays fixed;
    // semantic labels and collection order must never select the physical anchor.
    let mut component: BTreeMap<SemanticId, usize> = BTreeMap::new();
    let mut groups: Vec<Vec<SemanticId>> = vec![];
    for seed in edges.keys() {
        if component.contains_key(seed) {
            continue;
        }
        let index = groups.len();
        let mut group = vec![];
        let mut stack = vec![seed.clone()];
        component.insert(seed.clone(), index);
        while let Some(a) = stack.pop() {
            group.push(a.clone());
            for z in &rigid[&a] {
                if !component.contains_key(z) {
                    component.insert(z.clone(), index);
                    stack.push(z.clone());
                }
            }
        }
        groups.push(group);
    }
    let mut incoming = vec![0usize; groups.len()];
    for (a, z, id) in &directed_joints {
        if component[a] == component[z] {
            return Err(ConfiguratorError::InterfaceConflict {
                interface_id: id.to_string(),
                reason:
                    "An independent joint cannot displace one section of the same rigid component"
                        .into(),
            });
        }
        incoming[component[z]] += 1;
    }
    let mut offsets = BTreeMap::new();
    let mut visited = BTreeSet::new();
    for start in 0..groups.len() {
        if visited.contains(&start) {
            continue;
        }
        let mut weak = vec![];
        let mut pending = vec![start];
        visited.insert(start);
        while let Some(group) = pending.pop() {
            weak.push(group);
            for a in &groups[group] {
                for (z, _, _) in &edges[a] {
                    let c = component[z];
                    if visited.insert(c) {
                        pending.push(c);
                    }
                }
            }
        }
        let roots: Vec<_> = weak
            .iter()
            .filter(|c| incoming[**c] == 0)
            .copied()
            .collect();
        if roots.len() != 1 {
            return Err(ConfiguratorError::InterfaceConflict{interface_id:"joint_component_anchor".into(),reason:"Joint graph must have one directed source rigid component; ambiguous or cyclic absolute anchoring requires an explicit unsupported anchor contract".into()});
        }
        let seed = &groups[roots[0]][0];
        offsets.insert(seed.clone(), [0.0; 2]);
        let mut stack = vec![seed.clone()];
        while let Some(a) = stack.pop() {
            let da = offsets[&a];
            for (z, delta, interface_id) in &edges[&a] {
                let dz = [da[0] + delta[0], da[1] + delta[1]];
                if dz.iter().any(|x| !x.is_finite()) {
                    return Err(ConfiguratorError::ResourceLimit {
                        reason: "Nonfinite joint offset chain".into(),
                    });
                }
                if let Some(old) = offsets.get(z) {
                    if (old[0] - dz[0]).abs() > EPS || (old[1] - dz[1]).abs() > EPS {
                        return Err(ConfiguratorError::InterfaceConflict {
                            interface_id: interface_id.to_string(),
                            reason: "Inconsistent joint/rigid displacement cycle".into(),
                        });
                    }
                } else {
                    offsets.insert(z.clone(), dz);
                    stack.push(z.clone());
                }
            }
        }
    }
    Ok(offsets)
}
#[derive(serde::Serialize)]
struct RawSectionKey<'a> {
    cache_schema: &'static str,
    generator_version: &'static str,
    building_id: &'a SemanticId,
    section: &'a SectionConfig,
    transform: &'a LocalTransform,
    resolved_offset_m: [f64; 2],
    materials: Vec<&'a SteelMaterial>,
    profiles: Vec<&'a MemberProfile>,
}

fn raw_section_key(
    config: &BuildingConfig,
    section: &SectionConfig,
    offset: [f64; 2],
) -> Result<CacheKey, ConfiguratorError> {
    let rules = [
        &section.members.column,
        &section.members.rafter,
        &section.members.beam,
        &section.members.brace,
        &section.members.purlin,
        &section.members.wall_girt,
    ];
    let material_ids: BTreeSet<_> = rules.iter().map(|rule| rule.material_id.clone()).collect();
    let profile_ids: BTreeSet<_> = rules.iter().map(|rule| rule.profile_id.clone()).collect();
    let mut materials: Vec<_> = config
        .materials
        .iter()
        .filter(|m| material_ids.contains(&m.id))
        .collect();
    let mut profiles: Vec<_> = config
        .profiles
        .iter()
        .filter(|p| profile_ids.contains(&p.id))
        .collect();
    if materials.len() != material_ids.len() || profiles.len() != profile_ids.len() {
        return Err(ConfiguratorError::MissingReference {
            entity: "raw section cache dependency".into(),
            id: section.id.to_string(),
        });
    }
    materials.sort_by(|a, b| a.id.cmp(&b.id));
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    CacheKey::from_serializable(&RawSectionKey {
        cache_schema: "raw-section-cache-v1",
        generator_version: GENERATOR_VERSION,
        building_id: &config.id,
        section,
        transform: &config.transform,
        resolved_offset_m: offset,
        materials,
        profiles,
    })
}

fn produce_raw_section(
    config: &BuildingConfig,
    section: &SectionConfig,
    offset: [f64; 2],
    diagnostics: &mut GenerationDiagnostics,
) -> Result<RawSectionParts, ConfiguratorError> {
    let mut raw = Builder {
        config,
        out: output_shell(config, String::new(), false),
        offsets: BTreeMap::from([(section.id.clone(), offset)]),
    };
    section_generate(&mut raw, section, diagnostics)?;
    Ok(RawSectionParts {
        nodes: raw.out.nodes,
        members: raw.out.members,
        surfaces: raw.out.surfaces,
        outlines: raw.out.outlines,
        generation_limits: raw.out.generation_limits,
    })
}

fn merge_raw_section(
    out: &mut GeneratedBuilding,
    raw: &RawSectionParts,
) -> Result<(), ConfiguratorError> {
    for (current, extra) in [
        (out.nodes.len(), raw.nodes.len()),
        (out.members.len(), raw.members.len()),
        (out.surfaces.len(), raw.surfaces.len()),
        (out.outlines.len(), raw.outlines.len()),
        (out.generation_limits.len(), raw.generation_limits.len()),
    ] {
        if current
            .checked_add(extra)
            .is_none_or(|count| count > MAX_BIM_ITEMS)
        {
            return Err(ConfiguratorError::ResourceLimit {
                reason: "Combined raw section parts exceed BIM budget".into(),
            });
        }
    }
    macro_rules! merge_map {
        ($field:ident) => {
            for (key, value) in &raw.$field {
                if out.$field.insert(key.clone(), value.clone()).is_some() {
                    return Err(ConfiguratorError::GeneratedBindingMismatch {
                        message: format!(
                            "Conflicting raw section {} key {key}",
                            stringify!($field)
                        ),
                    });
                }
            }
        };
    }
    merge_map!(nodes);
    merge_map!(members);
    merge_map!(surfaces);
    merge_map!(outlines);
    merge_map!(generation_limits);
    Ok(())
}

pub fn generate_building(config: &BuildingConfig) -> Result<GeneratedBuilding, ConfiguratorError> {
    generate_building_internal(config, None).map(|(generated, _)| generated)
}

/// Optional process-local reuse. Canonical DTOs and the cold API are unchanged.
/// Configuration validation, interfaces and finalization always run fresh.
pub fn generate_building_with_cache(
    config: &BuildingConfig,
    cache: &mut GeneratorCache,
) -> Result<(GeneratedBuilding, GenerationDiagnostics), ConfiguratorError> {
    generate_building_internal(config, Some(cache))
}

fn generate_building_internal(
    config: &BuildingConfig,
    mut cache: Option<&mut GeneratorCache>,
) -> Result<(GeneratedBuilding, GenerationDiagnostics), ConfiguratorError> {
    let issues = validate_config(config);
    let fatal: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .cloned()
        .collect();
    if !fatal.is_empty() {
        return Err(ConfiguratorError::InvalidConfiguration { issues: fatal });
    }
    let canonical = canonical_config(config);
    let mut b = Builder {
        config: &canonical,
        out: empty(&canonical)?,
        offsets: compute_offsets(&canonical)?,
    };
    for i in issues.iter().filter(|i| i.severity != IssueSeverity::Error) {
        b.out
            .limitations
            .push(format!("{}:{}:{}", i.code, i.path, i.message));
    }
    let mut diagnostics = GenerationDiagnostics::default();
    let mut cache_uses = Vec::new();
    for s in &canonical.sections {
        if let Some(cache_ref) = cache.as_deref() {
            let offset = b.offsets.get(&s.id).copied().unwrap_or([0.0; 2]);
            let key = raw_section_key(&canonical, s, offset)?;
            let hit = cache_ref.lookup(&canonical.id, &s.id, &key)?;
            let (parts, was_hit) = if let Some(parts) = hit {
                (parts, true)
            } else {
                (
                    Arc::new(produce_raw_section(
                        &canonical,
                        s,
                        offset,
                        &mut diagnostics,
                    )?),
                    false,
                )
            };
            merge_raw_section(&mut b.out, &parts)?;
            cache_uses.push(CacheUse {
                building_id: canonical.id.clone(),
                section_id: s.id.clone(),
                key,
                parts,
                was_hit,
            });
        } else {
            section_generate(&mut b, s, &mut diagnostics)?;
        }
    }
    for i in &canonical.interfaces {
        let generated = connect(&mut b, i)?;
        b.out.interfaces.push(generated);
    }
    for (a, z) in crate::validation::undeclared_contact_pairs(&canonical)? {
        b.out.interfaces.push(GeneratedInterface {
            id: sid(format!(
                "building/{}/interface/implicit/{}/{}",
                esc(&canonical.id),
                esc(&a),
                esc(&z)
            ))?,
            section_a: a,
            section_b: z,
            status: InterfaceLoadPathStatus::InterfaceLoadPathNotVerified,
            shared_node_ids: vec![],
            shared_member_ids: vec![],
        });
    }
    finish(&mut b.out, &canonical)?;
    diagnostics.interfaces_regenerated = b.out.interfaces.len();
    diagnostics.full_graph_postprocess = true;
    if let Some(cache) = cache.as_deref_mut() {
        cache.commit_success(cache_uses, &mut diagnostics)?;
    }
    Ok((b.out, diagnostics))
}
pub fn generate(config: &BuildingConfig) -> Result<GeneratedBuilding, ConfiguratorError> {
    generate_building(config)
}
pub fn connected_analysis_scope(
    g: &GeneratedBuilding,
    seed: &str,
) -> Result<ConnectedAnalysisScope, ConfiguratorError> {
    let seed = sid(seed)?;
    if !g.outlines.contains_key(&seed) {
        return Err(ConfiguratorError::MissingReference {
            entity: "section".into(),
            id: seed.to_string(),
        });
    }
    let mut ids = BTreeSet::from([seed]);
    loop {
        let old = ids.len();
        for i in &g.interfaces {
            if i.status == InterfaceLoadPathStatus::ExplicitRigidIdealization
                && (ids.contains(&i.section_a) || ids.contains(&i.section_b))
            {
                ids.insert(i.section_a.clone());
                ids.insert(i.section_b.clone());
            }
        }
        if ids.len() == old {
            break;
        }
    }
    Ok(ConnectedAnalysisScope { section_ids: ids })
}
pub fn set_section_eave(
    config: &mut BuildingConfig,
    id: &str,
    eave: f64,
    mode: RoofHeightUpdate,
) -> Result<(), ConfiguratorError> {
    let mut next = config.clone();
    let s = next
        .sections
        .iter_mut()
        .find(|s| s.id.as_str() == id)
        .ok_or_else(|| ConfiguratorError::MissingReference {
            entity: "section".into(),
            id: id.into(),
        })?;
    let delta = eave - s.roof.eave_abs_m;
    s.roof.eave_abs_m = eave;
    if mode == RoofHeightUpdate::PreserveRise {
        s.roof.ridge_abs_m += delta;
    }
    let fatal: Vec<_> = validate_config(&next)
        .into_iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .collect();
    if !fatal.is_empty() {
        return Err(ConfiguratorError::InvalidConfiguration { issues: fatal });
    }
    *config = next;
    Ok(())
}
pub fn diff_generated(before: &GeneratedBuilding, after: &GeneratedBuilding) -> GenerationDelta {
    let inventory = |b: &GeneratedBuilding| -> BTreeMap<SemanticId, String> {
        b.nodes
            .iter()
            .map(|(k, v)| (k.clone(), v.fingerprint.clone()))
            .chain(
                b.members
                    .iter()
                    .map(|(k, v)| (k.clone(), v.fingerprint.clone())),
            )
            .chain(
                b.surfaces
                    .iter()
                    .map(|(k, v)| (k.clone(), v.fingerprint.clone())),
            )
            .chain(b.outlines.iter().filter_map(|(k, v)| {
                SemanticId::new(format!("{}/outline", section_prefix(&b.id, k)))
                    .ok()
                    .map(|id| (id, digest(v).unwrap_or_default()))
            }))
            .chain(b.materials.iter().filter_map(|v| {
                SemanticId::new(format!("building/{}/material/{}", esc(&b.id), esc(&v.id)))
                    .ok()
                    .map(|id| (id, digest(v).unwrap_or_default()))
            }))
            .chain(b.profiles.iter().filter_map(|v| {
                SemanticId::new(format!("building/{}/profile/{}", esc(&b.id), esc(&v.id)))
                    .ok()
                    .map(|id| (id, digest(v).unwrap_or_default()))
            }))
            .chain(
                SemanticId::new(format!("building/{}/parameters", esc(&b.id)))
                    .ok()
                    .map(|id| (id, b.config_fingerprint.clone())),
            )
            .collect()
    };
    let a = inventory(before);
    let b = inventory(after);
    let mut d = GenerationDelta {
        added: BTreeSet::new(),
        removed: BTreeSet::new(),
        changed: BTreeSet::new(),
        unchanged: BTreeSet::new(),
    };
    for (id, fp) in &a {
        match b.get(id) {
            None => {
                d.removed.insert(id.clone());
            }
            Some(other) if other != fp => {
                d.changed.insert(id.clone());
            }
            Some(_) => {
                d.unchanged.insert(id.clone());
            }
        }
    }
    for id in b.keys() {
        if !a.contains_key(id) {
            d.added.insert(id.clone());
        }
    }
    d
}
pub fn validate_generated_binding(
    config: &BuildingConfig,
    g: &GeneratedBuilding,
) -> Result<(), ConfiguratorError> {
    let fail = |message: String| ConfiguratorError::GeneratedBindingMismatch { message };
    let fatal: Vec<_> = validate_config(config)
        .into_iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .collect();
    if !fatal.is_empty() {
        return Err(ConfiguratorError::InvalidConfiguration { issues: fatal });
    }
    if g.id != config.id
        || g.generator_version != GENERATOR_VERSION
        || g.config_fingerprint != digest(&canonical_config(config))?
    {
        return Err(fail("Parameters or generator version do not match".into()));
    }
    for (key, n) in &g.nodes {
        let mut value = n.clone();
        let fp = value.fingerprint.clone();
        value.fingerprint.clear();
        if key != &n.id || n.position_m.iter().any(|v| !v.is_finite()) || digest(&value)? != fp {
            return Err(fail(format!("Node binding mismatch {key}")));
        }
    }
    let mut sources = BTreeSet::new();
    for (key, m) in &g.members {
        let mut value = m.clone();
        let fp = value.fingerprint.clone();
        value.fingerprint.clear();
        if key != &m.id
            || !m.length_m.is_finite()
            || digest(&value)? != fp
            || !sources.insert(m.self_weight_source_id.clone())
        {
            return Err(fail(format!("Member binding mismatch {key}")));
        }
        let a = g
            .nodes
            .get(&m.start)
            .ok_or_else(|| fail(format!("Missing node {}", m.start)))?;
        let b = g
            .nodes
            .get(&m.end)
            .ok_or_else(|| fail(format!("Missing node {}", m.end)))?;
        if (distance(a.position_m, b.position_m) - m.length_m).abs() > EPS || m.length_m <= EPS {
            return Err(fail(format!("Member length mismatch {key}")));
        }
    }
    for (key, s) in &g.surfaces {
        let mut value = s.clone();
        let fp = value.fingerprint.clone();
        value.fingerprint.clear();
        if key != &s.id
            || s.stiffness_enabled
            || !s.area_m2.is_finite()
            || s.vertices_m.iter().flatten().any(|v| !v.is_finite())
            || s.mass_kg.is_some_and(|m| !m.is_finite() || m < 0.0)
            || digest(&value)? != fp
            || (g::area3(&s.vertices_m) - s.area_m2).abs() > EPS
        {
            return Err(fail(format!("Surface binding mismatch {key}")));
        }
    }
    for (key, o) in &g.outlines {
        if key != &o.section_id {
            return Err(fail(format!("Outline binding mismatch {key}")));
        }
    }
    let mut value = g.clone();
    let expected = value.graph_fingerprint.clone();
    value.graph_fingerprint.clear();
    if digest(&value)? != expected {
        return Err(fail("Graph fingerprint mismatch".into()));
    }
    crate::binding::validate_semantics(config, g)?;
    Ok(())
}

#[cfg(test)]
mod station_budget_tests {
    use super::*;

    #[test]
    fn tiny_pitch_product_is_refused_without_station_allocation() {
        let mut config = crate::fixture(crate::FixtureKind::IndependentJoint);
        let section = &mut config.sections[0];
        section.grid.frame_stations = GridSpacing::Uniform { pitch_m: 12.0 };
        section.grid.purlin_pitch_m = 6.0 / 50_000.0;
        assert!(matches!(
            preflight_section_station_budget(section),
            Err(ConfiguratorError::ResourceLimit { .. })
        ));
    }

    #[test]
    fn product_guard_precedes_grid_validation_and_merge() {
        let mut config = crate::fixture(crate::FixtureKind::IndependentJoint);
        let section = &mut config.sections[0];
        section.grid.frame_stations = GridSpacing::Uniform { pitch_m: 12.0 };
        section.grid.column_stations = GridSpacing::Explicit { stations: vec![] };
        section.grid.purlin_pitch_m = 6.0 / 50_000.0;
        // Empty columns are an early cheap trap: without the preflight guard,
        // stations() fails before reaching the large purlin vector. No heavy
        // expansion occurs even if the guard is accidentally removed later.
        assert!(matches!(
            section_stations(section),
            Err(ConfiguratorError::ResourceLimit { .. })
        ));
    }
}
