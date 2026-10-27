//! Native Makepad presentation scaffold for the structural configurator.
//!
//! This module deliberately contains widget structure only. Stored building
//! configuration, validation, draft binding, and analysis remain owned by the
//! shared App Document and its typed Core commands.

use super::*;
use crate::drafting_canvas::{
    AnalysisCanvasLayer, CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput,
    DraftingCanvasWidgetRefExt, PointerButton, PointerPhase, ScreenLabel, ScreenLabelPlacement,
    ScreenSegment,
};
use crate::structural_configurator_api as sc_api;
use crate::structural_configurator_render::{
    self as sc_render, ScPick, ScRenderOptions, ScViewMode,
};
use crate::viewport::{CadViewportAction, CadViewportWidgetRefExt};
use drafting_core::{Bounds2, Camera2D, Point2 as DraftPoint};
use makepad_widgets::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use structural_configurator::{
    BuildingConfig, Corner, GridSpacing, InterfaceKind, MemberRole, MemberRule, MemberRules,
    RoofKind, SectionConfig, SemanticId, ValidationIssue,
};
use viewer_core::{Aabb, ObjectId, Viewer};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ScPolygonMode {
    #[default]
    Select,
    Add,
    Move,
    Insert,
    Delete,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ScCanvasLayout {
    PlanOnly,
    #[default]
    Split,
    ThreeD,
}

#[derive(Clone, Debug)]
enum ScFitTarget {
    Building,
    Section(SemanticId),
}

pub(super) struct StructuralConfiguratorUi {
    pub active: bool,
    pub view: ScViewMode,
    pub selected_section: Option<SemanticId>,
    pub selected_corner: Option<SemanticId>,
    selected_interface: Option<SemanticId>,
    pub selection: Option<ScPick>,
    pub draft_dirty: bool,
    pub binding_stale: bool,
    coordinate_draft_dirty: bool,
    pub binding: Option<sc_api::ScBinding>,
    pub scene_picks: BTreeMap<ObjectId, ScPick>,
    pub scene_sources: BTreeMap<ObjectId, sc_render::ScSourceIds>,
    pub scene_graph_fingerprint: Option<String>,
    pub scene_overlay_snapshot_fingerprint: Option<String>,
    pub draft: Option<BuildingConfig>,
    pub projection: Option<sc_api::ScProjection>,
    pub visible_sections: BTreeSet<SemanticId>,
    layout: ScCanvasLayout,
    pub camera: Camera2D,
    pub snap_enabled: bool,
    pub grid_step_m: f64,
    pub hide_cladding: bool,
    pub cladding_alpha: f32,
    pub member_role: MemberRole,
    section_ids: Vec<SemanticId>,
    corner_ids: Vec<SemanticId>,
    material_ids: Vec<SemanticId>,
    profile_ids: Vec<SemanticId>,
    interface_ids: Vec<SemanticId>,
    interface_section_a_ids: Vec<SemanticId>,
    interface_section_b_ids: Vec<SemanticId>,
    interface_edge_a_ids: Vec<SemanticId>,
    interface_edge_b_ids: Vec<SemanticId>,
    polygon_mode: ScPolygonMode,
    pan_anchor: Option<DraftPoint>,
    dragging_corner: bool,
    canvas_camera_change: Option<CanvasCameraChange>,
    canvas_frame: CanvasFrame,
    canvas_cache_key: Option<String>,
    scene_cache_key: Option<String>,
    scene_objects_by_section: BTreeMap<SemanticId, BTreeSet<ObjectId>>,
    scene_bounds: Option<Aabb>,
    section_bounds: BTreeMap<SemanticId, Aabb>,
    pending_fit: Option<ScFitTarget>,
    plan_needs_fit: bool,
    render_generation: u64,
    validation_issues: Vec<ValidationIssue>,
    attempt_status: Option<String>,
    attempt_message: String,
    local_input_error: Option<String>,
}

impl Default for StructuralConfiguratorUi {
    fn default() -> Self {
        Self {
            active: false,
            view: ScViewMode::Architectural,
            selected_section: None,
            selected_corner: None,
            selected_interface: None,
            selection: None,
            draft_dirty: false,
            binding_stale: false,
            coordinate_draft_dirty: false,
            binding: None,
            scene_picks: BTreeMap::new(),
            scene_sources: BTreeMap::new(),
            scene_graph_fingerprint: None,
            scene_overlay_snapshot_fingerprint: None,
            draft: None,
            projection: None,
            visible_sections: BTreeSet::new(),
            layout: ScCanvasLayout::Split,
            camera: Camera2D::default(),
            snap_enabled: false,
            grid_step_m: 0.1,
            hide_cladding: false,
            cladding_alpha: 0.45,
            member_role: MemberRole::Column,
            section_ids: Vec::new(),
            corner_ids: Vec::new(),
            material_ids: Vec::new(),
            profile_ids: Vec::new(),
            interface_ids: Vec::new(),
            interface_section_a_ids: Vec::new(),
            interface_section_b_ids: Vec::new(),
            interface_edge_a_ids: Vec::new(),
            interface_edge_b_ids: Vec::new(),
            polygon_mode: ScPolygonMode::Select,
            pan_anchor: None,
            dragging_corner: false,
            canvas_camera_change: None,
            canvas_frame: CanvasFrame::default(),
            canvas_cache_key: None,
            scene_cache_key: None,
            scene_objects_by_section: BTreeMap::new(),
            scene_bounds: None,
            section_bounds: BTreeMap::new(),
            pending_fit: None,
            plan_needs_fit: true,
            render_generation: 0,
            validation_issues: Vec::new(),
            attempt_status: None,
            attempt_message: String::new(),
            local_input_error: None,
        }
    }
}

const SC_ROOF_KINDS: [RoofKind; 8] = [
    RoofKind::Flat,
    RoofKind::MonoPitch,
    RoofKind::Gable,
    RoofKind::Hip,
    RoofKind::Curved,
    RoofKind::MultiFacet,
    RoofKind::Stepped,
    RoofKind::Custom,
];

const SC_MEMBER_ROLES: [MemberRole; 6] = [
    MemberRole::Column,
    MemberRole::Rafter,
    MemberRole::Beam,
    MemberRole::Brace,
    MemberRole::Purlin,
    MemberRole::WallGirt,
];

const SC_INTERFACE_KIND_LABELS: [&str; 4] = [
    "Structurally connected · rigid idealization",
    "Independent with joint",
    "Geometry-only contact",
    "Not defined · hold",
];

fn sc_roof_kind_label(kind: RoofKind) -> &'static str {
    match kind {
        RoofKind::Flat => "Flat",
        RoofKind::MonoPitch => "Mono-pitch",
        RoofKind::Gable => "Gable",
        RoofKind::Hip => "Hip · generation hold",
        RoofKind::Curved => "Curved · generation hold",
        RoofKind::MultiFacet => "Multi-facet · generation hold",
        RoofKind::Stepped => "Stepped · generation hold",
        RoofKind::Custom => "Custom · generation hold",
    }
}

fn sc_member_role_label(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Column => "Column",
        MemberRole::Rafter => "Rafter",
        MemberRole::Beam => "Beam",
        MemberRole::Brace => "Brace",
        MemberRole::Purlin => "Purlin",
        MemberRole::WallGirt => "Wall girt",
    }
}

fn sc_member_rule(rules: &MemberRules, role: MemberRole) -> &MemberRule {
    match role {
        MemberRole::Column => &rules.column,
        MemberRole::Rafter => &rules.rafter,
        MemberRole::Beam => &rules.beam,
        MemberRole::Brace => &rules.brace,
        MemberRole::Purlin => &rules.purlin,
        MemberRole::WallGirt => &rules.wall_girt,
    }
}

fn sc_member_rule_mut(rules: &mut MemberRules, role: MemberRole) -> &mut MemberRule {
    match role {
        MemberRole::Column => &mut rules.column,
        MemberRole::Rafter => &mut rules.rafter,
        MemberRole::Beam => &mut rules.beam,
        MemberRole::Brace => &mut rules.brace,
        MemberRole::Purlin => &mut rules.purlin,
        MemberRole::WallGirt => &mut rules.wall_girt,
    }
}

fn sc_parse_number(value: &str, label: &str) -> Result<f64, String> {
    let parsed = value
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("{label}: enter a number in meters"))?;
    if !parsed.is_finite() {
        return Err(format!("{label}: value must be finite"));
    }
    Ok(parsed)
}

fn sc_unique_id(config: &BuildingConfig, prefix: &str) -> Result<SemanticId, String> {
    let mut used = BTreeSet::<String>::new();
    let mut add = |id: &SemanticId| {
        used.insert(id.as_str().to_owned());
    };
    add(&config.id);
    for section in &config.sections {
        add(&section.id);
        for corner in &section.corners {
            add(&corner.id);
        }
    }
    for interface in &config.interfaces {
        add(&interface.id);
        for anchor in &interface.anchors {
            add(&anchor.id);
        }
    }
    for material in &config.materials {
        add(&material.id);
    }
    for profile in &config.profiles {
        add(&profile.id);
    }
    for assignment in &config.foundation_assignments {
        add(&assignment.section_id);
        add(&assignment.support_node_id);
        add(&assignment.foundation_id);
    }
    for index in 1_u64..u64::MAX {
        let candidate = format!("{prefix}-{index}");
        if !used.contains(&candidate) {
            return candidate
                .parse::<SemanticId>()
                .map_err(|error| format!("could not create a semantic ID: {error}"));
        }
    }
    Err("no free semantic ID could be allocated".into())
}

fn sc_section_by_id<'a>(config: &'a BuildingConfig, id: &SemanticId) -> Option<&'a SectionConfig> {
    config.sections.iter().find(|section| &section.id == id)
}

fn sc_section_by_id_mut<'a>(
    config: &'a mut BuildingConfig,
    id: &SemanticId,
) -> Option<&'a mut SectionConfig> {
    config.sections.iter_mut().find(|section| &section.id == id)
}

fn sc_format_grid_spacing(spacing: &GridSpacing) -> String {
    match spacing {
        GridSpacing::Uniform { pitch_m } => format!("{pitch_m:.6}"),
        GridSpacing::Explicit { .. } => {
            "Explicit stations · edit to convert to Uniform pitch".into()
        }
    }
}

fn sc_interface_kind_index(kind: &InterfaceKind) -> usize {
    match kind {
        InterfaceKind::StructurallyConnected { .. } => 0,
        InterfaceKind::IndependentWithJoint { .. } => 1,
        InterfaceKind::GeometryOnlyContact => 2,
        InterfaceKind::NotDefined => 3,
    }
}

fn sc_interface_kind(index: usize, width_m: f64, direction: [f64; 2]) -> InterfaceKind {
    match index {
        0 => InterfaceKind::StructurallyConnected {
            idealization: structural_configurator::ConnectionIdealization::Rigid,
        },
        1 => InterfaceKind::IndependentWithJoint {
            width_m,
            offset_b_direction: direction,
        },
        2 => InterfaceKind::GeometryOnlyContact,
        _ => InterfaceKind::NotDefined,
    }
}

fn sc_polygon_bounds(config: &BuildingConfig) -> Option<Bounds2> {
    let mut points = config.sections.iter().flat_map(|section| {
        section
            .corners
            .iter()
            .map(|corner| DraftPoint::new(corner.xy_m[0], corner.xy_m[1]))
    });
    let first = points.next()?;
    let mut bounds = Bounds2::from_points(first, first);
    for point in points {
        bounds = bounds.union(Bounds2::from_points(point, point));
    }
    Some(bounds)
}

