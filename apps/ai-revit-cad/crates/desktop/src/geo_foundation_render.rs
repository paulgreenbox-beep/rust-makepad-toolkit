//! Read-only native projection of the selected GEO snapshot and linked foundations.
//!
//! Geometry here is a display projection of typed source data. This module does
//! not mutate GeoState or Document, generate soil facts, or invoke a solver.

use std::collections::{BTreeMap, BTreeSet};

use ai_revit_core::{
    EngineeringApprovalStatus, EngineeringSnapshotId, EngineeringSnapshotStatus,
    GeoFoundationAnalysisSnapshot, GeoFoundationGroundMap, GeoFoundationLayoutState,
    GeoFoundationSourceRevision,
};
use drafting_core::{Bounds2, Camera2D, Point2};
use geo_core::{
    Borehole, BoreholeId, GeoCoordinateFrame, GeoQuery, GeoQueryResult,
    GeoSectionId, GeoSnapshot, GeoSoilId, GeoState, GeoTriangle, GroundModel, GroundModelId,
    HorizonId, IgeId, LayerContact, Parameter, QueryStatus, ResolvedGeoSnapshot,
    SnapshotCurrentness, SourceId, TriangleId, WaterObservation, WaterScenario,
};
use viewer_core::{
    Aabb, Mesh, NavigationPivot, ObjectId, Scene, SceneObject, Triangle, Transform3, Vec3, Viewer,
};

use crate::drafting_canvas::{
    AnalysisCanvasLayer, ScreenLabel, ScreenLabelPlacement, ScreenSegment, ScreenTriangle,
};

const GEO_OBJECT_NAMESPACE: u64 = 0xd1;
const OBJECT_PAYLOAD_MASK: u64 = 0x00ff_ffff_ffff_ffff;
const MAX_RENDER_OBJECTS: usize = 80_000;
const MAX_RENDER_TRIANGLES: usize = 1_000_000;
const MAX_RENDER_CANVAS_PRIMITIVES: usize = 120_000;
const MAX_RENDER_COORDINATE_M: f64 = 1_000_000.0;
const MAX_RENDER_SECTION_SPACING_M: f64 = 100_000.0;
const BOREHOLE_MARKER_WIDTH_M: f64 = 0.12;
const BOREHOLE_INTERVAL_MARKER_WIDTH_M: f64 = 0.045;
const FOUNDATION_SAMPLE_DISPLAY_OFFSET_M: f64 = 0.01;
const GEOMETRY_TOLERANCE_M: f64 = 1.0e-7;
// Mirrors geo_core's private ELEVATION_TOL_M for display-only prism thickness.
const GEO_LAYER_THICKNESS_TOLERANCE_M: f64 = 1.0e-8;

const COLOR_TERRAIN: [f32; 4] = [0.42, 0.55, 0.62, 0.24];
const COLOR_TERRAIN_EDGE: [f32; 4] = [0.58, 0.72, 0.80, 0.95];
const COLOR_BOREHOLE: [f32; 4] = [0.95, 0.88, 0.40, 1.0];
const COLOR_WATER: [f32; 4] = [0.18, 0.66, 0.96, 1.0];
const COLOR_UNKNOWN: [f32; 4] = [0.96, 0.48, 0.16, 1.0];
const COLOR_CONFLICT: [f32; 4] = [0.96, 0.20, 0.22, 1.0];
const COLOR_FOUNDATION: [f32; 4] = [0.68, 0.71, 0.76, 0.95];
const COLOR_SELECTED: [f32; 4] = [1.0, 0.78, 0.16, 1.0];
const COLOR_TEXT: [f32; 4] = [0.89, 0.93, 0.97, 1.0];
const COLOR_MUTED: [f32; 4] = [0.62, 0.70, 0.78, 1.0];

/// Display modes never alter the persisted GeoState or FoundationLayout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeoViewMode {
    GroundPlan,
    Section,
    Ground3D,
    FoundationPlan,
    Foundation3D,
    Pressure2D,
    Settlement2D,
}

impl GeoViewMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::GroundPlan => "Ground plan",
            Self::Section => "Geological section",
            Self::Ground3D => "Ground 3D",
            Self::FoundationPlan => "Foundation plan",
            Self::Foundation3D => "Foundation 3D",
            Self::Pressure2D => "Contact pressure",
            Self::Settlement2D => "Settlement samples",
        }
    }

    const fn is_plan(self) -> bool {
        matches!(
            self,
            Self::GroundPlan
                | Self::FoundationPlan
                | Self::Pressure2D
                | Self::Settlement2D
        )
    }

    const fn is_foundation(self) -> bool {
        matches!(
            self,
            Self::FoundationPlan | Self::Foundation3D | Self::Pressure2D | Self::Settlement2D
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ParameterDisplayState {
    Missing,
    Conflicting,
}

/// Typed pick identities. They refer to Geo/Core records, never BIM ElementIds.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum GeoPick {
    Borehole {
        model_id: GroundModelId,
        borehole_id: BoreholeId,
    },
    BoreholeInterval {
        model_id: GroundModelId,
        borehole_id: BoreholeId,
        interval_id: geo_core::BoreholeIntervalId,
    },
    TerrainTriangle {
        model_id: GroundModelId,
        triangle_id: TriangleId,
    },
    LayerVolume {
        model_id: GroundModelId,
        triangle_id: TriangleId,
        horizon_id: HorizonId,
        ige_id: IgeId,
    },
    Contact {
        model_id: GroundModelId,
        horizon_id: HorizonId,
        ige_id: IgeId,
    },
    ContactUnknown {
        model_id: GroundModelId,
        horizon_id: HorizonId,
        borehole_id: BoreholeId,
        state: ParameterDisplayState,
    },
    Section {
        model_id: GroundModelId,
        section_id: GeoSectionId,
    },
    Ige {
        model_id: GroundModelId,
        ige_id: IgeId,
    },
    Soil {
        model_id: GroundModelId,
        soil_id: GeoSoilId,
    },
    UnknownZone {
        model_id: GroundModelId,
        index: usize,
    },
    WaterObservation {
        model_id: GroundModelId,
        scenario_id: geo_core::GroundScenarioId,
        observation_id: geo_core::WaterObservationId,
    },
    FoundationPlacement {
        layout_id: String,
        placement_id: String,
    },
    FoundationTier {
        layout_id: String,
        placement_id: String,
        tier_id: String,
    },
    FoundationGroundSample {
        foundation_id: String,
        index: usize,
    },
    FoundationCell {
        foundation_id: foundation_core::FoundationId,
        cell_index: usize,
    },
}

/// Presentation filters are authoritative for each newly built scene.
#[derive(Clone, Debug)]
pub struct GeoRenderOptions {
    pub view: GeoViewMode,
    pub show_terrain: bool,
    pub show_contacts: bool,
    pub show_boreholes: bool,
    /// Empty shows every IGE; otherwise only these IDs' display layers are drawn.
    pub visible_ige_ids: BTreeSet<IgeId>,
    pub show_unknown_zones: bool,
    pub show_water: bool,
    pub show_foundations: bool,
    pub selected_section: Option<GeoSectionId>,
    pub section_max_spacing_m: f64,
}

impl Default for GeoRenderOptions {
    fn default() -> Self {
        Self {
            view: GeoViewMode::GroundPlan,
            show_terrain: true,
            show_contacts: true,
            show_boreholes: true,
            visible_ige_ids: BTreeSet::new(),
            show_unknown_zones: true,
            show_water: true,
            show_foundations: true,
            selected_section: None,
            section_max_spacing_m: 1.0,
        }
    }
}

/// A Core-currentness-tagged immutable foundation result from the API projection.
#[derive(Clone, Copy, Debug)]
pub struct FoundationSnapshotRef<'a> {
    pub id: EngineeringSnapshotId,
    pub status: EngineeringSnapshotStatus,
    pub snapshot: &'a GeoFoundationAnalysisSnapshot,
}

/// Inputs are borrowed from the current Document projection.
#[derive(Clone, Copy, Debug)]
pub struct GeoFoundationRenderInput<'a> {
    pub state: &'a GeoState,
    pub resolved: &'a ResolvedGeoSnapshot,
    /// Set only when the UI selected an immutable snapshot stored in GeoState.
    pub snapshot_is_stored: bool,
    pub layout: Option<&'a GeoFoundationLayoutState>,
    /// Read-only Core cache, refreshed only by the explicit UI action.
    pub ground_map: Option<&'a GeoFoundationGroundMap>,
    pub foundation_snapshot: Option<FoundationSnapshotRef<'a>>,
    pub options: &'a GeoRenderOptions,
    pub selection: Option<&'a GeoPick>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoBounds2D {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl GeoBounds2D {
    fn empty() -> Self {
        Self {
            min: [f64::INFINITY; 2],
            max: [f64::NEG_INFINITY; 2],
        }
    }

    fn include(&mut self, point: [f64; 2]) {
        for axis in 0..2 {
            self.min[axis] = self.min[axis].min(point[axis]);
            self.max[axis] = self.max[axis].max(point[axis]);
        }
    }

    pub fn is_empty(self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1]
    }

    pub fn drafting_bounds(self) -> Option<Bounds2> {
        (!self.is_empty()).then(|| {
            Bounds2::from_points(
                Point2::new(self.min[0], self.min[1]),
                Point2::new(self.max[0], self.max[1]),
            )
        })
    }
}

/// Actual source references attached to a rendered object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeoObjectSources {
    pub source_ids: BTreeSet<SourceId>,
    pub geo_snapshot_id: Option<geo_core::GeoSnapshotId>,
    pub geo_source_revision: Option<geo_core::GeoSourceRevision>,
    pub foundation_layout_source: Option<GeoFoundationSourceRevision>,
    pub foundation_analysis_snapshot_id: Option<EngineeringSnapshotId>,
}

/// 3D scene plus semantic picks and typed source bounds.
#[derive(Debug)]
pub struct GeoFoundationRenderScene {
    pub scene: Scene,
    pub view: GeoViewMode,
    pub picks: BTreeMap<ObjectId, GeoPick>,
    pub object_sources: BTreeMap<ObjectId, GeoObjectSources>,
    pub overall_bounds: Option<Aabb>,
    pub model_bounds: BTreeMap<GroundModelId, Aabb>,
    pub plan_bounds: Option<GeoBounds2D>,
    pub section_bounds: BTreeMap<GeoSectionId, GeoBounds2D>,
    pub placement_bounds: BTreeMap<String, Aabb>,
    pub legend: Vec<GeoLegendEntry>,
    pub status: Vec<String>,
    pub source_fingerprint: String,
    pub snapshot_currentness: SnapshotCurrentness,
    pub snapshot_persisted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeoLegendEntry {
    pub ige_id: IgeId,
    pub ige_name: String,
    pub soil_id: GeoSoilId,
    pub soil_name: String,
    pub color: [f32; 4],
}

#[derive(Default)]
struct RenderBudget {
    objects: usize,
    triangles: usize,
    canvas_primitives: usize,
}

#[derive(Default)]
struct TypedBounds {
    overall: Option<Aabb>,
    model: Option<Aabb>,
    plan: Option<GeoBounds2D>,
}

struct ValidatedInput<'a> {
    model: &'a GroundModel,
    snapshot: &'a GeoSnapshot,
    currentness: SnapshotCurrentness,
    snapshot_persisted: bool,
}

struct ValidatedFoundationGroundMap<'a> {
    map: &'a GeoFoundationGroundMap,
    center_m: [f64; 3],
    length_x_m: f64,
    width_y_m: f64,
    dx_m: f64,
    dy_m: f64,
    legend_entries: Vec<GeoLegendEntry>,
    known_cell_count: usize,
    unresolved_cell_count: usize,
}

/// Builds display-only 3D meshes from the explicit model topology.
pub fn scene(
    input: &GeoFoundationRenderInput<'_>,
) -> Result<GeoFoundationRenderScene, String> {
    let validated = validate_input(input)?;
    let model = validated.model;
    let records = &input.resolved.snapshot.records;
    let mut output = GeoFoundationRenderScene {
        scene: Scene::new(),
        view: input.options.view,
        picks: BTreeMap::new(),
        object_sources: BTreeMap::new(),
        overall_bounds: None,
        model_bounds: BTreeMap::new(),
        plan_bounds: None,
        section_bounds: BTreeMap::new(),
        placement_bounds: BTreeMap::new(),
        legend: build_legend(records, model, input.options)?,
        status: source_status(input, &validated),
        source_fingerprint: input.resolved.snapshot.selection_fingerprint.clone(),
        snapshot_currentness: validated.currentness,
        snapshot_persisted: validated.snapshot_persisted,
    };
    let mut bounds = TypedBounds::default();
    let mut budget = RenderBudget::default();
    add_source_bounds(input, model, &mut bounds)?;
    if input.options.view.is_plan()
        && input.options.show_boreholes
        && input.options.show_contacts
    {
        output.status.push(
            "PLAN PICK · repeated clicks at overlapping borehole/contact markers cycle through the hit borehole and visible contacts; marker positions stay fixed.".into(),
        );
    }

    if input.options.view == GeoViewMode::FoundationPlan && input.options.show_foundations {
        match validate_foundation_ground_map(input) {
            Ok(ground_map) => {
                let min_x = ground_map.center_m[0] - ground_map.length_x_m * 0.5;
                let max_x = ground_map.center_m[0] + ground_map.length_x_m * 0.5;
                let min_y = ground_map.center_m[1] - ground_map.width_y_m * 0.5;
                let max_y = ground_map.center_m[1] + ground_map.width_y_m * 0.5;
                for point in [[min_x, min_y], [max_x, min_y], [max_x, max_y], [min_x, max_y]] {
                    include_xy(&mut bounds, point, "foundation ground map footprint")?;
                }
                for entry in &ground_map.legend_entries {
                    if ige_is_visible(input.options, &entry.ige_id)
                        && !output.legend.iter().any(|existing| existing.ige_id == entry.ige_id)
                    {
                        output.legend.push(entry.clone());
                    }
                }
                output.status.push(format!(
                    "FOUNDATION GEO MAP · {} cached midpoint samples · {} known single-IGE · {} unresolved/interface; finite samples do not prove continuous coverage.",
                    ground_map.map.cells.len(),
                    ground_map.known_cell_count,
                    ground_map.unresolved_cell_count
                ));
            }
            Err(reason) => output.status.push(format!("FOUNDATION GEO MAP HOLD · {reason}")),
        }
    }

    if input.options.view.is_plan() || input.options.view == GeoViewMode::Section {
        output.plan_bounds = bounds.plan;
        if let Some(model_bounds) = bounds.model {
            output.model_bounds.insert(model.id.clone(), model_bounds);
            output.overall_bounds = bounds.overall;
        }
        if input.options.view == GeoViewMode::Section {
            if let Some(section_id) = &input.options.selected_section {
                if let Ok((_, section_bounds)) = query_section(input, section_id) {
                    output.section_bounds.insert(section_id.clone(), section_bounds);
                }
            }
        }
        return Ok(output);
    }

    if input.options.show_terrain {
        add_terrain_scene(input, model, &mut output, &mut budget)?;
    }
    if input.options.show_contacts {
        match input.resolved.query(&GeoQuery::LayerVolumes) {
            Ok(result) if result.status == QueryStatus::Known => {
                output.status.push(
                    "LAYER VOLUME GATE · Core status KNOWN; drawing explicit TIN prisms; numeric volume totals are not displayed.".into(),
                );
                for reason in &result.reasons {
                    output.status.push(format!("LAYER VOLUME CORE · {reason}"));
                }
                add_layer_volume_scene(input, model, &mut output, &mut budget)?;
            }
            Ok(result) => {
                if result.reasons.is_empty() {
                    output.status.push(format!(
                        "LAYER VOLUME HOLD · Core status {:?} returned no reason; contact surfaces remain display-only.",
                        result.status
                    ));
                } else {
                    for reason in &result.reasons {
                        output.status.push(format!(
                            "LAYER VOLUME HOLD · Core status {:?} · {reason} · contact surfaces remain display-only.",
                            result.status
                        ));
                    }
                }
                add_contact_scene(input, model, &mut output, &mut budget)?;
            }
            Err(error) => {
                output.status.push(format!(
                    "LAYER VOLUME HOLD · Core query error: {error} · contact surfaces remain display-only."
                ));
                add_contact_scene(input, model, &mut output, &mut budget)?;
            }
        }
    }
    if input.options.show_boreholes {
        add_borehole_scene(input, model, &mut output, &mut budget)?;
    }
    if input.options.show_water {
        add_water_scene(input, model, &mut output, &mut budget)?;
    }
    if input.options.show_foundations && input.options.view == GeoViewMode::Foundation3D {
        add_foundation_layout_scene(input, &mut output, &mut bounds, &mut budget)?;
        if let Some(snapshot) = current_foundation_snapshot(input, &mut output.status)? {
            add_foundation_result_scene(
                input,
                snapshot,
                &mut output,
                &mut bounds,
                &mut budget,
            )?;
        }
    }

    output.plan_bounds = bounds.plan;
    if let Some(model_bounds) = bounds.model {
        output.model_bounds.insert(model.id.clone(), model_bounds);
    }
    output.overall_bounds = bounds.overall;
    append_display_limits(&mut output.status, input);
    Ok(output)
}

