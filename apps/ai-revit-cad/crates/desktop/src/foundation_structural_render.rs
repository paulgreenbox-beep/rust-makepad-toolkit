//! Bounded, display-only rendering for the Stage 1 foundation and structural views.
//!
//! Scene geometry in this module is synthetic mesh geometry. Pressure samples and
//! structural displacements are consumed as typed projections from the caller; this
//! module does not derive engineering values or claim exact CAD geometry.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use drafting_core::{Camera2D, Point2 as DraftPoint};
use structural_core::{
    MAX_MEMBERS, MAX_NODES, StructuralAnalysisResult, StructuralModel, StructuralSection,
};
use viewer_core::{Mesh, ObjectId, Scene, SceneObject, Triangle, Vec3};

use crate::drafting_canvas::{
    AnalysisCanvasLayer, ScreenLabel, ScreenLabelPlacement, ScreenSegment, ScreenTriangle,
};

const NODE_ID_PREFIX: u64 = 0x1000_0000_0000_0000;
const MEMBER_ID_PREFIX: u64 = 0x2000_0000_0000_0000;
const DEFORMED_NODE_ID_PREFIX: u64 = 0x3000_0000_0000_0000;
const DEFORMED_MEMBER_ID_PREFIX: u64 = 0x4000_0000_0000_0000;
const FOUNDATION_ID: ObjectId = 0x5000_0000_0000_0001;
const GROUND_LAYER_ID_PREFIX: u64 = 0x6000_0000_0000_0000;
const ID_PAYLOAD_MASK: u64 = 0x0fff_ffff_ffff_ffff;
const MAX_ABS_COORDINATE_M: f64 = 1.0e9;
const MAX_SOIL_LAYERS: usize = 64;
const MAX_PRESSURE_CELLS: usize = 4096;
const MAX_PRESSURE_CELL_VERTICES: usize = 16;
const MAX_ABS_PRESSURE_KPA: f64 = 1.0e12;

/// The renderer's view namespace. These views do not create document entities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage1View {
    FoundationPlan,
    Frame3D,
    Pressure2D,
    Ground3D,
}

/// Typed renderer selection identity; values are not BIM document element IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage1Pick {
    Node(u64),
    Member(u64),
    Foundation,
    GroundLayer(usize),
}

/// Source status for the typed mat projection supplied by the domain/API layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatDisplaySource {
    /// Geometry and pressure samples came from the current reaction-linked input.
    MatchedEffectiveFoundationInput,
    /// Explicit manual values; they are not connected to a verified site model.
    ManualSynthetic,
    /// Declared geometry remains visible, but no current reaction snapshot backs it.
    DeclaredInputWithoutCurrentReactionSnapshot,
}

/// Minimal typed soil geometry consumed by the display renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct SoilDisplayLayer {
    /// Layer top measured downward from the global raft-base reference, in metres.
    pub depth_top_m: f64,
    pub thickness_m: f64,
}

/// One already-derived pressure sample cell in foundation-local XY coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PressureCell {
    pub vertices_xy_m: Vec<[f64; 2]>,
    pub pressure_kpa: f64,
}

/// Typed display projection; pressure is supplied by the domain layer, never invented here.
/// The footprint is centered at local XY origin and extends by half each plan dimension.
#[derive(Clone, Debug, PartialEq)]
pub struct MatDisplay {
    pub source: MatDisplaySource,
    /// Global XYZ center of the raft base; pressure cell vertices remain local XY.
    pub reference_point_m: [f64; 3],
    pub length_x_m: f64,
    pub width_y_m: f64,
    pub thickness_m: f64,
    pub soil_layers: Vec<SoilDisplayLayer>,
    pub pressure_cells: Vec<PressureCell>,
}

/// Immutable borrowed view over API-owned snapshots and projections.
///
/// `structural_result` must be matched to `model` by the caller. This renderer
/// also checks node/member identities and node coordinates before displaying it.
#[derive(Clone, Copy, Debug)]
pub struct RenderData<'a> {
    pub model: &'a StructuralModel,
    pub structural_result: Option<&'a StructuralAnalysisResult>,
    pub mat: Option<&'a MatDisplay>,
    /// Visual multiplier only; source displacements remain in metres.
    pub deformation_scale: f64,
}

