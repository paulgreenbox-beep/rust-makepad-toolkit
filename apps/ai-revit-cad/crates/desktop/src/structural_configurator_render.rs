//! Native viewer helpers for the structural configurator.
//!
//! This module projects the full generated BIM graph to native viewer meshes,
//! semantic picks, typed fit bounds, and honest presentation holds.

use std::collections::{BTreeMap, BTreeSet};

use structural_configurator::{
    AnalysisProjectionStatus, GeneratedBuilding, GeneratedMember, GeneratedSurface, MemberProfile,
    MemberRole, ProfileVisual, SurfaceRole,
};
use viewer_core::{
    Aabb, Mesh, NavigationPivot, ObjectId, Scene, SceneObject, Triangle, Vec3, Viewer,
};

/// Native display modes for one structural configurator viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScViewMode {
    Architectural,
    Frame,
    Connections,
    Analysis,
    Loads,
    Reactions,
    Foundation,
}

impl ScViewMode {
    /// User-facing name for this display mode.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Architectural => "Architectural",
            Self::Frame => "Frame",
            Self::Connections => "Connections",
            Self::Analysis => "Analysis",
            Self::Loads => "Loads",
            Self::Reactions => "Reactions",
            Self::Foundation => "Foundation",
        }
    }
}

/// Fits caller-supplied typed bounds and keeps orbit/dolly centered on them.
///
/// `bounds` must come from the authoritative typed projection, not from display
/// mesh measurements. The current viewer and its scene, visibility, selection,
/// isolation, highlights, and section box are restored unchanged.
pub fn fit_typed_bounds(viewer: &mut Viewer, bounds: Aabb, margin: f64) -> Result<(), String> {
    if bounds.is_empty() {
        return Ok(());
    }

    // Fit a copy first so invalid bounds or camera parameters cannot partially
    // mutate the live viewer before restore_state applies the completed pose.
    let mut camera = *viewer.camera();
    camera
        .fit_bounds(bounds, viewer.viewport(), margin)
        .map_err(|error| format!("could not fit typed bounds: {error}"))?;

    // Viewer::orbit uses its NavigationPivot separately from Camera::pivot.
    // Restore both from one captured state while retaining all other fields.
    let mut state = viewer.capture_state();
    state.camera = camera.state();
    state.navigation_pivot = NavigationPivot {
        object_id: None,
        world_point: bounds.center(),
    };
    viewer
        .restore_state(&state)
        .map_err(|error| format!("could not apply fitted viewer state: {error}"))?;
    Ok(())
}

/// A semantic selection target for one native viewer object.
///
/// These IDs come from GeneratedBuilding semantic IDs. Viewer object IDs are
/// stable renderer-local hashes and never reuse dense analysis IDs.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ScPick {
    Node(structural_configurator::SemanticId),
    Member(structural_configurator::SemanticId),
    Surface(structural_configurator::SemanticId),
    Section(structural_configurator::SemanticId),
    Interface(structural_configurator::SemanticId),
    Support(structural_configurator::SemanticId),
    Displacement(structural_configurator::SemanticId),
    DeformedMember(structural_configurator::SemanticId),
    AppliedLoad {
        load_case_id: u64,
        source_id: String,
        target: ScLoadTarget,
    },
    Reaction(structural_configurator::SemanticId),
    Foundation(foundation_core::FoundationId),
    FoundationCell {
        foundation_id: foundation_core::FoundationId,
        cell_index: usize,
    },
    GroundLayer {
        foundation_id: foundation_core::FoundationId,
        layer_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ScLoadTarget {
    Node(structural_configurator::SemanticId),
    Member(structural_configurator::SemanticId),
}

/// Source IDs attached to a rendered object when the typed source provides one.
///
/// These are source references only. They do not certify that a load was
/// applied to, or a result was produced by, an analysis solver.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScSourceIds {
    pub self_weight_source_id: Option<structural_configurator::SemanticId>,
    pub load_source_id: Option<structural_configurator::SemanticId>,
    pub applied_load_source_ids: BTreeSet<String>,
    pub foundation_id: Option<foundation_core::FoundationId>,
}

/// Presentation-only controls for projecting all generated sections.
#[derive(Clone, Debug)]
pub struct ScRenderOptions {
    pub mode: ScViewMode,
    /// Empty means all generated sections. Otherwise shared objects remain
    /// visible when at least one owning section is selected.
    pub visible_sections: BTreeSet<structural_configurator::SemanticId>,
    pub hide_cladding: bool,
    /// Linear alpha for displayed generated host surfaces, in 0..=1.
    pub cladding_alpha: f32,
}

impl Default for ScRenderOptions {
    fn default() -> Self {
        Self {
            mode: ScViewMode::Architectural,
            visible_sections: BTreeSet::new(),
            hide_cladding: false,
            cladding_alpha: 0.38,
        }
    }
}

/// Full generated-building scene plus semantic selection and typed fit maps.
#[derive(Debug)]
pub struct ScRenderScene {
    pub scene: Scene,
    pub mode: ScViewMode,
    pub visible_sections: BTreeSet<structural_configurator::SemanticId>,
    /// Renderer ObjectId to application semantic identity.
    pub picks: BTreeMap<ObjectId, ScPick>,
    /// All rendered-object IDs owned by each generated section, including
    /// objects hidden by the current presentation filters.
    pub by_section: BTreeMap<structural_configurator::SemanticId, BTreeSet<ObjectId>>,
    /// Bounds accumulated from generated nodes, member/profile dimensions,
    /// surfaces, and section outlines. No viewer mesh query is used.
    pub overall_bounds: Option<Aabb>,
    pub section_bounds: BTreeMap<structural_configurator::SemanticId, Aabb>,
    /// Actual generated source references. Empty values mean the object has no
    /// source ID in this DTO; no source identifier is synthesized.
    pub object_sources: BTreeMap<ObjectId, ScSourceIds>,
    /// Object IDs added by the current validated analysis snapshot. Replacing
    /// a snapshot removes this set before committing its new overlay.
    pub analysis_overlay_object_ids: BTreeSet<ObjectId>,
    pub overlay_snapshot_fingerprint: Option<String>,
    pub overlay_mode: Option<ScViewMode>,
    pub overlay_bounds: Option<Aabb>,
    pub overlay_section_bounds: BTreeMap<structural_configurator::SemanticId, Aabb>,
    pub building_id: structural_configurator::SemanticId,
    pub graph_fingerprint: String,
    /// View holds, generation limitations, and display-only fallbacks.
    pub status: Vec<String>,
}

// Independent display guards. These are intentionally far above the bounded
// analysis projection limits; exceeding them returns an error instead of
// truncating the full generated BIM scene.
const MAX_RENDER_NODES: usize = 50_000;
const MAX_RENDER_MEMBERS: usize = 100_000;
const MAX_RENDER_SURFACES: usize = 25_000;
const MAX_RENDER_INTERFACES: usize = 50_000;
const MAX_RENDER_SECTIONS: usize = 100_000;
const MAX_SCENE_OBJECTS: usize = 200_000;
const MAX_TOTAL_TRIANGLES: usize = 3_000_000;
const MAX_SURFACE_VERTICES: usize = 4_096;
const MAX_COORDINATE_M: f64 = 1.0e6;
const GEOMETRY_EPSILON: f64 = 1.0e-10;

/// Builds the native triangle-mesh presentation from the complete generated
/// BIM graph. It does not construct a StructuralModel or apply analysis limits.
pub fn build_scene(
    building: &GeneratedBuilding,
    options: &ScRenderOptions,
) -> Result<ScRenderScene, String> {
    validate_render_options(options)?;
    validate_render_budgets(building)?;

    let profiles = index_profiles(&building.profiles)?;
    let materials = index_material_ids(&building.materials)?;
    let mut node_points = BTreeMap::new();
    let mut bounds = TypedBounds::new();
    let mut section_ids = BTreeSet::new();

    for (key, node) in &building.nodes {
        if key != &node.id {
            return Err(format!(
                "generated node map key {} differs from node ID {}",
                key, node.id
            ));
        }
        let point = checked_point(node.position_m, &format!("node {}", node.id))?;
        node_points.insert(node.id.clone(), point);
        add_section_ids(&mut section_ids, &node.owners);
        bounds.include(point, &node.owners);
    }

    for (key, outline) in &building.outlines {
        if key != &outline.section_id {
            return Err(format!(
                "section outline map key {} differs from section ID {}",
                key, outline.section_id
            ));
        }
        if outline.world_corners_m.len() > MAX_SURFACE_VERTICES {
            return Err(format!(
                "section {} outline has {} points, exceeding the typed outline display budget {}",
                outline.section_id,
                outline.world_corners_m.len(),
                MAX_SURFACE_VERTICES
            ));
        }
        section_ids.insert(outline.section_id.clone());
        for (index, raw) in outline.world_corners_m.iter().enumerate() {
            let point = checked_point(
                *raw,
                &format!("section {} outline point {index}", outline.section_id),
            )?;
            bounds.include_one(point, &outline.section_id);
        }
    }

    for section_id in building.generation_limits.keys() {
        section_ids.insert(section_id.clone());
    }

    for (key, surface) in &building.surfaces {
        if key != &surface.id {
            return Err(format!(
                "generated surface map key {} differs from surface ID {}",
                key, surface.id
            ));
        }
        if surface.vertices_m.len() > MAX_SURFACE_VERTICES {
            return Err(format!(
                "surface {} has {} vertices, exceeding the per-surface display budget {}",
                surface.id,
                surface.vertices_m.len(),
                MAX_SURFACE_VERTICES
            ));
        }
        section_ids.insert(surface.section_id.clone());
        for (index, raw) in surface.vertices_m.iter().enumerate() {
            let point = checked_point(*raw, &format!("surface {} vertex {index}", surface.id))?;
            bounds.include_one(point, &surface.section_id);
        }
    }

    for member in building.members.values() {
        add_section_ids(&mut section_ids, &member.owners);
        if !profiles.contains_key(&member.profile_id) {
            return Err(format!(
                "member {} references missing profile {}",
                member.id, member.profile_id
            ));
        }
        if !materials.contains(&member.material_id) {
            return Err(format!(
                "member {} references missing material {}",
                member.id, member.material_id
            ));
        }
        let start = node_points.get(&member.start).ok_or_else(|| {
            format!(
                "member {} references missing start node {}",
                member.id, member.start
            )
        })?;
        let end = node_points.get(&member.end).ok_or_else(|| {
            format!(
                "member {} references missing end node {}",
                member.id, member.end
            )
        })?;
        validate_member_axis(member, *start, *end)?;
        if let Some(profile) = profiles.get(&member.profile_id) {
            if let Some(visual) = &profile.visualization {
                let (_x, _y, start_ring, end_ring) = profile_rings(member, *start, *end, visual)?;
                for point in start_ring.into_iter().chain(end_ring) {
                    bounds.include(point, &member.owners);
                }
            }
        }
    }

    let mut support_owners: BTreeMap<structural_configurator::SemanticId, BTreeSet<_>> =
        BTreeMap::new();
    for node in building.nodes.values() {
        if node
            .restrained
            .map_or(false, |dofs| dofs.iter().any(|restrained| *restrained))
        {
            support_owners
                .entry(node.id.clone())
                .or_default()
                .extend(node.owners.iter().cloned());
        }
    }
    for route in &building.reaction_routing {
        let node = node_points.get(&route.support_node_id).ok_or_else(|| {
            format!(
                "reaction routing references missing support node {}",
                route.support_node_id
            )
        })?;
        let owners = support_owners
            .entry(route.support_node_id.clone())
            .or_default();
        owners.extend(route.section_owners.iter().cloned());
        if owners.is_empty() {
            if let Some(source_node) = building.nodes.get(&route.support_node_id) {
                owners.extend(source_node.owners.iter().cloned());
            }
        }
        bounds.include(*node, owners);
        add_section_ids(&mut section_ids, &route.section_owners);
    }

    let mut interface_anchors = BTreeMap::new();
    for interface in &building.interfaces {
        section_ids.insert(interface.section_a.clone());
        section_ids.insert(interface.section_b.clone());
        let mut anchors = BTreeSet::new();
        for node_id in &interface.shared_node_ids {
            let point = node_points.get(node_id).ok_or_else(|| {
                format!(
                    "interface {} references missing shared node {}",
                    interface.id, node_id
                )
            })?;
            anchors.insert(point_key(*point));
            bounds.include_one(*point, &interface.section_a);
            bounds.include_one(*point, &interface.section_b);
        }
        for member_id in &interface.shared_member_ids {
            let member = building.members.get(member_id).ok_or_else(|| {
                format!(
                    "interface {} references missing shared member {}",
                    interface.id, member_id
                )
            })?;
            let start = node_points.get(&member.start).ok_or_else(|| {
                format!(
                    "member {} references missing start node {}",
                    member.id, member.start
                )
            })?;
            let end = node_points.get(&member.end).ok_or_else(|| {
                format!(
                    "member {} references missing end node {}",
                    member.id, member.end
                )
            })?;
            let middle = (*start + *end) * 0.5;
            anchors.insert(point_key(middle));
            bounds.include_one(middle, &interface.section_a);
            bounds.include_one(middle, &interface.section_b);
        }
        interface_anchors.insert(interface.id.clone(), anchors);
    }

    if section_ids.len() > MAX_RENDER_SECTIONS {
        return Err(format!(
            "generated section count {} exceeds renderer budget {}",
            section_ids.len(),
            MAX_RENDER_SECTIONS
        ));
    }
    bounds.ensure_valid()?;
    let raw_overall = bounds.overall();
    let display_scale = raw_overall
        .map(|aabb| aabb.extent().max_component().abs())
        .filter(|extent| extent.is_finite() && *extent > GEOMETRY_EPSILON)
        .unwrap_or(1.0);
    let symbol_radius = (display_scale * 0.002).clamp(0.006, 0.08);

    let mut scene = Scene::new();
    let mut picks = BTreeMap::new();
    let mut by_section: BTreeMap<_, BTreeSet<_>> = section_ids
        .iter()
        .cloned()
        .map(|id| (id, BTreeSet::new()))
        .collect();
    let mut object_sources = BTreeMap::new();
    let mut triangle_count = 0usize;
    let mut fallback_members = Vec::new();
    let mut status = initial_status(building);

    for (id, node) in &building.nodes {
        let point = node_points[id];
        let triangles = octahedron(point, symbol_radius)?;
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Node(id.clone()),
            format!("Node {}", id.as_str()),
            triangles,
            [0.82, 0.58, 0.16, 0.95],
            visible_for_owners(&options.visible_sections, &node.owners),
            &node.owners,
            ScSourceIds::default(),
        )?;
    }

    for (id, member) in &building.members {
        if id != &member.id {
            return Err(format!(
                "generated member map key {} differs from member ID {}",
                id, member.id
            ));
        }
        let start = node_points[&member.start];
        let end = node_points[&member.end];
        let profile = profiles.get(&member.profile_id).ok_or_else(|| {
            format!(
                "member {} references missing profile {}",
                member.id, member.profile_id
            )
        })?;
        let (triangles, used_fallback) = if let Some(visual) = &profile.visualization {
            (profile_prism_mesh(member, start, end, visual)?, false)
        } else {
            fallback_members.push(member.id.as_str().to_owned());
            (
                cylinder_mesh(start, end, (symbol_radius * 0.28).max(0.002), 8)?,
                true,
            )
        };
        let color = member_color(member.role, used_fallback);
        let name = if used_fallback {
            format!(
                "{} {} · illustrative axis only; profile visualization absent",
                member_role_name(member.role),
                id.as_str()
            )
        } else {
            format!(
                "{} {} · display profile only",
                member_role_name(member.role),
                id.as_str()
            )
        };
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Member(id.clone()),
            name,
            triangles,
            color,
            visible_for_owners(&options.visible_sections, &member.owners),
            &member.owners,
            ScSourceIds {
                self_weight_source_id: Some(member.self_weight_source_id.clone()),
                load_source_id: None,
                ..ScSourceIds::default()
            },
        )?;
    }

    for (id, surface) in &building.surfaces {
        let triangles = surface_mesh(surface)?;
        let owners = BTreeSet::from([surface.section_id.clone()]);
        let mode_shows_surfaces =
            !matches!(options.mode, ScViewMode::Frame | ScViewMode::Connections);
        let visible = mode_shows_surfaces
            && !options.hide_cladding
            && options.cladding_alpha > 0.0
            && visible_for_owners(&options.visible_sections, &owners);
        let role_name = surface_role_name(surface.role);
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Surface(id.clone()),
            format!(
                "{role_name} {} · generated host surface · geometry display only",
                id.as_str()
            ),
            triangles,
            [0.58, 0.70, 0.82, options.cladding_alpha],
            visible,
            &owners,
            ScSourceIds {
                self_weight_source_id: None,
                load_source_id: Some(surface.load_source_id.clone()),
                ..ScSourceIds::default()
            },
        )?;
    }

    for (section_id, section_bounds) in bounds.section_bounds() {
        let triangles = bounds_wire_mesh(*section_bounds, symbol_radius * 0.18)?;
        let owners = BTreeSet::from([section_id.clone()]);
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Section(section_id.clone()),
            format!(
                "Section {} · typed extent selection proxy",
                section_id.as_str()
            ),
            triangles,
            [0.34, 0.58, 0.72, 0.14],
            visible_for_owners(&options.visible_sections, &owners),
            &owners,
            ScSourceIds::default(),
        )?;
    }

    for interface in &building.interfaces {
        let owners = BTreeSet::from([interface.section_a.clone(), interface.section_b.clone()]);
        let visible = options.mode == ScViewMode::Connections
            && visible_for_owners(&options.visible_sections, &owners);
        let anchors = interface_anchors.get(&interface.id).ok_or_else(|| {
            format!(
                "interface {} has no prepared typed anchor set",
                interface.id
            )
        })?;
        let mut triangles = Vec::new();
        for point in anchors {
            let point = point_from_key(*point);
            triangles.extend(octahedron(point, symbol_radius * 1.8)?);
        }
        if triangles.is_empty() {
            status.push(format!(
                "Interface {} has no shared typed node/member anchor; no interface geometry was invented.",
                interface.id.as_str()
            ));
            continue;
        }
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Interface(interface.id.clone()),
            format!(
                "Interface {} · {:?} · anchor markers only",
                interface.id.as_str(),
                interface.status
            ),
            triangles,
            [0.78, 0.30, 0.82, 0.95],
            visible,
            &owners,
            ScSourceIds::default(),
        )?;
    }

    for (support_id, owners) in support_owners {
        let point = *node_points
            .get(&support_id)
            .ok_or_else(|| format!("support marker references missing node {support_id}"))?;
        let triangles = support_marker(point, symbol_radius * 2.2)?;
        let restrained = building
            .nodes
            .get(&support_id)
            .and_then(|node| node.restrained);
        insert_object(
            &mut scene,
            &mut picks,
            &mut by_section,
            &mut object_sources,
            &mut triangle_count,
            ScPick::Support(support_id.clone()),
            format!(
                "Support {} · symbolic restraint marker · dofs={}",
                support_id.as_str(),
                restraint_label(restrained)
            ),
            triangles,
            [0.86, 0.20, 0.16, 0.95],
            visible_for_owners(&options.visible_sections, &owners),
            &owners,
            ScSourceIds::default(),
        )?;
    }

    if !fallback_members.is_empty() {
        fallback_members.sort();
        fallback_members.dedup();
        let examples = fallback_members.iter().take(8).cloned().collect::<Vec<_>>();
        status.push(format!(
            "{} member(s) use illustrative axis geometry because profile.visualization is absent; no analytical profile dimensions were inferred. Examples: {}",
            fallback_members.len(),
            examples.join(", ")
        ));
    }

    add_mode_status(&mut status, building, options.mode);

    Ok(ScRenderScene {
        scene,
        mode: options.mode,
        visible_sections: options.visible_sections.clone(),
        picks,
        by_section,
        overall_bounds: bounds.overall(),
        section_bounds: bounds.section_bounds().clone(),
        object_sources,
        analysis_overlay_object_ids: BTreeSet::new(),
        overlay_snapshot_fingerprint: None,
        overlay_mode: None,
        overlay_bounds: None,
        overlay_section_bounds: BTreeMap::new(),
        building_id: building.id.clone(),
        graph_fingerprint: building.graph_fingerprint.clone(),
        status,
    })
}

