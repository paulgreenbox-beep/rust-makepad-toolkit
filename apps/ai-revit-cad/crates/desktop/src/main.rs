#![recursion_limit = "512"]
//! Native Makepad desktop shell for the unified AI Revit document.

mod capture;
mod gpu_export;
mod agent_api;
#[cfg(target_os = "linux")]
mod proof_lifecycle_gate;
#[cfg(target_os = "linux")]
mod linux_lifecycle_wire;
#[cfg(target_os = "linux")]
mod linux_lifecycle;
#[cfg(target_os = "linux")]
mod linux_geotech_gui;
#[cfg(target_os = "linux")]
mod linux_geotech_gui_wire;
#[cfg(target_os = "linux")]
mod linux_geotech_persistence_contract;
#[cfg(target_os = "linux")]
mod linux_geotech_input_state;
#[cfg(target_os = "linux")]
mod linux_geotech_persistence_io;
#[cfg(target_os = "linux")]
mod linux_geotech_persistence_registration;
#[cfg(target_os = "linux")]
mod linux_geotech_persistence_adapter_guard;
#[cfg(feature = "native-boundary-harness")]
mod queue_harness;
#[cfg(feature = "native-boundary-harness")]
mod circular_owner_order;
mod agent_handler;
mod incremental_scene;
mod bim_editor_context;
mod semantic_api;
mod geometry_api;
mod gpu;
mod viewport;
mod drafting_canvas;
mod drafting_controller;
mod drafting_workspace;
mod geotech;
mod geotech_render;
mod geotech_api;
mod geotech_circular_render;
mod geotech_circular_ui;
mod geotech_circular_api;
mod geotech_circular_archive_view;
mod geotech_circular_close;
mod geotech_circular_reportworker;
mod geotech_circular_report;
mod geotech_circular_report_api;
mod geotech_circular_workspace;
mod geotech_circular_ui_host;
mod drafting_gpu_probe;
mod drafting_allocation_probe;
mod drawing_api;
mod bim_underlay;
mod file_guard;
mod foundation_structural_ui;
mod foundation_structural_render;
mod foundation_structural_api;
mod structural_configurator_ui;
mod structural_configurator_render;
mod structural_configurator_api;
mod geo_foundation_ui;
mod geo_foundation_render;
mod geo_foundation_api;
#[cfg(windows)]
mod accessibility;

use ai_revit_core::{
    ClassDef, ClassId, Command, Document, ElementCategory, ElementId, ElementKind, EntityRef,
    FloorSpec, LevelId, ModelElementType, OpeningSpec, OperationId, OperationKind, Point2, PropertyDef,
    PropertyId, PropertySource, PropertyType, PropertyValue, Relationship, RelationshipId, Unit,
    RelationshipType, RoofSpec, TypeId, WallSpec, BUILTIN_FLOOR_CLASS_ID,
    BUILTIN_ROOF_CLASS_ID, BUILTIN_WALL_CLASS_ID,
};
use ai_revit_geometry::{GeometryCache, RebuildReport};
use makepad_widgets::*;
#[cfg(target_os = "linux")]
use linux_lifecycle::AppLifecycle;
#[cfg(target_os = "linux")]
use linux_geotech_gui::GuiHooks;
use serde_json::{json, Value};
pub use makepad_widgets;
pub use makepad_xr;
pub(crate) use geotech_host as circular_host;
pub(crate) use geotech_jobs as circular_jobs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use viewer_core::{Vec3, ViewerEvent};
use viewport::{CadViewport, CadViewportAction, CadViewportWidgetRefExt};
use drafting_canvas::DraftingCanvasWidgetRefExt;
use drafting_workspace::DraftingWorkspace;
use geotech::GeotechWorkspace;
use geotech_circular_workspace::CircularWorkspace;
use geotech_circular_ui_host::CircularNativeBridge;
use circular_host::CircularHostController;
use geotech_circular_reportworker::CircularReportWorker;
use geotech_circular_report::CircularReportUiState;
use geotech_api::AnalysisCache;
use foundation_structural_ui::FoundationStructuralUi;
use structural_configurator_ui::StructuralConfiguratorUi;
use geo_foundation_ui::GeoFoundationUi;
use bim_underlay::BimUnderlayCache;
use bim_editor_context::{EditorRefresh, EditorState, EditorTarget, commit_editor_commands};

#[global_allocator]
static DRAFTING_ALLOCATOR: drafting_allocation_probe::CountingAllocator = drafting_allocation_probe::CountingAllocator;

// Keep the public SDK's normal adapter selection when no preference is requested.
// The nested generated entry retains the standard Makepad lifecycle.
#[allow(dead_code)]
mod native_entry {
    use super::*;
    app_main!(App);
}

fn main() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    linux_lifecycle_wire::initialize_trusted()?;
    #[cfg(target_os = "linux")]
    linux_geotech_gui::initialize_profile()?;
    #[cfg(windows)]
    if let Err(error)=accessibility::validate_args() {eprintln!("{error}");std::process::exit(2);}
    #[cfg(target_os = "windows")]
    {
        match std::env::var("AI_REVIT_GPU_MODE") {
            Err(std::env::VarError::NotPresent) => {}
            Ok(requested) => {
                eprintln!("UNSUPPORTED_PUBLIC_SDK: cannot honor AI_REVIT_GPU_MODE={requested}; the public Makepad SDK has no adapter-preference contract");
                std::process::exit(2);
            }
            Err(error) => {
                eprintln!("UNSUPPORTED_PUBLIC_SDK: cannot read or honor AI_REVIT_GPU_MODE: {error}");
                std::process::exit(2);
            }
        }
    }
    native_entry::app_main();
    #[cfg(target_os = "linux")]
    linux_lifecycle_wire::finish_process()?;
    Ok(())
}

const HISTORY_PAGE_SIZE: usize = 16;
const SEMANTIC_TREE_PAGE_SIZE: usize = 24;
const SEMANTIC_RELATIONSHIP_PAGE_SIZE: usize = 4;
const OPENING_UI_PAGE_SIZE: usize = 4;
const OPENING_UI_ROW_IDS: [LiveId; OPENING_UI_PAGE_SIZE] = [
    id!(opening_row_0), id!(opening_row_1), id!(opening_row_2), id!(opening_row_3),
];
const HISTORY_ROW_IDS: [LiveId; HISTORY_PAGE_SIZE] = [
    id!(history_row_0), id!(history_row_1), id!(history_row_2), id!(history_row_3),
    id!(history_row_4), id!(history_row_5), id!(history_row_6), id!(history_row_7),
    id!(history_row_8), id!(history_row_9), id!(history_row_10), id!(history_row_11),
    id!(history_row_12), id!(history_row_13), id!(history_row_14), id!(history_row_15),
];
const SEMANTIC_TREE_ROW_IDS: [LiveId; 24] = [
    id!(semantic_tree_row_0), id!(semantic_tree_row_1), id!(semantic_tree_row_2), id!(semantic_tree_row_3), id!(semantic_tree_row_4), id!(semantic_tree_row_5), id!(semantic_tree_row_6), id!(semantic_tree_row_7), id!(semantic_tree_row_8), id!(semantic_tree_row_9), id!(semantic_tree_row_10), id!(semantic_tree_row_11), id!(semantic_tree_row_12), id!(semantic_tree_row_13), id!(semantic_tree_row_14), id!(semantic_tree_row_15), id!(semantic_tree_row_16), id!(semantic_tree_row_17), id!(semantic_tree_row_18), id!(semantic_tree_row_19), id!(semantic_tree_row_20), id!(semantic_tree_row_21), id!(semantic_tree_row_22), id!(semantic_tree_row_23)
];
const SEMANTIC_RELATION_ROW_IDS: [LiveId; 4] = [
    id!(semantic_relation_row_0), id!(semantic_relation_row_1), id!(semantic_relation_row_2), id!(semantic_relation_row_3)
];