/// Builds a pickable mesh scene for 3D views and an empty scene for plan views.
///
/// Every created `SceneObject` uses the viewer's mesh path and has no exact
/// geometry. Pressure surfaces intentionally stay in `AnalysisCanvasLayer`.
pub fn scene(
    data: &RenderData<'_>,
    view: Stage1View,
) -> Result<(Scene, BTreeMap<ObjectId, Stage1Pick>), String> {
    let mut result = Scene::new();
    let mut picks = BTreeMap::new();
    match view {
        Stage1View::FoundationPlan | Stage1View::Pressure2D => return Ok((result, picks)),
        Stage1View::Frame3D => build_frame_scene(data, &mut result, &mut picks)?,
        Stage1View::Ground3D => {
            if let Some(mat) = data.mat {
                validate_mat(mat)?;
                add_ground_scene(mat, &mut result, &mut picks)?;
            }
        }
    }
    Ok((result, picks))
}

/// Projects the foundation footprint and typed pressure cells into the drafting canvas.
pub fn plan_layer(
    data: &RenderData<'_>,
    view: Stage1View,
    camera: &Camera2D,
    selection: Option<&Stage1Pick>,
) -> Result<AnalysisCanvasLayer, String> {
    if !matches!(view, Stage1View::FoundationPlan | Stage1View::Pressure2D) {
        return Err("plan_layer requires FoundationPlan or Pressure2D".to_owned());
    }
    camera
        .validate()
        .map_err(|error| format!("invalid 2D camera: {error}"))?;

    let mut layer = AnalysisCanvasLayer::default();
    let Some(mat) = data.mat else {
        push_label(
            &mut layer,
            DraftPoint::new(16.0, 24.0),
            "Foundation input unavailable",
            COLOR_MUTED,
            ScreenLabelPlacement::Footer,
        );
        push_label(
            &mut layer,
            DraftPoint::new(16.0, camera.viewport_height - 18.0),
            "SYNTHETIC DISPLAY · MESH-DERIVED GEOMETRY",
            COLOR_MUTED,
            ScreenLabelPlacement::Footer,
        );
        return Ok(layer);
    };
    validate_mat(mat)?;

    let half_x = mat.length_x_m * 0.5;
    let half_y = mat.width_y_m * 0.5;
    let corners = [
        [-half_x, -half_y],
        [half_x, -half_y],
        [half_x, half_y],
        [-half_x, half_y],
    ];
    let foundation_selected = matches!(selection, Some(&Stage1Pick::Foundation));
    let outline_color = if foundation_selected {
        COLOR_SELECTED
    } else {
        COLOR_FOUNDATION
    };
    for index in 0..corners.len() {
        let start = camera.world_to_screen(DraftPoint::new(corners[index][0], corners[index][1]));
        let end = camera.world_to_screen(DraftPoint::new(
            corners[(index + 1) % corners.len()][0],
            corners[(index + 1) % corners.len()][1],
        ));
        layer.segments.push(ScreenSegment {
            start,
            end,
            color: outline_color,
            width: if foundation_selected { 3.0 } else { 2.0 },
        });
    }

    if view == Stage1View::Pressure2D {
        draw_pressure_cells(mat, camera, &mut layer)?;
    } else if !mat.pressure_cells.is_empty() {
        push_label(
            &mut layer,
            DraftPoint::new(16.0, 48.0),
            &format!(
                "{} pressure cells · open Pressure 2D for field display",
                mat.pressure_cells.len()
            ),
            COLOR_MUTED,
            ScreenLabelPlacement::Hud,
        );
    }

    let source_text = match mat.source {
        MatDisplaySource::MatchedEffectiveFoundationInput => {
            "Matched foundation input · synthetic mesh display"
        }
        MatDisplaySource::ManualSynthetic => "Manual synthetic input · mesh display",
        MatDisplaySource::DeclaredInputWithoutCurrentReactionSnapshot => {
            "Declared input · no current reaction snapshot"
        }
    };
    let dimensions = format!(
        "Footprint {:.3} m × {:.3} m · mat {:.3} m",
        mat.length_x_m, mat.width_y_m, mat.thickness_m
    );
    let origin = mat.reference_point_m;
    push_label(
        &mut layer,
        DraftPoint::new(16.0, camera.viewport_height - 40.0),
        &dimensions,
        COLOR_TEXT,
        ScreenLabelPlacement::Footer,
    );
    push_label(
        &mut layer,
        DraftPoint::new(16.0, camera.viewport_height - 20.0),
        &format!(
            "{source_text} · MESH-DERIVED · plan local XY · raft base center ({:.3}, {:.3}, {:.3}) m",
            origin[0], origin[1], origin[2]
        ),
        COLOR_MUTED,
        ScreenLabelPlacement::Footer,
    );
    Ok(layer)
}