/// Projects a world XY map, placement footprints and current result samples.
pub fn plan_layer(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
) -> Result<AnalysisCanvasLayer, String> {
    let validated = validate_input(input)?;
    if !input.options.view.is_plan() {
        return Err("plan_layer requires a plan or 2D foundation view".into());
    }
    camera
        .validate()
        .map_err(|error| format!("invalid plan camera: {error}"))?;

    let model = validated.model;
    let records = &input.resolved.snapshot.records;
    let mut layer = AnalysisCanvasLayer::default();
    let mut budget = RenderBudget::default();
    if input.options.show_terrain {
        draw_plan_tin(model, records, camera, &mut layer, &mut budget)?;
    }
    if input.options.show_unknown_zones {
        draw_unknown_zones(input, model, camera, &mut layer, &mut budget)?;
    }
    if input.options.show_contacts {
        draw_contact_markers(input, model, records, camera, &mut layer, &mut budget)?;
    }
    if input.options.show_boreholes {
        draw_borehole_markers(input, model, records, camera, &mut layer, &mut budget)?;
    }
    if input.options.show_water {
        draw_water_markers(input, model, camera, &mut layer, &mut budget)?;
    }
    if input.options.show_foundations && input.options.view.is_foundation() {
        if input.options.view == GeoViewMode::FoundationPlan {
            draw_foundation_ground_map(input, camera, &mut layer, &mut budget)?;
        }
        draw_foundation_layout(input, camera, &mut layer, &mut budget)?;
    }
    if matches!(
        input.options.view,
        GeoViewMode::Pressure2D | GeoViewMode::Settlement2D
    ) {
        draw_foundation_samples(input, camera, &mut layer, &mut budget)?;
    }
    if let Some(section_id) = &input.options.selected_section {
        draw_section_trace(input, model, section_id, camera, &mut layer, &mut budget)?;
    }
    draw_frame_labels(input, model, camera, &mut layer, &mut budget)?;
    Ok(layer)
}

/// Draws station/elevation samples returned by the authoritative Geo query.
pub fn section_layer(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
) -> Result<AnalysisCanvasLayer, String> {
    let validated = validate_input(input)?;
    if input.options.view != GeoViewMode::Section {
        return Err("section_layer requires the Section view".into());
    }
    camera
        .validate()
        .map_err(|error| format!("invalid section camera: {error}"))?;
    let Some(section_id) = input.options.selected_section.as_ref() else {
        return Ok(hold_layer(
            camera,
            "Section HOLD · select an interpreted section",
            "Station (m) · absolute elevation (m)",
        ));
    };
    let (query_result, _) = query_section(input, section_id)?;
    let samples = query_result
        .section
        .as_ref()
        .ok_or_else(|| "section query returned no typed section samples".to_string())?;
    let mut layer = AnalysisCanvasLayer::default();
    let mut budget = RenderBudget::default();
    let mut unknown_samples = 0usize;

    for pair in samples.samples.windows(2) {
        let left = &pair[0];
        let right = &pair[1];
        let (Some(left_column), Some(right_column)) =
            (known_column(&left.result), known_column(&right.result))
        else {
            continue;
        };
        add_canvas_segment(
            &mut layer,
            camera.world_to_screen(Point2::new(left.station_m, left_column.terrain_abs_m)),
            camera.world_to_screen(Point2::new(right.station_m, right_column.terrain_abs_m)),
            COLOR_TERRAIN_EDGE,
            2.4,
            &mut budget,
        )?;
        for (left_layer, right_layer) in left_column
            .layers
            .iter()
            .zip(&right_column.layers)
            .take_while(|(a, b)| a.ige_id == b.ige_id)
        {
            if !ige_is_visible(input.options, &left_layer.ige_id) {
                continue;
            }
            let color = ige_color(&left_layer.ige_id);
            if left_layer.pinched || right_layer.pinched {
                add_canvas_segment(
                    &mut layer,
                    camera.world_to_screen(Point2::new(
                        left.station_m,
                        left_layer.bottom_abs_m,
                    )),
                    camera.world_to_screen(Point2::new(
                        right.station_m,
                        right_layer.bottom_abs_m,
                    )),
                    color,
                    1.5,
                    &mut budget,
                )?;
            } else {
                let points = [
                    camera.world_to_screen(Point2::new(
                        left.station_m,
                        left_layer.top_abs_m,
                    )),
                    camera.world_to_screen(Point2::new(
                        right.station_m,
                        right_layer.top_abs_m,
                    )),
                    camera.world_to_screen(Point2::new(
                        right.station_m,
                        right_layer.bottom_abs_m,
                    )),
                    camera.world_to_screen(Point2::new(
                        left.station_m,
                        left_layer.bottom_abs_m,
                    )),
                ];
                add_canvas_triangle(
                    &mut layer,
                    [points[0], points[1], points[2]],
                    with_alpha(color, 0.34),
                    &mut budget,
                )?;
                add_canvas_triangle(
                    &mut layer,
                    [points[0], points[2], points[3]],
                    with_alpha(color, 0.34),
                    &mut budget,
                )?;
            }
        }
    }
    for sample in &samples.samples {
        if sample.result.status != QueryStatus::Known {
            unknown_samples = unknown_samples.saturating_add(1);
        }
    }
    push_canvas_label(
        &mut layer,
        Point2::new(16.0, 24.0),
        &format!(
            "Section {} · bounded query spacing ≤ {:.3} m",
            section_id.as_str(),
            input.options.section_max_spacing_m
        ),
        COLOR_TEXT,
        ScreenLabelPlacement::Hud,
        &mut budget,
    )?;
    push_canvas_label(
        &mut layer,
        Point2::new(16.0, camera.viewport_height - 18.0),
        &format!(
            "Station (m) · absolute elevation (m) · {} · no extrapolation",
            datum_label(&validated.model.frame)
        ),
        COLOR_MUTED,
        ScreenLabelPlacement::Footer,
        &mut budget,
    )?;
    if unknown_samples > 0 {
        push_canvas_label(
            &mut layer,
            Point2::new(16.0, 46.0),
            &format!(
                "GEO HOLD · {unknown_samples} samples UNKNOWN / CONFLICTING / OUTSIDE COVERAGE; no boundary filled"
            ),
            COLOR_UNKNOWN,
            ScreenLabelPlacement::Hud,
            &mut budget,
        )?;
    }
    Ok(layer)
}

/// Converts plan clicks to source picks without modifying domain selection.
pub fn pick_plan(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
    screen_point: Point2,
    tolerance_px: f64,
) -> Result<Option<GeoPick>, String> {
    let validated = validate_input(input)?;
    if !input.options.view.is_plan() {
        return Ok(None);
    }
    camera
        .validate()
        .map_err(|error| format!("invalid plan camera: {error}"))?;
    if !screen_point.is_finite() || !tolerance_px.is_finite() || tolerance_px < 0.0 {
        return Err("plan pick point/tolerance must be finite and tolerance nonnegative".into());
    }
    let model = validated.model;
    let records = &input.resolved.snapshot.records;
    let world = camera.screen_to_world(screen_point);

    if matches!(
        input.options.view,
        GeoViewMode::Pressure2D | GeoViewMode::Settlement2D
    ) {
        if let Some((snapshot, placement)) = current_foundation_snapshot_for_pick(input)? {
            for (index, cell) in snapshot.foundation_result.cells.iter().enumerate().rev() {
                let corners = foundation_cell_xy(
                    placement,
                    &snapshot.input.effective_input,
                    cell.x_m,
                    cell.y_m,
                    snapshot.foundation_result.mesh.dx_m,
                    snapshot.foundation_result.mesh.dy_m,
                )?;
                if point_in_polygon(world.x, world.y, &corners) {
                    return Ok(Some(GeoPick::FoundationCell {
                        foundation_id: snapshot.input.effective_input.foundation_id.clone(),
                        cell_index: index,
                    }));
                }
            }
        }
    }
    if input.options.show_foundations && input.options.view == GeoViewMode::FoundationPlan {
        if let Ok(ground_map) = validate_foundation_ground_map(input) {
            for cell in ground_map.map.cells.iter().rev() {
                if known_base_ige(cell)
                    .is_some_and(|ige_id| !ige_is_visible(input.options, ige_id))
                {
                    continue;
                }
                let corners = foundation_ground_map_cell_corners(&ground_map, cell);
                if point_in_polygon(world.x, world.y, &corners) {
                    return Ok(Some(GeoPick::FoundationGroundSample {
                        foundation_id: ground_map.map.foundation_id.clone(),
                        index: cell.index,
                    }));
                }
            }
        }
    }
    if input.options.show_foundations && input.options.view.is_foundation() {
        if let Some(layout) = input.layout {
            let geometry = foundation_core::derive_layout_geometry(&layout.input.layout)
                .map_err(|error| format!("foundation layout projection failed: {error}"))?;
            for element in geometry.elements.iter().rev() {
                if point_in_polygon(world.x, world.y, &element.corners_xy_m) {
                    return Ok(Some(GeoPick::FoundationPlacement {
                        layout_id: layout.input.layout.id.clone(),
                        placement_id: element.id.as_str().to_owned(),
                    }));
                }
            }
        }
    }
    if input.options.show_water {
        for observation in water_observations(input)?.into_iter().rev() {
            let point =
                camera.world_to_screen(Point2::new(observation.xy_m[0], observation.xy_m[1]));
            if screen_distance(point, screen_point) <= tolerance_px {
                return Ok(Some(GeoPick::WaterObservation {
                    model_id: model.id.clone(),
                    scenario_id: input.resolved.scenario.id.clone(),
                    observation_id: observation.id.clone(),
                }));
            }
        }
    }
    if input.options.show_unknown_zones {
        for (index, zone) in model.unknown_zones.iter().enumerate().rev() {
            if point_in_polygon(world.x, world.y, &zone.polygon_xy_m) {
                return Ok(Some(GeoPick::UnknownZone {
                    model_id: model.id.clone(),
                    index,
                }));
            }
        }
    }
    let mut overlap_candidates = Vec::with_capacity(
        geo_core::MAX_MODEL_WELLS + geo_core::MAX_CONTACTS,
    );
    if input.options.show_boreholes {
        for borehole_id in model.borehole_ids.iter().rev() {
            let borehole = required_borehole(records, borehole_id)?;
            let point = camera.world_to_screen(Point2::new(
                borehole.collar_m[0],
                borehole.collar_m[1],
            ));
            if screen_distance(point, screen_point) <= tolerance_px {
                overlap_candidates.push(GeoPick::Borehole {
                    model_id: model.id.clone(),
                    borehole_id: borehole_id.clone(),
                });
            }
        }
    }
    if input.options.show_contacts {
        for contact in model.contacts.iter().rev() {
            if !ige_is_visible(input.options, &contact.ige_id) {
                continue;
            }
            for borehole_id in contact.elevations_abs_m.keys().rev() {
                let borehole = required_borehole(records, borehole_id)?;
                let point = camera.world_to_screen(Point2::new(
                    borehole.collar_m[0],
                    borehole.collar_m[1],
                ));
                if screen_distance(point, screen_point) <= tolerance_px {
                    let candidate = GeoPick::Contact {
                        model_id: model.id.clone(),
                        horizon_id: contact.id.clone(),
                        ige_id: contact.ige_id.clone(),
                    };
                    if !overlap_candidates.contains(&candidate) {
                        overlap_candidates.push(candidate);
                    }
                    break;
                }
            }
        }
    }
    if let Some(candidate) = cycle_pick(&overlap_candidates, input.selection)? {
        return Ok(Some(candidate));
    }
    if let Some(section_id) = &input.options.selected_section {
        if let Some(section) = records.sections.get(section_id) {
            for pair in section.polyline_xy_m.windows(2) {
                if screen_point_segment_distance(
                    screen_point,
                    camera.world_to_screen(Point2::new(pair[0][0], pair[0][1])),
                    camera.world_to_screen(Point2::new(pair[1][0], pair[1][1])),
                ) <= tolerance_px
                {
                    return Ok(Some(GeoPick::Section {
                        model_id: model.id.clone(),
                        section_id: section_id.clone(),
                    }));
                }
            }
        }
    }
    if input.options.show_terrain {
        for triangle in model.triangles.iter().rev() {
            let points = triangle_plan_points(triangle, records)?;
            if point_in_triangle([world.x, world.y], points) {
                return Ok(Some(GeoPick::TerrainTriangle {
                    model_id: model.id.clone(),
                    triangle_id: triangle.id.clone(),
                }));
            }
        }
    }
    Ok(None)
}