fn validate_render_options(options: &ScRenderOptions) -> Result<(), String> {
    if !options.cladding_alpha.is_finite() || !(0.0..=1.0).contains(&options.cladding_alpha) {
        return Err("cladding_alpha must be finite and between 0 and 1".into());
    }
    Ok(())
}

fn validate_render_budgets(building: &GeneratedBuilding) -> Result<(), String> {
    for (name, actual, max) in [
        ("nodes", building.nodes.len(), MAX_RENDER_NODES),
        ("members", building.members.len(), MAX_RENDER_MEMBERS),
        ("surfaces", building.surfaces.len(), MAX_RENDER_SURFACES),
        (
            "interfaces",
            building.interfaces.len(),
            MAX_RENDER_INTERFACES,
        ),
        ("profiles", building.profiles.len(), MAX_RENDER_MEMBERS),
        ("materials", building.materials.len(), MAX_RENDER_MEMBERS),
        (
            "section outlines",
            building.outlines.len(),
            MAX_RENDER_SECTIONS,
        ),
    ] {
        if actual > max {
            return Err(format!(
                "full generated scene has {actual} {name}, exceeding the separate renderer budget {max}; nothing was truncated"
            ));
        }
    }
    Ok(())
}

fn index_profiles<'a>(
    profiles: &'a [MemberProfile],
) -> Result<BTreeMap<structural_configurator::SemanticId, &'a MemberProfile>, String> {
    let mut result = BTreeMap::new();
    for profile in profiles {
        if result.insert(profile.id.clone(), profile).is_some() {
            return Err(format!("duplicate generated profile ID {}", profile.id));
        }
    }
    Ok(result)
}

fn index_material_ids(
    materials: &[structural_configurator::SteelMaterial],
) -> Result<BTreeSet<structural_configurator::SemanticId>, String> {
    let mut result = BTreeSet::new();
    for material in materials {
        if !result.insert(material.id.clone()) {
            return Err(format!("duplicate generated material ID {}", material.id));
        }
    }
    Ok(result)
}

fn add_section_ids(
    section_ids: &mut BTreeSet<structural_configurator::SemanticId>,
    owners: &BTreeSet<structural_configurator::SemanticId>,
) {
    section_ids.extend(owners.iter().cloned());
}

fn checked_point(raw: [f64; 3], context: &str) -> Result<Vec3, String> {
    let point = Vec3::new(raw[0], raw[1], raw[2]);
    if !point.is_finite()
        || point.x.abs() > MAX_COORDINATE_M
        || point.y.abs() > MAX_COORDINATE_M
        || point.z.abs() > MAX_COORDINATE_M
    {
        return Err(format!(
            "{context} has a non-finite coordinate or exceeds the {MAX_COORDINATE_M} m display bound"
        ));
    }
    Ok(point)
}

fn validate_member_axis(member: &GeneratedMember, start: Vec3, end: Vec3) -> Result<Vec3, String> {
    let axis_vector = end - start;
    let axis_length = axis_vector.length();
    if !axis_length.is_finite() || axis_length <= GEOMETRY_EPSILON {
        return Err(format!(
            "member {} has a zero or invalid generated axis",
            member.id
        ));
    }
    if !member.length_m.is_finite() || member.length_m <= 0.0 {
        return Err(format!(
            "member {} has an invalid generated length",
            member.id
        ));
    }
    let tolerance = (axis_length * 1.0e-6).max(1.0e-6);
    if (axis_length - member.length_m).abs() > tolerance {
        return Err(format!(
            "member {} generated length {} m does not match its node coordinates {} m",
            member.id, member.length_m, axis_length
        ));
    }
    if member
        .local_y_reference
        .iter()
        .any(|component| !component.is_finite() || component.abs() > MAX_COORDINATE_M)
    {
        return Err(format!(
            "member {} local_y_reference is non-finite or outside the bounded coordinate scale",
            member.id
        ));
    }
    Ok(axis_vector / axis_length)
}

struct TypedBounds {
    overall: Aabb,
    section_bounds: BTreeMap<structural_configurator::SemanticId, Aabb>,
}

impl TypedBounds {
    fn new() -> Self {
        Self {
            overall: Aabb::empty(),
            section_bounds: BTreeMap::new(),
        }
    }

    fn include(&mut self, point: Vec3, owners: &BTreeSet<structural_configurator::SemanticId>) {
        self.overall.include_point(point);
        for owner in owners {
            self.include_one(point, owner);
        }
    }

    fn include_one(&mut self, point: Vec3, section: &structural_configurator::SemanticId) {
        self.section_bounds
            .entry(section.clone())
            .or_insert_with(Aabb::empty)
            .include_point(point);
    }

    fn ensure_valid(&self) -> Result<(), String> {
        if self.overall.is_empty() {
            return Ok(());
        }
        if !self.overall.min.is_finite() || !self.overall.max.is_finite() {
            return Err("generated typed bounds contain non-finite coordinates".into());
        }
        for (section, bounds) in &self.section_bounds {
            if !bounds.is_empty() && (!bounds.min.is_finite() || !bounds.max.is_finite()) {
                return Err(format!(
                    "generated typed bounds for section {section} contain non-finite coordinates"
                ));
            }
        }
        Ok(())
    }

    fn overall(&self) -> Option<Aabb> {
        if self.overall.is_empty() {
            None
        } else {
            Some(self.overall)
        }
    }

    fn section_bounds(&self) -> &BTreeMap<structural_configurator::SemanticId, Aabb> {
        &self.section_bounds
    }
}

fn profile_rings(
    member: &GeneratedMember,
    start: Vec3,
    end: Vec3,
    visual: &ProfileVisual,
) -> Result<(Vec3, Vec3, Vec<Vec3>, Vec<Vec3>), String> {
    for (label, value) in [
        ("width_m", visual.width_m),
        ("depth_m", visual.depth_m),
        ("web_thickness_m", visual.web_thickness_m),
        ("flange_thickness_m", visual.flange_thickness_m),
    ] {
        if !value.is_finite() || value <= 0.0 || value > MAX_COORDINATE_M {
            return Err(format!(
                "member {} profile visualization {label} is outside the bounded display scale",
                member.id
            ));
        }
    }
    if visual.web_thickness_m >= visual.width_m || 2.0 * visual.flange_thickness_m >= visual.depth_m
    {
        return Err(format!(
            "member {} profile visualization dimensions do not form a valid I-section",
            member.id
        ));
    }
    let axis = validate_member_axis(member, start, end)?;
    let reference = Vec3::new(
        member.local_y_reference[0],
        member.local_y_reference[1],
        member.local_y_reference[2],
    );
    let projected_y = reference - axis * reference.dot(axis);
    if !projected_y.is_finite() || projected_y.length() <= GEOMETRY_EPSILON {
        return Err(format!(
            "member {} local_y_reference is parallel to its axis",
            member.id
        ));
    }
    let local_y = projected_y.normalized();
    let local_x = local_y.cross(axis).normalized();
    if local_x.length() <= GEOMETRY_EPSILON {
        return Err(format!(
            "member {} profile orientation could not form a local frame",
            member.id
        ));
    }
    let half_width = visual.width_m * 0.5;
    let half_depth = visual.depth_m * 0.5;
    let half_web = visual.web_thickness_m * 0.5;
    let flange = visual.flange_thickness_m;
    let profile = [
        [-half_width, -half_depth],
        [half_width, -half_depth],
        [half_width, -half_depth + flange],
        [half_web, -half_depth + flange],
        [half_web, half_depth - flange],
        [half_width, half_depth - flange],
        [half_width, half_depth],
        [-half_width, half_depth],
        [-half_width, half_depth - flange],
        [-half_web, half_depth - flange],
        [-half_web, -half_depth + flange],
        [-half_width, -half_depth + flange],
    ];
    let mut start_ring = Vec::with_capacity(profile.len());
    let mut end_ring = Vec::with_capacity(profile.len());
    for [x, y] in profile {
        start_ring.push(checked_generated_vertex(
            start + local_x * x + local_y * y,
            &format!("member {} display profile", member.id),
        )?);
        end_ring.push(checked_generated_vertex(
            end + local_x * x + local_y * y,
            &format!("member {} display profile", member.id),
        )?);
    }
    Ok((local_x, local_y, start_ring, end_ring))
}