/// Returns the foundation identity when a plan-local XY point is inside its footprint.
/// `Camera2D` is intentionally interpreted in mat-local XY; the global datum is
/// informational on this view and is applied only by the 3D scene projection.
pub fn pick_plan(
    data: &RenderData<'_>,
    view: Stage1View,
    camera: &Camera2D,
    screen_point: DraftPoint,
) -> Option<Stage1Pick> {
    if !matches!(view, Stage1View::FoundationPlan | Stage1View::Pressure2D)
        || camera.validate().is_err()
        || !screen_point.is_finite()
    {
        return None;
    }
    let mat = data.mat?;
    if validate_mat(mat).is_err() {
        return None;
    }
    let point = camera.screen_to_world(screen_point);
    (point.x.abs() <= mat.length_x_m * 0.5 && point.y.abs() <= mat.width_y_m * 0.5)
        .then_some(Stage1Pick::Foundation)
}

const COLOR_FOUNDATION: [f32; 4] = [0.24, 0.55, 0.82, 1.0];
const COLOR_SELECTED: [f32; 4] = [1.0, 0.78, 0.16, 1.0];
const COLOR_TEXT: [f32; 4] = [0.92, 0.94, 0.98, 1.0];
const COLOR_MUTED: [f32; 4] = [0.66, 0.72, 0.80, 1.0];

fn build_frame_scene(
    data: &RenderData<'_>,
    scene: &mut Scene,
    picks: &mut BTreeMap<ObjectId, Stage1Pick>,
) -> Result<(), String> {
    validate_model(data.model)?;
    let nodes = data
        .model
        .nodes
        .iter()
        .map(|node| (node.id.get(), node))
        .collect::<BTreeMap<_, _>>();
    let sections = data
        .model
        .sections
        .iter()
        .map(|section| (section.id.get(), section))
        .collect::<BTreeMap<_, _>>();
    let extent = model_extent(data.model);
    let node_radius = (extent * 0.018).clamp(0.015, 0.12);

    let mut deformed_positions = BTreeMap::new();
    if let Some(result) = data.structural_result {
        if !data.deformation_scale.is_finite()
            || data.deformation_scale < 0.0
            || data.deformation_scale > 1.0e6
        {
            return Err("deformation display scale must be finite and in [0, 1e6]".to_owned());
        }
        deformed_positions = validate_result_binding(data.model, result)?;
        for (id, result_node) in result.nodes.iter().map(|node| (node.node.get(), node)) {
            let source = nodes
                .get(&id)
                .ok_or_else(|| format!("analysis result references missing node {id}"))?;
            let point = [
                source.position_m[0] + result_node.displacement[0] * data.deformation_scale,
                source.position_m[1] + result_node.displacement[1] * data.deformation_scale,
                source.position_m[2] + result_node.displacement[2] * data.deformation_scale,
            ];
            validate_position(point, "deformed node")?;
            deformed_positions.insert(id, point);
        }
    }

    for node in &data.model.nodes {
        let id = object_id(NODE_ID_PREFIX, node.id.get())?;
        let mesh = cube_mesh(node.position_m, node_radius)?;
        add_scene_object(
            scene,
            picks,
            id,
            Stage1Pick::Node(node.id.get()),
            mesh,
            [0.55, 0.65, 0.78, 0.52],
            format!("Node {} · synthetic mesh marker", node.id.get()),
        )?;
    }
    for member in &data.model.members {
        let start = nodes
            .get(&member.start.get())
            .ok_or_else(|| format!("member {} start node is missing", member.id.get()))?;
        let end = nodes
            .get(&member.end.get())
            .ok_or_else(|| format!("member {} end node is missing", member.id.get()))?;
        let section = sections
            .get(&member.section.get())
            .ok_or_else(|| format!("member {} section is missing", member.id.get()))?;
        let radius = member_display_radius(section, node_radius)?;
        let mesh = tube_mesh(start.position_m, end.position_m, radius)?;
        add_scene_object(
            scene,
            picks,
            object_id(MEMBER_ID_PREFIX, member.id.get())?,
            Stage1Pick::Member(member.id.get()),
            mesh,
            [0.30, 0.57, 0.83, 0.62],
            format!("Member {} · section display proxy", member.id.get()),
        )?;
    }

    if data.structural_result.is_some() {
        let scale_text = format!("×{:.3} display scale", data.deformation_scale);
        for node in &data.model.nodes {
            let point = *deformed_positions
                .get(&node.id.get())
                .ok_or_else(|| format!("deformed node {} is missing", node.id.get()))?;
            add_scene_object(
                scene,
                picks,
                object_id(DEFORMED_NODE_ID_PREFIX, node.id.get())?,
                Stage1Pick::Node(node.id.get()),
                cube_mesh(point, node_radius * 0.8)?,
                [0.98, 0.64, 0.22, 0.95],
                format!("Deformed node {} · {scale_text}", node.id.get()),
            )?;
        }
        for member in &data.model.members {
            let start = *deformed_positions
                .get(&member.start.get())
                .ok_or_else(|| format!("deformed member {} start is missing", member.id.get()))?;
            let end = *deformed_positions
                .get(&member.end.get())
                .ok_or_else(|| format!("deformed member {} end is missing", member.id.get()))?;
            let section = sections
                .get(&member.section.get())
                .ok_or_else(|| format!("member {} section is missing", member.id.get()))?;
            let mesh = tube_mesh(start, end, member_display_radius(section, node_radius)?)?;
            add_scene_object(
                scene,
                picks,
                object_id(DEFORMED_MEMBER_ID_PREFIX, member.id.get())?,
                Stage1Pick::Member(member.id.get()),
                mesh,
                [0.95, 0.42, 0.14, 1.0],
                format!("Deformed member {} · {scale_text}", member.id.get()),
            )?;
        }
    }
    Ok(())
}