fn cycle_pick(
    candidates: &[GeoPick],
    selection: Option<&GeoPick>,
) -> Result<Option<GeoPick>, String> {
    let candidate_limit = geo_core::MAX_MODEL_WELLS + geo_core::MAX_CONTACTS;
    if candidates.len() > candidate_limit {
        return Err("plan overlap pick candidates exceed the bounded borehole/contact limit".into());
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let next_index = selection
        .and_then(|selected| candidates.iter().position(|candidate| candidate == selected))
        .map_or(0, |current_index| (current_index + 1) % candidates.len());
    Ok(candidates.get(next_index).cloned())
}

/// Fits supplied typed bounds while preserving scene visibility and selection.
/// The navigation pivot follows the fitted source bounds.
pub fn fit_typed_bounds(
    viewer: &mut Viewer,
    bounds: Aabb,
    margin: f64,
) -> Result<(), String> {
    if bounds.is_empty() {
        return Ok(());
    }
    validate_point(bounds.min, "typed fit minimum")?;
    validate_point(bounds.max, "typed fit maximum")?;
    if !margin.is_finite() || !(1.0..=10.0).contains(&margin) {
        return Err("fit margin must be finite and within 1..=10".into());
    }
    let mut camera = *viewer.camera();
    camera
        .fit_bounds(bounds, viewer.viewport(), margin)
        .map_err(|error| format!("could not fit typed GEO bounds: {error}"))?;
    let mut state = viewer.capture_state();
    state.camera = camera.state();
    state.navigation_pivot = NavigationPivot {
        object_id: None,
        world_point: bounds.center(),
    };
    viewer
        .restore_state(&state)
        .map_err(|error| format!("could not apply typed GEO fit: {error}"))?;
    Ok(())
}

fn validate_input<'a>(
    input: &'a GeoFoundationRenderInput<'a>,
) -> Result<ValidatedInput<'a>, String> {
    if !input.options.section_max_spacing_m.is_finite()
        || input.options.section_max_spacing_m <= 0.0
        || input.options.section_max_spacing_m > MAX_RENDER_SECTION_SPACING_M
    {
        return Err("section spacing is outside finite renderer bounds".into());
    }
    let snapshot = &input.resolved.snapshot;
    geo_core::validate_snapshot_binding(snapshot).map_err(|error| {
        format!("GEO DATA HOLD · selected snapshot binding failed: {error}")
    })?;
    if snapshot.selection.scenario_id.as_ref() != Some(&input.resolved.scenario.id)
        || input.resolved.scenario.model_id != snapshot.selection.model_id
        || snapshot.records.scenarios.get(&input.resolved.scenario.id)
            != Some(&input.resolved.scenario)
    {
        return Err("GEO DATA HOLD · resolved scenario differs from immutable closure".into());
    }
    let model = snapshot
        .records
        .models
        .get(&snapshot.selection.model_id)
        .ok_or_else(|| "GEO DATA HOLD · selected closure has no GroundModel".to_string())?;
    if model.id != snapshot.selection.model_id {
        return Err("GEO DATA HOLD · model key/identity mismatch".into());
    }
    if model.borehole_ids.len() > geo_core::MAX_MODEL_WELLS
        || model.triangles.len() > geo_core::MAX_TRIANGLES
        || model.contacts.len() > geo_core::MAX_CONTACTS
        || model.unknown_zones.len() > geo_core::MAX_RECORDS
    {
        return Err("GEO DATA HOLD · model exceeds its source record limits".into());
    }
    for borehole_id in &model.borehole_ids {
        required_borehole(&snapshot.records, borehole_id)?;
    }
    if let Some(section_id) = &input.options.selected_section {
        if !model.section_ids.contains(section_id) {
            return Err(format!(
                "selected section {} is outside GroundModel {}",
                section_id.as_str(),
                model.id.as_str()
            ));
        }
    }
    let currentness = geo_core::snapshot_currentness(input.state, snapshot)
        .map_err(|error| format!("GEO DATA HOLD · currentness query failed: {error}"))?;
    let snapshot_persisted = if input.snapshot_is_stored {
        let stored = input
            .state
            .snapshots
            .get(&snapshot.id)
            .ok_or_else(|| "selected GEO snapshot is marked stored but is absent from GeoState".to_string())?;
        if stored != snapshot {
            return Err("selected GEO snapshot does not match the immutable GeoState entry".into());
        }
        true
    } else {
        false
    };
    Ok(ValidatedInput {
        model,
        snapshot,
        currentness,
        snapshot_persisted,
    })
}

fn source_status(
    input: &GeoFoundationRenderInput<'_>,
    validated: &ValidatedInput<'_>,
) -> Vec<String> {
    let model = validated.model;
    let mut status = vec![
        format!(
            "READ-ONLY GEO VIEW · snapshot {} · selection {} · source op {}",
            validated.snapshot.id.as_str(),
            validated.snapshot.selection_fingerprint,
            validated.snapshot.source_revision.operation_id
        ),
        format!(
            "GEO source currentness {:?} · {}",
            validated.currentness,
            if validated.snapshot_persisted {
                "saved immutable snapshot"
            } else {
                "derived view snapshot; not persisted"
            }
        ),
        format!(
            "Frame {} · CRS {} · vertical datum {} · world XYZ metres, Z up",
            nonempty(&model.frame.name, "unnamed"),
            optional_text(model.frame.crs.as_deref(), "unspecified"),
            optional_text(model.frame.vertical_datum.as_deref(), "unspecified")
        ),
        "KNOWN is source state, not review or engineering approval.".into(),
        "TIN surfaces use explicit triangle connectivity and known source vertices only; no Delaunay, extrapolation or inferred layer thickness.".into(),
        "Unknown-zone polygons have XY only; no 3D elevation or volume is invented.".into(),
        "Rendering, camera and selection do not mutate GeoState or Document.".into(),
        "ULS/SLS, bearing and engineering approval remain HOLD.".into(),
    ];
    if validated.currentness == SnapshotCurrentness::Stale {
        status.push(
            "STALE GEO SNAPSHOT · display uses its immutable closure; foundation result overlay is held.".into(),
        );
    }
    if let Some(layout) = input.layout {
        status.push(format!(
            "Foundation layout {} · source operation {} · geometry only",
            layout.input.layout.id,
            layout.source_revision.operation_id.get()
        ));
    } else if input.options.view.is_foundation() {
        status.push("FOUNDATION LAYOUT HOLD · no typed layout projection.".into());
    }
    status
}

fn append_display_limits(status: &mut Vec<String>, input: &GeoFoundationRenderInput<'_>) {
    if input.options.show_boreholes {
        status.push(format!(
            "Borehole interval width {:.3} m is a display marker, not a surveyed diameter.",
            BOREHOLE_INTERVAL_MARKER_WIDTH_M
        ));
    }
    if input.options.view == GeoViewMode::Foundation3D
        && status.iter().any(|line| line.starts_with("FOUNDATION RESULT CURRENT"))
    {
        status.push(format!(
            "Pressure sample patches are offset +{FOUNDATION_SAMPLE_DISPLAY_OFFSET_M:.3} m in display only."
        ));
    }
}