fn checked_generated_vertex(point: Vec3, context: &str) -> Result<Vec3, String> {
    if !point.is_finite()
        || point.x.abs() > MAX_COORDINATE_M
        || point.y.abs() > MAX_COORDINATE_M
        || point.z.abs() > MAX_COORDINATE_M
    {
        return Err(format!(
            "{context} exceeds the finite {MAX_COORDINATE_M} m display bounds"
        ));
    }
    Ok(point)
}

fn profile_prism_mesh(
    member: &GeneratedMember,
    start: Vec3,
    end: Vec3,
    visual: &ProfileVisual,
) -> Result<Vec<Triangle>, String> {
    let (_local_x, _local_y, start_ring, end_ring) = profile_rings(member, start, end, visual)?;
    let local_profile = [
        [-visual.width_m * 0.5, -visual.depth_m * 0.5],
        [visual.width_m * 0.5, -visual.depth_m * 0.5],
        [
            visual.width_m * 0.5,
            -visual.depth_m * 0.5 + visual.flange_thickness_m,
        ],
        [
            visual.web_thickness_m * 0.5,
            -visual.depth_m * 0.5 + visual.flange_thickness_m,
        ],
        [
            visual.web_thickness_m * 0.5,
            visual.depth_m * 0.5 - visual.flange_thickness_m,
        ],
        [
            visual.width_m * 0.5,
            visual.depth_m * 0.5 - visual.flange_thickness_m,
        ],
        [visual.width_m * 0.5, visual.depth_m * 0.5],
        [-visual.width_m * 0.5, visual.depth_m * 0.5],
        [
            -visual.width_m * 0.5,
            visual.depth_m * 0.5 - visual.flange_thickness_m,
        ],
        [
            -visual.web_thickness_m * 0.5,
            visual.depth_m * 0.5 - visual.flange_thickness_m,
        ],
        [
            -visual.web_thickness_m * 0.5,
            -visual.depth_m * 0.5 + visual.flange_thickness_m,
        ],
        [
            -visual.width_m * 0.5,
            -visual.depth_m * 0.5 + visual.flange_thickness_m,
        ],
    ];
    let cap_triangles = triangulate_2d(&local_profile, &format!("member {}", member.id))?;
    let signed_area = signed_area_2d(&local_profile);
    let mut triangles = Vec::with_capacity(2 * local_profile.len() + 2 * cap_triangles.len());
    for index in 0..local_profile.len() {
        let next = (index + 1) % local_profile.len();
        if signed_area > 0.0 {
            push_triangle(
                &mut triangles,
                start_ring[index],
                start_ring[next],
                end_ring[next],
            )?;
            push_triangle(
                &mut triangles,
                start_ring[index],
                end_ring[next],
                end_ring[index],
            )?;
        } else {
            push_triangle(
                &mut triangles,
                start_ring[index],
                end_ring[next],
                start_ring[next],
            )?;
            push_triangle(
                &mut triangles,
                start_ring[index],
                end_ring[index],
                end_ring[next],
            )?;
        }
    }
    for [a, b, c] in cap_triangles {
        if signed_area > 0.0 {
            push_triangle(&mut triangles, start_ring[a], start_ring[c], start_ring[b])?;
            push_triangle(&mut triangles, end_ring[a], end_ring[b], end_ring[c])?;
        } else {
            push_triangle(&mut triangles, start_ring[a], start_ring[b], start_ring[c])?;
            push_triangle(&mut triangles, end_ring[a], end_ring[c], end_ring[b])?;
        }
    }
    Ok(triangles)
}

fn surface_mesh(surface: &GeneratedSurface) -> Result<Vec<Triangle>, String> {
    if surface.vertices_m.len() > MAX_SURFACE_VERTICES {
        return Err(format!(
            "surface {} has {} vertices, exceeding the per-surface display budget {MAX_SURFACE_VERTICES}",
            surface.id,
            surface.vertices_m.len()
        ));
    }
    let vertices = surface
        .vertices_m
        .iter()
        .enumerate()
        .map(|(index, point)| {
            checked_point(*point, &format!("surface {} vertex {index}", surface.id))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let indices = triangulate_surface(&vertices, &format!("surface {}", surface.id))?;
    let mut triangles = Vec::with_capacity(indices.len());
    for [a, b, c] in indices {
        push_triangle(&mut triangles, vertices[a], vertices[b], vertices[c])?;
    }
    Ok(triangles)
}

fn triangulate_surface(vertices: &[Vec3], context: &str) -> Result<Vec<[usize; 3]>, String> {
    if vertices.len() < 3 {
        return Err(format!("{context} has fewer than three typed vertices"));
    }
    let mut normal = Vec3::ZERO;
    let origin = vertices[0];
    for index in 0..vertices.len() {
        let current = vertices[index] - origin;
        let next = vertices[(index + 1) % vertices.len()] - origin;
        normal += current.cross(next);
    }
    let normal_length = normal.length();
    if !normal_length.is_finite() || normal_length <= GEOMETRY_EPSILON {
        return Err(format!(
            "{context} vertices do not form a non-degenerate plane"
        ));
    }
    let unit_normal = normal / normal_length;
    let scale = vertices
        .iter()
        .map(|point| (*point - origin).length())
        .fold(1.0_f64, f64::max);
    let planarity_tolerance = (scale * 1.0e-8).max(1.0e-8);
    if vertices
        .iter()
        .any(|point| (*point - origin).dot(unit_normal).abs() > planarity_tolerance)
    {
        return Err(format!(
            "{context} vertices are not planar within the bounded display tolerance"
        ));
    }
    let absolute = [normal.x.abs(), normal.y.abs(), normal.z.abs()];
    let drop_axis = if absolute[0] >= absolute[1] && absolute[0] >= absolute[2] {
        0
    } else if absolute[1] >= absolute[2] {
        1
    } else {
        2
    };
    let projected: Vec<[f64; 2]> = vertices
        .iter()
        .map(|point| match drop_axis {
            0 => [point.y, point.z],
            1 => [point.x, point.z],
            _ => [point.x, point.y],
        })
        .collect();
    triangulate_2d(&projected, context)
}

fn signed_area_2d(points: &[[f64; 2]]) -> f64 {
    let origin = points[0];
    (0..points.len())
        .map(|index| {
            let current = [points[index][0] - origin[0], points[index][1] - origin[1]];
            let next_point = points[(index + 1) % points.len()];
            let next = [next_point[0] - origin[0], next_point[1] - origin[1]];
            current[0] * next[1] - next[0] * current[1]
        })
        .sum::<f64>()
        * 0.5
}

fn triangulate_2d(points: &[[f64; 2]], context: &str) -> Result<Vec<[usize; 3]>, String> {
    if points.len() < 3 {
        return Err(format!("{context} polygon has fewer than three vertices"));
    }
    let area = signed_area_2d(points);
    if !area.is_finite() || area.abs() <= GEOMETRY_EPSILON {
        return Err(format!(
            "{context} polygon has zero or invalid projected area"
        ));
    }
    let winding = area.signum();
    let origin = points[0];
    let scale = points
        .iter()
        .map(|point| {
            (point[0] - origin[0])
                .abs()
                .max((point[1] - origin[1]).abs())
        })
        .fold(1.0_f64, f64::max);
    let epsilon = (scale * scale * 1.0e-12).max(1.0e-12);
    let mut remaining: Vec<usize> = (0..points.len()).collect();
    let mut result = Vec::with_capacity(points.len() - 2);
    while remaining.len() > 3 {
        let mut clipped = false;
        for current_position in 0..remaining.len() {
            let previous = remaining[(current_position + remaining.len() - 1) % remaining.len()];
            let current = remaining[current_position];
            let next = remaining[(current_position + 1) % remaining.len()];
            let convexity = cross_2d(points[previous], points[current], points[next]) * winding;
            if convexity <= epsilon {
                continue;
            }
            let contains_other = remaining.iter().copied().any(|candidate| {
                candidate != previous
                    && candidate != current
                    && candidate != next
                    && point_in_triangle_2d(
                        points[candidate],
                        points[previous],
                        points[current],
                        points[next],
                        winding,
                        epsilon,
                    )
            });
            if contains_other {
                continue;
            }
            result.push([previous, current, next]);
            remaining.remove(current_position);
            clipped = true;
            break;
        }
        if !clipped {
            // A collinear boundary point can block an ear without changing the
            // polygon. Remove only that redundant index; all source vertices
            // remain in the display bounds.
            let redundant = (0..remaining.len()).find(|position| {
                let previous = remaining[(*position + remaining.len() - 1) % remaining.len()];
                let current = remaining[*position];
                let next = remaining[(*position + 1) % remaining.len()];
                cross_2d(points[previous], points[current], points[next]).abs() <= epsilon
            });
            if let Some(position) = redundant {
                remaining.remove(position);
            } else {
                return Err(format!(
                    "{context} polygon could not be triangulated without inventing geometry"
                ));
            }
        }
    }
    if remaining.len() != 3
        || cross_2d(
            points[remaining[0]],
            points[remaining[1]],
            points[remaining[2]],
        )
        .abs()
            <= epsilon
    {
        return Err(format!("{context} polygon ends in a degenerate triangle"));
    }
    result.push([remaining[0], remaining[1], remaining[2]]);
    Ok(result)
}

fn cross_2d(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn point_in_triangle_2d(
    point: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    winding: f64,
    epsilon: f64,
) -> bool {
    cross_2d(a, b, point) * winding >= -epsilon
        && cross_2d(b, c, point) * winding >= -epsilon
        && cross_2d(c, a, point) * winding >= -epsilon
}

fn cylinder_mesh(
    start: Vec3,
    end: Vec3,
    radius: f64,
    segments: usize,
) -> Result<Vec<Triangle>, String> {
    if !radius.is_finite() || radius <= 0.0 || segments < 3 {
        return Err("display cylinder has invalid radius or segment count".into());
    }
    let axis_vector = end - start;
    let length = axis_vector.length();
    if !length.is_finite() || length <= GEOMETRY_EPSILON {
        return Err("display cylinder has a zero or invalid axis".into());
    }
    let axis = axis_vector / length;
    let reference = if axis.x.abs() < 0.8 { Vec3::X } else { Vec3::Y };
    let u = axis.cross(reference).normalized();
    let v = axis.cross(u).normalized();
    if u.length() <= GEOMETRY_EPSILON || v.length() <= GEOMETRY_EPSILON {
        return Err("display cylinder could not form a finite local frame".into());
    }
    let mut start_ring = Vec::with_capacity(segments);
    let mut end_ring = Vec::with_capacity(segments);
    for index in 0..segments {
        let angle = std::f64::consts::TAU * index as f64 / segments as f64;
        let radial = u * (radius * angle.cos()) + v * (radius * angle.sin());
        start_ring.push(checked_generated_vertex(
            start + radial,
            "illustrative display cylinder",
        )?);
        end_ring.push(checked_generated_vertex(
            end + radial,
            "illustrative display cylinder",
        )?);
    }
    let mut triangles = Vec::with_capacity(segments * 4);
    for index in 0..segments {
        let next = (index + 1) % segments;
        push_triangle(
            &mut triangles,
            start_ring[index],
            start_ring[next],
            end_ring[next],
        )?;
        push_triangle(
            &mut triangles,
            start_ring[index],
            end_ring[next],
            end_ring[index],
        )?;
        push_triangle(&mut triangles, start, start_ring[next], start_ring[index])?;
        push_triangle(&mut triangles, end, end_ring[index], end_ring[next])?;
    }
    Ok(triangles)
}

fn octahedron(center: Vec3, radius: f64) -> Result<Vec<Triangle>, String> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err("display marker has invalid radius".into());
    }
    let vertices = [
        checked_generated_vertex(center + Vec3::X * radius, "display marker")?,
        checked_generated_vertex(center - Vec3::X * radius, "display marker")?,
        checked_generated_vertex(center + Vec3::Y * radius, "display marker")?,
        checked_generated_vertex(center - Vec3::Y * radius, "display marker")?,
        checked_generated_vertex(center + Vec3::Z * radius, "display marker")?,
        checked_generated_vertex(center - Vec3::Z * radius, "display marker")?,
    ];
    let faces = [
        [4, 0, 2],
        [4, 2, 1],
        [4, 1, 3],
        [4, 3, 0],
        [5, 2, 0],
        [5, 1, 2],
        [5, 3, 1],
        [5, 0, 3],
    ];
    let mut triangles = Vec::with_capacity(faces.len());
    for [a, b, c] in faces {
        push_triangle(&mut triangles, vertices[a], vertices[b], vertices[c])?;
    }
    Ok(triangles)
}

fn support_marker(center: Vec3, size: f64) -> Result<Vec<Triangle>, String> {
    if !size.is_finite() || size <= 0.0 {
        return Err("support symbol has invalid display size".into());
    }
    let mut triangles = Vec::new();
    triangles.extend(cylinder_mesh(
        center - Vec3::X * size,
        center + Vec3::X * size,
        size * 0.08,
        6,
    )?);
    triangles.extend(cylinder_mesh(
        center - Vec3::Y * size,
        center + Vec3::Y * size,
        size * 0.08,
        6,
    )?);
    triangles.extend(octahedron(center, size * 0.16)?);
    Ok(triangles)
}

fn bounds_wire_mesh(bounds: Aabb, radius: f64) -> Result<Vec<Triangle>, String> {
    if bounds.is_empty() || !bounds.min.is_finite() || !bounds.max.is_finite() {
        return Err("section display extent is empty or non-finite".into());
    }
    let corners = (0..8).map(|index| bounds.corner(index)).collect::<Vec<_>>();
    let edges = [
        (0, 1),
        (0, 2),
        (0, 4),
        (1, 3),
        (1, 5),
        (2, 3),
        (2, 6),
        (3, 7),
        (4, 5),
        (4, 6),
        (5, 7),
        (6, 7),
    ];
    let mut triangles = Vec::new();
    for (a, b) in edges {
        if (corners[b] - corners[a]).length() > GEOMETRY_EPSILON {
            triangles.extend(cylinder_mesh(corners[a], corners[b], radius, 5)?);
        }
    }
    if triangles.is_empty() {
        triangles = octahedron(bounds.center(), radius * 4.0)?;
    }
    Ok(triangles)
}

fn push_triangle(triangles: &mut Vec<Triangle>, a: Vec3, b: Vec3, c: Vec3) -> Result<(), String> {
    if triangles.len() >= u32::MAX as usize {
        return Err("one rendered mesh exceeds the native triangle ID range".into());
    }
    let id = triangles.len() as u32;
    triangles.push(Triangle::new(id, a, b, c));
    Ok(())
}

fn insert_object(
    scene: &mut Scene,
    picks: &mut BTreeMap<ObjectId, ScPick>,
    by_section: &mut BTreeMap<structural_configurator::SemanticId, BTreeSet<ObjectId>>,
    object_sources: &mut BTreeMap<ObjectId, ScSourceIds>,
    total_triangles: &mut usize,
    pick: ScPick,
    name: String,
    triangles: Vec<Triangle>,
    color: [f32; 4],
    visible: bool,
    owners: &BTreeSet<structural_configurator::SemanticId>,
    sources: ScSourceIds,
) -> Result<ObjectId, String> {
    if triangles.is_empty() {
        return Err(format!("{name} has no display triangles"));
    }
    if picks.len() >= MAX_SCENE_OBJECTS {
        return Err(format!(
            "rendered object count exceeds the separate renderer budget {MAX_SCENE_OBJECTS}; nothing was truncated"
        ));
    }
    *total_triangles = total_triangles
        .checked_add(triangles.len())
        .ok_or_else(|| "rendered triangle count overflowed".to_string())?;
    if *total_triangles > MAX_TOTAL_TRIANGLES {
        return Err(format!(
            "rendered triangle count exceeds the separate renderer budget {MAX_TOTAL_TRIANGLES}; nothing was truncated"
        ));
    }
    let object_id = stable_object_id(&pick);
    if let Some(previous) = picks.get(&object_id) {
        if previous != &pick {
            return Err(format!(
                "stable renderer ObjectId collision between {previous:?} and {pick:?}"
            ));
        }
        return Err(format!(
            "generated semantic object {:?} was projected more than once",
            pick
        ));
    }
    let mesh = Mesh::new(triangles);
    let mut object = SceneObject::new(object_id, name, mesh);
    object.color = color;
    object.visible = visible && color[3] > 0.0;
    // All renderer geometry is a display mesh derived from the typed graph.
    object.exact_geometry = None;
    if scene.insert(object).is_some() {
        return Err(format!(
            "renderer ObjectId {object_id} already exists in the scene"
        ));
    }
    picks.insert(object_id, pick);
    for owner in owners {
        by_section
            .entry(owner.clone())
            .or_default()
            .insert(object_id);
    }
    object_sources.insert(object_id, sources);
    Ok(object_id)
}

fn stable_object_id(pick: &ScPick) -> ObjectId {
    let (namespace, identity) = match pick {
        ScPick::Node(id) => (1_u8, id.as_str().to_owned()),
        ScPick::Member(id) => (2_u8, id.as_str().to_owned()),
        ScPick::Surface(id) => (3_u8, id.as_str().to_owned()),
        ScPick::Section(id) => (4_u8, id.as_str().to_owned()),
        ScPick::Interface(id) => (5_u8, id.as_str().to_owned()),
        ScPick::Support(id) => (6_u8, id.as_str().to_owned()),
        ScPick::Displacement(id) => (7_u8, id.as_str().to_owned()),
        ScPick::DeformedMember(id) => (8_u8, id.as_str().to_owned()),
        ScPick::AppliedLoad {
            load_case_id,
            source_id,
            target,
        } => {
            let target_key = match target {
                ScLoadTarget::Node(id) => format!("N:{}", id.as_str()),
                ScLoadTarget::Member(id) => format!("M:{}", id.as_str()),
            };
            (9_u8, format!("{load_case_id}:{source_id}:{target_key}"))
        }
        ScPick::Reaction(id) => (10_u8, id.as_str().to_owned()),
        ScPick::Foundation(id) => (11_u8, id.as_str().to_owned()),
        ScPick::FoundationCell {
            foundation_id,
            cell_index,
        } => (12_u8, format!("{}:{cell_index}", foundation_id.as_str())),
        ScPick::GroundLayer {
            foundation_id,
            layer_index,
        } => (13_u8, format!("{}:{layer_index}", foundation_id.as_str())),
    };
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in [namespace]
        .into_iter()
        .chain([0_u8])
        .chain(identity.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    const PAYLOAD_MASK: u64 = 0x00ff_ffff_ffff_ffff;
    ((namespace as u64) << 56) | (hash & PAYLOAD_MASK).max(1)
}

fn visible_for_owners(
    visible_sections: &BTreeSet<structural_configurator::SemanticId>,
    owners: &BTreeSet<structural_configurator::SemanticId>,
) -> bool {
    visible_sections.is_empty() || owners.iter().any(|owner| visible_sections.contains(owner))
}

fn member_role_name(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Column => "Column",
        MemberRole::Rafter => "Rafter",
        MemberRole::Beam => "Beam",
        MemberRole::Brace => "Brace",
        MemberRole::Purlin => "Purlin",
        MemberRole::WallGirt => "Wall girt",
    }
}

fn member_color(role: MemberRole, illustrative: bool) -> [f32; 4] {
    if illustrative {
        return [0.92, 0.55, 0.16, 1.0];
    }
    match role {
        MemberRole::Column => [0.24, 0.40, 0.62, 1.0],
        MemberRole::Rafter => [0.22, 0.56, 0.50, 1.0],
        MemberRole::Beam => [0.36, 0.50, 0.72, 1.0],
        MemberRole::Brace => [0.78, 0.42, 0.24, 1.0],
        MemberRole::Purlin => [0.48, 0.58, 0.68, 1.0],
        MemberRole::WallGirt => [0.55, 0.62, 0.68, 1.0],
    }
}

fn surface_role_name(role: SurfaceRole) -> &'static str {
    match role {
        SurfaceRole::RoofFacet => "Roof facet",
        SurfaceRole::WallFace => "Wall face",
    }
}