fn add_ground_scene(
    mat: &MatDisplay,
    scene: &mut Scene,
    picks: &mut BTreeMap<ObjectId, Stage1Pick>,
) -> Result<(), String> {
    let [origin_x, origin_y, origin_z] = mat.reference_point_m;
    let half_x = mat.length_x_m * 0.5;
    let half_y = mat.width_y_m * 0.5;
    let mat_mesh = box_mesh(
        [origin_x - half_x, origin_y - half_y, origin_z],
        [
            origin_x + half_x,
            origin_y + half_y,
            origin_z + mat.thickness_m,
        ],
    )?;
    let source_name = match mat.source {
        MatDisplaySource::MatchedEffectiveFoundationInput => "matched foundation input",
        MatDisplaySource::ManualSynthetic => "manual synthetic input",
        MatDisplaySource::DeclaredInputWithoutCurrentReactionSnapshot => {
            "declared input without current reaction snapshot"
        }
    };
    add_scene_object(
        scene,
        picks,
        FOUNDATION_ID,
        Stage1Pick::Foundation,
        mat_mesh,
        [0.26, 0.54, 0.80, 0.82],
        format!(
            "Foundation mat · {:.3} × {:.3} × {:.3} m · {source_name} · synthetic mesh",
            mat.length_x_m, mat.width_y_m, mat.thickness_m
        ),
    )?;

    for (index, layer) in mat.soil_layers.iter().enumerate() {
        let top_z = origin_z - layer.depth_top_m;
        let bottom_z = top_z - layer.thickness_m;
        validate_coordinate(top_z, "soil layer top elevation")?;
        validate_coordinate(bottom_z, "soil layer elevation")?;
        let mesh = box_mesh(
            [origin_x - half_x, origin_y - half_y, bottom_z],
            [origin_x + half_x, origin_y + half_y, top_z],
        )?;
        let index_u64 = u64::try_from(index + 1)
            .map_err(|_| "soil layer index exceeds scene identity range".to_owned())?;
        let id = GROUND_LAYER_ID_PREFIX
            .checked_add(index_u64)
            .ok_or_else(|| "soil layer scene identity overflow".to_owned())?;
        add_scene_object(
            scene,
            picks,
            id,
            Stage1Pick::GroundLayer(index),
            mesh,
            if index % 2 == 0 {
                [0.58, 0.43, 0.31, 0.48]
            } else {
                [0.68, 0.53, 0.39, 0.48]
            },
            format!(
                "Ground layer {} · {:.3} m · synthetic mesh display",
                index + 1,
                layer.thickness_m
            ),
        )?;
    }
    Ok(())
}