pub(super) const STRUCTURAL_CONFIGURATOR_ALIASES: &[&str] = &[
    "workspace_structural_configurator",
    "sc_workspace",
    "StructuralConfiguratorWorkspace",
    "sc_breadcrumb",
    "sc_section_add",
    "sc_section_delete",
    "sc_section_select",
    "sc_section_list",
    "sc_base_z",
    "sc_eave_height",
    "sc_ridge_max_height",
    "sc_roof_kind",
    "sc_roof_direction",
    "sc_roof_direction_x",
    "sc_roof_direction_y",
    "sc_frame_pitch",
    "sc_column_pitch",
    "sc_purlin_pitch",
    "sc_wall_girt_pitch",
    "sc_braces",
    "sc_material",
    "sc_profile",
    "sc_member_role",
    "sc_interface_add",
    "sc_interface_delete",
    "sc_interface_select",
    "sc_interface_list",
    "sc_interface_from",
    "sc_interface_to",
    "sc_interface_edge_a_start",
    "sc_interface_edge_a_end",
    "sc_interface_edge_b_start",
    "sc_interface_edge_b_end",
    "sc_interface_kind",
    "sc_interface_width",
    "sc_interface_offset_x",
    "sc_interface_offset_y",
    "sc_create_demo",
    "sc_apply",
    "sc_reload",
    "sc_regenerate",
    "sc_analyze",
    "sc_layout_plan",
    "sc_layout_split",
    "sc_layout_3d",
    "sc_fit_building",
    "sc_fit_section",
    "sc_hide_cladding",
    "sc_cladding_transparency",
    "sc_view_architectural",
    "sc_view_frame",
    "sc_view_connections",
    "sc_view_analysis",
    "sc_view_loads",
    "sc_view_reactions",
    "sc_view_foundation",
    "sc_mode_select",
    "sc_mode_add",
    "sc_mode_move",
    "sc_mode_insert",
    "sc_mode_delete",
    "sc_close_outline",
    "sc_snap_grid",
    "sc_grid_step",
    "sc_binding",
    "sc_plan_host",
    "sc_plan_title",
    "sc_plan_canvas",
    "sc_scene_host",
    "sc_scene_title",
    "sc_model_viewport",
    "sc_vertex_select",
    "sc_vertex_x",
    "sc_vertex_y",
    "sc_apply_coordinates",
    "sc_vertex_delete",
    "sc_vertex_table",
    "sc_status",
    "sc_summary",
    "sc_selection_inspector",
    "sc_validation_report",
    "sc_report",
];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.StructuralConfiguratorWorkspace = SolidView{
        width: Fill height: Fill flow: Right visible: false
        draw_bg.color: #x111922

        ScrollYView{
            width: 310 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            sc_breadcrumb := Label{width: Fill text: "Engineering → Structures → Steel Building Constructor" draw_text.color: #x9badbe}
            H3{width: Fill text: "Steel building sections" draw_text.color: #xe5edf6}
            Label{width: Fill text: "MODEL_INCOMPLETE · waiting for stored configuration" draw_text.color: #xf2c47c}
            View{width: Fill height: 32 flow: Right spacing: 5
                sc_section_add := Button{width: Fill text: "Add section"}
                sc_section_delete := Button{width: Fill text: "Delete selected"}
            }
            sc_section_select := DropDown{width: Fill labels: ["No stored sections"]}
            sc_section_list := TextInput{width: Fill height: 110 is_multiline: true is_read_only: true text: "No stored sections are loaded."}
            Hr{}

            Label{width: Fill text: "Absolute section elevations · meters" draw_text.color: #xb6c8d8}
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Base Z · absolute" draw_text.color: #xaebdcb}
                sc_base_z := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Eave Z · absolute" draw_text.color: #xaebdcb}
                sc_eave_height := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Ridge max Z · absolute" draw_text.color: #xaebdcb}
                sc_ridge_max_height := TextInput{width: Fill height: 30 empty_text: "m"}
            }

            Hr{}
            Label{width: Fill text: "Building layout" draw_text.color: #xb6c8d8}
            Label{width: Fill text: "Roof kind · loaded values only" draw_text.color: #xaebdcb}
            sc_roof_kind := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            Label{width: Fill text: "Roof span direction · building-local unit vector" draw_text.color: #xaebdcb}
            sc_roof_direction := View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 42 text: "X" draw_text.color: #xaebdcb}
                sc_roof_direction_x := TextInput{width: Fill height: 30 empty_text: "unit X"}
                Label{width: 42 text: "Y" draw_text.color: #xaebdcb}
                sc_roof_direction_y := TextInput{width: Fill height: 30 empty_text: "unit Y"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Frame pitch" draw_text.color: #xaebdcb}
                sc_frame_pitch := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Column pitch" draw_text.color: #xaebdcb}
                sc_column_pitch := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Purlin pitch" draw_text.color: #xaebdcb}
                sc_purlin_pitch := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 118 text: "Wall girt pitch" draw_text.color: #xaebdcb}
                sc_wall_girt_pitch := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            sc_braces := Button{width: Fill text: "Braces · stored value"}
            Label{width: Fill text: "Material · loaded catalog values only" draw_text.color: #xaebdcb}
            sc_material := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            Label{width: Fill text: "Profile · loaded catalog values only" draw_text.color: #xaebdcb}
            sc_profile := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            Label{width: Fill text: "Apply material/profile to one member role" draw_text.color: #xaebdcb}
            sc_member_role := DropDown{width: Fill labels: ["Column", "Rafter", "Beam", "Brace", "Purlin", "Wall girt"]}

            Hr{}
            Label{width: Fill text: "Explicit interfaces" draw_text.color: #xb6c8d8}
            View{width: Fill height: 32 flow: Right spacing: 5
                sc_interface_add := Button{width: Fill text: "Add interface"}
                sc_interface_delete := Button{width: Fill text: "Delete"}
            }
            sc_interface_select := DropDown{width: Fill labels: ["No stored interfaces"]}
            sc_interface_list := TextInput{width: Fill height: 90 is_multiline: true is_read_only: true text: "No stored interfaces are loaded."}
            Label{width: Fill text: "Explicit interface endpoints and type" draw_text.color: #xaebdcb}
            Label{width: Fill text: "Section A" draw_text.color: #xaebdcb}
            sc_interface_from := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            Label{width: Fill text: "Edge A · ordered corner IDs" draw_text.color: #xaebdcb}
            View{width: Fill height: 30 flow: Right spacing: 5
                sc_interface_edge_a_start := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
                sc_interface_edge_a_end := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            }
            Label{width: Fill text: "Section B" draw_text.color: #xaebdcb}
            sc_interface_to := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            Label{width: Fill text: "Edge B · ordered corner IDs" draw_text.color: #xaebdcb}
            View{width: Fill height: 30 flow: Right spacing: 5
                sc_interface_edge_b_start := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
                sc_interface_edge_b_end := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            }
            sc_interface_kind := DropDown{width: Fill labels: ["Awaiting stored configuration"]}
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 95 text: "Joint width, m" draw_text.color: #xaebdcb}
                sc_interface_width := TextInput{width: Fill height: 30 empty_text: "m"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                Label{width: 95 text: "B offset X/Y" draw_text.color: #xaebdcb}
                sc_interface_offset_x := TextInput{width: Fill height: 30 empty_text: "unit X"}
                sc_interface_offset_y := TextInput{width: Fill height: 30 empty_text: "unit Y"}
            }
            Label{width: Fill text: "Interface endpoints and kinds come from the stored typed configuration." draw_text.color: #x9badbe}
            View{width: Fill height: 32 flow: Right spacing: 5
                sc_create_demo := Button{width: Fill text: "Create demo"}
                sc_apply := Button{width: Fill text: "Apply + regenerate"}
                sc_reload := Button{width: Fill text: "Reload"}
            }
            View{width: Fill height: 32 flow: Right spacing: 5
                sc_regenerate := Button{width: Fill text: "Regenerate"}
                sc_analyze := Button{width: Fill text: "Analyze selected component"}
            }
        }

        View{width: Fill height: Fill flow: Down
            ScrollXView{width: Fill height: 40 flow: Right spacing: 5 padding: Inset{left: 7 right: 7}
                sc_layout_plan := Button{text: "Plan"}
                sc_layout_split := Button{text: "Plan + 3D"}
                sc_layout_3d := Button{text: "3D"}
                sc_fit_building := Button{text: "Fit building"}
                sc_fit_section := Button{text: "Fit section"}
                sc_hide_cladding := Button{text: "Hide cladding"}
                Label{text: "Cladding transparency" draw_text.color: #xb6c8d8}
                sc_cladding_transparency := Slider{width: 110 height: 28 min: 0 max: 100 step: 1 default: 55 precision: 0}
            }
            ScrollXView{width: Fill height: 40 flow: Right spacing: 5 padding: Inset{left: 7 right: 7}
                sc_view_architectural := Button{text: "Architectural"}
                sc_view_frame := Button{text: "Frame"}
                sc_view_connections := Button{text: "Connections"}
                sc_view_analysis := Button{text: "Analysis"}
                sc_view_loads := Button{text: "Loads"}
                sc_view_reactions := Button{text: "Reactions"}
                sc_view_foundation := Button{text: "Foundation"}
            }
            ScrollXView{width: Fill height: 40 flow: Right spacing: 5 padding: Inset{left: 7 right: 7}
                sc_mode_select := Button{text: "Select"}
                sc_mode_add := Button{text: "Add corner"}
                sc_mode_move := Button{text: "Move corner"}
                sc_mode_insert := Button{text: "Insert on edge"}
                sc_mode_delete := Button{text: "Delete corner"}
                sc_close_outline := Button{text: "Close outline"}
                sc_snap_grid := Button{text: "Grid snap: off"}
                Label{text: "Grid step, m" draw_text.color: #xb6c8d8}
                sc_grid_step := TextInput{width: 72 height: 30 empty_text: "0.100 m"}
            }

            Label{width: Fill height: 28 text: "MODEL_INCOMPLETE · no stored building configuration; no result overlays" draw_text.color: #xf2c47c}
            sc_binding := Label{width: Fill height: 24 text: "No Document binding" draw_text.color: #x9badbe}
            sc_summary := Label{width: Fill height: 48 text: "MODEL_INCOMPLETE · no current generated or analysis result" draw_text.color: #x9badbe}
            Label{width: Fill height: 32 text: "Synthetic BIM display · ULS/SLS/code checks, panel stiffness and load transfer are not verified." draw_text.color: #xe4b77a}
            View{width: Fill height: Fill flow: Right spacing: 6 padding: 6
                sc_plan_host := View{width: Fill height: Fill flow: Down draw_bg.color: #x17212b
                    sc_plan_title := Label{width: Fill height: 28 text: "Plan · building-local XY · exact meters" draw_text.color: #xb6c8d8}
                    sc_plan_canvas := mod.widgets.DraftingCanvas{}
                }
                sc_scene_host := View{width: Fill height: Fill flow: Down draw_bg.color: #x17212b
                    sc_scene_title := Label{width: Fill height: 28 text: "3D · waiting for stored configuration" draw_text.color: #xb6c8d8}
                    sc_model_viewport := mod.widgets.CadViewport{}
                }
            }
            View{width: Fill height: 36 flow: Right spacing: 5 padding: Inset{left: 7 right: 7}
                sc_vertex_select := DropDown{width: 160 labels: ["No stored outline"]}
                Label{width: Fit text: "Selected vertex XY, m" draw_text.color: #xb6c8d8}
                sc_vertex_x := TextInput{width: 110 height: 30 empty_text: "X, m"}
                sc_vertex_y := TextInput{width: 110 height: 30 empty_text: "Y, m"}
                sc_apply_coordinates := Button{text: "Set exact XY"}
                sc_vertex_delete := Button{text: "Delete vertex"}
            }
            ScrollYView{width: Fill height: 150 flow: Down spacing: 4 padding: 7
                draw_bg.color: #x17212b
                Label{width: Fill text: "Ordered polygon corners · exact coordinates in meters" draw_text.color: #xb6c8d8}
                sc_vertex_table := TextInput{width: Fill height: 112 is_multiline: true is_read_only: true text: "No outline is loaded."}
            }
        }

        ScrollYView{width: 340 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            H3{width: Fill text: "Inspector / report" draw_text.color: #xe5edf6}
            sc_status := Label{width: Fill height: Fit text: "MODEL_INCOMPLETE · no stored configuration" draw_text.color: #xf2c47c}
            sc_selection_inspector := TextInput{width: Fill height: 150 is_multiline: true is_read_only: true text: "No section or corner selected."}
            Hr{}
            Label{width: Fill text: "Validation and provenance" draw_text.color: #xb6c8d8}
            sc_validation_report := TextInput{width: Fill height: 220 is_multiline: true is_read_only: true text: "No stored configuration to validate."}
            Label{width: Fill text: "Display and analysis limitations" draw_text.color: #xb6c8d8}
            sc_report := TextInput{width: Fill height: 340 is_multiline: true is_read_only: true text: "No model or analysis results are loaded. Visualization and local input validation do not establish engineering acceptance."}
        }
    }
}

fn sc_positive_number(value: &str, label: &str) -> Result<f64, String> {
    let value = sc_parse_number(value, label)?;
    if value <= 0.0 {
        return Err(format!("{label}: value must be greater than zero"));
    }
    Ok(value)
}

fn sc_grid_spacing_from_text(
    current: &GridSpacing,
    value: &str,
    label: &str,
) -> Result<GridSpacing, String> {
    if value.trim().starts_with("Explicit stations") {
        return Ok(current.clone());
    }
    Ok(GridSpacing::Uniform {
        pitch_m: sc_positive_number(value, label)?,
    })
}

fn sc_distance_to_segment(point: DraftPoint, start: DraftPoint, end: DraftPoint) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let denominator = dx * dx + dy * dy;
    if !denominator.is_finite() || denominator <= f64::EPSILON {
        return point.distance(start);
    }
    let t = (((point.x - start.x) * dx + (point.y - start.y) * dy) / denominator).clamp(0.0, 1.0);
    point.distance(DraftPoint::new(start.x + t * dx, start.y + t * dy))
}

fn sc_id_label(id: &SemanticId) -> String {
    id.as_str().to_owned()
}

fn sc_analysis_attempt_code(message: &str) -> &'static str {
    for code in [
        "ANALYSIS_MODEL_CAPACITY_EXCEEDED",
        "INTERFACE_LOAD_PATH_NOT_VERIFIED",
        "GENERATION_BLOCKED",
        "DISCONNECTED_LOAD_PATHS_NOT_ACKNOWLEDGED",
    ] {
        if message.contains(code) {
            return code;
        }
    }
    let lower = message.to_ascii_lowercase();
    if lower.contains("source manifest")
        || lower.contains("executable")
        || lower.contains("source identity")
    {
        "SOURCE_IDENTITY_HOLD"
    } else if lower.contains("binding")
        || lower.contains("reload")
        || lower.contains("document changed")
    {
        "INVALID_INPUT"
    } else {
        "ANALYSIS_ATTEMPT_FAILED"
    }
}

impl App {
    pub(super) fn handle_structural_configurator_actions(
        &mut self,
        cx: &mut Cx,
        actions: &Actions,
    ) {
        if self
            .ui
            .button(cx, ids!(workspace_structural_configurator))
            .clicked(actions)
        {
            self.structural_configurator_set_active(cx, !self.structural_configurator_ui.active);
            return;
        }

        if self.structural_configurator_ui.active {
            let switched = [
                id!(workspace_3d),
                id!(workspace_2d),
                id!(workspace_engineering),
                id!(workspace_circular),
                id!(workspace_foundation_structural),
            ]
            .into_iter()
            .any(|id| self.ui.button(cx, &[id]).clicked(actions));
            if switched {
                self.structural_configurator_set_active(cx, false);
                return;
            }
        }
        if !self.structural_configurator_ui.active {
            return;
        }

        if self.ui.button(cx, ids!(sc_create_demo)).clicked(actions) {
            self.structural_configurator_execute_intent(cx, sc_api::ScIntent::CreateDemo, None);
            return;
        }
        if self.ui.button(cx, ids!(sc_reload)).clicked(actions) {
            self.structural_configurator_reload(cx);
            return;
        }
        if self.ui.button(cx, ids!(sc_regenerate)).clicked(actions)
            || self.ui.button(cx, ids!(sc_apply)).clicked(actions)
        {
            self.structural_configurator_regenerate(cx);
            return;
        }
        if self.ui.button(cx, ids!(sc_analyze)).clicked(actions) {
            self.structural_configurator_analyze(cx);
            return;
        }

        let mut selection_changed = false;
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(sc_section_select))
            .selected(actions)
        {
            if self.structural_configurator_ui.coordinate_draft_dirty
                || self.structural_configurator_ui.local_input_error.is_some()
            {
                if let Some(current) = self
                    .structural_configurator_ui
                    .selected_section
                    .as_ref()
                    .and_then(|id| {
                        self.structural_configurator_ui
                            .section_ids
                            .iter()
                            .position(|value| value == id)
                    })
                {
                    self.ui
                        .drop_down(cx, ids!(sc_section_select))
                        .set_selected_item(cx, current);
                }
                self.structural_configurator_set_status(cx, "INVALID_INPUT", "Resolve the current text draft before switching sections; its text is retained.");
            } else {
                self.structural_configurator_ui.selected_section = self
                    .structural_configurator_ui
                    .section_ids
                    .get(index)
                    .cloned();
                self.structural_configurator_ui.selected_corner = None;
                self.structural_configurator_ui.selection = None;
                self.structural_configurator_update_selectors(cx);
                self.structural_configurator_fill_form(cx);
                selection_changed = true;
            }
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(sc_vertex_select))
            .selected(actions)
        {
            if self.structural_configurator_ui.coordinate_draft_dirty {
                if let Some(current) = self
                    .structural_configurator_ui
                    .selected_corner
                    .as_ref()
                    .and_then(|id| {
                        self.structural_configurator_ui
                            .corner_ids
                            .iter()
                            .position(|value| value == id)
                    })
                {
                    self.ui
                        .drop_down(cx, ids!(sc_vertex_select))
                        .set_selected_item(cx, current);
                }
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "Apply or discard the typed XY draft before selecting another corner.",
                );
            } else {
                self.structural_configurator_ui.selected_corner = self
                    .structural_configurator_ui
                    .corner_ids
                    .get(index)
                    .cloned();
                self.structural_configurator_fill_vertex_fields(cx);
                selection_changed = true;
            }
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(sc_interface_select))
            .selected(actions)
        {
            if self.structural_configurator_ui.local_input_error.is_some() {
                if let Some(current) = self
                    .structural_configurator_ui
                    .selected_interface
                    .as_ref()
                    .and_then(|id| {
                        self.structural_configurator_ui
                            .interface_ids
                            .iter()
                            .position(|value| value == id)
                    })
                {
                    self.ui
                        .drop_down(cx, ids!(sc_interface_select))
                        .set_selected_item(cx, current);
                }
                self.structural_configurator_set_status(cx, "INVALID_INPUT", "Resolve the current text draft before switching interfaces; its text is retained.");
            } else {
                self.structural_configurator_ui.selected_interface = self
                    .structural_configurator_ui
                    .interface_ids
                    .get(index)
                    .cloned();
                self.structural_configurator_update_selectors(cx);
                self.structural_configurator_fill_interface_fields(cx);
                selection_changed = true;
            }
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(sc_member_role))
            .selected(actions)
        {
            self.structural_configurator_ui.member_role = SC_MEMBER_ROLES
                .get(index)
                .copied()
                .unwrap_or(MemberRole::Column);
            self.structural_configurator_fill_member_fields(cx);
            selection_changed = true;
        }

        let mut config_changed = false;
        for id in [
            id!(sc_base_z),
            id!(sc_eave_height),
            id!(sc_ridge_max_height),
            id!(sc_roof_direction_x),
            id!(sc_roof_direction_y),
            id!(sc_frame_pitch),
            id!(sc_column_pitch),
            id!(sc_purlin_pitch),
            id!(sc_wall_girt_pitch),
            id!(sc_interface_width),
            id!(sc_interface_offset_x),
            id!(sc_interface_offset_y),
        ] {
            if self.ui.text_input(cx, &[id]).changed(actions).is_some() {
                config_changed = true;
            }
        }
        let interface_sections_changed = self
            .ui
            .drop_down(cx, ids!(sc_interface_from))
            .changed(actions)
            .is_some()
            || self
                .ui
                .drop_down(cx, ids!(sc_interface_to))
                .changed(actions)
                .is_some();
        if interface_sections_changed {
            self.structural_configurator_update_interface_edge_selectors(cx);
            config_changed = true;
        }
        let dropdown_edits = [
            self.ui
                .drop_down(cx, ids!(sc_roof_kind))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_material))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_profile))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_a_start))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_a_end))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_b_start))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_b_end))
                .changed(actions)
                .is_some(),
            self.ui
                .drop_down(cx, ids!(sc_interface_kind))
                .changed(actions)
                .is_some(),
        ];
        config_changed |= dropdown_edits.into_iter().any(|changed| changed);
        if self.ui.button(cx, ids!(sc_braces)).clicked(actions) {
            self.structural_configurator_toggle_braces(cx);
            return;
        }
        if config_changed {
            self.structural_configurator_mark_draft(cx);
            return;
        }

        let coordinate_changed = self
            .ui
            .text_input(cx, ids!(sc_vertex_x))
            .changed(actions)
            .is_some()
            || self
                .ui
                .text_input(cx, ids!(sc_vertex_y))
                .changed(actions)
                .is_some();
        if coordinate_changed {
            self.structural_configurator_ui.coordinate_draft_dirty = true;
            self.structural_configurator_mark_unparsed_draft(cx);
            return;
        }
        if self
            .ui
            .button(cx, ids!(sc_apply_coordinates))
            .clicked(actions)
        {
            self.structural_configurator_apply_coordinates(cx);
            return;
        }
        if self.ui.button(cx, ids!(sc_vertex_delete)).clicked(actions) {
            self.structural_configurator_delete_selected_corner(cx);
            return;
        }

        if self.ui.button(cx, ids!(sc_section_add)).clicked(actions) {
            self.structural_configurator_add_section(cx);
            return;
        }
        if self.ui.button(cx, ids!(sc_section_delete)).clicked(actions) {
            self.structural_configurator_delete_section(cx);
            return;
        }
        if self.ui.button(cx, ids!(sc_interface_add)).clicked(actions) {
            self.structural_configurator_add_interface(cx);
            return;
        }
        if self
            .ui
            .button(cx, ids!(sc_interface_delete))
            .clicked(actions)
        {
            self.structural_configurator_delete_interface(cx);
            return;
        }

        if let Some(grid_step) = self.ui.text_input(cx, ids!(sc_grid_step)).changed(actions) {
            if let Ok(step) = sc_positive_number(&grid_step, "Grid step") {
                self.structural_configurator_ui.grid_step_m = step;
                self.structural_configurator_set_status(cx, "PRESENTATION_ONLY", "Grid snap applies only to pointer placement; exact XY fields remain unsnapped.");
            }
        }
        if self.ui.button(cx, ids!(sc_snap_grid)).clicked(actions) {
            self.structural_configurator_ui.snap_enabled =
                !self.structural_configurator_ui.snap_enabled;
            self.ui.button(cx, ids!(sc_snap_grid)).set_text(
                cx,
                if self.structural_configurator_ui.snap_enabled {
                    "Grid snap: on"
                } else {
                    "Grid snap: off"
                },
            );
        }
        if self.ui.button(cx, ids!(sc_hide_cladding)).clicked(actions) {
            self.structural_configurator_ui.hide_cladding =
                !self.structural_configurator_ui.hide_cladding;
            self.structural_configurator_render_scene(cx, false);
        }
        if let Some(value) = self
            .ui
            .slider(cx, ids!(sc_cladding_transparency))
            .slided(actions)
        {
            if value.is_finite() {
                self.structural_configurator_ui.cladding_alpha =
                    (1.0 - value / 100.0).clamp(0.0, 1.0) as f32;
                self.structural_configurator_render_scene(cx, false);
            }
        }

        for (id, view) in [
            (id!(sc_view_architectural), ScViewMode::Architectural),
            (id!(sc_view_frame), ScViewMode::Frame),
            (id!(sc_view_connections), ScViewMode::Connections),
            (id!(sc_view_analysis), ScViewMode::Analysis),
            (id!(sc_view_loads), ScViewMode::Loads),
            (id!(sc_view_reactions), ScViewMode::Reactions),
            (id!(sc_view_foundation), ScViewMode::Foundation),
        ] {
            if self.ui.button(cx, &[id]).clicked(actions) {
                self.structural_configurator_ui.view = view;
                self.structural_configurator_render_scene(cx, false);
                self.structural_configurator_apply_visibility(cx);
                break;
            }
        }
        for (id, mode) in [
            (id!(sc_mode_select), ScPolygonMode::Select),
            (id!(sc_mode_add), ScPolygonMode::Add),
            (id!(sc_mode_move), ScPolygonMode::Move),
            (id!(sc_mode_insert), ScPolygonMode::Insert),
            (id!(sc_mode_delete), ScPolygonMode::Delete),
        ] {
            if self.ui.button(cx, &[id]).clicked(actions) {
                self.structural_configurator_ui.polygon_mode = mode;
                self.structural_configurator_draw_plan(cx);
                break;
            }
        }
        if self.ui.button(cx, ids!(sc_close_outline)).clicked(actions) {
            self.structural_configurator_set_status(cx, "PRESENTATION_ONLY", "The ordered corner ring is implicitly closed; no duplicate closing vertex is stored.");
        }
        if self.ui.button(cx, ids!(sc_layout_plan)).clicked(actions) {
            self.structural_configurator_render_layout(cx, ScCanvasLayout::PlanOnly);
        }
        if self.ui.button(cx, ids!(sc_layout_split)).clicked(actions) {
            self.structural_configurator_render_layout(cx, ScCanvasLayout::Split);
        }
        if self.ui.button(cx, ids!(sc_layout_3d)).clicked(actions) {
            self.structural_configurator_render_layout(cx, ScCanvasLayout::ThreeD);
        }
        if self.ui.button(cx, ids!(sc_fit_building)).clicked(actions) {
            self.structural_configurator_fit(cx, ScFitTarget::Building);
        }
        if self.ui.button(cx, ids!(sc_fit_section)).clicked(actions) {
            if let Some(section_id) = self.structural_configurator_ui.selected_section.clone() {
                self.structural_configurator_fit(cx, ScFitTarget::Section(section_id));
            } else {
                self.structural_configurator_set_status(
                    cx,
                    "MODEL_INCOMPLETE",
                    "Select a section before Fit section.",
                );
            }
        }

        let canvas_actions = self
            .ui
            .drafting_canvas(cx, ids!(sc_plan_canvas))
            .actions(actions);
        for action in canvas_actions {
            self.structural_configurator_canvas_action(cx, action);
        }

        let viewport_actions = self
            .ui
            .cad_viewport(cx, ids!(sc_model_viewport))
            .actions(actions);
        let mut picked_object = None;
        let mut selection_changed = false;
        for action in viewport_actions {
            match action {
                CadViewportAction::Viewer(
                    ViewerEvent::ObjectPicked { .. } | ViewerEvent::SelectionChanged { .. },
                ) => selection_changed = true,
                CadViewportAction::Inspect(object_id) => {
                    picked_object = Some(object_id);
                    selection_changed = true;
                }
                _ => {}
            }
        }
        if selection_changed {
            if picked_object.is_none() {
                picked_object = self
                    .ui
                    .cad_viewport(cx, ids!(sc_model_viewport))
                    .borrow()
                    .and_then(|viewport| viewport.viewer().scene().selected_ids().first().copied());
            }
            self.structural_configurator_ui.selection = picked_object
                .and_then(|object_id| self.structural_configurator_ui.scene_picks.get(&object_id))
                .cloned();
            self.structural_configurator_update_inspector(cx);
        }

        if selection_changed {
            self.structural_configurator_draw_plan(cx);
        }
    }

    pub(super) fn structural_configurator_apply_visibility(&self, cx: &mut Cx) {
        let active = self.structural_configurator_ui.active;
        self.ui
            .widget(cx, ids!(sc_workspace))
            .set_visible(cx, active);
        self.ui
            .button(cx, ids!(workspace_structural_configurator))
            .set_text(
                cx,
                if active {
                    "● Steel buildings"
                } else {
                    "Steel buildings"
                },
            );
        if !active {
            return;
        }
        self.ui.widget(cx, ids!(sc_plan_host)).set_visible(
            cx,
            self.structural_configurator_ui.layout != ScCanvasLayout::ThreeD,
        );
        self.ui.widget(cx, ids!(sc_scene_host)).set_visible(
            cx,
            self.structural_configurator_ui.layout != ScCanvasLayout::PlanOnly,
        );
        for id in [
            ids!(history_panel),
            ids!(inspector_panel),
            ids!(viewport_panel),
            ids!(geotech_parameters),
            ids!(geotech_results),
            ids!(geotech_actions),
            ids!(circular_parameters_host),
            ids!(circular_results),
            ids!(circular_actions),
            ids!(circular_view_status),
            ids!(model_toolbar),
            ids!(drafting_toolbar),
            ids!(drafting_command_panel),
            ids!(viewport_3d_host),
            ids!(drafting_2d_host),
            ids!(fs_workspace),
        ] {
            self.ui.widget(cx, id).set_visible(cx, false);
        }
        let can_edit = self.document.is_some()
            && !self.structural_configurator_ui.binding_stale
            && self.structural_configurator_ui.draft.is_some();
        for id in [
            id!(sc_base_z),
            id!(sc_eave_height),
            id!(sc_ridge_max_height),
            id!(sc_roof_direction_x),
            id!(sc_roof_direction_y),
            id!(sc_frame_pitch),
            id!(sc_column_pitch),
            id!(sc_purlin_pitch),
            id!(sc_wall_girt_pitch),
            id!(sc_interface_width),
            id!(sc_interface_offset_x),
            id!(sc_interface_offset_y),
        ] {
            self.ui
                .text_input(cx, &[id])
                .set_is_read_only(cx, !can_edit);
        }
        for id in [id!(sc_vertex_x), id!(sc_vertex_y), id!(sc_grid_step)] {
            self.ui.text_input(cx, &[id]).set_is_read_only(
                cx,
                if id == id!(sc_grid_step) {
                    false
                } else {
                    !can_edit
                },
            );
        }
        self.ui
            .button(cx, ids!(sc_create_demo))
            .set_enabled(cx, self.document.is_some());
        self.ui.button(cx, ids!(sc_apply)).set_enabled(cx, can_edit);
        self.ui
            .button(cx, ids!(sc_regenerate))
            .set_enabled(cx, can_edit);
        self.ui.button(cx, ids!(sc_analyze)).set_enabled(
            cx,
            self.document.is_some()
                && !self.structural_configurator_ui.draft_dirty
                && !self.structural_configurator_ui.binding_stale
                && self
                    .structural_configurator_ui
                    .projection
                    .as_ref()
                    .is_some_and(|p| p.generated_current),
        );
        for id in [
            id!(sc_section_add),
            id!(sc_section_delete),
            id!(sc_interface_add),
            id!(sc_interface_delete),
            id!(sc_vertex_delete),
            id!(sc_apply_coordinates),
        ] {
            self.ui.button(cx, &[id]).set_enabled(cx, can_edit);
        }
        self.ui
            .button(cx, ids!(sc_reload))
            .set_enabled(cx, self.document.is_some());
    }

    pub(super) fn structural_configurator_after_event(&mut self, cx: &mut Cx, _event: &Event) {
        if !self.structural_configurator_ui.active {
            return;
        }
        let Some(document) = self.document.as_ref() else {
            self.structural_configurator_ui.binding_stale =
                self.structural_configurator_ui.draft_dirty;
            self.structural_configurator_ui.binding = None;
            self.structural_configurator_ui.projection = None;
            self.structural_configurator_ui.selection = None;
            if !self.structural_configurator_ui.draft_dirty {
                self.structural_configurator_ui.draft = None;
                self.structural_configurator_clear_editor(cx);
            }
            self.structural_configurator_ui.scene_picks.clear();
            self.structural_configurator_ui.scene_sources.clear();
            self.structural_configurator_ui.scene_graph_fingerprint = None;
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "No current Document analysis is available. Open or create a Document; a retained draft remains unbound and cannot be applied.",
            );
            self.ui.label(cx, ids!(sc_binding)).set_text(
                cx,
                if self.structural_configurator_ui.draft_dirty {
                    "No Document open · retained draft is unbound"
                } else {
                    "No Document open · no active binding"
                },
            );
            self.ui
                .text_input(cx, ids!(sc_report))
                .set_text(cx, "MODEL_INCOMPLETE · no current Document analysis is available. No structural result is shown.");
            self.ui.label(cx, ids!(sc_summary)).set_text(cx, if self.structural_configurator_ui.draft_dirty {
                "MODEL_INCOMPLETE · no current Document input or analysis is available; the retained draft is unbound."
            } else {
                "MODEL_INCOMPLETE · no current Document input or analysis is available."
            });
            self.structural_configurator_clear_scene(cx);
            self.ui
                .label(cx, ids!(sc_scene_title))
                .set_text(cx, "3D · no current Document result");
            self.structural_configurator_draw_plan(cx);
            self.structural_configurator_apply_visibility(cx);
            return;
        };

        let projection = match sc_api::projection(document, self.api_document_generation) {
            Ok(projection) => projection,
            Err(error) => {
                if self.structural_configurator_ui.draft_dirty {
                    self.structural_configurator_ui.binding_stale = true;
                    self.structural_configurator_set_status(cx, "STALE_BINDING", &error);
                    self.ui.label(cx, ids!(sc_binding)).set_text(
                        cx,
                        "STALE DRAFT · projection validation failed · reload required",
                    );
                    self.ui.text_input(cx, ids!(sc_report)).set_text(cx, &format!("MODEL_INCOMPLETE · stored projection could not be refreshed: {error}.\n\nThe existing draft is retained and cannot be rebased. Any cached graph is reference-only; analysis overlays are suppressed."));
                    self.structural_configurator_render_scene(cx, false);
                } else {
                    self.structural_configurator_ui.projection = None;
                    self.structural_configurator_ui.draft = None;
                    self.structural_configurator_ui.binding = None;
                    self.structural_configurator_set_status(cx, "PROJECTION_FAILURE", &error);
                    self.structural_configurator_clear_scene(cx);
                    self.ui
                        .label(cx, ids!(sc_scene_title))
                        .set_text(cx, "3D · projection unavailable");
                }
                self.structural_configurator_ui.local_input_error = Some(error);
                self.structural_configurator_apply_visibility(cx);
                return;
            }
        };
        let Some(projection) = projection else {
            if !self.structural_configurator_ui.draft_dirty {
                self.structural_configurator_ui.draft = None;
                self.structural_configurator_ui.binding = None;
                self.structural_configurator_ui.selected_section = None;
                self.structural_configurator_ui.selected_corner = None;
                self.structural_configurator_ui.selected_interface = None;
                self.structural_configurator_clear_editor(cx);
            } else {
                self.structural_configurator_ui.binding_stale = true;
            }
            if !self.structural_configurator_ui.draft_dirty {
                self.structural_configurator_ui.projection = None;
            }
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "No typed building configuration is stored in this Document.",
            );
            self.ui
                .label(cx, ids!(sc_binding))
                .set_text(cx, "Document open · no stored configurator input");
            self.ui.label(cx, ids!(sc_summary)).set_text(cx, if self.structural_configurator_ui.draft_dirty {
                "MODEL_INCOMPLETE · current Document input is absent; retained draft is not rebased and cached graph is reference-only."
            } else {
                "MODEL_INCOMPLETE · no stored configuration or current analysis result."
            });
            self.ui.text_input(cx, ids!(sc_report)).set_text(cx, if self.structural_configurator_ui.draft_dirty {
                "MODEL_INCOMPLETE · the current Document no longer contains the bound input. The retained draft is not rebased; any prior graph is reference-only and not analyzed." 
            } else {
                "MODEL_INCOMPLETE · input only · no generated graph or analysis result is available."
            });
            if !self.structural_configurator_ui.draft_dirty {
                self.structural_configurator_clear_scene(cx);
            } else {
                self.structural_configurator_render_scene(cx, false);
            }
            if !self.structural_configurator_ui.draft_dirty {
                self.structural_configurator_update_selectors(cx);
            }
            self.structural_configurator_apply_visibility(cx);
            return;
        };

        if self.structural_configurator_ui.draft_dirty {
            if self.structural_configurator_ui.binding.as_ref() != Some(&projection.binding) {
                self.structural_configurator_ui.binding_stale = true;
            }
            if self.structural_configurator_ui.binding_stale {
                self.structural_configurator_set_status(cx, "MODEL_INCOMPLETE", "The stored Document changed while the draft was being edited. Reload before applying; the draft is retained.");
                self.ui.label(cx, ids!(sc_binding)).set_text(
                    cx,
                    "STALE DRAFT · stored Document binding changed · reload required",
                );
                self.ui.label(cx, ids!(sc_summary)).set_text(cx, "STALE DRAFT · cached generated graph is reference-only; no analysis overlay is current for this draft.");
                let bound_report = self
                    .structural_configurator_ui
                    .projection
                    .as_ref()
                    .map(|old| old.report.as_str())
                    .unwrap_or("No prior stored report is cached.");
                self.ui.text_input(cx, ids!(sc_report)).set_text(cx, &format!("{bound_report}\n\nThe visible draft is retained and is not rebased. The cached graph is reference-only; analysis overlays are suppressed."));
                self.structural_configurator_draw_plan(cx);
                self.structural_configurator_render_scene(cx, false);
                self.structural_configurator_apply_visibility(cx);
                return;
            }
        } else {
            let changed_input =
                self.structural_configurator_ui
                    .binding
                    .as_ref()
                    .map_or(true, |previous| {
                        previous.document_uuid != projection.binding.document_uuid
                            || previous.runtime_id != projection.binding.runtime_id
                            || previous.open_generation != projection.binding.open_generation
                            || previous.input_revision != projection.binding.input_revision
                            || previous.input_fingerprint != projection.binding.input_fingerprint
                    });
            self.structural_configurator_ui.binding = Some(projection.binding.clone());
            self.structural_configurator_ui.binding_stale = false;
            if changed_input || self.structural_configurator_ui.draft.is_none() {
                self.structural_configurator_ui.draft = Some(projection.config.clone());
                self.structural_configurator_ui.selected_section = projection
                    .config
                    .sections
                    .first()
                    .map(|section| section.id.clone());
                self.structural_configurator_ui.selected_corner = None;
                self.structural_configurator_ui.selected_interface = projection
                    .config
                    .interfaces
                    .first()
                    .map(|interface| interface.id.clone());
                self.structural_configurator_fill_form(cx);
                self.structural_configurator_ui.plan_needs_fit = true;
                self.structural_configurator_ui.pending_fit = Some(ScFitTarget::Building);
            }
        }
        self.structural_configurator_ui.projection = Some(projection.clone());
        if !self.structural_configurator_ui.draft_dirty {
            self.structural_configurator_update_selectors(cx);
        }
        self.structural_configurator_update_validation(cx);
        self.structural_configurator_update_labels(cx);
        self.structural_configurator_update_inspector(cx);
        self.ui
            .text_input(cx, ids!(sc_report))
            .set_text(cx, &projection.report);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
        self.structural_configurator_apply_visibility(cx);
    }

    pub(super) fn structural_configurator_set_active(&mut self, cx: &mut Cx, active: bool) {
        if active {
            self.foundation_structural_set_active(cx, false);
            let _ = self.set_workspace(cx, "3d");
        }
        self.structural_configurator_ui.active = active;
        cx.revert_key_focus();
        if active {
            self.structural_configurator_ui.plan_needs_fit = true;
            self.structural_configurator_after_event(cx, &Event::Startup);
            self.structural_configurator_apply_visibility(cx);
        } else {
            self.structural_configurator_ui.pan_anchor = None;
            self.structural_configurator_ui.dragging_corner = false;
            self.refresh_workspace_visibility(cx);
            self.refresh_drafting(cx, true);
            self.structural_configurator_apply_visibility(cx);
        }
    }
}

impl App {
    fn structural_configurator_set_status(&mut self, cx: &mut Cx, code: &str, message: &str) {
        self.structural_configurator_ui.attempt_status = Some(code.to_owned());
        self.structural_configurator_ui.attempt_message = message.to_owned();
        self.ui
            .label(cx, ids!(sc_status))
            .set_text(cx, &format!("{code} · {message}"));
    }

    fn structural_configurator_can_edit(&self) -> bool {
        self.document.is_some()
            && self.structural_configurator_ui.binding.is_some()
            && !self.structural_configurator_ui.binding_stale
            && self.structural_configurator_ui.draft.is_some()
    }

    fn structural_configurator_can_change_geometry(&self) -> bool {
        self.structural_configurator_can_edit()
            && !self.structural_configurator_ui.coordinate_draft_dirty
            && self.structural_configurator_ui.local_input_error.is_none()
    }

    fn structural_configurator_clear_editor(&mut self, cx: &mut Cx) {
        for id in [
            id!(sc_base_z),
            id!(sc_eave_height),
            id!(sc_ridge_max_height),
            id!(sc_roof_direction_x),
            id!(sc_roof_direction_y),
            id!(sc_frame_pitch),
            id!(sc_column_pitch),
            id!(sc_purlin_pitch),
            id!(sc_wall_girt_pitch),
            id!(sc_interface_width),
            id!(sc_interface_offset_x),
            id!(sc_interface_offset_y),
            id!(sc_vertex_x),
            id!(sc_vertex_y),
        ] {
            self.ui.text_input(cx, &[id]).set_text(cx, "");
        }
        self.ui
            .text_input(cx, ids!(sc_section_list))
            .set_text(cx, "No stored sections are loaded.");
        self.ui
            .text_input(cx, ids!(sc_interface_list))
            .set_text(cx, "No stored interfaces are loaded.");
        self.ui
            .text_input(cx, ids!(sc_vertex_table))
            .set_text(cx, "No outline is loaded.");
        self.structural_configurator_ui.section_ids.clear();
        self.structural_configurator_ui.corner_ids.clear();
        self.structural_configurator_ui.material_ids.clear();
        self.structural_configurator_ui.profile_ids.clear();
        self.structural_configurator_ui.interface_ids.clear();
        self.structural_configurator_ui
            .interface_section_a_ids
            .clear();
        self.structural_configurator_ui
            .interface_section_b_ids
            .clear();
        self.structural_configurator_ui.interface_edge_a_ids.clear();
        self.structural_configurator_ui.interface_edge_b_ids.clear();
        self.structural_configurator_ui.selected_section = None;
        self.structural_configurator_ui.selected_corner = None;
        self.structural_configurator_ui.selected_interface = None;
        self.ui
            .drop_down(cx, ids!(sc_section_select))
            .set_labels(cx, vec!["No stored sections".into()]);
        self.ui
            .drop_down(cx, ids!(sc_vertex_select))
            .set_labels(cx, vec!["No stored outline".into()]);
        self.ui
            .drop_down(cx, ids!(sc_interface_select))
            .set_labels(cx, vec!["No stored interfaces".into()]);
        for id in [
            id!(sc_material),
            id!(sc_profile),
            id!(sc_interface_from),
            id!(sc_interface_to),
            id!(sc_interface_edge_a_start),
            id!(sc_interface_edge_a_end),
            id!(sc_interface_edge_b_start),
            id!(sc_interface_edge_b_end),
        ] {
            self.ui
                .drop_down(cx, &[id])
                .set_labels(cx, vec!["No stored values".into()]);
        }
    }

    fn structural_configurator_update_selectors(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            return;
        };
        let config = config.clone();
        let selected_section = self.structural_configurator_ui.selected_section.clone();
        let selected_interface = self.structural_configurator_ui.selected_interface.clone();
        self.structural_configurator_ui.section_ids = config
            .sections
            .iter()
            .map(|section| section.id.clone())
            .collect();
        let section_labels: Vec<String> = config
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| format!("{} · {}", index + 1, section.id))
            .collect();
        self.ui.drop_down(cx, ids!(sc_section_select)).set_labels(
            cx,
            if section_labels.is_empty() {
                vec!["No stored sections".into()]
            } else {
                section_labels
            },
        );
        if let Some(index) = selected_section.as_ref().and_then(|id| {
            self.structural_configurator_ui
                .section_ids
                .iter()
                .position(|value| value == id)
        }) {
            self.ui
                .drop_down(cx, ids!(sc_section_select))
                .set_selected_item(cx, index);
        }
        let mut section_rows = Vec::new();
        for (index, section) in config.sections.iter().enumerate() {
            section_rows.push(format!(
                "{} · {} · {} ordered corners · base {:.3} m · eave {:.3} m · ridge {:.3} m · {:?}",
                index + 1,
                section.id,
                section.corners.len(),
                section.roof.base_abs_m,
                section.roof.eave_abs_m,
                section.roof.ridge_abs_m,
                section.roof.kind,
            ));
        }
        let section_text = if section_rows.is_empty() {
            "No sections are stored.".to_owned()
        } else {
            section_rows.join("\n")
        };
        self.ui
            .text_input(cx, ids!(sc_section_list))
            .set_text(cx, &section_text);

        self.structural_configurator_ui.corner_ids = selected_section
            .as_ref()
            .and_then(|id| sc_section_by_id(&config, id))
            .map(|section| {
                section
                    .corners
                    .iter()
                    .map(|corner| corner.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let corner_labels: Vec<String> = selected_section
            .as_ref()
            .and_then(|id| sc_section_by_id(&config, id))
            .map(|section| {
                section
                    .corners
                    .iter()
                    .enumerate()
                    .map(|(index, corner)| {
                        format!(
                            "{} · {} · ({:.3}, {:.3}) m",
                            index + 1,
                            corner.id,
                            corner.xy_m[0],
                            corner.xy_m[1]
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.ui.drop_down(cx, ids!(sc_vertex_select)).set_labels(
            cx,
            if corner_labels.is_empty() {
                vec!["No stored outline".into()]
            } else {
                corner_labels
            },
        );
        if let Some(index) = self
            .structural_configurator_ui
            .selected_corner
            .as_ref()
            .and_then(|id| {
                self.structural_configurator_ui
                    .corner_ids
                    .iter()
                    .position(|value| value == id)
            })
        {
            self.ui
                .drop_down(cx, ids!(sc_vertex_select))
                .set_selected_item(cx, index);
        }
        let vertex_rows: Vec<String> = selected_section
            .as_ref()
            .and_then(|id| sc_section_by_id(&config, id))
            .map(|section| {
                section
                    .corners
                    .iter()
                    .enumerate()
                    .map(|(index, corner)| {
                        format!(
                            "{} · {} · X {:.6} m · Y {:.6} m",
                            index + 1,
                            corner.id,
                            corner.xy_m[0],
                            corner.xy_m[1]
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let vertex_text = if vertex_rows.is_empty() {
            "No ordered outline corners are loaded.".to_owned()
        } else {
            vertex_rows.join("\n")
        };
        self.ui
            .text_input(cx, ids!(sc_vertex_table))
            .set_text(cx, &vertex_text);

        self.structural_configurator_ui.material_ids = config
            .materials
            .iter()
            .map(|item| item.id.clone())
            .collect();
        let material_labels: Vec<String> = config
            .materials
            .iter()
            .map(|item| format!("{} · {}", item.name, item.id))
            .collect();
        self.ui.drop_down(cx, ids!(sc_material)).set_labels(
            cx,
            if material_labels.is_empty() {
                vec!["No stored material catalog".into()]
            } else {
                material_labels
            },
        );
        self.structural_configurator_ui.profile_ids =
            config.profiles.iter().map(|item| item.id.clone()).collect();
        let profile_labels: Vec<String> = config
            .profiles
            .iter()
            .map(|item| format!("{} · {}", item.name, item.id))
            .collect();
        self.ui.drop_down(cx, ids!(sc_profile)).set_labels(
            cx,
            if profile_labels.is_empty() {
                vec!["No stored profile catalog".into()]
            } else {
                profile_labels
            },
        );
        self.ui.drop_down(cx, ids!(sc_roof_kind)).set_labels(
            cx,
            SC_ROOF_KINDS
                .iter()
                .map(|kind| sc_roof_kind_label(*kind).to_owned())
                .collect(),
        );
        self.ui.drop_down(cx, ids!(sc_member_role)).set_labels(
            cx,
            SC_MEMBER_ROLES
                .iter()
                .map(|role| sc_member_role_label(*role).to_owned())
                .collect(),
        );

        self.structural_configurator_ui.interface_ids = config
            .interfaces
            .iter()
            .map(|item| item.id.clone())
            .collect();
        let interface_labels: Vec<String> = config
            .interfaces
            .iter()
            .enumerate()
            .map(|(index, item)| format!("{} · {}", index + 1, item.id))
            .collect();
        self.ui.drop_down(cx, ids!(sc_interface_select)).set_labels(
            cx,
            if interface_labels.is_empty() {
                vec!["No stored interfaces".into()]
            } else {
                interface_labels
            },
        );
        if let Some(index) = selected_interface.as_ref().and_then(|id| {
            self.structural_configurator_ui
                .interface_ids
                .iter()
                .position(|value| value == id)
        }) {
            self.ui
                .drop_down(cx, ids!(sc_interface_select))
                .set_selected_item(cx, index);
        }
        let section_choices: Vec<String> = config
            .sections
            .iter()
            .map(|section| sc_id_label(&section.id))
            .collect();
        self.structural_configurator_ui.interface_section_a_ids = config
            .sections
            .iter()
            .map(|section| section.id.clone())
            .collect();
        self.structural_configurator_ui.interface_section_b_ids = self
            .structural_configurator_ui
            .interface_section_a_ids
            .clone();
        self.ui.drop_down(cx, ids!(sc_interface_from)).set_labels(
            cx,
            if section_choices.is_empty() {
                vec!["No section".into()]
            } else {
                section_choices.clone()
            },
        );
        self.ui.drop_down(cx, ids!(sc_interface_to)).set_labels(
            cx,
            if section_choices.is_empty() {
                vec!["No section".into()]
            } else {
                section_choices
            },
        );
        if let Some(interface) = selected_interface
            .as_ref()
            .and_then(|id| config.interfaces.iter().find(|item| &item.id == id))
        {
            if let Some(index) = self
                .structural_configurator_ui
                .interface_section_a_ids
                .iter()
                .position(|id| id == &interface.section_a)
            {
                self.ui
                    .drop_down(cx, ids!(sc_interface_from))
                    .set_selected_item(cx, index);
            }
            if let Some(index) = self
                .structural_configurator_ui
                .interface_section_b_ids
                .iter()
                .position(|id| id == &interface.section_b)
            {
                self.ui
                    .drop_down(cx, ids!(sc_interface_to))
                    .set_selected_item(cx, index);
            }
            let corners_a =
                sc_section_by_id(&config, &interface.section_a).map(|section| &section.corners);
            let corners_b =
                sc_section_by_id(&config, &interface.section_b).map(|section| &section.corners);
            self.structural_configurator_ui.interface_edge_a_ids = corners_a
                .map(|corners| corners.iter().map(|corner| corner.id.clone()).collect())
                .unwrap_or_default();
            self.structural_configurator_ui.interface_edge_b_ids = corners_b
                .map(|corners| corners.iter().map(|corner| corner.id.clone()).collect())
                .unwrap_or_default();
            let labels_a: Vec<String> = self
                .structural_configurator_ui
                .interface_edge_a_ids
                .iter()
                .map(sc_id_label)
                .collect();
            let labels_b: Vec<String> = self
                .structural_configurator_ui
                .interface_edge_b_ids
                .iter()
                .map(sc_id_label)
                .collect();
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_a_start))
                .set_labels(
                    cx,
                    if labels_a.is_empty() {
                        vec!["No corners".into()]
                    } else {
                        labels_a.clone()
                    },
                );
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_a_end))
                .set_labels(
                    cx,
                    if labels_a.is_empty() {
                        vec!["No corners".into()]
                    } else {
                        labels_a
                    },
                );
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_b_start))
                .set_labels(
                    cx,
                    if labels_b.is_empty() {
                        vec!["No corners".into()]
                    } else {
                        labels_b.clone()
                    },
                );
            self.ui
                .drop_down(cx, ids!(sc_interface_edge_b_end))
                .set_labels(
                    cx,
                    if labels_b.is_empty() {
                        vec!["No corners".into()]
                    } else {
                        labels_b
                    },
                );
            for (widget, vector, id) in [
                (
                    id!(sc_interface_edge_a_start),
                    &self.structural_configurator_ui.interface_edge_a_ids,
                    &interface.edge_a[0],
                ),
                (
                    id!(sc_interface_edge_a_end),
                    &self.structural_configurator_ui.interface_edge_a_ids,
                    &interface.edge_a[1],
                ),
                (
                    id!(sc_interface_edge_b_start),
                    &self.structural_configurator_ui.interface_edge_b_ids,
                    &interface.edge_b[0],
                ),
                (
                    id!(sc_interface_edge_b_end),
                    &self.structural_configurator_ui.interface_edge_b_ids,
                    &interface.edge_b[1],
                ),
            ] {
                if let Some(index) = vector.iter().position(|value| value == id) {
                    self.ui
                        .drop_down(cx, &[widget])
                        .set_selected_item(cx, index);
                }
            }
            self.ui.drop_down(cx, ids!(sc_interface_kind)).set_labels(
                cx,
                SC_INTERFACE_KIND_LABELS
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
            );
            self.ui
                .drop_down(cx, ids!(sc_interface_kind))
                .set_selected_item(cx, sc_interface_kind_index(&interface.kind));
        } else {
            self.structural_configurator_ui.interface_edge_a_ids.clear();
            self.structural_configurator_ui.interface_edge_b_ids.clear();
            for id in [
                id!(sc_interface_edge_a_start),
                id!(sc_interface_edge_a_end),
                id!(sc_interface_edge_b_start),
                id!(sc_interface_edge_b_end),
            ] {
                self.ui
                    .drop_down(cx, &[id])
                    .set_labels(cx, vec!["No interface endpoints".into()]);
            }
            self.ui.drop_down(cx, ids!(sc_interface_kind)).set_labels(
                cx,
                SC_INTERFACE_KIND_LABELS
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
            );
        }
        let interface_rows: Vec<String> = config
            .interfaces
            .iter()
            .map(|item| {
                format!(
                    "{} · A {} [{} → {}] · B {} [{} → {}] · {:?}",
                    item.id,
                    item.section_a,
                    item.edge_a[0],
                    item.edge_a[1],
                    item.section_b,
                    item.edge_b[0],
                    item.edge_b[1],
                    item.kind,
                )
            })
            .collect();
        let interface_text = if interface_rows.is_empty() {
            "No explicit interfaces are stored.".to_owned()
        } else {
            interface_rows.join("\n")
        };
        self.ui
            .text_input(cx, ids!(sc_interface_list))
            .set_text(cx, &interface_text);
    }

    fn structural_configurator_fill_form(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref().cloned() else {
            self.structural_configurator_clear_editor(cx);
            return;
        };
        if self.structural_configurator_ui.selected_section.is_none() {
            self.structural_configurator_ui.selected_section =
                config.sections.first().map(|section| section.id.clone());
        }
        self.structural_configurator_update_selectors(cx);
        if let Some(section_id) = self.structural_configurator_ui.selected_section.as_ref() {
            if let Some(section) = sc_section_by_id(&config, section_id) {
                for (id, value) in [
                    (id!(sc_base_z), section.roof.base_abs_m),
                    (id!(sc_eave_height), section.roof.eave_abs_m),
                    (id!(sc_ridge_max_height), section.roof.ridge_abs_m),
                    (id!(sc_roof_direction_x), section.grid.span_direction[0]),
                    (id!(sc_roof_direction_y), section.grid.span_direction[1]),
                    (id!(sc_purlin_pitch), section.grid.purlin_pitch_m),
                    (id!(sc_wall_girt_pitch), section.grid.wall_girt_pitch_m),
                ] {
                    self.ui
                        .text_input(cx, &[id])
                        .set_text(cx, &format!("{value:.6}"));
                }
                self.ui
                    .text_input(cx, ids!(sc_frame_pitch))
                    .set_text(cx, &sc_format_grid_spacing(&section.grid.frame_stations));
                self.ui
                    .text_input(cx, ids!(sc_column_pitch))
                    .set_text(cx, &sc_format_grid_spacing(&section.grid.column_stations));
                let roof_index = SC_ROOF_KINDS
                    .iter()
                    .position(|kind| kind == &section.roof.kind)
                    .unwrap_or(0);
                self.ui
                    .drop_down(cx, ids!(sc_roof_kind))
                    .set_selected_item(cx, roof_index);
                self.ui.button(cx, ids!(sc_braces)).set_text(
                    cx,
                    if section.grid.braces {
                        "Braces · on"
                    } else {
                        "Braces · off"
                    },
                );
                self.structural_configurator_fill_member_fields(cx);
            }
        }
        self.structural_configurator_fill_vertex_fields(cx);
        self.structural_configurator_fill_interface_fields(cx);
        self.structural_configurator_ui.coordinate_draft_dirty = false;
        self.structural_configurator_ui.local_input_error = None;
    }

    fn structural_configurator_fill_member_fields(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            return;
        };
        let Some(section_id) = self.structural_configurator_ui.selected_section.as_ref() else {
            return;
        };
        let Some(section) = sc_section_by_id(config, section_id) else {
            return;
        };
        self.ui
            .drop_down(cx, ids!(sc_member_role))
            .set_selected_item(
                cx,
                SC_MEMBER_ROLES
                    .iter()
                    .position(|role| role == &self.structural_configurator_ui.member_role)
                    .unwrap_or(0),
            );
        let rule = sc_member_rule(
            &section.members,
            self.structural_configurator_ui.member_role,
        );
        if let Some(index) = self
            .structural_configurator_ui
            .material_ids
            .iter()
            .position(|id| id == &rule.material_id)
        {
            self.ui
                .drop_down(cx, ids!(sc_material))
                .set_selected_item(cx, index);
        }
        if let Some(index) = self
            .structural_configurator_ui
            .profile_ids
            .iter()
            .position(|id| id == &rule.profile_id)
        {
            self.ui
                .drop_down(cx, ids!(sc_profile))
                .set_selected_item(cx, index);
        }
    }

    fn structural_configurator_fill_vertex_fields(&mut self, cx: &mut Cx) {
        let point = self
            .structural_configurator_ui
            .draft
            .as_ref()
            .and_then(|config| {
                let section_id = self.structural_configurator_ui.selected_section.as_ref()?;
                let corner_id = self.structural_configurator_ui.selected_corner.as_ref()?;
                sc_section_by_id(config, section_id)?
                    .corners
                    .iter()
                    .find(|corner| &corner.id == corner_id)
                    .map(|corner| corner.xy_m)
            });
        if let Some([x, y]) = point {
            self.ui
                .text_input(cx, ids!(sc_vertex_x))
                .set_text(cx, &format!("{x:.6}"));
            self.ui
                .text_input(cx, ids!(sc_vertex_y))
                .set_text(cx, &format!("{y:.6}"));
        } else {
            self.ui.text_input(cx, ids!(sc_vertex_x)).set_text(cx, "");
            self.ui.text_input(cx, ids!(sc_vertex_y)).set_text(cx, "");
        }
        self.structural_configurator_ui.coordinate_draft_dirty = false;
    }

    fn structural_configurator_fill_interface_fields(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            return;
        };
        let selected = self.structural_configurator_ui.selected_interface.as_ref();
        let Some(interface) =
            selected.and_then(|id| config.interfaces.iter().find(|item| &item.id == id))
        else {
            for id in [
                id!(sc_interface_width),
                id!(sc_interface_offset_x),
                id!(sc_interface_offset_y),
            ] {
                self.ui.text_input(cx, &[id]).set_text(cx, "");
            }
            return;
        };
        let (width, direction) = match &interface.kind {
            InterfaceKind::IndependentWithJoint {
                width_m,
                offset_b_direction,
            } => (*width_m, *offset_b_direction),
            _ => (0.0, [0.0, 0.0]),
        };
        self.ui
            .text_input(cx, ids!(sc_interface_width))
            .set_text(cx, &format!("{width:.6}"));
        self.ui
            .text_input(cx, ids!(sc_interface_offset_x))
            .set_text(cx, &format!("{:.6}", direction[0]));
        self.ui
            .text_input(cx, ids!(sc_interface_offset_y))
            .set_text(cx, &format!("{:.6}", direction[1]));
    }

    fn structural_configurator_update_interface_edge_selectors(&mut self, cx: &mut Cx) {
        let section_a = self
            .structural_configurator_ui
            .interface_section_a_ids
            .get(
                self.ui
                    .drop_down(cx, ids!(sc_interface_from))
                    .selected_item(),
            )
            .cloned();
        let section_b = self
            .structural_configurator_ui
            .interface_section_b_ids
            .get(self.ui.drop_down(cx, ids!(sc_interface_to)).selected_item())
            .cloned();
        let corners_a: Vec<SemanticId> = self
            .structural_configurator_ui
            .draft
            .as_ref()
            .and_then(|config| {
                section_a
                    .as_ref()
                    .and_then(|id| sc_section_by_id(config, id))
            })
            .map(|section| {
                section
                    .corners
                    .iter()
                    .map(|corner| corner.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let corners_b: Vec<SemanticId> = self
            .structural_configurator_ui
            .draft
            .as_ref()
            .and_then(|config| {
                section_b
                    .as_ref()
                    .and_then(|id| sc_section_by_id(config, id))
            })
            .map(|section| {
                section
                    .corners
                    .iter()
                    .map(|corner| corner.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let old_a_start = self
            .structural_configurator_ui
            .interface_edge_a_ids
            .get(
                self.ui
                    .drop_down(cx, ids!(sc_interface_edge_a_start))
                    .selected_item(),
            )
            .cloned();
        let old_a_end = self
            .structural_configurator_ui
            .interface_edge_a_ids
            .get(
                self.ui
                    .drop_down(cx, ids!(sc_interface_edge_a_end))
                    .selected_item(),
            )
            .cloned();
        let old_b_start = self
            .structural_configurator_ui
            .interface_edge_b_ids
            .get(
                self.ui
                    .drop_down(cx, ids!(sc_interface_edge_b_start))
                    .selected_item(),
            )
            .cloned();
        let old_b_end = self
            .structural_configurator_ui
            .interface_edge_b_ids
            .get(
                self.ui
                    .drop_down(cx, ids!(sc_interface_edge_b_end))
                    .selected_item(),
            )
            .cloned();
        self.structural_configurator_ui.interface_edge_a_ids = corners_a.clone();
        self.structural_configurator_ui.interface_edge_b_ids = corners_b.clone();
        let labels_a: Vec<String> = corners_a.iter().map(sc_id_label).collect();
        let labels_b: Vec<String> = corners_b.iter().map(sc_id_label).collect();
        for (widget, labels) in [
            (
                id!(sc_interface_edge_a_start),
                if labels_a.is_empty() {
                    vec!["No corners".into()]
                } else {
                    labels_a.clone()
                },
            ),
            (
                id!(sc_interface_edge_a_end),
                if labels_a.is_empty() {
                    vec!["No corners".into()]
                } else {
                    labels_a
                },
            ),
            (
                id!(sc_interface_edge_b_start),
                if labels_b.is_empty() {
                    vec!["No corners".into()]
                } else {
                    labels_b.clone()
                },
            ),
            (
                id!(sc_interface_edge_b_end),
                if labels_b.is_empty() {
                    vec!["No corners".into()]
                } else {
                    labels_b
                },
            ),
        ] {
            self.ui.drop_down(cx, &[widget]).set_labels(cx, labels);
        }
        for (widget, ids, preferred) in [
            (id!(sc_interface_edge_a_start), &corners_a, old_a_start),
            (id!(sc_interface_edge_a_end), &corners_a, old_a_end),
            (id!(sc_interface_edge_b_start), &corners_b, old_b_start),
            (id!(sc_interface_edge_b_end), &corners_b, old_b_end),
        ] {
            let selected = preferred
                .as_ref()
                .and_then(|preferred| ids.iter().position(|id| id == preferred))
                .unwrap_or(0);
            if !ids.is_empty() {
                self.ui
                    .drop_down(cx, &[widget])
                    .set_selected_item(cx, selected);
            }
        }
    }

    fn structural_configurator_update_validation(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            self.structural_configurator_ui.validation_issues.clear();
            self.ui
                .text_input(cx, ids!(sc_validation_report))
                .set_text(cx, "MODEL_INCOMPLETE · no configuration to validate.");
            return;
        };
        let mut issues = structural_configurator::validate_config(config);
        let mut lines = vec![format!(
            "{} stored configuration validation issue(s).",
            issues.len()
        )];
        lines.extend(issues.iter().map(|issue| {
            format!(
                "{:?} · {} · {} · {}",
                issue.severity, issue.code, issue.path, issue.message
            )
        }));
        if let Some(error) = self.structural_configurator_ui.local_input_error.as_ref() {
            lines.insert(0, format!("INVALID_INPUT · {error}"));
        }
        if self.structural_configurator_ui.coordinate_draft_dirty {
            lines.insert(0, "Exact XY fields have unapplied text. Choose Set exact XY before applying or regenerating.".into());
        }
        self.structural_configurator_ui.validation_issues = std::mem::take(&mut issues);
        self.ui
            .text_input(cx, ids!(sc_validation_report))
            .set_text(cx, &lines.join("\n"));
    }

    fn structural_configurator_update_labels(&mut self, cx: &mut Cx) {
        let state = &self.structural_configurator_ui;
        let status = if state.binding_stale {
            "STALE_BINDING · reload required".to_owned()
        } else if state.local_input_error.is_some() {
            "INVALID_INPUT · text draft retained".to_owned()
        } else if state.draft_dirty {
            "MODEL_INCOMPLETE · draft is not stored or analyzed".to_owned()
        } else if let Some(projection) = state.projection.as_ref() {
            if projection.generated_current {
                if projection.analysis_current {
                    format!(
                        "{} · FrameSelfWeightOnly numerical scope only; no ULS/SLS acceptance",
                        projection.analysis_status
                    )
                } else {
                    "MODEL_INCOMPLETE · generated geometry only".to_owned()
                }
            } else {
                "MODEL_INCOMPLETE · no current generated graph".to_owned()
            }
        } else {
            "MODEL_INCOMPLETE · waiting for stored configuration".to_owned()
        };
        self.ui.label(cx, ids!(sc_status)).set_text(cx, &status);
        let binding_text = state.binding.as_ref().map(|binding| format!(
            "Document {} · runtime {} · open generation {} · revision {} · input revision {} · fingerprint {}",
            binding.document_uuid, binding.runtime_id, binding.open_generation, binding.document_revision, binding.input_revision, binding.input_fingerprint,
        )).unwrap_or_else(|| "No active Document binding".into());
        if state.binding_stale {
            self.ui
                .label(cx, ids!(sc_binding))
                .set_text(cx, &format!("STALE · {binding_text}"));
        } else {
            self.ui
                .label(cx, ids!(sc_binding))
                .set_text(cx, &binding_text);
        }
        if let Some(projection) = state.projection.as_ref() {
            let summary = if state.draft_dirty || state.binding_stale {
                format!(
                    "DRAFT NOT CURRENT · stored projection is reference only\n{}",
                    projection.summary
                )
            } else {
                projection.summary.clone()
            };
            self.ui.label(cx, ids!(sc_summary)).set_text(cx, &summary);
        }
    }

    fn structural_configurator_update_inspector(&mut self, cx: &mut Cx) {
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            self.ui
                .text_input(cx, ids!(sc_selection_inspector))
                .set_text(cx, "No section or corner selected.");
            return;
        };
        let mut lines = Vec::new();
        if let Some(section_id) = self.structural_configurator_ui.selected_section.as_ref() {
            if let Some(section) = sc_section_by_id(config, section_id) {
                lines.push(format!(
                    "Draft section {} · {:?}",
                    section.id, section.structural_system
                ));
                lines.push(format!(
                    "Ordered CCW ring: {} corners",
                    section.corners.len()
                ));
                lines.push(format!(
                    "Roof: {:?}; absolute base/eave/ridge {:.3}/{:.3}/{:.3} m",
                    section.roof.kind,
                    section.roof.base_abs_m,
                    section.roof.eave_abs_m,
                    section.roof.ridge_abs_m
                ));
                lines.push(format!(
                    "Grid: frame {:?}; columns {:?}; purlin {:.3} m; wall girt {:.3} m; braces {}",
                    section.grid.frame_stations,
                    section.grid.column_stations,
                    section.grid.purlin_pitch_m,
                    section.grid.wall_girt_pitch_m,
                    section.grid.braces
                ));
            }
        }
        lines.push(format!(
            "Declared analysis load scope: {:?}",
            config.analysis_load_scope
        ));
        if let Some(corner_id) = self.structural_configurator_ui.selected_corner.as_ref() {
            if let Some(corner) = self
                .structural_configurator_ui
                .selected_section
                .as_ref()
                .and_then(|id| sc_section_by_id(config, id))
                .and_then(|section| {
                    section
                        .corners
                        .iter()
                        .find(|corner| &corner.id == corner_id)
                })
            {
                lines.push(format!(
                    "Selected config corner {} · local X {:.6} m · Y {:.6} m",
                    corner.id, corner.xy_m[0], corner.xy_m[1]
                ));
            }
        }
        if let Some(pick) = self.structural_configurator_ui.selection.as_ref() {
            let generated = self
                .structural_configurator_ui
                .projection
                .as_ref()
                .and_then(|p| p.generated.as_ref());
            lines.push(match pick {
                ScPick::Node(id) => generated.and_then(|g| g.nodes.get(id)).map(|node| format!("Generated node {} · XYZ ({:.4}, {:.4}, {:.4}) m · owners {:?} · restraints {:?}", id, node.position_m[0], node.position_m[1], node.position_m[2], node.owners, node.restrained)).unwrap_or_else(|| format!("Generated node · semantic ID {id}")),
                ScPick::Member(id) => generated.and_then(|g| g.members.get(id)).map(|member| format!("Generated member {} · {:?} · {} → {} · {:.4} m · material {} · profile {} · owners {:?}", id, member.role, member.start, member.end, member.length_m, member.material_id, member.profile_id, member.owners)).unwrap_or_else(|| format!("Generated member · semantic ID {id}")),
                ScPick::Surface(id) => generated.and_then(|g| g.surfaces.get(id)).map(|surface| format!("Generated surface {} · {:?} · section {} · {:.4} m² · stiffness {} · load transfer {:?} · source {}", id, surface.role, surface.section_id, surface.area_m2, surface.stiffness_enabled, surface.load_transfer_status, surface.load_source_id)).unwrap_or_else(|| format!("Generated surface · semantic ID {id}")),
                ScPick::Section(id) => generated.and_then(|g| g.outlines.get(id)).map(|outline| format!("Generated section {} · footprint {:.4} m² · perimeter {:.4} m · generation limit {:?}", id, outline.footprint_area_m2, outline.perimeter_m, outline.generation_limit)).unwrap_or_else(|| format!("Generated section · semantic ID {id}")),
                ScPick::Interface(id) => generated.and_then(|g| g.interfaces.iter().find(|item| &item.id == id)).map(|interface| format!("Generated interface {} · sections {} / {} · load-path status {:?} · shared nodes {} · shared members {}", id, interface.section_a, interface.section_b, interface.status, interface.shared_node_ids.len(), interface.shared_member_ids.len())).unwrap_or_else(|| format!("Generated interface · semantic ID {id}")),
                ScPick::Support(id) => generated.and_then(|g| g.reaction_routing.iter().find(|route| &route.support_node_id == id)).map(|route| format!("Generated support routing {} · section owners {:?} · foundation assignments {:?} · explicit allocation required {}", id, route.section_owners, route.foundation_by_section, route.requires_explicit_allocation)).unwrap_or_else(|| format!("Generated support · semantic ID {id}")),
                ScPick::Displacement(node_id) => format!("Snapshot displacement selection · generated node semantic ID {node_id} · see the current bounded analysis report for source-bound numeric values."),
                ScPick::DeformedMember(member_id) => generated.and_then(|g| g.members.get(member_id)).map(|member| format!("Snapshot deformed-member selection · {} · {:?} · {} → {} · source graph {}", member.id, member.role, member.start, member.end, generated.unwrap().graph_fingerprint)).unwrap_or_else(|| format!("Snapshot deformed member · semantic ID {member_id}")),
                ScPick::AppliedLoad { load_case_id, source_id, target } => format!("Typed applied load · case {} · source {} · target {:?} · see the source-matched snapshot report; no inference is made from generated weight ledgers.", load_case_id, source_id, target),
                ScPick::Reaction(node_id) => format!("Typed reaction result · support/node semantic ID {node_id} · current snapshot provenance is shown in the report."),
                ScPick::Foundation(foundation_id) => format!("Explicit foundation group · ID {foundation_id:?} · current result is preliminary only; normative bearing, settlement, and reinforced-concrete checks are not established."),
                ScPick::FoundationCell { foundation_id, cell_index } => format!("Typed foundation contact cell · foundation {foundation_id:?} · cell index {cell_index} · mesh display does not establish bearing acceptance."),
                ScPick::GroundLayer { foundation_id, layer_index } => format!("Typed ground layer · foundation {foundation_id:?} · source layer index {layer_index} · inspect the stored input/report for material parameters."),
            });
        }
        if lines.is_empty() {
            lines.push("No section or generated element selected.".into());
        }
        self.ui
            .text_input(cx, ids!(sc_selection_inspector))
            .set_text(cx, &lines.join("\n"));
    }
}

impl App {
    fn structural_configurator_read_form(&self, cx: &Cx) -> Result<BuildingConfig, String> {
        let mut config = self
            .structural_configurator_ui
            .draft
            .clone()
            .ok_or_else(|| "No stored configuration is loaded.".to_owned())?;
        let Some(section_id) = self.structural_configurator_ui.selected_section.as_ref() else {
            return Ok(config);
        };
        let base_z = sc_parse_number(&self.ui.text_input(cx, ids!(sc_base_z)).text(), "Base Z")?;
        let eave_z = sc_parse_number(
            &self.ui.text_input(cx, ids!(sc_eave_height)).text(),
            "Eave Z",
        )?;
        let ridge_z = sc_parse_number(
            &self.ui.text_input(cx, ids!(sc_ridge_max_height)).text(),
            "Ridge max Z",
        )?;
        let direction = [
            sc_parse_number(
                &self.ui.text_input(cx, ids!(sc_roof_direction_x)).text(),
                "Roof span direction X",
            )?,
            sc_parse_number(
                &self.ui.text_input(cx, ids!(sc_roof_direction_y)).text(),
                "Roof span direction Y",
            )?,
        ];
        let frame_text = self.ui.text_input(cx, ids!(sc_frame_pitch)).text();
        let column_text = self.ui.text_input(cx, ids!(sc_column_pitch)).text();
        let purlin_pitch = sc_positive_number(
            &self.ui.text_input(cx, ids!(sc_purlin_pitch)).text(),
            "Purlin pitch",
        )?;
        let wall_girt_pitch = sc_positive_number(
            &self.ui.text_input(cx, ids!(sc_wall_girt_pitch)).text(),
            "Wall girt pitch",
        )?;
        let roof_kind = SC_ROOF_KINDS
            .get(self.ui.drop_down(cx, ids!(sc_roof_kind)).selected_item())
            .copied()
            .ok_or_else(|| "Select a roof kind from the stored choices.".to_owned())?;
        let frame_spacing;
        let column_spacing;
        {
            let section = sc_section_by_id(&config, section_id)
                .ok_or_else(|| "Selected section is no longer present in the draft.".to_owned())?;
            frame_spacing = sc_grid_spacing_from_text(
                &section.grid.frame_stations,
                &frame_text,
                "Frame pitch",
            )?;
            column_spacing = sc_grid_spacing_from_text(
                &section.grid.column_stations,
                &column_text,
                "Column pitch",
            )?;
        }
        let material_id = self
            .structural_configurator_ui
            .material_ids
            .get(self.ui.drop_down(cx, ids!(sc_material)).selected_item())
            .cloned();
        let profile_id = self
            .structural_configurator_ui
            .profile_ids
            .get(self.ui.drop_down(cx, ids!(sc_profile)).selected_item())
            .cloned();
        let role = self.structural_configurator_ui.member_role;
        if let Some(section) = sc_section_by_id_mut(&mut config, section_id) {
            section.roof.base_abs_m = base_z;
            section.roof.eave_abs_m = eave_z;
            section.roof.ridge_abs_m = ridge_z;
            section.roof.kind = roof_kind;
            section.grid.span_direction = direction;
            section.grid.frame_stations = frame_spacing;
            section.grid.column_stations = column_spacing;
            section.grid.purlin_pitch_m = purlin_pitch;
            section.grid.wall_girt_pitch_m = wall_girt_pitch;
            if let Some(material_id) = material_id {
                sc_member_rule_mut(&mut section.members, role).material_id = material_id;
            }
            if let Some(profile_id) = profile_id {
                sc_member_rule_mut(&mut section.members, role).profile_id = profile_id;
            }
        }

        if let Some(interface_id) = self.structural_configurator_ui.selected_interface.as_ref() {
            let from = self
                .structural_configurator_ui
                .interface_section_a_ids
                .get(
                    self.ui
                        .drop_down(cx, ids!(sc_interface_from))
                        .selected_item(),
                )
                .cloned();
            let to = self
                .structural_configurator_ui
                .interface_section_b_ids
                .get(self.ui.drop_down(cx, ids!(sc_interface_to)).selected_item())
                .cloned();
            let edge_a_start = self
                .structural_configurator_ui
                .interface_edge_a_ids
                .get(
                    self.ui
                        .drop_down(cx, ids!(sc_interface_edge_a_start))
                        .selected_item(),
                )
                .cloned();
            let edge_a_end = self
                .structural_configurator_ui
                .interface_edge_a_ids
                .get(
                    self.ui
                        .drop_down(cx, ids!(sc_interface_edge_a_end))
                        .selected_item(),
                )
                .cloned();
            let edge_b_start = self
                .structural_configurator_ui
                .interface_edge_b_ids
                .get(
                    self.ui
                        .drop_down(cx, ids!(sc_interface_edge_b_start))
                        .selected_item(),
                )
                .cloned();
            let edge_b_end = self
                .structural_configurator_ui
                .interface_edge_b_ids
                .get(
                    self.ui
                        .drop_down(cx, ids!(sc_interface_edge_b_end))
                        .selected_item(),
                )
                .cloned();
            let kind_index = self
                .ui
                .drop_down(cx, ids!(sc_interface_kind))
                .selected_item();
            if let Some(interface) = config
                .interfaces
                .iter_mut()
                .find(|item| &item.id == interface_id)
            {
                if let (Some(from), Some(to), Some(a0), Some(a1), Some(b0), Some(b1)) =
                    (from, to, edge_a_start, edge_a_end, edge_b_start, edge_b_end)
                {
                    interface.section_a = from;
                    interface.section_b = to;
                    interface.edge_a = [a0, a1];
                    interface.edge_b = [b0, b1];
                    let (width, offset) = if kind_index == 1 {
                        (
                            sc_positive_number(
                                &self.ui.text_input(cx, ids!(sc_interface_width)).text(),
                                "Joint width",
                            )?,
                            [
                                sc_parse_number(
                                    &self.ui.text_input(cx, ids!(sc_interface_offset_x)).text(),
                                    "B offset X",
                                )?,
                                sc_parse_number(
                                    &self.ui.text_input(cx, ids!(sc_interface_offset_y)).text(),
                                    "B offset Y",
                                )?,
                            ],
                        )
                    } else {
                        (0.0, [0.0, 0.0])
                    };
                    interface.kind = sc_interface_kind(kind_index, width, offset);
                }
            }
        }
        Ok(config)
    }

    fn structural_configurator_mark_unparsed_draft(&mut self, cx: &mut Cx) {
        if self.document.is_none() || self.structural_configurator_ui.binding_stale {
            self.structural_configurator_ui.local_input_error =
                Some("A current Document binding is required before editing.".into());
        } else {
            self.structural_configurator_ui.local_input_error = None;
        }
        self.structural_configurator_ui.draft_dirty = true;
        self.structural_configurator_update_validation(cx);
        self.structural_configurator_update_labels(cx);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
        self.structural_configurator_apply_visibility(cx);
    }

    fn structural_configurator_mark_draft(&mut self, cx: &mut Cx) {
        if self.document.is_none() || self.structural_configurator_ui.binding_stale {
            self.structural_configurator_ui.local_input_error = Some(
                "A current Document binding is required. Reload before editing this draft.".into(),
            );
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "No current Document binding is available.",
            );
            self.structural_configurator_apply_visibility(cx);
            return;
        }
        let mut accepted = false;
        match self.structural_configurator_read_form(cx) {
            Ok(config) => {
                self.structural_configurator_ui.draft = Some(config);
                self.structural_configurator_ui.local_input_error = None;
                accepted = true;
            }
            Err(error) => {
                self.structural_configurator_ui.local_input_error = Some(error.clone());
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
            }
        }
        self.structural_configurator_ui.draft_dirty = true;
        if accepted {
            self.structural_configurator_update_selectors(cx);
        }
        self.structural_configurator_update_validation(cx);
        self.structural_configurator_update_labels(cx);
        self.structural_configurator_update_inspector(cx);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
        self.structural_configurator_apply_visibility(cx);
    }

    fn structural_configurator_mark_geometry_changed(&mut self, cx: &mut Cx) {
        self.structural_configurator_ui.draft_dirty = true;
        self.structural_configurator_ui.coordinate_draft_dirty = false;
        self.structural_configurator_fill_vertex_fields(cx);
        self.structural_configurator_update_selectors(cx);
        self.structural_configurator_update_validation(cx);
        self.structural_configurator_update_labels(cx);
        self.structural_configurator_update_inspector(cx);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
        self.structural_configurator_apply_visibility(cx);
    }

    fn structural_configurator_apply_coordinates(&mut self, cx: &mut Cx) {
        if self.document.is_none() || self.structural_configurator_ui.binding_stale {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before changing corner coordinates.",
            );
            return;
        }
        let Some(section_id) = self.structural_configurator_ui.selected_section.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a section before applying exact XY.",
            );
            return;
        };
        let Some(corner_id) = self.structural_configurator_ui.selected_corner.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select an ordered corner before applying exact XY.",
            );
            return;
        };
        let point = match (
            sc_parse_number(
                &self.ui.text_input(cx, ids!(sc_vertex_x)).text(),
                "Corner X",
            ),
            sc_parse_number(
                &self.ui.text_input(cx, ids!(sc_vertex_y)).text(),
                "Corner Y",
            ),
        ) {
            (Ok(x), Ok(y)) => [x, y],
            (Err(error), _) | (_, Err(error)) => {
                self.structural_configurator_ui.local_input_error = Some(error.clone());
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                self.structural_configurator_update_validation(cx);
                return;
            }
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        let Some(corner) = sc_section_by_id_mut(config, &section_id).and_then(|section| {
            section
                .corners
                .iter_mut()
                .find(|corner| corner.id == corner_id)
        }) else {
            self.structural_configurator_ui.local_input_error =
                Some("The selected corner no longer exists in this draft.".into());
            self.structural_configurator_update_validation(cx);
            return;
        };
        corner.xy_m = point;
        self.structural_configurator_mark_geometry_changed(cx);
    }

    fn structural_configurator_delete_selected_corner(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before deleting a corner.",
            );
            return;
        }
        let (Some(section_id), Some(corner_id)) = (
            self.structural_configurator_ui.selected_section.clone(),
            self.structural_configurator_ui.selected_corner.clone(),
        ) else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a corner before deleting it.",
            );
            return;
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        let Some(section) = sc_section_by_id_mut(config, &section_id) else {
            return;
        };
        let Some(index) = section
            .corners
            .iter()
            .position(|corner| corner.id == corner_id)
        else {
            return;
        };
        section.corners.remove(index);
        self.structural_configurator_ui.selected_corner = section
            .corners
            .get(index.min(section.corners.len().saturating_sub(1)))
            .map(|corner| corner.id.clone());
        self.structural_configurator_mark_geometry_changed(cx);
    }

    fn structural_configurator_add_section(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before adding a section.",
            );
            return;
        }
        let Some(mut config) = self.structural_configurator_ui.draft.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Use Create demo or load a typed configuration before adding a section.",
            );
            return;
        };
        let Some(source_id) = self.structural_configurator_ui.selected_section.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a source section to duplicate its explicit typed parameters.",
            );
            return;
        };
        let Some(mut section) = sc_section_by_id(&config, &source_id).cloned() else {
            return;
        };
        let new_section_id = match sc_unique_id(&config, "section") {
            Ok(id) => id,
            Err(error) => {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                return;
            }
        };
        let bounds = section
            .corners
            .iter()
            .fold(None, |bounds: Option<Bounds2>, corner| {
                let point = DraftPoint::new(corner.xy_m[0], corner.xy_m[1]);
                Some(
                    bounds
                        .map(|value| value.union(Bounds2::from_points(point, point)))
                        .unwrap_or_else(|| Bounds2::from_points(point, point)),
                )
            });
        let offset = bounds.map(|b| b.max.x - b.min.x + 5.0).unwrap_or(5.0);
        let mut id_config = config.clone();
        section.id = new_section_id;
        for corner in &mut section.corners {
            corner.xy_m[0] += offset;
        }
        let mut remap = BTreeMap::<SemanticId, SemanticId>::new();
        id_config.sections.push(section.clone());
        let mut new_corners = section.corners.clone();
        for index in 0..new_corners.len() {
            let old_id = new_corners[index].id.clone();
            let new_id = match sc_unique_id(&id_config, "corner") {
                Ok(id) => id,
                Err(error) => {
                    self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                    return;
                }
            };
            remap.insert(old_id, new_id.clone());
            new_corners[index].id = new_id.clone();
            if let Some(last) = id_config.sections.last_mut() {
                if let Some(corner) = last.corners.get_mut(index) {
                    corner.id = new_id;
                }
            }
        }
        section.corners = new_corners;
        if let Some(details) = section.roof.unsupported_details.as_mut() {
            if details
                .corner_tops_abs_m
                .keys()
                .any(|id| !remap.contains_key(id))
            {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", "The stored roof details reference a corner outside this section. The section was not duplicated so those typed references are not lost or redirected.");
                return;
            }
            details.corner_tops_abs_m = std::mem::take(&mut details.corner_tops_abs_m)
                .into_iter()
                .map(|(old, elevation)| (remap[&old].clone(), elevation))
                .collect();
            for lines in [
                &mut details.break_lines_m,
                &mut details.ridge_lines_m,
                &mut details.facets_m,
            ] {
                for line in lines.iter_mut() {
                    for point in line {
                        point[0] += offset;
                    }
                }
            }
        }
        config.sections.push(section.clone());
        self.structural_configurator_ui.draft = Some(config);
        self.structural_configurator_ui.selected_section = Some(section.id.clone());
        self.structural_configurator_ui.selected_corner =
            section.corners.first().map(|corner| corner.id.clone());
        self.structural_configurator_mark_geometry_changed(cx);
    }

    fn structural_configurator_toggle_braces(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Resolve the current draft and reload a current Document before changing braces.",
            );
            return;
        }
        let mut config = match self.structural_configurator_read_form(cx) {
            Ok(config) => config,
            Err(error) => {
                self.structural_configurator_ui.local_input_error = Some(error.clone());
                self.structural_configurator_ui.draft_dirty = true;
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                self.structural_configurator_update_validation(cx);
                return;
            }
        };
        let Some(section_id) = self.structural_configurator_ui.selected_section.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a section before editing the brace flag.",
            );
            return;
        };
        let Some(section) = sc_section_by_id_mut(&mut config, &section_id) else {
            return;
        };
        section.grid.braces = !section.grid.braces;
        self.structural_configurator_ui.draft = Some(config);
        self.structural_configurator_ui.draft_dirty = true;
        self.structural_configurator_ui.local_input_error = None;
        self.structural_configurator_fill_form(cx);
        self.structural_configurator_update_validation(cx);
        self.structural_configurator_update_labels(cx);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
        self.structural_configurator_apply_visibility(cx);
    }

    fn structural_configurator_delete_section(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before deleting a section.",
            );
            return;
        }
        let Some(section_id) = self.structural_configurator_ui.selected_section.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a section before deleting it.",
            );
            return;
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        if config
            .interfaces
            .iter()
            .any(|interface| interface.section_a == section_id || interface.section_b == section_id)
            || config
                .foundation_assignments
                .iter()
                .any(|assignment| assignment.section_id == section_id)
        {
            self.structural_configurator_set_status(cx, "INVALID_INPUT", "This section is referenced by an explicit interface or foundation assignment. Remove or remap those typed references first.");
            return;
        }
        config.sections.retain(|section| section.id != section_id);
        self.structural_configurator_ui.selected_section =
            config.sections.first().map(|section| section.id.clone());
        self.structural_configurator_ui.selected_corner = None;
        self.structural_configurator_mark_geometry_changed(cx);
        self.structural_configurator_fill_form(cx);
    }

    fn structural_configurator_add_interface(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before adding an interface.",
            );
            return;
        }
        let Some(mut config) = self.structural_configurator_ui.draft.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Load a typed configuration before creating an interface.",
            );
            return;
        };
        let Some(section_a) = self
            .structural_configurator_ui
            .selected_section
            .clone()
            .or_else(|| config.sections.first().map(|section| section.id.clone()))
        else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "At least two typed sections are needed for an interface.",
            );
            return;
        };
        let Some(section_b) = config
            .sections
            .iter()
            .find(|section| section.id != section_a)
            .map(|section| section.id.clone())
        else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "At least two typed sections are needed for an interface.",
            );
            return;
        };
        let Some(a) =
            sc_section_by_id(&config, &section_a).filter(|section| section.corners.len() >= 2)
        else {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Section A needs two ordered corners before defining an edge.",
            );
            return;
        };
        let Some(b) =
            sc_section_by_id(&config, &section_b).filter(|section| section.corners.len() >= 2)
        else {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Section B needs two ordered corners before defining an edge.",
            );
            return;
        };
        let id = match sc_unique_id(&config, "interface") {
            Ok(id) => id,
            Err(error) => {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                return;
            }
        };
        config
            .interfaces
            .push(structural_configurator::InterfaceSpec {
                id: id.clone(),
                section_a: section_a.clone(),
                edge_a: [a.corners[0].id.clone(), a.corners[1].id.clone()],
                section_b: section_b.clone(),
                edge_b: [b.corners[0].id.clone(), b.corners[1].id.clone()],
                kind: InterfaceKind::NotDefined,
                anchors: Vec::new(),
            });
        self.structural_configurator_ui.draft = Some(config);
        self.structural_configurator_ui.selected_interface = Some(id);
        self.structural_configurator_mark_geometry_changed(cx);
        self.structural_configurator_fill_interface_fields(cx);
    }

    fn structural_configurator_delete_interface(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before deleting an interface.",
            );
            return;
        }
        let Some(interface_id) = self.structural_configurator_ui.selected_interface.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select an interface before deleting it.",
            );
            return;
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        config
            .interfaces
            .retain(|interface| interface.id != interface_id);
        self.structural_configurator_ui.selected_interface =
            config.interfaces.first().map(|item| item.id.clone());
        self.structural_configurator_mark_geometry_changed(cx);
        self.structural_configurator_fill_interface_fields(cx);
    }
}