fn restraint_label(restrained: Option<[bool; 6]>) -> String {
    match restrained {
        Some(dofs) => dofs
            .iter()
            .map(|restrained| if *restrained { '1' } else { '0' })
            .collect(),
        None => "unspecified".into(),
    }
}

fn point_key(point: Vec3) -> [u64; 3] {
    [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()]
}

fn point_from_key(key: [u64; 3]) -> Vec3 {
    Vec3::new(
        f64::from_bits(key[0]),
        f64::from_bits(key[1]),
        f64::from_bits(key[2]),
    )
}

fn initial_status(building: &GeneratedBuilding) -> Vec<String> {
    let mut status = vec![
        format!(
            "Full generated BIM scene: {} nodes, {} members, {} surfaces; rendering is independent of bounded analysis node/member limits.",
            building.nodes.len(),
            building.members.len(),
            building.surfaces.len()
        ),
        format!(
            "Generated building {} · graph fingerprint {}",
            building.id.as_str(),
            building.graph_fingerprint
        ),
        format!(
            "Analysis load scope {:?}: ledger status is a proposal, not a solver application receipt.",
            building.analysis_load_scope
        ),
    ];
    for excluded in &building.analysis_load_ledger.excluded_loads {
        status.push(format!("Excluded from the declared load scope: {excluded}"));
    }
    for limitation in &building.limitations {
        status.push(format!("Generated limitation: {limitation}"));
    }
    for (section, limit) in &building.generation_limits {
        status.push(format!(
            "Section {} generation limit: {limit}",
            section.as_str()
        ));
    }
    status
}

fn add_mode_status(status: &mut Vec<String>, building: &GeneratedBuilding, mode: ScViewMode) {
    match mode {
        ScViewMode::Architectural => status.push(
            "Architectural mode shows generated host surfaces as display geometry; panel layout, load transfer, and structural stiffness are not inferred from surfaces.".into(),
        ),
        ScViewMode::Frame => status.push(
            "Frame mode shows generated members; missing profile visualization uses the separately labeled illustrative axis fallback.".into(),
        ),
        ScViewMode::Connections => {
            status.push(
                "Connection markers use only generated shared nodes/member anchors; marker geometry is symbolic and does not define a connection design.".into(),
            );
            for interface in &building.interfaces {
                status.push(format!(
                    "Interface {} source status: {:?}",
                    interface.id.as_str(),
                    interface.status
                ));
            }
        }
        ScViewMode::Analysis => {
            status.push(format!(
                "ANALYSIS HOLD: generated projection status is {:?}; no source-matched solver result was supplied, so this is reference geometry only.",
                building.analysis_status
            ));
            if let AnalysisProjectionStatus::AnalysisModelCapacityExceeded {
                nodes,
                members,
                max_nodes,
                max_members,
            } = &building.analysis_status
            {
                status.push(format!(
                    "Bounded analysis projection exceeds {max_nodes} nodes / {max_members} members ({nodes} / {members}); the full BIM scene above remains rendered."
                ));
            }
        }
        ScViewMode::Loads => status.push(
            "LOADS HOLD: no source-matched applied-load output was supplied; FrameSelfWeightOnly ledger values remain a proposal and do not include panels, snow, or wind.".into(),
        ),
        ScViewMode::Reactions => status.push(
            "REACTIONS HOLD: support routing is metadata only; no source-matched reaction values were supplied.".into(),
        ),
        ScViewMode::Foundation => status.push(
            "FOUNDATION HOLD: no source-matched foundation geometry or result was supplied; no foundation objects were synthesized.".into(),
        ),
    }
}

const MAX_OVERLAY_OBJECTS: usize = 90_000;
const MAX_OVERLAY_TRIANGLES: usize = 1_000_000;
const MAX_ANALYSIS_VECTOR_ITEMS: usize = 65_536;
const DISPLACEMENT_ARROW_RADIUS_M: f64 = 0.004;
const APPLIED_FORCE_ARROW_METRES_PER_KN: f64 = 0.01;
const MEMBER_LOAD_ARROW_METRES_PER_KN_M: f64 = 0.05;
const REACTION_ARROW_METRES_PER_KN: f64 = 0.01;
const MAX_DISPLAY_SCALE: f64 = 1.0e6;

struct PendingOverlay {
    object_id: ObjectId,
    pick: ScPick,
    name: String,
    triangles: Vec<Triangle>,
    color: [f32; 4],
    visible: bool,
    owners: BTreeSet<structural_configurator::SemanticId>,
    sources: ScSourceIds,
    bounds_points: Vec<Vec3>,
}

/// Validates and overlays a saved, source-bound analysis snapshot at display
/// multiplier 1.0. Callers must still enforce CoreCurrent and stale/dirty state.
pub fn apply_analysis_snapshot(
    scene: &mut ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    mode: ScViewMode,
) -> Result<(), String> {
    apply_analysis_snapshot_with_scale(scene, building, snapshot, mode, 1.0)
}

/// Applies only saved outputs bound to this exact generated graph.
///
/// Geometry remains the full GeneratedBuilding scene. Snapshot results project
/// only snapshot.scope.section_ids; outside sections remain visible as
/// undeformed reference BIM. This function does not call a solver or generator.
pub fn apply_analysis_snapshot_with_scale(
    scene: &mut ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    mode: ScViewMode,
    deformation_scale: f64,
) -> Result<(), String> {
    if !deformation_scale.is_finite()
        || deformation_scale <= 0.0
        || deformation_scale > MAX_DISPLAY_SCALE
    {
        return Err(format!(
            "deformation display multiplier must be finite and in (0, {MAX_DISPLAY_SCALE}]"
        ));
    }
    snapshot
        .validate()
        .map_err(|error| format!("analysis snapshot validation failed: {error}"))?;
    if snapshot.source.generated != *building {
        return Err(
            "analysis snapshot generated source does not exactly match the rendered GeneratedBuilding"
                .into(),
        );
    }
    if scene.building_id != building.id || scene.graph_fingerprint != building.graph_fingerprint {
        return Err(
            "analysis snapshot cannot overlay a scene for another generated building".into(),
        );
    }
    if scene.mode != mode {
        return Err(format!(
            "analysis overlay mode {:?} does not match the current scene mode {:?}; rebuild the presentation first",
            mode, scene.mode
        ));
    }

    let mut pending = Vec::new();
    let mut pending_ids = BTreeSet::new();
    let mut pending_triangles = 0usize;
    let mut status = vec![format!(
        "Validated saved analysis snapshot {}; scoped sections: {}; full generated BIM outside this scope remains reference geometry.",
        snapshot.content_fingerprint,
        snapshot
            .scope
            .section_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )];

    match mode {
        ScViewMode::Analysis => build_deformation_overlay(
            scene,
            building,
            snapshot,
            deformation_scale,
            &mut pending,
            &mut pending_ids,
            &mut pending_triangles,
            &mut status,
        )?,
        ScViewMode::Loads => build_applied_load_overlay(
            scene,
            building,
            snapshot,
            &mut pending,
            &mut pending_ids,
            &mut pending_triangles,
            &mut status,
        )?,
        ScViewMode::Reactions => build_reaction_overlay(
            scene,
            building,
            snapshot,
            &mut pending,
            &mut pending_ids,
            &mut pending_triangles,
            &mut status,
        )?,
        ScViewMode::Foundation => build_foundation_overlay(
            scene,
            building,
            snapshot,
            &mut pending,
            &mut pending_ids,
            &mut pending_triangles,
            &mut status,
        )?,
        _ => status.push(format!(
            "Snapshot validated; {} mode keeps the generated reference geometry without an analysis overlay.",
            mode.name()
        )),
    }

    commit_analysis_overlay(scene, snapshot, mode, pending, status)
}