fn validate_model(model: &StructuralModel) -> Result<(), String> {
    if model.nodes.len() > MAX_NODES {
        return Err(format!(
            "structural model exceeds {MAX_NODES} display nodes"
        ));
    }
    if model.members.len() > MAX_MEMBERS {
        return Err(format!(
            "structural model exceeds {MAX_MEMBERS} display members"
        ));
    }
    let mut node_ids = BTreeSet::new();
    for node in &model.nodes {
        if !node_ids.insert(node.id.get()) {
            return Err(format!("duplicate structural node ID {}", node.id.get()));
        }
        validate_position(node.position_m, "structural node")?;
    }
    let mut section_ids = BTreeSet::new();
    for section in &model.sections {
        if !section_ids.insert(section.id.get()) {
            return Err(format!(
                "duplicate structural section ID {}",
                section.id.get()
            ));
        }
        if !section.area_m2.is_finite() || section.area_m2 <= 0.0 {
            return Err(format!(
                "section {} has invalid display area",
                section.id.get()
            ));
        }
    }
    let mut member_ids = BTreeSet::new();
    for member in &model.members {
        if !member_ids.insert(member.id.get()) {
            return Err(format!(
                "duplicate structural member ID {}",
                member.id.get()
            ));
        }
        if !node_ids.contains(&member.start.get()) || !node_ids.contains(&member.end.get()) {
            return Err(format!("member {} has a missing end node", member.id.get()));
        }
        if !section_ids.contains(&member.section.get()) {
            return Err(format!("member {} has a missing section", member.id.get()));
        }
        if member.start == member.end {
            return Err(format!(
                "member {} has identical end nodes",
                member.id.get()
            ));
        }
    }
    Ok(())
}

fn validate_result_binding(
    model: &StructuralModel,
    result: &StructuralAnalysisResult,
) -> Result<BTreeMap<u64, [f64; 3]>, String> {
    if result.nodes.len() != model.nodes.len() || result.members.len() != model.members.len() {
        return Err(
            "analysis result node/member count does not match the displayed model".to_owned(),
        );
    }
    let model_nodes = model
        .nodes
        .iter()
        .map(|node| (node.id.get(), node))
        .collect::<BTreeMap<_, _>>();
    let mut positions = BTreeMap::new();
    for result_node in &result.nodes {
        let id = result_node.node.get();
        let model_node = model_nodes
            .get(&id)
            .ok_or_else(|| format!("analysis result node {id} is absent from the model"))?;
        if positions.contains_key(&id) {
            return Err(format!("analysis result has duplicate node {id}"));
        }
        validate_position(result_node.position_m, "analysis result node")?;
        if (0..3)
            .any(|axis| (result_node.position_m[axis] - model_node.position_m[axis]).abs() > 1.0e-9)
        {
            return Err(format!("analysis result node {id} position is stale"));
        }
        if result_node
            .displacement
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(format!(
                "analysis result node {id} has a non-finite displacement"
            ));
        }
        positions.insert(id, model_node.position_m);
    }
    let model_members = model
        .members
        .iter()
        .map(|member| member.id.get())
        .collect::<BTreeSet<_>>();
    let result_members = result
        .members
        .iter()
        .map(|member| member.member.get())
        .collect::<BTreeSet<_>>();
    if model_members != result_members || result_members.len() != result.members.len() {
        return Err("analysis result members do not match the displayed model".to_owned());
    }
    Ok(positions)
}

fn validate_mat(mat: &MatDisplay) -> Result<(), String> {
    validate_position(mat.reference_point_m, "mat reference point")?;
    for (value, field) in [
        (mat.length_x_m, "foundation length"),
        (mat.width_y_m, "foundation width"),
        (mat.thickness_m, "foundation thickness"),
    ] {
        if !value.is_finite() || value <= 0.0 || value > MAX_ABS_COORDINATE_M {
            return Err(format!("{field} must be finite and positive"));
        }
    }
    if mat.soil_layers.len() > MAX_SOIL_LAYERS {
        return Err(format!("soil layer count exceeds {MAX_SOIL_LAYERS}"));
    }
    validate_coordinate(
        mat.reference_point_m[2] + mat.thickness_m,
        "mat top elevation",
    )?;
    for layer in &mat.soil_layers {
        if !layer.depth_top_m.is_finite() || layer.depth_top_m < 0.0 {
            return Err("soil layer top depth must be finite and nonnegative".to_owned());
        }
        if !layer.thickness_m.is_finite() || layer.thickness_m <= 0.0 {
            return Err("soil layer thickness must be finite and positive".to_owned());
        }
        let bottom_depth = layer.depth_top_m + layer.thickness_m;
        let top_z = mat.reference_point_m[2] - layer.depth_top_m;
        let bottom_z = top_z - layer.thickness_m;
        if !bottom_depth.is_finite() || bottom_depth > MAX_ABS_COORDINATE_M {
            return Err("soil layer display depth exceeds the bound".to_owned());
        }
        validate_coordinate(top_z, "soil layer top elevation")?;
        validate_coordinate(bottom_z, "soil layer bottom elevation")?;
    }
    if mat.pressure_cells.len() > MAX_PRESSURE_CELLS {
        return Err(format!("pressure cell count exceeds {MAX_PRESSURE_CELLS}"));
    }
    if matches!(
        mat.source,
        MatDisplaySource::DeclaredInputWithoutCurrentReactionSnapshot
    ) && !mat.pressure_cells.is_empty()
    {
        return Err(
            "declared input without a current reaction snapshot cannot carry pressure samples"
                .to_owned(),
        );
    }
    for (index, cell) in mat.pressure_cells.iter().enumerate() {
        if !cell.pressure_kpa.is_finite() || cell.pressure_kpa.abs() > MAX_ABS_PRESSURE_KPA {
            return Err(format!(
                "pressure cell {} has a non-finite or out-of-range value",
                index + 1
            ));
        }
        validate_convex_cell(cell, index, mat.length_x_m, mat.width_y_m)?;
    }
    Ok(())
}