fn add_source_bounds(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    bounds: &mut TypedBounds,
) -> Result<(), String> {
    let records = &input.resolved.snapshot.records;
    for borehole_id in &model.borehole_ids {
        let borehole = required_borehole(records, borehole_id)?;
        include_xyz(bounds, borehole.collar_m, "borehole collar")?;
        for interval in &borehole.intervals {
            validate_positive(interval.bottom_depth_m - interval.top_depth_m, "borehole interval")?;
            include_xyz(
                bounds,
                [
                    borehole.collar_m[0],
                    borehole.collar_m[1],
                    borehole.collar_m[2] - interval.bottom_depth_m,
                ],
                "borehole interval bottom",
            )?;
        }
    }
    for triangle in &model.triangles {
        for point in triangle_world_points(triangle, records)? {
            include_xyz(bounds, [point.x, point.y, point.z], "terrain TIN vertex")?;
        }
    }
    for zone in &model.unknown_zones {
        if zone.polygon_xy_m.len() < 3 || zone.polygon_xy_m.len() > geo_core::MAX_SECTION_POINTS {
            return Err("unknown-zone polygon exceeds the display vertex limit".into());
        }
        for point in &zone.polygon_xy_m {
            include_xy(bounds, *point, "unknown-zone vertex")?;
        }
    }
    for contact in &model.contacts {
        for (id, parameter) in &contact.elevations_abs_m {
            let borehole = required_borehole(records, id)?;
            if let Parameter::Known { value, .. } = parameter {
                include_xyz(
                    bounds,
                    [borehole.collar_m[0], borehole.collar_m[1], *value],
                    "known contact source vertex",
                )?;
            }
        }
    }
    for section_id in &model.section_ids {
        if let Some(section) = records.sections.get(section_id) {
            for point in &section.polyline_xy_m {
                include_xy(bounds, *point, "section trace")?;
            }
        }
    }
    for observation in water_observations(input)? {
        include_xy(bounds, observation.xy_m, "water observation")?;
        if let Parameter::Known { value, .. } = observation.elevation_abs_m {
            include_xyz(
                bounds,
                [observation.xy_m[0], observation.xy_m[1], value],
                "water observation elevation",
            )?;
        }
    }
    if let Some(layout) = input.layout {
        let geometry = foundation_core::derive_layout_geometry(&layout.input.layout)
            .map_err(|error| format!("foundation layout geometry failed: {error}"))?;
        for element in geometry.elements {
            for corner in element.corners_xy_m {
                include_xyz(
                    bounds,
                    [corner[0], corner[1], element.base_elevation_m],
                    "foundation base",
                )?;
                include_xy(bounds, corner, "foundation footprint")?;
            }
            for tier in element.tiers {
                for corner in tier.corners_xy_m {
                    include_prism_point(
                        bounds,
                        corner,
                        tier.center_m[2],
                        tier.center_m[2] + tier.thickness_m,
                        "foundation tier",
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn add_terrain_scene(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let records = &input.resolved.snapshot.records;
    for triangle in &model.triangles {
        let points = triangle_world_points(triangle, records)?;
        let points = orient_triangle_z_up(points);
        add_scene_object(
            input,
            output,
            budget,
            GeoPick::TerrainTriangle {
                model_id: model.id.clone(),
                triangle_id: triangle.id.clone(),
            },
            format!("Explicit TIN terrain triangle {}", triangle.id.as_str()),
            vec![Triangle::new(0, points[0], points[1], points[2])],
            COLOR_TERRAIN,
            triangle_sources(triangle, model, records)?,
            None,
            None,
        )?;
    }
    Ok(())
}

fn add_layer_volume_scene(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let records = &input.resolved.snapshot.records;
    let mut pinched_prisms = 0usize;
    let mut invalid_prisms = 0usize;
    let mut visible_prisms = 0usize;

    for triangle in &model.triangles {
        let wells = [
            required_borehole(records, &triangle.vertices[0])?,
            required_borehole(records, &triangle.vertices[1])?,
            required_borehole(records, &triangle.vertices[2])?,
        ];
        let mut top_abs_m = [
            wells[0].collar_m[2],
            wells[1].collar_m[2],
            wells[2].collar_m[2],
        ];
        let mut previous_contact_sources: Option<BTreeSet<SourceId>> = None;

        for contact in &model.contacts {
            let (bottom_abs_m, current_contact_sources) =
                known_contact_elevations(contact, triangle, records)?;
            let prism_top_abs_m = top_abs_m;
            // Preserve ordered contacts even when this IGE is hidden or a prism is skipped.
            top_abs_m = bottom_abs_m;
            let prior_contact_sources = previous_contact_sources.take();
            previous_contact_sources = Some(current_contact_sources.clone());

            if !ige_is_visible(input.options, &contact.ige_id) {
                continue;
            }

            let coordinates_are_valid = prism_top_abs_m
                .iter()
                .chain(bottom_abs_m.iter())
                .all(|value| value.is_finite() && value.abs() <= MAX_RENDER_COORDINATE_M);
            if !coordinates_are_valid {
                invalid_prisms = invalid_prisms.saturating_add(1);
                continue;
            }
            let thickness_m = [
                prism_top_abs_m[0] - bottom_abs_m[0],
                prism_top_abs_m[1] - bottom_abs_m[1],
                prism_top_abs_m[2] - bottom_abs_m[2],
            ];
            if thickness_m.iter().any(|thickness| !thickness.is_finite()) {
                invalid_prisms = invalid_prisms.saturating_add(1);
                continue;
            }
            let exceeds_negative_tolerance = (0..3).any(|index| {
                thickness_m[index]
                    < -geo_layer_elevation_tolerance_m(
                        prism_top_abs_m[index],
                        bottom_abs_m[index],
                    )
            });
            if exceeds_negative_tolerance {
                invalid_prisms = invalid_prisms.saturating_add(1);
                continue;
            }
            let is_pinched = (0..3).any(|index| {
                thickness_m[index]
                    <= geo_layer_elevation_tolerance_m(
                        prism_top_abs_m[index],
                        bottom_abs_m[index],
                    )
            });
            if is_pinched {
                pinched_prisms = pinched_prisms.saturating_add(1);
                continue;
            }

            let xy = [
                [wells[0].collar_m[0], wells[0].collar_m[1]],
                [wells[1].collar_m[0], wells[1].collar_m[1]],
                [wells[2].collar_m[0], wells[2].collar_m[1]],
            ];
            let signed_double_area = (xy[1][0] - xy[0][0]) * (xy[2][1] - xy[0][1])
                - (xy[1][1] - xy[0][1]) * (xy[2][0] - xy[0][0]);
            if !signed_double_area.is_finite() || signed_double_area.abs() <= f64::EPSILON {
                invalid_prisms = invalid_prisms.saturating_add(1);
                continue;
            }

            let mut top = [
                Vec3::new(xy[0][0], xy[0][1], prism_top_abs_m[0]),
                Vec3::new(xy[1][0], xy[1][1], prism_top_abs_m[1]),
                Vec3::new(xy[2][0], xy[2][1], prism_top_abs_m[2]),
            ];
            let mut bottom = [
                Vec3::new(xy[0][0], xy[0][1], bottom_abs_m[0]),
                Vec3::new(xy[1][0], xy[1][1], bottom_abs_m[1]),
                Vec3::new(xy[2][0], xy[2][1], bottom_abs_m[2]),
            ];
            if signed_double_area < 0.0 {
                top.swap(1, 2);
                bottom.swap(1, 2);
            }

            let mut sources = triangle_sources(triangle, model, records)?;
            sources.extend(current_contact_sources);
            if let Some(previous_sources) = prior_contact_sources {
                sources.extend(previous_sources);
            }
            let ige = records.iges.get(&contact.ige_id).ok_or_else(|| {
                format!("snapshot closure lacks IGE {}", contact.ige_id.as_str())
            })?;
            sources.extend(ige.source_ids.iter().cloned());

            let legend = legend_entry(records, &contact.ige_id)?;
            let pick = GeoPick::LayerVolume {
                model_id: model.id.clone(),
                triangle_id: triangle.id.clone(),
                horizon_id: contact.id.clone(),
                ige_id: contact.ige_id.clone(),
            };
            add_scene_object(
                input,
                output,
                budget,
                pick,
                format!(
                    "Explicit TIN layer prism · triangle {} · horizon {} · IGE {}",
                    triangle.id.as_str(),
                    contact.id.as_str(),
                    legend.ige_name
                ),
                triangular_prism_triangles(top, bottom),
                legend.color,
                sources,
                None,
                None,
            )?;
            visible_prisms = visible_prisms.saturating_add(1);
        }
    }

    if pinched_prisms > 0 || invalid_prisms > 0 {
        output.status.push(format!(
            "LAYER PRISM DISPLAY · skipped {pinched_prisms} pinched (Core scale-aware elevation tolerance) and {invalid_prisms} invalid prisms; gaps were not bridged."
        ));
    }
    if visible_prisms == 0 {
        output.status.push(
            "LAYER PRISM DISPLAY · no non-pinched prism is visible under the current IGE filter.".into(),
        );
    }
    Ok(())
}

fn known_contact_elevations(
    contact: &LayerContact,
    triangle: &GeoTriangle,
    _records: &geo_core::GeoRecords,
) -> Result<([f64; 3], BTreeSet<SourceId>), String> {
    let mut elevations = [0.0; 3];
    let mut sources: BTreeSet<SourceId> = contact.source_ids.iter().cloned().collect();
    for (index, borehole_id) in triangle.vertices.iter().enumerate() {
        let parameter = contact.elevations_abs_m.get(borehole_id).ok_or_else(|| {
            format!(
                "Core LayerVolumes gate was KNOWN but horizon {} lacks borehole {}",
                contact.id.as_str(),
                borehole_id.as_str()
            )
        })?;
        match parameter {
            Parameter::Known {
                value, source_ids, ..
            } => {
                elevations[index] = *value;
                sources.extend(source_ids.iter().cloned());
            }
            Parameter::Missing { .. } => {
                return Err(format!(
                    "Core LayerVolumes gate was KNOWN but horizon {} has a missing value at borehole {}",
                    contact.id.as_str(),
                    borehole_id.as_str()
                ));
            }
            Parameter::Conflicting { .. } => {
                return Err(format!(
                    "Core LayerVolumes gate was KNOWN but horizon {} has a conflicting value at borehole {}",
                    contact.id.as_str(),
                    borehole_id.as_str()
                ));
            }
        }
    }
    Ok((elevations, sources))
}

fn geo_layer_elevation_tolerance_m(top_abs_m: f64, bottom_abs_m: f64) -> f64 {
    GEO_LAYER_THICKNESS_TOLERANCE_M
        + 128.0 * f64::EPSILON * top_abs_m.abs().max(bottom_abs_m.abs())
}

fn triangular_prism_triangles(top: [Vec3; 3], bottom: [Vec3; 3]) -> Vec<Triangle> {
    let mut triangles = Vec::with_capacity(8);
    triangles.push(Triangle::new(0, top[0], top[1], top[2]));
    triangles.push(Triangle::new(1, bottom[0], bottom[2], bottom[1]));
    let mut triangle_id = 2u32;
    for edge in 0..3 {
        let next = (edge + 1) % 3;
        triangles.push(Triangle::new(
            triangle_id,
            top[edge],
            bottom[edge],
            bottom[next],
        ));
        triangle_id += 1;
        triangles.push(Triangle::new(
            triangle_id,
            top[edge],
            bottom[next],
            top[next],
        ));
        triangle_id += 1;
    }
    triangles
}

fn add_contact_scene(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let records = &input.resolved.snapshot.records;
    for contact in &model.contacts {
        if !ige_is_visible(input.options, &contact.ige_id) {
            continue;
        }
        let mut triangles = Vec::new();
        let mut sources: BTreeSet<SourceId> = contact.source_ids.iter().cloned().collect();
        let mut unknown_markers = BTreeSet::new();
        for triangle in &model.triangles {
            let mut vertices = [Vec3::ZERO; 3];
            let mut all_known = true;
            for (index, borehole_id) in triangle.vertices.iter().enumerate() {
                let borehole = required_borehole(records, borehole_id)?;
                match contact.elevations_abs_m.get(borehole_id) {
                    Some(Parameter::Known { value, source_ids, .. }) => {
                        validate_scalar(*value, "contact elevation")?;
                        vertices[index] = Vec3::new(
                            borehole.collar_m[0],
                            borehole.collar_m[1],
                            *value,
                        );
                        sources.extend(source_ids.iter().cloned());
                    }
                    Some(Parameter::Missing { .. }) | None => {
                        all_known = false;
                        if unknown_markers.insert((borehole.id.clone(), ParameterDisplayState::Missing)) {
                            add_contact_unknown_marker(
                                input,
                                model,
                                contact,
                                borehole,
                                ParameterDisplayState::Missing,
                                output,
                                budget,
                            )?;
                        }
                    }
                    Some(Parameter::Conflicting { candidates }) => {
                        all_known = false;
                        for candidate in candidates {
                            sources.extend(candidate.source_ids.iter().cloned());
                        }
                        if unknown_markers.insert((borehole.id.clone(), ParameterDisplayState::Conflicting)) {
                            add_contact_unknown_marker(
                                input,
                                model,
                                contact,
                                borehole,
                                ParameterDisplayState::Conflicting,
                                output,
                                budget,
                            )?;
                        }
                    }
                }
            }
            if all_known {
                let triangle_id = u32::try_from(triangles.len())
                    .map_err(|_| "contact triangle index overflowed".to_string())?;
                let vertices = orient_triangle_z_up(vertices);
                triangles.push(Triangle::new(
                    triangle_id,
                    vertices[0],
                    vertices[1],
                    vertices[2],
                ));
            }
        }
        if triangles.is_empty() {
            output.status.push(format!(
                "CONTACT HOLD · horizon {} / IGE {} has no triangle with three KNOWN source elevations.",
                contact.id.as_str(),
                contact.ige_id.as_str()
            ));
            continue;
        }
        let legend = legend_entry(records, &contact.ige_id)?;
        add_scene_object(
            input,
            output,
            budget,
            GeoPick::Contact {
                model_id: model.id.clone(),
                horizon_id: contact.id.clone(),
                ige_id: contact.ige_id.clone(),
            },
            format!("Source-vertex contact {} · {}", contact.id.as_str(), legend.ige_name),
            triangles,
            legend.color,
            sources,
            None,
            None,
        )?;
    }
    output.status.push(
        "Known contact triangles require all three source elevations; missing/conflicting faces stay absent and typed.".into(),
    );
    Ok(())
}

fn add_contact_unknown_marker(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    contact: &LayerContact,
    borehole: &Borehole,
    state: ParameterDisplayState,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let mut sources: BTreeSet<SourceId> = contact
        .source_ids
        .iter()
        .chain(&borehole.source_ids)
        .cloned()
        .collect();
    if let Some(parameter) = contact.elevations_abs_m.get(&borehole.id) {
        match parameter {
            Parameter::Known { source_ids, .. } => sources.extend(source_ids.iter().cloned()),
            Parameter::Missing { .. } => {}
            Parameter::Conflicting { candidates } => {
                for candidate in candidates {
                    sources.extend(candidate.source_ids.iter().cloned());
                }
            }
        }
    }
    add_marker(
        input,
        output,
        budget,
        GeoPick::ContactUnknown {
            model_id: model.id.clone(),
            horizon_id: contact.id.clone(),
            borehole_id: borehole.id.clone(),
            state,
        },
        format!(
            "Contact {} {} at borehole collar {} (marker is not an elevation)",
            contact.id.as_str(),
            if state == ParameterDisplayState::Missing { "MISSING" } else { "CONFLICTING" },
            borehole.id.as_str()
        ),
        checked_point3(borehole.collar_m, "contact status collar")?,
        BOREHOLE_MARKER_WIDTH_M * 0.55,
        if state == ParameterDisplayState::Missing { COLOR_UNKNOWN } else { COLOR_CONFLICT },
        sources,
        None,
        None,
    )
}

fn add_borehole_scene(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let records = &input.resolved.snapshot.records;
    for borehole_id in &model.borehole_ids {
        let borehole = required_borehole(records, borehole_id)?;
        add_marker(
            input,
            output,
            budget,
            GeoPick::Borehole {
                model_id: model.id.clone(),
                borehole_id: borehole.id.clone(),
            },
            format!("Borehole collar {}", borehole.id.as_str()),
            checked_point3(borehole.collar_m, "borehole collar")?,
            BOREHOLE_MARKER_WIDTH_M,
            COLOR_BOREHOLE,
            borehole_sources(borehole),
            None,
            None,
        )?;
        for interval in &borehole.intervals {
            if !ige_is_visible(input.options, &interval.ige_id) {
                continue;
            }
            validate_scalar(interval.top_depth_m, "interval top depth")?;
            validate_scalar(interval.bottom_depth_m, "interval bottom depth")?;
            if interval.top_depth_m < 0.0 || interval.bottom_depth_m <= interval.top_depth_m {
                return Err(format!("borehole interval {} has invalid depths", interval.id.as_str()));
            }
            let top = borehole.collar_m[2] - interval.top_depth_m;
            let bottom = borehole.collar_m[2] - interval.bottom_depth_m;
            let height = top - bottom;
            validate_positive(height, "borehole interval display extent")?;
            let color = legend_entry(records, &interval.ige_id)?.color;
            let pick = GeoPick::BoreholeInterval {
                model_id: model.id.clone(),
                borehole_id: borehole.id.clone(),
                interval_id: interval.id.clone(),
            };
            let mut object = SceneObject::new(
                stable_object_id(&pick),
                format!(
                    "Borehole {} interval {} · IGE {} · width {:.3} m is display only",
                    borehole.id.as_str(),
                    interval.id.as_str(),
                    interval.ige_id.as_str(),
                    BOREHOLE_INTERVAL_MARKER_WIDTH_M
                ),
                Mesh::cuboid(Vec3::new(
                    BOREHOLE_INTERVAL_MARKER_WIDTH_M,
                    BOREHOLE_INTERVAL_MARKER_WIDTH_M,
                    height,
                )),
            );
            object.transform = Transform3::new(
                Vec3::new(
                    borehole.collar_m[0],
                    borehole.collar_m[1],
                    (top + bottom) * 0.5,
                ),
                viewer_core::Quat::IDENTITY,
                Vec3::new(1.0, 1.0, 1.0),
            );
            object.color = color;
            object.exact_geometry = None;
            let sources = interval
                .source_ids
                .iter()
                .chain(&borehole.source_ids)
                .cloned()
                .collect();
            let object_sources = source_map(
                input,
                output.snapshot_currentness,
                &pick,
                sources,
                None,
                None,
            );
            insert_scene_object(
                input,
                output,
                budget,
                pick,
                object,
                object_sources,
            )?;
        }
    }
    Ok(())
}

fn add_water_scene(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    match &input.resolved.scenario.water {
        WaterScenario::Unknown => output
            .status
            .push("WATER HOLD · scenario is UNKNOWN; no dry condition assumed.".into()),
        WaterScenario::ExplicitDry { source_ids } => output.status.push(format!(
            "WATER · explicit dry assumption from sources [{}]; no water surface created.",
            source_ids.iter().map(SourceId::as_str).collect::<Vec<_>>().join(", ")
        )),
        WaterScenario::Observations { .. } => {
            for observation in water_observations(input)? {
                if let Parameter::Known { value, source_ids, .. } = &observation.elevation_abs_m {
                    let position = checked_point3(
                        [observation.xy_m[0], observation.xy_m[1], *value],
                        "water observation",
                    )?;
                    add_marker(
                        input,
                        output,
                        budget,
                        GeoPick::WaterObservation {
                            model_id: model.id.clone(),
                            scenario_id: input.resolved.scenario.id.clone(),
                            observation_id: observation.id.clone(),
                        },
                        format!("Water observation {} · absolute elevation", observation.id.as_str()),
                        position,
                        BOREHOLE_MARKER_WIDTH_M * 0.8,
                        COLOR_WATER,
                        source_ids.iter().cloned().collect(),
                        None,
                        None,
                    )?;
                } else {
                    output.status.push(format!(
                        "WATER OBSERVATION HOLD · {} elevation missing/conflicting; no 3D point placed.",
                        observation.id.as_str()
                    ));
                }
            }
        }
    }
    Ok(())
}

fn add_foundation_layout_scene(
    input: &GeoFoundationRenderInput<'_>,
    output: &mut GeoFoundationRenderScene,
    bounds: &mut TypedBounds,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let Some(layout) = input.layout else {
        output.status.push("FOUNDATION LAYOUT HOLD · no current typed layout.".into());
        return Ok(());
    };
    let geometry = foundation_core::derive_layout_geometry(&layout.input.layout)
        .map_err(|error| format!("foundation layout geometry failed: {error}"))?;
    for element in &geometry.elements {
        let placement = layout
            .input
            .layout
            .elements
            .iter()
            .find(|placement| ai_revit_core::placement_id(placement) == element.id.as_str())
            .ok_or_else(|| format!("placement {} missing from source layout", element.id.as_str()))?;
        if let foundation_core::FoundationPlacement::Stepped { id, tiers } = placement {
            for tier in &element.tiers {
                let source_tier = tiers
                    .iter()
                    .find(|source| source.id == tier.id)
                    .ok_or_else(|| format!("tier {} missing from source placement", tier.id))?;
                let top = source_tier.center_m[2] + source_tier.thickness_m;
                include_prism_bounds(
                    bounds,
                    tier.corners_xy_m,
                    source_tier.center_m[2],
                    top,
                    "foundation tier",
                )?;
                let pick = GeoPick::FoundationTier {
                    layout_id: layout.input.layout.id.clone(),
                    placement_id: id.as_str().to_owned(),
                    tier_id: tier.id.clone(),
                };
                add_scene_object(
                    input,
                    output,
                    budget,
                    pick,
                    format!("Foundation {} tier {}", id.as_str(), tier.id),
                    prism_triangles(tier.corners_xy_m, source_tier.center_m[2], source_tier.thickness_m)?,
                    COLOR_FOUNDATION,
                    BTreeSet::new(),
                    Some(layout.source_revision.clone()),
                    None,
                )?;
                record_placement_bounds(output, id.as_str(), tier.corners_xy_m, source_tier.center_m[2], top);
            }
        } else {
            let thickness = placement_thickness(placement)?;
            let top = element.base_elevation_m + thickness;
            include_prism_bounds(bounds, element.corners_xy_m, element.base_elevation_m, top, "foundation placement")?;
            add_scene_object(
                input,
                output,
                budget,
                GeoPick::FoundationPlacement {
                    layout_id: layout.input.layout.id.clone(),
                    placement_id: element.id.as_str().to_owned(),
                },
                format!("Foundation placement {}", element.id.as_str()),
                prism_triangles(element.corners_xy_m, element.base_elevation_m, thickness)?,
                COLOR_FOUNDATION,
                BTreeSet::new(),
                Some(layout.source_revision.clone()),
                None,
            )?;
            record_placement_bounds(output, element.id.as_str(), element.corners_xy_m, element.base_elevation_m, top);
        }
    }
    output.status.push(format!(
        "FOUNDATION LAYOUT · {} explicit placements · geometry only; no soil response inferred.",
        geometry.elements.len()
    ));
    Ok(())
}

fn current_foundation_snapshot<'a>(
    input: &GeoFoundationRenderInput<'a>,
    status: &mut Vec<String>,
) -> Result<Option<&'a GeoFoundationAnalysisSnapshot>, String> {
    let Some(reference) = input.foundation_snapshot else {
        status.push("FOUNDATION RESULT HOLD · no source-matched pressure/settlement snapshot.".into());
        return Ok(None);
    };
    if reference.status != EngineeringSnapshotStatus::Current {
        status.push(format!(
            "FOUNDATION RESULT HOLD · Core snapshot {} is {:?}.",
            reference.id.get(),
            reference.status
        ));
        return Ok(None);
    }
    let Some(layout) = input.layout else {
        status.push("FOUNDATION RESULT HOLD · no current FoundationLayout source.".into());
        return Ok(None);
    };
    let snapshot = reference.snapshot;
    if let Err(error) = snapshot.validate() {
        status.push(format!("FOUNDATION RESULT HOLD · validation failed: {error}"));
        return Ok(None);
    }
    let source_status = geo_core::snapshot_currentness(input.state, &snapshot.input.ground)
        .map_err(|error| format!("foundation GEO currentness query failed: {error}"))?;
    if source_status != SnapshotCurrentness::Current {
        status.push("FOUNDATION RESULT HOLD · source GEO closure is stale.".into());
        return Ok(None);
    }
    if !same_geo_closure(&snapshot.input.ground, &input.resolved.snapshot)
        || snapshot.input.scenario != input.resolved.scenario
    {
        status.push("FOUNDATION RESULT HOLD · model/scenario closure differs from displayed GEO.".into());
        return Ok(None);
    }
    let placement = layout
        .input
        .layout
        .elements
        .iter()
        .find(|placement| ai_revit_core::placement_id(placement) == snapshot.input.foundation_id);
    let linked_selection = layout.input.ground_links.get(&snapshot.input.foundation_id);
    let Some(placement) = placement else {
        status.push("FOUNDATION RESULT HOLD · source placement is absent.".into());
        return Ok(None);
    };
    if placement != &snapshot.input.placement
        || linked_selection != Some(&input.resolved.snapshot.selection)
    {
        status.push("FOUNDATION RESULT HOLD · explicit placement or GEO link differs.".into());
        return Ok(None);
    }
    if snapshot.input.effective_input.foundation_id.as_str() != snapshot.input.foundation_id {
        status.push("FOUNDATION RESULT HOLD · solver FoundationId differs from placement ID.".into());
        return Ok(None);
    }
    let foundation_core::FoundationPlacement::Raft {
        length_x_m,
        width_y_m,
        ..
    } = placement
    else {
        status.push("FOUNDATION RESULT HOLD · numerical response is supported for rectangular raft placements only.".into());
        return Ok(None);
    };
    let length_x = *length_x_m;
    let width_y = *width_y_m;
    let thickness = placement_thickness(placement)?;
    let geometry = &snapshot.input.effective_input.geometry;
    if !approximately_equal(geometry.width_x_m, length_x)
        || !approximately_equal(geometry.length_y_m, width_y)
        || !approximately_equal(geometry.thickness_m, thickness)
    {
        status.push("FOUNDATION RESULT HOLD · numerical dimensions differ from typed placement.".into());
        return Ok(None);
    }
    status.push(format!(
        "FOUNDATION RESULT CURRENT · snapshot {} · source-matched pressure/settlement samples; normative checks HOLD.",
        reference.id.get()
    ));
    Ok(Some(snapshot))
}

fn current_foundation_snapshot_for_pick<'a>(
    input: &GeoFoundationRenderInput<'a>,
) -> Result<Option<(&'a GeoFoundationAnalysisSnapshot, &'a foundation_core::FoundationPlacement)>, String> {
    let mut status = Vec::new();
    let Some(snapshot) = current_foundation_snapshot(input, &mut status)? else {
        return Ok(None);
    };
    let Some(layout) = input.layout else {
        return Ok(None);
    };
    let placement = layout
        .input
        .layout
        .elements
        .iter()
        .find(|placement| ai_revit_core::placement_id(placement) == snapshot.input.foundation_id)
        .ok_or_else(|| "validated FoundationPlacement disappeared".to_string())?;
    Ok(Some((snapshot, placement)))
}

fn add_foundation_result_scene(
    input: &GeoFoundationRenderInput<'_>,
    snapshot: &GeoFoundationAnalysisSnapshot,
    output: &mut GeoFoundationRenderScene,
    bounds: &mut TypedBounds,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let layout = input.layout.ok_or_else(|| "matched layout disappeared".to_string())?;
    let placement = layout
        .input
        .layout
        .elements
        .iter()
        .find(|placement| ai_revit_core::placement_id(placement) == snapshot.input.foundation_id)
        .ok_or_else(|| "matched placement disappeared".to_string())?;
    if snapshot.foundation_result.cells.len() > foundation_core::MAX_MESH_CELLS {
        return Err("foundation result exceeds Foundation Core cell limit".into());
    }
    let values: Vec<f64> = snapshot.foundation_result.cells.iter().map(|cell| cell.q_kpa).collect();
    let (min_value, max_value) = finite_range(&values, "foundation pressure")?;
    for (index, cell) in snapshot.foundation_result.cells.iter().enumerate() {
        let corners = cell_world_corners(
            placement,
            &snapshot.input.effective_input,
            cell.x_m,
            cell.y_m,
            snapshot.foundation_result.mesh.dx_m,
            snapshot.foundation_result.mesh.dy_m,
            FOUNDATION_SAMPLE_DISPLAY_OFFSET_M,
        )?;
        for point in corners {
            include_xyz(bounds, [point.x, point.y, point.z], "pressure sample patch")?;
        }
        add_scene_object(
            input,
            output,
            budget,
            GeoPick::FoundationCell {
                foundation_id: snapshot.input.effective_input.foundation_id.clone(),
                cell_index: index,
            },
            format!("Pressure cell {index} · {:.3} kPa", cell.q_kpa),
            quad_triangles(corners, index)?,
            scalar_color(cell.q_kpa, min_value, max_value),
            BTreeSet::new(),
            Some(snapshot.input.layout_source_revision.clone()),
            input.foundation_snapshot.map(|reference| reference.id),
        )?;
    }
    output.status.push(format!(
        "Pressure samples: {} actual solver cells · {:.3}..{:.3} kPa · no continuum field generated.",
        values.len(),
        min_value,
        max_value
    ));
    Ok(())
}

fn draw_plan_tin(
    model: &GroundModel,
    records: &geo_core::GeoRecords,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    for triangle in &model.triangles {
        let points = triangle_plan_points(triangle, records)?;
        let screen = points.map(|point| camera.world_to_screen(Point2::new(point[0], point[1])));
        add_canvas_triangle(layer, screen, COLOR_TERRAIN, budget)?;
        for edge in 0..3 {
            add_canvas_segment(layer, screen[edge], screen[(edge + 1) % 3], COLOR_TERRAIN_EDGE, 1.1, budget)?;
        }
    }
    push_canvas_label(
        layer,
        Point2::new(16.0, 24.0),
        "Explicit TIN topology · outside coverage remains blank",
        COLOR_TEXT,
        ScreenLabelPlacement::Hud,
        budget,
    )
}

fn draw_unknown_zones(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    for (index, zone) in model.unknown_zones.iter().enumerate() {
        if zone.polygon_xy_m.len() < 3 || zone.polygon_xy_m.len() > geo_core::MAX_SECTION_POINTS {
            return Err(format!("unknown zone {index} has invalid polygon size"));
        }
        let pick = GeoPick::UnknownZone {
            model_id: model.id.clone(),
            index,
        };
        draw_polygon(
            layer,
            &zone.polygon_xy_m,
            camera,
            if input.selection == Some(&pick) { COLOR_SELECTED } else { COLOR_UNKNOWN },
            if input.selection == Some(&pick) { 3.0 } else { 2.0 },
            budget,
        )?;
    }
    Ok(())
}

fn draw_contact_markers(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    records: &geo_core::GeoRecords,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    for contact in &model.contacts {
        if !ige_is_visible(input.options, &contact.ige_id) {
            continue;
        }
        for (borehole_id, value) in &contact.elevations_abs_m {
            let borehole = required_borehole(records, borehole_id)?;
            let point = camera.world_to_screen(Point2::new(borehole.collar_m[0], borehole.collar_m[1]));
            let pick = GeoPick::Contact {
                model_id: model.id.clone(),
                horizon_id: contact.id.clone(),
                ige_id: contact.ige_id.clone(),
            };
            let color = if input.selection == Some(&pick) {
                COLOR_SELECTED
            } else {
                match value {
                    Parameter::Known { .. } => ige_color(&contact.ige_id),
                    Parameter::Missing { .. } => COLOR_UNKNOWN,
                    Parameter::Conflicting { .. } => COLOR_CONFLICT,
                }
            };
            draw_cross(layer, point, 4.0, color, budget)?;
        }
    }
    Ok(())
}

fn draw_borehole_markers(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    records: &geo_core::GeoRecords,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    for id in &model.borehole_ids {
        let borehole = required_borehole(records, id)?;
        let point = camera.world_to_screen(Point2::new(borehole.collar_m[0], borehole.collar_m[1]));
        let pick = GeoPick::Borehole {
            model_id: model.id.clone(),
            borehole_id: id.clone(),
        };
        draw_cross(
            layer,
            point,
            if input.selection == Some(&pick) { 7.0 } else { 4.0 },
            if input.selection == Some(&pick) { COLOR_SELECTED } else { COLOR_BOREHOLE },
            budget,
        )?;
    }
    Ok(())
}

fn draw_water_markers(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    for observation in water_observations(input)? {
        let point = camera.world_to_screen(Point2::new(observation.xy_m[0], observation.xy_m[1]));
        let (color, text) = match &observation.elevation_abs_m {
            Parameter::Known { value, .. } => (COLOR_WATER, format!("WATER z={value:.3} m")),
            Parameter::Missing { .. } => (COLOR_UNKNOWN, "WATER elevation MISSING".into()),
            Parameter::Conflicting { .. } => (COLOR_CONFLICT, "WATER elevation CONFLICT".into()),
        };
        draw_cross(layer, point, 5.0, color, budget)?;
        push_canvas_label(layer, Point2::new(point.x + 6.0, point.y - 6.0), &text, color, ScreenLabelPlacement::Callout, budget)?;
    }
    match &input.resolved.scenario.water {
        WaterScenario::Unknown => push_canvas_label(layer, Point2::new(16.0, 46.0), "WATER HOLD · UNKNOWN; no dry condition assumed", COLOR_UNKNOWN, ScreenLabelPlacement::Hud, budget)?,
        WaterScenario::ExplicitDry { .. } => push_canvas_label(layer, Point2::new(16.0, 46.0), "WATER · explicit dry scenario assumption", COLOR_MUTED, ScreenLabelPlacement::Hud, budget)?,
        WaterScenario::Observations { .. } => {}
    }
    let _ = model;
    Ok(())
}

fn validate_foundation_ground_map<'a>(
    input: &GeoFoundationRenderInput<'a>,
) -> Result<ValidatedFoundationGroundMap<'a>, String> {
    let map = input
        .ground_map
        .ok_or_else(|| "no cached Foundation ground map was supplied".to_string())?;
    let source_currentness = geo_core::snapshot_currentness(input.state, &map.ground)
        .map_err(|error| format!("cached GEO currentness check failed: {error}"))?;
    if source_currentness != SnapshotCurrentness::Current {
        return Err("cached map GEO snapshot is stale".into());
    }
    if !same_geo_closure(&map.ground, &input.resolved.snapshot) {
        return Err("cached map and displayed GEO source closures differ".into());
    }
    let map_scenario = map
        .ground
        .records
        .scenarios
        .get(&input.resolved.scenario.id)
        .ok_or_else(|| "cached map closure lacks the displayed groundwater scenario".to_string())?;
    if map_scenario != &input.resolved.scenario || map.water_scenario != map_scenario.water {
        return Err("cached map groundwater scenario differs from the displayed GEO scenario".into());
    }

    let layout = input
        .layout
        .ok_or_else(|| "no current FoundationLayout projection is available".to_string())?;
    let source_placement = layout
        .input
        .layout
        .elements
        .iter()
        .find(|placement| ai_revit_core::placement_id(placement) == map.foundation_id.as_str())
        .ok_or_else(|| "cached map placement is absent from the current layout".to_string())?;
    if source_placement != &map.placement {
        return Err("cached map placement differs from the current layout placement".into());
    }
    if layout.input.ground_links.get(&map.foundation_id) != Some(&map.ground.selection) {
        return Err("cached map GEO selection differs from the placement's current link".into());
    }
    let foundation_core::FoundationPlacement::Raft {
        center_m,
        length_x_m,
        width_y_m,
        ..
    } = &map.placement
    else {
        return Err("Foundation ground map display supports rectangular raft placements only".into());
    };
    validate_xy([center_m[0], center_m[1]], "raft ground map center")?;
    validate_scalar(center_m[2], "raft ground map sole elevation")?;
    validate_positive(*length_x_m, "raft ground map length")?;
    validate_positive(*width_y_m, "raft ground map width")?;
    validate_xy(
        [center_m[0] - *length_x_m * 0.5, center_m[1] - *width_y_m * 0.5],
        "raft ground map minimum footprint",
    )?;
    validate_xy(
        [center_m[0] + *length_x_m * 0.5, center_m[1] + *width_y_m * 0.5],
        "raft ground map maximum footprint",
    )?;

    let cell_count = map
        .nx
        .checked_mul(map.ny)
        .filter(|count| *count > 0 && *count <= ai_revit_core::MAX_GEO_FOUNDATION_MAP_CELLS)
        .ok_or_else(|| "cached map grid exceeds its 1..4096 cell limit".to_string())?;
    if map.cells.len() != cell_count || map.cells.len() > ai_revit_core::MAX_GEO_FOUNDATION_MAP_CELLS {
        return Err("cached map cell count does not match the bounded grid dimensions".into());
    }
    if map.sampling_policy.is_empty()
        || map.sampling_policy.len() > 512
        || map.numerical_meaning.is_empty()
        || map.numerical_meaning.len() > 512
    {
        return Err("cached map sampling policy or numerical meaning is absent/oversized".into());
    }
    validate_area_value(map.unknown_or_interface_sample_area_m2, "unknown/interface sample area")?;
    if map.unknown_or_interface_sample_area_m2 < 0.0 {
        return Err("unknown/interface sample area is negative".into());
    }

    let dx_m = *length_x_m / map.nx as f64;
    let dy_m = *width_y_m / map.ny as f64;
    validate_positive(dx_m, "ground map cell width")?;
    validate_positive(dy_m, "ground map cell height")?;
    let cell_area_m2 = dx_m * dy_m;
    validate_positive_area(cell_area_m2, "ground map cell area")?;

    let mut legend_ids = BTreeSet::new();
    let mut classified_area_by_ige_m2 = BTreeMap::<IgeId, f64>::new();
    let mut known_cell_count = 0usize;
    let mut unresolved_cell_count = 0usize;
    let mut recomputed_unresolved_area_m2 = 0.0;
    for (expected_index, cell) in map.cells.iter().enumerate() {
        if cell.index != expected_index || cell.index >= cell_count {
            return Err("cached map has a missing, duplicate, or out-of-bounds cell index".into());
        }
        validate_xy(cell.xy_m, "ground map sample coordinate")?;
        validate_scalar(cell.sole_z_m, "ground map sample sole elevation")?;
        validate_positive_area(cell.area_m2, "ground map sample area")?;
        if !approximately_equal_area(cell.area_m2, cell_area_m2)
            || !approximately_equal(cell.sole_z_m, center_m[2])
        {
            return Err(format!("cached ground map cell {} disagrees with its typed raft grid", cell.index));
        }
        if let Some(value) = cell.terrain_z_m {
            validate_scalar(value, "ground map terrain elevation")?;
        }
        if let Some(value) = cell.embedment_depth_m {
            validate_scalar(value, "ground map embedment depth")?;
        }
        let column = expected_index % map.nx;
        let row = expected_index / map.nx;
        let expected_xy = [
            center_m[0] - *length_x_m * 0.5 + (column as f64 + 0.5) * dx_m,
            center_m[1] - *width_y_m * 0.5 + (row as f64 + 0.5) * dy_m,
        ];
        if !approximately_equal(cell.xy_m[0], expected_xy[0])
            || !approximately_equal(cell.xy_m[1], expected_xy[1])
        {
            return Err(format!("cached ground map cell {} is outside its typed midpoint grid", cell.index));
        }
        if let Some(ige_id) = known_base_ige(cell) {
            legend_ids.insert(ige_id.clone());
            known_cell_count = known_cell_count.saturating_add(1);
            *classified_area_by_ige_m2.entry(ige_id.clone()).or_insert(0.0) += cell.area_m2;
        } else {
            unresolved_cell_count = unresolved_cell_count.saturating_add(1);
            recomputed_unresolved_area_m2 += cell.area_m2;
        }
    }

    for (ige_id, area_m2) in &map.midpoint_area_estimate_by_ige_m2 {
        validate_area_value(*area_m2, "midpoint IGE quadrature area")?;
        if *area_m2 < 0.0 || !legend_ids.contains(ige_id) {
            return Err("cached midpoint area estimate has an invalid or unsampled IGE".into());
        }
    }
    if map.midpoint_area_estimate_by_ige_m2.len() != legend_ids.len() {
        return Err("cached midpoint area estimate and known sample IGE sets differ".into());
    }
    validate_area_value(recomputed_unresolved_area_m2, "recomputed unknown/interface sample area")?;
    if !approximately_equal_area(
        map.unknown_or_interface_sample_area_m2,
        recomputed_unresolved_area_m2,
    ) {
        return Err("cached unknown/interface sample area disagrees with classified cells".into());
    }

    let mut recomputed_total_area_m2 = recomputed_unresolved_area_m2;
    let mut supplied_total_area_m2 = map.unknown_or_interface_sample_area_m2;
    for ige_id in &legend_ids {
        let recomputed_area_m2 = *classified_area_by_ige_m2
            .get(ige_id)
            .ok_or_else(|| "classified sample area lacks a known IGE".to_string())?;
        let supplied_area_m2 = *map
            .midpoint_area_estimate_by_ige_m2
            .get(ige_id)
            .ok_or_else(|| "cached midpoint area estimate lacks a sampled IGE".to_string())?;
        validate_area_value(recomputed_area_m2, "recomputed midpoint IGE area")?;
        if !approximately_equal_area(supplied_area_m2, recomputed_area_m2) {
            return Err(format!(
                "cached midpoint area estimate for IGE {} disagrees with classified cells",
                ige_id.as_str()
            ));
        }
        recomputed_total_area_m2 += recomputed_area_m2;
        supplied_total_area_m2 += supplied_area_m2;
    }
    validate_area_value(recomputed_total_area_m2, "recomputed ground map sample total")?;
    validate_area_value(supplied_total_area_m2, "supplied ground map sample total")?;
    let footprint_area_m2 = *length_x_m * *width_y_m;
    validate_positive_area(footprint_area_m2, "raft ground map footprint area")?;
    if !approximately_equal_area(recomputed_total_area_m2, footprint_area_m2)
        || !approximately_equal_area(supplied_total_area_m2, footprint_area_m2)
    {
        return Err("ground map area totals do not equal the typed raft footprint".into());
    }
    let legend_entries = legend_ids
        .iter()
        .map(|ige_id| legend_entry(&map.ground.records, ige_id))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ValidatedFoundationGroundMap {
        map,
        center_m: *center_m,
        length_x_m: *length_x_m,
        width_y_m: *width_y_m,
        dx_m,
        dy_m,
        legend_entries,
        known_cell_count,
        unresolved_cell_count,
    })
}