fn queue_overlay(
    scene: &ScRenderScene,
    pending: &mut Vec<PendingOverlay>,
    pending_ids: &mut BTreeSet<ObjectId>,
    pending_triangles: &mut usize,
    pick: ScPick,
    name: String,
    triangles: Vec<Triangle>,
    color: [f32; 4],
    owners: BTreeSet<structural_configurator::SemanticId>,
    sources: ScSourceIds,
    bounds_points: Vec<Vec3>,
) -> Result<(), String> {
    if triangles.is_empty() {
        return Err(format!(
            "analysis overlay object {name} has no display geometry"
        ));
    }
    if pending.len() >= MAX_OVERLAY_OBJECTS {
        return Err(format!(
            "analysis overlay exceeds its separate object budget {MAX_OVERLAY_OBJECTS}; nothing was truncated"
        ));
    }
    *pending_triangles = pending_triangles
        .checked_add(triangles.len())
        .ok_or_else(|| "analysis overlay triangle count overflowed".to_string())?;
    if *pending_triangles > MAX_OVERLAY_TRIANGLES {
        return Err(format!(
            "analysis overlay exceeds its separate triangle budget {MAX_OVERLAY_TRIANGLES}; nothing was truncated"
        ));
    }
    for triangle in &triangles {
        if triangle.vertices.iter().any(|point| {
            !point.is_finite()
                || point.x.abs() > MAX_COORDINATE_M
                || point.y.abs() > MAX_COORDINATE_M
                || point.z.abs() > MAX_COORDINATE_M
        }) {
            return Err(format!(
                "analysis overlay object {name} contains a non-finite or out-of-bounds vertex"
            ));
        }
    }
    if bounds_points.iter().any(|point| {
        !point.is_finite()
            || point.x.abs() > MAX_COORDINATE_M
            || point.y.abs() > MAX_COORDINATE_M
            || point.z.abs() > MAX_COORDINATE_M
    }) {
        return Err(format!(
            "analysis overlay object {name} has invalid typed bounds points"
        ));
    }
    let object_id = stable_object_id(&pick);
    if scene.picks.contains_key(&object_id)
        && !scene.analysis_overlay_object_ids.contains(&object_id)
    {
        return Err(format!(
            "analysis overlay ObjectId {object_id} collides with existing generated BIM pick {:?}",
            scene.picks.get(&object_id)
        ));
    }
    if !pending_ids.insert(object_id) {
        return Err(format!(
            "analysis overlay stable ObjectId collision at {object_id}; no fallback ID was assigned"
        ));
    }
    let base_object_count = scene
        .picks
        .len()
        .saturating_sub(scene.analysis_overlay_object_ids.len());
    if base_object_count
        .saturating_add(pending.len())
        .saturating_add(1)
        > MAX_SCENE_OBJECTS
    {
        return Err(format!(
            "combined scene exceeds the separate renderer object budget {MAX_SCENE_OBJECTS}; nothing was truncated"
        ));
    }
    pending.push(PendingOverlay {
        object_id,
        pick,
        name,
        triangles,
        color,
        visible: visible_for_owners(&scene.visible_sections, &owners),
        owners,
        sources,
        bounds_points,
    });
    Ok(())
}

fn commit_analysis_overlay(
    scene: &mut ScRenderScene,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    mode: ScViewMode,
    pending: Vec<PendingOverlay>,
    mut status: Vec<String>,
) -> Result<(), String> {
    let remove_ids = scene.analysis_overlay_object_ids.clone();
    for object_id in &remove_ids {
        scene.scene.remove(*object_id);
        scene.picks.remove(object_id);
        scene.object_sources.remove(object_id);
        for object_ids in scene.by_section.values_mut() {
            object_ids.remove(object_id);
        }
    }

    let mut overlay_ids = BTreeSet::new();
    let mut overlay_bounds = Aabb::empty();
    let mut overlay_section_bounds: BTreeMap<structural_configurator::SemanticId, Aabb> =
        BTreeMap::new();
    let has_overlay_objects = !pending.is_empty();
    for item in pending {
        for point in &item.bounds_points {
            overlay_bounds.include_point(*point);
            for owner in &item.owners {
                overlay_section_bounds
                    .entry(owner.clone())
                    .or_insert_with(Aabb::empty)
                    .include_point(*point);
            }
        }
        let mut object = SceneObject::new(item.object_id, item.name, Mesh::new(item.triangles));
        object.color = item.color;
        object.visible = item.visible && item.color[3] > 0.0;
        object.exact_geometry = None;
        if scene.scene.insert(object).is_some() {
            return Err(format!(
                "analysis overlay ObjectId {} already exists after replacement",
                item.object_id
            ));
        }
        scene.picks.insert(item.object_id, item.pick);
        scene.object_sources.insert(item.object_id, item.sources);
        for owner in &item.owners {
            scene
                .by_section
                .entry(owner.clone())
                .or_default()
                .insert(item.object_id);
        }
        overlay_ids.insert(item.object_id);
    }
    scene.analysis_overlay_object_ids = overlay_ids;
    scene.overlay_bounds = if overlay_bounds.is_empty() {
        None
    } else {
        Some(overlay_bounds)
    };
    scene.overlay_section_bounds = overlay_section_bounds
        .into_iter()
        .filter(|(_, bounds)| !bounds.is_empty())
        .collect();
    scene.overlay_snapshot_fingerprint = Some(snapshot.content_fingerprint.clone());
    scene.overlay_mode = Some(mode);

    let hold_prefix = match mode {
        ScViewMode::Analysis => Some("ANALYSIS HOLD:"),
        ScViewMode::Loads => Some("LOADS HOLD:"),
        ScViewMode::Reactions => Some("REACTIONS HOLD:"),
        ScViewMode::Foundation => Some("FOUNDATION HOLD:"),
        _ => None,
    };
    if has_overlay_objects {
        if let Some(prefix) = hold_prefix {
            scene.status.retain(|line| !line.starts_with(prefix));
        }
    }
    scene
        .status
        .retain(|line| !line.starts_with("SNAPSHOT OVERLAY:"));
    status.insert(
        0,
        format!(
            "SNAPSHOT OVERLAY: {:?} · fingerprint {} · {} rendered object(s). No new solver or foundation calculation was run.",
            mode,
            snapshot.content_fingerprint,
            scene.analysis_overlay_object_ids.len()
        ),
    );
    scene.status.extend(status);
    if !has_overlay_objects && hold_prefix.is_some() {
        scene.status.push(format!(
            "{} No source-matched display objects were available for this mode.",
            hold_prefix.unwrap_or("RESULT HOLD:")
        ));
    }
    Ok(())
}

fn owner_set(
    id: &structural_configurator::SemanticId,
) -> BTreeSet<structural_configurator::SemanticId> {
    BTreeSet::from([id.clone()])
}

fn overlay_symbol_radius(scene: &ScRenderScene) -> f64 {
    scene
        .overall_bounds
        .map(|bounds| bounds.extent().max_component().abs())
        .filter(|extent| extent.is_finite() && *extent > GEOMETRY_EPSILON)
        .map(|extent| (extent * 0.002).clamp(0.006, 0.08))
        .unwrap_or(0.02)
}

fn build_deformation_overlay(
    scene: &ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    deformation_scale: f64,
    pending: &mut Vec<PendingOverlay>,
    pending_ids: &mut BTreeSet<ObjectId>,
    pending_triangles: &mut usize,
    status: &mut Vec<String>,
) -> Result<(), String> {
    let result = &snapshot.structural.structural_result;
    if result.nodes.len() > MAX_ANALYSIS_VECTOR_ITEMS
        || result.members.len() > MAX_ANALYSIS_VECTOR_ITEMS
    {
        return Err(format!(
            "saved structural result exceeds the display overlay vector budget {MAX_ANALYSIS_VECTOR_ITEMS}; no deformation geometry was emitted"
        ));
    }
    let nodes_by_dense_id: BTreeMap<u64, _> = snapshot
        .dense_ids
        .nodes
        .iter()
        .map(|(semantic_id, dense_id)| (*dense_id, semantic_id.clone()))
        .collect();
    let members_by_dense_id: BTreeMap<u64, _> = snapshot
        .dense_ids
        .members
        .iter()
        .map(|(semantic_id, dense_id)| (*dense_id, semantic_id.clone()))
        .collect();
    let mut deformed_positions = BTreeMap::new();
    let radius = overlay_symbol_radius(scene);
    let mut displacement_count = 0usize;
    for node_result in &result.nodes {
        if node_result
            .displacement
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(format!(
                "saved displacement for structural node {} is non-finite",
                node_result.node
            ));
        }
        let semantic_id = nodes_by_dense_id
            .get(&node_result.node.get())
            .ok_or_else(|| {
                format!(
                    "saved displacement node {} has no dense-to-semantic identity",
                    node_result.node
                )
            })?;
        let generated_node = building.nodes.get(semantic_id).ok_or_else(|| {
            format!(
                "saved displacement node {} is absent from the full generated BIM graph",
                semantic_id
            )
        })?;
        let source_position = checked_point(
            generated_node.position_m,
            &format!("generated node {}", semantic_id),
        )?;
        let result_position = checked_point(
            node_result.position_m,
            &format!("saved result node {}", semantic_id),
        )?;
        if (source_position - result_position).length() > 1.0e-8 {
            return Err(format!(
                "saved result node {} position differs from its generated semantic node",
                semantic_id
            ));
        }
        let owners = scoped_owners(&generated_node.owners, &snapshot.scope.section_ids);
        if owners.is_empty() {
            continue;
        }
        let displacement = Vec3::new(
            node_result.displacement[0] * deformation_scale,
            node_result.displacement[1] * deformation_scale,
            node_result.displacement[2] * deformation_scale,
        );
        if !displacement.is_finite() {
            return Err(format!(
                "scaled saved displacement for node {} is non-finite",
                semantic_id
            ));
        }
        let deformed = checked_generated_vertex(
            source_position + displacement,
            &format!("deformed node {}", semantic_id),
        )?;
        deformed_positions.insert(semantic_id.clone(), deformed);
        let triangles = if displacement.length() <= GEOMETRY_EPSILON {
            octahedron(source_position, (radius * 0.22).max(0.003))?
        } else {
            arrow_mesh(source_position, deformed, DISPLACEMENT_ARROW_RADIUS_M)?
        };
        queue_overlay(
            scene,
            pending,
            pending_ids,
            pending_triangles,
            ScPick::Displacement(semantic_id.clone()),
            format!(
                "Node displacement {} · [{:.6}, {:.6}, {:.6}] m · rotations [{:.6}, {:.6}, {:.6}] rad · display multiplier ×{deformation_scale}",
                semantic_id.as_str(),
                node_result.displacement[0],
                node_result.displacement[1],
                node_result.displacement[2],
                node_result.displacement[3],
                node_result.displacement[4],
                node_result.displacement[5]
            ),
            triangles,
            [0.94, 0.70, 0.12, 0.94],
            owners,
            ScSourceIds::default(),
            vec![source_position, deformed],
        )?;
        displacement_count += 1;
    }

    let mut member_count = 0usize;
    for member_result in &result.members {
        let semantic_id = members_by_dense_id
            .get(&member_result.member.get())
            .ok_or_else(|| {
                format!(
                    "saved result member {} has no dense-to-semantic identity",
                    member_result.member
                )
            })?;
        let member = building.members.get(semantic_id).ok_or_else(|| {
            format!(
                "saved result member {} is absent from the full generated BIM graph",
                semantic_id
            )
        })?;
        let owners = scoped_owners(&member.owners, &snapshot.scope.section_ids);
        if owners.is_empty() {
            continue;
        }
        let start = *deformed_positions.get(&member.start).ok_or_else(|| {
            format!(
                "deformed member {} has no in-scope start-node displacement",
                semantic_id
            )
        })?;
        let end = *deformed_positions.get(&member.end).ok_or_else(|| {
            format!(
                "deformed member {} has no in-scope end-node displacement",
                semantic_id
            )
        })?;
        let triangles = cylinder_mesh(start, end, (radius * 0.16).max(0.002), 7)?;
        queue_overlay(
            scene,
            pending,
            pending_ids,
            pending_triangles,
            ScPick::DeformedMember(semantic_id.clone()),
            format!(
                "Deformed member axis {} · translation-only overlay · multiplier ×{deformation_scale}",
                semantic_id.as_str()
            ),
            triangles,
            [0.94, 0.38, 0.08, 0.92],
            owners,
            ScSourceIds::default(),
            vec![start, end],
        )?;
        member_count += 1;
    }
    status.push(format!(
        "SCOPED NUMERICAL RESULT: deformation overlay uses {} node displacement(s) and {} member axis(es) from snapshot.scope.section_ids only; multiplier ×{deformation_scale}. The complete BIM outside that scope remains undeformed reference geometry. Member rotations are shown in node labels but are not applied to display profile geometry.",
        displacement_count, member_count
    ));
    status.push(format!(
        "Structural result numerical status {:?}; normative status {:?}; engineering check records {:?} remain separate holds.",
        result.diagnostics.status,
        result.normative_status,
        snapshot.structural.check_records
    ));
    for limitation in &result.limitations {
        status.push(format!("Structural result limitation: {limitation}"));
    }
    Ok(())
}

fn scoped_owners(
    owners: &BTreeSet<structural_configurator::SemanticId>,
    scope: &BTreeSet<structural_configurator::SemanticId>,
) -> BTreeSet<structural_configurator::SemanticId> {
    owners.intersection(scope).cloned().collect()
}