impl App {
    fn structural_configurator_execute_intent(
        &mut self,
        cx: &mut Cx,
        intent: sc_api::ScIntent,
        expected: Option<sc_api::ScBinding>,
    ) {
        let is_demo = matches!(&intent, sc_api::ScIntent::CreateDemo);
        if self.document.is_none() {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Open or create a Document before using this command.",
            );
            return;
        }
        if !is_demo {
            if self.structural_configurator_ui.binding_stale {
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "The draft binding is stale. Reload before applying or analyzing.",
                );
                return;
            }
            if self.structural_configurator_ui.coordinate_draft_dirty {
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "Apply the selected corner's exact XY fields before continuing.",
                );
                return;
            }
            if let Some(error) = self.structural_configurator_ui.local_input_error.clone() {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                return;
            }
            if let Some(config) = self.structural_configurator_ui.draft.as_ref() {
                let issues = structural_configurator::validate_config(config);
                let blocking: Vec<_> = issues
                    .iter()
                    .filter(|issue| {
                        matches!(
                            issue.severity,
                            structural_configurator::IssueSeverity::Error
                                | structural_configurator::IssueSeverity::GenerationLimit
                        )
                    })
                    .collect();
                if !blocking.is_empty() {
                    self.structural_configurator_ui.validation_issues = issues;
                    self.structural_configurator_update_validation(cx);
                    self.structural_configurator_set_status(cx, "INVALID_INPUT", "Resolve blocking typed configuration issues before applying, regenerating, or analyzing.");
                    return;
                }
            }
            let Some(expected) = expected.as_ref() else {
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "No stored input binding is available. Reload the Document.",
                );
                return;
            };
            if let Some(document) = self.document.as_ref() {
                if let Err(error) = sc_api::require_current_input(
                    document,
                    self.api_document_generation,
                    expected.input_revision,
                    &expected.input_fingerprint,
                    expected,
                ) {
                    self.structural_configurator_ui.binding_stale = true;
                    self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                    self.structural_configurator_after_event(cx, &Event::Startup);
                    return;
                }
            }
        }

        let is_analysis = matches!(&intent, sc_api::ScIntent::Analyze { .. });
        match self.structural_configurator_execute(cx, intent, expected) {
            Ok(()) => {
                self.structural_configurator_ui.draft_dirty = false;
                self.structural_configurator_ui.coordinate_draft_dirty = false;
                self.structural_configurator_ui.binding_stale = false;
                self.structural_configurator_ui.local_input_error = None;
                self.structural_configurator_ui.attempt_status = None;
                self.structural_configurator_ui.attempt_message.clear();
                self.structural_configurator_after_event(cx, &Event::Startup);
            }
            Err(error) => {
                if !is_analysis {
                    self.structural_configurator_ui.local_input_error = Some(error.clone());
                }
                self.structural_configurator_set_status(
                    cx,
                    if is_analysis {
                        sc_analysis_attempt_code(&error)
                    } else {
                        "INVALID_INPUT"
                    },
                    &error,
                );
                if !is_analysis {
                    self.structural_configurator_update_validation(cx);
                }
                self.structural_configurator_apply_visibility(cx);
            }
        }
    }

    fn structural_configurator_reload(&mut self, cx: &mut Cx) {
        self.structural_configurator_ui.draft_dirty = false;
        self.structural_configurator_ui.binding_stale = false;
        self.structural_configurator_ui.coordinate_draft_dirty = false;
        self.structural_configurator_ui.local_input_error = None;
        self.structural_configurator_ui.binding = None;
        self.structural_configurator_ui.draft = None;
        self.structural_configurator_ui.projection = None;
        self.structural_configurator_ui.selected_section = None;
        self.structural_configurator_ui.selected_corner = None;
        self.structural_configurator_ui.selected_interface = None;
        self.structural_configurator_ui.selection = None;
        self.structural_configurator_after_event(cx, &Event::Startup);
    }

    fn structural_configurator_regenerate(&mut self, cx: &mut Cx) {
        let Ok(config) = self.structural_configurator_read_form(cx) else {
            self.structural_configurator_mark_draft(cx);
            return;
        };
        self.structural_configurator_ui.draft = Some(config.clone());
        let expected = self.structural_configurator_ui.binding.clone();
        self.structural_configurator_execute_intent(
            cx,
            sc_api::ScIntent::Regenerate(config),
            expected,
        );
    }

    fn structural_configurator_analyze(&mut self, cx: &mut Cx) {
        if self.structural_configurator_ui.draft_dirty
            || self.structural_configurator_ui.binding_stale
            || self.structural_configurator_ui.coordinate_draft_dirty
            || self.structural_configurator_ui.local_input_error.is_some()
        {
            self.structural_configurator_set_status(cx, "INVALID_INPUT", "Apply or reload the changed draft before analysis. Analysis never runs against uncommitted text.");
            return;
        }
        let Some(projection) = self.structural_configurator_ui.projection.as_ref() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "A stored generated model is required before analysis.",
            );
            return;
        };
        if !projection.generated_current {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Regenerate the stored configuration before analysis.",
            );
            return;
        }
        let section_id = self.structural_configurator_ui.selected_section.clone();
        let expected = self.structural_configurator_ui.binding.clone();
        self.structural_configurator_execute_intent(
            cx,
            sc_api::ScIntent::Analyze { section_id },
            expected,
        );
    }

    fn structural_configurator_fit(&mut self, cx: &mut Cx, target: ScFitTarget) {
        let config = self.structural_configurator_ui.draft.clone();
        let Some(config) = config else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "No stored or draft configuration is available to fit.",
            );
            return;
        };
        let bounds = match &target {
            ScFitTarget::Building => sc_polygon_bounds(&config),
            ScFitTarget::Section(section_id) => {
                sc_section_by_id(&config, section_id).and_then(sc_section_bounds)
            }
        };
        if let Some(bounds) = bounds {
            let canvas = self.ui.drafting_canvas(cx, ids!(sc_plan_canvas));
            let size = canvas
                .borrow()
                .map(|value| (value.rect.size.x, value.rect.size.y));
            if let Some((width, height)) =
                size.filter(|(width, height)| *width > 1.0 && *height > 1.0)
            {
                let _ = self.structural_configurator_ui.camera.resize(width, height);
                if let Err(error) = self
                    .structural_configurator_ui
                    .camera
                    .fit_bounds(bounds, 24.0)
                {
                    self.structural_configurator_set_status(
                        cx,
                        "INVALID_INPUT",
                        &error.to_string(),
                    );
                }
                self.structural_configurator_ui.plan_needs_fit = false;
            } else {
                self.structural_configurator_ui.plan_needs_fit = true;
            }
        }
        self.structural_configurator_ui.pending_fit = Some(target);
        self.structural_configurator_render_scene(cx, true);
        self.structural_configurator_draw_plan(cx);
    }

    fn structural_configurator_render_layout(&mut self, cx: &mut Cx, layout: ScCanvasLayout) {
        self.structural_configurator_ui.layout = layout;
        self.structural_configurator_apply_visibility(cx);
        self.structural_configurator_draw_plan(cx);
        self.structural_configurator_render_scene(cx, false);
    }

    fn structural_configurator_canvas_action(&mut self, cx: &mut Cx, action: CanvasAction) {
        match action {
            CanvasAction::Resize { width, height, .. } => {
                if width.is_finite() && height.is_finite() && width > 1.0 && height > 1.0 {
                    let _ = self.structural_configurator_ui.camera.resize(width, height);
                    self.structural_configurator_ui.canvas_camera_change =
                        Some(CanvasCameraChange::Resize);
                    if self.structural_configurator_ui.plan_needs_fit {
                        let bounds = self
                            .structural_configurator_ui
                            .draft
                            .as_ref()
                            .and_then(sc_polygon_bounds);
                        if let Some(bounds) = bounds {
                            let _ = self
                                .structural_configurator_ui
                                .camera
                                .fit_bounds(bounds, 24.0);
                            self.structural_configurator_ui.plan_needs_fit = false;
                        }
                    }
                    self.structural_configurator_draw_plan(cx);
                }
            }
            CanvasAction::Input(CanvasInput::Scroll { screen, delta_y }) => {
                if delta_y.is_finite() {
                    let factor = (-delta_y * 0.001).exp().clamp(0.2, 5.0);
                    if self
                        .structural_configurator_ui
                        .camera
                        .zoom_at(screen, factor)
                        .is_ok()
                    {
                        self.structural_configurator_ui.canvas_camera_change =
                            Some(CanvasCameraChange::Zoom);
                        self.structural_configurator_draw_plan(cx);
                    }
                }
            }
            CanvasAction::Input(input) => self.structural_configurator_canvas_input(cx, input),
            CanvasAction::None => {}
        }
    }

    fn structural_configurator_canvas_input(&mut self, cx: &mut Cx, input: CanvasInput) {
        match input {
            CanvasInput::Cancel => {
                self.structural_configurator_ui.pan_anchor = None;
                self.structural_configurator_ui.dragging_corner = false;
            }
            CanvasInput::Confirm => {
                self.structural_configurator_ui.dragging_corner = false;
                self.structural_configurator_ui.pan_anchor = None;
            }
            CanvasInput::Delete => self.structural_configurator_delete_selected_corner(cx),
            CanvasInput::Scroll { .. } => {}
            CanvasInput::Pointer {
                phase,
                button,
                screen,
            } => {
                if button == PointerButton::Middle {
                    match phase {
                        PointerPhase::Down => {
                            self.structural_configurator_ui.pan_anchor = Some(screen)
                        }
                        PointerPhase::Move => {
                            if let Some(previous) =
                                self.structural_configurator_ui.pan_anchor.replace(screen)
                            {
                                if self
                                    .structural_configurator_ui
                                    .camera
                                    .pan_pixels(screen.x - previous.x, screen.y - previous.y)
                                    .is_ok()
                                {
                                    self.structural_configurator_ui.canvas_camera_change =
                                        Some(CanvasCameraChange::Pan);
                                    self.structural_configurator_draw_plan(cx);
                                }
                            }
                        }
                        PointerPhase::Up | PointerPhase::Leave => {
                            self.structural_configurator_ui.pan_anchor = None
                        }
                        PointerPhase::Hover => {}
                    }
                    return;
                }
                if button != PointerButton::Left {
                    return;
                }
                let mut point = self
                    .structural_configurator_ui
                    .camera
                    .screen_to_world(screen);
                if self.structural_configurator_ui.snap_enabled {
                    let step = self.structural_configurator_ui.grid_step_m;
                    if step.is_finite() && step > 0.0 {
                        point.x = (point.x / step).round() * step;
                        point.y = (point.y / step).round() * step;
                    }
                }
                match phase {
                    PointerPhase::Down => {
                        self.structural_configurator_pointer_down(cx, screen, point)
                    }
                    PointerPhase::Move if self.structural_configurator_ui.dragging_corner => {
                        self.structural_configurator_move_selected_corner(cx, point);
                    }
                    PointerPhase::Up => {
                        self.structural_configurator_ui.dragging_corner = false;
                        self.structural_configurator_ui.pan_anchor = None;
                    }
                    PointerPhase::Leave => {
                        self.structural_configurator_ui.dragging_corner = false;
                    }
                    PointerPhase::Move | PointerPhase::Hover => {}
                }
            }
        }
    }

    fn structural_configurator_pointer_down(
        &mut self,
        cx: &mut Cx,
        screen: DraftPoint,
        point: DraftPoint,
    ) {
        if self.document.is_none() || self.structural_configurator_ui.binding_stale {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before editing the plan.",
            );
            return;
        }
        if self.structural_configurator_ui.coordinate_draft_dirty
            || self.structural_configurator_ui.local_input_error.is_some()
        {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Resolve the current text draft before editing another plan corner.",
            );
            return;
        }
        let mode = self.structural_configurator_ui.polygon_mode;
        let nearest = self.structural_configurator_nearest_feature(screen);
        match mode {
            ScPolygonMode::Select => {
                if let Some((section_id, corner_id)) = nearest
                    .filter(|(_, _, distance)| *distance <= 12.0)
                    .map(|(section, corner, _)| (section, corner))
                {
                    self.structural_configurator_ui.selected_section = Some(section_id);
                    self.structural_configurator_ui.selected_corner = corner_id;
                    self.structural_configurator_ui.selection = None;
                    self.structural_configurator_update_selectors(cx);
                    self.structural_configurator_fill_form(cx);
                    self.structural_configurator_update_inspector(cx);
                } else if let Some(section_id) = self.structural_configurator_section_at(point) {
                    self.structural_configurator_ui.selected_section = Some(section_id);
                    self.structural_configurator_ui.selected_corner = None;
                    self.structural_configurator_ui.selection = None;
                    self.structural_configurator_update_selectors(cx);
                    self.structural_configurator_fill_form(cx);
                    self.structural_configurator_update_inspector(cx);
                }
                self.structural_configurator_draw_plan(cx);
            }
            ScPolygonMode::Add => self.structural_configurator_add_corner(cx, point, None),
            ScPolygonMode::Insert => {
                self.structural_configurator_add_corner(cx, point, Some(screen))
            }
            ScPolygonMode::Move | ScPolygonMode::Delete => {
                if let Some((section_id, corner_id, distance)) =
                    nearest.filter(|(_, corner, d)| corner.is_some() && *d <= 12.0)
                {
                    self.structural_configurator_ui.selected_section = Some(section_id);
                    self.structural_configurator_ui.selected_corner = corner_id;
                    if mode == ScPolygonMode::Move {
                        self.structural_configurator_ui.dragging_corner = true;
                        self.structural_configurator_fill_vertex_fields(cx);
                    } else {
                        self.structural_configurator_delete_selected_corner(cx);
                    }
                    let _ = distance;
                } else {
                    self.structural_configurator_set_status(
                        cx,
                        "MODEL_INCOMPLETE",
                        "Click within 12 screen pixels of an ordered corner.",
                    );
                }
            }
        }
    }

    fn structural_configurator_nearest_feature(
        &self,
        screen: DraftPoint,
    ) -> Option<(SemanticId, Option<SemanticId>, f64)> {
        let config = self.structural_configurator_ui.draft.as_ref()?;
        config
            .sections
            .iter()
            .flat_map(|section| {
                section.corners.iter().map(move |corner| {
                    let point = self
                        .structural_configurator_ui
                        .camera
                        .world_to_screen(DraftPoint::new(corner.xy_m[0], corner.xy_m[1]));
                    (
                        section.id.clone(),
                        Some(corner.id.clone()),
                        point.distance(screen),
                    )
                })
            })
            .min_by(|left, right| left.2.total_cmp(&right.2))
    }

    fn structural_configurator_section_at(&self, point: DraftPoint) -> Option<SemanticId> {
        let config = self.structural_configurator_ui.draft.as_ref()?;
        config
            .sections
            .iter()
            .find(|section| sc_point_in_polygon(point, &section.corners))
            .map(|section| section.id.clone())
    }

    fn structural_configurator_add_corner(
        &mut self,
        cx: &mut Cx,
        point: DraftPoint,
        screen: Option<DraftPoint>,
    ) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "Reload a current Document before adding a corner.",
            );
            return;
        }
        let Some(section_id) = self.structural_configurator_ui.selected_section.clone() else {
            self.structural_configurator_set_status(
                cx,
                "MODEL_INCOMPLETE",
                "Select a target section before adding a corner.",
            );
            return;
        };
        let Some(config) = self.structural_configurator_ui.draft.as_ref() else {
            return;
        };
        let corner_id = match sc_unique_id(config, "corner") {
            Ok(id) => id,
            Err(error) => {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
                return;
            }
        };
        let mut inserted_point = point;
        let insert_at = if let Some(screen_point) = screen {
            let Some(section) = sc_section_by_id(config, &section_id) else {
                return;
            };
            let Some((edge_index, distance)) = sc_nearest_edge_screen(
                section,
                screen_point,
                &self.structural_configurator_ui.camera,
            ) else {
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "Insert on Edge needs an ordered section edge.",
                );
                return;
            };
            if distance > 10.0 {
                self.structural_configurator_set_status(
                    cx,
                    "INVALID_INPUT",
                    "Insert on Edge requires a click within 10 screen pixels of an edge.",
                );
                return;
            }
            let start = section.corners[edge_index].xy_m;
            let end = section.corners[(edge_index + 1) % section.corners.len()].xy_m;
            inserted_point = sc_project_to_segment(inserted_point, start, end);
            if inserted_point.distance(DraftPoint::new(start[0], start[1])) <= 1.0e-8
                || inserted_point.distance(DraftPoint::new(end[0], end[1])) <= 1.0e-8
                || section.corners.iter().any(|corner| {
                    inserted_point.distance(DraftPoint::new(corner.xy_m[0], corner.xy_m[1]))
                        <= 1.0e-8
                })
            {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", "The projected edge point duplicates an existing endpoint/corner; no vertex was inserted.");
                return;
            }
            Some(edge_index + 1)
        } else {
            None
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        let Some(section) = sc_section_by_id_mut(config, &section_id) else {
            return;
        };
        let corner = Corner {
            id: corner_id.clone(),
            xy_m: [inserted_point.x, inserted_point.y],
        };
        if let Some(index) = insert_at {
            section
                .corners
                .insert(index.min(section.corners.len()), corner);
        } else {
            section.corners.push(corner);
        }
        self.structural_configurator_ui.selected_corner = Some(corner_id);
        self.structural_configurator_mark_geometry_changed(cx);
        if screen.is_some() {
            self.structural_configurator_set_status(cx, "DRAFT_EDIT", "Inserted a vertex on the finite edge. The edge constraint takes priority over grid snap; validation shows any remaining polygon issues.");
        }
    }

    fn structural_configurator_move_selected_corner(&mut self, cx: &mut Cx, point: DraftPoint) {
        if !self.structural_configurator_can_change_geometry() {
            self.structural_configurator_ui.dragging_corner = false;
            self.structural_configurator_set_status(
                cx,
                "INVALID_INPUT",
                "The Document binding changed during the drag. Reload before editing.",
            );
            return;
        }
        let (Some(section_id), Some(corner_id)) = (
            self.structural_configurator_ui.selected_section.clone(),
            self.structural_configurator_ui.selected_corner.clone(),
        ) else {
            return;
        };
        let Some(config) = self.structural_configurator_ui.draft.as_mut() else {
            return;
        };
        let Some(corner) = sc_section_by_id_mut(config, &section_id).and_then(|section| {
            section
                .corners
                .iter_mut()
                .find(|corner| corner.id == corner_id)
        }) else {
            return;
        };
        if corner.xy_m != [point.x, point.y] {
            corner.xy_m = [point.x, point.y];
            self.structural_configurator_mark_geometry_changed(cx);
        }
    }
}