fn known_base_ige(cell: &ai_revit_core::GeoFoundationMapCell) -> Option<&IgeId> {
    (cell.status == QueryStatus::Known && cell.base_ige_ids.len() == 1)
        .then(|| &cell.base_ige_ids[0])
}

fn foundation_ground_map_cell_corners(
    ground_map: &ValidatedFoundationGroundMap<'_>,
    cell: &ai_revit_core::GeoFoundationMapCell,
) -> [[f64; 2]; 4] {
    let column = cell.index % ground_map.map.nx;
    let row = cell.index / ground_map.map.nx;
    let min_x = ground_map.center_m[0] - ground_map.length_x_m * 0.5
        + column as f64 * ground_map.dx_m;
    let min_y = ground_map.center_m[1] - ground_map.width_y_m * 0.5
        + row as f64 * ground_map.dy_m;
    let max_x = min_x + ground_map.dx_m;
    let max_y = min_y + ground_map.dy_m;
    [[min_x, min_y], [max_x, min_y], [max_x, max_y], [min_x, max_y]]
}

fn draw_foundation_ground_map(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let ground_map = match validate_foundation_ground_map(input) {
        Ok(ground_map) => ground_map,
        Err(reason) => {
            push_canvas_label(
                layer,
                Point2::new(16.0, 116.0),
                &format!("FOUNDATION GEO MAP HOLD · {reason}"),
                COLOR_UNKNOWN,
                ScreenLabelPlacement::Hud,
                budget,
            )?;
            return Ok(());
        }
    };

    for cell in &ground_map.map.cells {
        let base_ige = known_base_ige(cell);
        if base_ige.is_some_and(|ige_id| !ige_is_visible(input.options, ige_id)) {
            continue;
        }
        let corners = foundation_ground_map_cell_corners(&ground_map, cell);
        let screen = corners.map(|point| camera.world_to_screen(Point2::new(point[0], point[1])));
        if let Some(ige_id) = base_ige {
            let color = with_alpha(ige_color(ige_id), 0.42);
            add_canvas_triangle(layer, [screen[0], screen[1], screen[2]], color, budget)?;
            add_canvas_triangle(layer, [screen[0], screen[2], screen[3]], color, budget)?;
        } else {
            let hatch = with_alpha(COLOR_MUTED, 0.82);
            add_canvas_segment(layer, screen[0], screen[2], hatch, 1.0, budget)?;
            add_canvas_segment(layer, screen[1], screen[3], hatch, 1.0, budget)?;
        }
        let selected = input.selection.is_some_and(|pick| {
            matches!(pick, GeoPick::FoundationGroundSample { foundation_id, index }
                if foundation_id == &ground_map.map.foundation_id && *index == cell.index)
        });
        if selected {
            draw_polygon(layer, &corners, camera, COLOR_SELECTED, 2.2, budget)?;
        }
    }

    let min_x = ground_map.center_m[0] - ground_map.length_x_m * 0.5;
    let max_x = ground_map.center_m[0] + ground_map.length_x_m * 0.5;
    let min_y = ground_map.center_m[1] - ground_map.width_y_m * 0.5;
    let max_y = ground_map.center_m[1] + ground_map.width_y_m * 0.5;
    let grid_color = with_alpha(COLOR_MUTED, 0.62);
    for column in 0..=ground_map.map.nx {
        let x = min_x + column as f64 * ground_map.dx_m;
        add_canvas_segment(
            layer,
            camera.world_to_screen(Point2::new(x, min_y)),
            camera.world_to_screen(Point2::new(x, max_y)),
            grid_color,
            0.65,
            budget,
        )?;
    }
    for row in 0..=ground_map.map.ny {
        let y = min_y + row as f64 * ground_map.dy_m;
        add_canvas_segment(
            layer,
            camera.world_to_screen(Point2::new(min_x, y)),
            camera.world_to_screen(Point2::new(max_x, y)),
            grid_color,
            0.65,
            budget,
        )?;
    }

    let known_area_m2 = ground_map
        .map
        .midpoint_area_estimate_by_ige_m2
        .iter()
        .filter(|(ige_id, _)| ige_is_visible(input.options, ige_id))
        .map(|(_, area)| *area)
        .sum::<f64>();
    validate_area_value(known_area_m2, "visible midpoint quadrature area")?;
    push_canvas_label(
        layer,
        Point2::new(16.0, 46.0),
        "Foundation base map · color = one KNOWN base IGE (see IGE legend); grey hatch = unknown/conflicting/outside/missing/interface (no tie-break); select a cell for its reasons",
        COLOR_TEXT,
        ScreenLabelPlacement::Hud,
        budget,
    )?;
    push_canvas_label(
        layer,
        Point2::new(16.0, 64.0),
        &format!("Sampling policy: {}", ground_map.map.sampling_policy),
        COLOR_MUTED,
        ScreenLabelPlacement::Hud,
        budget,
    )?;
    push_canvas_label(
        layer,
        Point2::new(16.0, 82.0),
        &format!("Numerical meaning: {}", ground_map.map.numerical_meaning),
        COLOR_MUTED,
        ScreenLabelPlacement::Hud,
        budget,
    )?;
    push_canvas_label(
        layer,
        Point2::new(16.0, 100.0),
        &format!(
            "MIDPOINT quadrature area estimate: {:.3} m² visible known base IGE · {:.3} m² unresolved/interface samples; no continuous coverage, settlement, bearing or k inferred.",
            known_area_m2,
            ground_map.map.unknown_or_interface_sample_area_m2
        ),
        COLOR_TEXT,
        ScreenLabelPlacement::Hud,
        budget,
    )?;
    Ok(())
}