fn arrow_mesh(start: Vec3, end: Vec3, shaft_radius: f64) -> Result<Vec<Triangle>, String> {
    let direction = end - start;
    let length = direction.length();
    if !length.is_finite() {
        return Err("display arrow has a non-finite length".into());
    }
    if length <= GEOMETRY_EPSILON {
        return octahedron(start, (shaft_radius * 2.0).max(0.003));
    }
    let unit = direction / length;
    let head_length = (length * 0.22).min(0.25).max(shaft_radius * 4.0);
    if head_length >= length * 0.9 {
        return cone_mesh(start, end, (shaft_radius * 2.4).max(0.006), 8);
    }
    let shoulder = end - unit * head_length;
    let mut triangles = cylinder_mesh(start, shoulder, shaft_radius, 7)?;
    triangles.extend(cone_mesh(
        shoulder,
        end,
        (shaft_radius * 2.4).max(0.006),
        8,
    )?);
    Ok(triangles)
}

fn cone_mesh(base: Vec3, tip: Vec3, radius: f64, segments: usize) -> Result<Vec<Triangle>, String> {
    let axis_vector = tip - base;
    let length = axis_vector.length();
    if !length.is_finite() || length <= GEOMETRY_EPSILON {
        return octahedron(tip, radius.max(0.003));
    }
    let axis = axis_vector / length;
    let reference = if axis.x.abs() < 0.8 { Vec3::X } else { Vec3::Y };
    let u = axis.cross(reference).normalized();
    let v = axis.cross(u).normalized();
    let mut ring = Vec::with_capacity(segments);
    for index in 0..segments {
        let angle = std::f64::consts::TAU * index as f64 / segments as f64;
        ring.push(checked_generated_vertex(
            base + u * (radius * angle.cos()) + v * (radius * angle.sin()),
            "analysis display arrow head",
        )?);
    }
    let mut triangles = Vec::with_capacity(segments * 2);
    for index in 0..segments {
        let next = (index + 1) % segments;
        push_triangle(&mut triangles, tip, ring[index], ring[next])?;
        push_triangle(&mut triangles, base, ring[next], ring[index])?;
    }
    Ok(triangles)
}

fn selected_load_cases<'a>(
    snapshot: &'a ai_revit_core::ConfiguratorAnalysisSnapshot,
) -> Result<Vec<(&'a structural_core::StructuralLoadCase, f64)>, String> {
    let model = &snapshot.structural.input.definition.structural_model;
    let target = snapshot.structural.structural_result.target;
    let mut coefficients: BTreeMap<structural_core::LoadCaseId, f64> = BTreeMap::new();
    match target {
        structural_core::AnalysisTarget::Case(case_id) => {
            coefficients.insert(case_id, 1.0);
        }
        structural_core::AnalysisTarget::Combination(combination_id) => {
            let combination = model
                .combinations
                .iter()
                .find(|combination| combination.id == combination_id)
                .ok_or_else(|| {
                    format!(
                        "saved analysis target combination {combination_id} is absent from its structural input"
                    )
                })?;
            for term in &combination.terms {
                let coefficient = coefficients.entry(term.case).or_insert(0.0);
                *coefficient += term.coefficient;
                if !coefficient.is_finite() {
                    return Err(format!(
                        "saved load combination {} has a non-finite summed case coefficient",
                        combination_id
                    ));
                }
            }
        }
    }
    coefficients
        .into_iter()
        .filter(|(_, coefficient)| *coefficient != 0.0)
        .map(|(case_id, coefficient)| {
            let case = model
                .load_cases
                .iter()
                .find(|case| case.id == case_id)
                .ok_or_else(|| format!("saved target references missing load case {case_id}"))?;
            Ok((case, coefficient))
        })
        .collect()
}

fn receipt_coefficient_sum(
    result: &structural_core::StructuralAnalysisResult,
    case_id: structural_core::LoadCaseId,
    source_id: &str,
    member: Option<structural_core::StructuralMemberId>,
) -> Option<f64> {
    let mut found = false;
    let mut total = 0.0;
    for receipt in result.load_sources.iter().filter(|receipt| {
        receipt.case == case_id && receipt.source_id == source_id && receipt.member == member
    }) {
        found = true;
        total += receipt.coefficient;
    }
    if found {
        Some(total)
    } else {
        None
    }
}

fn receipt_matches_coefficient(received: Option<f64>, expected: f64) -> bool {
    received.is_some_and(|received| {
        received.is_finite()
            && (received - expected).abs() <= 1.0e-10 * received.abs().max(expected.abs()).max(1.0)
    })
}