impl App {
    fn structural_configurator_draw_plan(&mut self, cx: &mut Cx) {
        if !self.structural_configurator_ui.active {
            return;
        }
        let config = self.structural_configurator_ui.draft.clone();
        let Some(config) = config else {
            let mut frame = CanvasFrame::default();
            frame.snapshot_token = "structural-configurator:no-input".into();
            frame.source_generation = self.structural_configurator_ui.render_generation;
            frame.model_generation = self.structural_configurator_ui.render_generation;
            frame.analysis = Arc::new(AnalysisCanvasLayer::default());
            if let Some(mut canvas) = self
                .ui
                .drafting_canvas(cx, ids!(sc_plan_canvas))
                .borrow_mut()
            {
                canvas.set_frame(cx, frame);
            }
            return;
        };
        let canvas_ref = self.ui.drafting_canvas(cx, ids!(sc_plan_canvas));
        if let Some(canvas) = canvas_ref.borrow() {
            let width = canvas.rect.size.x;
            let height = canvas.rect.size.y;
            if width.is_finite() && height.is_finite() && width > 1.0 && height > 1.0 {
                if (self.structural_configurator_ui.camera.viewport_width - width).abs() > 0.01
                    || (self.structural_configurator_ui.camera.viewport_height - height).abs()
                        > 0.01
                {
                    let _ = self.structural_configurator_ui.camera.resize(width, height);
                    self.structural_configurator_ui.canvas_camera_change =
                        Some(CanvasCameraChange::Resize);
                }
                if self.structural_configurator_ui.plan_needs_fit {
                    if let Some(bounds) = sc_polygon_bounds(&config) {
                        let _ = self
                            .structural_configurator_ui
                            .camera
                            .fit_bounds(bounds, 24.0);
                        self.structural_configurator_ui.plan_needs_fit = false;
                    }
                }
            }
        }

        let mut signature = format!(
            "{}|{}|{}|{}|{:?}|{:?}|{}|{}",
            config.id,
            self.structural_configurator_ui.camera.revision,
            self.structural_configurator_ui
                .selected_section
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            self.structural_configurator_ui
                .selected_corner
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            self.structural_configurator_ui.polygon_mode,
            self.structural_configurator_ui.selection,
            self.structural_configurator_ui.draft_dirty,
            self.structural_configurator_ui
                .local_input_error
                .as_deref()
                .unwrap_or(""),
        );
        for section in &config.sections {
            signature.push_str(section.id.as_str());
            for corner in &section.corners {
                signature.push_str(corner.id.as_str());
                signature.push_str(&format!(
                    "{:016x}{:016x}",
                    corner.xy_m[0].to_bits(),
                    corner.xy_m[1].to_bits()
                ));
            }
        }
        let serialized_config =
            serde_json::to_string(&config).unwrap_or_else(|_| format!("{config:?}"));
        signature.push_str("|presentation-config|");
        signature.push_str(&serialized_config);
        if self.structural_configurator_ui.canvas_cache_key.as_deref() == Some(signature.as_str()) {
            return;
        }
        self.structural_configurator_ui.canvas_cache_key = Some(signature.clone());
        self.structural_configurator_ui.render_generation = self
            .structural_configurator_ui
            .render_generation
            .saturating_add(1);
        let camera = &self.structural_configurator_ui.camera;
        let mut segments = Vec::<ScreenSegment>::new();
        let mut labels = Vec::<ScreenLabel>::new();
        let mut highlights = Vec::<ScreenSegment>::new();
        for section in &config.sections {
            if section.corners.len() < 2 {
                continue;
            }
            let selected =
                self.structural_configurator_ui.selected_section.as_ref() == Some(&section.id);
            let color = if selected {
                [0.32, 0.82, 1.0, 1.0]
            } else {
                [0.50, 0.62, 0.73, 0.92]
            };
            for index in 0..section.corners.len() {
                let a = section.corners[index].xy_m;
                let b = section.corners[(index + 1) % section.corners.len()].xy_m;
                segments.push(ScreenSegment {
                    start: camera.world_to_screen(DraftPoint::new(a[0], a[1])),
                    end: camera.world_to_screen(DraftPoint::new(b[0], b[1])),
                    color,
                    width: if selected { 2.2 } else { 1.35 },
                });
            }
            if let Some(bounds) = sc_section_bounds(section) {
                let label_position = camera.world_to_screen(bounds.center());
                labels.push(ScreenLabel {
                    position: label_position,
                    text: format!("{} · {:?}", section.id, section.roof.kind),
                    color: if selected {
                        [0.73, 0.90, 1.0, 1.0]
                    } else {
                        [0.80, 0.85, 0.90, 0.9]
                    },
                    placement: ScreenLabelPlacement::Absolute,
                });
            }
            for corner in &section.corners {
                let point = camera.world_to_screen(DraftPoint::new(corner.xy_m[0], corner.xy_m[1]));
                let selected_corner = selected
                    && self.structural_configurator_ui.selected_corner.as_ref() == Some(&corner.id);
                let half = if selected_corner { 6.0 } else { 3.0 };
                let cross_color = if selected_corner {
                    [1.0, 0.70, 0.20, 1.0]
                } else {
                    color
                };
                highlights.push(ScreenSegment {
                    start: DraftPoint::new(point.x - half, point.y),
                    end: DraftPoint::new(point.x + half, point.y),
                    color: cross_color,
                    width: if selected_corner { 2.0 } else { 1.1 },
                });
                highlights.push(ScreenSegment {
                    start: DraftPoint::new(point.x, point.y - half),
                    end: DraftPoint::new(point.x, point.y + half),
                    color: cross_color,
                    width: if selected_corner { 2.0 } else { 1.1 },
                });
            }
        }
        labels.push(ScreenLabel {
            position: DraftPoint::new(12.0, 12.0),
            text: if self.structural_configurator_ui.draft_dirty {
                "DRAFT · not stored · polygon validation remains authoritative".into()
            } else {
                "Building-local XY · exact numeric fields in meters · pointer grid is optional"
                    .into()
            },
            color: [0.95, 0.76, 0.36, 1.0],
            placement: ScreenLabelPlacement::Hud,
        });
        if let Some(error) = self.structural_configurator_ui.local_input_error.as_ref() {
            labels.push(ScreenLabel {
                position: DraftPoint::new(12.0, 42.0),
                text: format!("INVALID_INPUT · {error}"),
                color: [1.0, 0.48, 0.36, 1.0],
                placement: ScreenLabelPlacement::Footer,
            });
        }
        let document_revision = self
            .structural_configurator_ui
            .binding
            .as_ref()
            .map(|binding| binding.document_revision)
            .unwrap_or(0);
        let frame = CanvasFrame {
            snapshot_token: format!(
                "sc:{}:{}",
                config.id, self.structural_configurator_ui.render_generation
            ),
            source_generation: self.structural_configurator_ui.render_generation,
            model_generation: self.structural_configurator_ui.render_generation,
            document_revision,
            camera_revision: self.structural_configurator_ui.camera.revision,
            camera_change: self.structural_configurator_ui.canvas_camera_change.take(),
            cpu_frame_prepare_ms: None,
            segments: Arc::new(segments),
            highlights: Arc::new(highlights),
            preview: Vec::new(),
            crosshair: None,
            analysis: Arc::new(AnalysisCanvasLayer {
                labels,
                ..AnalysisCanvasLayer::default()
            }),
        };
        if let Some(mut canvas) = self
            .ui
            .drafting_canvas(cx, ids!(sc_plan_canvas))
            .borrow_mut()
        {
            canvas.set_frame(cx, frame);
        }
    }