fn validate_convex_cell(
    cell: &PressureCell,
    index: usize,
    length_x_m: f64,
    width_y_m: f64,
) -> Result<(), String> {
    let vertices = &cell.vertices_xy_m;
    if !(3..=MAX_PRESSURE_CELL_VERTICES).contains(&vertices.len()) {
        return Err(format!(
            "pressure cell {} has unsupported vertex count",
            index + 1
        ));
    }
    let half_x = length_x_m * 0.5;
    let half_y = width_y_m * 0.5;
    for point in vertices {
        if !point[0].is_finite()
            || !point[1].is_finite()
            || point[0].abs() > half_x + 1.0e-8
            || point[1].abs() > half_y + 1.0e-8
        {
            return Err(format!(
                "pressure cell {} lies outside the mat footprint",
                index + 1
            ));
        }
    }
    let mut signed_area = 0.0;
    let mut winding = 0_i8;
    for i in 0..vertices.len() {
        let a = vertices[i];
        let b = vertices[(i + 1) % vertices.len()];
        let c = vertices[(i + 2) % vertices.len()];
        signed_area += a[0] * b[1] - b[0] * a[1];
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() > 1.0e-12 {
            let next = if cross > 0.0 { 1 } else { -1 };
            if winding != 0 && winding != next {
                return Err(format!("pressure cell {} is not convex", index + 1));
            }
            winding = next;
        }
    }
    if signed_area.abs() <= 1.0e-12 || winding == 0 {
        return Err(format!("pressure cell {} has zero area", index + 1));
    }
    Ok(())
}

fn draw_pressure_cells(
    mat: &MatDisplay,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
) -> Result<(), String> {
    if mat.pressure_cells.is_empty() {
        push_label(
            layer,
            DraftPoint::new(16.0, 48.0),
            "Pressure samples unavailable",
            COLOR_MUTED,
            ScreenLabelPlacement::Hud,
        );
        return Ok(());
    }
    let min = mat
        .pressure_cells
        .iter()
        .map(|cell| cell.pressure_kpa)
        .fold(f64::INFINITY, f64::min);
    let max = mat
        .pressure_cells
        .iter()
        .map(|cell| cell.pressure_kpa)
        .fold(f64::NEG_INFINITY, f64::max);

    for cell in &mat.pressure_cells {
        let normalized = if (max - min).abs() <= f64::EPSILON {
            0.5
        } else {
            (cell.pressure_kpa - min) / (max - min)
        };
        let color = pressure_color(normalized);
        let world = cell
            .vertices_xy_m
            .iter()
            .map(|point| camera.world_to_screen(DraftPoint::new(point[0], point[1])))
            .collect::<Vec<_>>();
        for index in 1..world.len() - 1 {
            layer.triangles.push(ScreenTriangle {
                vertices: [world[0], world[index], world[index + 1]],
                color,
            });
        }
        let centroid = cell.vertices_xy_m.iter().fold([0.0, 0.0], |sum, point| {
            [sum[0] + point[0], sum[1] + point[1]]
        });
        let count = cell.vertices_xy_m.len() as f64;
        let label_position =
            camera.world_to_screen(DraftPoint::new(centroid[0] / count, centroid[1] / count));
        push_label(
            layer,
            label_position,
            &format!("{:.2} kPa", cell.pressure_kpa),
            COLOR_TEXT,
            ScreenLabelPlacement::Absolute,
        );
    }
    draw_pressure_legend(layer, camera.viewport_width, min, max);
    Ok(())
}