fn build_applied_load_overlay(
    scene: &ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    pending: &mut Vec<PendingOverlay>,
    pending_ids: &mut BTreeSet<ObjectId>,
    pending_triangles: &mut usize,
    status: &mut Vec<String>,
) -> Result<(), String> {
    let model = &snapshot.structural.input.definition.structural_model;
    let result = &snapshot.structural.structural_result;
    if model.load_cases.len() > MAX_ANALYSIS_VECTOR_ITEMS
        || result.load_sources.len() > MAX_ANALYSIS_VECTOR_ITEMS
    {
        return Err(format!(
            "saved load source vectors exceed the display overlay budget {MAX_ANALYSIS_VECTOR_ITEMS}; no load geometry was emitted"
        ));
    }
    let nodes_by_dense_id: BTreeMap<u64, _> = snapshot
        .dense_ids
        .nodes
        .iter()
        .map(|(semantic_id, dense_id)| (*dense_id, semantic_id.clone()))
        .collect();
    let members_by_dense_id: BTreeMap<u64, _> = snapshot
        .dense_ids
        .members
        .iter()
        .map(|(semantic_id, dense_id)| (*dense_id, semantic_id.clone()))
        .collect();
    let cases = selected_load_cases(snapshot)?;
    let mut source_objects = 0usize;

    for (case, coefficient) in &cases {
        for load in &case.nodal_loads {
            let receipt_sum = receipt_coefficient_sum(result, case.id, &load.source_id, None);
            if !receipt_matches_coefficient(receipt_sum, *coefficient) {
                return Err(format!(
                    "nodal load source {} in case {} does not match saved solver load-source receipts",
                    load.source_id, case.id
                ));
            }
            let semantic_id = nodes_by_dense_id.get(&load.node.get()).ok_or_else(|| {
                format!(
                    "nodal load source {} node {} has no dense-to-semantic identity",
                    load.source_id, load.node
                )
            })?;
            let node = building.nodes.get(semantic_id).ok_or_else(|| {
                format!("nodal load source node {semantic_id} is absent from GeneratedBuilding")
            })?;
            let owners = scoped_owners(&node.owners, &snapshot.scope.section_ids);
            if owners.is_empty() {
                continue;
            }
            if load.force_moment.iter().any(|value| !value.is_finite()) {
                return Err(format!(
                    "nodal load source {} has non-finite values",
                    load.source_id
                ));
            }
            let force = Vec3::new(
                load.force_moment[0] * *coefficient,
                load.force_moment[1] * *coefficient,
                load.force_moment[2] * *coefficient,
            );
            let position = checked_point(node.position_m, &format!("load node {semantic_id}"))?;
            let arrow_end = checked_generated_vertex(
                position + force * APPLIED_FORCE_ARROW_METRES_PER_KN,
                "applied nodal load display arrow",
            )?;
            let has_moment = load.force_moment[3..].iter().any(|value| *value != 0.0);
            let has_force = force.length() > GEOMETRY_EPSILON;
            if !has_force && !has_moment {
                status.push(format!(
                    "Zero nodal load source {} at node {} was retained in the source model and has no display glyph.",
                    load.source_id, semantic_id
                ));
                continue;
            }
            let triangles = if has_force {
                arrow_mesh(position, arrow_end, 0.008)?
            } else {
                octahedron(position, overlay_symbol_radius(scene) * 0.5)?
            };
            let mut sources = ScSourceIds::default();
            sources
                .applied_load_source_ids
                .insert(load.source_id.clone());
            queue_overlay(
                scene,
                pending,
                pending_ids,
                pending_triangles,
                ScPick::AppliedLoad {
                    load_case_id: case.id.get(),
                    source_id: load.source_id.clone(),
                    target: ScLoadTarget::Node(semantic_id.clone()),
                },
                format!(
                    "Applied nodal load {} · case {} · node {} · coefficient {:.6} · F=[{:.6},{:.6},{:.6}] kN · M=[{:.6},{:.6},{:.6}] kN m · arrow scale {:.4} m/kN",
                    load.source_id,
                    case.name,
                    semantic_id.as_str(),
                    coefficient,
                    load.force_moment[0] * *coefficient,
                    load.force_moment[1] * *coefficient,
                    load.force_moment[2] * *coefficient,
                    load.force_moment[3] * *coefficient,
                    load.force_moment[4] * *coefficient,
                    load.force_moment[5] * *coefficient,
                    APPLIED_FORCE_ARROW_METRES_PER_KN
                ),
                triangles,
                [0.12, 0.78, 0.92, 0.95],
                owners,
                sources,
                vec![position, arrow_end],
            )?;
            source_objects += 1;
        }

        for load in &case.uniform_member_loads {
            let receipt_sum =
                receipt_coefficient_sum(result, case.id, &load.source_id, Some(load.member));
            if !receipt_matches_coefficient(receipt_sum, *coefficient) {
                return Err(format!(
                    "member load source {} in case {} does not match saved solver load-source receipts",
                    load.source_id, case.id
                ));
            }
            let semantic_id = members_by_dense_id.get(&load.member.get()).ok_or_else(|| {
                format!(
                    "member load source {} member {} has no dense-to-semantic identity",
                    load.source_id, load.member
                )
            })?;
            let member = building.members.get(semantic_id).ok_or_else(|| {
                format!("loaded member {semantic_id} is absent from GeneratedBuilding")
            })?;
            let owners = scoped_owners(&member.owners, &snapshot.scope.section_ids);
            if owners.is_empty() {
                continue;
            }
            if load.local_force_kn_m.iter().any(|value| !value.is_finite()) {
                return Err(format!(
                    "member load source {} has non-finite values",
                    load.source_id
                ));
            }
            let member_result = result
                .members
                .iter()
                .find(|member_result| member_result.member == load.member)
                .ok_or_else(|| {
                    format!(
                        "member load source {} has no saved member-axis result",
                        load.source_id
                    )
                })?;
            let local_force = [
                load.local_force_kn_m[0] * *coefficient,
                load.local_force_kn_m[1] * *coefficient,
                load.local_force_kn_m[2] * *coefficient,
            ];
            let global_force = local_to_global(member_result.local_axes, local_force);
            if !global_force.is_finite() {
                return Err(format!(
                    "member load source {} transformed to a non-finite display vector",
                    load.source_id
                ));
            }
            let start_node = building
                .nodes
                .get(&member.start)
                .ok_or_else(|| format!("loaded member {} has a missing start node", semantic_id))?;
            let end_node = building
                .nodes
                .get(&member.end)
                .ok_or_else(|| format!("loaded member {} has a missing end node", semantic_id))?;
            let start = checked_point(start_node.position_m, "member load start")?;
            let end = checked_point(end_node.position_m, "member load end")?;
            let mut triangles = Vec::new();
            let mut bounds_points = Vec::new();
            for fraction in [0.25, 0.5, 0.75] {
                let point = start * (1.0 - fraction) + end * fraction;
                let arrow_end = checked_generated_vertex(
                    point + global_force * MEMBER_LOAD_ARROW_METRES_PER_KN_M,
                    "uniform member load display arrow",
                )?;
                triangles.extend(arrow_mesh(point, arrow_end, 0.006)?);
                bounds_points.push(point);
                bounds_points.push(arrow_end);
            }
            let mut sources = ScSourceIds::default();
            sources
                .applied_load_source_ids
                .insert(load.source_id.clone());
            queue_overlay(
                scene,
                pending,
                pending_ids,
                pending_triangles,
                ScPick::AppliedLoad {
                    load_case_id: case.id.get(),
                    source_id: load.source_id.clone(),
                    target: ScLoadTarget::Member(semantic_id.clone()),
                },
                format!(
                    "Applied uniform member load {} · case {} · member {} · coefficient {:.6} · local q=[{:.6},{:.6},{:.6}] kN/m · display scale {:.4} m/(kN/m)",
                    load.source_id,
                    case.name,
                    semantic_id.as_str(),
                    coefficient,
                    load.local_force_kn_m[0] * *coefficient,
                    load.local_force_kn_m[1] * *coefficient,
                    load.local_force_kn_m[2] * *coefficient,
                    MEMBER_LOAD_ARROW_METRES_PER_KN_M
                ),
                triangles,
                [0.12, 0.78, 0.92, 0.95],
                owners,
                sources,
                bounds_points,
            )?;
            source_objects += 1;
        }

        for self_weight in &case.self_weight {
            for member_id in &self_weight.members {
                let receipt_sum = receipt_coefficient_sum(
                    result,
                    case.id,
                    &self_weight.source_id,
                    Some(*member_id),
                );
                if !receipt_matches_coefficient(receipt_sum, *coefficient) {
                    return Err(format!(
                        "self-weight source {} member {} does not match saved solver load-source receipts",
                        self_weight.source_id, member_id
                    ));
                }
                let semantic_id = members_by_dense_id.get(&member_id.get()).ok_or_else(|| {
                    format!(
                        "self-weight source {} member {} has no dense-to-semantic identity",
                        self_weight.source_id, member_id
                    )
                })?;
                let member = building.members.get(semantic_id).ok_or_else(|| {
                    format!("self-weight member {semantic_id} is absent from GeneratedBuilding")
                })?;
                let owners = scoped_owners(&member.owners, &snapshot.scope.section_ids);
                if owners.is_empty() {
                    continue;
                }
                let start = building.nodes.get(&member.start).ok_or_else(|| {
                    format!(
                        "self-weight member {} has a missing start node",
                        semantic_id
                    )
                })?;
                let end = building.nodes.get(&member.end).ok_or_else(|| {
                    format!("self-weight member {} has a missing end node", semantic_id)
                })?;
                let start = checked_point(start.position_m, "self-weight member start")?;
                let end = checked_point(end.position_m, "self-weight member end")?;
                let middle = (start + end) * 0.5;
                let result_member = result
                    .members
                    .iter()
                    .find(|result_member| result_member.member == *member_id)
                    .ok_or_else(|| {
                        format!(
                            "self-weight member {} has no saved result record",
                            semantic_id
                        )
                    })?;
                let member_receipts: Vec<_> = result
                    .load_sources
                    .iter()
                    .filter(|receipt| receipt.member == Some(*member_id))
                    .collect();
                let self_weight_only_for_member = member_receipts.len() == 1
                    && member_receipts[0].source_id == self_weight.source_id
                    && member_receipts[0].kind == "body_self_weight_global_minus_z";
                let triangles = if self_weight_only_for_member {
                    let global_local_load = local_to_global(
                        result_member.local_axes,
                        result_member.local_uniform_load_kn_m,
                    );
                    let arrow_end = checked_generated_vertex(
                        middle + global_local_load * MEMBER_LOAD_ARROW_METRES_PER_KN_M,
                        "saved self-weight result display arrow",
                    )?;
                    if global_local_load.length() > GEOMETRY_EPSILON {
                        arrow_mesh(middle, arrow_end, 0.006)?
                    } else {
                        octahedron(middle, overlay_symbol_radius(scene) * 0.45)?
                    }
                } else {
                    // SelfWeightLoad declares members and source identity but
                    // no magnitude. Keep a coverage marker when other target
                    // sources prevent isolating the solver's combined result.
                    octahedron(middle, overlay_symbol_radius(scene) * 0.45)?
                };
                let mut sources = ScSourceIds::default();
                sources
                    .applied_load_source_ids
                    .insert(self_weight.source_id.clone());
                let bounds_points = if self_weight_only_for_member {
                    let global_local_load = local_to_global(
                        result_member.local_axes,
                        result_member.local_uniform_load_kn_m,
                    );
                    let arrow_end = checked_generated_vertex(
                        middle + global_local_load * MEMBER_LOAD_ARROW_METRES_PER_KN_M,
                        "saved self-weight result bounds",
                    )?;
                    vec![middle, arrow_end]
                } else {
                    vec![middle]
                };
                queue_overlay(
                    scene,
                    pending,
                    pending_ids,
                    pending_triangles,
                    ScPick::AppliedLoad {
                        load_case_id: case.id.get(),
                        source_id: self_weight.source_id.clone(),
                        target: ScLoadTarget::Member(semantic_id.clone()),
                    },
                    format!(
                        "Applied self-weight source {} · case {} · member {} · coefficient {:.6} · source receipts matched; arrow uses saved target member load only when this is its sole receipt",
                        self_weight.source_id,
                        case.name,
                        semantic_id.as_str(),
                        coefficient
                    ),
                    triangles,
                    [0.12, 0.78, 0.92, 0.95],
                    owners,
                    sources,
                    bounds_points,
                )?;
                source_objects += 1;
            }
        }
    }

    if snapshot.source.generated.analysis_load_scope
        == structural_configurator::AnalysisLoadScope::FrameSelfWeightOnly
    {
        status.push(format!(
            "SCOPED LOAD RESULT: source IDs and values are read from the saved structural input and matched solver receipts for sections [{}]. FrameSelfWeightOnly excludes panel/cladding loads, snow, and wind; all generated surfaces remain BIM reference geometry and provide no stiffness/load transfer.",
            snapshot
                .scope
                .section_ids
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else {
        status.push(format!(
            "SCOPED LOAD RESULT: saved target loads are shown for sections [{}]; this load scope is {:?}.",
            snapshot
                .scope
                .section_ids
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            snapshot.source.generated.analysis_load_scope
        ));
    }
    status.push(format!(
        "Load display arrow scales are presentation factors ({APPLIED_FORCE_ARROW_METRES_PER_KN} m/kN for nodal force; {MEMBER_LOAD_ARROW_METRES_PER_KN_M} m/(kN/m) for uniform member load). Exact source values and units remain in object labels; no values are derived from mesh geometry."
    ));
    if source_objects == 0 {
        status.push(
            "The validated saved target contains no nonzero in-scope source loads with matching receipts; no load arrows or markers were created.".into(),
        );
    }
    Ok(())
}

fn local_to_global(axes: [[f64; 3]; 3], local: [f64; 3]) -> Vec3 {
    Vec3::new(
        axes[0][0] * local[0] + axes[1][0] * local[1] + axes[2][0] * local[2],
        axes[0][1] * local[0] + axes[1][1] * local[1] + axes[2][1] * local[2],
        axes[0][2] * local[0] + axes[1][2] * local[1] + axes[2][2] * local[2],
    )
}

fn build_reaction_overlay(
    scene: &ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    pending: &mut Vec<PendingOverlay>,
    pending_ids: &mut BTreeSet<ObjectId>,
    pending_triangles: &mut usize,
    status: &mut Vec<String>,
) -> Result<(), String> {
    if snapshot.reactions.reactions.len() > MAX_ANALYSIS_VECTOR_ITEMS {
        return Err(format!(
            "saved support reaction ledger exceeds the display overlay budget {MAX_ANALYSIS_VECTOR_ITEMS}; no reaction glyphs were emitted"
        ));
    }
    let mut rendered = 0usize;
    for reaction in &snapshot.reactions.reactions {
        if reaction.force_moment.iter().any(|value| !value.is_finite()) {
            return Err(format!(
                "saved support reaction {} has non-finite components",
                reaction.support_node_id
            ));
        }
        let node = building
            .nodes
            .get(&reaction.support_node_id)
            .ok_or_else(|| {
                format!(
                    "saved support reaction {} has no generated semantic node",
                    reaction.support_node_id
                )
            })?;
        let position = checked_point(
            reaction.position_m,
            &format!("saved reaction {} position", reaction.support_node_id),
        )?;
        let generated_position = checked_point(
            node.position_m,
            &format!("generated support node {}", reaction.support_node_id),
        )?;
        if (position - generated_position).length() > 1.0e-8 {
            return Err(format!(
                "saved reaction {} position differs from its generated support node",
                reaction.support_node_id
            ));
        }
        let owners = scoped_owners(&reaction.section_owners, &snapshot.scope.section_ids);
        if owners.is_empty() {
            continue;
        }
        let force = Vec3::new(
            reaction.force_moment[0],
            reaction.force_moment[1],
            reaction.force_moment[2],
        );
        let arrow_end = checked_generated_vertex(
            position + force * REACTION_ARROW_METRES_PER_KN,
            "saved support reaction display arrow",
        )?;
        let has_force = force.length() > GEOMETRY_EPSILON;
        let has_moment = reaction.force_moment[3..].iter().any(|value| *value != 0.0);
        let triangles = if has_force {
            arrow_mesh(position, arrow_end, 0.009)?
        } else {
            octahedron(position, overlay_symbol_radius(scene) * 0.55)?
        };
        let ownership = match &reaction.ownership {
            ai_revit_core::ConfiguredReactionOwnership::Exclusive { section_id } => {
                format!("exclusive to section {}", section_id.as_str())
            }
            ai_revit_core::ConfiguredReactionOwnership::SharedUnallocated => {
                "shared and unallocated; rendered once at the physical support".into()
            }
        };
        let foundation_label = reaction
            .foundation_group_id
            .as_ref()
            .map(|id| id.as_str())
            .unwrap_or("none");
        let mut sources = ScSourceIds::default();
        sources.foundation_id = reaction.foundation_group_id.clone();
        queue_overlay(
            scene,
            pending,
            pending_ids,
            pending_triangles,
            ScPick::Reaction(reaction.support_node_id.clone()),
            format!(
                "Saved support reaction {} · {} · foundation group {} · F=[{:.6},{:.6},{:.6}] kN · M=[{:.6},{:.6},{:.6}] kN m · {} · arrow scale {:.4} m/kN",
                reaction.support_node_id.as_str(),
                ownership,
                foundation_label,
                reaction.force_moment[0],
                reaction.force_moment[1],
                reaction.force_moment[2],
                reaction.force_moment[3],
                reaction.force_moment[4],
                reaction.force_moment[5],
                if has_moment {
                    "moments retained numerically; marker has no torque scale"
                } else {
                    "saved reaction vector"
                },
                REACTION_ARROW_METRES_PER_KN
            ),
            triangles,
            [0.88, 0.20, 0.72, 0.96],
            owners,
            sources,
            vec![position, arrow_end],
        )?;
        rendered += 1;
    }
    status.push(format!(
        "SCOPED REACTION RESULT: {} physical support reaction(s) rendered once from the validated snapshot ledger for sections [{}]. SharedUnallocated reactions are not copied or apportioned across owners.",
        rendered,
        snapshot
            .scope
            .section_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    status.push(format!(
        "Saved support-on-structure totals [Fx,Fy,Fz,Mx,My,Mz]: global {:?}; shared unallocated {:?}. Units are kN and kN m as typed by Core.",
        snapshot.reactions.global_total,
        snapshot.reactions.shared_unallocated_total
    ));
    if rendered == 0 {
        status.push(
            "The validated snapshot has no in-scope support reaction rows; no reaction glyph was created.".into(),
        );
    }
    Ok(())
}

fn build_foundation_overlay(
    scene: &ScRenderScene,
    building: &GeneratedBuilding,
    snapshot: &ai_revit_core::ConfiguratorAnalysisSnapshot,
    pending: &mut Vec<PendingOverlay>,
    pending_ids: &mut BTreeSet<ObjectId>,
    pending_triangles: &mut usize,
    status: &mut Vec<String>,
) -> Result<(), String> {
    if snapshot.foundations.len() > MAX_ANALYSIS_VECTOR_ITEMS {
        return Err(format!(
            "saved foundation group results exceed the display overlay budget {MAX_ANALYSIS_VECTOR_ITEMS}; no foundation geometry was emitted"
        ));
    }
    let mut computed_groups = 0usize;
    let mut held_groups = 0usize;
    for calculation in &snapshot.foundations {
        let Some(group) = snapshot
            .source
            .foundation_groups
            .get(&calculation.foundation_id)
        else {
            return Err(format!(
                "saved foundation result {} has no source foundation group",
                calculation.foundation_id
            ));
        };
        if group.foundation_id != calculation.foundation_id
            || group.input.foundation_id != calculation.foundation_id
        {
            return Err(format!(
                "foundation result {} differs from its source assignment group identity",
                calculation.foundation_id
            ));
        }
        let in_scope_owners = scoped_owners(&group.member_section_ids, &snapshot.scope.section_ids);
        if calculation.status != ai_revit_core::ConfiguredFoundationStatus::Computed {
            held_groups += 1;
            status.push(format!(
                "FOUNDATION GROUP HOLD {}: source status {:?}; no raft, soil, or pressure geometry was emitted for this group. Failure: {:?}",
                calculation.foundation_id,
                calculation.status,
                calculation.failure
            ));
            continue;
        }
        if calculation.failure.is_some() {
            held_groups += 1;
            status.push(format!(
                "FOUNDATION GROUP HOLD {}: computed status carries a failure record; no foundation output was emitted.",
                calculation.foundation_id
            ));
            continue;
        }
        let effective_input = calculation.effective_input.as_ref().ok_or_else(|| {
            format!(
                "computed foundation {} has no saved effective input",
                calculation.foundation_id
            )
        })?;
        let result = calculation.result.as_ref().ok_or_else(|| {
            format!(
                "computed foundation {} has no saved result",
                calculation.foundation_id
            )
        })?;
        if effective_input.foundation_id != calculation.foundation_id
            || result.foundation_id != calculation.foundation_id
        {
            return Err(format!(
                "computed foundation {} input/result IDs do not match its group",
                calculation.foundation_id
            ));
        }
        if result.cells.len() > foundation_core::MAX_MESH_CELLS
            || result.cells.len() != result.mesh.cell_count
            || result.cells.len() > MAX_ANALYSIS_VECTOR_ITEMS
        {
            return Err(format!(
                "foundation {} pressure-cell vector is outside its typed result/display bounds",
                calculation.foundation_id
            ));
        }
        if effective_input.soil.layers.len() > 256 {
            return Err(format!(
                "foundation {} soil-layer count exceeds the display bound 256",
                calculation.foundation_id
            ));
        }
        if in_scope_owners.is_empty() {
            held_groups += 1;
            status.push(format!(
                "FOUNDATION GROUP HOLD {}: group has no section inside the saved result scope; no foundation geometry was emitted.",
                calculation.foundation_id
            ));
            continue;
        }
        let reference = checked_point(
            group.reference_point_m,
            &format!("foundation {} reference point", calculation.foundation_id),
        )?;
        let geometry = &effective_input.geometry;
        validate_foundation_geometry(geometry, &calculation.foundation_id)?;
        let base_z = reference.z;
        let mat_top_z = base_z + geometry.thickness_m;
        let (mat_mesh, mat_points) = cuboid_mesh(
            reference.x - geometry.width_x_m * 0.5,
            reference.x + geometry.width_x_m * 0.5,
            reference.y - geometry.length_y_m * 0.5,
            reference.y + geometry.length_y_m * 0.5,
            base_z,
            mat_top_z,
            &format!("foundation {} raft", calculation.foundation_id),
        )?;
        let mut mat_sources = ScSourceIds::default();
        mat_sources.foundation_id = Some(calculation.foundation_id.clone());
        queue_overlay(
            scene,
            pending,
            pending_ids,
            pending_triangles,
            ScPick::Foundation(calculation.foundation_id.clone()),
            format!(
                "Saved computed raft {} · effective input geometry {:.3} m × {:.3} m × {:.3} m · reference point is raft bottom centre",
                calculation.foundation_id,
                geometry.width_x_m,
                geometry.length_y_m,
                geometry.thickness_m
            ),
            mat_mesh,
            [0.42, 0.52, 0.62, 0.88],
            in_scope_owners.clone(),
            mat_sources,
            mat_points,
        )?;

        for (layer_index, layer) in effective_input.soil.layers.iter().enumerate() {
            if !layer.depth_top_m.is_finite()
                || !layer.visualization_bottom_depth_m.is_finite()
                || layer.depth_top_m < 0.0
                || layer.visualization_bottom_depth_m <= layer.depth_top_m
            {
                return Err(format!(
                    "foundation {} soil layer {layer_index} has invalid typed visualization depths",
                    calculation.foundation_id
                ));
            }
            let layer_top_z = base_z - layer.depth_top_m;
            let layer_bottom_z = base_z - layer.visualization_bottom_depth_m;
            let (layer_mesh, layer_points) = cuboid_mesh(
                reference.x - geometry.width_x_m * 0.5,
                reference.x + geometry.width_x_m * 0.5,
                reference.y - geometry.length_y_m * 0.5,
                reference.y + geometry.length_y_m * 0.5,
                layer_bottom_z,
                layer_top_z,
                &format!(
                    "foundation {} soil layer {layer_index}",
                    calculation.foundation_id
                ),
            )?;
            let mut layer_sources = ScSourceIds::default();
            layer_sources.foundation_id = Some(calculation.foundation_id.clone());
            let color = if layer_index % 2 == 0 {
                [0.48, 0.34, 0.22, 0.32]
            } else {
                [0.60, 0.45, 0.29, 0.32]
            };
            queue_overlay(
                scene,
                pending,
                pending_ids,
                pending_triangles,
                ScPick::GroundLayer {
                    foundation_id: calculation.foundation_id.clone(),
                    layer_index,
                },
                format!(
                    "Soil layer {} · {} · display depth {:.3}..{:.3} m below raft base · {:?} · site verified={}",
                    layer.name,
                    calculation.foundation_id,
                    layer.depth_top_m,
                    layer.visualization_bottom_depth_m,
                    layer.source_type,
                    layer.engineering_site_verified
                ),
                layer_mesh,
                color,
                in_scope_owners.clone(),
                layer_sources,
                layer_points,
            )?;
        }

        if result.cells.is_empty() {
            status.push(format!(
                "FOUNDATION GROUP HOLD {}: computed result contains no pressure cells; raft and typed soil context are shown without a pressure field.",
                calculation.foundation_id
            ));
        } else {
            if !result.mesh.dx_m.is_finite()
                || !result.mesh.dy_m.is_finite()
                || result.mesh.dx_m <= 0.0
                || result.mesh.dy_m <= 0.0
            {
                return Err(format!(
                    "foundation {} saved pressure mesh has invalid cell dimensions",
                    calculation.foundation_id
                ));
            }
            let min_q = result
                .cells
                .iter()
                .map(|cell| cell.q_kpa)
                .fold(f64::INFINITY, f64::min);
            let max_q = result
                .cells
                .iter()
                .map(|cell| cell.q_kpa)
                .fold(f64::NEG_INFINITY, f64::max);
            if !min_q.is_finite()
                || !max_q.is_finite()
                || result.cells.iter().any(|cell| {
                    !cell.q_kpa.is_finite() || !cell.x_m.is_finite() || !cell.y_m.is_finite()
                })
            {
                return Err(format!(
                    "foundation {} saved pressure cells contain non-finite values",
                    calculation.foundation_id
                ));
            }
            let pressure_plane_z = mat_top_z + (geometry.thickness_m * 0.01).max(0.001);
            let mut seen_cells = BTreeSet::new();
            for cell in &result.cells {
                if cell.index >= foundation_core::MAX_MESH_CELLS || !seen_cells.insert(cell.index) {
                    return Err(format!(
                        "foundation {} has an invalid or repeated pressure cell identity {}",
                        calculation.foundation_id, cell.index
                    ));
                }
                let x_min = reference.x + cell.x_m - result.mesh.dx_m * 0.5;
                let x_max = reference.x + cell.x_m + result.mesh.dx_m * 0.5;
                let y_min = reference.y + cell.y_m - result.mesh.dy_m * 0.5;
                let y_max = reference.y + cell.y_m + result.mesh.dy_m * 0.5;
                let (cell_mesh, cell_points) = quad_mesh(
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                    pressure_plane_z,
                    &format!(
                        "foundation {} pressure cell {}",
                        calculation.foundation_id, cell.index
                    ),
                )?;
                let mut cell_sources = ScSourceIds::default();
                cell_sources.foundation_id = Some(calculation.foundation_id.clone());
                queue_overlay(
                    scene,
                    pending,
                    pending_ids,
                    pending_triangles,
                    ScPick::FoundationCell {
                        foundation_id: calculation.foundation_id.clone(),
                        cell_index: cell.index,
                    },
                    format!(
                        "Saved contact-pressure cell {} · foundation {} · q={:.6} kPa · active sampled cell={} · pressure overlay raised {:.4} m for visibility",
                        cell.index,
                        calculation.foundation_id,
                        cell.q_kpa,
                        cell.active,
                        pressure_plane_z - mat_top_z
                    ),
                    cell_mesh,
                    pressure_color(cell.q_kpa, min_q, max_q),
                    in_scope_owners.clone(),
                    cell_sources,
                    cell_points,
                )?;
            }
            status.push(format!(
                "Saved sampled contact pressure for foundation {}: {} actual cells, q range [{:.6}, {:.6}] kPa, contact state {:?}. Colors are a display interpolation of saved q values; no pressure values were recomputed.",
                calculation.foundation_id,
                result.cells.len(),
                min_q,
                max_q,
                result.contact.state
            ));
        }

        let load_source = match snapshot.structural.input.definition.foundation_load_source {
            ai_revit_core::FoundationLoadSource::StructuralReactions => {
                "effective input matched to the saved structural reaction snapshot"
            }
            ai_revit_core::FoundationLoadSource::ManualSynthetic => {
                "manual synthetic effective input"
            }
        };
        status.push(format!(
            "Computed foundation {} shown from saved effective input/result; {}. Contact is {:?}; ULS bearing {:?}; settlement {:?}; reinforcement {:?}; site verified={}. These values do not establish code or site acceptance.",
            calculation.foundation_id,
            load_source,
            result.contact.state,
            result.engineering_limits.ultimate_bearing,
            result.engineering_limits.settlement,
            result.engineering_limits.reinforcement,
            result.engineering_limits.engineering_site_verified
        ));
        for note in &result.engineering_limits.notes {
            status.push(format!(
                "Foundation {} engineering limitation: {note}",
                calculation.foundation_id
            ));
        }
        if effective_input
            .soil
            .layers
            .iter()
            .any(|layer| layer.depth_bottom_m.is_none())
        {
            status.push(format!(
                "Foundation {} includes an open-ended soil layer; its finite display extent is the source visualization assumption, not a verified geological boundary.",
                calculation.foundation_id
            ));
        }
        computed_groups += 1;
    }

    status.push(format!(
        "SCOPED FOUNDATION RESULT: {} computed foundation group(s) shown for snapshot sections [{}]; {} group(s) remain held and emitted no foundation geometry.",
        computed_groups,
        snapshot
            .scope
            .section_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        held_groups
    ));
    if computed_groups == 0 {
        status.push(
            "No saved computed in-scope foundation output is available; no foundation objects were synthesized.".into(),
        );
    }
    let _ = building;
    Ok(())
}

fn validate_foundation_geometry(
    geometry: &foundation_core::MatGeometry,
    foundation_id: &foundation_core::FoundationId,
) -> Result<(), String> {
    for (name, value) in [
        ("width_x_m", geometry.width_x_m),
        ("length_y_m", geometry.length_y_m),
        ("thickness_m", geometry.thickness_m),
    ] {
        if !value.is_finite() || value <= 0.0 || value > MAX_COORDINATE_M {
            return Err(format!(
                "foundation {foundation_id} effective geometry {name} is invalid for display"
            ));
        }
    }
    Ok(())
}

fn cuboid_mesh(
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    z_min: f64,
    z_max: f64,
    context: &str,
) -> Result<(Vec<Triangle>, Vec<Vec3>), String> {
    if ![x_min, x_max, y_min, y_max, z_min, z_max]
        .iter()
        .all(|value| value.is_finite())
        || x_max <= x_min
        || y_max <= y_min
        || z_max <= z_min
    {
        return Err(format!("{context} has invalid typed bounds"));
    }
    let vertices = vec![
        checked_generated_vertex(Vec3::new(x_min, y_min, z_min), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_min, z_min), context)?,
        checked_generated_vertex(Vec3::new(x_min, y_max, z_min), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_max, z_min), context)?,
        checked_generated_vertex(Vec3::new(x_min, y_min, z_max), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_min, z_max), context)?,
        checked_generated_vertex(Vec3::new(x_min, y_max, z_max), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_max, z_max), context)?,
    ];
    let faces = [
        [0, 2, 3],
        [0, 3, 1],
        [4, 5, 7],
        [4, 7, 6],
        [0, 1, 5],
        [0, 5, 4],
        [2, 6, 7],
        [2, 7, 3],
        [0, 4, 6],
        [0, 6, 2],
        [1, 3, 7],
        [1, 7, 5],
    ];
    let mut triangles = Vec::with_capacity(faces.len());
    for [a, b, c] in faces {
        push_triangle(&mut triangles, vertices[a], vertices[b], vertices[c])?;
    }
    Ok((triangles, vertices))
}

fn quad_mesh(
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    z: f64,
    context: &str,
) -> Result<(Vec<Triangle>, Vec<Vec3>), String> {
    if ![x_min, x_max, y_min, y_max, z]
        .iter()
        .all(|value| value.is_finite())
        || x_max <= x_min
        || y_max <= y_min
    {
        return Err(format!("{context} has invalid typed cell bounds"));
    }
    let vertices = vec![
        checked_generated_vertex(Vec3::new(x_min, y_min, z), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_min, z), context)?,
        checked_generated_vertex(Vec3::new(x_max, y_max, z), context)?,
        checked_generated_vertex(Vec3::new(x_min, y_max, z), context)?,
    ];
    let mut triangles = Vec::with_capacity(2);
    push_triangle(&mut triangles, vertices[0], vertices[1], vertices[2])?;
    push_triangle(&mut triangles, vertices[0], vertices[2], vertices[3])?;
    Ok((triangles, vertices))
}