    fn structural_configurator_clear_scene(&mut self, cx: &mut Cx) {
        let already_empty = self
            .structural_configurator_ui
            .scene_graph_fingerprint
            .is_none()
            && self.structural_configurator_ui.scene_picks.is_empty()
            && self.structural_configurator_ui.scene_sources.is_empty()
            && self
                .ui
                .cad_viewport(cx, ids!(sc_model_viewport))
                .borrow()
                .is_some_and(|widget| widget.viewer().scene().objects().next().is_none());
        if already_empty {
            return;
        }
        let viewport_ref = self.ui.cad_viewport(cx, ids!(sc_model_viewport));
        let old_state = viewport_ref
            .borrow()
            .map(|widget| widget.viewer().capture_state());
        let mut viewer = Viewer::default();
        if let Some(state) = old_state.as_ref() {
            let _ = viewer.restore_state(state);
        }
        viewport_ref.set_viewer(cx, viewer);
        self.structural_configurator_ui.scene_picks.clear();
        self.structural_configurator_ui.scene_sources.clear();
        self.structural_configurator_ui
            .scene_objects_by_section
            .clear();
        self.structural_configurator_ui.scene_bounds = None;
        self.structural_configurator_ui.section_bounds.clear();
        self.structural_configurator_ui.scene_graph_fingerprint = None;
        self.structural_configurator_ui
            .scene_overlay_snapshot_fingerprint = None;
        self.structural_configurator_ui.scene_cache_key = None;
    }