fn draw_pressure_legend(layer: &mut AnalysisCanvasLayer, width: f64, min: f64, max: f64) {
    let x = (width - 212.0).max(16.0);
    let y = 18.0;
    let bar_width = 176.0;
    let bar_height = 12.0;
    let bands = 16;
    for band in 0..bands {
        let left = x + bar_width * band as f64 / bands as f64;
        let right = x + bar_width * (band + 1) as f64 / bands as f64;
        let top_left = DraftPoint::new(left, y);
        let top_right = DraftPoint::new(right, y);
        let bottom_left = DraftPoint::new(left, y + bar_height);
        let bottom_right = DraftPoint::new(right, y + bar_height);
        let color = pressure_color((band as f64 + 0.5) / bands as f64);
        layer.triangles.push(ScreenTriangle {
            vertices: [top_left, top_right, bottom_right],
            color,
        });
        layer.triangles.push(ScreenTriangle {
            vertices: [top_left, bottom_right, bottom_left],
            color,
        });
    }
    push_label(
        layer,
        DraftPoint::new(x, y + 30.0),
        &format!("Pressure kPa · {:.2} to {:.2}", min, max),
        COLOR_TEXT,
        ScreenLabelPlacement::Legend,
    );
}

fn pressure_color(value: f64) -> [f32; 4] {
    let value = value.clamp(0.0, 1.0);
    let stops = [
        [0.10, 0.30, 0.88],
        [0.08, 0.72, 0.76],
        [0.42, 0.82, 0.34],
        [0.98, 0.78, 0.18],
        [0.88, 0.20, 0.12],
    ];
    let position = value * (stops.len() - 1) as f64;
    let left = (position.floor() as usize).min(stops.len() - 2);
    let fraction = (position - left as f64) as f32;
    [
        stops[left][0] + (stops[left + 1][0] - stops[left][0]) * fraction,
        stops[left][1] + (stops[left + 1][1] - stops[left][1]) * fraction,
        stops[left][2] + (stops[left + 1][2] - stops[left][2]) * fraction,
        0.80,
    ]
}

fn push_label(
    layer: &mut AnalysisCanvasLayer,
    position: DraftPoint,
    text: &str,
    color: [f32; 4],
    placement: ScreenLabelPlacement,
) {
    layer.labels.push(ScreenLabel {
        position,
        text: text.to_owned(),
        color,
        placement,
    });
}

fn add_scene_object(
    scene: &mut Scene,
    picks: &mut BTreeMap<ObjectId, Stage1Pick>,
    id: ObjectId,
    pick: Stage1Pick,
    mesh: Mesh,
    color: [f32; 4],
    name: String,
) -> Result<(), String> {
    if mesh.triangles().is_empty() {
        return Err(format!("display mesh for scene object {id} is empty"));
    }
    if picks.insert(id, pick).is_some() {
        return Err(format!("duplicate Stage 1 scene identity {id}"));
    }
    let mut object = SceneObject::new(id, name, mesh);
    object.color = color;
    scene.insert(object);
    Ok(())
}

fn object_id(prefix: u64, source_id: u64) -> Result<ObjectId, String> {
    if source_id == 0 || source_id > ID_PAYLOAD_MASK {
        return Err(format!(
            "source ID {source_id} exceeds the Stage 1 scene ID payload"
        ));
    }
    Ok(prefix | source_id)
}

fn model_extent(model: &StructuralModel) -> f64 {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for node in &model.nodes {
        for axis in 0..3 {
            min[axis] = min[axis].min(node.position_m[axis]);
            max[axis] = max[axis].max(node.position_m[axis]);
        }
    }
    let extent = (0..3).map(|axis| max[axis] - min[axis]).fold(0.0, f64::max);
    if extent.is_finite() && extent > 0.0 {
        extent
    } else {
        1.0
    }
}

fn member_display_radius(section: &StructuralSection, node_radius: f64) -> Result<f64, String> {
    if !section.area_m2.is_finite() || section.area_m2 <= 0.0 {
        return Err(format!(
            "section {} has invalid display area",
            section.id.get()
        ));
    }
    let equivalent_radius = (section.area_m2 / std::f64::consts::PI).sqrt();
    if !equivalent_radius.is_finite() {
        return Err(format!(
            "section {} display proxy is non-finite",
            section.id.get()
        ));
    }
    Ok(equivalent_radius.clamp(node_radius * 0.12, node_radius * 0.42))
}

fn cube_mesh(center: [f64; 3], radius: f64) -> Result<Mesh, String> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err("node marker radius must be finite and positive".to_owned());
    }
    let min = [center[0] - radius, center[1] - radius, center[2] - radius];
    let max = [center[0] + radius, center[1] + radius, center[2] + radius];
    box_mesh(min, max)
}