fn draw_foundation_layout(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let Some(layout) = input.layout else {
        push_canvas_label(layer, Point2::new(16.0, 68.0), "FOUNDATION LAYOUT HOLD · no typed layout", COLOR_UNKNOWN, ScreenLabelPlacement::Hud, budget)?;
        return Ok(());
    };
    let geometry = foundation_core::derive_layout_geometry(&layout.input.layout)
        .map_err(|error| format!("foundation layout geometry failed: {error}"))?;
    for element in geometry.elements {
        let pick = GeoPick::FoundationPlacement {
            layout_id: layout.input.layout.id.clone(),
            placement_id: element.id.as_str().to_owned(),
        };
        draw_polygon(
            layer,
            &element.corners_xy_m,
            camera,
            if input.selection == Some(&pick) { COLOR_SELECTED } else { COLOR_FOUNDATION },
            if input.selection == Some(&pick) { 3.0 } else { 2.2 },
            budget,
        )?;
    }
    Ok(())
}

fn draw_foundation_samples(
    input: &GeoFoundationRenderInput<'_>,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let mut status = Vec::new();
    let Some(snapshot) = current_foundation_snapshot(input, &mut status)? else {
        push_canvas_label(layer, Point2::new(16.0, 68.0), "FOUNDATION RESULT HOLD · no current source-matched result", COLOR_UNKNOWN, ScreenLabelPlacement::Hud, budget)?;
        return Ok(());
    };
    let layout = input.layout.ok_or_else(|| "validated foundation layout disappeared".to_string())?;
    let placement = layout.input.layout.elements.iter()
        .find(|placement| ai_revit_core::placement_id(placement) == snapshot.input.foundation_id)
        .ok_or_else(|| "validated foundation placement disappeared".to_string())?;
    let pressure_view = input.options.view == GeoViewMode::Pressure2D;
    let values: Vec<f64> = snapshot.foundation_result.cells.iter()
        .map(|cell| if pressure_view { cell.q_kpa } else { cell.plane_settlement_m })
        .collect();
    let (min_value, max_value) = finite_range(&values, "foundation result")?;
    for (cell, value) in snapshot.foundation_result.cells.iter().zip(&values) {
        let corners = foundation_cell_xy(
            placement,
            &snapshot.input.effective_input,
            cell.x_m,
            cell.y_m,
            snapshot.foundation_result.mesh.dx_m,
            snapshot.foundation_result.mesh.dy_m,
        )?;
        let screen = corners.map(|point| camera.world_to_screen(Point2::new(point[0], point[1])));
        let color = scalar_color(*value, min_value, max_value);
        add_canvas_triangle(layer, [screen[0], screen[1], screen[2]], color, budget)?;
        add_canvas_triangle(layer, [screen[0], screen[2], screen[3]], color, budget)?;
    }
    let unit = if pressure_view { "kPa" } else { "m down" };
    push_canvas_label(
        layer,
        Point2::new(16.0, 46.0),
        &format!(
            "Source-matched {} samples · {:.4}..{:.4} {unit} · cell values only",
            if pressure_view { "pressure" } else { "settlement plane" },
            min_value,
            max_value
        ),
        COLOR_TEXT,
        ScreenLabelPlacement::Hud,
        budget,
    )?;
    if snapshot.engineering_approval != EngineeringApprovalStatus::NotImplemented {
        return Err("unsupported GEO/foundation approval status".into());
    }
    push_canvas_label(
        layer,
        Point2::new(16.0, camera.viewport_height - 18.0),
        "NUMERICAL DISPLAY · bearing, ULS/SLS and engineering approval HOLD",
        COLOR_MUTED,
        ScreenLabelPlacement::Footer,
        budget,
    )?;
    Ok(())
}

fn draw_section_trace(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    section_id: &GeoSectionId,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    let section = input.resolved.snapshot.records.sections.get(section_id)
        .ok_or_else(|| format!("section {} missing from selected snapshot", section_id.as_str()))?;
    if section.model_id != model.id {
        return Err("selected section references another GroundModel".into());
    }
    for pair in section.polyline_xy_m.windows(2) {
        add_canvas_segment(
            layer,
            camera.world_to_screen(Point2::new(pair[0][0], pair[0][1])),
            camera.world_to_screen(Point2::new(pair[1][0], pair[1][1])),
            COLOR_SELECTED,
            2.0,
            budget,
        )?;
    }
    Ok(())
}