    fn structural_configurator_render_scene(&mut self, cx: &mut Cx, force_fit: bool) {
        if !self.structural_configurator_ui.active {
            return;
        }
        let Some(projection) = self.structural_configurator_ui.projection.clone() else {
            self.structural_configurator_clear_scene(cx);
            self.ui
                .label(cx, ids!(sc_scene_title))
                .set_text(cx, "3D · no stored generated graph");
            return;
        };
        let Some(generated) = projection.generated.as_ref() else {
            self.structural_configurator_clear_scene(cx);
            self.ui
                .label(cx, ids!(sc_scene_title))
                .set_text(cx, "3D · MODEL_INCOMPLETE · no generated graph");
            return;
        };
        let stale_reference = self.structural_configurator_ui.draft_dirty
            || self.structural_configurator_ui.binding_stale
            || !projection.generated_current;
        let analysis_mode = matches!(
            self.structural_configurator_ui.view,
            ScViewMode::Analysis
                | ScViewMode::Loads
                | ScViewMode::Reactions
                | ScViewMode::Foundation
        );
        let analysis_available =
            !stale_reference && projection.analysis_current && projection.analysis.is_some();
        let render_mode = if analysis_mode && !analysis_available {
            ScViewMode::Architectural
        } else {
            self.structural_configurator_ui.view
        };
        let analysis_fp = projection
            .analysis
            .as_ref()
            .map(|snapshot| snapshot.content_fingerprint.as_str())
            .unwrap_or("no-analysis");
        let cache_key = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            projection.binding.input_fingerprint,
            generated.graph_fingerprint,
            analysis_fp,
            render_mode.name(),
            stale_reference,
            self.structural_configurator_ui.hide_cladding,
            self.structural_configurator_ui.cladding_alpha.to_bits(),
            self.structural_configurator_ui
                .visible_sections
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        if self.structural_configurator_ui.scene_cache_key.as_deref() == Some(cache_key.as_str())
            && !force_fit
            && self.structural_configurator_ui.pending_fit.is_none()
        {
            return;
        }
        let options = ScRenderOptions {
            mode: render_mode,
            visible_sections: self.structural_configurator_ui.visible_sections.clone(),
            hide_cladding: self.structural_configurator_ui.hide_cladding,
            cladding_alpha: self.structural_configurator_ui.cladding_alpha,
        };
        let mut render_scene = match sc_render::build_scene(generated, &options) {
            Ok(scene) => scene,
            Err(error) => {
                self.structural_configurator_set_status(
                    cx,
                    "RENDER_FAILURE",
                    &format!("3D projection failed: {error}"),
                );
                self.structural_configurator_clear_scene(cx);
                self.ui
                    .label(cx, ids!(sc_scene_title))
                    .set_text(cx, "3D · renderer error · see Inspector / report");
                self.structural_configurator_ui.scene_cache_key = None;
                return;
            }
        };
        if analysis_available && analysis_mode {
            if let Some(snapshot) = projection.analysis.as_ref() {
                if let Err(error) = sc_render::apply_analysis_snapshot(
                    &mut render_scene,
                    generated,
                    snapshot,
                    self.structural_configurator_ui.view,
                ) {
                    render_scene = match sc_render::build_scene(generated, &options) {
                        Ok(scene) => scene,
                        Err(rebuild_error) => {
                            self.structural_configurator_set_status(cx, "RENDER_FAILURE", &format!("Analysis overlay was rejected ({error}); base scene rebuild also failed ({rebuild_error})."));
                            self.structural_configurator_clear_scene(cx);
                            self.structural_configurator_ui.scene_cache_key = None;
                            return;
                        }
                    };
                    render_scene.status.push(format!(
                        "RESULT_VIEW_HOLD · current analysis overlay was rejected: {error}"
                    ));
                    self.structural_configurator_set_status(cx, "RESULT_VIEW_HOLD", &error);
                }
            }
        }
        if analysis_mode && !analysis_available {
            render_scene.status.push(
                "RESULT_VIEW_HOLD · no current source-matched analysis snapshot is available"
                    .into(),
            );
        }
        let bounds = render_scene.overall_bounds;
        let section_bounds = render_scene.section_bounds.clone();
        let picks = render_scene.picks.clone();
        let sources = render_scene.object_sources.clone();
        let by_section = render_scene.by_section.clone();
        let graph_fingerprint = render_scene.graph_fingerprint.clone();
        let desired_visibility: BTreeMap<ObjectId, bool> = render_scene
            .scene
            .objects()
            .map(|object| (object.object_id, object.visible))
            .collect();
        let viewer_state = self
            .ui
            .cad_viewport(cx, ids!(sc_model_viewport))
            .borrow()
            .map(|widget| widget.viewer().capture_state());
        let mut viewer = Viewer::default();
        *viewer.scene_mut() = render_scene.scene;
        if let Some(mut state) = viewer_state {
            // Keep the camera and navigation pose while preventing visibility
            // filters from the previous render mode from overriding this scene.
            state.hidden_object_ids.clear();
            state.isolated_object_ids.clear();
            state
                .selected_object_ids
                .retain(|id| desired_visibility.get(id).copied() == Some(true));
            state
                .highlighted_object_ids
                .retain(|id| desired_visibility.get(id).copied() == Some(true));
            state.highlighted_object_id = state
                .highlighted_object_id
                .filter(|id| desired_visibility.get(id).copied() == Some(true));
            if state
                .navigation_pivot
                .object_id
                .is_some_and(|id| desired_visibility.get(&id).copied() != Some(true))
            {
                state.navigation_pivot.object_id = None;
            }
            state.hover_hit = None;
            let _ = viewer.restore_state(&state);
        }
        for (object_id, visible) in &desired_visibility {
            viewer.scene_mut().set_visible(*object_id, *visible);
        }
        if let Some(viewport) = self
            .ui
            .cad_viewport(cx, ids!(sc_model_viewport))
            .borrow()
            .map(|widget| widget.rect())
        {
            if viewport.size.x > 1.0 && viewport.size.y > 1.0 {
                let _ = viewer
                    .set_viewport(viewer_core::Viewport::new(viewport.size.x, viewport.size.y));
            }
        }
        let fit_target = self.structural_configurator_ui.pending_fit.clone();
        let fit_bounds = match fit_target.as_ref() {
            Some(ScFitTarget::Building) => bounds,
            Some(ScFitTarget::Section(section_id)) => section_bounds.get(section_id).copied(),
            None if force_fit => bounds,
            None => None,
        };
        let measured = viewer.viewport().width > 1.0 && viewer.viewport().height > 1.0;
        if let Some(bounds) = fit_bounds.filter(|_| measured) {
            if let Err(error) = sc_render::fit_typed_bounds(&mut viewer, bounds, 1.18) {
                self.structural_configurator_set_status(cx, "INVALID_INPUT", &error);
            }
        } else if fit_target.is_some() || force_fit {
            if measured {
                self.structural_configurator_ui.pending_fit = None;
                self.structural_configurator_set_status(
                    cx,
                    "MODEL_INCOMPLETE",
                    "Typed bounds are not available for the selected fit target.",
                );
            } else {
                self.structural_configurator_ui.pending_fit =
                    fit_target.clone().or(Some(ScFitTarget::Building));
            }
        }
        let selected_object = viewer
            .scene()
            .selected_ids()
            .into_iter()
            .find_map(|id| picks.get(&id).cloned());
        self.ui
            .cad_viewport(cx, ids!(sc_model_viewport))
            .set_viewer(cx, viewer);
        self.structural_configurator_ui.scene_picks = picks;
        self.structural_configurator_ui.scene_sources = sources;
        self.structural_configurator_ui.scene_objects_by_section = by_section;
        self.structural_configurator_ui.scene_bounds = bounds;
        self.structural_configurator_ui.section_bounds = section_bounds;
        self.structural_configurator_ui.scene_graph_fingerprint = Some(graph_fingerprint);
        self.structural_configurator_ui
            .scene_overlay_snapshot_fingerprint = render_scene.overlay_snapshot_fingerprint.clone();
        self.structural_configurator_ui.scene_cache_key = Some(cache_key);
        self.structural_configurator_ui.selection = selected_object;
        if fit_target.is_some() && measured {
            self.structural_configurator_ui.pending_fit = None;
        }
        let title = if stale_reference {
            format!(
                "Stored generated reference · NOT CURRENT · {}",
                self.structural_configurator_ui.view.name()
            )
        } else if analysis_mode && !analysis_available {
            format!(
                "{} · RESULT_VIEW_HOLD · no current matched snapshot",
                self.structural_configurator_ui.view.name()
            )
        } else {
            format!(
                "{} · {}",
                self.structural_configurator_ui.view.name(),
                generated.graph_fingerprint
            )
        };
        self.ui.label(cx, ids!(sc_scene_title)).set_text(cx, &title);
        self.structural_configurator_update_inspector(cx);
        if !render_scene.status.is_empty() {
            let previous = self
                .structural_configurator_ui
                .projection
                .as_ref()
                .map(|value| value.summary.as_str())
                .unwrap_or("MODEL_INCOMPLETE");
            self.ui.label(cx, ids!(sc_summary)).set_text(
                cx,
                &format!("{previous}\n{}", render_scene.status.join("\n")),
            );
        }
    }
}