fn format_property_value(value: &PropertyValue) -> String {
    match value {
        PropertyValue::Text(value) => value.clone(),
        PropertyValue::Integer(value) => value.to_string(),
        PropertyValue::Boolean(value) => if *value { "Да" } else { "Нет" }.to_owned(),
        PropertyValue::Number { value, unit } => {
            let number = if *value == 0.0 {
                "0.0".to_owned()
            } else {
                let mut formatted = format!("{value:.6}");
                while formatted.ends_with('0') {
                    formatted.pop();
                }
                if formatted.ends_with('.') {
                    formatted.push('0');
                }
                if formatted == "0.0" || formatted == "-0.0" {
                    value.to_string()
                } else {
                    formatted
                }
            };
            let suffix = match unit {
                Unit::Unitless => "",
                Unit::Meter => "м",
                Unit::SquareMeter => "м²",
                Unit::CubicMeter => "м³",
                Unit::Radian => "рад",
                Unit::Kilogram => "кг",
                Unit::Second => "с",
            };
            if suffix.is_empty() { number } else { format!("{number} {suffix}") }
        }
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "AI Revit v0.5 — BIM + CAD"
                window.inner_size: vec2(1440, 900)
                body +: {
                    app_view := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x10161d}

                        file_toolbar := SolidView{
                            width: Fill
                            height: 52.0
                            flow: Down
                            draw_bg +: {color: #x1a232d}
                            ScrollXView{
                            width: Fill height: Fill flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 12.0 right: 12.0}
                            spacing: 7.0
                            title := H3{
                                width: Fit
                                text: "AI Revit"
                                draw_text +: {color: #xe8eef5}
                            }
                            workspace_3d := Button{text: "● 3D"}
                            workspace_2d := Button{text: "2D"}
                            workspace_engineering := Button{text: "Engineering"}
                            workspace_circular := Button{text: "Круговой расчёт"}
                            workspace_foundation_structural := Button{text: "Foundations / Structures"}
                            workspace_structural_configurator := Button{text: "Steel Building Constructor"}
                            workspace_geo_foundation := Button{text: "GEO / Foundation"}
                            new_document := Button{text: "Новый"}
                            open_document := Button{text: "Открыть"}
                            save_document := Button{text: "Сохранить"}
                            undo_document := Button{text: "Отменить"}
                            redo_document := Button{text: "Повторить"}
                            path_input := TextInput{
                                width: 320
                                height: 34.0
                                text: "ai-revit.json"
                                empty_text: "Путь к файлу"
                            }
                            }
                        }

                        model_toolbar := SolidView{
                            width: Fill
                            height: 88.0
                            flow: Down
                            draw_bg +: {color: #x151d26}
                            model_actions := View{
                                width: Fill height: 44.0
                                flow: Right
                                align: Align{x: 0.0 y: 0.5}
                                padding: Inset{left: 12.0 right: 12.0}
                                spacing: 7.0
                                add_wall := Button{text: "Стена"}
                                add_floor := Button{text: "Перекрытие"}
                                add_roof := Button{text: "Кровля"}
                                export_png := Button{text: "Экспорт 3D-вида PNG"}
                                Filler{}
                                gpu_status := Label{
                                    text: "GPU: проверка"
                                    draw_text +: {color: #x91a0af}
                                }
                                quality_label := Label{
                                    text: "Упрощенная сетка"
                                    draw_text +: {color: #x91a0af}
                                }
                            }
                            geometry_inputs := ScrollXView{
                                width: Fill height: 40.0
                                flow: Right
                                align: Align{x: 0.0 y: 0.5}
                                padding: Inset{left: 12.0 right: 12.0}
                                spacing: 6.0
                                geometry_x_label := Label{text: "X" draw_text +: {color: #x9aabba}}
                                geometry_x := TextInput{width: 64.0 height: 30.0 text: "0.4" empty_text: "X, м"}
                                geometry_y_label := Label{text: "Y" draw_text +: {color: #x9aabba}}
                                geometry_y := TextInput{width: 64.0 height: 30.0 text: "0.4" empty_text: "Y, м"}
                                geometry_z_label := Label{text: "Z" draw_text +: {color: #x9aabba}}
                                geometry_z := TextInput{width: 64.0 height: 30.0 text: "0" empty_text: "Z, м"}
                                geometry_sx_label := Label{text: "Дл./Ш." draw_text +: {color: #x9aabba}}
                                geometry_sx := TextInput{width: 64.0 height: 30.0 text: "7.2" empty_text: "Дл./Ш., м"}
                                geometry_sy_label := Label{text: "Глуб." draw_text +: {color: #x9aabba}}
                                geometry_sy := TextInput{width: 64.0 height: 30.0 text: "5.2" empty_text: "Глуб., м"}
                                geometry_height_label := Label{text: "Выс." draw_text +: {color: #x9aabba}}
                                geometry_height := TextInput{width: 64.0 height: 30.0 text: "3" empty_text: "Выс., м"}
                                geometry_thickness_label := Label{text: "Толщ." draw_text +: {color: #x9aabba}}
                                geometry_thickness := TextInput{width: 64.0 height: 30.0 text: "0.2" empty_text: "Толщ., м"}
                                geometry_hint := Label{
                                    text: "м · стена вдоль X · X/Y — начало, Z — отметка · глубина для перекрытия/кровли"
                                    draw_text +: {color: #x91a0af}
                                }
                            }
                        }

                        drafting_toolbar := SolidView{
                            width: Fill height: 88 flow: Down visible: false
                            draw_bg.color: #x151d26
                            ScrollXView{
                                width: Fill height: 44 flow: Right spacing: 6
                                align: Align{y: 0.5} padding: Inset{left: 12 right: 12}
                                draft_select := Button{text: "SELECT"}
                                draft_line := Button{text: "LINE"}
                                draft_circle := Button{text: "CIRCLE"}
                                draft_arc := Button{text: "ARC"}
                                draft_polyline := Button{text: "PLINE"}
                                draft_move := Button{text: "MOVE"}
                                draft_delete := Button{text: "Удалить"}
                                draft_fit := Button{text: "Вписать"}
                                draft_export_png := Button{text: "PNG"}
                                draft_export_svg := Button{text: "SVG"}
                                draft_export_dxf := Button{text: "DXF"}
                            }
                            ScrollXView{
                                width: Fill height: 40 flow: Right spacing: 6
                                align: Align{y: 0.5} padding: Inset{left: 12 right: 12}
                                Label{text: "Чертёж" draw_text.color: #x9aabba}
                                drawing_selector := DropDown{width: 240 labels: ["Нет чертежей"]}
                                drawing_name_input := TextInput{width: 180 height: 30 text: "Чертёж 1"}
                                draft_new_drawing := Button{text: "+ Чертёж"}
                                draft_rename_drawing := Button{text: "Переименовать"}
                                draft_bim_reference := Button{text: "+ BIM XY reference"}
                                drafting_quality := Label{text: "analytic_2d · м" draw_text.color: #x91a0af}
                            }
                        }

                        workspace := View{
                            width: Fill
                            height: Fill
                            flow: Right

                            geotech_parameters := mod.widgets.GeotechParameters{visible: false}
                            fs_workspace := mod.widgets.FoundationStructuralWorkspace{}
                            sc_workspace := mod.widgets.StructuralConfiguratorWorkspace{}
                            gf_workspace := mod.widgets.GeoFoundationWorkspace{}
                            circular_parameters_host := View{
                                width: 300 height: Fill flow: Down visible: false
                                View{width: Fill height: 32 flow: Right spacing: 4
                                    circular_ground_prev := Button{width: Fill text: "Разрезы ←"}
                                    circular_ground_next := Button{width: Fill text: "Разрезы →"}
                                }
                                View{width: Fill height: 32 flow: Right spacing: 4
                                    circular_case_prev := Button{width: Fill text: "Случаи ←"}
                                    circular_case_next := Button{width: Fill text: "Случаи →"}
                                }
                                circular_catalog_status := Label{width: Fill height: Fit text: "Страницы разрезов и расчётных случаев"}
                                circular_parameters := mod.widgets.GeotechCircularParameters{}
                            }

                            history_panel := SolidView{
                                width: 352.0
                                height: Fill
                                flow: Down
                                padding: 10.0
                                spacing: 8.0
                                draw_bg +: {color: #x17212b}
                                history_heading := H3{
                                    width: Fill
                                    text: "История построения"
                                    draw_text +: {color: #xe0e9f2}
                                }
                                navigator_tabs := View{
                                    width: Fill height: 32.0 flow: Right spacing: 6.0
                                    objects_tab := Button{width: Fill height: 30.0 text: "Объекты"}
                                    history_tab := Button{width: Fill height: 30.0 text: "История"}
                                }
                                drawings_tree := View{
                                    width: Fill height: Fit flow: Down spacing: 3 visible: false
                                    Label{width: Fill text: "Чертежи / Drawings" draw_text.color: #x9aabba}
                                    drawing_tree_0 := ButtonFlat{width: Fill height: 26 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    drawing_tree_1 := ButtonFlat{width: Fill height: 26 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    drawing_tree_2 := ButtonFlat{width: Fill height: 26 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    drawing_tree_3 := ButtonFlat{width: Fill height: 26 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    View{
                                        width: Fill height: 28 flow: Right spacing: 5
                                        drawing_tree_previous := Button{text: "‹"}
                                        drawing_tree_page := Label{text: "1 / 1" draw_text.color: #x9aabba}
                                        drawing_tree_next := Button{text: "›"}
                                    }
                                    Label{width: Fill text: "Виды / Views" draw_text.color: #x9aabba}
                                    drawing_view_reference := ButtonFlat{width: Fill height: 26 text: "BIM Plan Reference · XY"}
                                    Label{width: Fill text: "BIM" draw_text.color: #x9aabba}
                                }
                                semantic_tree_scroll := ScrollYView{
                                    width: Fill height: Fill
                                    flow: Down spacing: 2.0 visible: false
                                    semantic_tree_row_0 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_1 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_2 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_3 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_4 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_5 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_6 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_7 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_8 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_9 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_10 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_11 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_12 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_13 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_14 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_15 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_16 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_17 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_18 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_19 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_20 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_21 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_22 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_tree_row_23 := ButtonFlat{width: Fill height: 26.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    tree_empty_state := Label{
                                        width: Fill height: Fit
                                        text: "Создайте или откройте документ."
                                        draw_text +: {color: #x9aabba}
                                        visible: false
                                    }
                                    tree_pager := View{
                                        width: Fill height: 32.0 flow: Right
                                        align: Align{x: 0.5 y: 0.5} spacing: 8.0
                                        tree_previous := Button{text: "‹"}
                                        tree_page_label := Label{text: "1 / 1" draw_text +: {color: #xb1bfcc}}
                                        tree_next := Button{text: "›"}
                                    }
                                }
                                history_hint := Label{
                                    width: Fill
                                    height: Fit
                                    text: "Операции документа · выберите строку"
                                    draw_text +: {color: #x9aabba}
                                }
                                history_scroll := ScrollYView{
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    spacing: 3.0
                                    history_row_0 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_1 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_2 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_3 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_4 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_5 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_6 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_7 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_8 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_9 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_10 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_11 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_12 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_13 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_14 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    history_row_15 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                }
                                history_pager := SolidView{
                                    width: Fill height: 32.0 flow: Right
                                    align: Align{x: 0.5 y: 0.5} spacing: 8.0
                                    history_previous := Button{text: "‹"}
                                    history_page_label := Label{text: "1 / 1" draw_text +: {color: #xb1bfcc}}
                                    history_recent := Button{text: "К новым"}
                                    history_next := Button{text: "›"}
                                }
                            }

                            viewport_panel := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                viewport_header := SolidView{
                                    width: Fill
                                    height: 36.0
                                    flow: Right
                                    align: Align{x: 0.0 y: 0.5}
                                    padding: Inset{left: 12.0 right: 12.0}
                                    draw_bg +: {color: #x202a34}
                                    viewport_title := Label{
                                        text: "3D-вид"
                                        draw_text +: {color: #xe1eaf3}
                                    }
                                    Filler{}
                                    navigation_hint := Label{
                                        text: "Тянуть — вращать · колесо — масштаб · клик — выбрать"
                                        draw_text +: {color: #x92a1b0}
                                    }
                                }
                                circular_view_status := Label{width: Fill height: Fit visible: false text: "Предпросмотр сечения"}
                                viewport_3d_host := View{
                                    width: Fill height: Fill flow: Down
                                    viewport := mod.widgets.CadViewport{}
                                }
                                geotech_viewport_host := View{
                                    width: Fill height: Fill flow: Down visible: false
                                    geotech_viewport := mod.widgets.CadViewport{}
                                }
                                circular_viewport_host := View{
                                    width: Fill height: Fill flow: Down visible: false
                                    circular_viewport := mod.widgets.CadViewport{}
                                }
                                drafting_2d_host := View{
                                    width: Fill height: Fill flow: Down visible: false
                                    drafting_canvas_cache := CachedView{
                                        width: Fill height: Fill
                                        drafting_canvas := mod.widgets.DraftingCanvas{}
                                    }
                                }
                            }

                            inspector_panel := SolidView{
                                width: 350.0
                                height: Fill
                                flow: Down
                                padding: 12.0
                                spacing: 8.0
                                draw_bg +: {color: #x17212b}
                                drawing_source_banner := Label{
                                    width: Fill height: Fit text: "" visible: false
                                    draw_text.color: #x91a0af
                                }
                                draft_open_bim_source := Button{text: "Открыть BIM-источник в 3D" visible: false}
                                drafting_inspector := ScrollYView{
                                    width: Fill height: Fill flow: Down spacing: 7 visible: false
                                    H3{text: "CAD · аналитическая геометрия" draw_text.color: #xe0e9f2}
                                    draft_selected_label := Label{width: Fill text: "Нет выбранных объектов" draw_text.color: #xc2cfda}
                                    draft_select_id := TextInput{width: Fill height: 30 empty_text: "DraftEntity ID, десятичная строка"}
                                    draft_select_by_id := Button{text: "Выбрать по ID"}
                                    draft_inspect := TextInput{width: Fill height: 130 is_multiline: true is_read_only: true text: "Выберите CAD-объект."}
                                    Label{text: "Геометрия JSON · м / рад" draw_text.color: #x9aabba}
                                    draft_geometry_input := TextInput{width: Fill height: 130 is_multiline: true empty_text: "Аналитическая геометрия"}
                                    draft_apply_geometry := Button{text: "Применить геометрию"}
                                    Hr{}
                                    Label{text: "Активный слой" draw_text.color: #x9aabba}
                                    draft_layer_selector := DropDown{width: Fill labels: ["Нет слоя"]}
                                    draft_layer_summary := Label{width: Fill text: "" draw_text.color: #x9aabba}
                                    View{
                                        width: Fill height: 32 flow: Right spacing: 5
                                        draft_layer_visible := Button{text: "Видимость"}
                                        draft_layer_locked := Button{text: "Блокировка"}
                                    }
                                    Hr{}
                                    draft_underlay_summary := TextInput{width: Fill height: 120 is_multiline: true is_read_only: true text: "BIM reference не задан."}
                                }
                                inspector_scroll := ScrollYView{
                                    width: Fill height: Fill
                                    flow: Down spacing: 8.0
                                    inspector_heading := H3{
                                        width: Fill
                                        text: "Текущий выбор"
                                        draw_text +: {color: #xe0e9f2}
                                    }
                                    selected_label := Label{
                                        width: Fill
                                        height: Fit
                                        text: "Ничего не выбрано"
                                        draw_text +: {color: #xc2cfda}
                                    }
                                    quantities_panel := View{
                                        width: Fill height: Fit flow: Down spacing: 4.0
                                        quantities_heading := Label{width: Fill height: Fit text: "Количества" draw_text +: {color: #x8fa0b0}}
                                        quantities_quality := Label{width: Fill height: Fit text: "Approximate mesh" draw_text +: {color: #xb1bfcc}}
                                        quantities_gross_volume := Label{width: Fill height: Fit text: "Объём брутто: —" draw_text +: {color: #xb1bfcc}}
                                        quantities_net_volume := Label{width: Fill height: Fit text: "Объём нетто: —" draw_text +: {color: #xb1bfcc}}
                                        quantities_opening_area := Label{width: Fill height: Fit text: "Площадь проёмов: —" draw_text +: {color: #xb1bfcc}}
                                        quantities_surface_area := Label{width: Fill height: Fit text: "Поверхность брутто / нетто: —" draw_text +: {color: #xb1bfcc}}
                                        quantities_status := Label{width: Fill height: Fit text: "Выберите элемент." draw_text +: {color: #x9aabba}}
                                        Hr{}
                                    }
                                    openings_panel := View{
                                        width: Fill height: Fit flow: Down spacing: 5.0
                                        openings_heading := Label{width: Fill height: Fit text: "Проёмы" draw_text +: {color: #x8fa0b0}}
                                        opening_summary := Label{width: Fill height: Fit text: "Выберите стену или проём." draw_text +: {color: #xb1bfcc}}
                                        opening_row_0 := ButtonFlat{width: Fill height: 28.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                        opening_row_1 := ButtonFlat{width: Fill height: 28.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                        opening_row_2 := ButtonFlat{width: Fill height: 28.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                        opening_row_3 := ButtonFlat{width: Fill height: 28.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                        opening_pager := View{
                                            width: Fill height: 28.0 flow: Right spacing: 6.0 align: Align{x: 0.5 y: 0.5} visible: false
                                            opening_previous := Button{text: "‹"}
                                            opening_page_label := Label{text: "1 / 1" draw_text +: {color: #xb1bfcc}}
                                            opening_next := Button{text: "›"}
                                        }
                                        opening_create_inputs := View{
                                            width: Fill height: Fit flow: Down spacing: 4.0 visible: false
                                            View{
                                                width: Fill height: 30.0 flow: Right spacing: 5.0 align: Align{x: 0.0 y: 0.5}
                                                Label{text: "Шир., м" draw_text +: {color: #x9aabba}}
                                                opening_width := TextInput{width: 64.0 height: 28.0 text: "1.2"}
                                                Label{text: "Выс., м" draw_text +: {color: #x9aabba}}
                                                opening_height := TextInput{width: 64.0 height: 28.0 text: "1.5"}
                                            }
                                            View{
                                                width: Fill height: 30.0 flow: Right spacing: 5.0 align: Align{x: 0.0 y: 0.5}
                                                opening_offset_label := Label{text: "Центр, м" draw_text +: {color: #x9aabba}}
                                                opening_offset := TextInput{width: 64.0 height: 28.0 text: "" empty_text: "середина"}
                                                opening_sill_label := Label{text: "Низ, м" draw_text +: {color: #x9aabba}}
                                                opening_sill := TextInput{width: 64.0 height: 28.0 text: "0.9"}
                                            }
                                            opening_input_hint := Label{width: Fill height: Fit text: "Центр от начала стены; низ от её основания." draw_text +: {color: #x9aabba}}
                                        }
                                        View{
                                            width: Fill height: 30.0 flow: Right spacing: 4.0
                                            add_opening := Button{text: "+ Проём"}
                                            add_window := Button{text: "+ Окно"}
                                            add_door := Button{text: "+ Дверь"}
                                        }
                                        opening_wall_front := ButtonFlat{width: Fill height: 28.0 text: "Вид стены спереди"}
                                        opening_host_view := ButtonFlat{width: Fill height: 28.0 text: "Вид вдоль нормали перекрытия / крыши" visible: false}
                                        show_opening_volumes := ButtonFlat{width: Fill height: 28.0 text: "Объёмы проёмов: скрыты"}
                                        Hr{}
                                    }
                                    instance_heading := Label{
                                        width: Fill height: Fit text: "Экземпляр"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    instance_summary := Label{
                                        width: Fill height: Fit text: "Элемент не выбран."
                                        draw_text +: {color: #xb1bfcc}
                                    }
                                    type_heading := Label{
                                        width: Fill height: Fit text: "Тип"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    type_summary := Label{
                                        width: Fill height: Fit text: "Без типа"
                                        draw_text +: {color: #xb1bfcc}
                                    }
                                    type_selector_heading := Label{
                                        width: Fill height: Fit text: "Назначить тип"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    type_selector := DropDownFlat{
                                        width: Fill height: 34.0
                                        labels: ["Нет типов"]
                                        popup_menu: mod.widgets.PopupMenu{width: 320.0}
                                    }
                                    assign_type := Button{text: "Назначить тип"}
                                    level_heading := Label{
                                        width: Fill height: Fit text: "Уровень"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    level_summary := Label{
                                        width: Fill height: Fit text: "Без уровня"
                                        draw_text +: {color: #xb1bfcc}
                                    }
                                    property_editing_panel := View{
                                        width: Fill height: Fit flow: Down spacing: 5.0
                                        property_editing_heading := Label{width: Fill height: Fit text: "Основные параметры" draw_text +: {color: #x8fa0b0}}
                                        editable_property_selector := DropDownFlat{
                                            width: Fill height: 32.0 labels: ["Нет параметров"]
                                            popup_menu: mod.widgets.PopupMenu{width: 320.0}
                                        }
                                        editable_property_source := Label{width: Fill height: Fit text: "Выберите экземпляр или тип." draw_text +: {color: #xb1bfcc}}
                                        Label{width: Fill height: Fit text: "Значение экземпляра" draw_text +: {color: #x9aabba}}
                                        instance_override_value := TextInput{width: Fill height: 30.0 text: "" empty_text: "Переопределение"}
                                        View{
                                            width: Fill height: 30.0 flow: Right spacing: 4.0
                                            apply_instance_override := Button{text: "Применить"}
                                            clear_instance_override := Button{text: "Снять переопределение"}
                                        }
                                        Label{width: Fill height: Fit text: "Значение типа" draw_text +: {color: #x9aabba}}
                                        type_property_value := TextInput{width: Fill height: 30.0 text: "" empty_text: "Параметр типа"}
                                        apply_type_property := Button{text: "Применить к типу"}
                                        Hr{}
                                    }
                                    resolved_heading := Label{
                                        width: Fill height: Fit text: "Вычисленные свойства"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    resolved_summary := Label{
                                        width: Fill height: Fit text: "Нет вычисленных свойств."
                                        draw_text +: {color: #xb1bfcc}
                                    }
                                    relationships_heading := Label{
                                        width: Fill height: Fit text: "Связи"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    semantic_relation_row_0 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_relation_row_1 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_relation_row_2 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    semantic_relation_row_3 := ButtonFlat{width: Fill height: 30.0 align: Align{x: 0.0 y: 0.5} text: "" visible: false}
                                    relationships_empty := Label{
                                        width: Fill height: Fit text: "Нет связей."
                                        draw_text +: {color: #x9aabba}
                                        visible: false
                                    }
                                    relationship_pager := View{
                                        width: Fill height: 32.0 flow: Right
                                        align: Align{x: 0.5 y: 0.5} spacing: 8.0
                                        relationships_previous := Button{text: "‹"}
                                        relationships_page_label := Label{text: "1 / 1" draw_text +: {color: #xb1bfcc}}
                                        relationships_next := Button{text: "›"}
                                    }
                                    Hr{}
                                    class_heading := Label{
                                        width: Fill height: Fit
                                        text: "Класс"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    class_input := TextInput{
                                        width: Fill height: 34.0
                                        text: "Новая стена"
                                        empty_text: "Имя класса"
                                    }
                                    create_class := Button{text: "Создать класс"}
                                    property_heading := Label{
                                        width: Fill height: Fit
                                        text: "Свойства"
                                        draw_text +: {color: #x8fa0b0}
                                    }
                                    property_name := TextInput{
                                        width: Fill height: 34.0
                                        text: "Огнестойкость"
                                        empty_text: "Имя свойства"
                                    }
                                    property_value := TextInput{
                                        width: Fill height: 34.0
                                        text: "60 мин"
                                        empty_text: "Значение"
                                    }
                                    set_property := Button{text: "Задать свойство"}
                                    Hr{}
                                    property_summary := Label{
                                        width: Fill
                                        height: Fit
                                        text: "Здесь появятся свойства элемента."
                                        draw_text +: {color: #xb1bfcc}
                                    }
                                }
                            }
                            geotech_results := mod.widgets.GeotechResults{visible: false}
                            circular_results := mod.widgets.GeotechCircularResults{visible: false}
                        }

                        geotech_actions := mod.widgets.GeotechActions{visible: false}
                        circular_actions := mod.widgets.GeotechCircularActions{visible: false}

                        drafting_command_panel := SolidView{
                            width: Fill height: Fit flow: Down spacing: 5 visible: false
                            padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
                            draw_bg.color: #x1d2b39
                            draft_command_prompt := Label{width: Fill text: "SELECT" draw_text.color: #xf4c483}
                            View{
                                width: Fill height: 32 flow: Right spacing: 5 align: Align{y: 0.5}
                                draft_numeric_input := TextInput{width: Fill height: 30 empty_text: "X Y (м), радиус или параметры дуги · Enter"}
                                draft_confirm := Button{text: "Подтвердить"}
                                draft_finish_polyline := Button{text: "Завершить"}
                                draft_close_polyline := Button{text: "Замкнуть"}
                                draft_bulge_input := TextInput{width: 85 height: 30 text: "0"}
                                draft_set_bulge := Button{text: "Bulge"}
                                draft_cancel := Button{text: "Отмена"}
                            }
                        }
                        status_bar := SolidView{
                            width: Fill
                            height: 30.0
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 12.0 right: 12.0}
                            draw_bg +: {color: #x202a34}
                            status := Label{
                                width: Fill
                                text: "Готово"
                                draw_text +: {color: #xb6c6d5}
                            }
                            selection_status := Label{
                                width: Fit
                                text: "Элемент не выбран"
                                draw_text +: {color: #x8fa0b0}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct HistoryRow {
    operation_id: OperationId,
}

#[derive(Clone, Debug)]
struct SemanticTreeRow {
    control_id: String,
    row_kind: &'static str,
    label: String,
    entity: Option<EntityRef>,
}

#[derive(Clone, Debug)]
struct SemanticRelationshipRow {
    control_id: String,
    relationship_id: RelationshipId,
    source_element: ElementId,
    target: EntityRef,
    label: String,
}

#[derive(Clone, Debug)]
struct OpeningUiRow {
    element_id: ElementId,
    opening_id: Option<ElementId>,
    role: &'static str,
    label: String,
}

#[derive(Clone)]
struct PropertyUiOption {
    property_id: PropertyId,
    label: String,
    value_type: PropertyType,
    resolved_value: Option<PropertyValue>,
    resolved_source: Option<PropertySource>,
    instance_override: Option<PropertyValue>,
    type_value: Option<PropertyValue>,
    type_default_value: Option<PropertyValue>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    document: Option<Document>,
    #[rust]
    drafting_ui: DraftingWorkspace,
    #[rust]
    geotech_ui: GeotechWorkspace,
    #[rust]
    circular_ui: CircularWorkspace,
    #[rust]
    circular_native: CircularNativeBridge,
    #[rust]
    geotech_results: AnalysisCache,
    #[rust]
    circular_host: Option<CircularHostController>,
    #[rust]
    circular_close_window: Option<WindowId>,
    #[rust]
    circular_report: CircularReportWorker,
    #[rust]
    circular_report_ui: CircularReportUiState,
    #[rust]
    foundation_structural_ui: FoundationStructuralUi,
    #[rust]
    structural_configurator_ui: StructuralConfiguratorUi,
    #[rust]
    geo_foundation_ui: GeoFoundationUi,
    #[rust]
    bim_underlay_cache: BimUnderlayCache,
    #[rust]
    history_rows: Vec<HistoryRow>,
    #[rust]
    history_page: usize,
    #[rust]
    selected_element: Option<ElementId>,
    #[rust]
    selected_operation: Option<OperationId>,
    #[rust]
    semantic_selection: Option<EntityRef>,
    #[rust]
    showing_objects: bool,
    #[rust]
    semantic_tree_page: usize,
    #[rust]
    semantic_tree_rows: Vec<SemanticTreeRow>,
    #[rust]
    semantic_tree_visible_rows: Vec<SemanticTreeRow>,
    #[rust]
    semantic_type_options: Vec<TypeId>,
    #[rust]
    semantic_relationship_page: usize,
    #[rust]
    semantic_relationship_rows: Vec<SemanticRelationshipRow>,
    #[rust]
    opening_ui_rows: Vec<OpeningUiRow>,
    #[rust]
    opening_ui_page: usize,
    #[rust]
    opening_ui_count: usize,
    #[rust]
    opening_ui_details: Value,
    #[rust]
    opening_input_owner: Option<ElementId>,
    #[rust]
    opening_input_geometry: Option<ElementKind>,
    #[rust]
    quantity_ui_details: Value,
    #[rust]
    property_ui_options: Vec<PropertyUiOption>,
    #[rust]
    property_ui_owner: Option<EntityRef>,
    #[rust]
    property_ui_element: Option<ElementId>,
    #[rust]
    property_ui_type: Option<TypeId>,
    #[rust]
    property_ui_property: Option<PropertyId>,
    #[rust]
    property_ui_snapshot: Value,
    #[rust]
    property_editor: EditorState,
    #[rust]
    custom_property_editor: EditorState,
    #[rust]
    show_opening_volumes: bool,
    #[rust]
    geometry_cache: GeometryCache,
    #[rust]
    pending_gpu_png: Option<(PathBuf, std::time::Instant)> ,
    #[rust]
    last_rebuild: RebuildReport,
    #[rust]
    next_demo_wall: u32,
    #[rust]
    next_demo_floor: u32,
    #[rust]
    next_demo_roof: u32,
    #[rust]
    next_command_sequence: u64,
    #[rust]
    active_wall_class: Option<ClassId>,
    #[rust]
    active_floor_class: Option<ClassId>,
    #[rust]
    active_roof_class: Option<ClassId>,
    #[rust]
    api_document_generation: u64,
    #[rust]
    api_receiver: Option<Receiver<agent_api::ApiCommand>>,
    #[rust]
    api_poll_timer: Timer,
    #[cfg(target_os = "linux")]
    #[rust]
    proof_lifecycle: AppLifecycle,
    #[cfg(target_os = "linux")]
    #[rust]
    gui_hooks: GuiHooks,
}

impl App {
    // Shared by the native button and UIA Invoke. No separate document writer.
    fn save_current_document(&mut self, cx: &mut Cx) -> Result<(), String> {
        let path = self.path_text(cx);
        self.api_document_save(cx,&serde_json::json!({"path":path})).map(|_|())
    }
    fn viewport_widget(&self, cx: &Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(viewport))
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status)).set_text(cx, text);
    }

    fn path_text(&self, cx: &Cx) -> String {
        self.ui.text_input(cx, ids!(path_input)).text()
    }

    fn next_command_id(&mut self, label: &str) -> String {
        let id = format!("desktop-{}-{}-{label}", std::process::id(), self.next_command_sequence);
        self.next_command_sequence = self.next_command_sequence.saturating_add(1);
        id
    }

    fn commit_commands(&mut self, label: &str, commands: Vec<Command>) -> Result<Vec<ElementId>, String> {
        let command_id = self.next_command_id(label);
        let document = self.document.as_mut().ok_or_else(|| "no document is open".to_owned())?;
        let revision = document.revision();
        let mut transaction = document.begin_transaction(command_id, revision);
        for command in commands {
            transaction.push(command);
        }
        let receipt = document.commit(transaction).map_err(|error| error.to_string())?;
        self.circular_after_mutation();
        self.history_page = usize::MAX;
        Ok(receipt.created_elements)
    }

    fn select_element(&mut self, cx: &mut Cx, id: ElementId) {
        self.mirror_bim_selection(id);
        self.track_element_selection(id);
        self.sync_viewport_selection(cx);
        self.refresh_inspector(cx);
        self.refresh_history(cx);
        self.refresh_object_tree(cx);
    }

    fn track_element_selection(&mut self, id: ElementId) {
        let created_by = self.document.as_ref()
            .and_then(|document| document.element(id))
            .map(|element| element.created_by);
        if let Some(operation_id) = created_by {
            self.selected_element = Some(id);
            self.semantic_selection = Some(EntityRef::Element(id));
            self.set_selected_operation(operation_id);
        } else {
            self.selected_element = None;
                    self.semantic_selection = None;
            self.semantic_selection = None;
            self.selected_operation = None;
        }
    }

    fn set_selected_operation(&mut self, operation_id: OperationId) {
        if self.selected_operation != Some(operation_id) {
            self.selected_operation = Some(operation_id);
            self.reveal_history_operation(operation_id);
        }
    }

    fn reveal_history_operation(&mut self, operation_id: OperationId) {
        if let Some(index) = self.document.as_ref().and_then(|document| {
            document.operation_tree().position(|operation| operation.id == operation_id)
        }) {
            self.history_page = index / HISTORY_PAGE_SIZE;
        }
    }

    fn select_latest_operation(&mut self) {
        if let Some(operation_id) = self.document.as_ref()
            .and_then(|document| document.operation_tree().last().map(|operation| operation.id))
        {
            self.set_selected_operation(operation_id);
        }
    }

    fn sync_viewport_selection(&mut self, cx: &mut Cx) {
        if let Some(mut viewport) = self.viewport_widget(cx).borrow_mut::<CadViewport>() {
            let selected_id = self.selected_element.map(|id| id.get());
            let _ = viewport.viewer_mut().select(selected_id, false);
            viewport.redraw(cx);
        }
    }

    fn select_operation(&mut self, cx: &mut Cx, operation_id: OperationId) {
        let Some((output_element, property_target, label)) = self.document.as_ref()
            .and_then(|document| document.operation(operation_id))
            .map(|operation| {
                let property_target = match &operation.kind {
                    OperationKind::PropertySet { element_id, .. } => Some(*element_id),
                    _ => None,
                };
                (operation.outputs.first().copied(), property_target, operation.label.clone())
            }) else {
                return;
            };
        self.set_selected_operation(operation_id);
        self.selected_element = output_element.or(property_target).filter(|id| {
            self.document.as_ref().is_some_and(|document| document.element(*id).is_some())
        });
        self.semantic_selection = self.selected_element.map(EntityRef::Element);
        self.sync_viewport_selection(cx);
        self.refresh_inspector(cx);
        self.refresh_history(cx);
        self.refresh_object_tree(cx);
        self.set_status(cx, &format!("Выбрана операция #{}: {}", operation_id.get(), label));
    }

    fn refresh_scene(&mut self, cx: &mut Cx, fit: bool) {
        self.circular_after_mutation();
        if !self.circular_native_commit_in_progress() { self.refresh_circular(cx); }
        self.refresh_scene_incremental(cx, fit);
        self.drafting_ui.scene_epoch = self.api_document_generation;
        self.drafting_ui.scene_revision = self.document.as_ref().map_or(0, Document::revision);
        self.refresh_drafting(cx, true);
    }
    fn refresh_history(&mut self, cx: &mut Cx) {
        let rows: Vec<(HistoryRow, String, String)> = self
            .document
            .as_ref()
            .map(|document| {
                document
                    .operation_tree()
                    .map(|operation| {
                        let indent = if operation.parent.is_some() { "↳ " } else { "" };
                        let action = match &operation.kind {
                            OperationKind::GeoFoundationChanged { .. } => "GEO / Foundation · изменение".to_owned(),
                            OperationKind::StructuralConfiguratorChanged { command } => match command {
                                ai_revit_core::StructuralConfiguratorCommand::UpdateBuildingAndRegenerate { .. } => "Стальное здание · параметры и генерация".to_owned(),
                                ai_revit_core::StructuralConfiguratorCommand::SaveAnalysisSnapshot { .. } => "Стальное здание · результат расчёта".to_owned(),
                            },
                            OperationKind::FoundationStructuralChanged { command } => match command {
                                ai_revit_core::FoundationStructuralCommand::ReplaceInput { .. } => "Конструкции и фундамент · параметры".to_owned(),
                                ai_revit_core::FoundationStructuralCommand::SaveAnalysisSnapshot { .. } => "Конструкции и фундамент · результат расчёта".to_owned(),
                            },
                            OperationKind::ClassDefined { name, .. } => format!("Класс «{name}»"),
                            OperationKind::PropertyDefined { name, .. } => format!("Свойство «{name}»"),
                            OperationKind::ElementCreated { element_id, element_type, name, .. } => {
                                let kind = match element_type {
                                    ModelElementType::Wall => "Стена",
                                    ModelElementType::Floor => "Перекрытие",
                                    ModelElementType::Roof => "Кровля",
                                    ModelElementType::Opening => "Проём",
                                    ModelElementType::Door => "Дверь",
                                    ModelElementType::Window => "Окно",
                                    ModelElementType::Space => "Пространство",
                                };
                                format!("{kind} «{name}» · #{}", element_id.get())
                            }
                            OperationKind::PropertySet { element_id, property_id, .. } => {
                                format!("Свойство {} · #{}", property_id, element_id.get())
                            }
                            OperationKind::ElementDeleted { element_id, name, .. } => {
                                format!("Удаление «{name}» · #{}", element_id.get())
                            }
                            OperationKind::BimChanged { action } => format!("Изменение BIM · {action}"),
                            OperationKind::DraftingChanged { command } => format!("CAD · {}", drafting_workspace::command_label(command)),
                            OperationKind::EngineeringChanged { analysis_id, .. } => format!("Расчёт · #{}", analysis_id.get()),
                            OperationKind::EngineeringGroundChanged { ground_section_id, .. } => format!("Грунтовый разрез · #{}", ground_section_id.get()),
                        };
                        let marker = if self.selected_operation == Some(operation.id) { "▶ " } else { "" };
                        (
                            HistoryRow {
                                operation_id: operation.id,
                            },
                            format!("{marker}{indent}#{} · {}", operation.id.get(), compact_history_text(&action, 34)),
                            action,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let page_count = ((rows.len() + HISTORY_PAGE_SIZE - 1) / HISTORY_PAGE_SIZE).max(1);
        self.history_page = self.history_page.min(page_count - 1);
        let start = self.history_page * HISTORY_PAGE_SIZE;
        self.history_rows = rows.iter().skip(start).take(HISTORY_PAGE_SIZE).map(|(row, _, _)| *row).collect();
        for (index, row_id) in HISTORY_ROW_IDS.iter().copied().enumerate() {
            let widget = self.ui.widget(cx, &[row_id]);
            if let Some((_, label, _)) = rows.get(start + index) {
                widget.set_text(cx, label);
                widget.set_visible(cx, true);
            } else {
                widget.set_visible(cx, false);
            }
        }
        self.ui.label(cx, ids!(history_page_label)).set_text(cx, &format!(
            "стр. {}/{}", self.history_page + 1, page_count,
        ));
        self.ui.button(cx, ids!(history_previous)).set_enabled(cx, self.history_page > 0);
        self.ui.button(cx, ids!(history_next)).set_enabled(cx, self.history_page + 1 < page_count);
        self.ui.button(cx, ids!(history_recent)).set_enabled(cx, self.history_page + 1 < page_count);
        let hint = self.selected_operation.and_then(|selected| {
            rows.iter().enumerate().find(|(_, (row, _, _))| row.operation_id == selected)
        }).map_or_else(
            || "Операции документа · выберите строку".to_owned(),
            |(index, (row, _, action))| format!(
                "Выбрана #{} · {} · стр. {}/{}",
                row.operation_id.get(), compact_history_text(action, 28),
                index / HISTORY_PAGE_SIZE + 1, page_count,
            ),
        );
        let range = format!(
            "{}–{} из {} операций",
            if rows.is_empty() { 0 } else { start + 1 },
            (start + HISTORY_PAGE_SIZE).min(rows.len()), rows.len(),
        );
        self.ui.label(cx, ids!(history_hint)).set_text(cx, &format!("{range}\n{hint}"));
    }

    fn refresh_inspector(&mut self, cx: &mut Cx) {
        self.refresh_semantic_inspector(cx);
        self.refresh_opening_inspector(cx);
        self.refresh_quantity_inspector(cx);
        self.refresh_property_editor(cx);
        self.refresh_custom_property_editor(cx);
        let Some(document) = self.document.as_ref() else {
            return;
        };
        self.ui.label(cx, ids!(inspector_heading)).set_text(cx, "Текущий выбор");
        let selected_element = self.selected_element.and_then(|id| document.element(id).map(|element| (id, element)));
        let Some((id, element)) = selected_element else {
            if self.refresh_non_element_selection(cx, document) {
                return;
            }
            if let Some(operation) = self.selected_operation.and_then(|id| document.operation(id)) {
                let detail = match &operation.kind {
                    OperationKind::GeoFoundationChanged { .. } => "Изменены данные геологии, размещения или результата фундамента".to_owned(),
                    OperationKind::StructuralConfiguratorChanged { command } => match command {
                        ai_revit_core::StructuralConfiguratorCommand::UpdateBuildingAndRegenerate { .. } => "Изменена конфигурация и сохранена генерация стального здания".to_owned(),
                        ai_revit_core::StructuralConfiguratorCommand::SaveAnalysisSnapshot { .. } => "Сохранён результат расчёта стального здания для указанной области".to_owned(),
                    },
                    OperationKind::FoundationStructuralChanged { command } => match command {
                        ai_revit_core::FoundationStructuralCommand::ReplaceInput { .. } => "Изменены параметры конструкций и фундамента".to_owned(),
                        ai_revit_core::FoundationStructuralCommand::SaveAnalysisSnapshot { .. } => "Сохранён результат расчёта конструкций и фундамента".to_owned(),
                    },
                    OperationKind::ClassDefined { name, .. } => format!("Класс: {name}"),
                    OperationKind::PropertyDefined { name, .. } => format!("Свойство: {name}"),
                    OperationKind::ElementCreated { element_id, element_type, name, .. } => {
                        let kind = match element_type {
                            ModelElementType::Wall => "Стена",
                            ModelElementType::Floor => "Перекрытие",
                            ModelElementType::Roof => "Кровля",
                            ModelElementType::Opening => "Проём",
                            ModelElementType::Door => "Дверь",
                            ModelElementType::Window => "Окно",
                            ModelElementType::Space => "Пространство",
                        };
                        format!("Создано: {kind} «{name}» · элемент #{}", element_id.get())
                    }
                    OperationKind::PropertySet { element_id, property_id, .. } => {
                        format!("Изменено свойство {property_id} · элемент #{}", element_id.get())
                    }
                    OperationKind::ElementDeleted { element_id, name, .. } => {
                        format!("Удалено: {name} · элемент #{}", element_id.get())
                    }
                    OperationKind::BimChanged { action } => format!("Изменение BIM: {action}"),
                    OperationKind::DraftingChanged { command } => format!("CAD: {}", drafting_workspace::command_label(command)),
                    OperationKind::EngineeringChanged { analysis_id, .. } => format!("Расчёт: #{}", analysis_id.get()),
                    OperationKind::EngineeringGroundChanged { ground_section_id, .. } => format!("Грунтовый разрез: #{}", ground_section_id.get()),
                };
                self.ui.label(cx, ids!(inspector_heading)).set_text(cx, "Выбранная операция");
                self.ui.label(cx, ids!(selected_label)).set_text(cx, &format!(
                    "{} · #{}", operation.label, operation.id.get(),
                ));
                self.ui.label(cx, ids!(selection_status)).set_text(cx, &format!(
                    "Выбрана операция #{}", operation.id.get(),
                ));
                let parent = operation.parent.map_or_else(|| "нет".to_owned(), |parent| format!("#{}", parent.get()));
                self.ui.label(cx, ids!(property_summary)).set_text(cx, &format!(
                    "{detail}\nID операции: {}\nРодитель: {parent}", operation.id.get(),
                ));
                self.ui.button(cx, ids!(set_property)).set_enabled(cx, false);
                return;
            }
            self.ui.label(cx, ids!(selected_label)).set_text(cx, "Ничего не выбрано");
            self.ui
                .label(cx, ids!(selection_status))
                .set_text(cx, "Элемент не выбран");
            self.ui
                .label(cx, ids!(property_summary))
                .set_text(cx, "Выберите элемент в истории или 3D-виде.");
            self.ui.button(cx, ids!(set_property)).set_enabled(cx, false);
            return;
        };
        let effective = document.effective_element(id);
        let kind = match effective.as_ref().map(|effective| &effective.kind) {
            Ok(ElementKind::Wall(spec)) => format!(
                "Стена · высота {:.2} м · толщина {:.2} м",
                spec.height_m, spec.thickness_m
            ),
            Ok(ElementKind::Floor(spec)) => format!(
                "Перекрытие · толщина {:.2} м · отметка {:.2} м",
                spec.thickness_m, spec.elevation_m
            ),
            Ok(ElementKind::Roof(spec)) => format!(
                "Кровля · толщина {:.2} м · отметка {:.2} м",
                spec.thickness_m, spec.elevation_m
            ),
            Ok(kind) => format!("{} «{}»", category_label(element_kind_category(kind)), element.name),
            Err(error) => format!("Не удалось вычислить параметры: {error}"),
        };
        self.ui
            .label(cx, ids!(selected_label))
            .set_text(cx, &format!("{} · #{}", kind, id.get()));
        self.ui
            .label(cx, ids!(selection_status))
            .set_text(cx, &format!("Выбран элемент #{}", id.get()));
        let properties = element.properties.iter()
            .map(|(key, value)| format!("{key}: {}", format_property_value(value)))
            .collect::<Vec<_>>()
            .join("\n");
        let class_name = document.class(&element.class_id).map_or_else(
            || element.class_id.to_string(),
            |class| format!("{} ({})", class.name, class.id),
        );
        let selected_operation = self.selected_operation
            .filter(|operation_id| *operation_id != element.created_by)
            .and_then(|operation_id| document.operation(operation_id))
            .map_or_else(String::new, |operation| format!(
                "Выбрана операция: {} · #{}\n", operation.label, operation.id.get(),
            ));
        self.ui.label(cx, ids!(property_summary)).set_text(cx, &format!(
            "Класс: {class_name}\nID элемента: {}\nСоздан операцией: {}\n{selected_operation}\n{}",
            id.get(), element.created_by.get(),
            if properties.is_empty() { "Нет свойств экземпляра" } else { &properties }
        ));
        self.ui.button(cx, ids!(set_property)).set_enabled(cx, true);
    }

    fn refresh_non_element_selection(&self, cx: &mut Cx, document: &Document) -> bool {
        match self.semantic_selection {
            Some(EntityRef::Level(id)) => {
                let Some(level) = document.level(id) else { return false; };
                let label = format!("{} · уровень #{}", level.name, id.get());
                self.ui.label(cx, ids!(inspector_heading)).set_text(cx, "Уровень");
                self.ui.label(cx, ids!(selected_label)).set_text(cx, &label);
                self.ui.label(cx, ids!(selection_status)).set_text(cx, &format!("Выбран уровень #{}", id.get()));
                self.ui.label(cx, ids!(property_summary)).set_text(cx, &format!(
                    "Уровень: {}\nID: {}\nОтметка: {:.2} м\nВысота: {}",
                    level.name, id.get(), level.elevation_m,
                    level.height_m.map_or_else(|| "не задана".to_owned(), |height| format!("{height:.2} м")),
                ));
                self.ui.button(cx, ids!(set_property)).set_enabled(cx, false);
                true
            }
            Some(EntityRef::Type(id)) => {
                let Some(element_type) = document.element_type(id) else { return false; };
                let class_name = document.class(&element_type.class_id)
                    .map_or_else(|| element_type.class_id.to_string(), |class| class.name.clone());
                let label = format!("{} · тип #{}", element_type.name, id.get());
                self.ui.label(cx, ids!(inspector_heading)).set_text(cx, "Тип");
                self.ui.label(cx, ids!(selected_label)).set_text(cx, &label);
                self.ui.label(cx, ids!(selection_status)).set_text(cx, &format!("Выбран тип #{}", id.get()));
                self.ui.label(cx, ids!(property_summary)).set_text(cx, &format!(
                    "Тип: {}\nID: {}\nКласс: {}\nЭкземпляров: {}",
                    element_type.name, id.get(), class_name, document.type_instances(id).len(),
                ));
                self.ui.button(cx, ids!(set_property)).set_enabled(cx, false);
                true
            }
            _ => false,
        }
    }

    fn refresh_quantity_inspector(&mut self, cx: &mut Cx) {
        let element_id = self.selected_element;
        let record = element_id.and_then(|id| self.geometry_cache.record(id));
        let record_value = record.and_then(|record| serde_json::to_value(record).ok());
        let valid = record.is_some_and(|record| record.valid);
        let quantities = record_value.as_ref().filter(|_| valid)
            .and_then(|record| record.get("quantities")).cloned().unwrap_or(Value::Null);
        let quality = record_value.as_ref().and_then(|record| record.get("representation_quality"))
            .and_then(Value::as_str).unwrap_or("unavailable");
        let status = if element_id.is_none() {
            "Выберите элемент.".to_owned()
        } else if !valid {
            record_value.as_ref().and_then(|record| record.get("failure"))
                .filter(|failure| !failure.is_null())
                .map_or_else(|| "Геометрия не рассчитана.".to_owned(), |failure| format!("Ошибка геометрии: {}",failure.get("message").and_then(Value::as_str).unwrap_or("показатели недоступны")))
        } else if quantities.is_null() {
            "Количество для этой геометрии не рассчитано.".to_owned()
        } else {
            format!("Элемент #{} · геометрия #{}", element_id.expect("selected record").get(),
                record_value.as_ref().and_then(|record| record.get("geometry_revision")).and_then(Value::as_u64).unwrap_or(0))
        };
        self.quantity_ui_details = json!({
            "element_id":element_id.map(ElementId::get), "available":valid && !quantities.is_null(),
            "quality":quality, "quantities":quantities,
            "geometry_revision":record_value.as_ref().and_then(|record|record.get("geometry_revision")),
            "geometry_fingerprint":record_value.as_ref().and_then(|record|record.get("geometry_fingerprint")),
            "failure":record_value.as_ref().and_then(|record|record.get("failure")),
        });
        let quantities = &self.quantity_ui_details["quantities"];
        self.ui.label(cx,ids!(quantities_quality)).set_text(cx, match quality {
            "approximate_mesh" => "Approximate mesh · приближённая сетка",
            _ => "Качество геометрии: недоступно",
        });
        for (path, title, key, unit) in [
            (ids!(quantities_gross_volume),"Объём брутто","gross_volume","м³"),
            (ids!(quantities_net_volume),"Объём нетто","net_volume","м³"),
            (ids!(quantities_opening_area),"Площадь проёмов","opening_area","м²"),
        ] {
            self.ui.label(cx,path).set_text(cx,&format!("{title}: {}", quantity_text(quantities,key,unit)));
        }
        self.ui.label(cx,ids!(quantities_surface_area)).set_text(cx,&format!(
            "Поверхность брутто: {}\nПоверхность нетто: {}",
            quantity_text(quantities,"gross_surface_area","м²"), quantity_text(quantities,"net_surface_area","м²"),
        ));
        self.ui.label(cx,ids!(quantities_status)).set_text(cx,&status);
    }

    fn refresh_property_editor(&mut self, cx: &mut Cx) {
        let owner = match (self.semantic_selection, self.selected_element) {
            (Some(EntityRef::Type(id)), _) => Some(EntityRef::Type(id)),
            (_, Some(id)) => Some(EntityRef::Element(id)),
            _ => None,
        };
        let element_id = match owner { Some(EntityRef::Element(id)) => Some(id), _ => None };
        let document = self.document.as_ref();
        let element = element_id.and_then(|id| document?.element(id));
        let type_id = match owner {
            Some(EntityRef::Type(id)) => Some(id),
            _ => element.and_then(|element| element.type_id),
        };
        let element_type = type_id.and_then(|id| document?.element_type(id));
        let class_id = element.map(|element| &element.class_id)
            .or_else(|| element_type.map(|element_type| &element_type.class_id));
        let resolved = element_id.and_then(|id| document?.resolved_properties(id).ok());
        // A bounded editor for primary parameters; existing custom-property UI
        // remains available independently of this list.
        self.property_ui_options = ["height","thickness","width","depth","rotation","slope_x","slope_y","material"]
            .into_iter().filter_map(|key| {
                let property_id = PropertyId::new(key);
                let definition = document?.property_definition(class_id?, &property_id)?;
                let resolved_property = resolved.as_ref().and_then(|values| values.get(&property_id));
                let type_value = element_type.and_then(|element_type|element_type.properties.get(&property_id)).cloned();
                Some(PropertyUiOption {
                    property_id:property_id.clone(), label:basic_property_label(key,definition.value_type),
                    value_type:definition.value_type,
                    resolved_value:resolved_property.map(|property|property.value.clone()).or_else(||type_value.clone()).or_else(||definition.default_value.clone()),
                    resolved_source:resolved_property.map(|property|property.source).or_else(||type_value.as_ref().map(|_|PropertySource::Type)).or_else(||definition.default_value.as_ref().map(|_|PropertySource::ClassDefault)),
                    instance_override:element.and_then(|element|element.properties.get(&property_id)).cloned(),
                    type_value, type_default_value:definition.default_value.clone(),
                })
            }).collect();
        let chosen = self.property_ui_property.as_ref()
            .and_then(|id|self.property_ui_options.iter().position(|option|&option.property_id==id)).unwrap_or(0);
        let option = self.property_ui_options.get(chosen).cloned();
        let snapshot = option.as_ref().map_or(Value::Null, |option|json!({
            "owner":owner.map(semantic_entity_value),"property_id":option.property_id,
            "resolved_value":option.resolved_value,"source":option.resolved_source,
            "instance_override":option.instance_override,"type_value":option.type_value,"type_default_value":option.type_default_value,"type_id":type_id.map(TypeId::get),
        }));
        let target = owner.zip(option.as_ref()).map(|(owner, option)| EditorTarget::Primary {
            owner, element_id, type_id, property_id: option.property_id.clone(),
        });
        let refresh = self.property_editor.refresh(document, self.api_document_generation, target);
        if refresh == EditorRefresh::Reset || (refresh == EditorRefresh::Update && (self.property_ui_owner != owner || self.property_ui_snapshot != snapshot)) {
            self.ui.text_input(cx,ids!(instance_override_value)).set_text(cx,&option.as_ref()
                .and_then(|option|option.resolved_value.as_ref()).map(property_input_text).unwrap_or_default());
            self.ui.text_input(cx,ids!(type_property_value)).set_text(cx,&option.as_ref()
                .and_then(|option|option.type_value.as_ref().or(option.type_default_value.as_ref())).map(property_input_text).unwrap_or_default());
        }
        self.property_ui_owner = owner;
        self.property_ui_element = element_id;
        self.property_ui_type = element_type.map(|element_type|element_type.id);
        self.property_ui_property = option.as_ref().map(|option|option.property_id.clone());
        self.property_ui_snapshot = snapshot;
        let labels = if self.property_ui_options.is_empty() { vec!["Нет основных параметров".to_owned()] }
            else { self.property_ui_options.iter().map(|option|option.label.clone()).collect() };
        let selector = self.ui.drop_down(cx,ids!(editable_property_selector));
        selector.set_labels(cx,labels);
        selector.set_selected_item(cx,chosen);
        let mut source = option.as_ref().map_or_else(||"Выберите экземпляр или тип с параметрами.".to_owned(),|option| {
            let source = match option.resolved_source {
                Some(PropertySource::InstanceOverride) => "экземпляр".to_owned(),
                Some(PropertySource::Type) => format!("тип #{}",type_id.map(TypeId::get).unwrap_or(0)),
                Some(PropertySource::ClassDefault) => "класс".to_owned(),
                None => "значение не задано".to_owned(),
            };
            format!("Вычислено: {}\nИсточник: {source}",option.resolved_value.as_ref()
                .map(format_property_value).unwrap_or_else(||"—".to_owned()))
        });
        if self.property_editor.stale(document, self.api_document_generation) {
            source.push_str("\nВвод относится к предыдущей ревизии · выберите свойство заново.");
        }
        self.ui.label(cx,ids!(editable_property_source)).set_text(cx,&source);
        self.ui.button(cx,ids!(apply_instance_override)).set_enabled(cx,element.is_some() && option.is_some());
        self.ui.button(cx,ids!(clear_instance_override)).set_enabled(cx,element.is_some() && option.as_ref().is_some_and(|option|option.instance_override.is_some()));
        self.ui.button(cx,ids!(apply_type_property)).set_enabled(cx,element_type.is_some() && option.is_some());
    }

    fn refresh_custom_property_editor(&mut self, cx: &mut Cx) {
        let target = self.selected_element.and_then(|id| {
            let element = self.document.as_ref()?.element(id)?;
            Some(EditorTarget::Custom { element_id: id, class_id: element.class_id.clone() })
        });
        let enabled = target.is_some();
        if self.custom_property_editor.refresh(self.document.as_ref(), self.api_document_generation, target) == EditorRefresh::Reset {
            self.ui.text_input(cx, ids!(property_name)).set_text(cx, "Огнестойкость");
            self.ui.text_input(cx, ids!(property_value)).set_text(cx, "60 мин");
        }
        self.ui.button(cx, ids!(set_property)).set_enabled(cx, enabled);
    }

    fn primary_editor_target(&self) -> Option<EditorTarget> {
        Some(EditorTarget::Primary {
            owner: self.property_ui_owner?, element_id: self.property_ui_element,
            type_id: self.property_ui_type, property_id: self.property_ui_property.clone()?,
        })
    }

    fn select_editable_property(&mut self, cx: &mut Cx, property_id: &PropertyId) -> Result<Value,String> {
        if !self.property_ui_options.iter().any(|option|&option.property_id==property_id) {
            return Err(format!("property {property_id} is not an available primary parameter"));
        }
        self.property_editor.clear();
        self.property_ui_property = Some(property_id.clone());
        self.refresh_property_editor(cx);
        Ok(json!({"property_id":property_id,"document_mutated":false}))
    }

    /// Configure the same bounded property form used by native button actions.
    /// Accepts property_id and optional instance_value/type_value in display units.
    pub(crate) fn configure_property_editor(&mut self,cx:&mut Cx,params:&Value)->Result<Value,String> {
        let property_id = params.get("property_id").map(|value|value.as_str()
            .map(PropertyId::new).ok_or("property_id must be a string"))
            .transpose()?.or_else(||self.property_ui_property.clone()).ok_or("select a primary parameter")?;
        let option = self.property_ui_options.iter().find(|option|option.property_id==property_id)
            .cloned().ok_or("property is not available in the current form")?;
        let mut updates = Vec::new();
        for (key,path) in [("instance_value",ids!(instance_override_value)),("type_value",ids!(type_property_value))] {
            if let Some(value) = params.get(key) {
                let text = match value {
                    Value::String(text) => text.clone(),
                    Value::Number(_) | Value::Bool(_) => value.to_string(),
                    _ => return Err(format!("{key} must be text, a number or a boolean")),
                };
                parse_property_input(&text,option.value_type)?;
                updates.push((path,text));
            }
        }
        self.select_editable_property(cx,&property_id)?;
        if !updates.is_empty() { self.property_editor.dirty = true; }
        for (path,text) in updates { self.ui.text_input(cx,path).set_text(cx,&text); }
        Ok(json!({"configured":true,"document_mutated":false,"ui":self.semantic_ui_state(cx)}))
    }

    fn apply_property_editor(&mut self,cx:&mut Cx,action:&str)->Result<Value,String> {
        let target = self.primary_editor_target().ok_or("select a primary parameter")?;
        let context = self.property_editor.context.clone().ok_or("stale_ui_intent: выберите BIM-свойство заново")?;
        context.require_current(self.document.as_ref().ok_or("no document is open")?, self.api_document_generation, &target)?;
        let property_id = self.property_ui_property.clone().ok_or("select a primary parameter")?;
        let option = self.property_ui_options.iter().find(|option|option.property_id==property_id)
            .cloned().ok_or("selected primary parameter is no longer available")?;
        let command = match action {
            "instance" => Command::SetProperty {
                element_id:self.property_ui_element.ok_or("select an instance")?,property_id:property_id.clone(),
                value:parse_property_input(&self.ui.text_input(cx,ids!(instance_override_value)).text(),option.value_type)?,
            },
            "clear" => {
                if option.instance_override.is_none() { return Err("the instance has no override to clear".to_owned()); }
                Command::ClearPropertyOverride {element_id:self.property_ui_element.ok_or("select an instance")?,property_id:property_id.clone()}
            },
            "type" => {
                let type_id = self.property_ui_type.ok_or("select a type or an instance with a type")?;
                let properties = BTreeMap::from([(property_id.clone(),parse_property_input(&self.ui.text_input(cx,ids!(type_property_value)).text(),option.value_type)?)]);
                Command::UpdateType {type_id,name:None,properties}
            },
            _ => return Err("unknown property editing action".to_owned()),
        };
        let command_id = self.next_command_id("edit-primary-property");
        let receipt = {
            let document = self.document.as_mut().ok_or("no document is open")?;
            commit_editor_commands(document, self.api_document_generation, &context, &target, command_id, vec![command])?
        };
        self.property_editor.clear();
        self.property_ui_snapshot = Value::Null;
        let mut result = serde_json::to_value(receipt).map_err(|error|error.to_string())?;
        self.select_latest_operation();
        self.refresh_scene(cx,false);
        if let Some(result) = result.as_object_mut() {
            result.insert("property_id".to_owned(),json!(property_id));
            result.insert("property_action".to_owned(),json!(action));
            result.insert("geometry_rebuild".to_owned(),self.rebuild_observation());
        }
        Ok(result)
    }

    fn refresh_opening_inspector(&mut self, cx: &mut Cx) {
        let selected_id = self.selected_element;
        let mut rows = Vec::new();
        let mut count = 0;
        let mut summary = "Выберите стену или проём.".to_owned();
        let mut details = json!({"kind":"none"});
        let mut can_opening = false;
        let mut can_fill = false;
        let mut wall_id = None;
        let mut camera_host_id = None;
        let mut input_defaults = None;
        let mut input_geometry = None;
        if let (Some(document), Some(id)) = (self.document.as_ref(), selected_id) {
            wall_id = ui_wall_host(document, id);
            camera_host_id = ui_geometry_host(document,id);
            match document.effective_element(id) {
                Ok(element) => match &element.kind {
                    ElementKind::Wall(wall) => {
                        can_opening = true;
                        input_geometry = Some(ElementKind::Wall(wall.clone()));
                        let openings = document.hosted_opening_ids(id);
                        count = openings.len();
                        for opening_id in &openings {
                            rows.push(OpeningUiRow { element_id: *opening_id, opening_id: Some(*opening_id), role: "opening", label: format!("Проём #{}", opening_id.get()) });
                            for child in ui_hosted_children(document, *opening_id) {
                                if let Some(e) = document.element(child).filter(|e| matches!(&e.kind, ElementKind::Window(_) | ElementKind::Door(_))) {
                                    rows.push(OpeningUiRow { element_id: child, opening_id: Some(*opening_id), role: "hosted_element", label: format!("  #{} → {} #{}", opening_id.get(), category_label(element_kind_category(&e.kind)), child.get()) });
                                }
                            }
                        }
                        summary = format!("Проёмы: {count}");
                        details = json!({"kind":"wall","element_id":id.get(),"wall_id":id.get(),"opening_ids":openings.iter().map(|id|id.get()).collect::<Vec<_>>(),"opening_count":count});
                        let length = (wall.end.x-wall.start.x).hypot(wall.end.y-wall.start.y);
                        input_defaults = Some(("1.2".to_owned(), "1.5".to_owned(), String::new(), "0.9".to_owned(), format!("Центр от начала стены (0…{length:.2} м); низ от её основания.")));
                    }
                    ElementKind::Floor(_) | ElementKind::Roof(_) => {
                        let openings = document.hosted_opening_ids(id);
                        count = openings.len();
                        for opening_id in &openings {
                            rows.push(OpeningUiRow {element_id:*opening_id,opening_id:Some(*opening_id),role:"opening",label:format!("Проём #{}",opening_id.get())});
                            for child in ui_hosted_children(document,*opening_id) {
                                if let Some(element) = document.element(child) {
                                    rows.push(OpeningUiRow {element_id:child,opening_id:Some(*opening_id),role:"hosted_element",label:format!("  #{} → {} #{}",opening_id.get(),category_label(element_kind_category(&element.kind)),child.get())});
                                }
                            }
                        }
                        summary = format!("Проёмы: {count}");
                        details = json!({"kind":if matches!(&element.kind,ElementKind::Floor(_)){"floor"}else{"roof"},"element_id":id.get(),"host_id":id.get(),"opening_count":count,"opening_ids":openings.iter().map(|id|id.get()).collect::<Vec<_>>()});
                    }
                    ElementKind::Opening(spec) => {
                        can_fill = true;
                        input_geometry = Some(ElementKind::Opening(spec.clone()));
                        let host_id = ui_host_of(document, id);
                        let children = ui_hosted_children(document, id);
                        if let Some(host) = host_id {
                            rows.push(OpeningUiRow { element_id: host, opening_id: Some(id), role: "host", label: format!("{} / Host #{}",host_role_label(document,host),host.get()) });
                        }
                        for child in &children {
                            if let Some(e) = document.element(*child) {
                                rows.push(OpeningUiRow { element_id: *child, opening_id: Some(id), role: "hosted_element", label: format!("Проём #{} → {} #{}", id.get(), category_label(element_kind_category(&e.kind)), child.get()) });
                            }
                        }
                        let mut along = None;
                        let mut bottom = None;
                        if let Some(host) = host_id.and_then(|host| document.effective_element(host).ok()) {
                            if let ElementKind::Wall(wall) = host.kind {
                                let dx = wall.end.x-wall.start.x;
                                let dy = wall.end.y-wall.start.y;
                                let length = dx.hypot(dy);
                                if length > 0.0 { along = Some(((spec.center.x-wall.start.x)*dx+(spec.center.y-wall.start.y)*dy)/length); }
                                bottom = Some(spec.elevation_m-wall.base_elevation_m);
                            }
                        }
                        let status = self.opening_geometry_status(id);
                        summary = format!("Host: {}\nШирина: {:.3} м · высота: {:.3} м\nСмещение центра: {} · низ: {}\nЗаполнение: {}\nГеометрия: {status}",
                            host_id.map_or_else(|| "не задан".to_owned(), |id|format!("#{}",id.get())), spec.width_m, spec.height_m,
                            along.map_or_else(|| "—".to_owned(), |n|format!("{n:.3} м")), bottom.map_or_else(|| "—".to_owned(), |n|format!("{n:.3} м")),
                            if children.is_empty() { "нет".to_owned() } else { children.iter().map(|id|format!("#{}",id.get())).collect::<Vec<_>>().join(", ") });
                        if wall_id.is_none() {
                            summary = format!("Host: {}\nПлан: {:.3} × {:.3} м\nВысота выреза: {:.3} м · низ: {:.3} м\nЦентр XY: {:.3}; {:.3} м\nГеометрия: {status}",
                                host_id.map_or_else(||"не задан".to_owned(),|id|format!("#{}",id.get())),spec.width_m,spec.depth_m,spec.height_m,spec.elevation_m,spec.center.x,spec.center.y);
                        }
                        details = json!({"kind":"opening","element_id":id.get(),"wall_id":wall_id.map(ElementId::get),"host_id":host_id.map(ElementId::get),"width_m":spec.width_m,"height_m":spec.height_m,"depth_m":spec.depth_m,"offset_from_wall_start_m":along,"bottom_offset_m":bottom,"hosted_element_ids":children.iter().map(|id|id.get()).collect::<Vec<_>>(),"geometry_status":status});
                        input_defaults = Some((spec.width_m.to_string(), spec.height_m.to_string(), "0".to_owned(), "0".to_owned(), "Смещение от центра проёма; низ от нижней границы проёма.".to_owned()));
                    }
                    ElementKind::PolygonalOpening(spec) => {
                        can_fill = true;
                        input_geometry = Some(ElementKind::PolygonalOpening(spec.clone()));
                        input_defaults = Some(("1.2".to_owned(),"1.5".to_owned(),"0".to_owned(),"0".to_owned(),"Сдвиг от точки привязки проёма; низ от его отметки.".to_owned()));
                        let host_id = ui_host_of(document,id);
                        let children = ui_hosted_children(document,id);
                        if let Some(host) = host_id {
                            rows.push(OpeningUiRow {element_id:host,opening_id:Some(id),role:"host",label:format!("{} / Host #{}",host_role_label(document,host),host.get())});
                        }
                        for child in &children {
                            rows.push(OpeningUiRow {element_id:*child,opening_id:Some(id),role:"hosted_element",label:format!("Проём #{} → #{}",id.get(),child.get())});
                        }
                        let status = self.opening_geometry_status(id);
                        summary = format!("Полигональный проём · вершин: {}\nHost: {}\nГлубина: {:.3} м · поворот: {:.3} рад\nНиз: {:.3} м\nГеометрия: {status}",spec.boundary.len(),host_id.map_or_else(||"не задан".to_owned(),|id|format!("#{}",id.get())),spec.depth_m,spec.rotation_rad,spec.elevation_m);
                        details = json!({"kind":"polygonal_opening","element_id":id.get(),"host_id":host_id.map(ElementId::get),"wall_id":wall_id.map(ElementId::get),"boundary":spec.boundary,"vertex_count":spec.boundary.len(),"depth_m":spec.depth_m,"rotation_rad":spec.rotation_rad,"elevation_m":spec.elevation_m,"hosted_element_ids":children.iter().map(|id|id.get()).collect::<Vec<_>>(),"geometry_status":status});
                    }
                    ElementKind::Window(_) | ElementKind::Door(_) => {
                        if let Some(host) = ui_host_of(document, id) {
                            rows.push(OpeningUiRow { element_id: host, opening_id: Some(host), role: "host", label: format!("Проём / Host #{}", host.get()) });
                            summary = format!("Заполнение проёма #{}", host.get());
                            details = json!({"kind":"hosted_element","element_id":id.get(),"host_id":host.get(),"wall_id":wall_id.map(ElementId::get)});
                        }
                    }
                    _ => {}
                },
                Err(error) => summary = format!("Не удалось вычислить параметры: {error}"),
            }
        }
        // Preserve draft inputs across metadata/display refreshes, but refresh
        // their defaults when the selected host's effective geometry changes.
        if self.opening_input_owner != selected_id
            || self.opening_input_geometry != input_geometry
        {
            self.opening_input_owner = selected_id;
            self.opening_input_geometry = input_geometry;
            self.opening_ui_page = 0;
            if let Some((width, height, offset, sill, hint)) = input_defaults {
                for (path, text) in [(ids!(opening_width), width), (ids!(opening_height), height), (ids!(opening_offset), offset), (ids!(opening_sill), sill)] {
                    self.ui.text_input(cx, path).set_text(cx, &text);
                }
                self.ui.label(cx, ids!(opening_input_hint)).set_text(cx, &hint);
            }
        }
        self.opening_ui_rows = rows;
        self.opening_ui_count = count;
        self.opening_ui_details = details;
        self.ui.label(cx, ids!(opening_summary)).set_text(cx, &summary);
        self.ui.button(cx, ids!(add_opening)).set_enabled(cx, can_opening);
        self.ui.button(cx, ids!(add_window)).set_enabled(cx, can_fill);
        self.ui.button(cx, ids!(add_door)).set_enabled(cx, can_fill);
        self.ui.widget(cx, ids!(opening_create_inputs)).set_visible(cx, can_opening || can_fill);
        self.ui.label(cx, ids!(opening_offset_label)).set_text(cx, if can_opening { "Центр, м" } else { "Сдвиг, м" });
        self.ui.button(cx, ids!(opening_wall_front)).set_enabled(cx, wall_id.is_some());
        self.ui.widget(cx,ids!(opening_host_view)).set_visible(cx,camera_host_id.is_some() && wall_id.is_none());
        self.ui.button(cx,ids!(opening_host_view)).set_enabled(cx,camera_host_id.is_some());
        self.ui.widget(cx, ids!(show_opening_volumes)).set_text(cx, if self.show_opening_volumes { "Объёмы проёмов: видны" } else { "Объёмы проёмов: скрыты" });
        self.render_opening_page(cx);
    }

    fn opening_geometry_status(&self, id: ElementId) -> String {
        let Some(status) = self.geometry_cache.element_status(id) else { return "ещё не построена".to_owned(); };
        match status.status {
            ai_revit_geometry::GeometryStatusKind::Valid => "valid · approximate_mesh".to_owned(),
            ai_revit_geometry::GeometryStatusKind::Invalid => status.failure.as_ref().map_or_else(|| "invalid".to_owned(), |error|format!("invalid: {error}")),
            ai_revit_geometry::GeometryStatusKind::ResolutionFailed => format!("resolution_failed: {}", status.resolution_error.as_deref().unwrap_or("ошибка параметров")),
        }
    }

    fn render_opening_page(&mut self, cx: &mut Cx) {
        let pages = ((self.opening_ui_rows.len()+OPENING_UI_PAGE_SIZE-1)/OPENING_UI_PAGE_SIZE).max(1);
        self.opening_ui_page = self.opening_ui_page.min(pages-1);
        for (index, row_id) in OPENING_UI_ROW_IDS.iter().copied().enumerate() {
            let row = self.opening_ui_rows.get(self.opening_ui_page*OPENING_UI_PAGE_SIZE+index);
            let widget = self.ui.widget(cx, &[row_id]);
            widget.set_visible(cx, row.is_some());
            if let Some(row) = row { widget.set_text(cx, &row.label); }
            self.ui.button(cx, &[row_id]).set_enabled(cx, row.is_some());
        }
        self.ui.label(cx, ids!(opening_page_label)).set_text(cx, &format!("{} / {pages}",self.opening_ui_page+1));
        self.ui.widget(cx, ids!(opening_pager)).set_visible(cx, pages > 1);
        self.ui.button(cx, ids!(opening_previous)).set_enabled(cx, self.opening_ui_page > 0);
        self.ui.button(cx, ids!(opening_next)).set_enabled(cx, self.opening_ui_page+1 < pages);
    }

    fn create_hosted_from_ui(&mut self, cx: &mut Cx, category: ElementCategory) -> Result<Value, String> {
        let host_id = self.selected_element.ok_or("Выберите стену или проём")?;
        let document = self.document.as_ref().ok_or("no document is open")?;
        let host = document.effective_element(host_id).map_err(|e|e.to_string())?;
        let allocation = document.allocation_preview();
        let new_id = allocation.element_id;
        let width = self.meter_input(cx, ids!(opening_width), "Ширина проёма / изделия", true)?;
        let height = self.meter_input(cx, ids!(opening_height), "Высота проёма / изделия", true)?;
        let raw_offset = self.ui.text_input(cx, ids!(opening_offset)).text();
        let offset = if raw_offset.trim().is_empty() { None } else { Some(self.meter_input(cx, ids!(opening_offset), "Смещение", false)?) };
        let sill = self.meter_input(cx, ids!(opening_sill), "Нижняя граница", false)?;
        let spec = match (&host.kind, category) {
            (ElementKind::Wall(wall), ElementCategory::Opening) => {
                let dx = wall.end.x-wall.start.x;
                let dy = wall.end.y-wall.start.y;
                let length = dx.hypot(dy);
                if !length.is_finite() || length <= 0.0 { return Err("Стена имеет недопустимую длину".to_owned()); }
                let center = offset.unwrap_or(length/2.0);
                OpeningSpec { center: Point2::new(wall.start.x+dx/length*center, wall.start.y+dy/length*center), elevation_m:wall.base_elevation_m+sill, width_m:width, height_m:height, depth_m:wall.thickness_m, rotation_rad:dy.atan2(dx) }
            }
            (ElementKind::Opening(opening), ElementCategory::Window | ElementCategory::Door) => {
                let offset = offset.unwrap_or(0.0);
                let (sin, cos) = opening.rotation_rad.sin_cos();
                OpeningSpec { center:Point2::new(opening.center.x+cos*offset, opening.center.y+sin*offset), elevation_m:opening.elevation_m+sill, width_m:width, height_m:height, depth_m:opening.depth_m, rotation_rad:opening.rotation_rad }
            }
            (ElementKind::PolygonalOpening(opening),ElementCategory::Window | ElementCategory::Door) => {
                let offset = offset.unwrap_or(0.0);
                let (sin,cos) = opening.rotation_rad.sin_cos();
                OpeningSpec {center:Point2::new(opening.center.x+cos*offset,opening.center.y+sin*offset),elevation_m:opening.elevation_m+sill,width_m:width,height_m:height,depth_m:opening.depth_m,rotation_rad:opening.rotation_rad}
            }
            _ => return Err("Проём добавляется в стену, окно или дверь — в выбранный проём".to_owned()),
        };
        let mut commands = if category == ElementCategory::Opening {
            vec![Command::CreateOpening { name:format!("Проём #{}",new_id.get()), spec, properties:BTreeMap::new(), host_id, type_id:None }]
        } else {
            let (class_id, kind, name) = if category == ElementCategory::Window {
                (ai_revit_core::BUILTIN_WINDOW_CLASS_ID, ElementKind::Window(spec), "Окно")
            } else {
                (ai_revit_core::BUILTIN_DOOR_CLASS_ID, ElementKind::Door(spec), "Дверь")
            };
            vec![Command::CreateElement { name:format!("{name} #{}",new_id.get()), class_id:ClassId::new(class_id), kind, properties:BTreeMap::new() }, Command::HostElement { host_id, element_id:new_id }]
        };
        let level = document.element_level(host_id).or_else(|| ui_host_of(document,host_id).and_then(|host|document.element_level(host)));
        if let Some(level_id) = level { commands.push(Command::AssignLevel { element_id:new_id, level_id }); }
        let command_id = self.next_command_id("add-hosted-element");
        let receipt = {
            let document = self.document.as_mut().ok_or("no document is open")?;
            let mut transaction = document.begin_transaction(command_id, allocation.revision);
            for command in commands { transaction.push(command); }
            document.commit(transaction).map_err(|e|e.to_string())?
        };
        let created_id = receipt.created_elements.first().copied().ok_or("creation receipt contains no element")?;
        let mut result = serde_json::to_value(receipt).map_err(|e|e.to_string())?;
        self.track_element_selection(created_id);
        self.showing_objects = true;
        self.refresh_scene(cx, false);
        result["created_element_id"] = json!(created_id.get());
        result["geometry_rebuild"] = self.rebuild_observation();
        Ok(result)
    }

    /// Semantic field editing for the same native creation form. Validate the
    /// complete request before changing any field; document mutation stays in commit.
    pub(crate) fn configure_opening_inputs(&self, cx: &mut Cx, params: &Value) -> Result<Value, String> {
        let mut fields = Vec::new();
        for (key,path,positive) in [("width_m",ids!(opening_width),true),("height_m",ids!(opening_height),true),("offset_m",ids!(opening_offset),false),("bottom_offset_m",ids!(opening_sill),false)] {
            if let Some(value) = params.get(key) {
                let text = value.as_str().map(str::to_owned).or_else(||value.as_f64().map(|v|v.to_string())).ok_or_else(||format!("{key} must be a number or numeric text"))?;
                if key != "offset_m" || !text.trim().is_empty() {
                    let number = text.trim().replace(',', ".").parse::<f64>().map_err(|_|format!("{key} must be numeric"))?;
                    if !number.is_finite() || (positive && number <= 0.0) { return Err(format!("{key} must be finite{}",if positive { " and positive" } else { "" })); }
                }
                fields.push((path,text));
            }
        }
        for (path,text) in fields { self.ui.text_input(cx,path).set_text(cx,&text); }
        Ok(json!({"configured":true,"document_mutated":false,"ui":self.semantic_ui_state(cx)}))
    }

    fn refresh_semantic_inspector(&mut self, cx: &mut Cx) {
        let Some(document) = self.document.as_ref() else {
            self.semantic_type_options.clear();
            self.semantic_relationship_rows.clear();
            self.semantic_relationship_page = 0;
            self.ui.label(cx, ids!(instance_summary)).set_text(cx, "Элемент не выбран.");
            self.ui.label(cx, ids!(type_summary)).set_text(cx, "Без типа");
            self.ui.label(cx, ids!(level_summary)).set_text(cx, "Без уровня");
            self.ui.label(cx, ids!(resolved_summary)).set_text(cx, "Нет вычисленных свойств.");
            let dropdown = self.ui.drop_down(cx, ids!(type_selector));
            dropdown.set_labels(cx, vec!["Нет типов".to_owned()]);
            dropdown.set_selected_item(cx, 0);
            self.ui.button(cx, ids!(assign_type)).set_enabled(cx, false);
            self.ui.widget(cx, ids!(relationships_empty)).set_visible(cx, true);
            self.render_semantic_relationship_page(cx);
            return;
        };

        let selected = self.selected_element
            .and_then(|id| document.element(id).map(|element| (id, element)));
        let current_type_id = selected.and_then(|(_, element)| element.type_id);
        let focused_type_id = match self.semantic_selection {
            Some(EntityRef::Type(id)) => Some(id),
            _ => current_type_id,
        };
        let instance_text = selected.map_or_else(
            || "Элемент не выбран.".to_owned(),
            |(id, element)| {
                let overrides = element.properties.iter()
                    .map(|(property, value)| format!("{property}: {}", format_property_value(value)))
                    .collect::<Vec<_>>();
                format!(
                    "{} · #{}\nКатегория: {}\nПереопределения:\n{}",
                    element.name,
                    id.get(),
                    category_label(element_kind_category(&element.kind)),
                    if overrides.is_empty() { "нет".to_owned() } else { overrides.join("\n") },
                )
            },
        );
        let type_text = focused_type_id.map_or_else(
            || "Без типа".to_owned(),
            |type_id| document.element_type(type_id).map_or_else(
                || format!("Тип #{} не найден", type_id.get()),
                |element_type| {
                    let properties = element_type.properties.iter()
                        .map(|(property, value)| format!("{property}: {}", format_property_value(value)))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!(
                        "{} · #{}\nЭкземпляров: {}{}",
                        element_type.name,
                        type_id.get(),
                        document.type_instances(type_id).len(),
                        if properties.is_empty() { String::new() } else { format!("\n{properties}") },
                    )
                },
            ),
        );
        let level_text = match (self.semantic_selection, selected) {
            (Some(EntityRef::Level(id)), _) => document.level(id).map_or_else(
                || format!("Уровень #{} не найден", id.get()),
                |level| format!("{} · #{} · отметка {:.2} м", level.name, id.get(), level.elevation_m),
            ),
            (_, Some((element_id, _))) => document.element_level(element_id).map_or_else(
                || "Без уровня".to_owned(),
                |id| document.level(id).map_or_else(
                    || format!("Уровень #{} не найден", id.get()),
                    |level| format!("{} · #{} · отметка {:.2} м", level.name, id.get(), level.elevation_m),
                ),
            ),
            _ => "Без уровня".to_owned(),
        };
        let resolved_text = selected.map_or_else(
            || "Нет выбранного экземпляра.".to_owned(),
            |(id, _)| document.resolved_properties(id).map_or_else(
                |error| format!("Не удалось вычислить свойства: {error}"),
                |properties| {
                    if properties.is_empty() {
                        "Нет вычисленных свойств.".to_owned()
                    } else {
                        properties.values().map(|property| {
                            let source = match property.source {
                                PropertySource::InstanceOverride => "экземпляр",
                                PropertySource::Type => "тип",
                                PropertySource::ClassDefault => "класс",
                            };
                            format!("{}: {} · {}", property.property, format_property_value(&property.value), source)
                        }).collect::<Vec<_>>().join("\n")
                    }
                },
            ),
        );

        let type_options: Vec<(TypeId, String)> = document.types()
            .map(|element_type| (element_type.id, element_type.name.clone()))
            .collect();
        self.semantic_type_options = type_options.iter().map(|(id, _)| *id).collect();
        let mut dropdown_labels: Vec<String> = type_options.iter()
            .map(|(id, name)| format!("{name} · #{}", id.get()))
            .collect();
        if dropdown_labels.is_empty() {
            dropdown_labels.push("Нет типов".to_owned());
        }
        let selected_type_index = focused_type_id
            .and_then(|id| self.semantic_type_options.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let dropdown = self.ui.drop_down(cx, ids!(type_selector));
        dropdown.set_labels(cx, dropdown_labels);
        dropdown.set_selected_item(cx, selected_type_index);

        let relationship_rows = selected.map_or_else(Vec::new, |(element_id, _)| {
            document.element_relationships(element_id).into_iter().filter_map(|relationship| {
                let (role, target) = relationship_target_for_element(relationship, element_id)?;
                Some(SemanticRelationshipRow {
                    control_id: format!("relationships.click_{}", relationship.id.get()),
                    relationship_id: relationship.id,
                    source_element: element_id,
                    target,
                    label: format!(
                        "{} · {} · связь #{}",
                        role,
                        semantic_entity_label(document, target),
                        relationship.id.get(),
                    ),
                })
            }).collect()
        });
        self.semantic_relationship_rows = relationship_rows;
        let relationship_page_count = (self.semantic_relationship_rows.len()
            + SEMANTIC_RELATIONSHIP_PAGE_SIZE - 1) / SEMANTIC_RELATIONSHIP_PAGE_SIZE;
        self.semantic_relationship_page = self.semantic_relationship_page
            .min(relationship_page_count.saturating_sub(1));
        self.ui.label(cx, ids!(instance_summary)).set_text(cx, &instance_text);
        self.ui.label(cx, ids!(type_summary)).set_text(cx, &type_text);
        self.ui.label(cx, ids!(level_summary)).set_text(cx, &level_text);
        self.ui.label(cx, ids!(resolved_summary)).set_text(cx, &resolved_text);
        self.ui.widget(cx, ids!(relationships_empty)).set_visible(cx, self.semantic_relationship_rows.is_empty());
        let selected_type = selected.and_then(|(_, element)| element.type_id);
        let candidate_type = self.semantic_type_options.get(
            self.ui.drop_down(cx, ids!(type_selector)).selected_item(),
        ).copied();
        self.ui.button(cx, ids!(assign_type)).set_enabled(
            cx,
            selected.is_some() && candidate_type.is_some() && candidate_type != selected_type,
        );
        self.render_semantic_relationship_page(cx);
    }

    fn render_semantic_relationship_page(&self, cx: &mut Cx) {
        let page_count = (self.semantic_relationship_rows.len()
            + SEMANTIC_RELATIONSHIP_PAGE_SIZE - 1) / SEMANTIC_RELATIONSHIP_PAGE_SIZE;
        let page_count = page_count.max(1);
        let start = self.semantic_relationship_page * SEMANTIC_RELATIONSHIP_PAGE_SIZE;
        for (index, row_id) in SEMANTIC_RELATION_ROW_IDS.iter().copied().enumerate() {
            let widget = self.ui.widget(cx, &[row_id]);
            if let Some(row) = self.semantic_relationship_rows.get(start + index) {
                widget.set_text(cx, &row.label);
                widget.set_visible(cx, true);
                self.ui.button(cx, &[row_id]).set_enabled(cx, true);
            } else {
                widget.set_visible(cx, false);
                self.ui.button(cx, &[row_id]).set_enabled(cx, false);
            }
        }
        self.ui.label(cx, ids!(relationships_page_label)).set_text(cx, &format!(
            "{}/{}", self.semantic_relationship_page + 1, page_count,
        ));
        self.ui.button(cx, ids!(relationships_previous)).set_enabled(cx, self.semantic_relationship_page > 0);
        self.ui.button(cx, ids!(relationships_next)).set_enabled(cx, self.semantic_relationship_page + 1 < page_count);
        self.ui.widget(cx, ids!(relationship_pager)).set_visible(cx, self.semantic_relationship_rows.len() > SEMANTIC_RELATIONSHIP_PAGE_SIZE);
    }

    pub(crate) fn refresh_object_tree(&mut self, cx: &mut Cx) {
        self.semantic_tree_rows = self.document.as_ref()
            .map(|document| build_semantic_tree_rows(document))
            .unwrap_or_default();
        let page_count = (self.semantic_tree_rows.len() + SEMANTIC_TREE_PAGE_SIZE - 1) / SEMANTIC_TREE_PAGE_SIZE;
        let page_count = page_count.max(1);
        self.semantic_tree_page = self.semantic_tree_page.min(page_count - 1);
        let start = self.semantic_tree_page * SEMANTIC_TREE_PAGE_SIZE;
        self.semantic_tree_visible_rows = self.semantic_tree_rows.iter()
            .skip(start).take(SEMANTIC_TREE_PAGE_SIZE).cloned().collect();
        for (index, row_id) in SEMANTIC_TREE_ROW_IDS.iter().copied().enumerate() {
            let widget = self.ui.widget(cx, &[row_id]);
            if let Some(row) = self.semantic_tree_visible_rows.get(index) {
                let marker = if row.entity == self.semantic_selection { "▶ " } else { "  " };
                widget.set_text(cx, &format!("{marker}{}", row.label));
                widget.set_visible(cx, true);
                self.ui.button(cx, &[row_id]).set_enabled(cx, row.entity.is_some());
            } else {
                widget.set_visible(cx, false);
                self.ui.button(cx, &[row_id]).set_enabled(cx, false);
            }
        }
        self.ui.widget(cx, ids!(tree_empty_state)).set_visible(cx, self.semantic_tree_rows.is_empty());
        self.ui.label(cx, ids!(tree_page_label)).set_text(cx, &format!(
            "{}/{}", self.semantic_tree_page + 1, page_count,
        ));
        self.ui.button(cx, ids!(tree_previous)).set_enabled(cx, self.semantic_tree_page > 0);
        self.ui.button(cx, ids!(tree_next)).set_enabled(cx, self.semantic_tree_page + 1 < page_count);
        self.ui.widget(cx, ids!(tree_pager)).set_visible(cx, self.semantic_tree_rows.len() > SEMANTIC_TREE_PAGE_SIZE);
        self.ui.widget(cx, ids!(drawings_tree)).set_visible(cx, self.showing_objects);
        self.ui.label(cx, ids!(history_heading)).set_text(
            cx,
            if self.showing_objects { "Объекты проекта" } else { "История построения" },
        );
        self.ui.button(cx, ids!(objects_tab)).set_text(
            cx,
            if self.showing_objects { "● Объекты" } else { "Объекты" },
        );
        self.ui.button(cx, ids!(history_tab)).set_text(
            cx,
            if self.showing_objects { "История" } else { "● История" },
        );
        self.ui.widget(cx, ids!(semantic_tree_scroll)).set_visible(cx, self.showing_objects);
        self.ui.widget(cx, ids!(history_hint)).set_visible(cx, !self.showing_objects);
        self.ui.widget(cx, ids!(history_scroll)).set_visible(cx, !self.showing_objects);
        self.ui.widget(cx, ids!(history_pager)).set_visible(cx, !self.showing_objects);
    }

    fn select_semantic_entity(&mut self, cx: &mut Cx, entity: EntityRef) -> Result<Value, String> {
        let exists = self.document.as_ref().is_some_and(|document| match entity {
            EntityRef::Element(id) => document.element(id).is_some(),
            EntityRef::Type(id) => document.element_type(id).is_some(),
            EntityRef::Level(id) => document.level(id).is_some(),
        });
        if !exists {
            return Err(format!("semantic target no longer exists: {}", semantic_entity_label(
                self.document.as_ref().ok_or("no document is open")?,
                entity,
            )));
        }
        match entity {
            EntityRef::Element(id) => self.select_element(cx, id),
            EntityRef::Type(_) | EntityRef::Level(_) => {
                self.selected_element = None;
                self.semantic_selection = Some(entity);
                self.selected_operation = None;
                self.sync_viewport_selection(cx);
                self.refresh_inspector(cx);
                self.refresh_history(cx);
                self.refresh_object_tree(cx);
                self.set_status(cx, &format!("Выбран {}", semantic_entity_label(
                    self.document.as_ref().expect("document checked above"),
                    entity,
                )));
            }
        }
        Ok(semantic_entity_value(entity))
    }

    fn activate_tree_control(&mut self, cx: &mut Cx, control_id: &str) -> Result<Value, String> {
        let row = self.semantic_tree_visible_rows.iter()
            .find(|row| row.control_id == control_id)
            .ok_or_else(|| format!("tree control is not visible: {control_id}"))?;
        if let Some(raw) = control_id.strip_prefix("object_tree.level.") {
            let level_id = LevelId::new(parse_semantic_id(raw, "level id")?);
            if row.entity != Some(EntityRef::Level(level_id)) {
                return Err(format!("stale level tree control: {control_id}"));
            }
        }
        let entity = row.entity
            .ok_or_else(|| format!("tree control is not selectable: {control_id}"))?;
        self.select_semantic_entity(cx, entity)
    }

    fn activate_relationship_control(&mut self, cx: &mut Cx, relationship_id: RelationshipId) -> Result<Value, String> {
        let row = self.semantic_relationship_rows.iter()
            .skip(self.semantic_relationship_page * SEMANTIC_RELATIONSHIP_PAGE_SIZE)
            .take(SEMANTIC_RELATIONSHIP_PAGE_SIZE)
            .find(|row| row.relationship_id == relationship_id)
            .cloned()
            .ok_or_else(|| format!("relationship control is not visible: {}", relationship_id.get()))?;
        if self.selected_element != Some(row.source_element) {
            return Err("relationship row is stale; select the element again".to_owned());
        }
        let relationship = self.document.as_ref()
            .and_then(|document| document.relationship(relationship_id))
            .filter(|relationship| relationship.is_active())
            .ok_or_else(|| format!("relationship {} no longer exists", relationship_id.get()))?;
        let (role, target) = relationship_target_for_element(relationship, row.source_element)
            .ok_or_else(|| format!("relationship {} no longer targets the selected element", relationship_id.get()))?;
        if target != row.target {
            return Err(format!("relationship {} changed; refresh the inspector", relationship_id.get()));
        }
        let result = self.select_semantic_entity(cx, target)?;
        self.set_status(cx, &format!("{} · {}", role, semantic_entity_label(
            self.document.as_ref().expect("document checked above"),
            target,
        )));
        Ok(result)
    }

    fn update_type_apply_enabled(&self, cx: &mut Cx) {
        let selected_type = self.selected_element.and_then(|element_id| self.document.as_ref()
            .and_then(|document| document.element(element_id))
            .and_then(|element| element.type_id));
        let candidate = self.semantic_type_options.get(
            self.ui.drop_down(cx, ids!(type_selector)).selected_item(),
        ).copied();
        self.ui.button(cx, ids!(assign_type)).set_enabled(
            cx,
            self.selected_element.is_some() && candidate.is_some() && candidate != selected_type,
        );
    }

    fn assign_semantic_type(&mut self, cx: &mut Cx, type_id: TypeId) -> Result<Value, String> {
        let element_id = self.selected_element.ok_or("select an element before assigning a type")?;
        if !self.semantic_type_options.contains(&type_id) {
            return Err(format!("type {} is not an available selector option", type_id.get()));
        }
        let current_type = self.document.as_ref()
            .and_then(|document| document.element(element_id))
            .ok_or_else(|| format!("element {} no longer exists", element_id.get()))?
            .type_id;
        if current_type == Some(type_id) {
            let revision = self.document.as_ref().expect("document checked above").revision();
            return Ok(json!({
                "no_change": true,
                "element_id": element_id.get(),
                "type_id": type_id.get(),
                "revision": revision,
            }));
        }
        let command_id = self.next_command_id("assign-type");
        let receipt = {
            let document = self.document.as_mut().ok_or("no document is open")?;
            let revision = document.revision();
            let mut transaction = document.begin_transaction(command_id, revision);
            transaction.push(Command::AssignType { element_id, type_id });
            document.commit(transaction).map_err(|error| error.to_string())?
        };
        let result = serde_json::to_value(receipt).map_err(|error| error.to_string())?;
        self.semantic_selection = Some(EntityRef::Element(element_id));
        self.select_latest_operation();
        self.refresh_scene(cx, false);
        Ok(result)
    }

    fn change_semantic_tree_page(&mut self, cx: &mut Cx, forward: bool) {
        let page_count = (self.semantic_tree_rows.len() + SEMANTIC_TREE_PAGE_SIZE - 1) / SEMANTIC_TREE_PAGE_SIZE;
        if forward && self.semantic_tree_page + 1 < page_count {
            self.semantic_tree_page += 1;
        } else if !forward {
            self.semantic_tree_page = self.semantic_tree_page.saturating_sub(1);
        }
        self.refresh_object_tree(cx);
    }

    fn change_semantic_relationship_page(&mut self, cx: &mut Cx, forward: bool) {
        let page_count = (self.semantic_relationship_rows.len() + SEMANTIC_RELATIONSHIP_PAGE_SIZE - 1)
            / SEMANTIC_RELATIONSHIP_PAGE_SIZE;
        if forward && self.semantic_relationship_page + 1 < page_count {
            self.semantic_relationship_page += 1;
        } else if !forward {
            self.semantic_relationship_page = self.semantic_relationship_page.saturating_sub(1);
        }
        self.render_semantic_relationship_page(cx);
    }

    pub(crate) fn semantic_ui_state(&self, cx: &Cx) -> Value {
        let opening_rows = self.opening_ui_rows.iter().skip(self.opening_ui_page*OPENING_UI_PAGE_SIZE)
            .take(OPENING_UI_PAGE_SIZE).enumerate().map(|(index,row)| {
                let widget = self.ui.widget(cx, &[OPENING_UI_ROW_IDS[index]]);
                json!({"control_id":format!("openings.element.{}",row.element_id.get()),"element_id":row.element_id.get(),"opening_id":row.opening_id.map(ElementId::get),"role":row.role,"label":widget.text(),"visible":widget.visible(),"target":semantic_entity_value(EntityRef::Element(row.element_id))})
            }).collect::<Vec<_>>();
        let tree_rows: Vec<Value> = self.semantic_tree_visible_rows.iter().enumerate().map(|(index, row)| {
            let widget = self.ui.widget(cx, &[SEMANTIC_TREE_ROW_IDS[index]]);
            json!({
                "control_id": row.control_id,
                "kind": row.row_kind,
                "label": widget.text(),
                "visible": widget.visible(),
                "enabled": row.entity.is_some(),
                "target": row.entity.map(semantic_entity_value),
            })
        }).collect();
        let relationship_rows: Vec<Value> = self.semantic_relationship_rows.iter()
            .skip(self.semantic_relationship_page * SEMANTIC_RELATIONSHIP_PAGE_SIZE)
            .take(SEMANTIC_RELATIONSHIP_PAGE_SIZE)
            .enumerate().map(|(index, row)| {
                let widget = self.ui.widget(cx, &[SEMANTIC_RELATION_ROW_IDS[index]]);
                json!({
                    "control_id": row.control_id,
                    "relationship_id": row.relationship_id.get(),
                    "label": widget.text(),
                    "visible": widget.visible(),
                    "target": semantic_entity_value(row.target),
                })
            }).collect();
        let document = self.document.as_ref();
        let current_type_id = self.selected_element.and_then(|element_id| document
            .and_then(|document| document.element(element_id))
            .and_then(|element| element.type_id));
        let selected_type_index = self.ui.drop_down(cx, ids!(type_selector)).selected_item();
        let candidate_type_id = self.semantic_type_options.get(selected_type_index).map(|id| id.get());
        let viewer_selected_ids: Vec<u64> = self.viewport_widget(cx).borrow::<CadViewport>()
            .map(|viewport| viewport.viewer().scene().selected_ids().to_vec())
            .unwrap_or_default();
        let types: Vec<Value> = self.semantic_type_options.iter().map(|id| {
            let name = document.and_then(|document| document.element_type(*id))
                .map(|element_type| element_type.name.as_str()).unwrap_or("не найден");
            json!({
                "control_id": format!("type_selector.{}", id.get()),
                "type_id": id.get(),
                "name": name,
                "selected_in_document": current_type_id == Some(*id),
            })
        }).collect();
        json!({
            "navigator": {
                "active_tab": if self.showing_objects { "objects" } else { "history" },
                "heading": self.ui.label(cx, ids!(history_heading)).text(),
                "tree_page": self.ui.label(cx, ids!(tree_page_label)).text(),
                "tree_total_rows": self.semantic_tree_rows.len(),
                "tree_rows": tree_rows,
            },
            "inspector": {
                "selection_label": self.ui.label(cx, ids!(selected_label)).text(),
                "instance": self.ui.label(cx, ids!(instance_summary)).text(),
                "type": self.ui.label(cx, ids!(type_summary)).text(),
                "level": self.ui.label(cx, ids!(level_summary)).text(),
                "resolved": self.ui.label(cx, ids!(resolved_summary)).text(),
                "relationships_page": self.ui.label(cx, ids!(relationships_page_label)).text(),
            },
            "quantities": {
                "details":self.quantity_ui_details,
                "quality_label":self.ui.label(cx,ids!(quantities_quality)).text(),
                "gross_volume_label":self.ui.label(cx,ids!(quantities_gross_volume)).text(),
                "net_volume_label":self.ui.label(cx,ids!(quantities_net_volume)).text(),
                "opening_area_label":self.ui.label(cx,ids!(quantities_opening_area)).text(),
                "surface_area_label":self.ui.label(cx,ids!(quantities_surface_area)).text(),
                "status_label":self.ui.label(cx,ids!(quantities_status)).text(),
                "inspector_control_id":"inspector.quantities",
            },
            "property_editing": {
                "ui_mutation_context":self.property_editor.context,
                "dirty":self.property_editor.dirty,
                "stale":self.property_editor.stale(self.document.as_ref(),self.api_document_generation),
                "makepad_key_focus": !self.ui.text_input(cx,ids!(instance_override_value)).area().is_empty() && cx.has_key_focus(self.ui.text_input(cx,ids!(instance_override_value)).area()),
                "read_only": self.ui.text_input(cx,ids!(instance_override_value)).is_read_only(),
                "owner":self.property_ui_owner.map(semantic_entity_value),
                "element_id":self.property_ui_element.map(ElementId::get),
                "type_id":self.property_ui_type.map(TypeId::get),
                "property_id":self.property_ui_property,
                "options":self.property_ui_options.iter().map(|option|json!({
                    "control_id":format!("properties.select.{}",option.property_id),
                    "property_id":option.property_id,"label":option.label,"value_type":option.value_type,
                    "resolved_value":option.resolved_value,"source":option.resolved_source,
                    "instance_override":option.instance_override,"type_value":option.type_value,
                })).collect::<Vec<_>>(),
                "resolved_source_label":self.ui.label(cx,ids!(editable_property_source)).text(),
                "instance_value":self.ui.text_input(cx,ids!(instance_override_value)).text(),
                "type_value":self.ui.text_input(cx,ids!(type_property_value)).text(),
                "apply_instance":{"control_id":"properties.instance.apply","enabled":self.property_ui_element.is_some() && self.property_ui_property.is_some()},
                "clear_override":{"control_id":"properties.instance.clear_override","enabled":self.property_ui_options.iter().any(|option|Some(&option.property_id)==self.property_ui_property.as_ref() && option.instance_override.is_some())},
                "apply_type":{"control_id":"properties.type.apply","enabled":self.property_ui_type.is_some() && self.property_ui_property.is_some()},
                "inspector_control_id":"inspector.property_editing",
            },
            "geotech_circular": self.circular_observation(),
            "custom_property_editing": {
                "ui_mutation_context":self.custom_property_editor.context,
                "dirty":self.custom_property_editor.dirty,
                "stale":self.custom_property_editor.stale(self.document.as_ref(),self.api_document_generation),
                "name":self.ui.text_input(cx,ids!(property_name)).text(),
                "value":self.ui.text_input(cx,ids!(property_value)).text(),
            },
            "status":self.ui.label(cx,ids!(status)).text(),
            "type_selector": {
                "label": self.ui.widget(cx, ids!(type_selector)).text(),
                "selected_item": selected_type_index,
                "selected_type_id": candidate_type_id,
                "current_type_id": current_type_id.map(|id| id.get()),
                "options": types,
                "apply_control_id": "type_selector.apply",
            },
            "relationships": {
                "total_rows": self.semantic_relationship_rows.len(),
                "rows": relationship_rows,
            },
            "openings": {
                "opening_count":self.opening_ui_count,
                "total_rows":self.opening_ui_rows.len(),
                "page":self.opening_ui_page+1,
                "previous_control_id":"openings.page.previous",
                "next_control_id":"openings.page.next",
                "rows":opening_rows,
                "details":self.opening_ui_details,
                "summary":self.ui.label(cx,ids!(opening_summary)).text(),
                "show_opening_volumes":self.show_opening_volumes,
                "inputs":{"width_m":self.ui.text_input(cx,ids!(opening_width)).text(),"height_m":self.ui.text_input(cx,ids!(opening_height)).text(),"offset_m":self.ui.text_input(cx,ids!(opening_offset)).text(),"bottom_offset_m":self.ui.text_input(cx,ids!(opening_sill)).text(),"hint":self.ui.label(cx,ids!(opening_input_hint)).text()},
                "create_opening":{"control_id":"openings.add_opening","enabled":self.opening_ui_details.get("kind").and_then(Value::as_str)==Some("wall")},
                "create_window":{"control_id":"openings.add_window","enabled":matches!(self.opening_ui_details.get("kind").and_then(Value::as_str),Some("opening"|"polygonal_opening"))},
                "create_door":{"control_id":"openings.add_door","enabled":matches!(self.opening_ui_details.get("kind").and_then(Value::as_str),Some("opening"|"polygonal_opening"))},
                "volume_control_id":"openings.show_volumes",
                "wall_front_control_id":self.opening_ui_details.get("wall_id").and_then(Value::as_u64).map(|id|format!("viewport.wall_front.{id}")),
                "host_view_control_id":self.selected_element.filter(|id|document.and_then(|document|ui_geometry_host(document,*id)).is_some()).map(|id|format!("viewport.host_view.{}",id.get())),
            },
            "selection": {
                "entity": self.semantic_selection.map(semantic_entity_value),
                "element_id": self.selected_element.map(|id| id.get()),
                "operation_id": self.selected_operation.map(|id| id.get()),
                "status_label": self.ui.label(cx, ids!(selection_status)).text(),
                "viewer_selected_ids": viewer_selected_ids,
            },
        })
    }

    pub(crate) fn invoke_semantic_control(&mut self, cx: &mut Cx, control_id: &str) -> Result<Value, String> {
        let result = if let Some(property) = control_id.strip_prefix("properties.select.") {
            self.select_editable_property(cx,&PropertyId::new(property))?
        } else if control_id == "properties.instance.apply" {
            self.apply_property_editor(cx,"instance")?
        } else if control_id == "properties.instance.clear_override" {
            self.apply_property_editor(cx,"clear")?
        } else if control_id == "properties.type.apply" {
            self.apply_property_editor(cx,"type")?
        } else if control_id == "openings.add_opening" {
            self.create_hosted_from_ui(cx, ElementCategory::Opening)?
        } else if control_id == "openings.add_window" {
            self.create_hosted_from_ui(cx, ElementCategory::Window)?
        } else if control_id == "openings.add_door" {
            self.create_hosted_from_ui(cx, ElementCategory::Door)?
        } else if control_id == "openings.show_volumes" {
            self.set_show_opening_volumes(cx, !self.show_opening_volumes)?
        } else if control_id == "openings.page.previous" || control_id == "openings.page.next" {
            if control_id.ends_with("previous") { self.opening_ui_page = self.opening_ui_page.saturating_sub(1); }
            else { self.opening_ui_page = self.opening_ui_page.saturating_add(1); }
            self.render_opening_page(cx);
            json!({"page":self.opening_ui_page+1})
        } else if let Some(raw) = control_id.strip_prefix("openings.element.") {
            let id = ElementId::new(parse_semantic_id(raw,"opening list element id")?);
            if !self.opening_ui_rows.iter().skip(self.opening_ui_page*OPENING_UI_PAGE_SIZE).take(OPENING_UI_PAGE_SIZE).any(|row|row.element_id==id) { return Err("element is not on the visible opening-list page".to_owned()); }
            self.select_semantic_entity(cx, EntityRef::Element(id))?
        } else if let Some(raw) = control_id.strip_prefix("viewport.wall_front.") {
            self.set_wall_front_view(cx, ElementId::new(parse_semantic_id(raw,"wall id")?))?
        } else if let Some(raw) = control_id.strip_prefix("viewport.host_view.") {
            self.set_host_view(cx,ElementId::new(parse_semantic_id(raw,"host owner id")?))?
        } else if matches!(control_id,"inspector.top"|"inspector.relationships"|"inspector.openings"|"inspector.quantities"|"inspector.property_editing"|"inspector.custom_properties") {
            let section = control_id.strip_prefix("inspector.").expect("known inspector control");
            let y = if section == "top" { 0.0 } else {
                let first = self.ui.widget(cx,ids!(inspector_heading)).area();
                let target = self.ui.widget(cx,match section {
                    "openings" => ids!(openings_heading), "quantities" => ids!(quantities_heading),
                    "property_editing" => ids!(property_editing_heading), "custom_properties" => ids!(property_heading), _ => ids!(relationships_heading),
                }).area();
                if !first.is_valid(cx) || !target.is_valid(cx) { return Err("inspector layout is not ready for scrolling".to_owned()); }
                // Sibling headings share the scroll transform, so their relative
                // content distance remains valid at any current scroll position.
                let distance = target.rect(cx).pos.y-first.rect(cx).pos.y;
                if !distance.is_finite() { return Err("inspector anchor position is not finite".to_owned()); }
                distance.max(0.0)
            };
            self.ui.view(cx, ids!(inspector_scroll)).set_scroll_pos(cx, dvec2(0.0, y));
            self.ui.redraw(cx);
            json!({"section":section,"scroll_y":y})
        } else if control_id == "object_tree.page.previous" {
            self.change_semantic_tree_page(cx, false);
            json!({"page": self.semantic_tree_page + 1})
        } else if control_id == "object_tree.page.next" {
            self.change_semantic_tree_page(cx, true);
            json!({"page": self.semantic_tree_page + 1})
        } else if control_id == "relationships.page.previous" {
            self.change_semantic_relationship_page(cx, false);
            json!({"page": self.semantic_relationship_page + 1})
        } else if control_id == "relationships.page.next" {
            self.change_semantic_relationship_page(cx, true);
            json!({"page": self.semantic_relationship_page + 1})
        } else if control_id == "type_selector.apply" {
            let type_id = self.semantic_type_options.get(
                self.ui.drop_down(cx, ids!(type_selector)).selected_item(),
            ).copied().ok_or("the type selector has no available option")?;
            self.assign_semantic_type(cx, type_id)?
        } else if let Some(raw) = control_id.strip_prefix("type_selector.") {
            let id = TypeId::new(parse_semantic_id(raw, "type id")?);
            self.assign_semantic_type(cx, id)?
        } else if let Some(raw) = control_id.strip_prefix("relationships.click_") {
            let id = RelationshipId::new(parse_semantic_id(raw, "relationship id")?);
            self.activate_relationship_control(cx, id)?
        } else if control_id.starts_with("object_tree.") {
            self.activate_tree_control(cx, control_id)?
        } else {
            return Err(format!("unknown semantic control: {control_id}"));
        };
        if result.get("transaction_id").is_some() {
            let mut receipt = result;
            let object = receipt.as_object_mut().ok_or("core receipt must be an object")?;
            object.insert("control_id".to_owned(), json!(control_id));
            object.insert("ui".to_owned(), self.semantic_ui_state(cx));
            Ok(receipt)
        } else {
            Ok(json!({
                "control_id": control_id,
                "result": result,
                "ui": self.semantic_ui_state(cx),
            }))
        }
    }

    /// Resolve a semantic owner into the front view of its effective host wall.
    pub(crate) fn set_wall_front_view(&mut self, cx: &mut Cx, element_id: ElementId) -> Result<Value, String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        let wall_id = ui_wall_host(document, element_id).ok_or("element is not a wall or hosted by a wall")?;
        let effective = document.effective_element(wall_id).map_err(|e|e.to_string())?;
        let ElementKind::Wall(wall) = effective.kind else { return Err("host is not a wall".to_owned()); };
        let widget = self.viewport_widget(cx);
        let mut viewport = widget.borrow_mut::<CadViewport>().ok_or("viewport is not ready")?;
        viewport.set_wall_front_camera(cx, Vec3::new(wall.start.x,wall.start.y,wall.base_elevation_m), Vec3::new(wall.end.x,wall.end.y,wall.base_elevation_m), wall.height_m, wall.thickness_m)?;
        Ok(json!({"preset":"wall_front","owner_element_id":element_id.get(),"wall_id":wall_id.get(),"representation_quality":"approximate_mesh"}))
    }

    /// Fits the actual derived host mesh along its wall front or surface normal.
    pub(crate) fn set_host_view(&mut self,cx:&mut Cx,element_id:ElementId)->Result<Value,String> {
        let document = self.document.as_ref().ok_or("no document is open")?;
        let host_id = ui_geometry_host(document,element_id).ok_or("element has no Wall/Floor/Roof host")?;
        let effective = document.effective_element(host_id).map_err(|error|error.to_string())?;
        let (normal,up,host_kind) = match effective.kind {
            ElementKind::Wall(_) => return self.set_wall_front_view(cx,element_id),
            ElementKind::Floor(_) => (Vec3::Z,Vec3::Y,"floor"),
            ElementKind::Roof(spec) => (Vec3::new(-spec.slope_x,-spec.slope_y,1.0),Vec3::new(0.0,1.0,spec.slope_y),"roof"),
            _ => return Err("resolved host has no supported surface view".to_owned()),
        };
        let widget = self.viewport_widget(cx);
        let mut viewport = widget.borrow_mut::<CadViewport>().ok_or("viewport is not ready")?;
        viewport.set_host_plane_camera(cx,host_id.get(),normal,up)?;
        Ok(json!({"preset":"host_surface_normal","owner_element_id":element_id.get(),"host_id":host_id.get(),"host_kind":host_kind,"representation_quality":"approximate_mesh"}))
    }

    /// Fractions are within the effective rectangle, with v measured from bottom.
    pub(crate) fn opening_world_point(&self, opening_id: ElementId, u: f64, v: f64) -> Result<Vec3, String> {
        if !u.is_finite() || !v.is_finite() || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) { return Err("opening point fractions must be finite and in 0..1".to_owned()); }
        let document = self.document.as_ref().ok_or("no document is open")?;
        let effective = document.effective_element(opening_id).map_err(|e|e.to_string())?;
        let ElementKind::Opening(opening) = effective.kind else { return Err("aim owner must be an Opening".to_owned()); };
        let (sin,cos) = opening.rotation_rad.sin_cos();
        let along = (u-0.5)*opening.width_m;
        Ok(Vec3::new(opening.center.x+cos*along,opening.center.y+sin*along,opening.elevation_m+v*opening.height_m))
    }

    /// Selection and empty-hit clearing both run through the native action handler.
    pub(crate) fn pick_opening_point(&mut self, cx: &mut Cx, opening_id: ElementId, u: f64, v: f64) -> Result<Value, String> {
        let point = self.opening_world_point(opening_id,u,v)?;
        let widget = self.viewport_widget(cx);
        let mut picked = Err("viewport is not ready".to_owned());
        let actions = cx.capture_actions(|cx| {
            if let Some(mut viewport) = widget.borrow_mut::<CadViewport>() { picked = viewport.invoke_pick_world(cx,point); }
        });
        let hit = picked?;
        self.handle_actions(cx,&actions);
        if self.selected_element.map(ElementId::get) != hit { return Err("native viewport action did not synchronize selection".to_owned()); }
        Ok(json!({"opening_id":opening_id.get(),"fractions":{"u":u,"v":v},"world_point":[point.x,point.y,point.z],"picked_element_id":hit,"selection_path":"viewport_actions","selection":self.semantic_ui_state(cx)}))
    }

    fn initialize_document(&mut self, cx: &mut Cx) {
        let document = match Document::new("AI Revit") {
            Ok(document) => document,
            Err(error) => {
                self.set_status(cx, &format!("Не удалось создать документ: {error}"));
                return;
            }
        };
        self.document = Some(document);
        self.api_document_generation = self.api_document_generation.saturating_add(1);
        self.circular_after_mutation();
        self.geometry_cache.invalidate_all();
        self.showing_objects = true;
        self.semantic_tree_page = 0;
        self.semantic_relationship_page = 0;
        self.next_demo_wall = 0;
        self.next_demo_floor = 1;
        self.next_demo_roof = 1;
        self.history_page = 0;
        self.selected_element = None;
                    self.semantic_selection = None;
        self.selected_operation = None;
        match self.append_demo_model() {
            Ok(ids) => {
                self.active_wall_class = Some(ClassId::new("user.wall.exterior"));
                self.active_floor_class = None;
                self.active_roof_class = None;
                self.sync_document_defaults();
                self.set_status(cx, "Демонстрационная модель: 4 стены, перекрытие и кровля");
                self.refresh_scene(cx, true);
                if let Some(id) = ids.first().copied() {
                    self.select_element(cx, id);
                }
            }
            Err(error) => self.set_status(cx, &format!("Не удалось создать демонстрационную модель: {error}")),
        }
    }

    fn reconcile_active_classes(&mut self) {
        let Some(document) = self.document.as_ref() else {
            self.active_wall_class = None;
            self.active_floor_class = None;
            self.active_roof_class = None;
            return;
        };
        let wall = resolve_active_class(
            document, self.active_wall_class.as_ref(), ElementCategory::Wall, BUILTIN_WALL_CLASS_ID,
        );
        let floor = resolve_active_class(
            document, self.active_floor_class.as_ref(), ElementCategory::Floor, BUILTIN_FLOOR_CLASS_ID,
        );
        let roof = resolve_active_class(
            document, self.active_roof_class.as_ref(), ElementCategory::Roof, BUILTIN_ROOF_CLASS_ID,
        );
        self.active_wall_class = wall;
        self.active_floor_class = floor;
        self.active_roof_class = roof;
    }

    fn sync_document_defaults(&mut self) {
        self.reconcile_active_classes();
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let (mut walls, mut floors, mut roofs) = (0_u32, 0_u32, 0_u32);
        for element in document.elements() {
            match &element.kind {
                ElementKind::Wall(_) => walls = walls.saturating_add(1),
                ElementKind::Floor(_) => floors = floors.saturating_add(1),
                ElementKind::Roof(_) => roofs = roofs.saturating_add(1),
                ElementKind::Opening(_) | ElementKind::PolygonalOpening(_) | ElementKind::Door(_) | ElementKind::Window(_) | ElementKind::Space(_) => {}
            }
        }
        self.next_demo_wall = walls;
        self.next_demo_floor = floors;
        self.next_demo_roof = roofs;
    }

    fn meter_input(&self, cx: &Cx, id: &[LiveId], label: &str, positive: bool) -> Result<f64, String> {
        let raw = self.ui.text_input(cx, id).text();
        let normalized = raw.trim().replace(',', ".");
        let value = normalized.parse::<f64>().map_err(|_| {
            format!("Введите число в поле {label} (метры, десятичный разделитель — точка или запятая)")
        })?;
        if !value.is_finite() {
            return Err(format!("Поле {label} должно содержать конечное число"));
        }
        if positive && value <= 0.0 {
            return Err(format!("Значение {label} должно быть больше нуля"));
        }
        Ok(value)
    }

    fn append_demo_model(&mut self) -> Result<Vec<ElementId>, String> {
        let corners = [
            Point2 { x: 0.0, y: 0.0 },
            Point2 { x: 8.0, y: 0.0 },
            Point2 { x: 8.0, y: 6.0 },
            Point2 { x: 0.0, y: 6.0 },
        ];
        let class_id = ClassId::new("user.wall.exterior");
        let rating_id = PropertyId::new("fire_rating");
        let mut commands = vec![
            Command::DefineClass(ClassDef::new(
                class_id.clone(), "Наружная стена", ElementCategory::Wall,
            )),
            Command::DefineProperty {
                class_id: class_id.clone(),
                definition: PropertyDef::new(rating_id.clone(), "Огнестойкость", PropertyType::Text)
                    .with_default(PropertyValue::Text("60 мин".to_owned())),
            },
        ];
        for index in 0..4 {
            commands.push(Command::CreateElement {
                name: format!("Стена {}", index + 1),
                class_id: class_id.clone(),
                kind: ElementKind::Wall(WallSpec {
                    start: corners[index],
                    end: corners[(index + 1) % 4],
                    base_elevation_m: 0.0,
                    height_m: 3.0,
                    thickness_m: 0.20,
                }),
                properties: BTreeMap::from([(
                    rating_id.clone(), PropertyValue::Text("60 мин".to_owned()),
                )]),
            });
        }
        commands.push(Command::CreateElement {
            name: "Перекрытие 1 этажа".to_owned(),
            class_id: ClassId::new(BUILTIN_FLOOR_CLASS_ID),
            kind: ElementKind::Floor(FloorSpec {
                boundary: corners.to_vec(),
                holes: Vec::new(),
                elevation_m: 0.0,
                thickness_m: 0.25,
            }),
            properties: BTreeMap::new(),
        });
        commands.push(Command::CreateElement {
            name: "Кровля".to_owned(),
            class_id: ClassId::new(BUILTIN_ROOF_CLASS_ID),
            kind: ElementKind::Roof(RoofSpec {
                footprint: corners.to_vec(),
                elevation_m: 3.0,
                thickness_m: 0.18,
                slope_x: 0.0,
                slope_y: 0.15,
            }),
            properties: BTreeMap::new(),
        });
        self.commit_commands("Создать демонстрационную модель", commands)
    }

    fn add_wall(&mut self, cx: &mut Cx) {
        let parameters = (|| -> Result<WallSpec, String> {
            let x = self.meter_input(cx, ids!(geometry_x), "X", false)?;
            let y = self.meter_input(cx, ids!(geometry_y), "Y", false)?;
            let z = self.meter_input(cx, ids!(geometry_z), "Z", false)?;
            let length = self.meter_input(cx, ids!(geometry_sx), "Дл./Ш. (длина стены)", true)?;
            let height = self.meter_input(cx, ids!(geometry_height), "Высота", true)?;
            let thickness = self.meter_input(cx, ids!(geometry_thickness), "Толщина", true)?;
            let end_x = x + length;
            if !end_x.is_finite() {
                return Err("Координата конца стены выходит за допустимый диапазон".to_owned());
            }
            Ok(WallSpec {
                start: Point2 { x, y },
                end: Point2 { x: end_x, y },
                base_elevation_m: z,
                height_m: height,
                thickness_m: thickness,
            })
        })();
        let spec = match parameters {
            Ok(spec) => spec,
            Err(error) => { self.set_status(cx, &error); return; }
        };
        self.reconcile_active_classes();
        let Some(class_id) = self.active_wall_class.clone() else {
            self.set_status(cx, "В документе нет класса стен. Создайте класс стен перед добавлением элемента.");
            return;
        };
        let commands = vec![Command::CreateElement {
            name: format!("Стена {}", self.next_demo_wall + 1),
            class_id,
            kind: ElementKind::Wall(spec),
            properties: BTreeMap::new(),
        }];
        match self.commit_commands("Добавить стену", commands).map(|ids| ids[0]) {
            Ok(id) => {
                self.next_demo_wall += 1;
                self.track_element_selection(id);
                self.refresh_scene(cx, false);
            }
            Err(error) => self.set_status(cx, &format!("Не удалось добавить стену: {error}")),
        }
    }

    fn add_floor(&mut self, cx: &mut Cx) {
        let parameters = (|| -> Result<FloorSpec, String> {
            let x = self.meter_input(cx, ids!(geometry_x), "X", false)?;
            let y = self.meter_input(cx, ids!(geometry_y), "Y", false)?;
            let elevation = self.meter_input(cx, ids!(geometry_z), "Z", false)?;
            let width = self.meter_input(cx, ids!(geometry_sx), "Дл./Ш. (ширина)", true)?;
            let depth = self.meter_input(cx, ids!(geometry_sy), "Глубина", true)?;
            let thickness = self.meter_input(cx, ids!(geometry_thickness), "Толщина", true)?;
            let right = x + width;
            let top = y + depth;
            if !right.is_finite() || !top.is_finite() {
                return Err("Размер перекрытия выходит за допустимый диапазон координат".to_owned());
            }
            Ok(FloorSpec {
                boundary: vec![
                    Point2 { x, y },
                    Point2 { x: right, y },
                    Point2 { x: right, y: top },
                    Point2 { x, y: top },
                ],
                holes: Vec::new(),
                elevation_m: elevation,
                thickness_m: thickness,
            })
        })();
        let spec = match parameters {
            Ok(spec) => spec,
            Err(error) => { self.set_status(cx, &error); return; }
        };
        self.reconcile_active_classes();
        let Some(class_id) = self.active_floor_class.clone() else {
            self.set_status(cx, "В документе нет класса перекрытий. Создайте класс перекрытий перед добавлением элемента.");
            return;
        };
        let commands = vec![Command::CreateElement {
            name: format!("Перекрытие {} этажа", self.next_demo_floor + 1),
            class_id,
            kind: ElementKind::Floor(spec),
            properties: BTreeMap::new(),
        }];
        match self.commit_commands("Добавить перекрытие", commands).map(|ids| ids[0]) {
            Ok(id) => {
                self.next_demo_floor += 1;
                self.track_element_selection(id);
                self.refresh_scene(cx, false);
            }
            Err(error) => self.set_status(cx, &format!("Не удалось добавить перекрытие: {error}")),
        }
    }

    fn add_roof(&mut self, cx: &mut Cx) {
        let parameters = (|| -> Result<RoofSpec, String> {
            let x = self.meter_input(cx, ids!(geometry_x), "X", false)?;
            let y = self.meter_input(cx, ids!(geometry_y), "Y", false)?;
            let elevation = self.meter_input(cx, ids!(geometry_z), "Z", false)?;
            let width = self.meter_input(cx, ids!(geometry_sx), "Дл./Ш. (ширина)", true)?;
            let depth = self.meter_input(cx, ids!(geometry_sy), "Глубина", true)?;
            let thickness = self.meter_input(cx, ids!(geometry_thickness), "Толщина", true)?;
            let right = x + width;
            let top = y + depth;
            if !right.is_finite() || !top.is_finite() {
                return Err("Размер кровли выходит за допустимый диапазон координат".to_owned());
            }
            Ok(RoofSpec {
                footprint: vec![
                    Point2 { x, y },
                    Point2 { x: right, y },
                    Point2 { x: right, y: top },
                    Point2 { x, y: top },
                ],
                elevation_m: elevation,
                thickness_m: thickness,
                slope_x: 0.12,
                slope_y: 0.0,
            })
        })();
        let spec = match parameters {
            Ok(spec) => spec,
            Err(error) => { self.set_status(cx, &error); return; }
        };
        self.reconcile_active_classes();
        let Some(class_id) = self.active_roof_class.clone() else {
            self.set_status(cx, "В документе нет класса кровли. Создайте класс кровли перед добавлением элемента.");
            return;
        };
        let commands = vec![Command::CreateElement {
            name: format!("Кровля {}", self.next_demo_roof + 1),
            class_id,
            kind: ElementKind::Roof(spec),
            properties: BTreeMap::new(),
        }];
        match self.commit_commands("Добавить кровлю", commands).map(|ids| ids[0]) {
            Ok(id) => {
                self.next_demo_roof += 1;
                self.track_element_selection(id);
                self.refresh_scene(cx, false);
            }
            Err(error) => self.set_status(cx, &format!("Не удалось добавить кровлю: {error}")),
        }
    }

    fn click_history_row(&mut self, cx: &mut Cx, index: usize) {
        if let Some(row) = self.history_rows.get(index).copied() {
            self.select_operation(cx, row.operation_id);
        }
    }

    fn export_png(&mut self, cx: &mut Cx) {
        let mut path = PathBuf::from(self.path_text(cx));
        if !path.extension().and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png")) {
            path.set_extension("png");
        }
        if self.pending_gpu_png.is_some() {
            if let Some(mut viewport) = self.viewport_widget(cx).borrow_mut::<CadViewport>() {
                viewport.cancel_gpu_export(cx);
            }
        }
        self.pending_gpu_png = Some((path, std::time::Instant::now()));
        self.poll_gpu_png_export(cx);
    }

    fn poll_gpu_png_export(&mut self, cx: &mut Cx) {
        let Some((path, started)) = self.pending_gpu_png.as_ref() else { return; };
        let path = path.clone();
        if started.elapsed() > std::time::Duration::from_secs(15) {
            if let Some(mut viewport) = self.viewport_widget(cx).borrow_mut::<CadViewport>() {
                viewport.cancel_gpu_export(cx);
            }
            self.pending_gpu_png = None;
            self.set_status(cx, "Ошибка экспорта PNG: превышено время ожидания кадра GPU");
            return;
        }
        let result = self.viewport_widget(cx).borrow_mut::<CadViewport>()
            .ok_or_else(|| "3D viewport is not ready".to_owned())
            .and_then(|mut viewport| viewport.try_export_gpu_png(cx, &path));
        match result {
            Ok(_) => {
                self.pending_gpu_png = None;
                self.set_status(cx, &format!("Экспорт PNG с GPU завершён · {}", path.display()));
            }
            Err(error) if error.starts_with("GPU_FRAME_NOT_READY")
                || error.starts_with("GPU_READBACK_PENDING") => {
                self.set_status(cx, "Экспорт PNG: ожидание кадра GPU…");
            }
            Err(error) => {
                self.pending_gpu_png = None;
                if let Some(mut viewport) = self.viewport_widget(cx).borrow_mut::<CadViewport>() {
                    viewport.cancel_gpu_export(cx);
                }
                self.set_status(cx, &format!("Ошибка экспорта PNG: {error}"));
            }
        }
    }

    fn create_class(&mut self, cx: &mut Cx) {
        let class_name = self.ui.text_input(cx, ids!(class_input)).text().trim().to_owned();
        if class_name.is_empty() {
            self.set_status(cx, "Имя класса не может быть пустым");
            return;
        }
        let category = self.selected_element
            .and_then(|id| self.document.as_ref()?.element(id))
            .map_or(ElementCategory::Wall, |element| element_kind_category(&element.kind));
        let id_slug = stable_slug(&class_name);
        let category_id = match category {
            ElementCategory::Wall => "wall",
            ElementCategory::Floor => "floor",
            ElementCategory::Roof => "roof",
            ElementCategory::Opening => "opening",
            ElementCategory::Door => "door",
            ElementCategory::Window => "window",
            ElementCategory::Material => "material",
            ElementCategory::Space => "space",
            ElementCategory::Generic => "class",
        };
        let fallback_id = format!("custom_{}", self.next_command_sequence);
        let class_id = ClassId::new(format!("user.{category_id}.{}", if id_slug.is_empty() { &fallback_id } else { &id_slug }));
        let commands = vec![Command::DefineClass(ClassDef::new(class_id.clone(), class_name.clone(), category))];
        match self.commit_commands("Создать класс", commands) {
            Ok(_) => {
                match category {
                    ElementCategory::Wall => self.active_wall_class = Some(class_id),
                    ElementCategory::Floor => self.active_floor_class = Some(class_id),
                    ElementCategory::Roof => self.active_roof_class = Some(class_id),
                    ElementCategory::Opening
                    | ElementCategory::Door
                    | ElementCategory::Window
                    | ElementCategory::Material
                    | ElementCategory::Space
                    | ElementCategory::Generic => {}
                }
                self.selected_element = None;
                    self.semantic_selection = None;
                self.semantic_selection = None;
                self.select_latest_operation();
                self.sync_viewport_selection(cx);
                self.refresh_scene(cx, false);
                self.set_status(cx, "Класс создан и выбран для новых элементов этой категории");
            }
            Err(error) => self.set_status(cx, &format!("Не удалось создать класс: {error}")),
        }
    }

    fn apply_property(&mut self, cx: &mut Cx) {
        let Some(id) = self.selected_element else {
            self.set_status(cx, "Сначала выберите элемент");
            return;
        };
        let name = self.ui.text_input(cx, ids!(property_name)).text().trim().to_owned();
        let value = self.ui.text_input(cx, ids!(property_value)).text();
        if name.is_empty() {
            self.set_status(cx, "Имя свойства не может быть пустым");
            return;
        }
        let Some(element) = self.document.as_ref().and_then(|document| document.element(id)) else { return; };
        let class_id = element.class_id.clone();
        let target = EditorTarget::Custom { element_id: id, class_id: class_id.clone() };
        let context = match self.custom_property_editor.context.clone() {
            Some(context) => context,
            None => { self.set_status(cx, "stale_ui_intent: выберите BIM-элемент заново"); return; }
        };
        if let Err(error) = context.require_current(self.document.as_ref().expect("selected element exists"), self.api_document_generation, &target) {
            self.set_status(cx, &error);
            return;
        }
        let slug = stable_slug(&name);
        let fallback_id = format!("property_{}", self.next_command_sequence);
        let property_id = PropertyId::new(if slug.is_empty() { &fallback_id } else { &slug });
        let definition_exists = self.document.as_ref()
            .and_then(|document| document.property_definition(&class_id, &property_id)).is_some();
        let mut commands = Vec::new();
        if !definition_exists {
            commands.push(Command::DefineProperty {
                class_id: class_id.clone(),
                definition: PropertyDef::new(property_id.clone(), name, PropertyType::Text),
            });
        }
        commands.push(Command::SetProperty {
            element_id: id,
            property_id,
            value: PropertyValue::Text(value),
        });
        let command_id = self.next_command_id("custom-property");
        let result = commit_editor_commands(self.document.as_mut().expect("selected element exists"), self.api_document_generation, &context, &target, command_id, commands);
        match result {
            Ok(_) => {
                self.custom_property_editor.dirty = false;
                self.history_page = usize::MAX;
                self.select_latest_operation();
                self.refresh_scene(cx, false);
                self.set_status(cx, "Свойство записано в класс и элемент");
            }
            Err(error) => self.set_status(cx, &format!("Не удалось записать свойство: {error}")),
        }
    }
}

fn quantity_text(quantities:&Value,key:&str,unit:&str)->String {
    quantities.get(key).and_then(Value::as_f64).filter(|value|value.is_finite())
        .map_or_else(||"—".to_owned(),|value|format!("{value:.3} {unit}"))
}

fn property_input_text(value:&PropertyValue)->String {
    match value {
        PropertyValue::Text(value) => value.clone(), PropertyValue::Integer(value) => value.to_string(),
        PropertyValue::Boolean(value) => value.to_string(), PropertyValue::Number {value,..} => value.to_string(),
    }
}

fn parse_property_input(text:&str,value_type:PropertyType)->Result<PropertyValue,String> {
    let normalized = text.trim().replace(',',".");
    match value_type {
        PropertyType::Text => Ok(PropertyValue::Text(text.to_owned())),
        PropertyType::Integer => normalized.parse::<i64>().map(PropertyValue::Integer)
            .map_err(|_|"Введите целое число".to_owned()),
        PropertyType::Boolean => match normalized.to_lowercase().as_str() {
            "true"|"да"|"1" => Ok(PropertyValue::Boolean(true)),
            "false"|"нет"|"0" => Ok(PropertyValue::Boolean(false)),
            _ => Err("Введите true/false или да/нет".to_owned()),
        },
        PropertyType::Number {unit} => {
            let value = normalized.parse::<f64>().map_err(|_|"Введите число в указанных единицах".to_owned())?;
            if !value.is_finite() { return Err("Число должно быть конечным".to_owned()); }
            Ok(PropertyValue::Number {value,unit})
        },
    }
}

fn basic_property_label(key:&str,value_type:PropertyType)->String {
    let title = match key {
        "height" => "Высота", "thickness" => "Толщина", "width" => "Ширина", "depth" => "Глубина",
        "rotation" => "Поворот", "slope_x" => "Уклон X", "slope_y" => "Уклон Y", "material" => "Материал", _ => key,
    };
    match value_type {
        PropertyType::Number {unit:Unit::Meter} => format!("{title}, м"),
        PropertyType::Number {unit:Unit::Radian} => format!("{title}, рад"),
        _ => title.to_owned(),
    }
}

fn element_kind_category(kind: &ElementKind) -> ElementCategory {
    kind.category()
}
fn category_label(category: ElementCategory) -> &'static str {
    match category {
        ElementCategory::Generic => "Объекты",
        ElementCategory::Wall => "Стены",
        ElementCategory::Floor => "Перекрытия",
        ElementCategory::Roof => "Кровли",
        ElementCategory::Opening => "Проёмы",
        ElementCategory::Door => "Двери",
        ElementCategory::Window => "Окна",
        ElementCategory::Material => "Материалы",
        ElementCategory::Space => "Пространства",
    }
}

fn semantic_entity_value(entity: EntityRef) -> Value {
    match entity {
        EntityRef::Element(id) => json!({"kind": "element", "id": id.get()}),
        EntityRef::Type(id) => json!({"kind": "type", "id": id.get()}),
        EntityRef::Level(id) => json!({"kind": "level", "id": id.get()}),
    }
}

fn semantic_entity_label(document: &Document, entity: EntityRef) -> String {
    match entity {
        EntityRef::Element(id) => document.element(id).map_or_else(
            || format!("Элемент #{}", id.get()),
            |element| format!("{} · элемент #{}", element.name, id.get()),
        ),
        EntityRef::Type(id) => document.element_type(id).map_or_else(
            || format!("Тип #{}", id.get()),
            |element_type| format!("{} · тип #{}", element_type.name, id.get()),
        ),
        EntityRef::Level(id) => document.level(id).map_or_else(
            || format!("Уровень #{}", id.get()),
            |level| format!("{} · уровень #{}", level.name, id.get()),
        ),
    }
}

fn build_semantic_tree_rows(document: &Document) -> Vec<SemanticTreeRow> {
    let mut rows = vec![
        SemanticTreeRow {
            control_id: "object_tree.project".to_owned(),
            row_kind: "project",
            label: format!("Проект · {}", document.name()),
            entity: None,
        },
        SemanticTreeRow {
            control_id: "object_tree.levels".to_owned(),
            row_kind: "levels",
            label: "  Уровни".to_owned(),
            entity: None,
        },
    ];
    for level in document.levels() {
        rows.push(SemanticTreeRow {
            control_id: format!("object_tree.level.{}", level.id.get()),
            row_kind: "level",
            label: format!("    {} · {:.2} м", level.name, level.elevation_m),
            entity: Some(EntityRef::Level(level.id)),
        });
        let ids = document.level_elements(level.id);
        append_semantic_category_rows(
            document,
            &ids,
            &format!("level.{}", level.id.get()),
            3,
            &mut rows,
        );
    }
    let unassigned: Vec<ElementId> = document.elements()
        .filter(|element| document.element_level(element.id)
            .is_none_or(|level_id| document.level(level_id).is_none()))
        .map(|element| element.id)
        .collect();
    if !unassigned.is_empty() {
        rows.push(SemanticTreeRow {
            control_id: "object_tree.unassigned".to_owned(),
            row_kind: "unassigned",
            label: "  Без уровня".to_owned(),
            entity: None,
        });
        append_semantic_category_rows(
            document,
            &unassigned,
            "unassigned",
            2,
            &mut rows,
        );
    }
    rows
}

fn append_semantic_category_rows(
    document: &Document,
    element_ids: &[ElementId],
    parent: &str,
    depth: usize,
    rows: &mut Vec<SemanticTreeRow>,
) {
    let categories = [
        (ElementCategory::Generic, "generic"),
        (ElementCategory::Wall, "wall"),
        (ElementCategory::Floor, "floor"),
        (ElementCategory::Roof, "roof"),
        (ElementCategory::Opening, "opening"),
        (ElementCategory::Door, "door"),
        (ElementCategory::Window, "window"),
        (ElementCategory::Material, "material"),
        (ElementCategory::Space, "space"),
    ];
    for (category, key) in categories {
        let members: Vec<_> = element_ids.iter().filter_map(|id| {
            document.element(*id).filter(|element| element_kind_category(&element.kind) == category)
        }).collect();
        if members.is_empty() {
            continue;
        }
        rows.push(SemanticTreeRow {
            control_id: format!("object_tree.category.{parent}.{key}"),
            row_kind: "category",
            label: format!(
                "{}{} · {}",
                "  ".repeat(depth),
                category_label(category),
                members.len(),
            ),
            entity: None,
        });
        for element in members {
            rows.push(SemanticTreeRow {
                control_id: format!("object_tree.element.{}", element.id.get()),
                row_kind: "element",
                label: format!(
                    "{}{} · #{}",
                    "  ".repeat(depth + 1),
                    compact_history_text(&element.name, 36),
                    element.id.get(),
                ),
                entity: Some(EntityRef::Element(element.id)),
            });
        }
    }
}

fn relationship_target_for_element(
    relationship: &Relationship,
    element_id: ElementId,
) -> Option<(&'static str, EntityRef)> {
    let selected = EntityRef::Element(element_id);
    let (forward, target) = if relationship.source_id == selected {
        (true, relationship.target_id)
    } else if relationship.target_id == selected {
        (false, relationship.source_id)
    } else {
        return None;
    };
    let role = match (relationship.relationship_type, forward) {
        (RelationshipType::Hosts, true) => "Размещает",
        (RelationshipType::Hosts, false) => "Размещено в",
        (RelationshipType::HostedBy, true) => "Размещено в",
        (RelationshipType::HostedBy, false) => "Размещает",
        (RelationshipType::Contains, true) => "Содержит",
        (RelationshipType::Contains, false) => "Содержится в",
        (RelationshipType::ContainedIn, true) => "Содержится в",
        (RelationshipType::ContainedIn, false) => "Содержит",
        (RelationshipType::ConnectsTo, _) => "Соединено с",
        (RelationshipType::Supports, true) => "Поддерживает",
        (RelationshipType::Supports, false) => "Поддерживается",
        (RelationshipType::SupportedBy, true) => "Поддерживается",
        (RelationshipType::SupportedBy, false) => "Поддерживает",
        (RelationshipType::Bounds, true) => "Ограничивает",
        (RelationshipType::Bounds, false) => "Ограничено",
        (RelationshipType::References, true) => "Ссылается на",
        (RelationshipType::References, false) => "Упоминается в",
        (RelationshipType::InstanceOf, true) => "Экземпляр типа",
        (RelationshipType::InstanceOf, false) => "Имеет экземпляры",
        (RelationshipType::BelongsToLevel, true) => "Уровень",
        (RelationshipType::BelongsToLevel, false) => "Содержит элементы",
        (RelationshipType::BoundsSpace, true) => "Ограничивает пространство",
        (RelationshipType::BoundsSpace, false) => "Ограничено элементом",
        (RelationshipType::ContainedInSpace, true) => "В пространстве",
        (RelationshipType::ContainedInSpace, false) => "Содержит элементы пространства",
    };
    Some((role, target))
}

fn parse_semantic_id(value: &str, label: &str) -> Result<u64, String> {
    if value.is_empty() {
        return Err(format!("{label} is missing"));
    }
    value.parse::<u64>().map_err(|_| format!("invalid {label}: {value}"))
}
fn stable_slug(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().flat_map(char::to_lowercase) {
        let transliteration = match character {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d",
            'е' | 'ё' => "e", 'ж' => "zh", 'з' => "z", 'и' => "i", 'й' => "y",
            'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
            'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u",
            'ф' => "f", 'х' => "kh", 'ц' => "ts", 'ч' => "ch", 'ш' => "sh",
            'щ' => "shch", 'ъ' | 'ь' => "", 'ы' => "y", 'э' => "e", 'ю' => "yu",
            'я' => "ya",
            _ if character.is_ascii_alphanumeric() => {
                output.push(character);
                continue;
            }
            _ => "_",
        };
        output.push_str(transliteration);
    }
    output.split('_').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("_")
}

fn compact_history_text(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let text = characters.by_ref().take(max_chars.saturating_sub(1)).collect::<String>();
    if characters.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

fn resolve_active_class(
    document: &Document,
    preferred: Option<&ClassId>,
    category: ElementCategory,
    builtin_id: &str,
) -> Option<ClassId> {
    if let Some(class_id) = preferred.filter(|class_id| {
        document.class(class_id).is_some_and(|class| class.category == category)
    }) {
        return Some(class_id.clone());
    }
    let builtin = ClassId::new(builtin_id);
    if document.class(&builtin).is_some_and(|class| class.category == category) {
        return Some(builtin);
    }
    document.classes()
        .find(|class| class.category == category)
        .map(|class| class.id.clone())
}

// Core owns normalized and indexed hosting lookup; the UI only formats stable IDs.
fn ui_hosted_children(document: &Document, host: ElementId) -> Vec<ElementId> {
    document.hosted_element_ids(host)
}

fn ui_host_of(document: &Document, child: ElementId) -> Option<ElementId> {
    document.host_of(child)
}

fn host_role_label(document:&Document,id:ElementId)->&'static str {
    match document.element(id).map(|element|&element.kind) {
        Some(ElementKind::Wall(_)) => "Стена", Some(ElementKind::Floor(_)) => "Перекрытие",
        Some(ElementKind::Roof(_)) => "Кровля", _ => "Элемент",
    }
}

fn ui_geometry_host(document:&Document,id:ElementId)->Option<ElementId> {
    let mut current = id;
    for _ in 0..3 {
        if document.element(current).is_some_and(|element|matches!(&element.kind,ElementKind::Wall(_)|ElementKind::Floor(_)|ElementKind::Roof(_))) { return Some(current); }
        current = document.host_of(current)?;
    }
    None
}

fn ui_wall_host(document: &Document, id: ElementId) -> Option<ElementId> {
    let mut current = id;
    // v0.3 supports Wall → Opening → Window/Door; bound traversal even if an
    // invalid external document were to contain a cycle.
    for _ in 0..3 {
        if document.element(current).is_some_and(|e|matches!(&e.kind,ElementKind::Wall(_))) { return Some(current); }
        current = ui_host_of(document,current)?;
    }
    None
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if [ids!(instance_override_value), ids!(type_property_value)].into_iter()
            .any(|path| self.ui.text_input(cx,path).changed(actions).is_some()) {
            self.property_editor.dirty = true;
        }
        if [ids!(property_name), ids!(property_value)].into_iter()
            .any(|path| self.ui.text_input(cx,path).changed(actions).is_some()) {
            self.custom_property_editor.dirty = true;
        }
        self.handle_geo_foundation_actions(cx, actions);
        self.handle_structural_configurator_actions(cx, actions);
        self.handle_foundation_structural_actions(cx, actions);
        self.handle_circular_actions(cx, actions);
        self.handle_drafting_actions(cx, actions);
        self.handle_geotech_actions(cx, actions);
        if self.ui.button(cx,ids!(opening_host_view)).clicked(actions) {
            if let Some(id) = self.selected_element {
                if let Err(error) = self.invoke_semantic_control(cx,&format!("viewport.host_view.{}",id.get())) { self.set_status(cx,&error); }
            }
        }
        if let Some(index) = self.ui.drop_down(cx,ids!(editable_property_selector)).selected(actions) {
            if let Some(property_id) = self.property_ui_options.get(index).map(|option|option.property_id.clone()) {
                if let Err(error) = self.invoke_semantic_control(cx,&format!("properties.select.{property_id}")) { self.set_status(cx,&error); }
            }
        }
        for (path,control_id) in [
            (ids!(apply_instance_override),"properties.instance.apply"),
            (ids!(clear_instance_override),"properties.instance.clear_override"),
            (ids!(apply_type_property),"properties.type.apply"),
        ] {
            if self.ui.button(cx,path).clicked(actions) {
                if let Err(error) = self.invoke_semantic_control(cx,control_id) { self.set_status(cx,&error); }
            }
        }
        if self.ui.button(cx, ids!(new_document)).clicked(actions) {
            #[cfg(target_os = "linux")]
            let admitted = self.gui_persistence_begin(cx,linux_geotech_gui_wire::Target::NewDocument);
            #[cfg(not(target_os = "linux"))]
            let admitted: Result<(),String> = Ok(());
            match admitted {
                Ok(()) => {
                    self.initialize_document(cx);
                    #[cfg(target_os = "linux")]
                    self.gui_persistence_end(linux_geotech_gui_wire::Target::NewDocument,true);
                }
                Err(error) => self.set_status(cx,&error),
            }
        }
        if self.ui.button(cx, ids!(open_document)).clicked(actions) {
            #[cfg(target_os = "linux")]
            let admitted = self.gui_persistence_begin(cx,linux_geotech_gui_wire::Target::OpenDocument);
            #[cfg(not(target_os = "linux"))]
            let admitted: Result<(),String> = Ok(());
            match admitted {
                Ok(()) => {
                    let path = self.path_text(cx);
                    let result=self.api_document_open(cx,&serde_json::json!({"path":path}));
                    #[cfg(target_os = "linux")]
                    self.gui_persistence_end(linux_geotech_gui_wire::Target::OpenDocument,result.is_ok());
                    if let Err(error)=result {self.set_status(cx,&format!("Не удалось открыть файл: {error}"));}
                }
                Err(error) => self.set_status(cx,&error),
            }
        }
        if self.ui.button(cx, ids!(save_document)).clicked(actions) {
            #[cfg(target_os = "linux")]
            let admitted = self.gui_persistence_begin(cx,linux_geotech_gui_wire::Target::SaveDocument);
            #[cfg(not(target_os = "linux"))]
            let admitted: Result<(),String> = Ok(());
            match admitted {
                Ok(()) => {
                    let result=self.save_current_document(cx);
                    #[cfg(target_os = "linux")]
                    self.gui_persistence_end(linux_geotech_gui_wire::Target::SaveDocument,result.is_ok());
                }
                Err(error) => self.set_status(cx,&error),
            }
        }
        if self.ui.button(cx, ids!(undo_document)).clicked(actions) {
            let result = self.document.as_mut().ok_or_else(|| "no document is open".to_owned())
                .and_then(|document| document.undo().map_err(|error| error.to_string()));
            match result {
                Ok(Some(_)) => { self.selected_element = None;
                    self.semantic_selection = None; self.selected_operation = None; self.sync_document_defaults(); self.refresh_unified_after_commit(cx, false); self.set_status(cx, "Отмена выполнена"); }
                Ok(None) => self.set_status(cx, "Нет операций для отмены"),
                Err(error) => self.set_status(cx, &format!("Не удалось отменить операцию: {error}")),
            }
        }
        if self.ui.button(cx, ids!(redo_document)).clicked(actions) {
            let result = self.document.as_mut().ok_or_else(|| "no document is open".to_owned())
                .and_then(|document| document.redo().map_err(|error| error.to_string()));
            match result {
                Ok(Some(_)) => { self.selected_element = None;
                    self.semantic_selection = None; self.selected_operation = None; self.sync_document_defaults(); self.refresh_unified_after_commit(cx, false); self.set_status(cx, "Повтор выполнен"); }
                Ok(None) => self.set_status(cx, "Нет операций для повтора"),
                Err(error) => self.set_status(cx, &format!("Не удалось повторить операцию: {error}")),
            }
        }
        if self.ui.button(cx, ids!(add_wall)).clicked(actions) { self.add_wall(cx); }
        if self.ui.button(cx, ids!(add_floor)).clicked(actions) { self.add_floor(cx); }
        if self.ui.button(cx, ids!(add_roof)).clicked(actions) { self.add_roof(cx); }
        for (path,control) in [
            (ids!(add_opening), "openings.add_opening"),
            (ids!(add_window), "openings.add_window"),
            (ids!(add_door), "openings.add_door"),
            (ids!(show_opening_volumes), "openings.show_volumes"),
            (ids!(opening_previous), "openings.page.previous"),
            (ids!(opening_next), "openings.page.next"),
        ] {
            if self.ui.button(cx,path).clicked(actions) {
                if let Err(error) = self.invoke_semantic_control(cx,control) { self.set_status(cx,&error); }
            }
        }
        if self.ui.button(cx,ids!(opening_wall_front)).clicked(actions) {
            if let Some(id) = self.opening_ui_details.get("wall_id").and_then(Value::as_u64) {
                if let Err(error) = self.invoke_semantic_control(cx,&format!("viewport.wall_front.{id}")) { self.set_status(cx,&error); }
            }
        }
        for (index,row_id) in OPENING_UI_ROW_IDS.iter().copied().enumerate() {
            if self.ui.button(cx,&[row_id]).clicked(actions) {
                if let Some(id) = self.opening_ui_rows.get(self.opening_ui_page*OPENING_UI_PAGE_SIZE+index).map(|row|row.element_id) {
                    if let Err(error) = self.invoke_semantic_control(cx,&format!("openings.element.{}",id.get())) { self.set_status(cx,&error); }
                }
            }
        }
        if self.ui.button(cx, ids!(export_png)).clicked(actions) { self.export_png(cx); }
        if self.ui.button(cx, ids!(create_class)).clicked(actions) { self.create_class(cx); }
        if self.ui.button(cx, ids!(set_property)).clicked(actions) { self.apply_property(cx); }

        if self.ui.button(cx, ids!(objects_tab)).clicked(actions) {
            self.showing_objects = true;
            self.refresh_object_tree(cx);
        }
        if self.ui.button(cx, ids!(history_tab)).clicked(actions) {
            self.showing_objects = false;
            self.refresh_object_tree(cx);
        }
        if self.ui.button(cx, ids!(tree_previous)).clicked(actions) {
            self.change_semantic_tree_page(cx, false);
        }
        if self.ui.button(cx, ids!(tree_next)).clicked(actions) {
            self.change_semantic_tree_page(cx, true);
        }
        for (index, row_id) in SEMANTIC_TREE_ROW_IDS.iter().copied().enumerate() {
            if self.ui.widget(cx, &[row_id]).borrow::<Button>().is_some_and(|button| button.clicked(actions)) {
                if let Some(control_id) = self.semantic_tree_visible_rows.get(index).map(|row| row.control_id.clone()) {
                    if let Err(error) = self.activate_tree_control(cx, &control_id) {
                        self.set_status(cx, &format!("Не удалось выбрать объект: {error}"));
                    }
                }
            }
        }
        if self.ui.button(cx, ids!(relationships_previous)).clicked(actions) {
            self.change_semantic_relationship_page(cx, false);
        }
        if self.ui.button(cx, ids!(relationships_next)).clicked(actions) {
            self.change_semantic_relationship_page(cx, true);
        }
        for (index, row_id) in SEMANTIC_RELATION_ROW_IDS.iter().copied().enumerate() {
            if self.ui.widget(cx, &[row_id]).borrow::<Button>().is_some_and(|button| button.clicked(actions)) {
                let relationship_id = self.semantic_relationship_rows
                    .iter()
                    .skip(self.semantic_relationship_page * SEMANTIC_RELATIONSHIP_PAGE_SIZE)
                    .nth(index)
                    .map(|row| row.relationship_id);
                if let Some(id) = relationship_id {
                    if let Err(error) = self.activate_relationship_control(cx, id) {
                        self.set_status(cx, &format!("Не удалось перейти по связи: {error}"));
                    }
                }
            }
        }
        if self.ui.drop_down(cx, ids!(type_selector)).selected(actions).is_some() {
            self.update_type_apply_enabled(cx);
        }
        if self.ui.button(cx, ids!(assign_type)).clicked(actions) {
            let type_id = self.semantic_type_options.get(
                self.ui.drop_down(cx, ids!(type_selector)).selected_item(),
            ).copied();
            match type_id.map(|id| self.assign_semantic_type(cx, id)) {
                Some(Ok(_)) => {}
                Some(Err(error)) => self.set_status(cx, &format!("Не удалось назначить тип: {error}")),
                None => self.set_status(cx, "Выберите тип из списка"),
            }
        }

        if self.ui.button(cx, ids!(history_previous)).clicked(actions) && self.history_page > 0 {
            self.history_page -= 1;
            self.refresh_history(cx);
        }
        let operation_count = self.document.as_ref().map_or(0, |document| document.operation_tree().len());
        let page_count = ((operation_count + HISTORY_PAGE_SIZE - 1) / HISTORY_PAGE_SIZE).max(1);
        if self.ui.button(cx, ids!(history_next)).clicked(actions) && self.history_page + 1 < page_count {
            self.history_page += 1;
            self.refresh_history(cx);
        }
        if self.ui.button(cx, ids!(history_recent)).clicked(actions) && self.history_page + 1 < page_count {
            self.history_page = page_count - 1;
            self.refresh_history(cx);
        }

        for (index, row_id) in HISTORY_ROW_IDS.iter().copied().enumerate() {
            if self.ui.widget(cx, &[row_id]).borrow::<Button>().is_some_and(|button| button.clicked(actions)) {
                self.click_history_row(cx, index);
            }
        }
        let viewport_actions = self.ui.cad_viewport(cx, ids!(viewport)).actions(actions);
        let selection_changed = viewport_actions.iter().any(|action| matches!(
            action,
            CadViewportAction::Viewer(ViewerEvent::ObjectPicked { .. } | ViewerEvent::SelectionChanged { .. })
        ));
        if selection_changed {
            let selected = self.viewport_widget(cx)
                .borrow::<CadViewport>()
                .and_then(|viewport| viewport.viewer().scene().selected_ids().first().copied());
            let selected_element = selected.and_then(|object_id| {
                self.document.as_ref()?.element(ElementId::new(object_id)).map(|element| element.id)
            });
            if let Some(id) = selected_element {
                self.select_element(cx, id);
            } else {
                self.selected_element = None;
                    self.semantic_selection = None;
                self.selected_operation = None;
                self.refresh_inspector(cx);
                self.refresh_history(cx);
                self.refresh_object_tree(cx);
            }
        }
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.api_poll_timer.is_timer(event).is_some() {
            self.poll_gpu_png_export(cx);
            self.poll_agent_api(cx);
            if let Err(error) = self.circular_native_poll_jobs(cx) {
                self.set_status(cx, &error);
                #[cfg(target_os = "linux")]
                linux_lifecycle_wire::fail("LIFECYCLE_WORKER_POLL_FAILED");
            }
            self.circular_publish_report(cx);
            #[cfg(target_os = "linux")]
            self.lifecycle_tick(cx);
            #[cfg(target_os = "linux")]
            self.lifecycle_apply_pending_close(cx);
            self.circular_finish_deferred_close(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        viewport::script_mod(vm);
        drafting_canvas::script_mod(vm);
        geotech::script_mod(vm);
        geotech_circular_ui::script_mod(vm);
        foundation_structural_ui::script_mod(vm);
        structural_configurator_ui::script_mod(vm);
        geo_foundation_ui::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        #[cfg(target_os = "linux")]
        if !self.gui_before_event(cx, event) { return; }
        #[cfg(target_os = "linux")]
        if let Event::WindowClosed(closed) = event {
            self.lifecycle_observe_closed(closed.window_id);
        }
        let drafting_draw_started = std::time::Instant::now();
        let drafting_allocations = drafting_allocation_probe::snapshot();
        if let Event::WindowCloseRequested(request) = event {
            self.circular_observe_close_request(cx, request);
        }
        if let Event::Actions(actions) = event {
            if self.circular_defer_caption_close(cx, actions) {
                return;
            }
        }
        if matches!(event, Event::Startup) {
            #[cfg(windows)]
            eprintln!("{}", accessibility::STATUS);
            if let Some((luid, headroom)) = gpu::compute_device_budget(cx) {
                ai_revit_geometry::configure_gpu_compute(luid, headroom);
            }
        } else if matches!(event, Event::Actions(_) | Event::Timer(_)) {
            ai_revit_geometry::refresh_gpu_compute_budget(gpu::compute_device_budget(cx));
        }
        if matches!(event, Event::Shutdown) {
            self.circular_observe_shutdown();
            ai_revit_geometry::shutdown_gpu_compute();
        }
        self.match_event(cx, event);
        if matches!(event, Event::Draw(_)) { self.circular_before_draw(cx); }
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.after_drafting_event(cx, event, drafting_draw_started, drafting_allocations);
        self.foundation_structural_after_event(cx);
        self.structural_configurator_after_event(cx, event);
        self.geo_foundation_after_event(cx, event);
        #[cfg(target_os = "linux")]
        self.gui_after_event(cx, event);
        if matches!(event, Event::Startup) {
            if self.document.is_none() {
                self.initialize_document(cx);
            }
            let owner_admitted = geotech_circular_api::reviewed_circular_source();
            if let Err(error) = self.circular_native_set_gates(owner_admitted, owner_admitted) {
                self.set_status(cx, &error);
            }
            let graphics = gpu::observe(cx);
            let adapter = &graphics["actual_adapter"];
            let gpu_label = match (
                adapter["description"].as_str(),
                adapter["descriptor_capacity_bytes"]["dedicated_video"].as_u64(),
            ) {
                (Some(description), Some(capacity)) => format!(
                    "{} · VRAM {:.1} ГБ", description,
                    capacity as f64 / 1_073_741_824.0,
                ),
                _ => "GPU observation unavailable in the public SDK; adapter/VRAM not verified".into(),
            };
            self.ui.label(cx, ids!(gpu_status)).set_text(cx, &gpu_label);
            self.api_receiver = match agent_api::start_server() {
                Ok(receiver) => Some(receiver),
                Err(error) => {
                    eprintln!("AI Revit API unavailable: {error}");
                    #[cfg(target_os = "linux")]
                    linux_lifecycle_wire::fail("LIFECYCLE_API_BIND_OR_START_FAILED");
                    None
                }
            };
            // Also completes a one-click GPU export when the API is unavailable.
            self.api_poll_timer = cx.start_interval(0.05);
        }
        // Reuse the existing draw/API invalidation points, not a new polling loop.
    }
}