fn pressure_color(value: f64, min_value: f64, max_value: f64) -> [f32; 4] {
    let range = max_value - min_value;
    let normalized = if range.is_finite() && range > GEOMETRY_EPSILON {
        ((value - min_value) / range).clamp(0.0, 1.0)
    } else {
        0.5
    } as f32;
    [
        0.12 + 0.78 * normalized,
        0.34 + 0.18 * (1.0 - (2.0 * normalized - 1.0).abs()),
        0.90 - 0.74 * normalized,
        0.86,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use structural_configurator::{
        fixture, generate_building, AnalysisProjectionStatus, FixtureKind, GridSpacing, SemanticId,
    };

    fn physical_objects_for_section(
        rendered: &ScRenderScene,
        section_id: &SemanticId,
    ) -> BTreeMap<ObjectId, Vec<(u32, Vec<Vec3>)>> {
        let object_ids = rendered
            .by_section
            .get(section_id)
            .expect("fixture section must have renderer ownership entries");
        let mut geometry = BTreeMap::new();
        for object_id in object_ids {
            let Some(pick) = rendered.picks.get(object_id) else {
                continue;
            };
            if !matches!(
                pick,
                ScPick::Node(_) | ScPick::Member(_) | ScPick::Surface(_) | ScPick::Support(_)
            ) {
                continue;
            }
            let object = rendered
                .scene
                .get(*object_id)
                .expect("physical pick must reference a rendered scene object");
            let triangles = object
                .mesh
                .triangles()
                .iter()
                .map(|triangle| {
                    (
                        triangle.triangle_id,
                        triangle
                            .vertices
                            .iter()
                            .map(|point| object.transform.transform_point(*point))
                            .collect(),
                    )
                })
                .collect();
            geometry.insert(*object_id, triangles);
        }
        geometry
    }

    #[test]
    fn full_bim_renders_beyond_analysis_capacity() {
        let mut config = fixture(FixtureKind::IndependentJoint);
        assert!(config.allow_disconnected_analysis);
        assert!(config
            .interfaces
            .iter()
            .all(|interface| interface.anchors.is_empty()));

        for section in &mut config.sections {
            section.grid.frame_stations = GridSpacing::Uniform { pitch_m: 1.0 };
            section.grid.column_stations = GridSpacing::Uniform { pitch_m: 1.0 };
            section.grid.purlin_pitch_m = 1.0;
            section.grid.wall_girt_pitch_m = 1.0;
        }

        let building = generate_building(&config)
            .expect("bounded synthetic fixture should generate within BIM limits");
        match &building.analysis_status {
            AnalysisProjectionStatus::AnalysisModelCapacityExceeded {
                nodes,
                members,
                max_nodes,
                max_members,
            } => {
                assert_eq!(*nodes, building.nodes.len());
                assert_eq!(*members, building.members.len());
                assert!(building.nodes.len() > *max_nodes);
                assert!(building.members.len() > *max_members);
            }
            other => panic!("expected bounded analysis capacity hold, got {other:?}"),
        }

        let rendered = build_scene(&building, &ScRenderOptions::default())
            .expect("renderer should accept the full generated BIM graph");
        let rendered_member_ids: BTreeSet<_> = rendered
            .picks
            .values()
            .filter_map(|pick| match pick {
                ScPick::Member(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let generated_member_ids: BTreeSet<_> = building.members.keys().cloned().collect();
        assert_eq!(rendered_member_ids, generated_member_ids);
    }

    #[test]
    fn editing_a_preserves_independent_b_render_geometry() {
        let before_config = fixture(FixtureKind::IndependentJoint);
        let mut after_config = before_config.clone();
        let section_a = after_config
            .sections
            .iter_mut()
            .find(|section| section.id.as_str() == "A")
            .expect("shared fixture contains section A");
        section_a.roof.eave_abs_m = 8.0;

        let before_building =
            generate_building(&before_config).expect("original shared fixture should generate");
        let after_building =
            generate_building(&after_config).expect("edited shared fixture should generate");
        let options = ScRenderOptions::default();
        let before_scene =
            build_scene(&before_building, &options).expect("original full BIM graph should render");
        let after_scene =
            build_scene(&after_building, &options).expect("edited full BIM graph should render");
        let section_b = SemanticId::new("B").expect("fixture section ID is valid");
        let before_geometry = physical_objects_for_section(&before_scene, &section_b);
        let after_geometry = physical_objects_for_section(&after_scene, &section_b);

        assert!(
            !before_geometry.is_empty(),
            "section B must remain rendered"
        );
        assert_eq!(before_geometry, after_geometry);
    }
}