fn draw_frame_labels(
    input: &GeoFoundationRenderInput<'_>,
    model: &GroundModel,
    camera: &Camera2D,
    layer: &mut AnalysisCanvasLayer,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    push_canvas_label(
        layer,
        Point2::new(16.0, camera.viewport_height - 38.0),
        &format!("World XY (m) · {} · CRS {}", model.frame.name, optional_text(model.frame.crs.as_deref(), "unspecified")),
        COLOR_TEXT,
        ScreenLabelPlacement::Footer,
        budget,
    )?;
    push_canvas_label(
        layer,
        Point2::new(16.0, camera.viewport_height - 19.0),
        &format!("Vertical datum {} · absolute Z (m) · {:?}", optional_text(model.frame.vertical_datum.as_deref(), "unspecified"), geo_core::snapshot_currentness(input.state, &input.resolved.snapshot).map_err(|error| error.to_string())?),
        COLOR_MUTED,
        ScreenLabelPlacement::Footer,
        budget,
    )
}

fn query_section(
    input: &GeoFoundationRenderInput<'_>,
    section_id: &GeoSectionId,
) -> Result<(GeoQueryResult, GeoBounds2D), String> {
    let model = input.resolved.snapshot.records.models.get(&input.resolved.snapshot.selection.model_id)
        .ok_or_else(|| "selected GroundModel is missing".to_string())?;
    let section = input.resolved.snapshot.records.sections.get(section_id)
        .ok_or_else(|| format!("section {} is missing", section_id.as_str()))?;
    if section.model_id != model.id {
        return Err("section references a different GroundModel".into());
    }
    let result = geo_core::query_ground(
        input.resolved,
        &GeoQuery::Section {
            polyline_xy_m: section.polyline_xy_m.clone(),
            max_spacing_m: input.options.section_max_spacing_m,
        },
    )
    .map_err(|error| format!("authoritative section query failed: {error}"))?;
    let section_result = result.section.as_ref()
        .ok_or_else(|| "authoritative section query returned no typed samples".to_string())?;
    let mut bounds = GeoBounds2D::empty();
    for sample in &section_result.samples {
        if let Some(column) = known_column(&sample.result) {
            bounds.include([sample.station_m, column.terrain_abs_m]);
            for layer in &column.layers {
                bounds.include([sample.station_m, layer.top_abs_m]);
                bounds.include([sample.station_m, layer.bottom_abs_m]);
            }
        }
    }
    Ok((result, bounds))
}

fn known_column(result: &GeoQueryResult) -> Option<&geo_core::GroundColumn> {
    if result.status == QueryStatus::Known {
        result.column.as_ref()
    } else {
        None
    }
}

fn build_legend(
    records: &geo_core::GeoRecords,
    model: &GroundModel,
    options: &GeoRenderOptions,
) -> Result<Vec<GeoLegendEntry>, String> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    if options.show_contacts {
        for contact in &model.contacts {
            if ige_is_visible(options, &contact.ige_id) && seen.insert(contact.ige_id.clone()) {
                entries.push(legend_entry(records, &contact.ige_id)?);
            }
        }
    }
    if options.show_boreholes {
        for borehole_id in &model.borehole_ids {
            for interval in &required_borehole(records, borehole_id)?.intervals {
                if ige_is_visible(options, &interval.ige_id) && seen.insert(interval.ige_id.clone()) {
                    entries.push(legend_entry(records, &interval.ige_id)?);
                }
            }
        }
    }
    Ok(entries)
}

fn ige_is_visible(options: &GeoRenderOptions, id: &IgeId) -> bool {
    options.visible_ige_ids.is_empty() || options.visible_ige_ids.contains(id)
}

fn legend_entry(records: &geo_core::GeoRecords, id: &IgeId) -> Result<GeoLegendEntry, String> {
    let ige = records.iges.get(id)
        .ok_or_else(|| format!("snapshot closure has no IGE {}", id.as_str()))?;
    let soil = records.soils.get(&ige.soil_id)
        .ok_or_else(|| format!("snapshot closure has no GeoSoil {}", ige.soil_id.as_str()))?;
    Ok(GeoLegendEntry {
        ige_id: ige.id.clone(),
        ige_name: ige.name.clone(),
        soil_id: soil.id.clone(),
        soil_name: soil.name.clone(),
        color: ige_color(id),
    })
}

fn water_observations<'a>(
    input: &'a GeoFoundationRenderInput<'a>,
) -> Result<Vec<&'a WaterObservation>, String> {
    let WaterScenario::Observations { observation_ids } = &input.resolved.scenario.water else {
        return Ok(Vec::new());
    };
    if observation_ids.len() > geo_core::MAX_RECORDS {
        return Err("water observation list exceeds renderer limit".into());
    }
    observation_ids.iter().map(|id| {
        let observation = input.resolved.snapshot.records.water_observations.get(id)
            .ok_or_else(|| format!("snapshot closure lacks water observation {}", id.as_str()))?;
        validate_xy(observation.xy_m, "water observation")?;
        Ok(observation)
    }).collect()
}

fn required_borehole<'a>(
    records: &'a geo_core::GeoRecords,
    id: &BoreholeId,
) -> Result<&'a Borehole, String> {
    let borehole = records.boreholes.get(id)
        .ok_or_else(|| format!("snapshot closure lacks borehole {}", id.as_str()))?;
    checked_point3(borehole.collar_m, "borehole collar")?;
    Ok(borehole)
}

fn triangle_world_points(triangle: &GeoTriangle, records: &geo_core::GeoRecords) -> Result<[Vec3; 3], String> {
    let mut points = [Vec3::ZERO; 3];
    for (index, id) in triangle.vertices.iter().enumerate() {
        points[index] = checked_point3(required_borehole(records, id)?.collar_m, "TIN collar vertex")?;
    }
    Ok(points)
}

fn orient_triangle_z_up(points: [Vec3; 3]) -> [Vec3; 3] {
    let signed_xy_area = (points[1].x - points[0].x) * (points[2].y - points[0].y)
        - (points[1].y - points[0].y) * (points[2].x - points[0].x);
    if signed_xy_area < 0.0 {
        [points[0], points[2], points[1]]
    } else {
        points
    }
}

fn triangle_plan_points(triangle: &GeoTriangle, records: &geo_core::GeoRecords) -> Result<[[f64; 2]; 3], String> {
    let mut points = [[0.0; 2]; 3];
    for (index, id) in triangle.vertices.iter().enumerate() {
        let borehole = required_borehole(records, id)?;
        points[index] = [borehole.collar_m[0], borehole.collar_m[1]];
    }
    Ok(points)
}

fn triangle_sources(
    triangle: &GeoTriangle,
    model: &GroundModel,
    records: &geo_core::GeoRecords,
) -> Result<BTreeSet<SourceId>, String> {
    let mut sources: BTreeSet<SourceId> = triangle.source_ids.iter().chain(&model.source_ids).cloned().collect();
    for id in &triangle.vertices {
        let borehole = required_borehole(records, id)?;
        sources.extend(borehole.source_ids.iter().cloned());
        sources.extend(borehole.survey_source_ids.iter().cloned());
        sources.extend(borehole.evidence_source_ids.iter().cloned());
    }
    Ok(sources)
}

fn borehole_sources(borehole: &Borehole) -> BTreeSet<SourceId> {
    borehole.source_ids.iter().chain(&borehole.survey_source_ids).chain(&borehole.evidence_source_ids).cloned().collect()
}

fn source_map(
    input: &GeoFoundationRenderInput<'_>,
    snapshot_currentness: SnapshotCurrentness,
    pick: &GeoPick,
    source_ids: BTreeSet<SourceId>,
    layout_source: Option<GeoFoundationSourceRevision>,
    foundation_snapshot_id: Option<EngineeringSnapshotId>,
) -> GeoObjectSources {
    let geo_snapshot_is_current = snapshot_currentness == SnapshotCurrentness::Current;
    let geo_snapshot = match pick {
        GeoPick::FoundationPlacement { layout_id, placement_id }
        | GeoPick::FoundationTier { layout_id, placement_id, .. } => {
            input.layout.and_then(|layout| {
                if !geo_snapshot_is_current
                    || layout.input.layout.id.as_str() != layout_id.as_str()
                {
                    return None;
                }
                let linked_selection = layout.input.ground_links.get(placement_id)?;
                (linked_selection == &input.resolved.snapshot.selection)
                    .then_some(&input.resolved.snapshot)
            })
        }
        GeoPick::FoundationCell { .. } => foundation_snapshot_id
            .and_then(|id| input.foundation_snapshot.filter(|reference| reference.id == id))
            .map(|reference| &reference.snapshot.input.ground),
        _ => Some(&input.resolved.snapshot),
    };
    GeoObjectSources {
        source_ids,
        geo_snapshot_id: geo_snapshot.map(|snapshot| snapshot.id.clone()),
        geo_source_revision: geo_snapshot.map(|snapshot| snapshot.source_revision.clone()),
        foundation_layout_source: layout_source,
        foundation_analysis_snapshot_id: foundation_snapshot_id,
    }
}

fn add_marker(
    input: &GeoFoundationRenderInput<'_>,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
    pick: GeoPick,
    name: String,
    center: Vec3,
    size: f64,
    color: [f32; 4],
    sources: BTreeSet<SourceId>,
    layout_source: Option<GeoFoundationSourceRevision>,
    foundation_snapshot_id: Option<EngineeringSnapshotId>,
) -> Result<(), String> {
    validate_positive(size, "display marker size")?;
    let mut object = SceneObject::new(stable_object_id(&pick), name, Mesh::cuboid(Vec3::new(size, size, size)));
    object.transform = Transform3::new(center, viewer_core::Quat::IDENTITY, Vec3::new(1.0, 1.0, 1.0));
    object.color = color;
    object.exact_geometry = None;
    let object_sources = source_map(
        input,
        output.snapshot_currentness,
        &pick,
        sources,
        layout_source,
        foundation_snapshot_id,
    );
    insert_scene_object(input, output, budget, pick, object, object_sources)
}

fn add_scene_object(
    input: &GeoFoundationRenderInput<'_>,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
    pick: GeoPick,
    name: String,
    triangles: Vec<Triangle>,
    color: [f32; 4],
    source_ids: BTreeSet<SourceId>,
    layout_source: Option<GeoFoundationSourceRevision>,
    foundation_snapshot_id: Option<EngineeringSnapshotId>,
) -> Result<(), String> {
    if triangles.is_empty() {
        return Ok(());
    }
    for triangle in &triangles {
        for point in triangle.vertices {
            validate_point(point, "render mesh vertex")?;
        }
    }
    let mut object = SceneObject::new(stable_object_id(&pick), name, Mesh::new(triangles));
    object.color = color;
    object.exact_geometry = None;
    let object_sources = source_map(
        input,
        output.snapshot_currentness,
        &pick,
        source_ids,
        layout_source,
        foundation_snapshot_id,
    );
    insert_scene_object(
        input,
        output,
        budget,
        pick,
        object,
        object_sources,
    )
}

fn insert_scene_object(
    _input: &GeoFoundationRenderInput<'_>,
    output: &mut GeoFoundationRenderScene,
    budget: &mut RenderBudget,
    pick: GeoPick,
    object: SceneObject,
    sources: GeoObjectSources,
) -> Result<(), String> {
    if budget.objects >= MAX_RENDER_OBJECTS {
        return Err(format!("GEO render object budget {MAX_RENDER_OBJECTS} exceeded; nothing was truncated"));
    }
    let object_id = object.object_id;
    if let Some(previous) = output.picks.get(&object_id) {
        return Err(format!("stable GEO ObjectId collision between {previous:?} and {pick:?}"));
    }
    budget.triangles = budget.triangles.checked_add(object.mesh.triangles().len())
        .ok_or_else(|| "GEO render triangle counter overflowed".to_string())?;
    if budget.triangles > MAX_RENDER_TRIANGLES {
        return Err(format!("GEO render triangle budget {MAX_RENDER_TRIANGLES} exceeded; nothing was truncated"));
    }
    if output.scene.insert(object).is_some() {
        return Err(format!("GEO Scene already contains ObjectId {object_id}"));
    }
    output.picks.insert(object_id, pick);
    output.object_sources.insert(object_id, sources);
    budget.objects += 1;
    Ok(())
}

fn stable_object_id(pick: &GeoPick) -> ObjectId {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in format!("{pick:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    (GEO_OBJECT_NAMESPACE << 56) | (hash & OBJECT_PAYLOAD_MASK).max(1)
}

fn foundation_cell_xy(
    placement: &foundation_core::FoundationPlacement,
    input: &foundation_core::RigidMatInput,
    cell_x: f64,
    cell_y: f64,
    dx: f64,
    dy: f64,
) -> Result<[[f64; 2]; 4], String> {
    for (value, name) in [(cell_x, "cell x"), (cell_y, "cell y"), (dx, "cell dx"), (dy, "cell dy")] {
        validate_scalar(value, name)?;
    }
    if dx <= 0.0 || dy <= 0.0 {
        return Err("foundation sample spacing must be positive".into());
    }
    let (center, rotation, length_x, width_y, _) = placement_dimensions(placement)?;
    if !approximately_equal(input.geometry.width_x_m, length_x)
        || !approximately_equal(input.geometry.length_y_m, width_y)
    {
        return Err("Foundation Core dimensions do not match source placement".into());
    }
    let local = [
        [cell_x - dx * 0.5, cell_y - dy * 0.5],
        [cell_x + dx * 0.5, cell_y - dy * 0.5],
        [cell_x + dx * 0.5, cell_y + dy * 0.5],
        [cell_x - dx * 0.5, cell_y + dy * 0.5],
    ];
    Ok(local.map(|point| transform_foundation_xy(center, rotation, point)))
}

fn cell_world_corners(
    placement: &foundation_core::FoundationPlacement,
    input: &foundation_core::RigidMatInput,
    cell_x: f64,
    cell_y: f64,
    dx: f64,
    dy: f64,
    offset_z: f64,
) -> Result<[Vec3; 4], String> {
    let corners = foundation_cell_xy(placement, input, cell_x, cell_y, dx, dy)?;
    let (_, _, _, _, base_z) = placement_dimensions(placement)?;
    corners.map(|point| checked_point3([point[0], point[1], base_z + offset_z], "pressure patch")).into_iter().collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "foundation pressure patch corner count mismatch".into())
}