fn box_mesh(min: [f64; 3], max: [f64; 3]) -> Result<Mesh, String> {
    for axis in 0..3 {
        validate_coordinate(min[axis], "mesh minimum")?;
        validate_coordinate(max[axis], "mesh maximum")?;
        if max[axis] <= min[axis] {
            return Err("display box has non-positive extent".to_owned());
        }
    }
    let p = [
        vec3([min[0], min[1], min[2]]),
        vec3([max[0], min[1], min[2]]),
        vec3([max[0], max[1], min[2]]),
        vec3([min[0], max[1], min[2]]),
        vec3([min[0], min[1], max[2]]),
        vec3([max[0], min[1], max[2]]),
        vec3([max[0], max[1], max[2]]),
        vec3([min[0], max[1], max[2]]),
    ];
    let faces = [
        [0, 2, 1, 0, 3, 2],
        [4, 5, 6, 4, 6, 7],
        [0, 1, 5, 0, 5, 4],
        [1, 2, 6, 1, 6, 5],
        [2, 3, 7, 2, 7, 6],
        [3, 0, 4, 3, 4, 7],
    ];
    let mut triangles = Vec::with_capacity(12);
    for face in faces {
        for tri in face.chunks_exact(3) {
            push_triangle(&mut triangles, p[tri[0]], p[tri[1]], p[tri[2]])?;
        }
    }
    Ok(Mesh::new(triangles))
}

fn tube_mesh(start: [f64; 3], end: [f64; 3], radius: f64) -> Result<Mesh, String> {
    validate_position(start, "member start")?;
    validate_position(end, "member end")?;
    if !radius.is_finite() || radius <= 0.0 {
        return Err("member display radius must be finite and positive".to_owned());
    }
    let axis_vector = sub(end, start);
    let length = norm(axis_vector);
    if !length.is_finite() || length <= 1.0e-10 {
        return Err("member has zero display length".to_owned());
    }
    let axis = scale(axis_vector, 1.0 / length);
    let reference = if axis[2].abs() > 0.85 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let basis_u = normalize(cross(axis, reference))?;
    let basis_v = cross(axis, basis_u);
    const SIDES: usize = 8;
    let mut starts = Vec::with_capacity(SIDES);
    let mut ends = Vec::with_capacity(SIDES);
    for side in 0..SIDES {
        let angle = TAU * side as f64 / SIDES as f64;
        let radial = add(
            scale(basis_u, angle.cos() * radius),
            scale(basis_v, angle.sin() * radius),
        );
        starts.push(vec3(add(start, radial)));
        ends.push(vec3(add(end, radial)));
    }
    let mut triangles = Vec::with_capacity(SIDES * 4);
    for side in 0..SIDES {
        let next = (side + 1) % SIDES;
        push_triangle(&mut triangles, starts[side], ends[side], ends[next])?;
        push_triangle(&mut triangles, starts[side], ends[next], starts[next])?;
        push_triangle(&mut triangles, vec3(start), starts[next], starts[side])?;
        push_triangle(&mut triangles, vec3(end), ends[side], ends[next])?;
    }
    Ok(Mesh::new(triangles))
}

fn push_triangle(triangles: &mut Vec<Triangle>, a: Vec3, b: Vec3, c: Vec3) -> Result<(), String> {
    let triangle_id = u32::try_from(triangles.len())
        .map_err(|_| "display mesh exceeds triangle identity range".to_owned())?;
    triangles.push(Triangle::new(triangle_id, a, b, c));
    Ok(())
}

fn vec3(value: [f64; 3]) -> Vec3 {
    Vec3::new(value[0], value[1], value[2])
}

fn validate_position(value: [f64; 3], field: &str) -> Result<(), String> {
    for coordinate in value {
        validate_coordinate(coordinate, field)?;
    }
    Ok(())
}

fn validate_coordinate(value: f64, field: &str) -> Result<(), String> {
    if !value.is_finite() || value.abs() > MAX_ABS_COORDINATE_M {
        return Err(format!("{field} contains an out-of-range coordinate"));
    }
    Ok(())
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(value: [f64; 3], scalar: f64) -> [f64; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(value: [f64; 3]) -> f64 {
    (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt()
}

fn normalize(value: [f64; 3]) -> Result<[f64; 3], String> {
    let length = norm(value);
    if !length.is_finite() || length <= 1.0e-12 {
        return Err("cannot construct a finite display mesh basis".to_owned());
    }
    Ok(scale(value, 1.0 / length))
}