fn sc_section_bounds(section: &SectionConfig) -> Option<Bounds2> {
    let mut points = section
        .corners
        .iter()
        .map(|corner| DraftPoint::new(corner.xy_m[0], corner.xy_m[1]));
    let first = points.next()?;
    let mut bounds = Bounds2::from_points(first, first);
    for point in points {
        bounds = bounds.union(Bounds2::from_points(point, point));
    }
    Some(bounds)
}

fn sc_point_in_polygon(point: DraftPoint, corners: &[Corner]) -> bool {
    if corners.len() < 3 || !point.is_finite() {
        return false;
    }
    let mut inside = false;
    let mut previous = corners.len() - 1;
    for current in 0..corners.len() {
        let a = corners[current].xy_m;
        let b = corners[previous].xy_m;
        let crosses = (a[1] > point.y) != (b[1] > point.y)
            && point.x < (b[0] - a[0]) * (point.y - a[1]) / (b[1] - a[1]) + a[0];
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn sc_nearest_edge_screen(
    section: &SectionConfig,
    point: DraftPoint,
    camera: &Camera2D,
) -> Option<(usize, f64)> {
    if section.corners.len() < 2 {
        return None;
    }
    (0..section.corners.len())
        .map(|index| {
            let a = section.corners[index].xy_m;
            let b = section.corners[(index + 1) % section.corners.len()].xy_m;
            (
                index,
                sc_distance_to_segment(
                    point,
                    camera.world_to_screen(DraftPoint::new(a[0], a[1])),
                    camera.world_to_screen(DraftPoint::new(b[0], b[1])),
                ),
            )
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
}

fn sc_project_to_segment(point: DraftPoint, start: [f64; 2], end: [f64; 2]) -> DraftPoint {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let denominator = dx * dx + dy * dy;
    if !denominator.is_finite() || denominator <= f64::EPSILON {
        return DraftPoint::new(start[0], start[1]);
    }
    let t = (((point.x - start[0]) * dx + (point.y - start[1]) * dy) / denominator).clamp(0.0, 1.0);
    DraftPoint::new(start[0] + t * dx, start[1] + t * dy)
}