fn placement_dimensions(
    placement: &foundation_core::FoundationPlacement,
) -> Result<([f64; 2], f64, f64, f64, f64), String> {
    let (center, rotation, length_x, width_y, thickness) = match placement {
        foundation_core::FoundationPlacement::Raft { center_m, length_x_m, width_y_m, thickness_m, .. }
        | foundation_core::FoundationPlacement::Isolated { center_m, length_x_m, width_y_m, thickness_m, .. } =>
            (*center_m, 0.0, *length_x_m, *width_y_m, *thickness_m),
        foundation_core::FoundationPlacement::Strip { center_m, length_m, width_m, rotation_rad, thickness_m, .. } =>
            (*center_m, *rotation_rad, *length_m, *width_m, *thickness_m),
        foundation_core::FoundationPlacement::Stepped { tiers, .. } => {
            let base = tiers.first().ok_or_else(|| "stepped foundation has no base tier".to_string())?;
            (base.center_m, 0.0, base.length_x_m, base.width_y_m, base.thickness_m)
        }
    };
    for (value, name) in [
        (center[0], "foundation x"),
        (center[1], "foundation y"),
        (center[2], "foundation z"),
        (rotation, "foundation rotation"),
        (length_x, "foundation length"),
        (width_y, "foundation width"),
        (thickness, "foundation thickness"),
    ] {
        validate_scalar(value, name)?;
    }
    if length_x <= 0.0 || width_y <= 0.0 || thickness <= 0.0 {
        return Err("foundation dimensions must be positive".into());
    }
    Ok(([center[0], center[1]], rotation, length_x, width_y, center[2]))
}

fn placement_thickness(placement: &foundation_core::FoundationPlacement) -> Result<f64, String> {
    match placement {
        foundation_core::FoundationPlacement::Raft { thickness_m, .. }
        | foundation_core::FoundationPlacement::Strip { thickness_m, .. }
        | foundation_core::FoundationPlacement::Isolated { thickness_m, .. } => {
            validate_positive(*thickness_m, "foundation thickness")?;
            Ok(*thickness_m)
        }
        foundation_core::FoundationPlacement::Stepped { .. } => Err("stepped placement needs tier geometry".into()),
    }
}

fn transform_foundation_xy(center: [f64; 2], rotation: f64, local: [f64; 2]) -> [f64; 2] {
    let (sin, cos) = rotation.sin_cos();
    [
        center[0] + local[0] * cos - local[1] * sin,
        center[1] + local[0] * sin + local[1] * cos,
    ]
}

fn prism_triangles(corners_xy: [[f64; 2]; 4], base_z: f64, thickness: f64) -> Result<Vec<Triangle>, String> {
    validate_positive(thickness, "foundation thickness")?;
    let top_z = base_z + thickness;
    let bottom: [Vec3; 4] = corners_xy.map(|p| checked_point3([p[0], p[1], base_z], "foundation prism vertex"))
        .into_iter().collect::<Result<Vec<_>, _>>()?.try_into().map_err(|_| "foundation prism corner count mismatch")?;
    let top: [Vec3; 4] = corners_xy.map(|p| checked_point3([p[0], p[1], top_z], "foundation prism vertex"))
        .into_iter().collect::<Result<Vec<_>, _>>()?.try_into().map_err(|_| "foundation prism corner count mismatch")?;
    let mut triangles = Vec::with_capacity(12);
    push_quad(&mut triangles, [bottom[0], bottom[3], bottom[2], bottom[1]]);
    push_quad(&mut triangles, top);
    for index in 0..4 {
        let next = (index + 1) % 4;
        push_quad(&mut triangles, [bottom[index], bottom[next], top[next], top[index]]);
    }
    Ok(triangles)
}

fn push_quad(triangles: &mut Vec<Triangle>, points: [Vec3; 4]) {
    let id = triangles.len() as u32;
    triangles.push(Triangle::new(id, points[0], points[1], points[2]));
    triangles.push(Triangle::new(id + 1, points[0], points[2], points[3]));
}

fn quad_triangles(points: [Vec3; 4], cell_index: usize) -> Result<Vec<Triangle>, String> {
    let id = u32::try_from(cell_index).ok()
        .and_then(|index| index.checked_mul(2))
        .ok_or_else(|| "foundation cell triangle ID overflowed".to_string())?;
    Ok(vec![
        Triangle::new(id, points[0], points[1], points[2]),
        Triangle::new(id + 1, points[0], points[2], points[3]),
    ])
}

fn include_prism_bounds(
    bounds: &mut TypedBounds,
    corners: [[f64; 2]; 4],
    bottom_z: f64,
    top_z: f64,
    what: &str,
) -> Result<(), String> {
    for point in corners {
        include_xyz(bounds, [point[0], point[1], bottom_z], what)?;
        include_xyz(bounds, [point[0], point[1], top_z], what)?;
    }
    Ok(())
}

fn include_prism_point(
    bounds: &mut TypedBounds,
    point: [f64; 2],
    bottom: f64,
    top: f64,
    what: &str,
) -> Result<(), String> {
    include_xyz(bounds, [point[0], point[1], bottom], what)?;
    include_xyz(bounds, [point[0], point[1], top], what)
}

fn record_placement_bounds(
    output: &mut GeoFoundationRenderScene,
    id: &str,
    corners: [[f64; 2]; 4],
    bottom: f64,
    top: f64,
) {
    let bounds = output
        .placement_bounds
        .entry(id.to_owned())
        .or_insert_with(Aabb::empty);
    for point in corners {
        bounds.include_point(Vec3::new(point[0], point[1], bottom));
        bounds.include_point(Vec3::new(point[0], point[1], top));
    }
}

fn include_xy(bounds: &mut TypedBounds, point: [f64; 2], what: &str) -> Result<(), String> {
    validate_xy(point, what)?;
    bounds.plan.get_or_insert_with(GeoBounds2D::empty).include(point);
    Ok(())
}

fn include_xyz(bounds: &mut TypedBounds, point: [f64; 3], what: &str) -> Result<(), String> {
    let point = checked_point3(point, what)?;
    bounds.overall.get_or_insert_with(Aabb::empty).include_point(point);
    bounds.model.get_or_insert_with(Aabb::empty).include_point(point);
    bounds.plan.get_or_insert_with(GeoBounds2D::empty).include([point.x, point.y]);
    Ok(())
}

fn add_canvas_segment(
    layer: &mut AnalysisCanvasLayer,
    start: Point2,
    end: Point2,
    color: [f32; 4],
    width: f32,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    if !start.is_finite() || !end.is_finite() || !width.is_finite() || width <= 0.0 {
        return Err("non-finite GEO canvas segment".into());
    }
    charge_canvas(budget, 1)?;
    layer.segments.push(ScreenSegment { start, end, color, width });
    Ok(())
}

fn add_canvas_triangle(
    layer: &mut AnalysisCanvasLayer,
    vertices: [Point2; 3],
    color: [f32; 4],
    budget: &mut RenderBudget,
) -> Result<(), String> {
    if vertices.iter().any(|point| !point.is_finite()) {
        return Err("non-finite GEO canvas triangle".into());
    }
    charge_canvas(budget, 1)?;
    layer.triangles.push(ScreenTriangle { vertices, color });
    Ok(())
}

fn push_canvas_label(
    layer: &mut AnalysisCanvasLayer,
    position: Point2,
    text: &str,
    color: [f32; 4],
    placement: ScreenLabelPlacement,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    if !position.is_finite() || text.len() > 1024 {
        return Err("invalid/oversized GEO canvas label".into());
    }
    charge_canvas(budget, 1)?;
    layer.labels.push(ScreenLabel {
        position,
        text: text.to_owned(),
        color,
        placement,
    });
    Ok(())
}

fn draw_cross(
    layer: &mut AnalysisCanvasLayer,
    center: Point2,
    half_size: f64,
    color: [f32; 4],
    budget: &mut RenderBudget,
) -> Result<(), String> {
    if !half_size.is_finite() || half_size <= 0.0 {
        return Err("invalid GEO marker size".into());
    }
    add_canvas_segment(layer, Point2::new(center.x - half_size, center.y), Point2::new(center.x + half_size, center.y), color, 1.8, budget)?;
    add_canvas_segment(layer, Point2::new(center.x, center.y - half_size), Point2::new(center.x, center.y + half_size), color, 1.8, budget)
}

fn draw_polygon(
    layer: &mut AnalysisCanvasLayer,
    points: &[[f64; 2]],
    camera: &Camera2D,
    color: [f32; 4],
    width: f32,
    budget: &mut RenderBudget,
) -> Result<(), String> {
    if points.len() < 3 || points.len() > geo_core::MAX_SECTION_POINTS {
        return Err("GEO plan polygon has invalid vertex count".into());
    }
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        add_canvas_segment(layer, camera.world_to_screen(Point2::new(a[0], a[1])), camera.world_to_screen(Point2::new(b[0], b[1])), color, width, budget)?;
    }
    Ok(())
}

fn hold_layer(camera: &Camera2D, title: &str, footer: &str) -> AnalysisCanvasLayer {
    AnalysisCanvasLayer {
        triangles: Vec::new(),
        segments: Vec::new(),
        labels: vec![
            ScreenLabel {
                position: Point2::new(16.0, 24.0),
                text: title.to_owned(),
                color: COLOR_UNKNOWN,
                placement: ScreenLabelPlacement::Hud,
            },
            ScreenLabel {
                position: Point2::new(16.0, camera.viewport_height - 18.0),
                text: footer.to_owned(),
                color: COLOR_MUTED,
                placement: ScreenLabelPlacement::Footer,
            },
        ],
    }
}

fn charge_canvas(budget: &mut RenderBudget, count: usize) -> Result<(), String> {
    budget.canvas_primitives = budget.canvas_primitives.checked_add(count)
        .ok_or_else(|| "GEO canvas primitive counter overflowed".to_string())?;
    if budget.canvas_primitives > MAX_RENDER_CANVAS_PRIMITIVES {
        return Err(format!("GEO canvas primitive budget {MAX_RENDER_CANVAS_PRIMITIVES} exceeded; nothing was truncated"));
    }
    Ok(())
}

fn finite_range(values: &[f64], what: &str) -> Result<(f64, f64), String> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err(format!("{what} samples are empty or non-finite"));
    }
    Ok((
        values.iter().copied().fold(f64::INFINITY, f64::min),
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ))
}

fn scalar_color(value: f64, min: f64, max: f64) -> [f32; 4] {
    let amount = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.5 };
    let red = amount as f32;
    let blue = (1.0 - amount) as f32;
    let green = (1.0 - (2.0 * amount - 1.0).abs()).clamp(0.0, 1.0) as f32;
    [red, green, blue, 0.74]
}

fn ige_color(id: &IgeId) -> [f32; 4] {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in id.as_str().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    let hue = (hash % 360) as f32;
    let chroma = 0.82 * 0.54;
    let x = chroma * (1.0 - (((hue / 60.0) % 2.0) - 1.0).abs());
    let (r, g, b) = match hue as u16 / 60 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = 0.82 - chroma;
    [r + m, g + m, b + m, 0.92]
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha;
    color
}

fn point_in_polygon(x: f64, y: f64, points: &[[f64; 2]]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = points.len() - 1;
    for current in 0..points.len() {
        let [xi, yi] = points[current];
        let [xj, yj] = points[previous];
        let crosses = (yi > y) != (yj > y)
            && x < (xj - xi) * (y - yi) / (yj - yi) + xi;
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn point_in_triangle(point: [f64; 2], triangle: [[f64; 2]; 3]) -> bool {
    let [a, b, c] = triangle;
    let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denominator.abs() <= f64::EPSILON {
        return false;
    }
    let u = ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1])) / denominator;
    let v = ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1])) / denominator;
    let w = 1.0 - u - v;
    u >= 0.0 && v >= 0.0 && w >= 0.0
}

fn screen_distance(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn screen_point_segment_distance(point: Point2, start: Point2, end: Point2) -> f64 {
    let delta = Point2::new(end.x - start.x, end.y - start.y);
    let length_squared = delta.x * delta.x + delta.y * delta.y;
    if length_squared <= f64::EPSILON {
        return screen_distance(point, start);
    }
    let relative = Point2::new(point.x - start.x, point.y - start.y);
    let t = ((relative.x * delta.x + relative.y * delta.y) / length_squared).clamp(0.0, 1.0);
    screen_distance(point, Point2::new(start.x + delta.x * t, start.y + delta.y * t))
}

fn approximately_equal(a: f64, b: f64) -> bool {
    (a - b).abs() <= GEOMETRY_TOLERANCE_M * a.abs().max(b.abs()).max(1.0)
}

fn approximately_equal_area(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0e-9 * a.abs().max(b.abs()).max(1.0)
}

fn validate_scalar(value: f64, what: &str) -> Result<(), String> {
    if !value.is_finite() || value.abs() > MAX_RENDER_COORDINATE_M {
        return Err(format!("{what} is non-finite or outside render bounds"));
    }
    Ok(())
}

fn validate_positive(value: f64, what: &str) -> Result<(), String> {
    validate_scalar(value, what)?;
    if value <= 0.0 {
        return Err(format!("{what} must be positive"));
    }
    Ok(())
}

fn validate_area_value(value: f64, what: &str) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 || value > MAX_RENDER_COORDINATE_M * MAX_RENDER_COORDINATE_M {
        return Err(format!("{what} is non-finite, negative, or outside render area bounds"));
    }
    Ok(())
}

fn validate_positive_area(value: f64, what: &str) -> Result<(), String> {
    validate_area_value(value, what)?;
    if value <= 0.0 {
        return Err(format!("{what} must be positive"));
    }
    Ok(())
}

fn validate_xy(point: [f64; 2], what: &str) -> Result<(), String> {
    validate_scalar(point[0], what)?;
    validate_scalar(point[1], what)
}

fn checked_point3(point: [f64; 3], what: &str) -> Result<Vec3, String> {
    for value in point {
        validate_scalar(value, what)?;
    }
    Ok(Vec3::new(point[0], point[1], point[2]))
}

fn validate_point(point: Vec3, what: &str) -> Result<(), String> {
    if !point.is_finite()
        || point.x.abs() > MAX_RENDER_COORDINATE_M
        || point.y.abs() > MAX_RENDER_COORDINATE_M
        || point.z.abs() > MAX_RENDER_COORDINATE_M
    {
        return Err(format!("{what} is non-finite or outside render bounds"));
    }
    Ok(())
}

fn datum_label(frame: &GeoCoordinateFrame) -> String {
    format!("vertical datum {}", optional_text(frame.vertical_datum.as_deref(), "unspecified"))
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() { fallback } else { value }
}

fn optional_text(value: Option<&str>, fallback: &str) -> String {
    value.filter(|value| !value.trim().is_empty()).unwrap_or(fallback).to_owned()
}

fn same_geo_closure(a: &GeoSnapshot, b: &GeoSnapshot) -> bool {
    a.selection == b.selection
        && a.selection_fingerprint == b.selection_fingerprint
        && a.records == b.records
}

fn placement_contains(
    placement: &foundation_core::FoundationPlacement,
    point: [f64; 2],
) -> Result<bool, String> {
    let layout = foundation_core::FoundationLayout {
        id: "renderer-pick".to_owned(),
        elements: vec![placement.clone()],
    };
    foundation_core::validate_foundation_layout(&layout)
        .map_err(|error| format!("foundation pick layout invalid: {error}"))?;
    let geometry = foundation_core::derive_layout_geometry(&layout)
        .map_err(|error| format!("foundation pick geometry failed: {error}"))?;
    Ok(geometry.elements.first().is_some_and(|element| {
        point_in_polygon(point[0], point[1], &element.corners_xy_m)
    }))
}
